//! The conversion itself, apart from the command line so a test can run it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use rusqlite::Connection;
use world::World;

/// Why a conversion did not finish. A wrong password is told apart so it can be asked again.
#[derive(Debug)]
pub enum Failure {
    Locked,
    Other(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Failure {
    fn from(e: E) -> Self {
        Self::Other(e.into())
    }
}

fn world_failure(e: world::Error) -> Failure {
    match e {
        world::Error::Locked => Failure::Locked,
        other => Failure::Other(anyhow!(other)),
    }
}

/// What the tool needs of the corpus pack: its name and release for the save's record,
/// and the wording that framed a rollover summary inside the old prompt.
#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub version: String,
    summary: String,
    recent: String,
    you: String,
    host: String,
    placeholder: String,
}

impl Pack {
    pub fn from_folio(path: &Path) -> anyhow::Result<Self> {
        let folio = folio::Folio::open(path).with_context(|| format!("{}", path.display()))?;
        let m = folio.manifest();
        let word = |key: &str| m.wording.get(key).cloned().unwrap_or_default();
        Ok(Self {
            name: m.pack.clone(),
            version: m.pack_version.trim().to_owned(),
            summary: word("harness.summary"),
            recent: word("harness.recent"),
            you: word("user.you"),
            host: word("host.name"),
            placeholder: m.user_placeholder.clone(),
        })
    }

    /// A wording template as the old build wrote it for a user called `user`.
    fn render(&self, template: &str, user: &str) -> String {
        let mut text = template.to_owned();
        if !self.placeholder.is_empty() {
            text = text.replace(&self.placeholder, user);
        }
        text.replace("{user}", user).replace("{host}", &self.host)
    }

    /// The summary the opening block `c` carried, found by the wording around it.
    fn summary_in(&self, c: &str, user: &str) -> Option<String> {
        let template = self.render(&self.summary, user);
        let (head, tail) = template.split_once("{summary}")?;
        let from = c.find(head)? + head.len();
        let rest = &c[from..];
        let upto = if !tail.is_empty() {
            rest.find(tail)?
        } else {
            // The next part of the block is the recent-messages list, set off by a blank line.
            let recent = self.render(&self.recent, user);
            (!recent.is_empty())
                .then(|| rest.find(&format!("\n\n{recent}")))
                .flatten()
                .unwrap_or(rest.len())
        };
        let text = rest[..upto].trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// What a conversion kept and what it dropped.
#[derive(Debug, Default)]
pub struct Report {
    pub schema_from: i64,
    pub kept: BTreeMap<String, i64>,
    pub segments: usize,
    pub entries: usize,
    pub summaries_kept: usize,
    pub summaries_lost: usize,
    pub pack_given: bool,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "schema {} -> {}; version record written",
            self.schema_from,
            world::SCHEMA_VERSION
        )?;
        for (table, n) in &self.kept {
            writeln!(f, "kept {n:>6} {table}")?;
        }
        writeln!(
            f,
            "dropped the prompt log: {} segments, {} entries",
            self.segments, self.entries
        )?;
        if self.summaries_kept > 0 {
            writeln!(
                f,
                "conversation summaries carried over: {}",
                self.summaries_kept
            )?;
        }
        if self.summaries_lost > 0 {
            if self.pack_given {
                writeln!(
                    f,
                    "{} later segments had no summary to carry over",
                    self.summaries_lost
                )?;
            } else {
                writeln!(
                    f,
                    "{} later segments may have carried a summary; give --folio to carry them over",
                    self.summaries_lost
                )?;
            }
        }
        write!(
            f,
            "size {} -> {} bytes",
            self.bytes_before, self.bytes_after
        )
    }
}

/// Tables that hold prompt material or the new record, not the player's content.
const NOT_CONTENT: &[&str] = &["log_segment", "log_entry", "save_meta", "channel_summary"];

fn side(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn remove_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(side(path, suffix));
    }
}

/// Where the converted file is built before it takes its name.
fn part_of(dest: &Path) -> PathBuf {
    side(dest, &format!(".part-{}", std::process::id()))
}

/// The file an in-place conversion writes first, beside the original.
pub fn in_place_part(save: &Path) -> PathBuf {
    let stem = save.file_stem().and_then(|s| s.to_str()).unwrap_or("save");
    save.with_file_name(format!("{stem}.converted.sqlite"))
}

pub fn backup_of(save: &Path) -> PathBuf {
    side(save, ".bak")
}

/// Put the converted file in the original's place; the original (with the side files of
/// its journal) is kept as `<file>.bak`. Returns the path of the converted save.
pub fn swap_in(save: &Path, converted: &Path) -> anyhow::Result<PathBuf> {
    let bak = backup_of(save);
    if bak.exists() {
        bail!(
            "{} already exists; the converted save stays at {}",
            bak.display(),
            converted.display()
        );
    }
    std::fs::rename(save, &bak)?;
    // A journal left beside the original belongs to the original.
    for suffix in ["-wal", "-shm"] {
        if side(save, suffix).exists() {
            std::fs::rename(side(save, suffix), side(&bak, suffix))?;
        }
    }
    if let Err(e) = std::fs::rename(converted, save) {
        let _ = std::fs::rename(&bak, save);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::rename(side(&bak, suffix), side(save, suffix));
        }
        return Err(e).context("putting the converted save in place");
    }
    Ok(save.to_path_buf())
}

/// Copy the database file and the side files that are part of its state.
fn copy_raw(src: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(src, to).with_context(|| format!("copying {}", src.display()))?;
    for suffix in ["-wal", "-shm"] {
        if side(src, suffix).is_file() {
            std::fs::copy(side(src, suffix), side(to, suffix))?;
        }
    }
    Ok(())
}

fn counts(conn: &Connection) -> rusqlite::Result<BTreeMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut out = BTreeMap::new();
    for name in names
        .into_iter()
        .filter(|n| !NOT_CONTENT.contains(&n.as_str()))
    {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
            r.get(0)
        })?;
        out.insert(name, n);
    }
    Ok(out)
}

/// Move the summaries out of the old opening blocks into `channel_summary`. Returns
/// `(kept, lost)`; a block that carried one the wording cannot find counts as lost.
fn carry_summaries(w: &Connection, pack: Option<&Pack>) -> anyhow::Result<(usize, usize)> {
    let fallback = world::settings::user_name(w)?;
    let mut stmt = w.prepare(
        "SELECT channel, seq, prefix_c, shape, opened_at FROM log_segment ORDER BY channel, seq",
    )?;
    let rows: Vec<(i64, i64, Vec<u8>, String, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    drop(stmt);
    let (mut kept, mut lost) = (0, 0);
    let mut last: BTreeMap<i64, String> = BTreeMap::new();
    for (channel, seq, c, shape, at) in rows {
        // A first segment opens without a summary; a later one may carry one.
        if seq == 0 {
            continue;
        }
        let Some(pack) = pack else {
            lost += 1;
            continue;
        };
        let user = serde_json::from_str::<serde_json::Value>(&shape)
            .ok()
            .and_then(|v| v["user"].as_str().map(str::to_owned))
            .or_else(|| fallback.clone())
            .unwrap_or_else(|| pack.you.clone());
        let text = String::from_utf8(c)
            .ok()
            .and_then(|c| pack.summary_in(&c, &user));
        match text {
            Some(text) if last.get(&channel) != Some(&text) => {
                w.execute(
                    "INSERT OR REPLACE INTO channel_summary(channel, at, text) VALUES (?1, ?2, ?3)",
                    (channel, at, &text),
                )?;
                last.insert(channel, text);
                kept += 1;
            }
            Some(_) => {}
            // Rolled segments carry a summary only when the wrap-up produced one.
            None => lost += 1,
        }
    }
    Ok((kept, lost))
}

fn run(src: &Path, part: &Path, key: Option<&str>, pack: Option<&Pack>) -> Result<Report, Failure> {
    copy_raw(src, part)?;
    world::check_file(part, key).map_err(world_failure)?;
    let stamp = world::meta::peek(part, key).map_err(world_failure)?;
    let w = World::open_with(part, key).map_err(world_failure)?;
    let mut report = Report {
        schema_from: stamp.schema,
        pack_given: pack.is_some(),
        bytes_before: std::fs::metadata(src)?.len(),
        ..Report::default()
    };
    let before = counts(&w)?;
    report.segments = w.query_row("SELECT count(*) FROM log_segment", [], |r| {
        r.get::<_, i64>(0)
    })? as usize;
    report.entries =
        w.query_row("SELECT count(*) FROM log_entry", [], |r| r.get::<_, i64>(0))? as usize;

    world::atomic(&w, |w| {
        let (kept, lost) = carry_summaries(w, pack)
            .map_err(|e| world::Error::Invalid(format!("summaries: {e:#}")))?;
        report.summaries_kept = kept;
        report.summaries_lost = lost;
        w.execute_batch(
            "UPDATE channel SET log_segment = NULL;
             DELETE FROM log_entry;
             DELETE FROM log_segment;",
        )?;
        world::meta::write(
            w,
            world::meta::APP_VERSION,
            pack.map(|p| (p.name.as_str(), p.version.as_str())),
        )
    })
    .map_err(world_failure)?;

    let after = counts(&w)?;
    if before != after {
        return Err(
            anyhow!("a table changed size during the conversion: {before:?} -> {after:?}").into(),
        );
    }
    report.kept = after;
    w.execute_batch("VACUUM")?;
    let quick: String = w.pragma_query_value(None, "quick_check", |r| r.get(0))?;
    if quick != "ok" {
        return Err(anyhow!("the converted save failed its integrity check: {quick}").into());
    }
    drop(w);
    // Opened again as the app will, and found as it should be.
    world::check_file(part, key).map_err(world_failure)?;
    report.bytes_after = std::fs::metadata(part)?.len();
    Ok(report)
}

/// Convert the save at `src` into a new file `dest`, under the same password. `src` is only
/// read (from a copy); nothing is written at `dest` unless the conversion succeeded.
pub fn convert(
    src: &Path,
    dest: &Path,
    key: Option<&str>,
    pack: Option<&Pack>,
) -> Result<Report, Failure> {
    if dest.exists() {
        return Err(anyhow!("{} already exists", dest.display()).into());
    }
    let part = part_of(dest);
    remove_files(&part);
    let done = run(src, &part, key, pack);
    let done = done.and_then(|report| {
        std::fs::rename(&part, dest)?;
        Ok(report)
    });
    remove_files(&part);
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema ladder as 1.1.0 shipped it.
    const OLD: [&str; 7] = [
        include_str!("../../../crates/world/migrations/0001.sql"),
        include_str!("../../../crates/world/migrations/0002.sql"),
        include_str!("../../../crates/world/migrations/0003.sql"),
        include_str!("../../../crates/world/migrations/0004.sql"),
        include_str!("../../../crates/world/migrations/0005.sql"),
        include_str!("../../../crates/world/migrations/0006.sql"),
        include_str!("../../../crates/world/migrations/0007.sql"),
    ];

    fn pack() -> Pack {
        Pack {
            name: "demo".into(),
            version: "1.1.0".into(),
            summary: "Earlier talk: {summary}".into(),
            recent: "Recent lines:".into(),
            you: "Doctor".into(),
            host: "Host".into(),
            placeholder: "<name>".into(),
        }
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let d =
                std::env::temp_dir().join(format!("dramatis-migrate-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A save as the 1.1.0 code wrote it: schema 7, prompt prefixes and harness lines in
    /// the log, a rollover summary inside the second segment's opening block, and rows in
    /// every table the player's content lives in.
    fn legacy(path: &Path, password: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in OLD {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        conn.execute_batch(
            "INSERT INTO channel(id, kind, origin, direct_with) VALUES (1, 'direct', 'user', 'ann');
             INSERT INTO channel_participant VALUES (1, 'ann');
             INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'ann', 120, 'enabled');
             INSERT INTO message(id, channel, author, text, at) VALUES
                 (1, 1, 'user', 'hello there', 100),
                 (2, 1, 'ann', 'hi, good to see you', 101),
                 (3, 1, 'user', 'tea later?', 200);
             INSERT INTO fact(author, audience_kind, audience_ref, kind, text, created_at)
                 VALUES ('ann', 'participants', '1', 'fact', 'likes jasmine tea', 150);
             INSERT INTO daily_event(day, at, title, text) VALUES ('2026-03-01', 5, 'Rain', 'It rained on the deck.');
             INSERT INTO seed(person, kind, ref, created_at) VALUES ('ann', 'words', '2', 160);
             INSERT INTO meter(at, shape, model, uncached, cached, output, points)
                 VALUES (1, 'direct', 'm', 10, 0, 5, 1.5);
             INSERT INTO picture(hash, mime, data) VALUES ('h', 'image/png', x'89504e47');
             INSERT INTO settings VALUES ('user.name', '\"Rhea\"'), ('intensity', '\"high\"'),
                 ('presence.announced', '[\"k\"]');",
        )
        .unwrap();
        let seg = |id: i64, seq: i64, c: &str, at: i64| {
            conn.execute(
                "INSERT INTO log_segment(id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at)
                 VALUES (?1, 1, ?2, CAST('RULES TEXT' AS BLOB), CAST('PERSONA TEXT' AS BLOB), ?3, '{\"user\":\"Rhea\"}', ?4)",
                (id, seq, c.as_bytes(), at),
            )
            .unwrap();
        };
        seg(1, 0, "tone line", 50);
        seg(
            2,
            1,
            "tone line\n\nEarlier talk: they spoke about tea\nand rain\n\nRecent lines:\nRhea: hello there",
            120,
        );
        conn.execute_batch(
            "UPDATE channel SET log_segment = 2 WHERE id = 1;
             INSERT INTO log_entry VALUES (2, 0, 'user', CAST('harness: clock line' AS BLOB), 130),
                                          (2, 1, 'assistant', CAST('reply bytes' AS BLOB), 131);",
        )
        .unwrap();
        drop(conn);
        if let Some(k) = password {
            world::cipher::encrypt(path, k).unwrap();
        }
    }

    fn texts(path: &Path, key: Option<&str>) -> Vec<String> {
        let w = World::open_with(path, key).unwrap();
        let mut stmt = w.prepare("SELECT text FROM message ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn a_1_1_0_save_is_upgraded_keeping_everything_but_the_prompt_material() {
        let d = Dir::new("plain");
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        legacy(&src, None);
        let original = std::fs::read(&src).unwrap();

        let report = convert(&src, &out, None, Some(&pack())).unwrap();
        assert_eq!(
            std::fs::read(&src).unwrap(),
            original,
            "the original is not touched"
        );
        assert!(!side(&part_of(&out), "").exists());
        assert_eq!(
            (report.schema_from, report.segments, report.entries),
            (7, 2, 2)
        );
        assert_eq!((report.summaries_kept, report.summaries_lost), (1, 0));
        assert_eq!(report.kept["message"], 3);

        world::check_file(&out, None).unwrap();
        let w = World::open(&out).unwrap();
        // Content is all there.
        assert_eq!(
            texts(&out, None),
            ["hello there", "hi, good to see you", "tea later?"]
        );
        for (table, n) in [
            ("fact", 1),
            ("daily_event", 1),
            ("seed", 1),
            ("meter", 1),
            ("picture", 1),
            ("bond", 1),
            ("settings", 3),
        ] {
            let have: i64 = w
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(have, n, "{table}");
        }
        // The prompt material is not.
        for table in ["log_segment", "log_entry"] {
            let have: i64 = w
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(have, 0, "{table}");
        }
        assert!(
            world::channel::get(&w, world::ChannelId(1))
                .unwrap()
                .log_segment
                .is_none()
        );
        // The summary lives on, outside the log.
        let summary = world::log::last_summary(&w, world::ChannelId(1))
            .unwrap()
            .unwrap();
        assert_eq!(summary.text, "they spoke about tea\nand rain");
        // And the save says what it is.
        let stamp = world::meta::read(&w).unwrap();
        assert_eq!(stamp.schema, world::SCHEMA_VERSION);
        assert_eq!(stamp.app.as_deref(), Some(world::meta::APP_VERSION));
        assert_eq!(
            (stamp.pack.as_deref(), stamp.pack_version.as_deref()),
            (Some("demo"), Some("1.1.0"))
        );
        assert!(report.bytes_after > 0);
    }

    #[test]
    fn without_the_pack_the_summaries_are_counted_as_not_carried() {
        let d = Dir::new("nopack");
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        legacy(&src, None);
        let report = convert(&src, &out, None, None).unwrap();
        assert_eq!((report.summaries_kept, report.summaries_lost), (0, 1));
        assert!(
            world::log::last_summary(&World::open(&out).unwrap(), world::ChannelId(1))
                .unwrap()
                .is_none()
        );
        let stamp = world::meta::peek(&out, None).unwrap();
        assert_eq!((stamp.app.is_some(), stamp.pack), (true, None));
        assert_eq!(texts(&out, None).len(), 3);
    }

    #[test]
    fn an_encrypted_save_needs_its_password_and_stays_encrypted_under_it() {
        let d = Dir::new("cipher");
        let (src, out) = (d.path("old.sqlite"), d.path("new.sqlite"));
        legacy(&src, Some("secret"));
        let original = std::fs::read(&src).unwrap();

        assert!(matches!(
            convert(&src, &out, Some("wrong"), None),
            Err(Failure::Locked)
        ));
        assert!(matches!(
            convert(&src, &out, None, None),
            Err(Failure::Locked)
        ));
        assert!(!out.exists() && !side(&part_of(&out), "").exists());

        convert(&src, &out, Some("secret"), Some(&pack())).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), original);
        assert!(world::cipher::is_encrypted(&out));
        assert!(matches!(
            world::check_file(&out, None),
            Err(world::Error::Locked)
        ));
        assert_eq!(texts(&out, Some("secret")).len(), 3);
        let w = World::open_with(&out, Some("secret")).unwrap();
        assert!(
            world::log::last_summary(&w, world::ChannelId(1))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_failure_leaves_nothing_behind_and_an_existing_target_is_not_replaced() {
        let d = Dir::new("fail");
        let junk = d.path("junk.sqlite");
        std::fs::write(&junk, b"not a database at all").unwrap();
        let out = d.path("out.sqlite");
        assert!(convert(&junk, &out, None, None).is_err());
        assert!(!out.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&d.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["junk.sqlite"]);

        let src = d.path("old.sqlite");
        legacy(&src, None);
        std::fs::write(&out, b"mine").unwrap();
        assert!(convert(&src, &out, None, None).is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"mine");
    }

    #[test]
    fn a_save_from_a_newer_schema_is_refused() {
        let d = Dir::new("newer");
        let src = d.path("future.sqlite");
        legacy(&src, None);
        Connection::open(&src)
            .unwrap()
            .pragma_update(None, "user_version", world::SCHEMA_VERSION + 1)
            .unwrap();
        assert!(convert(&src, &d.path("o.sqlite"), None, None).is_err());
    }

    #[test]
    fn in_place_keeps_the_original_as_a_backup() {
        let d = Dir::new("inplace");
        let src = d.path("default.sqlite");
        legacy(&src, None);
        let original = std::fs::read(&src).unwrap();
        let part = in_place_part(&src);
        convert(&src, &part, None, Some(&pack())).unwrap();
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
        convert(&src, &again, None, None).unwrap();
        assert!(swap_in(&src, &again).is_err());
        assert!(again.exists());
        assert_eq!(std::fs::read(backup_of(&src)).unwrap(), original);
    }

    #[test]
    fn the_summary_is_found_by_its_wording_with_or_without_a_closing_phrase() {
        let mut p = pack();
        assert_eq!(
            p.summary_in("a\n\nEarlier talk: x y\n\nRecent lines:\nq", "Rhea")
                .as_deref(),
            Some("x y")
        );
        p.summary = "Before, {summary} (end)".into();
        assert_eq!(
            p.summary_in("Before, x y (end)\n\nRecent lines:", "Rhea")
                .as_deref(),
            Some("x y")
        );
        p.summary = "Hello <name>: {summary}".into();
        assert_eq!(
            p.summary_in("Hello Rhea: ok", "Rhea").as_deref(),
            Some("ok")
        );
        assert_eq!(p.summary_in("no summary here", "Rhea"), None);
    }
}
