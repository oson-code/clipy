//! Clipy's alerts. They never block: the answer arrives through a callback, so the tao event
//! loop is not re-entered by a nested GTK main loop.

use gtk::prelude::*;
use gtk::{ButtonsType, DialogFlags, MessageType, ResponseType};

use crate::i18n::tr;

/// "Are you sure you want to clear your clipboard history?" Calls back with
/// `(confirmed, do_not_show_again)`.
pub fn confirm_clear_history(on_done: impl Fn(bool, bool) + 'static) {
    alert(
        None,
        tr("Clear History"),
        tr("Are you sure you want to clear your clipboard history?"),
        &[
            (tr("Clear History"), ResponseType::Accept),
            (tr("Cancel"), ResponseType::Cancel),
        ],
        true,
        move |response, suppress| on_done(response == ResponseType::Accept, suppress),
    );
}

/// Asked at first launch. Calls back with `(launch_at_login, do_not_show_again)`.
pub fn ask_launch_at_login(on_done: impl Fn(bool, bool) + 'static) {
    alert(
        None,
        tr("Launch Clipy on system startup?"),
        tr("You can change this later in Preferences."),
        &[
            (tr("Launch on system startup"), ResponseType::Accept),
            (tr("Don't Launch"), ResponseType::Cancel),
        ],
        true,
        move |response, suppress| on_done(response == ResponseType::Accept, suppress),
    );
}

/// The snippet editor's delete confirmation.
pub fn confirm_delete_item(parent: &gtk::Window, on_done: impl Fn(bool) + 'static) {
    alert(
        Some(parent),
        tr("Delete Item"),
        tr("Are you sure want to delete this item?"),
        &[
            (tr("Delete Item"), ResponseType::Accept),
            (tr("Cancel"), ResponseType::Cancel),
        ],
        false,
        move |response, _| on_done(response == ResponseType::Accept),
    );
}

/// An error the user should see, such as a failed import.
pub fn show_error(parent: Option<&gtk::Window>, message: &str, detail: &str) {
    alert(
        parent,
        message,
        detail,
        &[(tr("OK"), ResponseType::Ok)],
        false,
        |_, _| {},
    );
}

fn alert(
    parent: Option<&gtk::Window>,
    message: &str,
    detail: &str,
    buttons: &[(&str, ResponseType)],
    with_suppression: bool,
    on_done: impl Fn(ResponseType, bool) + 'static,
) {
    let dialog = gtk::MessageDialog::new(
        parent,
        DialogFlags::MODAL | DialogFlags::DESTROY_WITH_PARENT,
        MessageType::Question,
        ButtonsType::None,
        message,
    );
    dialog.set_title("Clipy");
    dialog.set_secondary_text(Some(detail));
    for (label, response) in buttons {
        dialog.add_button(label, *response);
    }
    if let Some((_, first)) = buttons.first() {
        dialog.set_default_response(*first);
    }
    let suppress = with_suppression.then(|| {
        let check = gtk::CheckButton::with_label(tr("Do not show this message again"));
        if let Ok(area) = dialog.message_area().downcast::<gtk::Box>() {
            area.pack_start(&check, false, false, 0);
        }
        check
    });
    dialog.connect_response(move |dialog, response| {
        let suppressed = suppress.as_ref().is_some_and(|c| c.is_active());
        dialog.close();
        on_done(response, suppressed);
    });
    dialog.set_keep_above(true);
    dialog.show_all();
    dialog.present();
}
