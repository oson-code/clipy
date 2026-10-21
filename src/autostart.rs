//! "Launch on login": an XDG autostart entry in `~/.config/autostart/uz.osoncode.Clipy.desktop`.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

fn entry_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| {
        d.config_dir()
            .join("autostart")
            .join(format!("{}.desktop", crate::dbus::BUS_NAME))
    })
}

pub fn is_enabled() -> bool {
    entry_path().is_some_and(|p| p.exists())
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = entry_path().context("no home directory")?;
    if !enabled {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().context("locating the clipy binary")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(&path, desktop_entry(&exe.to_string_lossy()))
        .with_context(|| format!("writing {}", path.display()))
}

fn desktop_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Clipy\n\
         Comment=Clipboard extension app for Linux\n\
         Exec={}\n\
         Icon=clipy\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupWMClass=uz.osoncode.Clipy\n\
         X-GNOME-Autostart-enabled=true\n",
        quote_exec(exe)
    )
}

/// Quotes a path for the desktop entry `Exec` key when it needs it.
fn quote_exec(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        return path.to_owned();
    }
    let escaped: String = path
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("\"{}\"", escaped.replace('%', "%%"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting() {
        assert_eq!(quote_exec("/usr/bin/clipy"), "/usr/bin/clipy");
        assert_eq!(
            quote_exec("/home/me/my apps/clipy"),
            "\"/home/me/my apps/clipy\""
        );
        assert_eq!(quote_exec("/opt/a$b%c/clipy"), "\"/opt/a\\$b%%c/clipy\"");
    }

    #[test]
    fn entry_has_autostart_keys() {
        let entry = desktop_entry("/usr/bin/clipy");
        assert!(entry.contains("Exec=/usr/bin/clipy\n"));
        assert!(entry.contains("X-GNOME-Autostart-enabled=true"));
    }
}
