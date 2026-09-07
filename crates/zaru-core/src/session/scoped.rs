use super::*;
use crate::i18n::Message;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ApplyOperation { Xmp, Organization, #[default] Both }

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum BatchEdit { Rating(i8), Green(bool), Collection(Option<usize>) }

impl Session {
    fn selected_indices(&self, indices: Option<&[usize]>) -> Result<Vec<usize>, Message> {
        let indices: BTreeSet<usize> = match indices {
            Some(indices) => indices.iter().copied().collect(),
            None => (0..self.photos.len()).collect(),
        };
        if indices.iter().any(|i| *i >= self.photos.len()) { return Err(Message::new("selection.invalidPhoto")); }
        Ok(indices.into_iter().collect())
    }

    pub fn edit_selection(&mut self, indices: &[usize], edit: BatchEdit) -> Result<Vec<PhotoChange>, Message> {
        let indices = self.selected_indices(Some(indices))?;
        match &edit {
            BatchEdit::Rating(rating) if !(-1..=5).contains(rating) => return Err(Message::new("selection.invalidRating")),
            BatchEdit::Collection(Some(c)) if *c >= self.collections.len() => return Err(Message::new("selection.invalidCollection")),
            _ => {}
        }
        let mut marks = Vec::new();
        let mut assignments = Vec::new();
        for i in &indices {
            let i = *i;
            match &edit {
                BatchEdit::Collection(value) => {
                    if self.assigned[i] != *value {
                        assignments.push((i, self.assigned[i], *value));
                        self.assigned[i] = *value;
                    }
                }
                _ => {
                    let before = self.marks[i].clone();
                    match edit {
                        BatchEdit::Rating(value) => self.marks[i].rating = value,
                        BatchEdit::Green(value) => self.marks[i].label = value.then(|| "Green".into()),
                        _ => unreachable!(),
                    }
                    if before != self.marks[i] { marks.push((i, before, self.marks[i].clone())); }
                }
            }
        }
        if !marks.is_empty() || !assignments.is_empty() {
            self.undo.push(Change::Batch { marks, assignments });
            self.redo.clear();
        }
        Ok(indices.into_iter().map(|i| self.change_at(i)).collect())
    }

    fn move_files(&self, i: usize, include_xmp: bool, styles: &[SidecarStyle], indexes: &mut HashMap<PathBuf, HashMap<String, Vec<PathBuf>>>) -> Vec<PathBuf> {
        let photo = &self.photos[i];
        let parent = photo.path.parent().unwrap_or(&self.folder).to_path_buf();
        let index = indexes.entry(parent.clone()).or_insert_with(|| sibling_index(&parent, &self.photos));
        let mut files: BTreeSet<_> = siblings(index, photo).into_iter().collect();
        if include_xmp && self.marks[i] != self.saved_marks[i] {
            for style in styles { files.insert(zaru_xmp::sidecar_path(&photo.path, *style)); }
        }
        files.into_iter().collect()
    }

    pub fn plan_selection(&self, indices: Option<&[usize]>, operation: ApplyOperation, styles: &[SidecarStyle]) -> Result<ApplyPlan, Message> {
        let indices = self.selected_indices(indices)?;
        let mut plan = ApplyPlan::default();
        let mut per_collection = vec![(0, 0); self.collections.len()];
        let mut claimed = BTreeSet::new();
        let mut indexes = HashMap::new();
        for i in indices {
            let write = operation != ApplyOperation::Organization && self.marks[i] != self.saved_marks[i];
            let collection = if operation == ApplyOperation::Xmp { None } else { self.assigned[i] };
            if !write && collection.is_none() { plan.untouched += 1; continue; }
            plan.evaluated += 1;
            plan.sidecars += usize::from(write);
            plan.rejected += usize::from(self.marks[i].is_rejected());
            let Some(c) = collection else { continue };
            let target = self.folder.join(&self.collections[c]);
            if target.exists() && !target.is_dir() { plan.blockers.push(Message::new("apply.blocker.notAFolder").with("name", &self.collections[c])); }
            // Reject a collection symlink leading outside this working folder.
            if target.exists() && !target.canonicalize().map(|p| p.starts_with(self.folder.canonicalize().unwrap_or_default())).unwrap_or(false) {
                plan.blockers.push(Message::new("apply.blocker.outsideFolder").with("name", &self.collections[c]));
            }
            let files = self.move_files(i, write, styles, &mut indexes);
            per_collection[c].0 += 1;
            for source in files {
                let destination = target.join(source.file_name().unwrap());
                if source == destination { continue; }
                per_collection[c].1 += 1;
                if destination.exists() || !claimed.insert(destination.clone()) {
                    plan.blockers.push(Message::new("apply.blocker.exists").with("path", destination.display()));
                }
            }
        }
        plan.moves = self.collections.iter().zip(per_collection).filter(|(_, (photos, _))| *photos > 0)
            .map(|(name, (photos, files))| PlannedMove { collection: name.clone(), photos, files }).collect();
        plan.blockers.sort(); plan.blockers.dedup();
        Ok(plan)
    }

    pub fn apply_selection(&mut self, indices: Option<&[usize]>, operation: ApplyOperation, styles: &[SidecarStyle]) -> Result<ApplyReport, Message> {
        let chosen = self.selected_indices(indices)?;
        let plan = self.plan_selection(Some(&chosen), operation, styles)?;
        let mut report = ApplyReport { rejected: plan.rejected, untouched: plan.untouched, ..Default::default() };
        // The blockers are already a list the front end can render one per line;
        // joining them into a sentence here would take that apart.
        if !plan.blockers.is_empty() { report.error = Some(Message::new("apply.blocked")); return Ok(report); }
        let mut indexes = HashMap::new();
        for i in chosen {
            let files = if operation != ApplyOperation::Xmp && self.assigned[i].is_some() {
                self.move_files(i, operation != ApplyOperation::Organization, styles, &mut indexes)
            } else { Vec::new() };
            if operation != ApplyOperation::Organization && self.marks[i] != self.saved_marks[i] {
                for style in styles {
                    if let Err(error) = zaru_xmp::apply(&self.photos[i].path, &self.marks[i], *style) {
                        report.error = Some(Message::new("apply.sidecarFailed").with("name", &self.photos[i].name).with("reason", error));
                        report.failed_photo = Some(i);
                        self.prune_applied(&report);
                        return Ok(report);
                    }
                }
                self.saved_marks[i] = self.marks[i].clone();
                report.sidecars += 1; report.completed_xmp.push(i);
            }
            if operation == ApplyOperation::Xmp { continue; }
            let Some(c) = self.assigned[i] else { continue };
            let target = self.folder.join(&self.collections[c]);
            let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
            let result: std::io::Result<()> = (|| {
                std::fs::create_dir_all(&target)?;
                for source in files {
                    let destination = target.join(source.file_name().unwrap());
                    if source == destination { continue; }
                    // Do not replace a file introduced after the review.
                    if destination.exists() { return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, destination.display().to_string())); }
                    std::fs::rename(&source, &destination)?;
                    moved.push((source, destination));
                }
                Ok(())
            })();
            if let Err(error) = result {
                let mut rollback_errors = Vec::new();
                for (source, destination) in moved.iter().rev() {
                    if let Err(e) = std::fs::rename(destination, source) {
                        rollback_errors.push(format!("{}: {e}", destination.display()));
                        if *source == self.photos[i].path { self.photos[i].path = destination.clone(); }
                    }
                }
                // Two keys rather than one with an optional tail: a sentence
                // stitched together here could not be reordered by whoever
                // translates it.
                report.error = Some(if rollback_errors.is_empty() {
                    Message::new("apply.moveFailed")
                        .with("name", &self.photos[i].name)
                        .with("reason", &error)
                } else {
                    Message::new("apply.moveFailedRollback")
                        .with("name", &self.photos[i].name)
                        .with("reason", &error)
                        .with("rollback", rollback_errors.join("; "))
                });
                report.failed_photo = Some(i); break;
            }
            self.photos[i].path = target.join(&self.photos[i].name);
            self.assigned[i] = None;
            report.files_moved += moved.len(); report.moved += 1; report.completed_moves.push(i);
        }
        self.prune_applied(&report);
        Ok(report)
    }

    fn prune_applied(&mut self, report: &ApplyReport) {
        let marked: HashSet<_> = report.completed_xmp.iter().copied().collect();
        let moved: HashSet<_> = report.completed_moves.iter().copied().collect();
        for history in [&mut self.undo, &mut self.redo] {
            history.retain_mut(|change| match change {
                Change::Mark { index, .. } => !marked.contains(index),
                Change::Assign { index, .. } => !moved.contains(index),
                Change::AssignBurst { before, .. } => { before.retain(|(i, _)| !moved.contains(i)); !before.is_empty() },
                Change::Batch { marks, assignments } => {
                    marks.retain(|(i, _, _)| !marked.contains(i));
                    assignments.retain(|(i, _, _)| !moved.contains(i));
                    !marks.is_empty() || !assignments.is_empty()
                }
            });
        }
    }
}
