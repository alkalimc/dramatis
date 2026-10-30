//! Tab completion for the line editor: command names, the fixed words some commands take,
//! the names persons answer to, and save names. The editor runs on a blocking thread and cannot ask the engine, so the
//! session fills a shared snapshot and the completer only reads it.

use std::sync::{Arc, RwLock};

use rustyline::completion::Pair;

/// Every command's primary name, in the order `/help` lists them.
const COMMANDS: &[&str] = &[
    "list",
    "talk",
    "group",
    "host",
    "go",
    "open",
    "chats",
    "chat",
    "rename",
    "members",
    "invite",
    "remove",
    "archive",
    "unarchive",
    "leave",
    "read",
    "who",
    "mode",
    "ask",
    "requests",
    "clock",
    "find",
    "waiting",
    "birthday",
    "name",
    "bio",
    "wrapup",
    "memories",
    "forget",
    "restore",
    "edit",
    "tier",
    "quiet",
    "events",
    "lang",
    "password",
    "save",
    "logout",
    "history",
    "resend",
    "sources",
    "source",
    "library",
    "search",
    "stats",
    "storage",
    "about",
    "help",
    "quit",
    "exit",
];

/// The one fixed word that follows each of these commands (and their short forms).
const FIXED: &[(&[&str], &[&str])] = &[
    (
        &["tier"],
        &["low", "middle", "high", "extra_high", "max", "ultra"],
    ),
    (&["quiet"], &["on", "off"]),
    (&["events"], &["on", "off"]),
    (&["lang"], &["corpus", "en"]),
    (&["library", "lib"], &["off"]),
    (&["read"], &["all"]),
    (&["chats", "c"], &["archived"]),
    (&["password"], &["on", "off", "change"]),
    (&["about"], &["check"]),
];

/// What `/storage clear` takes.
const STORAGE_KINDS: &[&str] = &["avatars", "audio", "all"];

const MODES: &[&str] = &["enabled", "frozen", "disabled"];

const SAVE_VERBS: &[&str] = &[
    "list", "new", "switch", "copy", "rename", "delete", "export", "import",
];

/// What can be completed to, as of the last refresh.
#[derive(Default)]
pub struct Snapshot {
    /// Display names, other forms and aliases of persons who can be talked to.
    pub names: Vec<String>,
    /// Their ids, offered only when no name matches.
    pub ids: Vec<String>,
    pub saves: Vec<String>,
}

impl Snapshot {
    /// Sorted and without repeats, so the candidate lists come out stable.
    pub fn new(mut names: Vec<String>, mut ids: Vec<String>, mut saves: Vec<String>) -> Self {
        for list in [&mut names, &mut ids, &mut saves] {
            list.retain(|s| !s.trim().is_empty());
            list.sort();
            list.dedup();
        }
        Self { names, ids, saves }
    }
}

/// The snapshot shared between the session (writer) and the editor (reader).
#[derive(Clone, Default)]
pub struct Completions(Arc<RwLock<Snapshot>>);

impl Completions {
    pub fn set(&self, snapshot: Snapshot) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = snapshot;
    }

    pub fn complete(&self, line: &str, pos: usize) -> (usize, Vec<Pair>) {
        complete(line, pos, &self.0.read().unwrap_or_else(|e| e.into_inner()))
    }
}

fn pair(shown: &str, replacement: String) -> Pair {
    Pair {
        display: shown.to_owned(),
        replacement,
    }
}

fn starts_with_fold(candidate: &str, prefix: &str) -> bool {
    candidate.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// The candidates for the text before the cursor: where the replaced word starts, and
/// what it may become. Only a single-line draft that starts with a slash completes.
pub fn complete(line: &str, pos: usize, data: &Snapshot) -> (usize, Vec<Pair>) {
    let none = (pos, Vec::new());
    let Some(head) = line.get(..pos) else {
        return none;
    };
    if !head.starts_with('/') || head.starts_with("//") || head.contains('\n') {
        return none;
    }
    let Some(gap) = head.find(char::is_whitespace) else {
        let prefix = &head[1..];
        let found = COMMANDS
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| pair(c, (*c).to_owned()))
            .collect();
        return (1, found);
    };
    let command = &head[1..gap];
    let args_at = head.len() - head[gap..].trim_start().len();
    let args = &head[args_at..];
    match command {
        "talk" | "t" => (args_at, persons(args, data, true)),
        "who" => (args_at, persons(args, data, true)),
        "mode" => {
            // The person, then the mode after it.
            let words: Vec<&str> = args.split_whitespace().collect();
            let after_person = words.len() > 1 || (words.len() == 1 && args.ends_with(' '));
            if after_person {
                let at = args.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                let partial = &args[at..];
                let found = MODES
                    .iter()
                    .filter(|m| m.starts_with(partial))
                    .map(|m| pair(m, (*m).to_owned()))
                    .collect();
                return (args_at + at, found);
            }
            (args_at, persons(args, data, true))
        }
        "group" | "g" => {
            // Each member is one whitespace-free word.
            let at = args.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            (args_at + at, persons(&args[at..], data, false))
        }
        "invite" => {
            // Names are separated by commas or spaces; the word after the last one completes.
            let at = args.rfind([',', ' ']).map_or(0, |i| i + 1);
            (args_at + at, persons(args[at..].trim_start(), data, false))
        }
        "remove" => (args_at, persons(args, data, true)),
        "memories" | "mem" => (args_at, persons(args, data, true)),
        "ask" => {
            // The person is the first word; the question follows.
            if args.contains(char::is_whitespace) {
                none
            } else {
                (args_at, persons(args, data, false))
            }
        }
        "list" | "ls" => {
            // Any number of filter words, then a name fragment.
            let (_, start, partial, _) = last_word(args);
            (args_at + start, fixed(&partial, super::filter::WORDS))
        }
        "save" => save(args, args_at, data),
        "storage" => storage(args, args_at),
        _ => match FIXED.iter().find(|(names, _)| names.contains(&command)) {
            Some((_, words)) => (args_at, fixed(args, words)),
            None => none,
        },
    }
}

/// The fixed words that start with what is typed, while it is still the first word.
fn fixed(args: &str, words: &[&str]) -> Vec<Pair> {
    if args.contains(char::is_whitespace) {
        return Vec::new();
    }
    words
        .iter()
        .filter(|w| starts_with_fold(w, args))
        .map(|w| pair(w, (*w).to_owned()))
        .collect()
}

/// `/storage clear <kind>`.
fn storage(args: &str, args_at: usize) -> (usize, Vec<Pair>) {
    let (done, start, partial, _) = last_word(args);
    let at = args_at + start;
    let pick = |words: &[&str]| fixed(&partial, words);
    match (done, args.split_whitespace().next()) {
        (0, _) => (at, pick(&["clear"])),
        (1, Some("clear")) => (at, pick(STORAGE_KINDS)),
        _ => (at, Vec::new()),
    }
}

/// Persons matching `prefix` on any name; their ids only when no name does.
fn persons(prefix: &str, data: &Snapshot, spaces: bool) -> Vec<Pair> {
    let pick = |list: &[String]| -> Vec<Pair> {
        list.iter()
            .filter(|s| (spaces || !s.contains(char::is_whitespace)) && starts_with_fold(s, prefix))
            .map(|s| pair(s, s.clone()))
            .collect()
    };
    let by_name = pick(&data.names);
    if by_name.is_empty() {
        pick(&data.ids)
    } else {
        by_name
    }
}

/// `/save <verb> <name> ...`: the verb, then save names where the verb takes one.
fn save(args: &str, args_at: usize, data: &Snapshot) -> (usize, Vec<Pair>) {
    let (done, start, partial, open) = last_word(args);
    let at = args_at + start;
    if done == 0 {
        let found = SAVE_VERBS
            .iter()
            .filter(|v| v.starts_with(partial.as_str()))
            .map(|v| pair(v, (*v).to_owned()))
            .collect();
        return (at, found);
    }
    let verb = args.split_whitespace().next().unwrap_or("");
    // The existing save is the first name of copy, rename, delete and export.
    if done != 1 || !matches!(verb, "switch" | "delete" | "copy" | "rename" | "export") {
        return (at, Vec::new());
    }
    let found = data
        .saves
        .iter()
        .filter(|s| starts_with_fold(s, &partial))
        .map(|s| {
            let quoted = open || s.contains(char::is_whitespace);
            pair(
                s,
                if quoted {
                    format!("\"{s}\"")
                } else {
                    s.clone()
                },
            )
        })
        .collect();
    (at, found)
}

/// The word being typed in `args`: how many words precede it, where it starts (at its
/// opening quote, if any), its text without the quote, and whether a quote is open.
fn last_word(args: &str) -> (usize, usize, String, bool) {
    let (mut done, mut start, mut word) = (0, 0, String::new());
    let (mut quoted, mut any) = (false, false);
    for (i, c) in args.char_indices() {
        match c {
            '"' => {
                if !any {
                    start = i;
                }
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    done += 1;
                    any = false;
                    word.clear();
                }
                start = i + c.len_utf8();
            }
            c => {
                if !any {
                    start = i;
                }
                word.push(c);
                any = true;
            }
        }
    }
    (done, start, word, quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Snapshot {
        Snapshot::new(
            ["Ann Lee", "Anchor", "Bo", "Annie", "Ann Lee"]
                .map(String::from)
                .to_vec(),
            ["p_ann", "p_bo", "p_zed"].map(String::from).to_vec(),
            ["main", "story 2", "side"].map(String::from).to_vec(),
        )
    }

    fn shown(found: &(usize, Vec<Pair>)) -> Vec<&str> {
        found.1.iter().map(|p| p.display.as_str()).collect()
    }

    fn at(line: &str) -> (usize, Vec<Pair>) {
        complete(line, line.len(), &data())
    }

    #[test]
    fn command_names_follow_the_slash() {
        let found = at("/");
        assert_eq!(found.0, 1);
        assert_eq!(found.1.len(), COMMANDS.len());
        assert_eq!(
            shown(&at("/s")),
            ["save", "sources", "source", "search", "stats", "storage"]
        );
        assert_eq!(shown(&at("/tal")), ["talk"]);
        assert!(at("/zzz").1.is_empty());
    }

    #[test]
    fn talk_completes_any_name_by_prefix_ignoring_case() {
        let found = at("/talk an");
        assert_eq!(found.0, 6);
        assert_eq!(shown(&found), ["Anchor", "Ann Lee", "Annie"]);
        assert_eq!(shown(&at("/t BO")), ["Bo"]);
        // Several words are one name for /talk.
        assert_eq!(shown(&at("/talk Ann L")), ["Ann Lee"]);
        assert_eq!(shown(&at("/talk ")).len(), 4);
    }

    #[test]
    fn ids_are_offered_only_when_no_name_matches() {
        assert_eq!(shown(&at("/talk p_")), ["p_ann", "p_bo", "p_zed"]);
        assert!(at("/talk q").1.is_empty());
    }

    #[test]
    fn group_completes_the_last_word_and_skips_names_with_spaces() {
        let found = at("/group Bo An");
        assert_eq!(found.0, 10);
        assert_eq!(shown(&found), ["Anchor", "Annie"]);
        assert_eq!(shown(&at("/group ")), ["Anchor", "Annie", "Bo"]);
    }

    #[test]
    fn invite_completes_after_a_comma_and_remove_any_name() {
        let found = at("/invite Bo, An");
        assert_eq!(found.0, 12);
        assert_eq!(shown(&found), ["Anchor", "Annie"]);
        assert_eq!(shown(&at("/remove BO")), ["Bo"]);
        assert_eq!(shown(&at("/remove Ann L")), ["Ann Lee"]);
    }

    #[test]
    fn save_completes_verbs_then_save_names() {
        assert_eq!(shown(&at("/save s")), ["switch"]);
        assert_eq!(shown(&at("/save ")).len(), SAVE_VERBS.len());
        for verb in ["switch", "delete", "export", "rename", "copy"] {
            let found = at(&format!("/save {verb} s"));
            assert_eq!(shown(&found), ["side", "story 2"], "{verb}");
            assert_eq!(found.0, 7 + verb.len());
        }
        // Names with spaces are quoted for the parser.
        let found = at("/save switch st");
        assert_eq!(found.1[0].replacement, "\"story 2\"");
        // An open quote is kept and closed.
        let found = at("/save switch \"st");
        assert_eq!(found.0, 13);
        assert_eq!(found.1[0].replacement, "\"story 2\"");
    }

    #[test]
    fn save_names_are_not_offered_where_a_new_name_goes() {
        assert!(at("/save new m").1.is_empty());
        assert!(at("/save rename main m").1.is_empty());
        assert!(at("/save copy main ").1.is_empty());
        assert!(at("/save import m").1.is_empty());
    }

    #[test]
    fn list_completes_its_filter_words_at_every_position() {
        assert_eq!(
            shown(&at("/list ")),
            [
                "all", "enabled", "frozen", "disabled", "talked", "never", "persona"
            ]
        );
        assert_eq!(shown(&at("/list d")), ["disabled"]);
        let found = at("/ls enabled ta");
        assert_eq!(found.0, 12);
        assert_eq!(shown(&found), ["talked"]);
    }

    #[test]
    fn who_and_mode_complete_a_person_and_mode_then_the_mode() {
        assert_eq!(shown(&at("/who Ann")), ["Ann Lee", "Annie"]);
        assert_eq!(shown(&at("/mode Bo ")), ["enabled", "frozen", "disabled"]);
        let found = at("/mode Bo fr");
        assert_eq!(found.0, 9);
        assert_eq!(shown(&found), ["frozen"]);
        assert_eq!(shown(&at("/mode An")), ["Anchor", "Ann Lee", "Annie"]);
    }

    #[test]
    fn nothing_completes_elsewhere() {
        assert!(at("/go ").1.is_empty());
        assert!(at("hello").1.is_empty());
        assert!(at("//talk An").1.is_empty());
        assert!(at("/talk An\nmore").1.is_empty());
        // Only the text before the cursor counts.
        assert_eq!(shown(&complete("/talk Bo and", 8, &data())), ["Bo"]);
    }

    #[test]
    fn wide_names_complete_by_character() {
        let wide = Snapshot::new(
            vec!["\u{5c0f}\u{660e}".into(), "\u{5c0f}\u{7ea2}".into()],
            vec![],
            vec![],
        );
        let line = "/talk \u{5c0f}";
        let found = complete(line, line.len(), &wide);
        assert_eq!(found.0, 6);
        assert_eq!(found.1.len(), 2);
    }

    #[test]
    fn every_command_the_parser_accepts_completes_by_name() {
        for name in ["logout", "open", "exit", "save", "quit"] {
            assert!(COMMANDS.contains(&name), "{name}");
        }
        assert_eq!(shown(&at("/lo")), ["logout"]);
        assert_eq!(shown(&at("/ex")), ["exit"]);
    }

    #[test]
    fn fixed_words_follow_the_commands_that_take_them() {
        assert_eq!(shown(&at("/tier ")).len(), 6);
        assert_eq!(shown(&at("/tier ex")), ["extra_high"]);
        assert_eq!(shown(&at("/quiet o")), ["on", "off"]);
        assert_eq!(shown(&at("/events of")), ["off"]);
        assert_eq!(shown(&at("/lang ")), ["corpus", "en"]);
        assert_eq!(shown(&at("/library ")), ["off"]);
        assert_eq!(shown(&at("/lib o")), ["off"]);
        assert_eq!(shown(&at("/read a")), ["all"]);
        assert_eq!(shown(&at("/chats ar")), ["archived"]);
        assert_eq!(shown(&at("/password ")), ["on", "off", "change"]);
        assert_eq!(at("/password c").0, 10);
        // Only the first word is a fixed word.
        assert!(at("/quiet on ").1.is_empty());
        assert!(at("/tier zzz").1.is_empty());
    }

    #[test]
    fn storage_completes_clear_then_the_kind() {
        assert_eq!(shown(&at("/storage ")), ["clear"]);
        assert_eq!(shown(&at("/storage cl")), ["clear"]);
        let found = at("/storage clear a");
        assert_eq!(found.0, 15);
        assert_eq!(shown(&found), ["avatars", "audio", "all"]);
        assert_eq!(shown(&at("/storage clear au")), ["audio"]);
        assert!(at("/storage clear all ").1.is_empty());
        assert!(at("/storage other ").1.is_empty());
    }

    #[test]
    fn ask_and_memories_complete_a_person() {
        let found = at("/ask An");
        assert_eq!(found.0, 5);
        assert_eq!(shown(&found), ["Anchor", "Annie"]);
        // The question is free text.
        assert!(at("/ask Bo wh").1.is_empty());
        assert_eq!(shown(&at("/memories Ann L")), ["Ann Lee"]);
        assert_eq!(shown(&at("/mem BO")), ["Bo"]);
    }
}
