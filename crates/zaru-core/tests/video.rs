//! A video in the same folder as the photos, judged the same way.

use std::path::{Path, PathBuf};
use std::process::Command;

use zaru_core::{Keymap, Kind, Session};
use zaru_xmp::SidecarStyle;

const STYLE: &[SidecarStyle] = &[SidecarStyle::ReplaceExtension];

/// Writes a folder with CR3 files and one MP4. Returns `None` when ffmpeg is
/// missing, so the suite skips rather than failing on a machine without it.
fn folder(name: &str, photos: &[&str], clip: &str) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("video-session").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3");
    for photo in photos {
        let target = dir.join(photo);
        if std::fs::hard_link(&fixture, &target).is_err() {
            std::fs::copy(&fixture, &target).ok()?;
        }
    }

    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=640x360:rate=30:duration=1"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .args(["-metadata", "creation_time=2026-08-20T07:00:00Z"])
        .arg(dir.join(clip))
        .status()
        .ok()?;
    status.success().then_some(dir)
}

fn open(dir: &Path) -> Session {
    let mut session = Session::default();
    session.open(dir).expect("open");
    session
}

#[test]
fn a_video_joins_the_pass_alongside_the_photos() {
    let Some(dir) = folder("listed", &["IMG_4820.CR3", "IMG_4821.CR3"], "MVI_9001.MP4") else {
        return;
    };
    let view = open(&dir).view();

    let names: Vec<&str> = view.photos.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["IMG_4820.CR3", "IMG_4821.CR3", "MVI_9001.MP4"]);

    let clip = &view.photos[2];
    assert_eq!(clip.kind, Kind::Video);
    assert_eq!(clip.duration_ms, Some(1000));
    assert_eq!((clip.width, clip.height), (640, 360));
    assert!(clip.captured.is_some(), "a hora de gravação foi lida");

    // And the photos are still photos.
    assert_eq!(view.photos[0].kind, Kind::Photo);
    assert_eq!(view.photos[0].duration_ms, None);
}

#[test]
fn a_clip_is_never_folded_into_a_burst_of_stills() {
    // The two photos share a shutter time, so they are one burst; the clip must
    // not become a third frame of it however close the clock puts it.
    let Some(dir) = folder("burst", &["IMG_4820.CR3", "IMG_4821.CR3"], "MVI_9001.MP4") else {
        return;
    };
    let view = open(&dir).view();

    assert_eq!(view.photos[0].burst, view.photos[1].burst, "as fotos são uma rajada");
    assert_ne!(view.photos[2].burst, view.photos[0].burst);
    assert_eq!(view.photos[2].burst_size, 1, "o vídeo é rajada de si mesmo");
}

#[test]
fn a_video_is_rated_and_moved_like_any_frame() {
    let Some(dir) = folder("apply", &["IMG_4820.CR3"], "MVI_9001.MP4") else {
        return;
    };
    let mut session = open(&dir);
    let porsche = session
        .new_collection("porsche", Keymap::default().collections.len())
        .unwrap();

    session.set_star(1, 4);
    session.toggle_label(1, "Green");
    session.assign(1, Some(porsche));

    let report = session.apply(STYLE);
    assert_eq!(report.error, None);
    assert_eq!(report.moved, 1);

    // The sidecar follows the same stem rule as a photo, and travels with it.
    let moved = dir.join("porsche/MVI_9001.MP4");
    assert!(moved.is_file(), "o vídeo foi movido");
    let sidecar = std::fs::read_to_string(dir.join("porsche/MVI_9001.xmp")).unwrap();
    assert_eq!(zaru_xmp::read(&sidecar).rating, 4);
    assert_eq!(zaru_xmp::read(&sidecar).label.as_deref(), Some("Green"));

    // The photo stayed where it was.
    assert!(dir.join("IMG_4820.CR3").is_file());
}

#[test]
fn a_video_has_no_still_to_serve_and_says_so() {
    let Some(dir) = folder("nopreview", &["IMG_4820.CR3"], "MVI_9001.MP4") else {
        return;
    };
    let session = open(&dir);

    // There is no embedded picture in an MP4, so nothing can be handed to the
    // viewer as a frame; the thumbnail has to come from a decoder instead.
    assert!(session.photos()[1].info.preview.is_none());
    assert!(session.photos()[1].info.thumbnail_source().is_none());
    // But the file itself is streamable.
    assert_eq!(session.media_path(1), Some(dir.join("MVI_9001.MP4")));
}
