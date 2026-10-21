//! Snippet import and export in Clipy's XML format, so snippets move between Clipy for macOS
//! and this app:
//!
//! ```xml
//! <folders>
//!   <folder>
//!     <title>…</title>
//!     <snippets>
//!       <snippet><title>…</title><content>…</content></snippet>
//!     </snippets>
//!   </folder>
//! </folders>
//! ```

use anyhow::{bail, Context, Result};

use crate::db::ImportFolder;
use crate::i18n::tr;
use crate::model::{Folder, Snippet};

/// Reads folders and snippets. Missing titles become "untitled folder" / "untitled snippet"
/// and missing content an empty string, as in Clipy. Whitespace in content is kept.
pub fn parse(xml: &str) -> Result<Vec<ImportFolder>> {
    let doc = roxmltree::Document::parse(xml).context("reading the XML file")?;
    let root = doc.root_element();
    if root.tag_name().name() != "folders" {
        bail!(
            "not a Clipy snippets file: the root element is <{}>",
            root.tag_name().name()
        );
    }
    Ok(root
        .children()
        .filter(|n| n.has_tag_name("folder"))
        .map(|folder| ImportFolder {
            title: non_empty(child_text(folder, "title"))
                .unwrap_or_else(|| tr("untitled folder").to_owned()),
            snippets: folder
                .children()
                .filter(|n| n.has_tag_name("snippets"))
                .flat_map(|s| s.children().filter(|n| n.has_tag_name("snippet")))
                .map(|snippet| {
                    (
                        non_empty(child_text(snippet, "title"))
                            .unwrap_or_else(|| tr("untitled snippet").to_owned()),
                        child_text(snippet, "content").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect())
}

/// Clipy (AEXML) reads an empty element as having no value.
fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
}

fn child_text(node: roxmltree::Node, name: &str) -> Option<String> {
    let child = node.children().find(|n| n.has_tag_name(name))?;
    Some(
        child
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect(),
    )
}

/// Writes every folder and snippet, enabled or not.
pub fn export(folders: &[(Folder, Vec<Snippet>)]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<folders>\n");
    for (folder, snippets) in folders {
        out.push_str("  <folder>\n");
        out.push_str(&format!("    <title>{}</title>\n", escape(&folder.name)));
        out.push_str("    <snippets>\n");
        for snippet in snippets {
            out.push_str("      <snippet>\n");
            out.push_str(&format!(
                "        <title>{}</title>\n",
                escape(&snippet.title)
            ));
            out.push_str(&format!(
                "        <content>{}</content>\n",
                escape(&snippet.content)
            ));
            out.push_str("      </snippet>\n");
        }
        out.push_str("    </snippets>\n");
        out.push_str("  </folder>\n");
    }
    out.push_str("</folders>\n");
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Carriage returns would be normalised away by any XML reader.
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a file exported by Clipy for macOS (AEXML).
    const CLIPY_EXPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<folders>
	<folder>
		<title>Work</title>
		<snippets>
			<snippet>
				<title>Signature</title>
				<content>Best regards,
  Me &amp; my team</content>
			</snippet>
			<snippet>
				<title />
				<content>&lt;b&gt;</content>
			</snippet>
		</snippets>
	</folder>
	<folder>
		<snippets />
	</folder>
</folders>"#;

    #[test]
    fn reads_clipy_export() {
        let folders = parse(CLIPY_EXPORT).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].title, "Work");
        assert_eq!(
            folders[0].snippets,
            vec![
                (
                    "Signature".to_string(),
                    "Best regards,\n  Me & my team".to_string()
                ),
                ("untitled snippet".to_string(), "<b>".to_string()),
            ]
        );
        assert_eq!(folders[1].title, "untitled folder");
        assert!(folders[1].snippets.is_empty());
    }

    #[test]
    fn missing_fields_get_defaults() {
        let folders =
            parse("<folders><folder><snippets><snippet/></snippets></folder></folders>").unwrap();
        assert_eq!(
            folders[0].snippets,
            vec![("untitled snippet".to_string(), String::new())]
        );
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse("<plist/>").is_err());
        assert!(parse("not xml").is_err());
    }

    #[test]
    fn export_round_trips() {
        let folder = Folder {
            id: 1,
            name: "A & B <c>".into(),
            sort_order: 0,
            enabled: false,
        };
        let snippets = vec![Snippet {
            id: 1,
            folder_id: 1,
            title: "\"q\" 'a'".into(),
            content: "  line 1\r\nline 2\n\ttab ]]> end".into(),
            sort_order: 0,
            enabled: true,
        }];
        let xml = export(&[(folder, snippets)]);
        let back = parse(&xml).unwrap();
        assert_eq!(back[0].title, "A & B <c>");
        assert_eq!(back[0].snippets[0].0, "\"q\" 'a'");
        assert_eq!(back[0].snippets[0].1, "  line 1\r\nline 2\n\ttab ]]> end");
    }
}
