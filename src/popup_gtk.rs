//! The hotkey menu on X11: a native GtkMenu at the mouse pointer, with Clipy's numbered titles,
//! tooltips, thumbnails and number-key shortcuts.

use std::rc::Rc;

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;
use gtk::prelude::*;

use crate::config::{MenuPrefs, Modifiers};
use crate::menu::{self, Icon, ItemId, MenuNode};

pub type OnActivate = Rc<dyn Fn(ItemId, Modifiers)>;

/// Pops the menu up at the pointer. Keep the returned menu alive until it closes.
pub fn popup(nodes: &[MenuNode], prefs: &MenuPrefs, on_activate: OnActivate) -> gtk::Menu {
    let accels = Rc::new(std::cell::RefCell::new(Vec::new()));
    let root = build(nodes, prefs, &on_activate, &accels);
    let accels = Rc::new(accels.take());
    if !accels.is_empty() {
        connect_number_keys(&root, &root, &accels);
    }
    root.show_all();
    // A global shortcut gives no trigger event, which popup_at_pointer needs for its anchor
    // window; the older call positions the menu at the default seat's pointer instead.
    #[allow(deprecated)]
    root.popup_easy(0, gtk::current_event_time());
    root
}

type Accels = Rc<std::cell::RefCell<Vec<(char, gtk::MenuItem)>>>;

fn build(
    nodes: &[MenuNode],
    prefs: &MenuPrefs,
    on_activate: &OnActivate,
    accels: &Accels,
) -> gtk::Menu {
    let menu = gtk::Menu::new();
    for node in nodes {
        let item: gtk::MenuItem = match node {
            MenuNode::Header(label) => {
                let item = gtk::MenuItem::with_label(label);
                item.set_sensitive(false);
                item
            }
            MenuNode::Separator => gtk::SeparatorMenuItem::new().upcast(),
            MenuNode::Submenu {
                label,
                icon,
                children,
            } => {
                let item = labelled_item(label, icon_image(*icon), None);
                item.set_submenu(Some(&build(children, prefs, on_activate, accels)));
                item
            }
            MenuNode::Action { id, label, enabled } => {
                let item = gtk::MenuItem::with_label(label);
                item.set_sensitive(*enabled);
                connect_activate(&item, *id, on_activate);
                item
            }
            MenuNode::Item {
                id,
                label,
                tooltip,
                icon,
                preview,
                accel,
            } => {
                let image = preview
                    .as_ref()
                    .and_then(|p| menu::preview_rgba(p, prefs))
                    .map(|(rgba, w, h)| gtk::Image::from_pixbuf(Some(&rgba_pixbuf(rgba, w, h))))
                    .or_else(|| icon_image(*icon));
                let item = labelled_item(label, image, *accel);
                item.set_tooltip_text(tooltip.as_deref());
                connect_activate(&item, *id, on_activate);
                if let Some(c) = accel {
                    accels.borrow_mut().push((*c, item.clone()));
                }
                item
            }
        };
        menu.append(&item);
    }
    menu
}

fn connect_activate(item: &gtk::MenuItem, id: ItemId, on_activate: &OnActivate) {
    let on_activate = on_activate.clone();
    item.connect_activate(move |_| {
        let modifiers = gtk::current_event_state().map(|s| s.bits()).unwrap_or(0);
        on_activate(id, Modifiers(modifiers));
    });
}

/// A menu row: optional image, the title with tabular digits, and the number key on the right.
fn labelled_item(label: &str, image: Option<gtk::Image>, accel: Option<char>) -> gtk::MenuItem {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if let Some(image) = image {
        row.pack_start(&image, false, false, 0);
    }
    let title = gtk::Label::new(None);
    title.set_markup(&format!(
        "<span font_features=\"tnum\">{}</span>",
        glib::markup_escape_text(label)
    ));
    title.set_xalign(0.0);
    row.pack_start(&title, true, true, 0);
    if let Some(c) = accel {
        let key = gtk::Label::new(Some(&c.to_string()));
        key.style_context().add_class("dim-label");
        row.pack_end(&key, false, false, 0);
    }
    let item = gtk::MenuItem::new();
    item.add(&row);
    item
}

fn icon_image(icon: Icon) -> Option<gtk::Image> {
    let name = match icon {
        Icon::None => return None,
        Icon::Folder => "folder-symbolic",
        Icon::Snippet => "text-x-generic-symbolic",
    };
    Some(gtk::Image::from_icon_name(Some(name), gtk::IconSize::Menu))
}

fn rgba_pixbuf(rgba: Vec<u8>, width: u32, height: u32) -> Pixbuf {
    Pixbuf::from_bytes(
        &glib::Bytes::from_owned(rgba),
        Colorspace::Rgb,
        true,
        8,
        width as i32,
        height as i32,
        (width * 4) as i32,
    )
}

/// Number keys pick the matching history entry from any open level of the menu.
fn connect_number_keys(
    menu: &gtk::Menu,
    root: &gtk::Menu,
    accels: &Rc<Vec<(char, gtk::MenuItem)>>,
) {
    let root_ref = root.downgrade();
    let table = accels.clone();
    menu.connect_key_press_event(move |_, event| {
        let pressed = event.keyval().to_unicode();
        let Some((_, item)) = table.iter().find(|(c, _)| Some(*c) == pressed) else {
            return glib::Propagation::Proceed;
        };
        if let Some(root) = root_ref.upgrade() {
            root.deactivate();
        }
        item.activate();
        glib::Propagation::Stop
    });
    for child in menu.children() {
        if let Some(sub) = child
            .downcast_ref::<gtk::MenuItem>()
            .and_then(|i| i.submenu())
        {
            if let Ok(sub) = sub.downcast::<gtk::Menu>() {
                connect_number_keys(&sub, root, accels);
            }
        }
    }
}
