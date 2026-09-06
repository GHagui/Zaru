//! Reads and writes `xmp:Rating` and `xmp:Label` in XMP sidecar files.
//!
//! Zaru owns exactly those two properties. A sidecar may already carry develop
//! settings, keywords or crop data written by Lightroom or darktable, so the
//! merge is deliberately surgical: it rewrites the two properties in place and
//! leaves every other byte of the document untouched.
//!
//! Both serialisations are handled, because the two programs disagree:
//! Lightroom writes properties as attributes of `rdf:Description`, darktable
//! writes them as child elements.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod edit;

pub const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";

/// `xmp:Rating` doubles as the rejection flag: `-1` is rejected, `0` is
/// unrated, `1..=5` are stars. Rejecting and rating therefore contend for one
/// field, which is why they are one value here and not two.
pub const REJECTED: i8 = -1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Marks {
    /// `-1` rejected, `0` unrated, `1..=5` stars.
    pub rating: i8,
    /// A colour name, or `None` for no label.
    pub label: Option<String>,
}

impl Marks {
    pub fn is_rejected(&self) -> bool {
        self.rating == REJECTED
    }

    /// True when nothing would be written that a reader could distinguish
    /// from an absent sidecar.
    pub fn is_empty(&self) -> bool {
        self.rating == 0 && self.label.is_none()
    }
}

/// Where the sidecar goes. The two programs disagree here too, so the choice
/// is explicit rather than guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum SidecarStyle {
    /// `IMG_4821.CR3` -> `IMG_4821.xmp`. Lightroom's convention.
    #[default]
    ReplaceExtension,
    /// `IMG_4821.CR3` -> `IMG_4821.CR3.xmp`. darktable's convention.
    AppendExtension,
}

pub fn sidecar_path(raw: &Path, style: SidecarStyle) -> PathBuf {
    match style {
        SidecarStyle::ReplaceExtension => raw.with_extension("xmp"),
        SidecarStyle::AppendExtension => {
            let mut name = raw.file_name().unwrap_or_default().to_os_string();
            name.push(".xmp");
            raw.with_file_name(name)
        }
    }
}

/// Reads the two properties out of an XMP document.
pub fn read(xml: &str) -> Marks {
    let pfx = edit::xmp_prefix(xml);
    let rating = edit::get_property(xml, &pfx, "Rating")
        .and_then(|v| v.trim().parse::<i8>().ok())
        .unwrap_or(0);
    let label = edit::get_property(xml, &pfx, "Label")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Marks { rating, label }
}

/// Rewrites the two properties, preserving everything else in `existing`.
/// With no existing document, emits a minimal well-formed packet.
pub fn merge(existing: Option<&str>, marks: &Marks) -> String {
    let Some(xml) = existing.filter(|x| x.contains("<rdf:Description")) else {
        return fresh(marks);
    };

    let pfx = edit::xmp_prefix(xml);
    let mut out = edit::set_property(xml, &pfx, "Rating", Some(&marks.rating.to_string()));
    out = edit::set_property(&out, &pfx, "Label", marks.label.as_deref());
    out
}

/// Reads the sidecar for `raw` if present, merges `marks` into it and writes
/// it back. Returns the path written.
pub fn apply(raw: &Path, marks: &Marks, style: SidecarStyle) -> io::Result<PathBuf> {
    let path = sidecar_path(raw, style);
    let existing = match fs::read_to_string(&path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    write_atomic(&path, &merge(existing.as_deref(), marks))?;
    Ok(path)
}

/// Writes through a temporary file in the same directory, then renames.
///
/// A half-written sidecar is worse than no sidecar: the photo's own metadata
/// would look corrupt to every other program. The rename makes the swap
/// all-or-nothing.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let tmp = dir.join(format!(".{stem}.zaru-tmp"));

    fs::write(&tmp, contents)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn fresh(marks: &Marks) -> String {
    let label = match &marks.label {
        Some(l) => format!("\n      <xmp:Label>{}</xmp:Label>", edit::escape(l)),
        None => String::new(),
    };
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:xmp="{XMP_NS}">
      <xmp:Rating>{}</xmp:Rating>{label}
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#,
        marks.rating
    )
}
