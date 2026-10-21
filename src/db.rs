//! SQLite storage for clipboard history and snippets (`~/.local/share/clipy/clipy.db`).
//!
//! Every thread that touches the database opens its own [`Database`]; WAL mode lets the
//! clipboard watcher write while the main thread reads.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::config::SortBy;
use crate::model::{hash_hex, DataType, Folder, HistoryItem, NewClip, Snippet, StoredClip};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data_type TEXT NOT NULL CHECK(data_type IN ('text', 'image', 'html', 'files')),
    text_content TEXT,
    html_content TEXT,
    image_blob BLOB,
    image_width INTEGER,
    image_height INTEGER,
    thumbnail_png BLOB,
    content_hash TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_history_hash ON history(content_hash);
CREATE INDEX IF NOT EXISTS idx_history_updated ON history(updated_at);

CREATE TABLE IF NOT EXISTS snippet_folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    sort_order INTEGER DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS snippets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id INTEGER NOT NULL REFERENCES snippet_folders(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    sort_order INTEGER DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1
);
";

/// Millisecond timestamps keep the order of copies made within the same second. The text form
/// still sorts and reads like SQLite's `CURRENT_TIMESTAMP`.
const NOW: &str = "strftime('%Y-%m-%d %H:%M:%f', 'now')";

/// How the menus' longest text (a 10 000-character tooltip) is cut from large clips.
const MENU_TEXT_CHARS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOptions {
    /// Clipy's "Save duplicate clipboard content".
    pub copy_same: bool,
    /// Clipy's "Replace the previous entry" (only when duplicates are saved).
    pub overwrite_same: bool,
    pub limit: usize,
    pub sort: SortBy,
}

impl SaveOptions {
    #[allow(dead_code)]
    fn spec_default(limit: usize) -> Self {
        Self {
            copy_same: true,
            overwrite_same: true,
            limit,
            sort: SortBy::LastUsed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Inserted(i64),
    /// An entry with the same content moved to the top.
    Updated(i64),
    /// Duplicates are not saved and this content is already in history.
    Skipped(i64),
}

/// One folder of an XML import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFolder {
    pub title: String,
    /// `(title, content)` pairs.
    pub snippets: Vec<(String, String)>,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens (creating if needed) the database at `path`: directory 0700, file 0600, schema
    /// migrated, and a "Common" folder seeded on first creation.
    pub fn init(path: &Path) -> Result<Self> {
        prepare_file(path)?;
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        if version < 1 {
            tx.execute_batch(SCHEMA_V1)?;
            let folder = insert_folder(&tx, "Common")?;
            tx.execute(
                "INSERT INTO snippets (folder_id, title, content, sort_order) VALUES (?1, ?2, ?3, 0)",
                params![
                    folder,
                    "Example snippet",
                    "This is an example snippet. Change it in Edit Snippets…"
                ],
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    // ---- history ----------------------------------------------------------------------

    /// Stores a capture following Clipy's duplicate rules, then trims history to `opts.limit`.
    pub fn save(&mut self, clip: &NewClip, opts: &SaveOptions) -> Result<SaveOutcome> {
        let tx = self.conn.transaction()?;
        let hash = hash_hex(&clip.hash);
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM history WHERE content_hash = ?1 ORDER BY updated_at DESC, id DESC LIMIT 1",
                [&hash],
                |r| r.get(0),
            )
            .optional()?;
        let outcome = match existing {
            Some(id) if !opts.copy_same => SaveOutcome::Skipped(id),
            Some(id) if opts.overwrite_same => {
                // Copying existing content again moves it to the top in both sort orders; pasting it
                // from Clipy (Database::touch) only marks it as last used.
                tx.execute(
                    &format!(
                        "UPDATE history SET created_at = {NOW}, updated_at = {NOW} WHERE id = ?1"
                    ),
                    [id],
                )?;
                SaveOutcome::Updated(id)
            }
            _ => {
                let image = clip.image.as_ref();
                tx.execute(
                    &format!(
                        "INSERT INTO history (data_type, text_content, html_content, image_blob,
                            image_width, image_height, thumbnail_png, content_hash, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, {NOW}, {NOW})"
                    ),
                    params![
                        clip.data_type.as_str(),
                        clip.text,
                        clip.html,
                        image.map(|i| &i.png),
                        image.map(|i| i.width),
                        image.map(|i| i.height),
                        image.map(|i| &i.thumbnail_png),
                        hash,
                    ],
                )?;
                SaveOutcome::Inserted(tx.last_insert_rowid())
            }
        };
        trim_history(&tx, opts.limit, opts.sort)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// Spec API: saves plain text with the default duplicate rules. The watcher calls
    /// [`Database::save`] with the user's rules instead.
    #[allow(dead_code)]
    pub fn save_text(&mut self, text: &str, limit: usize) -> Result<SaveOutcome> {
        self.save(
            &NewClip::text(text.to_owned()),
            &SaveOptions::spec_default(limit),
        )
    }

    /// Spec API: saves an RGBA image with the default duplicate rules.
    #[allow(dead_code)]
    pub fn save_image(
        &mut self,
        rgba: &[u8],
        width: usize,
        height: usize,
        limit: usize,
    ) -> Result<SaveOutcome> {
        let clip = NewClip::image(rgba, width as u32, height as u32)?;
        self.save(&clip, &SaveOptions::spec_default(limit))
    }

    pub fn fetch_recent_history(&self, limit: usize, sort: SortBy) -> Result<Vec<HistoryItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, data_type, substr(text_content, 1, {MENU_TEXT_CHARS}),
                    html_content IS NOT NULL, thumbnail_png, created_at, updated_at
             FROM history ORDER BY {} DESC, id DESC LIMIT ?1",
            sort_column(sort)
        ))?;
        let rows = stmt.query_map([limit as i64], |r| {
            let data_type: String = r.get(1)?;
            Ok(HistoryItem {
                id: r.get(0)?,
                data_type: DataType::parse(&data_type).unwrap_or(DataType::Text),
                text: r.get(2)?,
                has_html: r.get(3)?,
                thumbnail_png: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn fetch_clip(&self, id: i64) -> Result<Option<StoredClip>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, data_type, text_content, html_content, image_blob FROM history WHERE id = ?1",
                [id],
                |r| {
                    let data_type: String = r.get(1)?;
                    Ok(StoredClip {
                        id: r.get(0)?,
                        data_type: DataType::parse(&data_type).unwrap_or(DataType::Text),
                        text: r.get(2)?,
                        html: r.get(3)?,
                        image_png: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Marks an entry as just used, which moves it to the top when sorting by "Last used".
    pub fn touch(&self, id: i64) -> Result<()> {
        self.conn.execute(
            &format!("UPDATE history SET updated_at = {NOW} WHERE id = ?1"),
            [id],
        )?;
        Ok(())
    }

    pub fn delete_history(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    /// Gives the space of deleted images back to the file system. Slow: run it off the main
    /// thread.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// Deletes everything after the newest `limit` entries in `sort` order.
    pub fn trim(&mut self, limit: usize, sort: SortBy) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let deleted = trim_history(&tx, limit, sort)?;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn history_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    // ---- snippets ---------------------------------------------------------------------

    pub fn fetch_snippets_hierarchy(&self) -> Result<Vec<(Folder, Vec<Snippet>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, sort_order, enabled FROM snippet_folders ORDER BY sort_order, id",
        )?;
        let folders = stmt
            .query_map([], |r| {
                Ok(Folder {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    sort_order: r.get(2)?,
                    enabled: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, folder_id, title, content, sort_order, enabled FROM snippets
             ORDER BY folder_id, sort_order, id",
        )?;
        let snippets = stmt
            .query_map([], |r| {
                Ok(Snippet {
                    id: r.get(0)?,
                    folder_id: r.get(1)?,
                    title: r.get(2)?,
                    content: r.get(3)?,
                    sort_order: r.get(4)?,
                    enabled: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(folders
            .into_iter()
            .map(|f| {
                let own = snippets
                    .iter()
                    .filter(|s| s.folder_id == f.id)
                    .cloned()
                    .collect();
                (f, own)
            })
            .collect())
    }

    pub fn fetch_snippet(&self, id: i64) -> Result<Option<Snippet>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, folder_id, title, content, sort_order, enabled FROM snippets WHERE id = ?1",
                [id],
                |r| {
                    Ok(Snippet {
                        id: r.get(0)?,
                        folder_id: r.get(1)?,
                        title: r.get(2)?,
                        content: r.get(3)?,
                        sort_order: r.get(4)?,
                        enabled: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn add_folder(&mut self, name: &str) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = insert_folder(&tx, name)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn rename_folder(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE snippet_folders SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn set_folder_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE snippet_folders SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    /// Deletes the folder and, through the foreign key, its snippets.
    pub fn delete_folder(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM snippet_folders WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Stores the folder order exactly as given.
    pub fn reorder_folders(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (index, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE snippet_folders SET sort_order = ?2 WHERE id = ?1",
                params![id, index as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_snippet(&mut self, folder_id: i64, title: &str, content: &str) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = insert_snippet(&tx, folder_id, title, content)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn rename_snippet(&self, id: i64, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE snippets SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    pub fn update_snippet_content(&self, id: i64, content: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE snippets SET content = ?2 WHERE id = ?1",
            params![id, content],
        )?;
        Ok(())
    }

    pub fn set_snippet_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE snippets SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM snippets WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Moves a snippet to `index` within `folder_id` (its own folder or another one) and
    /// renumbers that folder.
    pub fn move_snippet(&mut self, id: i64, folder_id: i64, index: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM snippets WHERE folder_id = ?1 AND id != ?2 ORDER BY sort_order, id",
            )?;
            let ids = stmt
                .query_map(params![folder_id, id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        ids.insert(index.min(ids.len()), id);
        tx.execute(
            "UPDATE snippets SET folder_id = ?2 WHERE id = ?1",
            params![id, folder_id],
        )?;
        for (order, sid) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE snippets SET sort_order = ?2 WHERE id = ?1",
                params![sid, order as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Appends imported folders after the existing ones, in one transaction.
    pub fn import_folders(&mut self, folders: &[ImportFolder]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for folder in folders {
            let folder_id = insert_folder(&tx, &folder.title)?;
            for (title, content) in &folder.snippets {
                insert_snippet(&tx, folder_id, title, content)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

fn sort_column(sort: SortBy) -> &'static str {
    match sort {
        SortBy::DateCreated => "created_at",
        SortBy::LastUsed => "updated_at",
    }
}

fn trim_history(tx: &Transaction<'_>, limit: usize, sort: SortBy) -> Result<usize> {
    Ok(tx.execute(
        &format!(
            "DELETE FROM history WHERE id NOT IN (
                SELECT id FROM history ORDER BY {} DESC, id DESC LIMIT ?1)",
            sort_column(sort)
        ),
        [limit as i64],
    )?)
}

fn insert_folder(tx: &Transaction<'_>, name: &str) -> Result<i64> {
    tx.execute(
        "INSERT INTO snippet_folders (name, sort_order)
         VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM snippet_folders))",
        [name],
    )?;
    Ok(tx.last_insert_rowid())
}

fn insert_snippet(tx: &Transaction<'_>, folder_id: i64, title: &str, content: &str) -> Result<i64> {
    tx.execute(
        "INSERT INTO snippets (folder_id, title, content, sort_order)
         VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM snippets WHERE folder_id = ?1))",
        params![folder_id, title, content],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Creates the parent directory (0700) and an empty database file (0600) before SQLite opens
/// it, so history is never readable by other users, not even briefly.
///
/// The file is opened only when it does not exist yet. Closing any descriptor of a file drops
/// every POSIX lock this process holds on it, including those of SQLite connections already
/// open on other threads; another process could then delete the live WAL.
fn prepare_file(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    if !path.exists() {
        let created = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path);
        match created {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(e).with_context(|| format!("creating {}", path.display()));
            }
            _ => {}
        }
    }
    fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(limit: usize) -> SaveOptions {
        SaveOptions::spec_default(limit)
    }

    /// Timestamps have millisecond resolution; make the next write sort as newer.
    fn tick() {
        std::thread::sleep(Duration::from_millis(3));
    }

    fn texts(db: &Database, sort: SortBy) -> Vec<String> {
        db.fetch_recent_history(100, sort)
            .unwrap()
            .into_iter()
            .map(|h| h.text.unwrap_or_default())
            .collect()
    }

    #[test]
    fn seeds_common_folder_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/clipy.db");
        {
            let mut db = Database::init(&path).unwrap();
            let tree = db.fetch_snippets_hierarchy().unwrap();
            assert_eq!(tree.len(), 1);
            assert_eq!(tree[0].0.name, "Common");
            assert_eq!(tree[0].1.len(), 1);
            db.delete_folder(tree[0].0.id).unwrap();
            db.add_folder("Mine").unwrap();
        }
        let db = Database::init(&path).unwrap();
        let names: Vec<_> = db
            .fetch_snippets_hierarchy()
            .unwrap()
            .into_iter()
            .map(|(f, _)| f.name)
            .collect();
        assert_eq!(names, ["Mine"]);
    }

    #[test]
    fn database_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/clipy.db");
        Database::init(&path).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn duplicate_moves_to_top_by_default() {
        let mut db = Database::in_memory().unwrap();
        let SaveOutcome::Inserted(a) = db.save_text("a", 10).unwrap() else {
            panic!("first save must insert")
        };
        db.save_text("b", 10).unwrap();
        tick();
        assert_eq!(db.save_text("a", 10).unwrap(), SaveOutcome::Updated(a));
        assert_eq!(texts(&db, SortBy::LastUsed), ["a", "b"]);
        assert_eq!(texts(&db, SortBy::DateCreated), ["a", "b"]);
    }

    #[test]
    fn duplicates_skipped_when_not_saved() {
        let mut db = Database::in_memory().unwrap();
        let o = SaveOptions {
            copy_same: false,
            ..opts(10)
        };
        db.save(&NewClip::text("a".into()), &o).unwrap();
        db.save(&NewClip::text("b".into()), &o).unwrap();
        let r = db.save(&NewClip::text("a".into()), &o).unwrap();
        assert!(matches!(r, SaveOutcome::Skipped(_)));
        assert_eq!(texts(&db, SortBy::LastUsed), ["b", "a"]);
    }

    #[test]
    fn duplicates_kept_as_new_entries_without_overwrite() {
        let mut db = Database::in_memory().unwrap();
        let o = SaveOptions {
            overwrite_same: false,
            ..opts(10)
        };
        db.save(&NewClip::text("a".into()), &o).unwrap();
        db.save(&NewClip::text("a".into()), &o).unwrap();
        assert_eq!(db.history_count().unwrap(), 2);
    }

    #[test]
    fn trims_to_limit_in_sort_order() {
        let mut db = Database::in_memory().unwrap();
        for t in ["1", "2", "3", "4"] {
            db.save_text(t, 3).unwrap();
        }
        assert_eq!(texts(&db, SortBy::LastUsed), ["4", "3", "2"]);
        // Touching the oldest makes it survive a trim by "last used" but not by "date created".
        let oldest = db.fetch_recent_history(10, SortBy::DateCreated).unwrap()[2].id;
        tick();
        db.touch(oldest).unwrap();
        assert_eq!(texts(&db, SortBy::LastUsed), ["2", "4", "3"]);
        assert_eq!(texts(&db, SortBy::DateCreated), ["4", "3", "2"]);
        assert_eq!(db.trim(2, SortBy::LastUsed).unwrap(), 1);
        assert_eq!(texts(&db, SortBy::LastUsed), ["2", "4"]);
    }

    #[test]
    fn stores_all_types() {
        let mut db = Database::in_memory().unwrap();
        let o = opts(10);
        db.save(
            &NewClip::html("<b>x</b>".into(), Some("x".into()), true),
            &o,
        )
        .unwrap();
        db.save(&NewClip::files(vec!["file:///tmp/a".into()]), &o)
            .unwrap();
        db.save_image(&[9u8; 16], 2, 2, 10).unwrap();
        let items = db.fetch_recent_history(10, SortBy::LastUsed).unwrap();
        assert_eq!(
            items.iter().map(|i| i.data_type).collect::<Vec<_>>(),
            [DataType::Image, DataType::Files, DataType::Html]
        );
        assert!(items[0].thumbnail_png.is_some());
        assert!(items[2].has_html);
        let image = db.fetch_clip(items[0].id).unwrap().unwrap();
        assert_eq!(
            crate::images::decode_png(&image.image_png.unwrap())
                .unwrap()
                .0,
            vec![9u8; 16]
        );
        let html = db.fetch_clip(items[2].id).unwrap().unwrap();
        assert_eq!(html.html.as_deref(), Some("<b>x</b>"));
        assert_eq!(html.text.as_deref(), Some("x"));
    }

    #[test]
    fn menu_text_is_cut() {
        let mut db = Database::in_memory().unwrap();
        db.save_text(&"x".repeat(20_000), 10).unwrap();
        let item = &db.fetch_recent_history(1, SortBy::LastUsed).unwrap()[0];
        assert_eq!(item.text.as_ref().unwrap().len(), 10_000);
        assert_eq!(
            db.fetch_clip(item.id).unwrap().unwrap().text.unwrap().len(),
            20_000
        );
    }

    #[test]
    fn clear_delete_and_vacuum() {
        let mut db = Database::in_memory().unwrap();
        db.save_text("a", 10).unwrap();
        db.save_text("b", 10).unwrap();
        let first = db.fetch_recent_history(1, SortBy::LastUsed).unwrap()[0].id;
        db.delete_history(first).unwrap();
        assert_eq!(texts(&db, SortBy::LastUsed), ["a"]);
        db.clear_history().unwrap();
        db.vacuum().unwrap();
        assert_eq!(db.history_count().unwrap(), 0);
    }

    #[test]
    fn folder_delete_cascades() {
        let mut db = Database::in_memory().unwrap();
        let f = db.add_folder("F").unwrap();
        let s = db.add_snippet(f, "t", "c").unwrap();
        db.delete_folder(f).unwrap();
        assert!(db.fetch_snippet(s).unwrap().is_none());
    }

    #[test]
    fn snippets_keep_order_and_move_between_folders() {
        let mut db = Database::in_memory().unwrap();
        let f1 = db.add_folder("One").unwrap();
        let f2 = db.add_folder("Two").unwrap();
        let a = db.add_snippet(f1, "a", "A").unwrap();
        let b = db.add_snippet(f1, "b", "B").unwrap();
        let c = db.add_snippet(f1, "c", "C").unwrap();
        db.move_snippet(c, f1, 0).unwrap();
        db.move_snippet(a, f2, 5).unwrap();
        let tree = db.fetch_snippets_hierarchy().unwrap();
        let titles = |i: usize| {
            tree[i]
                .1
                .iter()
                .map(|s| s.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tree.iter()
                .map(|(f, _)| f.name.as_str())
                .collect::<Vec<_>>(),
            ["Common", "One", "Two"]
        );
        assert_eq!(titles(1), ["c", "b"]);
        assert_eq!(titles(2), ["a"]);
        db.reorder_folders(&[f2, f1, tree[0].0.id]).unwrap();
        db.rename_snippet(b, "bee").unwrap();
        db.update_snippet_content(b, "BEE").unwrap();
        db.set_snippet_enabled(b, false).unwrap();
        db.set_folder_enabled(f2, false).unwrap();
        db.rename_folder(f2, "Deux").unwrap();
        let tree = db.fetch_snippets_hierarchy().unwrap();
        assert_eq!(
            tree[0].0,
            Folder {
                id: f2,
                name: "Deux".into(),
                sort_order: 0,
                enabled: false
            }
        );
        let bee = db.fetch_snippet(b).unwrap().unwrap();
        assert_eq!(
            (bee.title.as_str(), bee.content.as_str(), bee.enabled),
            ("bee", "BEE", false)
        );
    }

    #[test]
    fn import_appends_folders() {
        let mut db = Database::in_memory().unwrap();
        db.import_folders(&[ImportFolder {
            title: "Common".into(),
            snippets: vec![("t".into(), "c".into())],
        }])
        .unwrap();
        let tree = db.fetch_snippets_hierarchy().unwrap();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[1].0.name, "Common");
        assert_eq!(tree[1].1[0].content, "c");
    }
}
