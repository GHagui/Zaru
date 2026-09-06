//! Targeted edits to an XMP document, done on the text.
//!
//! A round trip through a DOM would reorder attributes, rewrite namespace
//! prefixes and normalise whitespace — a diff full of noise for a two-property
//! change, and a good way to lose something a program stored in a shape we did
//! not model. Editing the text in place keeps the rest of the file byte-exact.

use crate::XMP_NS;

/// The prefix bound to the XMP namespace in this document, `xmp` by default.
/// Older Adobe files bind it as `xap`.
pub fn xmp_prefix(xml: &str) -> String {
    let mut at = 0;
    while let Some(hit) = xml[at..].find(XMP_NS) {
        let ns = at + hit;
        // Step back over the opening quote and `="`.
        let head = &xml[..ns];
        if let Some(eq) = head.rfind("xmlns:") {
            let decl = &head[eq + "xmlns:".len()..];
            let name: String = decl.chars().take_while(|c| c.is_alphanumeric()).collect();
            let rest = &decl[name.len()..];
            if !name.is_empty() && rest.trim_start().starts_with('=') {
                return name;
            }
        }
        at = ns + XMP_NS.len();
    }
    "xmp".to_string()
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn get_property(xml: &str, pfx: &str, name: &str) -> Option<String> {
    if let Some(span) = attribute_value(xml, pfx, name) {
        return Some(unescape(&xml[span]));
    }
    element_value(xml, pfx, name).map(|span| unescape(&xml[span]))
}

/// Sets the property to `value`, or removes it when `value` is `None`.
pub fn set_property(xml: &str, pfx: &str, name: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => set(xml, pfx, name, v),
        None => remove(xml, pfx, name),
    }
}

fn set(xml: &str, pfx: &str, name: &str, value: &str) -> String {
    if let Some(span) = attribute_value(xml, pfx, name) {
        return splice(xml, span, &escape(value));
    }
    if let Some(span) = element_value(xml, pfx, name) {
        return splice(xml, span, &escape(value));
    }
    insert_element(xml, pfx, name, value)
}

fn remove(xml: &str, pfx: &str, name: &str) -> String {
    if let Some(span) = attribute_span(xml, pfx, name) {
        return splice(xml, span, "");
    }
    if let Some(span) = element_span(xml, pfx, name) {
        return splice(xml, span, "");
    }
    xml.to_string()
}

fn splice(xml: &str, span: std::ops::Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(xml.len() + with.len());
    out.push_str(&xml[..span.start]);
    out.push_str(with);
    out.push_str(&xml[span.end..]);
    out
}

/// Range of the value inside `pfx:name="..."`, quotes excluded.
fn attribute_value(xml: &str, pfx: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let (_, value) = find_attribute(xml, pfx, name)?;
    Some(value)
}

/// Range covering the whole attribute plus the whitespace in front of it, so
/// removing it does not leave a double space.
fn attribute_span(xml: &str, pfx: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let (whole, _) = find_attribute(xml, pfx, name)?;
    let mut start = whole.start;
    while start > 0 && xml.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    Some(start..whole.end)
}

fn find_attribute(
    xml: &str,
    pfx: &str,
    name: &str,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let needle = format!("{pfx}:{name}=");
    let mut at = 0;
    while let Some(hit) = xml[at..].find(&needle) {
        let start = at + hit;
        at = start + needle.len();
        // An attribute name is always preceded by whitespace inside a tag;
        // this also rules out matching `Rating` inside `MyRating`.
        let preceded_ok = start > 0 && xml.as_bytes()[start - 1].is_ascii_whitespace();
        let quote = xml.as_bytes().get(at).copied();
        if !preceded_ok || !matches!(quote, Some(b'"') | Some(b'\'')) {
            continue;
        }
        let q = quote.unwrap() as char;
        let value_start = at + 1;
        let value_end = value_start + xml[value_start..].find(q)?;
        return Some((start..value_end + 1, value_start..value_end));
    }
    None
}

/// Range of the text between `<pfx:name>` and `</pfx:name>`.
fn element_value(xml: &str, pfx: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let (open_end, close_start, _) = find_element(xml, pfx, name)?;
    Some(open_end..close_start)
}

/// Range covering the whole element plus the whitespace in front of it.
fn element_span(xml: &str, pfx: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let (_, _, whole) = find_element(xml, pfx, name)?;
    let mut start = whole.start;
    while start > 0 && xml.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    Some(start..whole.end)
}

/// Returns (end of open tag, start of close tag, whole element range).
/// A self-closing element reports an empty value range.
fn find_element(
    xml: &str,
    pfx: &str,
    name: &str,
) -> Option<(usize, usize, std::ops::Range<usize>)> {
    let needle = format!("<{pfx}:{name}");
    let mut at = 0;
    while let Some(hit) = xml[at..].find(&needle) {
        let start = at + hit;
        at = start + needle.len();
        // Guard against `<xmp:RatingPercent`.
        match xml.as_bytes().get(at) {
            Some(c) if c.is_ascii_whitespace() || *c == b'>' || *c == b'/' => {}
            _ => continue,
        }
        let open_end = start + xml[start..].find('>')? + 1;
        if xml.as_bytes()[open_end - 2] == b'/' {
            return Some((open_end, open_end, start..open_end));
        }
        let close = format!("</{pfx}:{name}>");
        let close_start = open_end + xml[open_end..].find(&close)?;
        return Some((open_end, close_start, start..close_start + close.len()));
    }
    None
}

/// Adds the property as a child element of the `rdf:Description` that declares
/// the XMP namespace, declaring the namespace first if no one has.
fn insert_element(xml: &str, pfx: &str, name: &str, value: &str) -> String {
    let Some(tag) = description_tag(xml) else {
        return xml.to_string();
    };

    let indent = line_indent(xml, tag.start);
    let child = format!("\n{indent}  <{pfx}:{name}>{}</{pfx}:{name}>", escape(value));

    if tag.self_closing {
        // `<rdf:Description .../>` has to grow a body first.
        let head = &xml[tag.start..tag.open_end - 2];
        let body = format!("{head}>{child}\n{indent}</rdf:Description>");
        return splice(xml, tag.start..tag.open_end, &body);
    }
    splice(xml, tag.open_end..tag.open_end, &child)
}

struct Tag {
    start: usize,
    open_end: usize,
    self_closing: bool,
}

/// The `rdf:Description` Zaru writes into: the one already bound to the XMP
/// namespace, otherwise the first, which in every real sidecar is the
/// top-level one.
fn description_tag(xml: &str) -> Option<Tag> {
    let tags = open_tags(xml, "rdf:Description");
    tags.iter()
        .find(|t| xml[t.start..t.open_end].contains(XMP_NS))
        .or_else(|| tags.first())
        .map(|t| Tag { start: t.start, open_end: t.open_end, self_closing: t.self_closing })
}

fn open_tags(xml: &str, name: &str) -> Vec<Tag> {
    let needle = format!("<{name}");
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(hit) = xml[at..].find(&needle) {
        let start = at + hit;
        at = start + needle.len();
        match bytes.get(at) {
            Some(c) if c.is_ascii_whitespace() || *c == b'>' || *c == b'/' => {}
            _ => continue,
        }
        // Walk to the closing `>`, ignoring any inside quoted values.
        let mut i = at;
        let mut quote: Option<u8> = None;
        while i < bytes.len() {
            let c = bytes[i];
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == b'"' || c == b'\'' => quote = Some(c),
                None if c == b'>' => break,
                None => {}
            }
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        out.push(Tag {
            start,
            open_end: i + 1,
            self_closing: bytes[i - 1] == b'/',
        });
        at = i + 1;
    }
    out
}

fn line_indent(xml: &str, at: usize) -> String {
    let line_start = xml[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    xml[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}
