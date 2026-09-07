//! The pool is what keeps a held-down navigation key from turning into one
//! disk round trip per frame.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use zaru_core::{Frame, Prefetch};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3")
}

/// Twenty frames that all point at the one real CR3, so the byte ranges are
/// real reads of a real file.
fn frames(count: usize) -> (Vec<Frame>, Vec<u8>) {
    let info = zaru_cr3::probe(fixture()).expect("probe");
    let bytes = zaru_cr3::read_preview(fixture(), &info).expect("read");
    let frames = (0..count)
        .map(|_| Frame {
            path: fixture(),
            offset: info.preview.offset,
            len: info.preview.len,
        })
        .collect();
    (frames, bytes)
}

fn fetch_blocking(pool: &Prefetch, index: usize) -> Option<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    pool.fetch(index, Box::new(move |bytes| {
        let _ = tx.send(bytes.map(|b| b.as_ref().clone()));
    }));
    rx.recv_timeout(Duration::from_secs(10)).expect("responder never fired")
}

fn wait_for(pool: &Prefetch, want: &[usize]) -> Vec<usize> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let cached = pool.cached();
        if want.iter().all(|i| cached.contains(i)) || Instant::now() > deadline {
            return cached;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_fetch_returns_the_same_bytes_the_extractor_would() {
    let (frames, expected) = frames(4);
    let pool = Prefetch::new();
    pool.load(frames);

    assert_eq!(fetch_blocking(&pool, 0).as_deref(), Some(&expected[..]));
    // Now served from memory rather than from the file.
    assert_eq!(fetch_blocking(&pool, 0).as_deref(), Some(&expected[..]));
    assert!(pool.cached().contains(&0));
}

#[test]
fn an_index_past_the_end_answers_rather_than_hanging() {
    let (frames, _) = frames(2);
    let pool = Prefetch::new();
    pool.load(frames);

    assert_eq!(fetch_blocking(&pool, 99), None);
}

#[test]
fn focusing_warms_the_window_and_drops_what_left_it() {
    let (frames, _) = frames(40);
    let pool = Prefetch::new();
    pool.load(frames);

    let window: Vec<usize> = vec![20, 21, 22, 23, 24, 25, 19, 18, 17];
    pool.focus(&window);
    let cached = wait_for(&pool, &window);
    for i in &window {
        assert!(cached.contains(i), "frame {i} should be warm, got {cached:?}");
    }

    // A window nowhere near the last one: the previous frames survive exactly
    // one more round, then go.
    let far: Vec<usize> = vec![35, 36, 37, 38, 39];
    pool.focus(&far);
    wait_for(&pool, &far);
    pool.focus(&far);
    let cached = pool.cached();
    assert!(
        cached.iter().all(|i| far.contains(i)),
        "frames outside two consecutive windows should be gone, got {cached:?}"
    );
}

#[test]
fn opening_another_folder_drops_the_previous_cache() {
    let (frames, _) = frames(10);
    let pool = Prefetch::new();
    pool.load(frames.clone());

    pool.focus(&[0, 1, 2]);
    wait_for(&pool, &[0, 1, 2]);
    assert!(!pool.cached().is_empty());

    pool.load(frames);
    assert!(
        pool.cached().is_empty(),
        "a new folder must not inherit the old folder's frames"
    );
}
