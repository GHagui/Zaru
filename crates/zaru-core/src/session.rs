//! The photos in the working folder, the marks the user has put on them, and
//! the collections they have been sorted into.
//!
//! All of it lives here, in memory, and only reaches the disk when the user
//! asks. Nothing in a culling pass may move or rewrite a file: the list index
//! is the app's whole sense of "where am I", and mutating the folder underneath
//! it would make the next keypress mean something different every time.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod scoped;
pub use scoped::{BatchEdit, ApplyOperation};
use crate::media::{self, Kind, MediaInfo};
use zaru_xmp::{Marks, SidecarStyle, REJECTED};

use crate::collections::validate;
use crate::recovery::Recovery;

/// Frames closer together than this belong to the same burst.
///
/// A camera on continuous drive puts eighty milliseconds between frames; a
/// photographer pressing the shutter again deliberately takes longer than this.
/// The gap is what separates "twelve tries at one corner" from "the next car".
const BURST_GAP_MS: i64 = 700;

pub struct Photo {
    pub path: PathBuf,
    pub name: String,
    pub info: MediaInfo,
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
    /// Which burst this frame belongs to, and where it sits inside it. In a
    /// motorsport pass the real question is which of twelve tries at one corner
    /// to keep, so the burst is the unit that matters, not the frame.
    pub burst: usize,
    pub burst_index: usize,
    pub burst_size: usize,
    /// When the shutter fired, for the caption under each grid tile. One number
    /// per photo is cheap; the rest of the Exif is fetched only when the panel
    /// that shows it is open.
    pub captured: Option<i64>,
    pub kind: Kind,
    /// Milliseconds of footage, for a video.
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub folder: String,
    pub photos: Vec<PhotoView>,
    pub marks: Vec<Marks>,
    pub collections: Vec<String>,
    /// Index into `collections`, per photo. A photo belongs to at most one.
    pub assigned: Vec<Option<usize>>,
    pub pending_xmp: Vec<bool>,
}

/// The result of one command that changed a photo. Undo returns the same shape,
/// which is why it carries an index: the front end has to jump back to the
/// photo it repaired.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoChange {
    pub index: usize,
    pub mark: Marks,
    pub collection: Option<usize>,
    pub pending_xmp: bool,
}

/// What Apply is about to do, worked out before anything is touched.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPlan {
    pub evaluated: usize,
    pub sidecars: usize,
    pub rejected: usize,
    pub untouched: usize,
    pub moves: Vec<PlannedMove>,
    /// Problems that would make the run fail partway. While this is non-empty,
    /// Apply refuses to start.
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedMove {
    pub collection: String,
    pub photos: usize,
    /// Photos plus their siblings — the sidecar, and the JPEG when the camera
    /// was shooting RAW+JPEG.
    pub files: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub completed_xmp: Vec<usize>,
    pub completed_moves: Vec<usize>,
    pub failed_photo: Option<usize>,
    pub sidecars: usize,
    pub moved: usize,
    pub files_moved: usize,
    pub rejected: usize,
    pub untouched: usize,
    /// Where the run stopped. Everything before it is done and everything after
    /// it is untouched.
    pub error: Option<String>,
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

enum Change {
    Batch { marks: Vec<(usize, Marks, Marks)>, assignments: Vec<(usize, Option<usize>, Option<usize>)> },
    Mark { index: usize, before: Marks, after: Marks },
    Assign { index: usize, before: Option<usize>, after: Option<usize> },
    /// A whole burst sent to one collection. It was one keystroke, so it is one
    /// step of undo — taking twelve presses of Ctrl+Z to walk back one press of
    /// the collection key would be its own kind of wrong.
    AssignBurst { before: Vec<(usize, Option<usize>)>, after: Option<usize> },
}

#[derive(Default)]
pub struct Session {
    folder: PathBuf,
    photos: Vec<Photo>,
    marks: Vec<Marks>,
    saved_marks: Vec<Marks>,
    collections: Vec<String>,
    assigned: Vec<Option<usize>>,
    /// Burst id per photo, plus where each burst starts and how long it is.
    bursts: Vec<usize>,
    burst_starts: Vec<usize>,
    burst_sizes: Vec<usize>,
    undo: Vec<Change>,
    redo: Vec<Change>,
}

impl Session {
    /// What the camera recorded about one frame, for the info panel.
    pub fn exif(&self, index: usize) -> Option<zaru_cr3::Exif> {
        self.photos.get(index).map(|p| p.info.exif.clone())
    }

    /// Where a file actually lives, for the handler that streams it.
    pub fn media_path(&self, index: usize) -> Option<PathBuf> {
        self.photos.get(index).map(|p| p.path.clone())
    }

    pub fn photos(&self) -> &[Photo] {
        &self.photos
    }

    pub fn collections(&self) -> &[String] {
        &self.collections
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            folder: self.folder.display().to_string(),
            photos: self
                .photos
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let burst = self.bursts[i];
                    PhotoView {
                        name: p.name.clone(),
                        width: p.info.width,
                        height: p.info.height,
                        rotation: p.info.rotation,
                        mirrored: p.info.mirrored,
                        burst,
                        burst_index: i - self.burst_starts[burst],
                        burst_size: self.burst_sizes[burst],
                        captured: p.info.captured_ms,
                        kind: p.info.kind,
                        duration_ms: p.info.duration_ms,
                    }
                })
                .collect(),
            marks: self.marks.clone(),
            collections: self.collections.clone(),
            assigned: self.assigned.clone(),
            pending_xmp: self.marks.iter().zip(&self.saved_marks).map(|(a,b)| a != b).collect(),
        }
    }

    /// Replaces the session with the CR3 files in `folder`, sorted by name.
    pub fn open(&mut self, folder: &Path) -> Result<(), String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|e| format!("{}: {e}", folder.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| media::is_supported(p))
            .collect();
        paths.sort();

        if paths.is_empty() {
            return Err(format!("nenhuma foto ou vídeo em {}", folder.display()));
        }

        let photos = probe_all(&paths);
        if photos.is_empty() {
            return Err(format!(
                "{} arquivos em {}, nenhum legível",
                paths.len(),
                folder.display()
            ));
        }

        let (bursts, burst_starts, burst_sizes) = group_bursts(&photos);
        self.folder = folder.to_path_buf();
        self.marks = vec![Marks::default(); photos.len()];
        self.saved_marks = self.marks.clone();
        self.assigned = vec![None; photos.len()];
        self.collections.clear();
        self.bursts = bursts;
        self.burst_starts = burst_starts;
        self.burst_sizes = burst_sizes;
        self.photos = photos;
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    // ------------------------------------------------------------ recovery

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn names(&self) -> Vec<String> {
        self.photos.iter().map(|p| p.name.clone()).collect()
    }

    /// True when there is anything worth saving against a crash.
    pub fn has_work(&self) -> bool {
        self.marks != self.saved_marks
            || self.assigned.iter().any(|a| a.is_some())
            || !self.collections.is_empty()
    }

    pub fn has_pending(&self) -> bool {
        self.marks != self.saved_marks || self.assigned.iter().any(Option::is_some)
    }

    pub fn snapshot(&self) -> Recovery {
        Recovery {
            folder: self.folder.display().to_string(),
            photos: self.names(),
            marks: self.marks.clone(),
            collections: self.collections.clone(),
            assigned: self.assigned.clone(),
            paths: self.photos.iter().map(|p| p.path.strip_prefix(&self.folder).unwrap().to_string_lossy().to_string()).collect(),
            saved_marks: self.saved_marks.clone(),
        }
    }

    pub fn restore(&mut self, saved: Recovery) -> bool {
        if !saved.valid_for(&self.folder) { return false; }
        let paths = saved.resolved_paths();
        let photos = probe_all(&paths);
        if photos.len() != paths.len() { return false; }
        let (bursts, starts, sizes) = group_bursts(&photos);
        self.photos = photos;
        self.bursts = bursts;
        self.burst_starts = starts;
        self.burst_sizes = sizes;
        self.saved_marks = if saved.saved_marks.is_empty() { vec![Marks::default(); saved.marks.len()] } else { saved.saved_marks };
        self.marks = saved.marks;
        self.collections = saved.collections;
        self.assigned = saved.assigned;
        self.undo.clear();
        self.redo.clear();
        true
    }

    /// Makes an all-moved folder available so recovery can still be offered.
    pub fn open_recoverable(&mut self, folder: &Path, saved: &Recovery) -> bool {
        if !saved.valid_for(folder) { return false; }
        self.folder = folder.to_path_buf();
        if !self.restore(saved.clone()) { return false; }
        self.marks = self.saved_marks.clone();
        self.assigned.fill(None);
        self.collections.clear();
        true
    }

    // ------------------------------------------------------------- marking

    /// Applies a star rating. Pressing the rating a photo already has clears
    /// it, so the same key both sets and unsets.
    pub fn set_star(&mut self, index: usize, stars: i8) -> Option<PhotoChange> {
        let current = self.marks.get(index)?.rating;
        let next = if current == stars { 0 } else { stars };
        self.edit_mark(index, |m| m.rating = next)
    }

    /// Rating and rejection are the same XMP field, so rating a rejected photo
    /// un-rejects it and rejecting a rated photo drops the stars. That is the
    /// standard's semantics, not a shortcut.
    pub fn toggle_reject(&mut self, index: usize) -> Option<PhotoChange> {
        let rejected = self.marks.get(index)?.is_rejected();
        self.edit_mark(index, |m| m.rating = if rejected { 0 } else { REJECTED })
    }

    pub fn toggle_label(&mut self, index: usize, label: &str) -> Option<PhotoChange> {
        let current = self.marks.get(index)?.label.clone();
        let next = if current.as_deref() == Some(label) {
            None
        } else {
            Some(label.to_string())
        };
        self.edit_mark(index, |m| m.label = next)
    }

    // ---------------------------------------------------------- collections

    /// Registers a collection. The folder is not created here — nothing on disk
    /// changes until Apply, so abandoning a session leaves no empty folders
    /// behind.
    ///
    /// `limit` is how many collection keys are bound: a collection the keyboard
    /// cannot reach is not worth having, so the key map is what caps this.
    pub fn new_collection(&mut self, name: &str, limit: usize) -> Result<usize, String> {
        if self.photos.is_empty() {
            return Err("abra uma pasta antes de criar coleções".into());
        }
        if self.collections.len() >= limit {
            return Err(format!(
                "o limite é {limit} coleções, uma por tecla mapeada"
            ));
        }
        let name = validate(name, &self.collections)?;
        self.collections.push(name);
        Ok(self.collections.len() - 1)
    }

    /// Puts a photo in a collection, or takes it out with `None`. A photo
    /// belongs to at most one, so this replaces rather than adds.
    pub fn assign(&mut self, index: usize, collection: Option<usize>) -> Option<PhotoChange> {
        if index >= self.photos.len() {
            return None;
        }
        if let Some(c) = collection {
            if c >= self.collections.len() {
                return None;
            }
        }

        let before = self.assigned[index];
        self.assigned[index] = collection;
        if before != collection {
            self.undo.push(Change::Assign { index, before, after: collection });
            self.redo.clear();
        }
        Some(self.change_at(index))
    }

    /// Sends every frame of `index`'s burst to one collection.
    ///
    /// In a motorsport pass a burst is one car through one corner, so it almost
    /// always belongs in one place. Deciding that twelve times is twelve chances
    /// to slip, and twelve keystrokes for one decision.
    pub fn assign_burst(&mut self, index: usize, collection: Option<usize>) -> Vec<PhotoChange> {
        if index >= self.photos.len() {
            return Vec::new();
        }
        if let Some(c) = collection {
            if c >= self.collections.len() {
                return Vec::new();
            }
        }

        let burst = self.bursts[index];
        let members: Vec<usize> = (0..self.photos.len())
            .filter(|i| self.bursts[*i] == burst)
            .collect();

        let before: Vec<(usize, Option<usize>)> =
            members.iter().map(|i| (*i, self.assigned[*i])).collect();
        for i in &members {
            self.assigned[*i] = collection;
        }
        if before.iter().any(|(_, was)| *was != collection) {
            self.undo.push(Change::AssignBurst { before, after: collection });
            self.redo.clear();
        }
        members.into_iter().map(|i| self.change_at(i)).collect()
    }

    // ---------------------------------------------------------------- undo

    /// Undo is not a nicety here. In a forty-frame burst the user will hit the
    /// wrong key, and without undo the only repair is to find the frame again.
    /// Returns every photo the step touched, the one to look at first.
    pub fn undo(&mut self) -> Vec<PhotoChange> {
        let Some(change) = self.undo.pop() else {
            return Vec::new();
        };
        let touched = self.rewind(&change, Direction::Back);
        self.redo.push(change);
        touched.into_iter().map(|i| self.change_at(i)).collect()
    }

    pub fn redo(&mut self) -> Vec<PhotoChange> {
        let Some(change) = self.redo.pop() else {
            return Vec::new();
        };
        let touched = self.rewind(&change, Direction::Forward);
        self.undo.push(change);
        touched.into_iter().map(|i| self.change_at(i)).collect()
    }

    // --------------------------------------------------------------- apply

    /// Works out what Apply would do, without doing any of it.
    ///
    /// The preview exists so the one irreversible step in the app — moving
    /// files — is never a surprise, and so a name collision is caught while it
    /// is still a sentence on screen rather than a half-finished move.
    pub fn plan(&self, styles: &[SidecarStyle]) -> ApplyPlan {
        self.plan_selection(None, ApplyOperation::Both, styles).unwrap_or_default()
    }

    pub fn apply(&mut self, styles: &[SidecarStyle]) -> ApplyReport {
        self.apply_selection(None, ApplyOperation::Both, styles).unwrap_or_else(|error| ApplyReport { error: Some(error), ..Default::default() })
    }

    /// Writes a sidecar for every marked photo, in each requested naming style.
    ///
    /// The first failure stops the run, so a full disk or a read-only folder
    /// leaves a partial, recoverable state rather than a half-rewritten one.
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

    // ------------------------------------------------------------ internals

    fn edit_mark(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Marks),
    ) -> Option<PhotoChange> {
        let mark = self.marks.get_mut(index)?;
        let before = mark.clone();
        edit(mark);
        let after = mark.clone();
        if before != after {
            self.undo.push(Change::Mark { index, before, after });
            self.redo.clear();
        }
        Some(self.change_at(index))
    }

    fn rewind(&mut self, change: &Change, direction: Direction) -> Vec<usize> {
        match change {
            Change::Batch { marks, assignments } => {
                let mut indices = BTreeSet::new();
                for (i, before, after) in marks {
                    self.marks[*i] = match direction { Direction::Back => before.clone(), Direction::Forward => after.clone() };
                    indices.insert(*i);
                }
                for (i, before, after) in assignments {
                    self.assigned[*i] = match direction { Direction::Back => *before, Direction::Forward => *after };
                    indices.insert(*i);
                }
                indices.into_iter().collect()
            }
            Change::Mark { index, before, after } => {
                self.marks[*index] = match direction {
                    Direction::Back => before.clone(),
                    Direction::Forward => after.clone(),
                };
                vec![*index]
            }
            Change::Assign { index, before, after } => {
                self.assigned[*index] = match direction {
                    Direction::Back => *before,
                    Direction::Forward => *after,
                };
                vec![*index]
            }
            Change::AssignBurst { before, after } => {
                for (index, was) in before {
                    self.assigned[*index] = match direction {
                        Direction::Back => *was,
                        Direction::Forward => *after,
                    };
                }
                before.iter().map(|(index, _)| *index).collect()
            }
        }
    }

    fn change_at(&self, index: usize) -> PhotoChange {
        PhotoChange {
            index,
            mark: self.marks[index].clone(),
            collection: self.assigned[index],
            pending_xmp: self.marks[index] != self.saved_marks[index],
        }
    }
}

enum Direction {
    Back,
    Forward,
}

/// Groups every file in the folder under the photo it belongs to.
///
/// Built once per Apply rather than per photo: a pass over two thousand files
/// for each of five hundred moves would be a million entries scanned to answer
/// a question one directory listing already contains.
///
/// A file belongs to a photo when peeling extensions off its name reaches that
/// photo's stem. Peeling, rather than matching the parsed stem, is what handles
/// darktable's `IMG_4821.CR3.xmp` — whose own stem is `IMG_4821.CR3`, not
/// `IMG_4821`. Peeling at the dots is also what keeps `IMG_48210.CR3` out.
fn sibling_index(folder: &Path, photos: &[Photo]) -> HashMap<String, Vec<PathBuf>> {
    let stems: HashSet<String> = photos.iter().filter_map(|p| stem_key(&p.path)).collect();

    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return map;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let mut candidate = name.as_str();
        while let Some(cut) = candidate.rfind('.') {
            candidate = &candidate[..cut];
            if stems.contains(candidate) {
                map.entry(candidate.to_string()).or_default().push(entry.path());
                break;
            }
        }
    }

    for files in map.values_mut() {
        files.sort();
    }
    map
}

/// Windows compares filenames without case, so the index keys do too.
fn stem_key(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_string_lossy().to_lowercase())
}

fn siblings(index: &HashMap<String, Vec<PathBuf>>, photo: &Photo) -> Vec<PathBuf> {
    stem_key(&photo.path)
        .and_then(|key| index.get(&key).cloned())
        .unwrap_or_else(|| vec![photo.path.clone()])
}

/// Splits the pass into bursts by the gap between shutter times.
///
/// A frame with no timestamp gets a burst of its own rather than being folded
/// into its neighbour: guessing would put an unrelated frame inside a group the
/// user then judges as one.
fn group_bursts(photos: &[Photo]) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut ids = Vec::with_capacity(photos.len());
    let mut starts: Vec<usize> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut previous: Option<i64> = None;
    let mut previous_was_video = false;

    for (index, photo) in photos.iter().enumerate() {
        let now = photo.info.captured_ms;
        let video = photo.info.is_video();
        // A burst is a run of tries at one subject, and a clip is not one of
        // them. A video that happens to start right after a burst would be
        // folded into it by time alone, and then judged as if it were another
        // frame of the same thing.
        let same = !video
            && !previous_was_video
            && match (previous, now) {
                (Some(before), Some(now)) => now >= before && now - before <= BURST_GAP_MS,
                _ => false,
            };
        if same {
            let id = sizes.len() - 1;
            ids.push(id);
            sizes[id] += 1;
        } else {
            ids.push(sizes.len());
            starts.push(index);
            sizes.push(1);
        }
        previous = now;
        previous_was_video = video;
    }
    (ids, starts, sizes)
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
                    let Ok(info) = MediaInfo::probe(path) else {
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
