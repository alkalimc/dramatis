//! Engine values to seam types and back. Ids cross as strings; a string that does not
//! parse names nothing, so it is [`ApiError::NotFound`].

use world::quota::{self, Band};

use crate::error::{ApiError, ApiResult};
use crate::types::{self as t, Author, Timestamp};
use crate::views::{self as v, CallShape, QuotaBand, QuotaWindow};

/// RFC 3339 at the user's offset, whole seconds: `2026-01-02T03:04:05+08:00`.
pub fn ts(ms: i64, offset_min: i32) -> Timestamp {
    let offset = chrono::FixedOffset::east_opt(offset_min * 60)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("zero offset"));
    let at = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
    Timestamp(
        at.with_timezone(&offset)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    )
}

macro_rules! ids {
    ($($to:ident / $from:ident: $world:ident),* $(,)?) => {$(
        pub fn $to(id: world::$world) -> t::$world {
            t::$world(id.0.to_string())
        }

        pub fn $from(id: &t::$world) -> ApiResult<world::$world> {
            id.0.parse().map(world::$world).map_err(|_| ApiError::NotFound)
        }
    )*};
}

ids! {
    channel_id / world_channel: ChannelId,
    message_id / world_message: MessageId,
    fact_id / world_fact: FactId,
}

pub fn person_id(p: &world::PersonId) -> t::PersonId {
    t::PersonId(p.0.clone())
}

pub fn world_person(p: &t::PersonId) -> world::PersonId {
    world::PersonId(p.0.clone())
}

pub fn author(a: &world::Actor) -> Author {
    match a {
        world::Actor::User => Author::User,
        world::Actor::Host => Author::Host,
        world::Actor::Person(p) => Author::Person { id: person_id(p) },
    }
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

pub fn tier(x: world::Tier) -> t::Tier {
    match x {
        world::Tier::Low => t::Tier::Low,
        world::Tier::Middle => t::Tier::Middle,
        world::Tier::High => t::Tier::High,
        world::Tier::ExtraHigh => t::Tier::ExtraHigh,
        world::Tier::Max => t::Tier::Max,
        world::Tier::Ultra => t::Tier::Ultra,
    }
}

pub fn world_tier(x: t::Tier) -> world::Tier {
    match x {
        t::Tier::Low => world::Tier::Low,
        t::Tier::Middle => world::Tier::Middle,
        t::Tier::High => world::Tier::High,
        t::Tier::ExtraHigh => world::Tier::ExtraHigh,
        t::Tier::Max => world::Tier::Max,
        t::Tier::Ultra => world::Tier::Ultra,
    }
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
pub fn unit_citation(u: &folio::Unit) -> t::Citation {
    let n = |x: Option<i64>| u32::try_from(x.unwrap_or(0).max(0)).unwrap_or(u32::MAX);
    t::Citation {
        page: u.page.clone(),
        revid: n(u.revid),
        span_from: n(u.span_from),
        span_to: n(u.span_to),
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

pub fn fact_kind(k: world::FactKind) -> v::FactKind {
    match k {
        world::FactKind::Fact => v::FactKind::Fact,
        world::FactKind::Commitment => v::FactKind::Commitment,
        world::FactKind::Hurt => v::FactKind::Hurt,
    }
}

pub fn fact_scope(s: v::FactScope) -> world::fact::Scope {
    use world::fact::Scope;
    match s {
        v::FactScope::All => Scope::All,
        v::FactScope::World => Scope::World,
        v::FactScope::Own => Scope::Own,
        v::FactScope::Commitment => Scope::Commitment,
        v::FactScope::Hurt => Scope::Hurt,
        v::FactScope::Deleted => Scope::Deleted,
    }
}

pub fn origin(o: world::Origin) -> v::Origin {
    match o {
        world::Origin::User => v::Origin::User,
        world::Origin::System => v::Origin::System,
    }
}

/// Spending in the shapes the UI knows; an interjection is group talk.
pub fn call_shape(s: world::Shape) -> CallShape {
    match s {
        world::Shape::Direct => CallShape::Direct,
        world::Shape::Group | world::Shape::Interject => CallShape::Group,
        world::Shape::Opening => CallShape::Opening,
        world::Shape::Host => CallShape::Host,
        world::Shape::Wrapup => CallShape::Wrapup,
    }
}

/// The quota panel: band, the window, and the calls spent per shape in it.
pub fn quota_status(
    conn: &rusqlite::Connection,
    status: &quota::Status,
    now: world::clock::Now,
    params: &world::params::Params,
) -> world::Result<v::QuotaStatus> {
    let pace = params.pacing.get(status.tier);
    let mut spent: Vec<v::ShapeSpend> = Vec::new();
    for (shape, calls) in quota::spent_by_shape(conn, now.ms, &params.quota)? {
        let shape = call_shape(shape);
        match spent.iter_mut().find(|s| s.shape == shape) {
            Some(s) => s.calls += calls,
            None => spent.push(v::ShapeSpend { shape, calls }),
        }
    }
    Ok(v::QuotaStatus {
        tier: tier(status.tier),
        band: match status.band {
            Band::Open => QuotaBand::Open,
            Band::Quiet => QuotaBand::Quiet,
            Band::Exhausted => QuotaBand::Exhausted,
        },
        window: status.window.as_ref().map(|w| QuotaWindow {
            used: w.used,
            release_at: w.release_at.map(|ms| ts(ms, now.offset_min)),
        }),
        spent,
        visits_per_day: pace.daily_total,
        events_max: pace.events_max,
    })
}

/// How a world error crosses the seam. Validation the api does itself names its own
/// field; what is left here is the world refusing a request as a whole.
pub fn world_err(e: world::Error) -> ApiError {
    match e {
        world::Error::NotFound { .. } => ApiError::NotFound,
        world::Error::Locked => ApiError::Locked,
        world::Error::Disabled(p) => ApiError::Disabled {
            person: person_id(&p),
        },
        world::Error::Invalid(_)
        | world::Error::NotParticipant { .. }
        | world::Error::ChannelClosed(_) => ApiError::Invalid {
            field: "request".into(),
        },
        other => ApiError::Io {
            detail: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_carry_the_offset_and_parse_back() {
        assert_eq!(ts(0, 0).0, "1970-01-01T00:00:00+00:00");
        assert_eq!(ts(0, 480).0, "1970-01-01T08:00:00+08:00");
        assert_eq!(ts(0, -90).0, "1969-12-31T22:30:00-01:30");
    }

    #[test]
    fn ids_round_trip_and_garbage_is_not_found() {
        assert_eq!(
            world_channel(&channel_id(world::ChannelId(7))),
            Ok(world::ChannelId(7))
        );
        assert_eq!(
            world_message(&t::MessageId::from("x")),
            Err(ApiError::NotFound)
        );
    }
}
