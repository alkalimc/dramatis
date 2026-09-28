//! Contract: the tool schemas the model sees.
//!
//! Both lists, in both wire shapes, are snapshotted byte for byte. A schema change drops
//! the cache for every session, and changes what a stored call parses as, so it has to be
//! a deliberate act: regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p api --test tools`
//! and commit the new file.

use std::path::PathBuf;

use api::endpoints::WireApi;
use api::tools::{Descriptions, host_tools, shared_tools, to_wire};
use serde_json::json;

fn render() -> String {
    let none = Descriptions::new();
    let (shared, host) = (shared_tools(&none), host_tools(&none));
    let mut all = json!({
        "shared": { "chat": to_wire(&shared, WireApi::Chat), "responses": to_wire(&shared, WireApi::Responses) },
        "host": { "chat": to_wire(&host, WireApi::Chat), "responses": to_wire(&host, WireApi::Responses) },
    });
    all.sort_all_objects();
    serde_json::to_string_pretty(&all).unwrap() + "\n"
}

#[test]
fn tool_schemas_match_snapshot() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/tools.snapshot.json");
    let now = render();
    assert_eq!(
        now,
        render(),
        "tool schema serialization is not deterministic"
    );
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &now).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == now,
        "tool schemas differ from {}.\n\
         If the change is intended, run `UPDATE_SNAPSHOTS=1 cargo test -p api --test tools` \
         and commit the snapshot.",
        path.display()
    );
}

#[test]
fn every_parsed_name_is_offered() {
    let none = Descriptions::new();
    let shared: Vec<_> = shared_tools(&none).iter().map(|t| t.name).collect();
    let host: Vec<_> = host_tools(&none).iter().map(|t| t.name).collect();
    assert_eq!(
        shared,
        [
            "search",
            "get_chunk",
            "request_join",
            "remember",
            "report",
            "wrapup"
        ]
    );
    assert_eq!(
        host,
        [
            "digest",
            "find_people",
            "ask",
            "create_group",
            "set_mode",
            "search",
            "remember",
            "forget"
        ]
    );
    for name in &shared {
        // Names parse; argument errors are fine here, unknown names are not.
        assert!(!matches!(
            api::tools::SharedCall::parse(name, "{}"),
            Err(api::tools::CallError::Unknown(_))
        ));
    }
    for name in &host {
        assert!(!matches!(
            api::tools::HostCall::parse(name, "{}"),
            Err(api::tools::CallError::Unknown(_))
        ));
    }
}
