//! Clipboard and snippet records shared by the database, the watcher and the menus.

use anyhow::Result;

use crate::images;

/// blake3 digest of a clipboard payload. Used for de-duplication and to recognise the
/// clipboard writes Clipy makes itself.
pub type Hash = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Text,
    Image,
    Html,
    Files,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            DataType::Text => "text",
            DataType::Image => "image",
            DataType::Html => "html",
            DataType::Files => "files",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "text" => DataType::Text,
            "image" => DataType::Image,
            "html" => DataType::Html,
            "files" => DataType::Files,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImagePayload {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub thumbnail_png: Vec<u8>,
}

/// A clipboard capture ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct NewClip {
    pub data_type: DataType,
    /// Plain text, the text alternative of HTML, or the `text/uri-list` of files.
    pub text: Option<String>,
    pub html: Option<String>,
    pub image: Option<ImagePayload>,
    pub hash: Hash,
}

impl NewClip {
    pub fn text(text: String) -> Self {
        Self {
            data_type: DataType::Text,
            hash: hash_text(&text),
            text: Some(text),
            html: None,
            image: None,
        }
    }

    /// HTML with its plain-text alternative. `html_first` tells whether the copying app offered
    /// HTML before plain text; only then is the entry an HTML one (labelled "(HTML)", as in
    /// Clipy). Either way the HTML is kept and pasted back.
    pub fn html(html: String, alt_text: Option<String>, html_first: bool) -> Self {
        Self {
            data_type: if html_first || alt_text.is_none() {
                DataType::Html
            } else {
                DataType::Text
            },
            hash: hash_html(&html, alt_text.as_deref()),
            text: alt_text,
            html: Some(html),
            image: None,
        }
    }

    /// `uris` are `file://` URIs, as found in `text/uri-list`.
    pub fn files(uris: Vec<String>) -> Self {
        let list = uris.join("\n");
        Self {
            data_type: DataType::Files,
            hash: hash_files(&list),
            text: Some(list),
            html: None,
            image: None,
        }
    }

    /// Encodes `rgba` as PNG and makes the stored thumbnail. Slow for large images: call it off
    /// the main thread.
    pub fn image(rgba: &[u8], width: u32, height: u32) -> Result<Self> {
        let png = images::encode_png(rgba, width, height)?;
        let thumbnail_png = images::thumbnail_png(rgba, width, height, images::STORED_THUMBNAIL)?;
        Ok(Self {
            data_type: DataType::Image,
            hash: hash_image(rgba, width, height),
            text: None,
            html: None,
            image: Some(ImagePayload {
                png,
                width,
                height,
                thumbnail_png,
            }),
        })
    }
}

pub fn hash_text(text: &str) -> Hash {
    hash_parts(&[b"text", text.as_bytes()])
}

pub fn hash_html(html: &str, alt_text: Option<&str>) -> Hash {
    hash_parts(&[b"html", html.as_bytes(), alt_text.unwrap_or("").as_bytes()])
}

pub fn hash_files(uri_list: &str) -> Hash {
    hash_parts(&[b"files", uri_list.as_bytes()])
}

/// Hashes the decoded pixels, not the PNG, so the same picture matches whichever encoder
/// produced it.
pub fn hash_image(rgba: &[u8], width: u32, height: u32) -> Hash {
    hash_parts(&[b"image", &width.to_le_bytes(), &height.to_le_bytes(), rgba])
}

fn hash_parts(parts: &[&[u8]]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

pub fn hash_hex(hash: &Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// A history row as the menus need it: text is cut to 10 000 characters and the full image is
/// left out.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub id: i64,
    pub data_type: DataType,
    pub text: Option<String>,
    pub has_html: bool,
    pub thumbnail_png: Option<Vec<u8>>,
    pub created_at: String,
    pub updated_at: String,
}

/// A complete history row, for writing it back to the clipboard.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredClip {
    pub id: i64,
    pub data_type: DataType,
    pub text: Option<String>,
    pub html: Option<String>,
    pub image_png: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub id: i64,
    pub folder_id: i64,
    pub title: String,
    pub content: String,
    pub sort_order: i64,
    pub enabled: bool,
}

/// Turns a `text/uri-list` into local paths, skipping comments and non-file URIs.
pub fn uri_list_paths(list: &str) -> Vec<String> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|p| percent_decode(p.trim_start_matches("localhost")))
        .collect()
}

/// `file://` URI for a local path, percent-encoding everything outside the unreserved set.
pub fn path_to_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_differ_by_type() {
        assert_ne!(hash_text("a"), hash_files("a"));
        assert_ne!(hash_text("a"), hash_html("a", None));
        assert_eq!(hash_text("a"), NewClip::text("a".into()).hash);
    }

    #[test]
    fn length_prefix_prevents_ambiguity() {
        assert_ne!(hash_html("ab", Some("c")), hash_html("a", Some("bc")));
    }

    #[test]
    fn image_hash_uses_pixels_and_size() {
        let px = [0u8; 16];
        assert_ne!(hash_image(&px, 2, 2), hash_image(&px, 4, 1));
        let clip = NewClip::image(&px, 2, 2).unwrap();
        assert_eq!(clip.hash, hash_image(&px, 2, 2));
        assert!(!clip.image.unwrap().png.is_empty());
    }

    #[test]
    fn uri_list_round_trip() {
        let uri = path_to_uri("/home/me/My File#1.txt");
        assert_eq!(uri, "file:///home/me/My%20File%231.txt");
        let list =
            format!("# comment\r\n{uri}\r\nhttps://example.com\r\nfile://localhost/tmp/a\r\n");
        assert_eq!(
            uri_list_paths(&list),
            vec!["/home/me/My File#1.txt".to_string(), "/tmp/a".to_string()]
        );
    }

    #[test]
    fn bad_percent_escapes_are_kept() {
        assert_eq!(uri_list_paths("file:///a%2"), vec!["/a%2".to_string()]);
        assert_eq!(uri_list_paths("file:///a%zz"), vec!["/a%zz".to_string()]);
        assert_eq!(uri_list_paths("file:///%é"), vec!["/%é".to_string()]);
    }
}
