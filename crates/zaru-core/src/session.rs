//! The photos in the working folder and the marks the user has put on them.
//!
//! Marks live here, in memory, and only reach the disk when the user asks.
//! Nothing in a culling pass may move or rewrite a file: the list index is the
//! app's whole sense of "where am I", and mutating the folder underneath it
//! would make the next keypress mean something different every time.

use std::path::{Path, PathBuf};

use serde::Serialize;
use zaru_cr3::Cr3Info;
use zaru_xmp::{Marks, SidecarStyle, REJECTED};

pub struct Photo {
    pub path: PathBuf,
    pub name: String,
    pub info: Cr3Info,
}

/// What the front end needs to draw a frame. The byte range stays on this side.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoView {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub mirrored: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub folder: String,
    pub photos: Vec<PhotoView>,
    pub marks: Vec<Marks>,
}

/// The result of one marking command: the mark that changed and the photo it
/// belongs to. Undo returns the same shape, which is why it carries an index.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkChange {
    pub index: usize,
    pub mark: Marks,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub written: usize,
    pub skipped: usize,
    pub rejected: usize,
    /// First failure, if any. Writing stops there so the rest stays recoverable.
    pub error: Option<String>,
}

struct Change {
    index: usize,
    before: Marks,
    after: Marks,
}

#[derive(Default)]
pub struct Session {
    folder: PathBuf,
    photos: Vec<Photo>,
    marks: Vec<Marks>,
    undo: Vec<Change>,
    redo: Vec<Change>,
}

impl Session {
    pub fn photos(&self) -> &[Photo] {
        &self.photos
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            folder: self.folder.display().to_string(),
            photos: self
                .photos
                .iter()
                .map(|p| {
                    let (rotation, mirrored) = p.info.rotation();
                    PhotoView {
                        name: p.name.clone(),
                        width: p.info.preview.width,
                        height: p.info.preview.height,
                        rotation,
                        mirrored,
                    }
                })
                .collect(),
            marks: self.marks.clone(),
        }
    }

    /// Replaces the session with the CR3 files in `folder`, sorted by name.
    pub fn open(&mut self, folder: &Path) -> Result<(), String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|e| format!("{}: {e}", folder.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("cr3"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();

        if paths.is_empty() {
            return Err(format!("no CR3 files in {}", folder.display()));
        }

        let photos = probe_all(&paths);
        if photos.is_empty() {
            return Err(format!(
                "found {} CR3 files in {} but none held a readable preview",
                paths.len(),
                folder.display()
            ));
        }

        self.folder = folder.to_path_buf();
        self.marks = vec![Marks::default(); photos.len()];
        self.photos = photos;
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    /// Applies a star rating. Pressing the rating a photo already has clears
    /// it, so the same key both sets and unsets.
    pub fn set_star(&mut self, index: usize, stars: i8) -> Option<MarkChange> {
        let current = self.marks.get(index)?.rating;
        let next = if current == stars { 0 } else { stars };
        self.apply(index, |m| m.rating = next)
    }

    /// Rating and rejection are the same XMP field, so rating a rejected photo
    /// un-rejects it and rejecting a rated photo drops the stars. That is the
    /// standard's semantics, not a shortcut.
    pub fn toggle_reject(&mut self, index: usize) -> Option<MarkChange> {
        let rejected = self.marks.get(index)?.is_rejected();
        self.apply(index, |m| m.rating = if rejected { 0 } else { REJECTED })
    }

    pub fn toggle_label(&mut self, index: usize, label: &str) -> Option<MarkChange> {
        let current = self.marks.get(index)?.label.clone();
        let next = if current.as_deref() == Some(label) {
            None
        } else {
            Some(label.to_string())
        };
        self.apply(index, |m| m.label = next)
    }

    fn apply(&mut self, index: usize, edit: impl FnOnce(&mut Marks)) -> Option<MarkChange> {
        let mark = self.marks.get_mut(index)?;
        let before = mark.clone();
        edit(mark);
        let after = mark.clone();
        if before == after {
            return Some(MarkChange { index, mark: after });
        }
        self.undo.push(Change { index, before, after: after.clone() });
        self.redo.clear();
        Some(MarkChange { index, mark: after })
    }

    /// Undo is not a nicety here. In a forty-frame burst the user will hit the
    /// wrong key, and without undo the only repair is to find the frame again.
    pub fn undo(&mut self) -> Option<MarkChange> {
        let change = self.undo.pop()?;
        self.marks[change.index] = change.before.clone();
        let mark = change.before.clone();
        let index = change.index;
        self.redo.push(change);
        Some(MarkChange { index, mark })
    }

    pub fn redo(&mut self) -> Option<MarkChange> {
        let change = self.redo.pop()?;
        self.marks[change.index] = change.after.clone();
        let mark = change.after.clone();
        let index = change.index;
        self.undo.push(change);
        Some(MarkChange { index, mark })
    }

    /// Writes a sidecar for every marked photo, in each requested naming style.
    ///
    /// Rejection is a note in the XMP and nothing more — no file is moved and
    /// none is ever deleted. The first failure stops the run so a full disk or
    /// a read-only folder leaves a partial, recoverable state rather than a
    /// half-rewritten one.
    pub fn write_xmp(&self, styles: &[SidecarStyle]) -> WriteReport {
        let mut report = WriteReport::default();
        for (photo, mark) in self.photos.iter().zip(&self.marks) {
            if mark.is_empty() {
                report.skipped += 1;
                continue;
            }
            if mark.is_rejected() {
                report.rejected += 1;
            }
            for style in styles {
                if let Err(e) = zaru_xmp::apply(&photo.path, mark, *style) {
                    report.error = Some(format!("{}: {e}", photo.name));
                    return report;
                }
            }
            report.written += 1;
        }
        report
    }
}

/// Probes every file up front, in parallel.
///
/// One probe reads only the header, so the whole folder costs well under a
/// second. Doing it here means navigation never waits on metadata, and a file
/// that cannot be read is dropped now rather than blowing up mid-pass.
fn probe_all(paths: &[PathBuf]) -> Vec<Photo> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(paths.len())
        .max(1);

    let mut slots: Vec<Option<Photo>> = (0..paths.len()).map(|_| None).collect();

    std::thread::scope(|scope| {
        let chunk = paths.len().div_ceil(workers);
        for (paths, slots) in paths.chunks(chunk).zip(slots.chunks_mut(chunk)) {
            scope.spawn(move || {
                for (path, slot) in paths.iter().zip(slots) {
                    let Ok(info) = zaru_cr3::probe(path) else {
                        continue;
                    };
                    *slot = Some(Photo {
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        path: path.clone(),
                        info,
                    });
                }
            });
        }
    });

    slots.into_iter().flatten().collect()
}
