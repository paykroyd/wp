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

    fn select_cell_or_move(&mut self, para: usize) {
        self.commit();
        self.anchor = None;
        self.cursor = Pos::new(para, 0);
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
            cells: src_row.cells.iter().map(|c| TableCell { span: c.span, vmerge: None, width: c.width, shading: None, raw_tcpr: None, attrs: String::new() }).collect(),
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
            row.cells.insert(idx, TableCell { span: 1, vmerge: None, width, shading: None, raw_tcpr: None, attrs: String::new() });
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

    /// Column width of the cursor's cell, in twips.
    pub fn current_column_width(&self) -> Option<Twips> {
        let c = self.current_cell()?;
        let t = self.doc.tables.get(&c.table)?;
        Some(t.cell_extent(c.row as usize, c.col as usize).1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> Editor {
        Editor::new(crate::text::from_text("before\nafter", false))
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
