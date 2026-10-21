//! Direct X11 access for what arboard does not cover: clipboard change events (XFixes), the
//! clipboard's TARGETS, the active window's WM_CLASS, and synthetic key presses (XTest).
//!
//! On GNOME Wayland these work through XWayland, which mirrors the Wayland clipboard.

use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, Keycode, Window, WindowClass,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::NONE;

/// True when an X server (or XWayland) is reachable through `$DISPLAY`.
pub fn display_available() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

fn connect() -> Result<(RustConnection, usize)> {
    x11rb::connect(None).context("connecting to the X server")
}

fn intern(conn: &RustConnection, name: &str) -> Result<Atom> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

/// Clipboard owner changes, reported through XFixes.
pub struct ClipboardEvents {
    conn: RustConnection,
    clipboard: Atom,
}

impl ClipboardEvents {
    pub fn new() -> Result<Self> {
        let (conn, screen) = connect()?;
        conn.xfixes_query_version(5, 0)?
            .reply()
            .context("XFixes is not available")?;
        let root = conn.setup().roots[screen].root;
        let clipboard = intern(&conn, "CLIPBOARD")?;
        conn.xfixes_select_selection_input(
            root,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER,
        )?
        .check()?;
        Ok(Self { conn, clipboard })
    }

    /// Blocks, calling `on_change` every time something takes ownership of CLIPBOARD. Returns
    /// when the X connection breaks.
    pub fn run(self, mut on_change: impl FnMut()) -> Result<()> {
        loop {
            match self.conn.wait_for_event()? {
                Event::XfixesSelectionNotify(ev) if ev.selection == self.clipboard => on_change(),
                _ => {}
            }
        }
    }
}

/// Queries about the current clipboard and the focused window.
pub struct X11Probe {
    conn: RustConnection,
    root: Window,
    window: Window,
    clipboard: Atom,
    targets: Atom,
    property: Atom,
    active_window: Atom,
    names: HashMap<Atom, String>,
}

impl X11Probe {
    pub fn new() -> Result<Self> {
        let (conn, screen) = connect()?;
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )?
        .check()?;
        Ok(Self {
            clipboard: intern(&conn, "CLIPBOARD")?,
            targets: intern(&conn, "TARGETS")?,
            property: intern(&conn, "CLIPY_TARGETS")?,
            active_window: intern(&conn, "_NET_ACTIVE_WINDOW")?,
            conn,
            root,
            window,
            names: HashMap::new(),
        })
    }

    /// The MIME types and X targets the clipboard owner offers. Empty when nothing owns it.
    pub fn targets(&mut self) -> Result<Vec<String>> {
        self.conn.convert_selection(
            self.window,
            self.clipboard,
            self.targets,
            self.property,
            x11rb::CURRENT_TIME,
        )?;
        self.conn.flush()?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if let Some(event) = self.conn.poll_for_event()? {
                if let Event::SelectionNotify(ev) = event {
                    if ev.requestor == self.window && ev.selection == self.clipboard {
                        if ev.property == NONE {
                            return Ok(Vec::new());
                        }
                        break;
                    }
                }
                continue;
            }
            if Instant::now() > deadline {
                bail!("clipboard owner did not answer TARGETS");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let reply = self
            .conn
            .get_property(true, self.window, self.property, AtomEnum::ATOM, 0, 1024)?
            .reply()?;
        let atoms: Vec<Atom> = reply.value32().map(|v| v.collect()).unwrap_or_default();
        atoms.into_iter().map(|a| self.atom_name(a)).collect()
    }

    fn atom_name(&mut self, atom: Atom) -> Result<String> {
        if let Some(name) = self.names.get(&atom) {
            return Ok(name.clone());
        }
        let name =
            String::from_utf8_lossy(&self.conn.get_atom_name(atom)?.reply()?.name).into_owned();
        self.names.insert(atom, name.clone());
        Ok(name)
    }

    /// WM_CLASS instance and class names of the focused window, e.g.
    /// `["gnome-terminal-server", "Gnome-terminal"]`.
    pub fn active_window_class(&self) -> Vec<String> {
        self.try_active_window_class().unwrap_or_default()
    }

    fn try_active_window_class(&self) -> Result<Vec<String>> {
        let active = self
            .conn
            .get_property(false, self.root, self.active_window, AtomEnum::WINDOW, 0, 1)?
            .reply()?
            .value32()
            .and_then(|mut v| v.next())
            .filter(|&w| w != NONE)
            .ok_or_else(|| anyhow!("no active window"))?;
        let reply = self
            .conn
            .get_property(false, active, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
            .reply()?;
        Ok(reply
            .value
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect())
    }
}

const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_V: u32 = 0x0076;

/// Modifier keysyms released before the paste keystroke, like `xdotool --clearmodifiers`, so a
/// key still held from a Beta modifier action does not change the shortcut.
const MODIFIER_KEYSYMS: [u32; 11] = [
    0xffe1, 0xffe2, // Shift
    0xffe3, 0xffe4, // Control
    0xffe7, 0xffe8, // Meta
    0xffe9, 0xffea, // Alt
    0xffeb, 0xffec, // Super
    0xfe03, // ISO_Level3_Shift
];

/// Sends Ctrl+V, or Ctrl+Shift+V when `shift` is set, with XTest.
pub fn send_paste(shift: bool) -> Result<()> {
    let (conn, screen) = connect()?;
    conn.xtest_get_version(2, 2)?
        .reply()
        .context("XTest is not available")?;
    let root = conn.setup().roots[screen].root;
    let min = conn.setup().min_keycode;
    let max = conn.setup().max_keycode;
    let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = usize::from(mapping.keysyms_per_keycode).max(1);
    let rows: Vec<&[u32]> = mapping.keysyms.chunks(per).collect();
    let keycode = |sym: u32| -> Option<Keycode> {
        let index = rows
            .iter()
            .position(|row| row.first() == Some(&sym))
            .or_else(|| rows.iter().position(|row| row.contains(&sym)))?;
        Some(min + index as u8)
    };

    let held = conn.query_keymap()?.reply()?.keys;
    let is_down = |kc: Keycode| held[usize::from(kc) / 8] & (1 << (kc % 8)) != 0;
    for kc in MODIFIER_KEYSYMS.iter().filter_map(|&s| keycode(s)) {
        if is_down(kc) {
            conn.xtest_fake_input(KEY_RELEASE_EVENT, kc, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
        }
    }

    let mut combo = vec![XK_CONTROL_L];
    if shift {
        combo.push(XK_SHIFT_L);
    }
    combo.push(XK_V);
    let codes = combo
        .iter()
        .map(|&s| keycode(s).ok_or_else(|| anyhow!("no keycode for keysym {s:#x}")))
        .collect::<Result<Vec<_>>>()?;
    for &kc in &codes {
        conn.xtest_fake_input(KEY_PRESS_EVENT, kc, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    }
    for &kc in codes.iter().rev() {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, kc, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    }
    conn.sync()?;
    Ok(())
}
