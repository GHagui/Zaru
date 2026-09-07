//! Burst grouping and crash recovery.
//!
//! The fixture is one frame, so the folders here are built by rewriting its
//! `DateTimeOriginal` in place — the string is fixed-width ASCII, so a copy with
//! a different second is still a valid CR3.

use std::path::{Path, PathBuf};

use zaru_core::{Recovery, Session};

const STAMP: &[u8] = b"2026:08:20 06:40:47";

fn fixture() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3")).unwrap()
}

/// Writes a folder of CR3 files, each stamped with the given second.
fn folder(name: &str, seconds: &[u8]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bursts").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let original = fixture();
    for (i, second) in seconds.iter().enumerate() {
        let mut bytes = original.clone();
        let stamp = format!("2026:08:20 06:40:{second:02}");
        replace_all(&mut bytes, STAMP, stamp.as_bytes());
        std::fs::write(dir.join(format!("IMG_{:04}.CR3", 4820 + i)), &bytes).unwrap();
    }
    dir
}

fn replace_all(haystack: &mut [u8], needle: &[u8], with: &[u8]) {
    assert_eq!(needle.len(), with.len());
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        if &haystack[at..at + needle.len()] == needle {
            haystack[at..at + needle.len()].copy_from_slice(with);
            at += needle.len();
        } else {
            at += 1;
        }
    }
}

fn open(name: &str, seconds: &[u8]) -> (Session, PathBuf) {
    let dir = folder(name, seconds);
    let mut session = Session::default();
    session.open(&dir).expect("open");
    (session, dir)
}

fn bursts(session: &Session) -> Vec<(usize, usize, usize)> {
    session
        .view()
        .photos
        .into_iter()
        .map(|p| (p.burst, p.burst_index, p.burst_size))
        .collect()
}

#[test]
fn frames_in_the_same_second_are_one_burst() {
    let (session, _) = open("one", &[47, 47, 47]);
    assert_eq!(bursts(&session), [(0, 0, 3), (0, 1, 3), (0, 2, 3)]);
}

#[test]
fn a_gap_longer_than_a_pressed_shutter_starts_a_new_burst() {
    // Three tries at one corner, then the next car two seconds later.
    let (session, _) = open("split", &[47, 47, 47, 49, 49]);
    assert_eq!(
        bursts(&session),
        [(0, 0, 3), (0, 1, 3), (0, 2, 3), (1, 0, 2), (1, 1, 2)]
    );
}

#[test]
fn every_frame_belongs_to_exactly_one_burst() {
    let (session, _) = open("cover", &[47, 49, 51, 51, 53]);
    let view = session.view();
    for (i, photo) in view.photos.iter().enumerate() {
        assert!(photo.burst_index < photo.burst_size, "frame {i} sits outside its burst");
    }
    let total: usize = view
        .photos
        .iter()
        .filter(|p| p.burst_index == 0)
        .map(|p| p.burst_size)
        .sum();
    assert_eq!(total, view.photos.len(), "the bursts have to add back up");
}

// -------------------------------------------------------------- recovery

#[test]
fn a_snapshot_comes_back_with_the_marks_and_the_collections() {
    let (mut session, _) = open("snapshot", &[47, 47, 49]);
    let porsche = session.new_collection("porsche").unwrap();
    session.set_star(0, 4);
    session.toggle_label(0, "Green");
    session.toggle_reject(2);
    session.assign(1, Some(porsche));

    let saved = session.snapshot();
    assert_eq!(saved.offer().marked, 2);
    assert_eq!(saved.offer().assigned, 1);
    assert_eq!(saved.offer().collections, 1);

    // A fresh session over the same folder takes it back.
    let mut reopened = Session::default();
    reopened.open(Path::new(&saved.folder)).unwrap();
    assert!(reopened.restore(saved));

    let view = reopened.view();
    assert_eq!(view.marks[0].rating, 4);
    assert_eq!(view.marks[0].label.as_deref(), Some("Green"));
    assert_eq!(view.marks[2].rating, zaru_xmp::REJECTED);
    assert_eq!(view.collections, ["porsche"]);
    assert_eq!(view.assigned, [None, Some(0), None]);

    // Undo history belongs to the session that ended, not to this one.
    assert!(reopened.undo().is_none());
}

#[test]
fn a_snapshot_from_a_different_folder_is_refused() {
    let (mut session, _) = open("mine", &[47, 47]);
    session.set_star(0, 5);
    let saved = session.snapshot();

    // Same marks, but the folder has three photos now.
    let (mut other, _) = open("theirs", &[47, 47, 47]);
    assert!(!other.restore(saved), "indices from another folder mean nothing");
    assert_eq!(other.view().marks[0].rating, 0);
}

#[test]
fn a_snapshot_survives_a_round_trip_through_the_disk() {
    let (mut session, _) = open("ondisk", &[47, 49]);
    let config = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bursts/config");
    let _ = std::fs::remove_dir_all(&config);

    assert!(!session.has_work());
    session.set_star(1, 3);
    assert!(session.has_work());

    session.snapshot().save(&config).unwrap();
    let loaded = Recovery::load(&config, session.folder()).expect("saved file comes back");
    assert_eq!(loaded.marks[1].rating, 3);

    Recovery::discard(&config, session.folder());
    assert!(Recovery::load(&config, session.folder()).is_none());
}
