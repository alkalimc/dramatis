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
        .manifest("avatar_url_pattern", r#""https://img.example/{page}.png""#)
        .manifest(
            "player_avatar_url",
            r#""https://img.example/{md5_1}/player.png""#,
        );
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
        id: "u-story1-next",
        template: "talk",
        page: "Story 1",
        text: "Evening fell over camp",
        span: Some(("Story 1", 13, 13)),
        ..UnitSpec::default()
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
    /// Pretend a reply is being written.
    busy: std::sync::atomic::AtomicBool,
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
    async fn rewind(&self, c: ChannelId, m: MessageId) -> ApiResult<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("rewind {} {}", c.0, m.0));
        Ok("again".into())
    }
    async fn wrapup(&self, c: ChannelId) -> ApiResult<()> {
        self.calls.lock().unwrap().push(format!("wrapup {}", c.0));
        Ok(())
    }
    async fn presence(&self, _: bool) -> ApiResult<()> {
        self.calls.lock().unwrap().push("presence".into());
        Ok(())
    }
    async fn group_opened(&self, c: ChannelId, by: Option<PersonId>, p: String) -> ApiResult<()> {
        let by = by.map_or("host".to_string(), |b| b.0);
        self.calls
            .lock()
            .unwrap()
            .push(format!("group {} {by} {p}", c.0));
        Ok(())
    }
    async fn members_changed(
        &self,
        c: ChannelId,
        joined: Vec<PersonId>,
        left: Vec<PersonId>,
    ) -> ApiResult<()> {
        let names = |v: Vec<PersonId>| v.into_iter().map(|p| p.0).collect::<Vec<_>>().join(",");
        self.calls.lock().unwrap().push(format!(
            "members {} +{} -{}",
            c.0,
            names(joined),
            names(left)
        ));
        Ok(())
    }
    async fn found_opened(&self, c: ChannelId, by: Option<PersonId>, t: String) -> ApiResult<()> {
        let by = by.map_or("host".to_string(), |b| b.0);
        self.calls
            .lock()
            .unwrap()
            .push(format!("found {} {by} {t}", c.0));
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
    fn idle(&self) -> bool {
        !self.busy.load(Ordering::Relaxed)
    }
    fn switch_world(&self, path: &std::path::Path, key: Option<&str>) {
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let locked = if key.is_some() { " locked" } else { "" };
        self.calls
            .lock()
            .unwrap()
            .push(format!("world {name}{locked}"));
    }
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
    // A home with a save to open, as a first run leaves it after the user picks one.
    drop(world::World::open(home.save_path("default")).unwrap());
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
    assert!(matches!(s.corpus, CorpusState::Loaded { .. }));

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
async fn interface_language_is_the_homes_and_applies_at_once() {
    let r = rig();
    let e = &r.engine;
    assert_eq!(
        e.interface_language().await.unwrap(),
        InterfaceLanguage::Corpus
    );
    assert_eq!(e.wording().await.unwrap().entries["user.you"], "Doc");
    e.set_interface_language(InterfaceLanguage::English)
        .await
        .unwrap();
    // Built-in defaults take over; the corpus's names stay.
    let w = e.wording().await.unwrap();
    assert!(!w.entries.contains_key("user.you"));
    assert_eq!(w.entries["host.name"], "Host");
    assert_eq!(
        e.home.interface_language(),
        Some(InterfaceLanguage::English)
    );
    e.set_interface_language(InterfaceLanguage::Corpus)
        .await
        .unwrap();
    assert_eq!(e.wording().await.unwrap().entries["user.you"], "Doc");
}

#[test]
fn english_keeps_names_and_model_facing_wording() {
    let all: BTreeMap<String, String> = [
        ("host.name", "H"),
        ("org.name", "O"),
        ("user.title", "Mx {name}"),
        ("lang.corpus", "L"),
        ("gender.facet", "g"),
        ("harness.x", "m"),
        ("tool.search", "t"),
        ("nav.chats", "c"),
        ("host.role", "r"),
        ("locale", "z"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let kept: Vec<_> = english_entries(&all).into_keys().collect();
    assert_eq!(
        kept,
        [
            "gender.facet",
            "harness.x",
            "host.name",
            "lang.corpus",
            "org.name",
            "tool.search",
            "user.title"
        ]
    );
}

#[tokio::test]
async fn missing_and_refused_corpora_are_states_not_failures() {
    let home = temp_home();
    let e = Engine::open(home.clone(), None, Ports::default()).unwrap();
    assert_eq!(e.app_status().await.unwrap().corpus, CorpusState::Missing);
    assert_eq!(e.roster().await.unwrap(), Roster::default());
    assert_eq!(
        e.search_corpus("x".into(), vec![], vec![]).await,
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
    let found = e
        .search_corpus("lantern".into(), vec![], vec![])
        .await
        .unwrap();
    let p = hit(&found);
    assert_eq!(p.text, "Hello Doc, welcome to the lantern base");
    assert_eq!(p.header, "Amy speaks to Doc");
    assert_eq!(e.coldstart().await.unwrap()[0].reason, "Knows Doc well");

    name_user(e, "Ada").await;
    let p = hit(&e
        .search_corpus("lantern".into(), vec![], vec![])
        .await
        .unwrap());
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
    e.wrapup(ch.clone()).await.unwrap();
    assert_eq!(
        r.spy.calls.lock().unwrap().last().unwrap(),
        &format!("wrapup {}", ch.0)
    );
    // Taking a message back goes to the agent and returns its text.
    let text = e.rewind(ch.clone(), MessageId("7".into())).await.unwrap();
    assert_eq!(text, "again");
    assert_eq!(
        r.spy.calls.lock().unwrap().last().unwrap(),
        &format!("rewind {} 7", ch.0)
    );
}

#[tokio::test]
async fn no_chat_role_means_no_model_commands() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    assert_eq!(
        e.send_message(ch.clone(), "hi".into(), None).await,
        Err(ApiError::NoEndpoint)
    );
    assert_eq!(e.wrapup(ch).await, Err(ApiError::NoEndpoint));
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
    // 2026-03-05 is amy's birthday: she is waiting, with a channel to click through to,
    // because the user has talked to her.
    let ch = e.open_direct(id("amy")).await.unwrap();
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
async fn notifications_pass_when_on_and_outside_quiet_hours() {
    let r = rig();
    let e = &r.engine;
    // Fresh home at noon: the switch defaults on and the default quiet hours are overnight.
    assert!(e.may_notify().unwrap());

    let mut s = e.settings().await.unwrap();
    let mut set = |quiet: Option<(&str, &str)>, on: bool| {
        s.quiet_hours = quiet.map(|(from, to)| QuietHours {
            from: from.into(),
            to: to.into(),
        });
        s.notifications = on;
        s.clone()
    };
    // Quiet hours off entirely: only the switch matters.
    e.update_settings(set(None, true)).await.unwrap();
    assert!(e.may_notify().unwrap());
    e.update_settings(set(None, false)).await.unwrap();
    assert!(!e.may_notify().unwrap());
    // The window is [from, to): noon is outside one that ends at noon, inside one that
    // starts at noon, and inside one that spans midnight through the afternoon.
    e.update_settings(set(Some(("10:00", "12:00")), true))
        .await
        .unwrap();
    assert!(e.may_notify().unwrap());
    e.update_settings(set(Some(("12:00", "13:00")), true))
        .await
        .unwrap();
    assert!(!e.may_notify().unwrap());
    e.update_settings(set(Some(("22:00", "13:00")), true))
        .await
        .unwrap();
    assert!(!e.may_notify().unwrap());
    // Off beats a window that does not cover now.
    e.update_settings(set(Some(("22:00", "07:00")), false))
        .await
        .unwrap();
    assert!(!e.may_notify().unwrap());
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
    let all = e.search_corpus("the".into(), vec![], vec![]).await.unwrap();
    assert!(all.hits.len() >= 2);
    let kal = e
        .search_corpus("the".into(), vec![id("kal")], vec![])
        .await
        .unwrap();
    assert!(
        kal.hits
            .iter()
            .all(|h| h.persons.iter().any(|p| p.id.0 == "kal"))
    );
    let empty = e.search_corpus("  ".into(), vec![], vec![]).await.unwrap();
    assert!(empty.hits.is_empty());
}

#[tokio::test]
async fn library_search_filters_by_template_and_units_read_with_neighbours() {
    let r = rig();
    let e = &r.engine;
    assert_eq!(e.corpus_templates().await.unwrap(), ["card", "talk"]);
    let cards = e
        .search_corpus("the".into(), vec![], vec!["card".into()])
        .await
        .unwrap();
    assert!(!cards.hits.is_empty());
    assert!(cards.hits.iter().all(|h| h.template == "card"));

    let read = e.read_unit("u-amy-talk".into(), 1).await.unwrap();
    assert_eq!(read.unit.citation.chunk_id, "u-amy-talk");
    assert!(read.before.is_empty());
    assert_eq!(read.after.len(), 1);
    assert_eq!(read.after[0].text, "Evening fell over camp");
    // Corpus text reads with the user's name in it, here and everywhere else.
    assert!(read.unit.text.contains("Doc"));
    assert_eq!(e.read_unit("nope".into(), 1).await, Err(ApiError::NotFound));
    let back = e.read_unit("u-story1-next".into(), 1).await.unwrap();
    assert_eq!(back.before.len(), 1);
    assert!(back.after.is_empty());
}

#[tokio::test]
async fn saves_switch_keep_their_own_worlds_and_guard_the_open_one() {
    let r = rig();
    let e = &r.engine;
    // The first start has one save, the default, and it is open.
    let all = e.saves().await.unwrap();
    assert_eq!(
        (all.len(), all[0].name.as_str(), all[0].current),
        (1, "default", true)
    );

    name_user(e, "Ada").await;
    e.new_save("second".into()).await.unwrap();
    assert_eq!(e.new_save("Second".into()).await, Err(ApiError::Exists));

    // A reply in flight: nothing that closes the world.
    r.spy.busy.store(true, Ordering::Relaxed);
    assert_eq!(e.switch_save("second".into()).await, Err(ApiError::Busy));
    assert_eq!(
        e.rename_save("default".into(), "main".into()).await,
        Err(ApiError::Busy)
    );
    r.spy.busy.store(false, Ordering::Relaxed);

    e.switch_save("second".into()).await.unwrap();
    assert_eq!(e.home.current_save().as_deref(), Some("second"));
    assert_eq!(
        e.settings().await.unwrap().user_name,
        None,
        "an empty world"
    );
    assert_eq!(r.spy.calls.lock().unwrap().last().unwrap(), "world second");
    assert!(
        e.saves()
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "second" && s.current)
    );

    e.switch_save("default".into()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );

    // The open save cannot be deleted; another can.
    assert!(matches!(
        e.delete_save("default".into()).await,
        Err(ApiError::Invalid { .. })
    ));
    e.delete_save("second".into()).await.unwrap();
    assert_eq!(
        e.switch_save("second".into()).await,
        Err(ApiError::NotFound)
    );

    // Renaming the open save moves its files and the world with them.
    e.rename_save("default".into(), "main".into())
        .await
        .unwrap();
    assert_eq!(e.home.current_save().as_deref(), Some("main"));
    assert_eq!(r.spy.calls.lock().unwrap().last().unwrap(), "world main");
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );
    name_user(e, "Bea").await;

    // Export is a consistent copy; importing it gives a save with the same world.
    let out = e.home.root().join("backup.sqlite");
    e.export_save("main".into(), out.display().to_string())
        .await
        .unwrap();
    let name = e
        .import_save(out.display().to_string(), None, None)
        .await
        .unwrap();
    assert_eq!(name, "backup");
    e.copy_save("main".into(), "twin".into()).await.unwrap();
    e.switch_save("backup".into()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Bea")
    );
    e.switch_save("twin".into()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Bea")
    );
}

#[tokio::test]
async fn a_suggested_group_is_made_once_and_the_line_remembers_it() {
    let r = rig();
    let e = &r.engine;
    let direct = e.open_direct(id("amy")).await.unwrap();
    let person = |p: &str| PersonRef {
        id: id(p),
        name: p.into(),
        pronoun: None,
    };
    let offer = ToolAction::CreateGroup {
        members: vec![person("amy"), person("kal")],
        channel: None,
        purpose: Some("plan a party".into()),
    };
    let (mid, other) = {
        let w = e.w();
        let ch = convert::world_channel(&direct).unwrap();
        let calls = serde_json::to_value([ToolAction::Report, offer]).unwrap();
        let m = world::message::append(
            &w,
            ch,
            &world::Actor::Person(world::PersonId("amy".into())),
            "let us talk",
            Some(&calls),
            None,
            e.now().ms,
        )
        .unwrap();
        (convert::message_id(m), m)
    };
    let invalid = Err(ApiError::Invalid {
        field: "action".into(),
    });
    assert_eq!(e.open_offer(mid.clone(), 0).await, invalid);
    assert_eq!(e.open_offer(mid.clone(), 7).await, invalid);
    let first = e.open_offer(mid.clone(), 1).await.unwrap();
    assert_eq!(e.open_offer(mid.clone(), 1).await.unwrap(), first);
    let groups = |e: &Engine| {
        let w = e.w();
        world::channel::list(&w)
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == world::ChannelKind::Group)
            .count()
    };
    assert_eq!(groups(e), 1);
    // The purpose is the group's topic, and the group's first line is asked for once,
    // naming the person who suggested it.
    let made = world::channel::get(&e.w(), convert::world_channel(&first).unwrap()).unwrap();
    assert_eq!(made.topic.as_deref(), Some("plan a party"));
    // And its name too, so the title stays what the group was made for.
    assert_eq!(made.name.as_deref(), Some("plan a party"));
    assert_eq!(
        r.spy.calls.lock().unwrap().as_slice(),
        [format!("group {} amy plan a party", first.0)]
    );
    // The stored line now names the group, so every front end shows it as opened.
    let stored = world::message::get(&e.w(), other).unwrap();
    let lines: Vec<ToolAction> = serde_json::from_value(stored.tool_calls.unwrap()).unwrap();
    assert!(matches!(&lines[1], ToolAction::CreateGroup { channel: Some(c), .. } if *c == first));
    // A group deleted since is made again, not returned dead.
    e.delete_group(first.clone()).await.unwrap();
    let again = e.open_offer(mid.clone(), 1).await.unwrap();
    assert_ne!(again, first);
    assert_eq!(groups(e), 1);
}

/// A person's message with `lines` under it, in `ch`.
fn offer_message(e: &Engine, ch: &ChannelId, lines: &[ToolAction]) -> MessageId {
    let w = e.w();
    let calls = serde_json::to_value(lines).unwrap();
    let m = world::message::append(
        &w,
        convert::world_channel(ch).unwrap(),
        &world::Actor::Person(world::PersonId("amy".into())),
        "a thought",
        Some(&calls),
        None,
        e.now().ms,
    )
    .unwrap();
    convert::message_id(m)
}

fn person_ref(p: &str) -> PersonRef {
    PersonRef {
        id: id(p),
        name: p.into(),
        pronoun: None,
    }
}

#[tokio::test]
async fn members_join_and_leave_a_group_and_the_group_says_so() {
    let r = rig();
    let e = &r.engine;
    let calls = || r.spy.calls.lock().unwrap().clone();
    let group = e.create_group(vec![id("amy")], None).await.unwrap();
    let members = |e: &Engine| {
        world::channel::get(&e.w(), convert::world_channel(&group).unwrap())
            .unwrap()
            .persons()
    };
    // Only people who can answer; nothing changes on a refusal.
    assert_eq!(
        e.add_members(group.clone(), vec![id("kal"), id("zed")])
            .await,
        Err(ApiError::NoPersona { person: id("zed") })
    );
    assert_eq!(members(e).len(), 1);
    assert!(calls().is_empty());

    e.add_members(group.clone(), vec![id("kal"), id("amy")])
        .await
        .unwrap();
    assert_eq!(members(e).len(), 2);
    assert_eq!(calls(), [format!("members {} +kal -", group.0)]);
    // Again: nobody new, nothing said.
    e.add_members(group.clone(), vec![id("kal")]).await.unwrap();
    assert_eq!(calls().len(), 1);

    e.remove_member(group.clone(), id("kal")).await.unwrap();
    assert_eq!(members(e).len(), 1);
    assert_eq!(calls()[1], format!("members {} + -kal", group.0));
    // Not in it: nothing happens. The last person stays.
    e.remove_member(group.clone(), id("kal")).await.unwrap();
    assert_eq!(calls().len(), 2);
    assert_eq!(
        e.remove_member(group.clone(), id("amy")).await,
        Err(ApiError::Invalid {
            field: "request".into()
        })
    );
    assert_eq!(members(e).len(), 1);

    // A direct chat has no roster to edit, and a disabled person cannot be added.
    let direct = e.open_direct(id("amy")).await.unwrap();
    assert!(e.add_members(direct, vec![id("kal")]).await.is_err());
    e.set_mode(Target::Person { id: id("kal") }, Mode::Disabled)
        .await
        .unwrap();
    assert!(e.add_members(group, vec![id("kal")]).await.is_err());
}

#[tokio::test]
async fn the_host_joins_and_leaves_a_group_by_its_own_id_and_is_listed_there() {
    let r = rig();
    let e = &r.engine;
    let calls = || r.spy.calls.lock().unwrap().clone();
    let group = e.create_group(vec![id("amy")], None).await.unwrap();
    let listed = |e: &Engine| -> Vec<String> {
        let channels = e.channels_impl().unwrap();
        channels
            .iter()
            .find(|c| c.id == group)
            .map(|c| c.participants.iter().map(|p| p.id.0.clone()).collect())
            .unwrap_or_default()
    };
    // Written to nothing yet, a group is not listed; give it a line.
    world::message::append(
        &e.w(),
        convert::world_channel(&group).unwrap(),
        &world::Actor::User,
        "hi",
        None,
        None,
        1,
    )
    .unwrap();
    assert_eq!(listed(e), ["amy"]);
    e.add_members(group.clone(), vec![id(HOST_MEMBER)])
        .await
        .unwrap();
    assert_eq!(listed(e), ["amy", HOST_MEMBER]);
    assert_eq!(calls(), [format!("members {} +{HOST_MEMBER} -", group.0)]);
    // Again: nothing new. An invitation of the host works like one of a person.
    e.add_members(group.clone(), vec![id(HOST_MEMBER)])
        .await
        .unwrap();
    assert_eq!(calls().len(), 1);
    e.remove_member(group.clone(), id(HOST_MEMBER))
        .await
        .unwrap();
    assert_eq!(listed(e), ["amy"]);
    assert_eq!(calls()[1], format!("members {} + -{HOST_MEMBER}", group.0));
    let mid = offer_message(
        e,
        &group,
        &[ToolAction::InviteToGroup {
            channel: group.clone(),
            persons: vec![person_ref(HOST_MEMBER)],
        }],
    );
    assert_eq!(
        e.open_invite(mid, 0, id(HOST_MEMBER)).await,
        Ok(group.clone())
    );
    assert_eq!(listed(e), ["amy", HOST_MEMBER]);
    // Never into a direct chat.
    let direct = e.open_direct(id("amy")).await.unwrap();
    assert!(e.add_members(direct, vec![id(HOST_MEMBER)]).await.is_err());
}

#[tokio::test]
async fn an_invitation_adds_the_person_it_names_and_only_them() {
    let r = rig();
    let e = &r.engine;
    let group = e.create_group(vec![id("amy")], None).await.unwrap();
    let mid = offer_message(
        e,
        &group,
        &[ToolAction::InviteToGroup {
            channel: group.clone(),
            persons: vec![person_ref("kal")],
        }],
    );
    let invalid = Err(ApiError::Invalid {
        field: "action".into(),
    });
    assert_eq!(e.open_invite(mid.clone(), 1, id("kal")).await, invalid);
    assert_eq!(e.open_invite(mid.clone(), 0, id("amy")).await, invalid);
    assert_eq!(
        e.open_invite(mid.clone(), 0, id("kal")).await,
        Ok(group.clone())
    );
    // Taking it again changes nothing.
    assert_eq!(e.open_invite(mid, 0, id("kal")).await, Ok(group.clone()));
    let now = world::channel::get(&e.w(), convert::world_channel(&group).unwrap()).unwrap();
    assert_eq!(now.persons().len(), 2);
    assert_eq!(
        r.spy.calls.lock().unwrap().as_slice(),
        [format!("members {} +kal -", group.0)]
    );
}

#[tokio::test]
async fn a_suggested_group_reuses_a_live_one_with_the_same_people_and_purpose() {
    let r = rig();
    let e = &r.engine;
    let existing = e
        .create_group(vec![id("kal"), id("amy")], Some("plan a party".into()))
        .await
        .unwrap();
    let direct = e.open_direct(id("amy")).await.unwrap();
    let offer = |purpose: Option<&str>| ToolAction::CreateGroup {
        members: vec![person_ref("amy"), person_ref("kal")],
        channel: None,
        purpose: purpose.map(str::to_owned),
    };
    let groups = |e: &Engine| {
        world::channel::list(&e.w())
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == world::ChannelKind::Group)
            .count()
    };
    let name_of = |e: &Engine, c: &ChannelId| {
        world::channel::get(&e.w(), convert::world_channel(c).unwrap())
            .unwrap()
            .name
    };
    // The same purpose, in other case and punctuation: the same group, left as it was.
    let mid = offer_message(e, &direct, &[offer(Some("Plan a Party!"))]);
    assert_eq!(e.open_offer(mid, 0).await.unwrap(), existing);
    assert_eq!(groups(e), 1);
    assert_eq!(name_of(e, &existing), None);
    // The group already exists, so it gets no opening line.
    assert!(r.spy.calls.lock().unwrap().is_empty());

    // The same people for another purpose are another group, named by it.
    let mid = offer_message(e, &direct, &[offer(Some("plan a trip"))]);
    let trip = e.open_offer(mid, 0).await.unwrap();
    assert_ne!(trip, existing);
    assert_eq!(groups(e), 2);
    assert_eq!(name_of(e, &trip).as_deref(), Some("plan a trip"));

    // An archived group is not reused: the offer makes a new one.
    e.archive_channel(existing.clone(), true).await.unwrap();
    let mid = offer_message(e, &direct, &[offer(Some("plan a party"))]);
    let fresh = e.open_offer(mid, 0).await.unwrap();
    assert_ne!(fresh, existing);
    assert_eq!(groups(e), 3);
    assert!(
        world::channel::get(&e.w(), convert::world_channel(&existing).unwrap())
            .unwrap()
            .archived
    );
}

#[tokio::test]
async fn a_suggested_group_without_a_purpose_stays_unnamed() {
    let r = rig();
    let e = &r.engine;
    let direct = e.open_direct(id("amy")).await.unwrap();
    let mid = offer_message(
        e,
        &direct,
        &[ToolAction::CreateGroup {
            members: vec![person_ref("amy"), person_ref("kal")],
            channel: None,
            purpose: None,
        }],
    );
    let made = e.open_offer(mid.clone(), 0).await.unwrap();
    let ch = world::channel::get(&e.w(), convert::world_channel(&made).unwrap()).unwrap();
    assert_eq!((ch.name, ch.topic), (None, None));
    // The same people with no purpose are that group.
    let again = offer_message(
        e,
        &direct,
        &[ToolAction::CreateGroup {
            members: vec![person_ref("kal"), person_ref("amy")],
            channel: None,
            purpose: None,
        }],
    );
    assert_eq!(e.open_offer(again, 0).await.unwrap(), made);
}

#[tokio::test]
async fn a_found_person_is_opened_with_the_reason_they_were_found() {
    let r = rig();
    let e = &r.engine;
    let host = e.host_channel().await.unwrap();
    let person = |p: &str| PersonRef {
        id: id(p),
        name: p.into(),
        pronoun: None,
    };
    let calls = serde_json::to_value([ToolAction::FindPeople {
        topic: "forging".into(),
        persons: vec![person("amy")],
    }])
    .unwrap();
    let mid = {
        let w = e.w();
        let ch = convert::world_channel(&host).unwrap();
        let m = world::message::append(
            &w,
            ch,
            &world::Actor::Host,
            "Here is someone.",
            Some(&calls),
            None,
            e.now().ms,
        )
        .unwrap();
        convert::message_id(m)
    };
    let invalid = Err(ApiError::Invalid {
        field: "action".into(),
    });
    assert_eq!(e.open_found(mid.clone(), 3, id("amy")).await, invalid);
    // Only people the line offered.
    assert_eq!(e.open_found(mid.clone(), 0, id("kal")).await, invalid);
    let ch = e.open_found(mid, 0, id("amy")).await.unwrap();
    assert_eq!(ch, e.open_direct(id("amy")).await.unwrap());
    assert_eq!(
        r.spy.calls.lock().unwrap().as_slice(),
        [format!("found {} host forging", ch.0)]
    );
}

/// Base64 of `bytes` (tests only; the engine just decodes).
fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn png_part(tag: u8) -> ImagePart {
    ImagePart {
        mime: "image/png".into(),
        data_base64: b64(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10, tag]),
    }
}

#[tokio::test]
async fn the_user_avatar_defaults_to_the_player_portrait_and_can_be_replaced() {
    let png = b"\x89PNG\r\n\x1a\nplayer".to_vec();
    let net = Arc::new(Net(Some(png.clone()), Mutex::new(0)));
    let r = rig_with(net.clone());
    let e = &r.engine;

    let default = e.user_avatar().await.unwrap().unwrap();
    assert_eq!(std::fs::read(&default.path).unwrap(), png);
    assert!(default.path.contains("media"));
    e.user_avatar().await.unwrap();
    assert_eq!(*net.1.lock().unwrap(), 1, "cached like a person's avatar");

    let own = e.set_user_avatar(Some(png_part(1))).await.unwrap().unwrap();
    // Their own lives in the save, not in a file beside it.
    assert!(own.path.starts_with("data:image/png;base64,"));
    assert_eq!(std::fs::read_dir(e.home.uploads()).unwrap().count(), 0);
    assert_eq!(e.user_avatar().await.unwrap().unwrap(), own);
    // The same picture again reads back the same.
    assert_eq!(
        e.set_user_avatar(Some(png_part(1))).await.unwrap().unwrap(),
        own
    );

    assert_eq!(
        e.set_user_avatar(Some(ImagePart {
            mime: "image/png".into(),
            data_base64: b64(b"<html>"),
        }))
        .await,
        Err(ApiError::Invalid {
            field: "image".into()
        })
    );
    assert_eq!(
        e.user_avatar().await.unwrap().unwrap(),
        own,
        "refusal changes nothing"
    );

    assert_eq!(e.set_user_avatar(None).await.unwrap().unwrap(), default);
    // No portrait configured and none set: nothing.
    assert_eq!(rig().engine.user_avatar().await.unwrap().map(|_| ()), None);
}

#[tokio::test]
async fn channels_carry_a_name_an_avatar_and_an_archive_flag() {
    let r = rig();
    let e = &r.engine;
    let g = e
        .create_group(vec![id("amy"), id("kal")], None)
        .await
        .unwrap();
    let wg = convert::world_channel(&g).unwrap();
    {
        let w = e.w();
        world::message::append(&w, wg, &world::Actor::User, "hi", None, None, 1).unwrap();
    }
    let row = |e: &Engine| {
        e.channels_impl()
            .unwrap()
            .into_iter()
            .find(|c| c.id == g)
            .unwrap()
    };
    let fresh = row(e);
    assert_eq!(
        (fresh.name, fresh.avatar, fresh.archived),
        (None, None, false)
    );

    e.rename_channel(g.clone(), Some(" Night shift ".into()))
        .await
        .unwrap();
    let file = e
        .set_channel_avatar(g.clone(), Some(png_part(2)))
        .await
        .unwrap()
        .unwrap();
    e.archive_channel(g.clone(), true).await.unwrap();
    let c = row(e);
    assert_eq!(c.name.as_deref(), Some("Night shift"));
    assert_eq!(c.avatar, Some(file));
    assert!(c.archived);
    assert!(matches!(
        r.events.take().last(),
        Some(Event::WorldChanged(_))
    ));

    // The user writing does not bring it back; a person does.
    {
        let w = e.w();
        world::message::append(&w, wg, &world::Actor::User, "again", None, None, 2).unwrap();
    }
    assert!(row(e).archived);
    {
        let w = e.w();
        let amy = world::Actor::Person(world::PersonId("amy".into()));
        world::message::append(&w, wg, &amy, "here", None, None, 3).unwrap();
    }
    assert!(!row(e).archived);

    e.rename_channel(g.clone(), None).await.unwrap();
    assert_eq!(e.set_channel_avatar(g.clone(), None).await.unwrap(), None);
    let c = row(e);
    assert!(c.name.is_none() && c.avatar.is_none());

    let host = e.host_channel().await.unwrap();
    assert_eq!(
        e.archive_channel(host, true).await,
        Err(ApiError::Invalid {
            field: "channel".into()
        })
    );
    assert_eq!(
        e.set_channel_avatar(
            g,
            Some(ImagePart {
                mime: "image/png".into(),
                data_base64: "!!".into(),
            })
        )
        .await,
        Err(ApiError::Invalid {
            field: "image".into()
        })
    );
}

#[tokio::test]
async fn another_process_writing_the_world_is_noticed() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    let wc = convert::world_channel(&ch).unwrap();
    // The first look only records where things stand.
    assert_eq!(e.poll_external().unwrap(), 0);
    assert_eq!(e.poll_external().unwrap(), 0);
    r.events.take();

    // The terminal: its own connection to the same file.
    let other = world::World::open(e.home.save_path(&e.home.current_save().unwrap())).unwrap();
    let said = world::message::append(
        &other,
        wc,
        &world::Actor::User,
        "from the terminal",
        None,
        None,
        5,
    )
    .unwrap();
    let mine = world::message::append(
        &other,
        wc,
        &world::Actor::User,
        "from my agent",
        None,
        None,
        6,
    )
    .unwrap();
    // The agent's message was announced by this process already.
    let announced = e.history_impl(&ch, None, 10).unwrap();
    let mine_id = announced
        .iter()
        .find(|m| m.text == "from my agent")
        .unwrap()
        .id
        .clone();
    e.ports
        .seen
        .record(&Event::MessageAdded(crate::events::MessageAdded {
            message: announced.iter().find(|m| m.id == mine_id).unwrap().clone(),
        }));

    // While a reply is being written nothing is announced.
    r.spy.busy.store(true, Ordering::Relaxed);
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());
    r.spy.busy.store(false, Ordering::Relaxed);

    assert_eq!(e.poll_external().unwrap(), 1);
    let events = r.events.take();
    let added: Vec<_> = events
        .iter()
        .filter_map(|ev| match ev {
            Event::MessageAdded(m) => Some(m.message.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(added, ["from the terminal"]);
    assert!(events.iter().any(|ev| matches!(ev,
        Event::WorldChanged(w) if w.changes.contains(&WorldChange::Channel { id: ch.clone() }))));
    let _ = (said, mine);

    // Nothing new, nothing announced.
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());
}

/// Appends as the terminal would: a second connection to the open save's file.
fn terminal_says(e: &Engine, ch: &ChannelId, text: &str) {
    let other = world::World::open(e.home.save_path(&e.home.current_save().unwrap())).unwrap();
    let wc = convert::world_channel(ch).unwrap();
    world::message::append(&other, wc, &world::Actor::User, text, None, None, 5).unwrap();
}

fn added(events: Vec<Event>) -> Vec<String> {
    events
        .iter()
        .filter_map(|ev| match ev {
            Event::MessageAdded(m) => Some(m.message.text.clone()),
            _ => None,
        })
        .collect()
}

fn person_says(e: &Engine, ch: &ChannelId, text: &str) -> world::MessageId {
    let other = world::World::open(e.home.world().unwrap()).unwrap();
    let wc = convert::world_channel(ch).unwrap();
    let who = world::Actor::Person("amy".into());
    world::message::append(&other, wc, &who, text, None, None, 7).unwrap()
}

#[tokio::test]
async fn the_badge_is_the_messages_after_the_saved_read_cursor() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    let unread = |e: &Engine| {
        e.channels_impl()
            .unwrap()
            .into_iter()
            .find(|c| c.id == ch)
            .map_or(0, |c| c.unread)
    };
    terminal_says(e, &ch, "mine");
    assert_eq!(unread(e), 0, "the user's own words are never unread");
    person_says(e, &ch, "one");
    let last = person_says(e, &ch, "two");
    assert_eq!(unread(e), 2);
    e.mark_read(ch.clone(), convert::message_id(last))
        .await
        .unwrap();
    assert_eq!(unread(e), 0);
    // Written while the app was closed: exactly one.
    person_says(e, &ch, "three");
    assert_eq!(unread(e), 1);
}

#[tokio::test]
async fn messages_written_before_the_first_look_are_not_announced() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    person_says(e, &ch, "while closed");
    r.events.take();
    assert_eq!(e.poll_external().unwrap(), 0);
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(added(r.events.take()).is_empty());
}

#[tokio::test]
async fn this_processs_own_writes_are_never_announced_by_the_poll() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    assert_eq!(e.poll_external().unwrap(), 0);
    // The engine's own connection is the one that polls, so its commits leave
    // `data_version` alone.
    e.rename_channel(ch.clone(), Some("Night".into()))
        .await
        .unwrap();
    r.events.take();
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());

    // Another process's message arrives once.
    terminal_says(e, &ch, "theirs");
    assert_eq!(e.poll_external().unwrap(), 1);
    assert_eq!(added(r.events.take()), ["theirs"]);
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());
}

#[tokio::test]
async fn the_poll_follows_a_save_switch_to_the_new_file() {
    let r = rig();
    let e = &r.engine;
    let old = e.open_direct(id("amy")).await.unwrap();
    e.poll_external().unwrap();
    e.new_save("second".into()).await.unwrap();
    e.switch_save("second".into()).await.unwrap();
    let ch = e.open_direct(id("amy")).await.unwrap();
    // The first look at a new file only records where it stands.
    assert_eq!(e.poll_external().unwrap(), 0);
    r.events.take();

    terminal_says(e, &ch, "in the new save");
    assert_eq!(e.poll_external().unwrap(), 1);
    assert_eq!(added(r.events.take()), ["in the new save"]);

    // Writes to the save that was left are nobody's business now.
    let left = world::World::open(e.home.save_path("default")).unwrap();
    let wc = convert::world_channel(&old).unwrap();
    world::message::append(&left, wc, &world::Actor::User, "elsewhere", None, None, 9).unwrap();
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());
}

#[tokio::test]
async fn the_watcher_ends_when_its_engine_is_dropped() {
    let r = rig();
    let engine = Arc::new(r.engine);
    let task = tokio::spawn(Engine::watch_every(
        Arc::downgrade(&engine),
        std::time::Duration::from_millis(10),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!task.is_finished());
    drop(engine);
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .expect("the watcher stops")
        .unwrap();
}

#[tokio::test]
async fn export_while_another_connection_writes_is_a_consistent_copy() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    let wc = convert::world_channel(&ch).unwrap();
    let path = e.home.save_path(&e.home.current_save().unwrap());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = std::thread::spawn({
        let stop = stop.clone();
        move || {
            let w = world::World::open(path).unwrap();
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                world::message::append(
                    &w,
                    wc,
                    &world::Actor::User,
                    &format!("m{n}"),
                    None,
                    None,
                    n,
                )
                .unwrap();
            }
            n
        }
    });
    let dir = e.home.root().join("out");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..5 {
        let file = dir.join(format!("copy{i}.sqlite"));
        e.export_save("default".into(), file.display().to_string())
            .await
            .unwrap();
        // Each copy opens, passes the integrity check and holds whole messages only.
        let copy = rusqlite::Connection::open(&file).unwrap();
        let ok: String = copy
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ok, "ok");
        let (count, max): (i64, i64) = copy
            .query_row(
                "SELECT count(*), coalesce(max(id), 0) FROM message",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, max, "no message missing below the newest");
    }
    stop.store(true, Ordering::Relaxed);
    assert!(writer.join().unwrap() > 0);
}

#[tokio::test]
async fn a_group_picture_is_a_file_and_the_users_own_stays_in_the_save() {
    let r = rig();
    let e = &r.engine;
    let g = e
        .create_group(vec![id("amy"), id("kal")], None)
        .await
        .unwrap();
    let group = e
        .set_channel_avatar(g.clone(), Some(png_part(7)))
        .await
        .unwrap()
        .unwrap();
    let own = e.set_user_avatar(Some(png_part(7))).await.unwrap().unwrap();
    assert_ne!(group, own);
    assert_eq!(std::fs::read_dir(e.home.uploads()).unwrap().count(), 1);

    e.set_channel_avatar(g, None).await.unwrap();
    assert_eq!(e.user_avatar().await.unwrap(), Some(own));
}

#[tokio::test]
async fn the_bio_is_trimmed_bounded_and_rolls_the_logs_over_when_it_changes() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    r.spy.calls.lock().unwrap().clear();
    let mut s = e.settings().await.unwrap();
    assert_eq!(s.bio, None);
    s.bio = Some("  Runs the night shift.  ".into());
    Api::update_settings(e, s.clone()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().bio.as_deref(),
        Some("Runs the night shift.")
    );
    assert_eq!(*r.spy.calls.lock().unwrap(), ["rename"]);
    // The same bio again changes nothing and rolls nothing.
    Api::update_settings(e, e.settings().await.unwrap())
        .await
        .unwrap();
    assert_eq!(r.spy.calls.lock().unwrap().len(), 1);

    s.bio = Some("x".repeat(world::settings::BIO_MAX + 1));
    assert_eq!(
        Api::update_settings(e, s.clone()).await,
        Err(ApiError::Invalid {
            field: "bio".into()
        })
    );
    s.bio = Some("   ".into());
    Api::update_settings(e, s).await.unwrap();
    assert_eq!(e.settings().await.unwrap().bio, None);
    assert_eq!(r.spy.calls.lock().unwrap().len(), 2);
}

/// A second engine on the same home, as a restarted process would be.
fn reopen(r: &Rig, password: Option<&str>) -> Engine {
    let ports = Ports {
        agent: r.spy.clone(),
        clock: Arc::new(FixedClock(at("2026-03-05T12:00:00+00:00"))),
        ..Ports::default()
    };
    Engine::open_with_password(
        r.engine.home().clone(),
        Some(r._folio.path().to_path_buf()),
        ports,
        password.map(str::to_owned),
    )
    .unwrap()
}

#[tokio::test]
async fn encryption_turns_on_changes_and_off_and_a_locked_start_waits_for_the_password() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    e.set_user_avatar(Some(png_part(3))).await.unwrap();
    let file = e.home.save_path("default");
    assert!(!e.saves().await.unwrap()[0].encrypted);

    // Needs a password that is a password, and one that fits the file's state.
    assert!(matches!(
        e.set_password(None, None).await,
        Err(ApiError::Invalid { .. })
    ));
    assert!(matches!(
        e.set_password(Some("x".into()), Some("y".into())).await,
        Err(ApiError::Invalid { .. })
    ));
    assert!(matches!(
        e.set_password(None, Some(String::new())).await,
        Err(ApiError::Invalid { .. })
    ));
    r.spy.busy.store(true, Ordering::Relaxed);
    assert_eq!(
        e.set_password(None, Some("one".into())).await,
        Err(ApiError::Busy)
    );
    r.spy.busy.store(false, Ordering::Relaxed);

    e.set_password(None, Some("one".into())).await.unwrap();
    assert!(e.saves().await.unwrap()[0].encrypted);
    assert_eq!(
        r.spy.calls.lock().unwrap().last().unwrap(),
        "world default locked"
    );
    // The engine goes on over the encrypted file.
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );
    name_user(e, "Bea").await;

    // Nothing but the one file (and its lock); its words are not readable.
    let names: Vec<_> = std::fs::read_dir(e.home.saves())
        .unwrap()
        .map(|f| f.unwrap().file_name())
        .filter(|n| !n.to_string_lossy().ends_with(".lock"))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        !std::fs::read(&file)
            .unwrap()
            .windows(3)
            .any(|w| w == b"Bea")
    );

    // A restart without the password is locked; with a wrong one too; with it, open.
    let locked = reopen(&r, None);
    assert!(locked.app_status().await.unwrap().locked);
    let wrong = reopen(&r, Some("nope"));
    assert!(wrong.app_status().await.unwrap().locked);
    assert_eq!(
        wrong.unlock("default".into(), "nope".into()).await,
        Err(ApiError::WrongPassword)
    );
    assert!(
        wrong.app_status().await.unwrap().locked,
        "a wrong password changes nothing"
    );
    wrong.unlock("default".into(), "one".into()).await.unwrap();
    assert!(!wrong.app_status().await.unwrap().locked);
    assert_eq!(
        wrong.settings().await.unwrap().user_name.as_deref(),
        Some("Bea")
    );
    assert!(
        wrong
            .user_avatar()
            .await
            .unwrap()
            .unwrap()
            .path
            .starts_with("data:image/png")
    );
    assert!(!reopen(&r, Some("one")).app_status().await.unwrap().locked);

    // A locked engine does not turn encryption on, off or over.
    assert_eq!(
        locked.set_password(Some("one".into()), None).await,
        Err(ApiError::Locked)
    );

    // Change it: the old one stops working, and the old one is asked for. (Another
    // process with the save open would refuse the rewrite, so they leave first.)
    drop((locked, wrong));
    assert_eq!(
        e.set_password(Some("bad".into()), Some("two".into())).await,
        Err(ApiError::WrongPassword)
    );
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Bea")
    );
    e.set_password(Some("one".into()), Some("two".into()))
        .await
        .unwrap();
    assert!(reopen(&r, Some("one")).app_status().await.unwrap().locked);
    assert!(!reopen(&r, Some("two")).app_status().await.unwrap().locked);

    // Export stays encrypted under the same password; importing it asks for that.
    let out = e.home.root().join("backup.sqlite");
    e.export_save("default".into(), out.display().to_string())
        .await
        .unwrap();
    assert!(world::cipher::is_encrypted(&out));
    assert_eq!(
        e.import_save(out.display().to_string(), None, None).await,
        Err(ApiError::Locked)
    );
    assert_eq!(
        e.import_save(out.display().to_string(), None, Some("one".into()))
            .await,
        Err(ApiError::WrongPassword)
    );
    assert_eq!(
        e.import_save(out.display().to_string(), None, Some("two".into()))
            .await
            .unwrap(),
        "backup"
    );
    assert!(e.saves().await.unwrap().iter().all(|s| s.encrypted));
    // Another save needs its password to be switched to once this process forgets it.
    let again = reopen(&r, Some("two"));
    assert_eq!(
        again.switch_save("backup".into()).await,
        Err(ApiError::Locked)
    );
    again.unlock("backup".into(), "two".into()).await.unwrap();
    again.switch_save("backup".into()).await.unwrap();

    // Off again: plain, one file, the data intact.
    assert_eq!(
        e.set_password(Some("bad".into()), None).await,
        Err(ApiError::WrongPassword)
    );
    e.set_password(Some("two".into()), None).await.unwrap();
    assert!(!world::cipher::is_encrypted(&file));
    // (`again` switched the home's current save to the import.)
    e.home.set_current_save("default").unwrap();
    assert!(!reopen(&r, None).app_status().await.unwrap().locked);
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Bea")
    );
}

#[tokio::test]
async fn a_rename_that_cannot_finish_leaves_the_open_save_open_under_its_old_name() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;

    // The new name cannot be written for the next start.
    let current = e.home.root().join("current_save");
    let _ = std::fs::remove_file(&current);
    std::fs::create_dir(&current).unwrap();
    assert!(
        e.rename_save("default".into(), "main".into())
            .await
            .is_err()
    );
    std::fs::remove_dir(&current).unwrap();
    std::fs::write(&current, "default").unwrap();
    assert!(e.home.save_path("default").exists());
    assert!(!e.home.save_path("main").exists());
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );
    // The write above is in the live world, and new writes land in the file.
    name_user(e, "Bea").await;
    let on_disk = world::World::open(e.home.save_path("default")).unwrap();
    assert_eq!(
        world::settings::get::<String>(&on_disk, "user.name")
            .unwrap()
            .as_deref(),
        Some("Bea")
    );
    drop(on_disk);

    // The file will not reopen under the password this process holds: the files go back
    // and the engine says it is locked instead of writing to a blank world.
    e.passwords
        .lock()
        .unwrap()
        .insert("default".into(), "wrong".into());
    assert!(
        e.rename_save("default".into(), "main".into())
            .await
            .is_err()
    );
    assert!(e.home.save_path("default").exists());
    assert!(!e.home.save_path("main").exists());
    assert_eq!(e.home.current_save().as_deref(), Some("default"));
    assert!(e.locked.load(Ordering::Relaxed));
}

#[tokio::test]
async fn a_rewind_frees_ids_that_a_terminal_write_then_reuses() {
    let r = rig();
    let e = &r.engine;
    let ch = e.open_direct(id("amy")).await.unwrap();
    let wc = convert::world_channel(&ch).unwrap();
    let sent = {
        let w = e.w();
        world::message::append(&w, wc, &world::Actor::User, "mine", None, None, 4).unwrap()
    };
    let view = e.history_impl(&ch, None, 10).unwrap().pop().unwrap();
    e.ports
        .seen
        .record(&Event::MessageAdded(crate::events::MessageAdded {
            message: view.clone(),
        }));
    assert_eq!(e.poll_external().unwrap(), 0);
    r.events.take();

    // The user takes it back (the agent's rewind announces `Rewound` through the sink).
    {
        let w = e.w();
        world::rewind::rewind(&w, wc, sent).unwrap();
    }
    e.ports.seen.record(&Event::Rewound(crate::events::Rewound {
        channel: ch.clone(),
        from: view.id.clone(),
    }));
    // The terminal's next message gets the freed id.
    terminal_says(e, &ch, "from the terminal");
    assert_eq!(e.poll_external().unwrap(), 1);
}

// ---- first run: the save picker, log out, the corpus ----

#[tokio::test]
async fn a_home_with_no_save_shows_the_picker_and_a_switch_opens_and_records_one() {
    let home = temp_home();
    let e = Engine::open(home.clone(), None, Ports::default()).unwrap();
    let s = e.app_status().await.unwrap();
    assert_eq!((s.save, s.missing_save), (None, None));
    assert_eq!(s.saves_dir, home.saves().display().to_string());
    assert_eq!(s.home_dir, home.root().display().to_string());
    assert!(home.current_save().is_none());

    e.new_save("first".into()).await.unwrap();
    e.switch_save("first".into()).await.unwrap();
    assert_eq!(e.app_status().await.unwrap().save.as_deref(), Some("first"));
    assert_eq!(home.current_save().as_deref(), Some("first"));
    assert!(e.saves().await.unwrap()[0].current);
}

#[tokio::test]
async fn a_recorded_save_that_is_gone_shows_the_picker_and_says_which() {
    let home = temp_home();
    home.create().unwrap();
    home.set_current_save("renamed-elsewhere").unwrap();
    let e = Engine::open(home.clone(), None, Ports::default()).unwrap();
    let s = e.app_status().await.unwrap();
    assert_eq!(s.save, None);
    assert_eq!(s.missing_save.as_deref(), Some("renamed-elsewhere"));
    // Opening another one clears the note.
    e.new_save("b".into()).await.unwrap();
    e.switch_save("b".into()).await.unwrap();
    assert_eq!(e.app_status().await.unwrap().missing_save, None);
}

#[tokio::test]
async fn log_out_closes_the_save_clears_the_record_and_the_next_start_picks() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    e.log_out().await.unwrap();
    assert_eq!(e.app_status().await.unwrap().save, None);
    assert!(e.saves().await.unwrap().iter().all(|s| !s.current));
    assert_eq!(
        e.home.start_save(),
        crate::home::Start::Pick { missing: None }
    );
    // The terminal or a restarted app comes up at the picker too.
    assert_eq!(reopen(&r, None).app_status().await.unwrap().save, None);
    // Logging out twice is harmless; the save is untouched and opens again.
    e.log_out().await.unwrap();
    e.switch_save("default".into()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );
    assert_eq!(
        reopen(&r, None).app_status().await.unwrap().save.as_deref(),
        Some("default")
    );
}

#[tokio::test]
async fn log_out_forgets_the_password_and_waits_for_an_idle_engine() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    e.set_password(None, Some("pw".into())).await.unwrap();
    e.log_out().await.unwrap();
    assert_eq!(e.switch_save("default".into()).await, Err(ApiError::Locked));
    e.unlock("default".into(), "pw".into()).await.unwrap();
    e.switch_save("default".into()).await.unwrap();

    r.spy.busy.store(true, Ordering::Relaxed);
    assert_eq!(e.log_out().await, Err(ApiError::Busy));
    assert_eq!(
        e.app_status().await.unwrap().save.as_deref(),
        Some("default")
    );
}

#[tokio::test]
async fn two_clients_on_one_home_start_on_the_same_last_save() {
    let r = rig();
    let e = &r.engine;
    e.new_save("second".into()).await.unwrap();
    e.switch_save("second".into()).await.unwrap();
    // The other client keeps what it had open until it restarts (it was on `default`).
    let started_before = e.app_status().await.unwrap().save;
    assert_eq!(started_before.as_deref(), Some("second"));
    let terminal = reopen(&r, None);
    assert_eq!(
        terminal.app_status().await.unwrap().save.as_deref(),
        Some("second")
    );
    // And it writes the record on its own switch, which the app reads at its next start.
    terminal.switch_save("default".into()).await.unwrap();
    assert_eq!(
        reopen(&r, None).app_status().await.unwrap().save.as_deref(),
        Some("default")
    );
}

#[tokio::test]
async fn choosing_a_corpus_checks_it_remembers_it_and_asks_for_a_reopen() {
    let reopened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = reopened.clone();
    let home = temp_home();
    let ports = Ports {
        restart: Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }),
        ..Ports::default()
    };
    let e = Engine::open(home.clone(), None, ports).unwrap();
    assert_eq!(e.app_status().await.unwrap().corpus, CorpusState::Missing);

    assert_eq!(
        e.set_corpus("/no/such/file.folio".into()).await,
        Err(ApiError::NotFound)
    );
    let junk = home.root().join("junk.folio");
    std::fs::write(&junk, b"not a database").unwrap();
    assert_eq!(
        e.set_corpus(junk.display().to_string()).await,
        Err(ApiError::Invalid {
            field: "corpus".into()
        })
    );
    let bad = Builder::new().manifest("format_version", "99").finish();
    assert_eq!(
        e.set_corpus(bad.path().display().to_string()).await,
        Err(ApiError::Invalid {
            field: "corpus_version".into()
        })
    );
    assert_eq!(reopened.load(Ordering::Relaxed), 0);
    assert_eq!(home.chosen_corpus(), None);

    let good = fixture();
    // Quotes and spaces from a pasted path do not matter.
    e.set_corpus(format!(" \"{}\" ", good.path().display()))
        .await
        .unwrap();
    assert_eq!(reopened.load(Ordering::Relaxed), 1);
    assert_eq!(
        home.chosen_corpus(),
        Some(good.path().canonicalize().unwrap())
    );
    // Without a file dialog the window falls back to a typed path.
    assert_eq!(e.choose_corpus().await, Err(ApiError::NotImplemented));
}

fn setup(model: &str) -> EndpointSetup {
    EndpointSetup {
        name: "mine".into(),
        base_url: "https://api.example.invalid/v1".into(),
        wire_api: WireApi::Chat,
        model: model.into(),
        host_model: None,
        key: Some(" sk-test ".into()),
    }
}

#[tokio::test]
async fn the_first_run_form_writes_a_valid_endpoints_file_and_the_key_to_the_keychain() {
    let r = rig();
    let e = &r.engine;
    assert!(!e.home.endpoints().exists());
    assert!(!e.chat_configured());

    // Every field is checked and a refusal changes nothing.
    for (bad, field) in [
        (
            EndpointSetup {
                name: " ".into(),
                ..setup("m")
            },
            "name",
        ),
        (
            EndpointSetup {
                base_url: "api.example.invalid".into(),
                ..setup("m")
            },
            "base_url",
        ),
        (setup(""), "model"),
        (setup("two words"), "model"),
        (
            EndpointSetup {
                host_model: Some("a b".into()),
                ..setup("m")
            },
            "host_model",
        ),
    ] {
        assert_eq!(
            e.setup_endpoint(bad).await,
            Err(ApiError::Invalid {
                field: field.into()
            })
        );
    }
    assert!(!e.home.endpoints().exists());

    e.setup_endpoint(EndpointSetup {
        host_model: Some("model-h".into()),
        ..setup("model-c")
    })
    .await
    .unwrap();
    let text = std::fs::read_to_string(e.home.endpoints()).unwrap();
    assert!(!text.contains("sk-test"), "the key never reaches the file");
    let parsed = Endpoints::from_toml(&text).unwrap();
    parsed.validate().unwrap();
    let roles = parsed.roles.unwrap();
    assert_eq!(roles.chat.model, "model-c");
    assert_eq!(roles.host.unwrap().model, "model-h");
    let view = e.endpoints().await.unwrap();
    assert!(view.profiles[0].has_key);
    assert!(e.chat_configured());
    // A fresh process reads the same file back.
    assert_eq!(
        reopen(&r, None)
            .endpoints()
            .await
            .unwrap()
            .roles
            .unwrap()
            .chat
            .model,
        "model-c"
    );

    // Running it again updates the profile and keeps its key when none is given.
    e.setup_endpoint(EndpointSetup {
        key: None,
        ..setup("model-d")
    })
    .await
    .unwrap();
    let view = e.endpoints().await.unwrap();
    assert_eq!(view.profiles.len(), 1);
    assert!(view.profiles[0].has_key);
    assert_eq!(view.roles.unwrap().host, None);
}

#[tokio::test]
async fn history_carries_the_picture_and_marks_a_lost_one() {
    let r = rig();
    let e = &r.engine;
    let direct = e.open_direct(id("amy")).await.unwrap();
    let ch = convert::world_channel(&direct).unwrap();
    let dir = e.home.root().join("old");
    std::fs::create_dir_all(&dir).unwrap();
    let kept = dir.join("kept.png");
    std::fs::write(&kept, b"\x89PNG\r\n\x1a\nold").unwrap();
    {
        let w = e.w();
        let hash = world::picture::put(&w, "image/png", b"\x89PNG\r\n\x1a\nnew").unwrap();
        let say = |text: &str, meta: serde_json::Value, at: i64| {
            world::message::append(&w, ch, &world::Actor::User, text, None, Some(&meta), at)
                .unwrap();
        };
        say("stored", serde_json::json!({ "picture": hash }), 1);
        // The blob is gone from the save.
        say(
            "blob lost",
            serde_json::json!({ "picture": "0".repeat(64) }),
            2,
        );
        // An older build left files: one still there, one not, one not a picture.
        say("file kept", serde_json::json!({ "file": kept }), 3);
        say(
            "file gone",
            serde_json::json!({ "file": dir.join("none.png") }),
            4,
        );
        std::fs::write(dir.join("page.html"), b"<html>").unwrap();
        say(
            "file junk",
            serde_json::json!({ "file": dir.join("page.html") }),
            5,
        );
    }
    let shown = |m: &Message| m.attachment.as_ref().map(|a| a.path.clone());
    let msgs = e.history(direct.clone(), None, 10).await.unwrap();
    let by = |t: &str| msgs.iter().find(|m| m.text == t).unwrap();
    assert!(
        shown(by("stored"))
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert!(!by("stored").picture_lost);
    assert_eq!(
        (shown(by("blob lost")), by("blob lost").picture_lost),
        (None, true)
    );
    assert!(shown(by("file kept")).is_some() && !by("file kept").picture_lost);
    for t in ["file gone", "file junk"] {
        assert_eq!((shown(by(t)), by(t).picture_lost), (None, true), "{t}");
    }

    // The file's bytes now live in the save: removing the file changes nothing.
    std::fs::remove_file(&kept).unwrap();
    let again = e.history(direct, None, 10).await.unwrap();
    assert_eq!(shown(&again[2]), shown(by("file kept")));
}

#[tokio::test]
async fn storage_usage_counts_each_kind_and_clearing_touches_only_caches() {
    let r = rig();
    let e = &r.engine;
    let home = e.home.clone();
    std::fs::write(home.uploads().join("group.png"), [0u8; 4096]).unwrap();
    std::fs::write(home.root().join("pack.folio"), [0u8; 9216]).unwrap();
    e.media.put("avatar", "a", "png", &[0; 1024]).unwrap();
    e.media.put("avatar", "b", "png", &[0; 1024]).unwrap();
    e.media.put("tts", "c", "mp3", &[0; 3000]).unwrap();
    {
        let w = e.w();
        world::picture::put(&w, "image/png", &[1, 2, 3]).unwrap();
    }

    let of = |u: &StorageUsage, k: StorageKind| {
        u.categories.iter().find(|c| c.kind == k).unwrap().clone()
    };
    let u = e.storage_usage().await.unwrap();
    assert_eq!(
        (
            of(&u, StorageKind::Avatars).kb,
            of(&u, StorageKind::Avatars).files
        ),
        (2, 2)
    );
    assert_eq!(of(&u, StorageKind::Audio).kb, 3);
    assert_eq!(of(&u, StorageKind::Uploads).kb, 4);
    assert_eq!(of(&u, StorageKind::Corpus).kb, 9);
    assert!(!of(&u, StorageKind::Corpus).clearable && !of(&u, StorageKind::Saves).clearable);
    assert_eq!((u.pictures, u.picture_kb), (1, 1));
    let save_file = std::fs::metadata(home.world().unwrap()).unwrap().len();
    assert_eq!(u.saves.len(), 1);
    assert!(u.saves[0].current && u64::from(u.saves[0].kb) * 1024 >= save_file);
    assert_eq!(u.total_kb, u.categories.iter().map(|c| c.kb).sum::<u32>());
    assert_eq!(
        u64::from(u.cache_limit_kb),
        e.params.media.cache_bytes().div_ceil(1024)
    );

    // A kind that is not a cache refuses the whole request.
    assert_eq!(
        e.clear_storage(vec![StorageKind::Audio, StorageKind::Saves])
            .await,
        Err(ApiError::Invalid {
            field: "categories".into()
        })
    );
    assert_eq!(
        of(&e.storage_usage().await.unwrap(), StorageKind::Audio).kb,
        3
    );

    let after = e
        .clear_storage(vec![StorageKind::Avatars, StorageKind::Audio])
        .await
        .unwrap();
    assert_eq!(
        of(&after, StorageKind::Avatars).kb + of(&after, StorageKind::Audio).kb,
        0
    );
    assert_eq!(of(&after, StorageKind::Uploads).kb, 4);
    assert_eq!(of(&after, StorageKind::Corpus).kb, 9);
    assert_eq!(after.pictures, 1, "pictures inside the save stay");
    assert!(home.world().unwrap().is_file() && home.root().join("pack.folio").is_file());
}

#[tokio::test]
async fn rewriting_a_save_is_refused_while_another_process_has_it_open() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    let file = e.home.save_path("default");
    let other = reopen(&r, None);

    // Encrypting, re-keying and decrypting replace the file; so do rename and delete.
    assert_eq!(
        e.set_password(None, Some("one".into())).await,
        Err(ApiError::OpenElsewhere)
    );
    assert!(!world::cipher::is_encrypted(&file));
    assert_eq!(
        e.rename_save("default".into(), "moved".into()).await,
        Err(ApiError::OpenElsewhere)
    );
    assert!(file.is_file());
    crate::saves::create(&e.home, "spare").unwrap();
    let spare = other.save.lock().unwrap().clone();
    assert_eq!(spare, "default");
    other.switch_save("spare".into()).await.unwrap();
    assert_eq!(
        e.delete_save("spare".into()).await,
        Err(ApiError::OpenElsewhere)
    );
    // The refusal leaves this process's own hold and detection as they were.
    assert!(!e.check_open_file());
    // Once the other has left the save, the step goes through.
    e.set_password(None, Some("one".into())).await.unwrap();
    assert!(world::cipher::is_encrypted(&file));
    assert!(!e.check_open_file());
    // And now the other cannot take the save back while it is being rewritten, but can
    // after: the shared hold is back once the step ends.
    assert_eq!(
        e.set_password(Some("one".into()), Some("two".into())).await,
        Ok(())
    );
    other.unlock("default".into(), "two".into()).await.unwrap();
    other.switch_save("default".into()).await.unwrap();
    assert_eq!(
        e.set_password(Some("two".into()), None).await,
        Err(ApiError::OpenElsewhere)
    );
    drop(other);
    e.set_password(Some("two".into()), None).await.unwrap();
    assert!(!world::cipher::is_encrypted(&file));
    e.delete_save("spare".into()).await.unwrap();
    assert!(!e.home.save_path("spare").exists());
    assert!(!crate::lock::lock_path(&e.home.save_path("spare")).exists());
}

#[tokio::test]
async fn a_save_replaced_by_another_process_is_noticed_and_never_written_again() {
    let r = rig();
    let e = &r.engine;
    name_user(e, "Ada").await;
    let file = e.home.save_path("default");
    assert!(!e.check_open_file());
    assert!(!e.app_status().await.unwrap().changed_elsewhere);

    // A process that ignores the lock encrypts the save by swapping a new file in.
    let part = e.home.saves().join("swap.part");
    std::fs::copy(&file, &part).unwrap();
    world::cipher::encrypt(&part, "secret").unwrap();
    std::fs::rename(&part, &file).unwrap();
    r.events.take();

    assert_eq!(e.poll_external().unwrap(), 0);
    let status = e.app_status().await.unwrap();
    assert!(status.changed_elsewhere && !status.locked);
    assert!(
        r.events.take().iter().any(
            |ev| matches!(ev, Event::WorldChanged(w) if w.changes.contains(&WorldChange::Save))
        )
    );
    // Announced once, and what is written now does not reach the file.
    assert_eq!(e.poll_external().unwrap(), 0);
    assert!(r.events.take().is_empty());
    let before = std::fs::read(&file).unwrap();
    let _ = e
        .update_settings(Settings {
            user_name: Some("Eve".into()),
            ..e.settings().await.unwrap()
        })
        .await;
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(
        e.set_password(None, Some("x".into())).await,
        Err(ApiError::Locked)
    );

    // Reopening finds it encrypted and asks for the password; the lock is held again.
    e.switch_save("default".into()).await.unwrap();
    let status = e.app_status().await.unwrap();
    assert!(status.locked && !status.changed_elsewhere);
    e.unlock("default".into(), "secret".into()).await.unwrap();
    assert_eq!(
        e.settings().await.unwrap().user_name.as_deref(),
        Some("Ada")
    );
    assert_eq!(
        reopen(&r, Some("secret"))
            .rename_save("default".into(), "x".into())
            .await,
        Err(ApiError::OpenElsewhere)
    );
}

#[tokio::test]
async fn a_renamed_or_deleted_save_is_noticed_too() {
    let r = rig();
    let e = &r.engine;
    let file = e.home.save_path("default");
    std::fs::rename(&file, e.home.save_path("elsewhere")).unwrap();
    assert!(e.check_open_file());
    // Gone: reopening says so, and the other saves stay available.
    assert_eq!(
        e.switch_save("default".into()).await,
        Err(ApiError::NotFound)
    );
    crate::saves::create(&e.home, "spare").unwrap();
    e.switch_save("spare".into()).await.unwrap();
    assert!(!e.app_status().await.unwrap().changed_elsewhere);
    assert!(!e.check_open_file());
    std::fs::remove_file(e.home.save_path("spare")).unwrap();
    assert!(e.check_open_file());
}

#[tokio::test]
async fn only_plain_web_addresses_go_to_the_browser() {
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = opened.clone();
    let folio = fixture();
    let home = temp_home();
    home.create().unwrap();
    drop(world::World::open(home.save_path("default")).unwrap());
    let ports = Ports {
        open_url: Some(Arc::new(move |u: &str| seen.lock().unwrap().push(u.into()))),
        ..Ports::default()
    };
    let e = Engine::open(home, Some(folio.path().to_path_buf()), ports).unwrap();
    e.open_external("https://wiki.example/Story_1".into())
        .await
        .unwrap();
    for bad in [
        "file:///etc/passwd",
        "-a Calculator",
        "javascript:alert(1)",
        "https://x.example/a b",
        "",
    ] {
        assert!(
            matches!(
                e.open_external(bad.into()).await,
                Err(ApiError::Invalid { .. })
            ),
            "{bad}"
        );
    }
    assert_eq!(*opened.lock().unwrap(), ["https://wiki.example/Story_1"]);
    // No browser wired (the terminal, tests): nothing to open with.
    let r = rig();
    assert_eq!(
        r.engine
            .open_external("https://wiki.example/x".into())
            .await,
        Err(ApiError::NotImplemented)
    );
}

fn release(tag: &str) -> Arc<Net> {
    let body = format!(
        r#"{{"tag_name":"{tag}","html_url":"https://github.com/alkalimc/dramatis/releases/tag/{tag}"}}"#
    );
    Arc::new(Net(Some(body.into_bytes()), Mutex::new(0)))
}

#[tokio::test]
async fn app_info_comes_from_the_build() {
    let info = rig().engine.app_info().await.unwrap();
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.author, "alkali");
    assert_eq!(
        info.author_url.as_deref(),
        Some("https://github.com/alkalimc")
    );
    assert_eq!(info.repository, "https://github.com/alkalimc/dramatis");
    assert_eq!(info.license, "Apache-2.0");
}

#[tokio::test]
async fn the_corpus_credit_comes_from_its_manifest_and_only_links_https() {
    let open = |credits: Option<&str>| {
        let mut b = Builder::new();
        if let Some(c) = credits {
            b = b.manifest("credits", c);
        }
        let folio = b.finish();
        let home = temp_home();
        home.create().unwrap();
        drop(world::World::open(home.save_path("default")).unwrap());
        let e = Engine::open(home, Some(folio.path().to_path_buf()), Ports::default()).unwrap();
        (e, folio)
    };
    let (e, _f) = open(Some(
        r#"{"by":"someone","url":"https://example.org/me","project":"https://example.org/corpus",
            "licence":"CC BY-NC-SA 4.0"}"#,
    ));
    let c = e.app_info().await.unwrap().corpus_credits.unwrap();
    assert_eq!(c.by.as_deref(), Some("someone"));
    assert_eq!(c.url.as_deref(), Some("https://example.org/me"));
    assert_eq!(c.project.as_deref(), Some("https://example.org/corpus"));
    assert_eq!(c.licence.as_deref(), Some("CC BY-NC-SA 4.0"));
    // The app's own repository is not repeated as the corpus's project.
    let (e, _f) = open(Some(
        r#"{"by":"someone","project":"https://github.com/alkalimc/dramatis/"}"#,
    ));
    let c = e.app_info().await.unwrap().corpus_credits.unwrap();
    assert_eq!(c.project, None);
    // A link that is not https is dropped, the rest stays.
    let (e, _f) = open(Some(r#"{"by":"someone","url":"http://example.org"}"#));
    let c = e.app_info().await.unwrap().corpus_credits.unwrap();
    assert_eq!((c.by.as_deref(), c.url), (Some("someone"), None));
    // Absent, empty, or the wrong shape: no credit, and the corpus still opens.
    for credits in [
        None,
        Some("{}"),
        Some(r#"{"by":"  "}"#),
        Some("[1]"),
        Some(r#"{"by":3}"#),
    ] {
        let (e, _f) = open(credits);
        assert_eq!(
            e.app_info().await.unwrap().corpus_credits,
            None,
            "{credits:?}"
        );
        assert!(matches!(
            e.app_status().await.unwrap().corpus,
            CorpusState::Loaded { .. }
        ));
    }
}

#[tokio::test]
async fn the_update_check_asks_at_most_once_a_day() {
    let net = release("v99.0.0");
    let r = rig_with(net.clone());
    let e = &r.engine;
    let none = e.update_status().await.unwrap();
    assert!(!none.newer && none.latest.is_none() && none.auto);

    let s = e.check_update(false).await.unwrap();
    assert_eq!(s.latest.as_deref(), Some("99.0.0"));
    assert!(s.newer && s.checked_at.is_some());
    assert_eq!(
        s.url.as_deref(),
        Some("https://github.com/alkalimc/dramatis/releases/tag/v99.0.0")
    );
    // The result is kept: the same status comes back without asking again.
    assert_eq!(e.update_status().await.unwrap(), s);
    assert_eq!(e.check_update(false).await.unwrap(), s);
    assert_eq!(*net.1.lock().unwrap(), 1);
    // "Check now" always asks.
    e.check_update(true).await.unwrap();
    assert_eq!(*net.1.lock().unwrap(), 2);
    // A result older than a day is asked for again.
    let mut old = e.home.update_cache().unwrap();
    old.at_ms -= crate::update::INTERVAL_MS;
    e.home.set_update_cache(&old).unwrap();
    e.check_update(false).await.unwrap();
    assert_eq!(*net.1.lock().unwrap(), 3);
}

#[tokio::test]
async fn the_off_switch_stops_automatic_checks_only() {
    let net = release("v99.0.0");
    let r = rig_with(net.clone());
    let e = &r.engine;
    e.set_auto_update_check(false).await.unwrap();
    assert!(!e.home.auto_update_check());
    let s = e.check_update(false).await.unwrap();
    assert!(!s.auto && s.latest.is_none());
    assert_eq!(*net.1.lock().unwrap(), 0);
    assert!(e.check_update(true).await.unwrap().newer);
    assert_eq!(*net.1.lock().unwrap(), 1);
    e.set_auto_update_check(true).await.unwrap();
    assert!(e.update_status().await.unwrap().auto);
}

#[tokio::test]
async fn an_update_is_not_newer_when_it_is_the_running_version_and_a_failure_keeps_the_last_result()
{
    let same = release(&format!("v{}", env!("CARGO_PKG_VERSION")));
    let r = rig_with(same);
    let s = r.engine.check_update(true).await.unwrap();
    assert!(!s.newer && s.latest.as_deref() == Some(env!("CARGO_PKG_VERSION")));

    // Offline, a rate-limited answer or a reply that is not a release: an error, and the
    // stored result stays.
    for net in [
        Arc::new(Net(None, Mutex::new(0))),
        Arc::new(Net(
            Some(b"{\"message\":\"rate limit\"}".to_vec()),
            Mutex::new(0),
        )),
    ] {
        let failing = rig_with(net);
        failing
            .engine
            .home
            .set_update_cache(&r.engine.home.update_cache().unwrap())
            .unwrap();
        let before = failing.engine.update_status().await.unwrap();
        assert!(matches!(
            failing.engine.check_update(true).await,
            Err(ApiError::Endpoint { .. })
        ));
        assert_eq!(failing.engine.update_status().await.unwrap(), before);
    }
}
