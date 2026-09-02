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
    let styles_part = pkg.styles_part.clone().unwrap_or_else(|| {
        let dir = pkg.main_part.rsplit_once('/').map(|(d, _)| format!("{}/", d)).unwrap_or_default();
        format!("{}styles.xml", dir)
    });
    // A file that never had a styles part keeps not having one, unless a
    // style was added or changed — then the part is created and registered.
    if doc.styles.dirty || (!pkg.has(&styles_part) && pkg.styles_part.is_some()) {
        pkg.put(&styles_part, render_styles(&doc.styles, &ctx).into_bytes());
        register_part(&mut pkg, &styles_part, "styles", "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml");
        pkg.styles_part = Some(styles_part);
    }
    for rel in &doc.extra_rels {
        add_relationship(&mut pkg, &rel.id, &rel.kind, &rel.target, rel.external);
    }
    if !doc.footnotes.is_empty() && pkg.footnotes_part.is_none() {
        let part = {
            let dir = pkg.main_part.rsplit_once('/').map(|(d, _)| format!("{}/", d)).unwrap_or_default();
            format!("{}footnotes.xml", dir)
        };
        pkg.put(&part, render_footnotes(doc, &ctx).into_bytes());
        register_part(&mut pkg, &part, "footnotes", "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml");
        pkg.footnotes_part = Some(part);
    }
    write_headers(doc, &mut pkg, &ctx);
    if doc.numbering.dirty {
        let part = pkg.numbering_part.clone().unwrap_or_else(|| {
            let dir = pkg.main_part.rsplit_once('/').map(|(d, _)| format!("{}/", d)).unwrap_or_default();
            format!("{}numbering.xml", dir)
        });
        pkg.put(&part, render_numbering(&doc.numbering).into_bytes());
        register_part(&mut pkg, &part, "numbering", "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml");
        pkg.numbering_part = Some(part);
    }
    pkg.to_bytes()

}

const HDR_ROOT: &str = concat!(
    "<w:hdr xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
    "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
    "xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" ",
    "xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"w14\">"
);

/// Write every header and footer body: untouched ones are already in the
/// package verbatim; edited or new ones are rendered into their part (a
/// new part is created and related for a body that has none yet). The
/// odd/even setting goes into the settings part.
fn write_headers(doc: &Document, pkg: &mut DocxPackage, ctx: &Ctx) {
    for (id, hf) in &doc.headers {
        let kind = hf.kind.unwrap_or(HfKind::Header);
        let (stem, rel, ct) = match kind {
            HfKind::Header => ("header", "header", "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"),
            HfKind::Footer => ("footer", "footer", "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"),
        };
        let part = match &hf.part {
            Some(p) => p.clone(),
            None => pkg.free_part_name(stem),
        };
        if hf.raw.is_some() && pkg.has(&part) {
            continue;
        }
        let scratch = wp_core::section::scratch_doc(doc, &hf.paragraphs);
        let mut out = String::new();
        out.push_str(XML_DECL);
        match &hf.root_tag {
            Some(t) => out.push_str(t),
            None => out.push_str(&HDR_ROOT.replace("w:hdr", if kind == HfKind::Footer { "w:ftr" } else { "w:hdr" })),
        }
        let mut ids = BookmarkIds::new(&pkg.bookmark_ids);
        for i in 0..scratch.paragraphs.len() {
            render_paragraph(&scratch, i, &mut out, ctx, &mut ids);
        }
        out.push_str(if kind == HfKind::Footer { "</w:ftr>" } else { "</w:hdr>" });
        pkg.put(&part, out.into_bytes());
        ensure_content_type(pkg, &part, ct);
        let dir = pkg.main_part.rsplit_once('/').map(|(d, _)| format!("{}/", d)).unwrap_or_default();
        let target = part.strip_prefix(&dir).unwrap_or(&part).to_string();
        add_relationship(pkg, id, rel, &target, false);
    }
    // Odd/even headers: a document setting.
    if let Some(part) = pkg.settings_part.clone().or_else(|| if pkg.has("word/settings.xml") { Some("word/settings.xml".into()) } else { None }) {
        if let Some(xml) = pkg.get_str(&part) {
            let has = children_of(&xml).iter().any(|c| c.tag == "w:evenAndOddHeaders" && start_tag(&c.xml).map_or(true, |t| attr_bool(&t, "w:val")));
            if has != doc.even_odd_headers {
                let mut children = children_of(&xml);
                children.retain(|c| c.tag != "w:evenAndOddHeaders");
                if doc.even_odd_headers {
                    let at = children.iter().position(|c| SETTINGS_AFTER_EVEN_ODD.contains(&c.tag.as_str())).unwrap_or(children.len());
                    children.insert(at, RawChild { tag: "w:evenAndOddHeaders".into(), xml: "<w:evenAndOddHeaders/>".into() });
                }
                let root_start = xml.find("<w:settings").unwrap_or(0);
                let root_end = xml[root_start..].find('>').map(|i| root_start + i + 1).unwrap_or(root_start);
                let mut new = String::new();
                new.push_str(&xml[..root_end]);
                for c in children {
                    new.push_str(&c.xml);
                }
                new.push_str("</w:settings>");
                pkg.put(&part, new.into_bytes());
            }
        }
    }
}

/// Settings children that come after `w:evenAndOddHeaders` in CT_Settings.
const SETTINGS_AFTER_EVEN_ODD: &[&str] = &[
    "w:bookFoldRevPrinting", "w:bookFoldPrinting", "w:bookFoldPrintingSheets", "w:drawingGridHorizontalSpacing",
    "w:drawingGridVerticalSpacing", "w:displayHorizontalDrawingGridEvery", "w:displayVerticalDrawingGridEvery",
    "w:doNotUseMarginsForDrawingGridOrigin", "w:drawingGridHorizontalOrigin", "w:drawingGridVerticalOrigin",
    "w:doNotShadeFormData", "w:noPunctuationKerning", "w:characterSpacingControl", "w:printTwoOnOne",
    "w:strictFirstAndLastChars", "w:noLineBreaksAfter", "w:noLineBreaksBefore", "w:savePreviewPicture",
    "w:doNotValidateAgainstSchema", "w:saveInvalidXml", "w:ignoreMixedContent", "w:alwaysShowPlaceholderText",
    "w:doNotDemarcateInvalidXml", "w:saveXmlDataOnly", "w:useXSLTWhenSaving", "w:saveThroughXslt",
    "w:showXMLTags", "w:alwaysMergeEmptyNamespace", "w:updateFields", "w:hdrShapeDefaults", "w:footnotePr",
    "w:endnotePr", "w:compat", "w:docVars", "w:rsids", "m:mathPr", "w:attachedSchema", "w:themeFontLang",
    "w:clrSchemeMapping", "w:doNotIncludeSubdocsInStats", "w:doNotAutoCompressPictures", "w:forceUpgrade",
    "w:captions", "w:readModeInkLockDown", "w:smartTagType", "sl:schemaLibrary", "w:shapeDefaults",
    "w:doNotEmbedSmartTags", "w:decimalSymbol", "w:listSeparator",
];

/// Make sure the content types part names `part`'s type.
pub fn ensure_content_type(pkg: &mut DocxPackage, part: &str, content_type: &str) {
    let ct_name = "[Content_Types].xml";
    let ct = pkg.get_str(ct_name).unwrap_or_else(|| format!("{}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"></Types>", XML_DECL));
    let part_name = format!("/{}", part);
    if !ct.contains(&format!("PartName=\"{}\"", part_name)) {
        let ov = format!("<Override PartName=\"{}\" ContentType=\"{}\"/>", part_name, content_type);
        let new = match ct.rfind("</Types>") {
            Some(i) => format!("{}{}{}", &ct[..i], ov, &ct[i..]),
            None => ct,
        };
        pkg.put(ct_name, new.into_bytes());
    }
}

/// Add a relationship of the main part if no relationship with that id exists.
pub fn add_relationship(pkg: &mut DocxPackage, id: &str, rel_type: &str, target: &str, external: bool) {
    let rels_name = pkg.main_rels_name();
    let rels = pkg.get_str(&rels_name).unwrap_or_else(|| format!("{}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>", XML_DECL));
    if rels.contains(&format!("Id=\"{}\"", id)) {
        return;
    }
    let ty = format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/{}", rel_type);
    let rel = format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>", escape_attr(id), ty, escape_attr(target), if external { " TargetMode=\"External\"" } else { "" });
    let new = match rels.rfind("</Relationships>") {
        Some(i) => format!("{}{}{}", &rels[..i], rel, &rels[i..]),
        None => rels,
    };
    pkg.put(&rels_name, new.into_bytes());
}

/// One paragraph as WordprocessingML (for table cells and footnotes built
/// outside the main body).
pub fn render_paragraph_xml(doc: &Document, para: usize, ctx: &Ctx) -> String {
    let mut out = String::new();
    let mut ids = BookmarkIds::new(&HashMap::new());
    render_paragraph(doc, para, &mut out, ctx, &mut ids);
    out
}

fn render_footnotes(doc: &Document, ctx: &Ctx) -> String {
    let mut out = String::from(XML_DECL);
    let _ = write!(out, "<w:footnotes xmlns:w=\"{}\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">", W_NS);
    out.push_str("<w:footnote w:type=\"separator\" w:id=\"-1\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote>");
    out.push_str("<w:footnote w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>");
    for fnote in &doc.footnotes {
        let _ = write!(out, "<w:footnote w:id=\"{}\">", fnote.id);
        let mut tmp = Document::new();
        tmp.styles = doc.styles.clone();
        tmp.paragraphs = fnote.paragraphs.clone();
        for (i, p) in tmp.paragraphs.iter_mut().enumerate() {
            let mut props = p.props.clone();
            if props.space_after.is_none() {
                props.space_after = Some(0);
                props.line_spacing = Some(LineSpacing::Auto(240));
            }
            if props.mark.size.is_none() {
                props.mark.size = Some(20);
            }
            props.touch();
            p.props = props;
            if i == 0 {
                let mut lead = vec![
                    Item::Code(Code::On(Attr::VertAlign(VertAlign::Superscript))),
                    Item::Code(Code::Opaque(OpaqueXml::element("<w:footnoteRef/>", "Note Ref"))),
                    Item::Code(Code::Off(AttrKind::VertAlign)),
                    Item::Char(' '),
                ];
                lead.append(&mut p.items);
                p.items = lead;
            }
        }
        for i in 0..tmp.paragraphs.len() {
            out.push_str(&render_paragraph_xml(&tmp, i, ctx));
        }
        out.push_str("</w:footnote>");
    }
    out.push_str("</w:footnotes>");
    out
}

/// Make sure a part is reachable: a content-type override and a relationship
/// from the main part. Both are inserted only when missing.
pub fn register_part(pkg: &mut DocxPackage, part: &str, rel_type: &str, content_type: &str) {

    let ct_name = "[Content_Types].xml";
    let ct = pkg.get_str(ct_name).unwrap_or_else(|| format!("{}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"></Types>", XML_DECL));
    let part_name = format!("/{}", part);
    if !ct.contains(&format!("PartName=\"{}\"", part_name)) {
        let ov = format!("<Override PartName=\"{}\" ContentType=\"{}\"/>", part_name, content_type);
        let new = match ct.rfind("</Types>") {
            Some(i) => format!("{}{}{}", &ct[..i], ov, &ct[i..]),
            None => ct,
        };
        pkg.put(ct_name, new.into_bytes());
    }
    let (dir, file) = pkg.main_part.rsplit_once('/').map(|(d, f)| (d.to_string(), f.to_string())).unwrap_or((String::new(), pkg.main_part.clone()));
    let rels_name = if dir.is_empty() { format!("_rels/{}.rels", file) } else { format!("{}/_rels/{}.rels", dir, file) };
    let target = part.strip_prefix(&format!("{}/", dir)).unwrap_or(part).to_string();
    let rels = pkg.get_str(&rels_name).unwrap_or_else(|| format!("{}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>", XML_DECL));
    let ty = format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/{}", rel_type);
    if !rels.contains(&format!("Type=\"{}\"", ty)) {
        let mut n = 1;
        while rels.contains(&format!("Id=\"rId{}\"", n)) {
            n += 1;
        }
        let rel = format!("<Relationship Id=\"rId{}\" Type=\"{}\" Target=\"{}\"/>", n, ty, escape_attr(&target));
        let new = match rels.rfind("</Relationships>") {
            Some(i) => format!("{}{}{}", &rels[..i], rel, &rels[i..]),
            None => rels,
        };
        pkg.put(&rels_name, new.into_bytes());
    }
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
    pkg.had_sectpr = true;
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
        let mut cur: Option<CellRef> = None;
        let mut table: Option<Table> = None;
        for i in 0..doc.paragraphs.len() {
            let cell = doc.paragraphs[i].props.cell;
            if cell != cur {
                // Close what the previous paragraph was in, open what this
                // one is in: cell, row, table — innermost first.
                if let Some(c) = cur {
                    out.push_str("</w:tc>");
                    if cell.map_or(true, |n| n.table != c.table || n.row != c.row) {
                        out.push_str("</w:tr>");
                    }
                    if cell.map_or(true, |n| n.table != c.table) {
                        out.push_str("</w:tbl>");
                        table = None;
                    }
                }
                if let Some(n) = cell {
                    if table.is_none() {
                        emit_lead_trail(&doc.paragraphs[i], &mut out, OpaqueLevel::Body, true);
                        let t = crate::table::table_for_write(doc, n.table);
                        crate::table::open_table(&t, &mut out);
                        table = Some(t);
                    }
                    let t = table.as_ref().unwrap();
                    if cur.map_or(true, |c| c.table != n.table || c.row != n.row) {
                        crate::table::open_row(t, n.row as usize, &mut out);
                    }
                    crate::table::open_cell(t, n.row as usize, n.col as usize, &mut out);
                }
                cur = cell;
            }
            let last_of_table = cell.is_some() && doc.paragraphs.get(i + 1).map_or(true, |p| p.props.cell.map_or(true, |n| n.table != cell.unwrap().table));
            let first_of_table = cell.is_some() && (i == 0 || doc.paragraphs[i - 1].props.cell.map_or(true, |p| p.table != cell.unwrap().table));
            render_paragraph_in(doc, i, &mut out, ctx, &mut bookmark_id, first_of_table, last_of_table);
        }
        if cur.is_some() {
            out.push_str("</w:tc></w:tr></w:tbl>");
            if let Some(p) = doc.paragraphs.last() {
                emit_lead_trail(p, &mut out, OpaqueLevel::Body, false);
            }
        }
    }
    if pkg.had_sectpr || doc.section != SectionProps::default() {
        render_sectpr(&doc.section, &mut out);
    }
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

/// The leading (or trailing) opaque elements of a paragraph that sat next to
/// it rather than in it, emitted verbatim. Body-level ones on a table's
/// first/last paragraph belong outside the table.
fn emit_lead_trail(p: &Paragraph, out: &mut String, level: OpaqueLevel, lead: bool) {
    let is_next = |it: &Item| matches!(it, Item::Code(Code::Opaque(o)) if o.kind == OpaqueKind::Element && matches!(o.level, OpaqueLevel::Body | OpaqueLevel::Cell));
    let n_lead = p.items.iter().take_while(|it| is_next(it)).count();
    let n_trail = if n_lead == p.items.len() { 0 } else { p.items.iter().rev().take_while(|it| is_next(it)).count() };
    let items = if lead { &p.items[..n_lead] } else { &p.items[p.items.len() - n_trail..] };
    for it in items {
        if let Item::Code(Code::Opaque(o)) = it {
            if o.level == level {
                out.push_str(&o.xml);
            }
        }
    }
}

fn render_paragraph(doc: &Document, para: usize, out: &mut String, ctx: &Ctx, bookmark_id: &mut BookmarkIds) {
    render_paragraph_in(doc, para, out, ctx, bookmark_id, false, false)
}

/// `outer_lead` / `outer_trail`: the paragraph is the first / last of a table,
/// so its body-level neighbours were (or will be) emitted around the table.
fn render_paragraph_in(doc: &Document, para: usize, out: &mut String, ctx: &Ctx, bookmark_id: &mut BookmarkIds, outer_lead: bool, outer_trail: bool) {
    let p = &doc.paragraphs[para];
    if p.props.raw_block {
        let mut any = false;
        for it in &p.items {
            if let Item::Code(Code::Opaque(o)) = it {
                out.push_str(&o.xml);
                any = true;
            }
        }
        if !any && p.props.cell.is_some() {
            out.push_str("<w:p/>"); // a cell must end with a paragraph
        }
        return;
    }
    // Elements that sat next to the paragraph in the body (a comment range
    // start, a bookmark before a table) go back outside it.
    let is_body = |it: &Item| matches!(it, Item::Code(Code::Opaque(o)) if o.kind == OpaqueKind::Element && matches!(o.level, OpaqueLevel::Body | OpaqueLevel::Cell));
    let lead = p.items.iter().take_while(|it| is_body(it)).count();
    let trail = if lead == p.items.len() { 0 } else { p.items.iter().rev().take_while(|it| is_body(it)).count() };
    for it in &p.items[..lead] {
        if let Item::Code(Code::Opaque(o)) = it {
            if !(outer_lead && o.level == OpaqueLevel::Body) {
                out.push_str(&o.xml);
            }
        }
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

        for it in &p.items[run.start.max(lead)..run.end.min(p.items.len() - trail).max(run.start.max(lead))] {
            match it {
                Item::Char('\u{ad}') => {
                    ensure_run!();
                    out.push_str("<w:softHyphen/>");
                }
                Item::Char('\u{2011}') => {
                    ensure_run!();
                    out.push_str("<w:noBreakHyphen/>");
                }
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
                Item::Code(Code::ColumnBreak) => {
                    ensure_run!();
                    out.push_str("<w:br w:type=\"column\"/>");
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
                        if o.level == OpaqueLevel::Run && is_run_level(&o.xml) {
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
    for it in &p.items[p.items.len() - trail..] {
        if let Item::Code(Code::Opaque(o)) = it {
            if !(outer_trail && o.level == OpaqueLevel::Body) {
                out.push_str(&o.xml);
            }
        }
    }
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
                s.push_str(&border_xml(tag, bd));
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
        let mut xml = String::new();
        render_sectpr(s, &mut xml);
        push("w:sectPr", xml);
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

/// Schema order of `w:sectPr` children (CT_SectPr).
const SECTPR_ORDER: &[&str] = &[
    "w:headerReference", "w:footerReference", "w:footnotePr", "w:endnotePr", "w:type", "w:pgSz", "w:pgMar",
    "w:paperSrc", "w:pgBorders", "w:lnNumType", "w:pgNumType", "w:cols", "w:formProt", "w:vAlign", "w:noEndnote",
    "w:titlePg", "w:textDirection", "w:bidi", "w:rtlGutter", "w:docGrid", "w:printerSettings", "w:sectPrChange",
];

/// One border element.
pub fn border_xml(tag: &str, bd: &Border) -> String {
    if bd.style == BorderStyle::None {
        return format!("<{} w:val=\"nil\"/>", tag);
    }
    let color = bd.color.map(|c| c.hex()).unwrap_or_else(|| "auto".into());
    format!("<{} w:val=\"{}\" w:sz=\"{}\" w:space=\"{}\" w:color=\"{}\"/>", tag, bd.style.docx_name(), bd.size, bd.space, color)
}

/// Emit a `w:sectPr`: the children as read, with the modelled ones
/// (page geometry, section start, title page, header and footer
/// references) regenerated only when they differ from what was read.
pub fn render_sectpr(s: &SectionProps, out: &mut String) {
    out.push_str("<w:sectPr");
    out.push_str(&s.attrs);
    out.push('>');

    let mut children: Vec<RawChild> = s
        .opaque_children
        .iter()
        .map(|c| RawChild { tag: start_tag(c).map(|e| String::from_utf8_lossy(e.name().as_ref()).into_owned()).unwrap_or_default(), xml: c.clone() })
        .collect();
    let original = if children.is_empty() { None } else { Some(parse_sectpr(&format!("<w:sectPr>{}</w:sectPr>", s.opaque_children.concat()))) };
    let geometry_unchanged = original.as_ref().map_or(false, |o| o.same_geometry(s));
    let start_unchanged = original.as_ref().map_or(s.start == SectionStart::NextPage, |o| o.start == s.start);
    let title_unchanged = original.as_ref().map_or(!s.title_page, |o| o.title_page == s.title_page);
    let hf_unchanged = original.as_ref().map_or(s.hf.is_empty(), |o| o.hf == s.hf);
    let page_start_unchanged = original.as_ref().map_or(s.page_start.is_none(), |o| o.page_start == s.page_start);

    // Replace a modelled child, or add it (schema order is restored below).
    let set = |children: &mut Vec<RawChild>, tag: &str, xml: Option<String>| {
        children.retain(|c| c.tag != tag);
        if let Some(xml) = xml {
            children.push(RawChild { tag: tag.to_string(), xml });
        }
    };
    if !geometry_unchanged {
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
        set(&mut children, "w:pgSz", Some(pgsz));
        set(&mut children, "w:pgMar", Some(pgmar));
        let cols_changed = original.as_ref().map_or(true, |o| o.columns != s.columns || o.column_space != s.column_space);
        if cols_changed || !children.iter().any(|c| c.tag == "w:cols") {
            let cols = if s.columns > 1 {
                format!("<w:cols w:num=\"{}\" w:space=\"{}\"/>", s.columns, s.column_space)
            } else {
                format!("<w:cols w:space=\"{}\"/>", s.column_space)
            };
            set(&mut children, "w:cols", Some(cols));
        }
        if original.is_none() {
            set(&mut children, "w:docGrid", Some("<w:docGrid w:linePitch=\"360\"/>".into()));
        }
    }
    if !start_unchanged {
        set(&mut children, "w:type", if s.start == SectionStart::NextPage && original.is_none() { None } else { Some(format!("<w:type w:val=\"{}\"/>", s.start.docx_name())) });
    }
    if !title_unchanged {
        set(&mut children, "w:titlePg", if s.title_page { Some("<w:titlePg/>".into()) } else { None });
    }
    if !page_start_unchanged {
        set(&mut children, "w:pgNumType", s.page_start.map(|n| format!("<w:pgNumType w:start=\"{}\"/>", n)));
    }
    if !hf_unchanged {
        children.retain(|c| c.tag != "w:headerReference" && c.tag != "w:footerReference");
        for r in &s.hf {
            let tag = match r.kind {
                HfKind::Header => "w:headerReference",
                HfKind::Footer => "w:footerReference",
            };
            children.push(RawChild { tag: tag.to_string(), xml: format!("<{} w:type=\"{}\" r:id=\"{}\"/>", tag, r.pages.docx_name(), escape_attr(&r.id)) });
        }
    }
    if !(geometry_unchanged && start_unchanged && title_unchanged && hf_unchanged && page_start_unchanged) {
        children.sort_by_key(|c| SECTPR_ORDER.iter().position(|t| *t == c.tag).unwrap_or(SECTPR_ORDER.len() - 3));
    }
    for c in children {
        out.push_str(&c.xml);
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

// ---------------------------------------------------------------------------
// numbering.xml
// ---------------------------------------------------------------------------

pub fn render_numbering(n: &wp_core::Numbering) -> String {
    let mut out = String::from(XML_DECL);

    match &n.root_tag {
        Some(r) => out.push_str(r),
        None => {
            let _ = write!(out, "<w:numbering xmlns:w=\"{}\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" mc:Ignorable=\"w14\">", W_NS);
        }
    }
    // Schema order: numPicBullet*, abstractNum*, num*, numIdMacAtCleanup?
    let (pics, rest): (Vec<&String>, Vec<&String>) = n.opaque.iter().partition(|s| s.starts_with("<w:numPicBullet"));
    for s in pics {
        out.push_str(s);
    }
    for a in &n.abstract_nums {
        if let Some(raw) = &a.raw {
            out.push_str(raw);
            continue;
        }
        let _ = write!(out, "<w:abstractNum w:abstractNumId=\"{}\"><w:multiLevelType w:val=\"{}\"/>", a.id, if a.levels.iter().any(|l| l.text.matches('%').count() > 1) { "multilevel" } else { "hybridMultilevel" });
        for (i, l) in a.levels.iter().enumerate() {
            render_level(&mut out, i as u8, l);
        }
        out.push_str("</w:abstractNum>");
    }
    for num in &n.nums {
        if let Some(raw) = &num.raw {
            out.push_str(raw);
            continue;
        }
        let _ = write!(out, "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/>", num.id, num.abstract_id);
        for o in &num.overrides {
            if let Some(raw) = &o.raw {
                out.push_str(raw);
                continue;
            }
            let _ = write!(out, "<w:lvlOverride w:ilvl=\"{}\">", o.ilvl);
            if let Some(s) = o.start {
                let _ = write!(out, "<w:startOverride w:val=\"{}\"/>", s);
            }
            if let Some(l) = &o.level {
                render_level(&mut out, o.ilvl, l);
            }
            out.push_str("</w:lvlOverride>");
        }
        out.push_str("</w:num>");
    }
    for s in rest {
        out.push_str(s);
    }
    out.push_str("</w:numbering>");
    out
}

fn render_level(out: &mut String, ilvl: u8, l: &wp_core::numbering::Level) {
    use wp_core::numbering::*;
    if let Some(raw) = &l.raw {
        out.push_str(raw);
        return;
    }
    let _ = write!(out, "<w:lvl w:ilvl=\"{}\"><w:start w:val=\"{}\"/><w:numFmt w:val=\"{}\"/>", ilvl, l.start, l.fmt.docx_name());
    match l.suffix {
        Suffix::Tab => {}
        Suffix::Space => out.push_str("<w:suff w:val=\"space\"/>"),
        Suffix::Nothing => out.push_str("<w:suff w:val=\"nothing\"/>"),
    }
    let _ = write!(out, "<w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"{}\"/>", escape_attr(&l.text), l.align.docx_name());
    let body = render_ppr_body(&l.para);
    if !body.is_empty() {
        let _ = write!(out, "<w:pPr>{}</w:pPr>", body);
    }
    let mut run = l.run.clone();
    if l.fmt.is_bullet() && run.font.is_none() {
        // Unicode bullets need a font that has them; Word's default for its own bullets is Symbol.
        run.font = Some("Segoe UI Symbol".into());
        if l.text == "•" || l.text == "◦" || l.text == "–" {
            run.font = None;
        }
    }
    out.push_str(&render_run_props(&run));
    out.push_str("</w:lvl>");
}
