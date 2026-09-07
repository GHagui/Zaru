//! A scratch copy of the marks, so a crash does not cost an hour of judgement.
//!
//! This is deliberately not a catalogue. The folder stays the only truth: the
//! file holds nothing that is not already about to be written into sidecars, it
//! is keyed to one working folder, and Apply deletes it. Reopening a folder
//! offers what was there and takes no as an answer.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zaru_xmp::Marks;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovery {
    pub folder: String,
    /// File names, in order. They are what proves the saved marks still line up
    /// with the folder: a photo added or removed and the indices mean nothing.
    pub photos: Vec<String>,
    pub marks: Vec<Marks>,
    pub collections: Vec<String>,
    pub assigned: Vec<Option<usize>>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub saved_marks: Vec<Marks>,
}

/// What the front end shows before asking whether to restore.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryOffer {
    pub marked: usize,
    pub assigned: usize,
    pub collections: usize,
}

impl Recovery {
    pub fn resolved_paths(&self) -> Vec<PathBuf> {
        let paths = if self.paths.is_empty() { &self.photos } else { &self.paths };
        paths.iter().map(|p| Path::new(&self.folder).join(p)).collect()
    }

    pub fn valid_for(&self, folder: &Path) -> bool {
        if Path::new(&self.folder) != folder || !self.matches(&self.photos)
            || (!self.paths.is_empty() && self.paths.len() != self.photos.len())
            || (!self.saved_marks.is_empty() && self.saved_marks.len() != self.photos.len()) { return false; }
        let Ok(root) = folder.canonicalize() else { return false };
        let paths = self.resolved_paths();
        let relative = if self.paths.is_empty() { &self.photos } else { &self.paths };
        if relative.iter().any(|p| Path::new(p).components().any(|c| !matches!(c, std::path::Component::Normal(_)))) { return false; }
        if paths.iter().zip(&self.photos).any(|(p, name)| !p.is_file() || p.file_name().map(|n| n != name.as_str()).unwrap_or(true)
            || !p.canonicalize().map(|p| p.starts_with(&root)).unwrap_or(false)) { return false; }
        let expected: std::collections::BTreeSet<_> = paths.iter().filter(|p| p.parent() == Some(folder)).cloned().collect();
        let Ok(entries) = fs::read_dir(folder) else { return false };
        let actual: std::collections::BTreeSet<_> = entries.filter_map(Result::ok).map(|e| e.path())
            .filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("cr3")).unwrap_or(false)).collect();
        expected == actual
    }
    pub fn offer(&self) -> RecoveryOffer {
        RecoveryOffer {
            marked: self.marks.iter().filter(|m| !m.is_empty()).count(),
            assigned: self.assigned.iter().filter(|a| a.is_some()).count(),
            collections: self.collections.len(),
        }
    }

    /// True when the saved marks still describe this exact list of photos.
    pub fn matches(&self, photos: &[String]) -> bool {
        self.photos == photos
            && self.marks.len() == photos.len()
            && self.assigned.len() == photos.len()
            && self.assigned.iter().flatten().all(|c| *c < self.collections.len())
    }

    pub fn load(dir: &Path, folder: &Path) -> Option<Recovery> {
        let text = fs::read_to_string(path(dir, folder)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let file = path(dir, Path::new(&self.folder));
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        // Through a temporary file and a rename: a half-written recovery file
        // is worse than none, because it would be offered and then fail.
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, serde_json::to_string(self).unwrap_or_default())?;
        match fs::rename(&tmp, &file) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    pub fn discard(dir: &Path, folder: &Path) {
        let _ = fs::remove_file(path(dir, folder));
    }
}

/// One file per working folder, named by a hash of its path — a path is not a
/// filename, and two folders must not share a scratch file.
fn path(dir: &Path, folder: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in folder.to_string_lossy().as_bytes() {
        hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    dir.join("sessions").join(format!("{hash:016x}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_folders_get_different_files() {
        let dir = Path::new("/config");
        let a = path(dir, Path::new("/photos/interlagos"));
        let b = path(dir, Path::new("/photos/goiania"));
        assert_ne!(a, b);
        assert_eq!(a, path(dir, Path::new("/photos/interlagos")), "and it is stable");
    }

    #[test]
    fn marks_for_a_different_set_of_photos_do_not_match() {
        let saved = Recovery {
            folder: "/photos".into(),
            photos: vec!["a.CR3".into(), "b.CR3".into()],
            marks: vec![Marks::default(), Marks::default()],
            collections: vec![],
            assigned: vec![None, None],
            paths: vec![], saved_marks: vec![],
        };
        assert!(saved.matches(&["a.CR3".to_string(), "b.CR3".to_string()]));
        // A photo added, removed or renamed and the indices mean nothing.
        assert!(!saved.matches(&["a.CR3".to_string()]));
        assert!(!saved.matches(&["a.CR3".to_string(), "c.CR3".to_string()]));
    }

    #[test]
    fn an_assignment_pointing_past_the_collections_is_rejected() {
        let saved = Recovery {
            folder: "/photos".into(),
            photos: vec!["a.CR3".into()],
            marks: vec![Marks::default()],
            collections: vec![],
            assigned: vec![Some(3)],
            paths: vec![], saved_marks: vec![],
        };
        assert!(!saved.matches(&["a.CR3".to_string()]));
    }
}
