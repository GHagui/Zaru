//! Phase 3: sorting photos into collections, and the one step in the whole app
//! that moves a file.

use std::path::{Path, PathBuf};

use zaru_core::collections::{validate, MAX_COLLECTIONS};
use zaru_core::Session;
use zaru_xmp::SidecarStyle;

const STYLE: &[SidecarStyle] = &[SidecarStyle::ReplaceExtension];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3")
}

/// Builds a working folder of real CR3 files, hard-linked so the fixture is not
/// copied 28 MB at a time.
fn folder(name: &str, photos: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("phase3").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for photo in photos {
        let target = dir.join(photo);
        if std::fs::hard_link(fixture(), &target).is_err() {
            std::fs::copy(fixture(), &target).unwrap();
        }
    }
    dir
}

fn open(name: &str, photos: &[&str]) -> (Session, PathBuf) {
    let dir = folder(name, photos);
    let mut session = Session::default();
    session.open(&dir).expect("open");
    (session, dir)
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

// ------------------------------------------------------------------- naming

#[test]
fn a_collection_name_has_to_survive_being_a_folder_name() {
    let taken = vec!["porsche".to_string()];

    assert_eq!(validate("  ferrari  ", &taken).unwrap(), "ferrari");

    for bad in ["", "   ", "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b"] {
        assert!(validate(bad, &taken).is_err(), "{bad:?} should be refused");
    }
    // Windows drops a trailing dot, so the folder would not carry the name the
    // user typed.
    assert!(validate("porsche.", &taken).is_err());
    assert!(validate(".", &taken).is_err());
    assert!(validate("..", &taken).is_err());
    assert!(validate(&"x".repeat(65), &taken).is_err());
    assert!(validate("a\u{7}b", &taken).is_err());
}

#[test]
fn windows_device_names_are_refused_with_or_without_an_extension() {
    let none: Vec<String> = Vec::new();
    for bad in ["CON", "con", "NUL", "COM1", "lpt9", "CON.raw", "aux.2026"] {
        assert!(validate(bad, &none).is_err(), "{bad:?} should be refused");
    }
    assert!(validate("console", &none).is_ok(), "only the exact device name is reserved");
}

#[test]
fn a_name_that_differs_only_in_case_is_the_same_folder() {
    let taken = vec!["Porsche".to_string()];
    assert!(validate("porsche", &taken).is_err());
    assert!(validate("PORSCHE", &taken).is_err());
    assert!(validate("porsche 962", &taken).is_ok());
}

#[test]
fn there_is_one_collection_per_reachable_digit() {
    let (mut session, _) = open("cap", &["IMG_4820.CR3"]);
    for i in 0..MAX_COLLECTIONS {
        session.new_collection(&format!("c{i}")).expect("within the limit");
    }
    // `M` picks by a single digit, so a tenth collection would have no key.
    let over = session.new_collection("c9").unwrap_err();
    assert!(over.contains("1 a 9"), "{over}");
    assert_eq!(session.collections().len(), MAX_COLLECTIONS);
}

// --------------------------------------------------------------- assigning

#[test]
fn a_photo_belongs_to_at_most_one_collection() {
    let (mut session, _) = open("assign", &["IMG_4820.CR3", "IMG_4821.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    let ferrari = session.new_collection("ferrari").unwrap();

    let change = session.assign(0, Some(porsche)).unwrap();
    assert_eq!(change.collection, Some(porsche));

    let change = session.assign(0, Some(ferrari)).unwrap();
    assert_eq!(change.collection, Some(ferrari), "assigning replaces, it does not add");

    let change = session.assign(0, None).unwrap();
    assert_eq!(change.collection, None);
    assert_eq!(session.view().assigned, [None, None]);
}

#[test]
fn assigning_to_a_collection_that_does_not_exist_does_nothing() {
    let (mut session, _) = open("bad-assign", &["IMG_4820.CR3"]);
    assert!(session.assign(0, Some(3)).is_none());
    assert!(session.assign(9, None).is_none());
    assert_eq!(session.view().assigned, [None]);
}

#[test]
fn undo_walks_back_through_collections_and_marks_alike() {
    let (mut session, _) = open("undo", &["IMG_4820.CR3", "IMG_4821.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();

    session.set_star(0, 4);
    session.assign(0, Some(porsche));
    session.toggle_reject(1);

    session.undo().expect("undo reject");
    assert_eq!(session.view().marks[1].rating, 0);

    let back = session.undo().expect("undo assign");
    assert_eq!(back.index, 0);
    assert_eq!(back.collection, None);
    assert_eq!(back.mark.rating, 4, "the star is untouched by undoing the collection");

    session.undo().expect("undo star");
    assert_eq!(session.view().marks[0].rating, 0);
    assert!(session.undo().is_none());

    session.redo();
    session.redo();
    assert_eq!(session.view().assigned[0], Some(porsche));
    assert_eq!(session.view().marks[0].rating, 4);
}

// ------------------------------------------------------------------- plan

#[test]
fn the_plan_says_what_apply_would_do_without_doing_it() {
    let (mut session, dir) = open(
        "plan",
        &["IMG_4820.CR3", "IMG_4821.CR3", "IMG_4822.CR3", "IMG_4823.CR3"],
    );
    let porsche = session.new_collection("porsche").unwrap();
    session.new_collection("vazia").unwrap();

    session.set_star(0, 5);
    session.assign(0, Some(porsche));
    session.assign(1, Some(porsche));
    session.toggle_reject(2);
    // Photo 3 is left alone.

    let plan = session.plan(STYLE);
    assert_eq!(plan.evaluated, 3);
    assert_eq!(plan.sidecars, 2, "only the two marked photos get a sidecar");
    assert_eq!(plan.rejected, 1);
    assert_eq!(plan.untouched, 1);
    assert!(plan.blockers.is_empty());

    // A collection nobody was put into is not part of the plan.
    assert_eq!(plan.moves.len(), 1);
    assert_eq!(plan.moves[0].collection, "porsche");
    assert_eq!(plan.moves[0].photos, 2);

    // And nothing has happened yet.
    assert!(!dir.join("porsche").exists());
    assert!(dir.join("IMG_4820.CR3").exists());
}

#[test]
fn a_destination_that_is_already_taken_blocks_the_run() {
    let (mut session, dir) = open("blocked", &["IMG_4820.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.assign(0, Some(porsche));

    std::fs::create_dir_all(dir.join("porsche")).unwrap();
    std::fs::write(dir.join("porsche/IMG_4820.CR3"), b"already here").unwrap();

    let plan = session.plan(STYLE);
    assert_eq!(plan.blockers.len(), 1);
    assert!(plan.blockers[0].contains("IMG_4820.CR3"), "{:?}", plan.blockers);

    // Refusing to start is the point: nothing is written and nothing is moved.
    let report = session.apply(STYLE);
    assert!(report.error.is_some());
    assert_eq!(report.sidecars, 0);
    assert_eq!(report.moved, 0);
    assert!(!dir.join("IMG_4820.xmp").exists());
    assert!(dir.join("IMG_4820.CR3").exists());
    assert_eq!(
        std::fs::read(dir.join("porsche/IMG_4820.CR3")).unwrap(),
        b"already here",
        "the file that was in the way is untouched"
    );
}

#[test]
fn a_file_standing_where_the_folder_must_go_blocks_the_run() {
    let (mut session, dir) = open("file-in-the-way", &["IMG_4820.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.assign(0, Some(porsche));
    std::fs::write(dir.join("porsche"), b"not a folder").unwrap();

    let plan = session.plan(STYLE);
    assert!(
        plan.blockers.iter().any(|b| b.contains("não é uma pasta")),
        "{:?}",
        plan.blockers
    );
}

// ------------------------------------------------------------------ apply

#[test]
fn apply_writes_the_sidecar_before_the_move_so_it_travels_with_the_photo() {
    let (mut session, dir) = open("apply", &["IMG_4820.CR3", "IMG_4821.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();

    session.set_star(0, 4);
    session.toggle_label(0, "Green");
    session.assign(0, Some(porsche));
    session.set_star(1, 2);

    let report = session.apply(STYLE);
    assert_eq!(report.error, None);
    assert_eq!(report.sidecars, 2);
    assert_eq!(report.moved, 1);

    // The sidecar was written first, so it moved with its photo.
    assert_eq!(names(&dir.join("porsche")), ["IMG_4820.CR3", "IMG_4820.xmp"]);
    let moved = std::fs::read_to_string(dir.join("porsche/IMG_4820.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&moved).rating, 4);

    // The unassigned photo keeps its sidecar where it is.
    assert!(dir.join("IMG_4821.CR3").exists());
    assert!(dir.join("IMG_4821.xmp").exists());
    assert!(!dir.join("IMG_4820.CR3").exists());
}

#[test]
fn everything_sharing_the_stem_travels_together() {
    let (mut session, dir) = open("siblings", &["IMG_4820.CR3", "IMG_48200.CR3"]);
    // Shot in RAW+JPEG, and previously edited in darktable.
    std::fs::write(dir.join("IMG_4820.JPG"), b"jpeg").unwrap();
    std::fs::write(dir.join("IMG_4820.CR3.xmp"), b"<x/>").unwrap();

    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 3);
    session.assign(0, Some(porsche));

    let report = session.apply(STYLE);
    assert_eq!(report.error, None);
    assert_eq!(report.moved, 1);
    assert_eq!(report.files_moved, 4);

    assert_eq!(
        names(&dir.join("porsche")),
        ["IMG_4820.CR3", "IMG_4820.CR3.xmp", "IMG_4820.JPG", "IMG_4820.xmp"]
    );
    // The dot in the prefix is what keeps the next frame out of it.
    assert!(dir.join("IMG_48200.CR3").exists());
}

#[test]
fn a_rejected_photo_is_marked_and_left_exactly_where_it_is() {
    let (mut session, dir) = open("rejected", &["IMG_4820.CR3"]);
    session.toggle_reject(0);

    let report = session.apply(STYLE);
    assert_eq!(report.error, None);
    assert_eq!(report.rejected, 1);
    assert_eq!(report.moved, 0);

    // Zaru never deletes. Rejection is a note in the metadata and nothing else.
    assert!(dir.join("IMG_4820.CR3").exists());
    let xmp = std::fs::read_to_string(dir.join("IMG_4820.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&xmp).rating, zaru_xmp::REJECTED);
}

#[test]
fn after_applying_the_session_still_points_at_the_files_it_moved() {
    let (mut session, dir) = open("repoint", &["IMG_4820.CR3", "IMG_4821.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 1);
    session.assign(0, Some(porsche));
    session.apply(STYLE);

    // Navigation and prefetch read these paths; a stale one would show a hole.
    assert_eq!(session.photos()[0].path, dir.join("porsche/IMG_4820.CR3"));
    assert!(session.photos()[0].path.exists());
    assert_eq!(session.photos()[0].name, "IMG_4820.CR3");
    assert_eq!(session.photos()[1].path, dir.join("IMG_4821.CR3"));

    // The assignments have been realised, and no undo can un-move a file.
    assert_eq!(session.view().assigned, [None, None]);
    assert!(session.undo().is_none());
    assert!(session.redo().is_none());

    // The marks stay, because they now describe what is on disk.
    assert_eq!(session.view().marks[0].rating, 1);
}

#[test]
fn applying_twice_moves_nothing_the_second_time() {
    let (mut session, dir) = open("twice", &["IMG_4820.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 5);
    session.assign(0, Some(porsche));

    assert_eq!(session.apply(STYLE).moved, 1);

    let second = session.apply(STYLE);
    assert_eq!(second.error, None);
    assert_eq!(second.moved, 0, "the assignment was consumed by the first run");
    assert_eq!(names(&dir.join("porsche")), ["IMG_4820.CR3", "IMG_4820.xmp"]);
}

#[test]
fn a_folder_with_no_marks_at_all_applies_to_nothing() {
    let (mut session, dir) = open("empty-apply", &["IMG_4820.CR3", "IMG_4821.CR3"]);

    let plan = session.plan(STYLE);
    assert_eq!(plan.evaluated, 0);
    assert_eq!(plan.untouched, 2);
    assert!(plan.moves.is_empty());

    let report = session.apply(STYLE);
    assert_eq!(report.error, None);
    assert_eq!(report.sidecars, 0);
    assert_eq!(names(&dir), ["IMG_4820.CR3", "IMG_4821.CR3"]);
}

#[test]
fn a_sidecar_that_does_not_exist_yet_still_counts_and_still_collides() {
    let (mut session, dir) = open("prospective", &["IMG_4820.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 3);
    session.assign(0, Some(porsche));

    // The plan has to count the sidecar Apply is about to write, not just what
    // is on disk right now.
    let plan = session.plan(STYLE);
    assert_eq!(plan.moves[0].files, 2, "the CR3 and the .xmp it is about to get");

    // And it has to see the collision that only appears once that file exists.
    std::fs::create_dir_all(dir.join("porsche")).unwrap();
    std::fs::write(dir.join("porsche/IMG_4820.xmp"), b"<x/>").unwrap();
    assert!(!dir.join("IMG_4820.xmp").exists(), "not written yet");

    let plan = session.plan(STYLE);
    assert!(
        plan.blockers.iter().any(|b| b.contains("IMG_4820.xmp")),
        "{:?}",
        plan.blockers
    );

    let report = session.apply(STYLE);
    assert!(report.error.is_some());
    assert!(dir.join("IMG_4820.CR3").exists(), "nothing moved");
    assert!(!dir.join("IMG_4820.xmp").exists(), "nothing written either");
}

#[test]
fn the_both_compat_setting_moves_both_sidecars() {
    let (mut session, dir) = open("both-styles", &["IMG_4820.CR3"]);
    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 4);
    session.assign(0, Some(porsche));

    let both = zaru_core::XmpCompat::Both.styles();
    assert_eq!(session.plan(both).moves[0].files, 3);

    let report = session.apply(both);
    assert_eq!(report.error, None);
    assert_eq!(report.files_moved, 3);
    assert_eq!(
        names(&dir.join("porsche")),
        ["IMG_4820.CR3", "IMG_4820.CR3.xmp", "IMG_4820.xmp"]
    );
}
