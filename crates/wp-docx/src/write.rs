//! Model → `.docx`. Preserved XML is emitted verbatim; only what changed is
//! regenerated.

use crate::package::DocxPackage;
use crate::read::{parse_rpr, parse_sectpr, Ctx};
use crate::xml::*;
use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write as _;
use wp_core::document::Run;
use wp_core::model::*;
use wp_core::style::{StyleKind, StyleSheet};
use wp_core::Document;

pub const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

const DEFAULT_ROOT: &str = concat!(
    "<w:document xmlns:wpc=\"http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas\" ",
    "xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" ",
    "xmlns:o=\"urn:schemas-microsoft-com:office:office\" ",
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
    "xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" ",
    "xmlns:v=\"urn:schemas-microsoft-com:vml\" ",
    "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" ",
    "xmlns:w10=\"urn:schemas-microsoft-com:office:word\" ",
    "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
    "xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" ",
    "xmlns:wne=\"http://schemas.microsoft.com/office/word/2006/wordml\" ",
    "mc:Ignorable=\"w14\">"
);

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

pub fn write(doc: &Document, pkg: Option<&DocxPackage>, path: &std::path::Path) -> Result<()> {
    let bytes = write_bytes(doc, pkg)?;
    // Write to a temp file and rename so a crash never leaves a half-written .docx.
    let tmp = path.with_extension("docx.wp-tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn write_bytes(doc: &Document, pkg: Option<&DocxPackage>) -> Result<Vec<u8>> {
    let mut pkg = pkg.cloned().unwrap_or_else(minimal_package);
    let ctx = Ctx::new(pkg.theme_major.clone(), pkg.theme_minor.clone());
    let main = render_document(doc, &pkg, &ctx);
    let main_part = pkg.main_part.clone();
    pkg.put(&main_part, main.into_bytes());
    let styles_part = pkg.styles_part.clone().unwrap_or_else(|| "word/styles.xml".to_string());
    if doc.styles.dirty || !pkg.has(&styles_part) {
        pkg.put(&styles_part, render_styles(&doc.styles, &ctx).into_bytes());
        pkg.styles_part = Some(styles_part);
    }
    pkg.to_bytes()
}

fn minimal_package() -> DocxPackage {
    let mut pkg = DocxPackage::default();
    pkg.entries.push(crate::package::PackageEntry {
        name: "[Content_Types].xml".into(),
        data: format!(
            "{}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
</Types>",
            XML_DECL
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.entries.push(crate::package::PackageEntry {
        name: "_rels/.rels".into(),
        data: format!(
            "{}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>\
</Relationships>",
            XML_DECL
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.entries.push(crate::package::PackageEntry {
        name: "word/_rels/document.xml.rels".into(),
        data: format!(
            "{}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>\
</Relationships>",
            XML_DECL
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.entries.push(crate::package::PackageEntry {
        name: "word/settings.xml".into(),
        data: format!(
            "{}<w:settings xmlns:w=\"{}\"><w:defaultTabStop w:val=\"720\"/><w:characterSpacingControl w:val=\"doNotCompress\"/><w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>",
            XML_DECL, W_NS
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.entries.push(crate::package::PackageEntry {
        name: "docProps/app.xml".into(),
        data: format!(
            "{}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>wp</Application></Properties>",
            XML_DECL
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.entries.push(crate::package::PackageEntry {
        name: "docProps/core.xml".into(),
        data: format!(
            "{}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:creator>wp</dc:creator></cp:coreProperties>",
            XML_DECL
        )
        .into_bytes(),
        deflated: true,
    });
    pkg.main_part = "word/document.xml".into();
    pkg.styles_part = Some("word/styles.xml".into());
    pkg
}

// ---------------------------------------------------------------------------
// document.xml
// ---------------------------------------------------------------------------

pub fn render_document(doc: &Document, pkg: &DocxPackage, ctx: &Ctx) -> String {
    let mut out = String::with_capacity(doc.char_count() * 2 + 4096);
    if pkg.prolog.is_empty() {
        out.push_str(XML_DECL);
    } else {
        out.push_str(&pkg.prolog);
    }
    if pkg.root_tag.is_empty() {
        out.push_str(DEFAULT_ROOT);
    } else {
        out.push_str(&pkg.root_tag);
    }
    out.push_str(&pkg.pre_body);
    out.push_str("<w:body>");
    let mut bookmark_id = BookmarkIds::new(&pkg.bookmark_ids);
    let only_placeholder = pkg.empty_body
        && doc.paragraphs.len() == 1
        && doc.paragraphs[0].items.is_empty()
        && doc.paragraphs[0].props == ParaProps::default();
    if !only_placeholder {
        for i in 0..doc.paragraphs.len() {
            render_paragraph(doc, i, &mut out, ctx, &mut bookmark_id);
        }
    }
    render_sectpr(&doc.section, &mut out);
    out.push_str("</w:body></w:document>");
    out
}

/// Bookmark ids: the original id for every bookmark read from the file, fresh
/// ids above them for new ones.
struct BookmarkIds {
    known: HashMap<String, u32>,
    next: u32,
}

impl BookmarkIds {
    fn new(original: &HashMap<String, u32>) -> BookmarkIds {
        let next = original.values().copied().max().map(|m| m + 1).unwrap_or(0);
        BookmarkIds { known: original.clone(), next }
    }
    fn id(&mut self, name: &str) -> u32 {
        if let Some(id) = self.known.get(name) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        self.known.insert(name.to_string(), id);
        id
    }
}

fn render_paragraph(doc: &Document, para: usize, out: &mut String, ctx: &Ctx, bookmark_id: &mut BookmarkIds) {
    let p = &doc.paragraphs[para];
    if p.props.raw_block {
        for it in &p.items {
            if let Item::Code(Code::Opaque(o)) = it {
                out.push_str(&o.xml);
            }
        }
        return;
    }
    out.push_str("<w:p");
    if let Some(a) = &p.props.p_attrs {
        out.push_str(a);
    }
    out.push('>');
    render_ppr(&p.props, out, ctx);
    let runs: Vec<Run> = doc.runs(para);
    let mut wrappers: Vec<(u32, bool)> = Vec::new();
    for run in &runs {
        let rpr = render_rpr_attrs(&run.attrs, ctx);
        let r_open = match run.attrs.iter().find_map(|a| if let Attr::RunAttrs(x) = a { Some(x.as_str()) } else { None }) {
            Some(a) => format!("<w:r{}>", a),
            None => "<w:r>".to_string(),
        };
        let mut open = false;
        let mut text = String::new();
        let deleted_now = |w: &Vec<(u32, bool)>| w.last().map(|x| x.1).unwrap_or(false);

        macro_rules! flush_text {
            () => {
                if !text.is_empty() {
                    if !open {
                        out.push_str(&r_open);
                        out.push_str(&rpr);
                        open = true;
                    }
                    let tag = if deleted_now(&wrappers) { "w:delText" } else { "w:t" };
                    let _ = write!(out, "<{} xml:space=\"preserve\">{}</{}>", tag, escape_text(&text), tag);
                    text.clear();
                }
            };
        }
        macro_rules! ensure_run {
            () => {
                flush_text!();
                if !open {
                    out.push_str(&r_open);
                    out.push_str(&rpr);
                    open = true;
                }
            };
        }
        macro_rules! close_run {
            () => {
                flush_text!();
                if open {
                    out.push_str("</w:r>");
                    open = false;
                }
            };
        }

        for it in &p.items[run.start..run.end] {
            match it {
                Item::Char(c) => text.push(*c),
                Item::Code(Code::On(_)) | Item::Code(Code::Off(_)) => {}
                Item::Code(Code::Tab) => {
                    ensure_run!();
                    out.push_str("<w:tab/>");
                }
                Item::Code(Code::LineBreak) => {
                    ensure_run!();
                    out.push_str("<w:br/>");
                }
                Item::Code(Code::PageBreak) => {
                    ensure_run!();
                    out.push_str("<w:br w:type=\"page\"/>");
                }
                Item::Code(Code::Bookmark(name)) => {
                    close_run!();
                    let id = bookmark_id.id(name);
                    let _ = write!(out, "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\"/>", id, escape_attr(name));
                }
                Item::Code(Code::BookmarkEnd(name)) => {
                    close_run!();
                    let id = bookmark_id.id(name);
                    let _ = write!(out, "<w:bookmarkEnd w:id=\"{}\"/>", id);
                }
                Item::Code(Code::Opaque(o)) => match o.kind {
                    OpaqueKind::Element => {
                        if is_run_level(&o.xml) {
                            ensure_run!();
                            out.push_str(&o.xml);
                        } else {
                            close_run!();
                            out.push_str(&o.xml);
                        }
                    }
                    OpaqueKind::Open(id) => {
                        close_run!();
                        out.push_str(&o.xml);
                        wrappers.push((id, o.deleted));
                    }
                    OpaqueKind::Close(id) => {
                        close_run!();
                        if let Some(pos) = wrappers.iter().rposition(|w| w.0 == id) {
                            // Close anything opened after it too (shouldn't happen, but stay well-formed).
                            wrappers.truncate(pos);
                            out.push_str(&o.xml);
                        }
                        // An unmatched close is dropped: emitting it would corrupt the XML.
                    }
                },
            }
        }
        close_run!();
        let _ = open;
    }
    // Close any wrapper left open (its close marker was deleted).
    close_unclosed_wrappers(p, out);
    out.push_str("</w:p>");
}


/// Elements that must live inside `<w:r>`.
fn is_run_level(xml: &str) -> bool {
    let tag = xml.trim_start_matches('<').split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or("");
    !matches!(
        tag,
        "w:commentRangeStart"
            | "w:commentRangeEnd"
            | "w:proofErr"
            | "w:permStart"
            | "w:permEnd"
            | "w:bookmarkStart"
            | "w:bookmarkEnd"
            | "m:oMath"
            | "m:oMathPara"
            | "w:moveFromRangeStart"
            | "w:moveFromRangeEnd"
            | "w:moveToRangeStart"
            | "w:moveToRangeEnd"
            | "w:customXmlInsRangeStart"
            | "w:customXmlInsRangeEnd"
            | "w:customXmlDelRangeStart"
            | "w:customXmlDelRangeEnd"
            | "w:ins"
            | "w:del"
            | "w:sdt"
            | "w:hyperlink"
            | "w:smartTag"
            | "w:customXml"
            | "w:fldSimple"
    )
}

/// If an Open wrapper marker survives without its Close, emit the closing
/// tag(s) so the paragraph stays well-formed.
fn close_unclosed_wrappers(p: &Paragraph, out: &mut String) {
    let mut stack: Vec<&OpaqueXml> = Vec::new();
    for it in &p.items {
        if let Item::Code(Code::Opaque(o)) = it {
            match o.kind {
                OpaqueKind::Open(_) => stack.push(o),
                OpaqueKind::Close(id) => {
                    if let Some(pos) = stack.iter().rposition(|x| x.kind == OpaqueKind::Open(id)) {
                        stack.truncate(pos);
                    }
                }
                OpaqueKind::Element => {}
            }
        }
    }
    for o in stack.iter().rev() {
        let tag = o.xml.trim_start_matches('<').split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("");
        if tag == "w:sdt" {
            out.push_str("</w:sdtContent></w:sdt>");
        } else if !tag.is_empty() {
            let _ = write!(out, "</{}>", tag);
        }
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Render `w:rPr` for a run's direct attributes, reusing the preserved XML
/// when nothing modelled has changed.
pub fn render_rpr_attrs(attrs: &[Attr], ctx: &Ctx) -> String {
    let raw = attrs.iter().find_map(|a| if let Attr::Raw(x) = a { Some(x.as_str()) } else { None });
    let known: Vec<&Attr> = attrs.iter().filter(|a| !matches!(a, Attr::Raw(_) | Attr::RunAttrs(_))).collect();
    let mut children: Vec<RawChild> = Vec::new();
    let mut covered: Vec<AttrKind> = Vec::new();
    if let Some(raw) = raw {
        let parsed = parse_rpr(raw, ctx);
        let raw_known: Vec<&Attr> = parsed.iter().filter_map(|(_, a)| a.as_ref()).collect();
        if same_attr_set(&raw_known, &known) {
            return raw.to_string();
        }
        for (child, a) in parsed {
            match a {
                None => children.push(child),
                Some(a) => {
                    if known.iter().any(|k| **k == a) {
                        covered.push(a.kind());
                        children.push(child);
                    }
                }
            }
        }
    }
    for a in &known {
        if covered.contains(&a.kind()) {
            continue;
        }
        if let Some(c) = attr_element(a) {
            children.push(c);
        }
    }
    if children.is_empty() {
        return String::new();
    }
    sort_children(RPR_ORDER, &mut children);
    let mut out = String::from("<w:rPr>");
    for c in children {
        out.push_str(&c.xml);
    }
    out.push_str("</w:rPr>");
    out
}

fn same_attr_set(a: &[&Attr], b: &[&Attr]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

fn onoff_el(tag: &str, on: bool) -> RawChild {
    RawChild { tag: tag.into(), xml: if on { format!("<{}/>", tag) } else { format!("<{} w:val=\"0\"/>", tag) } }
}

fn attr_element(a: &Attr) -> Option<RawChild> {
    Some(match a {
        Attr::Bold(b) => onoff_el("w:b", *b),
        Attr::Italic(b) => onoff_el("w:i", *b),
        Attr::Underline(u) => RawChild { tag: "w:u".into(), xml: format!("<w:u w:val=\"{}\"/>", u.docx_name()) },
        Attr::Strike(b) => onoff_el("w:strike", *b),
        Attr::DoubleStrike(b) => onoff_el("w:dstrike", *b),
        Attr::VertAlign(v) => RawChild {
            tag: "w:vertAlign".into(),
            xml: format!(
                "<w:vertAlign w:val=\"{}\"/>",
                match v {
                    VertAlign::Superscript => "superscript",
                    VertAlign::Subscript => "subscript",
                    VertAlign::Baseline => "baseline",
                }
            ),
        },
        Attr::SmallCaps(b) => onoff_el("w:smallCaps", *b),
        Attr::AllCaps(b) => onoff_el("w:caps", *b),
        Attr::Font(f) => RawChild {
            tag: "w:rFonts".into(),
            xml: format!("<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:cs=\"{0}\"/>", escape_attr(f)),
        },
        Attr::Size(s) => RawChild { tag: "w:sz".into(), xml: format!("<w:sz w:val=\"{0}\"/><w:szCs w:val=\"{0}\"/>", s) },
        Attr::Color(c) => RawChild { tag: "w:color".into(), xml: format!("<w:color w:val=\"{}\"/>", c.hex()) },
        Attr::Highlight(h) => RawChild { tag: "w:highlight".into(), xml: format!("<w:highlight w:val=\"{}\"/>", h.docx_name()) },
        Attr::CharStyle(s) => RawChild { tag: "w:rStyle".into(), xml: format!("<w:rStyle w:val=\"{}\"/>", escape_attr(s)) },
        Attr::Raw(_) | Attr::RunAttrs(_) => return None,
    })
}

/// Render `w:rPr` from resolved-style-like `RunProps` (styles, paragraph marks).
pub fn render_run_props(r: &RunProps) -> String {
    let mut children: Vec<RawChild> = Vec::new();
    if let Some(s) = &r.char_style {
        children.push(attr_element(&Attr::CharStyle(s.clone())).unwrap());
    }
    if let Some(f) = &r.font {
        children.push(attr_element(&Attr::Font(f.clone())).unwrap());
    }
    if let Some(b) = r.bold {
        children.push(onoff_el("w:b", b));
    }
    if let Some(b) = r.italic {
        children.push(onoff_el("w:i", b));
    }
    if let Some(b) = r.all_caps {
        children.push(onoff_el("w:caps", b));
    }
    if let Some(b) = r.small_caps {
        children.push(onoff_el("w:smallCaps", b));
    }
    if let Some(b) = r.strike {
        children.push(onoff_el("w:strike", b));
    }
    if let Some(b) = r.dstrike {
        children.push(onoff_el("w:dstrike", b));
    }
    if let Some(c) = r.color {
        children.push(attr_element(&Attr::Color(c)).unwrap());
    }
    if let Some(s) = r.size {
        children.push(attr_element(&Attr::Size(s)).unwrap());
    }
    if let Some(h) = r.highlight {
        children.push(attr_element(&Attr::Highlight(h.unwrap_or(Highlight::None))).unwrap());
    }
    if let Some(u) = r.underline {
        children.push(attr_element(&Attr::Underline(u.unwrap_or(Underline::None))).unwrap());
    }
    if let Some(v) = r.vert_align {
        children.push(attr_element(&Attr::VertAlign(v.unwrap_or(VertAlign::Baseline))).unwrap());
    }
    for o in &r.opaque {
        let tag = start_tag(o).map(|e| String::from_utf8_lossy(e.name().as_ref()).into_owned()).unwrap_or_default();
        children.push(RawChild { tag, xml: o.clone() });
    }
    if children.is_empty() {
        return String::new();
    }
    sort_children(RPR_ORDER, &mut children);
    let mut out = String::from("<w:rPr>");
    for c in children {
        out.push_str(&c.xml);
    }
    out.push_str("</w:rPr>");
    out
}

pub fn render_ppr(p: &ParaProps, out: &mut String, _ctx: &Ctx) {
    if let Some(raw) = &p.raw_ppr {
        out.push_str(raw);
        return;
    }
    let body = render_ppr_body(p);
    if body.is_empty() {
        return;
    }
    out.push_str("<w:pPr>");
    out.push_str(&body);
    out.push_str("</w:pPr>");
}

pub fn render_ppr_body(p: &ParaProps) -> String {
    let mut children: Vec<RawChild> = Vec::new();
    let mut push = |tag: &str, xml: String| children.push(RawChild { tag: tag.into(), xml });
    if let Some(s) = &p.style {
        push("w:pStyle", format!("<w:pStyle w:val=\"{}\"/>", escape_attr(s)));
    }
    if let Some(b) = p.keep_next {
        push("w:keepNext", onoff_el("w:keepNext", b).xml);
    }
    if let Some(b) = p.keep_lines {
        push("w:keepLines", onoff_el("w:keepLines", b).xml);
    }
    if let Some(b) = p.page_break_before {
        push("w:pageBreakBefore", onoff_el("w:pageBreakBefore", b).xml);
    }
    if let Some(b) = p.widow_control {
        push("w:widowControl", onoff_el("w:widowControl", b).xml);
    }
    if let Some(l) = p.list {
        push("w:numPr", format!("<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{}\"/></w:numPr>", l.level, l.num_id));
    }
    if let Some(b) = &p.borders {
        let mut s = String::from("<w:pBdr>");
        for (tag, bd) in [("w:top", &b.top), ("w:left", &b.left), ("w:bottom", &b.bottom), ("w:right", &b.right)] {
            if let Some(bd) = bd {
                let val = match bd.style {
                    BorderStyle::Single => "single",
                    BorderStyle::Double => "double",
                    BorderStyle::Dotted => "dotted",
                    BorderStyle::Dashed => "dashed",
                    BorderStyle::Thick => "thick",
                };
                let color = bd.color.map(|c| c.hex()).unwrap_or_else(|| "auto".into());
                let _ = write!(s, "<{} w:val=\"{}\" w:sz=\"{}\" w:space=\"{}\" w:color=\"{}\"/>", tag, val, bd.size, bd.space, color);
            }
        }
        s.push_str("</w:pBdr>");
        push("w:pBdr", s);
    }
    if let Some(c) = p.shading {
        push("w:shd", format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", c.hex()));
    }
    if !p.tabs.is_empty() {
        let mut s = String::from("<w:tabs>");
        for t in &p.tabs {
            let val = if t.clear {
                "clear"
            } else {
                match t.kind {
                    TabKind::Left => "left",
                    TabKind::Center => "center",
                    TabKind::Right => "right",
                    TabKind::Decimal => "decimal",
                    TabKind::Bar => "bar",
                }
            };
            let _ = write!(s, "<w:tab w:val=\"{}\"", val);
            match t.leader {
                TabLeader::None => {}
                TabLeader::Dot => s.push_str(" w:leader=\"dot\""),
                TabLeader::Hyphen => s.push_str(" w:leader=\"hyphen\""),
                TabLeader::Underscore => s.push_str(" w:leader=\"underscore\""),
            }
            let _ = write!(s, " w:pos=\"{}\"/>", t.pos);
        }
        s.push_str("</w:tabs>");
        push("w:tabs", s);
    }
    if p.space_before.is_some() || p.space_after.is_some() || p.line_spacing.is_some() {
        let mut s = String::from("<w:spacing");
        if let Some(v) = p.space_before {
            twips_attr(&mut s, "w:before", v);
        }
        if let Some(v) = p.space_after {
            twips_attr(&mut s, "w:after", v);
        }
        match p.line_spacing {
            Some(LineSpacing::Auto(v)) => {
                twips_attr(&mut s, "w:line", v);
                s.push_str(" w:lineRule=\"auto\"");
            }
            Some(LineSpacing::Exact(v)) => {
                twips_attr(&mut s, "w:line", v);
                s.push_str(" w:lineRule=\"exact\"");
            }
            Some(LineSpacing::AtLeast(v)) => {
                twips_attr(&mut s, "w:line", v);
                s.push_str(" w:lineRule=\"atLeast\"");
            }
            None => {}
        }
        s.push_str("/>");
        push("w:spacing", s);
    }
    if p.indent_left.is_some() || p.indent_right.is_some() || p.first_line.is_some() || p.hanging.is_some() {
        let mut s = String::from("<w:ind");
        if let Some(v) = p.indent_left {
            twips_attr(&mut s, "w:left", v);
        }
        if let Some(v) = p.indent_right {
            twips_attr(&mut s, "w:right", v);
        }
        if let Some(v) = p.hanging {
            twips_attr(&mut s, "w:hanging", v);
        } else if let Some(v) = p.first_line {
            twips_attr(&mut s, "w:firstLine", v);
        }
        s.push_str("/>");
        push("w:ind", s);
    }
    if let Some(a) = p.align {
        push("w:jc", format!("<w:jc w:val=\"{}\"/>", a.docx_name()));
    }
    if let Some(l) = p.outline_level {
        push("w:outlineLvl", format!("<w:outlineLvl w:val=\"{}\"/>", l));
    }
    let mark = render_run_props(&p.mark);
    if !mark.is_empty() {
        push("w:rPr", mark);
    }
    if let Some(s) = &p.sect_break {
        push("w:sectPr", s.clone());
    }
    for o in &p.opaque {
        let tag = start_tag(o).map(|e| String::from_utf8_lossy(e.name().as_ref()).into_owned()).unwrap_or_default();
        // Skip opaque copies of elements we regenerate (ind/spacing with *Chars attrs).
        if (tag == "w:ind" || tag == "w:spacing" || tag == "w:shd" || tag == "w:numPr") && children.iter().any(|c| c.tag == tag) {
            continue;
        }
        children.push(RawChild { tag, xml: o.clone() });
    }
    sort_children(PPR_ORDER, &mut children);
    let mut out = String::new();
    for c in children {
        out.push_str(&c.xml);
    }
    out
}

fn render_sectpr(s: &SectionProps, out: &mut String) {
    out.push_str("<w:sectPr");
    out.push_str(&s.attrs);
    out.push('>');

    let mut children: Vec<String> = s.opaque_children.clone();
    let original = if children.is_empty() {
        None
    } else {
        Some(parse_sectpr(&format!("<w:sectPr>{}</w:sectPr>", children.concat())))
    };
    let geometry_unchanged = original.as_ref().map(|o| {
        o.page_width == s.page_width
            && o.page_height == s.page_height
            && o.orientation == s.orientation
            && o.margin_top == s.margin_top
            && o.margin_bottom == s.margin_bottom
            && o.margin_left == s.margin_left
            && o.margin_right == s.margin_right
            && o.header_distance == s.header_distance
            && o.footer_distance == s.footer_distance
            && o.gutter == s.gutter
            && o.columns == s.columns
    });
    if geometry_unchanged != Some(true) {
        let pgsz = format!(
            "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/>",
            s.page_width,
            s.page_height,
            if s.orientation == Orientation::Landscape { " w:orient=\"landscape\"" } else { "" }
        );
        let pgmar = format!(
            "<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"{}\"/>",
            s.margin_top, s.margin_right, s.margin_bottom, s.margin_left, s.header_distance, s.footer_distance, s.gutter
        );
        let cols = format!("<w:cols w:num=\"{}\" w:space=\"720\"/>", s.columns);
        let mut replaced = [false; 3];
        for c in children.iter_mut() {
            let tag = start_tag(c).map(|e| String::from_utf8_lossy(e.name().as_ref()).into_owned()).unwrap_or_default();
            match tag.as_str() {
                "w:pgSz" => {
                    *c = pgsz.clone();
                    replaced[0] = true;
                }
                "w:pgMar" => {
                    *c = pgmar.clone();
                    replaced[1] = true;
                }
                "w:cols" => {
                    if s.columns != 1 || original.as_ref().map(|o| o.columns) != Some(1) {
                        *c = cols.clone();
                    }
                    replaced[2] = true;
                }
                _ => {}
            }
        }
        // Insert missing geometry in schema order: pgSz, pgMar come after headers/footers refs.
        let mut extra = Vec::new();
        if !replaced[0] {
            extra.push(pgsz);
        }
        if !replaced[1] {
            extra.push(pgmar);
        }
        if !replaced[2] && s.columns != 1 {
            extra.push(cols);
        }
        // Place after any header/footer references and type.
        let idx = children
            .iter()
            .position(|c| {
                let t = start_tag(c).map(|e| e.name().as_ref().to_vec()).unwrap_or_default();
                !matches!(t.as_slice(), b"w:headerReference" | b"w:footerReference" | b"w:footnotePr" | b"w:endnotePr" | b"w:type")
            })
            .unwrap_or(children.len());
        for (i, e) in extra.into_iter().enumerate() {
            children.insert(idx + i, e);
        }
        if original.is_none() {
            children.push("<w:docGrid w:linePitch=\"360\"/>".into());
        }
    }
    for c in children {
        out.push_str(&c);
    }
    out.push_str("</w:sectPr>");
}

// ---------------------------------------------------------------------------
// styles.xml
// ---------------------------------------------------------------------------

pub fn render_styles(sheet: &StyleSheet, _ctx: &Ctx) -> String {
    let mut out = String::from(XML_DECL);
    let mut opaque = sheet.opaque.iter();
    let root = opaque.next().filter(|s| s.starts_with("<w:styles"));
    match root {
        Some(r) => out.push_str(r),
        None => {
            let _ = write!(
                out,
                "<w:styles xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:w=\"{}\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" mc:Ignorable=\"w14\">",
                W_NS
            );
        }
    }
    let rest: Vec<&String> = if root.is_some() { opaque.collect() } else { sheet.opaque.iter().collect() };
    let has_defaults = rest.iter().any(|s| s.starts_with("<w:docDefaults"));
    if !has_defaults {
        out.push_str("<w:docDefaults><w:rPrDefault>");
        out.push_str(&render_run_props(&sheet.default_run));
        out.push_str("</w:rPrDefault><w:pPrDefault>");
        let body = render_ppr_body(&sheet.default_para);
        if !body.is_empty() {
            let _ = write!(out, "<w:pPr>{}</w:pPr>", body);
        }
        out.push_str("</w:pPrDefault></w:docDefaults>");
    }
    for s in rest {
        out.push_str(s);
    }
    for st in &sheet.styles {
        if let Some(raw) = &st.raw_xml {
            out.push_str(raw);
            continue;
        }
        let ty = match st.kind {
            StyleKind::Paragraph => "paragraph",
            StyleKind::Character => "character",
            StyleKind::Table => "table",
            StyleKind::Numbering => "numbering",
        };
        let _ = write!(out, "<w:style w:type=\"{}\"{} w:styleId=\"{}\">", ty, if st.is_default { " w:default=\"1\"" } else { "" }, escape_attr(&st.id));
        let _ = write!(out, "<w:name w:val=\"{}\"/>", escape_attr(&st.name));
        if let Some(b) = &st.based_on {
            let _ = write!(out, "<w:basedOn w:val=\"{}\"/>", escape_attr(b));
        }
        if let Some(n) = &st.next {
            let _ = write!(out, "<w:next w:val=\"{}\"/>", escape_attr(n));
        }
        if st.hidden {
            out.push_str("<w:semiHidden/>");
        } else {
            out.push_str("<w:qFormat/>");
        }
        let body = render_ppr_body(&st.para);
        if !body.is_empty() {
            let _ = write!(out, "<w:pPr>{}</w:pPr>", body);
        }
        out.push_str(&render_run_props(&st.run));
        out.push_str("</w:style>");
    }
    out.push_str("</w:styles>");
    out
}
