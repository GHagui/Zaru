//! The photos in the working folder, the marks the user has put on them, and
//! the collections they have been sorted into.
//!
//! All of it lives here, in memory, and only reaches the disk when the user
//! asks. Nothing in a culling pass may move or rewrite a file: the list index
//! is the app's whole sense of "where am I", and mutating the folder underneath
//! it would make the next keypress mean something different every time.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use zaru_cr3::Cr3Info;
use zaru_xmp::{Marks, SidecarStyle, REJECTED};

use crate::collections::{validate, MAX_COLLECTIONS};
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
    /// Which burst this frame belongs to, and where it sits inside it. In a
    /// motorsport pass the real question is which of twelve tries at one corner
    /// to keep, so the burst is the unit that matters, not the frame.
    pub burst: usize,
    pub burst_index: usize,
    pub burst_size: usize,
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
    Mark { index: usize, before: Marks, after: Marks },
    Assign { index: usize, before: Option<usize>, after: Option<usize> },
}

#[derive(Default)]
pub struct Session {
    folder: PathBuf,
    photos: Vec<Photo>,
    marks: Vec<Marks>,
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
                    let (rotation, mirrored) = p.info.rotation();
                    let burst = self.bursts[i];
                    PhotoView {
                        name: p.name.clone(),
                        width: p.info.preview.width,
                        height: p.info.preview.height,
                        rotation,
                        mirrored,
                        burst,
                        burst_index: i - self.burst_starts[burst],
                        burst_size: self.burst_sizes[burst],
                    }
                })
                .collect(),
            marks: self.marks.clone(),
            collections: self.collections.clone(),
            assigned: self.assigned.clone(),
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
            return Err(format!("nenhum arquivo CR3 em {}", folder.display()));
        }

        let photos = probe_all(&paths);
        if photos.is_empty() {
            return Err(format!(
                "{} arquivos CR3 em {}, nenhum com prévia legível",
                paths.len(),
                folder.display()
            ));
        }

        let (bursts, burst_starts, burst_sizes) = group_bursts(&photos);
        self.folder = folder.to_path_buf();
        self.marks = vec![Marks::default(); photos.len()];
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
        self.marks.iter().any(|m| !m.is_empty())
            || self.assigned.iter().any(|a| a.is_some())
            || !self.collections.is_empty()
    }

    pub fn snapshot(&self) -> Recovery {
        Recovery {
            folder: self.folder.display().to_string(),
            photos: self.names(),
            marks: self.marks.clone(),
            collections: self.collections.clone(),
            assigned: self.assigned.clone(),
        }
    }

    /// Takes back a snapshot, but only if it still describes this folder.
    /// Undo history is not restored — it belongs to a session that has ended.
    pub fn restore(&mut self, saved: Recovery) -> bool {
        if !saved.matches(&self.names()) {
            return false;
        }
        self.marks = saved.marks;
        self.collections = saved.collections;
        self.assigned = saved.assigned;
        self.undo.clear();
        self.redo.clear();
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
    pub fn new_collection(&mut self, name: &str) -> Result<usize, String> {
        if self.photos.is_empty() {
            return Err("abra uma pasta antes de criar coleções".into());
        }
        if self.collections.len() >= MAX_COLLECTIONS {
            return Err(format!(
                "o limite é {MAX_COLLECTIONS} coleções, uma por tecla de 1 a 9"
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

    // ---------------------------------------------------------------- undo

    /// Undo is not a nicety here. In a forty-frame burst the user will hit the
    /// wrong key, and without undo the only repair is to find the frame again.
    pub fn undo(&mut self) -> Option<PhotoChange> {
        let change = self.undo.pop()?;
        let index = self.rewind(&change, Direction::Back);
        self.redo.push(change);
        Some(self.change_at(index))
    }

    pub fn redo(&mut self) -> Option<PhotoChange> {
        let change = self.redo.pop()?;
        let index = self.rewind(&change, Direction::Forward);
        self.undo.push(change);
        Some(self.change_at(index))
    }

    // --------------------------------------------------------------- apply

    /// Works out what Apply would do, without doing any of it.
    ///
    /// The preview exists so the one irreversible step in the app — moving
    /// files — is never a surprise, and so a name collision is caught while it
    /// is still a sentence on screen rather than a half-finished move.
    pub fn plan(&self, styles: &[SidecarStyle]) -> ApplyPlan {
        let mut plan = ApplyPlan::default();
        let mut per_collection: Vec<(usize, usize)> = vec![(0, 0); self.collections.len()];
        // Destinations claimed by this run, so two photos with the same stem
        // going to one collection are caught as well as pre-existing files.
        let mut claimed: BTreeSet<PathBuf> = BTreeSet::new();
        let index_of_siblings = sibling_index(&self.folder, &self.photos);

        for (index, photo) in self.photos.iter().enumerate() {
            let mark = &self.marks[index];
            let collection = self.assigned[index];

            if mark.is_empty() && collection.is_none() {
                plan.untouched += 1;
                continue;
            }
            plan.evaluated += 1;
            if !mark.is_empty() {
                plan.sidecars += 1;
            }
            if mark.is_rejected() {
                plan.rejected += 1;
            }

            let Some(c) = collection else { continue };
            let target = self.folder.join(&self.collections[c]);
            // Sidecars that do not exist yet still have to be counted and
            // checked: Apply writes them, and then they move too.
            let files = planned_files(photo, &index_of_siblings, mark, styles);

            per_collection[c].0 += 1;
            per_collection[c].1 += files.len();

            for file in &files {
                let Some(name) = file.file_name() else { continue };
                let destination = target.join(name);
                if destination.exists() || !claimed.insert(destination.clone()) {
                    plan.blockers.push(format!(
                        "{}/{} já existe",
                        self.collections[c],
                        name.to_string_lossy()
                    ));
                }
            }
        }

        // A file sitting where a collection folder needs to be would make the
        // whole move impossible, so it is worth saying before anything starts.
        for name in &self.collections {
            let path = self.folder.join(name);
            if path.exists() && !path.is_dir() {
                plan.blockers.push(format!("{name} já existe e não é uma pasta"));
            }
        }

        plan.moves = self
            .collections
            .iter()
            .zip(&per_collection)
            .filter(|(_, (photos, _))| *photos > 0)
            .map(|(name, (photos, files))| PlannedMove {
                collection: name.clone(),
                photos: *photos,
                files: *files,
            })
            .collect();

        plan.blockers.sort();
        plan.blockers.dedup();
        plan
    }

    /// Writes the sidecars, then moves the files. In that order, always: the
    /// `.xmp` has to exist before the move, or it would be left behind in the
    /// working folder while its photo went into a collection.
    ///
    /// Nothing is deleted, here or anywhere. A rejected photo gets `-1` in its
    /// metadata and stays exactly where it is; what to do about it afterwards
    /// is the user's decision, with the user's own tools.
    pub fn apply(&mut self, styles: &[SidecarStyle]) -> ApplyReport {
        let plan = self.plan(styles);
        let mut report = ApplyReport {
            rejected: plan.rejected,
            untouched: plan.untouched,
            ..ApplyReport::default()
        };

        if !plan.blockers.is_empty() {
            report.error = Some(plan.blockers.join("; "));
            return report;
        }

        let written = self.write_xmp(styles);
        report.sidecars = written.written;
        if let Some(e) = written.error {
            report.error = Some(e);
            return report;
        }

        // Indexed only now, so the sidecars just written are in it.
        let index_of_siblings = sibling_index(&self.folder, &self.photos);

        for index in 0..self.photos.len() {
            let Some(c) = self.assigned[index] else { continue };
            let target = self.folder.join(&self.collections[c]);

            if let Err(e) = std::fs::create_dir_all(&target) {
                report.error = Some(format!("{}: {e}", target.display()));
                return report;
            }

            // Everything sharing the stem travels together: the sidecar Zaru
            // just wrote, and the JPEG if the camera was in RAW+JPEG. Splitting
            // them would quietly break the pair.
            for file in siblings(&index_of_siblings, &self.photos[index]) {
                let Some(name) = file.file_name() else { continue };
                let destination = target.join(name);
                if let Err(e) = std::fs::rename(&file, &destination) {
                    report.error = Some(format!("{}: {e}", file.display()));
                    return report;
                }
                report.files_moved += 1;
                if file == self.photos[index].path {
                    self.photos[index].path = destination;
                }
            }
            report.moved += 1;
        }

        // The assignments have been realised, and no undo can un-move a file.
        self.assigned = vec![None; self.photos.len()];
        self.undo.clear();
        self.redo.clear();
        report
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

    fn rewind(&mut self, change: &Change, direction: Direction) -> usize {
        match change {
            Change::Mark { index, before, after } => {
                self.marks[*index] = match direction {
                    Direction::Back => before.clone(),
                    Direction::Forward => after.clone(),
                };
                *index
            }
            Change::Assign { index, before, after } => {
                self.assigned[*index] = match direction {
                    Direction::Back => *before,
                    Direction::Forward => *after,
                };
                *index
            }
        }
    }

    fn change_at(&self, index: usize) -> PhotoChange {
        PhotoChange {
            index,
            mark: self.marks[index].clone(),
            collection: self.assigned[index],
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

/// What would end up moving: the files already on disk, plus the sidecars Apply
/// is about to write. Leaving the second group out would undercount the preview
/// and, worse, miss a collision that only appears once the sidecar exists.
fn planned_files(
    photo: &Photo,
    index: &HashMap<String, Vec<PathBuf>>,
    mark: &Marks,
    styles: &[SidecarStyle],
) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = siblings(index, photo).into_iter().collect();
    if !mark.is_empty() {
        for style in styles {
            files.insert(zaru_xmp::sidecar_path(&photo.path, *style));
        }
    }
    files.into_iter().collect()
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

    for (index, photo) in photos.iter().enumerate() {
        let now = photo.info.captured_ms;
        let same = match (previous, now) {
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
