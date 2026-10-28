//! Who a group message speaks to, asked of the model once per message when the cheap
//! rules cannot tell: one small request outside the group's log (so its cached prefix is
//! untouched), sent whole with a response schema. An unusable answer means "no answer"
//! and the caller falls back to the plain rule.

use llm::{Message, Request, SystemBlock};
use serde_json::{Value, json};
use world::{Actor, ChannelId, MessageId, PersonId, message};

use crate::agent::Agent;
use crate::error::Result;
use crate::session::LogKind;
use crate::wrapup_json;

/// What the model said about the audience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Audience {
    Everyone,
    Some(Vec<PersonId>),
}

/// Marker the model uses for the whole group.
const ALL: &str = "all";

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "addressed": {
                "type": "array",
                "items": {"type": "string"},
                "description": "ids of the members the message speaks to, or [\"all\"] for everyone"
            }
        },
        "required": ["addressed"],
        "additionalProperties": false
    })
}

/// The audience in a reply (a JSON object, possibly fenced): `all`, or the listed ids
/// that are members. Unknown ids are dropped; nothing usable is `None`.
pub(crate) fn parse(reply: &str, members: &[PersonId]) -> Option<Audience> {
    let doc = wrapup_json::unfence(reply.trim());
    let list = serde_json::from_str::<Value>(doc).ok()?;
    let list = list.get("addressed")?.as_array()?;
    let mut ids = Vec::new();
    for v in list.iter().filter_map(Value::as_str) {
        if v.eq_ignore_ascii_case(ALL) {
            return Some(Audience::Everyone);
        }
        if let Some(m) = members.iter().find(|m| m.as_str() == v)
            && !ids.contains(m)
        {
            ids.push(m.clone());
        }
    }
    (!ids.is_empty()).then_some(Audience::Some(ids))
}

/// The member a name in a hand-off line stands for: a display name or id (case aside),
/// else an alias that names exactly one member.
pub(crate) fn member_named(
    name: &str,
    members: &[(PersonId, String)],
    by_alias: impl Fn(&str) -> Vec<String>,
) -> Option<PersonId> {
    if let Some((p, _)) = members
        .iter()
        .find(|(p, d)| d.eq_ignore_ascii_case(name) || p.as_str().eq_ignore_ascii_case(name))
    {
        return Some(p.clone());
    }
    let hit: Vec<&PersonId> = members
        .iter()
        .map(|(p, _)| p)
        .filter(|p| by_alias(name).iter().any(|a| a == p.as_str()))
        .collect();
    match hit[..] {
        [one] => Some(one.clone()),
        _ => None,
    }
}

impl Agent {
    /// Who a reply's hand-off line gives the turn to, in the order named: members of the
    /// group who may speak (a persona, not disabled), each once. The user, the host,
    /// strangers and anyone in `skip` are left out.
    pub(crate) fn hand_off_targets(
        &self,
        channel: ChannelId,
        names: &[String],
        skip: &[PersonId],
    ) -> Result<Vec<PersonId>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let present = {
            let conn = self.db();
            let ch = world::channel::get(&conn, channel)?;
            world::bond::filter_disabled(&conn, ch.persons())?
        };
        let mut members = Vec::new();
        for p in self.corpus.personas.filter(present) {
            let display = self.display(p.as_str())?;
            members.push((p, display));
        }
        let ix = self.ix();
        let by_alias = |n: &str| ix.folio().persons_for_alias(n).unwrap_or_default();
        let mut out: Vec<PersonId> = Vec::new();
        for n in names {
            if let Some(p) = member_named(n, &members, by_alias)
                && !skip.contains(&p)
                && !out.contains(&p)
            {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Ask who `text` speaks to among `members`, given the lines before it. `None` when
    /// the call fails or the answer names nobody; the user's turn never depends on it.
    pub(crate) async fn audience(
        &self,
        channel: ChannelId,
        before: MessageId,
        text: &str,
        members: &[PersonId],
    ) -> Result<Option<Audience>> {
        let names = self.names_now(&self.db())?;
        let who = |a: &Actor| -> Result<String> {
            Ok(match a {
                Actor::Person(p) => self.display(p.as_str())?,
                _ => names.user().to_owned(),
            })
        };
        let mut roster = Vec::new();
        for m in members {
            roster.push(format!("{}: {}", m, self.display(m.as_str())?));
        }
        let lines = self.config.agent.group.classify_lines;
        let recent = message::history(&self.db(), channel, Some(before), lines)?;
        let mut shown = Vec::new();
        for m in recent {
            shown.push(format!("{}: {}", who(&m.author)?, names.sub(&m.text)));
        }
        let schema = schema();
        let shape = wrapup_json::example(&schema);
        let instruction = self.corpus.wording.fill(
            "harness.address",
            &names,
            &[
                ("members", &roster.join("\n")),
                ("recent", &shown.join("\n")),
                ("message", text),
                ("shape", &shape),
            ],
        );
        let (role, ep) = self.role_for(LogKind::Person)?;
        let req = Request {
            system: vec![SystemBlock {
                text: instruction,
                cache_mark: false,
            }],
            messages: vec![Message::user(text)],
            output_schema: Some(schema),
            ..Request::default()
        };
        // Whole reply or nothing: a failure here only costs the refinement.
        let Ok(reply) = self.llm.complete(&ep, &req).await else {
            return Ok(None);
        };
        let model = self.shape_for(&role, LogKind::Person, &names).model;
        crate::meter::record(
            &self.db(),
            self.now().ms,
            world::Shape::Group,
            Some(channel),
            &model,
            reply.usage,
            role.prices(),
            role.cache_write_ratio(),
            role.per_request(),
            &self.config.world.cost,
            &self.config.agent.cost,
        )?;
        self.emit_quota()?;
        Ok(parse(&reply.text(), members))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<PersonId> {
        v.iter().map(|s| PersonId::from(*s)).collect()
    }

    #[test]
    fn hand_off_names_resolve_to_members_only() {
        let m = vec![
            (PersonId::from("p1"), "Ann".to_owned()),
            (PersonId::from("p2"), "Bo".to_owned()),
        ];
        let alias = |n: &str| match n {
            "Annie" => vec!["p1".to_owned()],
            "Both" => vec!["p1".to_owned(), "p2".to_owned()],
            _ => Vec::new(),
        };
        assert_eq!(member_named("ann", &m, alias), Some(PersonId::from("p1")));
        assert_eq!(member_named("P2", &m, alias), Some(PersonId::from("p2")));
        assert_eq!(member_named("Annie", &m, alias), Some(PersonId::from("p1")));
        assert_eq!(member_named("Both", &m, alias), None);
        assert_eq!(member_named("Cy", &m, alias), None);
    }

    #[test]
    fn replies_are_read_strictly() {
        let m = ids(&["a", "b", "c"]);
        let all = Some(Audience::Everyone);
        assert_eq!(parse(r#"{"addressed": ["all"]}"#, &m), all);
        assert_eq!(parse("```json\n{\"addressed\": [\"ALL\"]}\n```", &m), all);
        assert_eq!(
            parse(r#"{"addressed": ["b", "x", "b"]}"#, &m),
            Some(Audience::Some(ids(&["b"])))
        );
        assert_eq!(parse(r#"{"addressed": ["c", "all"]}"#, &m), all);
        assert_eq!(parse(r#"{"addressed": []}"#, &m), None);
        assert_eq!(parse(r#"{"addressed": ["x"]}"#, &m), None);
        assert_eq!(parse(r#"{"who": ["a"]}"#, &m), None);
        assert_eq!(parse("everyone", &m), None);
    }
}
