//! The terminal's own sentences. Each is read from the corpus's wording under its key
//! (`cli.*`), so the pack can say it in its language; the English here is the fallback.

/// Fallbacks for every key the terminal reads.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("cli.typing", "Enter: new line · Option/Alt-Enter: send"),
    ("cli.typing.more", "Option/Alt-Enter: send"),
    ("cli.start", "/help for commands, /quit or Ctrl-D to leave"),
    (
        "cli.help.conversation",
        "Conversation
anything else       send it to the current channel
//text              send a message that starts with a slash
/history [n]        reprint the last n messages of this channel (default 10)
/resend [n]         take back your n-th last message (default 1) and everything after it,
                    and put its text back in the input line to edit and send again
/sources [n]        list in full the sources of the n-th last reply that has any (default 1)
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
/quiet [on|off|HH:MM-HH:MM]  hours when nobody comes to you unprompted",
    ),
    (
        "cli.help.other",
        "Other
/search <query>     retrieval only, as the current speaker would see it
/stats              model calls made in this session
/help, /quit        this text; leave (Ctrl-D also works)
Tab completes command names, person names (/talk, /group) and save names (/save).
A line that starts with a slash is a command and Enter runs it. With --plain each input
line is one message.",
    ),
];

/// The English fallback for `key`.
pub fn default(key: &str) -> &'static str {
    DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, v)| *v)
}
