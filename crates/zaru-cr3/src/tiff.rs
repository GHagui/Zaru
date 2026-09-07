//! Just enough TIFF to read the IFDs Canon stores in a CR3's `CMT*` boxes.
//!
//! `CMT1` is IFD0 and `CMT2` is the Exif IFD. Between them they carry the two
//! things Zaru needs and the JPEG does not have: which way up the frame is, and
//! when the shutter fired.

use crate::error::{Error, Result};

pub const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub const TAG_IMAGE_HEIGHT: u16 = 0x0101;
pub const TAG_ORIENTATION: u16 = 0x0112;
pub const TAG_MAKE: u16 = 0x010f;
pub const TAG_MODEL: u16 = 0x0110;
pub const TAG_EXPOSURE_TIME: u16 = 0x829a;
pub const TAG_F_NUMBER: u16 = 0x829d;
pub const TAG_ISO: u16 = 0x8827;
pub const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
pub const TAG_EXPOSURE_BIAS: u16 = 0x9204;
pub const TAG_FOCAL_LENGTH: u16 = 0x920a;
pub const TAG_SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;
pub const TAG_PIXEL_X: u16 = 0xa002;
pub const TAG_PIXEL_Y: u16 = 0xa003;
pub const TAG_LENS_MODEL: u16 = 0xa434;

/// Exif writes "not recorded" as this in a SRATIONAL, and cameras reach for it
/// whenever the lens cannot tell them the answer.
const UNKNOWN_RATIONAL: i64 = 0x8000_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Uint(u32),
    Text(String),
    /// A RATIONAL or SRATIONAL, kept as the pair the file holds rather than a
    /// float: `1/1000` is the shutter speed a photographer reads, and dividing
    /// it out would throw that away.
    Ratio(i64, i64),
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
/// Only the shapes Zaru asks for are decoded: SHORT and LONG, which fit in the
/// entry itself, and ASCII and the rationals, which do not and are followed to
/// their offset. Everything else is skipped rather than half-understood.
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
            // RATIONAL and SRATIONAL are two 32-bit words, so they never fit
            // in the entry and always live at an offset.
            5 | 10 => {
                let at = endian.u32(tiff, e + 8)? as usize;
                let (Ok(num), Ok(den)) = (endian.u32(tiff, at), endian.u32(tiff, at + 4)) else {
                    continue;
                };
                let (num, den) = if kind == 10 {
                    (num as i32 as i64, den as i32 as i64)
                } else {
                    (num as i64, den as i64)
                };
                Value::Ratio(num, den)
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

/// An ASCII tag, or `None` when the camera wrote it empty.
///
/// A blank `LensModel` is what an adapted manual lens produces, and showing an
/// empty row is worse than showing no row.
pub fn text(tags: &[(u16, Value)], tag: u16) -> Option<&str> {
    tags.iter().find_map(|(t, v)| match v {
        Value::Text(s) if *t == tag && !s.trim().is_empty() => Some(s.as_str()),
        _ => None,
    })
}

/// A rational tag as its numerator and denominator, or `None` when the value is
/// unusable: a zero denominator, or the `0x80000000` the camera writes when it
/// has nothing to record.
///
/// Zero in the numerator is *not* filtered here. For aperture and focal length
/// it does mean "unknown", but for exposure compensation zero is the answer,
/// and one accessor cannot be right about both — see [`positive_f32`].
pub fn ratio(tags: &[(u16, Value)], tag: u16) -> Option<(i64, i64)> {
    tags.iter().find_map(|(t, v)| match v {
        Value::Ratio(num, den) if *t == tag && *den != 0 && *num != UNKNOWN_RATIONAL => {
            Some((*num, *den))
        }
        _ => None,
    })
}

/// A rational read as a number, keeping zero as a value.
pub fn ratio_f32(tags: &[(u16, Value)], tag: u16) -> Option<f32> {
    ratio(tags, tag).map(|(num, den)| num as f32 / den as f32)
}

/// A rational that only means something above zero.
///
/// An adapted manual lens reports `0/1` for aperture and focal length because
/// it has no electronics to say otherwise. That is absence written as a number,
/// and it has to come back as absence.
pub fn positive_f32(tags: &[(u16, Value)], tag: u16) -> Option<f32> {
    ratio_f32(tags, tag).filter(|v| *v > 0.0)
}

/// A shutter speed the way a photographer says it: `1/1000`, or `2.5"` once it
/// is long enough that the fraction stops being the useful form.
pub fn shutter(tags: &[(u16, Value)], tag: u16) -> Option<String> {
    let (num, den) = ratio(tags, tag).filter(|(num, _)| *num > 0)?;
    let seconds = num as f64 / den as f64;
    Some(if seconds >= 1.0 {
        format!("{seconds:.1}\"")
    } else {
        format!("1/{}", (1.0 / seconds).round() as i64)
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

    fn tags(entries: &[(u16, Value)]) -> Vec<(u16, Value)> {
        entries.to_vec()
    }

    #[test]
    fn a_zero_denominator_and_the_unknown_sentinel_are_both_absence() {
        let t = tags(&[
            (TAG_F_NUMBER, Value::Ratio(28, 10)),
            (TAG_FOCAL_LENGTH, Value::Ratio(200, 0)),
            (TAG_EXPOSURE_BIAS, Value::Ratio(UNKNOWN_RATIONAL, 1)),
        ]);
        assert_eq!(positive_f32(&t, TAG_F_NUMBER), Some(2.8));
        assert_eq!(positive_f32(&t, TAG_FOCAL_LENGTH), None, "0 denominator");
        assert_eq!(ratio_f32(&t, TAG_EXPOSURE_BIAS), None, "the unknown sentinel");
    }

    #[test]
    fn zero_means_absence_for_a_lens_and_a_value_for_compensation() {
        // The same 0/1 has to read two different ways, which is why there are
        // two accessors and not one.
        let t = tags(&[
            (TAG_F_NUMBER, Value::Ratio(0, 1)),
            (TAG_EXPOSURE_BIAS, Value::Ratio(0, 1)),
        ]);
        assert_eq!(positive_f32(&t, TAG_F_NUMBER), None);
        assert_eq!(ratio_f32(&t, TAG_EXPOSURE_BIAS), Some(0.0));
    }

    #[test]
    fn a_shutter_speed_is_written_the_way_it_is_spoken() {
        let fast = tags(&[(TAG_EXPOSURE_TIME, Value::Ratio(1, 1000))]);
        assert_eq!(shutter(&fast, TAG_EXPOSURE_TIME).as_deref(), Some("1/1000"));

        // Canon writes 1/60 as 10/600; the fraction has to be reduced, not echoed.
        let odd = tags(&[(TAG_EXPOSURE_TIME, Value::Ratio(10, 600))]);
        assert_eq!(shutter(&odd, TAG_EXPOSURE_TIME).as_deref(), Some("1/60"));

        // Past a second the fraction stops being the useful form.
        let long = tags(&[(TAG_EXPOSURE_TIME, Value::Ratio(5, 2))]);
        assert_eq!(shutter(&long, TAG_EXPOSURE_TIME).as_deref(), Some("2.5\""));

        let none = tags(&[(TAG_EXPOSURE_TIME, Value::Ratio(0, 1))]);
        assert_eq!(shutter(&none, TAG_EXPOSURE_TIME), None);
    }

    #[test]
    fn a_blank_string_is_not_a_value() {
        // An adapted lens leaves LensModel empty rather than absent.
        let t = tags(&[(TAG_LENS_MODEL, Value::Text("   ".into()))]);
        assert_eq!(text(&t, TAG_LENS_MODEL), None);
    }

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
