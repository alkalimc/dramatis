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
/wrapup             end the current conversation and show the memories it wrote",
    ),
    (
        "cli.help.channels",
        "Channels
/list [filter]      persons who can be talked to, and whether the host is available
/talk <person>      open the direct channel with a person (id, name or alias)
/group <p1> <p2>    start a group channel with several persons
/host               switch to the host's channel
/go <n>             take choice n from the list the host just offered",
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
