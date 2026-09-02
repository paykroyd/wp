//! Layout: print-accurate line breaking and pagination in twips, and the
//! terminal-width wrap used by draft view. See DESIGN.md §5.

use crate::document::{Document, Run};
use crate::metrics;
use crate::model::*;
use crate::numbering::{ListLabel, Suffix};
use crate::section::Sections;

/// Default tab interval when no tab stop applies (Word: 0.5").
pub const DEFAULT_TAB: Twips = 720;

/// One laid-out line of a paragraph in print geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    /// Vertical advance including line spacing.
    pub height: Twips,
    /// Width of the content (excluding trailing spaces).
    pub width: Twips,
    /// Left edge of the line relative to the text area, after alignment.
    pub x: Twips,
    /// Width available to this line (for justification / Pos calculations).
    pub avail: Twips,
    /// The line ends with a hard page break code.
    pub page_break_after: bool,
}

/// A list label placed on the first line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelPlacement {
    pub text: String,
    /// Left edge relative to the text area.
    pub x: Twips,
    pub width: Twips,
    pub props: RunProps,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParaLayout {
    pub lines: Vec<Line>,
    pub label: Option<LabelPlacement>,
    pub space_before: Twips,
    pub space_after: Twips,
    pub keep_next: bool,
    pub keep_lines: bool,
    pub widow_control: bool,
    pub page_break_before: bool,
}

impl ParaLayout {
    pub fn height(&self) -> Twips {
        self.lines.iter().map(|l| l.height).sum()
    }
    pub fn line_of(&self, idx: usize) -> usize {
        // The line containing item index `idx` (a position before item idx).
        for (i, l) in self.lines.iter().enumerate() {
            if idx < l.end {
                return i;
            }
        }
        self.lines.len().saturating_sub(1)
    }
}

/// Where each item of a line sits horizontally; used for Pos and page view.
pub fn item_x_positions(doc: &Document, para: usize, line: &Line) -> Vec<Twips> {
    let runs = doc.runs(para);
    let p = &doc.paragraphs[para];
    let pp = doc.para_props(para);
    let mut xs = Vec::with_capacity(line.end - line.start + 1);
    let mut x = 0;
    let mut ri = 0;
    for i in line.start..line.end {
        while ri + 1 < runs.len() && runs[ri].end <= i {
            ri += 1;
        }
        xs.push(x);
        x += item_advance(&p.items[i], &runs[ri].props, x, &pp, line.avail);
    }
    xs.push(x);
    xs
}

fn next_tab_stop(pp: &ParaProps, x: Twips, avail: Twips) -> Twips {
    // Custom stops are relative to the left margin of the text area; our x is
    // relative to the paragraph's left indent, so convert.
    let abs = x + pp.indent_left();
    for t in &pp.tabs {
        if !t.clear && t.pos > abs {
            return (t.pos - pp.indent_left()).min(avail);
        }
    }
    let next = (abs / DEFAULT_TAB + 1) * DEFAULT_TAB;
    (next - pp.indent_left()).min(avail.max(x + 1))
}

fn item_advance(item: &Item, props: &RunProps, x: Twips, pp: &ParaProps, avail: Twips) -> Twips {
    match item {
        Item::Char(c) => {
            let c = if props.all_caps.unwrap_or(false) { c.to_ascii_uppercase() } else { *c };
            metrics::advance(props, c)
        }
        Item::Code(Code::Tab) => (next_tab_stop(pp, x, avail) - x).max(0),
        Item::Code(_) => 0,
    }
}

/// Where a list label sits and where the first line's text starts after it.
fn place_label(doc: &Document, para: usize, pp: &ParaProps, label: &ListLabel) -> (LabelPlacement, Twips) {
    let props = doc.base_run_props(para).merge(&label.run);
    let width: Twips = label.text.chars().map(|c| metrics::advance(&props, c)).sum();
    let base_x = pp.indent_left();
    let x = (base_x + pp.first_line_offset()).max(0);
    let end = x + width;
    let text_x = match label.suffix {
        Suffix::Tab => {
            if end < base_x {
                base_x
            } else {
                // Past the hanging position: the tab goes to the next stop.
                let mut stops: Vec<Twips> = pp.tabs.iter().filter(|t| !t.clear && t.pos > end).map(|t| t.pos).collect();
                stops.sort();
                stops.first().copied().unwrap_or((end / DEFAULT_TAB + 1) * DEFAULT_TAB)
            }
        }
        Suffix::Space => end + metrics::advance(&props, ' '),
        Suffix::Nothing => end,
    };
    (LabelPlacement { text: label.text.clone(), x, width, props }, text_x)
}

/// Lay out one paragraph against the width of the section that governs it
/// (found by a forward scan; the editor uses [`layout_paragraph_in`] with
/// its cached section map).
pub fn layout_paragraph(doc: &Document, para: usize, label: Option<&ListLabel>) -> ParaLayout {
    let sect = doc.section_at(para).clone();
    layout_paragraph_in(doc, para, label, &sect)
}

/// Lay out one paragraph against the text column of `sect`.
pub fn layout_paragraph_in(doc: &Document, para: usize, label: Option<&ListLabel>, sect: &SectionProps) -> ParaLayout {
    let p = &doc.paragraphs[para];
    let pp = doc.para_props(para);
    let runs: Vec<Run> = doc.runs(para);
    // Inside a table cell the column, not the page, bounds the text.
    let text_width = doc.cell_text_width(para).unwrap_or_else(|| sect.column_width());
    let base_x = pp.indent_left();
    let mut first_off = pp.first_line_offset();
    let label = label.filter(|l| !l.text.is_empty()).map(|l| place_label(doc, para, &pp, l));
    if let Some((_, text_x)) = &label {
        first_off = text_x - base_x;
    }
    let label = label.map(|(l, _)| l);
    let avail_rest = (text_width - pp.indent_right() - base_x).max(360);
    let avail_first = (text_width - pp.indent_right() - base_x - first_off).max(360);

    let mut lines: Vec<Line> = Vec::new();
    let mut ri = 0usize;
    let mut line_start = 0usize;
    let mut x: Twips = 0;
    let mut width_at_last_nonspace: Twips = 0;
    let mut last_break: Option<(usize, Twips)> = None; // (index after which we may break, width before trailing spaces)
    let mut max_h: Twips = 0;
    let mut had_glyph = false;
    let n = p.items.len();

    let line_height_of = |props: &RunProps| metrics::line_height(props);
    let spacing = |h: Twips| -> Twips {
        match pp.line_spacing() {
            LineSpacing::Auto(m) => ((h as i64) * (m as i64) / 240) as Twips,
            LineSpacing::Exact(t) => t,
            LineSpacing::AtLeast(t) => h.max(t),
        }
    };
    let empty_h = {
        let base = doc.base_run_props(para).merge(&pp.mark);
        line_height_of(&base)
    };

    let mut i = 0usize;
    while i < n {
        while ri + 1 < runs.len() && runs[ri].end <= i {
            ri += 1;
        }
        let props = &runs[ri].props;
        let first = lines.is_empty();
        let avail = if first { avail_first } else { avail_rest };
        let item = &p.items[i];

        let forced = matches!(item, Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak));
        if forced {
            let h = if had_glyph { max_h } else { empty_h };
            lines.push(Line {
                start: line_start,
                end: i + 1,
                height: spacing(h.max(line_height_of(props))),
                width: width_at_last_nonspace,
                x: 0,
                avail,
                page_break_after: matches!(item, Item::Code(Code::PageBreak)),
            });
            line_start = i + 1;
            x = 0;
            width_at_last_nonspace = 0;
            last_break = None;
            max_h = 0;
            had_glyph = false;
            i += 1;
            continue;
        }

        let adv = item_advance(item, props, x, &pp, avail);
        let is_space = matches!(item, Item::Char(c) if *c == ' ' || *c == '\u{2009}') || matches!(item, Item::Code(Code::Tab));
        let is_zero = adv == 0 && !is_space;

        if !is_space && !is_zero && x + adv > avail && had_glyph {
            // Need to break.
            if let Some((at, w)) = last_break {
                if at > line_start {
                    lines.push(Line {
                        start: line_start,
                        end: at,
                        height: spacing(max_h),
                        width: w,
                        x: 0,
                        avail,
                        page_break_after: false,
                    });
                    line_start = at;
                    // re-measure from `at`
                    i = at;
                    x = 0;
                    width_at_last_nonspace = 0;
                    last_break = None;
                    max_h = 0;
                    had_glyph = false;
                    // ri must be rewound since i moved back
                    ri = 0;
                    continue;
                }
            }
            // No break opportunity: break before this glyph.
            lines.push(Line {
                start: line_start,
                end: i,
                height: spacing(max_h),
                width: width_at_last_nonspace,
                x: 0,
                avail,
                page_break_after: false,
            });
            line_start = i;
            x = 0;
            width_at_last_nonspace = 0;
            last_break = None;
            max_h = 0;
            had_glyph = false;
            continue;
        }

        x += adv;
        if !is_zero {
            max_h = max_h.max(line_height_of(props));
        }
        if is_space {
            last_break = Some((i + 1, width_at_last_nonspace));
            had_glyph = true;
        } else if !is_zero {
            had_glyph = true;
            width_at_last_nonspace = x;
            // Allow breaking after a hyphen or dash.
            if matches!(item, Item::Char('-') | Item::Char('\u{2013}') | Item::Char('\u{2014}')) {
                last_break = Some((i + 1, x));
            }
        }
        i += 1;
    }
    // Final line (always at least one line, even for empty paragraphs).
    let h = if had_glyph { max_h } else { empty_h };
    let avail = if lines.is_empty() { avail_first } else { avail_rest };
    lines.push(Line {
        start: line_start,
        end: n,
        height: spacing(h),
        width: width_at_last_nonspace,
        x: 0,
        avail,
        page_break_after: false,
    });

    // Alignment: compute x offsets.
    for (li, l) in lines.iter_mut().enumerate() {
        let left = base_x + if li == 0 { first_off } else { 0 };
        let slack = (l.avail - l.width).max(0);
        l.x = match pp.align() {
            Align::Left | Align::Justify => left,
            Align::Center => left + slack / 2,
            Align::Right => left + slack,
        };
    }

    ParaLayout {
        lines,
        label,
        space_before: pp.space_before(),
        space_after: pp.space_after(),
        keep_next: pp.keep_next(),
        keep_lines: pp.keep_lines(),
        widow_control: pp.widow_control(),
        page_break_before: pp.page_break_before(),
    }
}

/// Where a page begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageStart {
    pub para: usize,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ParaPlacement {
    /// Page index (0-based) of each line.
    pub line_page: Vec<u32>,
    /// Y of each line's top relative to the top of the text area.
    pub line_y: Vec<Twips>,
}

/// Which header and footer a page shows, and how much of the page they
/// take beyond the margins. Computed by the caller of [`paginate`] (the
/// editor lays out the header bodies); the pager only needs the heights.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PageFurniture {
    /// Extra space the header needs below the top margin, and the footer
    /// above the bottom margin (0 when they fit inside the margins).
    pub extra_top: Twips,
    pub extra_bottom: Twips,
}

/// Header/footer space for a page: given the section index, whether the
/// page is the first of its section, and whether it is an even page.
pub type FurnitureFn<'a> = &'a dyn Fn(usize, bool, bool) -> PageFurniture;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pagination {
    pub pages: Vec<PageStart>,
    pub placements: Vec<ParaPlacement>,
    /// Section index (into [`Sections::list`]) of each page.
    pub page_section: Vec<u32>,
    /// Whether each page is the first page of its section.
    pub page_first_in_section: Vec<bool>,
    /// Text height available on each page (after headers and footers).
    pub page_text_height: Vec<Twips>,
    /// Pages on which a table continues from the previous page and repeats
    /// its header rows at the top: `(page, table id)`.
    pub repeated_headers: Vec<(u32, u32)>,
    /// Pages that are blank because an odd- or even-page section start
    /// needed one.
    pub blank_pages: Vec<u32>,
}

impl Pagination {
    pub fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }
    pub fn page_of(&self, para: usize, line: usize) -> usize {
        self.placements
            .get(para)
            .and_then(|p| p.line_page.get(line).copied())
            .unwrap_or(0) as usize
    }
    /// The paragraph/line at which page `page` (0-based) begins.
    pub fn start_of_page(&self, page: usize) -> Option<PageStart> {
        self.pages.get(page).copied()
    }
    pub fn section_of_page(&self, page: usize) -> usize {
        self.page_section.get(page).copied().unwrap_or(0) as usize
    }
    pub fn is_blank(&self, page: usize) -> bool {
        self.blank_pages.contains(&(page as u32))
    }
    /// The number printed on a page: pages count from 1, or from the
    /// section's `page_start` when it restarts numbering.
    pub fn page_number(&self, page: usize, secs: &Sections) -> i32 {
        let mut n = 1;
        for p in 0..=page.min(self.pages.len().saturating_sub(1)) {
            let si = self.section_of_page(p);
            if self.page_first_in_section.get(p).copied().unwrap_or(false) {
                if let Some(s) = secs.list.get(si).and_then(|s| s.page_start) {
                    n = s;
                    continue;
                }
            }
            if p > 0 {
                n += 1;
            }
        }
        n
    }
}

/// Paginate the document given per-paragraph layouts, with no headers or
/// footers taken into account.
pub fn paginate(doc: &Document, layouts: &[ParaLayout]) -> Pagination {
    let secs = Sections::build(doc);
    paginate_with(doc, layouts, &secs, &|_, _, _| PageFurniture::default())
}

/// Paginate the document. Sections start where their `start` says; table
/// rows are placed as units — the cells of a row start at the same y, the
/// row is as tall as its tallest cell, a row that may not split moves to
/// the next page whole (unless it is taller than a page), and header rows
/// repeat at the top of each page the table continues on.
pub fn paginate_with(doc: &Document, layouts: &[ParaLayout], secs: &Sections, furniture: FurnitureFn<'_>) -> Pagination {
    let n = layouts.len();
    let mut pg = Pager {
        secs,
        furniture,
        sec: 0,
        page_h: 0,
        pages: vec![PageStart { para: 0, line: 0 }],
        page_section: vec![0],
        page_first: vec![true],
        page_text_height: vec![0],
        repeated_headers: Vec::new(),
        blank_pages: Vec::new(),
        placements: Vec::with_capacity(n),
        y: 0,
        page: 0,
        after_hard_break: true, // start of document behaves like one
    };
    pg.page_h = pg.height_for(0, 0);
    pg.page_text_height[0] = pg.page_h;
    let mut i = 0;
    while i < n {
        pg.enter_section(secs.index_of(i), i);
        if let Some(c) = doc.cell_of(i) {
            let (_, end) = doc.table_bounds(i).unwrap();
            let rows = doc.table_paras(i).unwrap();
            let table = doc.tables.get(&c.table).cloned().unwrap_or_default();
            // Header rows: the leading rows flagged to repeat, and the
            // height they take on a continuation page.
            let n_header = table.rows.iter().take_while(|r| r.header).count().min(rows.len());
            let header_h: Twips = rows.iter().take(n_header).enumerate().map(|(ri, row)| pg.row_height(row, layouts, table.rows.get(ri))).sum();
            for (ri, row) in rows.iter().enumerate() {
                let repeat = if ri >= n_header && n_header > 0 { Some((c.table, header_h)) } else { None };
                pg.place_row(row, layouts, table.rows.get(ri), repeat);
            }
            if pg.y >= pg.page_h && end < n {
                pg.new_page(end, 0);
                pg.after_hard_break = false;
            }
            i = end;
            continue;
        }
        pg.place_para(i, layouts);
        i += 1;
    }
    Pagination {
        pages: pg.pages,
        placements: pg.placements,
        page_section: pg.page_section,
        page_first_in_section: pg.page_first,
        page_text_height: pg.page_text_height,
        repeated_headers: pg.repeated_headers,
        blank_pages: pg.blank_pages,
    }
}

struct Pager<'a> {
    secs: &'a Sections,
    furniture: FurnitureFn<'a>,
    sec: usize,
    /// Text height of the current page.
    page_h: Twips,
    pages: Vec<PageStart>,
    page_section: Vec<u32>,
    page_first: Vec<bool>,
    page_text_height: Vec<Twips>,
    repeated_headers: Vec<(u32, u32)>,
    blank_pages: Vec<u32>,
    placements: Vec<ParaPlacement>,
    y: Twips,
    page: u32,
    after_hard_break: bool,
}

impl<'a> Pager<'a> {
    /// Text height of page `page` (0-based) when it belongs to section `sec`.
    fn height_for(&self, sec: usize, page: u32) -> Twips {
        let s = &self.secs.list[sec.min(self.secs.list.len() - 1)];
        let first = self.page_first.get(page as usize).copied().unwrap_or(true);
        let even = page % 2 == 1;
        let f = (self.furniture)(sec, first, even);
        (s.text_height() - f.extra_top - f.extra_bottom).max(720)
    }

    fn new_page(&mut self, para: usize, line: usize) {
        self.page += 1;
        self.y = 0;
        self.pages.push(PageStart { para, line });
        self.page_section.push(self.sec as u32);
        self.page_first.push(false);
        self.page_h = self.height_for(self.sec, self.page);
        self.page_text_height.push(self.page_h);
    }

    /// Move to section `s` before placing paragraph `para`.
    fn enter_section(&mut self, s: usize, para: usize) {
        if s == self.sec {
            return;
        }
        self.sec = s;
        let start = self.secs.list[s].start;
        let at_top = self.y == 0;
        match start {
            SectionStart::Continuous => {
                // The rest of this page keeps its geometry; the section's
                // own applies from its next page.
                if at_top {
                    self.retag_page(true);
                }
            }
            SectionStart::NextPage | SectionStart::EvenPage | SectionStart::OddPage => {
                if at_top {
                    self.retag_page(true);
                } else {
                    self.new_page(para, 0);
                    self.retag_page(true);
                }
                let want_odd = start == SectionStart::OddPage;
                let want_even = start == SectionStart::EvenPage;
                let is_odd = self.page % 2 == 0; // page 0 is page 1
                if (want_odd && !is_odd) || (want_even && is_odd) {
                    // A blank page to reach the right side.
                    self.blank_pages.push(self.page);
                    self.page_first[self.page as usize] = false;
                    self.new_page(para, 0);
                    self.retag_page(true);
                }
                self.after_hard_break = true;
            }
        }
    }

    /// The current page belongs to the current section (used when a
    /// section starts at the top of a page that was opened for the
    /// previous one).
    fn retag_page(&mut self, first: bool) {
        let p = self.page as usize;
        self.page_section[p] = self.sec as u32;
        self.page_first[p] = first;
        self.page_h = self.height_for(self.sec, self.page);
        self.page_text_height[p] = self.page_h;
    }

    /// Height of a row: its tallest cell, or its set height when that is
    /// larger (or exact).
    fn row_height(&self, row: &[Vec<usize>], layouts: &[ParaLayout], props: Option<&TableRow>) -> Twips {
        let cell_h = |cell: &Vec<usize>| -> Twips { cell.iter().map(|&p| layouts[p].space_before + layouts[p].height() + layouts[p].space_after).sum() };
        let content = row.iter().map(cell_h).max().unwrap_or(0);
        match props {
            Some(TableRow { height: Some(h), height_exact: true, .. }) => *h,
            Some(TableRow { height: Some(h), .. }) => content.max(*h),
            _ => content,
        }
    }

    /// One table row: every cell starts at the same y. A row that may not
    /// split (or any row, when it fits on a fresh page but not here and is
    /// marked `cantSplit`) moves to the next page whole; otherwise its
    /// lines run on past the page bottom and continue on the next page.
    /// `repeat` names the table and the height of its header rows, placed
    /// again at the top of any page this row continues onto.
    fn place_row(&mut self, row: &[Vec<usize>], layouts: &[ParaLayout], props: Option<&TableRow>, repeat: Option<(u32, Twips)>) {
        let row_h = self.row_height(row, layouts, props);
        let first = row.first().and_then(|c| c.first().copied()).unwrap_or(0);
        let cant_split = props.map_or(false, |r| r.cant_split) || props.map_or(false, |r| r.header);
        let header_h = repeat.map(|r| r.1).unwrap_or(0);
        if self.y > 0 && self.y + row_h > self.page_h && cant_split && row_h <= self.page_h {
            self.new_page(first, 0);
            if let Some((id, h)) = repeat {
                self.repeated_headers.push((self.page, id));
                self.y = h.min(self.page_h / 2);
            }
        }
        let exact = matches!(props, Some(TableRow { height: Some(_), height_exact: true, .. }));
        let (page0, y0) = (self.page, self.y);
        let mut end = (page0, y0);
        for cell in row {
            let (mut pg, mut yy) = (page0, y0);
            for &p in cell {
                let l = &layouts[p];
                let mut pl = ParaPlacement { line_page: Vec::with_capacity(l.lines.len()), line_y: Vec::with_capacity(l.lines.len()) };
                yy += l.space_before;
                for (li, line) in l.lines.iter().enumerate() {
                    if yy + line.height > self.page_h && yy > 0 && !exact {
                        pg += 1;
                        if pg as usize >= self.pages.len() {
                            self.new_page(p, li);
                            if let Some((id, _)) = repeat {
                                self.repeated_headers.push((self.page, id));
                            }
                        }
                        yy = header_h.min(self.page_h / 2);
                    }
                    pl.line_page.push(pg);
                    pl.line_y.push(yy);
                    yy += line.height;
                }
                yy += l.space_after;
                self.placements.push(pl);
            }
            if (pg, yy) > end {
                end = (pg, yy);
            }
        }
        // A set row height pads (or, exact, bounds) the row.
        if end.0 == page0 {
            let min_end = y0 + row_h;
            if exact || min_end > end.1 {
                end.1 = min_end;
            }
        }
        self.page = end.0;
        self.y = end.1;
        self.after_hard_break = false;
    }

    fn place_para(&mut self, i: usize, layouts: &[ParaLayout]) {
        let n = layouts.len();
        let l = &layouts[i];
        let mut pl = ParaPlacement { line_page: Vec::with_capacity(l.lines.len()), line_y: Vec::with_capacity(l.lines.len()) };
        let mut sb = l.space_before;
        if self.y == 0 && !self.after_hard_break {
            sb = 0; // Word suppresses space-before at the top of a page after a soft break
        }
        if l.page_break_before && self.y > 0 {
            self.new_page(i, 0);
            sb = l.space_before;
        }
        let total = l.height();
        // Keep-with-next: the group this paragraph must stay with.
        let mut needed = sb + total;
        if l.keep_next {
            let mut j = i + 1;
            let mut extra = 0;
            while j < n {
                let lj = &layouts[j];
                let first_lines = if lj.widow_control { 2 } else { 1 };
                let head: Twips = lj.lines.iter().take(first_lines).map(|x| x.height).sum();
                if lj.keep_next && j + 1 < n {
                    extra += lj.space_before + lj.height() + lj.space_after;
                    j += 1;
                } else {
                    extra += lj.space_before + head;
                    break;
                }
            }
            needed += l.space_after + extra;
        }

        let fits_whole = self.y + needed <= self.page_h;
        let fits_fresh = needed <= self.page_h;
        if !fits_whole && fits_fresh && self.y > 0 {
            self.new_page(i, 0);
            sb = l.space_before;
            if !self.after_hard_break {
                sb = 0;
            }
        }

        // Place lines.
        let mut ly = self.y + sb;
        let nl = l.lines.len();
        let mut li = 0;
        while li < nl {
            let line = &l.lines[li];
            let page_h = self.page_h;
            if ly + line.height > page_h && ly > 0 {
                // Line doesn't fit. Widow/orphan: avoid leaving a lone line.
                let page = self.page;
                let lines_on_this_page = li - pl.line_page.iter().rposition(|&p| p != page).map(|p| p + 1).unwrap_or(0);
                if l.widow_control && !l.keep_lines {
                    if lines_on_this_page == 1 && nl >= 2 && li == 1 {
                        // Orphan: pull the first line down.
                        pl.line_page.pop();
                        pl.line_y.pop();
                        self.new_page(i, 0);
                        ly = 0;
                        li = 0;
                        self.after_hard_break = false;
                        continue;
                    }
                    if nl - li == 1 && lines_on_this_page >= 2 {
                        // Widow: push the previous line over too.
                        pl.line_page.pop();
                        pl.line_y.pop();
                        self.new_page(i, li - 1);
                        ly = 0;
                        li -= 1;
                        self.after_hard_break = false;
                        continue;
                    }
                } else if l.keep_lines && li > 0 && total <= page_h {
                    pl.line_page.clear();
                    pl.line_y.clear();
                    self.new_page(i, 0);
                    ly = 0;
                    li = 0;
                    self.after_hard_break = false;
                    continue;
                }
                self.new_page(i, li);
                ly = 0;
            }
            pl.line_page.push(self.page);
            pl.line_y.push(ly);
            ly += line.height;
            if line.page_break_after {
                if li + 1 < nl {
                    self.new_page(i, li + 1);
                    ly = 0;
                } else {
                    // Break at the end of the paragraph: next paragraph starts a page.
                    ly = self.page_h + 1;
                }
                self.after_hard_break = true;
            } else {
                self.after_hard_break = false;
            }
            li += 1;
        }
        self.y = ly + l.space_after;
        if self.y >= self.page_h && i + 1 < n {
            let hard = self.after_hard_break;
            self.new_page(i + 1, 0);
            self.after_hard_break = hard;
        }
        self.placements.push(pl);
    }
}

// ---------------------------------------------------------------------------
// Draft (screen) layout
// ---------------------------------------------------------------------------

/// Cells per inch used to render indents and tabs in draft view.
pub const CELLS_PER_INCH: Twips = 10;

pub fn twips_to_cells(t: Twips) -> i32 {
    (t * CELLS_PER_INCH + TWIPS_PER_INCH / 2) / TWIPS_PER_INCH
}

/// How draft view breaks lines on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// One screen row per printed line (WordPerfect 5.1's editing screen):
    /// lines break where they break on paper, so a page looks like a page.
    /// A printed line wider than the terminal continues on the next row.
    #[default]
    Page,
    /// Continuous text re-wrapped to the terminal's width; only the page
    /// rules come from the print layout.
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenLine {
    pub start: usize,
    pub end: usize,
    /// Left offset in cells (indent).
    pub indent: u16,
    /// Content width in cells (excluding trailing spaces).
    pub width: u16,
    /// The row width centring and right alignment measure against: the
    /// terminal's columns, or in page wrap the printed line's width in
    /// cells, so flush-right text ends where the left-aligned lines do.
    pub align_width: u16,
}

/// Cells to shift a screen line right for its paragraph's alignment.
pub fn align_offset(pp: &ParaProps, line: &ScreenLine, width: u16) -> u16 {
    let slack = line.align_width.min(width).saturating_sub(line.indent).saturating_sub(line.width);
    match pp.align() {
        Align::Center => slack / 2,
        Align::Right => slack,
        _ => 0,
    }
}

pub fn cell_width(c: char) -> u16 {
    if c == '\t' {
        return 0;
    }
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16
}

/// Cell advance of an item in draft view.
pub fn screen_advance(item: &Item, x: u16, pp: &ParaProps) -> u16 {
    match item {
        Item::Char(c) => cell_width(*c),
        Item::Code(Code::Tab) => {
            let abs = x as i32 + twips_to_cells(pp.indent_left());
            let mut next = None;
            for t in &pp.tabs {
                let tc = twips_to_cells(t.pos);
                if !t.clear && tc > abs {
                    next = Some(tc);
                    break;
                }
            }
            let default = twips_to_cells(DEFAULT_TAB).max(1);
            let next = next.unwrap_or((abs / default + 1) * default);
            (next - abs).max(1) as u16
        }
        Item::Code(_) => 0,
    }
}

/// Cell offset of a paragraph's first line before any list label.
pub fn screen_first_indent(pp: &ParaProps, cols: u16) -> u16 {
    let cols = cols.max(10);
    let left = twips_to_cells(pp.indent_left()).clamp(0, cols as i32 / 2) as u16;
    let first_off = twips_to_cells(pp.first_line_offset());
    (left as i32 + first_off).clamp(0, cols as i32 / 2) as u16
}

/// Wrap a paragraph to `cols` columns for draft view. `label_cells` is the
/// width of a list label drawn at the first-line position; the text starts
/// after it (at the left indent when the label fits in the hanging space).
pub fn wrap_screen(p: &Paragraph, pp: &ParaProps, cols: u16, label_cells: u16) -> Vec<ScreenLine> {
    wrap_screen_range(p, pp, cols, label_cells, 0, p.items.len(), true, true)
}

/// Twips one screen cell stands for in page wrap: the average advance of
/// ordinary English text in the document's body font. One figure for the
/// whole document, so the right margin is the same column on every line.
pub fn twips_per_cell(body: &RunProps) -> Twips {
    let sample = "the quick brown fox jumps over the lazy dog, and then some more words. ";
    (sample.chars().map(|c| metrics::advance(body, c)).sum::<Twips>() / sample.chars().count() as Twips).max(1)
}

/// Screen lines that follow the printed line breaks (`WrapMode::Page`):
/// each line of `print` becomes one screen line, or several when it is
/// wider than the terminal. `tpc` is [`twips_per_cell`] for the document.
pub fn screen_lines_from_print(p: &Paragraph, pp: &ParaProps, tpc: Twips, cols: u16, label_cells: u16, print: &ParaLayout) -> Vec<ScreenLine> {
    let tpc = tpc.max(1);
    let mut out = Vec::with_capacity(print.lines.len());
    for (li, l) in print.lines.iter().enumerate() {
        let rows = wrap_screen_range(p, pp, cols, label_cells, l.start, l.end, li == 0, false);
        let indent = rows.first().map(|r| r.indent).unwrap_or(0) as i32;
        let align_width = (l.avail / tpc + indent).clamp(0, cols as i32) as u16;
        out.extend(rows.into_iter().map(|mut r| {
            r.align_width = align_width;
            r
        }));
    }
    out
}

/// Wrap items `[start, end)` of a paragraph. `first` marks the paragraph's
/// first line (first-line indent and list label apply); `whole` says the
/// range is the entire paragraph.
fn wrap_screen_range(p: &Paragraph, pp: &ParaProps, cols: u16, label_cells: u16, range_start: usize, range_end: usize, first: bool, whole: bool) -> Vec<ScreenLine> {
    let cols = cols.max(3);
    let left = twips_to_cells(pp.indent_left()).clamp(0, cols as i32 / 2) as u16;
    let right = twips_to_cells(pp.indent_right()).clamp(0, cols as i32 / 4) as u16;
    let mut first_indent = screen_first_indent(pp, cols);
    if label_cells > 0 {
        first_indent = (first_indent + label_cells + 1).max(left).min(cols / 2 + label_cells + 1);
    }

    let mut lines: Vec<ScreenLine> = Vec::new();
    let mut start = range_start;
    let mut x: u16 = 0;
    let mut width_nonspace: u16 = 0;
    let mut last_break: Option<(usize, u16)> = None;
    let n = range_end.min(p.items.len());
    let mut i = range_start;
    let mut had_glyph = false;
    while i < n {
        let indent = if first && lines.is_empty() { first_indent } else { left };
        let avail = cols.saturating_sub(indent + right).max(3);
        let it = &p.items[i];
        if matches!(it, Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak)) {
            lines.push(ScreenLine { start, end: i + 1, indent, width: width_nonspace, align_width: cols });
            start = i + 1;
            x = 0;
            width_nonspace = 0;
            last_break = None;
            had_glyph = false;
            i += 1;
            continue;
        }
        let adv = screen_advance(it, x, pp);
        let is_space = it.is_whitespace();
        if !is_space && adv > 0 && x + adv > avail && had_glyph {
            if let Some((at, w)) = last_break {
                if at > start {
                    lines.push(ScreenLine { start, end: at, indent, width: w, align_width: cols });
                    start = at;
                    i = at;
                    x = 0;
                    width_nonspace = 0;
                    last_break = None;
                    had_glyph = false;
                    continue;
                }
            }
            lines.push(ScreenLine { start, end: i, indent, width: width_nonspace, align_width: cols });
            start = i;
            x = 0;
            width_nonspace = 0;
            last_break = None;
            had_glyph = false;
            continue;
        }
        x += adv;
        if is_space {
            last_break = Some((i + 1, width_nonspace));
            had_glyph = true;
        } else if adv > 0 {
            had_glyph = true;
            width_nonspace = x;
            if matches!(it, Item::Char('-')) {
                last_break = Some((i + 1, x));
            }
        }
        i += 1;
    }
    // The final line: always one for a whole paragraph (so an empty
    // paragraph, or one ending in a break, still has a row), but a printed
    // line that ends in a forced break has already been pushed.
    if start < n || lines.is_empty() || whole {
        let indent = if first && lines.is_empty() { first_indent } else { left };
        lines.push(ScreenLine { start, end: n, indent, width: width_nonspace, align_width: cols });
    }
    lines
}

/// X cell offset (relative to the line's indent) of item index `idx` within
/// a screen line.
pub fn screen_x_of(p: &Paragraph, pp: &ParaProps, line: &ScreenLine, idx: usize) -> u16 {
    let mut x = 0u16;
    for i in line.start..idx.min(line.end) {
        x += screen_advance(&p.items[i], x, pp);
    }
    x
}

/// The item index in `line` closest to cell offset `x`.
pub fn screen_idx_at_x(p: &Paragraph, pp: &ParaProps, line: &ScreenLine, target: u16) -> usize {
    let mut x = 0u16;
    let mut end = line.end;
    // Don't land after a trailing forced break or on the trailing space of a wrapped line.
    if end > line.start && matches!(p.items[end - 1], Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak)) {
        end -= 1;
    } else if end > line.start && end < p.items.len() && p.items[end - 1].is_whitespace() {
        end -= 1;
    }
    for i in line.start..end {
        let adv = screen_advance(&p.items[i], x, pp);
        if adv > 0 && x + adv / 2 >= target && x + adv > target {
            return i;
        }
        if x >= target && adv > 0 {
            return i;
        }
        x += adv;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_paginates() {
        let text = "word ".repeat(2000);
        let mut doc = Document::new();
        doc.paragraphs[0] = Paragraph::from_text(text.trim());
        let l = layout_paragraph(&doc, 0, None);
        assert!(l.lines.len() > 50, "lines: {}", l.lines.len());
        for w in l.lines.windows(2) {
            assert!(w[0].end == w[1].start);
        }
        let pg = paginate(&doc, &[l]);
        assert!(pg.page_count() >= 2);
    }

    #[test]
    fn screen_wrap_basic() {
        let p = Paragraph::from_text("The quick brown fox jumps over the lazy dog");
        let pp = ParaProps::default();
        let lines = wrap_screen(&p, &pp, 20, 0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].width, 19);
        assert_eq!(p.text()[lines[1].start..lines[1].end], *"jumps over the lazy ");
    }

    #[test]
    fn page_wrap_follows_printed_lines_and_continues_when_narrow() {
        let text = "word ".repeat(400);
        let mut doc = Document::new();
        doc.paragraphs[0] = Paragraph::from_text(text.trim());
        let pp = doc.para_props(0);
        let print = layout_paragraph(&doc, 0, None);
        assert!(print.lines.len() > 5);
        // Wide terminal: one screen line per printed line, same boundaries.
        let wide = screen_lines_from_print(&doc.paragraphs[0], &pp, twips_per_cell(&doc.base_run_props(0)), 300, 0, &print);
        assert_eq!(wide.len(), print.lines.len());
        for (s, l) in wide.iter().zip(&print.lines) {
            assert_eq!((s.start, s.end), (l.start, l.end));
        }
        // Narrow terminal: printed lines continue onto extra rows, and the
        // rows still tile the paragraph exactly.
        let narrow = screen_lines_from_print(&doc.paragraphs[0], &pp, twips_per_cell(&doc.base_run_props(0)), 40, 0, &print);
        assert!(narrow.len() > wide.len());
        assert_eq!(narrow[0].start, 0);
        assert_eq!(narrow.last().unwrap().end, doc.paragraphs[0].items.len());
        for w in narrow.windows(2) {
            assert_eq!(w[0].end, w[1].start);
            assert!(w[0].width <= 40);
        }
        // A line break inside the paragraph doesn't produce a phantom row.
        let mut p2 = Paragraph::from_text("ab");
        p2.items.insert(1, Item::Code(Code::LineBreak));
        doc.paragraphs[0] = p2;
        let print = layout_paragraph(&doc, 0, None);
        let rows = screen_lines_from_print(&doc.paragraphs[0], &pp, twips_per_cell(&doc.base_run_props(0)), 80, 0, &print);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.len(), print.lines.len());
        // …while a trailing break still gets its empty row, as on paper.
        let mut p3 = Paragraph::from_text("ab");
        p3.items.push(Item::Code(Code::LineBreak));
        doc.paragraphs[0] = p3;
        let print = layout_paragraph(&doc, 0, None);
        let rows = screen_lines_from_print(&doc.paragraphs[0], &pp, twips_per_cell(&doc.base_run_props(0)), 80, 0, &print);
        assert_eq!(rows.len(), 2);
        assert_eq!(wrap_screen(&doc.paragraphs[0], &pp, 80, 0).len(), 2);
    }

    #[test]
    fn sections_start_pages_and_change_widths() {
        let mut doc = Document::new();
        doc.paragraphs = vec![Paragraph::from_text("one"), Paragraph::from_text("two"), Paragraph::from_text("three")];
        let mut narrow = SectionProps::default();
        narrow.margin_left = 3600;
        narrow.margin_right = 3600;
        doc.paragraphs[0].props.sect_break = Some(narrow.clone());
        let layouts: Vec<ParaLayout> = (0..3).map(|i| layout_paragraph(&doc, i, None)).collect();
        // The first paragraph is laid out against the narrow section.
        assert_eq!(layouts[0].lines[0].avail, narrow.text_width());
        assert_eq!(layouts[1].lines[0].avail, SectionProps::default().text_width());
        let pg = paginate(&doc, &layouts);
        assert_eq!(pg.page_count(), 2, "a next-page section break starts a page");
        assert_eq!(pg.page_of(1, 0), 1);
        assert_eq!(pg.section_of_page(0), 0);
        assert_eq!(pg.section_of_page(1), 1);
        assert!(pg.page_first_in_section[1]);
        // Continuous: same page.
        doc.section.start = SectionStart::Continuous;
        let pg = paginate(&doc, &layouts);
        assert_eq!(pg.page_count(), 1);
        // Even page: page 2 is fine; odd page needs a blank page 2 first.
        doc.section.start = SectionStart::EvenPage;
        assert_eq!(paginate(&doc, &layouts).page_count(), 2);
        doc.section.start = SectionStart::OddPage;
        let pg = paginate(&doc, &layouts);
        assert_eq!(pg.page_count(), 3);
        assert!(pg.is_blank(1));
        assert_eq!(pg.page_of(1, 0), 2);
        // Page numbering restarts where a section says so.
        doc.section.page_start = Some(10);
        let pg = paginate(&doc, &layouts);
        let secs = Sections::build(&doc);
        assert_eq!(pg.page_number(0, &secs), 1);
        assert_eq!(pg.page_number(2, &secs), 10);
    }

    #[test]
    fn a_tall_header_shortens_the_page() {
        use crate::editor::Editor;
        let text = "line\n".repeat(200);
        let mut e = Editor::new(crate::text::from_text(text.trim(), false));
        let before = e.page_count();
        let mut hf = HeaderFooter::default();
        hf.kind = Some(HfKind::Header);
        hf.paragraphs = "h\n".repeat(12).trim().split('\n').map(Paragraph::from_text).collect();
        e.doc.headers.insert("rId9".into(), hf);
        e.doc.section.hf.push(HfRef { kind: HfKind::Header, pages: HfPages::Default, id: "rId9".into() });
        e.invalidate_headers();
        let after = e.page_count();
        assert!(after > before, "{} vs {}", before, after);
        assert!(e.layout.pagination.page_text_height[0] < e.doc.section.text_height());
        // A short header fits inside the margin and changes nothing.
        e.doc.headers.get_mut("rId9").unwrap().paragraphs.truncate(1);
        e.invalidate_headers();
        assert_eq!(e.page_count(), before);
    }

    #[test]
    fn table_rows_split_unless_told_not_to_and_headers_repeat() {
        use crate::editor::Editor;
        let mut e = Editor::new(crate::text::from_text("x\ny", false));
        e.move_to(Pos::new(1, 0), false);
        assert!(e.insert_table(3, 1));
        // A very tall row.
        e.insert_str(&"line\n".repeat(70));
        let count = |e: &mut Editor| e.page_count();
        assert_eq!(count(&mut e), 2, "the row runs on to the next page");
        let first_row_pages: Vec<u32> = e.layout.pagination.placements[1..3].iter().map(|p| p.line_page[0]).collect();
        assert_eq!(first_row_pages, [0, 0]);
        // Can't split: the row moves whole when it would fit on a page…
        let id = e.current_cell().unwrap().table;
        let mut t = e.doc.tables[&id].clone();
        t.rows[0].cant_split = true;
        e.doc.tables.insert(id, t.clone());
        e.invalidate_headers();
        assert_eq!(count(&mut e), 2);
        // …the second row, being short, stays with the flow; a header row
        // is repeated on the continuation page.
        t.rows[0].cant_split = false;
        t.rows[0].header = true;
        e.doc.tables.insert(id, t);
        e.invalidate_headers();
        e.move_to(Pos::new(e.doc.paragraphs.len() - 3, 0), false);
        e.insert_str(&"more\n".repeat(70));
        assert!(count(&mut e) >= 3);
        assert!(e.layout.pagination.repeated_headers.iter().any(|&(_, tid)| tid == id), "{:?}", e.layout.pagination.repeated_headers);
    }

    #[test]
    fn page_break_forces_page() {
        let mut doc = Document::new();
        doc.paragraphs = vec![Paragraph::from_text("a"), Paragraph::from_text("b")];
        doc.paragraphs[0].items.push(Item::Code(Code::PageBreak));
        let layouts: Vec<ParaLayout> = (0..2).map(|i| layout_paragraph(&doc, i, None)).collect();

        let pg = paginate(&doc, &layouts);
        assert_eq!(pg.page_count(), 2);
        assert_eq!(pg.page_of(1, 0), 1);
    }
}
