//! What a video is, read from its header alone.
//!
//! An MP4 is the same kind of container as a CR3, so the same box walker finds
//! both. Duration, size, rotation and the moment recording started all sit in
//! `moov`, which means Zaru learns everything it needs about a two-gigabyte
//! file by reading a few hundred bytes of it — and never decodes a frame.
//!
//! Drawing one is somebody else's job: the WebView has a video decoder and
//! Rust does not, so the picture comes from a `<video>` element and the bytes
//! come back here only to be cached.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use zaru_bmff::{self as bmff, be_u32, be_u64, read_at, BoxHeader};

pub use zaru_bmff::{Error, Result};

/// Seconds between the QuickTime epoch (1904-01-01) and the Unix one.
const EPOCH_OFFSET: i64 = 2_082_844_800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Clockwise rotation the player must apply, from the track matrix.
    pub rotation: u16,
    pub duration_ms: u64,
    /// When recording started, in milliseconds. `None` when the file carries
    /// no clock — some cameras and most transcoders leave it zero.
    pub captured_ms: Option<i64>,
}

/// Reads the header of an MP4 or MOV.
pub fn probe(path: impl AsRef<Path>) -> Result<VideoInfo> {
    let file = File::open(path.as_ref())?;
    let size = file.metadata()?.len();
    let mut r = BufReader::new(file);

    let top = bmff::children(&mut r, 0, size)?;
    let moov = top
        .iter()
        .find(|b| b.is(b"moov"))
        .ok_or(Error::Malformed("no moov box"))?;
    let inside = bmff::children(&mut r, moov.body, moov.end)?;

    let movie = inside
        .iter()
        .find(|b| b.is(b"mvhd"))
        .ok_or(Error::Malformed("no mvhd box"))?;
    let (duration_ms, captured_ms) = movie_header(&mut r, movie)?;
    let (width, height, rotation) = video_track(&mut r, &inside)?;

    Ok(VideoInfo { width, height, rotation, duration_ms, captured_ms })
}

/// `mvhd` holds how long the movie is and when it was made.
///
/// Version 0 keeps the times in 32 bits and version 1 in 64, which moves every
/// field after them; getting that wrong reads the duration out of the middle of
/// a timestamp.
fn movie_header<R: std::io::Read + std::io::Seek>(
    r: &mut R,
    movie: &BoxHeader,
) -> Result<(u64, Option<i64>)> {
    let p = read_at(r, movie.body, 32.min((movie.end - movie.body) as usize))?;
    let version = *p.first().ok_or(Error::Malformed("empty mvhd"))?;

    let (created, timescale, duration) = if version == 0 {
        (
            be_u32(&p, 4)? as u64,
            be_u32(&p, 12)? as u64,
            be_u32(&p, 16)? as u64,
        )
    } else {
        (be_u64(&p, 4)?, be_u32(&p, 20)? as u64, be_u64(&p, 24)?)
    };

    // A zero timescale is meaningless and would divide by zero; a file that
    // carries one simply has no duration to report.
    let duration_ms = duration.saturating_mul(1000).checked_div(timescale).unwrap_or(0);

    // Zero means "not recorded", and the QuickTime epoch is 1904, not 1970.
    let captured_ms = (created > 0).then(|| (created as i64 - EPOCH_OFFSET) * 1000);

    Ok((duration_ms, captured_ms))
}

/// Size and rotation from the first track that has a picture.
///
/// The video track is the one with a non-zero size in its `tkhd`: an audio
/// track carries the same box with width and height left at zero, so no
/// handler lookup is needed to tell them apart.
fn video_track<R: std::io::Read + std::io::Seek>(
    r: &mut R,
    moov: &[BoxHeader],
) -> Result<(u32, u32, u16)> {
    for trak in moov.iter().filter(|b| b.is(b"trak")) {
        let Some(header) = bmff::children(r, trak.body, trak.end)?
            .into_iter()
            .find(|b| b.is(b"tkhd"))
        else {
            continue;
        };

        let p = read_at(r, header.body, 92.min((header.end - header.body) as usize))?;
        let version = *p.first().ok_or(Error::Malformed("empty tkhd"))?;
        // Past the version, flags, the two times, the track id and a reserved
        // word: 32 bits each in version 0 and 64 for the times in version 1.
        let after_times = if version == 0 { 4 + 8 + 4 + 4 } else { 4 + 16 + 4 + 4 };
        // Then the track duration, another reserved pair, layer, alternate
        // group, volume and one more reserved word before the matrix.
        let matrix = after_times + if version == 0 { 4 } else { 8 } + 8 + 2 + 2 + 2 + 2;

        let width = be_u32(&p, matrix + 36)? >> 16;
        let height = be_u32(&p, matrix + 40)? >> 16;
        if width == 0 || height == 0 {
            continue;
        }

        // Only the top-left 2x2 of the display matrix decides the quarter turn,
        // and each entry is 16.16 fixed point.
        let cell = |i: usize| -> Result<i32> { Ok((be_u32(&p, matrix + i * 4)? as i32) >> 16) };
        let rotation = match (cell(0)?, cell(1)?, cell(3)?, cell(4)?) {
            (0, 1, -1, 0) => 90,
            (-1, 0, 0, -1) => 180,
            (0, -1, 1, 0) => 270,
            _ => 0,
        };

        // A quarter turn swaps what the viewer sees.
        return Ok(if rotation % 180 == 0 {
            (width, height, rotation)
        } else {
            (height, width, rotation)
        });
    }
    Err(Error::Malformed("no video track"))
}

/// True for the extensions Zaru treats as video.
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov"))
        .unwrap_or(false)
}

/// A duration written the way a clock shows it: `0:42`, or `1:02:03` past an
/// hour.
pub fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_reads_like_a_clock() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(42_000), "0:42");
        assert_eq!(format_duration(62_500), "1:02");
        assert_eq!(format_duration(3_723_000), "1:02:03");
    }

    #[test]
    fn only_mp4_and_mov_count_as_video() {
        assert!(is_video(Path::new("MVI_9001.MP4")));
        assert!(is_video(Path::new("clip.mov")));
        assert!(!is_video(Path::new("IMG_4820.CR3")));
        assert!(!is_video(Path::new("notes.txt")));
    }

    #[test]
    fn the_quicktime_epoch_is_1904_and_not_1970() {
        // Getting this wrong puts every video sixty-six years in the future,
        // which would file them all into one burst at the end of the pass.
        assert_eq!(EPOCH_OFFSET, 2_082_844_800);
    }
}
