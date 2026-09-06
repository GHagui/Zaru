//! The sidecars below are shaped like the real thing: darktable serialises the
//! properties as child elements, Lightroom as attributes on `rdf:Description`.
//! Zaru has to survive both without disturbing anything it does not own.

use std::path::Path;

use zaru_xmp::{apply, merge, read, sidecar_path, Marks, SidecarStyle, REJECTED};

const DARKTABLE: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:darktable="http://darktable.sf.net/">
   <xmp:Rating>1</xmp:Rating>
   <darktable:import_timestamp>-1</darktable:import_timestamp>
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:operation="flip" darktable:enabled="1"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#;

const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 5.6-c145">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:Rating="2"
   xmp:Label="Blue"
   xmp:CreatorTool="Adobe Photoshop Lightroom Classic 12.0"
   crs:Exposure2012="+0.35">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>interlagos</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

fn marks(rating: i8, label: Option<&str>) -> Marks {
    Marks { rating, label: label.map(String::from) }
}

#[test]
fn reads_both_serialisations() {
    assert_eq!(read(DARKTABLE), marks(1, None));
    assert_eq!(read(LIGHTROOM), marks(2, Some("Blue")));
}

#[test]
fn element_form_is_rewritten_in_place() {
    let out = merge(Some(DARKTABLE), &marks(4, Some("Green")));

    assert_eq!(read(&out), marks(4, Some("Green")));
    assert!(out.contains("<xmp:Rating>4</xmp:Rating>"));
    assert!(out.contains("<xmp:Label>Green</xmp:Label>"));
    // Everything darktable owns survives untouched.
    assert!(out.contains("<darktable:import_timestamp>-1</darktable:import_timestamp>"));
    assert!(out.contains(r#"<rdf:li darktable:operation="flip" darktable:enabled="1"/>"#));
    assert!(out.contains(r#"x:xmptk="XMP Core 4.4.0-Exiv2""#));
}

#[test]
fn attribute_form_is_rewritten_in_place() {
    let out = merge(Some(LIGHTROOM), &marks(5, Some("Green")));

    assert_eq!(read(&out), marks(5, Some("Green")));
    assert!(out.contains(r#"xmp:Rating="5""#));
    assert!(out.contains(r#"xmp:Label="Green""#));
    // Zaru writes two properties and only two.
    assert!(out.contains(r#"crs:Exposure2012="+0.35""#));
    assert!(out.contains(r#"xmp:CreatorTool="Adobe Photoshop Lightroom Classic 12.0""#));
    assert!(out.contains("<rdf:li>interlagos</rdf:li>"));
}

#[test]
fn clearing_the_label_removes_the_property() {
    let from_attribute = merge(Some(LIGHTROOM), &marks(3, None));
    assert!(!from_attribute.contains("xmp:Label"));
    assert_eq!(read(&from_attribute), marks(3, None));

    let with_label = merge(Some(DARKTABLE), &marks(3, Some("Green")));
    let cleared = merge(Some(&with_label), &marks(3, None));
    assert!(!cleared.contains("xmp:Label"));
    assert!(cleared.contains("<darktable:history>"));
}

#[test]
fn rejecting_writes_minus_one_and_a_star_undoes_it() {
    let rejected = merge(Some(DARKTABLE), &marks(REJECTED, None));
    assert_eq!(read(&rejected).rating, REJECTED);
    assert!(read(&rejected).is_rejected());

    // Rating and rejection share one field, so a star clears the rejection.
    let starred = merge(Some(&rejected), &marks(2, None));
    assert!(!read(&starred).is_rejected());
    assert_eq!(read(&starred).rating, 2);
}

#[test]
fn a_missing_sidecar_produces_a_readable_packet() {
    let out = merge(None, &marks(4, Some("Green")));
    assert_eq!(read(&out), marks(4, Some("Green")));
    assert!(out.starts_with("<?xpacket begin="));
    assert!(out.contains(r#"xmlns:xmp="http://ns.adobe.com/xap/1.0/""#));
    assert!(out.trim_end().ends_with(r#"<?xpacket end="w"?>"#));
}

#[test]
fn honours_the_xap_prefix_of_older_files() {
    let old = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:xap="http://ns.adobe.com/xap/1.0/">
  <xap:Rating>2</xap:Rating>
 </rdf:Description>
</rdf:RDF>"#;
    assert_eq!(read(old), marks(2, None));

    let out = merge(Some(old), &marks(5, Some("Green")));
    assert!(out.contains("<xap:Rating>5</xap:Rating>"));
    assert!(out.contains("<xap:Label>Green</xap:Label>"));
    assert_eq!(read(&out), marks(5, Some("Green")));
}

#[test]
fn a_self_closing_description_grows_a_body() {
    let empty = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"/>
</rdf:RDF>"#;
    let out = merge(Some(empty), &marks(3, None));
    assert_eq!(read(&out), marks(3, None));
    assert!(out.contains("</rdf:Description>"));
}

#[test]
fn a_longer_property_name_is_not_mistaken_for_ours() {
    let tricky = r#"<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
   xmp:RatingPercent="80">
  <xmp:LabelText>not ours</xmp:LabelText>
 </rdf:Description>"#;
    assert_eq!(read(tricky), marks(0, None));

    let out = merge(Some(tricky), &marks(4, Some("Green")));
    assert!(out.contains(r#"xmp:RatingPercent="80""#));
    assert!(out.contains("<xmp:LabelText>not ours</xmp:LabelText>"));
    assert_eq!(read(&out), marks(4, Some("Green")));
}

#[test]
fn markup_in_a_label_is_escaped_and_read_back() {
    let out = merge(None, &marks(1, Some("a & b <c>")));
    assert!(out.contains("a &amp; b &lt;c&gt;"));
    assert_eq!(read(&out).label.as_deref(), Some("a & b <c>"));
}

#[test]
fn sidecar_naming_follows_the_chosen_style() {
    let raw = Path::new("/photos/IMG_4821.CR3");
    assert_eq!(
        sidecar_path(raw, SidecarStyle::ReplaceExtension),
        Path::new("/photos/IMG_4821.xmp")
    );
    assert_eq!(
        sidecar_path(raw, SidecarStyle::AppendExtension),
        Path::new("/photos/IMG_4821.CR3.xmp")
    );
}

#[test]
fn apply_creates_then_merges_on_disk() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("apply");
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_4821.CR3");
    std::fs::write(&raw, b"not really a raw file").unwrap();
    let sidecar = sidecar_path(&raw, SidecarStyle::ReplaceExtension);
    let _ = std::fs::remove_file(&sidecar);

    let written = apply(&raw, &marks(4, Some("Green")), SidecarStyle::ReplaceExtension).unwrap();
    assert_eq!(written, sidecar);
    assert_eq!(read(&std::fs::read_to_string(&sidecar).unwrap()), marks(4, Some("Green")));

    apply(&raw, &marks(REJECTED, None), SidecarStyle::ReplaceExtension).unwrap();
    let after = std::fs::read_to_string(&sidecar).unwrap();
    assert_eq!(read(&after), marks(REJECTED, None));

    // No temporary file is left behind by a successful write.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("zaru-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}
