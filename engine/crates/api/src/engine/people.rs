//! The waiting panel, the roster, person pages, finding people, modes and avatars.

use std::collections::{HashMap, HashSet};

use world::{bond, channel};

use super::{Engine, io};
use crate::convert::{self, world_err, world_mode, world_person};
use crate::error::{ApiError, ApiResult};
use crate::events::{ModeChanged, RosterChanged};
use crate::media;
use crate::port::Event;
use crate::types::*;
use crate::views::*;

fn activity(a: world::presence::Activity, offset: i32) -> Activity {
    use world::presence::Activity as W;
    let ts = |ms| convert::ts(ms, offset);
    match a {
        W::Messaged { at } => Activity::Messaged { at: ts(at) },
        W::CommitmentDue { fact, text, due } => Activity::CommitmentDue {
            fact: convert::fact_id(fact),
            text,
            due: ts(due),
        },
        W::CameBy { at, .. } => Activity::CameBy { at: ts(at) },
        W::Birthday => Activity::Birthday,
    }
}

impl Engine {
    /// The daily events, newest first. A person the corpus no longer has is left out.
    pub(crate) fn daily_events_impl(&self) -> ApiResult<Vec<DailyEvent>> {
        let offset = self.now().offset_min;
        let events =
            world::event::recent(&self.w(), self.params.world.events.list).map_err(world_err)?;
        Ok(events
            .into_iter()
            .map(|e| DailyEvent {
                day: e.day,
                at: convert::ts(e.at, offset),
                title: e.title,
                text: e.text,
                persons: e
                    .persons
                    .iter()
                    .map(|p| self.person_ref(p.as_str()))
                    .collect(),
                message: e.message.map(convert::message_id),
            })
            .collect())
    }

    /// The waiting panel: disabled people and people without a persona are left out.
    pub(crate) fn digest_impl(&self) -> ApiResult<Digest> {
        let Some(c) = &self.corpus else {
            return Ok(Digest::default());
        };
        let now = self.now();
        let birthdays: Vec<(world::PersonId, world::MonthDay)> = c
            .persons()
            .iter()
            .filter_map(|p| Some((world::PersonId(p.id.clone()), p.birthday?)))
            .collect();
        let w = self.w();
        let d = world::presence::digest(&w, now, &birthdays).map_err(world_err)?;
        let mut items = Vec::new();
        for item in d.items {
            if !c.has_persona(item.person.as_str()) {
                continue;
            }
            // A click needs somewhere to go: a birthday has no channel yet.
            let ch = match item.channel {
                Some(ch) => ch,
                None => channel::direct(&w, &item.person).map_err(world_err)?,
            };
            items.push(Waiting {
                person: self.person_ref(item.person.as_str()),
                channel: convert::channel_id(ch),
                activity: activity(item.activity, now.offset_min),
            });
        }
        Ok(Digest { items })
    }

    pub(crate) async fn presence_impl(&self) -> ApiResult<Digest> {
        let digest = self.digest_impl()?;
        // What the model adds arrives as events; a failure there never hides the panel.
        let _ = self.ports.agent.presence(true).await;
        Ok(digest)
    }

    pub(crate) fn coldstart_impl(&self) -> ApiResult<Vec<Candidate>> {
        let Some(c) = &self.corpus else {
            return Ok(Vec::new());
        };
        let namer = self.namer()?;
        let w = self.w();
        let mut out = Vec::new();
        for (id, reason) in c.coldstart() {
            // The built list can name someone the corpus has no persona for.
            if !c.has_persona(id) {
                continue;
            }
            let mode = bond::mode(&w, &world::PersonId(id.clone())).map_err(world_err)?;
            if mode == world::Mode::Disabled {
                continue;
            }
            out.push(Candidate {
                person: self.person_ref(id),
                mode: convert::mode(mode),
                reason: namer.show(reason),
                citation: None,
            });
            if out.len() >= self.params.coldstart.picks {
                break;
            }
        }
        Ok(out)
    }

    pub(crate) fn roster_impl(&self) -> ApiResult<Roster> {
        let Some(c) = &self.corpus else {
            return Ok(Roster::default());
        };
        let recent: HashMap<String, Activity> = {
            let mut m = HashMap::new();
            for item in self.digest_impl()?.items {
                m.entry(item.person.id.0).or_insert(item.activity);
            }
            m
        };
        let bonds: HashMap<String, bond::Bond> = bond::user_bonds(&self.w())
            .map_err(world_err)?
            .into_iter()
            .map(|b| (b.b.0.clone(), b))
            .collect();
        let off: HashSet<&str> = bonds
            .values()
            .filter(|b| b.mode == world::Mode::Disabled)
            .map(|b| b.b.as_str())
            .collect();

        struct Row {
            row: RosterRow,
            seen: i64,
            confidence: f64,
        }
        let mut bands: [Vec<Row>; 4] = Default::default();
        let mut enabled = 0;
        for p in c.persons() {
            let bond = bonds.get(&p.id);
            let mode = bond.map_or(world::Mode::Frozen, |b| b.mode);
            let trust = bond.map_or(world::bond::USER_TRUST as u8, |b| b.trust);
            let seen = bond.and_then(|b| b.last_seen);
            let band = if mode == world::Mode::Disabled {
                3
            } else if seen.is_none() {
                2
            } else if trust >= self.params.roster.close_trust {
                0
            } else {
                1
            };
            if mode == world::Mode::Enabled {
                enabled += 1;
            }
            let companions = c
                .companions(&p.id)
                .iter()
                .filter(|q| !off.contains(q.as_str()))
                .take(self.params.roster.companions)
                .map(|q| self.person_ref(q))
                .collect();
            bands[band].push(Row {
                row: RosterRow {
                    person: self.person_ref(&p.id),
                    mode: convert::mode(mode),
                    has_persona: c.has_persona(&p.id),
                    talked: seen.is_some(),
                    recent: recent.get(&p.id).cloned(),
                    companions,
                },
                seen: seen.unwrap_or(0),
                confidence: p.persona_confidence,
            });
        }
        let order = [
            RosterBand::Close,
            RosterBand::Acquainted,
            RosterBand::NeverTalked,
            RosterBand::Disabled,
        ];
        let mut groups = Vec::new();
        for (band, mut rows) in order.into_iter().zip(bands) {
            if rows.is_empty() {
                continue;
            }
            match band {
                // Contact order: most recent first.
                RosterBand::Close | RosterBand::Acquainted => rows.sort_by(|a, b| {
                    b.seen
                        .cmp(&a.seen)
                        .then_with(|| a.row.person.id.cmp(&b.row.person.id))
                }),
                // Nobody to go by yet: people who can be talked to first, then material.
                RosterBand::NeverTalked => rows.sort_by(|a, b| {
                    b.row
                        .has_persona
                        .cmp(&a.row.has_persona)
                        .then_with(|| b.confidence.total_cmp(&a.confidence))
                        .then_with(|| a.row.person.id.cmp(&b.row.person.id))
                }),
                RosterBand::Disabled => {}
            }
            groups.push(RosterGroup {
                band,
                rows: rows.into_iter().map(|r| r.row).collect(),
            });
        }
        Ok(Roster {
            total: u32::try_from(c.persons().len()).unwrap_or(u32::MAX),
            enabled,
            groups,
        })
    }

    pub(crate) fn person_page_impl(&self, person: &PersonId) -> ApiResult<PersonPage> {
        let c = self.corpus()?;
        let p = c.person(&person.0).ok_or(ApiError::NotFound)?;
        let namer = self.namer()?;
        let summary = namer.show_opt(c.summary(&p.id)?.as_deref());
        let wp = world_person(person);
        let offset = self.now().offset_min;
        let w = self.w();
        let b = bond::user_bond(&w, &wp).map_err(world_err)?;
        let off: HashSet<world::PersonId> =
            bond::disabled(&w).map_err(world_err)?.into_iter().collect();
        Ok(PersonPage {
            person: self.person_ref(&p.id),
            summary,
            trust: b.trust,
            mode: convert::mode(b.mode),
            has_persona: c.has_persona(&p.id),
            last_seen: b.last_seen.map(|ms| convert::ts(ms, offset)),
            companions: c
                .companions(&p.id)
                .iter()
                .filter(|q| !off.contains(&world::PersonId((*q).clone())))
                .take(self.params.roster.companions)
                .map(|q| self.person_ref(q))
                .collect(),
            voice: world::settings::voice(&w, &wp).map_err(world_err)?,
            channel: channel::find_direct_with(&w, &wp)
                .map_err(world_err)?
                .map(convert::channel_id),
        })
    }

    /// People who know about a topic. Only people with a persona are candidates (the
    /// scope is applied before the cut, so it never shortens the list), and disabled
    /// people are excluded the same way.
    pub(crate) fn find_people_impl(&self, topic: &str) -> ApiResult<Vec<Candidate>> {
        let c = self.corpus()?;
        let namer = self.namer()?;
        let scope = c.persona_ids();
        let (off, bonds) = {
            let w = self.w();
            (
                bond::disabled(&w)
                    .map_err(world_err)?
                    .into_iter()
                    .map(|p| p.0)
                    .collect::<Vec<String>>(),
                bond::user_bonds(&w).map_err(world_err)?,
            )
        };
        let found = c.with_index(|ix| {
            ix.find_people(topic, Some(&scope), self.params.find_people.k, &off)
        })?;
        Ok(found
            .persons
            .iter()
            .map(|m| Candidate {
                person: self.person_ref(m.person.as_str()),
                mode: convert::mode(
                    bonds
                        .iter()
                        .find(|b| b.b.0 == m.person)
                        .map_or(world::Mode::Frozen, |b| b.mode),
                ),
                reason: namer.show(
                    m.reason
                        .text
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .unwrap_or_default(),
                ),
                citation: Some(convert::unit_citation(&m.reason)),
            })
            .collect())
    }

    pub(crate) fn set_mode_impl(&self, target: Target, mode: Mode) -> ApiResult<()> {
        match &target {
            Target::Person { id } => {
                let c = self.corpus()?;
                if c.person(&id.0).is_none() {
                    return Err(ApiError::NotFound);
                }
                // Enabling lets someone speak unprompted; without a persona they cannot.
                if mode == Mode::Enabled && !c.has_persona(&id.0) {
                    return Err(ApiError::NoPersona { person: id.clone() });
                }
                bond::set_mode(&self.w(), &world_person(id), world_mode(mode))
                    .map_err(world_err)?;
                self.emit(Event::RosterChanged(RosterChanged {
                    persons: vec![id.clone()],
                }));
            }
            Target::Channel { id } => {
                let ch = convert::world_channel(id)?;
                let w = self.w();
                let found = channel::get(&w, ch).map_err(world_err)?;
                if found.kind != world::ChannelKind::Group {
                    return Err(ApiError::Invalid {
                        field: "target".into(),
                    });
                }
                channel::set_mode(&w, ch, world_mode(mode)).map_err(world_err)?;
            }
        }
        self.emit(Event::ModeChanged(ModeChanged { target, mode }));
        Ok(())
    }

    /// The portrait from the cache, else fetched by page title from the source site.
    /// Offline, or no pattern in the corpus: `None`, and the UI draws a letter block.
    pub(crate) async fn avatar_impl(&self, person: &PersonId) -> ApiResult<Option<MediaFile>> {
        let c = self.corpus()?;
        let p = c.person(&person.0).ok_or(ApiError::NotFound)?;
        self.cached_avatar(&p.primary_page, c.avatar_url(&p.primary_page))
            .await
    }

    /// The host's picture: the corpus's `host_avatar_url`, cached like a person's.
    pub(crate) async fn host_avatar_impl(&self) -> ApiResult<Option<MediaFile>> {
        match self.corpus.as_ref().and_then(|c| c.host_avatar_url()) {
            Some(url) => self.cached_avatar("~host", Some(url)).await,
            None => Ok(None),
        }
    }

    /// A person's portrait only if it is already in the cache: no fetch, no waiting, for
    /// callers (a system notification) that must not touch the network.
    pub fn cached_person_avatar(&self, person: &str) -> Option<std::path::PathBuf> {
        let page = &self.corpus.as_ref()?.person(person)?.primary_page;
        self.media.get("avatar", &media::stem(&["avatar", page]))
    }

    /// The host's picture only if it is already in the cache.
    pub fn cached_host_avatar(&self) -> Option<std::path::PathBuf> {
        self.media.get("avatar", &media::stem(&["avatar", "~host"]))
    }

    /// An avatar for the page `page`: cached, else fetched from `url` (when there is one).
    pub(crate) async fn cached_avatar(
        &self,
        page: &str,
        url: Option<String>,
    ) -> ApiResult<Option<MediaFile>> {
        let stem = media::stem(&["avatar", page]);
        let file = |path: std::path::PathBuf| MediaFile {
            path: path.to_string_lossy().into_owned(),
        };
        if let Some(path) = self.media.get("avatar", &stem) {
            return Ok(Some(file(path)));
        }
        let Some(url) = url else {
            return Ok(None);
        };
        let Ok(_slot) = self.fetches.acquire().await else {
            return Ok(None);
        };
        // A queued call may find the file already fetched by the one ahead of it.
        if let Some(path) = self.media.get("avatar", &stem) {
            return Ok(Some(file(path)));
        }
        let Ok(Some(bytes)) = self.ports.net.get(&url).await else {
            return Ok(None);
        };
        let Some(ext) = media::image_ext(&bytes) else {
            return Ok(None);
        };
        let path = self.media.put("avatar", &stem, ext, &bytes).map_err(io)?;
        Ok(Some(file(path)))
    }
}
