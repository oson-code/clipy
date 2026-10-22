#!/bin/sh
# Shipped as install.sh in the tar.gz package: installs Clipy from the folder it is in.
#   ./install.sh                 for you only, into ~/.local (no sudo needed)
#   sudo ./install.sh            for all users, into /usr/local
#   ./install.sh --uninstall     removes it again (with sudo if it was installed with sudo)
# PREFIX=/some/dir ./install.sh installs somewhere else.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
UUID=clipy@osoncode.uz
APP_ID=uz.osoncode.Clipy
if [ "$(id -u)" -eq 0 ]; then
    PREFIX=${PREFIX:-/usr/local}
else
    PREFIX=${PREFIX:-$HOME/.local}
fi

# The GNOME extension can be switched on only from the user's own session, and only when it is
# installed where that session looks for extensions.
own_session() {
    [ "$(id -u)" -ne 0 ] && [ "$PREFIX" = "$HOME/.local" ] &&
        command -v gnome-extensions >/dev/null 2>&1
}

# Every file in the package, relative to the prefix: bin/clipy, share/…
package_files() {
    (cd "$HERE" && find bin share -type f)
}

uninstall() {
    package_files | while read -r f; do rm -f "$PREFIX/$f"; done
    rmdir "$PREFIX/share/gnome-shell/extensions/$UUID" 2>/dev/null || true
    if own_session; then
        gnome-extensions disable "$UUID" 2>/dev/null || true
    fi
    echo "Removed Clipy from $PREFIX."
    echo "Your history and settings stay in ~/.local/share/clipy and ~/.config/clipy;"
    echo "\"Launch on login\" leaves ~/.config/autostart/$APP_ID.desktop. Delete them to remove everything."
}

install_files() {
    package_files | while read -r f; do
        mode=644
        [ -x "$HERE/$f" ] && mode=755
        # install replaces the file instead of writing into it, so a running clipy is no obstacle.
        install -D -m "$mode" "$HERE/$f" "$PREFIX/$f"
    done
    # Exec=clipy would need ~/.local/bin on PATH, which Ubuntu adds only at the next login.
    sed -i "s|^Exec=clipy\$|Exec=\"$PREFIX/bin/clipy\"|" "$PREFIX/share/applications/$APP_ID.desktop"
    # A stale icon cache would hide the new icons; without a cache, GTK reads the folder.
    icons="$PREFIX/share/icons/hicolor"
    if [ -f "$icons/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -qtf "$icons" || true
    fi
    echo "Installed Clipy in $PREFIX."
}

check_libraries() {
    missing=$(ldd "$PREFIX/bin/clipy" 2>/dev/null | awk '/not found/ {print $1}')
    # Loaded at run time, so ldd does not list it.
    if ! ldconfig -p 2>/dev/null | grep -q 'libayatana-appindicator3\.so\.1'; then
        missing="$missing libayatana-appindicator3.so.1"
    fi
    if [ -n "$missing" ]; then
        echo
        echo "Missing libraries:" $missing
        echo "Install them with: sudo apt install libgtk-3-0 libayatana-appindicator3-1"
    fi
}

next_steps() {
    echo
    if pgrep -x clipy >/dev/null 2>&1; then
        echo "Clipy is already running. Restart it to use this version: clipy --quit, then start it again."
    fi
    case ":$PATH:" in
        *":$PREFIX/bin:"*) echo "Start Clipy from the app menu, or run: clipy &" ;;
        *) echo "Start Clipy from the app menu, or run: $PREFIX/bin/clipy &" ;;
    esac
    [ "${XDG_SESSION_TYPE:-}" = wayland ] || return 0
    echo
    if own_session && gnome-extensions enable "$UUID" 2>/dev/null; then
        echo "Enabled the Clipy GNOME Shell extension."
    else
        echo "GNOME on Wayland: log out and back in, then run: gnome-extensions enable $UUID"
    fi
}

case "${1:-}" in
    --uninstall) uninstall ;;
    "")
        install_files
        check_libraries
        next_steps
        ;;
    *)
        echo "usage: $0 [--uninstall]" >&2
        exit 2
        ;;
esac
