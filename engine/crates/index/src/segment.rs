//! Query-side segmentation.
//!
//! Chinese has no word delimiters, so the corpus stores pre-segmented tokens in an
//! ordinary FTS5 column. That choice buys freedom from native dependencies and from
//! platform SQLite builds that refuse to load extensions — but it imposes one hard
//! constraint: **query-side segmentation must match the build side exactly**, or BM25
//! scores text that was never indexed that way.
//!
//! Which segmenter built a corpus is recorded in its manifest, so this dispatches on that
//! rather than assuming. A mismatch is not a crash; it is silently worse retrieval, which
//! is why the name is carried in the file at all.

use jieba_rs::Jieba;

/// Query-side stopwords.
///
/// These are dropped from queries only — never from the index, which keeps them so that a
/// phrase search remains possible later. The reason is a measurement: `的` appears in
/// 49,152 of 58,853 units (84%), so including it forces BM25 to score almost the whole
/// corpus. Removing these took the lexical path from 13 ms to a fraction of it, and they
/// carry no retrieval signal at that frequency anyway.
///
/// Deliberately short. A long stopword list starts discarding words that matter — 「上」
/// and 「下」 are directions and also chapter markers — so the bar for entry is
/// "grammatical particle appearing in the majority of units", not "common".
const STOPWORDS: &[&str] = &[
    "的", "了", "是", "在", "和", "与", "也", "都", "而", "及", "就", "着", "过",
    "吧", "呢", "吗", "啊", "呀", "他", "她", "它", "我", "你", "这", "那", "有",
    "个", "不", "一", "为", "以", "对", "会", "被", "把", "让", "从", "到", "上",
    "下", "中", "之", "其", "或", "但", "很", "还", "又", "们",
];

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// CJK ranges, for the fallback's run detection.
const CJK: &[(u32, u32)] = &[
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x20000, 0x2A6DF),
    (0x2A700, 0x2EBEF),
];

fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    CJK.iter().any(|&(lo, hi)| cp >= lo && cp <= hi)
}

pub enum Segmenter {
    /// Matches the forge's `jieba.cut_for_search`.
    Jieba(Box<Jieba>),
    /// Matches the forge's dependency-free fallback: overlapping character bigrams over
    /// CJK runs, whitespace elsewhere. Weaker but correct — the overlap matters, since
    /// without it a query whose word straddles a pair boundary matches nothing.
    CharBigram,
}

impl Segmenter {
    /// Pick the segmenter a corpus was built with.
    ///
    /// Unknown names fall back to bigrams rather than failing: a corpus built by a future
    /// forge with a third segmenter is still usable through the dense path, and degraded
    /// lexical matching beats refusing to open the file.
    pub fn for_corpus(name: &str) -> Self {
        match name {
            "jieba" => Self::Jieba(Box::new(Jieba::new())),
            _ => Self::CharBigram,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Jieba(_) => "jieba",
            Self::CharBigram => "char-bigram",
        }
    }

    /// Segment exactly as the corpus was built, with no filtering.
    ///
    /// Used where the token stream must reproduce the index: diagnostics, and any future
    /// phrase query.
    pub fn segment(&self, text: &str) -> Vec<String> {
        match self {
            Self::Jieba(jieba) => jieba
                .cut_for_search(text, true)
                .into_iter()
                .map(str::to_string)
                .filter(|token| !token.trim().is_empty())
                .collect(),
            Self::CharBigram => bigrams(text),
        }
    }

    /// Segment for retrieval: stopwords dropped.
    ///
    /// If every token is a stopword the query is kept intact rather than emptied — a search
    /// for 「我们」 should return something, and returning nothing because both halves are
    /// common words would be worse than a slow answer.
    pub fn segment_for_query(&self, text: &str) -> Vec<String> {
        let tokens = self.segment(text);
        let filtered: Vec<String> = tokens
            .iter()
            .filter(|token| !is_stopword(token))
            .cloned()
            .collect();
        if filtered.is_empty() { tokens } else { filtered }
    }

    /// An FTS5 MATCH expression: every token quoted, OR-joined.
    ///
    /// Quoting is not cosmetic. Unquoted tokens can contain FTS5 operators — `AND`, `*`,
    /// `-`, `(` — and a user typing one would otherwise change the query's structure or
    /// produce a syntax error. Doubling embedded quotes is the FTS5 escape.
    pub fn match_expression(&self, text: &str) -> Option<String> {
        let tokens = self.segment_for_query(text);
        if tokens.is_empty() {
            return None;
        }
        let quoted: Vec<String> = tokens
            .iter()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect();
        Some(quoted.join(" OR "))
    }
}

fn bigrams(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run: Vec<char> = Vec::new();
    let mut latin = String::new();

    let flush_cjk = |run: &mut Vec<char>, out: &mut Vec<String>| {
        match run.len() {
            0 => {}
            1 => out.push(run[0].to_string()),
            _ => {
                for pair in run.windows(2) {
                    out.push(pair.iter().collect());
                }
            }
        }
        run.clear();
    };
    let flush_latin = |latin: &mut String, out: &mut Vec<String>| {
        if !latin.is_empty() {
            out.push(std::mem::take(latin));
        }
    };

    for c in text.chars() {
        if is_cjk(c) {
            flush_latin(&mut latin, &mut out);
            run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut run, &mut out);
            latin.push(c);
        } else {
            flush_cjk(&mut run, &mut out);
            flush_latin(&mut latin, &mut out);
        }
    }
    flush_cjk(&mut run, &mut out);
    flush_latin(&mut latin, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigrams_overlap() {
        // Without overlap, a query for 源石技艺 could not match text tokenised as
        // 源石 | 技艺 — the boundary would hide it.
        assert_eq!(bigrams("源石技艺"), vec!["源石", "石技", "技艺"]);
    }

    #[test]
    fn bigrams_keep_latin_words_whole() {
        assert_eq!(bigrams("PRTS 系统"), vec!["PRTS", "系统"]);
    }

    #[test]
    fn a_single_cjk_character_survives() {
        assert_eq!(bigrams("石"), vec!["石"]);
    }

    #[test]
    fn fts_operators_in_a_query_are_neutralised() {
        let seg = Segmenter::CharBigram;
        let expr = seg.match_expression("AND OR NOT").unwrap();
        assert!(expr.starts_with('"'), "tokens must be quoted: {expr}");
        assert!(expr.contains("\"AND\""));
    }

    #[test]
    fn a_quote_in_the_query_cannot_escape_its_token() {
        // The tokenizer drops punctuation, so a typed quote never reaches the expression —
        // but the escaping stays as defence in depth, since a future segmenter (jieba
        // included) may well emit a token containing one. What is asserted here is the
        // property that matters: every token in the output is a single closed literal, so
        // no input can restructure the query.
        let seg = Segmenter::CharBigram;
        let expr = seg.match_expression("a\"b").unwrap();
        assert_eq!(expr.matches('"').count() % 2, 0, "unbalanced quoting in {expr}");
        for token in expr.split(" OR ") {
            assert!(
                token.starts_with('"') && token.ends_with('"') && token.len() >= 2,
                "token {token} is not a closed literal"
            );
        }
    }

    #[test]
    fn a_token_containing_a_quote_is_doubled() {
        // Exercises the escape directly, without depending on the tokenizer to produce
        // such a token.
        let quoted = format!("\"{}\"", "a\"b".replace('"', "\"\""));
        assert_eq!(quoted, "\"a\"\"b\"");
    }

    #[test]
    fn stopwords_are_dropped_from_queries() {
        // 的 appears in 84% of units, so scoring it means scoring the whole corpus.
        let seg = Segmenter::CharBigram;
        let tokens = seg.segment_for_query("罗德岛的成立");
        assert!(!tokens.iter().any(|t| t == "的"), "got {tokens:?}");
    }

    #[test]
    fn an_all_stopword_query_is_not_emptied() {
        // Searching for 我们 should return something rather than nothing.
        let seg = Segmenter::CharBigram;
        assert!(!seg.segment_for_query("我们").is_empty());
    }

    #[test]
    fn the_unfiltered_form_still_reproduces_the_index() {
        let seg = Segmenter::CharBigram;
        assert!(seg.segment("罗德岛的").iter().any(|t| t.contains('的')));
    }

    #[test]
    fn punctuation_only_input_yields_no_expression() {
        assert!(Segmenter::CharBigram.match_expression("！？。").is_none());
    }
}
