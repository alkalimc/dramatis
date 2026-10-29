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
        "host.intro.terminal",
        "Welcome. You can talk to anyone here with /talk <name>, start a group with /group, \
         and ask me for anything: finding someone who knows a subject, or a summary of what \
         changed. Requests, memories, saves and settings all have commands; /help lists them, \
         and I can explain any of them.",
    ),
    (
        "host.event.frame",
        "Something small happened on the ship today, if you would like to hear it.\n\n{title}\n{text}\
         |A little news from the ship, whenever you have a moment.\n\n{title}\n{text}\
         |Just so you know, there was a small happening on board today.\n\n{title}\n{text}",
    ),
    (
        "host.intro.app",
        "Welcome. You can talk to anyone on the Roster, start a group, and ask me for anything: \
         finding someone who knows a subject, or a summary of what changed. Requests, memories, \
         saves and settings each have a page; ask me and I will explain any of them.",
    ),
    (
        "harness.host.manner",
        "You assist {user}. Your voice is warm, calm and brief. You lay out options and \
         {user} picks; whether {user} acts, and when, is {user}'s alone and neither is better. \
         You set no deadlines, count no time down, say nothing about how often {user} looks, \
         and suspect no one: when records differ you describe what you see without guessing \
         why. When nothing is new you say so in a line, or stay quiet.",
    ),
    (
        "harness.guide.intro",
        "{user} was welcomed with these words; tell it again, or expand on it, when asked how \
         things work: {intro}",
    ),
    (
        "harness.picture_unseen",
        "{user} attached a picture here, but this channel cannot show pictures, so it \
         could not be seen.",
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
         [#id] marks, at most three, and nothing else on that line. A line is never added or \
         shaped to have something to cite; most replies cite nothing, and tone and common \
         knowledge are never cited.",
    ),
    (
        "harness.about",
        "{user} says this about themselves: {about}",
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
    ("harness.context", "[{date} {weekday} {time}, {part}]"),
    (
        "harness.context.gap",
        "({user}'s previous message was {span} ago)",
    ),
    ("span.hours", "{n} hours"),
    ("span.days", "{n} days"),
    ("weekday.mon", "Monday"),
    ("weekday.tue", "Tuesday"),
    ("weekday.wed", "Wednesday"),
    ("weekday.thu", "Thursday"),
    ("weekday.fri", "Friday"),
    ("weekday.sat", "Saturday"),
    ("weekday.sun", "Sunday"),
    ("daypart.night", "night"),
    ("daypart.morning", "morning"),
    ("daypart.noon", "midday"),
    ("daypart.afternoon", "afternoon"),
    ("daypart.evening", "evening"),
    ("harness.host_offline", "{when}"),
    (
        "harness.context.turns",
        "[{date} {weekday} {time}, {part} · {turns} turns left on this request]",
    ),
    (
        "harness.task",
        "Request {id} to {name} ({question}): {state}",
    ),
    ("harness.task.active", "still open, {turns} turns left"),
    (
        "harness.task.failed",
        "it did not run and no answer is coming",
    ),
    (
        "harness.task.interrupted",
        "it stopped before it finished and no answer is coming",
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
    ("harness.find_mark", "⇒ find:"),
    ("harness.group_mark", "⇒ group:"),
    (
        "harness.propose",
        "You can offer {user} a choice through the host, and nothing happens until {user} \
         takes it; {user} sees it under your reply and decides, so never say you sent or \
         arranged anything. To offer someone who knows a subject, end the reply with one \
         final line that starts with {find} followed by the subject in plain words; the \
         person found is told that subject as the reason. To offer a group with colleagues \
         from the roster, end it with one final line that starts with {group} followed by \
         their names, separated by commas, and at most four, then a bar and the purpose of \
         the group in a few words (at most sixty characters), which every member is told. \
         Use at most one such line per reply, and only when {user} would gain from it. If a \
         citation line is also written, these lines come last.",
    ),
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
    (
        "harness.opening.event",
        "Something happened on the ship today that you are part of: {text}",
    ),
    (
        "harness.event",
        "Something happened on the ship today, and you may mention it if it comes up \
         naturally: {text}",
    ),
    (
        "harness.event.ask",
        "Make up one small happening on the ship for today, {date}, for {user} to hear about. \
         Keep it everyday and small, something people mention over a meal, told in two to \
         four sentences, set in the world as it is. It involves one to three of the people \
         listed below, and may suggest that a few of them gather for a reason, or that {user} \
         talk to someone about it.\n\nA theme for today: {topic}\n\nMaterial from the \
         world, so that it fits the place:\n{material}\n\nPeople it may involve (id: \
         name):\n{people}\n{extra}\n\nReply with a title, the text, the ids of the people \
         involved, a gathering (its members' ids and its purpose) or null, and the ids of \
         people {user} might talk to.",
    ),
    (
        "harness.event.recent",
        "Topics of the groups {user} started lately:\n{recent}",
    ),
    ("events.fact", "{title}: {text}"),
    (
        "events.topics",
        "training|the canteen|supplies|the medical bay|the weather|a festival|someone's hobby",
    ),
    ("harness.digest", "Since {user} last looked:"),
    (
        "harness.digest.none",
        "Nothing has changed since {user} last looked.",
    ),
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
    (
        "harness.error.once",
        "One suggestion is enough for a reply.",
    ),
    (
        "harness.channels",
        "The conversations {user} has, most recent first:",
    ),
    ("harness.channels.none", "No conversation yet."),
    ("harness.channels.direct", "- with {names}, last at {when}"),
    (
        "harness.channels.group",
        "- group of {names}, last at {when}",
    ),
    (
        "harness.guide.terminal",
        "How {user} operates this place from the terminal, for explaining it. /help lists every command. Name the commands as written.",
    ),
    (
        "harness.guide.terminal.channels",
        "Switching conversation: /talk <person> opens a person, /host comes back here, and /go <n> takes a choice from the numbered list you showed by listing the conversations.",
    ),
    (
        "harness.guide.terminal.group",
        "A group: /group <p1> <p2> ... with two or more people.",
    ),
    (
        "harness.guide.terminal.resend",
        "Say it again: /resend [n] takes back {user}'s n-th last message and puts its text back to edit.",
    ),
    (
        "harness.guide.terminal.sources",
        "Sources of a reply: /sources [n] lists them in full.",
    ),
    (
        "harness.guide.terminal.name",
        "The name {user} is called by: /name <name>.",
    ),
    (
        "harness.guide.terminal.intensity",
        "Intensity: /tier [level] shows or sets it.",
    ),
    (
        "harness.guide.terminal.saves",
        "Saves: /save lists them; /save new, switch, copy, rename, delete, export or import followed by what it needs.",
    ),
    (
        "harness.guide.terminal.memories",
        "Memories: /memories [person] lists them, /forget <n> deletes one, /restore <n> brings it back, /edit <n> <text> rewrites it, /wrapup closes the conversation and writes them.",
    ),
    (
        "harness.guide.app",
        "How {user} operates this place from the app, for explaining it. Name the buttons and places as they read on screen.",
    ),
    (
        "harness.guide.app.channels",
        "Switching conversation: open it from Home, or press the choice you offer.",
    ),
    (
        "harness.guide.app.group",
        "A group: New group on Roster, or Bring people in inside a conversation.",
    ),
    (
        "harness.guide.app.resend",
        "Say it again: Send again on {user}'s own message.",
    ),
    (
        "harness.guide.app.sources",
        "Sources of a reply: Sources under it.",
    ),
    (
        "harness.guide.app.name",
        "The name {user} is called by: System, tab Quota and quiet.",
    ),
    (
        "harness.guide.app.intensity",
        "Intensity: System, tab Quota and quiet.",
    ),
    ("harness.guide.app.saves", "Saves: System."),
    (
        "harness.guide.app.memories",
        "Memories: World lists what was remembered.",
    ),
    (
        "harness.guide.limits",
        "Explain these as operations of this place, in its own words. {user} acts on them; you can only list choices and offer them, and never say you switched, opened or changed anything for {user}. Say only what your tools did: found people, made a group, asked someone, listed conversations. You have no queue and no way to chase anyone, so nothing is queued, scheduled, chased or followed up; that is {user}'s to do.",
    ),
    ("harness.done", "Done."),
    (
        "harness.suggested",
        "The host has the suggestion; {user} decides, and nothing has happened yet.",
    ),
    ("harness.asked", "{name} is looking into it."),
    ("harness.group_created", "Group {id} created."),
    (
        "harness.group_purpose",
        "{name} suggested this group: {purpose}",
    ),
    (
        "harness.found_for",
        "{name} suggested you could help {user} with: {topic}",
    ),
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
    /// What the user wrote about themselves; `None` when nothing.
    about: Option<String>,
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
            about: None,
        }
    }

    /// With the user's own words about themselves (blank counts as none).
    pub fn with_about(mut self, about: Option<&str>) -> Self {
        self.about = about
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned);
        self
    }

    /// A segment's names: what its bytes already say the user is called.
    pub fn fixed(placeholder: &str, user: &str) -> Self {
        Self {
            placeholder: placeholder.to_owned(),
            user: user.to_owned(),
            about: None,
        }
    }

    /// What the placeholder becomes.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// The user's own words about themselves, if any.
    pub fn about(&self) -> Option<&str> {
        self.about.as_deref()
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
