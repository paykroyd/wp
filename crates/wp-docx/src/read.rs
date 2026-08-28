//! `.docx` → model. Everything recognised becomes model; everything else is
//! kept verbatim as opaque items, raw property blocks, or raw body blocks.

use crate::package::DocxPackage;
use crate::xml::*;
use anyhow::{anyhow, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use wp_core::document::rewrite_attrs;
use wp_core::model::*;
use wp_core::style::{Style, StyleKind, StyleSheet};
use wp_core::Document;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub label: String,
    pub count: usize,
}

pub struct Loaded {
    pub doc: Document,
    pub package: DocxPackage,
    pub warnings: Vec<Warning>,
}

impl Loaded {
    /// The single-line summary shown on open (spec §7.4).
    pub fn warning_line(&self) -> Option<String> {
        if self.warnings.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .warnings
            .iter()
            .map(|w| {
                if w.count == 1 {
                    format!("1 {}", w.label)
                } else {
                    format!("{} {}s", w.count, w.label)
                }
            })
            .collect();
        let joined = match parts.len() {
            1 => parts[0].clone(),
            2 => format!("{} and {}", parts[0], parts[1]),
            _ => {
                let (last, rest) = parts.split_last().unwrap();
                format!("{}, and {}", rest.join(", "), last)
            }
        };
        let verb = if self.warnings.len() == 1 && self.warnings[0].count == 1 { "is" } else { "are" };
        Some(format!("{} {} preserved but not editable. Ctrl+K → Warnings for detail.", joined, verb))
    }
}

/// Reader state shared across paragraphs.
pub struct Ctx {
    pub theme_major: Option<String>,
    pub theme_minor: Option<String>,
    bookmark_names: HashMap<String, String>,
    next_wrapper_id: u32,
    warnings: HashMap<String, usize>,
}

impl Ctx {
    pub fn new(theme_major: Option<String>, theme_minor: Option<String>) -> Ctx {
        Ctx { theme_major, theme_minor, bookmark_names: HashMap::new(), next_wrapper_id: 1, warnings: HashMap::new() }
    }
    fn warn(&mut self, label: &str) {
        *self.warnings.entry(label.to_string()).or_insert(0) += 1;
    }
    fn wrapper_id(&mut self) -> u32 {
        let id = self.next_wrapper_id;
        self.next_wrapper_id += 1;
        id
    }
}

pub fn read(path: &std::path::Path) -> Result<Loaded> {
    let bytes = std::fs::read(path)?;
    read_bytes(&bytes)
}

pub fn read_bytes(bytes: &[u8]) -> Result<Loaded> {
    let mut package = DocxPackage::from_bytes(bytes)?;
    let (major, minor) = theme_fonts(&package);
    package.theme_major = major.clone();
    package.theme_minor = minor.clone();
    let mut ctx = Ctx::new(major, minor);

    let styles = match package.styles_part.as_deref().and_then(|p| package.get_str(p)) {
        Some(xml) => parse_styles(&xml, &ctx)?,
        None => StyleSheet::builtin(),
    };

    let main = package.get_str(&package.main_part).ok_or_else(|| anyhow!("main part missing"))?;
    let (paragraphs, section, prolog, root_tag, pre_body) = parse_document(&main, &mut ctx)?;
    package.empty_body = paragraphs.is_empty();
    package.prolog = prolog;
    package.root_tag = root_tag;
    package.pre_body = pre_body;

    let mut doc = Document::from_paragraphs(paragraphs);
    doc.styles = styles;
    doc.section = section;

    let mut warnings: Vec<Warning> = ctx.warnings.iter().map(|(k, v)| Warning { label: k.clone(), count: *v }).collect();
    warnings.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
    Ok(Loaded { doc, package, warnings })
}

fn theme_fonts(pkg: &DocxPackage) -> (Option<String>, Option<String>) {
    let name = pkg
        .entries
        .iter()
        .find(|e| e.name.starts_with("word/theme/") && e.name.ends_with(".xml"))
        .map(|e| e.name.clone());
    let Some(name) = name else { return (None, None) };
    let xml = pkg.get_str(&name).unwrap_or_default();
    let mut reader = Reader::from_str(&xml);
    let mut major = None;
    let mut minor = None;
    let mut in_major = false;
    let mut in_minor = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"majorFont" => in_major = true,
                b"minorFont" => in_minor = true,
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if local(e.name().as_ref()) == b"latin" {
                    let tf = attr(&e, "typeface");
                    if in_major && major.is_none() {
                        major = tf;
                    } else if in_minor && minor.is_none() {
                        minor = tf;
                    }
                }
            }
            Ok(Event::End(e)) => match local(e.name().as_ref()) {
                b"majorFont" => in_major = false,
                b"minorFont" => in_minor = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (major, minor)
}

// ---------------------------------------------------------------------------
// document.xml
// ---------------------------------------------------------------------------

type ParsedDoc = (Vec<Paragraph>, SectionProps, String, String, String);

fn parse_document(xml: &str, ctx: &mut Ctx) -> Result<ParsedDoc> {
    let mut reader = Reader::from_str(xml);
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut section = SectionProps::default();
    let mut prolog = String::new();
    let mut root_tag = String::new();
    let mut pre_body = String::new();
    let mut in_body = false;
    let mut pending: Vec<Item> = Vec::new(); // body-level opaque items awaiting a paragraph
    let mut pre_body_start = 0usize;

    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                if !in_body {
                    if local(&n) == b"document" {
                        prolog = xml[..before].to_string();
                        root_tag = xml[before..reader.buffer_position() as usize].to_string();
                        pre_body_start = reader.buffer_position() as usize;
                    } else if local(&n) == b"body" {
                        pre_body = xml[pre_body_start..before].to_string();
                        in_body = true;
                    } else {
                        // Something between <w:document> and <w:body>: keep verbatim.
                        let _ = reader.read_to_end(name);
                    }
                    continue;
                }
                match n.as_slice() {
                    b"w:p" => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        let mut p = parse_paragraph(&xml[before..after], ctx)?;
                        if !pending.is_empty() {
                            let mut items = std::mem::take(&mut pending);
                            items.append(&mut p.items);
                            p.items = items;
                        }
                        paragraphs.push(p);
                    }
                    b"w:sectPr" => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        section = parse_sectpr(&xml[before..after]);
                    }
                    _ => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        let raw = &xml[before..after];
                        let label = block_label(&n, raw);
                        ctx.warn(&warning_label_for_block(&n));
                        paragraphs.push(raw_block(raw, &label));
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if !in_body {
                    continue;
                }
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:p" => paragraphs.push(Paragraph::new()),
                    b"w:sectPr" => section = parse_sectpr(&xml[before..after]),
                    b"w:bookmarkStart" | b"w:bookmarkEnd" => {
                        pending.push(bookmark_item(&e, &xml[before..after], ctx));
                    }
                    _ => {
                        let label = String::from_utf8_lossy(local(&n)).into_owned();
                        pending.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label))));
                    }
                }
            }
            Ok(Event::End(e)) => {
                if local(e.name().as_ref()) == b"body" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("XML error in document: {}", e)),
            _ => {}
        }
    }
    if !pending.is_empty() {
        match paragraphs.last_mut() {
            Some(p) => p.items.append(&mut pending),
            None => paragraphs.push(Paragraph { props: ParaProps::default(), items: pending }),
        }
    }
    Ok((paragraphs, section, prolog, root_tag, pre_body))
}

fn raw_block(raw: &str, label: &str) -> Paragraph {
    Paragraph {
        props: ParaProps { raw_block: true, ..Default::default() },
        items: vec![Item::Code(Code::Opaque(OpaqueXml::element(raw, label)))],
    }
}

fn block_label(tag: &[u8], raw: &str) -> String {
    match tag {
        b"w:tbl" => {
            let rows = raw.matches("<w:tr>").count() + raw.matches("<w:tr ").count();
            let first_row_end = raw.find("</w:tr>").unwrap_or(raw.len());
            let cols = raw[..first_row_end].matches("<w:tc>").count() + raw[..first_row_end].matches("<w:tc ").count();
            format!("Table {}×{}", rows, cols.max(1))
        }
        b"w:sdt" => "Content Control".into(),
        b"w:altChunk" => "Embedded Content".into(),
        b"m:oMathPara" | b"m:oMath" => "Equation".into(),
        _ => String::from_utf8_lossy(local(tag)).into_owned(),
    }
}

fn warning_label_for_block(tag: &[u8]) -> String {
    match tag {
        b"w:tbl" => "table".into(),
        b"w:sdt" => "content control".into(),
        b"m:oMathPara" | b"m:oMath" => "equation".into(),
        _ => format!("{} block", String::from_utf8_lossy(local(tag))),
    }
}

fn bookmark_item(e: &BytesStart<'_>, raw: &str, ctx: &mut Ctx) -> Item {
    let id = attr(e, "w:id").unwrap_or_default();
    if local(e.name().as_ref()) == b"bookmarkStart" {
        let name = attr(e, "w:name").unwrap_or_else(|| format!("bm{}", id));
        ctx.bookmark_names.insert(id, name.clone());
        Item::Code(Code::Bookmark(name))
    } else if let Some(name) = ctx.bookmark_names.get(&id) {
        Item::Code(Code::BookmarkEnd(name.clone()))
    } else {
        Item::Code(Code::Opaque(OpaqueXml::element(raw, "Bookmark End")))
    }
}

// ---------------------------------------------------------------------------
// Paragraphs and runs
// ---------------------------------------------------------------------------

pub fn parse_paragraph(xml: &str, ctx: &mut Ctx) -> Result<Paragraph> {
    let mut reader = Reader::from_str(xml);
    let mut para = Paragraph::new();
    // consume the <w:p> start
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => break,
            Ok(Event::Empty(_)) | Ok(Event::Eof) => return Ok(para),
            Err(e) => return Err(anyhow!("XML error: {}", e)),
            _ => {}
        }
    }
    let mut wrappers: Vec<(u32, bool)> = Vec::new(); // (id, deleted)
    parse_para_content(&mut reader, xml, &mut para, ctx, &mut wrappers, b"w:p")?;
    // Normalise: merge adjacent runs with identical formatting.
    let (items, _) = rewrite_attrs(&para.items, 0..0, None, |_| {});
    para.items = items;
    Ok(para)
}

/// Parse children of a paragraph-level container until its end tag.
fn parse_para_content(
    reader: &mut Reader<&[u8]>,
    xml: &str,
    para: &mut Paragraph,
    ctx: &mut Ctx,
    wrappers: &mut Vec<(u32, bool)>,
    end_tag: &[u8],
) -> Result<()> {
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                match n.as_slice() {
                    b"w:pPr" => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        para.props = parse_ppr(&xml[before..after], ctx);
                    }
                    b"w:r" => {
                        let deleted = wrappers.last().map(|w| w.1).unwrap_or(false);
                        parse_run(reader, xml, para, ctx, deleted)?;
                    }
                    b"w:sdt" => {
                        // Open = everything up to and including <w:sdtContent>
                        let id = ctx.wrapper_id();
                        let mut content_after = before;
                        loop {
                            match reader.read_event() {
                                Ok(Event::Start(s)) if s.name().as_ref() == b"w:sdtContent" => {
                                    content_after = reader.buffer_position() as usize;
                                    break;
                                }
                                Ok(Event::Start(s)) => {
                                    let _ = reader.read_to_end(s.name());
                                }
                                Ok(Event::End(_)) | Ok(Event::Eof) => break,
                                Err(e) => return Err(anyhow!("XML error: {}", e)),
                                _ => {}
                            }
                        }
                        ctx.warn("content control");
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml {
                            xml: xml[before..content_after].to_string(),
                            label: "Content Control".into(),
                            kind: OpaqueKind::Open(id),
                            protected: false,
                            deleted: false,
                        })));
                        wrappers.push((id, wrappers.last().map(|w| w.1).unwrap_or(false)));
                        parse_para_content(reader, xml, para, ctx, wrappers, b"w:sdtContent")?;
                        wrappers.pop();
                        // consume </w:sdt>
                        loop {
                            match reader.read_event() {
                                Ok(Event::End(en)) if en.name().as_ref() == b"w:sdt" => break,
                                Ok(Event::Eof) => break,
                                Err(e) => return Err(anyhow!("XML error: {}", e)),
                                _ => {}
                            }
                        }
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml {
                            xml: "</w:sdtContent></w:sdt>".into(),
                            label: "content control".into(),
                            kind: OpaqueKind::Close(id),
                            protected: false,
                            deleted: false,
                        })));
                    }
                    b"w:hyperlink" | b"w:ins" | b"w:del" | b"w:moveFrom" | b"w:moveTo" | b"w:smartTag" | b"w:customXml"
                    | b"w:fldSimple" | b"w:dir" | b"w:bdo" => {
                        let id = ctx.wrapper_id();
                        let after_start = reader.buffer_position() as usize;
                        let (label, protected, deleted) = match n.as_slice() {
                            b"w:hyperlink" => ("Hyperlink", false, false),
                            b"w:ins" => {
                                ctx.warn("tracked change");
                                ("Inserted Text", true, false)
                            }
                            b"w:del" => {
                                ctx.warn("tracked change");
                                ("Deleted Text", true, true)
                            }
                            b"w:moveFrom" => {
                                ctx.warn("tracked change");
                                ("Moved Text (from)", true, true)
                            }
                            b"w:moveTo" => {
                                ctx.warn("tracked change");
                                ("Moved Text (to)", true, false)
                            }
                            b"w:fldSimple" => {
                                ctx.warn("field");
                                ("Field", false, false)
                            }
                            _ => ("Tagged Text", false, false),
                        };
                        let inherited_deleted = wrappers.last().map(|w| w.1).unwrap_or(false);
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml {
                            xml: xml[before..after_start].to_string(),
                            label: label.into(),
                            kind: OpaqueKind::Open(id),
                            protected,
                            deleted: deleted || inherited_deleted,
                        })));
                        wrappers.push((id, deleted || inherited_deleted));
                        parse_para_content(reader, xml, para, ctx, wrappers, &n)?;
                        wrappers.pop();
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml {
                            xml: format!("</{}>", String::from_utf8_lossy(&n)),
                            label: label.to_lowercase(),
                            kind: OpaqueKind::Close(id),
                            protected,
                            deleted: deleted || inherited_deleted,
                        })));
                    }
                    _ => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        let label = element_label(&n, ctx);
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label))));
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:pPr" => para.props = parse_ppr(&xml[before..after], ctx),
                    b"w:r" => {}
                    b"w:bookmarkStart" | b"w:bookmarkEnd" => para.items.push(bookmark_item(&e, &xml[before..after], ctx)),
                    b"w:proofErr" => {} // spelling-error markers: Word regenerates them
                    _ => {
                        let label = element_label(&n, ctx);
                        para.items.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label))));
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == end_tag {
                    return Ok(());
                }
            }
            Ok(Event::Eof) => return Ok(()),
            Err(e) => return Err(anyhow!("XML error: {}", e)),
            _ => {}
        }
    }
}

fn element_label(tag: &[u8], ctx: &mut Ctx) -> String {
    match tag {
        b"w:commentRangeStart" | b"w:commentRangeEnd" | b"w:commentReference" => {
            if tag == b"w:commentRangeStart" {
                ctx.warn("comment");
            }
            "Comment".into()
        }
        b"w:drawing" | b"mc:AlternateContent" => {
            ctx.warn("drawing");
            "Drawing".into()
        }
        b"w:pict" => {
            ctx.warn("drawing");
            "Picture".into()
        }
        b"w:object" => {
            ctx.warn("embedded object");
            "Object".into()
        }
        b"w:fldChar" => {
            if false {
                ctx.warn("field");
            }
            "Field".into()
        }
        b"w:instrText" | b"w:delInstrText" => {
            ctx.warn("field");
            "Field Code".into()
        }
        b"w:footnoteReference" => {
            ctx.warn("footnote");
            "Footnote".into()
        }
        b"w:endnoteReference" => {
            ctx.warn("endnote");
            "Endnote".into()
        }
        b"m:oMath" | b"m:oMathPara" => {
            ctx.warn("equation");
            "Equation".into()
        }
        b"w:footnoteRef" | b"w:endnoteRef" => "Note Ref".into(),
        b"w:sym" => "Symbol".into(),
        b"w:ptab" => "Pos Tab".into(),
        b"w:ruby" => "Ruby".into(),
        b"w:permStart" | b"w:permEnd" => "Permission".into(),
        _ => String::from_utf8_lossy(local(tag)).into_owned(),
    }
}

fn parse_run(reader: &mut Reader<&[u8]>, xml: &str, para: &mut Paragraph, ctx: &mut Ctx, deleted: bool) -> Result<()> {
    let mut attrs: Vec<Attr> = Vec::new();
    let mut content: Vec<Item> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                match n.as_slice() {
                    b"w:rPr" => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        let raw = &xml[before..after];
                        attrs = parse_rpr(raw, ctx).into_iter().filter_map(|(_, a)| a).collect();
                        attrs.push(Attr::Raw(raw.to_string()));
                    }
                    b"w:t" | b"w:delText" => {
                        let mut text = String::new();
                        loop {
                            match reader.read_event() {
                                Ok(Event::Text(t)) => text.push_str(&t.unescape().unwrap_or_default()),
                                Ok(Event::CData(c)) => text.push_str(&String::from_utf8_lossy(&c)),
                                Ok(Event::End(_)) | Ok(Event::Eof) => break,
                                Err(e) => return Err(anyhow!("XML error: {}", e)),
                                _ => {}
                            }
                        }
                        content.extend(text.chars().map(Item::Char));
                    }
                    _ => {
                        let _ = reader.read_to_end(name);
                        let after = reader.buffer_position() as usize;
                        let label = element_label(&n, ctx);
                        content.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label))));
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:rPr" => attrs.push(Attr::Raw(xml[before..after].to_string())),
                    b"w:t" | b"w:delText" => {}
                    b"w:tab" => content.push(Item::Code(Code::Tab)),
                    b"w:br" => match attr(&e, "w:type").as_deref() {
                        Some("page") => content.push(Item::Code(Code::PageBreak)),
                        Some("column") => content.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], "Column Break")))),
                        _ => content.push(Item::Code(Code::LineBreak)),
                    },
                    b"w:cr" => content.push(Item::Code(Code::LineBreak)),
                    b"w:noBreakHyphen" => content.push(Item::Char('\u{2011}')),
                    b"w:softHyphen" => content.push(Item::Char('\u{ad}')),
                    b"w:lastRenderedPageBreak" => {}
                    _ => {
                        let label = element_label(&n, ctx);
                        content.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label))));
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"w:r" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("XML error: {}", e)),
            _ => {}
        }
    }
    let _ = deleted;
    if content.is_empty() {
        return Ok(());
    }
    for a in &attrs {
        para.items.push(Item::Code(Code::On(a.clone())));
    }
    para.items.append(&mut content);
    for a in attrs.iter().rev() {
        para.items.push(Item::Code(Code::Off(a.kind())));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Parse a `w:rPr` element. Returns each child with the attribute it maps to
/// (`None` for children wp does not model).
pub fn parse_rpr(xml: &str, ctx: &Ctx) -> Vec<(RawChild, Option<Attr>)> {
    let mut out = Vec::new();
    for child in children_of(xml) {
        let Some(e) = start_tag(&child.xml) else {
            out.push((child, None));
            continue;
        };
        let a = match child.tag.as_str() {
            "w:rStyle" => attr(&e, "w:val").map(Attr::CharStyle),
            "w:rFonts" => {
                let f = attr(&e, "w:ascii")
                    .or_else(|| attr(&e, "w:hAnsi"))
                    .or_else(|| attr(&e, "w:cs"))
                    .or_else(|| attr(&e, "w:eastAsia"))
                    .or_else(|| theme_font(attr(&e, "w:asciiTheme").or_else(|| attr(&e, "w:hAnsiTheme")), ctx));
                f.map(Attr::Font)
            }
            "w:b" => Some(Attr::Bold(attr_bool(&e, "w:val"))),
            "w:i" => Some(Attr::Italic(attr_bool(&e, "w:val"))),
            "w:u" => Some(Attr::Underline(Underline::from_docx(attr(&e, "w:val").as_deref().unwrap_or("single")))),
            "w:strike" => Some(Attr::Strike(attr_bool(&e, "w:val"))),
            "w:dstrike" => Some(Attr::DoubleStrike(attr_bool(&e, "w:val"))),
            "w:vertAlign" => Some(Attr::VertAlign(match attr(&e, "w:val").as_deref() {
                Some("superscript") => VertAlign::Superscript,
                Some("subscript") => VertAlign::Subscript,
                _ => VertAlign::Baseline,
            })),
            "w:smallCaps" => Some(Attr::SmallCaps(attr_bool(&e, "w:val"))),
            "w:caps" => Some(Attr::AllCaps(attr_bool(&e, "w:val"))),
            "w:sz" => attr_i32(&e, "w:val").map(|v| Attr::Size(v.clamp(1, 3276) as u16)),
            "w:color" => match attr(&e, "w:val") {
                Some(v) if v != "auto" => Rgb::parse_hex(&v).map(Attr::Color),
                _ => None,
            },
            "w:highlight" => attr(&e, "w:val").and_then(|v| Highlight::from_docx(&v)).map(Attr::Highlight),
            _ => None,
        };
        out.push((child, a));
    }
    out
}

fn theme_font(theme: Option<String>, ctx: &Ctx) -> Option<String> {
    let t = theme?;
    if t.starts_with("major") {
        ctx.theme_major.clone().or_else(|| Some("Calibri Light".into()))
    } else {
        ctx.theme_minor.clone().or_else(|| Some("Calibri".into()))
    }
}

/// Run properties from a `w:rPr` (for styles and paragraph marks).
pub fn run_props_from_rpr(xml: &str, ctx: &Ctx) -> RunProps {
    let mut r = RunProps::default();
    for (child, a) in parse_rpr(xml, ctx) {
        match a {
            Some(a) => r.apply(&a),
            None => r.opaque.push(child.xml),
        }
    }
    r
}

pub fn parse_ppr(xml: &str, ctx: &Ctx) -> ParaProps {
    let mut p = ParaProps { raw_ppr: Some(xml.to_string()), ..Default::default() };
    for child in children_of(xml) {
        let Some(e) = start_tag(&child.xml) else {
            p.opaque.push(child.xml);
            continue;
        };
        match child.tag.as_str() {
            "w:pStyle" => p.style = attr(&e, "w:val"),
            "w:jc" => p.align = attr(&e, "w:val").map(|v| Align::from_docx(&v)),
            "w:ind" => {
                p.indent_left = attr_i32(&e, "w:left").or_else(|| attr_i32(&e, "w:start"));
                p.indent_right = attr_i32(&e, "w:right").or_else(|| attr_i32(&e, "w:end"));
                p.first_line = attr_i32(&e, "w:firstLine");
                p.hanging = attr_i32(&e, "w:hanging");
                if attr(&e, "w:leftChars").is_some() || attr(&e, "w:firstLineChars").is_some() {
                    p.opaque.push(child.xml.clone());
                }
            }
            "w:spacing" => {
                p.space_before = attr_i32(&e, "w:before");
                p.space_after = attr_i32(&e, "w:after");
                if let Some(line) = attr_i32(&e, "w:line") {
                    p.line_spacing = Some(match attr(&e, "w:lineRule").as_deref() {
                        Some("exact") => LineSpacing::Exact(line),
                        Some("atLeast") => LineSpacing::AtLeast(line),
                        _ => LineSpacing::Auto(line),
                    });
                }
                if attr(&e, "w:beforeAutospacing").is_some() || attr(&e, "w:afterAutospacing").is_some() {
                    p.opaque.push(child.xml.clone());
                }
            }
            "w:keepNext" => p.keep_next = Some(attr_bool(&e, "w:val")),
            "w:keepLines" => p.keep_lines = Some(attr_bool(&e, "w:val")),
            "w:widowControl" => p.widow_control = Some(attr_bool(&e, "w:val")),
            "w:pageBreakBefore" => p.page_break_before = Some(attr_bool(&e, "w:val")),
            "w:outlineLvl" => p.outline_level = attr_i32(&e, "w:val").map(|v| v.clamp(0, 9) as u8),
            "w:tabs" => {
                for t in children_of(&child.xml) {
                    if let Some(te) = start_tag(&t.xml) {
                        let kind = match attr(&te, "w:val").as_deref() {
                            Some("center") => TabKind::Center,
                            Some("right") | Some("end") => TabKind::Right,
                            Some("decimal") => TabKind::Decimal,
                            Some("bar") => TabKind::Bar,
                            _ => TabKind::Left,
                        };
                        let clear = attr(&te, "w:val").as_deref() == Some("clear");
                        let leader = match attr(&te, "w:leader").as_deref() {
                            Some("dot") | Some("middleDot") => TabLeader::Dot,
                            Some("hyphen") => TabLeader::Hyphen,
                            Some("underscore") | Some("heavy") => TabLeader::Underscore,
                            _ => TabLeader::None,
                        };
                        if let Some(pos) = attr_i32(&te, "w:pos") {
                            p.tabs.push(TabStop { pos, kind, leader, clear });
                        }
                    }
                }
            }
            "w:numPr" => {
                let mut num_id = None;
                let mut level = 0u8;
                for c in children_of(&child.xml) {
                    if let Some(ce) = start_tag(&c.xml) {
                        match c.tag.as_str() {
                            "w:numId" => num_id = attr_i32(&ce, "w:val"),
                            "w:ilvl" => level = attr_i32(&ce, "w:val").unwrap_or(0).clamp(0, 8) as u8,
                            _ => {}
                        }
                    }
                }
                match num_id {
                    Some(n) => p.list = Some(ListRef { num_id: n, level }),
                    None => p.opaque.push(child.xml.clone()),
                }
            }
            "w:pBdr" => {
                let mut b = ParaBorders::default();
                for c in children_of(&child.xml) {
                    if let Some(ce) = start_tag(&c.xml) {
                        let border = Border {
                            style: match attr(&ce, "w:val").as_deref() {
                                Some("double") => BorderStyle::Double,
                                Some("dotted") => BorderStyle::Dotted,
                                Some("dashed") => BorderStyle::Dashed,
                                Some("thick") => BorderStyle::Thick,
                                _ => BorderStyle::Single,
                            },
                            size: attr_i32(&ce, "w:sz").unwrap_or(4).clamp(0, 255) as u16,
                            color: attr(&ce, "w:color").and_then(|v| Rgb::parse_hex(&v)),
                            space: attr_i32(&ce, "w:space").unwrap_or(1).clamp(0, 255) as u16,
                        };
                        match c.tag.as_str() {
                            "w:top" => b.top = Some(border),
                            "w:bottom" => b.bottom = Some(border),
                            "w:left" | "w:start" => b.left = Some(border),
                            "w:right" | "w:end" => b.right = Some(border),
                            _ => {}
                        }
                    }
                }
                p.borders = Some(b);
            }
            "w:shd" => {
                p.shading = attr(&e, "w:fill").filter(|f| f != "auto").and_then(|f| Rgb::parse_hex(&f));
                if p.shading.is_none() {
                    p.opaque.push(child.xml.clone());
                }
            }
            "w:rPr" => {
                if child.xml.contains("<w:ins") || child.xml.contains("<w:del") {
                    // Tracked paragraph-mark change; counted as a tracked change.
                }
                p.mark = run_props_from_rpr(&child.xml, ctx);
            }
            "w:sectPr" => p.sect_break = Some(child.xml.clone()),
            _ => p.opaque.push(child.xml.clone()),
        }
    }
    p
}

pub fn parse_sectpr(xml: &str) -> SectionProps {
    let mut s = SectionProps::default();
    s.opaque_children = Vec::new();
    for child in children_of(xml) {
        if let Some(e) = start_tag(&child.xml) {
            match child.tag.as_str() {
                "w:pgSz" => {
                    if let Some(w) = attr_i32(&e, "w:w") {
                        s.page_width = w;
                    }
                    if let Some(h) = attr_i32(&e, "w:h") {
                        s.page_height = h;
                    }
                    s.orientation = if attr(&e, "w:orient").as_deref() == Some("landscape") {
                        Orientation::Landscape
                    } else {
                        Orientation::Portrait
                    };
                }
                "w:pgMar" => {
                    if let Some(v) = attr_i32(&e, "w:top") {
                        s.margin_top = v.abs();
                    }
                    if let Some(v) = attr_i32(&e, "w:bottom") {
                        s.margin_bottom = v.abs();
                    }
                    if let Some(v) = attr_i32(&e, "w:left").or_else(|| attr_i32(&e, "w:start")) {
                        s.margin_left = v;
                    }
                    if let Some(v) = attr_i32(&e, "w:right").or_else(|| attr_i32(&e, "w:end")) {
                        s.margin_right = v;
                    }
                    if let Some(v) = attr_i32(&e, "w:header") {
                        s.header_distance = v;
                    }
                    if let Some(v) = attr_i32(&e, "w:footer") {
                        s.footer_distance = v;
                    }
                    if let Some(v) = attr_i32(&e, "w:gutter") {
                        s.gutter = v;
                    }
                }
                "w:cols" => {
                    s.columns = attr_i32(&e, "w:num").unwrap_or(1).clamp(1, 12) as u16;
                }
                _ => {}
            }
        }
        s.opaque_children.push(child.xml);
    }
    s
}

// ---------------------------------------------------------------------------
// styles.xml
// ---------------------------------------------------------------------------

pub fn parse_styles(xml: &str, ctx: &Ctx) -> Result<StyleSheet> {
    let mut sheet = StyleSheet::empty();
    let mut reader = Reader::from_str(xml);
    let mut depth = 0;
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                if depth == 0 {
                    // <w:styles>
                    sheet.opaque.push(xml[before..reader.buffer_position() as usize].to_string());
                    depth = 1;
                    continue;
                }
                let _ = reader.read_to_end(name);
                let after = reader.buffer_position() as usize;
                let raw = &xml[before..after];
                match n.as_slice() {
                    b"w:docDefaults" => {
                        for c in children_of(raw) {
                            match c.tag.as_str() {
                                "w:rPrDefault" => {
                                    if let Some(r) = children_of(&c.xml).into_iter().find(|x| x.tag == "w:rPr") {
                                        sheet.default_run = run_props_from_rpr(&r.xml, ctx);
                                    }
                                }
                                "w:pPrDefault" => {
                                    if let Some(p) = children_of(&c.xml).into_iter().find(|x| x.tag == "w:pPr") {
                                        sheet.default_para = parse_ppr(&p.xml, ctx);
                                        sheet.default_para.raw_ppr = None;
                                    }
                                }
                                _ => {}
                            }
                        }
                        sheet.opaque.push(raw.to_string());
                    }
                    b"w:style" => sheet.styles.push(parse_style(raw, &e, ctx)),
                    _ => sheet.opaque.push(raw.to_string()),
                }
            }
            Ok(Event::Empty(e)) => {
                if depth == 0 {
                    continue;
                }
                let after = reader.buffer_position() as usize;
                if e.name().as_ref() == b"w:style" {
                    sheet.styles.push(parse_style(&xml[before..after], &e, ctx));
                } else {
                    sheet.opaque.push(xml[before..after].to_string());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("XML error in styles: {}", e)),
            _ => {}
        }
    }
    if sheet.default_run.size.is_none() {
        sheet.default_run.size = Some(20); // Word's fallback when docDefaults omits sz: 10pt
    }
    if sheet.default_run.font.is_none() {
        sheet.default_run.font = Some(ctx.theme_minor.clone().unwrap_or_else(|| "Times New Roman".into()));
    }
    Ok(sheet)
}

fn parse_style(raw: &str, e: &BytesStart<'_>, ctx: &Ctx) -> Style {
    let id = attr(e, "w:styleId").unwrap_or_default();
    let kind = match attr(e, "w:type").as_deref() {
        Some("character") => StyleKind::Character,
        Some("table") => StyleKind::Table,
        Some("numbering") => StyleKind::Numbering,
        _ => StyleKind::Paragraph,
    };
    let mut st = Style::para(&id, &id);
    st.kind = kind;
    st.is_default = attr(e, "w:default").map(|v| v == "1" || v == "true").unwrap_or(false);
    for c in children_of(raw) {
        let Some(ce) = start_tag(&c.xml) else { continue };
        match c.tag.as_str() {
            "w:name" => st.name = attr(&ce, "w:val").unwrap_or_else(|| id.clone()),
            "w:basedOn" => st.based_on = attr(&ce, "w:val"),
            "w:next" => st.next = attr(&ce, "w:val"),
            "w:pPr" => {
                st.para = parse_ppr(&c.xml, ctx);
                st.para.raw_ppr = None;
            }
            "w:rPr" => st.run = run_props_from_rpr(&c.xml, ctx),
            "w:hidden" | "w:semiHidden" => st.hidden = attr_bool(&ce, "w:val"),
            _ => {}
        }
    }
    st.raw_xml = Some(raw.to_string());
    st
}
