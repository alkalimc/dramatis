//! Fixtures are saves as the releases wrote them, built from the steps kept in this crate.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use world::World;

use crate::convert::{Failure, Pack, backup_of, convert, in_place_part, swap_in};
use crate::ladder::{self, Family, REQUEST_FAMILY, STEPS};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("dramatis-migrate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pack() -> Pack {
    Pack {
        name: "demo".into(),
        version: "1.1.0".into(),
    }
}

/// A save as the ladder family wrote it at `schema`: rows in every table that layout has,
/// prompt material in the model log, and a settings key no release reads.
fn ladder_save(path: &Path, schema: i64, password: Option<&str>) {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    for step in STEPS.iter().filter(|s| s.to <= schema) {
        conn.execute_batch(step.sql).unwrap();
    }
    conn.pragma_update(None, "user_version", schema).unwrap();
    conn.execute_batch(
        "INSERT INTO channel(id, kind, origin) VALUES (1, 'direct', 'user');
         INSERT INTO channel_participant VALUES (1, 'ann');
         INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'ann', 120, 'enabled');
         INSERT INTO message(id, channel, author, text, at) VALUES
             (1, 1, 'user', 'hello there', 100),
             (2, 1, 'ann', 'hi, good to see you', 101),
             (3, 1, 'user', 'tea later?', 200);
         INSERT INTO fact(author, audience_kind, audience_ref, kind, text, created_at)
             VALUES ('ann', 'participants', '1', 'fact', 'likes jasmine tea', 150);
         INSERT INTO meter(at, shape, model, uncached, cached, output, points)
             VALUES (1, 'direct', 'm', 10, 0, 5, 1.5);
         INSERT INTO settings VALUES ('user.name', '\"Rhea\"'), ('intensity', '\"high\"'),
             ('voice.ann', '\"v1\"'), ('prompt.tone', '\"a long tone text\"');
         INSERT INTO log_segment(id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at)
             VALUES (1, 1, 0, CAST('RULES TEXT' AS BLOB), CAST('PERSONA' AS BLOB),
                     CAST('tone line' AS BLOB), '{\"user\":\"Rhea\"}', 50),
                    (2, 1, 1, CAST('RULES TEXT' AS BLOB), CAST('PERSONA' AS BLOB),
                     CAST('Earlier talk: tea' AS BLOB), '{}', 120);
         UPDATE channel SET log_segment = 2 WHERE id = 1;
         INSERT INTO log_entry VALUES (2, 0, 'user', CAST('harness: clock line' AS BLOB), 130),
                                      (2, 1, 'assistant', CAST('reply bytes' AS BLOB), 131);",
    )
    .unwrap();
    if schema >= 2 {
        conn.execute_batch(
            "INSERT INTO seed(person, kind, ref, created_at) VALUES ('ann', 'words', '2', 160);
             INSERT INTO session(channel, person, cause, opened_at, last_at)
                 VALUES (1, 'ann', 'addressed', 1, 1);",
        )
        .unwrap();
    }
    if schema >= 5 {
        conn.execute_batch("INSERT INTO user_avatar VALUES (1, 'image/png', x'89504e47');")
            .unwrap();
    }
    if schema >= 6 {
        conn.execute_batch("INSERT INTO picture VALUES ('h', 'image/png', x'89504e47');")
            .unwrap();
    }
    if schema >= 7 {
        conn.execute_batch(
            "INSERT INTO daily_event(day, at, title, text) VALUES ('2026-03-01', 5, 'Rain', 'It rained.');",
        )
        .unwrap();
    }
    if schema >= 8 {
        conn.execute_batch(
            "INSERT INTO channel_summary VALUES (1, 120, 'they spoke about tea');
             INSERT INTO save_meta VALUES ('app_version', '1.1.1'), ('pack', 'demo'),
                 ('pack_version', '1.0.0'), ('stray', 'x');",
        )
        .unwrap();
    }
    drop(conn);
    if let Some(k) = password {
        world::cipher::encrypt(path, k).unwrap();
    }
}

/// A save as 1.0.0 to 1.0.3 wrote it: the request model, with its own kinds.
fn request_save(path: &Path, password: Option<&str>) {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    for sql in REQUEST_FAMILY {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", 9).unwrap();
    conn.execute_batch(
        "INSERT INTO channel(id, kind, origin) VALUES (1, 'direct', 'user');
         INSERT INTO channel_participant VALUES (1, 'ann');
         INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'ann', 120, 'enabled');
         INSERT INTO message(id, channel, author, text, at) VALUES
             (1, 1, 'user', 'hello there', 100), (2, 1, 'ann', 'hi', 101);
         INSERT INTO task(id, question, turns_left, assignee) VALUES (1, 'find the tea', 3, 'ann');
         INSERT INTO fact(id, author, audience_kind, audience_ref, kind, about_kind, about_ref, text, created_at)
             VALUES (1, 'ann', 'participants', '1', 'fact', NULL, NULL, 'likes jasmine tea', 150),
                    (2, 'ann', 'participants', '1', 'conclusion', 'task', '1', 'the tea is in the cellar', 160);
         INSERT INTO seed(person, kind, ref, created_at) VALUES
             ('ann', 'words', '2', 160), ('ann', 'conclusion', '2', 161), ('ann', 'pinned', '1', 162);
         INSERT INTO session(channel, person, cause, opened_at, last_at, closed_at) VALUES
             (1, 'ann', 'addressed', 1, 1, 2), (1, 'ann', 'ask', 3, 3, 4);
         INSERT INTO meter(at, shape, model, uncached, cached, output, points) VALUES
             (1, 'direct', 'm', 10, 0, 5, 1.5), (2, 'ask', 'm', 4, 0, 1, 0.5);
         INSERT INTO settings VALUES ('user.name', '\"Rhea\"'), ('old.thing', '1');
         INSERT INTO log_segment(id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at)
             VALUES (1, 1, 0, CAST('R' AS BLOB), CAST('P' AS BLOB), CAST('C' AS BLOB), '{}', 50);
         UPDATE channel SET log_segment = 1 WHERE id = 1;
         INSERT INTO log_entry VALUES (1, 0, 'user', CAST('line' AS BLOB), 130);",
    )
    .unwrap();
    drop(conn);
    if let Some(k) = password {
        world::cipher::encrypt(path, k).unwrap();
    }
}

fn count(w: &Connection, table: &str) -> i64 {
    w.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn texts(path: &Path, key: Option<&str>) -> Vec<String> {
    let w = World::open_with(path, key).unwrap();
    let mut stmt = w.prepare("SELECT text FROM message ORDER BY id").unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// What every converted save must be, whichever release wrote it.
fn assert_current(out: &Path, key: Option<&str>) -> World {
    world::check_file(out, key).unwrap();
    let w = World::open_with(out, key).unwrap();
    for table in ["log_segment", "log_entry"] {
        assert_eq!(count(&w, table), 0, "{table}");
    }
    assert!(
        world::channel::get(&w, world::ChannelId(1))
            .unwrap()
            .log_segment
            .is_none()
    );
    let keys: Vec<String> = w
        .prepare("SELECT key FROM settings ORDER BY key")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        keys.iter().all(|k| world::settings::is_known(k)),
        "{keys:?}"
    );
    let stamp = world::meta::read(&w).unwrap();
    assert_eq!(stamp.schema, world::SCHEMA_VERSION);
    assert_eq!(stamp.app.as_deref(), Some(world::meta::APP_VERSION));
    w
}

#[test]
fn every_ladder_schema_is_brought_current_keeping_the_chat_and_dropping_the_prompt() {
    for schema in 1..=world::SCHEMA_VERSION {
        let d = Dir::new(&format!("ladder{schema}"));
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        ladder_save(&src, schema, None);
        let original = std::fs::read(&src).unwrap();

        let report = convert(&src, &out, None, Some(&pack()), false).unwrap();
        assert_eq!(
            std::fs::read(&src).unwrap(),
            original,
            "original untouched, schema {schema}"
        );
        assert_eq!(
            d.files(),
            ["new.sqlite", "old.sqlite"],
            "no side files, schema {schema}"
        );
        assert_eq!(
            (report.source.schema, report.source.family),
            (schema, Family::Ladder)
        );
        assert_eq!(
            (report.dropped.log_segments, report.dropped.log_entries),
            (2, 2)
        );
        assert_eq!(report.dropped.settings, ["prompt.tone"]);

        let w = assert_current(&out, None);
        assert_eq!(
            texts(&out, None),
            ["hello there", "hi, good to see you", "tea later?"],
            "schema {schema}"
        );
        for table in ["fact", "bond", "meter", "channel", "channel_participant"] {
            assert_eq!(count(&w, table), 1, "{table} from {schema}");
        }
        assert_eq!(
            count(&w, "settings"),
            3,
            "name, intensity and the voice stay"
        );
        let trust = world::bond::user_bond(&w, &world::PersonId::from("ann")).unwrap();
        assert_eq!((trust.trust, trust.trust_exact), (120, 120.0));
        if schema >= 2 {
            assert_eq!((count(&w, "seed"), count(&w, "session")), (1, 1));
        }
        for (from, table) in [
            (5, "user_avatar"),
            (6, "picture"),
            (7, "daily_event"),
            (8, "channel_summary"),
        ] {
            if schema >= from {
                assert_eq!(count(&w, table), 1, "{table} from {schema}");
            }
        }
        let stamp = world::meta::read(&w).unwrap();
        assert_eq!(
            (stamp.pack.as_deref(), stamp.pack_version.as_deref()),
            (Some("demo"), Some("1.1.0"))
        );
        if schema >= 8 {
            assert_eq!(report.dropped.meta, ["stray"]);
        }
        // The summary a roll-over wrote is content and stays.
        if schema >= 8 {
            let s = world::log::last_summary(&w, world::ChannelId(1))
                .unwrap()
                .unwrap();
            assert_eq!(s.text, "they spoke about tea");
        }
        assert!(report.bytes_after > 0);
    }
}

#[test]
fn a_save_without_a_pack_records_the_app_only() {
    let d = Dir::new("nopack");
    let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
    ladder_save(&src, 7, None);
    convert(&src, &out, None, None, false).unwrap();
    let stamp = world::meta::peek(&out, None).unwrap();
    assert_eq!((stamp.app.is_some(), stamp.pack), (true, None));
}

#[test]
fn the_request_family_is_rebuilt_into_the_current_layout() {
    let d = Dir::new("request");
    let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
    request_save(&src, None);
    let original = std::fs::read(&src).unwrap();
    let report = convert(&src, &out, None, None, false).unwrap();
    assert_eq!(std::fs::read(&src).unwrap(), original);
    assert_eq!(report.source.family, Family::Request);
    assert_eq!(
        (report.converted.conclusions, report.converted.asks),
        (1, 1)
    );
    assert_eq!((report.seeds_dropped, report.sessions_dropped), (2, 1));
    assert_eq!(report.dropped.settings, ["old.thing"]);

    let w = assert_current(&out, None);
    assert_eq!(texts(&out, None), ["hello there", "hi"]);
    assert_eq!(count(&w, "fact"), 2);
    let conclusion: (String, Option<String>) = w
        .query_row("SELECT kind, about_kind FROM fact WHERE id = 2", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(conclusion, ("fact".into(), None));
    assert_eq!(
        (count(&w, "seed"), count(&w, "session"), count(&w, "meter")),
        (1, 1, 2)
    );
    let shapes: i64 = w
        .query_row("SELECT count(*) FROM meter WHERE shape = 'ask'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(shapes, 0);
    let tables = crate::rules::tables(&w).unwrap();
    assert!(
        !tables.iter().any(|t| t == "task" || t.starts_with("old_")),
        "{tables:?}"
    );
    // The rebuilt file is the current layout, so the ladder family test and this agree.
    assert_eq!(ladder::family(&w).unwrap(), Family::Ladder);
}

#[test]
fn encrypted_saves_need_their_password_and_stay_encrypted_under_it() {
    for (name, build) in [
        (
            "ladder",
            (|p: &Path, k: Option<&str>| ladder_save(p, 7, k)) as fn(&Path, Option<&str>),
        ),
        ("ladder9", |p, k| ladder_save(p, 9, k)),
        ("request", |p, k| request_save(p, k)),
    ] {
        let d = Dir::new(&format!("cipher-{name}"));
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        build(&src, Some("secret"));
        let original = std::fs::read(&src).unwrap();

        assert!(matches!(
            convert(&src, &out, Some("wrong"), None, false),
            Err(Failure::Locked)
        ));
        assert!(matches!(
            convert(&src, &out, None, None, false),
            Err(Failure::Locked)
        ));
        assert_eq!(
            d.files(),
            ["old.sqlite"],
            "a refused password leaves nothing"
        );

        convert(&src, &out, Some("secret"), Some(&pack()), false).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), original);
        assert!(world::cipher::is_encrypted(&out));
        assert!(matches!(
            world::check_file(&out, None),
            Err(world::Error::Locked)
        ));
        assert_current(&out, Some("secret"));
        assert_eq!(
            texts(&out, Some("secret")).len(),
            if name == "request" { 2 } else { 3 }
        );
    }
}

#[test]
fn a_dry_run_reports_what_a_real_run_does_and_keeps_nothing() {
    for build in [(|p: &Path| ladder_save(p, 7, None)) as fn(&Path), |p| {
        request_save(p, None)
    }] {
        let d = Dir::new("dry");
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        build(&src);
        let original = std::fs::read(&src).unwrap();
        let dry = convert(&src, &out, None, None, true).unwrap();
        assert_eq!(d.files(), ["old.sqlite"]);
        assert_eq!(std::fs::read(&src).unwrap(), original);
        let real = convert(&src, &out, None, None, false).unwrap();
        assert_eq!(
            dry.to_string()
                .replace("dry run:", "converted:")
                .replace("would", "did"),
            real.to_string()
        );
        let text = dry.to_string();
        assert!(
            text.starts_with("dry run:") && text.contains("model log segments"),
            "{text}"
        );
    }
}

#[test]
fn a_save_already_current_still_loses_its_prompt_log() {
    let d = Dir::new("current");
    let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
    ladder_save(&src, world::SCHEMA_VERSION, None);
    let report = convert(&src, &out, None, None, false).unwrap();
    assert!(report.steps.is_empty());
    assert!(report.to_string().contains("already schema"));
    assert_current(&out, None);
}

#[test]
fn a_failure_leaves_nothing_behind_and_an_existing_target_is_not_replaced() {
    let d = Dir::new("fail");
    let junk = d.path("junk.sqlite");
    std::fs::write(&junk, b"not a database at all").unwrap();
    let out = d.path("out.sqlite");
    assert!(convert(&junk, &out, None, None, false).is_err());
    assert_eq!(d.files(), ["junk.sqlite"]);

    // A database that is not a world, and an empty one.
    let other = d.path("other.sqlite");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE t(x); PRAGMA user_version = 3;")
        .unwrap();
    assert!(
        convert(&other, &out, None, None, false)
            .unwrap_err_text()
            .contains("not a world")
    );
    assert!(!out.exists());

    let src = d.path("old.sqlite");
    ladder_save(&src, 7, None);
    std::fs::write(&out, b"mine").unwrap();
    assert!(convert(&src, &out, None, None, false).is_err());
    assert_eq!(std::fs::read(&out).unwrap(), b"mine");
}

trait ErrText {
    fn unwrap_err_text(self) -> String;
}

impl<T> ErrText for Result<T, Failure> {
    fn unwrap_err_text(self) -> String {
        match self {
            Err(Failure::Other(e)) => format!("{e:#}"),
            Err(Failure::Locked) => "locked".into(),
            Ok(_) => panic!("expected a failure"),
        }
    }
}

#[test]
fn a_save_from_a_newer_schema_is_refused() {
    let d = Dir::new("newer");
    let src = d.path("future.sqlite");
    ladder_save(&src, world::SCHEMA_VERSION, None);
    Connection::open(&src)
        .unwrap()
        .pragma_update(None, "user_version", world::SCHEMA_VERSION + 1)
        .unwrap();
    let why = convert(&src, &d.path("o.sqlite"), None, None, false).unwrap_err_text();
    assert!(why.contains("newer"), "{why}");
    assert_eq!(d.files(), ["future.sqlite"]);
}

#[test]
fn a_conversion_that_fails_midway_changes_nothing() {
    // A step that cannot apply (a clashing column) fails the run and leaves no output.
    let d = Dir::new("midway");
    let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
    ladder_save(&src, 3, None);
    Connection::open(&src)
        .unwrap()
        .execute_batch("ALTER TABLE channel ADD COLUMN avatar TEXT;")
        .unwrap();
    assert!(convert(&src, &out, None, None, false).is_err());
    assert_eq!(d.files(), ["old.sqlite"]);
}

#[test]
fn in_place_keeps_the_original_as_a_backup() {
    let d = Dir::new("inplace");
    let src = d.path("default.sqlite");
    ladder_save(&src, 7, None);
    let original = std::fs::read(&src).unwrap();
    let part = in_place_part(&src);
    convert(&src, &part, None, Some(&pack()), false).unwrap();
    let placed = swap_in(&src, &part).unwrap();
    assert_eq!(placed, src);
    assert_eq!(std::fs::read(backup_of(&src)).unwrap(), original);
    assert_eq!(
        world::meta::peek(&src, None).unwrap().schema,
        world::SCHEMA_VERSION
    );
    assert!(!part.exists());
    // A second run cannot overwrite the backup.
    let again = in_place_part(&src);
    convert(&src, &again, None, None, false).unwrap();
    assert!(swap_in(&src, &again).is_err());
    assert!(again.exists());
    assert_eq!(std::fs::read(backup_of(&src)).unwrap(), original);
}

#[test]
fn the_help_carries_the_keep_convert_drop_table() {
    use clap::CommandFactory;
    let help = crate::Args::command().render_long_help().to_string();
    for word in [
        "Kept as they are",
        "Converted",
        "Dropped",
        "log_segment",
        "--dry-run",
        "--in-place",
    ] {
        assert!(help.contains(word), "{word}");
    }
}
