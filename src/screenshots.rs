//! Clipy's Beta "Save screenshots in history": watches the screenshot folders and hands every
//! new screenshot to the clipboard watcher thread, which decodes and stores it.
//!
//! By default it watches `~/Pictures` and `~/Pictures/Screenshots` for PNGs whose names start
//! with "Screenshot" (GNOME, KDE Spectacle, Xfce and Cinnamon all name them so); both the
//! folders and the name prefix can be changed in the settings.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::clipboard_watcher::WatcherHandle;

pub const DEFAULT_PREFIX: &str = "Screenshot";

/// Keep it alive to keep watching; dropping it stops.
pub struct ScreenshotWatcher {
    _monitors: Vec<gio::FileMonitor>,
}

impl ScreenshotWatcher {
    /// `dirs` empty means the default folders; `prefix` empty accepts every PNG.
    pub fn start(watcher: WatcherHandle, dirs: &[String], prefix: &str) -> Self {
        let monitors = watched_dirs(dirs)
            .into_iter()
            .filter(|dir| dir.is_dir())
            .filter_map(|dir| {
                let monitor = gio::File::for_path(&dir)
                    .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                    .map_err(|e| log::warn!("watching {}: {e}", dir.display()))
                    .ok()?;
                let watcher = watcher.clone();
                let prefix = prefix.to_owned();
                monitor.connect_changed(move |_, file, other, event| {
                    // Screenshot tools either write the file in place or move a finished
                    // temporary file in.
                    let path = match event {
                        gio::FileMonitorEvent::ChangesDoneHint => file.path(),
                        gio::FileMonitorEvent::MovedIn | gio::FileMonitorEvent::Renamed => {
                            other.and_then(|f| f.path()).or_else(|| file.path())
                        }
                        _ => None,
                    };
                    if let Some(path) = path.filter(|p| is_screenshot(p, &prefix)) {
                        log::debug!("new screenshot {}", path.display());
                        watcher.add_image_file(path);
                    }
                });
                log::info!("watching {} for screenshots", dir.display());
                Some(monitor)
            })
            .collect();
        Self {
            _monitors: monitors,
        }
    }
}

/// `~/Pictures/Screenshots` and `~/Pictures`.
pub fn default_dirs() -> Vec<PathBuf> {
    let pictures = glib::user_special_dir(glib::UserDirectory::Pictures)
        .unwrap_or_else(|| glib::home_dir().join("Pictures"));
    vec![pictures.join("Screenshots"), pictures]
}

fn watched_dirs(configured: &[String]) -> Vec<PathBuf> {
    let configured: Vec<PathBuf> = configured
        .iter()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .map(|d| expand_home(d, &glib::home_dir()))
        .collect();
    if configured.is_empty() {
        default_dirs()
    } else {
        configured
    }
}

fn expand_home(dir: &str, home: &Path) -> PathBuf {
    match dir.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(dir),
    }
}

/// A PNG whose name starts with `prefix` (any case). An empty prefix accepts every PNG.
fn is_screenshot(path: &Path, prefix: &str) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.to_lowercase()
        .starts_with(&prefix.trim().to_lowercase())
        && !name.starts_with('.')
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_screenshot_names() {
        let p = |s| PathBuf::from(s);
        assert!(is_screenshot(
            &p("/p/Screenshot From 2026-10-21 10-00-00.png"),
            DEFAULT_PREFIX
        ));
        assert!(is_screenshot(
            &p("/p/Screenshot_20261021_100000.PNG"),
            DEFAULT_PREFIX
        ));
        assert!(!is_screenshot(&p("/p/holiday.png"), DEFAULT_PREFIX));
        assert!(!is_screenshot(&p("/p/Screenshot.jpg"), DEFAULT_PREFIX));
        assert!(!is_screenshot(
            &p("/p/.Screenshot.png.part"),
            DEFAULT_PREFIX
        ));
        assert!(is_screenshot(&p("/p/flameshot-2026.png"), "Flameshot"));
        assert!(is_screenshot(&p("/p/holiday.png"), ""));
        assert!(!is_screenshot(&p("/p/.hidden.png"), ""));
    }

    #[test]
    fn configured_dirs_expand_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            expand_home("~/Shots", home),
            PathBuf::from("/home/me/Shots")
        );
        assert_eq!(expand_home("~", home), PathBuf::from("/home/me"));
        assert_eq!(expand_home("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_home("/tmp/x", home), PathBuf::from("/tmp/x"));
    }
}
