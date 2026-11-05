//! One plugin process and the line protocol over its pipes.
//!
//! Each message is one JSON object on one line, UTF-8, newline-terminated. The host
//! writes requests (`id`, `method`, `params`) and notifications (`method`, `params`); the
//! plugin answers a request with `id` and either `result` or `error: {message}`. Stdout
//! carries nothing else; stderr goes to a log file.
//!
//! A reader thread and a writer thread keep the caller from ever blocking on a pipe: a
//! call waits on a channel with a deadline, and a process that misses it is killed.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::manifest::Manifest;

/// Longest line read from a plugin, in bytes; a longer one fails the call.
pub const REPLY_MAX: usize = 64 * 1024;
/// Longest line written to a plugin, in bytes; the host shortens its texts to fit.
pub const REQUEST_MAX: usize = 48 * 1024;
/// How long a starting plugin has to answer `initialize`.
pub const START_TIMEOUT: Duration = Duration::from_millis(3000);

/// Environment variables a plugin inherits. Everything else, API keys included, is
/// withheld; keys live in the system keychain, which a plugin is never handed.
const ENV_KEEP: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
];

/// Why a call to a plugin did not give an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// The process could not be started.
    Spawn(String),
    /// No answer by the deadline.
    Timeout,
    /// The process ended, or its pipe closed.
    Exited,
    /// The reply was not the protocol's.
    BadReply(String),
    /// A line was longer than the limit.
    TooLong,
    /// The plugin answered with an error of its own. The process is fine.
    Plugin(String),
    /// Failed too often in a row; not called again until it is turned off and on.
    Stopped,
    /// Not enabled, or not there.
    Off,
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Spawn(e) => write!(f, "could not start: {e}"),
            Fail::Timeout => f.write_str("timed out"),
            Fail::Exited => f.write_str("the process ended"),
            Fail::BadReply(e) => write!(f, "bad reply: {e}"),
            Fail::TooLong => f.write_str("a line was over the size limit"),
            Fail::Plugin(e) => write!(f, "plugin error: {e}"),
            Fail::Stopped => f.write_str("stopped after repeated failures"),
            Fail::Off => f.write_str("not enabled"),
        }
    }
}

enum Line {
    Text(String),
    TooLong,
    Eof,
}

/// A running plugin.
pub struct Proc {
    child: Child,
    out: Sender<Vec<u8>>,
    lines: Receiver<Line>,
    next: u64,
}

impl Proc {
    /// Start the plugin and complete the handshake.
    pub fn start(
        manifest: &Manifest,
        dir: &Path,
        log: Option<&Path>,
        app_version: &str,
    ) -> Result<Self, Fail> {
        let program = manifest.program(dir).map_err(Fail::Spawn)?;
        let mut cmd = Command::new(program);
        cmd.args(&manifest.command[1..])
            .current_dir(dir)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for key in ENV_KEEP {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd.env("DRAMATIS_PLUGIN_DIR", dir)
            .env("DRAMATIS_PLUGIN_NAME", &manifest.name);
        // The log is truncated at each start, so it never grows without bound across runs.
        let stderr = log
            .and_then(|p| {
                std::fs::create_dir_all(p.parent()?).ok()?;
                File::create(p).ok()
            })
            .map_or_else(Stdio::null, Stdio::from);
        cmd.stderr(stderr);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| Fail::Spawn(e.to_string()))?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(Fail::Spawn("no pipes".into()));
        };
        let (out, out_rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            for bytes in out_rx {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let (line_tx, lines) = mpsc::channel();
        thread::spawn(move || read_lines(BufReader::new(stdout), line_tx));
        let mut proc = Self {
            child,
            out,
            lines,
            next: 0,
        };
        proc.call(
            "initialize",
            json!({
                "protocol": crate::manifest::PROTOCOL,
                "app": "dramatis",
                "version": app_version,
                "plugin": manifest.name,
            }),
            START_TIMEOUT,
        )?;
        Ok(proc)
    }

    /// Send a request and wait for its answer.
    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, Fail> {
        self.next += 1;
        let id = self.next;
        let line = json!({"id": id, "method": method, "params": params});
        self.send(&line)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Fail::Timeout);
            }
            let text = match self.lines.recv_timeout(left) {
                Ok(Line::Text(t)) => t,
                Ok(Line::TooLong) => return Err(Fail::TooLong),
                Ok(Line::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Fail::Exited);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(Fail::Timeout),
            };
            if text.trim().is_empty() {
                continue;
            }
            let reply: Value =
                serde_json::from_str(&text).map_err(|e| Fail::BadReply(e.to_string()))?;
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(Fail::BadReply("the id does not match the request".into()));
            }
            if let Some(err) = reply.get("error") {
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                return Err(Fail::Plugin(message.chars().take(300).collect()));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Send a notification: no answer is read.
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), Fail> {
        self.send(&json!({"method": method, "params": params}))
    }

    fn send(&mut self, value: &Value) -> Result<(), Fail> {
        let mut bytes = value.to_string().into_bytes();
        if bytes.len() > REQUEST_MAX {
            return Err(Fail::TooLong);
        }
        bytes.push(b'\n');
        self.out.send(bytes).map_err(|_| Fail::Exited)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read newline-terminated lines of at most [`REPLY_MAX`] bytes.
fn read_lines(mut input: impl BufRead, tx: Sender<Line>) {
    loop {
        let mut buf = Vec::new();
        let n = (&mut input)
            .take(REPLY_MAX as u64 + 1)
            .read_until(b'\n', &mut buf)
            .unwrap_or_default();
        if n == 0 {
            let _ = tx.send(Line::Eof);
            return;
        }
        let complete = buf.last() == Some(&b'\n');
        if !complete && buf.len() > REPLY_MAX {
            let _ = tx.send(Line::TooLong);
            return;
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        if tx.send(Line::Text(text)).is_err() {
            return;
        }
        if !complete {
            let _ = tx.send(Line::Eof);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        read_lines(input, tx);
        rx.iter()
            .map(|l| match l {
                Line::Text(t) => t,
                Line::TooLong => "<long>".into(),
                Line::Eof => "<eof>".into(),
            })
            .collect()
    }

    #[test]
    fn splits_lines_and_marks_the_end() {
        assert_eq!(lines(b"a\nb\n"), vec!["a\n", "b\n", "<eof>"]);
        assert_eq!(lines(b"a\nb"), vec!["a\n", "b", "<eof>"]);
        assert_eq!(lines(b""), vec!["<eof>"]);
    }

    #[test]
    fn stops_at_an_over_long_line() {
        let mut big = vec![b'x'; REPLY_MAX + 10];
        big.push(b'\n');
        assert_eq!(lines(&big), vec!["<long>"]);
        let mut exact = vec![b'x'; REPLY_MAX - 1];
        exact.push(b'\n');
        assert_eq!(lines(&exact).len(), 2);
    }
}
