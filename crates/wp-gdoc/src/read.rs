//! `documents.get` JSON → `Document`, plus the baseline the writer diffs
//! against: every paragraph as read, with its Docs index range.

use crate::json::*;
use crate::project::ListMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wp_core::model::*;
use wp_core::numbering::ListKind;
use wp_core::style::Style;
use wp_core::Document;

/// A paragraph as read, with the index range it occupies in its segment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BPara {
    pub para: Paragraph,
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BBlock {
    Paras(Vec<BPara>),
    Table { start: i64, end: i64, cells: Vec<Vec<Vec<BPara>>> },
}

/// The body, or one footnote.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BSegment {
    pub id: Option<String>,
    pub blocks: Vec<BBlock>,
}

/// What the writer needs to turn edits into `batchUpdate` requests against
/// the revision that was read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Baseline {
    pub document_id: String,
    pub title: String,
    pub revision_id: String,
    pub tab_id: Option<String>,
    pub segments: Vec<BSegment>,
    /// Docs list id → `num_id` the reader assigned.
    pub lists: ListMap,
    /// Model footnote id (1-based) − 1 → Docs footnote id.
    pub footnote_ids: Vec<String>,
}

pub struct Loaded {
    pub doc: Document,
    pub baseline: Baseline,
    pub warnings: Vec<String>,
}

const OPAQUE_LEVEL_LABELS: [(&str, &str); 9] = [
    ("inlineObjectElement", "Image"),
    ("horizontalRule", "Rule"),
    ("equation", "Equation"),
    ("person", "Person"),
    ("richLink", "Link chip"),
    ("dateElement", "Date"),
    ("autoText", "Auto text"),
    ("columnBreak", "Column Break"),
    ("footnoteReference", "Footnote"),
];

struct Reader {
    doc: Document,
    lists: ListMap,
    footnote_ids: Vec<String>,
    lists_json: Value,
    next_rel: usize,
    next_wrapper: u32,
    warnings: Vec<String>,
    suggestions: usize,
}

pub fn read(json: &str) -> Result<Loaded, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("not a Google Docs document: {}", e))?;
    let (tab, tab_id) = if root.get("body").is_some() {
        (&root, None)
    } else {
        let t = root.get("tabs").and_then(|t| t.get(0)).ok_or("document has no body")?;
        (t.get("documentTab").ok_or("document has no body")?, str_of(t.get("tabProperties").unwrap_or(&Value::Null), "tabId").map(str::to_string))
    };
    let mut doc = Document::new();
    doc.paragraphs.clear();
    doc.styles.dirty = true;
    if doc.styles.get("Hyperlink").is_none() {
        let mut h = Style::character("Hyperlink", "Hyperlink");
        h.run.color = Some(Rgb(0x11, 0x55, 0xCC));
        h.run.underline = Some(Some(Underline::Single));
        doc.styles.upsert(h);
    }
    let mut r = Reader {
        doc,
        lists: ListMap::new(),
        footnote_ids: Vec::new(),
        lists_json: tab.get("lists").cloned().unwrap_or(Value::Null),
        next_rel: 1,
        next_wrapper: 1,
        warnings: Vec::new(),
        suggestions: 0,
    };
    if let Some(ns) = tab.get("namedStyles").and_then(|n| n.get("styles")).and_then(Value::as_array) {
        r.named_styles(ns);
    }
    if let Some(ds) = tab.get("documentStyle") {
        r.document_style(ds);
    }
    let body = tab.get("body").and_then(|b| b.get("content")).and_then(Value::as_array).ok_or("document has no body")?;
    let mut blocks = Vec::new();
    r.blocks(body, None, &mut blocks, true, true)?;
    let mut segments = vec![BSegment { id: None, blocks }];
    // Footnote bodies, in reference order.
    let fns = tab.get("footnotes").cloned().unwrap_or(Value::Null);
    let mut i = 0;
    while i < r.footnote_ids.len() {
        let gid = r.footnote_ids[i].clone();
        let content = fns.get(&gid).and_then(|f| f.get("content")).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut fb = Vec::new();
        r.blocks(&content, None, &mut fb, false, false)?;
        let mut paras: Vec<Paragraph> = Vec::new();
        for b in &fb {
            match b {
                BBlock::Paras(ps) => paras.extend(ps.iter().map(|p| p.para.clone())),
                BBlock::Table { .. } => return Err("a table inside a footnote".into()),
            }
        }
        if paras.is_empty() {
            paras.push(Paragraph::new());
        }
        r.doc.footnotes.push(Footnote { id: i as i32 + 1, paragraphs: paras });
        segments.push(BSegment { id: Some(gid), blocks: fb });
        i += 1;
    }
    if r.doc.paragraphs.is_empty() {
        r.doc.paragraphs.push(Paragraph::new());
    }
    if tab.get("headers").map_or(false, |h| h.as_object().map_or(false, |o| !o.is_empty())) || tab.get("footers").map_or(false, |h| h.as_object().map_or(false, |o| !o.is_empty())) {
        r.warnings.push("headers/footers not shown (kept)".into());
    }
    if tab.get("positionedObjects").map_or(false, |h| h.as_object().map_or(false, |o| !o.is_empty())) {
        r.warnings.push("positioned images not shown (kept)".into());
    }
    if r.suggestions > 0 {
        r.warnings.push(format!("{} suggestion{} shown as tracked changes", r.suggestions, if r.suggestions == 1 { "" } else { "s" }));
    }
    let baseline = Baseline {
        document_id: str_of(&root, "documentId").unwrap_or("").to_string(),
        title: str_of(&root, "title").unwrap_or("").to_string(),
        revision_id: str_of(&root, "revisionId").unwrap_or("").to_string(),
        tab_id,
        segments,
        lists: r.lists,
        footnote_ids: r.footnote_ids,
    };
    Ok(Loaded { doc: r.doc, baseline, warnings: r.warnings })
}

impl Reader {
    fn named_styles(&mut self, styles: &[Value]) {
        for ns in styles {
            let Some(kind) = str_of(ns, "namedStyleType") else { continue };
            let run = text_style_props(ns.get("textStyle").unwrap_or(&Value::Null));
            let mut para = ParaProps::default();
            if let Some(ps) = ns.get("paragraphStyle") {
                para_style_into(ps, &mut para);
            }
            para.style = None;
            if kind == "NORMAL_TEXT" {
                if let Some(n) = self.doc.styles.get_mut("Normal") {
                    n.run = run.clone();
                    n.para = para;
                    n.raw_xml = None;
                }
                self.doc.styles.default_run = run;
                continue;
            }
            let Some(id) = style_id_from(kind) else { continue };
            let name = match kind {
                "TITLE" => "Title".to_string(),
                "SUBTITLE" => "Subtitle".to_string(),
                _ => format!("heading {}", &id[7..]),
            };
            let mut st = Style::para(&id, &name);
            st.based_on = Some("Normal".into());
            st.next = Some("Normal".into());
            if let Some(n) = id.strip_prefix("Heading").and_then(|n| n.parse::<u8>().ok()) {
                para.outline_level = Some(n - 1);
            }
            st.para = para;
            st.run = run;
            self.doc.styles.upsert(st);
        }
    }

    fn document_style(&mut self, ds: &Value) {
        let s = &mut self.doc.section;
        if let Some(ps) = ds.get("pageSize") {
            if let (Some(w), Some(h)) = (dim_twips(ps.get("width")), dim_twips(ps.get("height"))) {
                if w > 0 && h > 0 {
                    s.page_width = w;
                    s.page_height = h;
                    s.orientation = if w > h { Orientation::Landscape } else { Orientation::Portrait };
                }
            }
        }
        for (key, field) in [("marginTop", &mut s.margin_top), ("marginBottom", &mut s.margin_bottom), ("marginLeft", &mut s.margin_left), ("marginRight", &mut s.margin_right)] {
            if let Some(t) = dim_twips(ds.get(key)) {
                *field = t;
            }
        }
    }

    /// Read a `content` array into blocks. `top` marks the body, whose
    /// leading section break is the document's own and is not a block.
    fn blocks(&mut self, content: &[Value], cell: Option<CellRef>, out: &mut Vec<BBlock>, top: bool, body: bool) -> Result<(), String> {
        let push_para = |out: &mut Vec<BBlock>, bp: BPara| match out.last_mut() {
            Some(BBlock::Paras(v)) => v.push(bp),
            _ => out.push(BBlock::Paras(vec![bp])),
        };
        for (i, el) in content.iter().enumerate() {
            let start = i64_of(el, "startIndex").unwrap_or(0);
            let end = i64_of(el, "endIndex").unwrap_or(start);
            if let Some(p) = el.get("paragraph") {
                let para = self.paragraph(p, cell)?;
                let bp = BPara { para, start, end };
                if body && cell.is_none() {
                    self.doc.paragraphs.push(bp.para.clone());
                }
                push_para(out, bp);
            } else if let Some(t) = el.get("table") {
                if cell.is_some() || !top {
                    // Nested table (or one in a footnote): preserved in place.
                    push_para(out, self.raw_block(el, "Table", start, end, cell, body));
                    continue;
                }
                match self.table(t, start, end) {
                    Some(block) => out.push(block),
                    None => {
                        self.warnings.push("a table with merged cells is shown as a preserved block".into());
                        let bp = self.raw_block(el, "Table", start, end, cell, body);
                        push_para(out, bp);
                    }
                }
            } else if el.get("sectionBreak").is_some() {
                if top && i == 0 {
                    continue;
                }
                push_para(out, self.raw_block(el, "Section Break", start, end, cell, body));
            } else if el.get("tableOfContents").is_some() {
                push_para(out, self.raw_block(el, "Table of Contents", start, end, cell, body));
            } else {
                push_para(out, self.raw_block(el, "Element", start, end, cell, body));
            }
        }
        Ok(())
    }

    fn raw_block(&mut self, el: &Value, label: &str, start: i64, end: i64, cell: Option<CellRef>, body: bool) -> BPara {
        let xml = opaque_json(el, end - start);
        let level = if cell.is_some() { OpaqueLevel::Cell } else { OpaqueLevel::Body };
        let mut props = ParaProps { raw_block: true, cell, ..Default::default() };
        props.touch();
        let para = Paragraph { props, items: vec![Item::Code(Code::Opaque(OpaqueXml { protected: true, ..OpaqueXml::element(xml, label).at(level) }))] };
        if body && cell.is_none() {
            self.doc.paragraphs.push(para.clone());
        }
        BPara { para, start, end }
    }

    /// A body-level table as cell-tagged paragraphs plus a grid. `None` when
    /// the table has spans, which the model would have to guess at.
    fn table(&mut self, t: &Value, start: i64, end: i64) -> Option<BBlock> {
        let rows = t.get("tableRows").and_then(Value::as_array)?;
        let ncols = i64_of(t, "columns").unwrap_or(0).max(1) as usize;
        let nrows = rows.len().max(1);
        for row in rows {
            let cells = row.get("tableCells").and_then(Value::as_array)?;
            if cells.len() != ncols {
                return None;
            }
            for c in cells {
                let st = c.get("tableCellStyle").unwrap_or(&Value::Null);
                if i64_of(st, "columnSpan").unwrap_or(1) != 1 || i64_of(st, "rowSpan").unwrap_or(1) != 1 {
                    return None;
                }
            }
        }
        let id = self.doc.next_table_id();
        let width = self.doc.section.text_width();
        let mut table = Table::new(nrows, ncols, width);
        if let Some(cols) = t.get("tableStyle").and_then(|s| s.get("tableColumnProperties")).and_then(Value::as_array) {
            let widths: Vec<Option<Twips>> = cols.iter().map(|c| dim_twips(c.get("width")).filter(|w| *w > 0)).collect();
            if widths.len() == ncols && widths.iter().all(Option::is_some) {
                table.grid = widths.iter().map(|w| w.unwrap()).collect();
                for r in &mut table.rows {
                    for (ci, c) in r.cells.iter_mut().enumerate() {
                        c.width = Some(table.grid[ci]);
                    }
                }
            }
        }
        table.style = None;
        let mut cells: Vec<Vec<Vec<BPara>>> = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            let mut out_row = Vec::new();
            for (ci, c) in row.get("tableCells").and_then(Value::as_array)?.iter().enumerate() {
                let cref = CellRef::new(id, ri as u32, ci as u32);
                let content = c.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut blocks = Vec::new();
                self.blocks(&content, Some(cref), &mut blocks, false, true).ok()?;
                let mut paras: Vec<BPara> = Vec::new();
                for b in blocks {
                    match b {
                        BBlock::Paras(v) => paras.extend(v),
                        BBlock::Table { .. } => return None,
                    }
                }
                if paras.is_empty() {
                    let s = i64_of(c, "startIndex").unwrap_or(start);
                    let mut p = Paragraph::new();
                    p.props.cell = Some(cref);
                    paras.push(BPara { para: p, start: s + 1, end: s + 2 });
                }
                for p in &paras {
                    self.doc.paragraphs.push(p.para.clone());
                }
                out_row.push(paras);
            }
            cells.push(out_row);
        }
        self.doc.tables.insert(id, table);
        Some(BBlock::Table { start, end, cells })
    }

    fn paragraph(&mut self, p: &Value, cell: Option<CellRef>) -> Result<Paragraph, String> {
        let mut props = ParaProps { cell, ..Default::default() };
        if let Some(ps) = p.get("paragraphStyle") {
            para_style_into(ps, &mut props);
        }
        if let Some(b) = p.get("bullet") {
            if let Some(list_id) = str_of(b, "listId") {
                let num_id = self.list_num(list_id);
                props.list = Some(ListRef { num_id, level: i64_of(b, "nestingLevel").unwrap_or(0).clamp(0, 8) as u8 });
            }
        }
        props.touch();
        let mut items: Vec<Item> = Vec::new();
        let mut link: Option<String> = None;
        let mut sugg: Option<(String, bool)> = None;
        let mut wrappers: Vec<u32> = Vec::new();
        let elements = p.get("elements").and_then(Value::as_array).cloned().unwrap_or_default();
        for el in &elements {
            let start = i64_of(el, "startIndex").unwrap_or(0);
            let end = i64_of(el, "endIndex").unwrap_or(start);
            if let Some(tr) = el.get("textRun") {
                let ts = tr.get("textStyle").unwrap_or(&Value::Null);
                // Suggestions: a wrapper per run of the same suggestion.
                let ins = tr.get("suggestedInsertionIds").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str);
                let del = tr.get("suggestedDeletionIds").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str);
                let want = ins.map(|s| (s.to_string(), false)).or_else(|| del.map(|s| (s.to_string(), true)));
                if want != sugg {
                    if sugg.is_some() {
                        self.close_wrapper(&mut items, &mut wrappers, if sugg.as_ref().unwrap().1 { "</w:del>" } else { "</w:ins>" });
                    }
                    if let Some((id, deleted)) = &want {
                        self.suggestions += 1;
                        let tag = if *deleted { "del" } else { "ins" };
                        let xml = format!("<w:{} w:id=\"{}\" w:author=\"Google Docs suggestion {}\" w:date=\"2000-01-01T00:00:00Z\">", tag, self.next_wrapper + 1000, id);
                        let label = if *deleted { "Suggested deletion" } else { "Suggested insertion" };
                        self.open_wrapper(&mut items, &mut wrappers, xml, label, true, *deleted);
                    }
                    sugg = want;
                }
                let url = ts.get("link").and_then(|l| str_of(l, "url")).map(str::to_string);
                if url != link {
                    if link.is_some() {
                        self.close_wrapper(&mut items, &mut wrappers, "</w:hyperlink>");
                    }
                    if let Some(u) = &url {
                        let rid = format!("rIdgd{}", self.next_rel);
                        self.next_rel += 1;
                        self.doc.extra_rels.push(ExtraRel { id: rid.clone(), kind: "hyperlink".into(), target: u.clone(), external: true });
                        let xml = format!("<w:hyperlink r:id=\"{}\" w:history=\"1\">", rid);
                        self.open_wrapper(&mut items, &mut wrappers, xml, "Hyperlink", false, false);
                    }
                    link = url;
                }
                let mut attrs = text_style_attrs(ts);
                if link.is_some() {
                    attrs.push(Attr::CharStyle("Hyperlink".into()));
                }
                let content = str_of(tr, "content").unwrap_or("");
                for a in &attrs {
                    items.push(Item::Code(Code::On(a.clone())));
                }
                for c in content.chars() {
                    items.push(match c {
                        '\n' => continue,
                        '\t' => Item::Code(Code::Tab),
                        '\u{b}' => Item::Code(Code::LineBreak),
                        c => Item::Char(c),
                    });
                }
                for a in attrs.iter().rev() {
                    items.push(Item::Code(Code::Off(a.kind())));
                }
                continue;
            }
            // Anything that is not text closes an open link.
            if link.is_some() {
                self.close_wrapper(&mut items, &mut wrappers, "</w:hyperlink>");
                link = None;
            }
            if let Some(fr) = el.get("footnoteReference") {
                let gid = str_of(fr, "footnoteId").unwrap_or("").to_string();
                let n = match self.footnote_ids.iter().position(|g| *g == gid) {
                    Some(i) => i + 1,
                    None => {
                        self.footnote_ids.push(gid);
                        self.footnote_ids.len()
                    }
                };
                items.push(Item::Code(Code::On(Attr::VertAlign(VertAlign::Superscript))));
                items.push(Item::Code(Code::Opaque(OpaqueXml::element(format!("<w:footnoteReference w:id=\"{}\"/>", n), "Footnote"))));
                items.push(Item::Code(Code::Off(AttrKind::VertAlign)));
            } else if el.get("pageBreak").is_some() {
                items.push(Item::Code(Code::PageBreak));
            } else {
                let key = el.as_object().and_then(|o| o.keys().find(|k| !matches!(k.as_str(), "startIndex" | "endIndex"))).cloned().unwrap_or_default();
                let label = OPAQUE_LEVEL_LABELS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or("Element");
                let xml = opaque_json(el, (end - start).max(1));
                items.push(Item::Code(Code::Opaque(OpaqueXml { protected: true, ..OpaqueXml::element(xml, label) })));
            }
        }
        if link.is_some() {
            self.close_wrapper(&mut items, &mut wrappers, "</w:hyperlink>");
        }
        if let Some((_, deleted)) = sugg {
            self.close_wrapper(&mut items, &mut wrappers, if deleted { "</w:del>" } else { "</w:ins>" });
        }
        Ok(Paragraph { props, items })
    }

    fn open_wrapper(&mut self, items: &mut Vec<Item>, wrappers: &mut Vec<u32>, xml: String, label: &str, protected: bool, deleted: bool) {
        let id = self.next_wrapper;
        self.next_wrapper += 1;
        wrappers.push(id);
        items.push(Item::Code(Code::Opaque(OpaqueXml { xml, label: label.into(), kind: OpaqueKind::Open(id), protected, deleted, hint: false, level: OpaqueLevel::Para })));
    }

    fn close_wrapper(&mut self, items: &mut Vec<Item>, wrappers: &mut Vec<u32>, xml: &str) {
        let id = wrappers.pop().unwrap_or(0);
        let label = xml.trim_start_matches("</w:").trim_end_matches('>').to_string();
        items.push(Item::Code(Code::Opaque(OpaqueXml { xml: xml.into(), label, kind: OpaqueKind::Close(id), protected: false, deleted: false, hint: false, level: OpaqueLevel::Para })));
    }

    /// The `num_id` for a Docs list, created from its level-0 glyph on first use.
    fn list_num(&mut self, list_id: &str) -> i32 {
        if let Some(n) = self.lists.get(list_id) {
            return *n;
        }
        let lvl0 = self.lists_json.get(list_id).and_then(|l| l.get("listProperties")).and_then(|p| p.get("nestingLevels")).and_then(|n| n.get(0)).cloned().unwrap_or(Value::Null);
        let fmt = str_of(&lvl0, "glyphFormat").unwrap_or("");
        let paren = fmt.trim_end().ends_with(')');
        let kind = match str_of(&lvl0, "glyphType") {
            Some("DECIMAL") | Some("ZERO_DECIMAL") => {
                if paren {
                    ListKind::DecimalParen
                } else {
                    ListKind::Decimal
                }
            }
            Some("ALPHA") => {
                if paren {
                    ListKind::LowerLetterParen
                } else {
                    ListKind::LowerLetter
                }
            }
            Some("UPPER_ALPHA") => ListKind::UpperLetter,
            Some("ROMAN") => ListKind::LowerRoman,
            Some("UPPER_ROMAN") => ListKind::UpperRoman,
            _ => match str_of(&lvl0, "glyphSymbol") {
                Some("-") | Some("–") | Some("—") => ListKind::Dash,
                _ => ListKind::Bullet,
            },
        };
        let n = self.doc.numbering.add_list(kind);
        self.lists.insert(list_id.to_string(), n);
        n
    }
}

/// A paragraph element (or structural element) as preserved JSON: the
/// element without its indexes, plus the length it occupies.
fn opaque_json(el: &Value, len: i64) -> String {
    let mut o = el.clone();
    if let Some(m) = o.as_object_mut() {
        m.remove("startIndex");
        m.remove("endIndex");
        m.insert("wpLen".into(), Value::from(len));
    }
    serde_json::to_string(&o).unwrap_or_default()
}

/// Direct character formatting of a `TextStyle`, as paired codes.
pub fn text_style_attrs(ts: &Value) -> Vec<Attr> {
    let mut a = Vec::new();
    if let Some(b) = bool_of(ts, "bold") {
        a.push(Attr::Bold(b));
    }
    if let Some(b) = bool_of(ts, "italic") {
        a.push(Attr::Italic(b));
    }
    if let Some(b) = bool_of(ts, "underline") {
        a.push(Attr::Underline(if b { Underline::Single } else { Underline::None }));
    }
    if let Some(b) = bool_of(ts, "strikethrough") {
        a.push(Attr::Strike(b));
    }
    if let Some(b) = bool_of(ts, "smallCaps") {
        a.push(Attr::SmallCaps(b));
    }
    if let Some(t) = dim_twips(ts.get("fontSize")) {
        // Twips → half-points.
        a.push(Attr::Size(((t as f64) / 10.0).round() as u16));
    }
    if let Some(f) = ts.get("weightedFontFamily").and_then(|w| str_of(w, "fontFamily")) {
        a.push(Attr::Font(f.to_string()));
    }
    if let Some(c) = color_rgb(ts.get("foregroundColor")) {
        a.push(Attr::Color(c));
    }
    if let Some(h) = color_rgb(ts.get("backgroundColor")).and_then(rgb_highlight) {
        a.push(Attr::Highlight(h));
    }
    match str_of(ts, "baselineOffset") {
        Some("SUPERSCRIPT") => a.push(Attr::VertAlign(VertAlign::Superscript)),
        Some("SUBSCRIPT") => a.push(Attr::VertAlign(VertAlign::Subscript)),
        Some("NONE") => a.push(Attr::VertAlign(VertAlign::Baseline)),
        _ => {}
    }
    a
}

/// The same, as run properties (for named styles).
fn text_style_props(ts: &Value) -> RunProps {
    let mut r = RunProps::default();
    for a in text_style_attrs(ts) {
        r.apply(&a);
    }
    r
}

pub fn para_style_into(ps: &Value, p: &mut ParaProps) {
    if let Some(n) = str_of(ps, "namedStyleType") {
        p.style = style_id_from(n);
    }
    if let Some(a) = str_of(ps, "alignment").and_then(align_from) {
        p.align = Some(a);
    }
    p.indent_left = dim_twips(ps.get("indentStart"));
    p.indent_right = dim_twips(ps.get("indentEnd"));
    if let Some(first) = dim_twips(ps.get("indentFirstLine")) {
        let delta = first - p.indent_left.unwrap_or(0);
        if delta >= 0 {
            p.first_line = Some(delta);
        } else {
            p.hanging = Some(-delta);
        }
    }
    p.space_before = dim_twips(ps.get("spaceAbove"));
    p.space_after = dim_twips(ps.get("spaceBelow"));
    if let Some(pct) = ps.get("lineSpacing").and_then(Value::as_f64) {
        p.line_spacing = Some(LineSpacing::Auto((pct * 240.0 / 100.0).round() as i32));
    }
    p.keep_lines = bool_of(ps, "keepLinesTogether");
    p.keep_next = bool_of(ps, "keepWithNext");
    p.widow_control = bool_of(ps, "avoidWidowAndOrphan");
    p.page_break_before = bool_of(ps, "pageBreakBefore");
    p.shading = color_rgb(ps.get("shading").and_then(|s| s.get("backgroundColor")));
    if let Some(stops) = ps.get("tabStops").and_then(Value::as_array) {
        p.tabs = stops
            .iter()
            .filter_map(|t| {
                let pos = dim_twips(t.get("offset"))?;
                let kind = match str_of(t, "alignment") {
                    Some("CENTER") => TabKind::Center,
                    Some("END") => TabKind::Right,
                    _ => TabKind::Left,
                };
                Some(TabStop { pos, kind, leader: TabLeader::None, clear: false })
            })
            .collect();
    }
}
