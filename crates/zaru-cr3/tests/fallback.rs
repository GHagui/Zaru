//! The `PRVW` fallback never fires on the R50 fixture, so it gets a synthetic
//! container: a CR3-shaped file whose only track holds something that is not a
//! JPEG. If the magic guard were dropped, this test would hand the viewer a
//! block of raw sensor data.

use std::path::PathBuf;

use zaru_cr3::{probe, read_preview, PreviewKind};

const UUID_CANON_META: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
const UUID_PREVIEW: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn uuid_box(uuid: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let mut inner = uuid.to_vec();
    inner.extend_from_slice(body);
    bx(b"uuid", &inner)
}

/// A TIFF IFD0 carrying one SHORT tag.
fn cmt1(tag: u16, value: u16) -> Vec<u8> {
    let mut t = b"II\x2a\x00".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes()); // IFD0 starts right after
    t.extend_from_slice(&1u16.to_le_bytes()); // one entry
    t.extend_from_slice(&tag.to_le_bytes());
    t.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    t.extend_from_slice(&1u32.to_le_bytes()); // count
    t.extend_from_slice(&value.to_le_bytes());
    t.extend_from_slice(&[0, 0]); // pad the 4-byte value field
    t.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    bx(b"CMT1", &t)
}

fn visual_sample_entry(width: u16, height: u16) -> Vec<u8> {
    let mut e = vec![0u8; 78];
    e[..4].copy_from_slice(&78u32.to_be_bytes());
    e[4..8].copy_from_slice(b"CRAW");
    e[32..34].copy_from_slice(&width.to_be_bytes());
    e[34..36].copy_from_slice(&height.to_be_bytes());
    e
}

fn stbl(sample_offset: u64, sample_len: u32, width: u16, height: u16) -> Vec<u8> {
    let mut stsd = vec![0u8; 4];
    stsd.extend_from_slice(&1u32.to_be_bytes());
    stsd.extend_from_slice(&visual_sample_entry(width, height));

    let mut stsz = vec![0u8; 4];
    stsz.extend_from_slice(&0u32.to_be_bytes()); // per-sample sizes follow
    stsz.extend_from_slice(&1u32.to_be_bytes());
    stsz.extend_from_slice(&sample_len.to_be_bytes());

    let mut co64 = vec![0u8; 4];
    co64.extend_from_slice(&1u32.to_be_bytes());
    co64.extend_from_slice(&sample_offset.to_be_bytes());

    let mut body = bx(b"stsd", &stsd);
    body.extend(bx(b"stsz", &stsz));
    body.extend(bx(b"co64", &co64));
    bx(b"stbl", &body)
}

fn prvw_payload(width: u16, height: u16, jpeg: &[u8]) -> Vec<u8> {
    let mut p = 0u32.to_be_bytes().to_vec(); // version/flags
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&width.to_be_bytes());
    p.extend_from_slice(&height.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
    p.extend_from_slice(jpeg);
    p
}

const TINY_JPEG: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0xD9];

/// Builds a CR3-shaped container whose single track holds `sample`.
fn synthetic(orientation: u16, sample: &[u8]) -> Vec<u8> {
    let jpeg: Vec<u8> = TINY_JPEG.to_vec();
    let sensor_data = sample.to_vec();

    // The track's sample offset is absolute, so the layout is assembled first
    // with a placeholder and the offset patched once the sizes are known.
    let build = |sample_offset: u64| -> Vec<u8> {
        let mut trak = bx(b"tkhd", &[0u8; 8]);
        let minf = bx(b"minf", &stbl(sample_offset, sensor_data.len() as u32, 6000, 4000));
        trak.extend(bx(b"mdia", &minf));
        let trak = bx(b"trak", &trak);

        let mut moov = uuid_box(&UUID_CANON_META, &cmt1(0x0112, orientation));
        moov.extend(trak);
        let moov = bx(b"moov", &moov);

        let mut file = bx(b"ftyp", b"crx isom");
        file.extend(moov);
        // A second uuid the parser must skip over before reaching the preview.
        file.extend(uuid_box(&[0x11; 16], &[0u8; 8]));
        file.extend(uuid_box(
            &UUID_PREVIEW,
            &[&0u64.to_be_bytes()[..], &bx(b"PRVW", &prvw_payload(1620, 1080, &jpeg))].concat(),
        ));
        file.extend(bx(b"mdat", &sensor_data));
        file
    };

    // `mdat` ends the file, so its payload starts that many bytes from the end.
    let sample_offset = (build(0).len() - sensor_data.len()) as u64;
    build(sample_offset)
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("synthetic");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_non_jpeg_track_falls_back_to_the_prvw_box() {
    let path = write("fallback.CR3", &synthetic(1, &[0xAA; 64]));

    let info = probe(&path).expect("probe");
    assert_eq!(info.preview.kind, PreviewKind::Prvw);
    assert_eq!((info.preview.width, info.preview.height), (1620, 1080));

    let jpeg = read_preview(&path, &info).expect("read");
    assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF]);
    assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
    assert_eq!(jpeg.len() as u64, info.preview.len);
}

#[test]
fn portrait_orientation_becomes_a_rotation() {
    let path = write("portrait.CR3", &synthetic(6, &[0xAA; 64]));

    let info = probe(&path).expect("probe");
    assert_eq!(info.orientation, 6);
    assert_eq!(info.rotation(), (90, false));
}

/// Guards the guard: with a JPEG in the track, the same container must take
/// the track path. Without this, the fallback test would still pass if the
/// track were never found at all.
#[test]
fn the_same_container_prefers_the_track_when_it_holds_a_jpeg() {
    let path = write("track.CR3", &synthetic(1, &TINY_JPEG));

    let info = probe(&path).expect("probe");
    assert_eq!(info.preview.kind, PreviewKind::Track);
    assert_eq!((info.preview.width, info.preview.height), (6000, 4000));
    assert_eq!(read_preview(&path, &info).expect("read"), TINY_JPEG);
}
