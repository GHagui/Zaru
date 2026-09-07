//! Just enough TIFF to read the IFDs Canon stores in a CR3's `CMT*` boxes.
//!
//! `CMT1` is IFD0 and `CMT2` is the Exif IFD. Between them they carry the two
//! things Zaru needs and the JPEG does not have: which way up the frame is, and
//! when the shutter fired.

use crate::error::{Error, Result};

pub const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub const TAG_IMAGE_HEIGHT: u16 = 0x0101;
pub const TAG_ORIENTATION: u16 = 0x0112;
pub const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
pub const TAG_SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Uint(u32),
    Text(String),
}

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

/// Reads the first IFD of a TIFF block.
///
/// Only the shapes Zaru asks for are decoded: SHORT and LONG, which always fit
/// in the entry itself, and ASCII, which usually does not and is then followed
/// to its offset. Everything else is skipped rather than half-understood.
pub fn ifd0(tiff: &[u8]) -> Result<Vec<(u16, Value)>> {
    let endian = match tiff.get(0..2) {
        Some(b"II") => Endian(true),
        Some(b"MM") => Endian(false),
        _ => return Err(Error::Malformed("CMT box is not a TIFF header")),
    };

    let ifd = endian.u32(tiff, 4)? as usize;
    let count = endian.u16(tiff, ifd)? as usize;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let e = ifd + 2 + i * 12;
        let tag = endian.u16(tiff, e)?;
        let kind = endian.u16(tiff, e + 2)?;
        let length = endian.u32(tiff, e + 4)? as usize;

        let value = match kind {
            3 => Value::Uint(endian.u16(tiff, e + 8)? as u32),
            4 => Value::Uint(endian.u32(tiff, e + 8)?),
            2 => {
                // Up to four bytes live in the entry; anything longer is at an
                // offset from the start of the TIFF block.
                let bytes = if length <= 4 {
                    tiff.get(e + 8..e + 8 + length)
                } else {
                    let at = endian.u32(tiff, e + 8)? as usize;
                    tiff.get(at..at + length)
                };
                let Some(bytes) = bytes else { continue };
                let text: String = bytes
                    .iter()
                    .take_while(|b| **b != 0)
                    .map(|b| *b as char)
                    .collect();
                Value::Text(text)
            }
            _ => continue,
        };
        out.push((tag, value));
    }
    Ok(out)
}

pub fn uint(tags: &[(u16, Value)], tag: u16) -> Option<u32> {
    tags.iter().find_map(|(t, v)| match v {
        Value::Uint(n) if *t == tag => Some(*n),
        _ => None,
    })
}

pub fn text(tags: &[(u16, Value)], tag: u16) -> Option<&str> {
    tags.iter().find_map(|(t, v)| match v {
        Value::Text(s) if *t == tag => Some(s.as_str()),
        _ => None,
    })
}

/// Milliseconds since the Unix epoch for `YYYY:MM:DD HH:MM:SS` plus an
/// optional fractional-seconds field.
///
/// Exif records no time zone, but every frame in a session comes off one camera
/// with one clock, so the differences — which is all burst detection looks at —
/// are right regardless of what the absolute value means.
pub fn timestamp_ms(date_time: &str, sub_sec: Option<&str>) -> Option<i64> {
    let bytes = date_time.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        date_time.get(range)?.trim().parse::<i64>().ok()
    };

    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;

    // `SubSecTimeOriginal` is a fraction written without its leading dot, so
    // "08" is eight hundredths and has to be padded, not parsed as eight.
    let millis = sub_sec
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| {
            let padded: String = s.chars().chain("000".chars()).take(3).collect();
            padded.parse::<i64>().ok()
        })
        .unwrap_or(0);

    Some(seconds * 1000 + millis)
}

/// Days from 1970-01-01 to the given civil date, by Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shifted = (month + 9) % 12;
    let day_of_year = (153 * month_shifted + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_a_few_dates_around_it_line_up() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // A leap day, which is where a naive formula goes wrong.
        assert_eq!(days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28), 2);
        assert_eq!(days_from_civil(2026, 8, 20), 20_685);
    }

    #[test]
    fn a_fractional_second_is_a_fraction_not_a_count() {
        let base = timestamp_ms("2026:08:20 06:40:47", None).unwrap();
        // "08" is eight hundredths of a second, so 80 ms — not 8.
        assert_eq!(timestamp_ms("2026:08:20 06:40:47", Some("08")).unwrap() - base, 80);
        assert_eq!(timestamp_ms("2026:08:20 06:40:47", Some("5")).unwrap() - base, 500);
        assert_eq!(timestamp_ms("2026:08:20 06:40:47", Some("123")).unwrap() - base, 123);
        assert_eq!(timestamp_ms("2026:08:20 06:40:47", Some("")).unwrap() - base, 0);
        assert_eq!(timestamp_ms("2026:08:20 06:40:47", Some("x")).unwrap() - base, 0);
    }

    #[test]
    fn one_second_apart_is_one_second_apart() {
        let a = timestamp_ms("2026:08:20 06:40:47", Some("90")).unwrap();
        let b = timestamp_ms("2026:08:20 06:40:48", Some("00")).unwrap();
        assert_eq!(b - a, 100);
    }

    #[test]
    fn a_malformed_stamp_is_none_rather_than_a_wrong_number() {
        assert_eq!(timestamp_ms("", None), None);
        assert_eq!(timestamp_ms("2026:08:20", None), None);
        assert_eq!(timestamp_ms("nao e uma data ok", None), None);
        assert_eq!(timestamp_ms("2026:13:20 06:40:47", None), None);
    }
}
