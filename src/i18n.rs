//! User-visible strings. The English text doubles as the message id, gettext style, which is
//! also how Clipy's catalogs are keyed, so Clipy's translations (de, it, ja, pt-BR, uk, zh-Hans)
//! are used as they are. Linux-only strings fall back to English.

use std::collections::HashMap;
use std::sync::OnceLock;

const LOCALIZABLE: &str = include_str!("../resources/i18n/Localizable.xcstrings");
const SETTINGS: &str = include_str!("../resources/i18n/Settings.xcstrings");
/// The snippet editor's strings come from its window's `.strings` files.
const EDITOR: [(&str, &str); 6] = [
    ("de", include_str!("../resources/i18n/editor/de.strings")),
    ("it", include_str!("../resources/i18n/editor/it.strings")),
    ("ja", include_str!("../resources/i18n/editor/ja.strings")),
    (
        "pt-BR",
        include_str!("../resources/i18n/editor/pt-BR.strings"),
    ),
    ("uk", include_str!("../resources/i18n/editor/uk.strings")),
    (
        "zh-Hans",
        include_str!("../resources/i18n/editor/zh-Hans.strings"),
    ),
];
/// Our wording → Clipy's key, where the meaning is the same.
const ALIASES: [(&str, &str); 2] = [
    ("Edit Snippets…", "Edit Snippets"),
    ("Preferences…", "Settings…"),
];

static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// Returns the translation of `msgid` for the current locale.
pub fn tr(msgid: &'static str) -> &'static str {
    CATALOG
        .get_or_init(|| language().map(|l| catalog(&l)).unwrap_or_default())
        .get(msgid)
        .copied()
        .unwrap_or(msgid)
}

/// Clipy's language code for the user's locale, or `None` for English.
fn language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .and_then(|locale| clipy_language(&locale))
}

fn clipy_language(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next().unwrap_or("");
    let (lang, region) = base.split_once('_').unwrap_or((base, ""));
    Some(
        match (lang, region) {
            ("de", _) => "de",
            ("it", _) => "it",
            ("ja", _) => "ja",
            ("pt", "BR") => "pt-BR",
            ("uk", _) => "uk",
            ("zh", "CN" | "SG") | ("zh", "") => "zh-Hans",
            _ => return None,
        }
        .to_owned(),
    )
}

fn catalog(language: &str) -> HashMap<&'static str, &'static str> {
    let mut map: HashMap<String, String> = HashMap::new();
    for source in [LOCALIZABLE, SETTINGS] {
        read_xcstrings(source, language, &mut map);
    }
    if let Some((_, strings)) = EDITOR.iter().find(|(l, _)| *l == language) {
        read_strings(strings, &mut map);
    }
    for (ours, theirs) in ALIASES {
        if let Some(value) = map.get(theirs).cloned() {
            map.insert(ours.to_owned(), value);
        }
    }
    // Built once per process; leaking gives callers plain &'static str.
    map.into_iter()
        .map(|(k, v)| (&*k.leak(), &*v.leak()))
        .collect()
}

/// Xcode string catalogs: `strings.<key>.localizations.<lang>.stringUnit.value`.
fn read_xcstrings(json: &str, language: &str, map: &mut HashMap<String, String>) {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(json) else {
        return log::warn!("unreadable string catalog");
    };
    let Some(strings) = doc["strings"].as_object() else {
        return;
    };
    for (key, entry) in strings {
        if let Some(value) = entry["localizations"][language]["stringUnit"]["value"].as_str() {
            map.insert(key.clone(), value.to_owned());
        }
    }
}

/// Interface Builder `.strings` files: the English text sits in the comment
/// (`title = "Delete"`) above each translated `"ID.title" = "…";` line.
fn read_strings(text: &str, map: &mut HashMap<String, String>) {
    let mut english: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("/*") {
            english = quoted_after(line, "title = ");
        } else if let (Some(source), Some(value)) = (english.take(), quoted_after(line, "= ")) {
            if !value.is_empty() {
                map.insert(source, value);
            }
        }
    }
}

fn quoted_after(line: &str, marker: &str) -> Option<String> {
    let rest = &line[line.find(marker)? + marker.len()..];
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                other => out.push(other),
            },
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_mapping() {
        assert_eq!(clipy_language("ja_JP.UTF-8").as_deref(), Some("ja"));
        assert_eq!(clipy_language("pt_BR.UTF-8").as_deref(), Some("pt-BR"));
        assert_eq!(clipy_language("pt_PT.UTF-8"), None);
        assert_eq!(clipy_language("zh_CN.UTF-8").as_deref(), Some("zh-Hans"));
        assert_eq!(clipy_language("zh_TW.UTF-8"), None);
        assert_eq!(clipy_language("de_DE@euro").as_deref(), Some("de"));
        assert_eq!(clipy_language("en_US.UTF-8"), None);
        assert_eq!(clipy_language("C"), None);
    }

    #[test]
    fn every_language_translates_core_strings() {
        for lang in ["de", "it", "ja", "pt-BR", "uk", "zh-Hans"] {
            let c = catalog(lang);
            for key in [
                "Clear History",
                "Quit Clipy",
                "History limit:",
                "Delete",
                "Edit Snippets…",
            ] {
                let value = c
                    .get(key)
                    .unwrap_or_else(|| panic!("{lang}: no translation for {key:?}"));
                assert!(!value.is_empty(), "{lang}: empty translation for {key:?}");
            }
        }
        assert_eq!(catalog("ja")["Delete"], "削除");
    }

    #[test]
    fn strings_file_parsing() {
        let mut map = HashMap::new();
        read_strings(
            "/* Class = \"X\"; title = \"Add \\\"Folder\\\"\"; ObjectID = \"a\"; */\n\"a.title\" = \"Ordner\";\n\n/* Class = \"X\"; title = \"Text\"; ObjectID = \"b\"; */\n\"b.title\" = \"\";\n",
            &mut map,
        );
        assert_eq!(
            map.get("Add \"Folder\"").map(String::as_str),
            Some("Ordner")
        );
        assert!(!map.contains_key("Text"));
    }
}
