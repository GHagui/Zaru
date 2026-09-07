//! One description for the two kinds of file a shoot leaves behind.
//!
//! A CR3 and an MP4 are the same kind of container, and Zaru learns about both
//! the same way: read the header, take the byte ranges, decode nothing. What
//! differs is where the picture comes from. A photo carries a JPEG the camera
//! already made; a video carries frames only a decoder can turn into a picture,
//! and Rust has no decoder — so the viewer draws it and hands a thumbnail back.

use std::path::Path;

use serde::Serialize;
use zaru_cr3::{Exif, Preview};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Photo,
    Video,
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub kind: Kind,
    /// The full-size preview, for photos. A video has none: its picture only
    /// exists once something decodes it.
    pub preview: Option<Preview>,
    /// The smaller preview the grid's cache is built from, when the file has
    /// one. For a video this is `None` and the thumbnail arrives from the
    /// viewer instead.
    pub thumbnail: Option<Preview>,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub mirrored: bool,
    pub captured_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub exif: Exif,
}

impl MediaInfo {
    /// Reads whatever the file is, by extension.
    pub fn probe(path: &Path) -> Result<MediaInfo, String> {
        if zaru_video::is_video(path) {
            let v = zaru_video::probe(path).map_err(|e| e.to_string())?;
            return Ok(MediaInfo {
                kind: Kind::Video,
                preview: None,
                thumbnail: None,
                width: v.width,
                height: v.height,
                rotation: v.rotation,
                mirrored: false,
                captured_ms: v.captured_ms,
                duration_ms: Some(v.duration_ms),
                exif: Exif::default(),
            });
        }

        let info = zaru_cr3::probe(path).map_err(|e| e.to_string())?;
        let (rotation, mirrored) = info.rotation();
        Ok(MediaInfo {
            kind: Kind::Photo,
            preview: Some(info.preview),
            thumbnail: info.thumbnail,
            width: info.preview.width,
            height: info.preview.height,
            rotation,
            mirrored,
            captured_ms: info.captured_ms,
            duration_ms: None,
            exif: info.exif,
        })
    }

    pub fn is_video(&self) -> bool {
        self.kind == Kind::Video
    }

    /// The byte range the grid's thumbnail is built from, if there is one.
    pub fn thumbnail_source(&self) -> Option<Preview> {
        self.thumbnail.or(self.preview)
    }
}

/// True for anything Zaru will put in a pass.
pub fn is_supported(path: &Path) -> bool {
    zaru_video::is_video(path)
        || path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("cr3"))
            .unwrap_or(false)
}

/// The slice of a file a `Range` request asks for, as an inclusive pair, or
/// `None` when the whole file should be sent.
///
/// A `<video>` element cannot seek without this: it needs a `206` and a
/// `Content-Range` back. Getting the clamping wrong is worse than not having
/// it — a range past the end makes the player stall on a response it cannot
/// use, and an unclamped one reads past the file.
pub fn range_slice(total: u64, header: Option<&str>) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    // Multi-range requests are legal and never come from a media element.
    // Answering only the first would silently corrupt the stream, so they fall
    // back to the whole file instead.
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;

    let last = total.checked_sub(1)?;
    let start: u64 = start.trim().parse().ok()?;
    if start > last {
        return None;
    }
    let end = match end.trim() {
        "" => last,
        value => value.parse::<u64>().ok()?.min(last),
    };
    (end >= start).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_holds_raw_photos_and_video_and_nothing_else() {
        assert!(is_supported(Path::new("IMG_4820.CR3")));
        assert!(is_supported(Path::new("img_4820.cr3")));
        assert!(is_supported(Path::new("MVI_9001.MP4")));
        assert!(is_supported(Path::new("clip.mov")));

        // The sidecar and the RAW+JPEG twin travel with a photo, but they are
        // not frames to judge on their own.
        assert!(!is_supported(Path::new("IMG_4820.xmp")));
        assert!(!is_supported(Path::new("IMG_4820.JPG")));
    }

    #[test]
    fn a_range_request_becomes_an_inclusive_slice() {
        assert_eq!(range_slice(1000, Some("bytes=0-99")), Some((0, 99)));
        // Open-ended is what a player sends first, and it means "to the end".
        assert_eq!(range_slice(1000, Some("bytes=500-")), Some((500, 999)));
        assert_eq!(range_slice(1000, Some("bytes=999-999")), Some((999, 999)));
    }

    #[test]
    fn a_range_past_the_end_is_clamped_and_never_read_past() {
        assert_eq!(range_slice(1000, Some("bytes=0-5000")), Some((0, 999)));
        // A start beyond the file has no slice at all; answering with the whole
        // file is better than answering with nothing the player can use.
        assert_eq!(range_slice(1000, Some("bytes=1000-")), None);
        assert_eq!(range_slice(0, Some("bytes=0-")), None, "arquivo vazio");
    }

    #[test]
    fn anything_that_is_not_a_single_byte_range_sends_the_whole_file() {
        assert_eq!(range_slice(1000, None), None);
        assert_eq!(range_slice(1000, Some("items=0-1")), None);
        assert_eq!(range_slice(1000, Some("bytes=0-9,20-29")), None, "múltiplas faixas");
        assert_eq!(range_slice(1000, Some("bytes=abc-")), None);
        assert_eq!(range_slice(1000, Some("bytes=900-100")), None, "fim antes do início");
    }
}
