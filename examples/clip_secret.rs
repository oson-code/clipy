//! Test helper: puts text on the clipboard marked as a password (`x-kde-passwordManagerHint`),
//! as KeePassXC does, and serves it until another app takes the clipboard.

use arboard::{Clipboard, SetExtLinux};

fn main() {
    let text = std::env::args().nth(1).unwrap_or_else(|| "secret".into());
    Clipboard::new()
        .expect("clipboard")
        .set()
        .exclude_from_history()
        .wait()
        .text(text)
        .expect("setting the clipboard");
}
