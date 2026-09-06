//! Phase 2 behaviour: what the marking keys mean, and what reaches the disk.
//!
//! The folder is built from hard links to the one real CR3 in the repo, so the
//! session probes actual files without copying 28 MB per photo.

use std::path::{Path, PathBuf};

use zaru_core::{Session, Settings, XmpCompat};
use zaru_xmp::{Marks, SidecarStyle, REJECTED};

fn folder(name: &str, count: usize) -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    for i in 0..count {
        let target = dir.join(format!("IMG_{:04}.CR3", 4820 + i));
        if std::fs::hard_link(&fixture, &target).is_err() {
            std::fs::copy(&fixture, &target).unwrap();
        }
    }
    dir
}

fn open(name: &str, count: usize) -> (Session, PathBuf) {
    let dir = folder(name, count);
    let mut session = Session::default();
    session.open(&dir).expect("open");
    (session, dir)
}

fn rating(session: &Session, index: usize) -> i8 {
    session.view().marks[index].rating
}

fn label(session: &Session, index: usize) -> Option<String> {
    session.view().marks[index].label.clone()
}

#[test]
fn opening_lists_every_cr3_in_name_order() {
    let (session, _) = open("order", 4);
    let names: Vec<_> = session.view().photos.into_iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        ["IMG_4820.CR3", "IMG_4821.CR3", "IMG_4822.CR3", "IMG_4823.CR3"]
    );
    assert!(session.view().marks.iter().all(|m| m.is_empty()));
}

#[test]
fn a_folder_without_cr3_files_is_an_error_not_an_empty_session() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), b"nothing here").unwrap();

    let mut session = Session::default();
    assert!(session.open(&dir).is_err());
}

#[test]
fn the_same_star_twice_clears_the_rating() {
    let (mut session, _) = open("stars", 2);

    session.set_star(0, 3);
    assert_eq!(rating(&session, 0), 3);
    session.set_star(0, 5);
    assert_eq!(rating(&session, 0), 5);
    session.set_star(0, 5);
    assert_eq!(rating(&session, 0), 0);
}

#[test]
fn rating_and_rejection_contend_for_one_field() {
    let (mut session, _) = open("contend", 2);

    session.set_star(0, 4);
    session.toggle_reject(0);
    assert_eq!(rating(&session, 0), REJECTED, "rejecting drops the stars");

    session.set_star(0, 2);
    assert_eq!(rating(&session, 0), 2, "a star lifts the rejection");

    session.toggle_reject(0);
    session.toggle_reject(0);
    assert_eq!(rating(&session, 0), 0, "un-rejecting lands on unrated");
}

#[test]
fn the_label_toggles_and_is_independent_of_the_rating() {
    let (mut session, _) = open("label", 2);

    session.toggle_label(0, "Green");
    assert_eq!(label(&session, 0).as_deref(), Some("Green"));
    session.toggle_reject(0);
    assert_eq!(label(&session, 0).as_deref(), Some("Green"));
    session.toggle_label(0, "Green");
    assert_eq!(label(&session, 0), None);
}

#[test]
fn undo_walks_back_through_every_mark_and_redo_returns() {
    let (mut session, _) = open("undo", 3);

    session.set_star(0, 3);
    session.toggle_label(1, "Green");
    session.toggle_reject(2);

    let back = session.undo().expect("undo reject");
    assert_eq!(back.index, 2);
    assert_eq!(rating(&session, 2), 0);

    let back = session.undo().expect("undo label");
    assert_eq!(back.index, 1);
    assert_eq!(label(&session, 1), None);

    session.undo().expect("undo star");
    assert_eq!(rating(&session, 0), 0);
    assert!(session.undo().is_none(), "nothing left to undo");

    session.redo();
    assert_eq!(rating(&session, 0), 3);
    session.redo();
    assert_eq!(label(&session, 1).as_deref(), Some("Green"));
    session.redo();
    assert_eq!(rating(&session, 2), REJECTED);
    assert!(session.redo().is_none());
}

#[test]
fn a_new_mark_discards_the_redo_branch() {
    let (mut session, _) = open("branch", 2);

    session.set_star(0, 3);
    session.undo();
    session.set_star(1, 1);
    assert!(session.redo().is_none(), "the undone star must not come back");
    assert_eq!(rating(&session, 0), 0);
}

#[test]
fn a_mark_that_changes_nothing_does_not_fill_the_undo_stack() {
    let (mut session, _) = open("noop", 2);

    session.set_star(0, 0);
    assert!(session.undo().is_none());
}

#[test]
fn writing_produces_sidecars_only_for_marked_photos() {
    let (mut session, dir) = open("write", 4);

    session.set_star(0, 4);
    session.toggle_label(0, "Green");
    session.toggle_reject(2);
    // Photos 1 and 3 stay untouched.

    let report = session.write_xmp(&[SidecarStyle::ReplaceExtension]);
    assert_eq!(report.error, None);
    assert_eq!(report.written, 2);
    assert_eq!(report.skipped, 2);
    assert_eq!(report.rejected, 1);

    let marked = std::fs::read_to_string(dir.join("IMG_4820.xmp")).unwrap();
    assert_eq!(
        zaru_xmp::read(&marked),
        Marks { rating: 4, label: Some("Green".into()) }
    );

    let rejected = std::fs::read_to_string(dir.join("IMG_4822.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&rejected).rating, REJECTED);

    assert!(!dir.join("IMG_4821.xmp").exists());
    // Rejection is a note in the metadata. No file is moved and none is deleted.
    assert!(dir.join("IMG_4822.CR3").exists());
}

#[test]
fn the_both_compat_setting_writes_both_names_with_the_same_content() {
    let (mut session, dir) = open("both", 2);
    session.set_star(0, 5);

    let report = session.write_xmp(XmpCompat::Both.styles());
    assert_eq!(report.written, 1);
    assert_eq!(report.error, None);

    let lightroom = std::fs::read_to_string(dir.join("IMG_4820.xmp")).unwrap();
    let darktable = std::fs::read_to_string(dir.join("IMG_4820.CR3.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&lightroom), zaru_xmp::read(&darktable));
    assert_eq!(zaru_xmp::read(&lightroom).rating, 5);
}

#[test]
fn writing_twice_updates_the_sidecar_instead_of_stacking_properties() {
    let (mut session, dir) = open("rewrite", 2);

    session.set_star(0, 2);
    session.write_xmp(&[SidecarStyle::ReplaceExtension]);
    session.set_star(0, 5);
    session.write_xmp(&[SidecarStyle::ReplaceExtension]);

    let xml = std::fs::read_to_string(dir.join("IMG_4820.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&xml).rating, 5);
    assert_eq!(xml.matches("<xmp:Rating>").count(), 1);
}

#[test]
fn writing_merges_into_a_sidecar_another_program_left_behind() {
    let (mut session, dir) = open("merge", 2);
    std::fs::write(
        dir.join("IMG_4820.xmp"),
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
   xmlns:darktable="http://darktable.sf.net/">
  <xmp:Rating>1</xmp:Rating>
  <darktable:history_end>7</darktable:history_end>
 </rdf:Description>
</rdf:RDF>"#,
    )
    .unwrap();

    session.set_star(0, 5);
    session.write_xmp(&[SidecarStyle::ReplaceExtension]);

    let xml = std::fs::read_to_string(dir.join("IMG_4820.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&xml).rating, 5);
    assert!(xml.contains("<darktable:history_end>7</darktable:history_end>"));
}

#[test]
fn compat_settings_survive_a_round_trip_and_a_corrupt_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("settings");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(Settings::load(&dir).xmp_compat, XmpCompat::Lightroom);

    let chosen = Settings { xmp_compat: XmpCompat::Darktable };
    chosen.save(&dir).unwrap();
    assert_eq!(Settings::load(&dir), chosen);

    // A settings file that cannot be parsed must never stop the app opening.
    std::fs::write(dir.join("settings.json"), b"{ not json").unwrap();
    assert_eq!(Settings::load(&dir).xmp_compat, XmpCompat::Lightroom);
}

#[test]
fn each_compat_choice_maps_to_the_names_it_promises() {
    let raw = Path::new("/photos/IMG_4821.CR3");
    let names = |compat: XmpCompat| -> Vec<PathBuf> {
        compat
            .styles()
            .iter()
            .map(|s| zaru_xmp::sidecar_path(raw, *s))
            .collect()
    };

    assert_eq!(names(XmpCompat::Lightroom), [Path::new("/photos/IMG_4821.xmp")]);
    assert_eq!(names(XmpCompat::Darktable), [Path::new("/photos/IMG_4821.CR3.xmp")]);
    assert_eq!(
        names(XmpCompat::Both),
        [
            Path::new("/photos/IMG_4821.xmp"),
            Path::new("/photos/IMG_4821.CR3.xmp")
        ]
    );
}
