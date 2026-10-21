#!/usr/bin/env bash
# Loads the Clipy GNOME Shell extension in a headless GNOME Shell (Wayland) and checks that it
# registers with the daemon, shows the menu, and reports clipboard changes.
# Run: docker run --rm -v "$PWD":/src -v clipy-2404-registry:/usr/local/cargo/registry \
#        -v clipy-2404-target:/src/target clipy-gnome:24.04 bash scripts/it-gnome.sh
set -uo pipefail

if [ "${1:-}" != "--inner" ]; then
    cargo build -q --bin clipy || exit 1
    export HOME="$(mktemp -d)" XDG_RUNTIME_DIR="$(mktemp -d)"
    chmod 700 "$XDG_RUNTIME_DIR"
    export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share"
    export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME
    # GNOME Shell talks to logind over the system bus at startup.
    mkdir -p /run/dbus && dbus-daemon --system --fork --nopidfile
    python3 -m dbusmock --system --template logind >/dev/null 2>&1 &
    sleep 1
    exec dbus-run-session -- "$0" --inner
fi

UUID=clipy@osoncode.uz
FAILED=0
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAILED=1; }
has_name() {
    dbus-send --session --print-reply --dest=org.freedesktop.DBus / \
        org.freedesktop.DBus.NameHasOwner "string:$1" 2>/dev/null | grep -q 'boolean true'
}
shot() {
    gdbus call --session --dest org.gnome.Shell.Screenshot --object-path /org/gnome/Shell/Screenshot \
        --method org.gnome.Shell.Screenshot.Screenshot false false "$PWD/dist/screenshots/$1.png" >/dev/null \
        && echo "saved dist/screenshots/$1.png"
}

mkdir -p "$XDG_DATA_HOME/gnome-shell/extensions/$UUID" dist/screenshots "$XDG_CONFIG_HOME/clipy"
cp gnome-extension/$UUID/* "$XDG_DATA_HOME/gnome-shell/extensions/$UUID/"
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.shell enabled-extensions "['$UUID']"
printf '[general]\nsuppress_login_alert = true\n[menu]\ninline_items = 3\nnumeric_keys = true\n' \
    >"$XDG_CONFIG_HOME/clipy/config.toml"

# Mutter starts Xwayland, which needs the usual sticky socket directory.
mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
gnome-shell --headless --wayland --virtual-monitor 1280x800 --unsafe-mode \
    >"$HOME/shell.log" 2>&1 &
for _ in $(seq 150); do has_name org.gnome.Shell && break; sleep 0.2; done
has_name org.gnome.Shell && pass "headless GNOME Shell started" || { fail "GNOME Shell did not start"; tail -30 "$HOME/shell.log"; exit 1; }
sleep 2
if ! has_name org.gnome.Shell; then
    fail "GNOME Shell exited"; grep -vE "^dbus-daemon" "$HOME/shell.log" | tail -40; exit 1
fi
STATE=$(gnome-extensions info "$UUID" 2>&1 | sed -n 's/^ *State: //p')
[ "$STATE" = "ACTIVE" ] && pass "extension is active" || fail "extension state: ${STATE:-unknown}"

# A real session exports these to every app; here they come from Mutter's runtime files.
export WAYLAND_DISPLAY=wayland-0 DISPLAY=:0
XAUTHORITY="$(ls "$XDG_RUNTIME_DIR"/.mutter-Xwaylandauth.* 2>/dev/null | head -1)"
export XAUTHORITY
RUST_LOG=clipy=debug target/debug/clipy </dev/null >"$HOME/clipy.log" 2>&1 &
for _ in $(seq 50); do grep -q "extension registered" "$HOME/clipy.log" && break; sleep 0.2; done
grep -q "extension registered" "$HOME/clipy.log" && pass "extension registered with the daemon" \
    || fail "extension did not register"

DB="$XDG_DATA_HOME/clipy/clipy.db"
shell_eval() { # runs JavaScript inside GNOME Shell, prints its result
    gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
        --method org.gnome.Shell.Eval "$1" | sed -E "s/^\(true, '\"?(.*[^\"])\"?'\)$/\1/"
}
check() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi; }

for t in "first copy" "second copy" "#FF8800"; do
    printf '%s' "$t" | xclip -selection clipboard >/dev/null 2>&1; sleep 0.8
done
check "copies captured through the extension" "3" "$(sqlite3 "$DB" 'SELECT COUNT(*) FROM history')"

# Super+Shift+V on a virtual keyboard, as if typed: goes through the extension's grab.
shell_eval "const C = imports.gi.Clutter, G = imports.gi.GLib;
    global._clipyKb ??= C.get_default_backend().get_default_seat().create_virtual_device(C.InputDeviceType.KEYBOARD_DEVICE);
    for (const k of [125, 42, 47]) global._clipyKb.notify_key(G.get_monotonic_time(), k, C.KeyState.PRESSED);
    for (const k of [47, 42, 125]) global._clipyKb.notify_key(G.get_monotonic_time(), k, C.KeyState.RELEASED);
    'sent'" >/dev/null
sleep 1
check "Super+Shift+V opens the menu" "1" "$(grep -c 'showing the main menu' "$HOME/clipy.log")"
shot gnome-menu

PICKED=$(shell_eval "const menu = Main.uiGroup.get_children().find(a => a.has_style_class_name?.('clipy-menu'))?._delegate;
    const item = menu?._getMenuItems().find(i => i.label?.text?.startsWith('2. '));
    item ? (item.activate(null), item.label.text) : 'none'")
sleep 1
check "menu item activated" "2. second copy" "$PICKED"
check "activation puts the item on the clipboard" "second copy" "$(xclip -selection clipboard -o 2>/dev/null)"
check "self-echo is not recorded" "3" "$(sqlite3 "$DB" 'SELECT COUNT(*) FROM history')"

target/debug/clipy --quit; sleep 0.5

echo "--- shell log (Clipy lines and JS errors)"
grep -iE "clipy|JS ERROR|error.*extension" "$HOME/shell.log" | head -20
echo "--- clipy log"; grep -v dbind "$HOME/clipy.log" | tail -15
[ $FAILED = 0 ] && echo "ALL PASSED" || exit 1
