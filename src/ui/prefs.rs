//! Persisted UI preferences.
//!
//! Only the colour theme lives here so far. The file is a tiny `key=value`
//! list next to the log, which keeps it human-readable and avoids pulling a
//! serialization dependency into the UI layer.
//!
//! Every read and write is best-effort: a missing or unwritable preference
//! file must never stop the app from starting or from switching theme.

use std::path::{Path, PathBuf};

/// The colour theme the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePref {
    Light,
    Dark,
}

impl ThemePref {
    /// Build from egui's boolean flag.
    pub fn from_dark(dark: bool) -> Self {
        if dark {
            Self::Dark
        } else {
            Self::Light
        }
    }

    /// Whether this preference asks for the dark theme.
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// Stable on-disk token.
    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

/// Preference file location: next to the log in `%LOCALAPPDATA%\sparrowpdf`.
///
/// `SPARROW_PREF_DIR` overrides the directory (portable setups and tests, the
/// same idea as `SPARROW_LOG_DIR`).
pub fn prefs_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("SPARROW_PREF_DIR") {
        return PathBuf::from(dir).join("ui.pref");
    }
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(dir).join("sparrowpdf").join("ui.pref");
    }
    std::env::temp_dir().join("sparrowpdf-ui.pref")
}

/// Read the saved theme, or `None` when the user never chose one.
pub fn load() -> Option<ThemePref> {
    load_from(&prefs_path())
}

/// Persist the theme choice; failures are logged, never surfaced.
pub fn save(pref: ThemePref) {
    if let Err(err) = save_to(&prefs_path(), pref) {
        crate::engine::logging::log("WARN", &format!("保存界面偏好失败：{err}"));
    }
}

fn load_from(path: &Path) -> Option<ThemePref> {
    let body = std::fs::read_to_string(path).ok()?;
    body.lines()
        .find_map(|line| line.trim().strip_prefix("theme="))
        .and_then(ThemePref::parse)
}

fn save_to(path: &Path, pref: ThemePref) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, format!("theme={}\n", pref.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_file_round_trips() {
        // Inside the workspace: the system temp dir is not always writable for
        // test processes (restricted sandboxes, locked-down machines).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("prefs-unit-{}", std::process::id()));
        let path = dir.join("ui.pref");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(load_from(&path), None, "missing file means no preference");
        save_to(&path, ThemePref::Dark).expect("write preference");
        assert_eq!(load_from(&path), Some(ThemePref::Dark));
        save_to(&path, ThemePref::Light).expect("overwrite preference");
        assert_eq!(load_from(&path), Some(ThemePref::Light));

        std::fs::write(&path, "theme=sepia\n").unwrap();
        assert_eq!(load_from(&path), None, "unknown values are ignored");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
