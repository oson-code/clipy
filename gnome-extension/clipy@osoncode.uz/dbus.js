// D-Bus interface of the clipy daemon (src/dbus.rs). Keep the two in step; bump
// PROTOCOL_VERSION on both sides for incompatible changes.

import Gio from 'gi://Gio';

export const BUS_NAME = 'uz.osoncode.Clipy';
export const OBJECT_PATH = '/uz/osoncode/Clipy';
export const PROTOCOL_VERSION = 1;

const INTERFACE_XML = `
<node>
  <interface name="uz.osoncode.Clipy1">
    <method name="RegisterFrontend">
      <arg type="u" name="version" direction="in"/>
      <arg type="u" name="daemon_version" direction="out"/>
    </method>
    <method name="GetShortcuts">
      <arg type="a{ss}" name="shortcuts" direction="out"/>
    </method>
    <method name="ShortcutActivated">
      <arg type="s" name="action" direction="in"/>
    </method>
    <method name="ClipboardChanged">
      <arg type="s" name="focused_app_id" direction="in"/>
      <arg type="as" name="mime_types" direction="in"/>
    </method>
    <method name="ActivateItem">
      <arg type="s" name="item_id" direction="in"/>
      <arg type="u" name="modifiers" direction="in"/>
      <arg type="s" name="focused_app_id" direction="in"/>
      <arg type="s" name="paste_combo" direction="out"/>
    </method>
    <signal name="ShortcutsChanged"/>
    <signal name="ShowMenu">
      <arg type="s" name="menu_json"/>
    </signal>
    <signal name="Paste">
      <arg type="s" name="combo"/>
    </signal>
  </interface>
</node>`;

export const ClipyProxy = Gio.DBusProxy.makeProxyWrapper(INTERFACE_XML);
