//! What goes into a session's bytes: the opening blocks and every appended harness text.
//!
//! Everything here is a pure function of its inputs, so the same world state always
//! yields the same bytes. Corpus and prompt text passes through [`Names::sub`] on the
//! way in; the user's own words never do.

use std::collections::HashSet;

use folio::Unit;
use index::{Hit, Item, Level, SearchResponse};
use world::fact::Fact;
use world::trust::Tone;

use crate::agent::Frontend;
use crate::persona::{HostLayers, Persona};
use crate::text::{Names, Wording};

/// The corpus-level pieces every block is built from.
pub struct Ctx<'a> {
    pub wording: &'a Wording,
    pub names: &'a Names,
    /// The pack's scene marker, `(open, close)`.
    pub scene: (&'a str, &'a str),
}

impl Ctx<'_> {
    fn fill(&self, key: &str, fields: &[(&str, &str)]) -> String {
        self.wording.fill(key, self.names, fields)
    }
}

/// Block A: the global static text, identical for every person of this user. A group's
/// log adds how a member hands the turn on.
pub fn block_a(ctx: &Ctx<'_>, group: bool) -> String {
    let mut parts = vec![ctx.fill("harness.rules", &[])];
    // The user's words go in as written, so the line is filled, never substituted into.
    if let Some(about) = ctx.names.about() {
        parts.push(ctx.fill("harness.about", &[("about", about)]));
    }
    parts.push(ctx.fill(
        "harness.output",
        &[("open", ctx.scene.0), ("close", ctx.scene.1)],
    ));
    parts.push(ctx.fill("harness.citation", &[]));
    if group {
        parts.push(ctx.fill("harness.next", &[("mark", hand_off_mark(ctx.wording))]));
    }
    parts.join("\n\n")
}

/// What the host's usage guide covers, in the order it is written.
const GUIDE_TOPICS: [&str; 8] = [
    "channels",
    "group",
    "resend",
    "sources",
    "name",
    "intensity",
    "saves",
    "memories",
];

/// How the host explains the front end the user is on: one line per operation, worded by
/// the corpus, for that front end only.
pub fn guide(ctx: &Ctx<'_>, frontend: Frontend) -> String {
    let which = frontend.key();
    let mut lines = vec![ctx.fill(&format!("harness.guide.{which}"), &[])];
    lines.extend(
        GUIDE_TOPICS
            .iter()
            .map(|t| ctx.fill(&format!("harness.guide.{which}.{t}"), &[])),
    );
    lines.push(ctx.fill("harness.guide.limits", &[]));
    // What the user was told at the first meeting, so the host can tell it again.
    let intro = ctx.fill(&format!("host.intro.{which}"), &[]);
    lines.push(ctx.fill("harness.guide.intro", &[("intro", &intro)]));
    lines.join("\n")
}

/// Block A of the host's own session: the same, then both of its layers and the usage
/// manner and usage guide. Only this block ever carries the meta layer, the manner or
/// the guide.
pub fn block_a_host(
    ctx: &Ctx<'_>,
    host_name: &str,
    layers: &HostLayers,
    frontend: Frontend,
) -> String {
    let mut parts = vec![block_a(ctx, false), heading(ctx, host_name)];
    for layer in [&layers.in_world, &layers.meta] {
        if !layer.trim().is_empty() {
            parts.push(ctx.names.sub(layer.trim()).into_owned());
        }
    }
    parts.push(ctx.fill("harness.host.manner", &[]));
    parts.push(guide(ctx, frontend));
    parts.join("\n\n")
}

fn heading(ctx: &Ctx<'_>, name: &str) -> String {
    ctx.fill("harness.persona", &[("name", name)])
}

/// Someone whose persona a person session carries.
pub enum Member<'a> {
    Person {
        name: &'a str,
        persona: &'a Persona,
    },
    /// The host in someone else's log: its in-world layer only.
    Host {
        name: &'a str,
        in_world: &'a str,
    },
}

/// One member's persona section. Also what a late joiner's entry carries.
pub fn persona_section(ctx: &Ctx<'_>, member: &Member<'_>) -> String {
    match member {
        Member::Person { name, persona } => {
            let mut parts = vec![heading(ctx, name)];
            for body in [&persona.system, &persona.tone] {
                if !body.trim().is_empty() {
                    parts.push(ctx.names.sub(body.trim()).into_owned());
                }
            }
            if !persona.lines.trim().is_empty() {
                let lines = ctx.names.sub(persona.lines.trim()).into_owned();
                parts.push(format!(
                    "{}\n{lines}",
                    ctx.fill("harness.lines", &[("name", name)])
                ));
            }
            parts.join("\n")
        }
        Member::Host { name, in_world } => {
            let mut parts = vec![heading(ctx, name)];
            if !in_world.trim().is_empty() {
                parts.push(ctx.names.sub(in_world.trim()).into_owned());
            }
            parts.join("\n")
        }
    }
}

/// The marker a member's section starts with: how a log is checked for his persona.
pub fn persona_marker(ctx: &Ctx<'_>, name: &str) -> String {
    heading(ctx, name)
}

/// Block B: every member, in the given order.
pub fn block_b(ctx: &Ctx<'_>, members: &[Member<'_>]) -> String {
    members
        .iter()
        .map(|m| persona_section(ctx, m))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What block C opens with.
pub struct Opening<'a> {
    /// Each person's tone band with the user.
    pub tones: Vec<(&'a str, Tone)>,
    pub topic: Option<&'a str>,
    /// Memories the readers may all see, oldest first; `shared` when several read them.
    pub memories: &'a [Fact],
    pub memory_owner: Option<&'a str>,
    pub summary: Option<&'a str>,
    /// `(author, text)` of the last messages, oldest first.
    pub recent: &'a [(String, String)],
}

pub fn memory_id(fact: &Fact) -> String {
    format!("m{}", fact.id)
}

/// Block C.
pub fn block_c(ctx: &Ctx<'_>, o: &Opening<'_>) -> String {
    let mut parts = Vec::new();
    for (name, tone) in &o.tones {
        parts.push(ctx.fill(
            &format!("harness.tone.{}", tone.as_str()),
            &[("name", name)],
        ));
    }
    if let Some(t) = o.topic.filter(|t| !t.trim().is_empty()) {
        parts.push(ctx.fill("harness.topic", &[("topic", t.trim())]));
    }
    if !o.memories.is_empty() {
        let head = match o.memory_owner {
            Some(name) => ctx.fill("harness.memories", &[("name", name)]),
            None => ctx.fill("harness.memories.shared", &[]),
        };
        let lines: Vec<String> = o
            .memories
            .iter()
            .map(|f| format!("[#{}] {}", memory_id(f), ctx.names.sub(f.text.trim())))
            .collect();
        parts.push(format!("{head}\n{}", lines.join("\n")));
    }
    if let Some(s) = o.summary.filter(|s| !s.trim().is_empty()) {
        parts.push(ctx.fill("harness.summary", &[("summary", s.trim())]));
    }
    if !o.recent.is_empty() {
        let lines: Vec<String> = o
            .recent
            .iter()
            .map(|(who, text)| format!("{who}: {text}"))
            .collect();
        parts.push(format!(
            "{}\n{}",
            ctx.fill("harness.recent", &[]),
            lines.join("\n")
        ));
    }
    parts.join("\n\n")
}

/// Retrieved material as it enters a turn: new units in full, repeats as `[#id]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Material {
    pub text: String,
    /// Ids sent in full here, units and memories alike.
    pub new: Vec<String>,
    /// Every id shown, in order, repeats included: what a reply may cite.
    pub shown: Vec<String>,
    pub level: Option<Level>,
}

fn unit_block(ctx: &Ctx<'_>, u: &Unit) -> String {
    let head = if u.header.trim().is_empty() {
        u.title.trim()
    } else {
        u.header.trim()
    };
    let body = ctx.names.sub(u.text.trim());
    if head.is_empty() {
        format!("[#{}]\n{body}", u.id)
    } else {
        format!("[#{}] {}\n{body}", u.id, ctx.names.sub(head))
    }
}

/// Render a search response. `seen` is what the session already carries; ids sent here
/// are added to it. At low confidence only the best `low_hits` passages go out, without
/// neighbours and marked as possibly unrelated; when nothing came back, only the plain
/// statement of that.
pub fn material(
    ctx: &Ctx<'_>,
    resp: &SearchResponse,
    seen: &mut HashSet<String>,
    low_hits: usize,
) -> Material {
    let level = resp.confidence.level;
    let low = level == Level::Low;
    if resp.hits.is_empty() && resp.excluded.is_empty() {
        return Material {
            text: ctx.fill("harness.nothing_found", &[]),
            level: Some(level),
            ..Material::default()
        };
    }
    let mut out = Material {
        level: Some(level),
        ..Material::default()
    };
    let mut blocks = Vec::new();
    if low {
        blocks.push(ctx.fill("harness.low_match", &[]));
    }
    let push_unit =
        |u: &Unit, out: &mut Material, blocks: &mut Vec<String>, seen: &mut HashSet<String>| {
            out.shown.push(u.id.clone());
            if seen.insert(u.id.clone()) {
                out.new.push(u.id.clone());
                blocks.push(unit_block(ctx, u));
            } else {
                blocks.push(format!("[#{}]", u.id));
            }
        };
    // Excluded ids ranked inside the window come first: they are the best matches.
    for id in &resp.excluded {
        out.shown.push(id.clone());
        blocks.push(format!("[#{id}]"));
    }
    let shown = if low { low_hits } else { resp.hits.len() };
    for Hit {
        item, neighbours, ..
    } in resp.hits.iter().take(shown)
    {
        match item {
            Item::Unit(u) => {
                push_unit(u, &mut out, &mut blocks, seen);
                for n in neighbours.iter().filter(|_| !low) {
                    push_unit(n, &mut out, &mut blocks, seen);
                }
            }
            Item::Memory { id, text } => {
                out.shown.push(id.clone());
                if seen.insert(id.clone()) {
                    out.new.push(id.clone());
                    blocks.push(format!("[#{id}] {}", ctx.names.sub(text.trim())));
                } else {
                    blocks.push(format!("[#{id}]"));
                }
            }
        }
    }
    out.text = blocks.join("\n\n");
    out
}

/// Units fetched by id (an opening's seed material), deduplicated like a search.
pub fn units(ctx: &Ctx<'_>, units: &[Unit], seen: &mut HashSet<String>) -> Material {
    let mut out = Material::default();
    let mut blocks = Vec::new();
    for u in units {
        out.shown.push(u.id.clone());
        if seen.insert(u.id.clone()) {
            out.new.push(u.id.clone());
            blocks.push(unit_block(ctx, u));
        } else {
            blocks.push(format!("[#{}]", u.id));
        }
    }
    out.text = blocks.join("\n\n");
    out
}

/// A user entry: the user's words (verbatim) or harness lines, then the attachment.
pub fn entry(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| p.trim_end())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The marker a group reply's closing hand-off line starts with (`harness.next_mark`).
pub fn hand_off_mark(wording: &Wording) -> &str {
    wording.get("harness.next_mark").trim()
}

/// A reply without its closing hand-off line, and the names that line listed. The line
/// is the last non-empty one and starts with `mark`, or comes just before closing lines
/// of citation marks only (a model that writes the two closing lines in the other order);
/// those stay. Names are separated by commas (ASCII or full-width), enumeration commas or
/// semicolons. A marker anywhere else is speech and stays.
pub fn split_hand_off(text: &str, mark: &str) -> (String, Vec<String>) {
    if mark.is_empty() {
        return (text.to_owned(), Vec::new());
    }
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let mut i = lines.len();
    while i > 0 && (lines[i - 1].trim().is_empty() || api::cites::is_cite_line(lines[i - 1])) {
        i -= 1;
    }
    let Some(list) = i
        .checked_sub(1)
        .and_then(|k| lines[k].trim_start().strip_prefix(mark))
    else {
        return (text.to_owned(), Vec::new());
    };
    let names = list
        .split([',', '\u{ff0c}', '\u{3001}', ';', '\u{ff1b}'])
        .map(|n| n.trim().trim_matches(['"', '\'', '`']).trim())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    let head = lines[..i - 1].join("\n");
    let cites = lines[i..].join("\n");
    let kept = match (head.trim_end(), cites.trim()) {
        (h, "") => h.to_owned(),
        ("", c) => c.to_owned(),
        (h, c) => format!("{h}\n{c}"),
    };
    (kept, names)
}

/// How much of the streamed text is safe to show: everything except a trailing line that
/// is, or may still turn out to be, the hand-off line. A marker line followed by more
/// speech was never a closing line and is released.
pub fn shown_len(pending: &str, mark: &str) -> usize {
    if mark.is_empty() {
        return pending.len();
    }
    let body = pending.trim_end();
    let mut start = body.rfind('\n').map_or(0, |i| i + 1);
    // Closing citation lines (or one still being written) after a marker line: the
    // marker line is still the hand-off line, so hold from there.
    let mut end = body.len();
    while start > 0 {
        let line = body[start..end].trim();
        let cite = api::cites::is_cite_line(line)
            || (end == body.len() && line.starts_with('[') && !line.contains(']'));
        if !(line.is_empty() || cite) {
            break;
        }
        end = start - 1;
        let prev = body[..end].rfind('\n').map_or(0, |i| i + 1);
        if body[prev..end].trim_start().starts_with(mark) {
            return prev;
        }
        start = prev;
    }
    let start = body.rfind('\n').map_or(0, |i| i + 1);
    let line = body[start..].trim_start();
    let held = line.starts_with(mark) || (pending.len() == body.len() && mark.starts_with(line));
    if held && !line.is_empty() {
        start
    } else {
        pending.len()
    }
}

/// Whether a reply is an endpoint's canned boilerplate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Is,
    /// Not yet, but more text could make it so.
    Maybe,
    No,
}

/// ASCII words from which a reply with no CJK text counts as boilerplate: a short "OK."
/// or a name alone stays a reply.
const BOILERPLATE_WORDS: usize = 3;

/// Longest reply, in characters, that is judged; a longer one is a real answer and
/// streams without being held.
const BOILERPLATE_CHARS: usize = 200;

/// Some endpoints answer with a canned error in English, worded differently each time.
/// In a corpus written in CJK text the persona never replies without any, so a whole
/// reply with no CJK character and at least [`BOILERPLATE_WORDS`] ASCII words is that
/// boilerplate. In any other corpus nothing can be told apart, so nothing is refused.
pub fn refusal(reply: &str, cjk: bool) -> Refusal {
    let text = reply.trim();
    if !cjk || text.chars().count() > BOILERPLATE_CHARS || text.chars().any(is_cjk) {
        return Refusal::No;
    }
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_ascii_alphabetic()))
        .count();
    if words >= BOILERPLATE_WORDS {
        Refusal::Is
    } else {
        Refusal::Maybe
    }
}

pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3000}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')
}

const FENCE: &str = "```tool_call";

/// The call an emulating endpoint wrote as text: `tool_call` on a line of its own
/// (usually behind a code fence, whose opening some endpoints drop), then a JSON object
/// `{"name": .., "arguments": ..}`. Returns the name, the arguments as JSON text and
/// the words before the block.
pub fn text_call(raw: &str) -> Option<(String, String, String)> {
    let at = raw.find("tool_call")?;
    let before = raw[..at].trim_end_matches('`');
    if !(before.is_empty() || before.ends_with('\n')) {
        return None;
    }
    let tail = &raw[at + "tool_call".len()..];
    let json = &tail[tail.find('{')?..];
    let value = serde_json::Deserializer::from_str(json)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    let name = value.get("name")?.as_str()?.to_owned();
    let args = match value.get("arguments") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "{}".to_owned(),
    };
    Some((name, args, before.trim_end().to_owned()))
}

/// A reply as the user may see it: emulated tool-call blocks the endpoint failed to parse
/// are dropped. A closed block goes whole. An unclosed one is usually a call whose
/// arguments carry the words themselves, so its opening line and the `"text": "` lead-in
/// go and the words stay. A line holding only an empty scene marker is dropped too.
pub fn visible_text(raw: &str, scene: (&str, &str)) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find(FENCE) {
        out.push_str(&rest[..i]);
        let after = &rest[i + FENCE.len()..];
        if let Some(end) = after.find("```") {
            rest = &after[end + 3..];
            continue;
        }
        let after = after.trim_start();
        let line_end = after.find('\n').unwrap_or(after.len());
        let first = &after[..line_end];
        const LEAD: &str = "\"text\"";
        let kept = match first.find(LEAD) {
            Some(k) => {
                let tail = first[k + LEAD.len()..].trim_start();
                let tail = tail.strip_prefix(':').unwrap_or(tail).trim_start();
                let tail = tail.strip_prefix('"').unwrap_or(tail);
                format!("{tail}{}", &after[line_end..])
            }
            None => after[line_end..].to_owned(),
        };
        out.push_str(&kept);
        rest = "";
    }
    out.push_str(rest);
    let out = drop_abandoned_draft(&out, scene);
    let lines: Vec<&str> = out.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if is_markup_line(line) || is_empty_scene(line, scene) {
            continue;
        }
        if let Some((name, closed)) = opening_tag_line(line) {
            if closed {
                continue;
            }
            // A block goes through its closing line; with none, it is left as written
            // rather than swallowing the rest of the reply.
            let close = format!("</{name}>");
            if let Some(n) = lines[i..].iter().position(|l| l.trim() == close) {
                i += n + 1;
                continue;
            }
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_owned()
}

/// A line that opens a UI element a proxy passes through, such as
/// `<ElicitationsGroup message="…">`: a tag whose name starts with an ASCII capital.
/// Returns the name, and whether the element also closes on this line
/// (`<Name …>…</Name>`). Lowercase tags and prose that merely contains `<` do not match.
fn opening_tag_line(line: &str) -> Option<(&str, bool)> {
    let t = line.trim();
    let body = t.strip_prefix('<')?;
    if !body.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let name_end = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .unwrap_or(body.len());
    let name = &body[..name_end];
    let after = &body[name_end..];
    if !(after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with('>')) {
        return None;
    }
    let close = format!("</{name}>");
    if t.ends_with(&close) {
        return Some((name, true));
    }
    // Otherwise the whole line is the one opening tag.
    (t.ends_with('>') && !t.ends_with("/>") && !after.contains('<')).then_some((name, false))
}

/// A line that is a scene marker with nothing inside it.
fn is_empty_scene(line: &str, (open, close): (&str, &str)) -> bool {
    !open.is_empty()
        && line
            .trim()
            .strip_prefix(open)
            .and_then(|t| t.strip_suffix(close))
            .is_some_and(|inner| inner.trim().is_empty())
}

/// Marks that end a sentence: a scene line after one of these is a new beat, not a
/// restart.
const SENTENCE_END: &[char] = &[
    '.', '!', '?', ';', '~', '"', ')', '\u{3002}', '\u{ff01}', '\u{ff1f}', '\u{ff1b}', '\u{2026}',
    '\u{ff5e}', '\u{201d}', '\u{300d}', '\u{300f}', '\u{ff09}',
];

/// How far a scene line reaches: it opens the text, closes on the same line, and a
/// blank line (or the end) follows, as an opening does. Returns the length through the
/// closing mark.
fn scene_line_len(s: &str, (open, close): (&str, &str)) -> Option<usize> {
    let inner = s.strip_prefix(open)?;
    let end = inner.find(close)?;
    if inner[..end].contains('\n') {
        return None;
    }
    let rest = &inner[end + close.len()..];
    let mut lines = rest.splitn(3, '\n');
    let tail_blank = lines.next()?.trim().is_empty();
    let next_blank = lines.next().is_none_or(|l| l.trim().is_empty());
    (tail_blank && next_blank).then_some(open.len() + end + close.len())
}

/// A model sometimes drops a draft mid-sentence and starts the reply over, with a fresh
/// scene line glued to the cut-off words. When the reply opened with a scene line and
/// such a scene line (alone on its line, followed by a blank line) follows words that do
/// not end a sentence, everything before the restart goes. A scene mark after a full
/// stop, one with words after it, or a reply whose only scene line is mid-line is left
/// alone.
fn drop_abandoned_draft<'a>(text: &'a str, scene: (&str, &str)) -> &'a str {
    let (open, close) = scene;
    let text = text.trim_start();
    if open.is_empty() || close.is_empty() || scene_line_len(text, scene).is_none() {
        return text;
    }
    let restart = text.rmatch_indices(open).map(|(i, _)| i).find(|&i| {
        let before = text[..i].trim_end_matches([' ', '\t']);
        i > 0
            && !before.is_empty()
            && !before.ends_with('\n')
            && !before.ends_with(SENTENCE_END)
            && scene_line_len(&text[i..], scene).is_some()
    });
    restart.map_or(text, |i| &text[i..])
}

/// A line that is one self-closing markup tag, such as a chat app's follow-up suggestion
/// (`<FollowUp label="…"/>`) a proxy passes through. Never speech.
fn is_markup_line(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('<')
        && t.ends_with("/>")
        && t[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// The full id a cited id stands for: itself, or the one known id that ends in it after
/// its kind prefix (a model may write `[#hash]` for `[#kind:hash]`).
pub fn resolve_id<'a>(id: &str, known: impl IntoIterator<Item = &'a String>) -> Option<String> {
    let mut hit = None;
    for k in known {
        if k == id {
            return Some(k.clone());
        }
        if k.rsplit_once(':').is_some_and(|(_, tail)| tail == id) {
            if hit.is_some() {
                return None;
            }
            hit = Some(k.clone());
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_closing_hand_off_line_is_split_off() {
        let m = "\u{2192}";
        let (t, n) = split_hand_off("Hi.\n\u{2192} Ann, Bo\n", m);
        assert_eq!(
            (t.as_str(), n),
            ("Hi.", vec!["Ann".to_owned(), "Bo".to_owned()])
        );
        let (t, n) = split_hand_off("\u{2192} \u{201c}x\u{201d}", m);
        assert!(t.is_empty() && n == ["\u{201c}x\u{201d}"]);
        // Not the last line, or no marker: speech.
        let kept = "a \u{2192} b\n\u{2192} Ann\nmore";
        assert_eq!(split_hand_off(kept, m), (kept.to_owned(), vec![]));
        assert_eq!(split_hand_off("plain", m).1, Vec::<String>::new());
        assert_eq!(split_hand_off("x\n\u{2192}", m), ("x".to_owned(), vec![]));
        assert_eq!(split_hand_off("x\n-> Ann", "").1.len(), 0);
        // Written before the citation line: still the hand-off, and the marks stay.
        let (t, n) = split_hand_off("Hi.\n\n\u{2192} Ann\n[#u1, #u2]\n", m);
        assert_eq!((t.as_str(), n), ("Hi.\n[#u1, #u2]", vec!["Ann".to_owned()]));
        let speech = "Hi.\n\u{2192} Ann\n[#u1] and more";
        assert_eq!(split_hand_off(speech, m).1, Vec::<String>::new());
    }

    #[test]
    fn the_stream_holds_back_only_a_possible_hand_off_line() {
        let m = "\u{2192}";
        let at = |s: &str| shown_len(s, m);
        assert_eq!(at("Hello"), 5);
        assert_eq!(at("Hello\n"), 6);
        assert_eq!(at("Hello\n\u{2192}"), 6);
        assert_eq!(at("Hello\n\u{2192} An"), 6);
        assert_eq!(at("Hello\n\u{2192} Ann\n"), 6);
        assert_eq!(
            at("Hello\n\u{2192} Ann\nmore"),
            "Hello\n\u{2192} Ann\nmore".len()
        );
        assert_eq!(at("\u{2192} Ann"), 0);
        // A citation line after the marker line keeps the marker line held.
        assert_eq!(at("Hello\n\u{2192} Ann\n[#u1]"), 6);
        assert_eq!(at("Hello\n\u{2192} Ann\n[#u"), 6);
        assert_eq!(at("Hello\n[#u1]"), "Hello\n[#u1]".len());
        // A multi-character marker is held while it could still complete.
        assert_eq!(shown_len("Hi\n=", "=>"), 3);
        assert_eq!(shown_len("Hi\n=x", "=>"), 5);
    }

    use super::*;

    fn visible(raw: &str) -> String {
        visible_text(raw, ("(", ")"))
    }
    use std::collections::BTreeMap;

    const FW: (&str, &str) = ("\u{ff08}", "\u{ff09}");

    #[test]
    fn a_restarted_opening_drops_the_abandoned_draft() {
        // Full-width marks, a cut-off sentence, then the scene line again.
        let raw = "\u{ff08}takes the report\u{2026}\u{ff09}\n\nThe small drift in this figure \
                   suggests you did not come at night only to check numbers and hear me\
                   \u{ff08}takes the report from the doctor and leafs through it\u{ff09}\n\n\
                   Sit down, Doctor.";
        assert_eq!(
            visible_text(raw, FW),
            "\u{ff08}takes the report from the doctor and leafs through it\u{ff09}\n\nSit down, Doctor."
        );
        assert_eq!(
            visible("(waves)\n\nWell, I was about to say(waves again)\n\nHello."),
            "(waves again)\n\nHello."
        );
    }

    #[test]
    fn a_scene_line_that_is_not_a_restart_is_kept() {
        // After a full stop it is a new beat.
        let a = "(nods)\n\nFine. (smiles)\n\nGo on.";
        assert_eq!(visible(a), a);
        // The only scene line comes mid-line: nothing to restart.
        let b = "I was about to say(smiles)\n\nGo on.";
        assert_eq!(visible(b), b);
        // Words follow the mark: an aside, not a new draft.
        let c = "(nods)\n\nWell, I was(smiles) about to say it.";
        assert_eq!(visible(c), c);
        // Ordinary parentheses are not scene marks.
        let d = "(nods)\n\nIt was fine (mostly) and then(more) I left.";
        assert_eq!(
            visible(d),
            "(nods)\n\nIt was fine (mostly) and then(more) I left."
        );
        // A mark at the start of a later line is not mid-line.
        let e = "(nods)\n\nWell, I was\n(smiles)\n\nGo on.";
        assert_eq!(visible(e), e);
    }

    #[test]
    fn a_reply_without_cjk_text_in_a_cjk_corpus_is_boilerplate() {
        // However the endpoint words it.
        for canned in [
            "I\u{2019}m having a hard time fulfilling your request.",
            "I seem to be encountering an error. Can I try something else for you?",
            "I encountered an error doing what you asked. Could you try again?",
            "Something went wrong with that one",
        ] {
            assert_eq!(refusal(canned, true), Refusal::Is, "{canned}");
        }
        // Short replies and names stay replies, though more text could still change that.
        assert_eq!(refusal("OK.", true), Refusal::Maybe);
        assert_eq!(refusal("Doctor Amiya", true), Refusal::Maybe);
        assert_eq!(refusal("", true), Refusal::Maybe);
        // Any CJK character, anywhere, makes it a reply.
        assert_eq!(
            refusal("I can't help with that. \u{4f60}\u{597d}", true),
            Refusal::No
        );
        assert_eq!(refusal("\u{597d}\u{7684}", true), Refusal::No);
        // A long reply is not judged.
        let long = format!("I can't help with that. {}", "x".repeat(200));
        assert_eq!(refusal(&long, true), Refusal::No);
        // Digits and symbols are not words.
        assert_eq!(refusal("12 34 56 !!", true), Refusal::Maybe);
    }

    #[test]
    fn nothing_is_boilerplate_in_a_corpus_that_is_not_cjk() {
        assert_eq!(
            refusal("I seem to be encountering an error.", false),
            Refusal::No
        );
    }

    #[test]
    fn a_text_call_needs_its_marker_on_a_line_of_its_own() {
        let (name, args, lead) = text_call(
            "Noted.\n```tool_call\n{\"name\": \"wrapup\", \"arguments\": {\"a\": 1}}\n```",
        )
        .unwrap();
        assert_eq!(
            (name.as_str(), args.as_str(), lead.as_str()),
            ("wrapup", "{\"a\":1}", "Noted.")
        );
        assert!(text_call("use a tool_call {\"name\": \"x\"}").is_none());
        assert!(text_call("```tool_call\n{broken").is_none());
    }

    #[test]
    fn tool_call_blocks_are_not_shown() {
        assert_eq!(
            visible("hi\n```tool_call\n{\"name\": \"search\"}\n```\nbye"),
            "hi\n\nbye"
        );
        assert_eq!(
            visible("```tool_call\n{\"name\": \"report\", \"arguments\": {\"text\": \"words\nmore"),
            "words\nmore"
        );
        assert_eq!(visible("plain"), "plain");
        assert_eq!(
            visible("Yes.\n<FollowUp label=\"more\" query=\"q\"/>\nGo."),
            "Yes.\nGo."
        );
        assert_eq!(visible("a < b and c/>"), "a < b and c/>");
    }

    #[test]
    fn an_empty_scene_line_is_dropped() {
        assert_eq!(visible("(  )\nHello.\n()\n(waves)"), "Hello.\n(waves)");
        assert_eq!(visible_text("a\n\u{2a}\u{2a}\nb", ("*", "*")), "a\nb");
        assert_eq!(visible_text("a ()\nb", ("(", ")")), "a ()\nb");
    }

    #[test]
    fn multi_line_ui_elements_are_dropped() {
        let raw = "Next.\n<ElicitationsGroup message=\"\u{63a5}\u{4e0b}\u{6765}\u{ff1f}\">\n</ElicitationsGroup>\nDone.";
        assert_eq!(visible(raw), "Next.\nDone.");
        let nested = "A\n<Group title=\"t\">\n  <Choice label=\"x\"/>\n  <Choice label=\"y\">go</Choice>\n</Group>\nB";
        assert_eq!(visible(nested), "A\nB");
        assert_eq!(visible("A\n<Note kind=\"k\">text</Note>\nB"), "A\nB");
        assert_eq!(visible("A\n  <Group>\n  </Group>  "), "A");
    }

    #[test]
    fn ordinary_angle_brackets_and_lowercase_tags_stay() {
        for keep in [
            "if a < b then <c> stays",
            "<Name> is a placeholder in the text",
            "<div>\nhtml\n</div>",
            "<b>bold</b>",
            "x <Group> y",
            "<Group>\nunclosed block stays",
            "<Groupie>\nnot closed by </Group>",
        ] {
            assert_eq!(visible(keep), keep, "{keep}");
        }
    }

    #[test]
    fn a_block_is_not_closed_by_another_name() {
        assert_eq!(visible("<A>\n</B>\nrest"), "<A>\n</B>\nrest");
    }

    #[test]
    fn a_bare_hash_resolves_to_the_one_id_that_ends_in_it() {
        let known = ["dialogue:ab12".to_string(), "voice:cd34".to_string()];
        assert_eq!(resolve_id("ab12", &known).as_deref(), Some("dialogue:ab12"));
        assert_eq!(
            resolve_id("voice:cd34", &known).as_deref(),
            Some("voice:cd34")
        );
        assert_eq!(resolve_id("ef56", &known), None);
        let twice = ["a:x1".to_string(), "b:x1".to_string()];
        assert_eq!(resolve_id("x1", &twice), None);
    }

    #[test]
    fn host_meta_only_in_its_own_block() {
        let w = Wording::new(&BTreeMap::new());
        let n = Names::new("<U>", Some("Rhea"), &w);
        let ctx = Ctx {
            wording: &w,
            names: &n,
            scene: ("*", "*"),
        };
        let layers = HostLayers {
            in_world: "IN <U>".into(),
            meta: "META".into(),
        };
        let a = block_a_host(&ctx, "H", &layers, Frontend::Terminal);
        assert!(a.contains("IN Rhea") && a.contains("META"));
        let b = persona_section(
            &ctx,
            &Member::Host {
                name: "H",
                in_world: &layers.in_world,
            },
        );
        assert!(b.contains("IN Rhea") && !b.contains("META"));
        assert_eq!(entry(&["hi", "", "  ", "ctx"]), "hi\n\nctx");
    }

    #[test]
    fn the_guide_is_the_hosts_and_covers_only_its_front_end() {
        let w = Wording::new(&BTreeMap::new());
        let n = Names::new("<U>", Some("Rhea"), &w);
        let ctx = Ctx {
            wording: &w,
            names: &n,
            scene: ("*", "*"),
        };
        let layers = HostLayers {
            in_world: String::new(),
            meta: String::new(),
        };
        let terminal = block_a_host(&ctx, "H", &layers, Frontend::Terminal);
        let app = block_a_host(&ctx, "H", &layers, Frontend::App);
        assert!(terminal.contains("/talk") && !terminal.contains("Roster"));
        assert!(app.contains("Roster") && !app.contains("/talk"));
        // Both say what the host may claim: only what its tools did.
        for text in [&terminal, &app] {
            assert!(
                text.contains("no queue") && text.contains("chase"),
                "{text}"
            );
        }
        // No key falls back to its own name: every line is worded.
        assert!(!terminal.contains("harness.guide") && !app.contains("harness.guide"));
        // Neither a person's persona section nor block A for persons carries it.
        let person = Persona {
            system: "SYS".into(),
            tone: String::new(),
            lines: String::new(),
        };
        let section = persona_section(
            &ctx,
            &Member::Person {
                name: "P",
                persona: &person,
            },
        );
        for text in [section, block_a(&ctx, false), block_a(&ctx, true)] {
            assert!(
                !text.contains("/talk") && !text.contains("Roster"),
                "{text}"
            );
        }
    }

    #[test]
    fn the_bio_goes_in_block_a_as_typed_and_only_when_there_is_one() {
        let w = Wording::new(&BTreeMap::new());
        let ctx_for = |names: &Names| {
            block_a(
                &Ctx {
                    wording: &w,
                    names,
                    scene: ("*", "*"),
                },
                false,
            )
        };
        let none = Names::new("<U>", Some("Rhea"), &w);
        assert!(!ctx_for(&none).contains("about themselves"));
        let blank = Names::new("<U>", Some("Rhea"), &w).with_about(Some("  "));
        assert_eq!(ctx_for(&blank), ctx_for(&none));
        // Its own braces and the placeholder are not substituted into.
        let set = Names::new("<U>", Some("Rhea"), &w).with_about(Some("Calls me {user} <U>"));
        let a = ctx_for(&set);
        assert!(
            a.contains("Rhea says this about themselves: Calls me {user} <U>"),
            "{a}"
        );
        // After the rules, before the output format: a stable place in the cached block.
        assert!(a.find("themselves").unwrap() > a.find("talks with").unwrap());
        assert!(a.find("themselves").unwrap() < a.find("speech").unwrap());
    }
}
