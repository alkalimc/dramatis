//! Everything the engine pushes to the UI unasked. The UI's external store is fed only by
//! these and by command results.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::types::{Author, ChannelId, FactId, MessageId, Mode, PersonId, Target, TaskId};
use crate::views::{Message, QuotaStatus, ToolAction};

macro_rules! events {
    ($($(#[$doc:meta])* $name:ident $body:tt)*) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
            #[cfg_attr(feature = "tauri", derive(tauri_specta::Event))]
            pub struct $name $body
        )*

        /// Event names as the UI listens for them.
        pub const EVENTS: &[&str] = &[$(stringify!($name)),*];

        #[cfg(feature = "tauri")]
        pub(crate) fn collect() -> tauri_specta::Events {
            tauri_specta::collect_events![$($name),*]
        }
    };
}

events! {
    /// A chunk of a reply being generated. `message` is stable across the stream.
    MessageDelta {
        pub channel: ChannelId,
        pub message: MessageId,
        pub author: Author,
        pub delta: String,
    }

    /// A message is complete and stored: a finished stream, a user message echoed back,
    /// an unprompted opening, an interjection.
    MessageAdded {
        pub message: Message,
    }

    /// A tool call, shown as a visible action line under the message it belongs to.
    ToolCalled {
        pub channel: ChannelId,
        pub message: MessageId,
        pub author: Author,
        pub action: ToolAction,
    }

    /// Narrative state changed; the UI refetches what it shows of it.
    WorldChanged {
        pub changes: Vec<WorldChange>,
    }

    /// A person or group changed mode, by the user or by the host on the user's behalf.
    ModeChanged {
        pub target: Target,
        pub mode: Mode,
    }

    /// Roster grouping changed (trust or contact moved people between groups).
    RosterChanged {
        pub persons: Vec<PersonId>,
    }

    /// Quota use, band or release time changed.
    QuotaChanged {
        pub status: QuotaStatus,
    }

    /// Shown as a system notification when the window is closed. The text is the
    /// person's own line.
    Notification {
        pub person: PersonId,
        pub channel: ChannelId,
        pub message: MessageId,
        pub text: String,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorldChange {
    Fact { id: FactId },
    Task { id: TaskId },
    Channel { id: ChannelId },
    Trust { person: PersonId },
    Digest,
}
