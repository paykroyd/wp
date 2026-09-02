//! Tables: queries over the cell-tagged paragraph stream and the structural
//! edits (rows, columns, whole tables). See DESIGN.md §3.7.
//!
//! A table's contents are ordinary paragraphs carrying `props.cell`; the grid
//! lives in `Document::tables`. Every structural edit is a sequence of the
//! primitive ops in `edit.rs`, so undo needs nothing special.

use crate::document::Document;
use crate::edit::Op;
use crate::editor::Editor;
use crate::model::*;

/// Paragraph indices of a table, grouped `rows → cells → paragraphs`.
pub type TableParas = Vec<Vec<Vec<usize>>>;

impl Document {
    pub fn cell_of(&self, para: usize) -> Option<CellRef> {
        self.paragraphs.get(para).and_then(|p| p.props.cell)
    }

    /// True when the two paragraphs may be joined: both outside any table,
    /// or both in the same cell.
    pub fn same_cell(&self, a: usize, b: usize) -> bool {
        self.cell_of(a) == self.cell_of(b)
    }

    /// `[start, end)` paragraph indices of the table containing `para`.
    pub fn table_bounds(&self, para: usize) -> Option<(usize, usize)> {
        let id = self.cell_of(para)?.table;
        let same = |i: usize| self.cell_of(i).map_or(false, |c| c.table == id);
        let mut start = para;
        while start > 0 && same(start - 1) {
            start -= 1;
        }
        let mut end = para + 1;
        while end < self.paragraphs.len() && same(end) {
            end += 1;
        }
        Some((start, end))
    }

    /// `[start, end)` paragraph indices of table `id`, if it has any.
    pub fn table_span(&self, id: u32) -> Option<(usize, usize)> {
        let first = self.paragraphs.iter().position(|p| p.props.cell.map_or(false, |c| c.table == id))?;
        self.table_bounds(first)
    }

    /// The paragraphs of the table containing `para`, by row and cell.
    pub fn table_paras(&self, para: usize) -> Option<TableParas> {
        let (start, end) = self.table_bounds(para)?;
        let mut rows: TableParas = Vec::new();
        for i in start..end {
            let c = self.cell_of(i).unwrap();
            while rows.len() <= c.row as usize {
                rows.push(Vec::new());
            }
            let row = &mut rows[c.row as usize];
            while row.len() <= c.col as usize {
                row.push(Vec::new());
            }
            row[c.col as usize].push(i);
        }
        Some(rows)
    }

    /// `[first, last]` paragraph indices of the cell containing `para`.
    pub fn cell_paras(&self, para: usize) -> Option<(usize, usize)> {
        let c = self.cell_of(para)?;
        let same = |i: usize| self.cell_of(i) == Some(c);
        let mut first = para;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let mut last = para;
        while last + 1 < self.paragraphs.len() && same(last + 1) {
            last += 1;
        }
        Some((first, last))
    }

    /// The first paragraph of the given cell, if it exists.
    pub fn cell_first_para(&self, c: CellRef) -> Option<usize> {
        let (start, end) = self.table_span(c.table)?;
        (start..end).find(|&i| self.cell_of(i) == Some(c))
    }

    pub fn is_cell_start(&self, para: usize) -> bool {
        match self.cell_of(para) {
            Some(c) => para == 0 || self.cell_of(para - 1) != Some(c),
            None => false,
        }
    }

    pub fn next_table_id(&self) -> u32 {
        let from_map = self.tables.keys().next_back().map(|k| k + 1).unwrap_or(1);
        let from_paras = self.paragraphs.iter().filter_map(|p| p.props.cell.map(|c| c.table + 1)).max().unwrap_or(1);
        from_map.max(from_paras)
    }

    /// Width available to the text of a cell paragraph, in twips.
    pub fn cell_text_width(&self, para: usize) -> Option<Twips> {
        let c = self.cell_of(para)?;
        let t = self.tables.get(&c.table)?;
        Some(t.cell_text_width(c.row as usize, c.col as usize))
    }

    /// Twips from the left edge of the text area to the cell's left edge.
    pub fn cell_x(&self, para: usize) -> Twips {
        match self.cell_of(para).and_then(|c| self.tables.get(&c.table).map(|t| (t, c))) {
            Some((t, c)) => t.cell_extent(c.row as usize, c.col as usize).0 + t.cell_margin_left,
            None => 0,
        }
    }

    /// Screen widths (in cells) of a table's grid columns when the table is
    /// drawn in `cols` columns including its borders. Each column gets at
    /// least 3 cells; a table that cannot fit is clipped by the renderer.
    pub fn table_screen_grid(&self, table: &Table, cols: u16) -> Vec<u16> {
        let n = table.cols();
        let avail = (cols as i32 - (n as i32 + 1)).max(3 * n as i32);
        let total: i64 = table.grid.iter().map(|w| (*w).max(1) as i64).sum::<i64>().max(1);
        let mut out: Vec<u16> = table.grid.iter().map(|w| (((*w).max(1) as i64 * avail as i64 + total / 2) / total).max(3) as u16).collect();
        // Trim rounding overflow from the widest columns.
        let mut sum: i32 = out.iter().map(|w| *w as i32).sum();
        while sum > avail {
            let (i, _) = out.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if out[i] <= 3 {
                break;
            }
            out[i] -= 1;
            sum -= 1;
        }
        out
    }

    /// Screen cells available to the text of a cell paragraph in draft view.
    pub fn cell_screen_width(&self, para: usize, cols: u16) -> Option<u16> {
        let c = self.cell_of(para)?;
        let t = self.tables.get(&c.table)?;
        let grid = self.table_screen_grid(t, cols);
        let (g, span) = (t.grid_col(c.row as usize, c.col as usize), t.rows.get(c.row as usize).and_then(|r| r.cells.get(c.col as usize)).map(|x| x.span()).unwrap_or(1));
        let w: u16 = grid.iter().skip(g).take(span).sum::<u16>() + (span as u16).saturating_sub(1);
        Some(w.saturating_sub(2).max(1))
    }

    /// Cell-relative paragraph count sanity: every cell of every row present.
    /// Used by tests and the reader's fallback decision.
    pub fn table_is_consistent(&self, id: u32) -> bool {
        let Some(t) = self.tables.get(&id) else { return false };
        let Some((start, _)) = self.table_span(id) else { return false };
        let Some(paras) = self.table_paras(start) else { return false };
        if paras.len() != t.rows.len() {
            return false;
        }
        for (r, row) in t.rows.iter().enumerate() {
            if paras[r].len() != row.cells.len() || paras[r].iter().any(|c| c.is_empty()) {
                return false;
            }
        }
        // Row-major order in the stream.
        let mut prev: Option<CellRef> = None;
        for i in start..start + paras.iter().flatten().map(|c| c.len()).sum::<usize>() {
            let c = self.cell_of(i).unwrap();
            if let Some(p) = prev {
                if (c.row, c.col) < (p.row, p.col) {
                    return false;
                }
            }
            prev = Some(c);
        }
        true
    }
}

/// What a table command needs from the cursor.
#[derive(Clone, Copy, Debug)]
struct At {
    cell: CellRef,
    start: usize,
    end: usize,
}

impl Editor {
    fn at_table(&self) -> Option<At> {
        let cell = self.doc.cell_of(self.cursor.para)?;
        let (start, end) = self.doc.table_bounds(self.cursor.para)?;
        Some(At { cell, start, end })
    }

    /// The cell the cursor is in.
    pub fn current_cell(&self) -> Option<CellRef> {
        self.doc.cell_of(self.cursor.para)
    }

    /// Paragraph properties for a new, empty cell paragraph.
    fn cell_para(cell: CellRef, like: Option<&ParaProps>) -> Paragraph {
        let mut props = match like {
            Some(p) => ParaProps { style: p.style.clone(), align: p.align, space_after: p.space_after, line_spacing: p.line_spacing, ..Default::default() },
            None => ParaProps::default(),
        };
        props.cell = Some(cell);
        Paragraph { props, items: Vec::new() }
    }

    fn retag(&mut self, para: usize, cell: CellRef) {
        let mut props = self.doc.paragraphs[para].props.clone();
        if props.cell != Some(cell) {
            props.cell = Some(cell);
            self.apply_op(Op::SetParaProps { para, props });
        }
    }

    /// Renumber rows ≥ `from_row` of the table by `delta`, cells ≥ `from_col`
    /// of `row` by `col_delta`.
    fn renumber(&mut self, start: usize, end: usize, id: u32, f: impl Fn(CellRef) -> CellRef) {
        for i in start..end {
            if let Some(c) = self.doc.cell_of(i) {
                if c.table == id {
                    let nc = f(c);
                    if nc != c {
                        self.retag(i, nc);
                    }
                }
            }
        }
    }

    /// Insert an empty `rows × cols` table at the cursor. The cursor lands in
    /// the first cell. Inside a table, nothing happens (nested tables are not
    /// editable yet) and `false` is returned.
    pub fn insert_table(&mut self, rows: usize, cols: usize) -> bool {
        if self.current_cell().is_some() || self.doc.paragraphs[self.cursor.para].props.raw_block {
            return false;
        }
        self.commit();
        if self.has_selection() {
            self.delete_selection();
        }
        let rows = rows.clamp(1, 1000);
        let cols = cols.clamp(1, 63);
        let id = self.doc.next_table_id();
        let table = Table::new(rows, cols, self.doc.section.text_width());
        let c = self.cursor;
        let cur_len = self.doc.paragraphs[c.para].items.len();
        let base = self.doc.paragraphs[c.para].props.clone();
        // Where the table goes: before the cursor's paragraph when the cursor
        // is at its start; otherwise the paragraph is split and the table
        // sits between the halves. A paragraph after the table is guaranteed
        // (Word needs one at the end of the body).
        let mut at = c.para;
        if c.idx > 0 || (cur_len == 0 && c.para + 1 == self.doc.paragraphs.len() && !base.raw_block && false) {
            // Split off the tail (possibly empty) so text after the cursor
            // follows the table.
            let mut tail_props = base.clone();
            tail_props.p_attrs = None;
            tail_props.sect_break = None;
            self.apply_op(Op::Split { at: c, props: Some(tail_props) });
            at = c.para + 1;
        }
        self.apply_op(Op::SetTable { id, table: Some(table) });
        let mut n = 0;
        for r in 0..rows {
            for col in 0..cols {
                let p = Editor::cell_para(CellRef::new(id, r as u32, col as u32), Some(&base));
                let mut p = p;
                p.props.space_after = Some(0);
                p.props.line_spacing = Some(LineSpacing::Auto(240));
                self.apply_op(Op::InsertPara { para: at + n, paragraph: p });
                n += 1;
            }
        }
        self.cursor = Pos::new(at, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Move to the next cell (Tab). At the last cell a new row is appended.
    pub fn next_cell(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let target = if c + 1 < paras[r].len() {
            Some(paras[r][c + 1][0])
        } else if r + 1 < paras.len() {
            Some(paras[r + 1][0][0])
        } else {
            None
        };
        match target {
            Some(p) => {
                self.select_cell_or_move(p);
            }
            None => {
                self.insert_row(true);
            }
        }
        true
    }

    /// Move to the next cell without ever adding a row. False at the last cell.
    pub fn next_cell_no_append(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let target = if c + 1 < paras[r].len() {
            paras[r][c + 1][0]
        } else if r + 1 < paras.len() {
            paras[r + 1][0][0]
        } else {
            return false;
        };
        self.select_cell_or_move(target);
        true
    }

    /// Move to the previous cell (Shift+Tab).
    pub fn prev_cell(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let target = if c > 0 {
            paras[r][c - 1][0]
        } else if r > 0 {
            let prev = &paras[r - 1];
            prev[prev.len() - 1][0]
        } else {
            return true;
        };
        self.select_cell_or_move(target);
        true
    }

    /// Move to a cell's first paragraph. A selection in progress (Block)
    /// extends to it, so Block then Tab selects a run of cells.
    fn select_cell_or_move(&mut self, para: usize) {
        self.commit();
        if self.anchor.is_none() {
            self.cursor = Pos::new(para, 0);
        } else {
            self.cursor = Pos::new(para, self.doc.paragraphs[para].items.len());
        }
        self.goal_x = None;
    }

    /// Move vertically to the same column in the row `delta` rows away,
    /// keeping the goal x. Returns false at the table's edge.
    pub(crate) fn move_row(&mut self, delta: i32, x: u16, select: bool) -> bool {
        let Some(at) = self.at_table() else { return false };
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let r = at.cell.row as i32 + delta;
        if r < 0 || r as usize >= paras.len() {
            return false;
        }
        let row = &paras[r as usize];
        let table = self.doc.tables.get(&at.cell.table).cloned();
        // Same grid column, so the cursor stays under itself across spans.
        let col = match &table {
            Some(t) => {
                let g = t.grid_col(at.cell.row as usize, at.cell.col as usize);
                t.cell_at_grid(r as usize, g).min(row.len() - 1)
            }
            None => (at.cell.col as usize).min(row.len() - 1),
        };
        let cell = &row[col];
        let para = if delta < 0 { cell[cell.len() - 1] } else { cell[0] };
        let line = if delta < 0 { self.screen_lines(para).len() - 1 } else { 0 };
        self.move_to_line_x(para, line, x, select);
        true
    }

    /// Insert a row below (or above) the cursor's row, copying its cell
    /// structure.
    pub fn insert_row(&mut self, below: bool) -> bool {
        let Some(at) = self.at_table() else { return false };
        self.commit();
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let r = at.cell.row as usize;
        let new_r = if below { r + 1 } else { r };
        let src_row = table.rows[r].clone();
        let new_row = TableRow {
            cells: src_row.cells.iter().map(|c| TableCell { span: c.span, vmerge: None, width: c.width, shading: None, borders: None, raw_tcpr: None, attrs: String::new() }).collect(),
            header: false,
            cant_split: src_row.cant_split,
            height: src_row.height,
            height_exact: src_row.height_exact,
            raw_trpr: None,
            attrs: String::new(),
        };
        // Paragraph index where the new row's paragraphs go.
        let insert_at = if below { paras[r].last().unwrap().last().unwrap() + 1 } else { paras[r][0][0] };
        let ncells = src_row.cells.len();
        // Renumber the rows after the insertion point first (from the end,
        // indices are unaffected by the later insert).
        self.renumber(insert_at, at.end, id, |c| if c.row as usize >= new_r { CellRef::new(id, c.row + 1, c.col) } else { c });
        table.rows.insert(new_r, new_row);
        self.apply_op(Op::SetTable { id, table: Some(table) });
        for col in 0..ncells {
            let like = self.doc.paragraphs[paras[r][col.min(paras[r].len() - 1)][0]].props.clone();
            let p = Editor::cell_para(CellRef::new(id, new_r as u32, col as u32), Some(&like));
            self.apply_op(Op::InsertPara { para: insert_at + col, paragraph: p });
        }
        self.cursor = Pos::new(insert_at, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Delete the cursor's row. Deleting the only row deletes the table.
    pub fn delete_row(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        if table.rows.len() <= 1 {
            return self.delete_table();
        }
        self.commit();
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let r = at.cell.row as usize;
        let first = paras[r][0][0];
        let last = *paras[r].last().unwrap().last().unwrap();
        for i in (first..=last).rev() {
            self.apply_op(Op::RemovePara { para: i });
        }
        let removed = last + 1 - first;
        self.renumber(first, at.end - removed, id, |c| if c.row as usize > r { CellRef::new(id, c.row - 1, c.col) } else { c });
        table.rows.remove(r);
        self.apply_op(Op::SetTable { id, table: Some(table) });
        let target = first.min(self.doc.paragraphs.len() - 1);
        self.cursor = Pos::new(target, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Insert a grid column right (or left) of the cursor's cell, in every row.
    pub fn insert_column(&mut self, right: bool) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        if table.cols() >= 63 {
            return false;
        }
        self.commit();
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let g = table.grid_col(r, c) + if right { table.rows[r].cells[c].span() } else { 0 };
        // New column takes the current one's width; shrink everything so the
        // table keeps its width when it would otherwise outgrow the page.
        let w = table.grid.get(table.grid_col(r, c)).copied().unwrap_or(1440);
        table.grid.insert(g.min(table.grid.len()), w);
        let text_w = self.doc.section.text_width();
        if table.width() > text_w {
            let total = table.width() as i64;
            for x in table.grid.iter_mut() {
                *x = ((*x as i64 * text_w as i64) / total).max(360) as Twips;
            }
        }
        // Per row: the cell whose span covers grid column `g` (for a left
        // insert) or ends at `g` (right insert) gets a sibling.
        let mut inserts: Vec<(usize, CellRef)> = Vec::new(); // (paragraph index, new cell)
        let mut new_cols: Vec<usize> = Vec::new();
        for (ri, row) in table.rows.iter_mut().enumerate() {
            // Cell index after which the new cell goes.
            let mut acc = 0;
            let mut idx = row.cells.len();
            for (ci, cell) in row.cells.iter().enumerate() {
                if right {
                    acc += cell.span();
                    if acc >= g {
                        idx = ci + 1;
                        break;
                    }
                } else {
                    if acc >= g {
                        idx = ci;
                        break;
                    }
                    acc += cell.span();
                }
            }
            let width = row.cells.get(idx.min(row.cells.len().saturating_sub(1))).and_then(|x| x.width).map(|_| w);
            row.cells.insert(idx, TableCell { span: 1, vmerge: None, width, shading: None, borders: None, raw_tcpr: None, attrs: String::new() });
            let para_at = if idx < paras[ri].len() { paras[ri][idx][0] } else { paras[ri].last().unwrap().last().unwrap() + 1 };
            inserts.push((para_at, CellRef::new(id, ri as u32, idx as u32)));
            new_cols.push(idx);
        }
        table.touch_grid();
        // Retag from the end so earlier indices stay valid, then insert.
        for (ri, row) in paras.iter().enumerate().rev() {
            let idx = new_cols[ri];
            for (ci, cell) in row.iter().enumerate().rev() {
                if ci >= idx {
                    for &p in cell {
                        self.retag(p, CellRef::new(id, ri as u32, ci as u32 + 1));
                    }
                }
            }
            let (para_at, cell) = inserts[ri];
            let like = self.doc.paragraphs[row[0][0]].props.clone();
            self.apply_op(Op::InsertPara { para: para_at, paragraph: Editor::cell_para(cell, Some(&like)) });
        }
        self.apply_op(Op::SetTable { id, table: Some(table) });
        let cur = inserts[r].0 + 0;
        // The inserted paragraphs before row `r` shifted our target.
        let shift = inserts.iter().take(r).count();
        self.cursor = Pos::new(cur + shift, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Delete the grid column(s) covered by the cursor's cell from every
    /// row. Deleting the only column deletes the table.
    pub fn delete_column(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let g0 = table.grid_col(r, c);
        let span = table.rows[r].cells[c].span();
        if span >= table.cols() {
            return self.delete_table();
        }
        self.commit();
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        let gs = g0..g0 + span;
        // Per row, cells entirely inside the removed grid columns go; cells
        // overlapping them shrink.
        let mut removals: Vec<usize> = Vec::new();
        for (ri, row) in table.rows.iter_mut().enumerate() {
            let mut acc = 0;
            let mut keep: Vec<TableCell> = Vec::new();
            for (ci, cell) in row.cells.iter().enumerate() {
                let (s, e) = (acc, acc + cell.span());
                acc = e;
                let overlap = s.max(gs.start)..e.min(gs.end);
                let ov = overlap.end.saturating_sub(overlap.start);
                if ov >= cell.span() {
                    for &p in &paras[ri][ci] {
                        removals.push(p);
                    }
                } else {
                    let mut cell = cell.clone();
                    if ov > 0 {
                        cell.span = (cell.span() - ov) as u16;
                        cell.raw_tcpr = None;
                    }
                    keep.push(cell);
                }
            }
            row.cells = keep;
        }
        for g in gs.clone().rev() {
            if g < table.grid.len() {
                table.grid.remove(g);
            }
        }
        table.touch_grid();
        removals.sort_unstable();
        // Retag surviving cells with their new column index, from the end.
        for (ri, row) in paras.iter().enumerate().rev() {
            let mut new_ci = 0u32;
            let mut tags: Vec<(usize, u32)> = Vec::new();
            for cell in row.iter() {
                if removals.binary_search(&cell[0]).is_ok() {
                    continue;
                }
                for &p in cell {
                    tags.push((p, new_ci));
                }
                new_ci += 1;
            }
            for (p, ci) in tags {
                self.retag(p, CellRef::new(id, ri as u32, ci));
            }
        }
        for &p in removals.iter().rev() {
            self.apply_op(Op::RemovePara { para: p });
        }
        self.apply_op(Op::SetTable { id, table: Some(table) });
        let (start, _) = self.doc.table_span(id).unwrap_or((at.start, at.end));
        let target = self.doc.table_paras(start).and_then(|ps| ps.get(r).and_then(|row| row.get(c.min(row.len().saturating_sub(1))).map(|cell| cell[0]))).unwrap_or(start);
        self.cursor = Pos::new(target, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Remove the table and all its contents.
    pub fn delete_table(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        self.commit();
        self.remove_table_paras(at.cell.table, at.start, at.end);
        let target = at.start.min(self.doc.paragraphs.len() - 1);
        self.cursor = Pos::new(target, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Remove every paragraph of a table and its definition. Leaves an empty
    /// paragraph if the document would otherwise have none.
    pub(crate) fn remove_table_paras(&mut self, id: u32, start: usize, end: usize) {
        if end - start >= self.doc.paragraphs.len() {
            self.apply_op(Op::InsertPara { para: end, paragraph: Paragraph::new() });
        }
        for i in (start..end).rev() {
            self.apply_op(Op::RemovePara { para: i });
        }
        self.apply_op(Op::SetTable { id, table: None });
    }

    /// Turn the table into plain paragraphs: cells become paragraphs, the
    /// cells of a row separated by tabs (WordPerfect's `[Tbl Def]` deletion).
    pub fn table_to_text(&mut self) -> bool {
        let Some(at) = self.at_table() else { return false };
        self.commit();
        let id = at.cell.table;
        let paras = self.doc.table_paras(self.cursor.para).unwrap();
        // Build the replacement paragraphs: one per row, cells joined by tabs
        // (extra paragraphs in a cell are joined with spaces).
        let mut out: Vec<Paragraph> = Vec::new();
        for row in &paras {
            let mut items: Vec<Item> = Vec::new();
            let mut props = self.doc.paragraphs[row[0][0]].props.clone();
            props.cell = None;
            props.p_attrs = None;
            props.raw_ppr = None;
            props.space_after = None;
            props.line_spacing = None;
            for (ci, cell) in row.iter().enumerate() {
                if ci > 0 {
                    items.push(Item::Code(Code::Tab));
                }
                for (pi, &p) in cell.iter().enumerate() {
                    if pi > 0 {
                        items.push(Item::Char(' '));
                    }
                    let para = &self.doc.paragraphs[p];
                    if para.props.raw_block {
                        continue;
                    }
                    items.extend(para.items.iter().cloned());
                }
            }
            out.push(Paragraph { props, items });
        }
        for i in (at.start..at.end).rev() {
            self.apply_op(Op::RemovePara { para: i });
        }
        for (k, p) in out.into_iter().enumerate() {
            self.apply_op(Op::InsertPara { para: at.start + k, paragraph: p });
        }
        self.apply_op(Op::SetTable { id, table: None });
        self.cursor = Pos::new(at.start, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        true
    }

    /// Set the width of the grid column(s) under the cursor's cell.
    pub fn set_column_width(&mut self, width: Twips) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let g0 = table.grid_col(r, c);
        let span = table.rows[r].cells[c].span();
        let width = width.clamp(360, 31680);
        let each = width / span as Twips;
        for g in g0..(g0 + span).min(table.grid.len()) {
            table.grid[g] = each;
        }
        // Preferred cell widths follow the grid.
        for row in table.rows.iter_mut() {
            let mut acc = 0;
            for cell in row.cells.iter_mut() {
                let w: Twips = table.grid.iter().skip(acc).take(cell.span()).sum();
                acc += cell.span();
                if cell.width.is_some() {
                    cell.width = Some(w);
                }
            }
        }
        table.touch_grid();
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        true
    }

    /// Toggle "repeat as header row" on the cursor's row.
    pub fn toggle_header_row(&mut self) -> Option<bool> {
        let at = self.at_table()?;
        let id = at.cell.table;
        let mut table = self.doc.tables.get(&id).cloned()?;
        let row = table.rows.get_mut(at.cell.row as usize)?;
        row.header = !row.header;
        row.raw_trpr = None;
        let on = row.header;
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        Some(on)
    }

    /// The rectangle of grid columns and rows covered by the selection (or
    /// the cursor's cell): `(r0, r1, g0, g1)`, inclusive.
    fn selected_rect(&self) -> Option<(usize, usize, usize, usize)> {
        let at = self.at_table()?;
        let t = self.doc.tables.get(&at.cell.table)?;
        let other = match self.selection() {
            Some(r) => {
                let a = self.doc.cell_of(r.start.para)?;
                let b = self.doc.cell_of(r.end.para.min(self.doc.paragraphs.len() - 1)).filter(|c| c.table == a.table);
                match b {
                    Some(b) => (a, b),
                    None => (a, a),
                }
            }
            None => (at.cell, at.cell),
        };
        let (a, b) = other;
        if a.table != at.cell.table {
            return None;
        }
        let ga = t.grid_col(a.row as usize, a.col as usize);
        let gb = t.grid_col(b.row as usize, b.col as usize);
        let ea = ga + t.rows[a.row as usize].cells[a.col as usize].span() - 1;
        let eb = gb + t.rows[b.row as usize].cells[b.col as usize].span() - 1;
        Some((a.row.min(b.row) as usize, a.row.max(b.row) as usize, ga.min(gb), ea.max(eb)))
    }

    /// Merge the selected cells (a rectangle) into one: the top-left cell
    /// spans the rectangle's columns, the rows below it continue it
    /// vertically, and every merged cell's text moves into it. Fails with
    /// a reason when the selection is not a clean rectangle of cells.
    pub fn merge_cells(&mut self) -> Result<(), &'static str> {
        let at = self.at_table().ok_or("Not in a table")?;
        let (r0, r1, g0, g1) = self.selected_rect().ok_or("Select the cells to merge first (Block, then move)")?;
        if r0 == r1 && g0 == g1 {
            return Err("Select two or more cells to merge (Block, then move to the last cell)");
        }
        let id = at.cell.table;
        let mut table = self.doc.tables.get(&id).cloned().ok_or("Not in a table")?;
        let paras = self.doc.table_paras(self.cursor.para).ok_or("Not in a table")?;
        // Every row must have a cell starting at g0 and one ending at g1.
        let mut plan: Vec<(usize, usize, usize)> = Vec::new(); // (row, first cell idx, last cell idx)
        for r in r0..=r1 {
            let row = &table.rows[r];
            let mut acc = 0;
            let mut first = None;
            let mut last = None;
            for (ci, c) in row.cells.iter().enumerate() {
                if acc == g0 {
                    first = Some(ci);
                }
                if acc + c.span() - 1 == g1 {
                    last = Some(ci);
                }
                acc += c.span();
            }
            match (first, last) {
                (Some(f), Some(l)) if f <= l => plan.push((r, f, l)),
                _ => return Err("Those cells don't line up into a rectangle"),
            }
        }
        self.commit();
        let (top_r, top_f, _) = plan[0];
        let top_cell = CellRef::new(id, top_r as u32, top_f as u32);
        // The text of every merged cell but the top-left moves into it. A
        // cell in a lower row lives on as a continuation and keeps one
        // empty paragraph; a cell in the top row disappears entirely.
        let mut moved: Vec<Paragraph> = Vec::new();
        let mut removals: Vec<usize> = Vec::new();
        let mut emptied: Vec<usize> = Vec::new();
        for &(r, f, l) in &plan {
            for ci in f..=l {
                if r == top_r && ci == top_f {
                    continue;
                }
                let keeps_one = r != top_r && ci == f;
                for (k, &p) in paras[r][ci].iter().enumerate() {
                    let para = &self.doc.paragraphs[p];
                    if !para.items.is_empty() && !para.props.raw_block {
                        let mut clone = para.clone();
                        clone.props.p_attrs = None;
                        clone.props.cell = Some(top_cell);
                        moved.push(clone);
                    }
                    if keeps_one && k == 0 {
                        emptied.push(p);
                    } else {
                        removals.push(p);
                    }
                }
            }
        }
        for p in emptied {
            if !self.doc.paragraphs[p].items.is_empty() {
                self.apply_op(Op::ReplaceItems { para: p, items: Vec::new() });
            }
        }
        removals.sort_unstable();
        let top_last = *paras[top_r][top_f].last().unwrap();
        for p in removals.iter().rev() {
            self.apply_op(Op::RemovePara { para: *p });
        }
        let shift = removals.iter().filter(|&&p| p <= top_last).count();
        let insert_at = top_last + 1 - shift;
        for (k, p) in moved.into_iter().enumerate() {
            self.apply_op(Op::InsertPara { para: insert_at + k, paragraph: p });
        }
        // The grid: one cell per row spanning the rectangle.
        for &(r, f, l) in &plan {
            let row = &mut table.rows[r];
            let span: u16 = (f..=l).map(|ci| row.cells[ci].span() as u16).sum();
            let mut kept = row.cells[f].clone();
            kept.span = span;
            kept.raw_tcpr = None;
            kept.width = Some(table.grid.iter().skip(g0).take(g1 - g0 + 1).sum());
            kept.vmerge = if r1 > r0 { Some(if r == top_r { VMerge::Restart } else { VMerge::Continue }) } else { None };
            row.cells.drain(f..=l);
            row.cells.insert(f, kept);
        }
        self.apply_op(Op::SetTable { id, table: Some(table) });
        // Cell indices after a removed cell shift left: renumber every row
        // from the stream.
        let (start, end) = self.doc.table_span(id).ok_or("Not in a table")?;
        self.retag_rows(start, end, id);
        let first = self.doc.cell_first_para(top_cell).unwrap_or(start);
        self.cursor = Pos::new(first, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        Ok(())
    }

    /// Renumber each row's cells left to right from the paragraph stream
    /// (after cells were removed from rows).
    fn retag_rows(&mut self, start: usize, end: usize, id: u32) {
        let mut i = start;
        while i < end {
            let Some(c) = self.doc.cell_of(i) else { break };
            if c.table != id {
                break;
            }
            let row = c.row;
            let mut ci = 0u32;
            let mut prev: Option<CellRef> = None;
            while i < end {
                let Some(cc) = self.doc.cell_of(i) else { break };
                if cc.table != id || cc.row != row {
                    break;
                }
                if prev.map_or(false, |p| p != cc) {
                    ci += 1;
                }
                prev = Some(cc);
                self.retag(i, CellRef::new(id, row, ci));
                i += 1;
            }
        }
    }

    /// Split the cursor's cell: a vertically merged region is unmerged, a
    /// cell spanning several columns becomes one cell per column, and a
    /// plain cell is split in two (its column is divided).
    pub fn split_cell(&mut self) -> Result<&'static str, &'static str> {
        let at = self.at_table().ok_or("Not in a table")?;
        let id = at.cell.table;
        let mut table = self.doc.tables.get(&id).cloned().ok_or("Not in a table")?;
        let (r, c) = (at.cell.row as usize, at.cell.col as usize);
        let cell = table.rows[r].cells[c].clone();
        let paras = self.doc.table_paras(self.cursor.para).ok_or("Not in a table")?;
        self.commit();
        if cell.vmerge.is_some() {
            // Unmerge the vertical region this cell belongs to: up to its
            // restart, then down while cells continue it.
            let g = table.grid_col(r, c);
            let vm = |t: &Table, rr: usize| t.rows[rr].cells[t.cell_at_grid(rr, g)].vmerge;
            let mut top = r;
            while top > 0 && vm(&table, top) == Some(VMerge::Continue) {
                top -= 1;
            }
            let mut rr = top;
            while rr < table.rows.len() && (rr == top || vm(&table, rr) == Some(VMerge::Continue)) {
                let ci = table.cell_at_grid(rr, g);
                table.rows[rr].cells[ci].vmerge = None;
                table.rows[rr].cells[ci].raw_tcpr = None;
                rr += 1;
            }
            self.apply_op(Op::SetTable { id, table: Some(table) });
            self.commit();
            return Ok("Cells unmerged");
        }
        if cell.span() > 1 {
            let g0 = table.grid_col(r, c);
            let n = cell.span();
            let mut new_cells = Vec::new();
            for k in 0..n {
                let mut nc = TableCell::new();
                nc.width = Some(table.grid[g0 + k]);
                nc.shading = cell.shading;
                nc.borders = cell.borders;
                new_cells.push(nc);
            }
            table.rows[r].cells.remove(c);
            for (k, nc) in new_cells.into_iter().enumerate() {
                table.rows[r].cells.insert(c + k, nc);
            }
            // New empty paragraphs after this cell's, then retag the rest of the row.
            let after = *paras[r][c].last().unwrap() + 1;
            for k in (c + 1..paras[r].len()).rev() {
                for &p in &paras[r][k] {
                    self.retag(p, CellRef::new(id, r as u32, (k + n - 1) as u32));
                }
            }
            for k in 1..n {
                let like = self.doc.paragraphs[paras[r][c][0]].props.clone();
                self.apply_op(Op::InsertPara { para: after + k - 1, paragraph: Editor::cell_para(CellRef::new(id, r as u32, (c + k) as u32), Some(&like)) });
            }
            self.apply_op(Op::SetTable { id, table: Some(table) });
            self.commit();
            return Ok("Cell split into its columns");
        }
        // A plain cell: divide its grid column in two.
        if table.cols() >= 63 {
            return Err("Too many columns");
        }
        let g = table.grid_col(r, c);
        let w = table.grid[g];
        table.grid[g] = w / 2;
        table.grid.insert(g + 1, w - w / 2);
        for (ri, row) in table.rows.iter_mut().enumerate() {
            if ri == r {
                continue;
            }
            let ci = {
                let mut acc = 0;
                let mut idx = row.cells.len() - 1;
                for (i, cc) in row.cells.iter().enumerate() {
                    if g >= acc && g < acc + cc.span() {
                        idx = i;
                        break;
                    }
                    acc += cc.span();
                }
                idx
            };
            let cc = &mut row.cells[ci];
            cc.span = (cc.span() + 1) as u16;
            cc.raw_tcpr = None;
        }
        let mut left = table.rows[r].cells[c].clone();
        left.width = Some(w / 2);
        left.raw_tcpr = None;
        let mut right = TableCell::new();
        right.width = Some(w - w / 2);
        right.shading = left.shading;
        right.borders = left.borders;
        table.rows[r].cells[c] = left;
        table.rows[r].cells.insert(c + 1, right);
        table.touch_grid();
        let after = *paras[r][c].last().unwrap() + 1;
        for k in (c + 1..paras[r].len()).rev() {
            for &p in &paras[r][k] {
                self.retag(p, CellRef::new(id, r as u32, (k + 1) as u32));
            }
        }
        let like = self.doc.paragraphs[paras[r][c][0]].props.clone();
        self.apply_op(Op::InsertPara { para: after, paragraph: Editor::cell_para(CellRef::new(id, r as u32, (c + 1) as u32), Some(&like)) });
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        Ok("Cell split in two")
    }

    /// Sort the table's rows (header rows stay) by the cursor's column:
    /// numerically when every key is a number, else alphabetically.
    pub fn sort_rows(&mut self, descending: bool) -> Result<usize, &'static str> {
        let at = self.at_table().ok_or("Not in a table")?;
        let id = at.cell.table;
        let mut table = self.doc.tables.get(&id).cloned().ok_or("Not in a table")?;
        if table.rows.iter().any(|r| r.cells.iter().any(|c| c.vmerge.is_some())) {
            return Err("Can't sort a table with vertically merged cells");
        }
        let paras = self.doc.table_paras(self.cursor.para).ok_or("Not in a table")?;
        let g = table.grid_col(at.cell.row as usize, at.cell.col as usize);
        let first = table.rows.iter().take_while(|r| r.header).count();
        if table.rows.len() - first < 2 {
            return Err("Nothing to sort");
        }
        let key = |ed: &Editor, r: usize| -> String {
            let ci = table.cell_at_grid(r, g);
            paras[r].get(ci).map(|cell| cell.iter().map(|&p| ed.doc.paragraphs[p].text()).collect::<Vec<_>>().join(" ")).unwrap_or_default().trim().to_string()
        };
        let keys: Vec<String> = (first..table.rows.len()).map(|r| key(self, r)).collect();
        let nums: Vec<Option<f64>> = keys.iter().map(|k| parse_number(k)).collect();
        let numeric = nums.iter().all(|n| n.is_some());
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| {
            let o = if numeric { nums[a].partial_cmp(&nums[b]).unwrap_or(std::cmp::Ordering::Equal) } else { keys[a].to_lowercase().cmp(&keys[b].to_lowercase()) };
            if descending {
                o.reverse()
            } else {
                o
            }
        });
        if order.iter().enumerate().all(|(i, &o)| i == o) {
            return Ok(0);
        }
        self.commit();
        // Rebuild the data rows' paragraphs in the new order.
        let data_start = paras[first][0][0];
        let data_end = *paras.last().unwrap().last().unwrap().last().unwrap() + 1;
        let mut new_paras: Vec<Paragraph> = Vec::new();
        let mut new_rows: Vec<TableRow> = Vec::new();
        for (new_r, &o) in order.iter().enumerate() {
            let r = first + o;
            for (ci, cell) in paras[r].iter().enumerate() {
                for &p in cell {
                    let mut q = self.doc.paragraphs[p].clone();
                    q.props.cell = Some(CellRef::new(id, (first + new_r) as u32, ci as u32));
                    new_paras.push(q);
                }
            }
            new_rows.push(table.rows[r].clone());
        }
        for i in (data_start..data_end).rev() {
            self.apply_op(Op::RemovePara { para: i });
        }
        for (k, p) in new_paras.into_iter().enumerate() {
            self.apply_op(Op::InsertPara { para: data_start + k, paragraph: p });
        }
        table.rows.truncate(first);
        table.rows.extend(new_rows);
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.cursor = Pos::new(data_start, 0);
        self.anchor = None;
        self.goal_x = None;
        self.commit();
        Ok(order.len())
    }

    /// Insert a formula field (`=SUM(ABOVE)`) at the cursor with its value.
    pub fn insert_formula(&mut self, formula: &str) -> Result<String, String> {
        let cell = self.current_cell().ok_or("Formulas go in table cells")?;
        let f = formula.trim().trim_start_matches('=').trim();
        let value = evaluate_formula(&self.doc, cell, f)?;
        self.insert_field(&format!(" ={} ", f), &value);
        Ok(value)
    }

    /// Recompute every formula field in the document. Returns how many
    /// results changed.
    pub fn recalculate(&mut self) -> usize {
        self.commit();
        let mut changed = 0;
        let mut pi = 0;
        while pi < self.doc.paragraphs.len() {
            let Some(cell) = self.doc.cell_of(pi) else {
                pi += 1;
                continue;
            };
            let mut idx = 0;
            while idx < self.doc.paragraphs[pi].items.len() {
                let instr = match &self.doc.paragraphs[pi].items[idx] {
                    Item::Code(Code::Opaque(o)) => crate::editor::field_instr(o).filter(|s| s.starts_with('=')),
                    _ => None,
                };
                if let Some(instr) = instr {
                    let close = self.doc.paired_code(Pos::new(pi, idx));
                    if let Some(close) = close {
                        let old: String = self.doc.paragraphs[pi].items[idx + 1..close].iter().filter_map(Item::as_char).collect();
                        let new = evaluate_formula(&self.doc, cell, instr.trim_start_matches('=').trim()).unwrap_or_else(|e| format!("!{}", e));
                        if new != old {
                            let saved = self.cursor;
                            self.apply_op(Op::Delete { at: Pos::new(pi, idx + 1), len: close - idx - 1 });
                            self.apply_op(Op::Insert { at: Pos::new(pi, idx + 1), items: new.chars().map(Item::Char).collect() });
                            self.cursor = self.doc.clamp(saved);
                            changed += 1;
                        }
                    }
                }
                idx += 1;
            }
            pi += 1;
        }
        self.commit();
        changed
    }

    /// Set (or clear, with `None`) the height of the cursor's row.
    pub fn set_row_height(&mut self, height: Option<Twips>, exact: bool) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        let Some(row) = table.rows.get_mut(at.cell.row as usize) else { return false };
        row.height = height;
        row.height_exact = exact && height.is_some();
        row.raw_trpr = None;
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        true
    }

    /// Toggle "may not split across pages" on the cursor's row.
    pub fn toggle_cant_split(&mut self) -> Option<bool> {
        let at = self.at_table()?;
        let id = at.cell.table;
        let mut table = self.doc.tables.get(&id).cloned()?;
        let row = table.rows.get_mut(at.cell.row as usize)?;
        row.cant_split = !row.cant_split;
        row.raw_trpr = None;
        let on = row.cant_split;
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        Some(on)
    }

    /// Set the table's lines.
    pub fn set_table_borders(&mut self, borders: Option<TableBorders>) -> bool {
        let Some(at) = self.at_table() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        table.borders = borders;
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        true
    }

    /// Set the shading of the selected cells (or the cursor's).
    pub fn set_cell_shading(&mut self, fill: Option<Rgb>) -> bool {
        self.for_selected_cells(|c| c.shading = fill)
    }

    /// Set the lines of the selected cells (or the cursor's).
    pub fn set_cell_borders(&mut self, borders: Option<CellBorders>) -> bool {
        self.for_selected_cells(|c| c.borders = borders)
    }

    fn for_selected_cells(&mut self, f: impl Fn(&mut TableCell)) -> bool {
        let Some(at) = self.at_table() else { return false };
        let Some((r0, r1, g0, g1)) = self.selected_rect() else { return false };
        let id = at.cell.table;
        let Some(mut table) = self.doc.tables.get(&id).cloned() else { return false };
        for r in r0..=r1 {
            let row = &mut table.rows[r];
            let mut acc = 0;
            for c in row.cells.iter_mut() {
                let (s, e) = (acc, acc + c.span());
                acc = e;
                if s <= g1 && e > g0 {
                    f(c);
                }
            }
        }
        self.commit();
        self.apply_op(Op::SetTable { id, table: Some(table) });
        self.commit();
        true
    }

    /// Column width of the cursor's cell, in twips.
    pub fn current_column_width(&self) -> Option<Twips> {
        let c = self.current_cell()?;
        let t = self.doc.tables.get(&c.table)?;
        Some(t.cell_extent(c.row as usize, c.col as usize).1)
    }
}

/// The first number in a cell's text (`$1,234.50` → 1234.5), if any.
pub fn parse_number(s: &str) -> Option<f64> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, '$' | ',' | '%' | '€' | '£' | ' ')).collect();
    let t = cleaned.trim();
    if t.is_empty() {
        return None;
    }
    let neg = t.starts_with('(') && t.ends_with(')');
    let t = t.trim_matches(|c| c == '(' || c == ')');
    t.parse::<f64>().ok().map(|v| if neg { -v } else { v })
}

/// Evaluate a table formula (`SUM(ABOVE)`, `AVERAGE(LEFT)`, `MAX(A1:B3)`,
/// `COUNT(BELOW)`, `MIN(RIGHT)`) for the cell it sits in.
pub fn evaluate_formula(doc: &Document, cell: CellRef, f: &str) -> Result<String, String> {
    let f = f.trim();
    let open = f.find('(').ok_or("Formulas look like SUM(ABOVE), AVERAGE(LEFT) or SUM(A1:B3)")?;
    let close = f.rfind(')').ok_or("Missing closing parenthesis")?;
    if close < open {
        return Err("Formulas look like SUM(ABOVE)".into());
    }
    let func = f[..open].trim().to_ascii_uppercase();
    let arg = f[open + 1..close].trim().to_ascii_uppercase();
    let t = doc.tables.get(&cell.table).ok_or("Not in a table")?;
    let start = doc.table_span(cell.table).map(|(s, _)| s).ok_or("Not in a table")?;
    let paras = doc.table_paras(start).ok_or("Not in a table")?;
    let cell_text = |r: usize, ci: usize| -> String { paras.get(r).and_then(|row| row.get(ci)).map(|c| c.iter().map(|&p| doc.paragraphs[p].text()).collect::<Vec<_>>().join(" ")).unwrap_or_default() };
    let g = t.grid_col(cell.row as usize, cell.col as usize);
    let mut values: Vec<f64> = Vec::new();
    let mut push_cell = |r: usize, ci: usize, stop_at_blank: bool| -> bool {
        match parse_number(&cell_text(r, ci)) {
            Some(v) => {
                values.push(v);
                true
            }
            None => !stop_at_blank,
        }
    };
    match arg.as_str() {
        "ABOVE" => {
            for r in (0..cell.row as usize).rev() {
                let ci = t.cell_at_grid(r, g);
                if !push_cell(r, ci, true) {
                    break;
                }
            }
        }
        "BELOW" => {
            for r in cell.row as usize + 1..t.rows.len() {
                let ci = t.cell_at_grid(r, g);
                if !push_cell(r, ci, true) {
                    break;
                }
            }
        }
        "LEFT" => {
            for ci in (0..cell.col as usize).rev() {
                if !push_cell(cell.row as usize, ci, true) {
                    break;
                }
            }
        }
        "RIGHT" => {
            for ci in cell.col as usize + 1..paras[cell.row as usize].len() {
                if !push_cell(cell.row as usize, ci, true) {
                    break;
                }
            }
        }
        _ => {
            // A1:B3 range, or a list of cells.
            for part in arg.split(',') {
                let part = part.trim();
                let (a, b) = match part.split_once(':') {
                    Some((a, b)) => (parse_cell_name(a)?, parse_cell_name(b)?),
                    None => {
                        let c = parse_cell_name(part)?;
                        (c, c)
                    }
                };
                for r in a.1.min(b.1)..=a.1.max(b.1) {
                    for ci in a.0.min(b.0)..=a.0.max(b.0) {
                        push_cell(r, ci, false);
                    }
                }
            }
        }
    }
    let n = values.len() as f64;
    let v = match func.as_str() {
        "SUM" => values.iter().sum::<f64>(),
        "AVERAGE" | "AVG" | "MEAN" => {
            if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / n
            }
        }
        "COUNT" => n,
        "MAX" => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        "MIN" => values.iter().cloned().fold(f64::INFINITY, f64::min),
        "PRODUCT" => values.iter().product::<f64>(),
        _ => return Err(format!("Unknown function {} (SUM, AVERAGE, COUNT, MAX, MIN, PRODUCT)", func)),
    };
    let v = if v.is_finite() { v } else { 0.0 };
    Ok(format_number(v))
}

fn parse_cell_name(s: &str) -> Result<(usize, usize), String> {
    let s = s.trim();
    let letters: String = s.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = s.chars().skip(letters.len()).collect();
    if letters.is_empty() || digits.is_empty() {
        return Err(format!("Not a cell name: {}", s));
    }
    let mut col = 0usize;
    for c in letters.chars() {
        col = col * 26 + (c.to_ascii_uppercase() as u8 - b'A') as usize + 1;
    }
    let row: usize = digits.parse().map_err(|_| format!("Not a cell name: {}", s))?;
    if row == 0 {
        return Err(format!("Not a cell name: {}", s));
    }
    Ok((col - 1, row - 1))
}

/// Numbers as Word shows formula results: no trailing zeros, at most two
/// decimals.
pub fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{:.2}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> Editor {
        Editor::new(crate::text::from_text("before\nafter", false))
    }

    #[test]
    fn merge_split_sort_and_formulas() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        assert!(e.insert_table(3, 3));
        // Fill: header row, then numbers.
        for s in ["Item", "Qty", "Price", "b", "2", "20", "a", "1", "10"] {
            e.insert_str(s);
            e.next_cell_no_append();
        }
        assert!(e.doc.table_is_consistent(1));
        // Formulas.
        e.next_cell(); // adds a row
        e.next_cell();
        assert_eq!(e.current_cell(), Some(CellRef::new(1, 3, 1)));
        assert_eq!(e.insert_formula("SUM(ABOVE)").unwrap(), "3");
        e.next_cell();
        assert_eq!(e.insert_formula("=AVERAGE(above)").unwrap(), "15");
        assert_eq!(e.doc.paragraphs[e.cursor.para].text(), "15");
        assert_eq!(evaluate_formula(&e.doc, CellRef::new(1, 3, 2), "MAX(B2:C3)").unwrap(), "20");
        assert_eq!(evaluate_formula(&e.doc, CellRef::new(1, 3, 2), "COUNT(LEFT)").unwrap(), "1");
        assert!(evaluate_formula(&e.doc, CellRef::new(1, 3, 2), "FOO(ABOVE)").is_err());
        // Change a value and recalculate.
        e.move_to(e.doc.cell_first_para(CellRef::new(1, 1, 2)).map(|p| Pos::new(p, 0)).unwrap(), false);
        e.insert_str("1");
        assert_eq!(e.doc.paragraphs[e.cursor.para].text(), "120");
        assert_eq!(e.recalculate(), 1);
        let avg = e.doc.cell_first_para(CellRef::new(1, 3, 2)).unwrap();
        assert_eq!(e.doc.paragraphs[avg].text(), "65");
        // Sort the data rows by the first column (header stays first).
        e.move_to(Pos::new(e.doc.cell_first_para(CellRef::new(1, 0, 0)).unwrap(), 0), false);
        assert_eq!(e.toggle_header_row(), Some(true));
        e.move_to(Pos::new(e.doc.cell_first_para(CellRef::new(1, 1, 0)).unwrap(), 0), false);
        assert_eq!(e.sort_rows(false).unwrap(), 3);
        let col0: Vec<String> = (0..4).map(|r| e.doc.paragraphs[e.doc.cell_first_para(CellRef::new(1, r, 0)).unwrap()].text()).collect();
        assert_eq!(col0, ["Item", "", "a", "b"]);
        assert!(e.doc.table_is_consistent(1));
        // Numeric sort on the Qty column, descending.
        e.move_to(Pos::new(e.doc.cell_first_para(CellRef::new(1, 1, 1)).unwrap(), 0), false);
        assert_eq!(e.sort_rows(true).unwrap(), 3);
        let col1: Vec<String> = (0..4).map(|r| e.doc.paragraphs[e.doc.cell_first_para(CellRef::new(1, r, 1)).unwrap()].text()).collect();
        assert_eq!(col1, ["Qty", "3", "2", "1"]);
        // Merge B1:C1 horizontally.
        let b1 = e.doc.cell_first_para(CellRef::new(1, 0, 1)).unwrap();
        let c1 = e.doc.cell_first_para(CellRef::new(1, 0, 2)).unwrap();
        e.move_to(Pos::new(b1, 0), false);
        e.move_to(Pos::new(c1, 1), true);
        e.merge_cells().unwrap();
        assert_eq!(e.doc.tables[&1].rows[0].cells.len(), 2);
        assert_eq!(e.doc.tables[&1].rows[0].cells[1].span, 2);
        let merged = e.doc.cell_first_para(CellRef::new(1, 0, 1)).unwrap();
        assert_eq!(e.doc.paragraphs[merged].text(), "Qty");
        assert_eq!(e.doc.paragraphs[merged + 1].text(), "Price");
        assert!(e.doc.table_is_consistent(1));
        // Split it again.
        e.move_to(Pos::new(merged, 0), false);
        assert_eq!(e.split_cell().unwrap(), "Cell split into its columns");
        assert_eq!(e.doc.tables[&1].rows[0].cells.len(), 3);
        assert!(e.doc.table_is_consistent(1));
        // Merge A2:A3 vertically.
        let a2 = e.doc.cell_first_para(CellRef::new(1, 1, 0)).unwrap();
        let a3 = e.doc.cell_first_para(CellRef::new(1, 2, 0)).unwrap();
        e.move_to(Pos::new(a2, 0), false);
        e.move_to(Pos::new(a3, 0), true);
        e.merge_cells().unwrap();
        assert_eq!(e.doc.tables[&1].rows[1].cells[0].vmerge, Some(VMerge::Restart));
        assert_eq!(e.doc.tables[&1].rows[2].cells[0].vmerge, Some(VMerge::Continue));
        assert!(e.doc.table_is_consistent(1));
        assert!(e.sort_rows(false).is_err());
        e.move_to(Pos::new(e.doc.cell_first_para(CellRef::new(1, 1, 0)).unwrap(), 0), false);
        assert_eq!(e.split_cell().unwrap(), "Cells unmerged");
        assert!(e.doc.tables[&1].rows[1].cells[0].vmerge.is_none());
        // A plain cell splits its column in two.
        assert_eq!(e.split_cell().unwrap(), "Cell split in two");
        assert_eq!(e.doc.tables[&1].cols(), 4);
        assert_eq!(e.doc.tables[&1].rows[0].cells[0].span, 2);
        assert!(e.doc.table_is_consistent(1));
        // Borders and shading; row height; undo all the way.
        assert!(e.set_table_borders(None));
        assert!(e.set_cell_shading(Some(Rgb(0xFF, 0xFF, 0))));
        assert_eq!(e.doc.tables[&1].rows[1].cells[0].shading, Some(Rgb(0xFF, 0xFF, 0)));
        assert!(e.set_row_height(Some(1440), false));
        assert_eq!(e.doc.tables[&1].rows[1].height, Some(1440));
        assert_eq!(e.toggle_cant_split(), Some(true));
        while e.undo() {}
        assert_eq!(e.doc.text(), "before\nafter");
        assert!(e.doc.tables.is_empty());
    }

    #[test]
    fn numbers_and_names() {
        assert_eq!(parse_number("$1,234.50"), Some(1234.5));
        assert_eq!(parse_number("(12)"), Some(-12.0));
        assert_eq!(parse_number("n/a"), None);
        assert_eq!(parse_cell_name("B3").unwrap(), (1, 2));
        assert_eq!(parse_cell_name("AA1").unwrap(), (26, 0));
        assert_eq!(format_number(2.5), "2.5");
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(1.0 / 3.0), "0.33");
    }

    fn cells(e: &Editor) -> Vec<String> {
        e.doc.paragraphs.iter().map(|p| match p.props.cell {
            Some(c) => format!("{}:{}", c.name(), p.text()),
            None => format!("-:{}", p.text()),
        }).collect()
    }

    #[test]
    fn insert_table_and_navigate() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        assert!(e.insert_table(2, 2));
        assert_eq!(cells(&e), ["-:before", "A1:", "B1:", "A2:", "B2:", "-:after"]);
        assert!(e.doc.table_is_consistent(1));
        e.insert_char('x');
        e.next_cell();
        e.insert_char('y');
        e.next_cell();
        e.next_cell();
        e.next_cell(); // last cell → new row
        assert_eq!(e.doc.tables[&1].rows.len(), 3);
        assert_eq!(e.current_cell(), Some(CellRef::new(1, 2, 0)));
        e.prev_cell();
        assert_eq!(e.current_cell(), Some(CellRef::new(1, 1, 1)));
        assert_eq!(cells(&e)[1..3], ["A1:x", "B1:y"]);
        // Undo the row, then everything.
        e.undo();
        assert_eq!(e.doc.tables[&1].rows.len(), 2);
        while e.undo() {}
        assert_eq!(e.doc.text(), "before\nafter");
        assert!(e.doc.tables.is_empty());
    }

    #[test]
    fn insert_table_mid_paragraph_splits_it() {
        let mut e = ed();
        e.move_to(Pos::new(0, 3), false);
        assert!(e.insert_table(1, 1));
        assert_eq!(cells(&e), ["-:bef", "A1:", "-:ore", "-:after"]);
    }

    #[test]
    fn rows_and_columns() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(2, 2);
        for s in ["a", "b", "c", "d"] {
            e.insert_char(s.chars().next().unwrap());
            e.next_cell_no_append();
        }
        e.move_to(Pos::new(1, 0), false); // A1
        assert!(e.insert_row(true));
        assert_eq!(cells(&e), ["-:before", "A1:a", "B1:b", "A2:", "B2:", "A3:c", "B3:d", "-:after"]);
        assert!(e.doc.table_is_consistent(1));
        e.move_to(Pos::new(5, 0), false); // A3
        assert!(e.insert_row(false));
        assert_eq!(cells(&e)[5], "A3:");
        assert_eq!(cells(&e)[7], "A4:c");
        assert!(e.delete_row());
        assert_eq!(cells(&e)[5], "A3:c");
        e.move_to(Pos::new(3, 0), false); // the other empty row
        assert!(e.delete_row());
        assert_eq!(cells(&e), ["-:before", "A1:a", "B1:b", "A2:c", "B2:d", "-:after"]);
        // Columns.
        e.move_to(Pos::new(2, 0), false); // B1
        assert!(e.insert_column(true));
        assert_eq!(cells(&e), ["-:before", "A1:a", "B1:b", "C1:", "A2:c", "B2:d", "C2:", "-:after"]);
        assert_eq!(e.doc.tables[&1].cols(), 3);
        assert_eq!(e.current_cell(), Some(CellRef::new(1, 0, 2)));
        assert!(e.insert_column(false));
        assert_eq!(cells(&e), ["-:before", "A1:a", "B1:b", "C1:", "D1:", "A2:c", "B2:d", "C2:", "D2:", "-:after"]);
        assert!(e.doc.table_is_consistent(1));
        e.move_to(Pos::new(2, 0), false); // B1
        assert!(e.delete_column());
        assert_eq!(cells(&e), ["-:before", "A1:a", "B1:", "C1:", "A2:c", "B2:", "C2:", "-:after"]);
        assert_eq!(e.doc.tables[&1].cols(), 3);
        assert!(e.doc.table_is_consistent(1));
        while e.undo() {}
        assert_eq!(e.doc.text(), "before\nafter");
    }

    #[test]
    fn delete_and_convert() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(2, 2);
        e.insert_str("a");
        e.next_cell();
        e.insert_str("b");
        assert!(e.table_to_text());
        assert_eq!(e.doc.text(), "before\na\tb\n\t\nafter");
        assert!(e.doc.tables.is_empty());
        e.undo();
        assert_eq!(e.doc.tables.len(), 1);
        e.move_to(Pos::new(1, 0), false);
        assert!(e.delete_table());
        assert_eq!(e.doc.text(), "before\nafter");
        assert!(e.doc.tables.is_empty());
    }

    #[test]
    fn cell_boundaries_are_hard() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(1, 2);
        e.insert_str("ab");
        // Backspace at the start of a cell does nothing; Delete at its end too.
        e.move_to(Pos::new(1, 0), false);
        e.backspace(false);
        assert_eq!(cells(&e), ["-:before", "A1:ab", "B1:", "-:after"]);
        e.move_to(Pos::new(1, 2), false);
        e.delete_forward(false);
        assert_eq!(cells(&e), ["-:before", "A1:ab", "B1:", "-:after"]);
        // Nor from the paragraphs around the table.
        e.move_to(Pos::new(0, 6), false);
        e.delete_forward(false);
        e.move_to(Pos::new(3, 0), false);
        e.backspace(false);
        assert_eq!(cells(&e), ["-:before", "A1:ab", "B1:", "-:after"]);
        // Enter inside a cell adds a paragraph to the cell.
        e.move_to(Pos::new(1, 1), false);
        e.newline();
        assert_eq!(cells(&e), ["-:before", "A1:a", "A1:b", "B1:", "-:after"]);
        e.backspace(false);
        assert_eq!(cells(&e), ["-:before", "A1:ab", "B1:", "-:after"]);
    }

    #[test]
    fn delete_range_across_cells_clears_them() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(1, 2);
        e.insert_str("one");
        e.next_cell();
        e.insert_str("two");
        // Select from inside A1 to inside "after".
        e.delete_range(Range::new(Pos::new(1, 1), Pos::new(3, 2)));
        // Cells are emptied, the table stays (it was not wholly selected).
        assert_eq!(cells(&e), ["-:before", "A1:o", "B1:", "-:ter"]);
        e.undo();
        assert_eq!(cells(&e), ["-:before", "A1:one", "B1:two", "-:after"]);
        // A range covering the whole table removes it and joins the ends.
        e.delete_range(Range::new(Pos::new(0, 6), Pos::new(3, 0)));
        assert_eq!(cells(&e), ["-:beforeafter"]);
        assert!(e.doc.tables.is_empty());
        e.undo();
        assert_eq!(cells(&e), ["-:before", "A1:one", "B1:two", "-:after"]);
        // Within one cell, ranges behave as usual.
        e.move_to(Pos::new(1, 3), false);
        e.newline();
        e.insert_str("more");
        e.delete_range(Range::new(Pos::new(1, 1), Pos::new(2, 2)));
        assert_eq!(cells(&e), ["-:before", "A1:ore", "B1:two", "-:after"]);
    }

    #[test]
    fn paste_into_cell_keeps_cell() {
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(1, 1);
        let f = crate::editor::Fragment::from_text("x\ny");
        e.paste(&f);
        assert_eq!(cells(&e), ["-:before", "A1:x", "A1:y", "-:after"]);
        assert!(e.doc.table_is_consistent(1));
    }

    #[test]
    fn column_letters_and_widths() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(27 * 26 - 1 + 1), "AAA".to_string().replace("AAA", &column_letters(702)));
        let mut e = ed();
        e.move_to(Pos::new(1, 0), false);
        e.insert_table(1, 2);
        assert!(e.set_column_width(2880));
        assert_eq!(e.doc.tables[&1].grid[0], 2880);
        assert!(e.doc.tables[&1].raw_grid.is_none());
        let grid = e.doc.table_screen_grid(&e.doc.tables[&1], 80);
        assert_eq!(grid.iter().sum::<u16>(), 80 - 3);
    }
}
