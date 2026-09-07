//! The disk cache behind the grid.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use zaru_core::thumbs::{self, Source, Thumbs};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3")
}

fn workspace(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("thumbs").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fetch_blocking(pool: &Thumbs, index: usize) -> Option<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    pool.fetch(index, Box::new(move |bytes| {
        let _ = tx.send(bytes.map(|b| b.as_ref().clone()));
    }));
    rx.recv_timeout(Duration::from_secs(20)).expect("o responder nunca disparou")
}

/// The `PRVW` preview inside the fixture, which is what a thumbnail is built
/// from — no decoding happens to find it.
fn source_from_fixture() -> Source {
    let info = zaru_cr3::probe(fixture()).expect("probe");
    let preview = info.thumbnail.expect("o CR3 traz a box PRVW");
    Source {
        path: fixture(),
        offset: preview.offset,
        len: preview.len,
        key: Source::key_for(&fixture(), preview.offset, preview.len),
    }
}

#[test]
fn a_thumbnail_is_built_once_and_then_read_from_disk() {
    let dir = workspace("build");
    let pool = Thumbs::new(dir.clone());
    let source = source_from_fixture();
    let key = source.key.clone();
    pool.load(vec![source]);

    let built = fetch_blocking(&pool, 0).expect("miniatura");
    assert_eq!(&built[..3], &[0xFF, 0xD8, 0xFF], "é um JPEG");
    assert_eq!(&built[built.len() - 2..], &[0xFF, 0xD9]);

    // Far smaller than the 114 KB preview it came from: that shrinking is the
    // whole reason the cache exists.
    assert!(built.len() < 60_000, "saiu com {} bytes", built.len());

    let file = dir.join(format!("{key}.jpg"));
    assert!(file.is_file(), "ficou em disco");
    assert_eq!(fs::read(&file).unwrap(), built);

    // A second pool over the same folder never decodes: it finds the file.
    let again = Thumbs::new(dir.clone());
    again.load(vec![source_from_fixture()]);
    assert_eq!(fetch_blocking(&again, 0).expect("miniatura"), built);
}

#[test]
fn the_thumbnail_is_smaller_than_the_preview_it_came_from() {
    let dir = workspace("smaller");
    let pool = Thumbs::new(dir);
    let source = source_from_fixture();
    let preview_len = source.len as usize;
    pool.load(vec![source]);

    let built = fetch_blocking(&pool, 0).expect("miniatura");
    assert!(
        built.len() * 3 < preview_len,
        "{} bytes contra {preview_len} da prévia",
        built.len()
    );
}

#[test]
fn a_thumbnail_from_somewhere_else_is_stored_like_any_other() {
    // This is the video path: Rust has no video decoder, so the WebView draws
    // the frame and hands the bytes back.
    let dir = workspace("store");
    let pool = Thumbs::new(dir.clone());
    let source = source_from_fixture();
    let key = source.key.clone();
    pool.load(vec![source]);

    let given = b"\xFF\xD8\xFF nao e um jpeg de verdade \xFF\xD9".to_vec();
    pool.store(0, given.clone()).unwrap();

    assert_eq!(fetch_blocking(&pool, 0).as_ref(), Some(&given));
    assert_eq!(fs::read(dir.join(format!("{key}.jpg"))).unwrap(), given);
}

#[test]
fn an_index_past_the_end_answers_rather_than_hanging() {
    let pool = Thumbs::new(workspace("bounds"));
    pool.load(vec![source_from_fixture()]);
    assert_eq!(fetch_blocking(&pool, 99), None);
}

#[test]
fn the_key_follows_the_content_and_not_just_the_name() {
    let dir = workspace("key");
    let file = dir.join("IMG_0001.CR3");

    fs::write(&file, b"first").unwrap();
    let before = Source::key_for(&file, 0, 5);
    assert_eq!(before, Source::key_for(&file, 0, 5), "e é estável");

    // A different byte range of the same file is a different thumbnail.
    assert_ne!(before, Source::key_for(&file, 8, 5));

    // Rewritten, the old thumbnail must never be served again.
    fs::write(&file, b"second, longer").unwrap();
    assert_ne!(before, Source::key_for(&file, 0, 5));
}

#[test]
fn the_sweep_drops_the_oldest_until_it_fits() {
    let dir = workspace("sweep");
    for i in 0..5 {
        fs::write(dir.join(format!("{i}.jpg")), vec![0u8; 1000]).unwrap();
        // `std` cannot set an mtime and the sweep orders by it, so the files
        // are written apart in time to make "oldest" well defined.
        std::thread::sleep(Duration::from_millis(12));
    }

    assert_eq!(thumbs::sweep(&dir, 10_000), 5_000, "abaixo do teto, nada sai");
    assert_eq!(thumbs::sweep(&dir, 2_500), 2_000, "aparado até caber");

    let left: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.contains(&"4.jpg".to_string()), "o mais novo fica");
    assert!(!left.contains(&"0.jpg".to_string()), "o mais velho sai primeiro");
}

/// The point of the pool is that a whole shoot warms while you are still
/// looking at the first frame. This drives it hard enough to catch a stall or a
/// deadlock; it is a smoke test for the parallelism, not a benchmark.
#[test]
fn a_whole_folder_warms_without_stalling() {
    let dir = workspace("warm");
    let pool = Thumbs::new(dir);

    // The same preview under many keys, so every one is a real decode and none
    // can be answered from another's cache file.
    let base = source_from_fixture();
    let count = 120;
    let sources: Vec<Source> = (0..count)
        .map(|i| Source { key: format!("{}-{i}", base.key), ..base.clone() })
        .collect();
    pool.load(sources);

    let started = std::time::Instant::now();
    for index in (0..count).rev() {
        assert!(fetch_blocking(&pool, index).is_some(), "faltou a {index}");
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "levou {:?} para {count}",
        started.elapsed()
    );
    assert_eq!(pool.cached().len(), count);
}
