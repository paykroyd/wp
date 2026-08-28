//! XML helpers shared by the reader and writer.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::fmt::Write as _;

pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The local name of a qualified name (`w:p` → `p`).
pub fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

pub fn attr<'a>(e: &'a BytesStart<'a>, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        let k = a.key.as_ref();
        if k == key.as_bytes() || local(k) == local(key.as_bytes()) && key.contains(':') {
            return a.unescape_value().ok().map(|v| v.into_owned());
        }
    }
    None
}

pub fn attr_i32(e: &BytesStart<'_>, key: &str) -> Option<i32> {
    attr(e, key).and_then(|v| v.trim().parse::<f64>().ok()).map(|f| f.round() as i32)
}

/// Toggle-property value (`w:b`, `w:i`, …): absent `w:val` means true.
pub fn attr_bool(e: &BytesStart<'_>, key: &str) -> bool {
    match attr(e, key).as_deref() {
        None => true,
        Some("0") | Some("false") | Some("off") => false,
        _ => true,
    }
}

/// A child element captured verbatim from a properties block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawChild {
    pub tag: String,
    pub xml: String,
}

/// Split an element's inner XML into its direct children, each captured
/// verbatim. `xml` must be the complete element (start tag through end tag).
pub fn children_of(xml: &str) -> Vec<RawChild> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if depth == 1 {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let _ = reader.read_to_end(e.name());
                    let after = reader.buffer_position() as usize;
                    out.push(RawChild { tag, xml: xml[before..after].to_string() });
                } else {
                    depth += 1;
                }
            }
            Ok(Event::Empty(e)) => {
                if depth == 1 {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let after = reader.buffer_position() as usize;
                    out.push(RawChild { tag, xml: xml[before..after].to_string() });
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// The attribute text of a start tag exactly as written (leading whitespace
/// included), for verbatim re-emission.
pub fn tag_attrs(e: &BytesStart<'_>) -> String {
    let raw = String::from_utf8_lossy(e.attributes_raw()).into_owned();
    let t = raw.trim_end();
    if t.trim().is_empty() {
        String::new()
    } else {
        t.to_string()
    }
}

/// Parse the start tag of a single raw element for attribute access.

pub fn start_tag(xml: &str) -> Option<BytesStart<'static>> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => return Some(e.into_owned()),
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Schema order of `w:pPr` children (CT_PPr). Unknown tags sort last, before
/// `w:rPr`, `w:sectPr`, `w:pPrChange`.
pub const PPR_ORDER: &[&str] = &[
    "w:pStyle", "w:keepNext", "w:keepLines", "w:pageBreakBefore", "w:framePr", "w:widowControl", "w:numPr",
    "w:suppressLineNumbers", "w:pBdr", "w:shd", "w:tabs", "w:suppressAutoHyphens", "w:kinsoku", "w:wordWrap",
    "w:overflowPunct", "w:topLinePunct", "w:autoSpaceDE", "w:autoSpaceDN", "w:bidi", "w:adjustRightInd",
    "w:snapToGrid", "w:spacing", "w:ind", "w:contextualSpacing", "w:mirrorIndents", "w:suppressOverlap", "w:jc",
    "w:textDirection", "w:textAlignment", "w:textboxTightWrap", "w:outlineLvl", "w:divId", "w:cnfStyle", "w:rPr",
    "w:sectPr", "w:pPrChange",
];

/// Schema order of `w:rPr` children (CT_RPr).
pub const RPR_ORDER: &[&str] = &[
    "w:rStyle", "w:rFonts", "w:b", "w:bCs", "w:i", "w:iCs", "w:caps", "w:smallCaps", "w:strike", "w:dstrike",
    "w:outline", "w:shadow", "w:emboss", "w:imprint", "w:noProof", "w:snapToGrid", "w:vanish", "w:webHidden",
    "w:color", "w:spacing", "w:w", "w:kern", "w:position", "w:sz", "w:szCs", "w:highlight", "w:u", "w:effect",
    "w:bdr", "w:shd", "w:fitText", "w:vertAlign", "w:rtl", "w:cs", "w:em", "w:lang", "w:eastAsianLayout",
    "w:specVanish", "w:oMath", "w:rPrChange",
];

pub fn order_index(order: &[&str], tag: &str) -> usize {
    order.iter().position(|t| *t == tag).unwrap_or(order.len() - 3) // before rPr/sectPr/pPrChange or bdr…
}

/// Sort raw children by schema order, stably.
pub fn sort_children(order: &[&str], children: &mut Vec<RawChild>) {
    children.sort_by_key(|c| order_index(order, &c.tag));
}

pub fn twips_attr(out: &mut String, name: &str, v: i32) {
    let _ = write!(out, " {}=\"{}\"", name, v);
}
