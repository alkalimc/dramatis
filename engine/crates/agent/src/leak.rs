//! Harness text a reply reproduces after its own words.
//!
//! Some endpoints keep writing past their reply into the next turn's prompt: the turn
//! instruction, the retrieval preface, the passages, the context line. Everything from the
//! first such line on is not the speaker's. The lines are known from the corpus's own
//! wording, so nothing here depends on a language.

use crate::assemble;
use crate::text::Wording;

/// Wording keys whose filled text the harness puts in front of a turn.
const KEYS: [&str; 10] = [
    "harness.speaker",
    "harness.interject",
    "harness.chorus",
    "harness.low_match",
    "harness.nothing_found",
    "harness.context",
    "harness.context.gap",
    "harness.joined",
    "harness.left",
    "harness.said",
];

/// Longest a name, a date or any other filled field is taken to be, in characters.
const FIELD_MAX: usize = 40;
const NAME_MAX: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// A person's or the user's name.
    Name,
    /// A date or a time of day.
    Digits,
    /// Anything else on one line.
    Any,
}

impl Field {
    fn of(name: &str) -> Self {
        match name {
            "name" | "user" | "host" => Field::Name,
            "date" | "time" => Field::Digits,
            _ => Field::Any,
        }
    }

    fn max(self) -> usize {
        if self == Field::Name {
            NAME_MAX
        } else {
            FIELD_MAX
        }
    }

    fn takes(self, c: char) -> bool {
        match self {
            Field::Name => {
                c.is_alphanumeric()
                    || matches!(c, '\u{b7}' | '\u{30fb}' | '.' | '-' | '\'' | '_' | ' ')
            }
            Field::Digits => c.is_ascii_digit() || matches!(c, '-' | ':' | '/' | '.'),
            Field::Any => c != '\n',
        }
    }
}

/// A wording template as a line shape: literal pieces with fields between them.
#[derive(Debug, Clone)]
struct Shape {
    pieces: Vec<String>,
    fields: Vec<Field>,
}

impl Shape {
    fn parse(template: &str, placeholder: &str) -> Option<Self> {
        let mut pieces = vec![String::new()];
        let mut fields = Vec::new();
        let mut rest = template;
        while !rest.is_empty() {
            if !placeholder.is_empty()
                && let Some(after) = rest.strip_prefix(placeholder)
            {
                fields.push(Field::Name);
                pieces.push(String::new());
                rest = after;
            } else if rest.starts_with('{')
                && let Some(end) = rest.find('}')
            {
                fields.push(Field::of(&rest[1..end]));
                pieces.push(String::new());
                rest = &rest[end + 1..];
            } else {
                let c = rest.chars().next().unwrap_or_default();
                pieces.last_mut()?.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
        // A shape with next to no fixed text would match ordinary speech.
        let fixed: usize = pieces.iter().map(|p| p.trim().chars().count()).sum();
        (fixed >= 2).then_some(Self { pieces, fields })
    }

    /// Whether `line` starts with this shape. With `partial`, a line that ends inside the
    /// shape counts too: it may still become one.
    fn starts(&self, line: &str, partial: bool) -> bool {
        self.from(0, line, partial)
    }

    fn from(&self, i: usize, rest: &str, partial: bool) -> bool {
        let piece = self.pieces[i].as_str();
        let rest = match rest.strip_prefix(piece) {
            Some(r) => r,
            None => return partial && piece.starts_with(rest),
        };
        let Some(&field) = self.fields.get(i) else {
            return true;
        };
        let mut at = 0;
        for (n, c) in rest.chars().enumerate() {
            if n >= field.max() || !field.takes(c) {
                break;
            }
            at += c.len_utf8();
            if self.from(i + 1, &rest[at..], partial) {
                return true;
            }
        }
        // The field itself may still be growing.
        partial && rest.chars().all(|c| field.takes(c)) && rest.chars().count() < field.max()
    }
}

/// The harness lines a reply may reproduce, from one corpus's wording.
#[derive(Debug, Clone, Default)]
pub struct Leaks {
    shapes: Vec<Shape>,
}

impl Leaks {
    pub fn new(wording: &Wording, placeholder: &str) -> Self {
        Self {
            shapes: KEYS
                .iter()
                .filter_map(|k| Shape::parse(wording.get(k), placeholder))
                .collect(),
        }
    }

    /// Where the first line that reproduces harness text starts, if any. `after_text` says
    /// the reply already said something before `text`, `mid_line` that `text` starts inside
    /// a line. Nothing before any words counts: a reply that opens with such a line is left
    /// alone. `marks` are the closing marks, which may follow a citation line.
    pub fn find(
        &self,
        text: &str,
        after_text: bool,
        mid_line: bool,
        marks: &[&str],
    ) -> Option<usize> {
        self.scan(text, after_text, mid_line, marks, false)
    }

    /// [`Leaks::find`] for text still arriving: an unfinished last line that may become
    /// harness text counts too.
    pub fn hold_from(
        &self,
        text: &str,
        after_text: bool,
        mid_line: bool,
        marks: &[&str],
    ) -> Option<usize> {
        self.scan(text, after_text, mid_line, marks, true)
    }

    fn scan(
        &self,
        text: &str,
        after_text: bool,
        mid_line: bool,
        marks: &[&str],
        streaming: bool,
    ) -> Option<usize> {
        let mut start = 0;
        let mut seen = false;
        for (k, raw) in text.split_inclusive('\n').enumerate() {
            let at = start;
            start += raw.len();
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let eligible = if k == 0 {
                after_text && !mid_line
            } else {
                after_text || seen
            };
            seen = true;
            if !eligible {
                continue;
            }
            let finished = raw.ends_with('\n') || !streaming;
            let hit = if finished {
                self.shapes.iter().any(|s| s.starts(line, false))
                    || self.passage(line, &text[start..], marks)
            } else {
                self.shapes.iter().any(|s| s.starts(line, true))
            };
            if hit {
                return Some(at);
            }
        }
        None
    }

    /// A line that opens a retrieved passage, or a line of citation marks that is not the
    /// reply's own (those end the reply, apart from closing lines).
    fn passage(&self, line: &str, after: &str, marks: &[&str]) -> bool {
        if !line.starts_with("[#") {
            return false;
        }
        if !api::cites::is_cite_line(line) {
            return true;
        }
        after.lines().map(str::trim).any(|l| {
            !l.is_empty()
                && !api::cites::is_cite_line(l)
                && !marks
                    .iter()
                    .any(|m| !m.is_empty() && assemble::starts_with_mark(l, m))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::assemble::{closing_marks, split_closing};

    /// The owner's corpus wording for the lines that matter here, written as escapes.
    fn wording() -> Wording {
        let pairs = [
            ("harness.find_mark", "\u{21d2} \u{627e}\u{4eba}\u{ff1a}"),
            ("harness.speaker", "{name}\u{56de}\u{7b54}\u{3002}"),
            (
                "harness.chorus",
                "{name}\u{4e5f}\u{63a5}\u{7740}\u{524d}\u{9762}\u{7684}\u{4eba}\u{56de}\u{7b54}\u{3002}",
            ),
            (
                "harness.low_match",
                "\u{4e0b}\u{9762}\u{8fd9}\u{4e9b}\u{6bb5}\u{843d}\u{53ef}\u{80fd}\u{4e0e}\u{95ee}\u{9898}\u{65e0}\u{5173}\u{ff0c}\u{53ea}\u{6709}\u{786e}\u{5b9e}\u{5bf9}\u{5f97}\u{4e0a}\u{624d}\u{7528}\u{3002}",
            ),
            ("harness.context", "[{date} {weekday} {time}\u{ff0c}{part}]"),
        ];
        Wording::new(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    /// Group message 79 of the owner's save, as stored.
    const MESSAGE_79: &str = "\u{ff08}\u{7406}\u{4e86}\u{7406}\u{624b}\u{4e2d}\u{7684}\u{6587}\u{4ef6}\u{ff0c}\u{5c06}\u{51e0}\u{4efd}\u{521a}\u{6c47}\u{603b}\u{597d}\u{7684}\u{533a}\u{57df}\u{62a5}\u{544a}\u{8f7b}\u{653e}\u{5728}\u{684c}\u{9762}\u{4e0a}\u{ff0c}\u{8ba4}\u{771f}\u{5730}\u{770b}\u{5411}alkali\u{ff09}\n\n\u{76ee}\u{524d}\u{6211}\u{4eec}\u{6551}\u{52a9}\u{548c}\u{5916}\u{52e4}\u{529e}\u{4e8b}\u{5904}\u{5206}\u{5e03}\u{8f83}\u{5e7f}\u{ff0c}\u{4f46}\u{4e3b}\u{8981}\u{96c6}\u{4e2d}\u{5728}\u{51e0}\u{4e2a}\u{91cd}\u{70b9}\u{533a}\u{57df}\u{3002}\u{7ef4}\u{591a}\u{5229}\u{4e9a}\u{53ca}\u{5468}\u{8fb9}\u{7684}\u{57ce}\u{90ca}\u{5730}\u{5e26}\u{56e0}\u{4e3a}\u{4e4b}\u{524d}\u{7684}\u{6218}\u{4e71}\u{548c}\u{5c40}\u{52bf}\u{52a8}\u{8361}\u{ff0c}\u{6ede}\u{7559}\u{4e86}\u{5927}\u{91cf}\u{8feb}\u{5207}\u{9700}\u{8981}\u{533b}\u{7597}\u{63f4}\u{52a9}\u{7684}\u{611f}\u{67d3}\u{8005}\u{5e73}\u{6c11}\u{ff0c}\u{6211}\u{4eec}\u{5728}\u{8fd9}\u{8fb9}\u{7684}\u{529e}\u{4e8b}\u{5904}\u{548c}\u{5916}\u{52e4}\u{5c0f}\u{961f}\u{4e00}\u{76f4}\u{5728}\u{8d85}\u{8d1f}\u{8377}\u{8fd0}\u{8f6c}\u{ff1b}\u{53e6}\u{5916}\u{ff0c}\u{5728}\u{8428}\u{5c14}\u{8d21}\u{8fb9}\u{5883}\u{4e0e}\u{51e0}\u{5927}\u{79fb}\u{52a8}\u{57ce}\u{90a6}\u{4ea4}\u{754c}\u{5904}\u{7684}\u{8352}\u{91ce}\u{ff0c}\u{4ee5}\u{53ca}\u{4e4c}\u{8428}\u{65af}\u{8fb9}\u{5883}\u{7684}\u{611f}\u{67d3}\u{8005}\u{805a}\u{5c45}\u{533a}\u{ff0c}\u{7f57}\u{5fb7}\u{5c9b}\u{4e5f}\u{6709}\u{957f}\u{671f}\u{9a7b}\u{7559}\u{7684}\u{533b}\u{7597}\u{5c0f}\u{7ec4}\u{548c}\u{5de1}\u{56de}\u{8bca}\u{6240}\u{ff0c}\u{8d1f}\u{8d23}\u{63d0}\u{4f9b}\u{57fa}\u{7840}\u{836f}\u{7269}\u{4e0e}\u{5b9a}\u{671f}\u{4f53}\u{68c0}\u{3002}\u{5269}\u{4e0b}\u{7684}\u{5c31}\u{662f}\u{5404}\u{5730}\u{529e}\u{4e8b}\u{5904}\u{6839}\u{636e}\u{4e34}\u{65f6}\u{6c42}\u{52a9}\u{53d1}\u{56de}\u{7684}\u{7d27}\u{6025}\u{6551}\u{52a9}\u{7533}\u{8bf7}\u{4e86}\u{3002}\n\n\u{21d2} \u{627e}\u{4eba}\u{ff1a}\u{51ef}\u{5c14}\u{5e0c}\u{ff5c}\u{8865}\u{5145}\u{611f}\u{67d3}\u{8005}\u{6551}\u{52a9}\u{7684}\u{5177}\u{4f53}\u{533a}\u{57df}\u{5206}\u{5e03}\u{4e0e}\u{8d44}\u{6e90}\u{8c03}\u{5ea6}\n\n\u{51ef}\u{5c14}\u{5e0c}\u{4e5f}\u{63a5}\u{7740}\u{524d}\u{9762}\u{7684}\u{4eba}\u{56de}\u{7b54}\u{3002}\n\n\u{4e0b}\u{9762}\u{8fd9}\u{4e9b}\u{6bb5}\u{843d}\u{53ef}\u{80fd}\u{4e0e}\u{95ee}\u{9898}\u{65e0}\u{5173}\u{ff0c}\u{53ea}\u{6709}\u{786e}\u{5b9e}\u{5bf9}\u{5f97}\u{4e0a}\u{624d}\u{7528}\u{3002}\n\n[#lore:7b99b14d698b4a35]\n\n[#dialogue:2b4f25050df8f64d]\n\n[#dialogue:dfea8b5362edc82d]\n\n[1103-10-30 \u{5468}\u{4e94} 19:19\u{ff0c}\u{665a}\u{4e0a}]";

    /// Direct message 118, as stored.
    const MESSAGE_118: &str = "\u{ff08}\u{8f7b}\u{8f7b}\u{62c9}\u{8d77}\u{4f60}\u{7684}\u{624b}\u{ff0c}\u{6307}\u{5c16}\u{5fae}\u{4e0d}\u{53ef}\u{5bdf}\u{5730}\u{642d}\u{5728}\u{4f60}\u{7684}\u{8109}\u{640f}\u{4e0a}\u{ff0c}\u{542b}\u{7b11}\u{63a2}\u{77e5}\u{7740}\u{90a3}\u{6c89}\u{7a33}\u{800c}\u{5fae}\u{5f31}\u{7684}\u{7535}\u{4fe1}\u{53f7}\u{ff09}\n\n\u{8109}\u{640f}\u{5f88}\u{7a33}\u{ff0c}\u{770b}\u{8d77}\u{6765}\u{535a}\u{58eb}\u{4eca}\u{5929}\u{72b6}\u{6001}\u{8fd8}\u{4e0d}\u{9519}\u{ff0c}\u{6ca1}\u{88ab}\u{8fd9}\u{4e9b}\u{7e41}\u{6742}\u{7684}\u{60c5}\u{62a5}\u{641e}\u{5f97}\u{5934}\u{75bc}\u{3002}\n\n\u{5173}\u{4e8e}\u{4f60}\u{63d0}\u{5230}\u{7684}\u{90a3}\u{6761}\u{60c5}\u{62a5}\u{ff0c}\u{6211}\u{4e5f}\u{7559}\u{610f}\u{5230}\u{4e86}\u{3002}\u{8428}\u{5c14}\u{8d21}\u{8fb9}\u{5883}\u{4e0e}\u{54e5}\u{4f26}\u{6bd4}\u{4e9a}\u{7684}\u{4ea4}\u{754c}\u{5e26}\u{7f3a}\u{4e4f}\u{7edf}\u{4e00}\u{7ba1}\u{8f96}\u{ff0c}\u{52a0}\u{4e0a}\u{62d3}\u{8352}\u{6f6e}\u{548c}\u{8d44}\u{6e90}\u{4e89}\u{593a}\u{ff0c}\u{50cf}\u{6c99}\u{5352}\u{3001}\u{94c1}\u{638c}\u{5e2e}\u{8fd9}\u{6837}\u{7684}\u{5730}\u{4e0b}\u{6b66}\u{88c5}\u{4ee5}\u{53ca}\u{4e13}\u{4e8b}\u{523a}\u{6740}\u{7684}\u{9690}\u{79d8}\u{7ec4}\u{7ec7}\u{786e}\u{5b9e}\u{9891}\u{7e41}\u{51fa}\u{6ca1}\u{3002}\n\n\u{4e0d}\u{8fc7}\u{8fd9}\u{7c7b}\u{5730}\u{65b9}\u{60c5}\u{62a5}\u{7684}\u{63cf}\u{8ff0}\u{5f80}\u{5f80}\u{4f1a}\u{6709}\u{4e3b}\u{89c2}\u{504f}\u{5dee}\u{ff0c}\u{5982}\u{679c}\u{4f60}\u{9700}\u{8981}\u{628a}\u{8fd9}\u{4efd}\u{62a5}\u{544a}\u{6838}\u{5bf9}\u{5f97}\u{66f4}\u{7cbe}\u{786e}\u{ff0c}\u{4e0d}\u{59a8}\u{76f4}\u{63a5}\u{95ee}\u{95ee}\u{6700}\u{719f}\u{6089}\u{5f53}\u{5730}\u{60c5}\u{51b5}\u{7684}\u{4eba}\u{3002}\u{54c8}\u{8482}\u{5a05}\u{5c31}\u{5728}\u{8428}\u{5c14}\u{8d21}\u{529e}\u{4e8b}\u{5904}\u{ff0c}\u{5bf9}\u{4e24}\u{8fb9}\u{8fb9}\u{5883}\u{7684}\u{73af}\u{5883}\u{90fd}\u{5f88}\u{4e86}\u{89e3}\u{ff1b}\u{6216}\u{8005}\u{6211}\u{4eec}\u{53bb}\u{5efa}\u{4e2a}\u{4e34}\u{65f6}\u{8ba8}\u{8bba}\u{7ec4}\u{ff0c}\u{628a}\u{9ed1}\u{4e5f}\u{62c9}\u{8fdb}\u{6765}\u{ff0c}\u{5979}\u{5bf9}\u{8fb9}\u{5883}\u{6697}\u{6d41}\u{548c}\u{54e5}\u{4f26}\u{6bd4}\u{4e9a}\u{7684}\u{52bf}\u{529b}\u{53d8}\u{5316}\u{6700}\u{6e05}\u{695a}\u{4e0d}\u{8fc7}\u{4e86}\u{3002}\n\n\u{21d2} \u{627e}\u{4eba}\u{ff1a}\u{54c8}\u{8482}\u{5a05}, \u{9ed1}\u{ff5c}\u{6838}\u{5bf9}\u{54e5}\u{4f26}\u{6bd4}\u{4e9a}\u{4e0e}\u{8428}\u{5c14}\u{8d21}\u{8fb9}\u{5883}\u{6b66}\u{88c5}\u{52bf}\u{529b}\u{60c5}\u{62a5}";

    #[test]
    fn the_owners_message_79_is_cut_before_the_copied_prompt() {
        let w = wording();
        let leaks = Leaks::new(&w, "");
        let marks = closing_marks(&w, true);
        let at = leaks
            .find(MESSAGE_79, false, false, &marks.list())
            .expect("a cut");
        assert!(MESSAGE_79[at..].starts_with("\u{51ef}\u{5c14}\u{5e0c}\u{4e5f}"));
        // With the leak gone the find line is the last one and is recognised again.
        let closing = split_closing(MESSAGE_79[..at].trim_end(), marks);
        assert_eq!(
            closing.find,
            Some(vec!["\u{51ef}\u{5c14}\u{5e0c}".to_owned()])
        );
        assert!(!closing.text.contains('\u{21d2}') && !closing.text.contains("[#"));
        assert!(closing.text.ends_with("\u{7533}\u{8bf7}\u{4e86}\u{3002}"));
    }

    #[test]
    fn the_owners_message_118_loses_its_find_line() {
        let w = wording();
        let leaks = Leaks::new(&w, "");
        let marks = closing_marks(&w, false);
        assert_eq!(leaks.find(MESSAGE_118, false, false, &marks.list()), None);
        let closing = split_closing(MESSAGE_118, marks);
        assert_eq!(
            closing.find,
            Some(vec![
                "\u{54c8}\u{8482}\u{5a05}".to_owned(),
                "\u{9ed1}".to_owned()
            ])
        );
        assert!(!closing.text.contains('\u{21d2}'));
    }

    #[test]
    fn every_kind_of_harness_line_is_found() {
        let w = wording();
        let leaks = Leaks::new(&w, "");
        let marks = closing_marks(&w, true).list();
        let name = "\u{51ef}\u{5c14}\u{5e0c}";
        let speech = "\u{597d}\u{7684}\u{3002}";
        for line in [
            format!("{name}\u{56de}\u{7b54}\u{3002}"),
            format!(
                "{name}\u{4e5f}\u{63a5}\u{7740}\u{524d}\u{9762}\u{7684}\u{4eba}\u{56de}\u{7b54}\u{3002}"
            ),
            "[1103-10-30 \u{5468}\u{4e94} 19:19\u{ff0c}\u{665a}\u{4e0a}]".to_owned(),
            "[#lore:abc] A title".to_owned(),
        ] {
            let text = format!("{speech}\n\n{line}\n\nmore");
            let at = leaks.find(&text, false, false, &marks);
            assert_eq!(at, Some(speech.len() + 2), "{line}");
        }
        // Citation marks alone and last are the reply's own; followed by speech they are not.
        let cited = format!("{speech}\n[#a1] [#b2]");
        assert_eq!(leaks.find(&cited, false, false, &marks), None);
        let more = format!("{speech}\n[#a1]\n{speech}");
        assert_eq!(
            leaks.find(&more, false, false, &marks),
            Some(speech.len() + 1)
        );
        // A closing line may follow the citation line.
        let find = w.get("harness.find_mark");
        let closing = format!("{speech}\n[#a1]\n{find} x");
        assert_eq!(leaks.find(&closing, false, false, &marks), None);
    }

    #[test]
    fn speech_that_only_resembles_a_harness_line_stays() {
        let w = wording();
        let leaks = Leaks::new(&w, "");
        let marks = closing_marks(&w, false).list();
        // The reply opening with such a line is left alone: nothing came before it.
        let alone = "\u{6211}\u{56de}\u{7b54}\u{3002}\n\u{597d}\u{7684}";
        assert_eq!(leaks.find(alone, false, false, &marks), None);
        // A name field never spans punctuation or a long clause.
        let clause =
            "\u{597d}\u{3002}\n\u{4ed6}\u{8bf4}\u{ff0c}\u{6211}\u{4f1a}\u{56de}\u{7b54}\u{3002}";
        assert_eq!(leaks.find(clause, false, false, &marks), None);
        // A scene line is not a context line.
        let scene = "\u{597d}\u{3002}\n[smiles at you, warmly]";
        assert_eq!(leaks.find(scene, false, false, &marks), None);
    }

    #[test]
    fn the_stream_holds_a_line_that_may_become_harness_text() {
        let w = wording();
        let leaks = Leaks::new(&w, "");
        let marks = closing_marks(&w, true).list();
        let speech = "\u{597d}\u{7684}\u{3002}\n\n";
        let name = "\u{51ef}\u{5c14}\u{5e0c}";
        // A name alone on a new line, a name and the start of the instruction, and the
        // whole instruction are all held; a name followed by other words is released.
        for tail in [name, "{n}\u{4e5f}\u{63a5}", "{n}\u{56de}\u{7b54}\u{3002}\n"] {
            let text = format!("{speech}{}", tail.replace("{n}", name));
            assert_eq!(
                leaks.hold_from(&text, true, false, &marks),
                Some(speech.len()),
                "{tail}"
            );
        }
        let text = format!("{speech}{name}\u{8bf4}\u{ff0c}");
        assert_eq!(leaks.hold_from(&text, true, false, &marks), None);
        // Text that starts inside a line is not a line start.
        assert_eq!(
            leaks.hold_from(
                &format!("{name}\u{56de}\u{7b54}\u{3002}"),
                true,
                true,
                &marks
            ),
            None
        );
        // Nothing is held before anything was said.
        assert_eq!(leaks.hold_from(name, false, false, &marks), None);
    }
}
