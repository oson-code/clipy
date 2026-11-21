// Shows the menu the daemon sends as JSON (src/menu.rs to_json) at the mouse pointer, with
// Clipy's numbered titles, previews and number-key shortcuts. Folders open in their own popup
// beside the row, as in Clipy on macOS, so opening one never resizes the main menu.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Shell from 'gi://Shell';
import St from 'gi://St';

import {ensureActorVisibleInScrollView} from 'resource:///org/gnome/shell/misc/animationUtils.js';
import * as BoxPointer from 'resource:///org/gnome/shell/ui/boxpointer.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const ICON_NAMES = {
    folder: 'folder-symbolic',
    snippet: 'text-x-generic-symbolic',
};

// How long the pointer rests on a row before its folder opens, or before another row closes the
// open folder. Long enough to cross a row on the way into the open popup.
const HOVER_DELAY_MS = 150;

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

// A folder row: opens its popup (`this.submenu`) to the side instead of activating.
const ClipyFolderItem = GObject.registerClass(
class ClipyFolderItem extends PopupMenu.PopupBaseMenuItem {
    _init(node, onOpen) {
        super._init({style_class: 'clipy-folder'});
        this._onOpen = onOpen;
        this.submenu = null;
        if (ICON_NAMES[node.icon]) {
            this.add_child(new St.Icon({
                icon_name: ICON_NAMES[node.icon],
                style_class: 'popup-menu-icon',
            }));
        }
        this.label = new St.Label({
            text: node.label,
            y_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this.add_child(this.label);
        this.label_actor = this.label;
        const arrow = PopupMenu.arrowIcon(St.Side.RIGHT);
        arrow.x_expand = true;
        arrow.x_align = Clutter.ActorAlign.END;
        this.add_child(arrow);
    }

    // A click, Return or Space: the base class would emit 'activate', and the menu would close.
    activate(_event) {
        this._onOpen(this, true);
    }
});

export class ClipyMenu {
    /** `onActivate(itemId, modifiers)` runs when the user picks an item. */
    constructor(onActivate) {
        this._onActivate = onActivate;
        this._anchor = null;
        this._menu = null;
        this._grab = null;
        this._submenus = null;
        this._hoverTimeoutId = 0;
        // The main menu and every folder popup live in one layer that holds the modal grab, so the
        // pointer and the key focus can move between them. GNOME's PopupMenuManager keeps only one
        // menu open at a time, which rules out a folder open beside its parent. BoxPointer places
        // itself through its parent's transform, which is only valid once the parent has been
        // allocated, so the layer covers the stage and stays for the extension's lifetime.
        this._layer = new St.Widget({style_class: 'clipy-menu-layer', reactive: false});
        this._layer.add_constraint(new Clutter.BindConstraint({
            source: global.stage,
            coordinate: Clutter.BindCoordinate.ALL,
        }));
        this._layer.connect('captured-event', (_actor, event) => this._onCapturedEvent(event));
        Main.uiGroup.add_child(this._layer);
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
        const workArea = Main.layoutManager.getWorkAreaForMonitor(global.display.get_current_monitor());
        this._anchor = new St.Widget({x, y, width: 1, height: 1, opacity: 0, reactive: false});
        Main.uiGroup.add_child(this._anchor);
        Main.uiGroup.set_child_above_sibling(this._layer, null);
        this._menu = new PopupMenu.PopupMenu(this._anchor, 0.0, St.Side.TOP);
        this._initMenu(this._menu);

        this._accels = new Map();
        this._submenus = [];
        this._fill(this._menu, tree.items ?? []);
        this._menu.connect('open-state-changed', (menu, open) => {
            if (open)
                return;
            if (menu.openChild)
                this._closeSubmenu(menu.openChild);
            GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                // Not a menu shown since: show() closes the previous one first.
                if (this._menu === menu)
                    this.close();
                return GLib.SOURCE_REMOVE;
            });
        });
        this._grab = Main.pushModal(this._layer, {actionMode: Shell.ActionMode.POPUP});
        this._place(y, workArea);
        this._menu.open(false);
        this._menu.actor.grab_key_focus();
    }

    _initMenu(menu) {
        menu.actor.add_style_class_name('clipy-menu');
        this._layer.add_child(menu.actor);
        menu.actor.hide();
        menu.openChild = null;
        menu.actor.connect('key-press-event', (_actor, event) => this._onKey(menu, event));
        menu.connect('active-changed', (_menu, item) => this._onActiveChanged(menu, item));
    }

    // BoxPointer opens the menu below the pointer and moves it above only when it fits there
    // whole; it never pushes the menu back onto the screen. Pick the side here, slide the menu
    // up when it fits on neither side, and cap its height so it never runs off the screen.
    _place(y, workArea) {
        const actor = this._menu.actor;
        const gap = actor.get_theme_node().get_length('-boxpointer-gap');
        const [, natHeight] = actor.get_preferred_height(-1);
        const height = natHeight + gap;
        const top = workArea.y;
        const bottom = workArea.y + workArea.height;
        // The anchor is 1px tall, and BoxPointer flips only when the menu is strictly smaller
        // than the space above, so the limits below keep a pixel to spare.
        const below = bottom - (y + 1);
        const above = y - top;
        let limit;
        if (height < below) {
            limit = below;
        } else if (height < above) {
            limit = above;
        } else {
            this._anchor.y = Math.max(top, bottom - 1 - height);
            limit = bottom - 1 - this._anchor.y;
        }
        const scaleFactor = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        actor.style = `max-height: ${Math.floor((limit - gap - 1) / scaleFactor)}px;`;
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
                const item = new ClipyFolderItem(node, (folder, focus) => this._openFolder(folder, focus));
                menu.addMenuItem(item);
                item.submenu = this._makeSubmenu(item, menu, node.items ?? []);
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

    // A folder's popup opens to the right of its row (BoxPointer flips it to the left when there is
    // no room) and lines up with the row, kept inside the work area. A long folder scrolls.
    _makeSubmenu(item, parent, nodes) {
        const submenu = new PopupMenu.PopupMenu(item, 0.0, St.Side.LEFT);
        // PopupMenu toggles itself on its source's Right, Return and Space; _openFolder and _onKey
        // do that here, keeping track of which folder is open.
        item.disconnectObject(submenu);
        submenu.setSourceAlignment(0.0);
        // An activated row closes the menu at the top of the chain: the main menu.
        submenu._setParent(parent);
        submenu.folderItem = item;
        submenu.parentMenu = parent;
        const scroll = new St.ScrollView({
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
            overlay_scrollbars: true,
        });
        submenu._boxPointer.bin.set_child(scroll);
        scroll.set_child(submenu.box);
        this._initMenu(submenu);
        this._submenus.push(submenu);
        this._fill(submenu, nodes);
        for (const child of submenu._getMenuItems())
            child.connect('key-focus-in', () => ensureActorVisibleInScrollView(scroll, child));
        return submenu;
    }

    _openFolder(item, focusFirst) {
        this._clearHoverTimeout();
        const submenu = item.submenu;
        const parent = submenu.parentMenu;
        if (parent.openChild !== submenu) {
            if (parent.openChild)
                this._closeSubmenu(parent.openChild);
            const monitor = Main.layoutManager.findIndexForActor(item);
            const workArea = Main.layoutManager.getWorkAreaForMonitor(monitor);
            const gap = submenu.actor.get_theme_node().get_length('-boxpointer-gap');
            const scaleFactor = St.ThemeContext.get_for_stage(global.stage).scale_factor;
            submenu.actor.style = `max-height: ${Math.floor((workArea.height - 2 * gap) / scaleFactor)}px;`;
            parent.openChild = submenu;
            item.add_style_class_name('clipy-open');
            submenu.open(BoxPointer.PopupAnimation.FADE);
        }
        if (focusFirst)
            submenu.actor.navigate_focus(null, St.DirectionType.TAB_FORWARD, false);
    }

    _closeSubmenu(submenu) {
        if (submenu.openChild)
            this._closeSubmenu(submenu.openChild);
        submenu.openChild = null;
        if (submenu.parentMenu.openChild === submenu)
            submenu.parentMenu.openChild = null;
        submenu.folderItem.remove_style_class_name('clipy-open');
        submenu.close(BoxPointer.PopupAnimation.NONE);
    }

    // The pointer resting on a row opens its folder, or closes the folder open in that menu. Moving
    // off a row (into the open popup, say) cancels that. The keyboard opens folders with Right,
    // Return or Space, and moving the focus to another row closes the open folder at once.
    _onActiveChanged(menu, item) {
        this._clearHoverTimeout();
        if (!item)
            return;
        const isOpenFolder = menu.openChild && item.submenu === menu.openChild;
        if (!item.hover) {
            if (menu.openChild && !isOpenFolder)
                this._closeSubmenu(menu.openChild);
            return;
        }
        if (isOpenFolder || (!item.submenu && !menu.openChild))
            return;
        this._hoverTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, HOVER_DELAY_MS, () => {
            this._hoverTimeoutId = 0;
            if (item.submenu)
                this._openFolder(item, false);
            else if (menu.openChild)
                this._closeSubmenu(menu.openChild);
            return GLib.SOURCE_REMOVE;
        });
    }

    _clearHoverTimeout() {
        if (this._hoverTimeoutId)
            GLib.source_remove(this._hoverTimeoutId);
        this._hoverTimeoutId = 0;
    }

    // What PopupMenuManager did for a single menu: a click outside every popup closes the menu.
    _onCapturedEvent(event) {
        if (!this._menu)
            return Clutter.EVENT_PROPAGATE;
        const type = event.type();
        if ((type === Clutter.EventType.BUTTON_PRESS || type === Clutter.EventType.TOUCH_BEGIN) &&
            !this._layer.contains(global.stage.get_event_actor(event)))
            this._menu.close(BoxPointer.PopupAnimation.FULL);
        return Clutter.EVENT_PROPAGATE;
    }

    _onKey(menu, event) {
        const symbol = event.get_key_symbol();
        // Before an item has the focus, Down starts from the top and Up from the bottom.
        if (global.stage.get_key_focus() === menu.actor &&
            (symbol === Clutter.KEY_Down || symbol === Clutter.KEY_Up)) {
            menu.actor.navigate_focus(null, symbol === Clutter.KEY_Down
                ? St.DirectionType.TAB_FORWARD : St.DirectionType.TAB_BACKWARD, false);
            return Clutter.EVENT_STOP;
        }
        if (symbol === Clutter.KEY_Escape) {
            this._menu.close(BoxPointer.PopupAnimation.FULL);
            return Clutter.EVENT_STOP;
        }
        const focus = global.stage.get_key_focus();
        if (symbol === Clutter.KEY_Right && focus instanceof ClipyFolderItem) {
            this._openFolder(focus, true);
            return Clutter.EVENT_STOP;
        }
        if (symbol === Clutter.KEY_Left && menu.folderItem) {
            this._closeSubmenu(menu);
            menu.folderItem.grab_key_focus();
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
        this._clearHoverTimeout();
        if (this._grab)
            Main.popModal(this._grab);
        this._grab = null;
        // A folder popup's source is a row of its parent, so it goes before the parent.
        for (const submenu of (this._submenus ?? []).reverse())
            submenu.destroy();
        this._menu?.destroy();
        this._anchor?.destroy();
        this._submenus = null;
        this._menu = null;
        this._anchor = null;
        this._accels = null;
    }

    destroy() {
        this.close();
        this._layer.destroy();
        this._layer = null;
        this._onActivate = null;
    }
}
