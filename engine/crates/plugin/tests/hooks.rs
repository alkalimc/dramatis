//! The host against real child processes: the example plugin and deliberately broken ones.
//! Needs Python; the test is skipped (and says so) where there is none. No network.

use std::path::{Path, PathBuf};
use std::process::Command;

use plugin::{Audience, Fail, Host};
use serde_json::json;

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// A fresh plugins folder under the system temp dir.
fn home(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("dramatis-plugin-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn example(dir: &Path, py: &str) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/plugins/dice");
    let dst = dir.join("dice");
    copy_dir(&src, &dst);
    let manifest = std::fs::read_to_string(dst.join("plugin.toml")).unwrap();
    std::fs::write(
        dst.join("plugin.toml"),
        manifest.replace("\"python3\"", &format!("\"{py}\"")),
    )
    .unwrap();
}

/// A plugin whose whole program is `code`, taking the given hooks.
fn script(dir: &Path, py: &str, name: &str, hooks: &str, timeout_ms: u32, code: &str) {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("p.py"), code).unwrap();
    std::fs::write(
        d.join("plugin.toml"),
        format!(
            "name = \"{name}\"\nprotocol = 1\ncommand = [\"{py}\", \"p.py\"]\nhooks = {hooks}\ntimeout_ms = {timeout_ms}\n"
        ),
    )
    .unwrap();
}

fn host(dir: &Path) -> Host {
    Host::new(dir.to_path_buf(), Some(dir.join("logs")), "test")
}

/// Answers `initialize`, then runs `body` for each following request line.
fn serve(body: &str) -> String {
    format!(
        "import sys, json, time\nfor line in sys.stdin:\n    req = json.loads(line)\n    if req.get('method') == 'initialize':\n        print(json.dumps({{'id': req['id'], 'result': {{}}}}), flush=True)\n        continue\n{}\n",
        body.lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[test]
fn disabled_by_default_and_listed() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = home("list");
    example(&dir, py);
    std::fs::create_dir_all(dir.join("broken")).unwrap();
    std::fs::write(dir.join("broken/plugin.toml"), "name = \"x\"").unwrap();
    let h = host(&dir);
    assert!(h.tools(Audience::Host).is_empty());
    assert!(h.prompt_lines(&json!({})).is_empty());
    let list = h.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "broken");
    assert!(list[0].problem.is_some() && !list[0].enabled);
    assert_eq!(list[1].name, "dice");
    assert!(!list[1].enabled && list[1].problem.is_none());
    assert_eq!(list[1].tools, vec!["dice__roll"]);
    assert!(h.set_enabled("broken", true).is_err());
    assert!(h.set_enabled("nope", true).is_err());
    h.set_enabled("dice", true).unwrap();
    assert!(h.list()[1].enabled);
    // A second host over the same folder sees the same choice.
    assert_eq!(host(&dir).tools(Audience::Persons).len(), 1);
    h.set_enabled("dice", false).unwrap();
    assert!(h.tools(Audience::Persons).is_empty());
}

#[test]
fn example_plugin_hooks() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = home("example");
    example(&dir, py);
    let h = host(&dir);
    h.set_enabled("dice", true).unwrap();

    let tools = h.tools(Audience::Host);
    assert_eq!(tools[0].name, "dice__roll");
    assert_eq!(
        tools[0].parameters["properties"]["sides"]["type"],
        "integer"
    );

    let out = h
        .call_tool("dice__roll", r#"{"sides": 6, "count": 3}"#, &json!({}))
        .unwrap();
    let (dice, total) = out.split_once(" (total ").unwrap();
    let rolled: Vec<u32> = dice.split(' ').map(|n| n.parse().unwrap()).collect();
    assert_eq!(rolled.len(), 3);
    assert!(rolled.iter().all(|n| (1..=6).contains(n)));
    assert_eq!(total, format!("{})", rolled.iter().sum::<u32>()));

    // A plugin's own error is returned, and does not count as a crash.
    let err = h
        .call_tool("dice__roll", r#"{"sides": 1}"#, &json!({}))
        .unwrap_err();
    assert!(matches!(err, Fail::Plugin(_)), "{err:?}");
    assert!(
        h.call_tool("dice__roll", r#"{"sides": 6}"#, &json!({}))
            .is_ok()
    );
    assert!(matches!(
        h.call_tool("dice__nope", "{}", &json!({})),
        Err(Fail::Off)
    ));

    let masked = h.tool_result(
        "search",
        "{}",
        "write to a.b@example.org today".into(),
        &json!({}),
    );
    assert_eq!(masked, "write to [address] today");
    assert_eq!(
        h.tool_result("search", "{}", "no address".into(), &json!({})),
        "no address"
    );

    // The prompt hook is silent until messages were observed; the message hook is async.
    assert!(h.prompt_lines(&json!({})).is_empty());
    h.message(json!({"id": 1, "text": "hi"}));
    h.message(json!({"id": 2, "text": "again"}));
    let mut lines = Vec::new();
    for _ in 0..50 {
        lines = h.prompt_lines(&json!({}));
        if !lines.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains('2'), "{lines:?}");
}

#[test]
fn failures_are_isolated_and_stop_the_plugin() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = home("failures");
    script(
        &dir,
        py,
        "crash",
        "[\"prompt\"]",
        1000,
        &serve("sys.exit(1)"),
    );
    script(
        &dir,
        py,
        "hang",
        "[\"prompt\"]",
        100,
        &serve("time.sleep(30)"),
    );
    script(
        &dir,
        py,
        "junk",
        "[\"prompt\"]",
        1000,
        &serve("print('not json', flush=True)"),
    );
    script(
        &dir,
        py,
        "huge",
        "[\"prompt\"]",
        1000,
        &serve("print('x' * 200000, flush=True)"),
    );
    script(
        &dir,
        py,
        "chatty",
        "[\"prompt\"]",
        1000,
        &serve(
            "lines = ['  one\\nline  ', ''] + ['y' * 900] * 10\nprint(json.dumps({'id': req['id'], 'result': {'lines': lines}}), flush=True)",
        ),
    );
    script(
        &dir,
        py,
        "gone",
        "[\"prompt\"]",
        1000,
        "raise SystemExit(0)\n",
    );
    let h = host(&dir);
    for name in ["crash", "hang", "junk", "huge", "chatty", "gone"] {
        h.set_enabled(name, true).unwrap();
    }
    let started = std::time::Instant::now();
    for round in 0..4 {
        let lines = h.prompt_lines(&json!({"text": "hi"}));
        // Only the chatty plugin contributes; the others never break the turn.
        assert!(lines[0] == "one line", "{lines:?}");
        assert!(
            lines.len() <= 1 + plugin::host::LINES_MAX,
            "round {round}: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|l| l.chars().count() <= plugin::host::LINE_CHARS)
        );
        assert!(
            lines.iter().map(|l| l.chars().count()).sum::<usize>()
                <= plugin::host::LINES_TOTAL_CHARS
        );
    }
    assert!(started.elapsed().as_secs() < 20);
    let list = h.list();
    let status = |n: &str| list.iter().find(|l| l.name == n).unwrap().status.clone();
    for n in ["crash", "hang", "junk", "huge", "gone"] {
        assert!(
            status(n).is_some_and(|s| s.contains("stopped")),
            "{n}: {:?}",
            status(n)
        );
    }
    assert_eq!(status("chatty"), None);
    // Turning a stopped plugin off and on gives it another chance.
    h.set_enabled("crash", false).unwrap();
    h.set_enabled("crash", true).unwrap();
    assert_eq!(
        h.list().iter().find(|l| l.name == "crash").unwrap().status,
        None
    );
}

#[test]
fn plugins_get_a_minimal_environment() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = home("env");
    // SAFETY: only this test sets this variable, before any thread reads it.
    unsafe { std::env::set_var("DRAMATIS_TEST_SECRET", "hunter2") };
    script(
        &dir,
        py,
        "env",
        "[\"prompt\"]",
        2000,
        &serve(
            "import os\nprint(json.dumps({'id': req['id'], 'result': {'lines': [','.join(sorted(k for k in os.environ if k.startswith('DRAMATIS_')))]}}), flush=True)",
        ),
    );
    let h = host(&dir);
    h.set_enabled("env", true).unwrap();
    let lines = h.prompt_lines(&json!({}));
    assert_eq!(lines, vec!["DRAMATIS_PLUGIN_DIR,DRAMATIS_PLUGIN_NAME"]);
}
