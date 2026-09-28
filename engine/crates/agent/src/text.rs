//! Model-visible wording and the user's name.
//!
//! Every sentence the harness puts in a session comes from the corpus's `wording`
//! (keys `harness.*`); the neutral English below is only the fallback for a corpus that
//! lacks a key. The corpus stores the user's name as a placeholder (manifest
//! `user_placeholder`); [`Names`] replaces it right before text enters a log, never in
//! the corpus itself.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// Fallbacks for every key the harness reads. `{field}` placeholders are filled by the
/// caller; `{user}` is always the user's name (or the corpus's word for "you").
const DEFAULTS: &[(&str, &str)] = &[
    ("user.you", "you"),
    (
        "harness.rules",
        "{user} talks with the people of this world through messages. Each reply is spoken \
         by one of them, as themselves.",
    ),
    (
        "harness.output",
        "A line wrapped in {open} and {close} describes the speaker's expression or action \
         in the third person; every other line is speech. Scene lines are optional. The \
         speaker's name is never written before a reply.",
    ),
    (
        "harness.citation",
        "Archive passages arrive marked [#id]. A passage already shown appears again only as \
         its [#id]. Writing [#id] in a reply cites that passage.",
    ),
    ("harness.persona", "## {name}"),
    (
        "harness.fallback",
        "When unsure, {name} says something like: {line}",
    ),
    ("harness.tone.guarded", "{name} is wary of {user}."),
    ("harness.tone.normal", "{name} knows {user}."),
    ("harness.tone.close", "{name} is close to {user}."),
    ("harness.tone.deep", "{name} and {user} share a deep bond."),
    ("harness.topic", "Topic: {topic}"),
    ("harness.memories", "{name} remembers:"),
    ("harness.memories.shared", "Everyone here remembers:"),
    ("harness.summary", "Earlier in this conversation: {summary}"),
    ("harness.recent", "The last messages:"),
    ("harness.context", "[{date} {time}]"),
    (
        "harness.context.turns",
        "[{date} {time} · {turns} turns left on this request]",
    ),
    ("harness.nothing_found", "The archive has nothing on this."),
    (
        "harness.join_available",
        "A colleague may know more: request_join can bring one in.",
    ),
    ("harness.speaker", "{name} answers."),
    ("harness.interject", "{name} may add one line."),
    ("harness.joined", "{name} joins the conversation."),
    (
        "harness.ask",
        "{user} asks {name} to look into this: {question}",
    ),
    (
        "harness.must_report",
        "This is the last turn on this request: answer it with report now.",
    ),
    (
        "harness.wrapup",
        "The conversation is closing. Call wrapup with what is worth remembering from it.",
    ),
    (
        "harness.wrapup.summary",
        "The conversation moves to a new log. Call wrapup with what is worth remembering, \
         and a summary of the conversation so far.",
    ),
    ("harness.opening", "{name} speaks first."),
    ("harness.opening.commitment", "A promise is due: {text}"),
    (
        "harness.opening.conclusion",
        "{user} wrote down a conclusion: {text}",
    ),
    ("harness.opening.pinned", "{user} pinned a question: {text}"),
    ("harness.opening.birthday", "Today is {user}'s birthday."),
    (
        "harness.opening.memory",
        "Something {name} remembers about {user}: {text}",
    ),
    ("harness.opening.words", "{user} said recently: {text}"),
    ("harness.digest", "Since {user} last looked:"),
    ("harness.digest.messaged", "- {name} sent messages."),
    (
        "harness.digest.replied",
        "- {name} answered the request: {question}",
    ),
    (
        "harness.digest.commitment_due",
        "- A promise with {name} is due: {text}",
    ),
    ("harness.digest.came_by", "- {name} came by."),
    ("harness.digest.birthday", "- Today is {name}'s birthday."),
];

/// The corpus's wording with the harness fallbacks under it.
#[derive(Debug, Clone, Default)]
pub struct Wording {
    entries: BTreeMap<String, String>,
}

impl Wording {
    pub fn new(corpus: &BTreeMap<String, String>) -> Self {
        Self {
            entries: corpus.clone(),
        }
    }

    /// The corpus's value, else the fallback, else the key itself (so a missing key is
    /// visible in a log rather than silently empty).
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.entries
            .get(key)
            .map(String::as_str)
            .or_else(|| DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
            .unwrap_or(key)
    }

    /// `get` with `{field}` placeholders filled, `{user}` included.
    pub fn fill(&self, key: &str, names: &Names, fields: &[(&str, &str)]) -> String {
        let mut out = self.get(key).to_owned();
        for (k, v) in fields {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out = out.replace("{user}", names.user());
        names.sub(&out).into_owned()
    }
}

/// How the user is named in model-visible text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    placeholder: String,
    user: String,
}

impl Names {
    /// `name` is the user's setting; without one the corpus's `user.you` stands in.
    pub fn new(placeholder: &str, name: Option<&str>, wording: &Wording) -> Self {
        let user = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| wording.get("user.you"))
            .to_owned();
        Self {
            placeholder: placeholder.to_owned(),
            user,
        }
    }

    /// What the placeholder becomes.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Replace every placeholder. A corpus without one is returned as is.
    pub fn sub<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.placeholder.is_empty() || !text.contains(&self.placeholder) {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(text.replace(&self.placeholder, &self.user))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wording(pairs: &[(&str, &str)]) -> Wording {
        Wording::new(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn placeholder_becomes_name_or_the_corpus_word_for_you() {
        let w = wording(&[("user.you", "Doc")]);
        let named = Names::new("<U>", Some("Rhea"), &w);
        assert_eq!(named.sub("Dr.<U>, hello <U>"), "Dr.Rhea, hello Rhea");
        let anonymous = Names::new("<U>", Some("  "), &w);
        assert_eq!(anonymous.sub("Dr.<U>"), "Dr.Doc");
        let none = Names::new("", Some("Rhea"), &w);
        assert!(matches!(none.sub("Dr.<U>"), Cow::Borrowed(_)));
    }

    #[test]
    fn corpus_wording_wins_and_fields_fill() {
        let w = wording(&[("harness.topic", "About {topic}, <U>")]);
        let n = Names::new("<U>", Some("Rhea"), &w);
        assert_eq!(
            w.fill("harness.topic", &n, &[("topic", "tea")]),
            "About tea, Rhea"
        );
        assert_eq!(
            w.fill("harness.opening.birthday", &n, &[]),
            "Today is Rhea's birthday."
        );
        assert_eq!(w.get("no.such.key"), "no.such.key");
    }
}
