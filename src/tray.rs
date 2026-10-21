//! Panel icon (StatusNotifierItem/AppIndicator through tray-icon) and its menu, rendered with
//! muda from the menu model.
//!
//! AppIndicator menus travel over D-Bus (dbusmenu), which has no tooltips and no custom
//! accelerators; the X11 popup and the GNOME extension show those.

use anyhow::{anyhow, Result};
use gtk::prelude::*;
use tray_icon::menu::{
    Icon as MenuIcon, IconMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::{MenuPrefs, PanelIcon};
use crate::images;
use crate::menu::{self, Icon as NodeIcon, MenuNode};
use crate::theme::Glyph;

const STATUS_WHITE: &[u8] = include_bytes!("../resources/icons/status_white.png");
const STATUS_BLACK: &[u8] = include_bytes!("../resources/icons/status_black.png");
const MENU_ICON_SIZE: i32 = 16;

pub struct Tray {
    icon: TrayIcon,
    /// What is shown now: the glyph, or `None` when hidden.
    shown: Option<Glyph>,
}

impl Tray {
    /// `auto` is the glyph that suits the current theme, used in Standard mode.
    pub fn new(
        nodes: &[MenuNode],
        prefs: &MenuPrefs,
        mode: PanelIcon,
        auto: Glyph,
    ) -> Result<Self> {
        let shown = glyph_for(mode, auto);
        let icon = TrayIconBuilder::new()
            .with_id(crate::dbus::BUS_NAME)
            .with_tooltip("Clipy")
            .with_icon(panel_icon(shown.unwrap_or(auto))?)
            .with_menu(Box::new(build_menu(nodes, prefs)))
            .build()
            .map_err(|e| anyhow!("creating the panel icon: {e}"))?;
        icon.set_visible(shown.is_some())
            .map_err(|e| anyhow!("{e}"))?;
        Ok(Self { icon, shown })
    }

    pub fn set_menu(&self, nodes: &[MenuNode], prefs: &MenuPrefs) {
        self.icon.set_menu(Some(Box::new(build_menu(nodes, prefs))));
    }

    /// Applies the panel icon setting; `auto` is the glyph for the current theme.
    pub fn set_mode(&mut self, mode: PanelIcon, auto: Glyph) -> Result<()> {
        let shown = glyph_for(mode, auto);
        if shown == self.shown {
            return Ok(());
        }
        if let Some(glyph) = shown {
            self.icon
                .set_icon(Some(panel_icon(glyph)?))
                .map_err(|e| anyhow!("{e}"))?;
        }
        self.icon
            .set_visible(shown.is_some())
            .map_err(|e| anyhow!("{e}"))?;
        self.shown = shown;
        Ok(())
    }
}

fn glyph_for(mode: PanelIcon, auto: Glyph) -> Option<Glyph> {
    match mode {
        PanelIcon::Standard => Some(auto),
        PanelIcon::Black => Some(Glyph::Black),
        PanelIcon::White => Some(Glyph::White),
        PanelIcon::Hidden => None,
    }
}

fn panel_icon(glyph: Glyph) -> Result<Icon> {
    let png = match glyph {
        Glyph::White => STATUS_WHITE,
        Glyph::Black => STATUS_BLACK,
    };
    let (rgba, w, h) = images::decode_png(png)?;
    Icon::from_rgba(rgba, w, h).map_err(|e| anyhow!("{e}"))
}

fn build_menu(nodes: &[MenuNode], prefs: &MenuPrefs) -> Menu {
    let menu = Menu::new();
    for node in nodes {
        if let Err(e) = menu.append(build_item(node, prefs).as_ref()) {
            log::warn!("tray menu: {e}");
        }
    }
    menu
}

fn build_item(node: &MenuNode, prefs: &MenuPrefs) -> Box<dyn IsMenuItem> {
    match node {
        MenuNode::Header(label) => Box::new(MenuItem::new(escape(label), false, None)),
        MenuNode::Separator => Box::new(PredefinedMenuItem::separator()),
        MenuNode::Action { id, label, enabled } => Box::new(MenuItem::with_id(
            MenuId::new(id.encode()),
            escape(label),
            *enabled,
            None,
        )),
        MenuNode::Submenu {
            label, children, ..
        } => {
            let sub = Submenu::new(escape(label), true);
            for child in children {
                if let Err(e) = sub.append(build_item(child, prefs).as_ref()) {
                    log::warn!("tray menu: {e}");
                }
            }
            Box::new(sub)
        }
        MenuNode::Item {
            id,
            label,
            icon,
            preview,
            ..
        } => {
            let image = preview
                .as_ref()
                .and_then(|p| menu::preview_rgba(p, prefs))
                .or_else(|| theme_icon(*icon));
            let id = MenuId::new(id.encode());
            match image.and_then(|(rgba, w, h)| MenuIcon::from_rgba(rgba, w, h).ok()) {
                Some(icon) => Box::new(IconMenuItem::with_id(
                    id,
                    escape(label),
                    true,
                    Some(icon),
                    None,
                )),
                None => Box::new(MenuItem::with_id(id, escape(label), true, None)),
            }
        }
    }
}

/// muda reads `&` as a mnemonic marker; clipboard text must show it literally.
fn escape(label: &str) -> String {
    label.replace('&', "&&")
}

/// RGBA of a themed icon, so menu icons follow the desktop theme.
pub fn theme_icon(icon: NodeIcon) -> Option<(Vec<u8>, u32, u32)> {
    let name = match icon {
        NodeIcon::None => return None,
        NodeIcon::Folder => "folder",
        NodeIcon::Snippet => "text-x-generic",
    };
    let pixbuf = gtk::IconTheme::default()?
        .load_icon(name, MENU_ICON_SIZE, gtk::IconLookupFlags::FORCE_SIZE)
        .ok()??;
    pixbuf_rgba(&pixbuf)
}

fn pixbuf_rgba(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Option<(Vec<u8>, u32, u32)> {
    if !pixbuf.has_alpha() || pixbuf.n_channels() != 4 || pixbuf.bits_per_sample() != 8 {
        return None;
    }
    let (w, h, stride) = (
        pixbuf.width() as usize,
        pixbuf.height() as usize,
        pixbuf.rowstride() as usize,
    );
    let bytes = pixbuf.read_pixel_bytes();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        rgba.extend_from_slice(bytes.get(row * stride..row * stride + w * 4)?);
    }
    Some((rgba, w as u32, h as u32))
}
