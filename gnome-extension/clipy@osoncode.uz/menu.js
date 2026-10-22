// Shows the menu the daemon sends as JSON (src/menu.rs to_json) at the mouse pointer, with
// Clipy's numbered titles, previews and number-key shortcuts.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const ICON_NAMES = {
    folder: 'folder-symbolic',
    snippet: 'text-x-generic-symbolic',
};

const ClipyMenuItem = GObject.registerClass(
class ClipyMenuItem extends PopupMenu.PopupBaseMenuItem {
    _init(node) {
        super._init({style_class: 'clipy-menu-item'});
        if (node.preview) {
            const bytes = new GLib.Bytes(GLib.base64_decode(node.preview));
            const width = node.preview_w ?? 16;
            const height = node.preview_h ?? 16;
            const preview = new St.Icon({
                gicon: Gio.BytesIcon.new(bytes),
                icon_size: Math.max(width, height),
                style_class: 'clipy-preview',
            });
            preview.set_size(width, height);
            this.add_child(preview);
        } else if (ICON_NAMES[node.icon]) {
            this.add_child(new St.Icon({
                icon_name: ICON_NAMES[node.icon],
                style_class: 'popup-menu-icon',
            }));
        }
        this.label = new St.Label({
            text: node.label,
            style_class: 'clipy-label',
            y_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this.add_child(this.label);
        this.label_actor = this.label;
        if (node.accel) {
            this.add_child(new St.Label({
                text: node.accel,
                style_class: 'clipy-accel',
                x_expand: true,
                x_align: Clutter.ActorAlign.END,
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }
    }
});

export class ClipyMenu {
    /** `onActivate(itemId, modifiers)` runs when the user picks an item. */
    constructor(onActivate) {
        this._onActivate = onActivate;
        this._anchor = null;
        this._menu = null;
        this._manager = null;
    }

    show(json) {
        this.close();
        let tree;
        try {
            tree = JSON.parse(json);
        } catch (e) {
            console.error(`Clipy: bad menu from the daemon: ${e}`);
            return;
        }
        const [x, y] = global.get_pointer();
        this._anchor = new St.Widget({x, y, width: 1, height: 1, opacity: 0, reactive: false});
        Main.uiGroup.add_child(this._anchor);
        this._menu = new PopupMenu.PopupMenu(this._anchor, 0.0, St.Side.TOP);
        this._menu.actor.add_style_class_name('clipy-menu');
        Main.uiGroup.add_child(this._menu.actor);
        this._menu.actor.hide();
        this._manager = new PopupMenu.PopupMenuManager(this._anchor);
        this._manager.addMenu(this._menu);

        this._accels = new Map();
        this._fill(this._menu, tree.items ?? []);
        this._menu.actor.connect('key-press-event', (_actor, event) => this._onKey(event));
        this._menu.connect('open-state-changed', (_menu, open) => {
            if (!open)
                GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                    this.close();
                    return GLib.SOURCE_REMOVE;
                });
        });
        this._menu.open(false);
    }

    _fill(menu, nodes) {
        for (const node of nodes) {
            switch (node.type) {
            case 'header': {
                // Not focusable either: a non-reactive item that takes the key focus gets no key
                // events, so arrow navigation would stop there.
                const item = new PopupMenu.PopupMenuItem(node.label, {reactive: false, can_focus: false});
                item.add_style_class_name('clipy-header');
                menu.addMenuItem(item);
                break;
            }
            case 'separator':
                menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
                break;
            case 'submenu': {
                const item = new PopupMenu.PopupSubMenuMenuItem(node.label, ICON_NAMES[node.icon] !== undefined);
                if (ICON_NAMES[node.icon])
                    item.icon.icon_name = ICON_NAMES[node.icon];
                this._fill(item.menu, node.items ?? []);
                menu.addMenuItem(item);
                break;
            }
            case 'action': {
                const item = new PopupMenu.PopupMenuItem(node.label);
                item.setSensitive(node.enabled);
                item.connect('activate', (_item, event) => this._activate(node.id, event));
                menu.addMenuItem(item);
                break;
            }
            case 'item': {
                const item = new ClipyMenuItem(node);
                item.connect('activate', (_item, event) => this._activate(node.id, event));
                if (node.accel)
                    this._accels.set(node.accel, item);
                menu.addMenuItem(item);
                break;
            }
            }
        }
    }

    _onKey(event) {
        // Before an item has the focus, PopupMenuManager handles Down; Up starts from the bottom.
        if (event.get_key_symbol() === Clutter.KEY_Up &&
            global.stage.get_key_focus() === this._menu.actor) {
            this._menu.actor.navigate_focus(null, St.DirectionType.TAB_BACKWARD, false);
            return Clutter.EVENT_STOP;
        }
        const item = this._accels?.get(String.fromCodePoint(event.get_key_unicode() || 0));
        if (!item)
            return Clutter.EVENT_PROPAGATE;
        item.activate(event);
        return Clutter.EVENT_STOP;
    }

    _activate(itemId, event) {
        const [, , pointerMods] = global.get_pointer();
        const modifiers = event ? event.get_state() : pointerMods;
        this._onActivate(itemId, modifiers);
    }

    close() {
        this._manager?.removeMenu(this._menu);
        this._menu?.destroy();
        this._anchor?.destroy();
        this._menu = null;
        this._anchor = null;
        this._manager = null;
        this._accels = null;
    }

    destroy() {
        this.close();
        this._onActivate = null;
    }
}
