#!/usr/bin/env bash
# Renders Clipy's windows and menus under Xvfb and saves PNGs to dist/screenshots, for checking
# the UI without a Linux desktop. Run inside the dev container: scripts/dev.sh bash scripts/screenshots.sh
set -uo pipefail

if [ "${1:-}" != "--inner" ]; then
    cargo build -q --bin clipy || exit 1
    export HOME="$(mktemp -d)"
    export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share" XDG_SESSION_TYPE=x11
    Xvfb :98 -screen 0 1100x760x24 -nolisten tcp </dev/null >/dev/null 2>&1 &
    XVFB=$!
    export DISPLAY=:98
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    dbus-run-session -- "$0" --inner
    STATUS=$?
    pkill -x xclip 2>/dev/null
    kill $XVFB 2>/dev/null
    exit $STATUS
fi

OUT="$PWD/dist/screenshots"
mkdir -p "$OUT" "$XDG_CONFIG_HOME/clipy"
printf '[general]\nsuppress_login_alert = true\n[menu]\ninline_items = 3\nnumeric_keys = true\n' \
    >"$XDG_CONFIG_HOME/clipy/config.toml"
target/debug/clipy </dev/null >"$HOME/clipy.log" 2>&1 &
sleep 2
copy() { printf '%s' "$1" | xclip -selection clipboard ${2:+-t "$2"} >/dev/null 2>&1; sleep 0.4; }
for i in $(seq 1 14); do copy "Clipboard entry number $i with some longer text to see truncation"; done
copy "#3A86FF"
xclip -selection clipboard -t image/png -i resources/icons/app_128.png >/dev/null 2>&1; sleep 0.6
copy "fn main() { println!(\"hello\"); }"
shot() { sleep "${2:-0.8}"; import -window root "$OUT/$1.png"; echo "saved $OUT/$1.png"; }

xdotool mousemove 120 80
target/debug/clipy --popup main; shot menu-main
xdotool key Down Down Down Down Right; shot menu-submenu 0.5
xdotool key Escape Escape Escape; sleep 0.3

target/debug/clipy --preferences; shot preferences-general 1.5
WID=$(xdotool search --name "^General$" | head -1)
if [ -n "$WID" ]; then
    eval "$(xdotool getwindowgeometry --shell "$WID")"
    # Focus the first toolbar button; arrow keys then step through the panes.
    xdotool mousemove $((X + WIDTH / 2 - 200)) $((Y + 30)) click 1
    for pane in menu type exclude shortcuts beta; do
        xdotool key Right space
        shot "preferences-$pane" 0.8
    done
    xdotool windowclose "$WID" 2>/dev/null || xdotool key --window "$WID" alt+F4
fi

target/debug/clipy --snippets; shot snippet-editor-folder 1.5
WID=$(xdotool search --name "Snippets Editor" | head -1)
if [ -n "$WID" ]; then
    eval "$(xdotool getwindowgeometry --shell "$WID")"
    # Focus the tree through an empty spot (a click on the selected row would start a rename).
    xdotool mousemove $((X + 60)) $((Y + 300)) click 1
    xdotool key Home shift+Right Down; shot snippet-editor-snippet 0.6
fi
target/debug/clipy --quit; sleep 0.5
grep -E "WARN|ERROR|panicked" "$HOME/clipy.log" | grep -v dbind || true
