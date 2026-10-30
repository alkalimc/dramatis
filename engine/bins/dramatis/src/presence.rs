//! The decisions behind the Dock icon and the system notifications, kept apart from the
//! Tauri calls so they can be tested: whether the app shows in the Dock, and which
//! arrivals become a notification and what it says.

use std::collections::VecDeque;

use api::types::Author;

/// Whether the Dock shows the app: only while the main window is open. Hidden, the app
/// lives in the menu bar alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dock {
    Shown,
    Hidden,
}

pub fn dock_for(window_visible: bool) -> Dock {
    if window_visible {
        Dock::Shown
    } else {
        Dock::Hidden
    }
}

/// A notification is for someone who is not looking: the window is hidden or another
/// app has the focus.
pub fn unattended(window_visible: bool, window_focused: bool) -> bool {
    !(window_visible && window_focused)
}

/// Whether persons may come to the user on the scheduled step: with the window shown, or
/// hidden while a notification could still reach the user. Hidden with notifications off or
/// in quiet hours nobody would learn of an opening, so no call is spent on one.
pub fn reachable(window_visible: bool, may_notify: bool) -> bool {
    window_visible || may_notify
}

/// What a notification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    pub title: String,
    pub body: String,
}

/// How much of a line the notification shows.
const PREVIEW: usize = 140;

/// Ids of the messages already notified, so a message announced twice (an event and the
/// external watcher, a replay) is notified once.
#[derive(Debug, Default)]
pub struct Recent(VecDeque<String>);

impl Recent {
    const KEEP: usize = 256;

    /// Remember `id`; `false` when it was already there.
    pub fn first(&mut self, id: &str) -> bool {
        if self.0.iter().any(|x| x == id) {
            return false;
        }
        if self.0.len() == Self::KEEP {
            self.0.pop_front();
        }
        self.0.push_back(id.to_owned());
        true
    }
}

/// Who the title names, and how the text is cleaned.
pub struct Style<'a> {
    pub host_name: &'a str,
    pub scene: (&'a str, &'a str),
}

/// The notification for a stored message, or `None` when it is the user's own, was
/// already notified, or has nothing to show. `display` gives a person's name.
pub fn arrival(
    author: &Author,
    id: &str,
    text: &str,
    style: &Style<'_>,
    display: impl Fn(&str) -> String,
    recent: &mut Recent,
) -> Option<Arrival> {
    let title = match author {
        Author::User => return None,
        Author::Host => style.host_name.to_owned(),
        Author::Person { id } => display(&id.0),
    };
    let body = preview(text, style.scene);
    if body.is_empty() || !recent.first(id) {
        return None;
    }
    Some(Arrival { title, body })
}

/// The first spoken line without citation marks, cut to a short length. A message made
/// only of scene description shows that description instead.
pub fn preview(text: &str, scene: (&str, &str)) -> String {
    let clean = api::cites::strip(text);
    let mut speech = None;
    let mut action = None;
    for line in clean.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match api::cites::scene_inner(line, scene.0, scene.1) {
            Some(inner) => action = action.or(Some(inner)),
            None => {
                speech = Some(line);
                break;
            }
        }
    }
    let line = speech.or(action).unwrap_or_default();
    if line.chars().count() <= PREVIEW {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(PREVIEW - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// The tray's text next to the icon: the number of arrivals nobody has looked at yet.
pub fn badge(unread: u32) -> Option<String> {
    (unread > 0).then(|| unread.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::types::PersonId;

    fn style() -> Style<'static> {
        Style {
            host_name: "Host",
            scene: ("(", ")"),
        }
    }

    fn person(id: &str) -> Author {
        Author::Person {
            id: PersonId(id.into()),
        }
    }

    fn name(id: &str) -> String {
        format!("Name of {id}")
    }

    #[test]
    fn the_dock_follows_the_window() {
        assert_eq!(dock_for(true), Dock::Shown);
        assert_eq!(dock_for(false), Dock::Hidden);
    }

    #[test]
    fn a_hidden_window_is_reachable_only_through_notifications() {
        assert!(reachable(true, false));
        assert!(reachable(false, true));
        assert!(!reachable(false, false));
    }

    #[test]
    fn only_a_window_in_front_is_attended() {
        assert!(!unattended(true, true));
        assert!(unattended(true, false));
        assert!(unattended(false, false));
        assert!(unattended(false, true));
    }

    #[test]
    fn a_person_titles_with_their_name_and_the_host_with_its_own() {
        let mut r = Recent::default();
        let a = arrival(&person("p1"), "m1", "Hello", &style(), name, &mut r).unwrap();
        assert_eq!((a.title.as_str(), a.body.as_str()), ("Name of p1", "Hello"));
        let h = arrival(&Author::Host, "m2", "Due today", &style(), name, &mut r).unwrap();
        assert_eq!(h.title, "Host");
    }

    #[test]
    fn the_users_own_lines_never_notify() {
        let mut r = Recent::default();
        assert!(arrival(&Author::User, "m1", "Hi", &style(), name, &mut r).is_none());
    }

    #[test]
    fn a_message_notifies_once() {
        let mut r = Recent::default();
        assert!(arrival(&person("p"), "m1", "a", &style(), name, &mut r).is_some());
        assert!(arrival(&person("p"), "m1", "a", &style(), name, &mut r).is_none());
        assert!(arrival(&person("p"), "m2", "a", &style(), name, &mut r).is_some());
    }

    #[test]
    fn the_memory_is_bounded() {
        let mut r = Recent::default();
        for i in 0..Recent::KEEP + 1 {
            assert!(r.first(&i.to_string()));
        }
        assert!(r.first("0"), "the oldest id was forgotten");
        assert!(!r.first("2"));
    }

    #[test]
    fn the_preview_skips_scene_lines_and_citations() {
        assert_eq!(
            preview("(she looks up)\nSee this [#u1]\nMore", ("(", ")")),
            "See this"
        );
        assert_eq!(preview("(she looks up)", ("(", ")")), "she looks up");
        assert_eq!(preview("  \n ", ("(", ")")), "");
    }

    #[test]
    fn a_long_preview_is_cut_on_a_character_boundary() {
        let p = preview(&"w".repeat(500), ("(", ")"));
        assert_eq!(p.chars().count(), PREVIEW);
        assert!(p.ends_with('\u{2026}'));
        let wide = preview(&"\u{00e9}".repeat(500), ("(", ")"));
        assert_eq!(wide.chars().count(), PREVIEW);
    }

    #[test]
    fn an_empty_message_does_not_notify() {
        let mut r = Recent::default();
        assert!(arrival(&person("p"), "m1", "[#u1]", &style(), name, &mut r).is_none());
        assert!(
            arrival(&person("p"), "m1", "now", &style(), name, &mut r).is_some(),
            "an empty one does not use up the id"
        );
    }

    #[test]
    fn the_badge_is_blank_at_zero() {
        assert_eq!(badge(0), None);
        assert_eq!(badge(3).as_deref(), Some("3"));
    }
}
