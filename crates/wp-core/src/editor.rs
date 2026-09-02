//! The editing session: document + cursor + selection + undo + layout caches.

use crate::document::{rewrite_attrs, AttrMap, Document};
use crate::edit::Op;
use crate::layout::{self, Pagination, ParaLayout, ScreenLine, WrapMode};
use crate::model::*;
use crate::section::Sections;
use crate::numbering::ListLabel;
use crate::reveal::{self, ParaCode};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKind {
    Typing,
    Backspacing,
    Other,
}

#[derive(Clone, Debug)]
struct Group {
    inverses: Vec<Op>,
    cursor_before: Pos,
    anchor_before: Option<Pos>,
    cursor_after: Pos,
    kind: GroupKind,
    time: Instant,
    last_char_ws: bool,
}

#[derive(Default)]
struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    open: Option<Group>,
}

/// A copied/cut fragment: one or more partial paragraphs.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Fragment {
    pub paragraphs: Vec<Paragraph>,
}

impl Fragment {
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&p.text());
        }
        s
    }
    pub fn plain(&self) -> Fragment {
        Fragment {
            paragraphs: self
                .paragraphs
                .iter()
                .map(|p| Paragraph {
                    props: ParaProps::default(),
                    items: p
                        .items
                        .iter()
                        .filter(|i| !matches!(i, Item::Code(c) if c.is_zero_width()))
                        .cloned()
                        .collect(),
                })
                .collect(),
        }
    }
    pub fn from_text(s: &str) -> Fragment {
        Fragment { paragraphs: s.split('\n').map(Paragraph::from_text).collect() }
    }
}

/// Layout state maintained in lockstep with the paragraphs.
#[derive(Default)]
pub struct LayoutCache {
    print: Vec<Option<ParaLayout>>,
    screen: Vec<Option<Vec<ScreenLine>>>,
    cols: u16,
    wrap: WrapMode,
    pub pagination: Pagination,
    pagination_dirty: bool,
    /// List labels, recomputed for the whole document whenever paragraph
    /// structure or properties change (one cheap pass).
    labels: Vec<Option<ListLabel>>,
    labels_dirty: bool,
    /// The section of each paragraph, rebuilt with the labels.
    pub sections: Sections,
    /// Header/footer bodies laid out by (id, text width).
    hf: std::collections::HashMap<(String, Twips), std::rc::Rc<Vec<ParaLayout>>>,
}

pub struct Editor {
    pub doc: Document,
    pub cursor: Pos,
    pub anchor: Option<Pos>,
    /// Column to keep when moving vertically.
    pub goal_x: Option<u16>,
    pub dirty: bool,
    history: History,
    pub layout: LayoutCache,
    cut_ring: VecDeque<Fragment>,
    /// Typeover (overwrite) mode.
    pub typeover: bool,
}

pub const CUT_RING_SIZE: usize = 16;
const GROUP_TIMEOUT: Duration = Duration::from_millis(1000);

impl Editor {
    pub fn new(doc: Document) -> Editor {
        let n = doc.paragraphs.len();
        let mut e = Editor {
            doc,
            cursor: Pos::default(),
            anchor: None,
            goal_x: None,
            dirty: false,
            history: History::default(),
            layout: LayoutCache {
                print: vec![None; n],
                screen: vec![None; n],
                cols: 80,
                wrap: WrapMode::default(),
                pagination: Pagination::default(),
                pagination_dirty: true,
                labels: vec![None; n],
                labels_dirty: true,
                sections: Sections::default(),
                hf: std::collections::HashMap::new(),
            },
            cut_ring: VecDeque::new(),
            typeover: false,
        };
        e.ensure_layout();
        e
    }

    // ------------------------------------------------------------------
    // Layout access
    // ------------------------------------------------------------------

    pub fn set_cols(&mut self, cols: u16) {
        if cols != self.layout.cols {
            self.layout.cols = cols;
            for s in self.layout.screen.iter_mut() {
                *s = None;
            }
        }
    }

    pub fn cols(&self) -> u16 {
        self.layout.cols
    }

    pub fn set_wrap(&mut self, wrap: WrapMode) {
        if wrap != self.layout.wrap {
            self.layout.wrap = wrap;
            for s in self.layout.screen.iter_mut() {
                *s = None;
            }
        }
    }

    pub fn wrap(&self) -> WrapMode {
        self.layout.wrap
    }

    fn invalidate(&mut self, para: usize) {
        if para < self.layout.print.len() {
            self.layout.print[para] = None;
            self.layout.screen[para] = None;
        }
        self.layout.pagination_dirty = true;
    }

    fn invalidate_all(&mut self) {
        for i in 0..self.layout.print.len() {
            self.layout.print[i] = None;
            self.layout.screen[i] = None;
        }
        self.layout.pagination_dirty = true;
        self.layout.labels_dirty = true;
    }

    /// Recompute list labels and the section map if paragraph structure
    /// changed; any paragraph whose label or text width changed is laid
    /// out again.
    fn refresh_labels(&mut self) {
        if !self.layout.labels_dirty {
            return;
        }
        let new = self.doc.list_labels();
        for (i, l) in new.iter().enumerate() {
            if self.layout.labels.get(i) != Some(l) {
                if i < self.layout.print.len() {
                    self.layout.print[i] = None;
                    self.layout.screen[i] = None;
                }
                self.layout.pagination_dirty = true;
            }
        }
        self.layout.labels = new;
        let secs = Sections::build(&self.doc);
        let old = &self.layout.sections;
        for i in 0..self.doc.paragraphs.len() {
            let same = old.of.get(i).map_or(false, |_| old.section_of(i).column_width() == secs.section_of(i).column_width());
            if !same && i < self.layout.print.len() {
                self.layout.print[i] = None;
                self.layout.screen[i] = None;
            }
        }
        if *old != secs {
            self.layout.pagination_dirty = true;
        }
        self.layout.sections = secs;
        self.layout.labels_dirty = false;
    }

    /// The section that governs a paragraph.
    pub fn section_at(&mut self, para: usize) -> SectionProps {
        self.refresh_labels();
        self.layout.sections.section_of(para).clone()
    }

    /// 1-based number of the section the cursor is in, and how many there are.
    pub fn cursor_section(&mut self) -> (usize, usize) {
        self.refresh_labels();
        (self.layout.sections.index_of(self.cursor.para) + 1, self.layout.sections.len())
    }

    /// Replace the properties of the section governing `para`: the section
    /// break that ends it, or the document's final section.
    pub fn set_section_at(&mut self, para: usize, s: SectionProps) {
        self.commit();
        match self.doc.section_owner(para) {
            Some(k) => {
                let mut props = self.doc.paragraphs[k].props.clone();
                props.sect_break = Some(s);
                props.touch();
                self.apply(Op::SetParaProps { para: k, props });
            }
            None => self.apply(Op::SetSection(s)),
        }
        self.commit();
    }

    /// End the current section at the cursor: the paragraph is split there
    /// and the half before the cursor carries a copy of the section's
    /// properties (Word's convention), so the new section that follows
    /// keeps the existing setup and begins as `start` says.
    pub fn insert_section_break(&mut self, start: SectionStart) -> bool {
        if self.current_cell().is_some() || self.doc.paragraphs[self.cursor.para].props.raw_block {
            return false;
        }
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        let para = self.cursor.para;
        let governing = self.doc.section_at(para).clone();
        let owner = self.doc.section_owner(para);
        // At the start of a paragraph the break goes on the paragraph before
        // (no empty paragraph is left behind); otherwise the paragraph is
        // split and the tail keeps any break that ended the section, now
        // describing the section that starts here.
        let prev_ok = self.cursor.idx == 0
            && para > 0
            && self.doc.paragraphs[para - 1].props.cell.is_none()
            && !self.doc.paragraphs[para - 1].props.raw_block
            && self.doc.paragraphs[para - 1].props.sect_break.is_none();
        if prev_ok {
            self.cursor = Pos::new(para, 0);
        } else {
            self.split_paragraph_raw();
        }
        let head = self.cursor.para - 1;
        let mut head_props = self.doc.paragraphs[head].props.clone();
        head_props.sect_break = Some(SectionProps { start: governing.start, ..governing.clone() });
        head_props.touch();
        self.apply(Op::SetParaProps { para: head, props: head_props });
        let mut after = governing;
        after.start = start;
        match owner {
            Some(_) => {
                let k = self.cursor.para + (owner.unwrap() - para);
                let mut props = self.doc.paragraphs[k].props.clone();
                props.sect_break = Some(after);
                props.touch();
                self.apply(Op::SetParaProps { para: k, props });
            }
            None => self.apply(Op::SetSection(after)),
        }
        self.commit();
        self.anchor = None;
        self.goal_x = None;
        true
    }

    /// The list label of a paragraph, if it is a list item.
    pub fn list_label(&mut self, para: usize) -> Option<ListLabel> {
        self.refresh_labels();
        self.layout.labels.get(para).cloned().flatten()
    }

    /// Bring every cached layout and the pagination up to date.
    pub fn ensure_layout(&mut self) {
        self.refresh_labels();
        let n = self.doc.paragraphs.len();
        debug_assert_eq!(self.layout.print.len(), n);
        for i in 0..n {
            if self.layout.print[i].is_none() {
                self.layout.print[i] = Some(layout::layout_paragraph_in(&self.doc, i, self.layout.labels[i].as_ref(), self.layout.sections.section_of(i)));
                self.layout.pagination_dirty = true;
            }
        }
        if self.layout.pagination_dirty {
            let layouts: Vec<ParaLayout> = self.layout.print.iter().map(|l| l.clone().unwrap()).collect();
            let furniture = self.furniture();
            self.layout.pagination = layout::paginate_with(&self.doc, &layouts, &self.layout.sections, &|sec, first, even| furniture.get(&(sec, first, even)).cloned().unwrap_or_default());
            self.layout.pagination_dirty = false;
        }
    }

    /// The laid-out body of header/footer `id` at the text width of `sect`.
    pub fn hf_layout(&mut self, id: &str, sect: &SectionProps) -> Option<std::rc::Rc<Vec<ParaLayout>>> {
        let width = sect.text_width();
        let key = (id.to_string(), width);
        if let Some(l) = self.layout.hf.get(&key) {
            return Some(l.clone());
        }
        let hf = self.doc.headers.get(id)?;
        let l = std::rc::Rc::new(crate::section::layout_body(&self.doc, hf, sect));
        self.layout.hf.insert(key, l.clone());
        Some(l)
    }

    /// The header or footer a page of section `sec` shows, by id.
    pub fn hf_id_for(&self, sec: &SectionProps, kind: HfKind, first: bool, even: bool) -> Option<String> {
        sec.hf_for_page(kind, first, even, self.doc.even_odd_headers).map(|s| s.to_string())
    }

    /// Header and footer bodies changed: lay them out again and repaginate.
    pub fn invalidate_headers(&mut self) {
        self.layout.hf.clear();
        self.layout.pagination_dirty = true;
    }

    /// Header and footer space per (section, first page, even page).
    fn furniture(&mut self) -> std::collections::HashMap<(usize, bool, bool), layout::PageFurniture> {
        let mut m = std::collections::HashMap::new();
        let secs = self.layout.sections.list.clone();
        for (si, s) in secs.iter().enumerate() {
            for first in [false, true] {
                for even in [false, true] {
                    let mut f = layout::PageFurniture::default();
                    for (kind, dist, margin) in [(HfKind::Header, s.header_distance, s.margin_top), (HfKind::Footer, s.footer_distance, s.margin_bottom)] {
                        let Some(id) = self.hf_id_for(s, kind, first, even) else { continue };
                        let Some(l) = self.hf_layout(&id, s) else { continue };
                        let h = crate::section::body_height(&l);
                        let extra = (dist + h - margin).max(0);
                        match kind {
                            HfKind::Header => f.extra_top = extra,
                            HfKind::Footer => f.extra_bottom = extra,
                        }
                    }
                    m.insert((si, first, even), f);
                }
            }
        }
        m
    }

    pub fn print_layout(&mut self, para: usize) -> &ParaLayout {
        self.refresh_labels();
        if self.layout.print[para].is_none() {
            self.layout.print[para] = Some(layout::layout_paragraph_in(&self.doc, para, self.layout.labels[para].as_ref(), self.layout.sections.section_of(para)));
            self.layout.pagination_dirty = true;
        }
        self.layout.print[para].as_ref().unwrap()
    }

    /// Cells a paragraph's list label occupies in draft view (0 if none).
    pub fn label_cells(&mut self, para: usize) -> u16 {
        self.list_label(para).map(|l| unicode_width::UnicodeWidthStr::width(l.text.as_str()) as u16).unwrap_or(0)
    }

    pub fn screen_lines(&mut self, para: usize) -> &Vec<ScreenLine> {
        self.refresh_labels();
        if self.layout.screen[para].is_none() {
            let pp = self.doc.para_props(para);
            let cells = self.label_cells(para);
            let cols = self.doc.cell_screen_width(para, self.layout.cols).unwrap_or(self.layout.cols);
            let lines = match self.layout.wrap {
                WrapMode::Terminal => layout::wrap_screen(&self.doc.paragraphs[para], &pp, cols, cells),
                WrapMode::Page => {
                    let print = self.print_layout(para).clone();
                    let tpc = layout::twips_per_cell(&self.doc.styles.resolve_para_style_run(None));
                    layout::screen_lines_from_print(&self.doc.paragraphs[para], &pp, tpc, cols, cells, &print)
                }
            };
            self.layout.screen[para] = Some(lines);
        }
        self.layout.screen[para].as_ref().unwrap()
    }

    pub fn page_count(&mut self) -> usize {
        self.ensure_layout();
        self.layout.pagination.page_count()
    }

    /// (page 1-based, Ln in twips from page top, Pos in twips from page left edge)
    pub fn cursor_page_ln_pos(&mut self) -> (usize, Twips, Twips) {
        self.ensure_layout();
        let c = self.cursor;
        let pl = self.layout.print[c.para].as_ref().unwrap();
        let li = pl.line_of(c.idx);
        let line = pl.lines[li].clone();
        let page = self.layout.pagination.page_of(c.para, li);
        let y = self.layout.pagination.placements[c.para].line_y.get(li).copied().unwrap_or(0);
        let xs = layout::item_x_positions(&self.doc, c.para, &line);
        let x = xs.get(c.idx.saturating_sub(line.start)).copied().unwrap_or(0);
        let cell_x = self.doc.cell_x(c.para);
        let sec = self.layout.sections.section_of(c.para);
        (page + 1, sec.margin_top + y, sec.margin_left + cell_x + line.x + x)
    }

    /// The position at which page `page` (1-based) starts.
    pub fn page_start_pos(&mut self, page: usize) -> Option<Pos> {
        self.ensure_layout();
        let ps = self.layout.pagination.start_of_page(page.checked_sub(1)?)?;
        let pl = self.layout.print[ps.para].as_ref()?;
        Some(Pos::new(ps.para, pl.lines.get(ps.line)?.start))
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    fn open_group(&mut self, kind: GroupKind) {
        if self.history.open.is_none() {
            self.history.open = Some(Group {
                inverses: Vec::new(),
                cursor_before: self.cursor,
                anchor_before: self.anchor,
                cursor_after: self.cursor,
                kind,
                time: Instant::now(),
                last_char_ws: false,
            });
        }
    }

    /// Close the current undo group. Call between logically separate edits.
    pub fn commit(&mut self) {
        if let Some(mut g) = self.history.open.take() {
            if !g.inverses.is_empty() {
                g.cursor_after = self.cursor;
                self.history.undo.push(g);
                self.history.redo.clear();
                if self.history.undo.len() > 1000 {
                    self.history.undo.remove(0);
                }
            }
        }
    }

    fn apply(&mut self, op: Op) {
        // Maintain layout caches in lockstep.
        match &op {
            Op::Insert { at, .. } | Op::Delete { at, .. } => self.invalidate(at.para),
            Op::Split { at, .. } => {
                let p = at.para;
                self.layout.print.insert(p + 1, None);
                self.layout.screen.insert(p + 1, None);
                self.layout.labels.insert((p + 1).min(self.layout.labels.len()), None);
                self.layout.labels_dirty = true;
                self.invalidate(p);
            }
            Op::Join { para } => {
                let p = *para;
                self.layout.print.remove(p + 1);
                self.layout.screen.remove(p + 1);
                if p + 1 < self.layout.labels.len() {
                    self.layout.labels.remove(p + 1);
                }
                self.layout.labels_dirty = true;
                self.invalidate(p);
            }
            Op::SetParaProps { para, .. } => {
                self.layout.labels_dirty = true;
                self.invalidate(*para);
            }
            Op::ReplaceItems { para, .. } => self.invalidate(*para),
            Op::SetSection(_) => self.invalidate_all(),
            Op::InsertPara { para, .. } => self.cache_insert(*para),
            Op::RemovePara { para } => self.cache_remove(*para),
            Op::SetTable { id, .. } => self.invalidate_table(*id),
        }
        let inv = self.doc.apply(op);
        self.dirty = true;
        self.open_group(GroupKind::Other);
        self.history.open.as_mut().unwrap().inverses.push(inv);
    }

    /// Apply a primitive op inside the current undo group (for the table
    /// operations in `table.rs`).
    pub(crate) fn apply_op(&mut self, op: Op) {
        self.apply(op)
    }

    fn cache_insert(&mut self, para: usize) {
        let p = para.min(self.layout.print.len());
        self.layout.print.insert(p, None);
        self.layout.screen.insert(p, None);
        self.layout.labels.insert(p.min(self.layout.labels.len()), None);
        self.layout.labels_dirty = true;
        self.layout.pagination_dirty = true;
    }

    fn cache_remove(&mut self, para: usize) {
        if para < self.layout.print.len() {
            self.layout.print.remove(para);
            self.layout.screen.remove(para);
        }
        if para < self.layout.labels.len() {
            self.layout.labels.remove(para);
        }
        self.layout.labels_dirty = true;
        self.layout.pagination_dirty = true;
    }

    /// A table's grid changed: every paragraph in it wraps differently.
    fn invalidate_table(&mut self, id: u32) {
        for i in 0..self.doc.paragraphs.len() {
            if self.doc.paragraphs[i].props.cell.map_or(false, |c| c.table == id) {
                self.invalidate(i);
            }
        }
        self.layout.pagination_dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty() || self.history.open.as_ref().map_or(false, |g| !g.inverses.is_empty())
    }
    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.commit();
        let Some(g) = self.history.undo.pop() else { return false };
        let mut redo = Group {
            inverses: Vec::new(),
            cursor_before: self.cursor,
            anchor_before: self.anchor,
            cursor_after: g.cursor_before,
            kind: GroupKind::Other,
            time: Instant::now(),
            last_char_ws: false,
        };
        for op in g.inverses.into_iter().rev() {
            self.sync_cache_for(&op);
            let inv = self.doc.apply(op);
            redo.inverses.push(inv);
        }
        self.cursor = self.doc.clamp(g.cursor_before);
        self.anchor = g.anchor_before.map(|a| self.doc.clamp(a));
        self.history.redo.push(redo);
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.commit();
        let Some(g) = self.history.redo.pop() else { return false };
        let mut undo = Group {
            inverses: Vec::new(),
            cursor_before: g.cursor_after,
            anchor_before: None,
            cursor_after: self.cursor,
            kind: GroupKind::Other,
            time: Instant::now(),
            last_char_ws: false,
        };
        for op in g.inverses.into_iter().rev() {
            self.sync_cache_for(&op);
            let inv = self.doc.apply(op);
            undo.inverses.push(inv);
        }
        undo.cursor_after = self.cursor;
        self.cursor = self.doc.clamp(g.cursor_before);
        self.anchor = None;
        self.history.undo.push(undo);
        self.dirty = true;
        true
    }

    fn sync_cache_for(&mut self, op: &Op) {
        match op {
            Op::Insert { at, .. } | Op::Delete { at, .. } => self.invalidate(at.para),
            Op::Split { at, .. } => {
                self.layout.print.insert(at.para + 1, None);
                self.layout.screen.insert(at.para + 1, None);
                self.layout.labels.insert((at.para + 1).min(self.layout.labels.len()), None);
                self.layout.labels_dirty = true;
                self.invalidate(at.para);
            }
            Op::Join { para } => {
                self.layout.print.remove(para + 1);
                self.layout.screen.remove(para + 1);
                if para + 1 < self.layout.labels.len() {
                    self.layout.labels.remove(para + 1);
                }
                self.layout.labels_dirty = true;
                self.invalidate(*para);
            }
            Op::SetParaProps { para, .. } => {
                self.layout.labels_dirty = true;
                self.invalidate(*para);
            }
            Op::ReplaceItems { para, .. } => self.invalidate(*para),
            Op::SetSection(_) => self.invalidate_all(),
            Op::InsertPara { para, .. } => self.cache_insert(*para),
            Op::RemovePara { para } => self.cache_remove(*para),
            Op::SetTable { id, .. } => self.invalidate_table(*id),
        }
    }

    // ------------------------------------------------------------------
    // Selection
    // ------------------------------------------------------------------

    pub fn selection(&self) -> Option<Range> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(Range::new(a, self.cursor))
    }

    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    pub fn start_selection(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Pos::default());
        self.cursor = self.doc.end_pos();
    }

    /// The selected content as a fragment.
    pub fn fragment(&self, r: Range) -> Fragment {
        let mut paras = Vec::new();
        for pi in r.start.para..=r.end.para {
            let p = &self.doc.paragraphs[pi];
            let s = if pi == r.start.para { r.start.idx } else { 0 };
            let e = if pi == r.end.para { r.end.idx } else { p.items.len() };
            let mut items: Vec<Item> = p.items[s..e].to_vec();
            // Make the fragment self-contained: open attrs at s are prepended,
            // attrs still open at e are closed.
            let open_at_start = self.doc.attrs_at(Pos::new(pi, s));
            let open_at_end = self.doc.attrs_at(Pos::new(pi, e));
            let mut prefix: Vec<Item> = open_at_start.values().map(|a| Item::Code(Code::On(a.clone()))).collect();
            prefix.append(&mut items);
            for k in open_at_end.keys().rev() {
                prefix.push(Item::Code(Code::Off(*k)));
            }
            let mut props = p.props.clone();
            props.p_attrs = None; // paragraph ids must not be duplicated on paste
            paras.push(Paragraph { props, items: prefix });
        }
        Fragment { paragraphs: paras }
    }

    // ------------------------------------------------------------------
    // Cursor movement (codes are invisible unless `codes_visible`)
    // ------------------------------------------------------------------

    fn is_skippable(&self, p: Pos, codes_visible: bool) -> bool {
        if codes_visible {
            return false;
        }
        matches!(self.doc.paragraphs[p.para].items.get(p.idx), Some(Item::Code(c)) if c.is_zero_width())
    }

    pub fn next_pos(&self, p: Pos, codes_visible: bool) -> Option<Pos> {
        let para = &self.doc.paragraphs[p.para];
        let mut q = p;
        loop {
            if q.idx < para.items.len() {
                q.idx += 1;
            } else if q.para + 1 < self.doc.paragraphs.len() {
                q = Pos::new(q.para + 1, 0);
                return Some(self.skip_forward(q, codes_visible));
            } else {
                return None;
            }
            if q.idx >= para.items.len() || !self.is_skippable(q, codes_visible) {
                return Some(q);
            }
        }
    }

    fn skip_forward(&self, mut p: Pos, codes_visible: bool) -> Pos {
        while self.is_skippable(p, codes_visible) {
            p.idx += 1;
        }
        p
    }

    pub fn prev_pos(&self, p: Pos, codes_visible: bool) -> Option<Pos> {
        let mut q = p;
        loop {
            if q.idx > 0 {
                q.idx -= 1;
            } else if q.para > 0 {
                q = Pos::new(q.para - 1, self.doc.paragraphs[q.para - 1].items.len());
                return Some(q);
            } else {
                return None;
            }
            if !self.is_skippable(q, codes_visible) {
                return Some(q);
            }
        }
    }

    pub fn move_to(&mut self, p: Pos, select: bool) {
        if select {
            self.start_selection();
        } else {
            self.anchor = None;
        }
        self.cursor = self.doc.clamp(p);
    }

    pub fn move_left(&mut self, select: bool, codes_visible: bool) {
        if let Some(p) = self.prev_pos(self.cursor, codes_visible) {
            self.move_to(p, select);
        } else if !select {
            self.anchor = None;
        }
        self.goal_x = None;
    }

    pub fn move_right(&mut self, select: bool, codes_visible: bool) {
        if let Some(p) = self.next_pos(self.cursor, codes_visible) {
            self.move_to(p, select);
        } else if !select {
            self.anchor = None;
        }
        self.goal_x = None;
    }

    fn item_is_word_char(it: &Item) -> bool {
        match it {
            Item::Char(c) => c.is_alphanumeric() || *c == '_' || *c == '\'',
            _ => false,
        }
    }

    pub fn word_left(&mut self, select: bool) {
        let mut p = self.cursor;
        let items = &self.doc.paragraphs[p.para].items;
        if p.idx == 0 {
            if let Some(q) = self.prev_pos(p, false) {
                self.move_to(q, select);
            }
            return;
        }
        // skip non-word backward, then word backward
        while p.idx > 0 && !Self::item_is_word_char(&items[p.idx - 1]) {
            p.idx -= 1;
        }
        while p.idx > 0 && Self::item_is_word_char(&items[p.idx - 1]) {
            p.idx -= 1;
        }
        self.move_to(p, select);
        self.goal_x = None;
    }

    pub fn word_right(&mut self, select: bool) {
        let mut p = self.cursor;
        let items = &self.doc.paragraphs[p.para].items;
        if p.idx >= items.len() {
            if let Some(q) = self.next_pos(p, false) {
                self.move_to(q, select);
            }
            return;
        }
        while p.idx < items.len() && Self::item_is_word_char(&items[p.idx]) {
            p.idx += 1;
        }
        while p.idx < items.len() && !Self::item_is_word_char(&items[p.idx]) {
            p.idx += 1;
        }
        self.move_to(p, select);
        self.goal_x = None;
    }

    pub fn move_home(&mut self, select: bool) {
        let (para, line) = self.screen_line_of_cursor();
        let start = self.screen_lines(para)[line].start;
        self.move_to(Pos::new(para, start), select);
        self.goal_x = None;
    }

    pub fn move_end(&mut self, select: bool) {
        let (para, line) = self.screen_line_of_cursor();
        let lines = self.screen_lines(para).clone();
        let l = &lines[line];
        let mut end = l.end;
        let items = &self.doc.paragraphs[para].items;
        if line + 1 < lines.len() {
            // Before the trailing space / break of a wrapped line.
            if end > l.start && (items[end - 1].is_whitespace() || matches!(items[end - 1], Item::Code(Code::LineBreak))) {
                end -= 1;
            }
        }
        self.move_to(Pos::new(para, end), select);
        self.goal_x = None;
    }

    pub fn move_doc_start(&mut self, select: bool) {
        self.move_to(Pos::default(), select);
        self.goal_x = None;
    }

    pub fn move_doc_end(&mut self, select: bool) {
        let e = self.doc.end_pos();
        self.move_to(e, select);
        self.goal_x = None;
    }

    pub fn move_para_up(&mut self, select: bool) {
        let c = self.cursor;
        let p = if c.idx > 0 || c.para == 0 { Pos::new(c.para, 0) } else { Pos::new(c.para - 1, 0) };
        self.move_to(p, select);
        self.goal_x = None;
    }

    pub fn move_para_down(&mut self, select: bool) {
        let c = self.cursor;
        let p = if c.para + 1 < self.doc.paragraphs.len() {
            Pos::new(c.para + 1, 0)
        } else {
            self.doc.end_pos()
        };
        self.move_to(p, select);
        self.goal_x = None;
    }

    /// (paragraph, screen line index) of the cursor.
    pub fn screen_line_of_cursor(&mut self) -> (usize, usize) {
        let c = self.cursor;
        let lines = self.screen_lines(c.para);
        let mut li = lines.len() - 1;
        for (i, l) in lines.iter().enumerate() {
            if c.idx < l.end {
                li = i;
                break;
            }
        }
        (c.para, li)
    }

    pub fn cursor_screen_x(&mut self) -> u16 {
        let (para, li) = self.screen_line_of_cursor();
        let pp = self.doc.para_props(para);
        let line = self.screen_lines(para)[li].clone();
        let p = &self.doc.paragraphs[para];
        line.indent + layout::screen_x_of(p, &pp, &line, self.cursor.idx)
    }

    pub fn move_up(&mut self, select: bool) {
        let x = self.goal_x.unwrap_or_else(|| self.cursor_screen_x());
        self.goal_x = Some(x);
        let (para, li) = self.screen_line_of_cursor();
        let (tp, tl) = if li > 0 {
            (para, li - 1)
        } else if self.doc.is_cell_start(para) {
            // Top of a cell: the same column one row up, or the paragraph
            // before the table.
            if self.move_row(-1, x, select) {
                return;
            }
            let (start, _) = self.doc.table_bounds(para).unwrap();
            if start == 0 {
                self.move_to(Pos::new(0, 0), select);
                return;
            }
            let n = self.screen_lines(start - 1).len();
            (start - 1, n - 1)
        } else if para > 0 {
            let n = self.screen_lines(para - 1).len();
            (para - 1, n - 1)
        } else {
            self.move_to(Pos::new(0, 0), select);
            return;
        };
        self.move_to_line_x(tp, tl, x, select);
    }

    pub fn move_down(&mut self, select: bool) {
        let x = self.goal_x.unwrap_or_else(|| self.cursor_screen_x());
        self.goal_x = Some(x);
        let (para, li) = self.screen_line_of_cursor();
        let n = self.screen_lines(para).len();
        let cell_end = self.doc.cell_of(para).is_some() && (para + 1 >= self.doc.paragraphs.len() || !self.doc.same_cell(para, para + 1));
        let (tp, tl) = if li + 1 < n {
            (para, li + 1)
        } else if cell_end {
            // Bottom of a cell: the same column one row down, or the
            // paragraph after the table.
            if self.move_row(1, x, select) {
                return;
            }
            let (_, end) = self.doc.table_bounds(para).unwrap();
            if end >= self.doc.paragraphs.len() {
                let e = self.doc.end_pos();
                self.move_to(e, select);
                return;
            }
            (end, 0)
        } else if para + 1 < self.doc.paragraphs.len() {
            (para + 1, 0)
        } else {
            let e = self.doc.end_pos();
            self.move_to(e, select);
            return;
        };
        self.move_to_line_x(tp, tl, x, select);
    }

    pub fn move_to_line_x(&mut self, para: usize, line: usize, x: u16, select: bool) {
        let pp = self.doc.para_props(para);
        let l = self.screen_lines(para)[line].clone();
        let p = &self.doc.paragraphs[para];
        let idx = layout::screen_idx_at_x(p, &pp, &l, x.saturating_sub(l.indent));
        self.move_to(Pos::new(para, idx), select);
    }

    /// Move by `n` screen lines (negative = up), for paging.
    pub fn move_lines(&mut self, n: i32, select: bool) {
        for _ in 0..n.abs() {
            if n < 0 {
                self.move_up(select);
            } else {
                self.move_down(select);
            }
        }
    }

    // ------------------------------------------------------------------
    // Editing
    // ------------------------------------------------------------------

    fn typing_group_ok(&self, c: char) -> bool {
        match &self.history.open {
            Some(g) if g.kind == GroupKind::Typing => {
                g.cursor_after == self.cursor
                    && g.time.elapsed() < GROUP_TIMEOUT
                    && !(c.is_whitespace() && !g.last_char_ws)
            }
            _ => false,
        }
    }

    /// If editing at the cursor would alter something wp only preserves
    /// (a table, a tracked change, a content control), say what.
    pub fn protected_at(&self, p: Pos) -> Option<String> {
        let para = &self.doc.paragraphs[p.para];
        if para.props.raw_block {
            if let Some(Item::Code(Code::Opaque(o))) = para.items.first() {
                return Some(format!("{} — preserved but not editable in this version", o.label));
            }
            return Some("Preserved block — not editable in this version".into());
        }
        // Inside a paired opaque wrapper (tracked change, field, …)?
        let mut depth: Vec<&OpaqueXml> = Vec::new();
        for it in para.items.iter().take(p.idx) {
            if let Item::Code(Code::Opaque(o)) = it {
                match o.kind {
                    OpaqueKind::Open(_) => depth.push(o),
                    OpaqueKind::Close(_) => {
                        depth.pop();
                    }
                    OpaqueKind::Element => {}
                }
            }
        }
        depth.iter().rev().find(|o| o.protected).map(|o| format!("Inside a {} — wp can't edit these yet", o.label.to_lowercase()))
    }

    pub fn selection_protected(&self) -> Option<String> {
        if let Some(r) = self.selection() {
            for pi in r.start.para..=r.end.para {
                let p = &self.doc.paragraphs[pi];
                if p.props.raw_block {
                    return self.protected_at(Pos::new(pi, 0));
                }
                let s = if pi == r.start.para { r.start.idx } else { 0 };
                let e = if pi == r.end.para { r.end.idx } else { p.items.len() };
                for (i, it) in p.items[s..e].iter().enumerate() {
                    if let Item::Code(Code::Opaque(o)) = it {
                        if o.protected || !matches!(o.kind, OpaqueKind::Element) {
                            return Some(format!("Selection contains a {} — wp can't edit these yet", o.label.to_lowercase()));
                        }
                    }
                    let _ = i;
                }
                if let Some(m) = self.protected_at(Pos::new(pi, s)) {
                    return Some(m);
                }
            }
        }
        self.protected_at(self.cursor)
    }

    pub fn insert_char(&mut self, c: char) {
        if self.has_selection() {
            self.delete_selection();
            self.commit();
        }
        if !self.typing_group_ok(c) {
            self.commit();
            self.open_group(GroupKind::Typing);
        }
        let at = self.cursor;
        if self.typeover {
            if let Some(Item::Char(_)) = self.doc.paragraphs[at.para].items.get(at.idx) {
                self.apply(Op::Delete { at, len: 1 });
            }
        }
        self.apply(Op::Insert { at, items: vec![Item::Char(c)] });
        self.cursor.idx += 1;
        let g = self.history.open.as_mut().unwrap();
        g.kind = GroupKind::Typing;
        g.cursor_after = self.cursor;
        g.time = Instant::now();
        g.last_char_ws = c.is_whitespace();
        self.anchor = None;
        self.goal_x = None;
    }

    pub fn insert_code(&mut self, code: Code) {
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        let at = self.cursor;
        self.apply(Op::Insert { at, items: vec![Item::Code(code)] });
        self.cursor.idx += 1;
        self.commit();
        self.goal_x = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                self.split_paragraph_raw();
            }
            let items: Vec<Item> = line
                .chars()
                .map(|c| if c == '\t' { Item::Code(Code::Tab) } else { Item::Char(c) })
                .collect();
            if !items.is_empty() {
                let n = items.len();
                self.apply(Op::Insert { at: self.cursor, items });
                self.cursor.idx += n;
            }
        }
        self.commit();
        self.goal_x = None;
    }

    /// Split at the cursor, closing/reopening open attributes so that no
    /// span crosses the paragraph boundary.
    fn split_paragraph_raw(&mut self) {
        let at = self.cursor;
        let open = self.doc.attrs_at(at);
        let closes: Vec<Item> = open.keys().rev().map(|k| Item::Code(Code::Off(*k))).collect();
        let opens: Vec<Item> = open.values().map(|a| Item::Code(Code::On(a.clone()))).collect();
        let n_close = closes.len();
        if n_close > 0 {
            self.apply(Op::Insert { at, items: closes });
        }
        let split_at = Pos::new(at.para, at.idx + n_close);
        // New paragraph props: same as current, except a heading's "next" style
        // applies when splitting at the end of the paragraph.
        let mut props = self.doc.paragraphs[at.para].props.clone();
        let at_end = at.idx >= self.doc.paragraphs[at.para].items.len() - n_close;
        // A section break belongs to the last paragraph of its section, so
        // it moves to the new paragraph and leaves the one it was on.
        let sect_break = props.sect_break.take();
        if at_end {
            if let Some(sid) = props.style.clone() {
                if let Some(st) = self.doc.styles.get(&sid) {
                    if let Some(next) = st.next.clone() {
                        if next != sid {
                            props = ParaProps { style: Some(next), cell: props.cell, ..ParaProps::default() };
                        }
                    }
                }
            }
            props.page_break_before = None;
        }
        props.p_attrs = None;
        props.sect_break = sect_break.clone();
        if sect_break.is_some() {
            let mut head = self.doc.paragraphs[at.para].props.clone();
            head.sect_break = None;
            head.touch();
            self.apply(Op::SetParaProps { para: at.para, props: head });
        }
        self.apply(Op::Split { at: split_at, props: Some(props) });

        let n_open = opens.len();
        if n_open > 0 {
            self.apply(Op::Insert { at: Pos::new(at.para + 1, 0), items: opens });
        }
        self.cursor = Pos::new(at.para + 1, n_open);
    }

    pub fn newline(&mut self) {
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        self.split_paragraph_raw();
        self.commit();
        self.anchor = None;
        self.goal_x = None;
    }

    /// Delete the range, joining paragraphs as needed. Cursor ends at start.
    pub fn delete_range(&mut self, r: Range) {
        if r.is_empty() {
            return;
        }
        self.open_group(GroupKind::Other);
        if r.start.para == r.end.para {
            self.apply(Op::Delete { at: r.start, len: r.end.idx - r.start.idx });
        } else if self.doc.same_cell(r.start.para, r.end.para) && (r.start.para + 1..r.end.para).all(|i| self.doc.same_cell(i, r.start.para)) {
            // Tail of last paragraph, whole middle paragraphs, head of first.
            self.apply(Op::Delete { at: Pos::new(r.end.para, 0), len: r.end.idx });
            for pi in (r.start.para + 1..r.end.para).rev() {
                let n = self.doc.paragraphs[pi].items.len();
                self.apply(Op::Delete { at: Pos::new(pi, 0), len: n });
                self.apply(Op::Join { para: pi - 1 });
            }
            let n = self.doc.paragraphs[r.start.para].items.len();
            self.apply(Op::Delete { at: r.start, len: n - r.start.idx });
            self.apply(Op::Join { para: r.start.para });
        } else {
            self.delete_range_across_cells(r);
        }
        self.cursor = self.doc.clamp(r.start);
        self.anchor = None;
        let para = self.cursor.para;
        self.normalize_para(para);
    }

    /// A range crossing table cells: text is removed, but paragraphs are
    /// never joined across a cell boundary and every cell keeps one
    /// paragraph. A table lying wholly inside the range is removed.
    fn delete_range_across_cells(&mut self, r: Range) {
        // Tables strictly inside the range (neither end paragraph in them).
        let mut whole: Vec<u32> = Vec::new();
        for pi in r.start.para + 1..r.end.para {
            if let Some(c) = self.doc.cell_of(pi) {
                let ends_in = self.doc.cell_of(r.start.para).map_or(false, |x| x.table == c.table) || self.doc.cell_of(r.end.para).map_or(false, |x| x.table == c.table);
                if !ends_in && !whole.contains(&c.table) {
                    whole.push(c.table);
                }
            }
        }
        // Tail of the end paragraph.
        self.apply(Op::Delete { at: Pos::new(r.end.para, 0), len: r.end.idx });
        // Middle paragraphs, from the end: removed, or cleared when they are
        // the last paragraph left in their cell.
        for pi in (r.start.para + 1..r.end.para).rev() {
            let n = self.doc.paragraphs[pi].items.len();
            let in_cell = self.doc.cell_of(pi).is_some();
            let sole = in_cell && !self.doc.same_cell(pi - 1, pi) && (pi + 1 >= self.doc.paragraphs.len() || !self.doc.same_cell(pi, pi + 1));
            if sole || self.doc.paragraphs[pi].props.raw_block {
                self.apply(Op::Delete { at: Pos::new(pi, 0), len: n });
            } else {
                self.apply(Op::RemovePara { para: pi });
            }
        }
        // Head of the start paragraph.
        let n = self.doc.paragraphs[r.start.para].items.len();
        self.apply(Op::Delete { at: r.start, len: n - r.start.idx });
        // Whole tables go entirely.
        for id in whole {
            if let Some((s, e)) = self.doc.table_span(id) {
                self.remove_table_paras(id, s, e);
            }
        }
        // Join the ends when nothing structural separates them any more.
        let next = r.start.para + 1;
        if next < self.doc.paragraphs.len() && self.doc.same_cell(r.start.para, next) && !self.doc.paragraphs[next].props.raw_block {
            self.apply(Op::Join { para: r.start.para });
        }
    }

    pub fn delete_selection(&mut self) {
        if let Some(r) = self.selection() {
            self.delete_range(r);
        }
    }

    /// Replace a range with items, inside the current undo group (callers
    /// `commit` around it). The cursor ends after the inserted items, which
    /// take the formatting in force at the start of the range.
    pub fn replace_range(&mut self, r: Range, items: Vec<Item>) {
        self.delete_range(r);
        if !items.is_empty() {
            let at = self.cursor;
            let n = items.len();
            self.apply(Op::Insert { at, items });
            self.cursor.idx += n;
        }
        let para = self.cursor.para;
        self.normalize_para(para);
        self.anchor = None;
        self.goal_x = None;
    }


    /// Remove redundant code pairs (e.g. `[bold][BOLD]` adjacent) after a
    /// join or delete, keeping any empty pair around the cursor.
    fn normalize_para(&mut self, para: usize) {
        let items = &self.doc.paragraphs[para].items;
        if !items.iter().any(|i| matches!(i, Item::Code(Code::On(_)) | Item::Code(Code::Off(_)))) {
            return;
        }
        let cursor = if self.cursor.para == para { Some(self.cursor.idx) } else { None };
        let (new_items, map) = rewrite_attrs(items, 0..0, None, |_| {});
        if new_items != *items {
            let new_cursor = cursor.map(|c| map[c.min(map.len() - 1)]);
            self.apply(Op::ReplaceItems { para, items: new_items });
            if let Some(c) = new_cursor {
                self.cursor.idx = c;
            }
        }
    }

    pub fn backspace(&mut self, codes_visible: bool) {
        if self.has_selection() {
            self.commit();
            self.delete_selection();
            self.commit();
            return;
        }
        let c = self.cursor;
        if c.idx == 0 {
            if c.para == 0 || !self.doc.same_cell(c.para - 1, c.para) {
                return; // a cell boundary is not a character
            }
            self.commit();
            self.join_with_previous(c.para);
            self.commit();
            return;
        }
        let Some(prev) = self.prev_pos(c, codes_visible) else { return };
        if prev.para != c.para {
            // Only invisible codes precede the cursor: join with the previous paragraph.
            if !self.doc.same_cell(prev.para, c.para) {
                return;
            }
            self.commit();
            self.join_with_previous(c.para);
            self.commit();
            return;
        }
        if codes_visible {
            self.commit();
            self.delete_item_at(prev);
            self.commit();
            return;
        }
        // Delete the previous visible item; codes between stay.
        let cont = matches!(&self.history.open, Some(g) if g.kind == GroupKind::Backspacing && g.cursor_after == c && g.time.elapsed() < GROUP_TIMEOUT);
        if !cont {
            self.commit();
            self.open_group(GroupKind::Backspacing);
        }
        self.apply(Op::Delete { at: prev, len: 1 });
        self.cursor = Pos::new(prev.para, prev.idx);
        // If a zero-width code pair was left empty by this, keep it (it may be
        // intentional formatting the user is about to type into).
        let g = self.history.open.as_mut().unwrap();
        g.kind = GroupKind::Backspacing;
        g.cursor_after = self.cursor;
        g.time = Instant::now();
        self.goal_x = None;
    }

    pub fn delete_forward(&mut self, codes_visible: bool) {
        if self.has_selection() {
            self.commit();
            self.delete_selection();
            self.commit();
            return;
        }
        let c = self.cursor;
        let n_items = self.doc.paragraphs[c.para].items.len();
        let p = if codes_visible { c } else { self.skip_forward(c, false) };
        self.commit();
        if p.idx >= n_items {
            if c.para + 1 < self.doc.paragraphs.len() && self.doc.same_cell(c.para, c.para + 1) {
                self.join_with_previous(c.para + 1);
                self.cursor = c;
            }
        } else {
            self.delete_item_at(p);
            self.cursor = c;
        }
        self.commit();
        self.goal_x = None;
    }

    /// Delete one item; if it is a paired code, its partner goes too.
    pub fn delete_item_at(&mut self, p: Pos) {
        let partner = self.doc.paired_code(p);
        self.open_group(GroupKind::Other);
        match partner {
            Some(q) if q > p.idx => {
                self.apply(Op::Delete { at: Pos::new(p.para, q), len: 1 });
                self.apply(Op::Delete { at: p, len: 1 });
                self.cursor = p;
            }
            Some(q) => {
                self.apply(Op::Delete { at: p, len: 1 });
                self.apply(Op::Delete { at: Pos::new(p.para, q), len: 1 });
                self.cursor = Pos::new(p.para, p.idx - 1);
            }
            None => {
                self.apply(Op::Delete { at: p, len: 1 });
                self.cursor = p;
            }
        }
        self.anchor = None;
    }

    fn join_with_previous(&mut self, para: usize) {
        let prev = para - 1;
        let at = Pos::new(prev, self.doc.paragraphs[prev].items.len());
        self.apply(Op::Join { para: prev });
        self.cursor = at;
        self.anchor = None;
        self.normalize_para(prev);
        self.goal_x = None;
    }

    /// Delete a paragraph-level code (from Reveal Codes).
    pub fn clear_para_code(&mut self, para: usize, which: ParaCode) {
        self.commit();
        let mut props = self.doc.paragraphs[para].props.clone();
        reveal::clear_para_code(&mut props, which);
        self.apply(Op::SetParaProps { para, props });
        self.commit();
    }

    // ------------------------------------------------------------------
    // Formatting
    // ------------------------------------------------------------------

    /// Is `kind` effectively on for every character in the selection (or at
    /// the cursor)?
    pub fn attr_active(&self, kind: AttrKind) -> bool {
        let r = match self.selection() {
            Some(r) => r,
            None => {
                let m = self.doc.attrs_at(self.cursor);
                let rp = self.doc.run_props_at(self.cursor);
                return match kind {
                    AttrKind::Bold => rp.is_bold(),
                    AttrKind::Italic => rp.is_italic(),
                    AttrKind::Underline => rp.underline().is_some(),
                    AttrKind::Strike => rp.strike.unwrap_or(false),
                    AttrKind::DoubleStrike => rp.dstrike.unwrap_or(false),
                    AttrKind::SmallCaps => rp.small_caps.unwrap_or(false),
                    AttrKind::AllCaps => rp.all_caps.unwrap_or(false),
                    AttrKind::VertAlign => rp.vert_align().is_some(),
                    AttrKind::Highlight => rp.highlight().is_some(),
                    _ => m.contains_key(&kind),
                };
            }
        };
        let mut any = false;
        for pi in r.start.para..=r.end.para {
            let runs = self.doc.runs(pi);
            let s = if pi == r.start.para { r.start.idx } else { 0 };
            let e = if pi == r.end.para { r.end.idx } else { self.doc.paragraphs[pi].items.len() };
            for run in runs {
                if run.end <= s || run.start >= e {
                    continue;
                }
                let has_content = self.doc.paragraphs[pi].items[run.start.max(s)..run.end.min(e)]
                    .iter()
                    .any(|i| !i.is_code());
                if !has_content {
                    continue;
                }
                any = true;
                let on = match kind {
                    AttrKind::Bold => run.props.is_bold(),
                    AttrKind::Italic => run.props.is_italic(),
                    AttrKind::Underline => run.props.underline().is_some(),
                    AttrKind::Strike => run.props.strike.unwrap_or(false),
                    AttrKind::DoubleStrike => run.props.dstrike.unwrap_or(false),
                    AttrKind::SmallCaps => run.props.small_caps.unwrap_or(false),
                    AttrKind::AllCaps => run.props.all_caps.unwrap_or(false),
                    AttrKind::VertAlign => run.props.vert_align().is_some(),
                    AttrKind::Highlight => run.props.highlight().is_some(),
                    _ => run.attrs.iter().any(|a| a.kind() == kind),
                };
                if !on {
                    return false;
                }
            }
        }
        any
    }

    /// Toggle a boolean-ish attribute over the selection or at the cursor.
    /// If the style already supplies the attribute, toggling off inserts an
    /// explicit "off" code rather than merely removing the direct code.
    pub fn toggle_attr(&mut self, attr: Attr) {
        let kind = attr.kind();
        let active = self.attr_active(kind);
        if active {
            let base = self.doc.base_run_props(self.cursor.para);
            let from_style = match kind {
                AttrKind::Bold => base.is_bold(),
                AttrKind::Italic => base.is_italic(),
                AttrKind::Underline => base.underline().is_some(),
                AttrKind::Strike => base.strike.unwrap_or(false),
                AttrKind::DoubleStrike => base.dstrike.unwrap_or(false),
                AttrKind::SmallCaps => base.small_caps.unwrap_or(false),
                AttrKind::AllCaps => base.all_caps.unwrap_or(false),
                AttrKind::VertAlign => base.vert_align().is_some(),
                AttrKind::Highlight => base.highlight().is_some(),
                _ => false,
            };
            if from_style {
                let off = match attr {
                    Attr::Bold(_) => Attr::Bold(false),
                    Attr::Italic(_) => Attr::Italic(false),
                    Attr::Underline(_) => Attr::Underline(Underline::None),
                    Attr::Strike(_) => Attr::Strike(false),
                    Attr::DoubleStrike(_) => Attr::DoubleStrike(false),
                    Attr::SmallCaps(_) => Attr::SmallCaps(false),
                    Attr::AllCaps(_) => Attr::AllCaps(false),
                    Attr::VertAlign(_) => Attr::VertAlign(VertAlign::Baseline),
                    Attr::Highlight(_) => Attr::Highlight(Highlight::None),
                    other => other,
                };
                self.set_attr(kind, Some(off));
            } else {
                self.set_attr(kind, None);
            }
        } else {
            self.set_attr(kind, Some(attr));
        }
    }

    /// Apply `Some(attr)` (or remove the kind with `None`) over the selection,
    /// or at the cursor for subsequent typing.
    pub fn set_attr(&mut self, kind: AttrKind, attr: Option<Attr>) {
        self.commit();
        match self.selection() {
            Some(r) => {
                let cursor = self.cursor;
                let anchor = self.anchor;
                let mut new_cursor = cursor;
                let mut new_anchor = anchor;
                for pi in r.start.para..=r.end.para {
                    let items = self.doc.paragraphs[pi].items.clone();
                    let s = if pi == r.start.para { r.start.idx } else { 0 };
                    let e = if pi == r.end.para { r.end.idx } else { items.len() };
                    let a = attr.clone();
                    let (new_items, map) = rewrite_attrs(&items, s..e, None, |m| match &a {
                        Some(a) => {
                            m.insert(kind, a.clone());
                        }
                        None => {
                            m.remove(&kind);
                        }
                    });
                    if cursor.para == pi {
                        new_cursor.idx = map[cursor.idx.min(items.len())];
                    }
                    if let Some(an) = anchor {
                        if an.para == pi {
                            new_anchor = Some(Pos::new(pi, map[an.idx.min(items.len())]));
                        }
                    }
                    self.apply(Op::ReplaceItems { para: pi, items: new_items });
                }
                self.cursor = new_cursor;
                self.anchor = new_anchor;
            }
            None => {
                let c = self.cursor;
                let items = &self.doc.paragraphs[c.para].items;
                // WordPerfect: pressing the key again at `[bold]` steps over it.
                if attr.is_none() {
                    if let Some(Item::Code(Code::Off(k))) = items.get(c.idx) {
                        if *k == kind {
                            self.cursor.idx += 1;
                            return;
                        }
                    }
                }
                let items = items.clone();
                let a = attr.clone();
                let (new_items, map) = rewrite_attrs(&items, c.idx..c.idx, Some(c.idx), |m| match &a {
                    Some(a) => {
                        m.insert(kind, a.clone());
                    }
                    None => {
                        m.remove(&kind);
                    }
                });
                let nc = map[c.idx];
                self.apply(Op::ReplaceItems { para: c.para, items: new_items });
                self.cursor.idx = nc;
            }
        }
        self.commit();
    }

    /// Attributes to remove: all direct character formatting in the selection.
    pub fn clear_char_formatting(&mut self) {
        self.commit();
        let Some(r) = self.selection() else { return };
        let cursor = self.cursor;
        let anchor = self.anchor;
        let mut nc = cursor;
        let mut na = anchor;
        for pi in r.start.para..=r.end.para {
            let items = self.doc.paragraphs[pi].items.clone();
            let s = if pi == r.start.para { r.start.idx } else { 0 };
            let e = if pi == r.end.para { r.end.idx } else { items.len() };
            let (new_items, map) = rewrite_attrs(&items, s..e, None, |m: &mut AttrMap| m.clear());
            if cursor.para == pi {
                nc.idx = map[cursor.idx.min(items.len())];
            }
            if let Some(an) = anchor {
                if an.para == pi {
                    na = Some(Pos::new(pi, map[an.idx.min(items.len())]));
                }
            }
            self.apply(Op::ReplaceItems { para: pi, items: new_items });
        }
        self.cursor = nc;
        self.anchor = na;
        self.commit();
    }

    /// Paragraph indices covered by the selection (or the cursor's paragraph).
    pub fn selected_paras(&self) -> std::ops::RangeInclusive<usize> {
        match self.selection() {
            Some(r) => r.start.para..=r.end.para,
            None => self.cursor.para..=self.cursor.para,
        }
    }

    pub fn update_para_props(&mut self, f: impl Fn(&mut ParaProps)) {
        self.commit();
        for pi in self.selected_paras() {
            if self.doc.paragraphs[pi].props.raw_block {
                continue;
            }
            let mut props = self.doc.paragraphs[pi].props.clone();
            f(&mut props);
            if props != self.doc.paragraphs[pi].props {
                props.touch();
                self.apply(Op::SetParaProps { para: pi, props });
            }
        }
        self.commit();
    }

    pub fn set_style(&mut self, style_id: &str) {
        let id = style_id.to_string();
        self.update_para_props(move |p| p.style = Some(id.clone()));
    }

    pub fn set_section(&mut self, s: SectionProps) {
        self.commit();
        self.apply(Op::SetSection(s));
        self.commit();
    }

    // ------------------------------------------------------------------
    // Clipboard
    // ------------------------------------------------------------------

    pub fn copy(&mut self) -> Option<Fragment> {
        let r = self.selection()?;
        let f = self.fragment(r);
        self.push_cut(f.clone());
        Some(f)
    }

    pub fn cut(&mut self) -> Option<Fragment> {
        let r = self.selection()?;
        let f = self.fragment(r);
        self.commit();
        self.delete_range(r);
        self.commit();
        self.push_cut(f.clone());
        Some(f)
    }

    fn push_cut(&mut self, f: Fragment) {
        self.cut_ring.push_front(f);
        while self.cut_ring.len() > CUT_RING_SIZE {
            self.cut_ring.pop_back();
        }
    }

    pub fn cut_ring(&self) -> &VecDeque<Fragment> {
        &self.cut_ring
    }

    pub fn paste(&mut self, f: &Fragment) {
        if f.paragraphs.is_empty() {
            return;
        }
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        let n = f.paragraphs.len();
        let cell = self.doc.cell_of(self.cursor.para);
        for (i, p) in f.paragraphs.iter().enumerate() {
            if i > 0 {
                self.split_paragraph_raw();
                // Middle/last paragraphs carry their own props, but always
                // belong to the cell they are pasted into.
                let props = if i + 1 < n || p.props != ParaProps::default() {
                    let mut props = p.props.clone();
                    props.cell = cell;
                    props.raw_block = false;
                    Some(props)
                } else {
                    None
                };
                if let Some(props) = props {
                    self.apply(Op::SetParaProps { para: self.cursor.para, props });
                }
            }
            if !p.items.is_empty() {
                let at = self.cursor;
                self.apply(Op::Insert { at, items: p.items.clone() });
                self.cursor.idx += p.items.len();
            }
        }
        let para = self.cursor.para;
        self.normalize_para(para);
        self.commit();
        self.anchor = None;
        self.goal_x = None;
    }

    // ------------------------------------------------------------------
    // Whole-document replacement (open/recover)
    // ------------------------------------------------------------------

    pub fn replace_document(&mut self, doc: Document) {
        let n = doc.paragraphs.len();
        self.doc = doc;
        self.cursor = Pos::default();
        self.anchor = None;
        self.history = History::default();
        self.layout.print = vec![None; n];
        self.layout.screen = vec![None; n];
        self.layout.labels = vec![None; n];
        self.layout.labels_dirty = true;
        self.layout.pagination_dirty = true;
        self.layout.hf.clear();
        self.dirty = false;
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(s: &str) -> Editor {
        Editor::new(crate::text::from_text(s, false))
    }

    #[test]
    fn section_breaks_split_and_scope_page_setup() {
        let mut e = ed("alpha\nbeta\ngamma");
        e.move_to(Pos::new(1, 2), false);
        assert!(e.insert_section_break(SectionStart::NextPage));
        // "be" ends section 1; "ta" starts section 2.
        assert_eq!(e.doc.text(), "alpha\nbe\nta\ngamma");
        assert!(e.doc.paragraphs[1].props.sect_break.is_some());
        assert_eq!(e.cursor, Pos::new(2, 0));
        assert_eq!(e.cursor_section(), (2, 2));
        assert_eq!(e.doc.section.start, SectionStart::NextPage);
        assert_eq!(e.page_count(), 2);
        // Landscape applies to the cursor's section only.
        let mut s = e.section_at(2);
        s.orientation = Orientation::Landscape;
        std::mem::swap(&mut s.page_width, &mut s.page_height);
        e.set_section_at(2, s);
        assert_eq!(e.section_at(0).orientation, Orientation::Portrait);
        assert_eq!(e.section_at(3).orientation, Orientation::Landscape);
        // Enter at the end of a section's last paragraph keeps the break at
        // the end of the section.
        e.move_to(Pos::new(1, 2), false);
        e.newline();
        assert!(e.doc.paragraphs[1].props.sect_break.is_none());
        assert!(e.doc.paragraphs[2].props.sect_break.is_some());
        assert_eq!(e.cursor_section(), (1, 2));
        // Undo it all.
        while e.undo() {}
        assert_eq!(e.doc.text(), "alpha\nbeta\ngamma");
        assert_eq!(e.doc.section_count(), 1);
        assert_eq!(e.page_count(), 1);
    }

    #[test]
    fn type_and_undo_by_word() {
        let mut e = ed("");
        for c in "hello world".chars() {
            e.insert_char(c);
        }
        assert_eq!(e.doc.text(), "hello world");
        e.undo();
        assert_eq!(e.doc.text(), "hello");
        e.undo();
        assert_eq!(e.doc.text(), "");
        e.redo();
        assert_eq!(e.doc.text(), "hello");
    }

    #[test]
    fn bold_selection_and_delete_code() {
        let mut e = ed("hello world");
        e.anchor = Some(Pos::new(0, 0));
        e.cursor = Pos::new(0, 5);
        e.toggle_attr(Attr::Bold(true));
        assert!(e.attr_active(AttrKind::Bold));
        let items = &e.doc.paragraphs[0].items;
        assert_eq!(items[0], Item::Code(Code::On(Attr::Bold(true))));
        assert_eq!(items[6], Item::Code(Code::Off(AttrKind::Bold)));
        assert_eq!(e.doc.runs(0).len(), 2);
        // Delete the [BOLD] code: its partner goes too.
        e.delete_item_at(Pos::new(0, 0));
        assert!(e.doc.paragraphs[0].items.iter().all(|i| !i.is_code()));
        assert_eq!(e.doc.text(), "hello world");
        e.undo();
        assert_eq!(e.doc.runs(0).len(), 2);
    }

    #[test]
    fn bold_at_cursor_then_type() {
        let mut e = ed("ab");
        e.cursor = Pos::new(0, 1);
        e.toggle_attr(Attr::Bold(true));
        e.insert_char('X');
        let items = &e.doc.paragraphs[0].items;
        assert_eq!(
            *items,
            vec![
                Item::Char('a'),
                Item::Code(Code::On(Attr::Bold(true))),
                Item::Char('X'),
                Item::Code(Code::Off(AttrKind::Bold)),
                Item::Char('b')
            ]
        );
        // Toggling again steps past the closing code.
        e.toggle_attr(Attr::Bold(true));
        assert_eq!(e.cursor.idx, 4);
        e.insert_char('Y');
        assert_eq!(e.doc.text(), "aXYb");
        assert!(!e.doc.run_props_at(e.cursor).is_bold());
    }

    #[test]
    fn newline_splits_spans() {
        let mut e = ed("abcd");
        e.anchor = Some(Pos::new(0, 0));
        e.cursor = Pos::new(0, 4);
        e.toggle_attr(Attr::Bold(true));
        e.anchor = None;
        e.cursor = Pos::new(0, 3); // after 'b' inside bold ([BOLD]ab|cd[bold])
        e.newline();
        assert_eq!(e.doc.paragraphs.len(), 2);
        assert_eq!(e.doc.paragraphs[0].text(), "ab");
        assert_eq!(e.doc.paragraphs[1].text(), "cd");
        assert!(e.doc.runs(1)[0].props.is_bold());
        assert_eq!(e.cursor, Pos::new(1, 1));
        e.backspace(false);
        assert_eq!(e.doc.paragraphs.len(), 1);
        assert_eq!(e.doc.text(), "abcd");
        assert_eq!(e.doc.runs(0).len(), 1);
    }

    #[test]
    fn multi_paragraph_delete_and_paste() {
        let mut e = ed("one\ntwo\nthree");
        e.anchor = Some(Pos::new(0, 2));
        e.cursor = Pos::new(2, 2);
        let f = e.cut().unwrap();
        assert_eq!(e.doc.text(), "onree");
        assert_eq!(f.text(), "e\ntwo\nth");
        e.paste(&f);
        assert_eq!(e.doc.text(), "one\ntwo\nthree");
        e.undo();
        assert_eq!(e.doc.text(), "onree");
        e.undo();
        assert_eq!(e.doc.text(), "one\ntwo\nthree");
    }

    #[test]
    fn page_position_reports() {
        let mut e = ed("hello");
        let (pg, ln, pos) = e.cursor_page_ln_pos();
        assert_eq!(pg, 1);
        assert_eq!(ln, 1440);
        assert_eq!(pos, 1440);
        e.cursor = Pos::new(0, 5);
        let (_, _, pos2) = e.cursor_page_ln_pos();
        assert!(pos2 > pos);
    }
}
