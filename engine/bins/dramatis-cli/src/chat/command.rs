//! The REPL's line grammar: a slash command, or a message for the current channel.

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
    /// Take one of the choices the host last offered (1-based).
    Go(usize),
    /// Set the user's display name.
    Name(String),
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
    Help,
    Quit,
    /// A known command used wrongly, or an unknown one: the text to show.
    Invalid(String),
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
}

const SAVE_USAGE: &str = "usage: /save [new <name> | switch <name> | copy <from> <to> | \
rename <from> <to> | delete <name> | export <name> <path> | import <path> [name]]";

/// Split on spaces, keeping "double quoted" text together.
fn words(args: &str) -> Vec<String> {
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
        "go" | "open" => match args.parse::<usize>() {
            Ok(n) if n > 0 => Command::Go(n),
            _ => Command::Invalid("usage: /go <n>, a number from the host's list".into()),
        },
        "name" if args.is_empty() => Command::Invalid("usage: /name <new name>".into()),
        "name" => Command::Name(args.to_owned()),
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
        assert_eq!(parse("/forget 2"), Command::Forget(2));
        assert_eq!(parse("/restore 3"), Command::Restore(3));
        assert_eq!(
            parse("/edit 1 new  words "),
            Command::Edit(1, "new  words".into())
        );
        assert_eq!(parse("/tier"), Command::Tier(None));
        assert_eq!(parse("/quiet off"), Command::Quiet(Some("off".into())));
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
            "/nope",
        ] {
            assert!(matches!(parse(bad), Command::Invalid(_)), "{bad}");
        }
    }
}
