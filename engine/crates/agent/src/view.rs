//! Conversions from engine values to the types that cross the seam.

use api::types::{self as t, Author, Timestamp};
use api::views::{self as v, CallShape, QuotaBand, QuotaWindow, ToolAction, WindowSpan};
use folio::Folio;
use rusqlite::Connection;
use world::quota::{self, Band, Span};
use world::{Actor, ChannelId, FactId, MessageId, PersonId, Shape, TaskId, Tier};

use crate::error::{Result, timestamp};
use crate::store;

pub fn channel_id(c: ChannelId) -> t::ChannelId {
    t::ChannelId(c.0.to_string())
}

pub fn message_id(m: MessageId) -> t::MessageId {
    t::MessageId(m.0.to_string())
}

pub fn task_id(t_: TaskId) -> t::TaskId {
    t::TaskId(t_.0.to_string())
}

pub fn fact_id(f: FactId) -> t::FactId {
    t::FactId(f.0.to_string())
}

pub fn person_id(p: &PersonId) -> t::PersonId {
    t::PersonId(p.0.clone())
}

pub fn author(a: &Actor) -> Author {
    match a {
        Actor::User => Author::User,
        Actor::Host => Author::Host,
        Actor::Person(p) => Author::Person { id: person_id(p) },
    }
}

/// A person with the corpus's display name (the id when the roster lacks him).
pub fn person_ref(folio: &Folio, p: &str) -> Result<t::PersonRef> {
    let name = folio
        .person(p)?
        .map(|x| x.display)
        .unwrap_or_else(|| p.to_owned());
    Ok(t::PersonRef {
        id: t::PersonId(p.to_owned()),
        name,
    })
}

pub fn citation(c: &world::Citation) -> t::Citation {
    t::Citation {
        page: c.page.clone(),
        revid: c.revid,
        span_from: c.span_from,
        span_to: c.span_to,
        chunk_id: c.chunk_id.clone(),
    }
}

/// A unit's portable pointer.
pub fn unit_citation(u: &folio::Unit) -> world::Citation {
    world::Citation {
        page: u.page.clone(),
        revid: u.revid.unwrap_or(0).max(0) as u32,
        span_from: u.span_from.unwrap_or(0).max(0) as u32,
        span_to: u.span_to.unwrap_or(0).max(0) as u32,
        chunk_id: u.id.clone(),
    }
}

pub fn confidence(l: index::Level) -> t::Confidence {
    match l {
        index::Level::High => t::Confidence::High,
        index::Level::Medium => t::Confidence::Medium,
        index::Level::Low => t::Confidence::Low,
    }
}

/// A stored message as the UI renders it. `office` resolves a request's note.
pub fn message(m: &world::message::Message, offset_min: i32, office: &std::path::Path) -> v::Message {
    let meta = store::meta_of(m.attachment.as_ref());
    let actions: Vec<ToolAction> = m
        .tool_calls
        .as_ref()
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default();
    v::Message {
        id: message_id(m.id),
        channel: channel_id(m.channel),
        author: author(&m.author),
        text: m.text.clone(),
        actions,
        attachment: None,
        cites: meta.cites.iter().map(citation).collect(),
        confidence: meta.confidence.map(confidence),
        relayed: meta.relayed,
        task: meta.task.map(task_id),
        note: meta.note.map(|n| t::MediaFile {
            path: office.join(n).to_string_lossy().into_owned(),
        }),
        at: timestamp(m.at, offset_min),
    }
}

/// Spending is shown in the shapes the UI knows; an interjection is group talk.
pub fn call_shape(s: Shape) -> CallShape {
    match s {
        Shape::Direct => CallShape::Direct,
        Shape::Group | Shape::Interject => CallShape::Group,
        Shape::Opening => CallShape::Opening,
        Shape::Ask => CallShape::Ask,
        Shape::Host => CallShape::Host,
        Shape::Wrapup => CallShape::Wrapup,
    }
}

pub fn tier(tier_: Tier) -> t::Tier {
    match tier_ {
        Tier::Low => t::Tier::Low,
        Tier::Middle => t::Tier::Middle,
        Tier::High => t::Tier::High,
        Tier::ExtraHigh => t::Tier::ExtraHigh,
        Tier::Max => t::Tier::Max,
        Tier::Ultra => t::Tier::Ultra,
    }
}

fn span(s: Span) -> WindowSpan {
    match s {
        Span::FiveHours => WindowSpan::FiveHours,
        Span::SevenDays => WindowSpan::SevenDays,
    }
}

/// The quota panel: bands, windows and what was spent per shape in the tighter window.
pub fn quota_status(
    conn: &Connection,
    status: &quota::Status,
    now: world::clock::Now,
) -> Result<v::QuotaStatus> {
    let ts = |ms: i64| timestamp(ms, now.offset_min);
    let tight = status
        .binding
        .or_else(|| {
            status
                .windows
                .iter()
                .max_by(|a, b| a.used.total_cmp(&b.used))
                .map(|w| w.span)
        })
        .unwrap_or(Span::FiveHours);
    let mut spent: Vec<v::ShapeSpend> = Vec::new();
    for (shape, points) in quota::spent_by_shape(conn, now.ms, tight)? {
        let shape = call_shape(shape);
        match spent.iter_mut().find(|s| s.shape == shape) {
            Some(s) => s.points += points,
            None => spent.push(v::ShapeSpend { shape, points }),
        }
    }
    Ok(v::QuotaStatus {
        tier: tier(status.tier),
        band: match status.band {
            Band::Open => QuotaBand::Open,
            Band::Quiet => QuotaBand::Quiet,
            Band::Exhausted => QuotaBand::Exhausted,
        },
        windows: status
            .windows
            .iter()
            .map(|w| QuotaWindow {
                span: span(w.span),
                used: w.used,
                release_at: w.release_at.map(ts),
            })
            .collect(),
        binding: status.binding.map(span),
        spent,
    })
}

pub fn target_person(p: &PersonId) -> t::Target {
    t::Target::Person { id: person_id(p) }
}

pub fn target_channel(c: ChannelId) -> t::Target {
    t::Target::Channel { id: channel_id(c) }
}

pub fn mode(m: world::Mode) -> t::Mode {
    match m {
        world::Mode::Enabled => t::Mode::Enabled,
        world::Mode::Frozen => t::Mode::Frozen,
        world::Mode::Disabled => t::Mode::Disabled,
    }
}

pub fn world_mode(m: t::Mode) -> world::Mode {
    match m {
        t::Mode::Enabled => world::Mode::Enabled,
        t::Mode::Frozen => world::Mode::Frozen,
        t::Mode::Disabled => world::Mode::Disabled,
    }
}

pub fn timestamp_of(ms: i64, offset_min: i32) -> Timestamp {
    timestamp(ms, offset_min)
}
