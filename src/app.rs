//! Application state on the main (GTK) thread. Every input — panel menu, global shortcuts,
//! D-Bus calls, the clipboard watcher, windows — arrives here as a [`UserEvent`].

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use futures_channel::oneshot;
use gtk::glib;
use gtk::prelude::*;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopProxy};
use tray_icon::menu::MenuEvent;
use zbus::Connection;

use crate::clipboard::ClipboardIo;
use crate::clipboard_watcher::{self, SelfEcho, WatcherHandle};
use crate::config::{self, Modifiers, Settings, SharedSettings, Shortcuts};
use crate::db::Database;
use crate::hotkeys::{Action, Hotkeys};
use crate::i18n::tr;
use crate::menu::{self, ItemId, MenuKind, MenuNode, MenuSource};
use crate::popup_gtk::{self, OnActivate};
use crate::screenshots::ScreenshotWatcher;
use crate::theme::{self, Glyph, ThemeWatcher};
use crate::tray::Tray;
use crate::ui::dialogs;
use crate::ui::preferences::PreferencesWindow;
use crate::ui::snippet_editor::SnippetEditor;
use crate::ui::widgets::Ctx;
use crate::{autostart, dbus, images, paste, x11};

const APP_ICON: &[u8] = include_bytes!("../resources/icons/app_256.png");
const MENU_REBUILD_DELAY: Duration = Duration::from_millis(300);
const EXTENSION_CHECK_INTERVAL: u32 = 5;

pub enum UserEvent {
    TrayMenu(String),
    Hotkey {
        action: Action,
        pressed: bool,
    },
    HistoryChanged,
    RebuildMenus,
    Command(Command),
    Activate {
        item: ItemId,
        modifiers: Modifiers,
        /// Source app as reported by the GNOME extension.
        app: Option<Vec<String>>,
        /// Set when the GNOME extension asked; it then sends the paste keystroke itself.
        reply: Option<oneshot::Sender<String>>,
    },
    FrontendRegistered {
        sender: String,
    },
    CheckFrontend,
    ClearHistoryAnswered {
        confirmed: bool,
        suppress: bool,
    },
    LoginAlertAnswered {
        launch: bool,
        suppress: bool,
    },
    PeriodicClear,
    PasteFailed(String),
    /// The shared settings were changed (by the Preferences window); save and apply them.
    SettingsChanged,
    PreferencesClosed,
    SnippetsChanged,
    SnippetEditorClosed,
    /// The GTK or Cinnamon theme changed.
    ThemeChanged,
    /// The freedesktop colour scheme, from the settings portal.
    ColorScheme(Option<u32>),
}

pub enum Command {
    Popup(MenuKind),
    ShowPreferences,
    ShowSnippetEditor,
    ClearHistory,
    Quit,
    Shortcut(Action),
}

/// Who shows menus, grabs shortcuts and sends the paste keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Frontend {
    /// X11 session: global-hotkey, GtkMenu at the pointer, XTest.
    X11,
    /// The GNOME Shell extension, identified by its unique bus name.
    Extension(String),
    /// Wayland without the extension: only the panel menu works.
    TrayOnly,
}

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    settings: SharedSettings,
    /// The settings whose side effects (shortcuts, timers, autostart…) are in place.
    applied: Settings,
    config_path: PathBuf,
    db_path: PathBuf,
    db: Database,
    clipboard: Option<ClipboardIo>,
    self_echo: SelfEcho,
    watcher: WatcherHandle,
    bus: Option<Connection>,
    frontend: Frontend,
    probe: Option<x11::X11Probe>,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    hotkey_errors: Vec<(Action, String)>,
    popup: Option<gtk::Menu>,
    preferences: Option<PreferencesWindow>,
    snippet_editor: Option<SnippetEditor>,
    screenshots: Option<ScreenshotWatcher>,
    theme: Option<ThemeWatcher>,
    color_scheme: Option<u32>,
    rebuild_scheduled: bool,
    clear_timer: Option<glib::SourceId>,
    extension_timer: Option<glib::SourceId>,
    clear_alert_open: bool,
    told_about_paste: bool,
    told_about_frontend: bool,
    pending: Option<Command>,
    exit: bool,
}

impl App {
    /// `initial` runs once the event loop starts, for `clipy --popup …` and the like when no
    /// other instance was running.
    pub fn new(
        proxy: EventLoopProxy<UserEvent>,
        bus: Option<Connection>,
        initial: Option<Command>,
    ) -> Result<Self> {
        let config_path = config::config_path();
        let applied = Settings::load(&config_path);
        let settings: SharedSettings = Arc::new(RwLock::new(applied.clone()));
        let db_path = config::database_path();
        let db = Database::init(&db_path)?;
        let self_echo = SelfEcho::default();
        let watcher = clipboard_watcher::spawn(
            db_path.clone(),
            settings.clone(),
            self_echo.clone(),
            proxy.clone(),
        );
        if let Some(conn) = &bus {
            dbus::serve(conn, proxy.clone(), settings.clone(), watcher.clone())?;
        }
        let clipboard = ClipboardIo::new()
            .map_err(|e| log::error!("clipboard unavailable: {e:#}"))
            .ok();
        let x11_session = x11::display_available() && !paste::is_wayland_session();
        let probe = if x11_session {
            x11::X11Probe::new().map_err(|e| log::warn!("{e:#}")).ok()
        } else {
            None
        };
        Ok(Self {
            proxy,
            settings,
            applied,
            config_path,
            db_path,
            db,
            clipboard,
            self_echo,
            watcher,
            bus,
            frontend: if x11_session {
                Frontend::X11
            } else {
                Frontend::TrayOnly
            },
            probe,
            tray: None,
            hotkeys: None,
            hotkey_errors: Vec::new(),
            popup: None,
            preferences: None,
            snippet_editor: None,
            screenshots: None,
            theme: None,
            color_scheme: None,
            rebuild_scheduled: false,
            clear_timer: None,
            extension_timer: None,
            clear_alert_open: false,
            told_about_paste: false,
            told_about_frontend: false,
            pending: initial,
            exit: false,
        })
    }

    pub fn handle(&mut self, event: Event<'_, UserEvent>, control_flow: &mut ControlFlow) {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => self.start(),
            Event::UserEvent(event) => self.on_user_event(event),
            _ => {}
        }
        if self.exit {
            *control_flow = ControlFlow::Exit;
        }
    }

    fn settings(&self) -> Settings {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn start(&mut self) {
        if let Ok((rgba, w, h)) = images::decode_png(APP_ICON) {
            let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_bytes(
                &glib::Bytes::from_owned(rgba),
                gtk::gdk_pixbuf::Colorspace::Rgb,
                true,
                8,
                w as i32,
                h as i32,
                (w * 4) as i32,
            );
            gtk::Window::set_default_icon(&pixbuf);
        }

        let proxy = self.proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = proxy.send_event(UserEvent::TrayMenu(e.id.0));
        }));
        let s = self.settings();
        self.theme = Some(theme::watch(self.proxy.clone(), self.bus.clone()));
        match Tray::new(
            &self.menu_nodes(MenuKind::Main),
            &s.menu,
            s.general.panel_icon,
            self.panel_glyph(),
        ) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => log::error!("{e:#}"),
        }

        if self.frontend == Frontend::X11 {
            let proxy = self.proxy.clone();
            let on_event = move |action, pressed| {
                let _ = proxy.send_event(UserEvent::Hotkey { action, pressed });
            };
            match Hotkeys::new(on_event) {
                Ok(hotkeys) => self.hotkeys = Some(hotkeys),
                Err(e) => log::error!("global shortcuts unavailable: {e:#}"),
            }
            self.apply_hotkeys(&s.shortcuts);
        } else {
            // Give the GNOME extension a moment to register before telling the user it is missing.
            let proxy = self.proxy.clone();
            glib::timeout_add_seconds_local_once(EXTENSION_CHECK_INTERVAL, move || {
                let _ = proxy.send_event(UserEvent::CheckFrontend);
            });
        }

        self.schedule_periodic_clear(&s);
        self.restart_screenshots(&s);
        if !s.general.launch_at_login && !s.general.suppress_login_alert && !autostart::is_enabled()
        {
            let proxy = self.proxy.clone();
            dialogs::ask_launch_at_login(move |launch, suppress| {
                let _ = proxy.send_event(UserEvent::LoginAlertAnswered { launch, suppress });
            });
        }
        if let Some(command) = self.pending.take() {
            self.run_command(command);
        }
        log::info!("Clipy started ({:?} frontend)", self.frontend);
    }

    fn on_user_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::TrayMenu(id) => {
                if let Some(item) = ItemId::parse(&id) {
                    self.activate(item, Modifiers::default(), None, false);
                }
            }
            UserEvent::Hotkey { action, pressed } => {
                if self.frontend == Frontend::X11 && action.fires_on_release() != pressed {
                    self.run_action(action);
                }
            }
            UserEvent::HistoryChanged => self.schedule_rebuild(),
            UserEvent::RebuildMenus => {
                self.rebuild_scheduled = false;
                if let Some(tray) = &self.tray {
                    tray.set_menu(&self.menu_nodes(MenuKind::Main), &self.settings().menu);
                }
            }
            UserEvent::Command(command) => self.run_command(command),
            UserEvent::Activate {
                item,
                modifiers,
                app,
                reply,
            } => {
                let combo = self.activate(item, modifiers, app, reply.is_some());
                if let Some(reply) = reply {
                    let _ = reply.send(combo);
                }
            }
            UserEvent::FrontendRegistered { sender } => self.extension_registered(sender),
            UserEvent::CheckFrontend => self.check_frontend(),
            UserEvent::ClearHistoryAnswered {
                confirmed,
                suppress,
            } => {
                self.clear_alert_open = false;
                if confirmed {
                    if suppress {
                        self.update_settings(|s| s.menu.confirm_clear = false);
                    }
                    self.clear_history();
                }
            }
            UserEvent::LoginAlertAnswered { launch, suppress } => {
                self.update_settings(|s| {
                    s.general.launch_at_login = launch;
                    s.general.suppress_login_alert = suppress;
                });
            }
            UserEvent::PeriodicClear => {
                log::info!("clearing history (periodic)");
                self.clear_history();
            }
            UserEvent::PasteFailed(reason) => {
                log::warn!("automatic paste failed: {reason}");
                if !self.told_about_paste {
                    self.told_about_paste = true;
                    self.notify(
                        tr("Clipy can't paste automatically"),
                        tr("The item is on the clipboard: press Ctrl+V to paste it. Install xdotool (X11) or wtype/ydotool (Wayland), or enable the Clipy GNOME Shell extension."),
                    );
                }
            }
            UserEvent::SettingsChanged => self.apply_settings(),
            UserEvent::PreferencesClosed => self.preferences = None,
            UserEvent::SnippetsChanged => self.schedule_rebuild(),
            UserEvent::SnippetEditorClosed => self.snippet_editor = None,
            UserEvent::ThemeChanged => self.refresh_panel_icon(),
            UserEvent::ColorScheme(scheme) => {
                self.color_scheme = scheme;
                self.refresh_panel_icon();
            }
        }
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::Popup(kind) => self.popup(kind),
            Command::ShowPreferences => self.show_preferences(),
            Command::ShowSnippetEditor => self.show_snippet_editor(),
            Command::ClearHistory => self.request_clear_history(),
            Command::Quit => self.quit(),
            Command::Shortcut(action) => self.run_action(action),
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::MainMenu => self.popup(MenuKind::Main),
            Action::HistoryMenu => self.popup(MenuKind::History),
            Action::SnippetsMenu => self.popup(MenuKind::Snippets),
            Action::Folder(id) => self.popup(MenuKind::Folder(id)),
            Action::ClearHistory => self.request_clear_history(),
            Action::EditSnippets => self.show_snippet_editor(),
            Action::Preferences => self.show_preferences(),
        }
    }

    // ---- menus ------------------------------------------------------------------------

    fn menu_nodes(&self, kind: MenuKind) -> Vec<MenuNode> {
        let s = self.settings();
        let max_history = s.general.max_history as usize;
        let history = self
            .db
            .fetch_recent_history(max_history, s.general.sort_by)
            .unwrap_or_else(|e| {
                log::error!("reading history: {e:#}");
                Vec::new()
            });
        let folders = self.db.fetch_snippets_hierarchy().unwrap_or_else(|e| {
            log::error!("reading snippets: {e:#}");
            Vec::new()
        });
        menu::build(
            kind,
            &MenuSource {
                history: &history,
                folders: &folders,
                prefs: &s.menu,
                max_history,
            },
        )
    }

    fn schedule_rebuild(&mut self) {
        if self.rebuild_scheduled {
            return;
        }
        self.rebuild_scheduled = true;
        let proxy = self.proxy.clone();
        glib::timeout_add_local_once(MENU_REBUILD_DELAY, move || {
            let _ = proxy.send_event(UserEvent::RebuildMenus);
        });
    }

    fn popup(&mut self, kind: MenuKind) {
        log::debug!("showing the {} menu", kind.encode());
        let nodes = self.menu_nodes(kind);
        if nodes.is_empty() {
            return;
        }
        let prefs = self.settings().menu;
        match &self.frontend {
            Frontend::Extension(_) => {
                let json = menu::to_json(kind, &nodes, &prefs).to_string();
                if let Some(bus) = &self.bus {
                    if let Err(e) = dbus::emit_show_menu(bus, &json) {
                        log::error!("asking the GNOME extension to show the menu: {e:#}");
                    }
                }
            }
            Frontend::X11 => {
                if let Some(old) = self.popup.take() {
                    old.deactivate();
                }
                let proxy = self.proxy.clone();
                let on_activate: OnActivate = Rc::new(move |item, modifiers| {
                    let _ = proxy.send_event(UserEvent::Activate {
                        item,
                        modifiers,
                        app: None,
                        reply: None,
                    });
                });
                self.popup = Some(popup_gtk::popup(&nodes, &prefs, on_activate));
            }
            Frontend::TrayOnly => {
                log::warn!("no way to show a menu at the pointer in this session");
                self.tell_about_frontend();
            }
        }
    }

    // ---- selection --------------------------------------------------------------------

    /// Runs a menu item. Returns the paste keystroke for the GNOME extension when
    /// `for_extension` is set, otherwise sends it here and returns an empty string.
    fn activate(
        &mut self,
        item: ItemId,
        modifiers: Modifiers,
        app: Option<Vec<String>>,
        for_extension: bool,
    ) -> String {
        let result = match item {
            ItemId::History(id) => self.paste_history(id, modifiers, app, for_extension),
            ItemId::Snippet(id) => self.paste_snippet(id, app, for_extension),
            ItemId::ClearHistory => {
                self.request_clear_history();
                Ok(String::new())
            }
            ItemId::EditSnippets => {
                self.show_snippet_editor();
                Ok(String::new())
            }
            ItemId::Preferences => {
                self.show_preferences();
                Ok(String::new())
            }
            ItemId::Quit => {
                self.quit();
                Ok(String::new())
            }
        };
        result.unwrap_or_else(|e| {
            log::error!("{e:#}");
            beep();
            String::new()
        })
    }

    fn paste_history(
        &mut self,
        id: i64,
        modifiers: Modifiers,
        app: Option<Vec<String>>,
        for_extension: bool,
    ) -> Result<String> {
        let Some(clip) = self.db.fetch_clip(id)? else {
            beep();
            return Ok(String::new());
        };
        let beta = self.settings().beta;
        if beta.delete_history && beta.delete_history_modifier.is_held(modifiers) {
            self.db.delete_history(id)?;
            self.schedule_rebuild();
            return Ok(String::new());
        }
        let plain = beta.paste_plain_text && beta.paste_plain_text_modifier.is_held(modifiers);
        let and_delete = beta.paste_and_delete && beta.paste_and_delete_modifier.is_held(modifiers);
        let echo = self.self_echo.clone();
        self.clipboard()?.write_clip(&clip, plain, &echo)?;
        if and_delete {
            self.db.delete_history(id)?;
        } else {
            self.db.touch(id)?;
        }
        self.schedule_rebuild();
        Ok(self.paste(app, for_extension))
    }

    /// Snippets go on the clipboard as a normal copy, so they also enter history, as in Clipy.
    fn paste_snippet(
        &mut self,
        id: i64,
        app: Option<Vec<String>>,
        for_extension: bool,
    ) -> Result<String> {
        let Some(snippet) = self.db.fetch_snippet(id)? else {
            beep();
            return Ok(String::new());
        };
        self.clipboard()?.write_text(&snippet.content)?;
        Ok(self.paste(app, for_extension))
    }

    /// The clipboard, opened on first use: at login the X server (Xwayland) may come up after
    /// Clipy does.
    fn clipboard(&mut self) -> Result<&mut ClipboardIo> {
        if self.clipboard.is_none() {
            self.clipboard = Some(ClipboardIo::new()?);
        }
        Ok(self.clipboard.as_mut().expect("just opened"))
    }

    fn paste(&mut self, app: Option<Vec<String>>, for_extension: bool) -> String {
        let s = self.settings();
        if !s.general.paste_automatically {
            return String::new();
        }
        let app = app.unwrap_or_else(|| {
            self.probe
                .as_ref()
                .map(|p| p.active_window_class())
                .unwrap_or_default()
        });
        let combo = paste::combo_for(&app, &s.advanced.terminal_wm_classes);
        if for_extension {
            return combo.as_str().to_owned();
        }
        if let (Frontend::Extension(_), Some(bus)) = (&self.frontend, &self.bus) {
            // Chosen from the panel menu on GNOME: the extension types the keystroke.
            if let Err(e) = dbus::emit_paste(bus, combo.as_str()) {
                log::error!("asking the GNOME extension to paste: {e:#}");
            }
            return String::new();
        }
        let proxy = self.proxy.clone();
        paste::paste_later(
            combo,
            Duration::from_millis(u64::from(s.advanced.paste_delay_ms)),
            move |reason| {
                let _ = proxy.send_event(UserEvent::PasteFailed(reason));
            },
        );
        String::new()
    }

    // ---- history clearing -------------------------------------------------------------

    fn request_clear_history(&mut self) {
        if self.db.history_count().unwrap_or(0) == 0 {
            return;
        }
        if !self.settings().menu.confirm_clear {
            self.clear_history();
            return;
        }
        if self.clear_alert_open {
            return;
        }
        self.clear_alert_open = true;
        let proxy = self.proxy.clone();
        dialogs::confirm_clear_history(move |confirmed, suppress| {
            let _ = proxy.send_event(UserEvent::ClearHistoryAnswered {
                confirmed,
                suppress,
            });
        });
    }

    fn clear_history(&mut self) {
        if let Err(e) = self.db.clear_history() {
            return log::error!("clearing history: {e:#}");
        }
        let path = self.db_path.clone();
        std::thread::spawn(move || {
            if let Err(e) = Database::init(&path).and_then(|db| db.vacuum()) {
                log::warn!("vacuuming the database: {e:#}");
            }
        });
        self.schedule_rebuild();
    }

    fn schedule_periodic_clear(&mut self, s: &Settings) {
        if let Some(timer) = self.clear_timer.take() {
            timer.remove();
        }
        if !s.general.clear_periodically {
            return;
        }
        let proxy = self.proxy.clone();
        self.clear_timer = Some(glib::timeout_add_seconds_local(
            s.general.clear_interval.seconds(),
            move || {
                let _ = proxy.send_event(UserEvent::PeriodicClear);
                glib::ControlFlow::Continue
            },
        ));
    }

    fn quit(&mut self) {
        if self.settings().general.clear_on_quit {
            if let Err(e) = self.db.clear_history() {
                log::error!("clearing history on quit: {e:#}");
            }
        }
        self.exit = true;
    }

    // ---- frontends --------------------------------------------------------------------

    fn extension_registered(&mut self, sender: String) {
        if let Some(hotkeys) = &mut self.hotkeys {
            hotkeys.apply(&Shortcuts {
                main: String::new(),
                history: String::new(),
                snippets: String::new(),
                clear_history: String::new(),
                edit_snippets: String::new(),
                preferences: String::new(),
                folders: Default::default(),
            });
        }
        self.watcher.set_extension_active(true);
        self.frontend = Frontend::Extension(sender);
        if self.extension_timer.is_none() {
            let proxy = self.proxy.clone();
            self.extension_timer = Some(glib::timeout_add_seconds_local(
                EXTENSION_CHECK_INTERVAL,
                move || {
                    let _ = proxy.send_event(UserEvent::CheckFrontend);
                    glib::ControlFlow::Continue
                },
            ));
        }
    }

    /// Falls back from the extension when it leaves the bus, and tells the user once when a
    /// Wayland session has no way to show menus.
    fn check_frontend(&mut self) {
        if let Frontend::Extension(sender) = &self.frontend {
            let alive = self
                .bus
                .as_ref()
                .is_some_and(|bus| dbus::has_owner(bus, sender));
            if alive {
                return;
            }
            log::warn!("the GNOME Shell extension went away");
            if let Some(timer) = self.extension_timer.take() {
                timer.remove();
            }
            self.watcher.set_extension_active(false);
            self.frontend = if self.probe.is_some() {
                Frontend::X11
            } else {
                Frontend::TrayOnly
            };
            let shortcuts = self.settings().shortcuts;
            self.apply_hotkeys(&shortcuts);
        }
        if self.frontend == Frontend::TrayOnly {
            self.tell_about_frontend();
        }
    }

    fn tell_about_frontend(&mut self) {
        if self.told_about_frontend {
            return;
        }
        self.told_about_frontend = true;
        if paste::is_gnome_session() {
            self.notify(
                tr("Clipy needs its GNOME Shell extension"),
                tr("On Wayland, shortcuts, the menu at the pointer and automatic paste need the Clipy extension. Run `make install-extension`, log out and back in, then enable it in Extensions."),
            );
        } else {
            self.notify(
                tr("Clipy works from the panel on this desktop"),
                tr("On this Wayland desktop, Clipy cannot use global shortcuts or show a menu at the pointer yet. Use the panel icon, or log in with an X11 session."),
            );
        }
    }

    fn apply_hotkeys(&mut self, shortcuts: &Shortcuts) {
        if let (Frontend::X11, Some(hotkeys)) = (&self.frontend, &mut self.hotkeys) {
            self.hotkey_errors = hotkeys.apply(shortcuts);
        }
        if let Some(window) = &self.preferences {
            window.set_shortcut_errors(&self.hotkey_errors);
        }
        if let Some(editor) = &self.snippet_editor {
            editor.set_shortcut_errors(&self.hotkey_errors);
        }
    }

    // ---- settings and windows ---------------------------------------------------------

    fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings.write().unwrap_or_else(|e| e.into_inner()));
        self.apply_settings();
    }

    /// Saves the shared settings and applies whatever changed since the last call.
    fn apply_settings(&mut self) {
        let new = self.settings().sanitized();
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = new.clone();
        let old = std::mem::replace(&mut self.applied, new.clone());
        if new == old {
            return;
        }
        if let Err(e) = new.save(&self.config_path) {
            log::error!("saving settings: {e:#}");
        }

        if new.general.launch_at_login != old.general.launch_at_login {
            if let Err(e) = autostart::set_enabled(new.general.launch_at_login) {
                log::error!("launch on login: {e:#}");
            }
        }
        if new.shortcuts != old.shortcuts {
            self.apply_hotkeys(&new.shortcuts);
            if let (Frontend::Extension(_), Some(bus)) = (&self.frontend, &self.bus) {
                if let Err(e) = dbus::emit_shortcuts_changed(bus) {
                    log::error!("{e:#}");
                }
            }
        }
        if new.general.panel_icon != old.general.panel_icon {
            self.refresh_panel_icon();
        }
        if (new.general.clear_periodically, new.general.clear_interval)
            != (old.general.clear_periodically, old.general.clear_interval)
        {
            self.schedule_periodic_clear(&new);
        }
        let screenshots = |s: &Settings| {
            (
                s.beta.observe_screenshots,
                s.beta.screenshot_dirs.clone(),
                s.beta.screenshot_prefix.clone(),
            )
        };
        if screenshots(&new) != screenshots(&old) {
            self.restart_screenshots(&new);
        }
        if (new.general.max_history, new.general.sort_by)
            != (old.general.max_history, old.general.sort_by)
        {
            if let Err(e) = self
                .db
                .trim(new.general.max_history as usize, new.general.sort_by)
            {
                log::error!("trimming history: {e:#}");
            }
        }
        self.schedule_rebuild();
    }

    fn restart_screenshots(&mut self, s: &Settings) {
        self.screenshots = s.beta.observe_screenshots.then(|| {
            ScreenshotWatcher::start(
                self.watcher.clone(),
                &s.beta.screenshot_dirs,
                &s.beta.screenshot_prefix,
            )
        });
    }

    /// The glyph that suits the current theme (used by the Standard panel icon).
    fn panel_glyph(&self) -> Glyph {
        theme::choose_glyph(&theme::local_appearance(self.color_scheme))
    }

    fn refresh_panel_icon(&mut self) {
        let (mode, glyph) = (self.settings().general.panel_icon, self.panel_glyph());
        if let Some(tray) = &mut self.tray {
            if let Err(e) = tray.set_mode(mode, glyph) {
                log::error!("panel icon: {e:#}");
            }
        }
    }

    fn show_preferences(&mut self) {
        if self.preferences.is_none() {
            let ctx = Ctx {
                settings: self.settings.clone(),
                proxy: self.proxy.clone(),
            };
            self.preferences = Some(PreferencesWindow::new(ctx));
        }
        if let Some(window) = &self.preferences {
            window.set_shortcut_errors(&self.hotkey_errors);
            window.present();
        }
    }

    fn show_snippet_editor(&mut self) {
        if self.snippet_editor.is_none() {
            let ctx = Ctx {
                settings: self.settings.clone(),
                proxy: self.proxy.clone(),
            };
            match SnippetEditor::new(ctx, &self.db_path) {
                Ok(editor) => self.snippet_editor = Some(editor),
                Err(e) => return log::error!("opening the snippet editor: {e:#}"),
            }
        }
        if let Some(editor) = &self.snippet_editor {
            editor.set_shortcut_errors(&self.hotkey_errors);
            editor.present();
        }
    }

    fn notify(&self, summary: &str, body: &str) {
        match &self.bus {
            Some(bus) => dbus::notify(bus, summary, body),
            None => log::warn!("{summary}: {body}"),
        }
    }
}

fn beep() {
    if let Some(display) = gtk::gdk::Display::default() {
        display.beep();
    }
}
