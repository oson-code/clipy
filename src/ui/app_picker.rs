//! Chooser for the Exclude pane: the installed applications, searchable, several at a time.

use gtk::gio;
use gtk::prelude::*;

use crate::config::ExcludedApp;
use crate::i18n::tr;

/// Shows the chooser; `on_chosen` receives the selected apps when the user confirms.
pub fn choose_apps(parent: &gtk::Window, on_chosen: impl Fn(Vec<ExcludedApp>) + 'static) {
    let dialog = gtk::Dialog::with_buttons(
        Some(tr("Add Apps…")),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            (tr("Cancel"), gtk::ResponseType::Cancel),
            (tr("Add"), gtk::ResponseType::Accept),
        ],
    );
    dialog.set_default_size(420, 480);
    dialog.set_default_response(gtk::ResponseType::Accept);

    let search = gtk::SearchEntry::new();
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Multiple);
    list.set_activate_on_single_click(false);

    let mut apps: Vec<(gio::AppInfo, String)> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .map(|a| {
            let name = a.display_name().to_string();
            (a, name)
        })
        .collect();
    apps.sort_by_key(|(_, name)| name.to_lowercase());
    let mut entries = Vec::new();
    for (info, name) in apps {
        let Some(id) = info.id().map(|i| i.to_string()) else {
            continue;
        };
        let wm_class = info
            .downcast_ref::<gio::DesktopAppInfo>()
            .and_then(|d| d.startup_wm_class())
            .map(|c| c.to_string());
        list.add(&app_row(info.icon().as_ref(), &name, &id));
        entries.push(ExcludedApp {
            desktop_id: id,
            name,
            wm_class,
        });
    }

    let filter_text = search.clone();
    let names: Vec<String> = entries
        .iter()
        .map(|e| format!("{} {}", e.name, e.desktop_id).to_lowercase())
        .collect();
    list.set_filter_func(Some(Box::new(move |row| {
        let needle = filter_text.text().to_lowercase();
        needle.is_empty()
            || names
                .get(row.index() as usize)
                .is_some_and(|n| n.contains(&needle))
    })));
    let refilter = list.clone();
    search.connect_search_changed(move |_| refilter.invalidate_filter());

    let scroll = gtk::ScrolledWindow::builder().vexpand(true).build();
    scroll.add(&list);
    let content = dialog.content_area();
    content.set_spacing(6);
    content.pack_start(&search, false, false, 6);
    content.pack_start(&scroll, true, true, 0);

    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            let chosen = list
                .selected_rows()
                .iter()
                .filter_map(|row| entries.get(row.index() as usize).cloned())
                .collect::<Vec<_>>();
            if !chosen.is_empty() {
                on_chosen(chosen);
            }
        }
        dialog.close();
    });
    dialog.show_all();
}

/// Icon, name and desktop id, as in Clipy's Exclude list.
pub fn app_row(icon: Option<&gio::Icon>, name: &str, desktop_id: &str) -> gtk::Box {
    let image = match icon {
        Some(icon) => gtk::Image::from_gicon(icon, gtk::IconSize::Dnd),
        None => gtk::Image::from_icon_name(Some("application-x-executable"), gtk::IconSize::Dnd),
    };
    image.set_pixel_size(28);
    let title = gtk::Label::new(Some(name));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let id = crate::ui::widgets::caption(desktop_id);
    id.set_line_wrap(false);
    id.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.pack_start(&title, false, false, 0);
    text.pack_start(&id, false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.set_margin_start(6);
    row.set_margin_end(6);
    row.pack_start(&image, false, false, 0);
    row.pack_start(&text, true, true, 0);
    row
}
