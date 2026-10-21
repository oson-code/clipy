//! Global shortcuts: GTK accelerator strings (`<Super><Shift>v`) as stored in the settings,
//! grabbed on the X11 root window. A dedicated thread blocks on the X connection, so idle
//! shortcuts cost no CPU. On GNOME the Shell extension grabs the same strings instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, bail, Context, Result};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, Keycode, Mapping, ModMask, Window};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

use crate::config::Shortcuts;
use crate::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MainMenu,
    HistoryMenu,
    SnippetsMenu,
    ClearHistory,
    EditSnippets,
    Preferences,
    Folder(i64),
}

impl Action {
    pub fn encode(self) -> String {
        match self {
            Action::MainMenu => "main".into(),
            Action::HistoryMenu => "history".into(),
            Action::SnippetsMenu => "snippets".into(),
            Action::ClearHistory => "clear-history".into(),
            Action::EditSnippets => "edit-snippets".into(),
            Action::Preferences => "preferences".into(),
            Action::Folder(id) => format!("folder:{id}"),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "main" => Action::MainMenu,
            "history" => Action::HistoryMenu,
            "snippets" => Action::SnippetsMenu,
            "clear-history" => Action::ClearHistory,
            "edit-snippets" => Action::EditSnippets,
            "preferences" => Action::Preferences,
            _ => Action::Folder(s.strip_prefix("folder:")?.parse().ok()?),
        })
    }

    /// Menus pop up when the shortcut is released: on X11 a menu cannot grab the keyboard
    /// while the shortcut's own key grab is still active.
    pub fn fires_on_release(self) -> bool {
        matches!(
            self,
            Action::MainMenu | Action::HistoryMenu | Action::SnippetsMenu | Action::Folder(_)
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::MainMenu => tr("Main Menu:"),
            Action::HistoryMenu => tr("History:"),
            Action::SnippetsMenu => tr("Snippets:"),
            Action::ClearHistory => tr("Clear History:"),
            Action::EditSnippets => tr("Edit Snippets:"),
            Action::Preferences => tr("Open Preferences:"),
            Action::Folder(_) => tr("Snippet folder:"),
        }
    }
}

/// Every configured shortcut, skipping empty ones.
pub fn shortcut_list(s: &Shortcuts) -> Vec<(Action, String)> {
    let mut list: Vec<(Action, String)> = [
        (Action::MainMenu, &s.main),
        (Action::HistoryMenu, &s.history),
        (Action::SnippetsMenu, &s.snippets),
        (Action::ClearHistory, &s.clear_history),
        (Action::EditSnippets, &s.edit_snippets),
        (Action::Preferences, &s.preferences),
    ]
    .into_iter()
    .map(|(a, accel)| (a, accel.clone()))
    .collect();
    list.extend(
        s.folders
            .iter()
            .filter_map(|(id, accel)| Some((Action::Folder(id.parse().ok()?), accel.clone()))),
    );
    list.retain(|(_, accel)| !accel.trim().is_empty());
    list
}

/// A shortcut as X11 sees it: a modifier mask and a keysym.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u16,
    pub keysym: u32,
}

const SHIFT: u16 = 1 << 0;
const LOCK: u16 = 1 << 1;
const CONTROL: u16 = 1 << 2;
const MOD1: u16 = 1 << 3;
const MOD2: u16 = 1 << 4;
const MOD4: u16 = 1 << 6;
/// Caps Lock and Num Lock must not stop a shortcut from working.
const IGNORED: [u16; 4] = [0, LOCK, MOD2, LOCK | MOD2];

/// Parses a GTK accelerator such as `<Control><Alt>t` or `<Super><Shift>F5`.
pub fn parse_accel(accel: &str) -> Result<Hotkey> {
    let mut rest = accel.trim();
    let mut mods = 0;
    while let Some(after) = rest.strip_prefix('<') {
        let end = after
            .find('>')
            .ok_or_else(|| anyhow!("unclosed modifier in {accel:?}"))?;
        let name = after[..end].to_ascii_lowercase();
        mods |= match name.as_str() {
            "shift" => SHIFT,
            "control" | "ctrl" | "primary" => CONTROL,
            "alt" | "mod1" => MOD1,
            "super" | "meta" | "mod4" | "hyper" => MOD4,
            _ => bail!("unknown modifier <{name}> in {accel:?}"),
        };
        rest = &after[end + 1..];
    }
    if rest.is_empty() {
        bail!("no key in {accel:?}");
    }
    let keysym = keysym(rest).ok_or_else(|| anyhow!("unsupported key {rest:?} in {accel:?}"))?;
    Ok(Hotkey { mods, keysym })
}

fn keysym(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let c = c.to_ascii_lowercase();
        // Latin-1 printable characters are their own keysyms.
        return (c.is_ascii_graphic()).then_some(c as u32);
    }
    let lower = name.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0xffbe + n - 1);
    }
    Some(match lower.as_str() {
        "space" => 0x0020,
        "return" | "enter" => 0xff0d,
        "tab" => 0xff09,
        "escape" => 0xff1b,
        "backspace" => 0xff08,
        "insert" => 0xff63,
        "delete" => 0xffff,
        "home" => 0xff50,
        "end" => 0xff57,
        "page_up" | "prior" => 0xff55,
        "page_down" | "next" => 0xff56,
        "up" => 0xff52,
        "down" => 0xff54,
        "left" => 0xff51,
        "right" => 0xff53,
        "grave" => 0x0060,
        "minus" => 0x002d,
        "equal" => 0x003d,
        "comma" => 0x002c,
        "period" => 0x002e,
        "slash" => 0x002f,
        "backslash" => 0x005c,
        "semicolon" => 0x003b,
        "apostrophe" => 0x0027,
        "bracketleft" => 0x005b,
        "bracketright" => 0x005d,
        _ => return None,
    })
}

#[derive(Default)]
struct Grabs {
    wanted: Vec<(Action, Hotkey)>,
    active: HashMap<(Keycode, u16), Action>,
}

/// X11 key grabs for the configured shortcuts.
pub struct Hotkeys {
    conn: Arc<RustConnection>,
    root: Window,
    grabs: Arc<Mutex<Grabs>>,
}

impl Hotkeys {
    /// Connects to X and starts the thread that reports `(action, pressed)` for every grabbed
    /// shortcut.
    pub fn new(on_event: impl Fn(Action, bool) + Send + 'static) -> Result<Self> {
        let (conn, screen) = x11rb::connect(None).context("connecting to the X server")?;
        let root = conn.setup().roots[screen].root;
        // Without this, holding a shortcut sends a release with every auto-repeat.
        let detectable = conn
            .xkb_use_extension(1, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .filter(|r| r.supported)
            .and_then(|_| {
                let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
                conn.xkb_per_client_flags(
                    xkb::ID::USE_CORE_KBD.into(),
                    flag,
                    flag,
                    0u32.into(),
                    0u32.into(),
                    0u32.into(),
                )
                .ok()?
                .reply()
                .ok()
            })
            .is_some();
        if !detectable {
            log::warn!("XKB detectable auto-repeat unavailable; holding a shortcut may repeat it");
        }
        let conn = Arc::new(conn);
        let grabs = Arc::new(Mutex::new(Grabs::default()));
        let (thread_conn, thread_grabs) = (conn.clone(), grabs.clone());
        thread::Builder::new()
            .name("clipy-hotkeys".into())
            .spawn(move || {
                if let Err(e) = listen(&thread_conn, root, &thread_grabs, on_event) {
                    log::error!("global shortcuts stopped: {e:#}");
                }
            })
            .context("starting the shortcut thread")?;
        Ok(Self { conn, root, grabs })
    }

    /// Replaces every grab with `shortcuts`. Returns the shortcuts that could not be
    /// registered (bad syntax, or already taken by another app) with the reason.
    pub fn apply(&mut self, shortcuts: &Shortcuts) -> Vec<(Action, String)> {
        let mut failures = Vec::new();
        let mut wanted = Vec::new();
        for (action, accel) in shortcut_list(shortcuts) {
            match parse_accel(&accel) {
                Ok(hotkey) => wanted.push((action, hotkey)),
                Err(e) => failures.push((action, format!("{e:#}"))),
            }
        }
        let mut grabs = self.grabs.lock().unwrap_or_else(|e| e.into_inner());
        grabs.wanted = wanted;
        failures.extend(regrab(&self.conn, self.root, &mut grabs));
        for (action, reason) in &failures {
            log::warn!("shortcut for {}: {reason}", action.encode());
        }
        failures
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        let mut grabs = self.grabs.lock().unwrap_or_else(|e| e.into_inner());
        grabs.wanted.clear();
        regrab(&self.conn, self.root, &mut grabs);
    }
}

/// Releases every grab, then grabs `wanted` with the current keyboard layout.
fn regrab(conn: &RustConnection, root: Window, grabs: &mut Grabs) -> Vec<(Action, String)> {
    for (keycode, mods) in grabs.active.drain().map(|(k, _)| k) {
        for extra in IGNORED {
            let _ = conn.ungrab_key(keycode, root, ModMask::from(mods | extra));
        }
    }
    let mut failures = Vec::new();
    let keycodes = match keycode_table(conn) {
        Ok(table) => table,
        Err(e) => {
            let reason = format!("{e:#}");
            return grabs
                .wanted
                .iter()
                .map(|(a, _)| (*a, reason.clone()))
                .collect();
        }
    };
    for (action, hotkey) in grabs.wanted.clone() {
        let Some(&keycode) = keycodes.get(&hotkey.keysym) else {
            failures.push((action, tr("this key is not on the keyboard").to_owned()));
            continue;
        };
        if grabs.active.contains_key(&(keycode, hotkey.mods)) {
            failures.push((action, tr("the same shortcut is used twice").to_owned()));
            continue;
        }
        match grab(conn, root, keycode, hotkey.mods) {
            Ok(()) => {
                log::debug!(
                    "grabbed keycode {keycode} mods {:#x} for {}",
                    hotkey.mods,
                    action.encode()
                );
                grabs.active.insert((keycode, hotkey.mods), action);
            }
            Err(reason) => failures.push((action, reason)),
        }
    }
    let _ = conn.flush();
    failures
}

fn grab(
    conn: &RustConnection,
    root: Window,
    keycode: Keycode,
    mods: u16,
) -> std::result::Result<(), String> {
    for (i, extra) in IGNORED.iter().enumerate() {
        let result = conn
            .grab_key(
                false,
                root,
                ModMask::from(mods | extra),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.check());
        if let Err(e) = result {
            for undo in &IGNORED[..i] {
                let _ = conn.ungrab_key(keycode, root, ModMask::from(mods | undo));
            }
            return Err(match e {
                ReplyError::X11Error(x) if x.error_kind == ErrorKind::Access => {
                    tr("already used by another app").to_owned()
                }
                other => other.to_string(),
            });
        }
    }
    Ok(())
}

/// keysym → the first keycode that produces it, unshifted or shifted.
fn keycode_table(conn: &RustConnection) -> Result<HashMap<u32, Keycode>> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = usize::from(mapping.keysyms_per_keycode).max(1);
    let mut table = HashMap::new();
    for column in 0..per.min(2) {
        for (i, row) in mapping.keysyms.chunks(per).enumerate() {
            if let Some(&sym) = row.get(column) {
                if sym != 0 {
                    table.entry(sym).or_insert(min + i as u8);
                }
            }
        }
    }
    Ok(table)
}

fn listen(
    conn: &RustConnection,
    root: Window,
    grabs: &Mutex<Grabs>,
    on_event: impl Fn(Action, bool),
) -> Result<()> {
    // The shortcut being held. Its release is matched by key alone: people (and xdotool) often
    // let go of the modifiers first, so the key comes up with no modifiers left.
    let mut held: Option<(Keycode, Action)> = None;
    loop {
        let (keycode, state, pressed) = match conn.wait_for_event()? {
            Event::KeyPress(ev) => (ev.detail, u16::from(ev.state), true),
            Event::KeyRelease(ev) => (ev.detail, u16::from(ev.state), false),
            Event::MappingNotify(ev) if ev.request == Mapping::KEYBOARD => {
                let mut grabs = grabs.lock().unwrap_or_else(|e| e.into_inner());
                regrab(conn, root, &mut grabs);
                continue;
            }
            _ => continue,
        };
        log::trace!("key {keycode} state {state:#x} pressed {pressed}");
        if !pressed {
            if let Some((_, action)) = held.filter(|(key, _)| *key == keycode) {
                held = None;
                on_event(action, false);
            }
            continue;
        }
        if held.is_some_and(|(key, _)| key == keycode) {
            continue; // auto-repeat of the shortcut already down
        }
        let mods = state & (SHIFT | CONTROL | MOD1 | MOD4);
        let action = grabs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .get(&(keycode, mods))
            .copied();
        if let Some(action) = action {
            held = Some((keycode, action));
            on_event(action, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gtk_accelerators() {
        assert_eq!(
            parse_accel("<Super><Shift>v").unwrap(),
            Hotkey {
                mods: MOD4 | SHIFT,
                keysym: 0x76
            }
        );
        assert_eq!(
            parse_accel("<Primary><Mod1>F5").unwrap(),
            Hotkey {
                mods: CONTROL | MOD1,
                keysym: 0xffc2
            }
        );
        assert_eq!(
            parse_accel("<Control><Alt><Shift><Super>V").unwrap(),
            Hotkey {
                mods: CONTROL | MOD1 | SHIFT | MOD4,
                keysym: 0x76
            }
        );
        assert_eq!(
            parse_accel("Insert").unwrap(),
            Hotkey {
                mods: 0,
                keysym: 0xff63
            }
        );
        assert_eq!(
            parse_accel("<Super>1").unwrap(),
            Hotkey {
                mods: MOD4,
                keysym: 0x31
            }
        );
        assert_eq!(parse_accel("<Super>F24").unwrap().keysym, 0xffd5);
    }

    #[test]
    fn rejects_bad_accelerators() {
        assert!(parse_accel("").is_err());
        assert!(parse_accel("<Super>").is_err());
        assert!(parse_accel("<Bogus>v").is_err());
        assert!(parse_accel("<Super v").is_err());
        assert!(parse_accel("<Super>F25").is_err());
        assert!(parse_accel("<Super>é").is_err());
    }

    #[test]
    fn shortcut_list_skips_empty_and_adds_folders() {
        let mut s = Shortcuts::default();
        s.history.clear();
        s.folders.insert("3".into(), "<Super>3".into());
        s.folders.insert("bad".into(), "<Super>4".into());
        let actions: Vec<_> = shortcut_list(&s).into_iter().map(|(a, _)| a).collect();
        assert_eq!(
            actions,
            [
                Action::MainMenu,
                Action::SnippetsMenu,
                Action::Preferences,
                Action::Folder(3)
            ]
        );
    }

    #[test]
    fn actions_round_trip() {
        for a in [
            Action::MainMenu,
            Action::HistoryMenu,
            Action::SnippetsMenu,
            Action::ClearHistory,
            Action::EditSnippets,
            Action::Preferences,
            Action::Folder(42),
        ] {
            assert_eq!(Action::parse(&a.encode()), Some(a));
        }
        assert!(Action::MainMenu.fires_on_release());
        assert!(!Action::ClearHistory.fires_on_release());
    }
}
