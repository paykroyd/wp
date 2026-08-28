//! Layout: print-accurate line breaking and pagination in twips, and the
//! terminal-width wrap used by draft view. See DESIGN.md §5.

use crate::document::{Document, Run};
use crate::metrics;
use crate::model::*;
use crate::numbering::{ListLabel, Suffix};

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

/// Lay out one paragraph against the section's text width.
pub fn layout_paragraph(doc: &Document, para: usize, label: Option<&ListLabel>) -> ParaLayout {
    let p = &doc.paragraphs[para];
    let pp = doc.para_props(para);
    let runs: Vec<Run> = doc.runs(para);
    let text_width = doc.section.text_width();
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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pagination {
    pub pages: Vec<PageStart>,
    pub placements: Vec<ParaPlacement>,
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
}

/// Paginate the document given per-paragraph layouts.
pub fn paginate(section: &SectionProps, layouts: &[ParaLayout]) -> Pagination {
    let page_h = section.text_height();
    let mut pages = vec![PageStart { para: 0, line: 0 }];
    let mut placements: Vec<ParaPlacement> = Vec::with_capacity(layouts.len());
    let mut y: Twips = 0;
    let mut page: u32 = 0;
    let mut after_hard_break = true; // start of document behaves like one
    let n = layouts.len();

    let new_page = |pages: &mut Vec<PageStart>, page: &mut u32, y: &mut Twips, para: usize, line: usize| {
        *page += 1;
        *y = 0;
        pages.push(PageStart { para, line });
    };

    let mut i = 0;
    while i < n {
        let l = &layouts[i];
        let mut pl = ParaPlacement { line_page: Vec::with_capacity(l.lines.len()), line_y: Vec::with_capacity(l.lines.len()) };
        let mut sb = l.space_before;
        if y == 0 && !after_hard_break {
            sb = 0; // Word suppresses space-before at the top of a page after a soft break
        }
        if l.page_break_before && y > 0 {
            new_page(&mut pages, &mut page, &mut y, i, 0);
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

        let fits_whole = y + needed <= page_h;
        let fits_fresh = needed <= page_h;
        if !fits_whole && fits_fresh && y > 0 {
            new_page(&mut pages, &mut page, &mut y, i, 0);
            sb = l.space_before;
            if !after_hard_break {
                sb = 0;
            }
        }

        // Place lines.
        let mut ly = y + sb;
        let nl = l.lines.len();
        let mut li = 0;
        while li < nl {
            let line = &l.lines[li];
            if ly + line.height > page_h && ly > 0 {
                // Line doesn't fit. Widow/orphan: avoid leaving a lone line.
                let lines_on_this_page = li - pl.line_page.iter().rposition(|&p| p != page).map(|p| p + 1).unwrap_or(0);
                if l.widow_control && !l.keep_lines {
                    if lines_on_this_page == 1 && nl >= 2 && li == 1 {
                        // Orphan: pull the first line down.
                        pl.line_page.pop();
                        pl.line_y.pop();
                        new_page(&mut pages, &mut page, &mut y, i, 0);
                        ly = 0;
                        li = 0;
                        after_hard_break = false;
                        continue;
                    }
                    if nl - li == 1 && lines_on_this_page >= 2 {
                        // Widow: push the previous line over too.
                        pl.line_page.pop();
                        pl.line_y.pop();
                        new_page(&mut pages, &mut page, &mut y, i, li - 1);
                        ly = 0;
                        li -= 1;
                        after_hard_break = false;
                        continue;
                    }
                } else if l.keep_lines && li > 0 && total <= page_h {
                    pl.line_page.clear();
                    pl.line_y.clear();
                    new_page(&mut pages, &mut page, &mut y, i, 0);
                    ly = 0;
                    li = 0;
                    after_hard_break = false;
                    continue;
                }
                new_page(&mut pages, &mut page, &mut y, i, li);
                ly = 0;
            }
            pl.line_page.push(page);
            pl.line_y.push(ly);
            ly += line.height;
            if line.page_break_after {
                if li + 1 < nl {
                    new_page(&mut pages, &mut page, &mut y, i, li + 1);
                    ly = 0;
                } else {
                    // Break at the end of the paragraph: next paragraph starts a page.
                    ly = page_h + 1;
                }
                after_hard_break = true;
            } else {
                after_hard_break = false;
            }
            li += 1;
        }
        y = ly + l.space_after;
        if y >= page_h && i + 1 < n {
            let hard = after_hard_break;
            new_page(&mut pages, &mut page, &mut y, i + 1, 0);
            after_hard_break = hard;
        }
        placements.push(pl);
        i += 1;
    }
    Pagination { pages, placements }
}

// ---------------------------------------------------------------------------
// Draft (screen) layout
// ---------------------------------------------------------------------------

/// Cells per inch used to render indents and tabs in draft view.
pub const CELLS_PER_INCH: Twips = 10;

pub fn twips_to_cells(t: Twips) -> i32 {
    (t * CELLS_PER_INCH + TWIPS_PER_INCH / 2) / TWIPS_PER_INCH
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenLine {
    pub start: usize,
    pub end: usize,
    /// Left offset in cells (indent).
    pub indent: u16,
    /// Content width in cells (excluding trailing spaces).
    pub width: u16,
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
    let cols = cols.max(10);
    let left = twips_to_cells(pp.indent_left()).clamp(0, cols as i32 / 2) as u16;
    let right = twips_to_cells(pp.indent_right()).clamp(0, cols as i32 / 4) as u16;
    let mut first_indent = screen_first_indent(pp, cols);
    if label_cells > 0 {
        first_indent = (first_indent + label_cells + 1).max(left).min(cols / 2 + label_cells + 1);
    }

    let mut lines: Vec<ScreenLine> = Vec::new();
    let mut start = 0usize;
    let mut x: u16 = 0;
    let mut width_nonspace: u16 = 0;
    let mut last_break: Option<(usize, u16)> = None;
    let n = p.items.len();
    let mut i = 0;
    let mut had_glyph = false;
    while i < n {
        let indent = if lines.is_empty() { first_indent } else { left };
        let avail = cols.saturating_sub(indent + right).max(8);
        let it = &p.items[i];
        if matches!(it, Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak)) {
            lines.push(ScreenLine { start, end: i + 1, indent, width: width_nonspace });
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
                    lines.push(ScreenLine { start, end: at, indent, width: w });
                    start = at;
                    i = at;
                    x = 0;
                    width_nonspace = 0;
                    last_break = None;
                    had_glyph = false;
                    continue;
                }
            }
            lines.push(ScreenLine { start, end: i, indent, width: width_nonspace });
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
    let indent = if lines.is_empty() { first_indent } else { left };
    lines.push(ScreenLine { start, end: n, indent, width: width_nonspace });
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
        let pg = paginate(&doc.section, &[l]);
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
    fn page_break_forces_page() {
        let mut doc = Document::new();
        doc.paragraphs = vec![Paragraph::from_text("a"), Paragraph::from_text("b")];
        doc.paragraphs[0].items.push(Item::Code(Code::PageBreak));
        let layouts: Vec<ParaLayout> = (0..2).map(|i| layout_paragraph(&doc, i, None)).collect();

        let pg = paginate(&doc.section, &layouts);
        assert_eq!(pg.page_count(), 2);
        assert_eq!(pg.page_of(1, 0), 1);
    }
}
