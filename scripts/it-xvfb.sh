#!/usr/bin/env bash
# Headless integration test: runs clipy under Xvfb with a private session bus and checks
# clipboard capture, the confidential filter, self-echo, trimming and single-instance forwarding.
# Run inside the dev container: scripts/dev.sh it
set -uo pipefail

if [ "${1:-}" != "--inner" ]; then
    set -e
    cargo build -q --bin clipy --example clip_secret
    [ -n "${CLIPY_BIN:-}" ] && cargo build -q --release --bin clipy
    export HOME="$(mktemp -d)"
    export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share" XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY
    # Background helpers must not hold our stdout, or a pipe reading this script never ends.
    Xvfb :99 -screen 0 1280x800x24 -nolisten tcp </dev/null >/dev/null 2>&1 &
    XVFB=$!
    export DISPLAY=:99
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    set +e
    dbus-run-session -- "$0" --inner
    STATUS=$?
    pkill -x xclip 2>/dev/null
    kill $XVFB 2>/dev/null
    exit $STATUS
fi

BIN="$PWD/target/debug/clipy"
SECRET="$PWD/target/debug/examples/clip_secret"
DB="$XDG_DATA_HOME/clipy/clipy.db"
CONF="$XDG_CONFIG_HOME/clipy/config.toml"
NAME=uz.osoncode.Clipy
FAILED=0
PID=

pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAILED=1; }
check() { # check DESCRIPTION EXPECTED ACTUAL
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
sql() { sqlite3 "$DB" "$1"; }
copy() { printf '%s' "$1" | xclip -selection clipboard ${2:+-t "$2"} >/dev/null 2>&1; sleep 0.4; }
has_owner() {
    dbus-send --session --print-reply --dest=org.freedesktop.DBus / \
        org.freedesktop.DBus.NameHasOwner "string:$NAME" 2>/dev/null | grep -q 'boolean true'
}
call() { # call METHOD [ARGS...]
    dbus-send --session --print-reply --dest=$NAME /uz/osoncode/Clipy \
        "uz.osoncode.Clipy1.$1" "${@:2}"
}
start() {
    RUST_LOG=clipy=debug "${CLIPY_BIN:-$BIN}" </dev/null >>"$HOME/clipy.log" 2>&1 &
    PID=$!
    for _ in $(seq 100); do has_owner && return 0; sleep 0.1; done
    echo "--- clipy log"; cat "$HOME/clipy.log"; exit 1
}
stop() {
    "$BIN" --quit
    for _ in $(seq 50); do kill -0 "$PID" 2>/dev/null || return 0; sleep 0.1; done
    fail "clipy did not exit on --quit"; kill "$PID"
}
write_config() { # write_config IGNORE_CONFIDENTIAL
    mkdir -p "$(dirname "$CONF")" "$HOME/shots"
    cat >"$CONF" <<TOML
[general]
max_history = 3
suppress_login_alert = true
[menu]
confirm_clear = false
[types]
ignore_confidential = $1
[beta]
observe_screenshots = true
screenshot_dirs = ["~/shots"]
screenshot_prefix = "shot"
TOML
}

write_config false
start
check "database is private" "600" "$(stat -c %a "$DB")"
check "Common folder seeded" "Common" "$(sql 'SELECT name FROM snippet_folders')"

copy "hello world"
check "text captured" "text|hello world" "$(sql 'SELECT data_type, text_content FROM history ORDER BY id DESC LIMIT 1')"

xclip -selection clipboard -t image/png -i resources/icons/app_16.png >/dev/null 2>&1; sleep 0.6
check "image captured" "image|16x16" "$(sql "SELECT data_type, image_width || 'x' || image_height FROM history ORDER BY id DESC LIMIT 1")"

copy '<b>bold</b>' text/html
check "html without text captured" "html|<b>bold</b>" "$(sql 'SELECT data_type, html_content FROM history ORDER BY id DESC LIMIT 1')"

"$SECRET" "password-1" >/dev/null 2>&1 & SPID=$!; sleep 0.6
check "secret saved when the filter is off" "1" "$(sql "SELECT COUNT(*) FROM history WHERE text_content = 'password-1'")"
kill $SPID 2>/dev/null || true

copy "one"; copy "two"; copy "three"; copy "four"
check "history trimmed to the limit" "3" "$(sql 'SELECT COUNT(*) FROM history')"
check "newest first" "four three two" "$(sql 'SELECT text_content FROM history ORDER BY updated_at DESC, id DESC' | tr '\n' ' ' | sed 's/ $//')"

copy "four"
check "re-copy does not duplicate" "3" "$(sql 'SELECT COUNT(*) FROM history')"

ID=$(sql "SELECT id FROM history WHERE text_content = 'two'")
REPLY=$(call ActivateItem "string:h:$ID" uint32:0 string:gnome-terminal-server 2>&1 | sed -n 's/.*string "\(.*\)"/\1/p')
sleep 0.6
check "activate returns the terminal paste keys" "ctrl+shift+v" "$REPLY"
check "activate puts the item on the clipboard" "two" "$(xclip -selection clipboard -o)"
check "self-echo is not recorded again" "3" "$(sql 'SELECT COUNT(*) FROM history')"
check "pasted item moves to the top" "two" "$(sql 'SELECT text_content FROM history ORDER BY updated_at DESC, id DESC LIMIT 1')"

SNIP=$(sql 'SELECT id FROM snippets LIMIT 1')
call ActivateItem "string:s:$SNIP" uint32:0 string: >/dev/null; sleep 0.6
check "snippet pasted and recorded" "Example snippet" "$(sql "SELECT COUNT(*) FROM history WHERE text_content LIKE 'This is an example snippet%'" | sed 's/^1$/Example snippet/')"

call Popup string:main >/dev/null; sleep 0.5
xdotool key Escape; sleep 0.2
kill -0 "$PID" && pass "popup menu opens and closes" || fail "clipy died showing the popup"

shortcut() { # shortcut KEYS MENU
    local before; before=$(grep -c "showing the $2 menu" "$HOME/clipy.log")
    xdotool key "$1"; sleep 0.5; xdotool key Escape; sleep 0.2
    check "shortcut $1 opens the $2 menu" "$((before + 1))" "$(grep -c "showing the $2 menu" "$HOME/clipy.log")"
}
shortcut super+shift+v main
shortcut super+ctrl+v history
shortcut super+shift+b snippets
BEFORE=$(grep -c "showing the main menu" "$HOME/clipy.log")
xdotool keydown super+shift+v; sleep 1.5; xdotool keyup super+shift+v; sleep 0.5; xdotool key Escape
check "holding a shortcut opens the menu once" "$((BEFORE + 1))" "$(grep -c "showing the main menu" "$HOME/clipy.log")"

"$BIN" --preferences; sleep 1.2
WID=$(xdotool search --name "^General$" | head -1)
if [ -n "$WID" ]; then
    xdotool windowfocus "$WID"; sleep 0.2
    # Focus starts on the first of the six pane buttons; the General pane's first control
    # comes after them.
    xdotool key Tab Tab Tab Tab Tab Tab space; sleep 0.6
    check "preferences change is saved" "true" "$(sed -n 's/^launch_at_login = //p' "$CONF")"
    check "launch on login writes the autostart entry" "yes" \
        "$([ -f "$XDG_CONFIG_HOME/autostart/uz.osoncode.Clipy.desktop" ] && echo yes || echo no)"
    xdotool key space; sleep 0.6
    check "turning it off removes the entry" "no" \
        "$([ -f "$XDG_CONFIG_HOME/autostart/uz.osoncode.Clipy.desktop" ] && echo yes || echo no)"
    xdotool windowclose "$WID" 2>/dev/null
else
    fail "Preferences window did not open"
fi

"$BIN" --clear-history; sleep 0.5
check "second instance forwards --clear-history" "0" "$(sql 'SELECT COUNT(*) FROM history')"
stop

write_config true
start
"$SECRET" "password-2" >/dev/null 2>&1 & SPID=$!; sleep 0.6
check "secret skipped when the filter is on" "0" "$(sql "SELECT COUNT(*) FROM history WHERE text_content = 'password-2'")"
kill $SPID 2>/dev/null || true
copy "after secret"
check "normal copies still saved" "after secret" "$(sql 'SELECT text_content FROM history ORDER BY id DESC LIMIT 1')"

cp resources/icons/app_32.png "$HOME/shots/holiday.png"; sleep 0.8
check "other PNGs in the screenshot folder are ignored" "0" "$(sql "SELECT COUNT(*) FROM history WHERE data_type = 'image'")"
cp resources/icons/app_32.png "$HOME/shots/shot-001.png"; sleep 0.8
check "a new screenshot in the configured folder is saved" "image|32x32" \
    "$(sql "SELECT data_type, image_width || 'x' || image_height FROM history ORDER BY id DESC LIMIT 1")"

if [ "${CPU_CHECK:-0}" = 1 ]; then
    sleep 2
    CPU=$(pidstat -p "$PID" 1 30 | awk '/Average/ {print $8}')
    RSS=$(ps -o rss= -p "$PID")
    echo "INFO  idle CPU ${CPU}% over 30 s, RSS $((RSS / 1024)) MB"
    awk -v c="$CPU" 'BEGIN { exit !(c < 0.5) }' && pass "idle CPU below 0.5%" || fail "idle CPU ${CPU}%"
fi
stop

if [ $FAILED = 0 ]; then echo "ALL PASSED"; else echo "--- clipy log"; tail -40 "$HOME/clipy.log"; exit 1; fi
