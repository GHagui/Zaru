//! Every number here was read out of `example_cr3.CR3` (Canon EOS R50) by hand
//! before the parser existed, so the test pins the parser to the file, not to
//! its own output.

use std::path::PathBuf;

use zaru_cr3::{probe, read_preview, PreviewKind};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../example_cr3.CR3")
}

#[test]
fn finds_the_full_resolution_track_preview() {
    let info = probe(fixture()).expect("probe");
    assert_eq!(info.preview.kind, PreviewKind::Track);
    assert_eq!(info.preview.offset, 206_848);
    assert_eq!(info.preview.len, 1_150_804);
    assert_eq!((info.preview.width, info.preview.height), (6000, 4000));
}

#[test]
fn reads_orientation_from_cmt1_not_from_the_jpeg() {
    let info = probe(fixture()).expect("probe");
    assert_eq!(info.orientation, 1);
    assert_eq!(info.rotation(), (0, false));
    assert_eq!((info.sensor_width, info.sensor_height), (6000, 4000));

    let bytes = read_preview(fixture(), &info).expect("read");
    let exif = bytes.windows(6).any(|w| w == b"Exif\0\0");
    assert!(!exif, "the track JPEG is expected to carry no Exif segment");
}

#[test]
fn preview_bytes_are_a_complete_jpeg() {
    let info = probe(fixture()).expect("probe");
    let bytes = read_preview(fixture(), &info).expect("read");

    assert_eq!(bytes.len() as u64, info.preview.len);
    assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF]);
    assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9]);

    let (w, h) = sof_dimensions(&bytes).expect("SOF marker");
    assert_eq!((w, h), (info.preview.width, info.preview.height));
}

/// Walks JPEG segments to the start-of-frame and reads the real dimensions,
/// so a wrong `stsd` offset cannot pass unnoticed.
fn sof_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

#[test]
fn the_shutter_time_comes_from_cmt2_with_its_fraction() {
    let info = probe(fixture()).expect("probe");
    // exiftool reads this frame as 2026:08:20 06:40:47.08.
    let captured = info.captured_ms.expect("CMT2 carries DateTimeOriginal");
    assert_eq!(captured % 1000, 80, "the sub-second field is hundredths");
    assert_eq!(captured / 1000, 1_787_208_047);
}

#[test]
fn the_exposure_the_camera_recorded_comes_back_readable() {
    let info = probe(fixture()).expect("probe");
    let exif = &info.exif;

    assert_eq!(exif.camera.as_deref(), Some("Canon EOS R50"));
    // Written the way a photographer says it, not as 0.001.
    assert_eq!(exif.shutter.as_deref(), Some("1/1000"));
    assert_eq!(exif.iso, Some(800));
    assert_eq!((exif.width, exif.height), (Some(6000), Some(4000)));
}

#[test]
fn what_the_lens_never_reported_stays_absent() {
    // This frame was shot on a lens with no electronics, so the body wrote
    // 0/1 for aperture and focal length and left the name blank. Turning that
    // into "f/0" and "0mm" would be inventing data the camera never had.
    let exif = probe(fixture()).expect("probe").exif;
    assert_eq!(exif.aperture, None);
    assert_eq!(exif.focal_mm, None);
    assert_eq!(exif.lens, None);

    // But zero exposure compensation is a real answer, not a missing one, and
    // the same zero must not be swallowed here.
    assert_eq!(exif.exposure_bias, Some(0.0));
    assert!(!exif.is_empty(), "shutter and ISO are there");
}
