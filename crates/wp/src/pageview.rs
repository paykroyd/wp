//! Page view: the print layout drawn as pages (DESIGN.md §5.1).
//!
//! Every page is a box as wide as the paper, with its margins, header and
//! footer in place, and each printed line on the row its top twip falls in.
//! Glyphs are placed by rounding their twip x-position to a column and
//! never overwriting the previous glyph, so proportional text looks ragged
//! — page view shows where things truly are, not how they would look in a
//! monospace font (SPEC §6.4). Draft view remains the writing screen.

use crate::app::App;
use crate::ui::{self, Caps, Chrome};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph as RParagraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;
use wp_core::editor::{field_instr, field_part, FieldPart};
use wp_core::layout::{self, ParaLayout};
use wp_core::model::*;
use wp_core::Document;

/// The scale of a page on screen: twips per column and per row.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub hunit: Twips,
    pub vunit: Twips,
}

impl Geom {
    pub fn cols(&self, twips: Twips) -> u16 {
        ((twips + self.hunit / 2) / self.hunit).max(0) as u16
    }
}

/// The scale for a document area `width` cells wide: the body font's
/// average glyph width per column when the widest page fits, else enough
/// twips per column for it to.
pub fn geom(app: &mut App, width: u16) -> Geom {
    let body = app.ed.doc.styles.resolve_para_style_run(None);
    let tpc = layout::twips_per_cell(&body);
    let vunit = wp_core::metrics::line_height(&body).max(1);
    app.ed.ensure_layout();
    let max_w = app.ed.layout.sections.list.iter().map(|s| s.page_width).max().unwrap_or(12240);
    let avail = width.saturating_sub(2).max(20) as Twips;
    let hunit = tpc.max((max_w + avail - 1) / avail);
    Geom { hunit, vunit }
}

/// Where a line drawn on a page comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Src {
    /// A line of the document body (or of a table cell).
    Body { para: usize, line: usize },
    /// A line of a header or footer body.
    Hf { id: String, para: usize, line: usize },
    /// A table header row repeated at the top of a continuation page.
    Repeat { para: usize, line: usize },
}

/// A printed line placed on a page row.
#[derive(Clone, Debug)]
pub struct Placed {
    pub src: Src,
    /// Left edge of the line's text, in twips from the paper's left edge.
    pub x: Twips,
    /// Columns where a table cell edge falls (drawn as a rule).
    pub borders: Vec<u16>,
}

#[derive(Clone, Debug)]
pub enum PRow {
    Top,
    Bottom,
    /// The space between pages.
    Gap,
    /// Inside the page: the lines that start on this row (none = blank).
    Text(Vec<Placed>),
    /// The middle row of a page that is blank by design.
    BlankLabel,
}

/// Rows a page occupies on screen, including its borders and the gap after it.
pub fn rows_of_page(app: &mut App, page: usize, g: &Geom) -> usize {
    app.ed.ensure_layout();
    let si = app.ed.layout.pagination.section_of_page(page);
    let sec = &app.ed.layout.sections.list[si.min(app.ed.layout.sections.list.len() - 1)];
    inner_rows(sec, g) + 3
}

fn inner_rows(sec: &SectionProps, g: &Geom) -> usize {
    (sec.page_height / g.vunit).max(3) as usize
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Flow {
    Body(u8),
    Cell(CellRef),
    Header,
    Footer,
    Repeat(CellRef),
}

struct Placer {
    rows: Vec<Vec<Placed>>,
    last: HashMap<Flow, usize>,
    vunit: Twips,
}

impl Placer {
    fn put(&mut self, flow: Flow, y: Twips, placed: Placed) {
        let mut r = (y.max(0) / self.vunit) as usize;
        if let Some(&l) = self.last.get(&flow) {
            r = r.max(l + 1);
        }
        let r = r.min(self.rows.len() - 1);
        self.rows[r].push(placed);
        self.last.insert(flow, r);
    }
}

/// Build the rows of page `page`.
pub fn page_rows(app: &mut App, page: usize, g: &Geom) -> Vec<PRow> {
    app.ed.ensure_layout();
    let si = app.ed.layout.pagination.section_of_page(page);
    let sec = app.ed.layout.sections.list[si.min(app.ed.layout.sections.list.len() - 1)].clone();
    let inner = inner_rows(&sec, g);
    let mut out = Vec::with_capacity(inner + 3);
    out.push(PRow::Top);
    if app.ed.layout.pagination.is_blank(page) {
        for r in 0..inner {
            out.push(if r == inner / 2 { PRow::BlankLabel } else { PRow::Text(Vec::new()) });
        }
        out.push(PRow::Bottom);
        out.push(PRow::Gap);
        return out;
    }
    let mut pl = Placer { rows: vec![Vec::new(); inner], last: HashMap::new(), vunit: g.vunit };
    let first = app.ed.layout.pagination.page_first_in_section.get(page).copied().unwrap_or(true);
    let even = page % 2 == 1;
    // Everything that needs the editor mutably (header layouts, page
    // furniture) comes first; the body walk then borrows it read-only.
    let header = app.ed.hf_id_for(&sec, HfKind::Header, first, even).and_then(|id| app.ed.hf_layout(&id, &sec).map(|l| (id, l)));
    let footer = app.ed.hf_id_for(&sec, HfKind::Footer, first, even).and_then(|id| app.ed.hf_layout(&id, &sec).map(|l| (id, l)));
    let furniture = app.ed.page_furniture(page);
    let pg = &app.ed.layout.pagination;
    let ed = &app.ed;

    // Header.
    {
        if let Some((id, layouts)) = &header {
            let mut y = sec.header_distance;
            for (pi, l) in layouts.iter().enumerate() {
                y += l.space_before;
                for (li, line) in l.lines.iter().enumerate() {
                    pl.put(Flow::Header, y, Placed { src: Src::Hf { id: id.clone(), para: pi, line: li }, x: sec.margin_left + line.x, borders: Vec::new() });
                    y += line.height;
                }
                y += l.space_after;
            }
        }
    }

    // Body: the paragraphs with lines on this page.
    let top = sec.margin_top + furniture.extra_top;
    let col_pitch = sec.column_width() + sec.column_space;
    let n = ed.doc.paragraphs.len();
    let mut i = pg.start_of_page(page).map(|s| s.para).unwrap_or(0).min(n.saturating_sub(1));
    if let Some((start, _)) = ed.doc.table_bounds(i) {
        i = start;
    }
    let mut table_end: Option<usize> = None;
    while i < n {
        let placement = &pg.placements[i];
        let starts_later = placement.line_page.first().map_or(true, |&p| p as usize > page);
        let in_table = table_end.map_or(false, |e| i < e);
        if starts_later && !in_table {
            break;
        }
        let cell = ed.doc.cell_of(i);
        if cell.is_some() {
            if table_end.is_none() {
                table_end = ed.doc.table_bounds(i).map(|(_, e)| e);
            }
        } else {
            table_end = None;
        }
        let Some(layout) = ed.print_layout_ref(i) else { break };
        for (li, line) in layout.lines.iter().enumerate() {
            if placement.line_page.get(li).copied() != Some(page as u32) {
                continue;
            }
            let y = top + placement.line_y[li];
            let col_x = placement.col(li) as Twips * col_pitch;
            let (flow, borders, x) = match cell {
                Some(c) => {
                    let t = ed.doc.tables.get(&c.table);
                    let (bx, bw) = t.map(|t| t.cell_extent(c.row as usize, c.col as usize)).unwrap_or((0, 0));
                    let left = sec.margin_left + col_x + bx;
                    let borders = if t.map_or(false, |t| t.lines_visible()) { vec![g.cols(left), g.cols(left + bw)] } else { Vec::new() };
                    (Flow::Cell(c), borders, left + t.map(|t| t.cell_margin_left).unwrap_or(0) + line.x)
                }
                None => (Flow::Body(placement.col(li)), Vec::new(), sec.margin_left + col_x + line.x),
            };
            pl.put(flow, y, Placed { src: Src::Body { para: i, line: li }, x, borders });
        }
        i += 1;
    }

    // Table header rows repeated at the top of a continuation page.
    for &(p, table) in &pg.repeated_headers {
        if p as usize != page {
            continue;
        }
        let Some((start, _)) = ed.doc.table_span(table) else { continue };
        let Some(rows) = ed.doc.table_paras(start) else { continue };
        let Some(t) = ed.doc.tables.get(&table) else { continue };
        let mut y = top;
        for (ri, row) in rows.iter().enumerate() {
            if !t.rows.get(ri).map_or(false, |r| r.header) {
                break;
            }
            let mut row_h = 0;
            for (ci, cellp) in row.iter().enumerate() {
                let c = CellRef::new(table, ri as u32, ci as u32);
                let (bx, bw) = t.cell_extent(ri, ci);
                let left = sec.margin_left + bx;
                let mut yy = y;
                for &p in cellp {
                    let Some(layout) = ed.print_layout_ref(p) else { continue };
                    yy += layout.space_before;
                    for (li, line) in layout.lines.iter().enumerate() {
                        pl.put(Flow::Repeat(c), yy, Placed { src: Src::Repeat { para: p, line: li }, x: left + t.cell_margin_left + line.x, borders: vec![g.cols(left), g.cols(left + bw)] });
                        yy += line.height;
                    }
                    yy += layout.space_after;
                }
                row_h = row_h.max(yy - y);
            }
            y += row_h;
        }
    }

    // Footer: its bottom sits footer_distance above the paper's bottom.
    {
        if let Some((id, layouts)) = &footer {
            let h = wp_core::section::body_height(layouts);
            let mut y = sec.page_height - sec.footer_distance - h;
            for (pi, l) in layouts.iter().enumerate() {
                y += l.space_before;
                for (li, line) in l.lines.iter().enumerate() {
                    pl.put(Flow::Footer, y, Placed { src: Src::Hf { id: id.clone(), para: pi, line: li }, x: sec.margin_left + line.x, borders: Vec::new() });
                    y += line.height;
                }
                y += l.space_after;
            }
        }
    }

    out.extend(pl.rows.into_iter().map(PRow::Text));
    out.push(PRow::Bottom);
    out.push(PRow::Gap);
    out
}

/// One character placed on a row.
#[derive(Clone, Debug)]
pub struct Glyph {
    pub col: u16,
    pub ch: char,
    pub style: Style,
    /// The document position of the item (body lines only).
    pub pos: Option<Pos>,
}

/// The glyphs of a placed line, and the cursor's column when it is on it.
pub fn glyphs(app: &mut App, placed: &Placed, page: usize, g: &Geom, caps: Caps, sel: Option<Range>, cursor: Pos) -> (Vec<Glyph>, Option<u16>) {
    let th = ui::theme(app, caps);
    let base_hp = ui::body_size_hp(&app.ed.doc);
    app.ed.ensure_layout();
    let page_no = app.ed.layout.pagination.page_number(page, &app.ed.layout.sections);
    let page_count = app.ed.layout.pagination.page_count();
    // Which document and layout the line belongs to: the body's, or a
    // header's own (cached as a document of its own).
    let (hf_doc, layout, para, li, is_body): (Option<Rc<Document>>, Rc<Vec<ParaLayout>>, usize, usize, bool) = match &placed.src {
        Src::Body { para, line } | Src::Repeat { para, line } => {
            let l = app.ed.print_layout(*para).clone();
            (None, Rc::new(vec![l]), *para, *line, matches!(placed.src, Src::Body { .. }))
        }
        Src::Hf { id, para, line } => {
            let si = app.ed.layout.pagination.section_of_page(page);
            let sec = app.ed.layout.sections.list[si.min(app.ed.layout.sections.list.len() - 1)].clone();
            let Some(doc) = app.ed.hf_doc(id) else { return (Vec::new(), None) };
            let Some(layouts) = app.ed.hf_layout(id, &sec) else { return (Vec::new(), None) };
            (Some(doc), layouts, *para, *line, false)
        }
    };
    let doc: &Document = match &hf_doc {
        Some(d) => d,
        None => &app.ed.doc,
    };
    let Some(pl) = layout.get(if hf_doc.is_some() { para } else { 0 }) else { return (Vec::new(), None) };
    let Some(line) = pl.lines.get(li) else { return (Vec::new(), None) };
    let p = &doc.paragraphs[para];
    let runs = doc.runs(para);
    let xs = layout::item_x_positions(doc, para, line);
    let mut out: Vec<Glyph> = Vec::new();
    let mut last_col: Option<u16> = None;
    let mut cursor_col: Option<u16> = None;
    let place = |out: &mut Vec<Glyph>, last_col: &mut Option<u16>, x: Twips, ch: char, style: Style, pos: Option<Pos>| {
        let mut col = g.cols(x);
        if let Some(l) = *last_col {
            if col <= l {
                col = l + 1;
            }
        }
        *last_col = Some(col);
        out.push(Glyph { col, ch, style, pos });
    };
    // List label on the first line.
    if li == 0 {
        if let Some(label) = &pl.label {
            let st = ui::style_for(&label.props, base_hp, caps, &th);
            let mut x = placed.x - line.x + label.x;
            for c in label.text.chars() {
                place(&mut out, &mut last_col, x, c, st, None);
                x += wp_core::metrics::advance(&label.props, c);
            }
        }
    }
    let mut ri = 0;
    let mut field: Option<u32> = None; // inside a PAGE/NUMPAGES simple field: skip its stored result
    // Complex fields: the instruction between begin and separate, then a
    // result to replace (or keep) until end.
    let mut complex_instr: Option<String> = None;
    let mut complex_skip = false;
    let subst_for = |instr: &str| -> Option<String> {
        match instr.split_whitespace().next().unwrap_or("").to_ascii_uppercase().as_str() {
            "PAGE" => Some(page_no.to_string()),
            "NUMPAGES" => Some(page_count.to_string()),
            _ => None,
        }
    };
    for i in line.start..line.end {
        while ri + 1 < runs.len() && runs[ri].end <= i {
            ri += 1;
        }
        let x = placed.x + xs[i - line.start];
        let pos = if is_body { Some(Pos::new(para, i)) } else { None };
        if is_body && Pos::new(para, i) == cursor && cursor_col.is_none() {
            cursor_col = Some(last_col.map_or(g.cols(x), |l| g.cols(x).max(l + 1)));
        }
        let mut st = ui::style_for(&runs[ri].props, base_hp, caps, &th);
        if is_body && sel.map_or(false, |r| r.contains(Pos::new(para, i))) {
            st = st.add_modifier(Modifier::REVERSED);
        }
        match &p.items[i] {
            Item::Char(c) if field.is_none() && !complex_skip => {
                let c = if runs[ri].props.all_caps.unwrap_or(false) { c.to_uppercase().next().unwrap_or(*c) } else { *c };
                if (c as u32) >= 0x20 && c != '\u{ad}' {
                    place(&mut out, &mut last_col, x, c, st, pos);
                }
            }
            Item::Char(_) => {}
            Item::Code(Code::Opaque(o)) => match o.kind {
                OpaqueKind::Element if field_part(o).is_some() => match field_part(o).unwrap() {
                    FieldPart::Begin => complex_instr = Some(String::new()),
                    FieldPart::Instr(t) => {
                        if let Some(ci) = &mut complex_instr {
                            ci.push_str(&t);
                        }
                    }
                    FieldPart::Separate => {
                        if let Some(text) = complex_instr.take().and_then(|ci| subst_for(&ci)) {
                            complex_skip = true;
                            let mut xx = x;
                            for c in text.chars() {
                                place(&mut out, &mut last_col, xx, c, st, pos);
                                xx += wp_core::metrics::advance(&runs[ri].props, c);
                            }
                        }
                    }
                    FieldPart::End => {
                        complex_skip = false;
                        complex_instr = None;
                    }
                },
                OpaqueKind::Open(id) => {
                    let subst = field_instr(o).as_deref().and_then(subst_for);
                    if let Some(text) = subst {
                        field = Some(id);
                        let mut xx = x;
                        for c in text.chars() {
                            place(&mut out, &mut last_col, xx, c, st, pos);
                            xx += wp_core::metrics::advance(&runs[ri].props, c);
                        }
                    }
                }
                OpaqueKind::Close(id) => {
                    if field == Some(id) {
                        field = None;
                    }
                }
                OpaqueKind::Element => {
                    if matches!(o.label.as_str(), "Drawing" | "Picture" | "Object") {
                        for c in format!("[{}]", o.label).chars() {
                            place(&mut out, &mut last_col, x, c, st.fg(th.dim), pos);
                        }
                    }
                }
            },
            _ => {}
        }
    }
    if is_body && cursor.para == para && cursor_col.is_none() {
        let last = li + 1 == pl.lines.len();
        if cursor.idx >= line.start && (cursor.idx < line.end || (last && cursor.idx >= line.end)) {
            let end_x = placed.x + xs[(cursor.idx - line.start).min(xs.len() - 1)];
            cursor_col = Some(last_col.map_or(g.cols(end_x), |l| g.cols(end_x).max(l + 1)));
        }
    }
    (out, cursor_col)
}

/// The screen row of the cursor within its page.
fn cursor_row(app: &mut App, g: &Geom) -> (usize, usize) {
    app.ed.ensure_layout();
    let c = app.ed.cursor;
    let li = app.ed.print_layout(c.para).line_of(c.idx);
    let page = app.ed.layout.pagination.page_of(c.para, li);
    let rows = page_rows(app, page, g);
    for (r, row) in rows.iter().enumerate() {
        if let PRow::Text(lines) = row {
            if lines.iter().any(|l| l.src == Src::Body { para: c.para, line: li }) {
                return (page, r);
            }
        }
    }
    (page, 1)
}

fn abs_row(app: &mut App, at: (usize, usize), g: &Geom) -> usize {
    let mut n = 0;
    for p in 0..at.0 {
        n += rows_of_page(app, p, g);
    }
    n + at.1
}

fn from_abs(app: &mut App, mut abs: usize, g: &Geom) -> (usize, usize) {
    let pages = app.ed.page_count();
    let mut p = 0;
    while p < pages {
        let n = rows_of_page(app, p, g);
        if abs < n {
            return (p, abs);
        }
        abs -= n;
        p += 1;
    }
    (pages.saturating_sub(1), 0)
}

/// Scroll so the cursor's row is on screen.
pub fn ensure_cursor_visible(app: &mut App, visible: usize, g: &Geom) {
    let pages = app.ed.page_count();
    if app.page_scroll.0 >= pages {
        app.page_scroll = (0, 0);
    }
    let cur = cursor_row(app, g);
    let cur_abs = abs_row(app, cur, g);
    let scroll_abs = abs_row(app, app.page_scroll, g);
    if cur_abs < scroll_abs {
        app.page_scroll = from_abs(app, cur_abs.saturating_sub(2), g);
    } else if cur_abs >= scroll_abs + visible {
        app.page_scroll = from_abs(app, cur_abs + 3 - visible.min(cur_abs + 3), g);
    }
}

/// Scroll the page view by `delta` rows without moving the cursor.
pub fn scroll_by(app: &mut App, delta: i32) {
    let g = geom(app, app.size.0);
    let abs = abs_row(app, app.page_scroll, &g) as i64 + delta as i64;
    let pages = app.ed.page_count();
    let total: usize = (0..pages).map(|p| rows_of_page(app, p, &g)).sum();
    let visible = app.doc_rows() as usize;
    let max = total.saturating_sub(visible.min(total)) as i64;
    let abs = abs.clamp(0, max.max(0)) as usize;
    app.page_scroll = from_abs(app, abs, &g);
    app.page_followed = Some(app.ed.cursor);
    app.needs_redraw = true;
}

/// The rows from the scroll position: `(page, row, PRow)`.
fn visible_rows(app: &mut App, count: usize, g: &Geom) -> Vec<(usize, PRow)> {
    let pages = app.ed.page_count();
    let (mut page, mut row) = app.page_scroll;
    let mut out = Vec::with_capacity(count);
    let mut cache: Option<(usize, Vec<PRow>)> = None;
    while out.len() < count && page < pages {
        if cache.as_ref().map_or(true, |c| c.0 != page) {
            cache = Some((page, page_rows(app, page, g)));
        }
        let rows = &cache.as_ref().unwrap().1;
        if row >= rows.len() {
            page += 1;
            row = 0;
            continue;
        }
        out.push((page, rows[row].clone()));
        row += 1;
    }
    out
}

/// Draw the page view; returns the cursor's screen position.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect, caps: Caps, ch: &Chrome) -> Option<(u16, u16)> {
    let rows = area.height as usize;
    if rows == 0 {
        return None;
    }
    let th = ui::theme(app, caps);
    let g = geom(app, area.width);
    if app.page_followed != Some(app.ed.cursor) {
        ensure_cursor_visible(app, rows, &g);
        app.page_followed = Some(app.ed.cursor);
    }
    let sel = app.ed.selection();
    let cursor = app.ed.cursor;
    let border = Style::default().fg(th.dim);
    let mut cursor_xy = None;
    let mut lines_out: Vec<Line> = Vec::with_capacity(rows);
    let visible = visible_rows(app, rows, &g);
    for (ri, (page, row)) in visible.into_iter().enumerate() {
        let y = area.y + ri as u16;
        let si = app.ed.layout.pagination.section_of_page(page);
        let sec = app.ed.layout.sections.list[si.min(app.ed.layout.sections.list.len() - 1)].clone();
        let width = g.cols(sec.page_width).max(10).min(area.width.saturating_sub(2));
        let x0 = (area.width.saturating_sub(width + 2)) / 2;
        let pad = " ".repeat(x0 as usize);
        match row {
            PRow::Gap => lines_out.push(Line::default()),
            PRow::Top | PRow::Bottom => {
                let (l, r) = if caps.ascii {
                    ('+', '+')
                } else if matches!(row, PRow::Top) {
                    ('┌', '┐')
                } else {
                    ('└', '┘')
                };
                let s = format!("{}{}{}{}", pad, l, ch.h.repeat(width as usize), r);
                lines_out.push(Line::from(Span::styled(s, border)));
            }
            PRow::BlankLabel => {
                let label = "(blank page)";
                let left = (width as usize).saturating_sub(label.width()) / 2;
                let right = (width as usize).saturating_sub(label.width() + left);
                lines_out.push(Line::from(vec![Span::raw(pad.clone()), Span::styled(ch.v.to_string(), border), Span::raw(" ".repeat(left)), Span::styled(label, border), Span::raw(" ".repeat(right)), Span::styled(ch.v.to_string(), border)]));
            }
            PRow::Text(placed) => {
                let mut cells: Vec<Option<(char, Style)>> = vec![None; width as usize];
                for pl in &placed {
                    for &b in &pl.borders {
                        if (b as usize) < cells.len() && cells[b as usize].is_none() {
                            cells[b as usize] = Some((ch.v, border));
                        }
                    }
                    let (gs, ccol) = glyphs(app, pl, page, &g, caps, sel, cursor);
                    for gl in gs {
                        if (gl.col as usize) < cells.len() {
                            cells[gl.col as usize] = Some((gl.ch, gl.style));
                        }
                    }
                    if let Some(c) = ccol {
                        cursor_xy = Some((area.x + x0 + 1 + c.min(width.saturating_sub(1)), y));
                    }
                }
                let mut spans: Vec<Span> = vec![Span::raw(pad.clone()), Span::styled(ch.v.to_string(), border)];
                let mut buf = String::new();
                let mut cur: Option<Style> = None;
                for c in cells {
                    let (chr, st) = c.unwrap_or((' ', Style::default()));
                    if Some(st) != cur {
                        if !buf.is_empty() {
                            spans.push(Span::styled(std::mem::take(&mut buf), cur.unwrap_or_default()));
                        }
                        cur = Some(st);
                    }
                    buf.push(chr);
                }
                if !buf.is_empty() {
                    spans.push(Span::styled(buf, cur.unwrap_or_default()));
                }
                spans.push(Span::styled(ch.v.to_string(), border));
                lines_out.push(Line::from(spans));
            }
        }
    }
    while lines_out.len() < rows {
        lines_out.push(Line::default());
    }
    f.render_widget(RParagraph::new(lines_out), area);
    cursor_xy
}

/// The document position under a screen cell of the page view.
pub fn pos_at(app: &mut App, x: u16, y: u16) -> Option<Pos> {
    let rows = app.doc_rows() as usize;
    let width_total = app.size.0;
    let g = geom(app, width_total);
    let visible = visible_rows(app, rows, &g);
    let (page, row) = visible.get(y as usize)?.clone();
    let PRow::Text(placed) = row else { return None };
    let si = app.ed.layout.pagination.section_of_page(page);
    let sec = app.ed.layout.sections.list[si.min(app.ed.layout.sections.list.len() - 1)].clone();
    let width = g.cols(sec.page_width).max(10).min(width_total.saturating_sub(2));
    let x0 = (width_total.saturating_sub(width + 2)) / 2 + 1;
    let col = x.checked_sub(x0)?;
    let mut best: Option<(u16, Pos)> = None;
    let mut line_end: Option<Pos> = None;
    for pl in &placed {
        let Src::Body { para, line } = pl.src else { continue };
        let (gs, _) = glyphs(app, pl, page, &g, Caps { ascii: false, colors: false, truecolor: false }, None, Pos::default());
        for gl in gs {
            if let Some(p) = gl.pos {
                let d = (gl.col as i32 - col as i32).unsigned_abs() as u16;
                if gl.col <= col && best.map_or(true, |(bd, _)| d <= bd) {
                    best = Some((d, p));
                }
            }
        }
        let l = app.ed.print_layout(para).lines[line].clone();
        let mut end = l.end;
        let items = &app.ed.doc.paragraphs[para].items;
        if end > l.start && end < items.len() && (items[end - 1].is_whitespace() || matches!(items[end - 1], Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak) | Item::Code(Code::ColumnBreak))) {
            end -= 1;
        }
        if line_end.is_none() {
            line_end = Some(Pos::new(para, end));
        }
        if let Some((_, p)) = best {
            if gs_after(app, pl, page, &g, col) {
                return Some(Pos::new(para, end));
            }
            return Some(p);
        }
    }
    line_end
}

/// True when `col` is past the last glyph of the line.
fn gs_after(app: &mut App, pl: &Placed, page: usize, g: &Geom, col: u16) -> bool {
    let (gs, _) = glyphs(app, pl, page, g, Caps { ascii: false, colors: false, truecolor: false }, None, Pos::default());
    gs.last().map_or(true, |l| col > l.col)
}
