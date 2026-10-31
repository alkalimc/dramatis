//! Showing one system notification. Everywhere but macOS this is the notification
//! plugin. On macOS it is the library under the plugin, called directly, because the
//! plugin has no way to set the picture at the banner's left (where the app's icon
//! sits), which is where a message's sender belongs.
//!
//! The plugin sends a banner and forgets it: it does not wait for a click, and a click
//! brings the app forward, which reaches `RunEvent::Reopen` and shows the window. The
//! macOS path sends the same way (no `wait_for_click`), so a click does the same.

use std::path::PathBuf;

use tauri::AppHandle;

/// What one banner says and shows.
pub struct Banner {
    pub title: String,
    pub body: String,
    /// A picture on this machine; `None` leaves the app's own icon. Only macOS shows it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub image: Option<PathBuf>,
}

/// Where on the banner the picture goes.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// In place of the app's icon, at the left.
    Icon,
    /// A thumbnail at the right, beside the app's icon.
    Thumbnail,
}

/// Show `banner`; a failure after this returns (the send runs off the calling thread, as
/// the plugin's does) goes to `on_error`.
#[cfg(not(target_os = "macos"))]
pub fn show(app: &AppHandle, banner: Banner, on_error: impl FnOnce(String) + Send + 'static) {
    use tauri_plugin_notification::NotificationExt;

    // The plugin takes an icon by name, not a picture to show.
    if let Err(e) = app
        .notification()
        .builder()
        .title(banner.title)
        .body(banner.body)
        .show()
    {
        on_error(e.to_string());
    }
}

/// Show `banner` with the picture in place of the app's icon.
#[cfg(target_os = "macos")]
pub fn show(app: &AppHandle, banner: Banner, on_error: impl FnOnce(String) + Send + 'static) {
    show_at(app, banner, Place::Icon, on_error);
}

#[cfg(target_os = "macos")]
pub fn show_at(
    app: &AppHandle,
    banner: Banner,
    place: Place,
    on_error: impl FnOnce(String) + Send + 'static,
) {
    let identifier = app.config().identifier.clone();
    // The send waits (a couple of seconds at most) for the system to take the banner.
    tauri::async_runtime::spawn_blocking(move || {
        // Same choice as the plugin: a dev build is not a registered app, so it borrows
        // Terminal's identity. Only the first call takes effect; the rest report that.
        let _ = mac_notification_sys::set_application(if tauri::is_dev() {
            "com.apple.Terminal"
        } else {
            &identifier
        });
        let image = banner.image.as_deref().and_then(|p| p.to_str());
        let mut n = mac_notification_sys::Notification::new();
        n.title(&banner.title).message(&banner.body);
        match (place, image) {
            (Place::Icon, Some(path)) => {
                n.app_icon(path);
            }
            (Place::Thumbnail, Some(path)) => {
                n.content_image(path);
            }
            (_, None) => {}
        }
        if let Err(e) = n.send() {
            on_error(e.to_string());
        }
    });
}

/// Debug builds only: `DRAMATIS_NOTIFY_TEST` sends one sample banner at startup, whatever
/// the window and the settings, to see the picture without waiting for a message. The
/// value picks the place (`icon`, `thumbnail`, or `none` for no picture); the picture is
/// the first one in the avatar cache.
#[cfg(all(debug_assertions, target_os = "macos"))]
pub fn preview(app: &AppHandle, avatars: &std::path::Path) {
    let Some(mode) = std::env::var("DRAMATIS_NOTIFY_TEST")
        .ok()
        .filter(|v| !v.is_empty())
    else {
        return;
    };
    let place = if mode == "thumbnail" {
        Place::Thumbnail
    } else {
        Place::Icon
    };
    let image = (mode != "none")
        .then(|| {
            let mut found: Vec<_> = std::fs::read_dir(avatars)
                .ok()?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            found.sort();
            found.into_iter().next()
        })
        .flatten();
    eprintln!("dramatis: sample notification ({mode}) with {image:?}");
    let banner = Banner {
        title: "Sample".into(),
        body: "A line to see where the picture goes.".into(),
        image,
    };
    show_at(app, banner, place, |e| {
        eprintln!("dramatis: sample notification failed: {e}");
    });
}
