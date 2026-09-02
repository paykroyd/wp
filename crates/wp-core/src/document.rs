//! The document: paragraphs plus stylesheet and page geometry, and the
//! operations that resolve formatting from the item stream.

use crate::model::*;
use crate::numbering::Numbering;
use crate::style::StyleSheet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    pub styles: StyleSheet,
    pub section: SectionProps,
    pub numbering: Numbering,
    pub footnotes: Vec<Footnote>,
    pub extra_rels: Vec<ExtraRel>,
    /// Table grids, by id. Contents are the paragraphs whose `props.cell`
    /// names the table.
    pub tables: BTreeMap<u32, Table>,
    /// Header and footer bodies by id; sections refer to them by `HfRef`.
    pub headers: BTreeMap<String, HeaderFooter>,
    /// Odd and even pages have different headers and footers
    /// (`w:evenAndOddHeaders` in the settings part).
    pub even_odd_headers: bool,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

/// A maximal stretch of a paragraph's items sharing the same effective
/// character formatting. Codes inside the range that are not attribute
/// codes (tabs, breaks, bookmarks, opaque) are included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub props: RunProps,
    /// The direct (in-stream) attributes open over this run, innermost last.
    pub attrs: Vec<Attr>,
}

/// Effective attribute state: one value per kind.
pub type AttrMap = BTreeMap<AttrKind, Attr>;

pub fn attr_map(stack: &[Attr]) -> AttrMap {
    let mut m = AttrMap::new();
    for a in stack {
        m.insert(a.kind(), a.clone());
    }
    m
}

impl Document {
    pub fn new() -> Document {
        Document {
            paragraphs: vec![Paragraph::new()],
            styles: StyleSheet::builtin(),
            section: SectionProps::default(),
            numbering: Numbering::default(),
            footnotes: Vec::new(),
            extra_rels: Vec::new(),
            tables: BTreeMap::new(),
            headers: BTreeMap::new(),
            even_odd_headers: false,
        }
    }


    pub fn from_paragraphs(paragraphs: Vec<Paragraph>) -> Document {
        let mut d = Document::new();
        if !paragraphs.is_empty() {
            d.paragraphs = paragraphs;
        }
        d
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            for it in &p.items {
                match it {
                    Item::Char(c) => s.push(*c),
                    Item::Code(Code::Tab) => s.push('\t'),
                    Item::Code(Code::LineBreak) => s.push('\n'),
                    _ => {}
                }
            }
        }
        s
    }

    pub fn char_count(&self) -> usize {
        self.paragraphs.iter().map(|p| p.char_count()).sum()
    }

    pub fn word_count(&self) -> usize {
        let mut n = 0;
        for p in &self.paragraphs {
            let mut in_word = false;
            for it in &p.items {
                let ws = match it {
                    Item::Char(c) => c.is_whitespace(),
                    Item::Code(Code::Tab) | Item::Code(Code::LineBreak) => true,
                    Item::Code(_) => continue,
                };
                if ws {
                    in_word = false;
                } else if !in_word {
                    in_word = true;
                    n += 1;
                }
            }
        }
        n
    }

    pub fn end_pos(&self) -> Pos {
        let last = self.paragraphs.len() - 1;
        Pos::new(last, self.paragraphs[last].items.len())
    }

    pub fn clamp(&self, p: Pos) -> Pos {
        let para = p.para.min(self.paragraphs.len() - 1);
        Pos::new(para, p.idx.min(self.paragraphs[para].items.len()))
    }

    /// Effective paragraph properties: defaults ← style chain ← list level ← direct.
    pub fn para_props(&self, para: usize) -> ParaProps {
        let p = &self.paragraphs[para].props;
        let from_style = self.styles.resolve_para_style(p.style.as_deref());
        let list = p.list.or(from_style.list).filter(|l| l.num_id > 0);
        match list.and_then(|l| self.numbering.level(l.num_id, l.level)) {
            Some(level) if !p.raw_block => {
                // Only the level's indent and tabs apply to the paragraph; the
                // rest of its pPr is for the label.
                let lvl = ParaProps { indent_left: level.para.indent_left, first_line: level.para.first_line, hanging: level.para.hanging, tabs: level.para.tabs.clone(), ..Default::default() };
                let mut merged = from_style.merge(&lvl).merge(p);
                merged.raw_ppr = p.raw_ppr.clone();
                merged
            }
            _ => from_style.merge(p),
        }
    }


    /// The run properties a character with no direct attributes would have
    /// in this paragraph.
    pub fn base_run_props(&self, para: usize) -> RunProps {
        let p = &self.paragraphs[para].props;
        self.styles.resolve_para_style_run(p.style.as_deref())
    }

    /// Resolve a full attribute state to effective run properties.
    pub fn resolve_attrs(&self, base: &RunProps, attrs: &AttrMap) -> RunProps {
        let mut r = base.clone();
        if let Some(Attr::CharStyle(id)) = attrs.get(&AttrKind::CharStyle) {
            r = r.merge(&self.styles.resolve_char_style(id));
        }
        for (k, a) in attrs {
            if *k != AttrKind::CharStyle {
                r.apply(a);
            }
        }
        r
    }

    /// Split a paragraph into runs of uniform formatting.
    pub fn runs(&self, para: usize) -> Vec<Run> {
        let base = self.base_run_props(para);
        let items = &self.paragraphs[para].items;
        let mut runs: Vec<Run> = Vec::new();
        let mut stack: Vec<Attr> = Vec::new();
        let mut start = 0usize;
        let mut cur_map = AttrMap::new();
        let mut cur_props = self.resolve_attrs(&base, &cur_map);
        let mut has_content = false;
        for (i, it) in items.iter().enumerate() {
            match it {
                Item::Code(Code::On(a)) => {
                    stack.push(a.clone());
                }
                Item::Code(Code::Off(k)) => {
                    if let Some(pos) = stack.iter().rposition(|a| a.kind() == *k) {
                        stack.remove(pos);
                    }
                }
                _ => {
                    has_content = true;
                    continue;
                }
            }
            let new_map = attr_map(&stack);
            if new_map != cur_map {
                if has_content {
                    runs.push(Run { start, end: i + 1, props: cur_props.clone(), attrs: stack_before(&cur_map) });
                    start = i + 1;
                    has_content = false;
                } else {
                    // no content yet in this run: extend it rather than emit empty
                }
                cur_map = new_map;
                cur_props = self.resolve_attrs(&base, &cur_map);
            }
        }
        if start < items.len() || runs.is_empty() {
            runs.push(Run { start, end: items.len(), props: cur_props, attrs: stack_before(&cur_map) });
        }
        runs
    }

    /// Effective run properties at a position (what a typed character would get).
    pub fn run_props_at(&self, pos: Pos) -> RunProps {
        let base = self.base_run_props(pos.para);
        let m = self.attrs_at(pos);
        self.resolve_attrs(&base, &m)
    }

    /// Attribute state at a position — the set of codes open just before `idx`.
    pub fn attrs_at(&self, pos: Pos) -> AttrMap {
        let items = &self.paragraphs[pos.para].items;
        let mut stack: Vec<Attr> = Vec::new();
        for it in items.iter().take(pos.idx) {
            match it {
                Item::Code(Code::On(a)) => stack.push(a.clone()),
                Item::Code(Code::Off(k)) => {
                    if let Some(p) = stack.iter().rposition(|a| a.kind() == *k) {
                        stack.remove(p);
                    }
                }
                _ => {}
            }
        }
        attr_map(&stack)
    }

    /// Find the index of the code paired with the code at `idx`, if any.
    pub fn paired_code(&self, pos: Pos) -> Option<usize> {
        let items = &self.paragraphs[pos.para].items;
        match items.get(pos.idx)? {
            Item::Code(Code::On(a)) => {
                let k = a.kind();
                let mut depth = 0;
                for (j, it) in items.iter().enumerate().skip(pos.idx + 1) {
                    match it {
                        Item::Code(Code::On(b)) if b.kind() == k => depth += 1,
                        Item::Code(Code::Off(kk)) if *kk == k => {
                            if depth == 0 {
                                return Some(j);
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                None
            }
            Item::Code(Code::Off(k)) => {
                let mut depth = 0;
                for j in (0..pos.idx).rev() {
                    match &items[j] {
                        Item::Code(Code::Off(kk)) if kk == k => depth += 1,
                        Item::Code(Code::On(b)) if b.kind() == *k => {
                            if depth == 0 {
                                return Some(j);
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                None
            }
            Item::Code(Code::Bookmark(name)) => items
                .iter()
                .enumerate()
                .skip(pos.idx + 1)
                .find(|(_, it)| matches!(it, Item::Code(Code::BookmarkEnd(n)) if n == name))
                .map(|(j, _)| j),
            Item::Code(Code::BookmarkEnd(name)) => (0..pos.idx)
                .rev()
                .find(|&j| matches!(&items[j], Item::Code(Code::Bookmark(n)) if n == name)),
            Item::Code(Code::Opaque(o)) => match o.kind {
                OpaqueKind::Open(id) => items
                    .iter()
                    .enumerate()
                    .skip(pos.idx + 1)
                    .find(|(_, it)| matches!(it, Item::Code(Code::Opaque(c)) if c.kind == OpaqueKind::Close(id)))
                    .map(|(j, _)| j),
                OpaqueKind::Close(id) => (0..pos.idx)
                    .rev()
                    .find(|&j| matches!(&items[j], Item::Code(Code::Opaque(c)) if c.kind == OpaqueKind::Open(id))),
                OpaqueKind::Element => None,
            },
            _ => None,
        }
    }

    /// Heading outline: (paragraph index, level, text) for every paragraph
    /// whose effective style has an outline level.
    pub fn headings(&self) -> Vec<(usize, u8, String)> {
        let mut out = Vec::new();
        for i in 0..self.paragraphs.len() {
            let pp = self.para_props(i);
            if let Some(l) = pp.outline_level {
                if l < 9 {
                    out.push((i, l, self.paragraphs[i].text()));
                }
            }
        }
        out
    }

    pub fn bookmarks(&self) -> Vec<(String, Pos)> {
        let mut out = Vec::new();
        for (pi, p) in self.paragraphs.iter().enumerate() {
            for (ii, it) in p.items.iter().enumerate() {
                if let Item::Code(Code::Bookmark(n)) = it {
                    out.push((n.clone(), Pos::new(pi, ii)));
                }
            }
        }
        out
    }
}

fn stack_before(m: &AttrMap) -> Vec<Attr> {
    m.values().cloned().collect()
}

/// Rewrite the attribute codes of a paragraph from a per-item attribute
/// state, after letting `f` modify the state of items in `range`
/// (`range` is in *old* item indices). `cursor` (an old index) is treated as
/// a zero-width phantom item so an empty code pair can be left around it.
///
/// Returns the new items and a map from old index → new index (length
/// `old.len() + 1`).
pub fn rewrite_attrs(
    items: &[Item],
    range: std::ops::Range<usize>,
    cursor: Option<usize>,
    mut f: impl FnMut(&mut AttrMap),
) -> (Vec<Item>, Vec<usize>) {
    struct Entry {
        old_idx: usize,
        item: Option<Item>,
        state: AttrMap,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(items.len() + 1);
    let mut stack: Vec<Attr> = Vec::new();
    let mut cursor_done = false;
    for (i, it) in items.iter().enumerate() {
        if cursor == Some(i) && !cursor_done {
            entries.push(Entry { old_idx: i, item: None, state: attr_map(&stack) });
            cursor_done = true;
        }
        match it {
            Item::Code(Code::On(a)) => stack.push(a.clone()),
            Item::Code(Code::Off(k)) => {
                if let Some(p) = stack.iter().rposition(|a| a.kind() == *k) {
                    stack.remove(p);
                }
            }
            _ => entries.push(Entry { old_idx: i, item: Some(it.clone()), state: attr_map(&stack) }),
        }
    }
    if cursor.is_some() && !cursor_done {
        entries.push(Entry { old_idx: items.len(), item: None, state: attr_map(&stack) });
    }
    for e in entries.iter_mut() {
        let in_range = if e.item.is_none() {
            // phantom: in range if the range is empty and at the cursor, or
            // if it lies strictly inside a non-empty range.
            (range.start == range.end && e.old_idx == range.start)
                || (e.old_idx > range.start && e.old_idx < range.end)
        } else {
            range.contains(&e.old_idx)
        };
        if in_range {
            f(&mut e.state);
        }
    }

    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    let mut map = vec![usize::MAX; items.len() + 1];
    let mut open: Vec<Attr> = Vec::new(); // in opening order
    for e in &entries {
        // close attrs that are gone or changed, innermost first
        let mut to_close: Vec<usize> = Vec::new();
        for (i, a) in open.iter().enumerate() {
            match e.state.get(&a.kind()) {
                Some(b) if b == a => {}
                _ => to_close.push(i),
            }
        }
        // closing an outer span requires closing inner ones after it too
        if let Some(&first) = to_close.first() {
            let reopen: Vec<Attr> = open[first..]
                .iter()
                .filter(|a| e.state.get(&a.kind()) == Some(*a))
                .cloned()
                .collect();
            for a in open[first..].iter().rev() {
                out.push(Item::Code(Code::Off(a.kind())));
            }
            open.truncate(first);
            for a in reopen {
                out.push(Item::Code(Code::On(a.clone())));
                open.push(a);
            }
        }
        for (k, a) in &e.state {
            if !open.iter().any(|o| o.kind() == *k) {
                out.push(Item::Code(Code::On(a.clone())));
                open.push(a.clone());
            }
        }
        if map[e.old_idx] == usize::MAX {
            map[e.old_idx] = out.len();
        }
        if let Some(it) = &e.item {
            out.push(it.clone());
        }
    }
    for a in open.iter().rev() {
        out.push(Item::Code(Code::Off(a.kind())));
    }
    // Fill the map: positions with no entry map to the next entry's position.
    let mut next = out.len();
    for i in (0..map.len()).rev() {
        if map[i] == usize::MAX {
            map[i] = next;
        } else {
            next = map[i];
        }
    }
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(s: &str) -> Vec<Item> {
        s.chars().map(Item::Char).collect()
    }

    #[test]
    fn rewrite_bold_range() {
        let it = items("hello world");
        let (out, map) = rewrite_attrs(&it, 0..5, None, |m| {
            m.insert(AttrKind::Bold, Attr::Bold(true));
        });
        assert_eq!(out[0], Item::Code(Code::On(Attr::Bold(true))));
        assert_eq!(out[6], Item::Code(Code::Off(AttrKind::Bold)));
        assert_eq!(out.len(), 13);
        assert_eq!(map[0], 1);
        assert_eq!(map[5], 7);
        assert_eq!(map[11], 13);
    }

    #[test]
    fn rewrite_phantom_pair_at_cursor() {
        let it = items("ab");
        let (out, map) = rewrite_attrs(&it, 1..1, Some(1), |m| {
            m.insert(AttrKind::Bold, Attr::Bold(true));
        });
        assert_eq!(
            out,
            vec![
                Item::Char('a'),
                Item::Code(Code::On(Attr::Bold(true))),
                Item::Code(Code::Off(AttrKind::Bold)),
                Item::Char('b')
            ]
        );
        assert_eq!(map[1], 2);
    }

    #[test]
    fn runs_split_on_codes() {
        let mut d = Document::new();
        d.paragraphs[0].items = vec![
            Item::Char('a'),
            Item::Code(Code::On(Attr::Bold(true))),
            Item::Char('b'),
            Item::Code(Code::Off(AttrKind::Bold)),
            Item::Char('c'),
        ];
        let r = d.runs(0);
        assert_eq!(r.len(), 3);
        assert!(!r[0].props.is_bold());
        assert!(r[1].props.is_bold());
        assert!(!r[2].props.is_bold());
        assert_eq!(d.paired_code(Pos::new(0, 1)), Some(3));
        assert_eq!(d.paired_code(Pos::new(0, 3)), Some(1));
    }
}
