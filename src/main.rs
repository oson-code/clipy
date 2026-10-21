//! Clipy for Linux: clipboard history and snippets in a panel menu, a port of Clipy for macOS.

mod app;
mod autostart;
mod clipboard;
mod clipboard_watcher;
mod config;
mod db;
mod dbus;
mod hotkeys;
mod i18n;
mod images;
mod menu;
mod model;
mod paste;
mod popup_gtk;
mod screenshots;
mod snippets_xml;
mod theme;
mod tray;
mod ui;
mod x11;

use anyhow::{bail, Result};
use tao::event_loop::EventLoopBuilder;

use crate::app::{App, Command, UserEvent};
use crate::menu::MenuKind;

const USAGE: &str = "\
Usage: clipy [OPTION]

Starts Clipy, or sends a command to the Clipy that is already running.

  --popup KIND      show a menu at the pointer: main, history, snippets or folder:ID
  --preferences     open Preferences (also what a second plain `clipy` does)
  --snippets        open the snippet editor
  --clear-history   clear clipboard history
  --quit            quit the running Clipy
  --version         print the version
  --help            print this help";

/// A command given on the command line.
enum Cli {
    Start,
    Popup(MenuKind),
    Preferences,
    Snippets,
    ClearHistory,
    Quit,
}

impl Cli {
    fn parse() -> Result<Option<Cli>> {
        use lexopt::prelude::*;
        let mut parser = lexopt::Parser::from_env();
        let mut cli = Cli::Start;
        while let Some(arg) = parser.next()? {
            cli = match arg {
                Long("popup") => {
                    let kind: String = parser.value()?.parse()?;
                    match MenuKind::parse(&kind) {
                        Some(kind) => Cli::Popup(kind),
                        None => {
                            bail!("unknown menu {kind:?}; use main, history, snippets or folder:ID")
                        }
                    }
                }
                Long("preferences") => Cli::Preferences,
                Long("snippets") => Cli::Snippets,
                Long("clear-history") => Cli::ClearHistory,
                Long("quit") => Cli::Quit,
                Long("version") => {
                    println!("clipy {}", env!("CARGO_PKG_VERSION"));
                    return Ok(None);
                }
                Long("help") | Short('h') => {
                    println!("{USAGE}");
                    return Ok(None);
                }
                _ => return Err(arg.unexpected().into()),
            };
        }
        Ok(Some(cli))
    }

    /// The D-Bus call that hands this command to a running instance.
    fn forward(&self) -> (&'static str, Option<String>) {
        match self {
            Cli::Start | Cli::Preferences => ("ShowPreferences", None),
            Cli::Popup(kind) => ("Popup", Some(kind.encode())),
            Cli::Snippets => ("ShowSnippetEditor", None),
            Cli::ClearHistory => ("ClearHistory", None),
            Cli::Quit => ("Quit", None),
        }
    }

    /// What to do once this process becomes the running instance.
    fn initial_command(self) -> Option<Command> {
        match self {
            Cli::Start => None,
            Cli::Popup(kind) => Some(Command::Popup(kind)),
            Cli::Preferences => Some(Command::ShowPreferences),
            Cli::Snippets => Some(Command::ShowSnippetEditor),
            Cli::ClearHistory => Some(Command::ClearHistory),
            Cli::Quit => None,
        }
    }
}

fn main() {
    // GTK3 on X11 loads the GL stack at startup just to probe visuals; Clipy draws no GL, and
    // skipping it saves tens of megabytes. Set before any thread starts.
    if std::env::var_os("GDK_GL").is_none() {
        std::env::set_var("GDK_GL", "disable");
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = match Cli::parse() {
        Ok(Some(cli)) => cli,
        Ok(None) => return,
        Err(e) => {
            eprintln!("clipy: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(cli) {
        log::error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let (method, arg) = cli.forward();
    let bus = match dbus::connect((method, arg.as_deref())) {
        Ok(dbus::Startup::Forwarded) => return Ok(()),
        Ok(dbus::Startup::Primary(conn)) => Some(conn),
        Err(e) => {
            log::warn!(
                "running without D-Bus (no single-instance check, no GNOME extension): {e:#}"
            );
            None
        }
    };
    if matches!(cli, Cli::Quit) {
        return Ok(());
    }
    // Our windows then carry the app id, which links them to uz.osoncode.Clipy.desktop.
    gtk::glib::set_prgname(Some(dbus::BUS_NAME));
    gtk::glib::set_application_name("Clipy");
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let mut app = App::new(event_loop.create_proxy(), bus, cli.initial_command())?;
    event_loop.run(move |event, _, control_flow| app.handle(event, control_flow));
}
