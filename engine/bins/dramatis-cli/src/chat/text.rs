//! The terminal's own sentences. Each is read from the corpus's wording under its key
//! (`cli.*`), so the pack can say it in its language; the English here is the fallback.

/// Fallbacks for every key the terminal reads.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("cli.typing", "Enter: new line · Option/Alt-Enter: send"),
    ("cli.typing.more", "Option/Alt-Enter: send"),
    ("cli.start", "/help for commands, /quit or Ctrl-D to leave"),
    (
        "cli.library.start",
        "No language model is set up, so this is library mode: the roster, search, reading \
         and saves work without one. /help lists the commands.",
    ),
    (
        "cli.library.on",
        "library mode: nothing here calls a language model; /library off goes back",
    ),
    ("cli.library.off", "back to conversations"),
    (
        "cli.library.stays",
        "no language model is set up, so library mode stays on",
    ),
    (
        "cli.library.needs_model",
        "that needs a language model, and none is set up; restart this program to be \
         asked for one, or use the app's settings",
    ),
    (
        "cli.library.needs_model_off",
        "that needs a language model; library mode is on, and /library off leaves it",
    ),
    (
        "cli.help.conversation",
        "Conversation
anything else       send it to the current channel
//text              send a message that starts with a slash
/history [n]        reprint the last n messages of this channel (default 10)
/resend [n]         take back your n-th last message (default 1) and everything after it,
                    and put its text back in the input line to edit and send again
/sources [n]        list the sources of the n-th last reply that has any (default 1), numbered
/sources [n] <k>    print the text of entry k of that list (/source <k> for the last reply)
/wrapup             end the current conversation and show the memories it wrote
/read [all]         mark this conversation (or every one) read; what is printed here counts
                    as read, in the app too",
    ),
    (
        "cli.help.channels",
        "Channels
/list [filter]      persons who can be talked to, and whether {host} is available
/talk <person>      open the direct channel with a person (id, name or alias)
/group <p1> <p2>    start a group channel with several persons
/host               switch to {host}'s channel
/go <n>             take choice n from the list {host} just offered
/chats [archived]   your conversations, newest first, with what is unread; /chat <n> opens one
/rename [title]     title this conversation; no title returns to the members' names
/members            who is in this group
/invite <p1>, <p2>  bring persons into this group
/remove <person>    take a person out of this group; what they said stays
/archive, /unarchive  take this conversation out of the list, or put it back
/leave              delete this group and what was said in it (asks first)",
    ),
    (
        "cli.help.people",
        "People and requests
/who <person>       profile: mode, trust, last seen, requests done
/mode [person] <enabled|frozen|disabled>
                    enabled may speak first, frozen only answers, disabled is out of reach;
                    without a person, this conversation's (a group has its own)
/ask <person> <question>
                    ask a person to look into something; \"quotes\" hold a name with spaces
/requests           the requests made so far and whether they are done
/clock              the date in the world and the local time
/find <topic>       who knows about it, with a line from their material
/waiting            who has something waiting for you
/birthday [MM-DD|clear]
                    show, set or clear your birthday",
    ),
    (
        "cli.help.memories",
        "Memories
/memories [person]  numbered memories, or those a person may recall; deleted ones last
/forget <n>         delete memory n of that list
/restore <n>        bring back a deleted memory
/edit <n> <text>    rewrite memory n",
    ),
    (
        "cli.help.saves",
        "Saves (one world each: channels, memories, bonds, requests, usage)
/save               list saves; * marks the one in use
/save new <name>    an empty save
/save switch <name> close this world and open another
/save logout        close this world and go back to the list of saves (also /logout)
/save copy <from> <to>, /save rename <from> <to>
/save delete <name> asks first; the save in use cannot be deleted
/save export <name> <path>, /save import <path> [name]
                    a single file to keep or move, and back; \"quotes\" hold spaces",
    ),
    (
        "cli.help.settings",
        "Settings
/name <new name>    change the name you are called by
/bio [text|clear]   show, set or clear what the people are told about you (200 characters)
/password [on|change|off]
                    encrypt this save with a password, change it, or remove it; asked for
                    at start when a save is encrypted
/tier [level]       how much the people may do on their own: low, middle, high,
                    extra_high, max, ultra
/quiet [on|off|HH:MM-HH:MM]  hours when nobody comes to you unprompted
/events [on|off|n]  the small happenings of the day the host tells of: list them, switch
                    them, or set how many a day (0 to 2)
/lang [corpus|en]   the words of the interface: the corpus's language or English; the
                    people keep speaking the corpus's language",
    ),
    (
        "cli.help.other",
        "Other
/library [off]      library mode: the corpus without a language model (/list, /who, /search,
                    /read, /sources, /history, /memories and saves work; talking does not).
                    It starts by itself when no endpoint or key is set up
/search <query>     retrieval only, as the current speaker would see it (the whole corpus
                    when no conversation is open); results are numbered
/read <n|unit id>   a unit of the corpus with its neighbours and its source link; n is a
                    number from /search
/stats              model calls made in this session
/storage [clear avatars|audio|all]  disk use by kind; empty the avatar or audio cache
/help, /quit        this text; leave (Ctrl-D also works)
Short forms: /ls /t /g /c /lib /req /time /mem /h /r /src /s /? /open /exit /q
Tab completes command names, their fixed words (/tier, /quiet, /events, /lang, /library,
/read, /chats, /storage, /password), person names (/talk, /group, /invite, /remove, /who,
/mode, /ask, /memories) and save names (/save).
A line that starts with a slash is a command and Enter runs it. With --plain each input
line is one message.",
    ),
    ("cli.action.join", "asked to bring someone in"),
    ("cli.action.wrapup", "wrapped up the conversation"),
    ("cli.action.list_channels", "listed {n} conversations"),
    ("cli.offer.person", "talk to {name}"),
    ("cli.offer.group", "open group: {names}"),
    ("cli.offer.new_group", "start a group: {names}"),
    ("cli.offer.invite", "invite {name} to join"),
    ("cli.invite.already", "they are already in this group"),
    ("cli.remove.not_member", "{name} is not in this group"),
    (
        "cli.remove.last",
        "a group keeps at least one person; /leave deletes it",
    ),
    ("cli.sources.count", "sources: {n}  (/sources to show)"),
    ("cli.picture", "[picture]"),
    ("cli.picture.lost", "[picture unavailable]"),
    ("cli.error", "error: {text}"),
    ("cli.cancelled", "cancelled"),
    ("cli.answer.yes", "y|yes"),
    ("cli.answer.again", "answer y or n"),
    ("cli.prompt.save", "save>"),
    ("cli.prompt.library", "library"),
    ("cli.unknown_command", "unknown command /{name}; try /help"),
    ("cli.name.set", "you are called {name} now"),
    ("cli.unread.header", "{n} unread:"),
    ("cli.list.host", "host: {host} ({state})"),
    ("cli.list.host.available", "available"),
    ("cli.list.host.unavailable", "not available"),
    ("cli.list.count", "{n} persons"),
    ("cli.list.count.persona", "{n} persons with a persona"),
    (
        "cli.resolve.none",
        "nobody matches \"{q}\"; /list shows who can be talked to",
    ),
    (
        "cli.resolve.no_persona",
        "{name} has no persona, so cannot be talked to",
    ),
    ("cli.resolve.ambiguous", "\"{q}\" is ambiguous: {names}"),
    (
        "cli.talk.person",
        "talking to {name} ({id}); /history shows earlier lines",
    ),
    ("cli.talk.group", "group with {label}"),
    ("cli.talk.host", "talking to the host, {name}"),
    (
        "cli.group.too_few",
        "a group needs at least two different persons",
    ),
    ("cli.go.nothing", "the host has offered nothing to go to"),
    ("cli.go.no_such", "no such choice; the host's list is above"),
    ("cli.go.hint", "  /go <n> takes you there"),
    ("cli.host.none", "this corpus has no host"),
    ("cli.chats.none_archived", "no archived conversations"),
    (
        "cli.chats.none",
        "no conversations yet; /talk <person> starts one",
    ),
    ("cli.chats.kind.host", "host"),
    ("cli.chats.kind.direct", "direct"),
    ("cli.chats.kind.group", "group"),
    ("cli.chats.unread", "{n} unread"),
    ("cli.chats.hint", "/chat <n> opens one"),
    (
        "cli.chat.no_list",
        "no list yet; /chats shows the conversations",
    ),
    (
        "cli.chat.no_such",
        "no such conversation; /chats shows the list",
    ),
    ("cli.who.mode", "mode: {mode}  trust: {trust}"),
    ("cli.who.no_persona", "no persona, so cannot be talked to"),
    ("cli.who.never", "never"),
    ("cli.who.seen", "last seen: {seen}  requests done: {done}"),
    ("cli.who.close", "close to: {names}"),
    ("cli.mode.host", "the host has no mode"),
    (
        "cli.no_channel",
        "no channel yet: /talk <person> or /host first",
    ),
    ("cli.mode.set", "{name} is {mode} now"),
    (
        "cli.archive.on",
        "archived; a new message from a person brings it back",
    ),
    ("cli.archive.off", "back in the main list"),
    (
        "cli.archived.open",
        "this conversation is archived; /unarchive brings it back to the main list",
    ),
    ("cli.archive.host", "the host's channel cannot be archived"),
    (
        "cli.rename.cleared",
        "the title is derived from the members again",
    ),
    ("cli.rename.done", "renamed"),
    ("cli.members.not_group", "this is not a group"),
    (
        "cli.leave.ask",
        "delete this group and everything said in it? (y/n)",
    ),
    ("cli.leave.only_group", "only a group can be deleted"),
    ("cli.read.marked.one", "marked read: {n} conversation"),
    ("cli.read.marked", "marked read: {n} conversations"),
    ("cli.ask.sent", "{name} answers in their own channel"),
    (
        "cli.requests.none",
        "no requests yet; /ask <person> <question> makes one",
    ),
    ("cli.requests.nobody", "nobody yet"),
    ("cli.requests.pinned", "(pinned)"),
    ("cli.find.none", "nobody on the roster matches"),
    ("cli.waiting.none", "nobody is waiting on you"),
    (
        "cli.interrupted",
        "cancelled; the message stays in the history",
    ),
    ("cli.footer.one", "· {n} call, {secs}s"),
    ("cli.footer", "· {n} calls, {secs}s"),
    (
        "cli.wrapup.host",
        "the host keeps no memories; nothing to wrap up",
    ),
    ("cli.wrapup.empty", "wrapped up; no memories were written"),
    ("cli.wrapup.written", "wrapped up; {n} memories written"),
    ("cli.audience.world", "everyone"),
    ("cli.audience.own", "only {name}"),
    ("cli.audience.channel", "channel {channel} ({names})"),
    ("cli.memories.seen_by", "    seen by: {who}"),
    ("cli.memories.none", "no memories"),
    ("cli.memories.deleted_mark", "(deleted)"),
    (
        "cli.memories.summary",
        "{live} memories, {deleted} deleted; /forget, /restore or /edit <n>",
    ),
    ("cli.memory.no_list", "no list yet: /memories first"),
    ("cli.memory.no_such", "no such memory in the last list"),
    (
        "cli.memory.is_deleted",
        "that one is deleted; /restore it first",
    ),
    ("cli.memory.not_deleted", "that one is not deleted"),
    ("cli.memory.deleted", "deleted; /memories lists again"),
    ("cli.memory.restored", "restored; /memories lists again"),
    ("cli.memory.rewritten", "rewritten; /memories lists again"),
    ("cli.history.none", "nothing said yet"),
    ("cli.resend.none", "no message of yours to send again"),
    (
        "cli.resend.done.one",
        "took back {n} message; edit it and send again",
    ),
    (
        "cli.resend.done",
        "took back {n} messages; edit it and send again",
    ),
    (
        "cli.resend.busy",
        "a reply is still being written here; try again when it ends",
    ),
    ("cli.sources.none", "no reply with sources there"),
    ("cli.sources.header", "sources ({n}):"),
    (
        "cli.sources.hint",
        "  /sources [n] <k> shows the text of entry k",
    ),
    ("cli.sources.no_entry", "no entry {k}; the reply has {n}"),
    ("cli.sources.gone", "that source is no longer in the corpus"),
    ("cli.source.link", "source: {url}"),
    ("cli.search.as", "as {name}"),
    ("cli.search.as_host", "as the host: whole corpus"),
    ("cli.search.confidence", "confidence: {level}"),
    ("cli.search.none", "no hits"),
    (
        "cli.search.hint",
        "/read <n> opens a result with its neighbours",
    ),
    (
        "cli.read.no_results",
        "no results to number yet; /search first, or give a unit id",
    ),
    (
        "cli.read.no_such",
        "no such result; the list from /search is above",
    ),
    ("cli.read.no_unit", "no unit \"{what}\" in the corpus"),
    ("cli.password.new", "new password: "),
    ("cli.password.again", "again: "),
    ("cli.password.mismatch", "the two did not match"),
    (
        "cli.password.is_on",
        "this save is encrypted; /password change or /password off",
    ),
    (
        "cli.password.is_off",
        "this save is not encrypted; /password on turns it on",
    ),
    (
        "cli.password.warning",
        "A forgotten password cannot be recovered: nobody can open this save without it.",
    ),
    (
        "cli.password.ask_on",
        "encrypt this save with a password? (y/n)",
    ),
    (
        "cli.password.ask_off",
        "remove the password from this save? (y/n)",
    ),
    ("cli.password.current", "current password: "),
    ("cli.password.changed", "password changed"),
    (
        "cli.password.already_on",
        "this save is already encrypted; /password change or /password off",
    ),
    (
        "cli.password.not_on",
        "this save is not encrypted; /password on first",
    ),
    ("cli.bio.show", "bio: {bio}"),
    ("cli.bio.none", "no bio; /bio <text> sets one"),
    ("cli.bio.saved", "bio saved"),
    ("cli.bio.cleared", "bio cleared"),
    ("cli.bio.too_long", "a bio is at most {max} characters"),
    ("cli.tier.levels", "levels: {levels}"),
    ("cli.tier.status", "tier {tier}, quota {band}"),
    ("cli.usage.quiet", "usage: /quiet [on|off|HH:MM-HH:MM]"),
    (
        "cli.quiet.invalid",
        "hours are 24-hour HH:MM, like 23:00-07:00",
    ),
    ("cli.quiet.on", "quiet hours on, {from} to {to}"),
    ("cli.quiet.off", "quiet hours off"),
    ("cli.usage.events", "usage: /events [on|off|0|1|2]"),
    ("cli.events.on", "daily events on, {n} a day"),
    ("cli.events.off", "daily events off"),
    ("cli.events.none", "nothing has happened yet"),
    (
        "cli.save.busy",
        "a reply is still being written; try again when it ends",
    ),
    (
        "cli.save.open_elsewhere",
        "this save is open in another window; close it there first",
    ),
    (
        "cli.save.changed",
        "this save was changed in another window, so this one stopped using it",
    ),
    ("cli.save.exists", "a save with that name already exists"),
    (
        "cli.save.locked",
        "that save is encrypted; /save switch to it and enter its password",
    ),
    ("cli.save.wrong_password", "wrong password"),
    ("cli.save.bad_password", "that is not a usable password"),
    ("cli.save.not_found", "no such save (or file)"),
    (
        "cli.save.bad_name",
        "that is not a usable name (no slashes, no leading dot, at most 48 characters)",
    ),
    (
        "cli.save.bad_file",
        "that file is not a world database this version can read",
    ),
    ("cli.save.created", "created {n}"),
    ("cli.save.copied", "copied {a} to {b}"),
    ("cli.save.renamed", "renamed {a} to {b}"),
    (
        "cli.save.imported",
        "imported as {n}; /save switch {n} opens it",
    ),
    ("cli.save.encrypted", "encrypted"),
    (
        "cli.save.playing",
        "now playing {name}; /talk <person> or /host to begin",
    ),
    ("cli.save.logged_out", "logged out of the save"),
    (
        "cli.picker.gone",
        "The save you used last, \"{gone}\", is not there any more (renamed or deleted elsewhere).",
    ),
    ("cli.picker.header", "Saves in {dir}"),
    ("cli.picker.none", "  (none yet)"),
    (
        "cli.picker.help",
        "Type a number to open a save, \"new <name>\" to start one, \"import <path> [name]\" to add a file, or \"quit\".",
    ),
    ("cli.picker.no_number", "there is no save with that number"),
    (
        "cli.picker.reminder",
        "No save is open. Type a number from the list, \"new <name>\", \"import <path> [name]\" or \"quit\".",
    ),
    ("cli.password.for", "password for {name}: "),
    ("cli.password.file", "password of that file: "),
    ("cli.password.plain", "password: "),
    ("cli.password.now_on", "this save is encrypted now"),
    ("cli.password.now_off", "this save is not encrypted now"),
    (
        "cli.delete.in_use",
        "that is the save in use; /save switch to another first",
    ),
    ("cli.delete.ask", "delete save {name} for good? (y/n)"),
    ("cli.delete.done", "deleted {name}"),
    ("cli.export.replace", "{path} exists; replace it? (y/n)"),
    ("cli.export.wrote", "wrote {path}"),
    ("cli.leave.done", "group deleted"),
    ("cli.storage.cleared", "cleared"),
    (
        "cli.storage.ask",
        "clear {size} of cached files? Saves and the corpus stay. (y/n)",
    ),
    ("cli.storage.clearable", "(/storage clear)"),
    ("cli.storage.files", "{n} files"),
    ("cli.storage.save", "  save {name}: {size}"),
    (
        "cli.storage.pictures",
        "pictures in this save: {n} ({size})",
    ),
    ("cli.storage.total", "total    {size}"),
    ("cli.stats.none", "no model calls yet"),
    ("cli.stats.shape", "shape"),
    ("cli.stats.model", "model"),
    ("cli.stats.uncached", "uncached"),
    ("cli.stats.cached", "cached"),
    ("cli.stats.output", "output"),
    ("cli.stats.secs", "secs"),
    ("cli.stats.total", "total"),
    ("cli.birthday.show", "birthday: {date}"),
    ("cli.birthday.none", "no birthday; /birthday MM-DD sets one"),
    ("cli.birthday.saved", "birthday saved"),
    ("cli.birthday.cleared", "birthday cleared"),
    ("cli.birthday.invalid", "that is not a date; use MM-DD"),
    ("cli.lang.current", "interface language: {language}"),
    ("cli.lang.set", "interface language is now {language}"),
    ("cli.usage.lang", "usage: /lang [corpus|en]"),
    ("cli.usage.birthday", "usage: /birthday [MM-DD | clear]"),
    ("cli.usage.talk", "usage: /talk <person>"),
    ("cli.usage.group", "usage: /group <p1> <p2> [...]"),
    ("cli.usage.invite", "usage: /invite <p1>, <p2> [...]"),
    ("cli.usage.remove", "usage: /remove <person>"),
    ("cli.usage.chats", "usage: /chats [archived]"),
    ("cli.usage.chat", "usage: /chat <n>, a number from /chats"),
    ("cli.usage.who", "usage: /who <person>"),
    (
        "cli.usage.mode",
        "usage: /mode [person] enabled|frozen|disabled",
    ),
    ("cli.usage.library", "usage: /library [off]"),
    (
        "cli.usage.ask",
        "usage: /ask <person> <question>; \"quotes\" hold a name with spaces",
    ),
    ("cli.usage.find", "usage: /find <topic>"),
    (
        "cli.usage.go",
        "usage: /go <n>, a number from the host's list",
    ),
    ("cli.usage.name", "usage: /name <new name>"),
    ("cli.usage.password", "usage: /password [on | change | off]"),
    (
        "cli.usage.forget",
        "usage: /forget <n>, a number from /memories",
    ),
    (
        "cli.usage.restore",
        "usage: /restore <n>, a number from /memories",
    ),
    (
        "cli.usage.edit",
        "usage: /edit <n> <new text>, n from /memories",
    ),
    ("cli.usage.logout", "usage: /logout"),
    ("cli.usage.history", "usage: /history [n]"),
    ("cli.usage.resend", "usage: /resend [n]"),
    ("cli.usage.sources", "usage: /sources [n] [entry]"),
    (
        "cli.usage.source",
        "usage: /source <entry>, a number from /sources",
    ),
    ("cli.usage.search", "usage: /search <query>"),
    (
        "cli.usage.storage",
        "usage: /storage [clear avatars|audio|all]",
    ),
    (
        "cli.usage.save",
        "usage: /save [new <name> | switch <name> | copy <from> <to> | rename <from> <to> | delete <name> | export <name> <path> | import <path> [name] | logout]",
    ),
    ("cli.answer.no", "n|no"),
    (
        "cli.setup.no_key",
        "The endpoint \"{profile}\" has no API key in the keychain.",
    ),
    ("cli.setup.key_prompt", "API key (hidden, empty to skip): "),
    ("cli.setup.key_stored", "key stored in the keychain"),
    (
        "cli.setup.no_model",
        "No chat model is set up. Without one you can search the corpus but not talk to anyone.",
    ),
    ("cli.setup.offer", "Set one up now? [Y/n] "),
    ("cli.setup.presets", "Starting points:"),
    (
        "cli.setup.preset_prompt",
        "Number of a starting point, or Enter to type everything: ",
    ),
    (
        "cli.setup.saved",
        "saved; the endpoints file is in your home directory and holds no key",
    ),
    ("cli.setup.again", "{problem}; let us go through it again"),
    ("cli.setup.field.name", "profile name"),
    (
        "cli.setup.field.address",
        "address (e.g. https://api.example.com/v1)",
    ),
    ("cli.setup.field.wire", "wire: chat, responses or messages"),
    ("cli.setup.field.model", "model id"),
    (
        "cli.setup.field.host_model",
        "a different model for the host (Enter: same as above)",
    ),
    (
        "cli.setup.field.key",
        "API key (hidden, empty to add it later): ",
    ),
    (
        "cli.setup.bad_wire",
        "the wire is chat, responses or messages; using chat",
    ),
    ("cli.setup.bad.name", "the profile needs a name"),
    (
        "cli.setup.bad.base_url",
        "the address must start with http:// or https:// and have no spaces",
    ),
    (
        "cli.setup.bad.model",
        "the model id is a single word (no spaces)",
    ),
    (
        "cli.setup.bad.host_model",
        "the host's model id is a single word (no spaces)",
    ),
    ("cli.setup.bad.other", "{field} is not usable"),
    (
        "cli.setup.bad.keychain",
        "the keychain refused the key: {detail}",
    ),
    (
        "cli.first.language",
        "Welcome. A few quick questions; press Enter to skip any of them.",
    ),
    (
        "cli.first.language_prompt",
        "Language: 1 = the corpus's language, 2 = English (Enter: 1): ",
    ),
    ("cli.first.name", "What should they call you? "),
    ("cli.first.birthday", "Birthday as MM-DD (optional): "),
    (
        "cli.first.bio",
        "About you, up to {max} characters (optional): ",
    ),
    (
        "cli.first.avatar",
        "A picture of you: the path to an image file (optional): ",
    ),
    (
        "cli.first.avatar.unreadable",
        "That file could not be read; the default picture stays.",
    ),
    (
        "cli.first.avatar.invalid",
        "That is not a picture that can be used; the default picture stays.",
    ),
    (
        "cli.header",
        "folio {folio} | world {world} | chat {chat} | host {host_model}",
    ),
    ("cli.header.none", "none"),
    ("cli.header.throwaway", "throwaway"),
    ("cli.header.no_world", "none open"),
];

/// The English fallback for `key`.
pub fn default(key: &str) -> &'static str {
    DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, v)| *v)
}
