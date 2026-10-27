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

/// Block A: the global static text, identical for every person of this user.
pub fn block_a(ctx: &Ctx<'_>) -> String {
    [
        ctx.fill("harness.rules", &[]),
        ctx.fill(
            "harness.output",
            &[("open", ctx.scene.0), ("close", ctx.scene.1)],
        ),
        ctx.fill("harness.citation", &[]),
    ]
    .join("\n\n")
}

/// Block A of the host's own session: the same, then both of its layers. Only this
/// block ever carries the meta layer.
pub fn block_a_host(ctx: &Ctx<'_>, host_name: &str, layers: &HostLayers) -> String {
    let mut parts = vec![block_a(ctx), heading(ctx, host_name)];
    for layer in [&layers.in_world, &layers.meta] {
        if !layer.trim().is_empty() {
            parts.push(ctx.names.sub(layer.trim()).into_owned());
        }
    }
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

/// Every id cited in a text, in order. A bracket holds one id or a comma-separated list
/// (`[#a]`, `[#a, #b]`, `[#a,b]`); a bracket with any malformed item cites nothing.
pub fn ids_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[#") {
        rest = &rest[i + 2..];
        let Some(end) = rest.find(']') else { break };
        if let Some(ids) = id_list(&rest[..end]) {
            out.extend(ids);
            rest = &rest[end + 1..];
        }
    }
    out
}

/// The ids of one bracket's inside (after its first `#`).
fn id_list(inside: &str) -> Option<Vec<String>> {
    if inside.starts_with(char::is_whitespace) {
        return None;
    }
    inside
        .split(',')
        .map(|item| {
            let id = item.trim().trim_start_matches('#');
            (!id.is_empty() && id.len() <= 128 && !id.contains(char::is_whitespace))
                .then(|| id.to_owned())
        })
        .collect()
}

const FENCE: &str = "```tool_call";

/// A reply as the user may see it: emulated tool-call blocks the endpoint failed to parse
/// are dropped. A closed block goes whole. An unclosed one is usually a call whose
/// arguments carry the words themselves, so its opening line and the `"text": "` lead-in
/// go and the words stay.
pub fn visible_text(raw: &str) -> String {
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
    let lines: Vec<&str> = out.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if is_markup_line(line) {
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
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ids_are_found_and_nonsense_skipped() {
        assert_eq!(
            ids_in("a [#u1] b [#m12] c [# x] [#] [#u1]"),
            ["u1", "m12", "u1"]
        );
    }

    #[test]
    fn lists_in_one_bracket_are_read() {
        assert_eq!(
            ids_in("a [#x:1,#y:2] b [#p:3, #q:4] c [#r:5, s:6] [#bad, ] [#z:7]"),
            ["x:1", "y:2", "p:3", "q:4", "r:5", "s:6", "z:7"]
        );
    }

    #[test]
    fn tool_call_blocks_are_not_shown() {
        assert_eq!(
            visible_text("hi\n```tool_call\n{\"name\": \"search\"}\n```\nbye"),
            "hi\n\nbye"
        );
        assert_eq!(
            visible_text(
                "```tool_call\n{\"name\": \"report\", \"arguments\": {\"text\": \"words\nmore"
            ),
            "words\nmore"
        );
        assert_eq!(visible_text("plain"), "plain");
        assert_eq!(
            visible_text("Yes.\n<FollowUp label=\"more\" query=\"q\"/>\nGo."),
            "Yes.\nGo."
        );
        assert_eq!(visible_text("a < b and c/>"), "a < b and c/>");
    }

    #[test]
    fn multi_line_ui_elements_are_dropped() {
        let raw = "Next.\n<ElicitationsGroup message=\"\u{63a5}\u{4e0b}\u{6765}\u{ff1f}\">\n</ElicitationsGroup>\nDone.";
        assert_eq!(visible_text(raw), "Next.\nDone.");
        let nested = "A\n<Group title=\"t\">\n  <Choice label=\"x\"/>\n  <Choice label=\"y\">go</Choice>\n</Group>\nB";
        assert_eq!(visible_text(nested), "A\nB");
        assert_eq!(visible_text("A\n<Note kind=\"k\">text</Note>\nB"), "A\nB");
        assert_eq!(visible_text("A\n  <Group>\n  </Group>  "), "A");
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
            assert_eq!(visible_text(keep), keep, "{keep}");
        }
    }

    #[test]
    fn a_block_is_not_closed_by_another_name() {
        assert_eq!(visible_text("<A>\n</B>\nrest"), "<A>\n</B>\nrest");
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
        let a = block_a_host(&ctx, "H", &layers);
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
}
