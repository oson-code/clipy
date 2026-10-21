# Clipy for Linux

A clipboard history and snippet manager for Ubuntu and Linux Mint, ported from
[Clipy](https://github.com/Clipy/Clipy) for macOS. It keeps the same menus, settings and
snippet editor:

- the copy history appears in a menu at the mouse pointer (default <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>) and in the panel
- choosing an item pastes it into the app you were using
- snippets are organised in folders, with Clipy-compatible XML import and export

It is written in Rust. History is stored in SQLite (`~/.local/share/clipy/clipy.db`, readable by
you only) and settings in `~/.config/clipy/config.toml`.

## What works where

| Session | Shortcuts | Menu at the pointer | Automatic paste | Clipboard capture |
|---|---|---|---|---|
| X11: Linux Mint Cinnamon, Ubuntu on Xorg, XFCE, MATE, KDE X11 | yes | yes (GTK menu) | yes (XTest, else `xdotool`) | yes (XFixes events) |
| GNOME on Wayland 45–50 (Ubuntu 24.04, 25.x, 26.04) **with the Clipy GNOME Shell extension** | yes | yes (Shell menu) | yes (virtual keyboard) | yes |
| GNOME on Wayland without the extension | no | panel menu only | no | yes |
| KDE Plasma / wlroots on Wayland (partial) | not yet (needs the GlobalShortcuts portal) | panel menu only | `ydotool`, or `wtype` on wlroots | text, polled every 500 ms |

Ubuntu 22.04 runs GNOME 42, which the extension does not support. Choose "Ubuntu on Xorg" at the
login screen there.

## Install

Build dependencies (Ubuntu 22.04+, Mint 21+):

```bash
sudo apt install build-essential pkg-config libgtk-3-dev libayatana-appindicator3-dev libxdo-dev
curl https://sh.rustup.rs -sSf | sh     # Rust 1.90 or newer (Ubuntu's packaged rustc is too old)
```

Build and install:

```bash
make build
sudo make install          # /usr/local/bin/clipy, desktop entry and icons
clipy &                    # first start asks whether to launch on login
```

On GNOME Wayland, also install the extension. Log out and back in, then enable it:

```bash
make install-extension
gnome-extensions enable clipy@osoncode.uz
```

Ubuntu and Mint ship the AppIndicator support that the panel icon needs. Vanilla GNOME needs the
"AppIndicator and KStatusNotifierItem Support" extension.

A `.deb` package can be built with `cargo install cargo-deb && make deb`. It installs the
extension system-wide. Build it on the oldest release you target: a package built on 24.04 needs
glibc 2.39, so it will not install on 22.04.

## Use

| Default shortcut | Action |
|---|---|
| <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> | main menu (history and snippets) |
| <kbd>Super</kbd>+<kbd>Ctrl</kbd>+<kbd>V</kbd> | history menu |
| <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>B</kbd> | snippet menu |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>Super</kbd>+<kbd>V</kbd> | Preferences (cannot be removed, so the app stays reachable when the panel icon is hidden) |

Shortcuts are changed in Preferences → Shortcuts. Snippet folders can have their own shortcut
in the snippet editor.

Behaviour that matches Clipy:
- Terminals receive <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>. The list of terminals is `advanced.terminal_wm_classes` in the config file.
- Hold a modifier while choosing an item to paste plain text, delete the item, or paste and delete it. The key is <kbd>Ctrl</kbd> by default (Clipy's <kbd>⌘</kbd>). Set this up in Preferences → Beta.
- The panel icon is "Standard" by default and follows the system theme as it changes. Black, White or Hidden can be picked in Preferences → General.
- "Save screenshots in history" (Beta) watches `~/Pictures` and `~/Pictures/Screenshots` for PNGs named `Screenshot…`. The folder and the name prefix can be changed there, or in `[beta] screenshot_dirs` and `screenshot_prefix` in the config file.
- Number keys pick the first ten items while a menu is open. Turn this on in Preferences → Menu.

Command line, which drives the running instance over D-Bus:

```text
clipy                      start, or open Preferences if already running
clipy --popup KIND         main | history | snippets | folder:ID
clipy --snippets           open the snippet editor
clipy --clear-history
clipy --quit
```

## Differences from Clipy for macOS

These macOS-only features are not ported:
- Universal Clipboard
- RTF, RTFD and PDF clipboard types
- OCR
- Sparkle updates
- the Diagnostics and Donate panes

Platform limitations:
- Panel and GNOME Shell menus cannot show tooltips; the X11 menu can.
- On Wayland without the GNOME extension, the app that copied is unknown, so the Exclude list cannot apply.

Linux defaults that differ:
- History limit 50 and title length 35 (Clipy uses 30 and 20). These come from this project's spec.
- Global shortcuts use <kbd>Super</kbd> where Clipy uses <kbd>⌘</kbd>. <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> is the paste key in Linux terminals. The "hold a key" actions use <kbd>Ctrl</kbd>.

Open questions about the port are tracked in [ASSUMPTIONS.md](ASSUMPTIONS.md).

## Development

All builds and tests run in an Ubuntu container, so a Linux machine is not needed:

```bash
scripts/dev.sh image        # build the dev image (UBUNTU=22.04 for the old LTS)
scripts/dev.sh check        # rustfmt, clippy -D warnings, unit tests
scripts/dev.sh it           # X11 integration tests under Xvfb (CPU_CHECK=1 adds an idle-CPU check)
scripts/dev.sh bash scripts/screenshots.sh   # renders menus and windows to dist/screenshots
docker build -t clipy-gnome:24.04 -f docker/Dockerfile.gnome docker
docker run --rm -v "$PWD":/src -v clipy-2404-registry:/usr/local/cargo/registry \
    -v clipy-2404-target:/src/target clipy-gnome:24.04 bash scripts/it-gnome.sh
                            # the GNOME extension in a headless GNOME Shell 46
```

| Source | Role |
|---|---|
| `src/app.rs` | event routing on the GTK main thread |
| `src/clipboard_watcher.rs` | capture thread: XFixes events, the extension's events, or polling |
| `src/db.rs` | SQLite schema and queries |
| `src/menu.rs` | Clipy's menu layout, independent of toolkit |
| `src/tray.rs`, `src/popup_gtk.rs` | menu rendering in the panel and on X11 |
| `src/hotkeys.rs`, `src/paste.rs`, `src/x11.rs` | X11 key grabs, XTest paste, TARGETS |
| `src/dbus.rs` | single instance, the CLI, and the API the extension uses |
| `src/ui/` | Preferences, snippet editor and dialogs |
| `gnome-extension/` | GNOME Shell companion |

### Manual test checklist (real desktops)

Run through this on Ubuntu 24.04 Xorg, Ubuntu 24.04/26.04 Wayland with the extension, and
Linux Mint 22 Cinnamon:

- [ ] The shortcut opens the menu at the pointer. Choosing an item pastes into a text editor, and into a terminal.
- [ ] The panel menu shows the same items, and choosing one pastes.
- [ ] Numbering, the inline/folder split and previews (an image, a `#RRGGBB` colour) look right.
- [ ] Preferences changes apply immediately: shortcuts, panel icon, history limit.
- [ ] The three Beta modifier actions work.
- [ ] Snippets: add, rename, drag and drop, enable/disable, folder shortcut, and XML import of a Clipy export.
- [ ] Clear History shows the warning, and "Do not show this message again" sticks.
- [ ] "Launch on Login" survives logging out and back in.
- [ ] With "Save screenshots in history" on, a new screenshot appears in the history.
- [ ] Copying from KeePassXC is skipped when "Do not save content marked as confidential" is on.
- [ ] Pasting a copied file works in Nautilus and Nemo.
- [ ] "Standard" panel icon is visible with light and dark themes and switches when the theme changes (ASSUMPTIONS.md A-014).

## Credits

Clipy for macOS by the Clipy Project, MIT license. The status bar and app icons and the
translations (de, it, ja, pt-BR, uk, zh-Hans) come from it; see `resources/icons/LICENSE-Clipy`.
