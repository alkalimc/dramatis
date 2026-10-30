//! A small diagnostics log for turns that ended without a reply. An app started from the
//! desktop has no terminal, so this is the only place the endpoint's own words survive.
//! Never written: keys, headers, the prompt.

use std::io::Write;
use std::path::Path;

/// Size at which the file moves to `.1` (replacing the older one) and a new one starts.
const LOG_MAX: u64 = 1_000_000;
/// How much of an endpoint's text one line keeps.
const TEXT_MAX: usize = 300;

/// One line: UTC time, then the fields, then the text cut short on one line. A log that
/// cannot be written is not an error worth failing a turn for.
pub fn record(dir: &Path, ms: i64, fields: &[(&str, &str)], text: &str) {
    if dir.as_os_str().is_empty() {
        return;
    }
    let date = world::clock::Now::utc(ms).local_date();
    let secs = (ms / 1000).rem_euclid(86_400);
    let mut line = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        date.year,
        date.month,
        date.day,
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    );
    for (k, v) in fields {
        line.push_str(&format!(" {k}={v}"));
    }
    let text: String = text
        .chars()
        .take(TEXT_MAX)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    line.push_str(&format!(" text={text:?}\n"));
    let _ = write(dir, &line);
}

fn write(dir: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let file = dir.join("dramatis.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() >= LOG_MAX) {
        std::fs::rename(&file, dir.join("dramatis.log.1"))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_keeps_one_previous_file() {
        let dir = std::env::temp_dir().join(format!("dramatis-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dramatis.log"), vec![b'x'; LOG_MAX as usize]).unwrap();
        record(&dir, 0, &[("channel", "1")], &"a\nb".repeat(400));
        let now = std::fs::read_to_string(dir.join("dramatis.log")).unwrap();
        assert!(now.starts_with("1970-01-01T00:00:00Z channel=1 text=\"a b"));
        assert_eq!(now.lines().count(), 1);
        assert!(now.len() < 400);
        assert_eq!(
            std::fs::metadata(dir.join("dramatis.log.1")).unwrap().len(),
            LOG_MAX
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
