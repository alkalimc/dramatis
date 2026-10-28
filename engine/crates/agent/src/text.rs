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
    ("host.name", "Host"),
    (
        "host.quota_exhausted",
        "The quota for this period is used up until {when}. The archive stays available.",
    ),
    (
        "host.rate_limited",
        "The language model service is rate limited right now. Your message is kept; \
         send it again later.",
    ),
    (
        "harness.introduce",
        "{user} opens the app for the first time. Introduce yourself in a few lines.",
    ),
    (
        "harness.rules",
        "{user} talks with the people of this world through messages. Each reply is spoken \
         by one of them, as themselves. When a person does not know something, they react \
         the way that person would: brush it off, ask back, go by impression or change the \
         subject. They never talk about whether they have evidence, records or sources.",
    ),
    (
        "harness.output",
        "A line wrapped in {open} and {close} describes only the speaker's own expression or \
         action, in the third person, writing {user} by name, never \"you\", wherever the user is \
         meant; a scene line is short, one line, wholly inside the markers; every other line is \
         speech. Speak as in person: usually two to five sentences, longer when the topic \
         deserves it, but still words one person says aloud, never several paragraphs of \
         narration or explanation; at most one scene line per reply. Do not end every reply \
         with a question. The speaker's name is \
         never written before a reply.",
    ),
    (
        "harness.citation",
        "Reference passages arrive marked [#id]. A passage already shown appears again only as \
         its [#id]. Write the reply as ordinary speech with no marks inside it. Only if the \
         reply really drew on specific passages, end it with one final line listing their \
         [#id] marks, at most two, and nothing else on that line. A line is never added or \
         shaped to have something to cite; most replies cite nothing, and tone and common \
         knowledge are never cited.",
    ),
    ("harness.persona", "## {name}"),
    (
        "harness.lines",
        "Lines {name} has said, only to get the voice. Do not repeat them:",
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
    ("harness.host_offline", "{when}"),
    (
        "harness.context.turns",
        "[{date} {time} · {turns} turns left on this request]",
    ),
    ("harness.nothing_found", "Nothing came back for this."),
    (
        "harness.low_match",
        "These passages may be unrelated to the question. Use one only if it really fits.",
    ),
    ("harness.speaker", "{name} answers."),
    ("harness.interject", "{name} may add one line."),
    ("harness.chorus", "{name} answers too, after the others."),
    ("harness.collective", "everyone everybody"),
    ("harness.next_mark", "→"),
    (
        "harness.next",
        "In a group, a member may hand the turn on. Only if the reply really calls for one \
         particular member to answer next, end it with one final line that starts with {mark} \
         followed by the names of those members, separated by commas, and nothing else on \
         that line. If a citation line is also written, this line comes last. Most replies \
         hand the turn to nobody. {user} and the host are never named there.",
    ),
    (
        "harness.address",
        "Decide who the last message in a group chat speaks to. Members:\n{members}\n\n\
         Lines before it:\n{recent}\n\nMessage: {message}\n\nReply with one JSON object \
         only, no code fence, using exactly these keys: {shape}",
    ),
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
        "The conversation is closing. Step out of the scene and do not answer as the \
         character. Give only the wrap-up: what happened that is worth remembering, and any \
         promises with their dates. Use the wrapup call if it is available; otherwise reply \
         with just the JSON object the schema describes.",
    ),
    (
        "harness.wrapup.summary",
        "The conversation moves to a new log. Step out of the scene and do not answer as the \
         character. Give only the wrap-up: what happened that is worth remembering, any \
         promises with their dates, and a summary of the conversation so far. Use the wrapup \
         call if it is available; otherwise reply with just the JSON object the schema \
         describes.",
    ),
    (
        "harness.wrapup.shape",
        "Reply with one JSON object only, no code fence, using exactly these keys: {shape}",
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
    (
        "harness.error.wrapup_only",
        "wrapup is only for closing a conversation.",
    ),
    ("harness.error.not_asked", "Only when {user} asks for it."),
    ("harness.error.unavailable", "Nobody suitable is available."),
    ("harness.error.not_found", "Not found: {id}"),
    (
        "harness.error.arguments",
        "Could not read the arguments: {detail}",
    ),
    ("harness.error.refused", "Refused: {detail}"),
    ("harness.done", "Done."),
    ("harness.asked", "{name} is looking into it."),
    ("harness.group_created", "Group {id} created."),
    ("harness.remembered", "Noted."),
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

    /// `get` with `{field}` placeholders filled, `{user}` included. The template's own
    /// name placeholders are replaced before the fields go in, so a field (a question the
    /// user typed) reaches the model exactly as written.
    pub fn fill(&self, key: &str, names: &Names, fields: &[(&str, &str)]) -> String {
        let mut out = names.sub(self.get(key)).replace("{user}", names.user());
        for (k, v) in fields {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
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

    /// A segment's names: what its bytes already say the user is called.
    pub fn fixed(placeholder: &str, user: &str) -> Self {
        Self {
            placeholder: placeholder.to_owned(),
            user: user.to_owned(),
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
