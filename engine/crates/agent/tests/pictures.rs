//! Pictures the user attaches: how they travel on each wire, how they are shrunk, and what
//! happens when the endpoint will not take them. Over a scripted endpoint.

mod common;

use agent::Error;
use agent::request::Image;
use api::endpoints::{Endpoints, WireApi};
use api::events::{ActivityFailure, ActivityState, TurnActivity};
use common::*;

/// A 1x1 PNG.
const TINY: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn tiny() -> Image {
    Image {
        mime: "image/png".into(),
        data_base64: TINY.into(),
    }
}

/// A picture that compresses badly, as a PNG, at least `side` pixels wide.
fn noisy(side: u32) -> Image {
    let mut seed = 11u32;
    let img = image::RgbImage::from_fn(side, side, |_, _| {
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        };
        image::Rgb([next(), next(), next()])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    // Standard base64 by way of the decoder's own alphabet.
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for c in png.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            text.push(if i <= c.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    Image {
        mime: "image/png".into(),
        data_base64: text,
    }
}

fn endpoints_with(model_keys: &str) -> Endpoints {
    Endpoints::from_toml(&format!(
        "[[profile]]\nname = 'p'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'chat'\n\
         [[profile.model]]\nid = 'model-a'\nforced_tool = true\n{model_keys}\n\
         [roles]\nchat = {{ profile = 'p', model = 'model-a' }}\n"
    ))
    .unwrap()
}

#[tokio::test]
async fn a_picture_goes_out_in_each_protocols_shape() {
    for wire in WIRES {
        let e = env(wire);
        let ch = e.agent.open_direct(&p("ann")).unwrap();
        e.fake.push([say("I see it.")]);
        e.agent
            .send(ch, "what is this?", Some(tiny()))
            .await
            .unwrap();
        let body = String::from_utf8(e.fake.bodies().remove(0)).unwrap();
        let sent = match wire {
            WireApi::Chat => format!(r#""image_url":{{"url":"data:image/png;base64,{TINY}"}}"#),
            WireApi::Responses => format!(r#""image_url":"data:image/png;base64,{TINY}""#),
            WireApi::Messages => format!(r#""data":"{TINY}""#),
        };
        assert!(body.contains(&sent), "{wire:?}: {body}");
    }
}

#[tokio::test]
async fn a_large_picture_is_shrunk_under_the_models_cap() {
    let e = env_with(endpoints_with("max_image_bytes = 60000"), |_| {});
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    let big = noisy(1200);
    assert!(big.data_base64.len() > 128_000);
    e.fake.push([say("A lot of noise.")]);
    e.agent.send(ch, "this one?", Some(big)).await.unwrap();
    let bodies = e.fake.bodies();
    // The whole request, system text and all, stays under what such an endpoint takes.
    assert!(bodies[0].len() < 128_000, "{}", bodies[0].len());
    assert!(String::from_utf8_lossy(&bodies[0]).contains("data:image/jpeg;base64,"));
}

#[tokio::test]
async fn a_refused_picture_names_its_kind_and_does_not_poison_the_channel() {
    let e = env(WireApi::Chat);
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([Reply::Rejected(
        400,
        "image input needs an account cookie".into(),
    )]);
    let posted = e.agent.post(ch, "what is this?", Some(tiny())).unwrap();
    let err = e.agent.respond(posted).await.unwrap_err();
    assert!(
        matches!(&err, Error::ImageRejected { detail } if detail.contains("account cookie")),
        "{err:?}"
    );
    let failed: Vec<_> = e
        .sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            agent::Event::TurnActivity(TurnActivity { state, .. }) => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        failed.last(),
        Some(&ActivityState::Failed {
            reason: ActivityFailure::ImageUnsupported
        })
    );

    // The next, text-only message is answered: the stored picture is described, not sent.
    e.fake.push([say("Sure.")]);
    e.agent.send(ch, "never mind, hello", None).await.unwrap();
    let body = String::from_utf8(e.fake.bodies().pop().unwrap()).unwrap();
    assert!(
        !body.contains(TINY) && !body.contains("image_url"),
        "{body}"
    );
    assert!(body.contains("cannot show pictures"), "{body}");
}

#[tokio::test]
async fn an_error_without_a_picture_stays_an_ordinary_error() {
    let e = env(WireApi::Chat);
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([Reply::Rejected(400, "bad request".into())]);
    let posted = e.agent.post(ch, "hello", None).unwrap();
    let err = e.agent.respond(posted).await.unwrap_err();
    assert!(matches!(err, Error::Endpoint { .. }), "{err:?}");
}

#[tokio::test]
async fn a_model_marked_blind_is_never_sent_a_picture() {
    let e = env_with(endpoints_with("vision = false"), |_| {});
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([say("Tell me about it.")]);
    e.agent
        .send(ch, "what is this?", Some(tiny()))
        .await
        .unwrap();
    let body = String::from_utf8(e.fake.bodies().pop().unwrap()).unwrap();
    assert!(
        !body.contains(TINY) && !body.contains("image_url"),
        "{body}"
    );
    assert!(body.contains("cannot show pictures"), "{body}");
}

#[tokio::test]
async fn a_sent_picture_is_in_the_message_the_history_and_the_save() {
    let e = env(WireApi::Chat);
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    let posted = e.agent.post(ch, "look", Some(tiny())).unwrap();
    let url = format!("data:image/png;base64,{TINY}");

    let announced = e
        .sink
        .take()
        .into_iter()
        .find_map(|ev| match ev {
            agent::Event::MessageAdded(m) => Some(m.message),
            _ => None,
        })
        .unwrap();
    assert_eq!(announced.attachment.map(|a| a.path), Some(url.clone()));

    let shown = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(
        shown[0].attachment.as_ref().map(|a| a.path.clone()),
        Some(url)
    );
    assert!(!shown[0].picture_lost);
    assert_eq!(
        e.agent.with_world(|w| world::picture::usage(w).unwrap().0),
        1
    );

    // Taking the message back takes the picture with it.
    e.agent.rewind(ch, posted.message).unwrap();
    assert_eq!(
        e.agent.with_world(|w| world::picture::usage(w).unwrap().0),
        0
    );
}

#[tokio::test]
async fn one_picture_sent_twice_is_kept_until_both_messages_are_gone() {
    let e = env(WireApi::Chat);
    let ch = e.agent.open_direct(&p("ann")).unwrap();
    let first = e.agent.post(ch, "one", Some(tiny())).unwrap();
    e.clock.advance(60_000);
    let second = e.agent.post(ch, "two", Some(tiny())).unwrap();
    assert_eq!(
        e.agent.with_world(|w| world::picture::usage(w).unwrap().0),
        1
    );
    e.clock.advance(60_000);
    e.agent.rewind(ch, second.message).unwrap();
    assert_eq!(
        e.agent.with_world(|w| world::picture::usage(w).unwrap().0),
        1
    );
    e.agent.rewind(ch, first.message).unwrap();
    assert_eq!(
        e.agent.with_world(|w| world::picture::usage(w).unwrap().0),
        0
    );
}
