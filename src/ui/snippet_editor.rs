//! The snippet editor, a port of Clipy's `CPYSnippetsEditorWindowController`: folders and
//! snippets on the left, the selected folder's settings or snippet text on the right.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::app::UserEvent;
use crate::db::Database;
use crate::hotkeys::Action;
use crate::i18n::tr;
use crate::snippets_xml;
use crate::ui::dialogs;
use crate::ui::shortcut_recorder::ShortcutRecorder;
use crate::ui::widgets::Ctx;

const COL_IS_FOLDER: u32 = 0;
const COL_ID: u32 = 1;
const COL_TITLE: u32 = 2;
const COL_ENABLED: u32 = 3;
const COL_ICON: u32 = 4;
const SAVE_DELAY: Duration = Duration::from_millis(300);
const DRAG_TARGET: &str = "CLIPY_SNIPPET_ROW";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Folder(i64),
    Snippet(i64),
}

/// Keeps the window alive; dropping it does not close the window.
pub struct SnippetEditor {
    editor: Rc<Editor>,
}

struct Editor {
    ctx: Ctx,
    db: RefCell<Database>,
    window: gtk::Window,
    store: gtk::TreeStore,
    tree: gtk::TreeView,
    stack: gtk::Stack,
    folder_title: gtk::Entry,
    recorder_slot: gtk::Box,
    recorder: RefCell<Option<ShortcutRecorder>>,
    text: gtk::TextView,
    placeholder: gtk::Label,
    current: Cell<Option<Row>>,
    /// Set while the editor fills widgets itself, so their change handlers stay quiet.
    loading: Cell<bool>,
    pending_save: RefCell<Option<glib::SourceId>>,
    dragged: Cell<Option<Row>>,
    shortcut_errors: RefCell<Vec<(Action, String)>>,
}

impl SnippetEditor {
    pub fn new(ctx: Ctx, db_path: &Path) -> anyhow::Result<Self> {
        let db = Database::init(db_path)?;
        let editor = Rc::new(build(ctx, db));
        connect(&editor);
        // Shown first: a GtkStack ignores pages that are not visible yet.
        editor.window.show_all();
        editor.reload(None);
        if let Some(first) = editor.store.iter_first() {
            editor.tree.selection().select_iter(&first);
        }
        Ok(Self { editor })
    }

    pub fn present(&self) {
        self.editor.window.present();
    }

    pub fn set_shortcut_errors(&self, errors: &[(Action, String)]) {
        *self.editor.shortcut_errors.borrow_mut() = errors.to_vec();
        self.editor.show_folder_error();
    }
}

fn build(ctx: Ctx, db: Database) -> Editor {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(tr("Clipy - Snippets Editor"));
    window.set_default_size(760, 480);
    window.set_position(gtk::WindowPosition::Center);

    let store = gtk::TreeStore::new(&[
        bool::static_type(),
        i64::static_type(),
        String::static_type(),
        bool::static_type(),
        String::static_type(),
    ]);
    let tree = gtk::TreeView::with_model(&store);
    tree.set_headers_visible(false);
    tree.set_enable_search(false);
    let column = gtk::TreeViewColumn::new();
    let icon = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&column, &icon, false);
    TreeViewColumnExt::add_attribute(&column, &icon, "icon-name", COL_ICON as i32);
    TreeViewColumnExt::add_attribute(&column, &icon, "sensitive", COL_ENABLED as i32);
    let title = gtk::CellRendererText::new();
    title.set_editable(true);
    TreeViewColumnExt::pack_start(&column, &title, true);
    TreeViewColumnExt::add_attribute(&column, &title, "text", COL_TITLE as i32);
    TreeViewColumnExt::add_attribute(&column, &title, "sensitive", COL_ENABLED as i32);
    tree.append_column(&column);
    let targets = [gtk::TargetEntry::new(
        DRAG_TARGET,
        gtk::TargetFlags::SAME_WIDGET,
        0,
    )];
    // Plain widget drag and drop, handled by drop_target/drop_at: the tree's own model drag and
    // drop would let folders nest and snippets sit outside folders.
    tree.drag_source_set(
        gdk::ModifierType::BUTTON1_MASK,
        &targets,
        gdk::DragAction::MOVE,
    );
    tree.drag_dest_set(gtk::DestDefaults::empty(), &targets, gdk::DragAction::MOVE);

    let folder_title = gtk::Entry::new();
    folder_title.set_placeholder_text(Some(tr("folder title")));
    folder_title.set_hexpand(true);
    let recorder_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let shortcut_label = gtk::Label::new(Some(tr("Shortcut")));
    shortcut_label.set_xalign(0.0);
    let folder_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    folder_page.set_margin_top(16);
    folder_page.set_margin_start(16);
    folder_page.set_margin_end(16);
    folder_page.pack_start(&folder_title, false, false, 0);
    folder_page.pack_start(&shortcut_label, false, false, 0);
    folder_page.pack_start(&recorder_slot, false, false, 0);

    let text = gtk::TextView::new();
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    text.set_left_margin(8);
    text.set_right_margin(8);
    text.set_top_margin(8);
    text.set_bottom_margin(8);
    let placeholder = gtk::Label::new(Some(tr("Please fill in the contents of the snippet")));
    placeholder.style_context().add_class("dim-label");
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_start(10);
    placeholder.set_margin_top(8);
    let text_scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    text_scroll.add(&text);
    let snippet_page = gtk::Overlay::new();
    snippet_page.add(&text_scroll);
    snippet_page.add_overlay(&placeholder);
    snippet_page.set_overlay_pass_through(&placeholder, true);

    let stack = gtk::Stack::new();
    stack.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), "empty");
    stack.add_named(&folder_page, "folder");
    stack.add_named(&snippet_page, "snippet");

    let tree_scroll = gtk::ScrolledWindow::builder()
        .min_content_width(150)
        .build();
    tree_scroll.add(&tree);
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.pack1(&tree_scroll, false, false);
    paned.pack2(&stack, true, false);
    paned.set_position(240);

    let toolbar = gtk::Toolbar::new();
    toolbar.set_style(gtk::ToolbarStyle::Both);
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.pack_start(&toolbar, false, false, 0);
    layout.pack_start(&paned, true, true, 0);
    window.add(&layout);

    Editor {
        ctx,
        db: RefCell::new(db),
        window,
        store,
        tree,
        stack,
        folder_title,
        recorder_slot,
        recorder: RefCell::new(None),
        text,
        placeholder,
        current: Cell::new(None),
        loading: Cell::new(false),
        pending_save: RefCell::new(None),
        dragged: Cell::new(None),
        shortcut_errors: RefCell::new(Vec::new()),
    }
}

/// Runs `f` with the editor if it still exists.
fn with(weak: &Weak<Editor>, f: impl FnOnce(&Rc<Editor>)) {
    if let Some(editor) = weak.upgrade() {
        f(&editor);
    }
}

/// A toolbar button: label, icon name and what it does.
type ToolAction = (&'static str, &'static str, fn(&Rc<Editor>));

fn connect(editor: &Rc<Editor>) {
    let toolbar = editor
        .window
        .child()
        .and_then(|c| c.downcast::<gtk::Box>().ok())
        .and_then(|b| b.children().first().cloned())
        .and_then(|c| c.downcast::<gtk::Toolbar>().ok())
        .expect("toolbar");
    let buttons: [ToolAction; 6] = [
        (
            tr("Add Snippet"),
            "document-new-symbolic",
            Editor::add_snippet,
        ),
        (tr("Add Folder"), "folder-new-symbolic", Editor::add_folder),
        (
            tr("Delete"),
            "edit-delete-symbolic",
            Editor::delete_selected,
        ),
        (
            tr("Enable/Disable"),
            "object-select-symbolic",
            Editor::toggle_enabled,
        ),
        (tr("Import"), "document-open-symbolic", Editor::import),
        (tr("Export"), "document-save-as-symbolic", Editor::export),
    ];
    for (label, icon, action) in buttons {
        let button = gtk::ToolButton::new(
            Some(&gtk::Image::from_icon_name(
                Some(icon),
                gtk::IconSize::LargeToolbar,
            )),
            Some(label),
        );
        let weak = Rc::downgrade(editor);
        button.connect_clicked(move |_| with(&weak, action));
        toolbar.insert(&button, -1);
    }

    let weak = Rc::downgrade(editor);
    editor
        .tree
        .selection()
        .connect_changed(move |_| with(&weak, |e| e.selection_changed()));

    let renderer = editor
        .tree
        .column(0)
        .and_then(|c| {
            c.cells()
                .into_iter()
                .find_map(|r| r.downcast::<gtk::CellRendererText>().ok())
        })
        .expect("title renderer");
    let weak = Rc::downgrade(editor);
    renderer.connect_edited(move |_, path, title| with(&weak, |e| e.rename(&path, title)));

    let weak = Rc::downgrade(editor);
    editor
        .folder_title
        .connect_activate(move |_| with(&weak, |e| e.rename_from_entry()));
    let weak = Rc::downgrade(editor);
    editor.folder_title.connect_focus_out_event(move |_, _| {
        with(&weak, |e| e.rename_from_entry());
        glib::Propagation::Proceed
    });

    let weak = Rc::downgrade(editor);
    editor
        .text
        .buffer()
        .expect("text buffer")
        .connect_changed(move |buffer| {
            with(&weak, |e| {
                e.placeholder.set_visible(buffer.char_count() == 0);
                if !e.loading.get() {
                    e.schedule_save();
                }
            })
        });

    let weak = Rc::downgrade(editor);
    editor
        .tree
        .connect_drag_begin(move |_, _| with(&weak, |e| e.dragged.set(e.current.get())));
    let weak = Rc::downgrade(editor);
    editor
        .tree
        .connect_drag_motion(move |tree, context, x, y, time| {
            let mut allowed = false;
            with(&weak, |e| {
                let target = e.drop_target(x, y);
                allowed = target.is_some();
                match target {
                    Some((path, position)) => tree.set_drag_dest_row(path.as_ref(), position),
                    None => tree.set_drag_dest_row(None, gtk::TreeViewDropPosition::Before),
                }
            });
            let action = if allowed {
                gdk::DragAction::MOVE
            } else {
                gdk::DragAction::empty()
            };
            context.drag_status(action, time);
            true
        });
    let weak = Rc::downgrade(editor);
    editor
        .tree
        .connect_drag_drop(move |_, context, x, y, time| {
            let mut done = false;
            with(&weak, |e| done = e.drop_at(x, y));
            // Never let GTK delete the source row itself: the tree is reloaded from the database.
            context.drag_finish(done, false, time);
            true
        });
    let weak = Rc::downgrade(editor);
    editor
        .tree
        .connect_drag_end(move |_, _| with(&weak, |e| e.dragged.set(None)));

    let weak = Rc::downgrade(editor);
    editor.window.connect_delete_event(move |_, _| {
        with(&weak, |e| e.flush_save());
        glib::Propagation::Proceed
    });
    let proxy = editor.ctx.proxy.clone();
    editor.window.connect_destroy(move |_| {
        let _ = proxy.send_event(UserEvent::SnippetEditorClosed);
    });
}

impl Editor {
    // ---- tree contents ----------------------------------------------------------------

    /// Refills the tree from the database, keeping open folders open and selecting `select`
    /// (or the current row).
    fn reload(&self, select: Option<Row>) {
        let select = select.or(self.current.get());
        let expanded = self.expanded_folders();
        let tree = match self.db.borrow().fetch_snippets_hierarchy() {
            Ok(tree) => tree,
            Err(e) => return log::error!("reading snippets: {e:#}"),
        };
        self.loading.set(true);
        self.store.clear();
        for (folder, snippets) in tree {
            let parent = self.store.insert_with_values(
                None,
                None,
                &[
                    (COL_IS_FOLDER, &true),
                    (COL_ID, &folder.id),
                    (COL_TITLE, &folder.name),
                    (COL_ENABLED, &folder.enabled),
                    (COL_ICON, &"folder-symbolic"),
                ],
            );
            for snippet in snippets {
                self.store.insert_with_values(
                    Some(&parent),
                    None,
                    &[
                        (COL_IS_FOLDER, &false),
                        (COL_ID, &snippet.id),
                        (COL_TITLE, &snippet.title),
                        (COL_ENABLED, &snippet.enabled),
                        (COL_ICON, &"text-x-generic-symbolic"),
                    ],
                );
            }
        }
        for id in expanded {
            if let Some(path) = self
                .find(Row::Folder(id))
                .and_then(|it| self.store.path(&it))
            {
                self.tree.expand_row(&path, false);
            }
        }
        self.loading.set(false);
        self.current.set(None);
        match select.and_then(|row| self.find(row)) {
            Some(iter) => self.select_iter(&iter),
            None => self.selection_changed(),
        }
    }

    fn expanded_folders(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        if let Some(iter) = self.store.iter_first() {
            loop {
                if self
                    .store
                    .path(&iter)
                    .is_some_and(|p| self.tree.row_expanded(&p))
                {
                    ids.push(self.cell::<i64>(&iter, COL_ID as i32));
                }
                if !self.store.iter_next(&iter) {
                    break;
                }
            }
        }
        ids
    }

    /// The value in `column` of the row at `iter`.
    fn cell<T: for<'a> glib::value::FromValue<'a> + Default>(
        &self,
        iter: &gtk::TreeIter,
        column: i32,
    ) -> T {
        self.store
            .value(iter, column)
            .get::<T>()
            .unwrap_or_default()
    }

    fn row_at(&self, iter: &gtk::TreeIter) -> Row {
        let id = self.cell::<i64>(iter, COL_ID as i32);
        if self.cell::<bool>(iter, COL_IS_FOLDER as i32) {
            Row::Folder(id)
        } else {
            Row::Snippet(id)
        }
    }

    fn find(&self, row: Row) -> Option<gtk::TreeIter> {
        let found = RefCell::new(None);
        self.store.foreach(|_, _, iter| {
            if self.row_at(iter) == row {
                *found.borrow_mut() = Some(*iter);
                return true;
            }
            false
        });
        found.into_inner()
    }

    fn select_iter(&self, iter: &gtk::TreeIter) {
        let Some(path) = self.store.path(iter) else {
            return;
        };
        if path.depth() > 1 {
            let mut parent = path.clone();
            parent.up();
            self.tree.expand_row(&parent, false);
        }
        self.tree.selection().select_iter(iter);
        self.tree
            .scroll_to_cell(Some(&path), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
    }

    fn selected(&self) -> Option<(gtk::TreeIter, Row)> {
        let (_, iter) = self.tree.selection().selected()?;
        let row = self.row_at(&iter);
        Some((iter, row))
    }

    fn changed(&self) {
        let _ = self.ctx.proxy.send_event(UserEvent::SnippetsChanged);
    }

    // ---- right pane -------------------------------------------------------------------

    fn selection_changed(&self) {
        if self.loading.get() {
            return;
        }
        self.flush_save();
        let row = self.selected().map(|(_, row)| row);
        if row == self.current.get() {
            return;
        }
        self.current.set(row);
        self.loading.set(true);
        match row {
            None => self.stack.set_visible_child_name("empty"),
            Some(Row::Folder(id)) => {
                let name = self
                    .find(Row::Folder(id))
                    .map(|it| self.cell::<String>(&it, COL_TITLE as i32));
                self.folder_title.set_text(&name.unwrap_or_default());
                self.show_folder_recorder(id);
                self.stack.set_visible_child_name("folder");
            }
            Some(Row::Snippet(id)) => {
                let content = self
                    .db
                    .borrow()
                    .fetch_snippet(id)
                    .ok()
                    .flatten()
                    .map(|s| s.content);
                // A fresh buffer also resets undo history, as Clipy does on selection change.
                let buffer = self.text.buffer().expect("text buffer");
                buffer.set_text(&content.unwrap_or_default());
                self.placeholder.set_visible(buffer.char_count() == 0);
                self.stack.set_visible_child_name("snippet");
            }
        }
        self.loading.set(false);
    }

    fn show_folder_recorder(&self, folder_id: i64) {
        for child in self.recorder_slot.children() {
            self.recorder_slot.remove(&child);
        }
        let accel = self
            .ctx
            .get()
            .shortcuts
            .folders
            .get(&folder_id.to_string())
            .cloned()
            .unwrap_or_default();
        let ctx = self.ctx.clone();
        let recorder = ShortcutRecorder::new(&accel, true, move |accel| {
            ctx.update(|s| {
                if accel.is_empty() {
                    s.shortcuts.folders.remove(&folder_id.to_string());
                } else {
                    s.shortcuts.folders.insert(folder_id.to_string(), accel);
                }
            })
        });
        self.recorder_slot
            .pack_start(recorder.widget(), false, false, 0);
        self.recorder_slot.show_all();
        *self.recorder.borrow_mut() = Some(recorder);
        self.show_folder_error();
    }

    fn show_folder_error(&self) {
        let Some(Row::Folder(id)) = self.current.get() else {
            return;
        };
        let errors = self.shortcut_errors.borrow();
        let message = errors
            .iter()
            .find(|(a, _)| *a == Action::Folder(id))
            .map(|(_, m)| m.as_str());
        if let Some(recorder) = self.recorder.borrow().as_ref() {
            recorder.set_error(message);
        }
    }

    fn schedule_save(self: &Rc<Self>) {
        if let Some(timer) = self.pending_save.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(SAVE_DELAY, move || {
            with(&weak, |e| {
                e.pending_save.borrow_mut().take();
                e.save_text();
            })
        });
        *self.pending_save.borrow_mut() = Some(timer);
    }

    /// Saves the snippet text now if a save is waiting.
    fn flush_save(&self) {
        if let Some(timer) = self.pending_save.borrow_mut().take() {
            timer.remove();
            self.save_text();
        }
    }

    fn save_text(&self) {
        let Some(Row::Snippet(id)) = self.current.get() else {
            return;
        };
        let buffer = self.text.buffer().expect("text buffer");
        let content = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .map(|t| t.to_string())
            .unwrap_or_default();
        match self.db.borrow().update_snippet_content(id, &content) {
            Ok(()) => self.changed(),
            Err(e) => log::error!("saving snippet: {e:#}"),
        }
    }

    // ---- editing ----------------------------------------------------------------------

    fn rename(&self, path: &gtk::TreePath, title: &str) {
        let Some(iter) = self.store.iter(path) else {
            return;
        };
        if title.trim().is_empty() {
            beep();
            return;
        }
        let result = match self.row_at(&iter) {
            Row::Folder(id) => self.db.borrow().rename_folder(id, title),
            Row::Snippet(id) => self.db.borrow().rename_snippet(id, title),
        };
        match result {
            Ok(()) => {
                self.store.set_value(&iter, COL_TITLE, &title.to_value());
                if self.current.get() == Some(self.row_at(&iter))
                    && matches!(self.current.get(), Some(Row::Folder(_)))
                {
                    self.folder_title.set_text(title);
                }
                self.changed();
            }
            Err(e) => log::error!("renaming: {e:#}"),
        }
    }

    fn rename_from_entry(&self) {
        let Some(Row::Folder(id)) = self.current.get() else {
            return;
        };
        let Some(iter) = self.find(Row::Folder(id)) else {
            return;
        };
        let title = self.folder_title.text().to_string();
        let old = self.cell::<String>(&iter, COL_TITLE as i32);
        if title == old {
            return;
        }
        if title.trim().is_empty() {
            beep();
            self.folder_title.set_text(&old);
            return;
        }
        if let Some(path) = self.store.path(&iter) {
            self.rename(&path, &title);
        }
    }

    fn add_snippet(self: &Rc<Self>) {
        let folder = match self.current.get() {
            Some(Row::Folder(id)) => id,
            Some(Row::Snippet(_)) => {
                let Some((iter, _)) = self.selected() else {
                    return;
                };
                match self.store.iter_parent(&iter) {
                    Some(parent) => self.cell::<i64>(&parent, COL_ID as i32),
                    None => return beep(),
                }
            }
            None => return beep(),
        };
        let result = self
            .db
            .borrow_mut()
            .add_snippet(folder, tr("untitled snippet"), "");
        match result {
            Ok(id) => {
                self.reload(Some(Row::Snippet(id)));
                self.edit_title(Row::Snippet(id));
                self.changed();
            }
            Err(e) => log::error!("adding a snippet: {e:#}"),
        }
    }

    fn add_folder(self: &Rc<Self>) {
        let result = self.db.borrow_mut().add_folder(tr("untitled folder"));
        match result {
            Ok(id) => {
                self.reload(Some(Row::Folder(id)));
                self.edit_title(Row::Folder(id));
                self.changed();
            }
            Err(e) => log::error!("adding a folder: {e:#}"),
        }
    }

    fn edit_title(&self, row: Row) {
        if let Some(path) = self.find(row).and_then(|iter| self.store.path(&iter)) {
            self.tree
                .set_cursor(&path, self.tree.column(0).as_ref(), true);
        }
    }

    fn delete_selected(self: &Rc<Self>) {
        let Some(row) = self.current.get() else {
            return beep();
        };
        let weak = Rc::downgrade(self);
        dialogs::confirm_delete_item(&self.window, move |confirmed| {
            if !confirmed {
                return;
            }
            with(&weak, |e| {
                if let Some(timer) = e.pending_save.borrow_mut().take() {
                    timer.remove();
                }
                let result = match row {
                    Row::Folder(id) => e.db.borrow().delete_folder(id),
                    Row::Snippet(id) => e.db.borrow().delete_snippet(id),
                };
                if let Err(err) = result {
                    return log::error!("deleting: {err:#}");
                }
                if let Row::Folder(id) = row {
                    e.ctx.update(|s| {
                        s.shortcuts.folders.remove(&id.to_string());
                    });
                }
                e.current.set(None);
                e.reload(None);
                e.changed();
            });
        });
    }

    fn toggle_enabled(self: &Rc<Self>) {
        let Some((iter, row)) = self.selected() else {
            return beep();
        };
        let enabled = !self.cell::<bool>(&iter, COL_ENABLED as i32);
        let result = match row {
            Row::Folder(id) => self.db.borrow().set_folder_enabled(id, enabled),
            Row::Snippet(id) => self.db.borrow().set_snippet_enabled(id, enabled),
        };
        match result {
            Ok(()) => {
                self.store
                    .set_value(&iter, COL_ENABLED, &enabled.to_value());
                self.changed();
            }
            Err(e) => log::error!("changing status: {e:#}"),
        }
    }

    fn import(self: &Rc<Self>) {
        let dialog = file_dialog(
            &self.window,
            tr("Import"),
            gtk::FileChooserAction::Open,
            tr("Import"),
        );
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            let path = dialog.filename();
            dialog.close();
            let (gtk::ResponseType::Accept, Some(path)) = (response, path) else {
                return;
            };
            with(&weak, |e| {
                let result = std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|xml| snippets_xml::parse(&xml))
                    .and_then(|folders| e.db.borrow_mut().import_folders(&folders));
                match result {
                    Ok(()) => {
                        e.reload(None);
                        e.changed();
                    }
                    Err(err) => {
                        beep();
                        dialogs::show_error(Some(&e.window), tr("Import"), &format!("{err:#}"));
                    }
                }
            });
        });
        dialog.show();
    }

    fn export(self: &Rc<Self>) {
        self.flush_save();
        let dialog = file_dialog(
            &self.window,
            tr("Export"),
            gtk::FileChooserAction::Save,
            tr("Export"),
        );
        dialog.set_current_name("snippets.xml");
        dialog.set_do_overwrite_confirmation(true);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            let path = dialog.filename();
            dialog.close();
            let (gtk::ResponseType::Accept, Some(path)) = (response, path) else {
                return;
            };
            with(&weak, |e| {
                let result =
                    e.db.borrow()
                        .fetch_snippets_hierarchy()
                        .and_then(|tree| write_atomically(&path, &snippets_xml::export(&tree)));
                if let Err(err) = result {
                    beep();
                    dialogs::show_error(Some(&e.window), tr("Export"), &format!("{err:#}"));
                }
            });
        });
        dialog.show();
    }

    // ---- drag and drop ----------------------------------------------------------------

    /// Where the dragged row would land, following Clipy's rules: folders move among folders
    /// only; snippets move within or between folders.
    fn drop_target(
        &self,
        x: i32,
        y: i32,
    ) -> Option<(Option<gtk::TreePath>, gtk::TreeViewDropPosition)> {
        use gtk::TreeViewDropPosition as Pos;
        let dragged = self.dragged.get()?;
        let Some((Some(path), position)) = self.tree.dest_row_at_pos(x, y) else {
            // Below the last row.
            return matches!(dragged, Row::Folder(_)).then_some((None, Pos::After));
        };
        let iter = self.store.iter(&path)?;
        if self.row_at(&iter) == dragged {
            return None;
        }
        match (dragged, path.depth()) {
            (Row::Folder(_), 1) => {
                let position = match position {
                    Pos::Before | Pos::IntoOrBefore => Pos::Before,
                    _ => Pos::After,
                };
                Some((Some(path), position))
            }
            (Row::Folder(_), _) => None,
            (Row::Snippet(_), 1) => Some((Some(path), Pos::IntoOrBefore)),
            (Row::Snippet(_), _) => {
                let position = match position {
                    Pos::Before | Pos::IntoOrBefore => Pos::Before,
                    _ => Pos::After,
                };
                Some((Some(path), position))
            }
        }
    }

    fn drop_at(&self, x: i32, y: i32) -> bool {
        use gtk::TreeViewDropPosition as Pos;
        let Some(dragged) = self.dragged.get() else {
            return false;
        };
        let Some((path, position)) = self.drop_target(x, y) else {
            return false;
        };
        let result = match dragged {
            Row::Folder(id) => {
                let mut order: Vec<i64> =
                    self.folder_ids().into_iter().filter(|f| *f != id).collect();
                let index = match path.and_then(|p| self.store.iter(&p)) {
                    Some(target) => {
                        let target_id = self.cell::<i64>(&target, COL_ID as i32);
                        let at = order
                            .iter()
                            .position(|f| *f == target_id)
                            .unwrap_or(order.len());
                        if position == Pos::After {
                            at + 1
                        } else {
                            at
                        }
                    }
                    None => order.len(),
                };
                order.insert(index.min(order.len()), id);
                self.db.borrow_mut().reorder_folders(&order)
            }
            Row::Snippet(id) => {
                let Some(target) = path.and_then(|p| self.store.iter(&p)) else {
                    return false;
                };
                match self.row_at(&target) {
                    // Dropped on a folder: to the top of it, as in Clipy.
                    Row::Folder(folder) => self.db.borrow_mut().move_snippet(id, folder, 0),
                    Row::Snippet(target_id) => {
                        let Some(parent) = self.store.iter_parent(&target) else {
                            return false;
                        };
                        let folder = self.cell::<i64>(&parent, COL_ID as i32);
                        let siblings: Vec<i64> = self
                            .child_ids(&parent)
                            .into_iter()
                            .filter(|s| *s != id)
                            .collect();
                        let at = siblings
                            .iter()
                            .position(|s| *s == target_id)
                            .unwrap_or(siblings.len());
                        let index = if position == Pos::After { at + 1 } else { at };
                        self.db.borrow_mut().move_snippet(id, folder, index)
                    }
                }
            }
        };
        match result {
            Ok(()) => {
                self.reload(Some(dragged));
                self.changed();
                true
            }
            Err(e) => {
                log::error!("moving: {e:#}");
                false
            }
        }
    }

    fn folder_ids(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        if let Some(iter) = self.store.iter_first() {
            loop {
                ids.push(self.cell::<i64>(&iter, COL_ID as i32));
                if !self.store.iter_next(&iter) {
                    break;
                }
            }
        }
        ids
    }

    fn child_ids(&self, parent: &gtk::TreeIter) -> Vec<i64> {
        let mut ids = Vec::new();
        if let Some(iter) = self.store.iter_children(Some(parent)) {
            loop {
                ids.push(self.cell::<i64>(&iter, COL_ID as i32));
                if !self.store.iter_next(&iter) {
                    break;
                }
            }
        }
        ids
    }
}

fn file_dialog(
    parent: &gtk::Window,
    title: &str,
    action: gtk::FileChooserAction,
    accept: &str,
) -> gtk::FileChooserDialog {
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(title),
        Some(parent),
        action,
        &[
            (tr("Cancel"), gtk::ResponseType::Cancel),
            (accept, gtk::ResponseType::Accept),
        ],
    );
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(tr("Clipy snippets (XML)")));
    filter.add_pattern("*.xml");
    filter.add_mime_type("application/xml");
    filter.add_mime_type("text/xml");
    dialog.add_filter(filter);
    dialog.set_modal(true);
    dialog
}

fn write_atomically(path: &PathBuf, text: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("xml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn beep() {
    if let Some(display) = gdk::Display::default() {
        display.beep();
    }
}
