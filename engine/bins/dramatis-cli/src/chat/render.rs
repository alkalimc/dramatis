//! How a reply is written to the terminal: scene lines set apart, citation marks hidden,
//! colour only when asked for. Pure text in, text out, so it is tested without a terminal.

use std::io::IsTerminal;

/// ANSI styling, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
}

impl Style {
    #[cfg(test)]
    pub const PLAIN: Style = Style { color: false };

    /// Colour when stdout is a terminal and `NO_COLOR` is unset or empty.
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Self {
            color: !no_color && std::io::stdout().is_terminal(),
        }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color && !s.is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }

    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }

    pub fn scene(&self, s: &str) -> String {
        self.wrap("2;3", s)
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }

    pub fn error(&self, s: &str) -> String {
        self.wrap("31", s)
    }
}

/// The corpus's scene marker: a line wrapped in `open` and `close` describes the scene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub open: String,
    pub close: String,
}

impl Marker {
    pub fn new(pair: &(String, String)) -> Self {
        Self {
            open: pair.0.clone(),
            close: pair.1.clone(),
        }
    }

    /// The inside of a scene line, or `None` when the line is not one. Only whole lines
    /// count, and a doubled marker (emphasis) is not a scene.
    pub fn inner<'a>(&self, line: &'a str) -> Option<&'a str> {
        let (open, close) = (self.open.as_str(), self.close.as_str());
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

    /// Whether a line still being written could turn out to be a scene line.
    fn may_open(&self, partial: &str) -> bool {
        let p = partial.trim_start();
        !self.open.is_empty() && (p.starts_with(&self.open) || self.open.starts_with(p))
    }
}

/// `text` without its `[#id]` citation marks (one id, or several separated by commas in
/// one bracket, each optionally prefixed by `#`), with the blanks before a mark and at
/// line ends removed. The marks tell the engine which passages a reply drew on; a reader
/// sees those as the sources, not inside the speech.
pub fn without_cites(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("[#")
            && let Some(end) = cite_end(text, i)
        {
            out.truncate(out.trim_end_matches([' ', '\t']).len());
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap_or_default();
        out.push(c);
        i += c.len_utf8().max(1);
    }
    out.split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the citation mark starting at `start` (`[#`) ends, if it is one.
fn cite_end(text: &str, start: usize) -> Option<usize> {
    let mut rest = &text[start + 2..];
    let mut at = start + 2;
    let take = |rest: &mut &str, at: &mut usize| -> Option<()> {
        let n = rest
            .find(|c: char| c == ']' || c == ',' || c.is_whitespace())
            .unwrap_or(rest.len());
        let chars = rest[..n].chars().count();
        if chars == 0 || chars > 128 {
            return None;
        }
        *rest = &rest[n..];
        *at += n;
        Some(())
    };
    take(&mut rest, &mut at)?;
    loop {
        let trimmed = rest.trim_start();
        at += rest.len() - trimmed.len();
        rest = trimmed;
        if let Some(r) = rest.strip_prefix(',') {
            let t = r.trim_start();
            at += 1 + (r.len() - t.len());
            rest = t.strip_prefix('#').map_or(t, |s| {
                at += 1;
                s
            });
            take(&mut rest, &mut at)?;
        } else {
            return rest.starts_with(']').then_some(at + 1);
        }
    }
}

/// A whole message, rendered: speech as written minus citation marks, scene lines styled.
/// Every line ends with a newline. A line that held nothing but marks is dropped, and so
/// are the blank lines left at the end, so a closing list of marks leaves no gap.
pub fn message(text: &str, marker: &Marker, style: Style) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        match marker.inner(line) {
            Some(inner) => lines.push(style.scene(inner)),
            None => {
                let speech = without_cites(line);
                if speech.trim().is_empty() && !line.trim().is_empty() {
                    continue;
                }
                lines.push(speech);
            }
        }
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Renders a reply as its deltas arrive. Speech is written as soon as it cannot change
/// (a citation mark that is still open is held back); a line that might be a scene line
/// is held until it is complete, since only whole lines count.
pub struct Streamer {
    marker: Marker,
    style: Style,
    line: String,
    /// Bytes of the line's rendered speech already handed out.
    printed: usize,
    /// Whether the cursor sits at the start of a line. A reply starts after its name.
    col0: bool,
    /// Blank lines seen but not yet written: they separate paragraphs, so they are only
    /// written once more text follows and never trail the reply.
    blanks: usize,
}

impl Streamer {
    pub fn new(marker: Marker, style: Style) -> Self {
        Self {
            marker,
            style,
            line: String::new(),
            printed: 0,
            col0: false,
            blanks: 0,
        }
    }

    /// The text to write for `delta`.
    pub fn push(&mut self, delta: &str) -> String {
        let mut out = String::new();
        for piece in delta.split_inclusive('\n') {
            let (body, complete) = match piece.strip_suffix('\n') {
                Some(b) => (b, true),
                None => (piece, false),
            };
            self.line.push_str(body.trim_end_matches('\r'));
            if complete {
                out.push_str(&self.complete());
            } else {
                out.push_str(&self.partial());
            }
        }
        out
    }

    /// The rest of the reply; always leaves the cursor at the start of a line.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.line.is_empty() {
            out.push_str(&self.complete());
        }
        if !self.col0 {
            out.push('\n');
            self.col0 = true;
        }
        out
    }

    fn complete(&mut self) -> String {
        let line = std::mem::take(&mut self.line);
        let printed = std::mem::take(&mut self.printed);
        if printed == 0
            && let Some(inner) = self.marker.inner(&line)
        {
            return self.write(&format!("{}\n", self.style.scene(inner)));
        }
        let speech = without_cites(&line);
        let rest = speech.get(printed..).unwrap_or_default();
        if printed == 0 && speech.trim().is_empty() {
            // A blank line waits for what follows; a line of marks alone is dropped.
            if line.trim().is_empty() {
                self.blanks += 1;
            }
            return String::new();
        }
        self.write(&format!("{rest}\n"))
    }

    /// `text` preceded by the blank lines held back, marking the cursor position.
    fn write(&mut self, text: &str) -> String {
        let mut out = "\n".repeat(std::mem::take(&mut self.blanks));
        out.push_str(text);
        self.col0 = text.ends_with('\n');
        out
    }

    fn partial(&mut self) -> String {
        if self.marker.may_open(&self.line) {
            return String::new();
        }
        let mut end = self.line.len();
        if let Some(open) = self.line.rfind('[')
            && !self.line[open..].contains(']')
        {
            end = open;
        }
        let safe = self.line[..end].trim_end_matches([' ', '\t']);
        let speech = without_cites(safe);
        let out = speech.get(self.printed..).unwrap_or_default().to_owned();
        self.printed = speech.len().max(self.printed);
        if out.is_empty() {
            return out;
        }
        self.write(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker() -> Marker {
        Marker {
            open: "(".into(),
            close: ")".into(),
        }
    }

    #[test]
    fn cites_are_removed_with_the_blank_before_them() {
        assert_eq!(without_cites("It works [#u2]."), "It works.");
        assert_eq!(without_cites("a [#u1, #u2] b [#x,y]"), "a b");
        assert_eq!(without_cites("two  \t[#a]\nnext [#b]  "), "two\nnext");
        // Not citations: no hash, empty id, unclosed, too long.
        assert_eq!(
            without_cites("see [1] and [#] and [#abc"),
            "see [1] and [#] and [#abc"
        );
        let long = format!("x [#{}]", "a".repeat(129));
        assert_eq!(without_cites(&long), long);
        assert_eq!(without_cites("[#a b]"), "[#a b]");
    }

    #[test]
    fn scene_lines_are_whole_lines_only() {
        let m = marker();
        assert_eq!(m.inner("(smiles)"), Some("smiles"));
        assert_eq!(m.inner("  (smiles)  "), Some("smiles"));
        assert_eq!(m.inner("(smiles) and speaks"), None);
        assert_eq!(m.inner("(())"), None);
        assert_eq!(m.inner("()"), None);
        let star = Marker {
            open: "*".into(),
            close: "*".into(),
        };
        assert_eq!(star.inner("*nods*"), Some("nods"));
        assert_eq!(star.inner("**bold**"), None);
    }

    #[test]
    fn colour_is_off_for_plain_style_and_on_otherwise() {
        let text = "(waves)\nHello [#u1]\n";
        assert_eq!(message(text, &marker(), Style::PLAIN), "waves\nHello\n");
        let coloured = message(text, &marker(), Style { color: true });
        assert!(coloured.starts_with("\x1b[2;3mwaves\x1b[0m\n"));
        assert!(coloured.ends_with("Hello\n"));
        assert_eq!(Style::PLAIN.dim("x"), "x");
        assert_eq!(Style { color: true }.dim("x"), "\x1b[2mx\x1b[0m");
    }

    /// Feed a reply in pieces of `size` characters and collect what is written.
    fn stream(text: &str, size: usize) -> String {
        let mut s = Streamer::new(marker(), Style::PLAIN);
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        for piece in chars.chunks(size) {
            out.push_str(&s.push(&piece.iter().collect::<String>()));
        }
        out.push_str(&s.finish());
        out
    }

    #[test]
    fn streaming_matches_whole_message_rendering_at_any_split() {
        let text = "(she looks up)\nThe water must be cooler [#u2] than that, [#u3, #u4] really.\nSecond line (not a scene)\n\n(done)";
        let whole = message(text, &marker(), Style::PLAIN);
        for size in 1..12 {
            assert_eq!(stream(text, size), whole, "size {size}");
        }
    }

    #[test]
    fn speech_streams_before_the_line_ends_but_citations_wait() {
        let mut s = Streamer::new(marker(), Style::PLAIN);
        assert_eq!(s.push("Hello there"), "Hello there");
        assert_eq!(s.push(" [#u"), "");
        assert_eq!(s.push("1] friend"), " friend");
        assert_eq!(s.push("\n"), "\n");
        // A possible scene line is held.
        assert_eq!(s.push("(nods"), "");
        assert_eq!(s.push(")\n"), "nods\n");
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn an_unfinished_line_is_closed_on_finish() {
        let mut s = Streamer::new(marker(), Style::PLAIN);
        assert_eq!(s.push("no newline [#a]"), "no newline");
        assert_eq!(s.finish(), "\n");
    }

    #[test]
    fn paragraphs_stay_apart_and_a_closing_marks_line_leaves_no_gap() {
        let text = "First part.\n\nSecond part.\n\n[#u1, #u2]\n";
        let whole = message(text, &marker(), Style::PLAIN);
        assert_eq!(whole, "First part.\n\nSecond part.\n");
        for size in 1..12 {
            assert_eq!(stream(text, size), whole, "size {size}");
            assert_eq!(stream(text.trim_end(), size), whole, "trimmed, size {size}");
        }
        assert_eq!(message("[#a]", &marker(), Style::PLAIN), "");
        assert_eq!(stream("[#a]", 2), "\n");
    }
}
