//! Judging a reply before it is kept, and tidying the ones that are.
//!
//! A reply is no reply when it is empty, is a JSON document or tool-call syntax written as
//! speech, or is an endpoint's boilerplate: a refusal, an error text, the provider talking
//! about itself. [`Guard::judge`] says which; the caller treats it like a failed generation
//! (one more try, then a failed turn). The boilerplate rules come from the corpus
//! (`boilerplate.start`, `boilerplate.any`, `boilerplate.whole`: one rule per line, matched
//! without regard to case), with English ones as the fallback, so a corpus adds its own
//! language. [`tidy`] removes what a good reply still carries by habit: the speaker's own
//! name in front of a line, and the harness's message tags.

use crate::assemble;
use crate::text::Wording;

/// Characters of a reply held back while it could still turn out to be boilerplate: a
/// rule that opens a reply matches within this much.
pub const HOLD_CHARS: usize = 160;

/// Longest reply, in characters, in which an `any` rule counts. A longer reply is a real
/// answer that may mention a brand in passing.
const ANY_CHARS: usize = 240;

/// What a reply, or the start of one, is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Fine,
    /// Not yet known: more text could settle it.
    Wait,
    /// No reply, for this reason (a short word for the diagnostics log).
    Bad(&'static str),
}

#[derive(Debug, Clone, Default)]
pub struct Guard {
    start: Vec<String>,
    any: Vec<String>,
    whole: Vec<String>,
    /// Names of the tools the turn offers, which a reply must not write as a call.
    tools: Vec<String>,
}

fn rules(wording: &Wording, key: &str) -> Vec<String> {
    wording
        .get(key)
        .lines()
        .map(fold)
        .filter(|l| !l.is_empty())
        .collect()
}

/// Lower case, curly quotes straight, blanks collapsed.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars() {
        let c = match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            c => c,
        };
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

impl Guard {
    pub fn new(wording: &Wording, tools: impl IntoIterator<Item = String>) -> Self {
        Self {
            start: rules(wording, "boilerplate.start"),
            any: rules(wording, "boilerplate.any"),
            whole: rules(wording, "boilerplate.whole")
                .iter()
                .map(|r| core(r).to_owned())
                .collect(),
            tools: tools.into_iter().collect(),
        }
    }

    /// Judge `text`; `complete` says nothing more will come.
    pub fn judge(&self, text: &str, complete: bool) -> Verdict {
        let t = text.trim();
        if t.is_empty() || !t.chars().any(char::is_alphanumeric) {
            return if complete {
                Verdict::Bad("empty")
            } else {
                Verdict::Wait
            };
        }
        if let Some(v) = self.syntax(t, complete) {
            return v;
        }
        if self.boilerplate(t) {
            return Verdict::Bad("boilerplate");
        }
        if !complete && t.chars().count() < HOLD_CHARS {
            return Verdict::Wait;
        }
        Verdict::Fine
    }

    /// Tool-call syntax and JSON documents where speech should be.
    fn syntax(&self, t: &str, complete: bool) -> Option<Verdict> {
        if assemble::text_call(t).is_some() {
            return Some(Verdict::Bad("tool call"));
        }
        let opens = t.starts_with("```") || t.starts_with('{') || t.starts_with("[{");
        if opens {
            let body = unfenced(t);
            if matches!(
                serde_json::from_str::<serde_json::Value>(body),
                Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
            ) {
                return Some(Verdict::Bad("json"));
            }
            if !complete {
                return Some(Verdict::Wait);
            }
        }
        // The wrap-up document, whole or cut short.
        if t.contains("\"facts\"") && (opens || t.contains("\"summary\"") || t.contains("\"hurt\""))
        {
            return Some(Verdict::Bad("wrap-up json"));
        }
        let writes_call = self.tools.iter().any(|n| {
            t.contains(&format!("{n}("))
                || t.contains(&format!("\"name\": \"{n}\""))
                || t.contains(&format!("\"name\":\"{n}\""))
        });
        writes_call.then_some(Verdict::Bad("tool call"))
    }

    fn boilerplate(&self, t: &str) -> bool {
        let folded = fold(t);
        let whole = core(&folded);
        if self.whole.iter().any(|g| glob(g, whole)) {
            return true;
        }
        for line in lead_lines(t) {
            let line = fold(line);
            let line = line.trim_start_matches(|c: char| !c.is_alphanumeric());
            if self.start.iter().any(|r| line.starts_with(r.as_str())) {
                return true;
            }
        }
        folded.chars().count() <= ANY_CHARS && self.any.iter().any(|r| contains_phrase(&folded, r))
    }
}

/// Without the punctuation and quotes around it.
fn core(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric() && c != '*')
}

/// The reply's first line, and its first line that is not a scene line, each also
/// without a scene in brackets at its start.
fn lead_lines(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for l in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
        out.push(l);
        if let Some(rest) = after_scene(l) {
            out.push(rest);
        }
        if !is_scene(l) {
            break;
        }
    }
    out
}

/// What follows a bracketed scene at the start of a line.
fn after_scene(line: &str) -> Option<&str> {
    let close = if line.starts_with('\u{ff08}') {
        '\u{ff09}'
    } else if line.starts_with('(') {
        ')'
    } else {
        return None;
    };
    let (_, rest) = line.split_once(close)?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

fn is_scene(line: &str) -> bool {
    let open = line.starts_with(['\u{ff08}', '(', '*', '[']);
    let close = line.ends_with(['\u{ff09}', ')', '*', ']']);
    open && close
}

/// The text inside a code fence, else the text.
fn unfenced(t: &str) -> &str {
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.split_once('\n').map_or("", |(_, r)| r);
    rest.rsplit_once("```").map_or(rest, |(r, _)| r).trim()
}

/// `*` in the pattern stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut at = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text.starts_with(part) {
                return false;
            }
            at = part.len();
        } else if i == parts.len() - 1 {
            return text.len() >= at + part.len() && text.ends_with(part);
        } else {
            match text[at..].find(part) {
                Some(k) => at += k + part.len(),
                None => return false,
            }
        }
    }
    true
}

/// Whether `rule` occurs in `text`, as a whole word where it starts and ends with a
/// letter or digit of the Latin script (so "server" is not found inside "observer").
fn contains_phrase(text: &str, rule: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric();
    let first = rule.chars().next().is_some_and(word);
    let last = rule.chars().last().is_some_and(word);
    let mut from = 0;
    while let Some(i) = text[from..].find(rule) {
        let at = from + i;
        let end = at + rule.len();
        let before = text[..at].chars().next_back().is_some_and(word);
        let after = text[end..].chars().next().is_some_and(word);
        if !(first && before) && !(last && after) {
            return true;
        }
        from = at + rule.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Take the speaker's own name off the front of the lines and the harness's message tags
/// out of the text. A closing line (one that opens with a mark of `marks`) is left as it
/// is: the reply line's `#msg7` is its meaning. A line that held nothing but a tag goes.
pub fn tidy(text: &str, names: &[&str], marks: &[&str]) -> String {
    let marks: Vec<&str> = marks.iter().copied().filter(|m| !m.is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if marks.iter().any(|m| trimmed.starts_with(m)) {
            out.push(line.to_owned());
            continue;
        }
        let line = strip_name(line, names);
        let (clean, had) = strip_tags(&line);
        if had && clean.trim().is_empty() {
            continue;
        }
        out.push(clean);
    }
    out.join("\n")
}

/// [`tidy`]'s first half alone: the lines of `text` without the speaker's own name in front.
pub fn strip_names<S: AsRef<str>>(text: &str, names: &[S]) -> String {
    let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
    text.split('\n')
        .map(|l| strip_name(l, &names))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `name:` or `name\u{ff1a}` at the start of a line, or just inside the opening bracket
/// of a scene line, or behind emphasis marks.
fn strip_name(line: &str, names: &[&str]) -> String {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let lead_len = body
        .find(|c: char| !matches!(c, '\u{ff08}' | '(' | '\u{3010}' | '[' | '*' | '_' | ' '))
        .unwrap_or(body.len());
    let (lead, rest) = body.split_at(lead_len);
    for name in names.iter().filter(|n| !n.is_empty()) {
        let Some(after) = strip_prefix_ci(rest, name) else {
            continue;
        };
        let after = after.trim_start_matches(['*', '_']);
        let Some(after) = after.strip_prefix([':', '\u{ff1a}']) else {
            continue;
        };
        let lead = lead.trim_end_matches([' ', '*', '_']);
        let after = after.trim_start();
        // A name that stood alone in front of the colon leaves the bracket open.
        return format!("{indent}{lead}{after}");
    }
    line.to_owned()
}

/// `text` without `prefix` at its start, letters compared without regard to case.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let mut chars = text.char_indices();
    for want in prefix.chars() {
        let (_, got) = chars.next()?;
        if !got.to_lowercase().eq(want.to_lowercase()) {
            return None;
        }
    }
    Some(chars.next().map_or("", |(i, _)| &text[i..]))
}

/// `#msg12` and `[#msg12]`, wherever they stand. Returns the line and whether any went.
fn strip_tags(line: &str) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let mut had = false;
    while let Some(i) = rest.find("#msg") {
        let digits = rest[i + 4..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let before_ok = rest[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if digits == 0 || !before_ok {
            out.push_str(&rest[..i + 4]);
            rest = &rest[i + 4..];
            continue;
        }
        had = true;
        let mut head = &rest[..i];
        let mut tail = &rest[i + 4 + digits..];
        // The brackets of `[#msg12]` go with it.
        if head.ends_with('[') && tail.starts_with(']') {
            head = &head[..head.len() - 1];
            tail = &tail[1..];
        }
        out.push_str(head.trim_end_matches([' ', '\t']));
        rest = tail;
    }
    out.push_str(rest);
    (out.trim_end().to_owned(), had)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn guard(extra: &[(&str, &str)]) -> Guard {
        let map: BTreeMap<String, String> = extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Guard::new(&Wording::new(&map), ["find_people".to_owned()])
    }

    #[test]
    fn the_owners_canned_replies_are_boilerplate() {
        let g = guard(&[]);
        for canned in [
            "Sorry, something went wrong. Please try your request again.",
            "I encountered an error doing what you asked. Could you try again?",
            "I'm having a hard time fulfilling your request. Can I help you with something else instead?",
            "I seem to be encountering an error.",
            "As a large language model, I cannot do that.",
            "I'm Gemini, a large language model trained by Google.",
            "\u{2018}Server error\u{2019}",
        ] {
            assert_eq!(
                g.judge(canned, true),
                Verdict::Bad("boilerplate"),
                "{canned}"
            );
        }
    }

    #[test]
    fn a_person_talking_about_a_server_or_a_scholar_is_not() {
        let g = guard(&[]);
        for said in [
            "The server room is on the third floor, I will walk you there.",
            "A scholar of old texts once told me the same.",
            "(smiles) Just an observer of the servers, nothing more.",
        ] {
            assert_eq!(g.judge(said, true), Verdict::Fine, "{said}");
        }
    }

    #[test]
    fn the_corpus_adds_its_own_rules() {
        let g = guard(&[
            ("boilerplate.start", "I am an assistant"),
            ("boilerplate.any", "search engine\nscholar"),
            ("boilerplate.whole", "no reply*"),
        ]);
        assert_eq!(
            g.judge("(nods) I am an assistant here.", true),
            Verdict::Bad("boilerplate")
        );
        assert_eq!(
            g.judge("The scholar results say so.", true),
            Verdict::Bad("boilerplate")
        );
        assert_eq!(
            g.judge("No reply was produced", true),
            Verdict::Bad("boilerplate")
        );
        // A long answer may mention one in passing.
        let long = format!("{} scholar {}", "word ".repeat(40), "word ".repeat(40));
        assert_eq!(g.judge(&long, true), Verdict::Fine);
    }

    #[test]
    fn json_documents_tool_calls_and_nothing_are_not_replies() {
        let g = guard(&[]);
        assert_eq!(
            g.judge(r#"{"facts":[],"hurt":{},"summary":""}"#, true),
            Verdict::Bad("json")
        );
        assert_eq!(
            g.judge("```json\n{\"facts\":[]}\n```", true),
            Verdict::Bad("json")
        );
        assert_eq!(
            g.judge(
                r#"```tool_call {"name": "wrapup", "arguments": {"facts":[]}} ```"#,
                true
            ),
            Verdict::Bad("tool call")
        );
        assert_eq!(
            g.judge("Let me look: find_people(\"tea\")", true),
            Verdict::Bad("tool call")
        );
        assert_eq!(g.judge("  \n", true), Verdict::Bad("empty"));
        assert_eq!(g.judge("[] ...", true), Verdict::Bad("empty"));
        // Cut short, a document is still one.
        assert_eq!(
            g.judge(r#"{"facts":[{"text":"a"}],"summ"#, true),
            Verdict::Bad("wrap-up json")
        );
    }

    #[test]
    fn a_start_that_could_be_a_document_is_held_until_it_is_known() {
        let g = guard(&[]);
        assert_eq!(g.judge("{\"fa", false), Verdict::Wait);
        assert_eq!(g.judge("Hello there", false), Verdict::Wait);
        assert_eq!(g.judge(&"word ".repeat(40), false), Verdict::Fine);
        assert_eq!(g.judge("{not json at all", true), Verdict::Fine);
    }

    #[test]
    fn the_own_name_in_front_of_a_line_goes() {
        let names = ["Ann", "W"];
        assert_eq!(tidy("Ann: hello", &names, &[]), "hello");
        assert_eq!(tidy("Ann\u{ff1a}hello\nW: bye", &names, &[]), "hello\nbye");
        assert_eq!(tidy("**Ann**: hello", &names, &[]), "hello");
        assert_eq!(
            tidy("\u{ff08}Ann\u{ff1a}smiles\u{ff09}\nhello", &names, &[]),
            "\u{ff08}smiles\u{ff09}\nhello"
        );
        // Not at the start of a line, or another name, or no colon: left alone.
        assert_eq!(tidy("Says Ann: no", &names, &[]), "Says Ann: no");
        assert_eq!(tidy("Bob: hi", &names, &[]), "Bob: hi");
        assert_eq!(tidy("Annie: hi", &names, &[]), "Annie: hi");
        assert_eq!(tidy("Ann is here", &names, &[]), "Ann is here");
    }

    #[test]
    fn message_tags_go_but_the_reply_line_stays() {
        let marks = ["\u{21a9}"];
        assert_eq!(tidy("Fine.\n#msg217", &[], &marks), "Fine.");
        assert_eq!(tidy("Fine.\n[#msg217]\n", &[], &marks), "Fine.\n");
        assert_eq!(tidy("Fine. #msg71", &[], &marks), "Fine.");
        assert_eq!(tidy("See [#msg3] here", &[], &marks), "See here");
        assert_eq!(
            tidy("Back.\n\u{21a9} #msg1", &[], &marks),
            "Back.\n\u{21a9} #msg1"
        );
        assert_eq!(tidy("#msgs are tags", &[], &marks), "#msgs are tags");
    }
}
