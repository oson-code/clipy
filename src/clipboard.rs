//! Reading and writing the system clipboard through arboard, which speaks X11 and the Wayland
//! data-control protocol (and falls back to XWayland when a compositor lacks it).

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use arboard::{Clipboard, ImageData};

use crate::clipboard_watcher::SelfEcho;
use crate::images;
use crate::model::{self, path_to_uri, uri_list_paths, DataType, StoredClip};

/// Keep one instance alive for the life of the app: on X11 the clipboard contents Clipy sets
/// are served by arboard for as long as an instance exists.
pub struct ClipboardIo {
    inner: Clipboard,
}

impl ClipboardIo {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Clipboard::new().context("opening the clipboard")?,
        })
    }

    pub fn read_text(&mut self) -> Option<String> {
        self.inner.get_text().ok()
    }

    pub fn read_html(&mut self) -> Option<String> {
        self.inner.get().html().ok()
    }

    /// Returns `file://` URIs.
    pub fn read_files(&mut self) -> Option<Vec<String>> {
        let paths = self.inner.get().file_list().ok()?;
        let uris: Vec<String> = paths
            .iter()
            .filter_map(|p| p.to_str())
            .map(path_to_uri)
            .collect();
        (!uris.is_empty()).then_some(uris)
    }

    pub fn read_image(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let img = self.inner.get_image().ok()?;
        Some((img.bytes.into_owned(), img.width as u32, img.height as u32))
    }

    /// Puts a history entry back on the clipboard. The hash the watcher will see for it goes
    /// into `echo` just before the write, so the watcher does not record it as a new copy. With
    /// `plain`, only its text goes on the clipboard (empty for images), as with Clipy's "paste as
    /// plain text".
    pub fn write_clip(&mut self, clip: &StoredClip, plain: bool, echo: &SelfEcho) -> Result<()> {
        let text = clip.text.clone().unwrap_or_default();
        if plain {
            let text = match clip.data_type {
                DataType::Files => uri_list_paths(&text).join("\n"),
                DataType::Image => String::new(),
                _ => text,
            };
            echo.set(model::hash_text(&text));
            return self.write_text(&text);
        }
        match clip.data_type {
            DataType::Text | DataType::Html => match &clip.html {
                Some(html) => {
                    let alt = clip.text.as_deref();
                    echo.set(model::hash_html(html, alt));
                    self.inner
                        .set_html(html.as_str(), alt)
                        .context("setting HTML")
                }
                None => {
                    echo.set(model::hash_text(&text));
                    self.write_text(&text)
                }
            },
            DataType::Files => {
                // Nautilus, Nemo and Caja all paste files from text/uri-list.
                let paths: Vec<PathBuf> = uri_list_paths(&text)
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                // The watcher rebuilds URIs from the paths it reads back; hash the same form.
                let uris: Vec<String> = paths
                    .iter()
                    .filter_map(|p| p.to_str())
                    .map(path_to_uri)
                    .collect();
                echo.set(model::hash_files(&uris.join("\n")));
                self.inner.set().file_list(&paths).context("setting files")
            }
            DataType::Image => {
                let png = clip
                    .image_png
                    .as_deref()
                    .ok_or_else(|| anyhow!("image entry has no data"))?;
                let (rgba, width, height) = images::decode_png(png)?;
                echo.set(model::hash_image(&rgba, width, height));
                self.inner
                    .set_image(ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(rgba),
                    })
                    .context("setting image")
            }
        }
    }

    pub fn write_text(&mut self, text: &str) -> Result<()> {
        self.inner.set_text(text).context("setting text")
    }
}
