//! The inline marks of a reply: `[#id]` citations and whole-line scene descriptions.
//!
//! One grammar for the engine, the CLI and the tests. A citation is `[#` and one id, or
//! several ids separated by commas (each optionally prefixed by `#`, blanks allowed around
//! the commas), then `]`. An id is 1 to 128 characters without blanks, commas or
//! brackets. A bracket with any malformed item is plain text. The UI keeps a TypeScript
//! copy (`ui/src/lib/scene.ts`) that is tested with the same cases.

/// The longest id, in characters.
const ID_MAX: usize = 128;

/// The ids of the citation whose `[#` is at `start`, and the index just past its `]`.
fn cite_at(text: &str, start: usize) -> Option<(Vec<&str>, usize)> {
    let inside = &text[start + 2..];
    let end = inside.find(']')?;
    let inside = &inside[..end];
    if inside.starts_with(char::is_whitespace) {
        return None;
    }
    let ids = inside
        .split(',')
        .map(|item| {
            let id = item.trim().trim_start_matches('#');
            let ok = !id.is_empty()
                && id.chars().count() <= ID_MAX
                && !id.contains(|c: char| c.is_whitespace() || c == '[');
            ok.then_some(id)
        })
        .collect::<Option<Vec<_>>>()?;
    Some((ids, start + 2 + end + 1))
}

/// Every id cited in `text`, in order.
pub fn ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("[#") {
        let start = at + i;
        match cite_at(text, start) {
            Some((found, end)) => {
                out.extend(found.into_iter().map(str::to_owned));
                at = end;
            }
            None => at = start + 2,
        }
    }
    out
}

/// `text` without its citations, the blanks before a mark and at line ends removed. The
/// marks name sources; a reader or a voice does not get them as speech.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(i) = text[at..].find("[#") {
        let start = at + i;
        match cite_at(text, start) {
            Some((_, end)) => {
                out.push_str(text[at..start].trim_end_matches([' ', '\t']));
                at = end;
            }
            None => {
                out.push_str(&text[at..start + 2]);
                at = start + 2;
            }
        }
    }
    out.push_str(&text[at..]);
    out.split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a line holds citations and nothing else.
pub fn is_cite_line(line: &str) -> bool {
    !ids(line).is_empty() && strip(line).trim().is_empty()
}

/// The inside of a scene line, or `None` when the line is not one. A scene line is a whole
/// line wrapped in `open` and `close`; a doubled marker (emphasis) or an empty inside
/// is not a scene.
pub fn scene_inner<'a>(line: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let line = line.trim();
    if open.is_empty() || close.is_empty() || line.len() <= open.len() + close.len() {
        return None;
    }
    if !line.starts_with(open) || !line.ends_with(close) {
        return None;
    }
    let inner = &line[open.len()..line.len() - close.len()];
    if inner.starts_with(open) || inner.ends_with(close) || inner.trim().is_empty() {
        return None;
    }
    Some(inner.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_found_and_nonsense_skipped() {
        assert_eq!(
            ids("a [#u1] b [#m12] c [# x] [#] [#u1]"),
            ["u1", "m12", "u1"]
        );
    }

    #[test]
    fn lists_in_one_bracket_are_read() {
        assert_eq!(
            ids("a [#x:1,#y:2] b [#p:3, #q:4] c [#r:5, s:6] [#bad, ] [#z:7]"),
            ["x:1", "y:2", "p:3", "q:4", "r:5", "s:6", "z:7"]
        );
    }

    #[test]
    fn cites_are_removed_with_the_blank_before_them() {
        assert_eq!(strip("It works [#u2]."), "It works.");
        assert_eq!(strip("a [#u1, #u2] b [#x,y]"), "a b");
        assert_eq!(strip("two  \t[#a]\nnext [#b]  "), "two\nnext");
        assert_eq!(strip("Yes. [#voice:ab12] Go."), "Yes. Go.");
        assert_eq!(
            strip("Yes [#a:1,#b:2] and [#c:3, #d:4] go [#e:5, ]."),
            "Yes and go [#e:5, ]."
        );
        // Not citations: no hash, empty id, unclosed, too long, a blank inside an id.
        assert_eq!(
            strip("see [1] and [#] and [#abc"),
            "see [1] and [#] and [#abc"
        );
        let long = format!("x [#{}]", "a".repeat(129));
        assert_eq!(strip(&long), long);
        assert!(ids(&long).is_empty());
        assert_eq!(strip("[#a b]"), "[#a b]");
        assert_eq!(strip("[# not an id] ok"), "[# not an id] ok");
    }

    #[test]
    fn cite_lines_hold_nothing_else() {
        assert!(is_cite_line("  [#a] [#b, #c]  "));
        assert!(!is_cite_line("[#a] and more"));
        assert!(!is_cite_line("[#a] [open"));
        assert!(!is_cite_line("[# x]"));
        assert!(!is_cite_line(""));
    }

    #[test]
    fn scene_lines_are_whole_lines_only() {
        let s = |l| scene_inner(l, "(", ")");
        assert_eq!(s("(smiles)"), Some("smiles"));
        assert_eq!(s("  (smiles)  "), Some("smiles"));
        assert_eq!(s("(a) b"), None);
        assert_eq!(s("()"), None);
        assert_eq!(scene_inner("**bold**", "*", "*"), None);
        assert_eq!(scene_inner("(x)", "", ""), None);
    }
}
