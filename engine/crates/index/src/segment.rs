//! Query-side segmentation.
//!
//! The corpus stores pre-segmented tokens in an ordinary FTS5 column. That choice buys
//! freedom from native dependencies and from platform SQLite builds that refuse to load
//! extensions — but it imposes one hard constraint: **query-side segmentation must match
//! the build side exactly**, or BM25 scores text that was never indexed that way.
//!
//! Which segmenter built a corpus is recorded in its manifest, so this dispatches on that
//! rather than assuming. A mismatch is not a crash; it is silently worse retrieval, which
//! is why the name is carried in the file at all. The same holds for stopwords: the list is
//! supplied by the pack through the manifest, so the query side and the build side agree on
//! it and the engine bakes in no language of its own.

use std::collections::HashSet;

use jieba_rs::Jieba;

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

/// The tokenizer half of a segmenter.
pub enum Kind {
    /// Dictionary segmentation for scripts without word delimiters. Matches the forge's
    /// `jieba.cut_for_search`.
    Jieba(Box<Jieba>),
    /// Matches the forge's dependency-free fallback: overlapping character bigrams over
    /// CJK runs, whitespace elsewhere. Weaker but correct — the overlap matters, since
    /// without it a query whose word straddles a pair boundary matches nothing.
    CharBigram,
}

/// A tokenizer plus the query-side stopword set, both as declared by the corpus.
pub struct Segmenter {
    kind: Kind,
    /// Query-side stopwords, from the manifest's `stopwords` key.
    ///
    /// These are dropped from queries only — never from the index, which keeps them so that
    /// a phrase search remains possible later. The reason is a measurement: on a real corpus
    /// the most frequent particle occurred in the large majority of units, so including it
    /// forced BM25 to score almost the whole corpus. Dropping such tokens cut the lexical
    /// path by nearly an order of magnitude, and they carry no retrieval signal at that
    /// frequency anyway.
    ///
    /// The list should be deliberately short. A long one starts discarding words that
    /// matter — a common word can also be a direction, a name or a section marker — so the
    /// bar for entry is "grammatical particle appearing in the majority of units", not
    /// "common". The pack owns that judgement; empty means no stopwords.
    stopwords: HashSet<String>,
}

impl Segmenter {
    pub fn new(kind: Kind, stopwords: impl IntoIterator<Item = String>) -> Self {
        Self { kind, stopwords: stopwords.into_iter().collect() }
    }

    /// Pick the segmenter a corpus was built with, and its stopword list.
    ///
    /// Unknown names fall back to bigrams rather than failing: a corpus built by a future
    /// forge with a third segmenter is still usable through the dense path, and degraded
    /// lexical matching beats refusing to open the file.
    pub fn for_corpus(name: &str, stopwords: &[String]) -> Self {
        let kind = match name {
            "jieba" => Kind::Jieba(Box::new(Jieba::new())),
            _ => Kind::CharBigram,
        };
        Self::new(kind, stopwords.iter().cloned())
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Jieba(_) => "jieba",
            Kind::CharBigram => "char-bigram",
        }
    }

    pub fn is_stopword(&self, token: &str) -> bool {
        self.stopwords.contains(token)
    }

    /// Segment exactly as the corpus was built, with no filtering.
    ///
    /// Used where the token stream must reproduce the index: diagnostics, and any future
    /// phrase query.
    pub fn segment(&self, text: &str) -> Vec<String> {
        match &self.kind {
            Kind::Jieba(jieba) => jieba
                .cut_for_search(text, true)
                .into_iter()
                .map(str::to_string)
                .filter(|token| !token.trim().is_empty())
                .collect(),
            Kind::CharBigram => bigrams(text),
        }
    }

    /// Segment for retrieval: stopwords dropped.
    ///
    /// If every token is a stopword the query is kept intact rather than emptied — a search
    /// made only of common words should return something, and returning nothing would be
    /// worse than a slow answer.
    pub fn segment_for_query(&self, text: &str) -> Vec<String> {
        let tokens = self.segment(text);
        let filtered: Vec<String> = tokens
            .iter()
            .filter(|token| !self.is_stopword(token))
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

    fn bigram() -> Segmenter {
        Segmenter::new(Kind::CharBigram, Vec::new())
    }

    fn bigram_with(stopwords: &[&str]) -> Segmenter {
        Segmenter::new(Kind::CharBigram, stopwords.iter().map(|s| s.to_string()))
    }

    #[test]
    fn bigrams_overlap() {
        // Overlap is what lets a four-character query match text that was tokenised as
        // two separate words: without the middle bigram, the boundary between them
        // hides any term that straddles it. The characters are the ideographic
        // placeholders 甲乙丙丁 ("A B C D"), not corpus text.
        assert_eq!(bigrams("甲乙丙丁"), vec!["甲乙", "乙丙", "丙丁"]);
    }

    #[test]
    fn bigrams_keep_latin_words_whole() {
        assert_eq!(bigrams("ABC 甲乙"), vec!["ABC", "甲乙"]);
    }

    #[test]
    fn delimited_text_splits_on_whitespace_and_punctuation() {
        assert_eq!(bigrams("alpha beta, gamma"), vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn a_single_cjk_character_survives() {
        assert_eq!(bigrams("甲"), vec!["甲"]);
    }

    #[test]
    fn fts_operators_in_a_query_are_neutralised() {
        let expr = bigram().match_expression("AND OR NOT").unwrap();
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
        let expr = bigram().match_expression("a\"b").unwrap();
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
        // A particle appearing in the majority of units forces BM25 to score nearly the
        // whole corpus, which is why the query side drops them.
        let seg = bigram_with(&["the", "of"]);
        let tokens = seg.segment_for_query("the history of alpha");
        assert_eq!(tokens, vec!["history", "alpha"]);
    }

    #[test]
    fn stopwords_apply_to_single_character_cjk_tokens() {
        let seg = bigram_with(&["乙"]);
        // A lone character between delimiters is emitted as its own token, so it can match.
        let tokens = seg.segment_for_query("甲丙 乙 丁戊");
        assert!(!tokens.iter().any(|t| t == "乙"), "got {tokens:?}");
    }

    #[test]
    fn no_stopwords_means_nothing_is_dropped() {
        let seg = bigram();
        assert_eq!(seg.segment_for_query("the of"), seg.segment("the of"));
    }

    #[test]
    fn an_all_stopword_query_is_not_emptied() {
        // A query made entirely of stopwords should still return something rather than
        // nothing, so filtering falls back to the unfiltered form.
        let seg = bigram_with(&["the", "of"]);
        assert_eq!(seg.segment_for_query("of the"), vec!["of", "the"]);
    }

    #[test]
    fn the_unfiltered_form_still_reproduces_the_index() {
        let seg = bigram_with(&["the"]);
        assert!(seg.segment("alpha the").iter().any(|t| t == "the"));
    }

    #[test]
    fn corpus_dispatch_uses_the_manifest_name_and_list() {
        let seg = Segmenter::for_corpus("unknown-segmenter", &["x".to_string()]);
        assert_eq!(seg.name(), "char-bigram");
        assert!(seg.is_stopword("x"));
    }

    #[test]
    fn punctuation_only_input_yields_no_expression() {
        assert!(bigram().match_expression("!?.").is_none());
        assert!(bigram().match_expression("！？。").is_none());
    }
}
