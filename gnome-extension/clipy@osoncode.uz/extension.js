// Clipy GNOME Shell companion. The clipy daemon keeps history, snippets and settings; this
// extension does what GNOME Wayland only allows the Shell to do: grab global shortcuts, pop a
// menu at the pointer, follow the clipboard owner and type the paste keystroke.

import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {BUS_NAME, ClipyProxy, OBJECT_PATH, PROTOCOL_VERSION} from './dbus.js';
import {ClipyMenu} from './menu.js';
import {Paster} from './paste.js';

export default class ClipyExtension extends Extension {
    enable() {
        this._proxy = null;
        this._proxySignals = [];
        this._grabs = new Map();
        this._paster = new Paster();
        this._menu = new ClipyMenu((itemId, modifiers) => this._activateItem(itemId, modifiers));

        this._acceleratorId = global.display.connect('accelerator-activated',
            (_display, action) => this._onAccelerator(action));
        this._selection = global.display.get_selection();
        this._ownerChangedId = this._selection.connect('owner-changed', (_selection, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                this._onClipboardChanged();
        });
        this._watchId = Gio.bus_watch_name(Gio.BusType.SESSION, BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => this._connect(),
            () => this._disconnect());
    }

    disable() {
        Gio.bus_unwatch_name(this._watchId);
        this._disconnect();
        global.display.disconnect(this._acceleratorId);
        this._selection.disconnect(this._ownerChangedId);
        this._menu.destroy();
        this._paster.destroy();
        this._menu = null;
        this._paster = null;
        this._selection = null;
        this._grabs = null;
    }

    _connect() {
        new ClipyProxy(Gio.DBus.session, BUS_NAME, OBJECT_PATH, (proxy, error) => {
            if (error) {
                console.error(`Clipy: cannot reach the daemon: ${error}`);
                return;
            }
            this._proxy = proxy;
            this._proxySignals = [
                proxy.connectSignal('ShowMenu', (_p, _sender, [json]) => this._menu.show(json)),
                proxy.connectSignal('ShortcutsChanged', () => this._grabShortcuts()),
                proxy.connectSignal('Paste', (_p, _sender, [combo]) => this._paster.paste(combo)),
            ];
            proxy.RegisterFrontendRemote(PROTOCOL_VERSION, ([version], err) => {
                if (err)
                    console.error(`Clipy: registration failed: ${err}`);
                else if (version !== PROTOCOL_VERSION)
                    console.warn(`Clipy: daemon speaks protocol ${version}, extension ${PROTOCOL_VERSION}`);
            });
            this._grabShortcuts();
        });
    }

    _disconnect() {
        this._ungrabShortcuts();
        for (const id of this._proxySignals)
            this._proxy?.disconnectSignal(id);
        this._proxySignals = [];
        this._proxy = null;
        this._menu?.close();
    }

    _grabShortcuts() {
        this._ungrabShortcuts();
        this._proxy?.GetShortcutsRemote(([shortcuts], err) => {
            if (err) {
                console.error(`Clipy: reading shortcuts failed: ${err}`);
                return;
            }
            for (const [action, accelerator] of Object.entries(shortcuts)) {
                const id = global.display.grab_accelerator(accelerator, Meta.KeyBindingFlags.NONE);
                if (id === Meta.KeyBindingAction.NONE) {
                    console.warn(`Clipy: ${accelerator} for ${action} is taken by another shortcut`);
                    continue;
                }
                const name = Meta.external_binding_name_for_action(id);
                Main.wm.allowKeybinding(name,
                    Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP);
                this._grabs.set(id, {action, name});
            }
        });
    }

    _ungrabShortcuts() {
        for (const [id, {name}] of this._grabs ?? []) {
            global.display.ungrab_accelerator(id);
            Main.wm.allowKeybinding(name, Shell.ActionMode.NONE);
        }
        this._grabs?.clear();
    }

    _onAccelerator(id) {
        const grab = this._grabs.get(id);
        if (grab)
            this._proxy?.ShortcutActivatedRemote(grab.action, () => {});
    }

    _onClipboardChanged() {
        if (!this._proxy)
            return;
        const mimeTypes = St.Clipboard.get_default().get_mimetypes(St.ClipboardType.CLIPBOARD);
        this._proxy.ClipboardChangedRemote(focusedAppId(), mimeTypes, () => {});
    }

    _activateItem(itemId, modifiers) {
        this._proxy?.ActivateItemRemote(itemId, modifiers, focusedAppId(), ([combo], err) => {
            if (err)
                console.error(`Clipy: activating ${itemId} failed: ${err}`);
            else if (combo)
                this._paster.paste(combo);
        });
    }
}

/** Desktop id of the focused app (for the Exclude list and terminal detection). */
function focusedAppId() {
    const window = global.display.get_focus_window();
    if (!window)
        return '';
    const app = Shell.WindowTracker.get_default().get_window_app(window);
    return app?.get_id() ?? window.get_wm_class() ?? '';
}
