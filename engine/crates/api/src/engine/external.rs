//! Noticing what another process wrote to the world database (the terminal client next to
//! the app). SQLite's `data_version` changes on this connection only when a different
//! connection committed, so one pragma every few seconds says whether to look further.

use std::time::Duration;

use world::message;

use super::Engine;
use crate::convert::{self, world_err};
use crate::error::ApiResult;
use crate::events::{MessageAdded, WorldChange};
use crate::port::Event;

/// How often [`Engine::watch_external`] looks.
pub const POLL: Duration = Duration::from_secs(2);

/// The most messages announced from one look; the rest follow on the next.
const BATCH: u32 = 200;

/// Where the last look stopped.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// The save the numbers below belong to; a switch starts over.
    save: String,
    version: Option<i64>,
    /// A change was seen but not yet announced (the agent was busy).
    dirty: bool,
    message: i64,
    fact: i64,
    task: i64,
}

fn max_id(conn: &rusqlite::Connection, table: &str) -> ApiResult<i64> {
    conn.query_row(
        &format!("SELECT coalesce(max(id), 0) FROM {table}"),
        [],
        |r| r.get(0),
    )
    .map_err(|e| world_err(e.into()))
}

impl Engine {
    /// Look once for writes by another process and announce them: `MessageAdded` for each
    /// new message nobody has announced yet, and `WorldChanged` for the channels, facts and
    /// requests that are new. Writes by this process's own agent are already announced
    /// (they are in `seen`), and while the agent is working nothing is announced at all, so
    /// a half-streamed reply is never taken for someone else's message; the look repeats
    /// once it is idle. The first look only records where things stand. Returns how many
    /// messages were announced.
    pub fn poll_external(&self) -> ApiResult<usize> {
        // The name is held until the connection is read: a switch swaps both under this
        // lock, so the numbers below always belong to the save they are filed under.
        let open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        let save = open.clone();
        let mut watch = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        let (version, now) = {
            let w = self.w();
            let version: i64 = w
                .pragma_query_value(None, "data_version", |r| r.get(0))
                .map_err(|e| world_err(e.into()))?;
            let now = (
                max_id(&w, "message")?,
                max_id(&w, "fact")?,
                max_id(&w, "task")?,
            );
            (version, now)
        };
        drop(open);
        // This process's own rewind: ids from the cut on are free again.
        let rewound = self.ports.seen.take_rewound();
        if watch.save != save || watch.version.is_none() {
            *watch = Watch {
                save,
                version: Some(version),
                dirty: false,
                message: now.0,
                fact: now.1,
                task: now.2,
            };
            return Ok(0);
        }
        if let Some(floor) = rewound {
            watch.message = watch.message.min(floor);
            watch.dirty = true;
        }
        if watch.version != Some(version) {
            watch.version = Some(version);
            watch.dirty = true;
        }
        if !watch.dirty || !self.ports.agent.idle() {
            return Ok(0);
        }
        // A rewind frees ids that are then handed out again.
        if now.0 < watch.message {
            self.ports.seen.forget_above(now.0);
        }
        watch.message = watch.message.min(now.0);
        watch.fact = watch.fact.min(now.1);
        watch.task = watch.task.min(now.2);

        let offset = self.now().offset_min;
        let rows = message::all_since(&self.w(), watch.message, BATCH).map_err(world_err)?;
        let mut announced = 0;
        let mut changes = Vec::new();
        for m in &rows {
            watch.message = m.id.0;
            let view = self.message_view(m, offset);
            if self.ports.seen.has(&view.id.0) {
                continue;
            }
            let channel = view.channel.clone();
            self.emit(Event::MessageAdded(MessageAdded { message: view }));
            announced += 1;
            if !changes.contains(&WorldChange::Channel {
                id: channel.clone(),
            }) {
                changes.push(WorldChange::Channel { id: channel });
            }
        }
        if announced > 0 {
            changes.push(WorldChange::Digest);
        }
        if rows.len() as u32 >= BATCH {
            // More to read; the next look continues from the cursor.
            self.changed(changes);
            return Ok(announced);
        }
        // Only what is new since the last look: ids above the cursors.
        for (table, cursor, newest) in [("fact", watch.fact, now.1), ("task", watch.task, now.2)] {
            if newest > cursor {
                let w = self.w();
                let mut stmt = w
                    .prepare(&format!("SELECT id FROM {table} WHERE id > ?1 ORDER BY id"))
                    .map_err(|e| world_err(e.into()))?;
                let ids: Vec<i64> = stmt
                    .query_map([cursor], |r| r.get(0))
                    .and_then(|rows| rows.collect())
                    .map_err(|e| world_err(e.into()))?;
                for id in ids {
                    changes.push(if table == "fact" {
                        WorldChange::Fact {
                            id: convert::fact_id(world::FactId(id)),
                        }
                    } else {
                        WorldChange::Task {
                            id: convert::task_id(world::TaskId(id)),
                        }
                    });
                }
            }
        }
        watch.fact = now.1;
        watch.task = now.2;
        watch.dirty = false;
        self.changed(changes);
        Ok(announced)
    }

    /// Poll for other processes' writes every [`POLL`] until the engine is dropped (a
    /// replaced corpus or save-less reopen ends the old engine's watcher). The app spawns
    /// this once per engine.
    pub async fn watch_external(engine: std::sync::Weak<Self>) {
        Self::watch_every(engine, POLL).await;
    }

    pub(super) async fn watch_every(engine: std::sync::Weak<Self>, period: Duration) {
        let mut failing = false;
        loop {
            tokio::time::sleep(period).await;
            let Some(engine) = engine.upgrade() else {
                return;
            };
            // The pragma and the queries are blocking calls on a mutex-guarded connection.
            let looked = tokio::task::spawn_blocking(move || engine.poll_external()).await;
            // Say so once when a look fails and again when it works, not every 2 s.
            let failure = match looked {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if failure.is_some() != failing {
                match &failure {
                    Some(e) => {
                        eprintln!("dramatis: checking for changes by another process failed: {e}")
                    }
                    None => {
                        eprintln!("dramatis: checking for changes by another process works again")
                    }
                }
                failing = failure.is_some();
            }
        }
    }
}
