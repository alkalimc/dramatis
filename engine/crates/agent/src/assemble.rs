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
            if !persona.fallback.trim().is_empty() {
                let line = ctx.names.sub(persona.fallback.trim()).into_owned();
                parts.push(ctx.fill("harness.fallback", &[("name", name), ("line", &line)]));
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
/// are added to it. Low confidence sends no material at all, only the plain statement
/// (and, for a person, that a colleague can be brought in).
pub fn material(
    ctx: &Ctx<'_>,
    resp: &SearchResponse,
    seen: &mut HashSet<String>,
    offer_join: bool,
) -> Material {
    let level = resp.confidence.level;
    if level == Level::Low {
        let mut text = ctx.fill("harness.nothing_found", &[]);
        if offer_join {
            text.push('\n');
            text.push_str(&ctx.fill("harness.join_available", &[]));
        }
        return Material {
            text,
            level: Some(level),
            ..Material::default()
        };
    }
    let mut out = Material {
        level: Some(level),
        ..Material::default()
    };
    let mut blocks = Vec::new();
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
    for Hit {
        item, neighbours, ..
    } in &resp.hits
    {
        match item {
            Item::Unit(u) => {
                push_unit(u, &mut out, &mut blocks, seen);
                for n in neighbours {
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

/// Every `[#id]` in a text, in order.
pub fn ids_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[#") {
        rest = &rest[i + 2..];
        let Some(end) = rest.find(']') else { break };
        let id = &rest[..end];
        if !id.is_empty() && id.len() <= 128 && !id.contains(char::is_whitespace) {
            out.push(id.to_owned());
            rest = &rest[end + 1..];
        }
    }
    out
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
