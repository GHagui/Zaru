//! Locates the camera-generated JPEG preview inside a Canon CR3 file.
//!
//! A CR3 is an ISO base media file. The full-resolution preview Canon writes
//! for its own playback is the single sample of the first video track: its
//! offset comes from `co64`/`stco` and its length from `stsz`, both inside
//! `moov/trak/mdia/minf/stbl`. Serving it costs a seek and a read — there is
//! no image decoding on the Rust side at all.
//!
//! Two details the container hides:
//!
//! * That JPEG carries no Exif segment, so orientation has to be read from the
//!   `CMT1` box, which is a bare TIFF IFD0.
//! * Some CR3 variants lay the tracks out differently, so the track scan is
//!   guarded by the JPEG magic and falls back to the smaller `PRVW` box.

mod bmff;
mod error;
mod tiff;

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use bmff::{be_u16, be_u32, be_u64, read_at, BoxHeader};
pub use error::{Error, Result};

/// The `uuid` box under `moov` that holds Canon's `CMT*` metadata records.
const UUID_CANON_META: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];

/// The top-level `uuid` box that holds the 1620x1080 `PRVW` JPEG.
const UUID_PREVIEW: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// Full-resolution JPEG, the single sample of the first track.
    Track,
    /// The smaller `PRVW` box, used when no track holds a JPEG.
    Prvw,
}

#[derive(Clone, Copy, Debug)]
pub struct Preview {
    pub offset: u64,
    pub len: u64,
    pub width: u32,
    pub height: u32,
    pub kind: PreviewKind,
}

#[derive(Clone, Copy, Debug)]
pub struct Cr3Info {
    pub preview: Preview,
    /// Exif orientation, 1 through 8. Falls back to 1 when absent.
    pub orientation: u16,
    pub sensor_width: u32,
    pub sensor_height: u32,
}

impl Cr3Info {
    /// Clockwise rotation the viewer must apply, and whether it is mirrored.
    pub fn rotation(&self) -> (u16, bool) {
        match self.orientation {
            2 => (0, true),
            3 => (180, false),
            4 => (180, true),
            5 => (90, true),
            6 => (90, false),
            7 => (270, true),
            8 => (270, false),
            _ => (0, false),
        }
    }
}

/// A file format Zaru can pull a ready-made preview out of.
///
/// Only CR3 is implemented. The trait exists so a second format slots in
/// without touching the prefetch ring or the custom protocol handler.
pub trait PreviewSource {
    fn probe(&self, path: &Path) -> Result<Cr3Info>;

    /// Reads the preview bytes. Cheap: one seek, one read, no decoding.
    fn read_preview(&self, path: &Path, info: &Cr3Info) -> Result<Vec<u8>> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(info.preview.offset))?;
        let mut buf = vec![0u8; info.preview.len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
}

pub struct Cr3;

impl PreviewSource for Cr3 {
    fn probe(&self, path: &Path) -> Result<Cr3Info> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        let mut r = BufReader::new(file);
        let top = bmff::children(&mut r, 0, size)?;

        let moov = top
            .iter()
            .find(|b| b.is(b"moov"))
            .ok_or(Error::Malformed("no moov box"))?;
        let inside_moov = bmff::children(&mut r, moov.body, moov.end)?;

        let tags = canon_tags(&mut r, &inside_moov)?;
        let orientation = match tiff::get(&tags, tiff::TAG_ORIENTATION) {
            Some(v) if (1..=8).contains(&v) => v as u16,
            _ => 1,
        };
        let sensor_width = tiff::get(&tags, tiff::TAG_IMAGE_WIDTH).unwrap_or(0);
        let sensor_height = tiff::get(&tags, tiff::TAG_IMAGE_HEIGHT).unwrap_or(0);

        let preview = track_preview(&mut r, &inside_moov)?
            .map(Ok)
            .unwrap_or_else(|| prvw_preview(&mut r, &top))?;

        Ok(Cr3Info { preview, orientation, sensor_width, sensor_height })
    }
}

/// Convenience wrapper over [`Cr3::probe`].
pub fn probe(path: impl AsRef<Path>) -> Result<Cr3Info> {
    Cr3.probe(path.as_ref())
}

/// Convenience wrapper over [`PreviewSource::read_preview`].
pub fn read_preview(path: impl AsRef<Path>, info: &Cr3Info) -> Result<Vec<u8>> {
    Cr3.read_preview(path.as_ref(), info)
}

/// Walks the tracks in order and returns the first whose sample is a JPEG.
fn track_preview<R: Read + Seek>(r: &mut R, moov: &[BoxHeader]) -> Result<Option<Preview>> {
    for trak in moov.iter().filter(|b| b.is(b"trak")) {
        let stbl = match bmff::find(r, trak.body, trak.end, b"stbl")? {
            Some(b) => b,
            None => continue,
        };
        let entries = bmff::children(r, stbl.body, stbl.end)?;

        let offset = match sample_offset(r, &entries)? {
            Some(o) => o,
            None => continue,
        };
        let len = match sample_len(r, &entries)? {
            Some(l) if l > 0 => l,
            _ => continue,
        };

        let mut magic = [0u8; 3];
        r.seek(SeekFrom::Start(offset))?;
        if r.read_exact(&mut magic).is_err() || magic != JPEG_MAGIC {
            continue;
        }

        let (width, height) = sample_dimensions(r, &entries)?.unwrap_or((0, 0));
        return Ok(Some(Preview { offset, len, width, height, kind: PreviewKind::Track }));
    }
    Ok(None)
}

/// `co64` holds 64-bit chunk offsets, `stco` the 32-bit form. Zaru only ever
/// wants the first chunk, because these preview tracks hold a single sample.
fn sample_offset<R: Read + Seek>(r: &mut R, stbl: &[BoxHeader]) -> Result<Option<u64>> {
    if let Some(b) = stbl.iter().find(|b| b.is(b"co64")) {
        let p = read_at(r, b.body, 16)?;
        if be_u32(&p, 4)? == 0 {
            return Ok(None);
        }
        return Ok(Some(be_u64(&p, 8)?));
    }
    if let Some(b) = stbl.iter().find(|b| b.is(b"stco")) {
        let p = read_at(r, b.body, 12)?;
        if be_u32(&p, 4)? == 0 {
            return Ok(None);
        }
        return Ok(Some(be_u32(&p, 8)? as u64));
    }
    Ok(None)
}

/// `stsz` gives one size for every sample, or zero followed by a size table.
fn sample_len<R: Read + Seek>(r: &mut R, stbl: &[BoxHeader]) -> Result<Option<u64>> {
    let b = match stbl.iter().find(|b| b.is(b"stsz")) {
        Some(b) => b,
        None => return Ok(None),
    };
    let p = read_at(r, b.body, 16)?;
    let uniform = be_u32(&p, 4)?;
    if uniform != 0 {
        return Ok(Some(uniform as u64));
    }
    if be_u32(&p, 8)? == 0 {
        return Ok(None);
    }
    Ok(Some(be_u32(&p, 12)? as u64))
}

/// Width and height of the first `stsd` entry. Canon writes `CRAW`, which is
/// laid out as a plain `VisualSampleEntry`, so the dimensions sit 32 bytes in.
fn sample_dimensions<R: Read + Seek>(r: &mut R, stbl: &[BoxHeader]) -> Result<Option<(u32, u32)>> {
    let b = match stbl.iter().find(|b| b.is(b"stsd")) {
        Some(b) => b,
        None => return Ok(None),
    };
    // 4 bytes version/flags, 4 bytes entry count, then the entry itself.
    let p = read_at(r, b.body + 8, 40)?;
    Ok(Some((be_u16(&p, 32)? as u32, be_u16(&p, 34)? as u32)))
}

/// Fallback path: the 1620x1080 JPEG in the top-level `PRVW` uuid box.
fn prvw_preview<R: Read + Seek>(r: &mut R, top: &[BoxHeader]) -> Result<Preview> {
    let uuid = top
        .iter()
        .find(|b| b.uuid == Some(UUID_PREVIEW))
        .ok_or(Error::NoPreview)?;

    // A short prefix of unknown purpose sits between the uuid and the box, so
    // find the fourcc rather than assuming its distance.
    let head = read_at(r, uuid.body, 64.min((uuid.end - uuid.body) as usize))?;
    let at = head
        .windows(4)
        .position(|w| w == b"PRVW")
        .ok_or(Error::NoPreview)?;

    // 4 bytes version/flags, 2 unknown, then width, height, 2 unknown, length.
    let body = uuid.body + at as u64 + 4;
    let p = read_at(r, body, 16)?;
    let width = be_u16(&p, 6)? as u32;
    let height = be_u16(&p, 8)? as u32;
    let len = be_u32(&p, 12)? as u64;

    let mut magic = [0u8; 3];
    r.seek(SeekFrom::Start(body + 16))?;
    r.read_exact(&mut magic)?;
    if magic != JPEG_MAGIC {
        return Err(Error::NoPreview);
    }

    Ok(Preview { offset: body + 16, len, width, height, kind: PreviewKind::Prvw })
}

fn canon_tags<R: Read + Seek>(r: &mut R, moov: &[BoxHeader]) -> Result<Vec<(u16, u32)>> {
    let uuid = match moov.iter().find(|b| b.uuid == Some(UUID_CANON_META)) {
        Some(b) => b,
        None => return Ok(Vec::new()),
    };
    let cmt1 = match bmff::children(r, uuid.body, uuid.end)?
        .into_iter()
        .find(|b| b.is(b"CMT1"))
    {
        Some(b) => b,
        None => return Ok(Vec::new()),
    };
    let payload = read_at(r, cmt1.body, (cmt1.end - cmt1.body) as usize)?;
    tiff::ifd0(&payload)
}

