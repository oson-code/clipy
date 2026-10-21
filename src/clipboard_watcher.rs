//! Background thread that records clipboard changes into history.
//!
//! Changes are signalled by XFixes (X11 and XWayland, so no idle CPU), by the GNOME Shell
//! extension, or, when neither is available, by polling the clipboard text every 500 ms.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::clipboard::ClipboardIo;
use crate::config::{ExcludedApp, SharedSettings, TypePrefs};
use crate::db::{Database, SaveOptions};
use crate::model::{Hash, NewClip};
use crate::{images, paste, x11};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Owners often re-announce the clipboard right after taking it; read once after the burst.
const SETTLE: Duration = Duration::from_millis(40);

const SECRET_TARGET: &str = "x-kde-passwordManagerHint";
const TEXT_TARGETS: [&str; 5] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
    "TEXT",
];
const FILE_TARGETS: [&str; 2] = ["x-special/gnome-copied-files", "text/uri-list"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentType {
    Text,
    Html,
    Image,
    Files,
}

/// The first kind of content the copying app offered, in its own order (Clipy's "primary
/// type"). Bookkeeping targets such as TARGETS and TIMESTAMP are skipped.
fn first_content_type(targets: &[String]) -> Option<ContentType> {
    targets.iter().find_map(|t| match t.as_str() {
        "text/html" => Some(ContentType::Html),
        t if TEXT_TARGETS.contains(&t) => Some(ContentType::Text),
        t if FILE_TARGETS.contains(&t) => Some(ContentType::Files),
        t if t.starts_with("image/") => Some(ContentType::Image),
        _ => None,
    })
}

/// Hash of the last thing Clipy itself put on the clipboard, so that paste does not record its
/// own write as a new copy.
#[derive(Clone, Default)]
pub struct SelfEcho(Arc<Mutex<Option<Hash>>>);

impl SelfEcho {
    pub fn set(&self, hash: Hash) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(hash);
    }

    fn take_if(&self, hash: &Hash) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref() == Some(hash) {
            *slot = None;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    X11,
    Extension,
}

enum Msg {
    Changed {
        source: Source,
        app_id: Option<String>,
    },
    ImageFile(PathBuf),
}

#[derive(Clone)]
pub struct WatcherHandle {
    tx: Sender<Msg>,
    extension_active: Arc<AtomicBool>,
}

impl WatcherHandle {
    /// The GNOME Shell extension reports a change, with the desktop id of the focused app.
    pub fn clipboard_changed(&self, app_id: Option<String>) {
        let _ = self.tx.send(Msg::Changed {
            source: Source::Extension,
            app_id,
        });
    }

    /// While the extension is active its events, which name the source app, replace XFixes
    /// events: XWayland cannot tell which Wayland app copied, so an excluded app would slip
    /// through on the XFixes event that arrives first.
    pub fn set_extension_active(&self, active: bool) {
        self.extension_active.store(active, Ordering::Relaxed);
    }

    /// Adds a PNG that did not come through the clipboard (a new screenshot file).
    pub fn add_image_file(&self, path: PathBuf) {
        let _ = self.tx.send(Msg::ImageFile(path));
    }
}

pub fn spawn(
    db_path: PathBuf,
    settings: SharedSettings,
    self_echo: SelfEcho,
    proxy: EventLoopProxy<UserEvent>,
) -> WatcherHandle {
    let (tx, rx) = mpsc::channel();
    let handle = WatcherHandle {
        tx: tx.clone(),
        extension_active: Arc::new(AtomicBool::new(false)),
    };
    let extension_active = handle.extension_active.clone();
    thread::Builder::new()
        .name("clipy-watcher".into())
        .spawn(move || {
            if let Err(e) = run(
                db_path,
                settings,
                self_echo,
                proxy,
                extension_active,
                tx,
                rx,
            ) {
                log::error!("clipboard watcher stopped: {e:#}");
            }
        })
        .expect("spawning the clipboard watcher");
    handle
}

fn run(
    db_path: PathBuf,
    settings: SharedSettings,
    self_echo: SelfEcho,
    proxy: EventLoopProxy<UserEvent>,
    extension_active: Arc<AtomicBool>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
) -> Result<()> {
    let clipboard = open_clipboard();
    let event_driven = x11::display_available() && start_xfixes(tx);
    let mut watcher = Watcher {
        db: Database::init(&db_path)?,
        clipboard,
        probe: if x11::display_available() {
            x11::X11Probe::new()
                .map_err(|e| log::warn!("X11 probe unavailable: {e:#}"))
                .ok()
        } else {
            None
        },
        settings,
        self_echo,
        proxy,
        last_hash: None,
        last_poll_text: None,
    };
    // Like Clipy, start from what is already on the clipboard instead of recording it.
    watcher.last_poll_text = watcher.clipboard.read_text();
    log::info!(
        "watching the clipboard ({})",
        if event_driven {
            "XFixes events"
        } else {
            "polling"
        }
    );

    loop {
        let msg = if !needs_polling(event_driven, &extension_active) {
            match rx.recv() {
                Ok(msg) => msg,
                Err(_) => return Ok(()),
            }
        } else {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    watcher.poll();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        };
        let from_extension_only = |source: Source| {
            source == Source::Extension || !extension_active.load(Ordering::Relaxed)
        };
        match msg {
            Msg::Changed { source, mut app_id } if from_extension_only(source) => {
                thread::sleep(SETTLE);
                for later in rx.try_iter() {
                    match later {
                        Msg::Changed {
                            source,
                            app_id: Some(id),
                        } if from_extension_only(source) => app_id = Some(id),
                        Msg::Changed { .. } => {}
                        Msg::ImageFile(path) => watcher.add_image_file(&path),
                    }
                }
                watcher.capture(app_id);
            }
            Msg::Changed { .. } => {}
            Msg::ImageFile(path) => watcher.add_image_file(&path),
        }
    }
}

/// Whether clipboard changes must be found by polling. XFixes through XWayland sees every
/// change on X11 and on GNOME, whose compositor mirrors the Wayland clipboard to X eagerly;
/// KWin and wlroots compositors mirror it only while an X11 window has focus, so there the
/// Wayland clipboard (data-control) is polled unless the GNOME extension reports changes.
fn needs_polling(event_driven: bool, extension_active: &AtomicBool) -> bool {
    if extension_active.load(Ordering::Relaxed) {
        return false;
    }
    !event_driven || (paste::is_wayland_session() && !paste::is_gnome_session())
}

/// Opens the clipboard, retrying while the display is not up yet (Clipy can start at login
/// before Xwayland does).
fn open_clipboard() -> ClipboardIo {
    let mut delay = Duration::from_secs(1);
    loop {
        match ClipboardIo::new() {
            Ok(clipboard) => return clipboard,
            Err(e) => {
                log::warn!(
                    "clipboard unavailable, retrying in {}s: {e:#}",
                    delay.as_secs()
                );
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }
}

fn start_xfixes(tx: Sender<Msg>) -> bool {
    let events = match x11::ClipboardEvents::new() {
        Ok(events) => events,
        Err(e) => {
            log::warn!("XFixes clipboard events unavailable, falling back to polling: {e:#}");
            return false;
        }
    };
    thread::Builder::new()
        .name("clipy-xfixes".into())
        .spawn(move || {
            let result = events.run(|| {
                let _ = tx.send(Msg::Changed {
                    source: Source::X11,
                    app_id: None,
                });
            });
            if let Err(e) = result {
                log::error!("XFixes connection lost: {e:#}");
            }
        })
        .is_ok()
}

struct Watcher {
    db: Database,
    clipboard: ClipboardIo,
    probe: Option<x11::X11Probe>,
    settings: SharedSettings,
    self_echo: SelfEcho,
    proxy: EventLoopProxy<UserEvent>,
    last_hash: Option<Hash>,
    last_poll_text: Option<String>,
}

impl Watcher {
    fn poll(&mut self) {
        let text = self.clipboard.read_text();
        if text != self.last_poll_text {
            self.last_poll_text = text;
            self.capture(None);
        }
    }

    fn capture(&mut self, app_id: Option<String>) {
        let settings = self
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let targets = self.probe.as_mut().and_then(|p| {
            p.targets()
                .map_err(|e| log::debug!("reading TARGETS: {e:#}"))
                .ok()
        });
        if settings.types.ignore_confidential
            && targets
                .as_ref()
                .is_some_and(|t| t.iter().any(|t| t == SECRET_TARGET))
        {
            log::debug!("skipping content marked confidential");
            return;
        }
        let app = match app_id {
            Some(id) => vec![id],
            // XWayland only knows X11 windows; on Wayland its answer would name the wrong app.
            None if paste::is_wayland_session() => Vec::new(),
            None => self
                .probe
                .as_ref()
                .map(|p| p.active_window_class())
                .unwrap_or_default(),
        };
        if is_excluded(&app, &settings.excluded_apps) {
            log::debug!("skipping a copy from excluded app {app:?}");
            return;
        }
        let Some(clip) = self.read(targets.as_deref(), &settings.types) else {
            return;
        };
        if self.last_hash == Some(clip.hash) {
            return;
        }
        self.last_hash = Some(clip.hash);
        if self.self_echo.take_if(&clip.hash) {
            return;
        }
        let opts = SaveOptions {
            copy_same: settings.menu.copy_same_history,
            overwrite_same: settings.menu.overwrite_same_history,
            limit: settings.general.max_history as usize,
            sort: settings.general.sort_by,
        };
        match self.db.save(&clip, &opts) {
            Ok(outcome) => {
                log::debug!("saved {:?}: {outcome:?}", clip.data_type);
                let _ = self.proxy.send_event(UserEvent::HistoryChanged);
            }
            Err(e) => log::error!("saving clipboard history: {e:#}"),
        }
    }

    /// Picks what to store from the formats on offer: files, then images that come without
    /// text, then HTML with its text, then plain text. Formats switched off in the Type pane are
    /// skipped, as in Clipy.
    fn read(&mut self, targets: Option<&[String]>, types: &TypePrefs) -> Option<NewClip> {
        let offers =
            |names: &[&str]| targets.is_none_or(|t| t.iter().any(|x| names.contains(&x.as_str())));
        let offers_image = targets.is_none_or(|t| t.iter().any(|x| x.starts_with("image/")));
        let offers_text = offers(&TEXT_TARGETS);

        if types.files && targets.is_some() && offers(&FILE_TARGETS) {
            if let Some(uris) = self.clipboard.read_files() {
                return Some(NewClip::files(uris));
            }
        }
        // A copy that offers both text and an image is stored as text; the image only when no
        // text comes with it.
        let text = if offers_text {
            self.clipboard.read_text().filter(|t| !t.trim().is_empty())
        } else {
            None
        };
        if types.images && offers_image && text.is_none() {
            if let Some((rgba, w, h)) = self.clipboard.read_image() {
                return NewClip::image(&rgba, w, h)
                    .map_err(|e| log::warn!("encoding clipboard image: {e:#}"))
                    .ok();
            }
        }
        if types.html && targets.is_some() && offers(&["text/html"]) {
            if let Some(html) = self.clipboard.read_html().filter(|h| !h.trim().is_empty()) {
                let html_first = targets.and_then(first_content_type) == Some(ContentType::Html);
                return Some(NewClip::html(html, text, html_first));
            }
        }
        if types.text {
            return text.map(NewClip::text);
        }
        None
    }

    fn add_image_file(&mut self, path: &std::path::Path) {
        let settings = self
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let clip = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|png| images::decode_png(&png))
            .and_then(|(rgba, w, h)| NewClip::image(&rgba, w, h));
        let clip = match clip {
            Ok(clip) => clip,
            Err(e) => return log::warn!("reading screenshot {}: {e:#}", path.display()),
        };
        let opts = SaveOptions {
            copy_same: settings.menu.copy_same_history,
            overwrite_same: settings.menu.overwrite_same_history,
            limit: settings.general.max_history as usize,
            sort: settings.general.sort_by,
        };
        match self.db.save(&clip, &opts) {
            Ok(_) => {
                let _ = self.proxy.send_event(UserEvent::HistoryChanged);
            }
            Err(e) => log::error!("saving screenshot: {e:#}"),
        }
    }
}

/// Matches the source app (X11 WM_CLASS names, or a desktop id from the GNOME extension)
/// against the Exclude list.
fn is_excluded(app: &[String], excluded: &[ExcludedApp]) -> bool {
    // With no X11 window class and no extension (Wayland), the source app is unknown and
    // nothing is excluded.
    let stem = |s: &str| s.strip_suffix(".desktop").unwrap_or(s).to_ascii_lowercase();
    excluded.iter().any(|e| {
        let keys = [Some(stem(&e.desktop_id)), e.wm_class.as_deref().map(stem)];
        app.iter()
            .map(|a| stem(a))
            .any(|a| keys.iter().flatten().any(|k| *k == a))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    fn app(desktop_id: &str, wm_class: Option<&str>) -> ExcludedApp {
        ExcludedApp {
            desktop_id: desktop_id.into(),
            name: String::new(),
            wm_class: wm_class.map(Into::into),
        }
    }

    #[test]
    fn exclusion_matches_class_or_desktop_id() {
        let list = [app("org.keepassxc.KeePassXC.desktop", Some("keepassxc"))];
        assert!(is_excluded(
            &["KeePassXC".into(), "keepassxc".into()],
            &list
        ));
        assert!(is_excluded(
            &["org.keepassxc.KeePassXC.desktop".into()],
            &list
        ));
        assert!(!is_excluded(&["gedit".into()], &list));
        assert!(!is_excluded(&[], &list));
        assert!(is_excluded(
            &["Bitwarden".into()],
            &[app("bitwarden.desktop", None)]
        ));
    }

    #[test]
    fn first_content_type_follows_the_offer_order() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let firefox = t(&[
            "TIMESTAMP",
            "TARGETS",
            "text/html",
            "text/_moz_htmlinfo",
            "UTF8_STRING",
        ]);
        assert_eq!(first_content_type(&firefox), Some(ContentType::Html));
        let chrome = t(&[
            "TIMESTAMP",
            "TARGETS",
            "SAVE_TARGETS",
            "UTF8_STRING",
            "text/html",
        ]);
        assert_eq!(first_content_type(&chrome), Some(ContentType::Text));
        assert_eq!(
            first_content_type(&t(&["TARGETS", "image/png"])),
            Some(ContentType::Image)
        );
        assert_eq!(
            first_content_type(&t(&["x-special/gnome-copied-files"])),
            Some(ContentType::Files)
        );
        assert_eq!(first_content_type(&t(&["TARGETS"])), None);
    }

    #[test]
    fn self_echo_is_consumed_once() {
        let echo = SelfEcho::default();
        let h = model::hash_text("x");
        assert!(!echo.take_if(&h));
        echo.set(h);
        assert!(!echo.take_if(&model::hash_text("y")));
        assert!(echo.take_if(&h));
        assert!(!echo.take_if(&h));
    }
}
