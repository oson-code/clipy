//! Menu model: a toolkit-neutral port of Clipy's `MenuManager`. The tray (muda), the X11 popup
//! (gtk::Menu) and the GNOME Shell extension (JSON) all render this tree.

use base64::Engine;
use serde_json::{json, Value};

use crate::config::MenuPrefs;
use crate::i18n::tr;
use crate::images;
use crate::model::{uri_list_paths, DataType, Folder, HistoryItem, Snippet};

/// Side of the swatch Clipy draws for colour codes.
pub const SWATCH_SIZE: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Main,
    History,
    Snippets,
    Folder(i64),
}

impl MenuKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "main" => MenuKind::Main,
            "history" => MenuKind::History,
            "snippets" => MenuKind::Snippets,
            _ => MenuKind::Folder(s.strip_prefix("folder:")?.parse().ok()?),
        })
    }

    pub fn encode(self) -> String {
        match self {
            MenuKind::Main => "main".into(),
            MenuKind::History => "history".into(),
            MenuKind::Snippets => "snippets".into(),
            MenuKind::Folder(id) => format!("folder:{id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemId {
    History(i64),
    Snippet(i64),
    ClearHistory,
    EditSnippets,
    Preferences,
    Quit,
}

impl ItemId {
    pub fn encode(self) -> String {
        match self {
            ItemId::History(id) => format!("h:{id}"),
            ItemId::Snippet(id) => format!("s:{id}"),
            ItemId::ClearHistory => "a:clear".into(),
            ItemId::EditSnippets => "a:edit".into(),
            ItemId::Preferences => "a:prefs".into(),
            ItemId::Quit => "a:quit".into(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "a:clear" => ItemId::ClearHistory,
            "a:edit" => ItemId::EditSnippets,
            "a:prefs" => ItemId::Preferences,
            "a:quit" => ItemId::Quit,
            _ => {
                let (kind, id) = s.split_once(':')?;
                let id = id.parse().ok()?;
                match kind {
                    "h" => ItemId::History(id),
                    "s" => ItemId::Snippet(id),
                    _ => return None,
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    None,
    Folder,
    Snippet,
}

impl Icon {
    fn name(self) -> &'static str {
        match self {
            Icon::None => "none",
            Icon::Folder => "folder",
            Icon::Snippet => "snippet",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// The stored thumbnail of an image entry, as PNG.
    Image(Vec<u8>),
    /// RGBA of a colour code entry.
    Color([u8; 4]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuNode {
    /// A disabled title such as "History".
    Header(String),
    Item {
        id: ItemId,
        label: String,
        tooltip: Option<String>,
        icon: Icon,
        preview: Option<Preview>,
        /// Number key that picks the item while the menu is open.
        accel: Option<char>,
    },
    Submenu {
        label: String,
        icon: Icon,
        children: Vec<MenuNode>,
    },
    Separator,
    Action {
        id: ItemId,
        label: String,
        enabled: bool,
    },
}

pub struct MenuSource<'a> {
    pub history: &'a [HistoryItem],
    pub folders: &'a [(Folder, Vec<Snippet>)],
    pub prefs: &'a MenuPrefs,
    pub max_history: usize,
}

pub fn build(kind: MenuKind, src: &MenuSource) -> Vec<MenuNode> {
    match kind {
        MenuKind::Main => main_menu(src),
        MenuKind::History => history_section(src),
        MenuKind::Snippets => snippet_section(src),
        MenuKind::Folder(id) => folder_menu(src, id),
    }
}

fn main_menu(src: &MenuSource) -> Vec<MenuNode> {
    let history = history_section(src);
    let snippets = snippet_section(src);
    let (first, second) = if src.prefs.snippets_above_history {
        (snippets, history)
    } else {
        (history, snippets)
    };
    let mut nodes = first;
    if !second.is_empty() {
        if !nodes.is_empty() {
            nodes.push(MenuNode::Separator);
        }
        nodes.extend(second);
    }
    nodes.push(MenuNode::Separator);
    if src.prefs.show_clear_item {
        nodes.push(MenuNode::Action {
            id: ItemId::ClearHistory,
            label: tr("Clear History").into(),
            enabled: !src.history.is_empty(),
        });
    }
    nodes.push(MenuNode::Action {
        id: ItemId::EditSnippets,
        label: tr("Edit Snippets…").into(),
        enabled: true,
    });
    nodes.push(MenuNode::Action {
        id: ItemId::Preferences,
        label: tr("Preferences…").into(),
        enabled: true,
    });
    nodes.push(MenuNode::Separator);
    nodes.push(MenuNode::Action {
        id: ItemId::Quit,
        label: tr("Quit Clipy").into(),
        enabled: true,
    });
    nodes
}

/// The first `inline_items` entries sit at the top level; the rest go into submenus of
/// `items_per_folder` titled with their cumulative range ("11 - 20"). Inside each submenu the
/// numbering starts again from the first index, as in Clipy.
fn history_section(src: &MenuSource) -> Vec<MenuNode> {
    let prefs = src.prefs;
    let first = first_index(prefs);
    let items: Vec<&HistoryItem> = src.history.iter().take(src.max_history).collect();
    let total = items.len();
    let inline = prefs.inline_items as usize;
    let per_folder = (prefs.items_per_folder as usize).max(1);

    let mut nodes = vec![MenuNode::Header(tr("History").into())];
    for (index, item) in items.iter().enumerate() {
        if index < inline {
            nodes.push(history_item(item, index, first + index, prefs));
            continue;
        }
        let offset = index - inline;
        if offset.is_multiple_of(per_folder) {
            let start = index;
            let end = (start + per_folder).min(total);
            nodes.push(MenuNode::Submenu {
                label: range_title(first + start, first + end - 1),
                icon: folder_icon(prefs),
                children: Vec::new(),
            });
        }
        let number = first + offset % per_folder;
        if let Some(MenuNode::Submenu { children, .. }) = nodes.last_mut() {
            children.push(history_item(item, index, number, prefs));
        }
    }
    nodes
}

fn snippet_section(src: &MenuSource) -> Vec<MenuNode> {
    let folders: Vec<_> = src.folders.iter().filter(|(f, _)| f.enabled).collect();
    if folders.is_empty() {
        return Vec::new();
    }
    let mut nodes = vec![MenuNode::Header(tr("Snippet").into())];
    for (folder, snippets) in folders {
        nodes.push(MenuNode::Submenu {
            label: folder.name.clone(),
            icon: folder_icon(src.prefs),
            children: snippet_items(snippets, src.prefs),
        });
    }
    nodes
}

fn folder_menu(src: &MenuSource, folder_id: i64) -> Vec<MenuNode> {
    let Some((folder, snippets)) = src
        .folders
        .iter()
        .find(|(f, _)| f.id == folder_id && f.enabled)
    else {
        return Vec::new();
    };
    let mut nodes = vec![MenuNode::Header(folder.name.clone())];
    nodes.extend(snippet_items(snippets, src.prefs));
    nodes
}

fn snippet_items(snippets: &[Snippet], prefs: &MenuPrefs) -> Vec<MenuNode> {
    let first = first_index(prefs);
    snippets
        .iter()
        .filter(|s| s.enabled)
        .enumerate()
        .map(|(i, s)| MenuNode::Item {
            id: ItemId::Snippet(s.id),
            label: numbered_title(
                first + i,
                &trimmed_menu_title(&s.title, prefs.max_title_len),
                prefs.mark_numbers,
            ),
            tooltip: tooltip(&s.content, prefs),
            icon: if prefs.show_icons {
                Icon::Snippet
            } else {
                Icon::None
            },
            preview: None,
            accel: None,
        })
        .collect()
}

fn history_item(item: &HistoryItem, index: usize, number: usize, prefs: &MenuPrefs) -> MenuNode {
    let text = item.text.as_deref().unwrap_or("");
    let preview = match item.data_type {
        DataType::Image if prefs.show_images => item.thumbnail_png.clone().map(Preview::Image),
        DataType::Text if prefs.show_color_preview => parse_color_code(text).map(Preview::Color),
        _ => None,
    };
    let tooltip_text = match item.data_type {
        DataType::Files => uri_list_paths(text).join("\n"),
        DataType::Image => String::new(),
        _ => text.to_owned(),
    };
    MenuNode::Item {
        id: ItemId::History(item.id),
        label: numbered_title(
            number,
            &typed_title(item, prefs.max_title_len),
            prefs.mark_numbers,
        ),
        tooltip: tooltip(&tooltip_text, prefs),
        icon: Icon::None,
        preview,
        accel: numeric_accel(index, prefs),
    }
}

fn first_index(prefs: &MenuPrefs) -> usize {
    usize::from(!prefs.start_at_zero)
}

fn folder_icon(prefs: &MenuPrefs) -> Icon {
    if prefs.show_icons {
        Icon::Folder
    } else {
        Icon::None
    }
}

pub fn range_title(first: usize, last: usize) -> String {
    format!("{first} - {last}")
}

pub fn numbered_title(number: usize, title: &str, mark: bool) -> String {
    if mark {
        format!("{number}. {title}")
    } else {
        title.to_owned()
    }
}

/// Clipy's `trimmedMenuTitle`: trim, keep the first line, cut to `max` characters with "...".
pub fn trimmed_menu_title(s: &str, max: u32) -> String {
    let trimmed = s.trim();
    let line = trimmed.split(is_newline).next().unwrap_or("");
    let max = (max as usize).max(3);
    if line.chars().count() <= max {
        line.to_owned()
    } else {
        let mut out: String = line.chars().take(max - 3).collect();
        out.push_str("...");
        out
    }
}

fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Clipy's `typedTitle`: a type prefix followed by the trimmed title.
pub fn typed_title(item: &HistoryItem, max: u32) -> String {
    let text = item.text.as_deref().unwrap_or("");
    let (prefix, title) = match item.data_type {
        DataType::Text => (None, text.to_owned()),
        DataType::Image => (Some(tr("(Image)")), String::new()),
        DataType::Files => (Some(tr("(Files)")), uri_list_paths(text).join("\n")),
        // Like Clipy, "(HTML)" marks copies whose app offered HTML before plain text.
        DataType::Html => (Some(tr("(HTML)")), text.to_owned()),
    };
    let title = trimmed_menu_title(&title, max);
    [prefix.unwrap_or(""), title.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn tooltip(text: &str, prefs: &MenuPrefs) -> Option<String> {
    if !prefs.show_tooltips || text.is_empty() {
        return None;
    }
    Some(text.chars().take(prefs.tooltip_len as usize).collect())
}

/// Number keys for the first ten history entries: 1…9 then 0, or 0…9 when numbering starts at 0.
fn numeric_accel(index: usize, prefs: &MenuPrefs) -> Option<char> {
    if !prefs.numeric_keys || index >= 10 {
        return None;
    }
    let digit = if prefs.start_at_zero {
        index
    } else {
        (index + 1) % 10
    };
    char::from_digit(digit as u32, 10)
}

/// Clipy's colour detection (SwiftHEXColors): an optional `#` followed by exactly 3, 4, 6 or 8
/// hex digits (RGB, RGBA, RRGGBB, RRGGBBAA). The text is not trimmed.
pub fn parse_color_code(text: &str) -> Option<[u8; 4]> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let dup = |v: u8| v << 4 | v;
    Some(match hex.len() {
        3 => [dup(digit(0)?), dup(digit(1)?), dup(digit(2)?), 0xff],
        4 => [
            dup(digit(0)?),
            dup(digit(1)?),
            dup(digit(2)?),
            dup(digit(3)?),
        ],
        6 => [pair(0)?, pair(2)?, pair(4)?, 0xff],
        8 => [pair(0)?, pair(2)?, pair(4)?, pair(6)?],
        _ => return None,
    })
}

/// Serializes the menu for the GNOME Shell extension. Previews become base64 PNGs already sized
/// for the menu.
pub fn to_json(kind: MenuKind, nodes: &[MenuNode], prefs: &MenuPrefs) -> Value {
    json!({ "kind": kind.encode(), "items": nodes_json(nodes, prefs) })
}

fn nodes_json(nodes: &[MenuNode], prefs: &MenuPrefs) -> Vec<Value> {
    nodes
        .iter()
        .map(|node| match node {
            MenuNode::Header(label) => json!({ "type": "header", "label": label }),
            MenuNode::Separator => json!({ "type": "separator" }),
            MenuNode::Submenu {
                label,
                icon,
                children,
            } => json!({
                "type": "submenu",
                "label": label,
                "icon": icon.name(),
                "items": nodes_json(children, prefs),
            }),
            MenuNode::Action { id, label, enabled } => json!({
                "type": "action",
                "id": id.encode(),
                "label": label,
                "enabled": enabled,
            }),
            MenuNode::Item {
                id,
                label,
                tooltip,
                icon,
                preview,
                accel,
            } => {
                let preview = preview.as_ref().and_then(|p| preview_png(p, prefs));
                json!({
                    "type": "item",
                    "id": id.encode(),
                    "label": label,
                    "tooltip": tooltip,
                    "icon": icon.name(),
                    "preview": preview.as_ref().map(|(png, _, _)| {
                        base64::engine::general_purpose::STANDARD.encode(png)
                    }),
                    "preview_w": preview.as_ref().map(|(_, w, _)| w),
                    "preview_h": preview.as_ref().map(|(_, _, h)| h),
                    "accel": accel.map(String::from),
                })
            }
        })
        .collect()
}

/// A PNG of the preview scaled to the configured thumbnail size.
/// A PNG of the preview scaled to the configured thumbnail size, with its size.
pub fn preview_png(preview: &Preview, prefs: &MenuPrefs) -> Option<(Vec<u8>, u32, u32)> {
    let (rgba, w, h) = preview_rgba(preview, prefs)?;
    images::encode_png(&rgba, w, h)
        .map_err(|e| log::warn!("menu preview: {e:#}"))
        .ok()
        .map(|png| (png, w, h))
}

/// RGBA pixels of the preview scaled to the configured thumbnail size.
pub fn preview_rgba(preview: &Preview, prefs: &MenuPrefs) -> Option<(Vec<u8>, u32, u32)> {
    match preview {
        Preview::Image(png) => images::fit_png(png, prefs.thumb_w, prefs.thumb_h)
            .map_err(|e| log::warn!("menu preview: {e:#}"))
            .ok(),
        Preview::Color(rgba) => Some(images::swatch(*rgba, SWATCH_SIZE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_item(id: i64, text: &str) -> HistoryItem {
        HistoryItem {
            id,
            data_type: DataType::Text,
            text: Some(text.into()),
            has_html: false,
            thumbnail_png: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn history(n: i64) -> Vec<HistoryItem> {
        (1..=n)
            .map(|i| text_item(i, &format!("clip {i}")))
            .collect()
    }

    fn labels(nodes: &[MenuNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| match n {
                MenuNode::Header(l) => format!("[{l}]"),
                MenuNode::Separator => "---".into(),
                MenuNode::Submenu {
                    label, children, ..
                } => format!("{label} ({})", children.len()),
                MenuNode::Item { label, .. } | MenuNode::Action { label, .. } => label.clone(),
            })
            .collect()
    }

    fn children(node: &MenuNode) -> &[MenuNode] {
        match node {
            MenuNode::Submenu { children, .. } => children,
            _ => panic!("not a submenu: {node:?}"),
        }
    }

    fn folder(
        id: i64,
        name: &str,
        enabled: bool,
        snippets: &[(&str, bool)],
    ) -> (Folder, Vec<Snippet>) {
        (
            Folder {
                id,
                name: name.into(),
                sort_order: id,
                enabled,
            },
            snippets
                .iter()
                .enumerate()
                .map(|(i, (title, enabled))| Snippet {
                    id: id * 100 + i as i64,
                    folder_id: id,
                    title: (*title).into(),
                    content: format!("{title} content"),
                    sort_order: i as i64,
                    enabled: *enabled,
                })
                .collect(),
        )
    }

    fn src<'a>(
        h: &'a [HistoryItem],
        f: &'a [(Folder, Vec<Snippet>)],
        p: &'a MenuPrefs,
    ) -> MenuSource<'a> {
        MenuSource {
            history: h,
            folders: f,
            prefs: p,
            max_history: 50,
        }
    }

    #[test]
    fn default_main_menu_layout() {
        let h = history(25);
        let f = vec![folder(1, "Common", true, &[("Hello", true)])];
        let p = MenuPrefs::default();
        let nodes = build(MenuKind::Main, &src(&h, &f, &p));
        assert_eq!(
            labels(&nodes),
            [
                "[History]",
                "1 - 10 (10)",
                "11 - 20 (10)",
                "21 - 25 (5)",
                "---",
                "[Snippet]",
                "Common (1)",
                "---",
                "Clear History",
                "Edit Snippets…",
                "Preferences…",
                "---",
                "Quit Clipy",
            ]
        );
        assert_eq!(
            labels(children(&nodes[2])),
            (1..=10)
                .map(|i| format!("{i}. clip {}", i + 10))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_items_then_folders() {
        let h = history(12);
        let p = MenuPrefs {
            inline_items: 3,
            items_per_folder: 5,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert_eq!(
            labels(&nodes),
            [
                "[History]",
                "1. clip 1",
                "2. clip 2",
                "3. clip 3",
                "4 - 8 (5)",
                "9 - 12 (4)"
            ]
        );
        assert_eq!(labels(children(&nodes[5]))[0], "1. clip 9");
    }

    #[test]
    fn numbering_from_zero_and_without_numbers() {
        let h = history(11);
        let p = MenuPrefs {
            start_at_zero: true,
            inline_items: 2,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert_eq!(
            labels(&nodes),
            ["[History]", "0. clip 1", "1. clip 2", "2 - 10 (9)"]
        );
        let p = MenuPrefs {
            mark_numbers: false,
            inline_items: 1,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert_eq!(labels(&nodes)[1], "clip 1");
    }

    #[test]
    fn history_is_capped() {
        let h = history(30);
        let p = MenuPrefs::default();
        let s = MenuSource {
            max_history: 12,
            ..src(&h, &[], &p)
        };
        assert_eq!(
            labels(&build(MenuKind::History, &s)),
            ["[History]", "1 - 10 (10)", "11 - 12 (2)"]
        );
    }

    #[test]
    fn numeric_keys_follow_global_index() {
        let h = history(12);
        let p = MenuPrefs {
            numeric_keys: true,
            inline_items: 12,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        let accels: Vec<_> = nodes
            .iter()
            .filter_map(|n| match n {
                MenuNode::Item { accel, .. } => Some(*accel),
                _ => None,
            })
            .collect();
        let mut expected: Vec<_> = "1234567890".chars().map(Some).collect();
        expected.extend([None, None]);
        assert_eq!(accels, expected);
        let p = MenuPrefs {
            start_at_zero: true,
            ..p
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert!(matches!(
            &nodes[1],
            MenuNode::Item {
                accel: Some('0'),
                ..
            }
        ));
        assert!(matches!(
            &nodes[10],
            MenuNode::Item {
                accel: Some('9'),
                ..
            }
        ));
    }

    #[test]
    fn snippets_above_history_and_disabled_items_hidden() {
        let h = history(1);
        let f = vec![
            folder(1, "On", true, &[("a", true), ("b", false), ("c", true)]),
            folder(2, "Off", false, &[("x", true)]),
        ];
        let p = MenuPrefs {
            snippets_above_history: true,
            inline_items: 5,
            ..Default::default()
        };
        let nodes = build(MenuKind::Main, &src(&h, &f, &p));
        assert_eq!(
            labels(&nodes)[..5],
            ["[Snippet]", "On (2)", "---", "[History]", "1. clip 1"]
        );
        assert_eq!(labels(children(&nodes[1])), ["1. a", "2. c"]);
    }

    #[test]
    fn empty_history_disables_clear_and_hides_snippet_header() {
        let p = MenuPrefs::default();
        let nodes = build(MenuKind::Main, &src(&[], &[], &p));
        assert_eq!(labels(&nodes)[..3], ["[History]", "---", "Clear History"]);
        assert!(matches!(&nodes[2], MenuNode::Action { enabled: false, .. }));
        let p = MenuPrefs {
            show_clear_item: false,
            ..p
        };
        assert_eq!(
            labels(&build(MenuKind::Main, &src(&[], &[], &p)))[2],
            "Edit Snippets…"
        );
    }

    #[test]
    fn folder_popup() {
        let f = vec![
            folder(4, "Work", true, &[("one", true), ("two", true)]),
            folder(5, "Off", false, &[]),
        ];
        let p = MenuPrefs::default();
        assert_eq!(
            labels(&build(MenuKind::Folder(4), &src(&[], &f, &p))),
            ["[Work]", "1. one", "2. two"]
        );
        assert!(build(MenuKind::Folder(5), &src(&[], &f, &p)).is_empty());
        assert!(build(MenuKind::Folder(9), &src(&[], &f, &p)).is_empty());
    }

    #[test]
    fn trimmed_titles_match_clipy() {
        assert_eq!(trimmed_menu_title("  hello\nworld  ", 20), "hello");
        assert_eq!(trimmed_menu_title("abcdefghij", 10), "abcdefghij");
        assert_eq!(trimmed_menu_title("abcdefghijk", 10), "abcdefg...");
        assert_eq!(trimmed_menu_title("abcdef", 1), "...");
        assert_eq!(trimmed_menu_title("привет мир", 8), "приве...");
        assert_eq!(trimmed_menu_title("\r\nsecond", 10), "second");
    }

    #[test]
    fn typed_titles() {
        let mut img = text_item(1, "");
        img.data_type = DataType::Image;
        img.text = None;
        assert_eq!(typed_title(&img, 20), "(Image)");
        let mut files = text_item(2, "file:///home/me/a%20b.txt\nfile:///tmp/c");
        files.data_type = DataType::Files;
        assert_eq!(typed_title(&files, 40), "(Files) /home/me/a b.txt");
        let mut html = text_item(3, "plain");
        html.data_type = DataType::Html;
        assert_eq!(typed_title(&html, 20), "(HTML) plain");
        html.text = None;
        assert_eq!(typed_title(&html, 20), "(HTML)");
    }

    #[test]
    fn tooltips_cut_raw_text() {
        let h = vec![text_item(1, "  line one\nline two")];
        let p = MenuPrefs {
            tooltip_len: 12,
            inline_items: 1,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert!(
            matches!(&nodes[1], MenuNode::Item { tooltip: Some(t), .. } if t == "  line one\nl")
        );
        let p = MenuPrefs {
            show_tooltips: false,
            ..p
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert!(matches!(&nodes[1], MenuNode::Item { tooltip: None, .. }));
    }

    #[test]
    fn color_codes() {
        assert_eq!(parse_color_code("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color_code("F80"), Some([0xff, 0x88, 0x00, 0xff]));
        assert_eq!(
            parse_color_code("#12345678"),
            Some([0x12, 0x34, 0x56, 0x78])
        );
        assert_eq!(parse_color_code("#abcd"), Some([0xaa, 0xbb, 0xcc, 0xdd]));
        assert_eq!(parse_color_code("#12345"), None);
        assert_eq!(parse_color_code(" #fff"), None);
        assert_eq!(parse_color_code("#ggg"), None);
        assert_eq!(parse_color_code("#"), None);
        assert_eq!(parse_color_code("é#ab"), None);
        let h = vec![text_item(1, "#00ff00")];
        let p = MenuPrefs {
            inline_items: 1,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        assert!(matches!(
            &nodes[1],
            MenuNode::Item {
                preview: Some(Preview::Color([0, 255, 0, 255])),
                ..
            }
        ));
    }

    #[test]
    fn ids_round_trip() {
        for id in [
            ItemId::History(3),
            ItemId::Snippet(9),
            ItemId::ClearHistory,
            ItemId::EditSnippets,
            ItemId::Preferences,
            ItemId::Quit,
        ] {
            assert_eq!(ItemId::parse(&id.encode()), Some(id));
        }
        for kind in [
            MenuKind::Main,
            MenuKind::History,
            MenuKind::Snippets,
            MenuKind::Folder(12),
        ] {
            assert_eq!(MenuKind::parse(&kind.encode()), Some(kind));
        }
        assert_eq!(ItemId::parse("x:1"), None);
        assert_eq!(MenuKind::parse("folder:x"), None);
    }

    #[test]
    fn json_has_previews_and_ids() {
        let mut img = text_item(7, "");
        img.data_type = DataType::Image;
        img.text = None;
        img.thumbnail_png = Some(images::encode_png(&[0u8; 4 * 400 * 100], 400, 100).unwrap());
        let h = vec![img];
        let p = MenuPrefs {
            inline_items: 1,
            ..Default::default()
        };
        let nodes = build(MenuKind::History, &src(&h, &[], &p));
        let v = to_json(MenuKind::History, &nodes, &p);
        assert_eq!(v["kind"], "history");
        assert_eq!(v["items"][1]["id"], "h:7");
        assert_eq!(v["items"][1]["label"], "1. (Image)");
        let png = base64::engine::general_purpose::STANDARD
            .decode(v["items"][1]["preview"].as_str().unwrap())
            .unwrap();
        let (_, w, h) = images::decode_png(&png).unwrap();
        assert_eq!((w, h), (100, 25));
        assert_eq!(
            (
                v["items"][1]["preview_w"].clone(),
                v["items"][1]["preview_h"].clone()
            ),
            (100.into(), 25.into())
        );
    }
}
