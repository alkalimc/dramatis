//! The REPL's line grammar: a slash command, or a message for the current channel.

use api::views::StorageKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Blank line.
    Noop,
    /// A message for the current channel.
    Say(String),
    List(Option<String>),
    Talk(String),
    Group(Vec<String>),
    Host,
    /// The conversations, newest first (or the archived ones): `/chat <n>` opens one.
    Chats(bool),
    Chat(usize),
    /// A person's profile.
    Who(String),
    /// Set a person's mode (enabled, frozen, disabled); with no person, the current
    /// conversation's (a group's own mode, or the person it is with).
    Mode(String, String),
    /// Move the current channel out of, or back into, the main list.
    Archive(bool),
    /// Title the current channel; empty text returns to the derived title.
    Rename(String),
    Members,
    /// Delete the current group, after asking.
    Leave,
    /// Mark the current channel (or every one) read.
    Read(bool),
    /// Ask a person to look into a question.
    Ask(String, String),
    /// The requests and their state.
    Requests,
    Clock,
    /// Who knows about a topic.
    Find(String),
    /// Who has something waiting for the user.
    Waiting,
    /// Show the user's birthday, set it as `MM-DD`, or clear it.
    Birthday(Option<String>),
    /// Take one of the choices the host last offered (1-based).
    Go(usize),
    /// Set the user's display name.
    Name(String),
    /// Show the personal bio, set it, or (empty text) clear it.
    Bio(Option<String>),
    /// Encryption of the open save: its state, or turn it on, change the password, turn it off.
    Password(Option<PasswordAct>),
    Wrapup,
    /// Numbered list of memories, all or those a person may recall.
    Memories(Option<String>),
    /// Delete, restore or rewrite memory n of the last `/memories` list.
    Forget(usize),
    Restore(usize),
    Edit(usize, String),
    /// Show or set the intensity tier.
    Tier(Option<String>),
    /// Show quiet hours, switch them `on`/`off`, or set `HH:MM-HH:MM`.
    Quiet(Option<String>),
    Save(Save),
    History(usize),
    /// Take back the n-th last message of the user in this channel (1 = the last) and
    /// put its text in the input line.
    Resend(usize),
    /// Show in full the sources of the n-th last reply that has any (1 = the last).
    Sources(usize),
    Search(String),
    Stats,
    /// Disk use by kind, or empty some caches.
    Storage(StorageAct),
    Help,
    Quit,
    /// A known command used wrongly, or an unknown one: the text to show.
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordAct {
    On,
    Change,
    Off,
}

/// What `/storage` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageAct {
    Show,
    /// Empty these caches (after a yes).
    Clear(Vec<StorageKind>),
}

const STORAGE_USAGE: &str = "usage: /storage [clear avatars|audio|all]";

fn parse_storage(args: &str) -> Command {
    let w: Vec<&str> = args.split_whitespace().collect();
    Command::Storage(match w.as_slice() {
        [] => StorageAct::Show,
        ["clear", "avatars"] => StorageAct::Clear(vec![StorageKind::Avatars]),
        ["clear", "audio"] => StorageAct::Clear(vec![StorageKind::Audio]),
        ["clear", "all"] => StorageAct::Clear(vec![StorageKind::Avatars, StorageKind::Audio]),
        _ => return Command::Invalid(STORAGE_USAGE.into()),
    })
}

/// What `/save` was asked to do. Names and paths may be "double quoted" to hold spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Save {
    List,
    New(String),
    Switch(String),
    Copy(String, String),
    Rename(String, String),
    Delete(String),
    Export(String, String),
    Import(String, Option<String>),
    /// Close the open save and go to the save list.
    Logout,
}

const SAVE_USAGE: &str = "usage: /save [new <name> | switch <name> | copy <from> <to> | \
rename <from> <to> | delete <name> | export <name> <path> | import <path> [name] | logout]";

/// Split on spaces, keeping "double quoted" text together.
pub(super) fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut word, mut quoted, mut any) = (String::new(), false, false);
    for c in args.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            c => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(word);
    }
    out
}

fn parse_save(args: &str) -> Command {
    let w = words(args);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let save = match w.as_slice() {
        [] | ["list"] => Save::List,
        ["new", n] => Save::New((*n).into()),
        ["switch", n] => Save::Switch((*n).into()),
        ["copy", a, b] => Save::Copy((*a).into(), (*b).into()),
        ["rename", a, b] => Save::Rename((*a).into(), (*b).into()),
        ["delete", n] => Save::Delete((*n).into()),
        ["export", n, p] => Save::Export((*n).into(), (*p).into()),
        ["logout"] => Save::Logout,
        ["import", p] => Save::Import((*p).into(), None),
        ["import", p, n] => Save::Import((*p).into(), Some((*n).into())),
        _ => return Command::Invalid(SAVE_USAGE.into()),
    };
    Command::Save(save)
}

/// Lines shown by `/history` without an argument.
pub const HISTORY_DEFAULT: usize = 10;

pub fn parse(line: &str) -> Command {
    let line = line.trim();
    if line.is_empty() {
        return Command::Noop;
    }
    if let Some(literal) = line.strip_prefix("//") {
        return Command::Say(format!("/{literal}"));
    }
    let Some(rest) = line.strip_prefix('/') else {
        return Command::Say(line.to_owned());
    };
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    let optional = |a: &str| (!a.is_empty()).then(|| a.to_owned());
    match name {
        "list" | "ls" => Command::List(optional(args)),
        "talk" | "t" if args.is_empty() => Command::Invalid("usage: /talk <person>".into()),
        "talk" | "t" => Command::Talk(args.to_owned()),
        "group" | "g" => {
            let members: Vec<String> = args.split_whitespace().map(str::to_owned).collect();
            if members.len() < 2 {
                Command::Invalid("usage: /group <p1> <p2> [...]".into())
            } else {
                Command::Group(members)
            }
        }
        "host" => Command::Host,
        "chats" | "c" => match args {
            "" => Command::Chats(false),
            "archived" => Command::Chats(true),
            _ => Command::Invalid("usage: /chats [archived]".into()),
        },
        "chat" => match args.parse::<usize>() {
            Ok(n) if n > 0 => Command::Chat(n),
            _ => Command::Invalid("usage: /chat <n>, a number from /chats".into()),
        },
        "who" if args.is_empty() => Command::Invalid("usage: /who <person>".into()),
        "who" => Command::Who(args.to_owned()),
        "mode" if matches!(args, "enabled" | "frozen" | "disabled") => {
            Command::Mode(String::new(), args.to_owned())
        }
        "mode" => match args.rsplit_once(char::is_whitespace) {
            Some((who, mode))
                if !who.trim().is_empty() && matches!(mode, "enabled" | "frozen" | "disabled") =>
            {
                Command::Mode(who.trim().to_owned(), mode.to_owned())
            }
            _ => Command::Invalid("usage: /mode [person] enabled|frozen|disabled".into()),
        },
        "archive" => Command::Archive(true),
        "unarchive" => Command::Archive(false),
        "rename" => Command::Rename(args.to_owned()),
        "members" => Command::Members,
        "leave" => Command::Leave,
        "read" => match args {
            "" => Command::Read(false),
            "all" => Command::Read(true),
            _ => Command::Invalid("usage: /read [all]".into()),
        },
        "ask" => {
            let w = words(args);
            match w.split_first() {
                Some((who, rest)) if !rest.is_empty() => Command::Ask(who.clone(), rest.join(" ")),
                _ => Command::Invalid(
                    "usage: /ask <person> <question>; \"quotes\" hold a name with spaces".into(),
                ),
            }
        }
        "requests" | "req" => Command::Requests,
        "clock" | "time" => Command::Clock,
        "find" if args.is_empty() => Command::Invalid("usage: /find <topic>".into()),
        "find" => Command::Find(args.to_owned()),
        "waiting" => Command::Waiting,
        "birthday" => Command::Birthday(optional(args)),
        "go" | "open" => match args.parse::<usize>() {
            Ok(n) if n > 0 => Command::Go(n),
            _ => Command::Invalid("usage: /go <n>, a number from the host's list".into()),
        },
        "name" if args.is_empty() => Command::Invalid("usage: /name <new name>".into()),
        "name" => Command::Name(args.to_owned()),
        "bio" => Command::Bio(match args {
            "" => None,
            "clear" => Some(String::new()),
            text => Some(text.to_owned()),
        }),
        "password" => match args {
            "" => Command::Password(None),
            "on" => Command::Password(Some(PasswordAct::On)),
            "change" => Command::Password(Some(PasswordAct::Change)),
            "off" => Command::Password(Some(PasswordAct::Off)),
            _ => Command::Invalid("usage: /password [on | change | off]".into()),
        },
        "wrapup" => Command::Wrapup,
        "memories" | "mem" => Command::Memories(optional(args)),
        "forget" | "restore" => match args.parse::<usize>() {
            Ok(n) if n > 0 && name == "forget" => Command::Forget(n),
            Ok(n) if n > 0 => Command::Restore(n),
            _ => Command::Invalid(format!("usage: /{name} <n>, a number from /memories")),
        },
        "edit" => match args.split_once(char::is_whitespace) {
            Some((n, text))
                if n.parse::<usize>().is_ok_and(|n| n > 0) && !text.trim().is_empty() =>
            {
                Command::Edit(n.parse().unwrap_or(1), text.trim().to_owned())
            }
            _ => Command::Invalid("usage: /edit <n> <new text>, n from /memories".into()),
        },
        "tier" => Command::Tier(optional(args)),
        "quiet" => Command::Quiet(optional(args)),
        "save" => parse_save(args),
        "logout" if args.is_empty() => Command::Save(Save::Logout),
        "logout" => Command::Invalid("usage: /logout".into()),
        "history" | "h" => match args {
            "" => Command::History(HISTORY_DEFAULT),
            n => match n.parse::<usize>() {
                Ok(n) if n > 0 => Command::History(n),
                _ => Command::Invalid("usage: /history [n]".into()),
            },
        },
        "resend" | "r" => match args {
            "" => Command::Resend(1),
            n => match n.parse::<usize>() {
                Ok(n) if n > 0 => Command::Resend(n),
                _ => Command::Invalid("usage: /resend [n]".into()),
            },
        },
        "sources" | "src" => match args {
            "" => Command::Sources(1),
            n => match n.parse::<usize>() {
                Ok(n) if n > 0 => Command::Sources(n),
                _ => Command::Invalid("usage: /sources [n]".into()),
            },
        },
        "search" | "s" if args.is_empty() => Command::Invalid("usage: /search <query>".into()),
        "search" | "s" => Command::Search(args.to_owned()),
        "stats" => Command::Stats,
        "storage" => parse_storage(args),
        "help" | "?" => Command::Help,
        "quit" | "exit" | "q" => Command::Quit,
        other => Command::Invalid(format!("unknown command /{other}; try /help")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_a_message() {
        assert_eq!(parse("  hello there "), Command::Say("hello there".into()));
        assert_eq!(parse("   "), Command::Noop);
        assert_eq!(
            parse("//not a command"),
            Command::Say("/not a command".into())
        );
    }

    #[test]
    fn commands_take_their_arguments() {
        assert_eq!(parse("/list"), Command::List(None));
        assert_eq!(parse("/list  an "), Command::List(Some("an".into())));
        assert_eq!(parse("/talk Some Name"), Command::Talk("Some Name".into()));
        assert_eq!(
            parse("/group a b c"),
            Command::Group(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(parse("/history"), Command::History(HISTORY_DEFAULT));
        assert_eq!(parse("/history 3"), Command::History(3));
        assert_eq!(
            parse("/search two words"),
            Command::Search("two words".into())
        );
        assert_eq!(parse("/memories x"), Command::Memories(Some("x".into())));
        assert_eq!(parse("/resend"), Command::Resend(1));
        assert_eq!(parse("/resend 3"), Command::Resend(3));
        assert_eq!(parse("/sources"), Command::Sources(1));
        assert_eq!(parse("/sources 2"), Command::Sources(2));
        assert_eq!(parse("/quit"), Command::Quit);
        assert_eq!(parse("/host"), Command::Host);
        assert_eq!(parse("/go 2"), Command::Go(2));
        assert_eq!(parse("/open 1"), Command::Go(1));
        assert_eq!(parse("/name  Dr. Doc "), Command::Name("Dr. Doc".into()));
        assert_eq!(parse("/bio"), Command::Bio(None));
        assert_eq!(
            parse("/bio  Likes tea "),
            Command::Bio(Some("Likes tea".into()))
        );
        assert_eq!(parse("/bio clear"), Command::Bio(Some(String::new())));
        assert_eq!(parse("/password"), Command::Password(None));
        assert_eq!(
            parse("/password on"),
            Command::Password(Some(PasswordAct::On))
        );
        assert_eq!(
            parse("/password change"),
            Command::Password(Some(PasswordAct::Change))
        );
        assert_eq!(
            parse("/password off"),
            Command::Password(Some(PasswordAct::Off))
        );
        assert_eq!(parse("/forget 2"), Command::Forget(2));
        assert_eq!(parse("/restore 3"), Command::Restore(3));
        assert_eq!(
            parse("/edit 1 new  words "),
            Command::Edit(1, "new  words".into())
        );
        assert_eq!(parse("/tier"), Command::Tier(None));
        assert_eq!(parse("/quiet off"), Command::Quiet(Some("off".into())));
        assert_eq!(parse("/chats"), Command::Chats(false));
        assert_eq!(parse("/chats archived"), Command::Chats(true));
        assert_eq!(parse("/chat 2"), Command::Chat(2));
        assert_eq!(parse("/who Ann Lee"), Command::Who("Ann Lee".into()));
        assert_eq!(
            parse("/mode Ann Lee frozen"),
            Command::Mode("Ann Lee".into(), "frozen".into())
        );
        assert_eq!(
            parse("/mode disabled"),
            Command::Mode(String::new(), "disabled".into())
        );
        assert_eq!(parse("/archive"), Command::Archive(true));
        assert_eq!(parse("/unarchive"), Command::Archive(false));
        assert_eq!(
            parse("/rename Night shift"),
            Command::Rename("Night shift".into())
        );
        assert_eq!(parse("/rename"), Command::Rename(String::new()));
        assert_eq!(parse("/members"), Command::Members);
        assert_eq!(parse("/leave"), Command::Leave);
        assert_eq!(parse("/read"), Command::Read(false));
        assert_eq!(parse("/read all"), Command::Read(true));
        assert_eq!(
            parse("/ask \"Ann Lee\" what is in the east wing"),
            Command::Ask("Ann Lee".into(), "what is in the east wing".into())
        );
        assert_eq!(parse("/requests"), Command::Requests);
        assert_eq!(parse("/clock"), Command::Clock);
        assert_eq!(
            parse("/find tea brewing"),
            Command::Find("tea brewing".into())
        );
        assert_eq!(parse("/waiting"), Command::Waiting);
        assert_eq!(
            parse("/birthday 03-14"),
            Command::Birthday(Some("03-14".into()))
        );
    }

    #[test]
    fn save_subcommands_and_quoting() {
        use Save::*;
        let save = |line| match parse(line) {
            Command::Save(s) => s,
            other => panic!("{line}: {other:?}"),
        };
        assert_eq!(save("/save"), List);
        assert_eq!(save("/save new second"), New("second".into()));
        assert_eq!(save("/save switch \"story 2\""), Switch("story 2".into()));
        assert_eq!(save("/save copy a b"), Copy("a".into(), "b".into()));
        assert_eq!(
            save("/save rename a \"b c\""),
            Rename("a".into(), "b c".into())
        );
        assert_eq!(save("/save delete a"), Delete("a".into()));
        assert_eq!(save("/save logout"), Logout);
        assert_eq!(save("/logout"), Logout);
        assert_eq!(
            save("/save export a \"/tmp/my dir/a.sqlite\""),
            Export("a".into(), "/tmp/my dir/a.sqlite".into())
        );
        assert_eq!(
            save("/save import /x.sqlite"),
            Import("/x.sqlite".into(), None)
        );
        assert_eq!(
            save("/save import /x.sqlite name"),
            Import("/x.sqlite".into(), Some("name".into()))
        );
    }

    #[test]
    fn storage_shows_or_clears_named_caches() {
        use StorageAct::*;
        assert_eq!(parse("/storage"), Command::Storage(Show));
        assert_eq!(
            parse("/storage clear audio"),
            Command::Storage(Clear(vec![StorageKind::Audio]))
        );
        assert_eq!(
            parse("/storage clear all"),
            Command::Storage(Clear(vec![StorageKind::Avatars, StorageKind::Audio]))
        );
        for bad in ["/storage clear", "/storage clear saves", "/storage x"] {
            assert!(matches!(parse(bad), Command::Invalid(_)), "{bad}");
        }
    }

    #[test]
    fn misuse_explains_itself() {
        for bad in [
            "/talk",
            "/group solo",
            "/history 0",
            "/history x",
            "/resend 0",
            "/resend x",
            "/sources 0",
            "/search",
            "/go",
            "/go 0",
            "/go x",
            "/name",
            "/forget",
            "/forget 0",
            "/restore x",
            "/edit 1",
            "/edit x text",
            "/save new",
            "/save copy a",
            "/save frobnicate",
            "/password secret",
            "/chats x",
            "/chat 0",
            "/who",
            "/mode Ann",
            "/mode Ann loud",
            "/read some",
            "/ask Ann",
            "/find",
            "/nope",
        ] {
            assert!(matches!(parse(bad), Command::Invalid(_)), "{bad}");
        }
    }
}
