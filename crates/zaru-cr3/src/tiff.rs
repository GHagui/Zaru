//! Just enough TIFF to read IFD0 out of a CR3 `CMT1` box.
//!
//! The JPEG in trak #1 carries no `APP1`/Exif segment, so orientation has to
//! come from here. Without it every portrait frame shows up on its side.

use crate::error::{Error, Result};

pub const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub const TAG_IMAGE_HEIGHT: u16 = 0x0101;
pub const TAG_ORIENTATION: u16 = 0x0112;

#[derive(Clone, Copy)]
struct Endian(bool);

impl Endian {
    fn u16(self, b: &[u8], at: usize) -> Result<u16> {
        let s = b.get(at..at + 2).ok_or(Error::Malformed("truncated IFD"))?;
        Ok(if self.0 {
            u16::from_le_bytes([s[0], s[1]])
        } else {
            u16::from_be_bytes([s[0], s[1]])
        })
    }

    fn u32(self, b: &[u8], at: usize) -> Result<u32> {
        let s = b.get(at..at + 4).ok_or(Error::Malformed("truncated IFD"))?;
        Ok(if self.0 {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        })
    }
}

/// Reads the tags of IFD0. Only SHORT and LONG values that fit inline are
/// returned — every tag Zaru cares about is one of those.
pub fn ifd0(tiff: &[u8]) -> Result<Vec<(u16, u32)>> {
    let endian = match tiff.get(0..2) {
        Some(b"II") => Endian(true),
        Some(b"MM") => Endian(false),
        _ => return Err(Error::Malformed("CMT1 is not a TIFF header")),
    };

    let ifd = endian.u32(tiff, 4)? as usize;
    let count = endian.u16(tiff, ifd)? as usize;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let e = ifd + 2 + i * 12;
        let tag = endian.u16(tiff, e)?;
        let typ = endian.u16(tiff, e + 2)?;
        let value = match typ {
            3 => endian.u16(tiff, e + 8)? as u32, // SHORT
            4 => endian.u32(tiff, e + 8)?,        // LONG
            _ => continue,                        // offsets and strings: not needed
        };
        out.push((tag, value));
    }
    Ok(out)
}

pub fn get(tags: &[(u16, u32)], tag: u16) -> Option<u32> {
    tags.iter().find(|(t, _)| *t == tag).map(|(_, v)| *v)
}
