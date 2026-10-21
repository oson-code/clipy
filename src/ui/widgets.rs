//! Building blocks for the Preferences window: a two-column settings grid in the style of
//! Clipy's `SettingsGrid`, and controls bound to [`Settings`] fields.

use gtk::prelude::*;
use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::config::{Settings, SharedSettings};

/// Access to the live settings from widget callbacks.
#[derive(Clone)]
pub struct Ctx {
    pub settings: SharedSettings,
    pub proxy: EventLoopProxy<UserEvent>,
}

impl Ctx {
    pub fn get(&self) -> Settings {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Changes the settings right away and tells the app to save and apply them.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings.write().unwrap_or_else(|e| e.into_inner()));
        let _ = self.proxy.send_event(UserEvent::SettingsChanged);
    }
}

/// Section titles on the left, right-aligned; controls on the right.
pub struct Pane {
    grid: gtk::Grid,
    row: i32,
}

impl Pane {
    pub fn new() -> Self {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(8);
        grid.set_margin_top(20);
        grid.set_margin_bottom(20);
        grid.set_margin_start(24);
        grid.set_margin_end(24);
        Self { grid, row: 0 }
    }

    /// Starts a section: its title on the left and the first control on the right.
    pub fn section(&mut self, title: &str, widget: &impl IsA<gtk::Widget>) {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(1.0);
        label.set_valign(gtk::Align::Baseline);
        self.grid.attach(&label, 0, self.row, 1, 1);
        self.row(widget);
    }

    /// Another control in the current section.
    pub fn row(&mut self, widget: &impl IsA<gtk::Widget>) {
        widget.set_halign(gtk::Align::Start);
        self.grid.attach(widget, 1, self.row, 1, 1);
        self.row += 1;
    }

    /// A control that depends on the one above it.
    pub fn indented(&mut self, widget: &impl IsA<gtk::Widget>) {
        widget.set_margin_start(20);
        self.row(widget);
    }

    /// Explanatory text under a control.
    pub fn caption(&mut self, text: &str) {
        self.row(&caption(text));
    }

    /// A control spanning both columns.
    pub fn full(&mut self, widget: &impl IsA<gtk::Widget>) {
        self.grid.attach(widget, 0, self.row, 2, 1);
        self.row += 1;
    }

    pub fn separator(&mut self) {
        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_margin_top(4);
        sep.set_margin_bottom(4);
        self.full(&sep);
    }

    pub fn widget(&self) -> gtk::Widget {
        self.grid.clone().upcast()
    }
}

pub fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.set_max_width_chars(60);
    label.style_context().add_class("dim-label");
    let attrs = gtk::pango::AttrList::new();
    attrs.insert(gtk::pango::AttrFloat::new_scale(gtk::pango::SCALE_SMALL));
    label.set_attributes(Some(&attrs));
    label
}

pub fn check(
    ctx: &Ctx,
    label: &str,
    get: fn(&Settings) -> bool,
    set: fn(&mut Settings, bool),
) -> gtk::CheckButton {
    let button = gtk::CheckButton::with_label(label);
    button.set_active(get(&ctx.get()));
    let ctx = ctx.clone();
    button.connect_toggled(move |b| {
        let value = b.is_active();
        ctx.update(|s| set(s, value));
    });
    button
}

/// Makes `child` usable only while `parent` is checked.
pub fn depends_on(child: &impl IsA<gtk::Widget>, parent: &gtk::CheckButton) {
    child.set_sensitive(parent.is_active());
    let child = child.clone().upcast::<gtk::Widget>();
    parent.connect_toggled(move |p| child.set_sensitive(p.is_active()));
}

pub fn spin(
    ctx: &Ctx,
    min: u32,
    max: u32,
    get: fn(&Settings) -> u32,
    set: fn(&mut Settings, u32),
) -> gtk::SpinButton {
    let button = gtk::SpinButton::with_range(f64::from(min), f64::from(max), 1.0);
    button.set_value(f64::from(get(&ctx.get())));
    button.set_numeric(true);
    button.set_width_chars(5);
    let ctx = ctx.clone();
    button.connect_value_changed(move |b| {
        let value = b.value_as_int().max(0) as u32;
        ctx.update(|s| set(s, value));
    });
    button
}

/// A spin button followed by a unit such as "items" or "px".
pub fn spin_with_unit(spin: &gtk::SpinButton, unit: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.pack_start(spin, false, false, 0);
    row.pack_start(&gtk::Label::new(Some(unit)), false, false, 0);
    row
}

/// A drop-down whose entries map to `values`.
pub fn combo<T: Copy + PartialEq + 'static>(
    ctx: &Ctx,
    values: &'static [T],
    label: fn(T) -> &'static str,
    get: fn(&Settings) -> T,
    set: fn(&mut Settings, T),
) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for v in values {
        combo.append_text(label(*v));
    }
    let current = get(&ctx.get());
    combo.set_active(values.iter().position(|v| *v == current).map(|i| i as u32));
    let ctx = ctx.clone();
    combo.connect_changed(move |c| {
        if let Some(value) = c.active().and_then(|i| values.get(i as usize)) {
            let value = *value;
            ctx.update(|s| set(s, value));
        }
    });
    combo
}

/// A horizontal row of widgets.
pub fn hbox(children: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for child in children {
        row.pack_start(*child, false, false, 0);
    }
    row
}
