//! D-Bus service `uz.osoncode.Clipy`: the single-instance lock, the CLI's way into a
//! running daemon, and the GNOME Shell extension's API.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use futures_channel::oneshot;
use tao::event_loop::EventLoopProxy;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{interface, Connection};

use crate::app::{Command, UserEvent};
use crate::clipboard_watcher::WatcherHandle;
use crate::config::{Modifiers, SharedSettings};
use crate::hotkeys::{self, Action};
use crate::menu::{ItemId, MenuKind};

/// The app id, also used for the desktop file and the panel icon.
pub const BUS_NAME: &str = "uz.osoncode.Clipy";
pub const OBJECT_PATH: &str = "/uz/osoncode/Clipy";
pub const INTERFACE: &str = "uz.osoncode.Clipy1";
/// Bumped when the extension-facing API changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

pub struct Service {
    proxy: EventLoopProxy<UserEvent>,
    settings: SharedSettings,
    watcher: WatcherHandle,
}

impl Service {
    fn send(&self, event: UserEvent) -> fdo::Result<()> {
        self.proxy
            .send_event(event)
            .map_err(|_| fdo::Error::Failed("Clipy is shutting down".into()))
    }
}

#[interface(name = "uz.osoncode.Clipy1")]
impl Service {
    /// Shows a menu: `main`, `history`, `snippets` or `folder:<id>`.
    fn popup(&self, kind: &str) -> fdo::Result<()> {
        let kind = MenuKind::parse(kind)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown menu {kind:?}")))?;
        self.send(UserEvent::Command(Command::Popup(kind)))
    }

    fn show_preferences(&self) -> fdo::Result<()> {
        self.send(UserEvent::Command(Command::ShowPreferences))
    }

    fn show_snippet_editor(&self) -> fdo::Result<()> {
        self.send(UserEvent::Command(Command::ShowSnippetEditor))
    }

    fn clear_history(&self) -> fdo::Result<()> {
        self.send(UserEvent::Command(Command::ClearHistory))
    }

    fn quit(&self) -> fdo::Result<()> {
        self.send(UserEvent::Command(Command::Quit))
    }

    /// Called by the GNOME Shell extension when it starts; it then owns hotkeys, menus and
    /// paste. Returns the daemon's protocol version.
    fn register_frontend(
        &self,
        version: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<u32> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::Failed("no sender".into()))?;
        log::info!("GNOME Shell extension registered (protocol {version}) as {sender}");
        self.send(UserEvent::FrontendRegistered { sender })?;
        Ok(PROTOCOL_VERSION)
    }

    /// Action name → GTK accelerator, for the extension to grab.
    fn get_shortcuts(&self) -> HashMap<String, String> {
        let settings = self.settings.read().unwrap_or_else(|e| e.into_inner());
        hotkeys::shortcut_list(&settings.shortcuts)
            .into_iter()
            .map(|(action, accel)| (action.encode(), accel))
            .collect()
    }

    fn shortcut_activated(&self, action: &str) -> fdo::Result<()> {
        let action = Action::parse(action)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown action {action:?}")))?;
        self.send(UserEvent::Command(Command::Shortcut(action)))
    }

    /// The extension saw the clipboard owner change. `focused_app_id` is the desktop id of the
    /// focused app, used for the Exclude list.
    fn clipboard_changed(&self, focused_app_id: &str, _mime_types: Vec<String>) {
        let app = (!focused_app_id.is_empty()).then(|| focused_app_id.to_owned());
        self.watcher.clipboard_changed(app);
    }

    /// Puts the chosen item on the clipboard. Returns the keystroke the extension should send
    /// (`ctrl+v` or `ctrl+shift+v`), or an empty string for no paste.
    async fn activate_item(
        &self,
        item_id: &str,
        modifiers: u32,
        focused_app_id: &str,
    ) -> fdo::Result<String> {
        let item = ItemId::parse(item_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown item {item_id:?}")))?;
        let (reply, answer) = oneshot::channel();
        self.send(UserEvent::Activate {
            item,
            modifiers: Modifiers(modifiers),
            app: (!focused_app_id.is_empty()).then(|| vec![focused_app_id.to_owned()]),
            reply: Some(reply),
        })?;
        answer
            .await
            .map_err(|_| fdo::Error::Failed("the item could not be activated".into()))
    }

    #[zbus(signal)]
    async fn shortcuts_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_menu(emitter: &SignalEmitter<'_>, menu_json: &str) -> zbus::Result<()>;

    /// Asks the extension to type `combo` into the focused window, after an item was chosen
    /// from the panel menu.
    #[zbus(signal)]
    async fn paste(emitter: &SignalEmitter<'_>, combo: &str) -> zbus::Result<()>;
}

/// Outcome of connecting at startup.
pub enum Startup {
    /// No other instance: serve the API and own the name.
    Primary(Connection),
    /// Another instance owns the name; the command was handed to it.
    Forwarded,
}

/// Connects to the session bus and finds out whether Clipy already runs. When it does,
/// `forward` (method name and optional menu kind) is sent to it.
pub fn connect(forward: (&str, Option<&str>)) -> Result<Startup> {
    zbus::block_on(async {
        let conn = Connection::session()
            .await
            .context("connecting to the session bus")?;
        let dbus = fdo::DBusProxy::new(&conn).await?;
        if dbus.name_has_owner(BUS_NAME.try_into()?).await? {
            call(&conn, forward).await?;
            return Ok(Startup::Forwarded);
        }
        Ok(Startup::Primary(conn))
    })
}

async fn call(conn: &Connection, (method, arg): (&str, Option<&str>)) -> Result<()> {
    match arg {
        Some(arg) => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                method,
                &(arg,),
            )
            .await?
        }
        None => {
            conn.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, &())
                .await?
        }
    };
    Ok(())
}

/// Publishes the service object and takes the bus name. Fails when another instance won the
/// race for the name.
pub fn serve(
    conn: &Connection,
    proxy: EventLoopProxy<UserEvent>,
    settings: SharedSettings,
    watcher: WatcherHandle,
) -> Result<()> {
    zbus::block_on(async {
        conn.object_server()
            .at(
                OBJECT_PATH,
                Service {
                    proxy,
                    settings,
                    watcher,
                },
            )
            .await?;
        match conn
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DO_NOT_QUEUE)
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
            reply => bail!("another Clipy owns {BUS_NAME} ({reply:?})"),
        }
    })
}

pub fn emit_show_menu(conn: &Connection, json: &str) -> Result<()> {
    zbus::block_on(async {
        let emitter = SignalEmitter::new(conn, OBJECT_PATH)?;
        Service::show_menu(&emitter, json).await?;
        Ok(())
    })
}

pub fn emit_paste(conn: &Connection, combo: &str) -> Result<()> {
    zbus::block_on(async {
        let emitter = SignalEmitter::new(conn, OBJECT_PATH)?;
        Service::paste(&emitter, combo).await?;
        Ok(())
    })
}

pub fn emit_shortcuts_changed(conn: &Connection) -> Result<()> {
    zbus::block_on(async {
        let emitter = SignalEmitter::new(conn, OBJECT_PATH)?;
        Service::shortcuts_changed(&emitter).await?;
        Ok(())
    })
}

/// True while `unique_name` (the extension's connection) is still on the bus.
pub fn has_owner(conn: &Connection, unique_name: &str) -> bool {
    zbus::block_on(async {
        let dbus = fdo::DBusProxy::new(conn).await.ok()?;
        dbus.name_has_owner(unique_name.try_into().ok()?).await.ok()
    })
    .unwrap_or(false)
}

/// Shows a desktop notification.
pub fn notify(conn: &Connection, summary: &str, body: &str) {
    let result: Result<()> = zbus::block_on(async {
        let hints: HashMap<&str, zbus::Variant<'_>> = HashMap::new();
        conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Clipy",
                0u32,
                "edit-paste",
                summary,
                body,
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
        .await?;
        Ok(())
    });
    if let Err(e) = result {
        log::warn!("notification failed ({summary}): {e:#}");
    }
}
