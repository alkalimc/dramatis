//! Group membership over a scripted endpoint: offers made inside a group, a person added
//! later reading the earlier lines, the groups a person is told about, and a group reused
//! for the same people.

mod common;

use api::endpoints::WireApi;
use api::views::ToolAction;
use common::*;
use serde_json::json;
use world::{ChannelId, channel};

fn tool_actions(e: &Env) -> Vec<(api::types::Author, ToolAction)> {
    e.sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            agent::Event::ToolCalled(t) => Some((t.author, t.action)),
            _ => None,
        })
        .collect()
}

fn group_count(e: &Env) -> i64 {
    e.agent.with_world(|w| {
        w.query_row(
            "SELECT count(*) FROM channel WHERE kind = 'group'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

#[tokio::test]
async fn offers_made_inside_a_group_never_name_people_already_in_it() {
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    // A find names one member and one outsider: only the outsider is invited, into this group.
    e.fake.push([
        call("ask_host", json!({"find": ["bo", "dee"]})),
        say("Dee could help."),
    ]);
    e.agent.send(g, "ann, who can help?", None).await.unwrap();
    let seen = tool_actions(&e);
    let invited: Vec<_> = seen
        .iter()
        .filter_map(|(_, a)| match a {
            ToolAction::InviteToGroup { channel, persons } => Some((
                channel.clone(),
                persons.iter().map(|x| x.id.0.clone()).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        invited,
        [(api::types::ChannelId(g.to_string()), vec!["dee".to_owned()])]
    );
    assert!(
        !seen
            .iter()
            .any(|(_, a)| matches!(a, ToolAction::FindPeople { .. }))
    );

    // Only members named: no offer at all.
    e.fake.push([
        call("ask_host", json!({"find": ["bo"]})),
        say("Bo is right here."),
    ]);
    e.agent.send(g, "ann, anyone else?", None).await.unwrap();
    assert!(tool_actions(&e).iter().all(|(_, a)| !matches!(
        a,
        ToolAction::InviteToGroup { .. } | ToolAction::FindPeople { .. }
    )));

    // A group of people who are all here already is not offered; one with a newcomer is.
    e.fake.push([
        call("ask_host", json!({"group": ["bo"]})),
        say("Nothing to arrange."),
    ]);
    e.agent.send(g, "ann, a group?", None).await.unwrap();
    assert!(
        tool_actions(&e)
            .iter()
            .all(|(_, a)| !matches!(a, ToolAction::CreateGroup { .. }))
    );
    e.fake.push([
        call("ask_host", json!({"group": ["bo", "dee"]})),
        say("A group of three."),
    ]);
    e.agent.send(g, "ann, a bigger group?", None).await.unwrap();
    assert!(
        tool_actions(&e)
            .iter()
            .any(|(_, a)| matches!(a, ToolAction::CreateGroup { channel: None, .. }))
    );
}

#[tokio::test]
async fn someone_added_to_a_group_reads_what_was_said_before() {
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Ann: the harbor is quiet tonight.")]);
    e.agent
        .send(g, "ann, how is the harbor?", None)
        .await
        .unwrap();
    // The user adds dee by hand: a service line for the user and one for the log, no call.
    let added = e
        .agent
        .with_world(|w| channel::add_members(w, g, &[p("dee")]).unwrap());
    assert_eq!(added, [p("dee")]);
    let calls = e.fake.bodies().len();
    e.agent.members_changed(g, &added, &[]).await.unwrap();
    assert_eq!(e.fake.bodies().len(), calls);
    let shown = e.agent.history(g, None, 10).unwrap();
    let last = shown.last().unwrap();
    assert!(last.service && last.text.contains("dee joins"), "{last:?}");

    // Dee's first turn carries the whole segment so far, and her persona.
    e.fake.push([say("Dee: then I will keep watch.")]);
    e.agent
        .send(g, "dee, will you keep watch?", None)
        .await
        .unwrap();
    let body = body_text(e.fake.bodies().last().unwrap());
    assert!(
        body.contains("how is the harbor?") && body.contains("the harbor is quiet tonight"),
        "{body}"
    );
    assert!(body.contains("You are dee."), "{body}");
    assert!(body.contains("dee joins the conversation."), "{body}");
    assert_eq!(segments(&e.agent, g).len(), 1);

    // Leaving is told the same way; what the person said stays in the history.
    let left = e
        .agent
        .with_world(|w| channel::remove_member(w, g, &p("dee")).unwrap());
    assert!(left);
    e.agent.members_changed(g, &[], &[p("dee")]).await.unwrap();
    let shown = e.agent.history(g, None, 20).unwrap();
    assert!(shown.last().unwrap().text.contains("dee leaves"));
    assert!(
        shown
            .iter()
            .any(|m| m.text.contains("then I will keep watch"))
    );
    assert!(log_bytes(&e.agent, g).contains("dee leaves the conversation."));
}

#[tokio::test]
async fn a_direct_chat_lists_the_live_groups_of_its_person_only() {
    let e = env(WireApi::Chat);
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo")], Some("plan a party"))
        .unwrap();
    let other = e.agent.create_group(&[p("ann"), p("dee")], None).unwrap();
    let archived = e
        .agent
        .create_group(&[p("ann"), p("bo"), p("dee")], None)
        .unwrap();
    e.agent.with_world(|w| {
        channel::set_name(w, other, Some("Night shift")).unwrap();
        channel::set_archived(w, archived, true).unwrap();
    });
    let ch = direct(&e, "ann");
    e.fake.push([say("Hello.")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    let bytes = log_bytes(&e.agent, ch);
    assert!(bytes.contains("also in these groups"), "{bytes}");
    assert!(bytes.contains("plan a party"), "{bytes}");
    assert!(bytes.contains("Night shift"), "{bytes}");
    // Only the two live, unarchived groups.
    assert_eq!(bytes.matches("\n- ").count(), 2, "{bytes}");

    // A group's own log lists none.
    e.fake.push([say("Hi all.")]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert!(!log_bytes(&e.agent, g).contains("also in these groups"));
}

#[tokio::test]
async fn the_hosts_group_for_the_same_people_is_reused() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    let args = json!({"members": ["ann", "dee"], "purpose": "plan a party"});
    e.fake
        .push([call("create_group", args.clone()), say("Done.")]);
    e.agent.send(host, "make a group", None).await.unwrap();
    assert_eq!(group_count(&e), 1);
    e.fake.push([call("create_group", args), say("Done.")]);
    e.agent.send(host, "again", None).await.unwrap();
    assert_eq!(group_count(&e), 1);
    let body = body_text(e.fake.bodies().last().unwrap());
    assert!(body.contains("already has exactly these people"), "{body}");
}
