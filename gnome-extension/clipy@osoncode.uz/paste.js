// Types the paste keystroke into the focused window through a virtual keyboard, which is
// the only way to synthesize keys on GNOME Wayland.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';

// Linux evdev key codes. Physical keys, so Ctrl+V works whatever the keyboard layout.
const KEY_LEFTCTRL = 29;
const KEY_LEFTSHIFT = 42;
const KEY_V = 47;

export class Paster {
    constructor() {
        const seat = Clutter.get_default_backend().get_default_seat();
        this._keyboard = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
        this._timeouts = new Set();
    }

    /** Sends `combo` ("ctrl+v" or "ctrl+shift+v") after `delayMs`. */
    paste(combo, delayMs = 50) {
        const keys = combo === 'ctrl+shift+v'
            ? [KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_V]
            : [KEY_LEFTCTRL, KEY_V];
        const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, delayMs, () => {
            this._timeouts.delete(id);
            if (!this._keyboard)
                return GLib.SOURCE_REMOVE;
            for (const key of keys)
                this._keyboard.notify_key(GLib.get_monotonic_time(), key, Clutter.KeyState.PRESSED);
            for (const key of [...keys].reverse())
                this._keyboard.notify_key(GLib.get_monotonic_time(), key, Clutter.KeyState.RELEASED);
            return GLib.SOURCE_REMOVE;
        });
        this._timeouts.add(id);
    }

    destroy() {
        for (const id of this._timeouts)
            GLib.source_remove(id);
        this._timeouts.clear();
        this._keyboard?.run_dispose();
        this._keyboard = null;
    }
}
