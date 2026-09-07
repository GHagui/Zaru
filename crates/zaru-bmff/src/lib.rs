//! Minimal ISO base media file format reader.
//!
//! Walks a box list, recurses into the containers we know, and hands back byte
//! ranges. Nothing is buffered beyond a box header.
//!
//! A CR3 and an MP4 are the same kind of file, which is why this is its own
//! crate: the container list below is exactly what both use, so the reader that
//! finds a Canon preview also finds a video's duration.

use std::fmt;
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The file is not an ISO-BMFF container, or a box header is malformed.
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Malformed(what) => write!(f, "container malformado: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Boxes whose payload is another box list. Everything else is opaque —
/// recursing into a leaf like `CMT1` yields garbage, so the set stays explicit.
const CONTAINERS: &[&[u8; 4]] = &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"dinf"];

#[derive(Clone, Copy, Debug)]
pub struct BoxHeader {
    pub kind: [u8; 4],
    /// Offset of the box itself, from the start of the file. Kept for error
    /// reporting and for tests that pin a box to a known position.
    #[allow(dead_code)]
    pub offset: u64,
    /// Offset of the first payload byte (past the size/type, past a `uuid`).
    pub body: u64,
    /// Offset one past the last byte of the box.
    pub end: u64,
    /// Present only for `uuid` boxes.
    pub uuid: Option<[u8; 16]>,
}

impl BoxHeader {
    pub fn is(&self, kind: &[u8; 4]) -> bool {
        &self.kind == kind
    }

    pub fn is_container(&self) -> bool {
        CONTAINERS.contains(&&self.kind)
    }
}

/// Reads the box headers between `start` and `end`, without descending.
pub fn children<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> Result<Vec<BoxHeader>> {
    let mut out = Vec::new();
    let mut at = start;
    while at + 8 <= end {
        r.seek(SeekFrom::Start(at))?;
        let mut hdr = [0u8; 8];
        r.read_exact(&mut hdr)?;
        let mut size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let mut body = at + 8;

        match size {
            // 64-bit size follows the header.
            1 => {
                let mut ext = [0u8; 8];
                r.read_exact(&mut ext)?;
                size = u64::from_be_bytes(ext);
                body += 8;
            }
            // Runs to the end of the enclosing box.
            0 => size = end - at,
            _ => {}
        }

        if size < body - at || at + size > end {
            return Err(Error::Malformed("box size runs past its parent"));
        }

        let uuid = if kind == *b"uuid" {
            let mut u = [0u8; 16];
            r.read_exact(&mut u)?;
            body += 16;
            Some(u)
        } else {
            None
        };

        out.push(BoxHeader { kind, offset: at, body, end: at + size, uuid });
        at += size;
    }
    Ok(out)
}

/// Depth-first search for the first box of `kind` reachable through containers.
pub fn find<R: Read + Seek>(
    r: &mut R,
    start: u64,
    end: u64,
    kind: &[u8; 4],
) -> Result<Option<BoxHeader>> {
    for b in children(r, start, end)? {
        if b.is(kind) {
            return Ok(Some(b));
        }
        if b.is_container() {
            if let Some(hit) = find(r, b.body, b.end, kind)? {
                return Ok(Some(hit));
            }
        }
    }
    Ok(None)
}

pub fn read_at<R: Read + Seek>(r: &mut R, offset: u64, len: usize) -> Result<Vec<u8>> {
    r.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn be_u32(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(Error::Malformed("truncated box payload"))
}

pub fn be_u64(b: &[u8], at: usize) -> Result<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or(Error::Malformed("truncated box payload"))
}

pub fn be_u16(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or(Error::Malformed("truncated box payload"))
}
