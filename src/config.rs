//! User settings: every Clipy preference, stored as TOML in `~/.config/clipy/config.toml`.
//!
//! Defaults follow Clipy's `AppStorageValues.swift`, except where the Linux spec sets its own
//! value (history limit, title length, main shortcut).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

pub type SharedSettings = Arc<RwLock<Settings>>;

const APP_DIR: &str = "clipy";

/// `~/.config/clipy`
pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".config").join(APP_DIR))
}

/// `~/.local/share/clipy`
pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".local/share").join(APP_DIR))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn database_path() -> PathBuf {
    data_dir().join("clipy.db")
}

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", APP_DIR)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    DateCreated,
    LastUsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelIcon {
    /// Follows the system theme, like Clipy's template icon.
    Standard,
    Black,
    #[serde(alias = "reversed")]
    White,
    Hidden,
}

impl PanelIcon {
    pub const ALL: [PanelIcon; 4] = [
        PanelIcon::Standard,
        PanelIcon::Black,
        PanelIcon::White,
        PanelIcon::Hidden,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PanelIcon::Standard => tr("Standard"),
            PanelIcon::Black => tr("Black"),
            PanelIcon::White => tr("White"),
            PanelIcon::Hidden => tr("Hidden"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClearInterval {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "30m")]
    ThirtyMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "4h")]
    FourHours,
    #[serde(rename = "8h")]
    EightHours,
    #[serde(rename = "1d")]
    OneDay,
    #[serde(rename = "3d")]
    ThreeDays,
    #[serde(rename = "7d")]
    SevenDays,
}

impl ClearInterval {
    pub const ALL: [ClearInterval; 8] = [
        ClearInterval::FiveMinutes,
        ClearInterval::ThirtyMinutes,
        ClearInterval::OneHour,
        ClearInterval::FourHours,
        ClearInterval::EightHours,
        ClearInterval::OneDay,
        ClearInterval::ThreeDays,
        ClearInterval::SevenDays,
    ];

    pub fn seconds(self) -> u32 {
        match self {
            ClearInterval::FiveMinutes => 300,
            ClearInterval::ThirtyMinutes => 1_800,
            ClearInterval::OneHour => 3_600,
            ClearInterval::FourHours => 14_400,
            ClearInterval::EightHours => 28_800,
            ClearInterval::OneDay => 86_400,
            ClearInterval::ThreeDays => 259_200,
            ClearInterval::SevenDays => 604_800,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ClearInterval::FiveMinutes => tr("Every 5 minutes"),
            ClearInterval::ThirtyMinutes => tr("Every 30 minutes"),
            ClearInterval::OneHour => tr("Every hour"),
            ClearInterval::FourHours => tr("Every 4 hours"),
            ClearInterval::EightHours => tr("Every 8 hours"),
            ClearInterval::OneDay => tr("Every day"),
            ClearInterval::ThreeDays => tr("Every 3 days"),
            ClearInterval::SevenDays => tr("Every 7 days"),
        }
    }
}

/// A modifier key held while choosing a menu item (Clipy's Beta actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKey {
    Super,
    Shift,
    Ctrl,
    Alt,
}

impl ModifierKey {
    /// In the order of Clipy's choices: ⌘ (Ctrl here), ⇧, ⌃ (Super here), ⌥.
    pub const ALL: [ModifierKey; 4] = [
        ModifierKey::Ctrl,
        ModifierKey::Shift,
        ModifierKey::Super,
        ModifierKey::Alt,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ModifierKey::Super => tr("Super"),
            ModifierKey::Shift => tr("Shift"),
            ModifierKey::Ctrl => tr("Ctrl"),
            ModifierKey::Alt => tr("Alt"),
        }
    }

    pub fn is_held(self, modifiers: Modifiers) -> bool {
        match self {
            ModifierKey::Super => modifiers.has(Modifiers::SUPER) || modifiers.has(Modifiers::MOD4),
            ModifierKey::Shift => modifiers.has(Modifiers::SHIFT),
            ModifierKey::Ctrl => modifiers.has(Modifiers::CONTROL),
            ModifierKey::Alt => modifiers.has(Modifiers::ALT),
        }
    }
}

/// Modifier mask in the GDK/Clutter bit layout, which is also what the GNOME Shell extension
/// sends over D-Bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const SHIFT: u32 = 1 << 0;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const MOD4: u32 = 1 << 6;
    pub const SUPER: u32 = 1 << 26;

    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedApp {
    /// Desktop entry id, e.g. `org.keepassxc.KeePassXC.desktop`.
    pub desktop_id: String,
    pub name: String,
    /// `StartupWMClass` of the desktop entry, when it declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wm_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub launch_at_login: bool,
    pub suppress_login_alert: bool,
    pub paste_automatically: bool,
    pub max_history: u32,
    pub sort_by: SortBy,
    pub clear_on_quit: bool,
    pub clear_periodically: bool,
    pub clear_interval: ClearInterval,
    pub panel_icon: PanelIcon,
}

impl Default for General {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            suppress_login_alert: false,
            paste_automatically: true,
            max_history: 50,
            sort_by: SortBy::LastUsed,
            clear_on_quit: false,
            clear_periodically: false,
            clear_interval: ClearInterval::OneHour,
            panel_icon: PanelIcon::Standard,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuPrefs {
    pub inline_items: u32,
    pub items_per_folder: u32,
    pub max_title_len: u32,
    pub copy_same_history: bool,
    pub overwrite_same_history: bool,
    pub show_icons: bool,
    pub mark_numbers: bool,
    pub start_at_zero: bool,
    pub numeric_keys: bool,
    pub snippets_above_history: bool,
    pub show_clear_item: bool,
    pub confirm_clear: bool,
    pub show_tooltips: bool,
    pub tooltip_len: u32,
    pub show_images: bool,
    pub thumb_w: u32,
    pub thumb_h: u32,
    pub show_color_preview: bool,
}

impl Default for MenuPrefs {
    fn default() -> Self {
        Self {
            inline_items: 0,
            items_per_folder: 10,
            max_title_len: 35,
            copy_same_history: true,
            overwrite_same_history: true,
            show_icons: true,
            mark_numbers: true,
            start_at_zero: false,
            numeric_keys: false,
            snippets_above_history: false,
            show_clear_item: true,
            confirm_clear: true,
            show_tooltips: true,
            tooltip_len: 200,
            show_images: true,
            thumb_w: 100,
            thumb_h: 32,
            show_color_preview: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypePrefs {
    pub text: bool,
    pub html: bool,
    pub images: bool,
    pub files: bool,
    /// Skip content that password managers mark with `x-kde-passwordManagerHint`.
    pub ignore_confidential: bool,
}

impl Default for TypePrefs {
    fn default() -> Self {
        Self {
            text: true,
            html: true,
            images: true,
            files: true,
            ignore_confidential: false,
        }
    }
}

/// Global shortcuts as GTK accelerator strings (`<Super><Shift>v`). An empty string means
/// "no shortcut", so a cleared shortcut survives a reload instead of coming back as the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub main: String,
    pub history: String,
    pub snippets: String,
    pub clear_history: String,
    pub edit_snippets: String,
    /// Always set: it is the way back into the app when the panel icon is hidden.
    pub preferences: String,
    /// Snippet folder id → accelerator.
    pub folders: BTreeMap<String, String>,
}

impl Default for Shortcuts {
    fn default() -> Self {
        // Clipy's ⌘ shortcuts move to Super: with Ctrl, ⌘⇧V would become Ctrl+Shift+V, which
        // terminals use for paste (and which Clipy itself sends to them).
        Self {
            main: "<Super><Shift>v".into(),
            history: "<Super><Control>v".into(),
            snippets: "<Super><Shift>b".into(),
            clear_history: String::new(),
            edit_snippets: String::new(),
            preferences: "<Control><Alt><Shift><Super>v".into(),
            folders: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beta {
    pub paste_plain_text: bool,
    pub paste_plain_text_modifier: ModifierKey,
    pub delete_history: bool,
    pub delete_history_modifier: ModifierKey,
    pub paste_and_delete: bool,
    pub paste_and_delete_modifier: ModifierKey,
    pub observe_screenshots: bool,
    /// Folders watched for screenshots; empty means ~/Pictures and ~/Pictures/Screenshots.
    pub screenshot_dirs: Vec<String>,
    /// Screenshot file names start with this (any case); empty accepts every PNG.
    pub screenshot_prefix: String,
}

impl Default for Beta {
    fn default() -> Self {
        // Holding Clipy's ⌘ while choosing an item maps to Ctrl.
        Self {
            paste_plain_text: true,
            paste_plain_text_modifier: ModifierKey::Ctrl,
            delete_history: false,
            delete_history_modifier: ModifierKey::Ctrl,
            paste_and_delete: false,
            paste_and_delete_modifier: ModifierKey::Ctrl,
            observe_screenshots: false,
            screenshot_dirs: Vec::new(),
            screenshot_prefix: crate::screenshots::DEFAULT_PREFIX.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    /// Wait between closing the menu and sending the paste keystroke.
    pub paste_delay_ms: u32,
    /// Windows that paste with Ctrl+Shift+V, matched on WM_CLASS or the desktop app id.
    pub terminal_wm_classes: Vec<String>,
}

impl Default for Advanced {
    fn default() -> Self {
        // Terminals paste with Ctrl+Shift+V; matched on WM_CLASS or the desktop app id.
        let terminals = [
            "gnome-terminal",
            "gnome-terminal-server",
            "org.gnome.Terminal",
            "org.gnome.Console",
            "kgx",
            "org.gnome.Ptyxis",
            "ptyxis",
            "tilix",
            "com.gexperts.Tilix",
            "konsole",
            "org.kde.konsole",
            "xfce4-terminal",
            "mate-terminal",
            "x-terminal-emulator",
            "terminator",
            "kitty",
            "alacritty",
            "Alacritty",
            "wezterm",
            "org.wezfurlong.wezterm",
            "foot",
            "xterm",
            "urxvt",
        ];
        Self {
            paste_delay_ms: 50,
            terminal_wm_classes: terminals.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub menu: MenuPrefs,
    pub types: TypePrefs,
    pub shortcuts: Shortcuts,
    pub beta: Beta,
    pub advanced: Advanced,
    pub excluded_apps: Vec<ExcludedApp>,
}

impl Settings {
    /// Loads settings from `path`. A missing file gives the defaults; an unreadable one is moved
    /// aside to `config.toml.broken` so the user's edits are not lost when we save again.
    pub fn load(path: &Path) -> Settings {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
            Err(e) => {
                log::error!("cannot read {}: {e}", path.display());
                return Settings::default();
            }
        };
        match toml::from_str::<Settings>(&text) {
            Ok(settings) => settings.sanitized(),
            Err(e) => {
                let backup = path.with_extension("toml.broken");
                log::error!(
                    "invalid {}: {e}; moving it to {}",
                    path.display(),
                    backup.display()
                );
                if let Err(e) = fs::rename(path, &backup) {
                    log::error!("cannot move {}: {e}", path.display());
                }
                Settings::default()
            }
        }
    }

    /// Writes the settings atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing settings")?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Clamps every number to the range Clipy's settings panes allow.
    pub fn sanitized(mut self) -> Self {
        let g = &mut self.general;
        g.max_history = g.max_history.clamp(1, 1000);
        let m = &mut self.menu;
        m.inline_items = m.inline_items.min(100);
        m.items_per_folder = m.items_per_folder.clamp(1, 100);
        m.max_title_len = m.max_title_len.clamp(1, 1000);
        m.tooltip_len = m.tooltip_len.clamp(1, 10_000);
        m.thumb_w = m.thumb_w.clamp(1, 512);
        m.thumb_h = m.thumb_h.clamp(1, 512);
        if self.shortcuts.preferences.trim().is_empty() {
            self.shortcuts.preferences = Shortcuts::default().preferences;
        }
        self.advanced.paste_delay_ms = self.advanced.paste_delay_ms.min(2_000);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_clipy_and_spec() {
        let s = Settings::default();
        assert_eq!(s.general.max_history, 50);
        assert_eq!(s.menu.max_title_len, 35);
        assert_eq!(s.menu.items_per_folder, 10);
        assert_eq!(s.menu.inline_items, 0);
        assert_eq!((s.menu.thumb_w, s.menu.thumb_h), (100, 32));
        assert_eq!(s.menu.tooltip_len, 200);
        assert_eq!(s.general.sort_by, SortBy::LastUsed);
        assert_eq!(s.general.clear_interval, ClearInterval::OneHour);
        assert!(!s.types.ignore_confidential);
        assert_eq!(s.shortcuts.main, "<Super><Shift>v");
    }

    #[test]
    fn toml_round_trip_keeps_cleared_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut s = Settings::default();
        s.shortcuts.main.clear();
        s.shortcuts.folders.insert("7".into(), "<Super>1".into());
        s.excluded_apps.push(ExcludedApp {
            desktop_id: "org.keepassxc.KeePassXC.desktop".into(),
            name: "KeePassXC".into(),
            wm_class: Some("keepassxc".into()),
        });
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn partial_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[menu]\ninline_items = 5\n").unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.menu.inline_items, 5);
        assert_eq!(s.menu.items_per_folder, 10);
        assert_eq!(s.general, General::default());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[general]\nmax_history = 0\n[menu]\nitems_per_folder = 0\nthumb_w = 9999\n[shortcuts]\npreferences = \"\"\n",
        )
        .unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.general.max_history, 1);
        assert_eq!(s.menu.items_per_folder, 1);
        assert_eq!(s.menu.thumb_w, 512);
        assert!(!s.shortcuts.preferences.is_empty());
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(path.with_extension("toml.broken").exists());
        assert!(!path.exists());
    }

    #[test]
    fn modifier_keys_read_gdk_bits() {
        let m = Modifiers(Modifiers::SHIFT | Modifiers::MOD4);
        assert!(ModifierKey::Shift.is_held(m));
        assert!(ModifierKey::Super.is_held(m));
        assert!(!ModifierKey::Ctrl.is_held(m));
        assert!(!ModifierKey::Alt.is_held(m));
    }
}
