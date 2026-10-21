//! Sends the paste keystroke to the focused window after a menu selection.
//!
//! X11: XTest in-process, then `xdotool`. Wayland compositors without the GNOME extension:
//! `wtype` (wlroots), then `ydotool`. Each missing tool is skipped; when none works the item
//! simply stays on the clipboard.

use std::io::ErrorKind;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use crate::x11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteCombo {
    CtrlV,
    CtrlShiftV,
}

impl PasteCombo {
    /// The form used on D-Bus and in the GNOME extension.
    pub fn as_str(self) -> &'static str {
        match self {
            PasteCombo::CtrlV => "ctrl+v",
            PasteCombo::CtrlShiftV => "ctrl+shift+v",
        }
    }
}

/// Terminals paste with Ctrl+Shift+V; everything else with Ctrl+V.
pub fn combo_for(app: &[String], terminals: &[String]) -> PasteCombo {
    let stem = |s: &str| s.strip_suffix(".desktop").unwrap_or(s).to_ascii_lowercase();
    let is_terminal = app
        .iter()
        .any(|a| terminals.iter().any(|t| stem(t) == stem(a)));
    if is_terminal {
        PasteCombo::CtrlShiftV
    } else {
        PasteCombo::CtrlV
    }
}

/// Waits `delay` for focus to return to the target window, then sends `combo` on a background
/// thread. `on_failure` runs on that thread when no injection method works.
pub fn paste_later(
    combo: PasteCombo,
    delay: Duration,
    on_failure: impl FnOnce(String) + Send + 'static,
) {
    let spawned = thread::Builder::new()
        .name("clipy-paste".into())
        .spawn(move || {
            thread::sleep(delay);
            match inject(combo) {
                Ok(method) => log::debug!("pasted with {method}"),
                Err(e) => on_failure(format!("{e:#}")),
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start the paste thread: {e}");
    }
}

fn inject(combo: PasteCombo) -> Result<&'static str> {
    let shift = combo == PasteCombo::CtrlShiftV;
    let mut errors = Vec::new();
    if is_wayland_session() {
        let (wtype, ydotool): (&[&str], &[&str]) = if shift {
            (
                &[
                    "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
                ],
                &["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"],
            )
        } else {
            (
                &["-M", "ctrl", "-k", "v", "-m", "ctrl"],
                &["key", "29:1", "47:1", "47:0", "29:0"],
            )
        };
        for (tool, args) in [("wtype", wtype), ("ydotool", ydotool)] {
            match run_tool(tool, args) {
                Ok(()) => return Ok(tool),
                Err(e) => errors.push(format!("{tool}: {e:#}")),
            }
        }
    } else if x11::display_available() {
        match x11::send_paste(shift) {
            Ok(()) => return Ok("XTest"),
            Err(e) => errors.push(format!("XTest: {e:#}")),
        }
        let keys = if shift { "ctrl+shift+v" } else { "ctrl+v" };
        match run_tool("xdotool", &["key", "--clearmodifiers", keys]) {
            Ok(()) => return Ok("xdotool"),
            Err(e) => errors.push(format!("xdotool: {e:#}")),
        }
    }
    if errors.is_empty() {
        bail!("no display to paste into");
    }
    Err(anyhow!(errors.join("; ")))
}

fn run_tool(tool: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(tool)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => bail!("exited with {s}"),
        Err(e) if e.kind() == ErrorKind::NotFound => bail!("not installed"),
        Err(e) => Err(e.into()),
    }
}

/// GNOME (and Ubuntu's `ubuntu:GNOME`) by `XDG_CURRENT_DESKTOP`.
pub fn is_gnome_session() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("GNOME")))
}

pub fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_get_ctrl_shift_v() {
        let terminals = vec![
            "gnome-terminal-server".to_string(),
            "org.gnome.Ptyxis".to_string(),
        ];
        let class = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            combo_for(
                &class(&["gnome-terminal-server", "Gnome-terminal"]),
                &terminals
            ),
            PasteCombo::CtrlShiftV
        );
        assert_eq!(
            combo_for(&class(&["org.gnome.Ptyxis.desktop"]), &terminals),
            PasteCombo::CtrlShiftV
        );
        assert_eq!(
            combo_for(&class(&["gedit", "Gedit"]), &terminals),
            PasteCombo::CtrlV
        );
        assert_eq!(combo_for(&[], &terminals), PasteCombo::CtrlV);
    }

    #[test]
    fn missing_tool_is_reported() {
        let err = run_tool("clipy-no-such-tool", &[]).unwrap_err();
        assert!(err.to_string().contains("not installed"));
    }
}
