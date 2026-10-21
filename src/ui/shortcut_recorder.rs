//! A button that records a keyboard shortcut, like Clipy's `ShortcutRecorder`: click it, press
//! the keys; Backspace clears (when allowed), Escape cancels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::i18n::tr;

pub struct ShortcutRecorder {
    root: gtk::Box,
    error: gtk::Label,
}

impl ShortcutRecorder {
    /// `on_change` receives the new GTK accelerator, or an empty string when cleared.
    pub fn new(accel: &str, clearable: bool, on_change: impl Fn(String) + 'static) -> Self {
        let button = gtk::ToggleButton::new();
        button.set_size_request(240, -1);
        let accel = Rc::new(RefCell::new(accel.to_owned()));
        let on_change: Rc<dyn Fn(String)> = Rc::new(on_change);
        show_label(&button, &accel.borrow());

        let clear = gtk::Button::from_icon_name(Some("edit-clear-symbolic"), gtk::IconSize::Button);
        clear.set_tooltip_text(Some(tr("Remove")));
        clear.set_relief(gtk::ReliefStyle::None);
        clear.set_no_show_all(!clearable);
        clear.set_visible(clearable);

        let error = gtk::Label::new(None);
        error.set_xalign(0.0);
        error.style_context().add_class("error");
        error.set_no_show_all(true);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.pack_start(&button, false, false, 0);
        row.pack_start(&clear, false, false, 0);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.pack_start(&row, false, false, 0);
        root.pack_start(&error, false, false, 0);

        let recording = Rc::new(Cell::new(false));
        {
            let (accel, recording) = (accel.clone(), recording.clone());
            button.connect_toggled(move |b| {
                recording.set(b.is_active());
                if b.is_active() {
                    b.set_label(tr("Type shortcut…"));
                    b.grab_focus();
                } else {
                    show_label(b, &accel.borrow());
                }
            });
        }
        {
            let (accel, recording, on_change) =
                (accel.clone(), recording.clone(), on_change.clone());
            button.connect_key_press_event(move |b, event| {
                if !recording.get() {
                    return glib::Propagation::Proceed;
                }
                let mods = event.state() & gtk::accelerator_get_default_mod_mask();
                let key = *event.keyval();
                match key {
                    k if k == *gdk::keys::constants::Escape && mods.is_empty() => {}
                    k if k == *gdk::keys::constants::BackSpace && mods.is_empty() => {
                        if clearable {
                            accel.borrow_mut().clear();
                            on_change(String::new());
                        }
                    }
                    k if is_modifier(k) => return glib::Propagation::Stop,
                    _ => {
                        let Some(name) = accelerator_for(event, mods) else {
                            return glib::Propagation::Stop;
                        };
                        *accel.borrow_mut() = name.clone();
                        on_change(name);
                    }
                }
                b.set_active(false);
                glib::Propagation::Stop
            });
        }
        {
            let recording = recording.clone();
            button.connect_focus_out_event(move |b, _| {
                if recording.get() {
                    b.set_active(false);
                }
                glib::Propagation::Proceed
            });
        }
        {
            clear.connect_clicked(move |_| {
                accel.borrow_mut().clear();
                show_label(&button, "");
                on_change(String::new());
            });
        }
        Self { root, error }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows why the shortcut could not be registered, or hides the message.
    pub fn set_error(&self, message: Option<&str>) {
        self.error.set_text(message.unwrap_or(""));
        self.error.set_visible(message.is_some());
    }
}

fn show_label(button: &gtk::ToggleButton, accel: &str) {
    let label = if accel.is_empty() {
        tr("Record Shortcut").to_owned()
    } else {
        let (key, mods) = gtk::accelerator_parse(accel);
        if key == 0 {
            accel.to_owned()
        } else {
            gtk::accelerator_get_label(key, mods)
                .map(|l| l.to_string())
                .unwrap_or_else(|| accel.to_owned())
        }
    };
    button.set_label(&label);
}

/// The accelerator for a key press, using the unshifted key so Shift+1 records as
/// `<Shift>1`, not `<Shift>exclam`. Shortcuts need a modifier, except function keys.
fn accelerator_for(event: &gdk::EventKey, mods: gdk::ModifierType) -> Option<String> {
    let display = gdk::Display::default()?;
    let keymap = gdk::Keymap::for_display(&display)?;
    let (key, ..) = keymap.translate_keyboard_state(
        u32::from(event.hardware_keycode()),
        gdk::ModifierType::empty(),
        i32::from(event.group()),
    )?;
    let key = *gdk::keys::Key::from(key).to_lower();
    let is_function_key = (*gdk::keys::constants::F1..=*gdk::keys::constants::F35).contains(&key);
    if mods.is_empty() && !is_function_key {
        return None;
    }
    gtk::accelerator_name(key, mods).map(|n| n.to_string())
}

fn is_modifier(key: u32) -> bool {
    use gdk::keys::constants as k;
    [
        *k::Shift_L,
        *k::Shift_R,
        *k::Control_L,
        *k::Control_R,
        *k::Alt_L,
        *k::Alt_R,
        *k::Super_L,
        *k::Super_R,
        *k::Meta_L,
        *k::Meta_R,
        *k::Hyper_L,
        *k::Hyper_R,
        *k::ISO_Level3_Shift,
        *k::Caps_Lock,
        *k::Num_Lock,
    ]
    .contains(&key)
}
