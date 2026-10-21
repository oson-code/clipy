//! Panel icon colour. In "Standard" mode the glyph follows the system theme, the way Clipy's
//! template icon follows the macOS menu bar; the user can also pick black or white.

use std::future::poll_fn;
use std::pin::Pin;
use std::thread;

use gtk::gio;
use gtk::prelude::*;
use tao::event_loop::EventLoopProxy;
use zbus::export::futures_core::Stream;
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream, OwnedVariant, Variant};

use crate::app::UserEvent;
use crate::paste;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const APPEARANCE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME: &str = "color-scheme";
const CINNAMON_THEME: &str = "org.cinnamon.theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    /// For dark panels.
    White,
    /// For light panels.
    Black,
}

/// What is known about the desktop's colours.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Appearance {
    pub gnome: bool,
    /// `Some(true)` when Cinnamon's panel theme is a dark one.
    pub cinnamon_dark: Option<bool>,
    /// The freedesktop `color-scheme`: 0 no preference, 1 dark, 2 light.
    pub color_scheme: Option<u32>,
    /// From the GTK theme: dark variant preferred, or a theme named "…dark…".
    pub gtk_dark: Option<bool>,
}

/// The glyph that stays visible on the panel.
pub fn choose_glyph(a: &Appearance) -> Glyph {
    // TODO(assumption A-014): how each desktop colours its panel
    //   Assumed:   GNOME's top bar is always dark; Cinnamon's panel is dark when its Cinnamon
    //              theme name says "dark"; other panels follow the colour scheme or GTK theme;
    //              unknown panels are dark.
    //   Question:  Does "Standard" pick a visible glyph on your Mint (light and dark themes) and
    //              Ubuntu setups?
    //   Key points:
    //     - A wrong pick shows a black glyph on a dark panel (or the reverse) until the user
    //       chooses Black or White by hand.
    //     - Only choose_glyph and local_appearance change if wrong.
    // GNOME's top bar is black in both its light and dark styles.
    if a.gnome {
        return Glyph::White;
    }
    // Cinnamon draws its panel with the Cinnamon theme, separate from the app theme.
    if let Some(dark) = a.cinnamon_dark {
        return if dark { Glyph::White } else { Glyph::Black };
    }
    match (a.color_scheme, a.gtk_dark) {
        (Some(1), _) => Glyph::White,
        (Some(2), _) => Glyph::Black,
        (_, Some(false)) => Glyph::Black,
        // Most default panels are dark.
        _ => Glyph::White,
    }
}

/// Reads the parts of [`Appearance`] that GTK and GSettings know. The portal colour scheme
/// arrives separately through [`UserEvent::ColorScheme`].
pub fn local_appearance(color_scheme: Option<u32>) -> Appearance {
    Appearance {
        gnome: paste::is_gnome_session(),
        cinnamon_dark: cinnamon_settings().and_then(|s| {
            let name = s.string("name").to_lowercase();
            // Mint's default panel themes are named "Mint-Y-Dark-…"; a name without "dark"
            // says nothing certain about the panel, so fall back to the colour scheme.
            name.contains("dark").then_some(true)
        }),
        color_scheme,
        gtk_dark: gtk::Settings::default().map(|s| {
            s.is_gtk_application_prefer_dark_theme()
                || s.gtk_theme_name()
                    .is_some_and(|n| n.to_lowercase().contains("dark"))
        }),
    }
}

fn cinnamon_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    source.lookup(CINNAMON_THEME, true)?;
    Some(gio::Settings::new(CINNAMON_THEME))
}

/// Keeps the theme listeners alive.
pub struct ThemeWatcher {
    _cinnamon: Option<gio::Settings>,
}

/// Sends [`UserEvent::ThemeChanged`] when the GTK or Cinnamon theme changes, and
/// [`UserEvent::ColorScheme`] with the portal's colour scheme now and on every change.
pub fn watch(proxy: EventLoopProxy<UserEvent>, bus: Option<Connection>) -> ThemeWatcher {
    if let Some(settings) = gtk::Settings::default() {
        for property in ["gtk-theme-name", "gtk-application-prefer-dark-theme"] {
            let proxy = proxy.clone();
            settings.connect_notify_local(Some(property), move |_, _| {
                let _ = proxy.send_event(UserEvent::ThemeChanged);
            });
        }
    }
    let cinnamon = cinnamon_settings();
    if let Some(settings) = &cinnamon {
        let proxy = proxy.clone();
        settings.connect_changed(Some("name"), move |_, _| {
            let _ = proxy.send_event(UserEvent::ThemeChanged);
        });
    }
    if let Some(conn) = bus {
        let spawned = thread::Builder::new()
            .name("clipy-theme".into())
            .spawn(move || zbus::block_on(follow_portal(conn, proxy)));
        if let Err(e) = spawned {
            log::warn!("cannot follow the colour scheme: {e}");
        }
    }
    ThemeWatcher {
        _cinnamon: cinnamon,
    }
}

/// Reports the portal colour scheme, then follows `SettingChanged`. Runs off the main thread:
/// the portal may take a moment to start, or not exist at all.
async fn follow_portal(conn: Connection, proxy: EventLoopProxy<UserEvent>) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(SETTINGS)
        .member("SettingChanged")
        .path(PORTAL_PATH)
        .build();
    let mut stream = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, &conn, None).await.ok(),
        Err(e) => {
            log::warn!("portal match rule: {e}");
            None
        }
    };
    let _ = proxy.send_event(UserEvent::ColorScheme(read_color_scheme(&conn).await));
    let Some(stream) = stream.as_mut() else {
        return;
    };
    while let Some(message) = poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await {
        let Ok(message) = message else { continue };
        let Ok((namespace, key, value)) = message
            .body()
            .deserialize::<(String, String, OwnedVariant)>()
        else {
            continue;
        };
        if namespace == APPEARANCE && key == COLOR_SCHEME {
            let _ = proxy.send_event(UserEvent::ColorScheme(as_u32(&value)));
        }
    }
}

async fn read_color_scheme(conn: &Connection) -> Option<u32> {
    // ReadOne (portal version 2) returns the value; the older Read wraps it in a second variant.
    for method in ["ReadOne", "Read"] {
        let reply = conn
            .call_method(
                Some(PORTAL),
                PORTAL_PATH,
                Some(SETTINGS),
                method,
                &(APPEARANCE, COLOR_SCHEME),
            )
            .await;
        if let Ok(reply) = reply {
            if let Ok(value) = reply.body().deserialize::<OwnedVariant>() {
                return as_u32(&value);
            }
        }
    }
    None
}

fn as_u32(value: &Variant<'_>) -> Option<u32> {
    match value {
        Variant::U32(n) => Some(*n),
        Variant::Variant(inner) => as_u32(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_top_bar_is_always_dark() {
        let a = Appearance {
            gnome: true,
            color_scheme: Some(2),
            gtk_dark: Some(false),
            ..Default::default()
        };
        assert_eq!(choose_glyph(&a), Glyph::White);
    }

    #[test]
    fn cinnamon_panel_theme_wins_over_app_theme() {
        let a = Appearance {
            cinnamon_dark: Some(true),
            color_scheme: Some(2),
            ..Default::default()
        };
        assert_eq!(choose_glyph(&a), Glyph::White);
    }

    #[test]
    fn colour_scheme_then_gtk_theme() {
        let scheme = |s| Appearance {
            color_scheme: Some(s),
            gtk_dark: Some(true),
            ..Default::default()
        };
        assert_eq!(choose_glyph(&scheme(1)), Glyph::White);
        assert_eq!(choose_glyph(&scheme(2)), Glyph::Black);
        assert_eq!(choose_glyph(&scheme(0)), Glyph::White);
        let gtk = |d| Appearance {
            gtk_dark: Some(d),
            ..Default::default()
        };
        assert_eq!(choose_glyph(&gtk(false)), Glyph::Black);
        assert_eq!(choose_glyph(&gtk(true)), Glyph::White);
        assert_eq!(choose_glyph(&Appearance::default()), Glyph::White);
    }

    #[test]
    fn nested_variants_unwrap() {
        let v = Variant::Variant(Box::new(Variant::U32(1)));
        assert_eq!(as_u32(&v), Some(1));
        assert_eq!(as_u32(&Variant::Bool(true)), None);
    }
}
