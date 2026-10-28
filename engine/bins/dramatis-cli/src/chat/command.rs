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
    Memories(Option<String>),
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

/// Lines shown by `/history` without an argument.
pub const HISTORY_DEFAULT: usize = 10;

pub const HELP: &str = "\
/list [filter]         persons who can be talked to, and whether the host is available
/talk <person>         open the direct channel with a person (id, name or alias)
/group <p1> <p2> ...   start a group channel with several persons
/host                  switch to the host's channel
/go <n>                take choice n from the list the host just offered
/name <new name>       change the name you are called by
/wrapup                end the current conversation and show the memories it wrote
/memories [person]     stored memories, or those a person may recall
/history [n]           reprint the last n messages of this channel (default 10)
/resend [n]            take back your n-th last message (default 1) and everything after it,
                       and put its text back in the input line to edit and send again
/sources [n]           list in full the sources of the n-th last reply that has any (default 1)
/search <query>        retrieval only, as the current speaker would see it
/stats                 model calls made in this session
/help, /quit           this text; leave (Ctrl-D also works)
//text                 send a message that starts with a slash
anything else is sent to the current channel

Typing: Enter starts a new line; Ctrl-J or Alt-Enter sends (Alt is Option set as Meta in
macOS Terminal and iTerm). A line that starts with a slash is a command and Enter runs it.
With --plain each input line is one message.";

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
            "/nope",
        ] {
            assert!(matches!(parse(bad), Command::Invalid(_)), "{bad}");
        }
    }
}
