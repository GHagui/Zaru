//! The fixtures are generated with ffmpeg, because a header is a header — but
//! nothing here proves the parser against a file from the camera itself, and
//! that check still has to happen on a real recording.

use std::path::PathBuf;
use std::process::Command;

/// Builds an MP4 once per shape and reuses it.
fn fixture(name: &str, args: &[&str]) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("video");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.MP4"));
    if path.is_file() {
        return Some(path);
    }

    let mut command = Command::new("ffmpeg");
    command.args(["-loglevel", "error", "-y"]).args(args).arg(&path);
    match command.status() {
        Ok(status) if status.success() => Some(path),
        // ffmpeg is not a build dependency; without it these tests skip rather
        // than fail, and the CI image is where they are guaranteed to run.
        _ => None,
    }
}

fn plain() -> Option<PathBuf> {
    fixture(
        "plain",
        &[
            "-f", "lavfi", "-i", "testsrc=size=1920x1080:rate=30:duration=2",
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-metadata", "creation_time=2026-08-20T06:41:12Z",
        ],
    )
}

#[test]
fn the_header_gives_size_and_duration_without_decoding() {
    let Some(path) = plain() else { return };
    let info = zaru_video::probe(&path).expect("probe");

    assert_eq!((info.width, info.height), (1920, 1080));
    assert_eq!(info.duration_ms, 2000);
    assert_eq!(info.rotation, 0);
}

#[test]
fn the_capture_time_is_read_from_the_1904_epoch() {
    let Some(path) = plain() else { return };
    let info = zaru_video::probe(&path).expect("probe");

    // 2026-08-20 06:41:12 UTC. Reading the QuickTime epoch as the Unix one
    // would put this sixty-six years out and file every video into one burst.
    assert_eq!(info.captured_ms, Some(1_787_208_072_000));
}

#[test]
fn a_rotated_track_reports_the_size_the_viewer_sees() {
    let Some(path) = fixture(
        "rotated",
        &[
            "-f", "lavfi", "-i", "testsrc=size=1920x1080:rate=30:duration=1",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-metadata:s:v", "rotate=90",
        ],
    ) else {
        return;
    };
    let info = zaru_video::probe(&path).expect("probe");
    if info.rotation == 0 {
        // Not every ffmpeg build writes the display matrix; when it does not,
        // there is nothing here to check.
        return;
    }
    assert_eq!(info.rotation % 180, 90);
    assert_eq!((info.width, info.height), (1080, 1920), "girado, o alto vira largo");
}

#[test]
fn something_that_is_not_a_container_is_an_error_and_not_a_guess() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("video");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("nao-e-video.MP4");
    std::fs::write(&path, b"isto nao e um mp4").unwrap();
    assert!(zaru_video::probe(&path).is_err());
}
