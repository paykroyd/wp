//! Rendering: draft view, Reveal Codes pane, status line, overlays.

use crate::app::{App, Overlay, View};
use crate::config::KeymapChoice;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph as RParagraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;
use wp_core::layout::{self, ScreenLine};
use wp_core::model::*;
use wp_core::reveal;

/// Terminal capability tier.
#[derive(Clone, Copy)]
pub struct Caps {
    pub ascii: bool,
    pub colors: bool,
}

pub fn detect_caps() -> Caps {
    let term = std::env::var("TERM").unwrap_or_default();
    let lang = std::env::var("LANG").unwrap_or_default().to_lowercase() + &std::env::var("LC_ALL").unwrap_or_default().to_lowercase();
    let ascii = term == "linux" || term == "dumb" || term == "vt100" || (!lang.contains("utf") && !lang.is_empty());
    let colors = term != "dumb";
    Caps { ascii, colors }
}

pub struct Chrome {
    pub h: &'static str,
}

pub fn chrome(c: Caps) -> Chrome {
    if c.ascii {
        Chrome { h: "-" }
    } else {
        Chrome { h: "─" }
    }
}

/// One rendered row of the draft view.
#[derive(Clone, Debug)]
pub enum Row {
    Line { para: usize, line: usize },
    Gap,
    PageRule(usize),
    Block { para: usize },
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn highlight_color(h: Highlight) -> Color {
    match h {
        Highlight::Yellow => Color::Yellow,
        Highlight::Green => Color::Green,
        Highlight::Cyan => Color::Cyan,
        Highlight::Magenta => Color::Magenta,
        Highlight::Blue => Color::Blue,
        Highlight::Red => Color::Red,
        Highlight::DarkBlue => Color::Blue,
        Highlight::DarkCyan => Color::Cyan,
        Highlight::DarkGreen => Color::Green,
        Highlight::DarkMagenta => Color::Magenta,
        Highlight::DarkRed => Color::Red,
        Highlight::DarkYellow => Color::Yellow,
        Highlight::DarkGray => Color::DarkGray,
        Highlight::LightGray => Color::Gray,
        Highlight::Black => Color::Black,
        Highlight::White => Color::White,
        Highlight::None => Color::Reset,
    }
}

fn style_for(props: &RunProps, caps: Caps) -> Style {
    let mut s = Style::default();
    if props.is_bold() {
        s = s.add_modifier(Modifier::BOLD);
    }
    if props.is_italic() {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if props.underline().is_some() {
        s = s.add_modifier(Modifier::UNDERLINED);
    }
    if props.is_strike() {
        s = s.add_modifier(Modifier::CROSSED_OUT);
    }
    if caps.colors {
        if let Some(c) = props.color {
            s = s.fg(rgb(c));
        }
        if let Some(h) = props.highlight() {
            s = s.bg(highlight_color(h));
        }
    }
    s
}

/// Walk rows starting at a scroll position.
pub struct RowWalker<'a> {
    app: &'a mut App,
    para: usize,
    line: usize,
    pending: Vec<Row>,
    done: bool,
    page_starts: Vec<(usize, usize, usize)>, // (para, item idx, page number 1-based)
}

impl<'a> RowWalker<'a> {
    pub fn new(app: &'a mut App, start: (usize, usize)) -> RowWalker<'a> {
        app.ed.ensure_layout();
        let mut page_starts = Vec::new();
        let pages = app.ed.layout.pagination.pages.clone();
        for (i, ps) in pages.iter().enumerate().skip(1) {
            let idx = app.ed.print_layout(ps.para).lines.get(ps.line).map(|l| l.start).unwrap_or(0);
            page_starts.push((ps.para, idx, i + 1));
        }
        RowWalker { app, para: start.0, line: start.1, pending: Vec::new(), done: false, page_starts }
    }

    fn page_rule_for(&self, para: usize, lines: &[ScreenLine], line: usize) -> Option<usize> {
        for &(p, idx, n) in &self.page_starts {
            if p != para {
                continue;
            }
            let l = &lines[line];
            let last = line + 1 == lines.len();
            if (idx >= l.start && idx < l.end) || (last && idx >= l.end) || (line == 0 && idx == 0 && l.start == 0) {
                return Some(n);
            }
        }
        None
    }

    pub fn next_row(&mut self) -> Option<Row> {
        if let Some(r) = self.pending.pop() {
            return Some(r);
        }
        if self.done {
            return None;
        }
        let n = self.app.ed.doc.paragraphs.len();
        if self.para >= n {
            self.done = true;
            return None;
        }
        let raw_block = self.app.ed.doc.paragraphs[self.para].props.raw_block;
        let lines = self.app.ed.screen_lines(self.para).clone();
        if self.line >= lines.len() {
            self.para += 1;
            self.line = 0;
            return self.next_row();
        }
        let para = self.para;
        let line = self.line;
        let rule = self.page_rule_for(para, &lines, line);
        let row = if raw_block { Row::Block { para } } else { Row::Line { para, line } };
        // Advance.
        let last_line = line + 1 >= lines.len() || raw_block;
        if last_line {
            // Gap after paragraph?
            if self.app.cfg.draft_spacing && para + 1 < n {
                let pp = self.app.ed.doc.para_props(para);
                let np = self.app.ed.doc.para_props(para + 1);
                let empty_next = self.app.ed.doc.paragraphs[para + 1].items.is_empty();
                let empty_this = self.app.ed.doc.paragraphs[para].items.is_empty();
                if (pp.space_after() >= 100 || np.space_before() >= 100) && !empty_next && !empty_this {
                    self.pending.push(Row::Gap);
                }
            }
            self.para += 1;
            self.line = 0;
        } else {
            self.line += 1;
        }
        // Page rule precedes the row it applies to: push row, return rule.
        if let Some(pg) = rule {
            self.pending.push(row); // popped before any Gap queued earlier
            return Some(Row::PageRule(pg));
        }
        Some(row)
    }
}

/// Rows from `scroll` to the cursor line, counting rows. Returns None if the
/// cursor precedes the scroll position.
fn rows_to_cursor(app: &mut App, scroll: (usize, usize), limit: usize) -> Option<usize> {
    let (cp, cl) = app.ed.screen_line_of_cursor();
    if (cp, cl) < scroll {
        return None;
    }
    let mut w = RowWalker::new(app, scroll);
    let mut count = 0;
    while let Some(r) = w.next_row() {
        if let Row::Line { para, line } = normalize(&r) {
            if para == cp && (line == cl || matches!(r, Row::Block { .. })) {
                return Some(count);
            }
        }
        count += 1;
        if count > limit + 4096 {
            return Some(count);
        }
    }
    Some(count)
}

fn normalize(r: &Row) -> Row {
    match r {
        Row::Block { para } => Row::Line { para: *para, line: 0 },
        other => other.clone(),
    }
}

/// Step the scroll position forward by one text line.
fn scroll_forward(app: &mut App, s: (usize, usize)) -> (usize, usize) {
    let n = app.ed.screen_lines(s.0).len();
    if s.1 + 1 < n && !app.ed.doc.paragraphs[s.0].props.raw_block {
        (s.0, s.1 + 1)
    } else if s.0 + 1 < app.ed.doc.paragraphs.len() {
        (s.0 + 1, 0)
    } else {
        s
    }
}

pub fn ensure_cursor_visible(app: &mut App, rows: usize) {
    let (cp, cl) = app.ed.screen_line_of_cursor();
    let cl = if app.ed.doc.paragraphs[cp].props.raw_block { 0 } else { cl };
    if app.scroll.0 >= app.ed.doc.paragraphs.len() {
        app.scroll = (0, 0);
    }
    if (cp, cl) < app.scroll {
        // Cursor above: scroll up so the cursor sits a few rows down.
        let mut s = (cp, cl);
        for _ in 0..3 {
            s = scroll_back(app, s);
        }
        app.scroll = s;
        return;
    }
    let mut guard = 0;
    loop {
        match rows_to_cursor(app, app.scroll, rows) {
            Some(k) if k < rows => break,
            Some(k) => {
                let step = (k + 1 - rows).max(1);
                for _ in 0..step {
                    app.scroll = scroll_forward(app, app.scroll);
                }
            }
            None => {
                app.scroll = (cp, cl);
                break;
            }
        }
        guard += 1;
        if guard > 64 {
            app.scroll = (cp, cl);
            break;
        }
    }
}

fn scroll_back(app: &mut App, s: (usize, usize)) -> (usize, usize) {
    if s.1 > 0 {
        (s.0, s.1 - 1)
    } else if s.0 > 0 {
        let n = app.ed.screen_lines(s.0 - 1).len();
        (s.0 - 1, n.saturating_sub(1))
    } else {
        s
    }
}

pub fn draw(f: &mut Frame, app: &mut App, caps: Caps) {
    let area = f.area();
    app.size = (area.width, area.height);
    let ch = chrome(caps);
    let mut y_bottom = area.height;

    // Status line (bottom).
    y_bottom -= 1;
    draw_status(f, app, Rect::new(0, y_bottom, area.width, 1), caps);

    // F-key legend.
    if app.cfg.fkey_legend && y_bottom >= 8 {
        y_bottom -= 5;
        draw_legend(f, app, Rect::new(0, y_bottom, area.width, 5), caps);
    }

    // Hint line.
    if app.hint && y_bottom >= 4 {
        y_bottom -= 1;
        let pal = app.keymap.label_for(crate::commands::Cmd::Palette).unwrap_or_else(|| "Ctrl+K".into());
        let help = app.keymap.label_for(crate::commands::Cmd::Help).unwrap_or_else(|| "F1".into());
        let hint = Line::from(vec![Span::styled(format!(" {} for commands · {} for help · Alt+F3 for Reveal Codes · any key dismisses this line", pal, help), Style::default().fg(Color::DarkGray))]);
        f.render_widget(RParagraph::new(hint), Rect::new(0, y_bottom, area.width, 1));
    }

    // Reveal Codes pane.
    let mut doc_area = Rect::new(0, 0, area.width, y_bottom);
    if app.reveal && y_bottom >= 6 {
        let pane_h = y_bottom * 2 / 5 + 1;
        doc_area.height = y_bottom - pane_h;
        draw_reveal(f, app, Rect::new(0, doc_area.height, area.width, pane_h), caps, &ch);
    }

    // Document.
    let cursor = draw_draft(f, app, doc_area, caps, &ch);

    // Overlays.
    match &app.overlay {
        Overlay::None => {
            if let Some((x, y)) = cursor {
                f.set_cursor_position((x, y));
            }
        }
        _ => draw_overlay(f, app, area, caps, &ch),
    }
}

/// Returns the cursor screen position.
fn draw_draft(f: &mut Frame, app: &mut App, area: Rect, caps: Caps, ch: &Chrome) -> Option<(u16, u16)> {
    let rows = area.height as usize;
    if rows == 0 {
        return None;
    }
    ensure_cursor_visible(app, rows);
    let left_margin: u16 = 1;
    let width = area.width.saturating_sub(left_margin + 1);
    let sel = app.ed.selection();
    let cursor = app.ed.cursor;
    let mut cursor_xy = None;
    let scroll = app.scroll;
    let mut lines_out: Vec<Line> = Vec::with_capacity(rows);
    let mut walker = RowWalker::new(app, scroll);
    let mut collected: Vec<Row> = Vec::new();
    while collected.len() < rows {
        match walker.next_row() {
            Some(r) => collected.push(r),
            None => break,
        }
    }
    drop(walker);
    let page_label = if app.view == View::Page { "Page" } else { "Page" };

    for (ri, row) in collected.iter().enumerate() {
        let y = area.y + ri as u16;
        match row {
            Row::Gap => lines_out.push(Line::default()),
            Row::PageRule(n) => {
                let label = format!(" {} {} ", page_label, n);
                let total = area.width as usize;
                let lw = label.width();
                let left = (total.saturating_sub(lw)) / 2;
                let right = total.saturating_sub(lw + left);
                let s = format!("{}{}{}", ch.h.repeat(left), label, ch.h.repeat(right));
                lines_out.push(Line::from(Span::styled(s, Style::default().fg(Color::DarkGray))));
            }
            Row::Block { para } => {
                let p = &app.ed.doc.paragraphs[*para];
                let label = match p.items.first() {
                    Some(Item::Code(Code::Opaque(o))) => o.label.clone(),
                    _ => "Block".into(),
                };
                let text = format!("[{} — preserved, not editable in this version]", label);
                let mut style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
                let selected = sel.map_or(false, |r| r.contains(Pos::new(*para, 0)));
                if selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                lines_out.push(Line::from(vec![Span::raw(" ".repeat(left_margin as usize)), Span::styled(text, style)]));
                if cursor.para == *para {
                    cursor_xy = Some((area.x + left_margin, y));
                }
            }
            Row::Line { para, line } => {
                let pi = *para;
                let pp = app.ed.doc.para_props(pi);
                let sl = app.ed.screen_lines(pi)[*line].clone();
                let nlines = app.ed.layout_screen_len(pi);
                let label = if *line == 0 { app.ed.list_label(pi) } else { None };
                let cols = app.ed.cols();
                let runs = app.ed.doc.runs(pi);
                let p = &app.ed.doc.paragraphs[pi];
                // Alignment offset.
                let avail = width.saturating_sub(sl.indent);
                let slack = avail.saturating_sub(sl.width);
                let align_off = match pp.align() {
                    Align::Center => slack / 2,
                    Align::Right => slack,
                    _ => 0,
                };
                let mut spans: Vec<Span> = Vec::new();
                let x: u16 = left_margin + sl.indent + align_off;
                match label {
                    Some(l) if !l.text.is_empty() => {
                        let first = layout::screen_first_indent(&pp, cols);

                        let lx = left_margin + first + align_off;
                        let mut text = l.text.clone();
                        if caps.ascii {
                            text = text.chars().map(|c| if c.is_ascii() { c } else { '*' }).collect();
                        }
                        let props = app.ed.doc.base_run_props(pi).merge(&l.run);
                        spans.push(Span::raw(" ".repeat(lx.min(x) as usize)));
                        spans.push(Span::styled(text.clone(), style_for(&props, caps)));
                        spans.push(Span::raw(" ".repeat((x as usize).saturating_sub(lx as usize + text.width()))));
                    }
                    _ => spans.push(Span::raw(" ".repeat(x as usize))),
                }

                let mut ri = 0;
                let mut cur_style: Option<Style> = None;
                let mut buf = String::new();
                let mut rel_x: u16 = 0;
                let flush = |spans: &mut Vec<Span>, buf: &mut String, st: Option<Style>| {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(buf), st.unwrap_or_default()));
                    }
                };
                for i in sl.start..sl.end {
                    while ri + 1 < runs.len() && runs[ri].end <= i {
                        ri += 1;
                    }
                    let it = &p.items[i];
                    let pos = Pos::new(pi, i);
                    if pos == cursor && cursor_xy.is_none() {
                        cursor_xy = Some((area.x + x + rel_x, y));
                    }
                    let mut st = style_for(&runs[ri].props, caps);
                    if sel.map_or(false, |r| r.contains(pos)) {
                        st = st.add_modifier(Modifier::REVERSED);
                    }
                    let adv = layout::screen_advance(it, rel_x, &pp);
                    let text: String = match it {
                        Item::Char(c) => {
                            let c = if runs[ri].props.all_caps.unwrap_or(false) { c.to_uppercase().next().unwrap_or(*c) } else { *c };
                            if (c as u32) < 0x20 || c == '\u{ad}' { String::new() } else { c.to_string() }
                        }
                        Item::Code(Code::Tab) => " ".repeat(adv as usize),
                        Item::Code(Code::Opaque(o)) if o.kind == OpaqueKind::Element => {
                            // Zero-width in layout, but show a marker for non-trivial preserved content.
                            match o.label.as_str() {
                                "Drawing" | "Picture" | "Object" => String::new(),
                                _ => String::new(),
                            }
                        }
                        _ => String::new(),
                    };
                    if Some(st) != cur_style {
                        flush(&mut spans, &mut buf, cur_style);
                        cur_style = Some(st);
                    }
                    buf.push_str(&text);
                    rel_x += adv;
                }
                flush(&mut spans, &mut buf, cur_style);
                // Cursor at end of line.
                if cursor.para == pi && cursor_xy.is_none() {
                    let is_last = *line + 1 == nlines;
                    let in_line = cursor.idx >= sl.start && (cursor.idx < sl.end || (is_last && cursor.idx >= sl.end));
                    if in_line {
                        let cx = layout::screen_x_of(p, &pp, &sl, cursor.idx);
                        cursor_xy = Some((area.x + x + cx, y));
                    }
                }
                // Drawings/placeholder boxes: show inline marker for opaque drawings on this line.
                for i in sl.start..sl.end {
                    if let Item::Code(Code::Opaque(o)) = &p.items[i] {
                        if matches!(o.label.as_str(), "Drawing" | "Picture" | "Object") {
                            spans.push(Span::styled(format!(" [{}] ", o.label), Style::default().fg(Color::DarkGray).add_modifier(Modifier::REVERSED)));
                        }
                    }
                }
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

fn draw_status(f: &mut Frame, app: &mut App, area: Rect, caps: Caps) {
    let (pg, ln, pos) = app.ed.cursor_page_ln_pos();
    let pages = app.ed.page_count();
    let title = app.title();
    let dirty = if app.ed.dirty { " *" } else { "" };
    let mut indicators: Vec<String> = Vec::new();
    if app.ed.has_selection() || app.block_mode {
        indicators.push("Block".into());
    }
    if app.ed.typeover {
        indicators.push("Typeover".into());
    }
    if app.reveal {
        indicators.push("Reveal".into());
    }
    if app.view == View::Page {
        indicators.push("Page view".into());
    }
    if let Some(r) = app.repeat {
        indicators.push(format!("Repeat {}", r));
    }
    let msg = app.status_text().unwrap_or_default();
    let right = format!("Doc 1  Pg {}/{}  Ln {:.2}\"  Pos {:.2}\"", pg, pages, ln as f64 / 1440.0, pos as f64 / 1440.0);
    let left = format!(" {}{}", title, dirty);
    let mid = if msg.is_empty() { indicators.join("  ") } else { msg };
    let w = area.width as usize;
    let lw = left.width();
    let rw = right.width() + 1;
    let mid_w = w.saturating_sub(lw + rw + 2);
    let mid_trunc: String = truncate(&mid, mid_w);
    let pad = w.saturating_sub(lw + 2 + mid_trunc.width() + rw);
    let line = Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(mid_trunc, if caps.colors { Style::default().fg(Color::Yellow) } else { Style::default() }),
        Span::raw(" ".repeat(pad)),
        Span::raw(right),
        Span::raw(" "),
    ]);
    let style = if caps.colors { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default().add_modifier(Modifier::REVERSED) };
    f.render_widget(RParagraph::new(line).style(style), area);
}

fn truncate(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + 2 > w {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

fn draw_legend(f: &mut Frame, app: &mut App, area: Rect, caps: Caps) {
    let mods = ["Ctrl", "Alt", "Shift", ""];
    let colw = (area.width as usize / 12).max(6);
    let mut lines: Vec<Line> = Vec::new();
    let header: String = (1..=12).map(|n| format!("{:<w$}", format!("F{}", n), w = colw)).collect();
    lines.push(Line::from(Span::styled(header, Style::default().add_modifier(Modifier::BOLD))));
    for (mi, m) in mods.iter().enumerate() {
        let mut spans = Vec::new();
        for n in 1..=12u8 {
            let cmd = app.keymap.fkey_row(n)[mi];
            let label = cmd.map(|c| short_title(crate::app::cmd_title(c))).unwrap_or_default();
            let cell = truncate(&format!("{}{}", if m.is_empty() { "" } else { "" }, label), colw - 1);
            spans.push(Span::styled(format!("{:<w$}", cell, w = colw), legend_style(mi, caps)));
        }
        lines.push(Line::from(spans));
    }
    let _ = mods;
    f.render_widget(RParagraph::new(lines), area);
}

fn legend_style(mi: usize, caps: Caps) -> Style {
    if !caps.colors {
        return Style::default();
    }
    match mi {
        0 => Style::default().fg(Color::Red),
        1 => Style::default().fg(Color::Blue),
        2 => Style::default().fg(Color::Green),
        _ => Style::default(),
    }
}

fn short_title(t: &str) -> String {
    let t = t.split(" (").next().unwrap_or(t);
    let t = t.split('…').next().unwrap_or(t);
    let t = t.replace("Style: ", "").replace("Line Spacing: ", "Sp ").replace("Keyboard: ", "");
    t
}

/// A token in the Reveal Codes stream.
#[derive(Clone)]
struct Tok {
    text: String,
    /// (para, item index) — item index == len means [HRt]
    pos: Option<(usize, usize)>,
    para_code: Option<(usize, usize)>,
    style: Style,
}

fn draw_reveal(f: &mut Frame, app: &mut App, area: Rect, caps: Caps, ch: &Chrome) {
    // Header rule.
    let title = " Reveal Codes ";
    let w = area.width as usize;
    let head = format!("{}{}{}", ch.h.repeat(w.saturating_sub(title.width() + 4)), title, ch.h.repeat(4));
    f.render_widget(RParagraph::new(Line::from(Span::styled(head, Style::default().fg(Color::DarkGray)))), Rect::new(area.x, area.y, area.width, 1));
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let rows = body.height as usize;
    if rows == 0 {
        return;
    }
    let cursor = app.ed.cursor;
    let sel = app.ed.selection();
    let code_style = if caps.colors { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().add_modifier(Modifier::REVERSED) };
    let soft_style = if caps.colors { Style::default().fg(Color::DarkGray).add_modifier(Modifier::REVERSED) } else { Style::default().add_modifier(Modifier::DIM) };

    // Build tokens for a window of paragraphs around the cursor.
    let first = cursor.para.saturating_sub(1);
    let last = (cursor.para + 2).min(app.ed.doc.paragraphs.len() - 1);
    let mut toks: Vec<Tok> = Vec::new();
    for pi in first..=last {
        let p = app.ed.doc.paragraphs[pi].clone();
        let pl = app.ed.print_layout(pi).clone();
        let page_breaks: Vec<usize> = {
            app.ed.ensure_layout();
            let pages = &app.ed.layout.pagination.pages;
            pages.iter().filter(|ps| ps.para == pi && ps.line > 0).filter_map(|ps| pl.lines.get(ps.line).map(|l| l.start)).collect()
        };
        for (ci, (_, label)) in reveal::para_codes(&p.props).iter().enumerate() {
            toks.push(Tok { text: label.clone(), pos: None, para_code: Some((pi, ci)), style: code_style });
        }
        let soft_starts: Vec<usize> = pl.lines.iter().skip(1).map(|l| l.start).collect();
        for (i, it) in p.items.iter().enumerate() {
            if soft_starts.contains(&i) && i > 0 {
                let hard_before = matches!(p.items[i - 1], Item::Code(Code::LineBreak) | Item::Code(Code::PageBreak));
                if !hard_before {
                    let label = if page_breaks.contains(&i) { "[SPg]" } else { "[SRt]" };
                    toks.push(Tok { text: label.into(), pos: None, para_code: None, style: soft_style });
                }
            }
            let mut st = Style::default();
            let in_sel = sel.map_or(false, |r| r.contains(Pos::new(pi, i)));
            match it {
                Item::Char(c) => {
                    if in_sel {
                        st = st.add_modifier(Modifier::REVERSED);
                    }
                    let s = if *c == ' ' { " ".to_string() } else if (*c as u32) < 0x20 { "·".into() } else { c.to_string() };
                    toks.push(Tok { text: s, pos: Some((pi, i)), para_code: None, style: st });
                }
                Item::Code(code) => {
                    let preserved = matches!(code, Code::On(Attr::Raw(_)) | Code::Off(AttrKind::Raw) | Code::On(Attr::RunAttrs(_)) | Code::Off(AttrKind::RunAttrs))
                        || matches!(code, Code::Opaque(o) if o.hint);
                    if preserved && !app.reveal_all {
                        continue;
                    }

                    let mut st = code_style;
                    if in_sel {
                        st = st.add_modifier(Modifier::UNDERLINED);
                    }
                    toks.push(Tok { text: reveal::code_label(code), pos: Some((pi, i)), para_code: None, style: st });
                }
            }
        }
        toks.push(Tok { text: "[HRt]".into(), pos: Some((pi, p.items.len())), para_code: None, style: code_style });
    }

    // Wrap tokens into rows.
    let width = area.width.saturating_sub(2) as usize;
    let mut rowsv: Vec<Vec<Tok>> = vec![Vec::new()];
    let mut x = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;
    let cursor_key = match app.reveal_para_code {
        Some(ci) => (None, Some((cursor.para, ci))),
        None => (Some((cursor.para, cursor.idx)), None),
    };
    let mut found = false;
    for t in toks {
        let tw = t.text.width().max(1);
        if x + tw > width && x > 0 {
            rowsv.push(Vec::new());
            x = 0;
        }
        let is_cursor = !found && ((t.pos.is_some() && t.pos == cursor_key.0) || (t.para_code.is_some() && t.para_code == cursor_key.1));
        if is_cursor {
            cursor_row = rowsv.len() - 1;
            cursor_col = x;
            found = true;
        }
        x += tw;
        let is_hrt = t.text == "[HRt]";
        rowsv.last_mut().unwrap().push(Tok { style: if is_cursor { t.style.add_modifier(Modifier::REVERSED).add_modifier(Modifier::BOLD) } else { t.style }, ..t });
        if is_hrt {
            rowsv.push(Vec::new());
            x = 0;
        }
    }
    let _ = cursor_col;
    // Choose the window of rows around the cursor row.
    let start = cursor_row.saturating_sub(rows / 2);
    let mut lines: Vec<Line> = Vec::new();
    for r in rowsv.iter().skip(start).take(rows) {
        let mut spans = vec![Span::raw(" ")];
        for t in r {
            spans.push(Span::styled(t.text.clone(), t.style));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(RParagraph::new(lines), body);
}

fn draw_overlay(f: &mut Frame, app: &mut App, area: Rect, caps: Caps, ch: &Chrome) {
    let overlay = app.overlay.clone();
    let border_style = if caps.colors { Style::default().fg(Color::Cyan) } else { Style::default() };
    let boxed = |title: &str| Block::default().borders(Borders::ALL).title(format!(" {} ", title)).border_style(border_style);
    let _ = ch;
    match overlay {
        Overlay::None => {}
        Overlay::Palette { input, selected } => {
            let rows = app.palette_rows(&input);
            let w = area.width.saturating_sub(4).min(78).max(30);
            let n = rows.len().min(12) as u16;
            let h = n + 4;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            f.render_widget(Clear, r);
            let mode = match input.chars().next() {
                Some('@') => "Heading",
                Some('#') => "Page",
                Some('/') => "Find",
                Some('?') => "Help",
                _ => "Command",
            };
            f.render_widget(boxed(mode), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let prompt = Line::from(vec![Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(input.clone())]);
            f.render_widget(RParagraph::new(prompt), Rect::new(inner.x, inner.y, inner.width, 1));
            f.set_cursor_position((inner.x + 2 + input.width() as u16, inner.y));
            let sep = Line::from(Span::styled(ch.h.repeat(inner.width as usize), Style::default().fg(Color::DarkGray)));
            f.render_widget(RParagraph::new(sep), Rect::new(inner.x, inner.y + 1, inner.width, 1));
            let sel_i = selected.min(rows.len().saturating_sub(1));
            let first = sel_i.saturating_sub(11);
            let mut lines = Vec::new();
            for (i, row) in rows.iter().enumerate().skip(first).take(12) {
                let key_w = row.key.width();
                let label_w = (inner.width as usize).saturating_sub(key_w + 4);
                let label = truncate(&row.label, label_w);
                let detail = if row.detail.is_empty() { String::new() } else { format!("  {}", row.detail) };
                let detail = truncate(&detail, label_w.saturating_sub(label.width()));
                let pad = (inner.width as usize).saturating_sub(2 + label.width() + detail.width() + key_w + 1);
                let mut style = Style::default();
                if i == sel_i {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}", label), style),
                    Span::styled(detail, style.fg(Color::DarkGray)),
                    Span::styled(" ".repeat(pad), style),
                    Span::styled(format!("{} ", row.key), style.fg(if caps.colors { Color::Yellow } else { Color::Reset })),
                ]));
            }
            if rows.is_empty() {
                lines.push(Line::from(Span::styled("  No matching command", Style::default().fg(Color::DarkGray))));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Prompt { label, input, .. } => {
            let y = area.height.saturating_sub(2);
            let r = Rect::new(0, y, area.width, 1);
            f.render_widget(Clear, r);
            let style = if caps.colors { Style::default().bg(Color::Blue).fg(Color::White) } else { Style::default().add_modifier(Modifier::REVERSED) };
            let text = format!("{}{}", label, input);
            let shown = if text.width() > area.width as usize { truncate(&text, area.width as usize) } else { text.clone() };
            f.render_widget(RParagraph::new(Line::from(shown)).style(style), r);
            f.set_cursor_position((text.width().min(area.width as usize - 1) as u16, y));
        }
        Overlay::List { title, items, selected, filter, .. } => {
            let visible: Vec<&crate::app::ListItem> = items.iter().filter(|it| crate::palette::score(&filter, &format!("{} {}", it.label, it.detail)).is_some()).collect();
            let w = area.width.saturating_sub(4).min(90).max(30);
            let n = visible.len().min(16) as u16;
            let h = n + 4;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            f.render_widget(Clear, r);
            f.render_widget(boxed(&title), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let prompt = Line::from(vec![Span::styled("filter: ", Style::default().fg(Color::DarkGray)), Span::raw(filter.clone())]);
            f.render_widget(RParagraph::new(prompt), Rect::new(inner.x, inner.y, inner.width, 1));
            f.set_cursor_position((inner.x + 8 + filter.width() as u16, inner.y));
            let sel_i = selected.min(visible.len().saturating_sub(1));
            let first = sel_i.saturating_sub(15);
            let mut lines = Vec::new();
            for (i, it) in visible.iter().enumerate().skip(first).take(16) {
                let mut style = Style::default();
                if i == sel_i {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let label = truncate(&it.label, (inner.width as usize / 2).max(10));
                let detail = truncate(&it.detail, (inner.width as usize).saturating_sub(label.width() + 4));
                let pad = (inner.width as usize).saturating_sub(2 + label.width() + 2 + detail.width());
                lines.push(Line::from(vec![Span::styled(format!("  {}  ", label), style), Span::styled(detail, style.fg(Color::DarkGray)), Span::styled(" ".repeat(pad), style)]));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Confirm { question, .. } => {
            let y = area.height.saturating_sub(2);
            let r = Rect::new(0, y, area.width, 1);
            f.render_widget(Clear, r);
            let style = if caps.colors { Style::default().bg(Color::Red).fg(Color::White) } else { Style::default().add_modifier(Modifier::REVERSED) };
            f.render_widget(RParagraph::new(Line::from(truncate(&question, area.width as usize))).style(style), r);
        }
        Overlay::Help => {
            let lines = help_lines(app);
            let w = area.width.saturating_sub(4).min(84).max(30);
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1, w, h);
            f.render_widget(Clear, r);
            f.render_widget(boxed("Help — any key closes"), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            f.render_widget(RParagraph::new(lines.into_iter().map(|l| Line::from(l)).collect::<Vec<_>>()), inner);
        }
        Overlay::ReplacePreview { find, with, matches, selected } => {
            let w = area.width.saturating_sub(4).min(100).max(30);
            let n = matches.len().min(14) as u16;
            let h = n + 5;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            f.render_widget(Clear, r);
            let title = format!("Replace {} → “{}” — {} match{}", app.find.build(&find).describe(), with, matches.len(), if matches.len() == 1 { "" } else { "es" });
            f.render_widget(boxed(&truncate(&title, w as usize - 4)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let hint = Line::from(Span::styled("Enter/A: replace all   O: one at a time   ↑↓: preview in place   Esc: cancel", Style::default().fg(Color::DarkGray)));
            f.render_widget(RParagraph::new(hint), Rect::new(inner.x, inner.y, inner.width, 1));
            let sel_i = selected.min(matches.len().saturating_sub(1));
            let first = sel_i.saturating_sub(13);
            let mut lines = Vec::new();
            let regex = app.find.build(&find).regex;
            for (i, m) in matches.iter().enumerate().skip(first).take(14) {
                let mut style = Style::default();
                if i == sel_i {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let ctx = wp_core::search::context(&app.ed.doc, m, (inner.width as usize).saturating_sub(24));
                let rep = wp_core::search::expand_replacement(&with, m, regex);
                let row = format!(" ¶{:<4} {}  → {}", m.range.start.para + 1, ctx, rep);
                let row = truncate(&row, inner.width as usize);
                let pad = (inner.width as usize).saturating_sub(row.width());
                lines.push(Line::from(vec![Span::styled(row, style), Span::styled(" ".repeat(pad), style)]));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1)));
        }
        Overlay::ReplaceStep { with, done, total, .. } => {
            let y = area.height.saturating_sub(2);
            let r = Rect::new(0, y, area.width, 1);
            f.render_widget(Clear, r);
            let style = if caps.colors { Style::default().bg(Color::Blue).fg(Color::White) } else { Style::default().add_modifier(Modifier::REVERSED) };
            let text = format!("Replace this one with “{}”?  y = yes  n = skip  a = all remaining  Esc = stop   ({} of {} replaced)", with, done, total);
            f.render_widget(RParagraph::new(Line::from(truncate(&text, area.width as usize))).style(style), r);
        }
        Overlay::Message { title, lines } => {
            let w = area.width.saturating_sub(4).min(80).max(30);

            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1, w, h);
            f.render_widget(Clear, r);
            f.render_widget(boxed(&format!("{} — any key closes", title)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            f.render_widget(RParagraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()), inner);
        }
    }
}

fn help_lines(app: &App) -> Vec<String> {
    let k = |cmd: crate::commands::Cmd| app.keymap.label_for(cmd).unwrap_or_else(|| "Ctrl+K".into());
    use crate::commands::Cmd::*;
    let mut v = vec![
        format!("Keyboard: {}", match app.keymap.choice {
            KeymapChoice::Classic => "Classic (WordPerfect 5.1). Switch with Ctrl+K → Keyboard: Modern",
            KeymapChoice::Modern => "Modern, with classic F-keys underneath. Switch with Ctrl+K → Keyboard: Classic",
        }),
        String::new(),
        format!("  {:<14} Command palette — every command, with its key", k(Palette)),
        format!("  {:<14} Reveal Codes — see, select, and delete formatting codes", k(RevealCodes)),
        format!("  {:<14} Save      {:<14} Open      {:<14} Exit", k(Save), k(Open), k(Exit)),
        format!("  {:<14} Bold      {:<14} Italic    {:<14} Underline", k(Bold), k(Italic), k(Underline)),
        format!("  {:<14} Undo      {:<14} Find      {:<14} Go to page", k(Undo), k(Find), k(GoToPage)),
        format!("  {:<14} Center    {:<14} Right     {:<14} Indent", k(AlignCenter), k(AlignRight), k(Indent)),
        format!("  {:<14} Cut       {:<14} Copy      {:<14} Paste", k(Cut), k(Copy), k(Paste)),
        format!("  {:<14} Block/select (then move; Shift+arrows also select)", k(Block)),
        format!("  {:<14} Styles    {:<14} Page break  {:<14} Word count", k(ApplyStyle), k(PageBreak), k(WordCount)),
        String::new(),
        "  Palette prefixes:  @ heading   # page   / find   ? help".into(),
        "  Reveal Codes: ←/→ step over codes; Del/Backspace removes a code and its pair;".into(),
        "  paragraph codes ([Style:], [Just:], [L Ind:]) sit at the paragraph start.".into(),
        format!("  Config: {}", crate::config::config_path().display()),
    ];
    if app.keymap.choice == KeymapChoice::Classic {
        v.push("  Esc n <key> repeats a key n times (e.g. Esc 8 ↓). F1 cancels.".into());
    }
    v
}

/// Helper so ui can ask for the number of screen lines without borrowing twice.
pub trait ScreenLen {
    fn layout_screen_len(&mut self, para: usize) -> usize;
}

impl ScreenLen for wp_core::Editor {
    fn layout_screen_len(&mut self, para: usize) -> usize {
        self.screen_lines(para).len()
    }
}

