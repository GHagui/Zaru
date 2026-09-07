//! Persisted preferences. There is exactly one so far, and it exists because
//! Lightroom and darktable disagree about what an XMP sidecar is called.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zaru_xmp::SidecarStyle;

use crate::keymap::Keymap;

/// Which sidecar filenames Zaru writes.
///
/// Lightroom looks for `IMG_4821.xmp`; darktable writes `IMG_4821.CR3.xmp`.
/// Neither reliably reads the other's name, so the choice belongs to the user
/// and not to a guess in the code.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum XmpCompat {
    /// `IMG_4821.CR3` -> `IMG_4821.xmp`
    #[default]
    Lightroom,
    /// `IMG_4821.CR3` -> `IMG_4821.CR3.xmp`
    Darktable,
    /// Both names, same content. Costs a second small file per photo and
    /// settles the question when a folder is opened in both programs.
    Both,
}

impl XmpCompat {
    pub fn styles(self) -> &'static [SidecarStyle] {
        match self {
            XmpCompat::Lightroom => &[SidecarStyle::ReplaceExtension],
            XmpCompat::Darktable => &[SidecarStyle::AppendExtension],
            XmpCompat::Both => {
                &[SidecarStyle::ReplaceExtension, SidecarStyle::AppendExtension]
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub xmp_compat: XmpCompat,
    pub keymap: Keymap,
}

impl Settings {
    /// Reads the settings file, falling back to defaults for anything missing
    /// or unreadable. A corrupt settings file must never stop the app opening.
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(Self::file(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        fs::write(Self::file(dir), json)
    }

    fn file(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }
}
