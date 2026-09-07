use std::path::{Path, PathBuf};
use zaru_core::{ApplyOperation as Op, BatchEdit as Edit, Recovery, Session};
use zaru_xmp::{Marks, SidecarStyle};

fn session(name: &str, count: usize) -> (Session, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("selection").join(name);
    if dir.exists() { std::fs::remove_dir_all(&dir).unwrap(); }
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3");
    for i in 0..count {
        let file = dir.join(format!("IMG_{i:04}.CR3"));
        std::fs::hard_link(&fixture, &file).or_else(|_| std::fs::copy(&fixture, &file).map(|_| ())).unwrap();
    }
    let mut session = Session::default(); session.open(&dir).unwrap(); (session, dir)
}
const STYLES: &[SidecarStyle] = &[SidecarStyle::ReplaceExtension];

#[test]
fn batches_are_uniform_atomic_deduplicated_and_one_undo_step() {
    let (mut s, _) = session("batch", 3);
    s.set_star(0, 5);
    let result = s.edit_selection(&[0, 0, 1, 2], Edit::Rating(3)).unwrap();
    assert_eq!(result.len(), 3);
    assert!(s.view().marks.iter().all(|m| m.rating == 3));
    assert!(s.edit_selection(&[0, 99], Edit::Rating(1)).is_err());
    assert!(s.edit_selection(&[0], Edit::Rating(8)).is_err());
    assert_eq!(s.undo().len(), 3);
    assert_eq!(s.view().marks.iter().map(|m| m.rating).collect::<Vec<_>>(), vec![5,0,0]);
    assert_eq!(s.redo().len(), 3);
    s.edit_selection(&[0, 1], Edit::Green(true)).unwrap();
    s.edit_selection(&[0, 1], Edit::Green(true)).unwrap();
    assert_eq!(s.undo().len(), 2, "no-op batch must not add history");
}

#[test]
fn selected_xmp_never_moves_or_writes_unselected_and_can_clear_saved_values() {
    for (name, styles) in [("lightroom", vec![SidecarStyle::ReplaceExtension]), ("darktable", vec![SidecarStyle::AppendExtension]), ("both", vec![SidecarStyle::ReplaceExtension, SidecarStyle::AppendExtension])] {
        let (mut s, dir) = session(name, 2);
        s.new_collection("keep", 10).unwrap();
        s.edit_selection(&[0,1], Edit::Rating(4)).unwrap();
        s.edit_selection(&[0,1], Edit::Green(true)).unwrap();
        s.edit_selection(&[0,1], Edit::Collection(Some(0))).unwrap();
        let report = s.apply_selection(Some(&[0]), Op::Xmp, &styles).unwrap();
        assert_eq!(report.completed_xmp, vec![0]); assert!(report.completed_moves.is_empty());
        assert!(!dir.join("keep").exists()); assert_eq!(s.view().pending_xmp, vec![false,true]);
        for style in &styles { assert!(!zaru_xmp::sidecar_path(&dir.join("IMG_0001.CR3"), *style).exists()); }
        s.edit_selection(&[0], Edit::Rating(0)).unwrap(); s.edit_selection(&[0], Edit::Green(false)).unwrap();
        assert_eq!(s.plan_selection(Some(&[0]), Op::Xmp, &styles).unwrap().sidecars, 1);
        s.apply_selection(Some(&[0]), Op::Xmp, &styles).unwrap();
        for style in &styles {
            let xml = std::fs::read_to_string(zaru_xmp::sidecar_path(&dir.join("IMG_0000.CR3"), *style)).unwrap();
            assert_eq!(zaru_xmp::read(&xml), Marks::default());
        }
        assert_eq!(s.apply_selection(Some(&[0]), Op::Xmp, &styles).unwrap().sidecars, 0);
    }
}

#[test]
fn organization_moves_only_selected_siblings_and_preserves_pending_marks() {
    let (mut s, dir) = session("organization", 2);
    std::fs::write(dir.join("IMG_0000.JPG"), b"jpeg").unwrap();
    std::fs::write(dir.join("IMG_0000.xmp"), b"existing").unwrap();
    s.new_collection("keep", 10).unwrap();
    s.edit_selection(&[0,1], Edit::Rating(5)).unwrap();
    s.edit_selection(&[0,1], Edit::Collection(Some(0))).unwrap();
    let report = s.apply_selection(Some(&[0]), Op::Organization, STYLES).unwrap();
    assert_eq!(report.files_moved, 3); assert_eq!(report.sidecars, 0);
    assert_eq!(std::fs::read(dir.join("keep/IMG_0000.xmp")).unwrap(), b"existing");
    assert!(dir.join("IMG_0001.CR3").exists());
    assert_eq!(s.view().assigned, vec![None,Some(0)]); assert_eq!(s.view().pending_xmp, vec![true,true]);
    s.apply_selection(Some(&[0]), Op::Xmp, STYLES).unwrap();
    assert_eq!(zaru_xmp::read(&std::fs::read_to_string(dir.join("keep/IMG_0000.xmp")).unwrap()).rating, 5);
    assert_eq!(s.apply_selection(Some(&[0]), Op::Organization, STYLES).unwrap().moved, 0);
    s.new_collection("second", 10).unwrap();
    s.edit_selection(&[0], Edit::Collection(Some(1))).unwrap();
    assert_eq!(s.apply_selection(Some(&[0]), Op::Organization, STYLES).unwrap().files_moved, 3);
    assert!(dir.join("second/IMG_0000.JPG").exists());
}

#[test]
fn unrelated_collisions_do_not_block_selected_photos() {
    let (mut s, dir) = session("collision", 2);
    s.new_collection("keep",10).unwrap(); s.edit_selection(&[0,1], Edit::Collection(Some(0))).unwrap();
    std::fs::create_dir(dir.join("keep")).unwrap(); std::fs::write(dir.join("keep/IMG_0001.CR3"), b"existing").unwrap();
    assert!(s.plan_selection(Some(&[0]), Op::Organization, STYLES).unwrap().blockers.is_empty());
    assert!(!s.plan_selection(Some(&[1]), Op::Organization, STYLES).unwrap().blockers.is_empty());
    assert_eq!(s.apply_selection(Some(&[0]), Op::Organization, STYLES).unwrap().moved, 1);
    assert_eq!(std::fs::read(dir.join("keep/IMG_0001.CR3")).unwrap(), b"existing");
    assert!(s.apply_selection(Some(&[999]), Op::Xmp, STYLES).is_err());
    assert_eq!(s.apply_selection(Some(&[]), Op::Both, STYLES).unwrap().sidecars, 0);
}

#[test]
fn partial_write_failure_reports_completed_items_and_preserves_remaining_undo() {
    let (mut s, dir) = session("failure", 3);
    s.edit_selection(&[0,1,2], Edit::Rating(5)).unwrap();
    std::fs::create_dir(dir.join("IMG_0001.xmp")).unwrap();
    let report = s.apply_selection(None, Op::Xmp, STYLES).unwrap();
    assert!(report.error.is_some()); assert_eq!(report.completed_xmp, vec![0]); assert_eq!(report.failed_photo, Some(1));
    assert_eq!(s.view().pending_xmp, vec![false,true,true]);
    assert_eq!(s.undo().iter().map(|c| c.index).collect::<Vec<_>>(), vec![1,2]);
    assert_eq!(s.view().marks[0].rating,5); assert_eq!(s.redo().len(),2);
}

#[test]
fn recovery_restores_moved_paths_even_when_root_is_empty() {
    let (mut s, dir) = session("recovery", 2);
    s.new_collection("keep",10).unwrap(); s.edit_selection(&[0,1],Edit::Rating(3)).unwrap();
    s.edit_selection(&[0,1],Edit::Collection(Some(0))).unwrap();
    s.apply_selection(Some(&[0]),Op::Organization,STYLES).unwrap();
    let saved = s.snapshot(); assert!(saved.valid_for(&dir));
    let mut reopened = Session::default(); reopened.open(&dir).unwrap(); assert!(reopened.restore(saved));
    assert_eq!(reopened.photos().len(),2); assert!(reopened.photos()[0].path.starts_with(dir.join("keep")));
    reopened.apply_selection(Some(&[1]),Op::Organization,STYLES).unwrap();
    let saved = reopened.snapshot(); let config = dir.join("config"); saved.save(&config).unwrap();
    let saved = Recovery::load(&config,&dir).unwrap();
    let mut empty = Session::default(); assert!(empty.open(&dir).is_err());
    assert!(empty.open_recoverable(&dir,&saved)); assert!(empty.restore(saved));
    assert_eq!(empty.view().pending_xmp, vec![true,true]);
    assert_eq!(empty.apply_selection(None,Op::Xmp,STYLES).unwrap().sidecars,2);
    assert!(!empty.has_pending());
}

#[test]
fn recovery_rejects_missing_extra_and_outside_files_and_reads_legacy() {
    let (s, dir) = session("invalid-recovery",1);
    let mut saved = s.snapshot(); saved.paths[0] = "../other.CR3".into(); assert!(!saved.valid_for(&dir));
    let mut saved = s.snapshot(); saved.paths.clear(); saved.saved_marks.clear(); assert!(saved.valid_for(&dir));
    let legacy = serde_json::to_value(&saved).unwrap(); let mut legacy = legacy.as_object().unwrap().clone(); legacy.remove("paths"); legacy.remove("savedMarks");
    let recovered: Recovery = serde_json::from_value(legacy.into()).unwrap(); assert!(recovered.valid_for(&dir));
    std::fs::write(dir.join("extra.CR3"),b"extra").unwrap(); assert!(!recovered.valid_for(&dir));
}

#[test]
fn assigning_invalid_collection_does_not_change_any_photo() {
    let (mut s,_) = session("invalid-collection",2);
    assert!(s.edit_selection(&[0,1],Edit::Collection(Some(0))).is_err());
    assert!(s.view().assigned.iter().all(Option::is_none)); assert!(s.undo().is_empty());
}
