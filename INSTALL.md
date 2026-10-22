# Installing Clipy for Linux

Clipy keeps your clipboard history and snippets. Press <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>
to open the menu at the mouse pointer, or click the clipboard icon in the top panel.

## Which file to download

| File | Use it when |
|---|---|
| `clipy_0.1.0_amd64.deb` | Ubuntu, Linux Mint, Pop!_OS, Debian. Easiest: apt installs everything it needs. |
| `clipy-0.1.0-linux-x86_64.tar.gz` | You want to install for your user only, without sudo. |

Both are for 64-bit Intel/AMD PCs (x86_64).

## Supported systems

| System | What works |
|---|---|
| Ubuntu 24.04, 25.04, 25.10, 26.04 (GNOME on Wayland, the default) | Everything, once the Clipy GNOME extension is enabled (step 2 below) |
| Ubuntu 22.04 and newer, "Ubuntu on Xorg" session | Everything, no extension needed |
| Linux Mint 21 and 22 (Cinnamon, MATE, Xfce) | Everything |
| KDE Plasma, Xfce, MATE on X11 | Everything |
| KDE Plasma or wlroots desktops on Wayland | Panel icon menu only, no shortcuts |

Ubuntu 22.04 runs GNOME 42, which the extension does not support. Pick "Ubuntu on Xorg" with the
gear button on the login screen there.

## Install the .deb package

1. Open a terminal in the folder with the download and run:

   ```bash
   sudo apt install ./clipy_0.1.0_amd64.deb
   ```

   Keep the `./`: it tells apt to use the file instead of searching online for it.

2. **GNOME on Wayland only** (Ubuntu 24.04 and newer): log out and back in, then run

   ```bash
   gnome-extensions enable clipy@osoncode.uz
   ```

   or switch on "Clipy" in the Extensions app. Without it, Clipy works only from the panel icon.

3. Start Clipy from the app menu (search for "Clipy"), or run `clipy &`. The first time, it asks
   whether to start at login.

## Install the .tar.gz package

1. Unpack it and run the installer:

   ```bash
   tar xzf clipy-0.1.0-linux-x86_64.tar.gz
   cd clipy-0.1.0-linux-x86_64
   ./install.sh
   ```

   This installs for your user only, into `~/.local`, without sudo. To install for all users
   into `/usr/local`, run `sudo ./install.sh` instead.

2. If the installer reports missing libraries, install them:

   ```bash
   sudo apt install libgtk-3-0 libayatana-appindicator3-1
   ```

3. **GNOME on Wayland only**: the installer enables the extension when it can. If it says so,
   log out and back in, then run `gnome-extensions enable clipy@osoncode.uz`.

4. Start Clipy from the app menu, or run `clipy &`.

## Check the download

`SHA256SUMS` lists the checksum of each file. In the download folder, run:

```bash
sha256sum -c --ignore-missing SHA256SUMS
```

Each file you downloaded should say `OK`.

## Update

Install the new version the same way as the first time, over the old one. Then restart Clipy:

```bash
clipy --quit
clipy &
```

On GNOME Wayland, log out and back in so GNOME Shell loads the new extension.

## Uninstall

| Installed with | Remove with |
|---|---|
| `.deb` | `sudo apt remove clipy` |
| `./install.sh` | `./install.sh --uninstall`, from the unpacked folder |
| `sudo ./install.sh` | `sudo ./install.sh --uninstall` |

Your history and settings are kept. To delete them too:

```bash
rm -rf ~/.local/share/clipy ~/.config/clipy ~/.config/autostart/uz.osoncode.Clipy.desktop
```

## Troubleshooting

**No icon in the top panel.** Ubuntu and Mint show it out of the box. Other GNOME systems (Fedora,
Debian) need the "AppIndicator and KStatusNotifierItem Support" extension. Also check
Preferences → General → Panel icon is not set to Hidden.

**The panel icon is hard to see.** Open Preferences (<kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>Super</kbd>+<kbd>V</kbd>,
or run `clipy --preferences`) and pick White for a dark panel or Black for a light one under
General → Panel icon.

**The shortcut does nothing on Ubuntu 24.04 or newer.** The GNOME extension is not enabled. Check
with `gnome-extensions info clipy@osoncode.uz`: it should say `State: ACTIVE`. If it says the
extension does not exist, log out and back in first.

**The item is copied but not pasted.** On X11, install `xdotool` (`sudo apt install xdotool`).
On GNOME Wayland, enable the extension. The item is always on the clipboard, so
<kbd>Ctrl</kbd>+<kbd>V</kbd> pastes it by hand.

**An old version still starts after updating.** An earlier `sudo make install` left a copy in
`/usr/local/bin`, which comes first on the PATH. Remove it with
`sudo rm /usr/local/bin/clipy /usr/local/share/applications/uz.osoncode.Clipy.desktop`.

**See what Clipy is doing.** Quit it and start it from a terminal; it prints what it does
there. `RUST_LOG=debug` prints more:

```bash
clipy --quit
RUST_LOG=debug clipy
```
