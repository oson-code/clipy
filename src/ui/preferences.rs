//! The Preferences window: Clipy's General, Menu, Type, Exclude, Shortcuts and Beta panes.
//! Every change is applied immediately; there is no OK button, as in Clipy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::app::UserEvent;
use crate::config::{ClearInterval, ModifierKey, PanelIcon, Settings, SortBy};
use crate::hotkeys::Action;
use crate::i18n::tr;
use crate::screenshots;
use crate::ui::app_picker;
use crate::ui::shortcut_recorder::ShortcutRecorder;
use crate::ui::widgets::{
    caption, check, combo, depends_on, hbox, spin, spin_with_unit, Ctx, Pane,
};

struct PaneInfo {
    name: &'static str,
    title: &'static str,
    icon: &'static str,
}

const PANES: [PaneInfo; 6] = [
    PaneInfo {
        name: "general",
        title: "General",
        icon: "preferences-system-symbolic",
    },
    PaneInfo {
        name: "menu",
        title: "Menu",
        icon: "open-menu-symbolic",
    },
    PaneInfo {
        name: "type",
        title: "Type",
        icon: "edit-paste-symbolic",
    },
    PaneInfo {
        name: "exclude",
        title: "Exclude",
        icon: "action-unavailable-symbolic",
    },
    PaneInfo {
        name: "shortcuts",
        title: "Shortcuts",
        icon: "input-keyboard-symbolic",
    },
    PaneInfo {
        name: "beta",
        title: "Beta",
        icon: "applications-science-symbolic",
    },
];

pub struct PreferencesWindow {
    window: gtk::Window,
    recorders: Rc<HashMap<Action, ShortcutRecorder>>,
}

impl PreferencesWindow {
    pub fn new(ctx: Ctx) -> Self {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title(tr(PANES[0].title));
        window.set_resizable(false);
        window.set_position(gtk::WindowPosition::Center);

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_vhomogeneous(false);
        stack.set_hhomogeneous(false);
        let recorders = Rc::new(shortcut_recorders(&ctx));
        let panes = [
            general_pane(&ctx),
            menu_pane(&ctx),
            type_pane(&ctx),
            exclude_pane(&ctx, &window),
            shortcuts_pane(&recorders),
            beta_pane(&ctx),
        ];
        for (info, pane) in PANES.iter().zip(panes) {
            stack.add_named(&pane, info.name);
        }

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toolbar.set_halign(gtk::Align::Center);
        toolbar.set_margin_top(6);
        let mut group: Option<gtk::RadioButton> = None;
        for info in &PANES {
            let button = match &group {
                Some(first) => gtk::RadioButton::from_widget(first),
                None => gtk::RadioButton::new(),
            };
            button.set_mode(false);
            button.set_relief(gtk::ReliefStyle::None);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let icon = gtk::Image::from_icon_name(Some(info.icon), gtk::IconSize::LargeToolbar);
            content.pack_start(&icon, false, false, 0);
            content.pack_start(&gtk::Label::new(Some(tr(info.title))), false, false, 0);
            content.set_margin_start(8);
            content.set_margin_end(8);
            button.add(&content);
            let (stack, window, name, title) =
                (stack.clone(), window.clone(), info.name, info.title);
            button.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name(name);
                    window.set_title(tr(title));
                    // Shrink to the new pane, as Clipy's window does.
                    window.resize(1, 1);
                }
            });
            toolbar.pack_start(&button, false, false, 0);
            group.get_or_insert(button);
        }

        let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
        layout.pack_start(&toolbar, false, false, 0);
        layout.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            6,
        );
        // Long panes (Menu) scroll instead of growing past small screens.
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .max_content_height(540)
            .build();
        scroll.add(&stack);
        layout.pack_start(&scroll, true, true, 0);
        window.add(&layout);

        let proxy = ctx.proxy.clone();
        window.connect_destroy(move |_| {
            let _ = proxy.send_event(UserEvent::PreferencesClosed);
        });
        window.show_all();
        Self { window, recorders }
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Shows registration problems under the matching shortcut recorders.
    pub fn set_shortcut_errors(&self, errors: &[(Action, String)]) {
        for (action, recorder) in self.recorders.iter() {
            let message = errors
                .iter()
                .find(|(a, _)| a == action)
                .map(|(_, m)| m.as_str());
            recorder.set_error(message);
        }
    }
}

fn general_pane(ctx: &Ctx) -> gtk::Widget {
    let mut p = Pane::new();
    let login = check(
        ctx,
        tr("Launch on Login"),
        |s| s.general.launch_at_login,
        |s, v| s.general.launch_at_login = v,
    );
    p.section(tr("Behavior:"), &login);
    p.row(&check(
        ctx,
        tr("Paste automatically after selection"),
        |s| s.general.paste_automatically,
        |s, v| s.general.paste_automatically = v,
    ));
    let limit = spin(
        ctx,
        1,
        1000,
        |s| s.general.max_history,
        |s, v| s.general.max_history = v,
    );
    p.section(tr("History limit:"), &spin_with_unit(&limit, tr("items")));
    p.separator();
    p.section(
        tr("Sort history by:"),
        &combo(
            ctx,
            &[SortBy::DateCreated, SortBy::LastUsed],
            |v| match v {
                SortBy::DateCreated => tr("Date Created"),
                SortBy::LastUsed => tr("Last Used"),
            },
            |s| s.general.sort_by,
            |s, v| s.general.sort_by = v,
        ),
    );
    p.separator();
    p.section(
        &format!("{}:", tr("Automatic History Deletion")),
        &check(
            ctx,
            tr("Clear history when Clipy quits"),
            |s| s.general.clear_on_quit,
            |s, v| s.general.clear_on_quit = v,
        ),
    );
    let periodic = check(
        ctx,
        tr("Clear History Periodically"),
        |s| s.general.clear_periodically,
        |s, v| s.general.clear_periodically = v,
    );
    p.row(&periodic);
    let interval = combo(
        ctx,
        &ClearInterval::ALL,
        ClearInterval::label,
        |s| s.general.clear_interval,
        |s, v| s.general.clear_interval = v,
    );
    depends_on(&interval, &periodic);
    p.indented(&interval);
    p.caption(tr("Clears all history at the selected interval while Clipy is running. Snippets are not deleted."));
    p.separator();
    p.section(
        tr("Panel icon:"),
        &combo(
            ctx,
            &PanelIcon::ALL,
            PanelIcon::label,
            |s| s.general.panel_icon,
            |s, v| s.general.panel_icon = v,
        ),
    );
    p.caption(tr(
        "Standard follows the system theme. When hidden, open Preferences with its shortcut or by starting Clipy again.",
    ));
    p.widget()
}

fn menu_pane(ctx: &Ctx) -> gtk::Widget {
    let mut p = Pane::new();
    p.section(
        tr("History items shown at the top level:"),
        &spin(
            ctx,
            0,
            100,
            |s| s.menu.inline_items,
            |s, v| s.menu.inline_items = v,
        ),
    );
    p.caption(tr(
        "Additional history is grouped into folders. Set to 0 to group all history into folders.",
    ));
    p.section(
        tr("Items per folder:"),
        &spin(
            ctx,
            1,
            100,
            |s| s.menu.items_per_folder,
            |s, v| s.menu.items_per_folder = v,
        ),
    );
    let length = spin(
        ctx,
        1,
        1000,
        |s| s.menu.max_title_len,
        |s, v| s.menu.max_title_len = v,
    );
    p.section(
        tr("Maximum displayed text length:"),
        &spin_with_unit(&length, tr("characters")),
    );
    p.separator();

    let dup = check(
        ctx,
        tr("Save duplicate clipboard content"),
        |s| s.menu.copy_same_history,
        |s, v| s.menu.copy_same_history = v,
    );
    p.section(tr("Clipboard History:"), &dup);
    let replace = check(
        ctx,
        tr("Replace the previous entry"),
        |s| s.menu.overwrite_same_history,
        |s, v| s.menu.overwrite_same_history = v,
    );
    depends_on(&replace, &dup);
    p.indented(&replace);
    p.separator();

    p.section(
        tr("Menu Items:"),
        &check(
            ctx,
            tr("Show icons in menu items"),
            |s| s.menu.show_icons,
            |s, v| s.menu.show_icons = v,
        ),
    );
    let numbers = check(
        ctx,
        tr("Mark menu items with numbers"),
        |s| s.menu.mark_numbers,
        |s, v| s.menu.mark_numbers = v,
    );
    p.row(&numbers);
    let zero = check(
        ctx,
        tr("Start numbering at 0"),
        |s| s.menu.start_at_zero,
        |s, v| s.menu.start_at_zero = v,
    );
    depends_on(&zero, &numbers);
    p.indented(&zero);
    p.row(&check(
        ctx,
        tr("Use number keys as shortcuts"),
        |s| s.menu.numeric_keys,
        |s, v| s.menu.numeric_keys = v,
    ));
    p.row(&check(
        ctx,
        tr("Show snippets above history"),
        |s| s.menu.snippets_above_history,
        |s, v| s.menu.snippets_above_history = v,
    ));
    p.separator();

    let clear_item = check(
        ctx,
        tr("Add a menu item to clear clipboard history"),
        |s| s.menu.show_clear_item,
        |s, v| s.menu.show_clear_item = v,
    );
    p.section(tr("Clear History:"), &clear_item);
    let warn = check(
        ctx,
        tr("Show a warning before clearing history"),
        |s| s.menu.confirm_clear,
        |s, v| s.menu.confirm_clear = v,
    );
    depends_on(&warn, &clear_item);
    p.indented(&warn);
    p.separator();

    let tooltips = check(
        ctx,
        tr("Show tooltips on menu items"),
        |s| s.menu.show_tooltips,
        |s, v| s.menu.show_tooltips = v,
    );
    p.section(tr("Tooltips:"), &tooltips);
    let tooltip_len = spin(
        ctx,
        1,
        10_000,
        |s| s.menu.tooltip_len,
        |s, v| s.menu.tooltip_len = v,
    );
    let tooltip_row = hbox(&[
        gtk::Label::new(Some(tr("Maximum characters:"))).upcast_ref(),
        tooltip_len.upcast_ref(),
        gtk::Label::new(Some(tr("characters"))).upcast_ref(),
    ]);
    depends_on(&tooltip_row, &tooltips);
    p.indented(&tooltip_row);
    p.separator();

    let images = check(
        ctx,
        tr("Show images"),
        |s| s.menu.show_images,
        |s, v| s.menu.show_images = v,
    );
    p.section(tr("Previews:"), &images);
    let width = spin(ctx, 1, 512, |s| s.menu.thumb_w, |s, v| s.menu.thumb_w = v);
    let height = spin(ctx, 1, 512, |s| s.menu.thumb_h, |s, v| s.menu.thumb_h = v);
    let size_row = hbox(&[
        gtk::Label::new(Some(tr("Width"))).upcast_ref(),
        spin_with_unit(&width, tr("px")).upcast_ref(),
        gtk::Label::new(Some(tr("Height"))).upcast_ref(),
        spin_with_unit(&height, tr("px")).upcast_ref(),
    ]);
    depends_on(&size_row, &images);
    p.indented(&size_row);
    p.row(&check(
        ctx,
        tr("Show color code preview"),
        |s| s.menu.show_color_preview,
        |s, v| s.menu.show_color_preview = v,
    ));
    p.widget()
}

fn type_pane(ctx: &Ctx) -> gtk::Widget {
    let mut p = Pane::new();
    p.section(
        tr("Save to History:"),
        &check(
            ctx,
            tr("Plain Text"),
            |s| s.types.text,
            |s, v| s.types.text = v,
        ),
    );
    p.row(&check(
        ctx,
        tr("HTML"),
        |s| s.types.html,
        |s, v| s.types.html = v,
    ));
    p.row(&check(
        ctx,
        tr("Images"),
        |s| s.types.images,
        |s, v| s.types.images = v,
    ));
    p.row(&check(
        ctx,
        tr("Files"),
        |s| s.types.files,
        |s, v| s.types.files = v,
    ));
    p.separator();
    p.row(&check(
        ctx,
        tr("Do not save content marked as confidential to history"),
        |s| s.types.ignore_confidential,
        |s, v| s.types.ignore_confidential = v,
    ));
    p.caption(tr(
        "Some apps, such as password managers, mark sensitive content as confidential.",
    ));
    p.widget()
}

fn exclude_pane(ctx: &Ctx, window: &gtk::Window) -> gtk::Widget {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Multiple);
    let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 6);
    placeholder.set_valign(gtk::Align::Center);
    let empty_icon = gtk::Image::from_icon_name(
        Some("application-x-executable-symbolic"),
        gtk::IconSize::Dialog,
    );
    empty_icon.style_context().add_class("dim-label");
    let empty_title = gtk::Label::new(None);
    empty_title.set_markup(&format!(
        "<b>{}</b>",
        glib::markup_escape_text(tr("No Excluded Apps"))
    ));
    placeholder.pack_start(&empty_icon, false, false, 0);
    placeholder.pack_start(&empty_title, false, false, 0);
    let hint = caption(tr("Add an app to stop saving its clipboard history."));
    hint.set_xalign(0.5);
    hint.set_justify(gtk::Justification::Center);
    placeholder.pack_start(&hint, false, false, 0);
    placeholder.show_all();
    list.set_placeholder(Some(&placeholder));
    fill_excluded(&list, &ctx.get());

    let scroll = gtk::ScrolledWindow::builder()
        .min_content_width(500)
        .min_content_height(320)
        .build();
    scroll.set_shadow_type(gtk::ShadowType::In);
    scroll.add(&list);

    let add = gtk::Button::from_icon_name(Some("list-add-symbolic"), gtk::IconSize::Button);
    add.set_tooltip_text(Some(tr("Add Apps…")));
    let remove = gtk::Button::from_icon_name(Some("list-remove-symbolic"), gtk::IconSize::Button);
    remove.set_tooltip_text(Some(tr("Remove")));
    remove.set_sensitive(false);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    buttons.style_context().add_class("linked");
    buttons.pack_start(&add, false, false, 0);
    buttons.pack_start(&remove, false, false, 0);

    {
        let remove = remove.clone();
        list.connect_selected_rows_changed(move |l| {
            remove.set_sensitive(!l.selected_rows().is_empty())
        });
    }
    let remove_selected = {
        let (ctx, list) = (ctx.clone(), list.clone());
        Rc::new(move || {
            let mut indexes: Vec<usize> = list
                .selected_rows()
                .iter()
                .map(|r| r.index() as usize)
                .collect();
            if indexes.is_empty() {
                return;
            }
            indexes.sort_unstable_by(|a, b| b.cmp(a));
            ctx.update(|s| {
                for i in &indexes {
                    if *i < s.excluded_apps.len() {
                        s.excluded_apps.remove(*i);
                    }
                }
            });
            fill_excluded(&list, &ctx.get());
        })
    };
    {
        let remove_selected = remove_selected.clone();
        remove.connect_clicked(move |_| remove_selected());
    }
    list.connect_key_press_event(move |_, event| {
        let key = *event.keyval();
        if key == *gtk::gdk::keys::constants::Delete || key == *gtk::gdk::keys::constants::BackSpace
        {
            remove_selected();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    {
        let (ctx, list, window) = (ctx.clone(), list.clone(), window.clone());
        add.connect_clicked(move |_| {
            let (ctx, list) = (ctx.clone(), list.clone());
            app_picker::choose_apps(&window, move |chosen| {
                ctx.update(|s| {
                    for app in chosen {
                        if !s
                            .excluded_apps
                            .iter()
                            .any(|e| e.desktop_id == app.desktop_id)
                        {
                            s.excluded_apps.push(app);
                        }
                    }
                });
                fill_excluded(&list, &ctx.get());
            });
        });
    }

    let mut p = Pane::new();
    let title = gtk::Label::new(Some(tr("Excluded applications")));
    title.set_xalign(0.0);
    p.full(&title);
    p.full(&scroll);
    p.full(&buttons);
    p.widget()
}

fn fill_excluded(list: &gtk::ListBox, settings: &Settings) {
    for child in list.children() {
        list.remove(&child);
    }
    for app in &settings.excluded_apps {
        let icon = gio::DesktopAppInfo::new(&app.desktop_id).and_then(|info| info.icon());
        let row = app_picker::app_row(icon.as_ref(), &app.name, &app.desktop_id);
        row.show_all();
        list.add(&row);
    }
}

const SHORTCUT_ROWS: [Action; 6] = [
    Action::MainMenu,
    Action::HistoryMenu,
    Action::SnippetsMenu,
    Action::ClearHistory,
    Action::EditSnippets,
    Action::Preferences,
];

fn shortcut_recorders(ctx: &Ctx) -> HashMap<Action, ShortcutRecorder> {
    let current = ctx.get().shortcuts;
    SHORTCUT_ROWS
        .iter()
        .map(|&action| {
            let accel = match action {
                Action::MainMenu => &current.main,
                Action::HistoryMenu => &current.history,
                Action::SnippetsMenu => &current.snippets,
                Action::ClearHistory => &current.clear_history,
                Action::EditSnippets => &current.edit_snippets,
                Action::Preferences => &current.preferences,
                Action::Folder(_) => unreachable!("folder shortcuts live in the snippet editor"),
            };
            let ctx = ctx.clone();
            let recorder =
                ShortcutRecorder::new(accel, action != Action::Preferences, move |accel| {
                    ctx.update(|s| {
                        let slot = match action {
                            Action::MainMenu => &mut s.shortcuts.main,
                            Action::HistoryMenu => &mut s.shortcuts.history,
                            Action::SnippetsMenu => &mut s.shortcuts.snippets,
                            Action::ClearHistory => &mut s.shortcuts.clear_history,
                            Action::EditSnippets => &mut s.shortcuts.edit_snippets,
                            Action::Preferences | Action::Folder(_) => &mut s.shortcuts.preferences,
                        };
                        *slot = accel;
                    })
                });
            (action, recorder)
        })
        .collect()
}

fn shortcuts_pane(recorders: &HashMap<Action, ShortcutRecorder>) -> gtk::Widget {
    let mut p = Pane::new();
    for (i, action) in SHORTCUT_ROWS.iter().enumerate() {
        if i == 3 || i == 5 {
            p.separator();
        }
        p.section(action.label(), recorders[action].widget());
    }
    p.caption(tr(
        "To keep Preferences accessible even if you cannot open the menu, this shortcut can be changed but cannot be removed.",
    ));
    p.widget()
}

/// A Beta modifier action: its label, on/off accessors and modifier key accessors.
type BetaAction = (
    &'static str,
    fn(&Settings) -> bool,
    fn(&mut Settings, bool),
    fn(&Settings) -> ModifierKey,
    fn(&mut Settings, ModifierKey),
);

fn beta_pane(ctx: &Ctx) -> gtk::Widget {
    let mut p = Pane::new();
    p.full(&caption(tr(
        "Beta feature settings may not be preserved after an update.",
    )));
    let actions: [BetaAction; 3] = [
        (
            tr("Paste as plain text"),
            |s| s.beta.paste_plain_text,
            |s, v| s.beta.paste_plain_text = v,
            |s| s.beta.paste_plain_text_modifier,
            |s, v| s.beta.paste_plain_text_modifier = v,
        ),
        (
            tr("Delete from history"),
            |s| s.beta.delete_history,
            |s, v| s.beta.delete_history = v,
            |s| s.beta.delete_history_modifier,
            |s, v| s.beta.delete_history_modifier = v,
        ),
        (
            tr("Paste and delete from history"),
            |s| s.beta.paste_and_delete,
            |s, v| s.beta.paste_and_delete = v,
            |s| s.beta.paste_and_delete_modifier,
            |s, v| s.beta.paste_and_delete_modifier = v,
        ),
    ];
    for (i, (label, get, set, get_mod, set_mod)) in actions.into_iter().enumerate() {
        let toggle = check(ctx, label, get, set);
        let key = combo(ctx, &ModifierKey::ALL, ModifierKey::label, get_mod, set_mod);
        depends_on(&key, &toggle);
        let row = hbox(&[toggle.upcast_ref(), key.upcast_ref()]);
        if i == 0 {
            p.section(tr("Modifier Key Actions:"), &row);
        } else {
            p.row(&row);
        }
    }
    p.caption(tr(
        "Hold the selected key while choosing a history item to perform the action.",
    ));
    p.separator();
    let observe = check(
        ctx,
        tr("Save screenshots in history"),
        |s| s.beta.observe_screenshots,
        |s, v| s.beta.observe_screenshots = v,
    );
    p.section(tr("Screenshot:"), &observe);
    let folder = screenshot_folder(ctx);
    depends_on(&folder, &observe);
    p.indented(&folder);
    let prefix = gtk::Entry::new();
    prefix.set_text(&ctx.get().beta.screenshot_prefix);
    prefix.set_width_chars(16);
    {
        let ctx = ctx.clone();
        prefix.connect_changed(move |e| {
            let value = e.text().to_string();
            ctx.update(|s| s.beta.screenshot_prefix = value);
        });
    }
    let prefix_row = hbox(&[
        gtk::Label::new(Some(tr("File names start with:"))).upcast_ref(),
        prefix.upcast_ref(),
    ]);
    depends_on(&prefix_row, &observe);
    p.indented(&prefix_row);
    p.caption(tr(
        "By default Clipy watches Pictures and Pictures/Screenshots. Leave the name empty to save every new PNG.",
    ));
    p.widget()
}

/// Folder chooser for screenshots, with a button back to the default folders.
fn screenshot_folder(ctx: &Ctx) -> gtk::Box {
    let chooser = gtk::FileChooserButton::new(
        tr("Screenshot folder"),
        gtk::FileChooserAction::SelectFolder,
    );
    let show_current = {
        let chooser = chooser.clone();
        move |dirs: &[String]| {
            let shown = dirs
                .first()
                .map(PathBuf::from)
                .or_else(|| screenshots::default_dirs().into_iter().find(|d| d.is_dir()));
            if let Some(dir) = shown {
                chooser.set_filename(dir);
            }
        }
    };
    show_current(&ctx.get().beta.screenshot_dirs);
    {
        let ctx = ctx.clone();
        chooser.connect_file_set(move |c| {
            if let Some(dir) = c.filename() {
                let dir = dir.to_string_lossy().into_owned();
                ctx.update(|s| s.beta.screenshot_dirs = vec![dir]);
            }
        });
    }
    let reset = gtk::Button::with_label(tr("Default"));
    {
        let ctx = ctx.clone();
        reset.connect_clicked(move |_| {
            ctx.update(|s| s.beta.screenshot_dirs.clear());
            show_current(&[]);
        });
    }
    hbox(&[
        gtk::Label::new(Some(tr("Folder:"))).upcast_ref(),
        chooser.upcast_ref(),
        reset.upcast_ref(),
    ])
}
