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
    ("host.alias", ""),
    (
        "host.quota_exhausted",
        "The quota for this period is used up until {when}. The archive stays available.",
    ),
    (
        "host.reminder_failed",
        "A reminder of mine fell due and I could not act on it: {text}",
    ),
    (
        "host.reminder_done",
        "A reminder of mine fell due, and I have dealt with it: {text}",
    ),
    ("host.away_back", "While you were away: {items}"),
    (
        "host.rate_limited",
        "The language model service is rate limited right now. Your message is kept; \
         send it again later.",
    ),
    (
        "host.intro.terminal",
        "Welcome. The terminal is maintained by alkali. You can talk to anyone here with \
         /talk <name>, start a group with /group, \
         and ask me for anything: finding someone who knows a subject, or a summary of what \
         changed. Memories, saves and settings all have commands; /help lists them, \
         and I can explain any of them.",
    ),
    (
        "host.event.frame",
        "Something small happened today, if you would like to hear it.\n\n{title}\n{text}\
         |A little news from today, whenever you have a moment.\n\n{title}\n{text}\
         |Just so you know, there was a small happening today.\n\n{title}\n{text}",
    ),
    (
        "host.intro.app",
        "Welcome. The terminal is maintained by alkali. You can talk to anyone on the \
         Roster, start a group, and ask me for anything: \
         finding someone who knows a subject, or a summary of what changed. Memories, \
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
        "{user} showed a picture here, but it could not be seen.",
    ),
    (
        "harness.rules",
        "{user} is here in person with the people of this world: a direct chat is one person \
         and {user} together, a group is its members gathered with {user}. What is said is \
         said aloud to those present, not sent as a message. Each reply is spoken by one of \
         them, as themselves. When a person does not know something, they react the way \
         that person would: brush it off, ask back, go by impression or change the subject. \
         They never talk about whether they have evidence, records or sources. A person acts \
         only within this gathering. Other chats, groups and the host are out of reach \
         except through the means given here, and so are removing someone from a group, \
         changing a setting or taking {user} somewhere; what has no means is offered or left \
         to {user}, never reported as done. The host answers for itself alone: a person never \
         voices the host or makes up what it would say.",
    ),
    (
        "harness.rules.host",
        "{user} talks with the people of this world through messages. Each reply is spoken \
         by one of them, as themselves. When a person does not know something, they react \
         the way that person would: brush it off, ask back, go by impression or change the \
         subject. They never talk about whether they have evidence, records or sources. A \
         person speaks only in this conversation; other chats, groups and the host are out of \
         reach except through the means given here, and what has no means is offered or left \
         to {user}, never reported as done.",
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
    ("harness.recent", "The last things said:"),
    ("harness.context", "[{date} {weekday} {time}, {part}]"),
    ("harness.context.gap", "({user} last spoke {span} ago)"),
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
    ("harness.say_mark", "⇒ say:"),
    ("harness.reply_mark", "↩"),
    (
        "harness.propose",
        "You help {user} yourself first: you answer, offer to do the thing, and take it on \
         within your own role and abilities. Only when the matter is clearly outside what \
         you know or can do, or {user} asks you to find someone or set up a group, you can \
         offer {user} a choice through the host; nothing happens until {user} takes it, and \
         it sits under your reply, so never say you sent or arranged anything. To point \
         {user} to particular colleagues, end the reply with one final line that starts with \
         {find} followed by their names, separated by commas, at most three, and if you like \
         a bar and what it is about in a few words, which the one {user} goes to is told. \
         Only the people on that line are offered; a colleague merely mentioned needs no \
         line. To offer a group with colleagues from the roster, end it with one final line \
         that starts with {group} followed by their names, separated by commas, a few people \
         who matter for the matter to begin with (more can be invited later), then a bar \
         and the purpose in a few words (at most sixty characters), which every member is \
         told. To continue a group you already have with those people, \
         write its name as the purpose; any other purpose makes a new group. Such a line \
         comes at most once per reply, and after a citation line if there is one.",
    ),
    (
        "harness.say",
        "Only when {user} has plainly asked you to say something in a group you are in, end \
         the reply with one final line that starts with {say} followed by the group (its \
         name, its purpose or its other members), a bar and the one line to post there. It \
         goes out under your name and the others answer there, out of your hearing, so say \
         in the reply that you will ask, and never that they have answered.",
    ),
    ("harness.said", "{name} says in the group: {text}"),
    (
        "harness.reply",
        "Each of {user}'s messages ends with its id in brackets, such as {tag}. Most replies \
         quote nothing. Only when you answer one particular earlier message of {user}, not \
         the latest, end the reply with one final line that starts with {mark} followed by \
         that id, for example {example}, and nothing else on that line. If a citation line \
         is also written, this line comes before it.",
    ),
    ("harness.reply_to", "{user} replies to {name}'s \"{text}\":"),
    ("harness.reply_to.picture", "a picture"),
    (
        "harness.forwarded",
        "{user} forwards you this message to look at. {author} said: \"{text}\"",
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
    ("harness.left", "{name} leaves the conversation."),
    ("list.separator", ", "),
    (
        "harness.groups",
        "{name} is also in these groups with {user}, each by its other members:\n{list}",
    ),
    ("harness.groups.line", "- {others}"),
    ("harness.groups.line_named", "- {title}: {others}"),
    ("harness.groups.about", " ({purpose})"),
    (
        "harness.wrapup",
        "The conversation is closing. Step out of the scene and do not answer as the \
         character. Give only the wrap-up: write what is worth remembering as short \
         memories, such as what {user} told you of their own life, what was decided or \
         agreed, and what passed between you, even when small; anything either of you said \
         you would do, and can do within a talk with {user}, becomes a promise with its \
         date, in the calendar the context line shows. Going elsewhere, asking others or \
         checking other chats is not something a person can promise. Write none only if nothing but \
         greetings were exchanged. Use the wrapup call if it is available; otherwise reply \
         with just the JSON object the schema describes.",
    ),
    (
        "harness.wrapup.summary",
        "The conversation moves to a new log. Step out of the scene and do not answer as the \
         character. Give only the wrap-up: write what is worth remembering as short \
         memories, such as what {user} told you of their own life, what was decided or \
         agreed, and what passed between you, even when small; anything either of you said \
         you would do, and can do within a talk with {user}, becomes a promise with its \
         date, in the calendar the context line shows. Going elsewhere, asking others or \
         checking other chats is not something a person can promise. Write none only if nothing but \
         greetings were exchanged. Add a summary of the conversation so far. Use the \
         wrapup call if it is available; otherwise reply with just the JSON object the \
         schema describes.",
    ),
    (
        "harness.wrapup.shape",
        "Reply with one JSON object only, no code fence, using exactly these keys: {shape}",
    ),
    (
        "harness.opening",
        "{name} speaks first. {name} is the one who goes to {user}, or the two happen to run \
         into each other; {user} did not come looking for {name}. A scene line, if there is \
         one, shows that.",
    ),
    ("harness.opening.commitment", "A promise is due: {text}"),
    ("harness.opening.birthday", "Today is {user}'s birthday."),
    (
        "harness.opening.visit",
        "Nothing in particular brings you to {user}. You stop by where {user} is, or the two \
         of you run into each other. Speak first with a short, natural line, as someone \
         stopping by.",
    ),
    (
        "harness.opening.memory",
        "Something {name} remembers about {user}: {text}",
    ),
    ("harness.opening.words", "{user} said recently: {text}"),
    (
        "harness.opening.event",
        "Something happened today that you are part of: {text}",
    ),
    (
        "harness.event",
        "Something happened today, and you may mention it if it comes up \
         naturally: {text}",
    ),
    (
        "harness.event.ask",
        "Make up one small happening for today, {date}, for {user} to hear about. \
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
    ("harness.error.self", "Name colleagues other than yourself."),
    (
        "harness.error.quiet",
        "Suggestions are held back for now; try again later.",
    ),
    (
        "harness.error.no_match",
        "Nobody on the roster answers to {detail}.",
    ),
    (
        "harness.error.nobody_new",
        "Everyone named is already in this group.",
    ),
    ("harness.error.disabled", "That colleague is not available."),
    (
        "harness.error.not_member",
        "You are not a member of the group {detail}.",
    ),
    (
        "harness.error.post_here",
        "That is this conversation; write the message here as your reply.",
    ),
    (
        "harness.error.date",
        "Could not read the day {detail}. Use a month and day such as 12-24.",
    ),
    (
        "harness.error.due",
        "Could not read the due time {detail}. Use a date such as 2027-01-31 or a date and time such as 2027-01-31T18:00.",
    ),
    (
        "harness.error.due_past",
        "The due time {detail} has already passed.",
    ),
    (
        "harness.error.promises_full",
        "{max} reminders are already waiting; no more can be set until one has been brought up.",
    ),
    (
        "harness.error.own_channel",
        "That is this conversation; recent lines are for the others.",
    ),
    (
        "harness.channels",
        "The conversations {user} has, most recent first:",
    ),
    ("harness.channels.none", "No conversation yet."),
    (
        "harness.channels.direct",
        "- {id}: with {names}, last at {when}",
    ),
    (
        "harness.channels.group",
        "- {id}: group of {names}, last at {when}",
    ),
    (
        "harness.channels.group_named",
        "- {id}: {title}, group of {names}, last at {when}",
    ),
    (
        "harness.read_recent",
        "The latest lines of that conversation:",
    ),
    (
        "harness.read_recent.none",
        "Nothing has been said there yet.",
    ),
    ("harness.read_recent.line", "- {when} {tag} {name}: {text}"),
    (
        "harness.promised",
        "Reminder set: {name} will bring it up when it is due ({due}).",
    ),
    (
        "harness.promised.own",
        "Reminder set: you will take it up yourself when it is due ({due}).",
    ),
    (
        "harness.posted",
        "Posted in group {id}; the members answer there.",
    ),
    (
        "harness.posted.report",
        "Posted in group {id}; the members answer there, and you will get a turn to tell \
         {user} what they said.",
    ),
    (
        "harness.report_back",
        "Group {id} answered what you posted:\n{lines}\nTell {user} briefly what they said.",
    ),
    ("harness.report_back.line", "- {name}: {text}"),
    (
        "harness.report_back.none",
        "Nobody in group {id} answered what you posted. Tell {user} briefly.",
    ),
    (
        "harness.reminder",
        "A reminder you set for {user} has fallen due: {text}\nDo what it says with your own \
         means, then tell {user} briefly what you did or found out. A message you post in a \
         group gets answers there, and you will get a turn to tell {user} what they said.",
    ),
    ("harness.birthdays", "Birthdays on {date}:"),
    (
        "harness.birthdays.none",
        "Nobody on the roster has a birthday on {date}.",
    ),
    (
        "harness.host_here",
        "{host} is in this group. It answers only when called by name, and only {host} \
         voices what it says: nobody else writes its answers.",
    ),
    (
        "harness.host_away",
        "{host} is not in this group. What {host} would answer comes from {host} alone, so \
         nobody here voices it or makes it up. To have {host} here, offer {user} {host} among \
         the colleagues to bring in.",
    ),
    ("harness.offer_only", "Here is what I suggest."),
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
        "harness.guide.terminal.forward",
        "Answering and passing on a message: /history numbers the messages; /reply <n> <text> answers one, /forward <n> <person|host|fav> passes one on, /fav opens the favorites.",
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
        "harness.guide.terminal.forget",
        "Forgetting: you cannot delete memories. When {user} asks to forget something, point to /forget <n> after /memories.",
    ),
    (
        "harness.guide.app",
        "How {user} operates this place from the app, for explaining it. Name the buttons and places as they read on screen.",
    ),
    (
        "harness.guide.app.channels",
        "Switching conversation: open it from Home, or press the choice you offer.",
    ),
    ("harness.guide.app.group", "A group: New group in the Menu."),
    (
        "harness.guide.app.forward",
        "Answering and passing on a message: right-click it, then Reply, or Forward to a person, {host} or Favorites. Favorites is the chat pinned at the top of the list.",
    ),
    (
        "harness.guide.app.resend",
        "Say it again: right-click {user}'s own message, then Send again.",
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
        "harness.guide.app.forget",
        "Forgetting: you cannot delete memories. When {user} asks to forget something, point to World, where each memory can be deleted.",
    ),
    (
        "harness.guide.app.memories",
        "Memories: World lists what was remembered.",
    ),
    (
        "harness.guide.limits",
        "Explain these as operations of this place, in its own words. {user} acts on them; you can only list choices and offer them, and never say you switched, opened or changed anything for {user}. Say only what your tools did: found people, made a group, listed conversations. You have no queue and no way to chase anyone, so nothing is queued, chased or followed up; that is {user}'s to do, except a reminder you set with promise, which the person you name brings up when it falls due, or which you take up yourself. Removing someone from a group, renaming a group or deleting one is likewise {user}'s own, and you have no means for it.",
    ),
    (
        "harness.host.group",
        "You are in a group (conversation {id}) with {user} and the people here, and {user} \
         called you. Answer as the assistant, briefly. Everyone here hears what you say; what \
         your tools offer appears under your reply and {user} decides.",
    ),
    (
        "harness.guide.promise",
        "When {user} asks to be reminded of something, you can set a reminder with promise: give the text and the due date. With a person it is that person who brings it up; without one it is yours, and when it falls due you act on it and tell {user}.",
    ),
    (
        "harness.guide.post",
        "In a group you are a member of, you can say something with post, by the id list_channels shows; the members answer there and you can be given a turn to tell {user} what they said.",
    ),
    (
        "harness.guide.birthdays",
        "Who has a birthday on a day is answered by birthdays; a search cannot answer it.",
    ),
    (
        "harness.guide.recent",
        "When {user} asks about a conversation, you can read its recent lines with recent, by the id list_channels shows.",
    ),
    ("harness.done", "Done."),
    (
        "harness.suggested",
        "The host has the suggestion; {user} decides, and nothing has happened yet.",
    ),
    ("harness.group_created", "Group {id} created."),
    (
        "harness.group_reused",
        "Group {id} already has exactly these people, so that one is offered instead of a new group.",
    ),
    (
        "harness.group_purpose",
        "{name} suggested this group: {purpose}",
    ),
    (
        "harness.found_for",
        "{name} suggested you could help {user} with: {topic}",
    ),
    (
        "harness.talk_offered",
        "{user} is shown a way to open a chat with {name} ({id}). No group exists and nothing \
         has happened yet; the chat opens only if {user} takes it.",
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

    /// `get` with `{field}` placeholders filled, `{user}` and `{host}` included. The
    /// template's own name placeholders are replaced before the fields go in, so a field (a
    /// question the user typed) reaches the model exactly as written.
    pub fn fill(&self, key: &str, names: &Names, fields: &[(&str, &str)]) -> String {
        self.fill_text(self.get(key), names, fields)
    }

    /// [`Wording::fill`] for a template already in hand (one variant of several).
    pub fn fill_text(&self, template: &str, names: &Names, fields: &[(&str, &str)]) -> String {
        let mut out = names
            .sub(template)
            .replace("{user}", names.user())
            .replace("{host}", self.get("host.name"));
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

    #[test]
    fn the_host_name_fills_wherever_the_template_comes_from() {
        let w = wording(&[("host.name", "Hostess"), ("a", "I am {host}, hi <U>.")]);
        let n = Names::new("<U>", Some("Rhea"), &w);
        assert_eq!(w.fill("a", &n, &[]), "I am Hostess, hi Rhea.");
        assert_eq!(
            w.fill_text("{host}|{t}", &n, &[("t", "{host}")]),
            "Hostess|{host}"
        );
    }
}
