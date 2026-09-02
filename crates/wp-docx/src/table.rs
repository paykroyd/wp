//! `w:tbl` ↔ the cell-tagged paragraph model (DESIGN.md §3.7, §6.1).
//!
//! Reading: a body-level table becomes a `Table` plus one paragraph per
//! `w:p` in its cells, each tagged with its `CellRef`. `tblPr`, `trPr` and
//! `tcPr` are kept verbatim and only regenerated when the grid changes.
//! Anything structurally unexpected (a content control wrapping rows, an
//! element we don't know between cells) makes the whole table fall back to a
//! preserved block, exactly as before tables were editable.
//!
//! Writing: a run of cell paragraphs is wrapped in `w:tbl`/`w:tr`/`w:tc` as
//! the writer walks the body.

use crate::read::{block_label, bookmark_item, element_label, parse_paragraph, raw_block, Ctx};
use crate::xml::*;
use anyhow::Result;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fmt::Write as _;
use wp_core::model::*;
use wp_core::Document;

/// Parse a body-level `<w:tbl>…</w:tbl>`. `None` means "keep it verbatim".
/// `pending` receives range markers found between rows or cells; they are
/// attached to the next paragraph.
pub(crate) fn parse_table(xml: &str, id: u32, ctx: &mut Ctx, pending: &mut Vec<Item>) -> Result<Option<(Table, Vec<Paragraph>)>> {
    let mut reader = Reader::from_str(xml);
    let mut table = Table { cell_margin_left: DEFAULT_CELL_MARGIN, cell_margin_right: DEFAULT_CELL_MARGIN, ..Default::default() };
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    // <w:tbl …>
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                table.attrs = tag_attrs(&e);
                break;
            }
            Ok(Event::Eof) | Err(_) => return Ok(None),
            _ => {}
        }
    }
    let mut row_idx = 0u32;
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                let _ = reader.read_to_end(name);
                let after = reader.buffer_position() as usize;
                let raw = &xml[before..after];
                match n.as_slice() {
                    b"w:tblPr" => {
                        table.raw_tblpr = Some(raw.to_string());
                        parse_tblpr(raw, &mut table);
                    }
                    b"w:tblGrid" => {
                        table.raw_grid = Some(raw.to_string());
                        table.grid = children_of(raw).iter().filter(|c| c.tag == "w:gridCol").map(|c| start_tag(&c.xml).and_then(|t| attr_i32(&t, "w:w")).unwrap_or(0).max(0)).collect();
                    }
                    b"w:tr" => {
                        let Some((row, mut paras)) = parse_row(raw, id, row_idx, ctx, pending)? else { return Ok(None) };
                        table.rows.push(row);
                        paragraphs.append(&mut paras);
                        row_idx += 1;
                    }
                    _ => return Ok(None),
                }
            }
            Ok(Event::Empty(e)) => {
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:tblPr" => table.raw_tblpr = Some(xml[before..after].to_string()),
                    b"w:tblGrid" => table.raw_grid = Some(xml[before..after].to_string()),
                    b"w:bookmarkStart" | b"w:bookmarkEnd" => pending.push(bookmark_item(&e, &xml[before..after], ctx, OpaqueLevel::Body)),
                    _ => return Ok(None),
                }
            }
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Err(_) => return Ok(None),
            _ => {}
        }
    }
    if table.rows.is_empty() || paragraphs.is_empty() {
        return Ok(None);
    }
    if table.grid.is_empty() {
        // No grid: derive one from the widest row.
        let cols = table.rows.iter().map(|r| r.cells.iter().map(|c| c.span()).sum::<usize>()).max().unwrap_or(1).max(1);
        let w = (9360 / cols as i32).max(360);
        table.grid = vec![w; cols];
        table.raw_grid = None;
    }
    Ok(Some((table, paragraphs)))
}

fn parse_tblpr(raw: &str, table: &mut Table) {
    for c in children_of(raw) {
        match c.tag.as_str() {
            "w:tblStyle" => table.style = start_tag(&c.xml).and_then(|t| attr(&t, "w:val")),
            "w:tblCellMar" => {
                for m in children_of(&c.xml) {
                    let Some(t) = start_tag(&m.xml) else { continue };
                    if attr(&t, "w:type").as_deref().unwrap_or("dxa") != "dxa" {
                        continue;
                    }
                    let w = attr(&t, "w:w").and_then(|v| v.parse::<f64>().ok()).map(|v| v as Twips);
                    match (m.tag.as_str(), w) {
                        ("w:left" | "w:start", Some(w)) => table.cell_margin_left = w,
                        ("w:right" | "w:end", Some(w)) => table.cell_margin_right = w,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_row(xml: &str, id: u32, row_idx: u32, ctx: &mut Ctx, pending: &mut Vec<Item>) -> Result<Option<(TableRow, Vec<Paragraph>)>> {
    let mut reader = Reader::from_str(xml);
    let mut row = TableRow::default();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                row.attrs = tag_attrs(&e);
                break;
            }
            Ok(Event::Eof) | Err(_) => return Ok(None),
            _ => {}
        }
    }
    let mut col_idx = 0u32;
    let mut raw_trpr = String::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                let _ = reader.read_to_end(name);
                let after = reader.buffer_position() as usize;
                let raw = &xml[before..after];
                match n.as_slice() {
                    b"w:tblPrEx" => raw_trpr.push_str(raw),
                    b"w:trPr" => {
                        raw_trpr.push_str(raw);
                        for c in children_of(raw) {
                            match c.tag.as_str() {
                                "w:tblHeader" => row.header = start_tag(&c.xml).map_or(true, |t| attr_bool(&t, "w:val")),
                                "w:cantSplit" => row.cant_split = start_tag(&c.xml).map_or(true, |t| attr_bool(&t, "w:val")),
                                "w:trHeight" => {
                                    let t = start_tag(&c.xml);
                                    row.height = t.as_ref().and_then(|t| attr_i32(t, "w:val"));
                                    row.height_exact = t.as_ref().and_then(|t| attr(t, "w:hRule")).as_deref() == Some("exact");
                                }
                                _ => {}
                            }
                        }
                    }
                    b"w:tc" => {
                        let Some((cell, mut paras)) = parse_cell(raw, CellRef::new(id, row_idx, col_idx), ctx, pending)? else { return Ok(None) };
                        row.cells.push(cell);
                        paragraphs.append(&mut paras);
                        col_idx += 1;
                    }
                    _ => return Ok(None),
                }
            }
            Ok(Event::Empty(e)) => {
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:tblPrEx" | b"w:trPr" => raw_trpr.push_str(&xml[before..after]),
                    b"w:bookmarkStart" | b"w:bookmarkEnd" => pending.push(bookmark_item(&e, &xml[before..after], ctx, OpaqueLevel::Body)),
                    _ => return Ok(None),
                }
            }
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Err(_) => return Ok(None),
            _ => {}
        }
    }
    if row.cells.is_empty() {
        return Ok(None);
    }
    if !raw_trpr.is_empty() {
        row.raw_trpr = Some(raw_trpr);
    }
    Ok(Some((row, paragraphs)))
}

fn parse_cell(xml: &str, cell_ref: CellRef, ctx: &mut Ctx, pending: &mut Vec<Item>) -> Result<Option<(TableCell, Vec<Paragraph>)>> {
    let mut reader = Reader::from_str(xml);
    let mut cell = TableCell::new();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                cell.attrs = tag_attrs(&e);
                break;
            }
            Ok(Event::Empty(e)) => {
                // `<w:tc/>`: an empty cell (not valid, but harmless).
                cell.attrs = tag_attrs(&e);
                let mut p = Paragraph::new();
                p.props.cell = Some(cell_ref);
                return Ok(Some((cell, vec![p])));
            }
            Ok(Event::Eof) | Err(_) => return Ok(None),
            _ => {}
        }
    }
    let take_pending = |pending: &mut Vec<Item>, p: &mut Paragraph| {
        if !pending.is_empty() {
            let mut items = std::mem::take(pending);
            items.append(&mut p.items);
            p.items = items;
        }
    };
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref().to_vec();
                let _ = reader.read_to_end(name);
                let after = reader.buffer_position() as usize;
                let raw = &xml[before..after];
                match n.as_slice() {
                    b"w:tcPr" => {
                        cell.raw_tcpr = Some(raw.to_string());
                        parse_tcpr(raw, &mut cell);
                    }
                    b"w:p" => {
                        let mut p = parse_paragraph(raw, ctx)?;
                        take_pending(pending, &mut p);
                        p.props.cell = Some(cell_ref);
                        paragraphs.push(p);
                    }
                    _ => {
                        // A nested table, a content control, math…: preserved
                        // as a block inside the cell.
                        let label = block_label(&n, raw);
                        ctx.warn(&match n.as_slice() {
                            b"w:tbl" => "nested table".to_string(),
                            b"w:sdt" => "content control".to_string(),
                            _ => crate::read::warning_label_for_block(&n),
                        });
                        let mut p = raw_block(raw, &label);
                        take_pending(pending, &mut p);
                        p.props.cell = Some(cell_ref);
                        paragraphs.push(p);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let after = reader.buffer_position() as usize;
                let n = e.name().as_ref().to_vec();
                match n.as_slice() {
                    b"w:tcPr" => {
                        cell.raw_tcpr = Some(xml[before..after].to_string());
                    }
                    b"w:p" => {
                        let attrs = tag_attrs(&e);
                        let mut p = Paragraph::new();
                        p.props.p_attrs = if attrs.is_empty() { None } else { Some(attrs) };
                        take_pending(pending, &mut p);
                        p.props.cell = Some(cell_ref);
                        paragraphs.push(p);
                    }
                    b"w:bookmarkStart" | b"w:bookmarkEnd" => pending.push(bookmark_item(&e, &xml[before..after], ctx, OpaqueLevel::Cell)),
                    _ => {
                        let label = element_label(&n, ctx);
                        pending.push(Item::Code(Code::Opaque(OpaqueXml::element(&xml[before..after], label).at(OpaqueLevel::Cell))));
                    }
                }
            }
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Err(_) => return Ok(None),
            _ => {}
        }
    }
    if paragraphs.is_empty() {
        let mut p = Paragraph::new();
        p.props.cell = Some(cell_ref);
        paragraphs.push(p);
    }
    if cell.raw_tcpr.is_none() {
        // Had no `w:tcPr`: emit none until the cell is changed.
        cell.raw_tcpr = Some(String::new());
    }
    // Markers after the last paragraph stay in the cell, after it.
    if !pending.is_empty() {
        let items = std::mem::take(pending);
        paragraphs.last_mut().unwrap().items.extend(items.into_iter().map(|it| match it {
            Item::Code(Code::Opaque(o)) => Item::Code(Code::Opaque(o.at(OpaqueLevel::Cell))),
            other => other,
        }));
    }
    Ok(Some((cell, paragraphs)))
}

fn parse_tcpr(raw: &str, cell: &mut TableCell) {
    for c in children_of(raw) {
        let t = start_tag(&c.xml);
        match c.tag.as_str() {
            "w:gridSpan" => cell.span = t.and_then(|t| attr_i32(&t, "w:val")).unwrap_or(1).clamp(1, 63) as u16,
            "w:vMerge" => {
                cell.vmerge = Some(match t.and_then(|t| attr(&t, "w:val")).as_deref() {
                    Some("restart") => VMerge::Restart,
                    _ => VMerge::Continue,
                })
            }
            "w:tcW" => {
                if let Some(t) = t {
                    if attr(&t, "w:type").as_deref().unwrap_or("dxa") == "dxa" {
                        cell.width = attr(&t, "w:w").and_then(|v| v.parse::<f64>().ok()).map(|v| v as Twips);
                    }
                }
            }
            "w:shd" => cell.shading = t.and_then(|t| attr(&t, "w:fill")).and_then(|f| Rgb::parse_hex(&f)),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The table definition to write for `id`: the stored one, or a plain grid
/// reconstructed from the cell tags if the definition is missing.
pub(crate) fn table_for_write(doc: &Document, id: u32) -> Table {
    if let Some(t) = doc.tables.get(&id) {
        if !t.rows.is_empty() {
            return t.clone();
        }
    }
    let mut rows = 0usize;
    let mut cols = 1usize;
    for p in &doc.paragraphs {
        if let Some(c) = p.props.cell {
            if c.table == id {
                rows = rows.max(c.row as usize + 1);
                cols = cols.max(c.col as usize + 1);
            }
        }
    }
    Table::new(rows.max(1), cols, doc.section.text_width())
}

pub(crate) fn open_table(t: &Table, out: &mut String) {
    let _ = write!(out, "<w:tbl{}>", t.attrs);
    match &t.raw_tblpr {
        Some(raw) => out.push_str(raw),
        None => {
            out.push_str("<w:tblPr>");
            if let Some(s) = &t.style {
                let _ = write!(out, "<w:tblStyle w:val=\"{}\"/>", escape_attr(s));
            }
            out.push_str("<w:tblW w:w=\"0\" w:type=\"auto\"/>");
            out.push_str("<w:tblBorders>");
            for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
                let _ = write!(out, "<w:{} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>", side);
            }
            out.push_str("</w:tblBorders>");
            if t.cell_margin_left != DEFAULT_CELL_MARGIN || t.cell_margin_right != DEFAULT_CELL_MARGIN {
                let _ = write!(out, "<w:tblCellMar><w:left w:w=\"{}\" w:type=\"dxa\"/><w:right w:w=\"{}\" w:type=\"dxa\"/></w:tblCellMar>", t.cell_margin_left, t.cell_margin_right);
            }
            out.push_str("<w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/>");
            out.push_str("</w:tblPr>");
        }
    }
    match &t.raw_grid {
        Some(raw) => out.push_str(raw),
        None => {
            out.push_str("<w:tblGrid>");
            for w in &t.grid {
                let _ = write!(out, "<w:gridCol w:w=\"{}\"/>", w);
            }
            out.push_str("</w:tblGrid>");
        }
    }
}

pub(crate) fn open_row(t: &Table, row: usize, out: &mut String) {
    let r = t.rows.get(row).cloned().unwrap_or_default();
    let _ = write!(out, "<w:tr{}>", r.attrs);
    match &r.raw_trpr {
        Some(raw) => out.push_str(raw),
        None => {
            let mut body = String::new();
            if r.cant_split {
                body.push_str("<w:cantSplit/>");
            }
            if let Some(h) = r.height {
                let _ = write!(body, "<w:trHeight w:val=\"{}\"{}/>", h, if r.height_exact { " w:hRule=\"exact\"" } else { "" });
            }
            if r.header {
                body.push_str("<w:tblHeader/>");
            }
            if !body.is_empty() {
                let _ = write!(out, "<w:trPr>{}</w:trPr>", body);
            }
        }
    }
}

pub(crate) fn open_cell(t: &Table, row: usize, col: usize, out: &mut String) {
    let c = t.rows.get(row).and_then(|r| r.cells.get(col)).cloned().unwrap_or_else(TableCell::new);
    let _ = write!(out, "<w:tc{}>", c.attrs);
    match &c.raw_tcpr {
        Some(raw) => out.push_str(raw),
        None => {
            let width = c.width.unwrap_or_else(|| t.cell_extent(row, col).1);
            let _ = write!(out, "<w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>", width);
            if c.span() > 1 {
                let _ = write!(out, "<w:gridSpan w:val=\"{}\"/>", c.span());
            }
            match c.vmerge {
                Some(VMerge::Restart) => out.push_str("<w:vMerge w:val=\"restart\"/>"),
                Some(VMerge::Continue) => out.push_str("<w:vMerge/>"),
                None => {}
            }
            if let Some(s) = c.shading {
                let _ = write!(out, "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", s.hex());
            }
            out.push_str("</w:tcPr>");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::read::Ctx;
    use wp_core::model::*;

    #[test]
    fn parses_spans_and_merges() {
        let xml = r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblCellMar><w:left w:w="50" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>wide</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p/></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let mut ctx = Ctx::new(None, None);
        let mut pending = Vec::new();
        let (t, paras) = super::parse_table(xml, 7, &mut ctx, &mut pending).unwrap().unwrap();
        assert_eq!(t.grid, vec![1000, 2000]);
        assert_eq!(t.style.as_deref(), Some("TableGrid"));
        assert_eq!(t.cell_margin_left, 50);
        assert_eq!(t.rows.len(), 2);
        assert!(t.rows[0].header);
        assert_eq!(t.rows[0].cells[0].span, 2);
        assert_eq!(t.rows[1].cells[0].vmerge, Some(VMerge::Restart));
        let tags: Vec<String> = paras.iter().map(|p| format!("{}:{}", p.props.cell.unwrap().name(), p.text())).collect();
        assert_eq!(tags, ["A1:wide", "A2:", "B2:b", "B2:c"]);
        assert_eq!(t.cell_text_width(0, 0), 3000 - 50 - 108);
    }

    #[test]
    fn unexpected_structure_falls_back() {
        let xml = r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid><w:sdt><w:sdtContent><w:tr><w:tc><w:p/></w:tc></w:tr></w:sdtContent></w:sdt></w:tbl>"#;
        let mut ctx = Ctx::new(None, None);
        let mut pending = Vec::new();
        assert!(super::parse_table(xml, 1, &mut ctx, &mut pending).unwrap().is_none());
    }
}
