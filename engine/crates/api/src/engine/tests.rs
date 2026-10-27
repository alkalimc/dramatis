use std::sync::Mutex;

use folio::fixture::{Builder, TempFolio, UnitSpec};

use super::*;
use crate::endpoints::WireApi;
use crate::port::{FixedClock, Recorder};

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_home() -> Home {
    let dir = std::env::temp_dir().join(format!(
        "dramatis-api-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Home::new(dir)
}

fn at(rfc: &str) -> Now {
    Now::utc(
        chrono::DateTime::parse_from_rfc3339(rfc)
            .unwrap()
            .timestamp_millis(),
    )
}

fn persona(b: &Builder, who: &str, slots: &[&str]) {
    for slot in slots {
        b.conn()
            .execute(
                "INSERT INTO prompts(subject, slot, body) VALUES (?1, ?2, 'text')",
                [who, slot],
            )
            .unwrap();
    }
}

const ALL: [&str; 3] = ["system", "tone", "capability"];

/// Three people: `amy` and `kal` have personas, `zed` has half of one.
fn fixture() -> TempFolio {
    let mut b = Builder::new()
        .manifest("user_placeholder", r#""<U>""#)
        .manifest("wording", r#"{"user.you":"Doc","host.name":"Host"}"#)
        .manifest("template_shapes", r#"{"card":"profile","talk":"dialogue"}"#)
        .manifest("clock.year_offset", "1000")
        .manifest("source_url_pattern", r#""https://wiki.example/{page}""#)
        .manifest(
            "revision_url_pattern",
            r#""https://wiki.example/?oldid={revid}&p={page}""#,
        )
        .manifest("avatar_url_pattern", r#""https://img.example/{page}.png""#);
    b.person("amy", Some("03-05"));
    b.person("kal", None);
    b.person("zed", None);
    b.cooccur("amy", "kal", 5).cooccur("amy", "zed", 2);
    b.unit(UnitSpec {
        id: "u-amy-card",
        template: "card",
        page: "Amy",
        text: "Amy leads the base.\nSecond line.",
        persons: &["amy"],
        ..UnitSpec::default()
    });
    b.unit(UnitSpec {
        id: "u-amy-talk",
        template: "talk",
        page: "Story 1",
        header: "Amy speaks to <U>",
        text: "Hello <U>, welcome to the lantern base",
        persons: &["amy"],
        span: Some(("Story 1", 10, 12)),
    });
    b.unit(UnitSpec {
        id: "u-kal-talk",
        template: "talk",
        page: "Story 2",
        text: "Kal studies the furnace carefully",
        persons: &["kal"],
        ..UnitSpec::default()
    });
    b.unit(UnitSpec {
        id: "u-zed-talk",
        template: "talk",
        page: "Story 3",
        text: "Zed guards the anvil gate",
        persons: &["zed"],
        ..UnitSpec::default()
    });
    persona(&b, "amy", &ALL);
    persona(&b, "kal", &ALL);
    persona(&b, "zed", &["system"]);
    b.conn()
        .execute(
            "INSERT INTO prompts(subject, slot, body) VALUES ('host', 'coldstart', ?1)",
            [r#"[{"person_id":"amy","reason":"Knows <U> well"},{"person_id":"zed","reason":"Guards"}]"#],
        )
        .unwrap();
    b.finish()
}

#[derive(Default)]
struct Spy {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentPort for Spy {
    async fn send_message(
        &self,
        c: ChannelId,
        t: String,
        _: Option<ImagePart>,
    ) -> ApiResult<MessageId> {
        self.calls.lock().unwrap().push(format!("send {} {t}", c.0));
        Ok(MessageId("1".into()))
    }
    async fn relay(&self, _: String, _: MessageId, to: PersonId) -> ApiResult<ChannelId> {
        self.calls.lock().unwrap().push(format!("relay {}", to.0));
        Ok(ChannelId("1".into()))
    }
    async fn ask(
        &self,
        p: PersonId,
        q: String,
        _: Option<u32>,
        _: Option<TaskId>,
    ) -> ApiResult<TaskId> {
        self.calls.lock().unwrap().push(format!("ask {} {q}", p.0));
        Ok(TaskId("1".into()))
    }
    async fn presence(&self, _: bool) -> ApiResult<()> {
        self.calls.lock().unwrap().push("presence".into());
        Ok(())
    }
    async fn on_event(&self) -> ApiResult<()> {
        self.calls.lock().unwrap().push("event".into());
        Ok(())
    }
    async fn on_rename(&self) -> ApiResult<()> {
        self.calls.lock().unwrap().push("rename".into());
        Ok(())
    }
    fn set_endpoints(&self, _: &Endpoints) {}
}

struct Net(Option<Vec<u8>>, Mutex<u32>);

#[async_trait]
impl NetPort for Net {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
        *self.1.lock().unwrap() += 1;
        self.0
            .clone()
            .map(Some)
            .ok_or_else(|| "offline".to_string())
    }
    async fn reachable(&self, _: &str) -> bool {
        self.0.is_some()
    }
}

struct Rig {
    engine: Engine,
    spy: Arc<Spy>,
    events: Arc<Recorder>,
    _folio: TempFolio,
}

fn rig_with(net: Arc<dyn NetPort>) -> Rig {
    rig_full(net, "")
}

fn rig_full(net: Arc<dyn NetPort>, params: &str) -> Rig {
    let folio = fixture();
    let home = temp_home();
    home.create().unwrap();
    std::fs::write(home.params(), params).unwrap();
    let spy = Arc::new(Spy::default());
    let events = Arc::new(Recorder::default());
    let ports = Ports {
        agent: spy.clone(),
        sink: events.clone(),
        net,
        clock: Arc::new(FixedClock(at("2026-03-05T12:00:00+00:00"))),
        ..Ports::default()
    };
    let engine = Engine::open(home, Some(folio.path().to_path_buf()), ports).unwrap();
    Rig {
        engine,
        spy,
        events,
        _folio: folio,
    }
}

fn rig() -> Rig {
    rig_with(Arc::new(Offline))
}

fn profile(name: &str) -> Profile {
    Profile {
        name: name.into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        wire_api: WireApi::Chat,
        models: Vec::new(),
    }
}

fn chat_role(profile: &str) -> Role {
    Role {
        profile: profile.into(),
        model: "m".into(),
        reasoning: None,
    }
}

fn id(s: &str) -> PersonId {
    PersonId(s.into())
}

async fn name_user(e: &Engine, name: &str) {
    let mut s = e.settings().await.unwrap();
    s.user_name = Some(name.into());
    Api::update_settings(e, s).await.unwrap();
}

#[tokio::test]
async fn status_clock_and_wording() {
    let r = rig();
    let e = &r.engine;
    let s = e.app_status().await.unwrap();
    assert_eq!(s.form, AppForm::Library);
    assert!(s.needs_setup);
    assert!(matches!(
        s.corpus,
        CorpusState::Loaded {
            encoder_mismatch: false,
            ..
        }
    ));

    e.add_profile(profile("p")).await.unwrap();
    e.set_role(RoleKind::Chat, Some(chat_role("p")))
        .await
        .unwrap();
    assert_eq!(e.app_status().await.unwrap().form, AppForm::Full);
    // No network: back to the library.
    e.online.store(false, Ordering::Relaxed);
    assert_eq!(e.app_status().await.unwrap().form, AppForm::Library);

    let c = Api::clock(e).await.unwrap();
    assert_eq!(
        (c.in_world.year, c.in_world.month, c.in_world.day),
        (1026, 3, 5)
    );
    assert_eq!(c.local_time, "12:00");
    let w = e.wording().await.unwrap();
    assert_eq!(w.entries["host.name"], "Host");
}

#[tokio::test]
async fn missing_and_refused_corpora_are_states_not_failures() {
    let home = temp_home();
    let e = Engine::open(home.clone(), None, Ports::default()).unwrap();
    assert_eq!(e.app_status().await.unwrap().corpus, CorpusState::Missing);
    assert_eq!(e.roster().await.unwrap(), Roster::default());
    assert_eq!(
        e.search_corpus("x".into(), vec![]).await,
        Err(ApiError::NotFound)
    );

    let bad = Builder::new().manifest("format_version", "99").finish();
    let e = Engine::open(
        home.clone(),
        Some(bad.path().to_path_buf()),
        Ports::default(),
    )
    .unwrap();
    assert_eq!(
        e.app_status().await.unwrap().corpus,
        CorpusState::UnsupportedVersion { format_version: 99 }
    );
    let needy = Builder::new()
        .manifest("requires", r#"["time_travel"]"#)
        .finish();
    let e = Engine::open(home, Some(needy.path().to_path_buf()), Ports::default()).unwrap();
    assert!(matches!(
        e.app_status().await.unwrap().corpus,
        CorpusState::MissingRequirement { .. }
    ));
}

#[tokio::test]
async fn encoder_mismatch_shows_a_banner_and_keeps_going() {
    let folio = Builder::new()
        .manifest("embed_model", r#""enc-a""#)
        .finish();
    let mismatch = |encoder: Option<&str>| {
        let ports = Ports {
            encoder: encoder.map(str::to_owned),
            ..Ports::default()
        };
        Engine::open(temp_home(), Some(folio.path().to_path_buf()), ports).unwrap()
    };
    let state = |e: &Engine| e.state.clone();
    assert!(matches!(
        state(&mismatch(None)),
        CorpusState::Loaded {
            encoder_mismatch: true,
            ..
        }
    ));
    assert!(matches!(
        state(&mismatch(Some("enc-b"))),
        CorpusState::Loaded {
            encoder_mismatch: true,
            ..
        }
    ));
    assert!(matches!(
        state(&mismatch(Some("enc-a"))),
        CorpusState::Loaded {
            encoder_mismatch: false,
            ..
        }
    ));
}

#[tokio::test]
async fn the_user_name_replaces_the_placeholder_everywhere_corpus_text_shows() {
    let r = rig();
    let e = &r.engine;
    let hit = |res: &SearchResults| {
        res.hits
            .iter()
            .find(|h| h.citation.chunk_id == "u-amy-talk")
            .unwrap()
            .clone()
    };

    // No name yet: the pack's word for "you".
    let found = e.search_corpus("lantern".into(), vec![]).await.unwrap();
    let p = hit(&found);
    assert_eq!(p.text, "Hello Doc, welcome to the lantern base");
    assert_eq!(p.header, "Amy speaks to Doc");
    assert_eq!(e.coldstart().await.unwrap()[0].reason, "Knows Doc well");

    name_user(e, "Ada").await;
    let p = hit(&e.search_corpus("lantern".into(), vec![]).await.unwrap());
    assert_eq!(p.text, "Hello Ada, welcome to the lantern base");
    let cited = e.resolve_citation(p.citation.clone()).await.unwrap();
    assert_eq!(cited.passage.unwrap().header, "Amy speaks to Ada");
    let people = e.find_people("lantern".into()).await.unwrap();
    assert!(people[0].reason.contains("Ada"), "{:?}", people[0].reason);
    assert_eq!(e.coldstart().await.unwrap()[0].reason, "Knows Ada well");

    // The corpus itself is never written.
    let raw: String = e
        .corpus()
        .unwrap()
        .with_folio(|f| Ok(f.unit_by_id("u-amy-talk")?.unwrap().text))
        .unwrap();
    assert!(raw.contains("<U>"));
    // A rename rolls every session over.
    assert!(r.spy.calls.lock().unwrap().contains(&"rename".to_string()));
}

#[tokio::test]
async fn citations_resolve_in_three_levels() {
    let r = rig();
    let e = &r.engine;
    let current = Citation {
        page: "Story 1".into(),
        revid: 1,
        span_from: 10,
        span_to: 12,
        chunk_id: "u-amy-talk".into(),
    };
    let v = e.resolve_citation(current.clone()).await.unwrap();
    assert_eq!(v.status, CitationStatus::Current);
    assert_eq!(
        v.source_url.as_deref(),
        Some("https://wiki.example/Story_1")
    );

    // The page changed: the id is gone, the span still overlaps.
    let moved = Citation {
        chunk_id: "old-hash".into(),
        span_from: 11,
        span_to: 20,
        ..current.clone()
    };
    let v = e.resolve_citation(moved).await.unwrap();
    assert_eq!(v.status, CitationStatus::Updated);
    assert_eq!(v.passage.unwrap().citation.chunk_id, "u-amy-talk");

    let gone = Citation {
        chunk_id: "old-hash".into(),
        span_from: 500,
        span_to: 510,
        ..current
    };
    let v = e.resolve_citation(gone).await.unwrap();
    assert_eq!(v.status, CitationStatus::Gone);
    assert!(v.passage.is_none());
    assert_eq!(
        v.revision_url.as_deref(),
        Some("https://wiki.example/?oldid=1&p=Story_1")
    );
}

#[tokio::test]
async fn endpoints_round_trip_through_the_file_and_the_keychain() {
    let r = rig();
    let e = &r.engine;
    assert!(!e.endpoints().await.unwrap().presets.is_empty());
    e.add_profile(profile("a")).await.unwrap();
    assert_eq!(
        e.add_profile(profile("a")).await,
        Err(ApiError::Invalid {
            field: "name".into()
        })
    );
    e.set_key("a".into(), "sk-secret".into()).await.unwrap();
    assert_eq!(
        e.set_key("nope".into(), "k".into()).await,
        Err(ApiError::NotFound)
    );
    e.set_role(RoleKind::Chat, Some(chat_role("a")))
        .await
        .unwrap();
    e.set_role(RoleKind::Tts, Some(chat_role("a")))
        .await
        .unwrap();
    e.set_role(RoleKind::Host, Some(chat_role("a")))
        .await
        .unwrap();

    let view = e.endpoints().await.unwrap();
    assert!(view.profiles[0].has_key);
    assert!(!serde_json::to_string(&view).unwrap().contains("sk-secret"));
    let file = std::fs::read_to_string(e.home.endpoints()).unwrap();
    assert!(!file.contains("sk-secret"));
    assert_eq!(Endpoints::from_toml(&file).unwrap().roles, view.roles);

    // A profile in use cannot be removed; renaming moves roles and key along.
    assert_eq!(
        e.remove_profile("a".into()).await,
        Err(ApiError::Invalid {
            field: "profile".into()
        })
    );
    e.update_profile("a".into(), profile("b")).await.unwrap();
    let view = e.endpoints().await.unwrap();
    assert_eq!(view.roles.as_ref().unwrap().chat.profile, "b");
    assert_eq!(
        view.roles.as_ref().unwrap().host.as_ref().unwrap().profile,
        "b"
    );
    assert!(view.profiles[0].has_key && !r.engine.ports.keys.has("a"));

    e.set_role(RoleKind::Tts, None).await.unwrap();
    e.set_role(RoleKind::Host, None).await.unwrap();
    assert!(e.endpoints().await.unwrap().roles.unwrap().host.is_none());
    assert_eq!(
        e.set_role(RoleKind::Chat, None).await,
        Err(ApiError::Invalid {
            field: "role".into()
        })
    );
    assert_eq!(
        e.set_role(RoleKind::Chat, Some(chat_role("zz"))).await,
        Err(ApiError::Invalid {
            field: "profile".into()
        })
    );

    // A fresh engine over the same home reads the same state back.
    let again = Engine::open(e.home.clone(), None, Ports::default()).unwrap();
    assert_eq!(
        again.endpoints().await.unwrap().roles,
        e.endpoints().await.unwrap().roles
    );

    // Model listing and voices sit behind the adapter until the client is wired.
    assert_eq!(
        e.fetch_models("b".into()).await,
        Err(ApiError::NotImplemented)
    );
    assert_eq!(e.fetch_models("zz".into()).await, Err(ApiError::NotFound));
    assert_eq!(e.tts_voices().await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn people_without_a_persona_are_visible_but_cannot_be_reached() {
    let r = rig();
    let e = &r.engine;
    e.add_profile(profile("p")).await.unwrap();
    e.set_role(RoleKind::Chat, Some(chat_role("p")))
        .await
        .unwrap();

    let roster = e.roster().await.unwrap();
    let rows: Vec<&RosterRow> = roster.groups.iter().flat_map(|g| &g.rows).collect();
    let flag = |who: &str| {
        rows.iter()
            .find(|r| r.person.id.0 == who)
            .unwrap()
            .has_persona
    };
    assert!(flag("amy") && flag("kal") && !flag("zed"));
    assert!(!e.person_page(id("zed")).await.unwrap().has_persona);
    assert!(e.person_page(id("amy")).await.unwrap().has_persona);

    let none = ApiError::NoPersona { person: id("zed") };
    assert_eq!(e.open_direct(id("zed")).await, Err(none.clone()));
    assert_eq!(
        e.ask(id("zed"), "q".into(), None, None).await,
        Err(none.clone())
    );
    assert_eq!(
        e.relay("x".into(), MessageId("1".into()), id("zed")).await,
        Err(ApiError::NotFound)
    );
    assert_eq!(
        e.create_group(vec![id("amy"), id("zed")], None).await,
        Err(none.clone())
    );
    assert_eq!(
        e.set_mode(Target::Person { id: id("zed") }, Mode::Enabled)
            .await,
        Err(none)
    );
    // Freezing or disabling someone is harmless.
    e.set_mode(Target::Person { id: id("zed") }, Mode::Disabled)
        .await
        .unwrap();

    // Retrieval offers only people who can answer; so does the first-run card.
    let found = e.find_people("anvil gate".into()).await.unwrap();
    assert!(found.iter().all(|c| c.person.id.0 != "zed"));
    let cold: Vec<_> = e
        .coldstart()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.person.id.0)
        .collect();
    assert_eq!(cold, ["amy"]);
    assert!(
        e.find_people("furnace".into())
            .await
            .unwrap()
            .iter()
            .any(|c| c.person.id.0 == "kal")
    );

    // Chatting with someone who can be reached goes to the agent.
    let ch = e.open_direct(id("amy")).await.unwrap();
    e.send_message(ch.clone(), "hi".into(), None).await.unwrap();
    assert_eq!(
        r.spy.calls.lock().unwrap().last().unwrap(),
        &format!("send {} hi", ch.0)
    );
}

#[tokio::test]
async fn no_chat_role_means_no_model_commands() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    assert_eq!(
        e.send_message(ch, "hi".into(), None).await,
        Err(ApiError::NoEndpoint)
    );
    assert_eq!(
        e.ask(id("amy"), "q".into(), None, None).await,
        Err(ApiError::NoEndpoint)
    );
    assert_eq!(
        e.send_message(ChannelId("1".into()), " ".into(), None)
            .await,
        Err(ApiError::Invalid {
            field: "text".into()
        })
    );
}

#[tokio::test]
async fn roster_groups_by_contact_and_collapses_the_disabled() {
    let r = rig();
    let e = &r.engine;
    e.set_mode(Target::Person { id: id("zed") }, Mode::Disabled)
        .await
        .unwrap();
    e.set_mode(Target::Person { id: id("amy") }, Mode::Enabled)
        .await
        .unwrap();
    // Talking to kal makes him a contact.
    let ch = e.open_direct(id("kal")).await.unwrap();
    {
        let w = e.w();
        world::message::append(
            &w,
            convert::world_channel(&ch).unwrap(),
            &world::Actor::User,
            "hello",
            None,
            None,
            e.now().ms,
        )
        .unwrap();
    }
    let roster = e.roster().await.unwrap();
    assert_eq!((roster.total, roster.enabled), (3, 1));
    let bands: Vec<RosterBand> = roster.groups.iter().map(|g| g.band).collect();
    assert_eq!(
        bands,
        [
            RosterBand::Acquainted,
            RosterBand::NeverTalked,
            RosterBand::Disabled
        ]
    );
    let never = &roster.groups[1].rows;
    assert_eq!(never[0].person.id.0, "amy");
    // Companions never include the disabled.
    assert!(never[0].companions.iter().all(|c| c.id.0 != "zed"));
    assert_eq!(never[0].companions[0].id.0, "kal");
    let page = e.person_page(id("kal")).await.unwrap();
    assert_eq!(page.channel, Some(ch));
    assert_eq!(page.trust, 100);
    assert_eq!(page.summary, None);
    assert_eq!(
        e.person_page(id("amy")).await.unwrap().summary.as_deref(),
        Some("Amy leads the base.")
    );
}

#[tokio::test]
async fn channels_history_groups_and_read_marks() {
    let r = rig();
    let e = &r.engine;
    let host = e.host_channel().await.unwrap();
    let direct = e.open_direct(id("amy")).await.unwrap();
    assert_eq!(e.open_direct(id("amy")).await.unwrap(), direct);
    // An empty direct channel is not a conversation yet; the host's own always is.
    let list = e.channels().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, ChannelKind::Host);

    let first = {
        let w = e.w();
        let ch = convert::world_channel(&direct).unwrap();
        let a =
            world::message::append(&w, ch, &world::Actor::User, "one", None, None, 1_000).unwrap();
        world::message::append(
            &w,
            ch,
            &world::Actor::Person("amy".into()),
            "two",
            None,
            Some(&serde_json::json!({"relayed": true, "confidence": "low"})),
            2_000,
        )
        .unwrap();
        a
    };
    let msgs = e.history(direct.clone(), None, 10).await.unwrap();
    assert_eq!(
        msgs.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(msgs[1].relayed && msgs[1].confidence == Some(Confidence::Low));
    let older = e
        .history(direct.clone(), Some(msgs[1].id.clone()), 10)
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    e.mark_read(direct.clone(), convert::message_id(first))
        .await
        .unwrap();
    assert_eq!(
        e.history(ChannelId("9999".into()), None, 5).await,
        Err(ApiError::NotFound)
    );

    let group = e
        .create_group(vec![id("amy"), id("kal")], Some(" plans ".into()))
        .await
        .unwrap();
    assert_eq!(
        e.create_group(vec![], None).await,
        Err(ApiError::Invalid {
            field: "members".into()
        })
    );
    e.set_mode(Target::Channel { id: group.clone() }, Mode::Enabled)
        .await
        .unwrap();
    e.delete_group(group.clone()).await.unwrap();
    assert_eq!(
        e.delete_group(direct).await,
        Err(ApiError::Invalid {
            field: "channel".into()
        })
    );
    let kinds: Vec<ChannelKind> = e.channels().await.unwrap().iter().map(|c| c.kind).collect();
    assert!(!kinds.contains(&ChannelKind::Group));
    assert_ne!(host, group);
    assert!(!r.events.take().is_empty());
}

#[tokio::test]
async fn conclusions_seed_the_people_their_material_touches() {
    // Two units cannot reach the production confidence bands; open them for this test.
    let r = rig_full(
        Arc::new(Offline),
        "[retrieve.confidence]\nlow_top1 = 0.0\nlow_entropy = 10.0\n",
    );
    let e = &r.engine;
    e.set_mode(Target::Person { id: id("kal") }, Mode::Enabled)
        .await
        .unwrap();
    let case = e
        .open_case("What does the furnace do?".into())
        .await
        .unwrap();
    assert!(e.tasks().await.unwrap()[0].pinned);
    let seeds = |e: &Engine, kind: &str| -> i64 {
        e.w()
            .query_row("SELECT count(*) FROM seed WHERE kind = ?1", [kind], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(seeds(e, "pinned"), 1);
    let fact = e
        .write_conclusion(case.clone(), "The furnace studies matter".into())
        .await
        .unwrap();
    assert_eq!(
        seeds(e, "conclusion"),
        1,
        "kal enabled, furnace matches his unit"
    );
    let file = e.case_file(case.clone()).await.unwrap();
    assert_eq!(file.conclusion.as_ref().unwrap().id, fact);
    assert!(r.spy.calls.lock().unwrap().iter().any(|c| c == "event"));

    // Replacing retracts the old one.
    let next = e
        .write_conclusion(case.clone(), "Something else".into())
        .await
        .unwrap();
    assert_ne!(next, fact);
    let all = e.facts(FactFilter::default()).await.unwrap();
    assert_eq!(all.items.len(), 1);
    assert_eq!(all.deleted, 1);

    e.unpin_task(case.clone()).await.unwrap();
    assert!(!e.tasks().await.unwrap()[0].pinned);
    assert_eq!(
        e.write_conclusion(case, " ".into()).await,
        Err(ApiError::Invalid {
            field: "text".into()
        })
    );
}

#[tokio::test]
async fn facts_edit_delete_and_restore() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    let made = {
        let w = e.w();
        world::fact::write(
            &w,
            &world::fact::NewFact::new(
                world::Actor::User,
                world::fact::Audience::Participants(convert::world_channel(&ch).unwrap()),
                world::FactKind::Fact,
                "Likes tea.",
            ),
            1,
        )
        .unwrap()
        .id()
    };
    let fid = convert::fact_id(made);
    let list = e.facts(FactFilter::default()).await.unwrap();
    assert_eq!((list.total, list.deleted), (1, 0));
    assert!(
        matches!(&list.items[0].audience, Audience::Participants { persons, .. } if persons[0].id.0 == "amy")
    );

    let edited = e
        .edit_fact(fid.clone(), "Likes green tea.".into())
        .await
        .unwrap();
    assert_ne!(edited, fid);
    e.delete_fact(edited.clone()).await.unwrap();
    let deleted = e
        .facts(FactFilter {
            scope: FactScope::Deleted,
            ..FactFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(deleted.items.len(), 2);
    e.restore_fact(edited).await.unwrap();
    assert_eq!(e.facts(FactFilter::default()).await.unwrap().total, 1);
}

#[tokio::test]
async fn digest_and_presence_are_data_plus_one_background_call() {
    let r = rig();
    let e = &r.engine;
    // 2026-03-05 is amy's birthday: she is waiting, with a channel to click through to.
    let d = e.digest().await.unwrap();
    assert_eq!(d.items.len(), 1);
    assert_eq!(d.items[0].activity, Activity::Birthday);
    assert_eq!(d.items[0].person.id.0, "amy");
    let p = e.presence().await.unwrap();
    assert_eq!(p, d);
    assert_eq!(r.spy.calls.lock().unwrap().as_slice(), ["presence"]);
    // Disabled people never wait.
    e.set_mode(Target::Person { id: id("amy") }, Mode::Disabled)
        .await
        .unwrap();
    assert!(e.digest().await.unwrap().items.is_empty());
}

#[tokio::test]
async fn settings_quota_and_voice_round_trip() {
    let r = rig();
    let e = &r.engine;
    let mut s = e.settings().await.unwrap();
    assert!(s.notifications && s.quiet_hours.is_some());
    s.user_name = Some("Ada".into());
    s.birthday = Some(MonthDay { month: 2, day: 29 });
    s.quiet_hours = None;
    s.notifications = false;
    e.update_settings(s.clone()).await.unwrap();
    assert_eq!(e.settings().await.unwrap(), s);
    assert!(!e.app_status().await.unwrap().needs_setup);
    assert!(!e.may_notify().unwrap());

    s.quiet_hours = Some(QuietHours {
        from: "25:00".into(),
        to: "07:00".into(),
    });
    assert_eq!(
        e.update_settings(s.clone()).await,
        Err(ApiError::Invalid {
            field: "quiet_hours".into()
        })
    );
    s.quiet_hours = Some(QuietHours {
        from: "12:00".into(),
        to: "13:00".into(),
    });
    s.notifications = true;
    e.update_settings(s).await.unwrap();
    assert!(!e.may_notify().unwrap(), "inside quiet hours at noon");

    e.set_tier(Tier::High).await.unwrap();
    let q = e.quota().await.unwrap();
    assert_eq!(q.tier, Tier::High);
    assert_eq!(q.band, QuotaBand::Open);

    e.set_voice(id("amy"), Some(" alloy ".into()))
        .await
        .unwrap();
    assert_eq!(
        e.person_page(id("amy")).await.unwrap().voice.as_deref(),
        Some("alloy")
    );
    e.set_voice(id("amy"), None).await.unwrap();
    assert_eq!(e.person_page(id("amy")).await.unwrap().voice, None);
}

#[tokio::test]
async fn avatars_are_fetched_once_and_degrade_offline() {
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    let net = Arc::new(Net(Some(png.clone()), Mutex::new(0)));
    let r = rig_with(net.clone());
    let a = r.engine.avatar(id("amy")).await.unwrap().unwrap();
    assert_eq!(std::fs::read(&a.path).unwrap(), png);
    assert!(a.path.contains("media"));
    assert_eq!(r.engine.avatar(id("amy")).await.unwrap().unwrap(), a);
    assert_eq!(*net.1.lock().unwrap(), 1, "the second call is a cache hit");
    assert_eq!(r.engine.avatar(id("nobody")).await, Err(ApiError::NotFound));

    // An error page served as 200 is not an avatar; no network is no avatar.
    let page = Arc::new(Net(Some(b"<html>".to_vec()), Mutex::new(0)));
    assert_eq!(rig_with(page).engine.avatar(id("amy")).await.unwrap(), None);
    assert_eq!(rig().engine.avatar(id("amy")).await.unwrap(), None);
}

#[tokio::test]
async fn search_in_library_form_filters_by_person() {
    let r = rig();
    let e = &r.engine;
    let all = e.search_corpus("the".into(), vec![]).await.unwrap();
    assert!(all.hits.len() >= 2);
    let kal = e
        .search_corpus("the".into(), vec![id("kal")])
        .await
        .unwrap();
    assert!(
        kal.hits
            .iter()
            .all(|h| h.persons.iter().any(|p| p.id.0 == "kal"))
    );
    let empty = e.search_corpus("  ".into(), vec![]).await.unwrap();
    assert!(empty.hits.is_empty());
}
