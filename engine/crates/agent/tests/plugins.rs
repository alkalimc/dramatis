//! Plugins in a turn: the hooks reach the model's request without touching the cached
//! prefix, a broken plugin never fails a turn, and the tool list follows the enabled set.
//! Needs Python for the child processes; skipped without it. No network.

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use api::endpoints::WireApi;
use common::*;
use serde_json::json;

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
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

/// A plugins folder holding the example plugin, turned on, and the env that uses it.
fn with_example(
    py: &str,
    extra: impl FnOnce(&Path, &str),
) -> (Env, Arc<plugin::Host>, tempdir::Dir) {
    let dir = tempdir::Dir::new();
    let plugins = dir.0.join("plugins");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/plugins/dice");
    copy_dir(&src, &plugins.join("dice"));
    let manifest = plugins.join("dice/plugin.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("\"python3\"", &format!("\"{py}\""))).unwrap();
    extra(&plugins, py);
    let host = Arc::new(plugin::Host::new(plugins.clone(), None, "test"));
    host.set_enabled("dice", true).unwrap();
    let e = env(WireApi::Chat);
    let agent = Arc::try_unwrap(e.agent)
        .ok()
        .expect("one owner")
        .with_plugins(host.clone());
    let e = Env {
        agent: Arc::new(agent),
        ..e
    };
    (e, host, dir)
}

fn tools_of(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn hooks_reach_the_request_and_leave_the_prefix_alone() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let (e, host, _dir) = with_example(py, |_, _| {});
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([
        call("dice__roll", json!({"sides": 6, "count": 2})),
        say("Rolled."),
        say("Again."),
    ]);
    e.agent
        .send(ch, "roll for me, mail me at a@b.org", None)
        .await
        .unwrap();
    // The message hook is asynchronous: wait until the plugin has counted the messages.
    for _ in 0..100 {
        if !host.prompt_lines(&json!({})).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    e.agent.send(ch, "once more", None).await.unwrap();

    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 3);
    assert_prefix_chain(&bodies);
    let json = e.fake.json();
    for v in &json {
        assert!(
            tools_of(v).contains(&"dice__roll".to_string()),
            "{:?}",
            tools_of(v)
        );
        assert_eq!(head(v), head(&json[0]));
    }
    let text = |i: usize| String::from_utf8(bodies[i].clone()).unwrap();
    // The tool ran in the plugin and its result went back to the model.
    assert!(text(1).contains("(total "));
    // The note joins a turn's own entry, after the cached prefix (asserted above).
    assert!(text(2).contains("Note from the dice plugin"));
    assert_eq!(segments(&e.agent, ch).len(), 1);
}

#[tokio::test]
async fn a_broken_plugin_never_fails_a_turn() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let (e, host, _dir) = with_example(py, |plugins, py| {
        let d = plugins.join("bad");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("p.py"), "import sys\nsys.exit(3)\n").unwrap();
        std::fs::write(
            d.join("plugin.toml"),
            format!(
                "name = \"bad\"\nprotocol = 1\ncommand = [\"{py}\", \"p.py\"]\nhooks = [\"prompt\", \"tool_result\", \"message\"]\n[[tools]]\nname = \"t\"\ndescription = \"Never answers.\"\n"
            ),
        )
        .unwrap();
    });
    host.set_enabled("bad", true).unwrap();
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake
        .push([call("bad__t", json!({})), say("Done."), say("Still here.")]);
    e.agent.send(ch, "try it", None).await.unwrap();
    e.agent.send(ch, "and again", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 3);
    assert_eq!(segments(&e.agent, ch).len(), 1);
    // The failed call came back to the model as an ordinary tool result.
    assert!(e.fake.json()[1].to_string().contains("tool"));
}

#[tokio::test]
async fn turning_a_plugin_off_rolls_the_segment_onto_the_smaller_tool_list() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let (e, host, _dir) = with_example(py, |_, _| {});
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([say("a")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    host.set_enabled("dice", false).unwrap();
    e.fake
        .push([call("wrapup", json!({"facts": []})), say("b")]);
    e.agent.send(ch, "hi again", None).await.unwrap();
    assert_eq!(segments(&e.agent, ch).len(), 2);
    let json = e.fake.json();
    assert!(tools_of(&json[0]).contains(&"dice__roll".to_string()));
    assert!(!tools_of(json.last().unwrap()).contains(&"dice__roll".to_string()));
}
