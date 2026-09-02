//! Rendering: draft view, Reveal Codes pane, status line, overlays.

use crate::app::{App, Overlay, View};
use crate::config::{KeymapChoice, ThemeChoice};
use crate::menu::{self, Item as MenuItem, MENUS};
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
    /// 24-bit colour: the classic theme uses the exact CGA values; without
    /// it, the nearest of the 16 ANSI colours.
    pub truecolor: bool,
}

pub fn detect_caps() -> Caps {
    let term = std::env::var("TERM").unwrap_or_default();
    let lang = std::env::var("LANG").unwrap_or_default().to_lowercase() + &std::env::var("LC_ALL").unwrap_or_default().to_lowercase();
    let ascii = term == "linux" || term == "dumb" || term == "vt100" || (!lang.contains("utf") && !lang.is_empty());
    let colors = term != "dumb";
    let colorterm = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let truecolor = colors
        && (colorterm == "truecolor"
            || colorterm == "24bit"
            || ["kitty", "ghostty", "wezterm", "alacritty", "foot", "direct"].iter().any(|t| term.contains(t))
            || ["ghostty", "iterm", "wezterm", "vscode"].iter().any(|t| program.contains(t)));
    Caps { ascii, colors, truecolor }
}

/// The CGA/VGA text-mode palette WordPerfect 5.1 drew with, by attribute
/// index, with the ANSI colour a 16-colour terminal shows instead.
fn cga(caps: Caps, idx: u8) -> Color {
    const RGB: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00), (0x00, 0x00, 0xAA), (0x00, 0xAA, 0x00), (0x00, 0xAA, 0xAA),
        (0xAA, 0x00, 0x00), (0xAA, 0x00, 0xAA), (0xAA, 0x55, 0x00), (0xAA, 0xAA, 0xAA),
        (0x55, 0x55, 0x55), (0x55, 0x55, 0xFF), (0x55, 0xFF, 0x55), (0x55, 0xFF, 0xFF),
        (0xFF, 0x55, 0x55), (0xFF, 0x55, 0xFF), (0xFF, 0xFF, 0x55), (0xFF, 0xFF, 0xFF),
    ];
    const ANSI: [Color; 16] = [
        Color::Black, Color::Blue, Color::Green, Color::Cyan, Color::Red, Color::Magenta, Color::Yellow, Color::Gray,
        Color::DarkGray, Color::LightBlue, Color::LightGreen, Color::LightCyan, Color::LightRed, Color::LightMagenta, Color::LightYellow, Color::White,
    ];
    if caps.truecolor {
        let (r, g, b) = RGB[idx as usize & 15];
        Color::Rgb(r, g, b)
    } else {
        ANSI[idx as usize & 15]
    }
}
const CGA_BLUE: u8 = 1;
const CGA_YELLOW: u8 = 14;
const CGA_CYAN: u8 = 3;
const CGA_RED: u8 = 4;
const CGA_GRAY: u8 = 7;
const CGA_WHITE: u8 = 15;

pub struct Chrome {
    pub h: &'static str,
    /// Table borders: vertical, and the corner/junction set indexed by
    /// which of (up, down, left, right) arms are present.
    pub v: char,
    pub junction: [char; 16],
}

pub fn chrome(c: Caps) -> Chrome {
    if c.ascii {
        Chrome { h: "-", v: '|', junction: ['+'; 16] }
    } else {
        // Bits: 1 = up, 2 = down, 4 = left, 8 = right.
        let mut j = ['─'; 16];
        j[0b0011] = '│';
        j[0b0001] = '│';
        j[0b0010] = '│';
        j[0b1100] = '─';
        j[0b0100] = '─';
        j[0b1000] = '─';
        j[0b1010] = '┌';
        j[0b0110] = '┐';
        j[0b1001] = '└';
        j[0b0101] = '┘';
        j[0b1011] = '├';
        j[0b0111] = '┤';
        j[0b1110] = '┬';
        j[0b1101] = '┴';
        j[0b1111] = '┼';
        Chrome { h: "─", v: '│', junction: j }
    }
}

/// Screen colours. `Default` leaves the terminal's own colours alone.
/// `Classic` is the WordPerfect 5.1 screen, taken from the real thing: one
/// blue ground (CGA 1, #0000AA) under everything — text, menu bar, status
/// line, pop-ups; body text CGA light grey (#AAAAAA); bold, the file name,
/// the menu titles and `Doc 1 Pg 1 Ln 1" Pos 1"` bright white; menu
/// mnemonics CGA red; block and the current menu item in reverse video.
#[derive(Clone, Copy)]
pub struct Theme {
    pub classic: bool,
    /// Painted over the whole screen first (classic only).
    pub base: Style,
    /// Rules, hints, secondary text.
    pub dim: Color,
    /// Bold runs (classic brightens them; default leaves the modifier alone).
    pub bold_fg: Option<Color>,
    /// Text at "Very Large" or above (≥ 150 % of the body size), WP 5.1's
    /// way of showing a size the screen cannot: a colour.
    pub size_fg: Option<Color>,
    pub status: Style,
    /// The transient message / indicators in the middle of the status line.
    pub status_mid: Style,
    pub bar: Style,
    pub bar_open: Style,
    /// The mnemonic letter in a menu title.
    pub mnemonic: Style,
    pub menu: Style,
    pub menu_sel: Style,
    pub border: Style,
    pub key: Color,
    pub prompt: Style,
    pub confirm: Style,
    /// Ground of pop-up boxes.
    pub popup: Style,
    /// A code in Reveal Codes.
    pub code: Style,
}

pub fn theme(app: &App, caps: Caps) -> Theme {
    if !caps.colors {
        let rev = Style::default().add_modifier(Modifier::REVERSED);
        return Theme {
            classic: false,
            base: Style::default(),
            dim: Color::Reset,
            bold_fg: None,
            size_fg: None,
            status: rev,
            status_mid: Style::default(),
            bar: rev,
            bar_open: Style::default(),
            mnemonic: Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
            menu: Style::default(),
            menu_sel: rev,
            border: Style::default(),
            key: Color::Reset,
            prompt: rev,
            confirm: rev,
            popup: Style::default(),
            code: rev,
        };
    }
    match app.theme() {
        ThemeChoice::Default => Theme {
            classic: false,
            base: Style::default(),
            dim: Color::DarkGray,
            bold_fg: None,
            size_fg: Some(Color::Cyan),
            status: Style::default().add_modifier(Modifier::REVERSED),
            status_mid: Style::default().fg(Color::Yellow),
            bar: Style::default().add_modifier(Modifier::REVERSED),
            bar_open: Style::default(),
            mnemonic: Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
            menu: Style::default(),
            menu_sel: Style::default().add_modifier(Modifier::REVERSED),
            border: Style::default().fg(Color::Cyan),
            key: Color::Yellow,
            prompt: Style::default().bg(Color::Blue).fg(Color::White),
            confirm: Style::default().bg(Color::Red).fg(Color::White),
            popup: Style::default(),
            code: Style::default().fg(Color::Black).bg(Color::Cyan),
        },
        ThemeChoice::Classic => {
            let blue = cga(caps, CGA_BLUE);
            let grey = cga(caps, CGA_GRAY);
            let white = cga(caps, CGA_WHITE);
            let ground = Style::default().bg(blue).fg(grey);
            let bright = Style::default().bg(blue).fg(white);
            let reverse = Style::default().bg(grey).fg(blue);
            Theme {
                classic: true,
                base: ground,
                dim: cga(caps, CGA_CYAN),
                bold_fg: Some(white),
                size_fg: Some(cga(caps, CGA_YELLOW)),
                status: bright,
                status_mid: bright,
                bar: bright,
                bar_open: reverse,
                mnemonic: Style::default().fg(cga(caps, CGA_RED)),
                menu: ground,
                menu_sel: reverse,
                border: ground,
                key: grey,
                prompt: bright,
                confirm: bright,
                popup: ground,
                code: reverse,
            }
        }
    }
}

/// Blank a region, painting the theme's pop-up ground under it.
fn clear(f: &mut Frame, r: Rect, th: &Theme) {
    f.render_widget(Clear, r);
    f.render_widget(Block::default().style(th.popup), r);
}

/// One rendered row of the draft view.
#[derive(Clone, Debug)]
pub enum Row {
    Line { para: usize, line: usize },
    Gap,
    PageRule(usize),
    Block { para: usize },
    /// Screen line `line` of table row `row`, whose first paragraph is `para`.
    TableLine { table: u32, row: usize, para: usize, line: usize },
    /// The horizontal rule above row `row` (`row == rows` is the bottom rule).
    TableRule { table: u32, row: usize },
}

/// (row's first paragraph, line within the row) for a screen line of a
/// table-cell paragraph; unchanged for any other paragraph. Scroll positions
/// and cursor rows inside tables are compared in these coordinates.
fn row_coords(app: &mut App, para: usize, line: usize) -> (usize, usize) {
    let Some(c) = app.ed.doc.cell_of(para) else { return (para, line) };
    let Some(paras) = app.ed.doc.table_paras(para) else { return (para, line) };
    let row = &paras[c.row as usize];
    let cell = &row[c.col as usize];
    let mut k = 0;
    for &p in cell {
        if p == para {
            return (row[0][0], k + line);
        }
        k += app.ed.screen_lines(p).len();
    }
    (row[0][0], k)
}

/// Screen lines in the table row that starts at paragraph `row_start`.
fn row_lines(app: &mut App, row_start: usize) -> usize {
    let Some(c) = app.ed.doc.cell_of(row_start) else { return 1 };
    let Some(paras) = app.ed.doc.table_paras(row_start) else { return 1 };
    let row = &paras[c.row as usize];
    let mut n = 1;
    for cell in row {
        let mut k = 0;
        for &p in cell {
            k += app.ed.screen_lines(p).len();
        }
        n = n.max(k);
    }
    n
}

/// The first paragraph of the next table row, or the paragraph after the
/// table.
fn next_row_start(app: &App, row_start: usize) -> usize {
    let Some(c) = app.ed.doc.cell_of(row_start) else { return row_start + 1 };
    let Some((_, end)) = app.ed.doc.table_bounds(row_start) else { return row_start + 1 };
    let mut i = row_start;
    while i < end && app.ed.doc.cell_of(i).map_or(false, |x| x.row == c.row) {
        i += 1;
    }
    i
}

/// The body text size a document's sizes are read against.
pub fn body_size_hp(doc: &wp_core::Document) -> u16 {
    doc.styles.resolve_para_style_run(None).size_hp()
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

/// Size classes relative to the body text, as WordPerfect 5.1 named them.
/// A terminal cannot show a size, so each class gets an attribute instead —
/// Large is bold, Very Large and above add the theme's size colour, Fine
/// and Small are dim — and Reveal Codes shows the real points.
fn size_class(props: &RunProps, base_hp: u16) -> Option<&'static str> {
    let ratio = props.size_hp() as u32 * 100 / base_hp.max(1) as u32;
    if ratio >= 150 {
        Some("very-large")
    } else if ratio >= 120 {
        Some("large")
    } else if ratio <= 85 {
        Some("small")
    } else {
        None
    }
}

fn style_for(props: &RunProps, base_hp: u16, caps: Caps, th: &Theme) -> Style {
    let mut s = Style::default();
    if props.is_bold() {
        s = s.add_modifier(Modifier::BOLD);
        if let Some(c) = th.bold_fg {
            s = s.fg(c);
        }
    }
    match size_class(props, base_hp) {
        Some("very-large") => {
            s = s.add_modifier(Modifier::BOLD);
            if let Some(c) = th.size_fg {
                s = s.fg(c);
            } else if let Some(c) = th.bold_fg {
                s = s.fg(c);
            }
        }
        Some("large") => {
            s = s.add_modifier(Modifier::BOLD);
            if let Some(c) = th.bold_fg {
                s = s.fg(c);
            }
        }
        Some(_) => s = s.add_modifier(Modifier::DIM),
        None => {}
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
    // WordPerfect 5.1 showed attributes, never the document's colours: on
    // the classic screen a dark-blue heading would vanish into the ground.
    // Colour and highlight stay visible in Reveal Codes.
    if caps.colors && !th.classic {
        // A document's black is "the text colour", not a colour: forcing
        // #000000 makes the text vanish on a dark terminal (Google Docs
        // sets it explicitly on Normal text).
        if let Some(c) = props.color.filter(|c| *c != Rgb(0, 0, 0)) {
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
        let start = row_coords(app, start.0, start.1);
        RowWalker { app, para: start.0, line: start.1, pending: Vec::new(), done: false, page_starts }
    }

    /// Rows of a table: the rule above each row, its lines, the bottom rule.
    fn next_table_row(&mut self, c: CellRef) -> Option<Row> {
        let para = self.para;
        let n = row_lines(self.app, para);
        let nrows = self.app.ed.doc.tables.get(&c.table).map(|t| t.rows.len()).unwrap_or(c.row as usize + 1);
        let r = c.row as usize;
        let line = self.line;
        let line_row = Row::TableLine { table: c.table, row: r, para, line };
        let last_line = line + 1 >= n;
        let last_row = r + 1 >= nrows;
        // Advance.
        if last_line {
            self.para = next_row_start(self.app, para);
            self.line = 0;
        } else {
            self.line += 1;
        }
        if last_line && last_row {
            self.pending.push(Row::TableRule { table: c.table, row: nrows });
        }
        if line == 0 {
            self.pending.push(line_row);
            let top = Row::TableRule { table: c.table, row: r };
            // A page beginning at this row: the rule precedes the border.
            let pg = self.page_starts.iter().find(|&&(p, idx, _)| idx == 0 && p >= para && p < next_row_start(self.app, para)).map(|x| x.2);
            if let Some(pg) = pg {
                self.pending.push(top);
                return Some(Row::PageRule(pg));
            }
            return Some(top);
        }
        Some(line_row)
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
        if let Some(c) = self.app.ed.doc.cell_of(self.para) {
            return self.next_table_row(c);
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
    let (cp, cl) = row_coords(app, cp, cl);
    let scroll = row_coords(app, scroll.0, scroll.1);
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
        Row::TableLine { para, line, .. } => Row::Line { para: *para, line: *line },
        other => other.clone(),
    }
}

/// Step the scroll position forward by one text line.
fn scroll_forward(app: &mut App, s: (usize, usize)) -> (usize, usize) {
    if app.ed.doc.cell_of(s.0).is_some() {
        let s = row_coords(app, s.0, s.1);
        let n = row_lines(app, s.0);
        if s.1 + 1 < n {
            return (s.0, s.1 + 1);
        }
        let next = next_row_start(app, s.0);
        return if next < app.ed.doc.paragraphs.len() { (next, 0) } else { s };
    }
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
    let (cp, cl) = row_coords(app, cp, cl);
    if app.scroll.0 >= app.ed.doc.paragraphs.len() {
        app.scroll = (0, 0);
    }
    app.scroll = row_coords(app, app.scroll.0, app.scroll.1);
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
    let s = row_coords(app, s.0, s.1);
    if s.1 > 0 {
        (s.0, s.1 - 1)
    } else if s.0 > 0 {
        let prev = s.0 - 1;
        if app.ed.doc.cell_of(prev).is_some() {
            let (rs, _) = row_coords(app, prev, 0);
            let n = row_lines(app, rs);
            (rs, n.saturating_sub(1))
        } else {
            let n = app.ed.screen_lines(prev).len();
            (prev, n.saturating_sub(1))
        }
    } else {
        s
    }
}

pub fn draw(f: &mut Frame, app: &mut App, caps: Caps) {
    let area = f.area();
    app.size = (area.width, area.height);
    let ch = chrome(caps);
    let th = theme(app, caps);
    if th.classic {
        f.render_widget(Block::default().style(th.base), area);
    }
    let mut y_bottom = area.height;

    // Menu bar (top), pinned or while a menu is open.
    let y_top = app.doc_top();
    if y_top > 0 {
        let open = match &app.overlay {
            Overlay::Menu { menu, .. } => Some(*menu),
            _ => None,
        };
        draw_menu_bar(f, app, Rect::new(0, 0, area.width, 1), &th, open);
    }

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
        let menu = app.keymap.label_for(crate::commands::Cmd::Menu).unwrap_or_else(|| "Alt+=".into());
        let hint = Line::from(vec![Span::styled(format!(" {} for commands · {} for menus · {} for help · Alt+F3 for Reveal Codes · any key dismisses this line", pal, menu, help), Style::default().fg(th.dim))]);
        f.render_widget(RParagraph::new(hint), Rect::new(0, y_bottom, area.width, 1));
    }

    // Reveal Codes pane.
    let mut doc_area = Rect::new(0, y_top, area.width, y_bottom.saturating_sub(y_top));
    if app.reveal && doc_area.height >= 6 {
        let pane_h = doc_area.height * 2 / 5 + 1;
        doc_area.height -= pane_h;
        draw_reveal(f, app, Rect::new(0, y_top + doc_area.height, area.width, pane_h), caps, &ch);
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

/// The document position under a screen cell, if it is in the document area.
pub fn pos_at(app: &mut App, x: u16, y: u16) -> Option<Pos> {
    let rows = app.doc_rows() as usize;
    let y = y.checked_sub(app.doc_top())?;
    if y as usize >= rows {
        return None;
    }
    let scroll = app.scroll;
    let cols = app.ed.cols();
    let mut walker = RowWalker::new(app, scroll);
    let mut row = None;
    for _ in 0..=y {
        row = walker.next_row();
        if row.is_none() {
            break;
        }
    }
    drop(walker);
    match row {
        Some(Row::Line { para, line }) => {
            let pp = app.ed.doc.para_props(para);
            let sl = app.ed.screen_lines(para)[line].clone();
            let width = app.size.0.saturating_sub(2);
            let x0 = 1 + sl.indent + layout::align_offset(&pp, &sl, width);
            let _ = cols;
            let p = &app.ed.doc.paragraphs[para];
            let idx = layout::screen_idx_at_x(p, &pp, &sl, x.saturating_sub(x0));
            Some(Pos::new(para, idx))
        }
        Some(Row::Block { para }) => Some(Pos::new(para, 0)),
        Some(Row::TableLine { table, row, para, line }) => {
            let t = app.ed.doc.tables.get(&table)?.clone();
            let width = app.size.0.saturating_sub(2);
            let grid = app.ed.doc.table_screen_grid(&t, width);
            let paras = app.ed.doc.table_paras(para)?;
            let cells = paras.get(row)?;
            // Which cell is under x?
            let mut cx: u16 = 1 + 1; // left margin + border
            let mut hit: Option<(usize, u16, u16)> = None; // (cell, x0, width)
            for (ci, _) in cells.iter().enumerate() {
                let g = t.grid_col(row, ci);
                let span = t.rows.get(row).and_then(|r| r.cells.get(ci)).map(|c| c.span()).unwrap_or(1);
                let w: u16 = grid.iter().skip(g).take(span).sum::<u16>() + (span as u16).saturating_sub(1);
                if x < cx + w || ci + 1 == cells.len() {
                    hit = Some((ci, cx + 1, w.saturating_sub(2).max(1)));
                    break;
                }
                cx += w + 1;
            }
            let (ci, x0, _w) = hit?;
            // Which paragraph/line of the cell is on this screen line?
            let mut k = 0;
            let mut target: Option<(usize, usize)> = None;
            for &p in &cells[ci] {
                let n = app.ed.screen_lines(p).len();
                if line < k + n {
                    target = Some((p, line - k));
                    break;
                }
                k += n;
            }
            let (p, l) = match target {
                Some(t) => t,
                None => {
                    let p = *cells[ci].last()?;
                    let n = app.ed.screen_lines(p).len();
                    (p, n - 1)
                }
            };
            let pp = app.ed.doc.para_props(p);
            let sl = app.ed.screen_lines(p)[l].clone();
            let para_ref = &app.ed.doc.paragraphs[p];
            let idx = layout::screen_idx_at_x(para_ref, &pp, &sl, x.saturating_sub(x0 + sl.indent));
            Some(Pos::new(p, idx))
        }
        Some(Row::TableRule { .. }) | Some(Row::Gap) | Some(Row::PageRule(_)) | None => {
            // Between paragraphs: land on the nearer line above.
            let mut walker = RowWalker::new(app, scroll);
            let mut last = None;
            for _ in 0..=y {
                match walker.next_row() {
                    Some(Row::Line { para, line }) => last = Some((para, line)),
                    Some(Row::Block { para }) => last = Some((para, 0)),
                    Some(Row::TableLine { para, .. }) => last = Some((para, 0)),
                    Some(_) => {}
                    None => break,
                }
            }
            drop(walker);
            let (para, line) = last?;
            let end = app.ed.screen_lines(para)[line].end;
            Some(Pos::new(para, end))
        }
    }
}

/// Returns the cursor screen position.
fn draw_draft(f: &mut Frame, app: &mut App, area: Rect, caps: Caps, ch: &Chrome) -> Option<(u16, u16)> {

    let rows = area.height as usize;
    if rows == 0 {
        return None;
    }
    let th = theme(app, caps);
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
                lines_out.push(Line::from(Span::styled(s, Style::default().fg(th.dim))));
            }
            Row::Block { para } => {
                let p = &app.ed.doc.paragraphs[*para];
                let label = match p.items.first() {
                    Some(Item::Code(Code::Opaque(o))) => o.label.clone(),
                    _ => "Block".into(),
                };
                let text = format!("[{} — preserved, not editable in this version]", label);
                let mut style = Style::default().fg(th.dim).add_modifier(Modifier::ITALIC);
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
                let (mut spans, cx) = render_screen_line(app, *para, *line, width, caps, sel, cursor);
                spans.insert(0, Span::raw(" ".repeat(left_margin as usize)));
                if let Some(cx) = cx {
                    cursor_xy = Some((area.x + left_margin + cx, y));
                }
                lines_out.push(Line::from(spans));
            }
            Row::TableRule { table, row } => {
                let s = table_rule(app, *table, *row, width, ch);
                lines_out.push(Line::from(vec![Span::raw(" ".repeat(left_margin as usize)), Span::styled(s, Style::default().fg(th.dim))]));
            }
            Row::TableLine { table, row, para, line } => {
                let border = Style::default().fg(th.dim);
                let mut spans: Vec<Span> = vec![Span::raw(" ".repeat(left_margin as usize))];
                let Some(t) = app.ed.doc.tables.get(table).cloned() else { continue };
                let grid = app.ed.doc.table_screen_grid(&t, width);
                let Some(paras) = app.ed.doc.table_paras(*para) else { continue };
                let cells = &paras[*row];
                let header = t.rows.get(*row).map_or(false, |r| r.header);
                spans.push(Span::styled(ch.v.to_string(), border));
                let mut x: u16 = left_margin + 1;
                for (ci, cell) in cells.iter().enumerate() {
                    let g = t.grid_col(*row, ci);
                    let span = t.rows.get(*row).and_then(|r| r.cells.get(ci)).map(|c| c.span()).unwrap_or(1);
                    let w: u16 = grid.iter().skip(g).take(span).sum::<u16>() + (span as u16).saturating_sub(1);
                    let inner = w.saturating_sub(2).max(1);
                    let continued = t.rows.get(*row).and_then(|r| r.cells.get(ci)).map_or(false, |c| c.vmerge == Some(VMerge::Continue));
                    // The paragraph/line of this cell on this screen line.
                    let mut k = 0;
                    let mut target: Option<(usize, usize)> = None;
                    for &p in cell {
                        let n = app.ed.screen_lines(p).len();
                        if *line < k + n {
                            target = Some((p, *line - k));
                            break;
                        }
                        k += n;
                    }
                    let mut used: u16 = 0;
                    spans.push(Span::raw(" "));
                    if let Some((p, l)) = target.filter(|_| !continued) {
                        if app.ed.doc.paragraphs[p].props.raw_block {
                            let label = match app.ed.doc.paragraphs[p].items.first() {
                                Some(Item::Code(Code::Opaque(o))) => o.label.clone(),
                                _ => "Block".into(),
                            };
                            let text: String = format!("[{}]", label).chars().take(inner as usize).collect();
                            used = text.width() as u16;
                            spans.push(Span::styled(text, Style::default().fg(th.dim).add_modifier(Modifier::ITALIC)));
                            if cursor.para == p {
                                cursor_xy = Some((area.x + x + 1, y));
                            }
                        } else {
                            let (mut cs, cx) = render_screen_line(app, p, l, inner, caps, sel, cursor);
                            if header {
                                for sp in cs.iter_mut() {
                                    sp.style = sp.style.add_modifier(Modifier::BOLD);
                                }
                            }
                            used = cs.iter().map(|sp| sp.content.width() as u16).sum();
                            if let Some(cx) = cx {
                                cursor_xy = Some((area.x + x + 1 + cx.min(inner.saturating_sub(1)), y));
                            }
                            spans.extend(cs);
                        }
                    }
                    if used < inner {
                        spans.push(Span::raw(" ".repeat((inner - used) as usize)));
                    }
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(ch.v.to_string(), border));
                    x += w + 1;
                    if x >= left_margin + width {
                        break;
                    }
                }
                lines_out.push(Line::from(clip_spans(spans, area.width as usize)));
            }
        }
    }
    while lines_out.len() < rows {
        lines_out.push(Line::default());
    }
    f.render_widget(RParagraph::new(lines_out), area);
    cursor_xy
}

/// Render screen line `line` of paragraph `para` into spans no wider than
/// `width`, with the list label, alignment and selection applied. Returns
/// the cursor's x offset within the line when the cursor is on it.
fn render_screen_line(app: &mut App, pi: usize, line: usize, width: u16, caps: Caps, sel: Option<Range>, cursor: Pos) -> (Vec<Span<'static>>, Option<u16>) {
    let th = theme(app, caps);
    let pp = app.ed.doc.para_props(pi);
    let sl = app.ed.screen_lines(pi)[line].clone();
    let nlines = app.ed.layout_screen_len(pi);
    let label = if line == 0 { app.ed.list_label(pi) } else { None };
    let cols = app.ed.cols();
    let runs = app.ed.doc.runs(pi);
    let p = &app.ed.doc.paragraphs[pi];
    let mut cursor_x: Option<u16> = None;
    let align_off = layout::align_offset(&pp, &sl, width);
    let mut spans: Vec<Span> = Vec::new();
    let x: u16 = sl.indent + align_off;
    let base_hp = body_size_hp(&app.ed.doc);
    match label {
        Some(l) if !l.text.is_empty() => {
            let first = layout::screen_first_indent(&pp, cols);
            let lx = first + align_off;
            let mut text = l.text.clone();
            if caps.ascii {
                text = text.chars().map(|c| if c.is_ascii() { c } else { '*' }).collect();
            }
            let props = app.ed.doc.base_run_props(pi).merge(&l.run);
            spans.push(Span::raw(" ".repeat(lx.min(x) as usize)));
            spans.push(Span::styled(text.clone(), style_for(&props, base_hp, caps, &th)));
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
        if pos == cursor && cursor_x.is_none() {
            cursor_x = Some(x + rel_x);
        }
        let mut st = style_for(&runs[ri].props, base_hp, caps, &th);
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
    if cursor.para == pi && cursor_x.is_none() {
        let is_last = line + 1 == nlines;
        let in_line = cursor.idx >= sl.start && (cursor.idx < sl.end || (is_last && cursor.idx >= sl.end));
        if in_line {
            let cx = layout::screen_x_of(p, &pp, &sl, cursor.idx);
            cursor_x = Some(x + cx);
        }
    }
    // Drawings/placeholder boxes: show inline marker for opaque drawings on this line.
    for i in sl.start..sl.end {
        if let Item::Code(Code::Opaque(o)) = &p.items[i] {
            if matches!(o.label.as_str(), "Drawing" | "Picture" | "Object") {
                spans.push(Span::styled(format!(" [{}] ", o.label), Style::default().fg(th.dim).add_modifier(Modifier::REVERSED)));
            }
        }
    }
    (clip_spans(spans, width as usize), cursor_x)
}

/// Truncate spans to `width` terminal cells.
fn clip_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for sp in spans {
        let w = sp.content.width();
        if used + w <= width {
            used += w;
            out.push(sp);
            continue;
        }
        let mut text = String::new();
        for c in sp.content.chars() {
            let cw = layout::cell_width(c) as usize;
            if used + cw > width {
                break;
            }
            used += cw;
            text.push(c);
        }
        if !text.is_empty() {
            out.push(Span::styled(text, sp.style));
        }
        break;
    }
    out
}

/// The horizontal rule above table row `row` (or below the last row), with
/// junctions where the cell borders of the rows above and below meet.
fn table_rule(app: &App, table: u32, row: usize, width: u16, ch: &Chrome) -> String {
    let Some(t) = app.ed.doc.tables.get(&table) else { return String::new() };
    let grid = app.ed.doc.table_screen_grid(t, width);
    // x positions of cell borders in a row (relative to the table's left edge).
    let borders = |r: usize| -> Vec<u16> {
        let mut v = vec![0u16];
        let mut x = 0u16;
        let mut g = 0usize;
        if let Some(rr) = t.rows.get(r) {
            for c in &rr.cells {
                for _ in 0..c.span() {
                    x += grid.get(g).copied().unwrap_or(3) + 1;
                    g += 1;
                }
                v.push(x);
            }
        }
        v
    };
    let above: Vec<u16> = if row > 0 { borders(row - 1) } else { Vec::new() };
    let below: Vec<u16> = if row < t.rows.len() { borders(row) } else { Vec::new() };
    let total: u16 = grid.iter().sum::<u16>() + grid.len() as u16;
    let mut s = String::new();
    for x in 0..=total {
        if x as usize >= width as usize {
            break;
        }
        let up = above.contains(&x);
        let down = below.contains(&x);
        let mut bits = 0;
        if up {
            bits |= 1;
        }
        if down {
            bits |= 2;
        }
        if x > 0 {
            bits |= 4;
        }
        if x < total {
            bits |= 8;
        }
        if !up && !down {
            s.push_str(ch.h);
        } else {
            s.push(ch.junction[bits]);
        }
    }
    s
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
    if let Some(c) = app.ed.current_cell() {
        indicators.push(format!("Cell {}", c.name()));
    }
    if let Some(h) = &app.hf_edit {
        let secs = h.main.doc.section_count();
        indicators.insert(0, if secs > 1 { format!("{} ({}) · section {}", h.kind.title(), h.pages.title(), h.section) } else { format!("{} ({})", h.kind.title(), h.pages.title()) });
    } else {
        let (sec, secs) = app.ed.cursor_section();
        if secs > 1 {
            indicators.push(format!("Sec {}/{}", sec, secs));
        }
    }
    // The paragraph style at the cursor, when it is not the default one.
    if let Some(id) = app.ed.doc.paragraphs.get(app.ed.cursor.para).and_then(|p| p.props.style.as_deref()) {
        let name = app.ed.doc.styles.get(id).map(|s| s.name.as_str()).unwrap_or(id);
        let mut cs = name.chars();
        let shown: String = cs.next().map(|c| c.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default();
        indicators.insert(0, shown);
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
    let th = theme(app, caps);
    let line = Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(mid_trunc, th.status_mid),
        Span::raw(" ".repeat(pad)),
        Span::raw(right),
        Span::raw(" "),
    ]);
    f.render_widget(RParagraph::new(line).style(th.status), area);
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

/// Like `truncate`, but keeps the end — a path's own name matters more than the
/// root it hangs off.
fn tail(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars().rev() {
        if out.width() + 2 > w {
            break;
        }
        out.insert(0, c);
    }
    format!("…{}", out)
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
    let th = theme(app, caps);
    f.render_widget(RParagraph::new(Line::from(Span::styled(head, Style::default().fg(th.dim)))), Rect::new(area.x, area.y, area.width, 1));
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let rows = body.height as usize;
    if rows == 0 {
        return;
    }
    let cursor = app.ed.cursor;
    let sel = app.ed.selection();
    let code_style = th.code;
    let soft_style = if caps.colors { Style::default().fg(th.dim).add_modifier(Modifier::REVERSED) } else { Style::default().add_modifier(Modifier::DIM) };

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
        for (ci, (_, label)) in reveal::para_codes_at(&app.ed.doc, pi).iter().enumerate() {
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
    let th = theme(app, caps);
    let boxed = |title: &str| Block::default().borders(Borders::ALL).title(format!(" {} ", title)).border_style(th.border).style(th.popup);
    match overlay {
        Overlay::None => {}
        Overlay::Menu { menu, item } => draw_menu(f, app, area, &th, ch, menu, item),
        Overlay::Palette { input, selected } => {
            let rows = app.palette_rows(&input);
            let w = area.width.saturating_sub(4).min(78).max(30);
            let n = rows.len().min(12) as u16;
            let h = n + 4;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            clear(f, r, &th);
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
            let sep = Line::from(Span::styled(ch.h.repeat(inner.width as usize), Style::default().fg(th.dim)));
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
                    Span::styled(detail, style.fg(th.dim)),
                    Span::styled(" ".repeat(pad), style),
                    Span::styled(format!("{} ", row.key), style.fg(th.key)),
                ]));
            }
            if rows.is_empty() {
                lines.push(Line::from(Span::styled("  No matching command", Style::default().fg(th.dim))));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Prompt { label, input, .. } => {
            let y = area.height.saturating_sub(2);
            let r = Rect::new(0, y, area.width, 1);
            clear(f, r, &th);
            let style = th.prompt;
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
            clear(f, r, &th);
            f.render_widget(boxed(&title), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let prompt = Line::from(vec![Span::styled("filter: ", Style::default().fg(th.dim)), Span::raw(filter.clone())]);
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
                lines.push(Line::from(vec![Span::styled(format!("  {}  ", label), style), Span::styled(detail, style.fg(th.dim)), Span::styled(" ".repeat(pad), style)]));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Browse { dir, entries, selected, filter, all } => {
            let rows = crate::app::browse_rows(&entries, &filter, all);
            let w = area.width.saturating_sub(4).min(96).max(34);
            let n = rows.len().clamp(1, crate::app::BROWSE_ROWS) as u16;
            let h = n + 4;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            clear(f, r, &th);
            let title = format!("Open — {}", dir.display());
            f.render_widget(boxed(&tail(&title, w.saturating_sub(4) as usize)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let prompt = Line::from(vec![Span::styled("name: ", Style::default().fg(th.dim)), Span::raw(filter.clone())]);
            f.render_widget(RParagraph::new(prompt), Rect::new(inner.x, inner.y, inner.width, 1));
            f.set_cursor_position((inner.x + 6 + filter.width() as u16, inner.y));
            let hint = if all { "Enter open · ←→ up/into · Tab complete · Alt+A documents only" } else { "Enter open · ←→ up/into · Tab complete · Alt+A all files" };
            f.render_widget(RParagraph::new(Line::from(Span::styled(truncate(hint, inner.width as usize), Style::default().fg(th.dim)))), Rect::new(inner.x, inner.y + 1, inner.width, 1));
            let sel_i = selected.min(rows.len().saturating_sub(1));
            let first = sel_i.saturating_sub(crate::app::BROWSE_ROWS - 1);
            let mut lines = Vec::new();
            for (i, e) in rows.iter().enumerate().skip(first).take(crate::app::BROWSE_ROWS) {
                let mut style = Style::default();
                if i == sel_i {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let name = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
                let name = truncate(&name, (inner.width as usize).saturating_sub(e.detail.width() + 6));
                let pad = (inner.width as usize).saturating_sub(2 + name.width() + e.detail.width() + 2);
                let name_style = if e.is_dir && caps.colors {
                    style.fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else if !e.is_doc && !e.is_dir {
                    style.fg(th.dim)
                } else {
                    style
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}", name), name_style),
                    Span::styled(" ".repeat(pad), style),
                    Span::styled(format!("{}  ", e.detail), style.fg(th.dim)),
                ]));
            }
            if rows.is_empty() {
                lines.push(Line::from(Span::styled("  Nothing here matches", Style::default().fg(th.dim))));
            }
            f.render_widget(RParagraph::new(lines), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Drive(d) => {
            let rows = d.visible();
            let w = area.width.saturating_sub(4).min(96).max(34);
            let sel_i = d.selected.min(rows.len().saturating_sub(1));
            // Lines carry the row they stand for; the divider stands for none.
            let inner_w = w.saturating_sub(2) as usize;
            let mut lines: Vec<(Option<usize>, Line)> = Vec::new();
            for (i, (e, from_search)) in rows.iter().enumerate() {
                if *from_search && (i == 0 || !rows[i - 1].1) {
                    lines.push((None, Line::from(Span::styled("  — more from Drive —", Style::default().fg(th.dim)))));
                }
                let mut style = Style::default();
                if i == sel_i {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let folder = e.kind != crate::google::DriveKind::Doc;
                let name = if folder { format!("{}/", e.name) } else { e.name.clone() };
                let name = truncate(&name, inner_w.saturating_sub(e.detail.width() + 6));
                let pad = inner_w.saturating_sub(2 + name.width() + e.detail.width() + 2);
                let name_style = if folder && caps.colors { style.fg(Color::Cyan).add_modifier(Modifier::BOLD) } else { style };
                lines.push((Some(i), Line::from(vec![Span::styled(format!("  {}", name), name_style), Span::styled(" ".repeat(pad), style), Span::styled(format!("{}  ", e.detail), style.fg(th.dim))])));
            }
            if lines.is_empty() {
                let text = match (&d.error, d.loading) {
                    (Some(e), _) => format!("  Could not list Drive: {}", e),
                    (None, true) => "  Loading…".to_string(),
                    (None, false) if d.rows.is_empty() && d.mode == crate::app::DriveMode::Recent => "  No Google Docs found".to_string(),
                    (None, false) => "  Nothing here matches".to_string(),
                };
                lines.push((None, Line::from(Span::styled(truncate(&text, inner_w), Style::default().fg(th.dim)))));
            }
            let n = lines.len().clamp(1, crate::app::BROWSE_ROWS) as u16;
            let h = n + 4;
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1.min(area.height.saturating_sub(h)), w, h.min(area.height));
            clear(f, r, &th);
            f.render_widget(boxed(&tail(&d.title(), w.saturating_sub(4) as usize)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let prompt = Line::from(vec![Span::styled("name: ", Style::default().fg(th.dim)), Span::raw(d.filter.clone())]);
            f.render_widget(RParagraph::new(prompt), Rect::new(inner.x, inner.y, inner.width, 1));
            f.set_cursor_position((inner.x + 6 + d.filter.width() as u16, inner.y));
            let hint = match d.mode {
                crate::app::DriveMode::Recent => "Enter open · Tab folders · type to filter (a pause searches Drive) or paste a Docs URL",
                crate::app::DriveMode::Folders => "Enter open · ←→ up/into · Tab recent",
            };
            f.render_widget(RParagraph::new(Line::from(Span::styled(truncate(hint, inner.width as usize), Style::default().fg(th.dim)))), Rect::new(inner.x, inner.y + 1, inner.width, 1));
            let sel_line = lines.iter().position(|(i, _)| *i == Some(sel_i)).unwrap_or(0);
            let first = sel_line.saturating_sub(crate::app::BROWSE_ROWS - 1);
            let shown: Vec<Line> = lines.into_iter().skip(first).take(crate::app::BROWSE_ROWS).map(|(_, l)| l).collect();
            f.render_widget(RParagraph::new(shown), Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2)));
        }
        Overlay::Confirm { question, .. } => {
            let y = area.height.saturating_sub(2);
            let r = Rect::new(0, y, area.width, 1);
            clear(f, r, &th);
            f.render_widget(RParagraph::new(Line::from(truncate(&question, area.width as usize))).style(th.confirm), r);
        }
        Overlay::Help => {
            let lines = help_lines(app);
            let w = area.width.saturating_sub(4).min(84).max(30);
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1, w, h);
            clear(f, r, &th);
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
            clear(f, r, &th);
            let title = format!("Replace {} → “{}” — {} match{}", app.find.build(&find).describe(), with, matches.len(), if matches.len() == 1 { "" } else { "es" });
            f.render_widget(boxed(&truncate(&title, w as usize - 4)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            let hint = Line::from(Span::styled("Enter/A: replace all   O: one at a time   ↑↓: preview in place   Esc: cancel", Style::default().fg(th.dim)));
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
            clear(f, r, &th);
            let style = th.prompt;
            let text = format!("Replace this one with “{}”?  y = yes  n = skip  a = all remaining  Esc = stop   ({} of {} replaced)", with, done, total);
            f.render_widget(RParagraph::new(Line::from(truncate(&text, area.width as usize))).style(style), r);
        }
        Overlay::Message { title, lines } => {
            let w = area.width.saturating_sub(4).min(80).max(30);

            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(w)) / 2;
            let r = Rect::new(x, 1, w, h);
            clear(f, r, &th);
            f.render_widget(boxed(&format!("{} — any key closes", title)), r);
            let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
            f.render_widget(RParagraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()), inner);
        }
    }
}

/// A command's title as it reads inside its menu: the menu name is already
/// the context, so "Table: Insert…" becomes "Insert…".
fn menu_label(cmd: crate::commands::Cmd) -> String {
    let t = crate::commands::info(cmd).title;
    let t = t.strip_prefix("Table: ").or_else(|| t.strip_prefix("Style: ")).or_else(|| t.strip_prefix("Find Option: ")).unwrap_or(t);
    let t = t.split(" — ").next().unwrap_or(t);
    let t = t.split(" (").next().unwrap_or(t);
    t.to_string()
}

/// Width of a menu's drop-down, borders included.
pub fn menu_width(app: &App, menu: usize) -> u16 {
    let mut w = menu::title_width(&MENUS[menu]) as usize + 4;
    for it in MENUS[menu].items {
        if let MenuItem::Cmd(c) = it {
            let key = app.keymap.label_for(*c).unwrap_or_default();
            w = w.max(2 + menu_label(*c).width() + 3 + key.width() + 1);
        }
    }
    (w + 2).min(app.size.0 as usize).max(8) as u16
}

/// The drop-down's frame: (x, width, first visible item, visible item count).
/// A menu taller than the screen scrolls so the selected item stays visible.
pub fn menu_frame(app: &App, menu: usize, item: usize) -> (u16, u16, usize, usize) {
    let items = MENUS[menu].items;
    let w = menu_width(app, menu);
    let (mut x0, _) = menu::title_span(menu);
    if x0 + w > app.size.0 {
        x0 = app.size.0.saturating_sub(w);
    }
    let avail = (app.size.1.saturating_sub(1 + 2) as usize).min(items.len());
    let first = if item >= avail { item + 1 - avail } else { 0 };
    (x0, w, first, avail)
}

fn draw_menu_bar(f: &mut Frame, _app: &App, area: Rect, th: &Theme, open: Option<usize>) {
    let mut spans: Vec<Span> = vec![Span::styled(" ", th.bar)];
    for (i, m) in MENUS.iter().enumerate() {
        let st = if open == Some(i) { th.bar_open } else { th.bar };
        spans.push(Span::styled(" ", st));
        let mut marked = false;
        for c in m.title.chars() {
            if !marked && c.to_ascii_uppercase() == m.mnemonic {
                let ms = if open == Some(i) { st.add_modifier(Modifier::BOLD) } else { st.patch(th.mnemonic) };
                spans.push(Span::styled(c.to_string(), ms));
                marked = true;
            } else {
                spans.push(Span::styled(c.to_string(), st));
            }
        }
        spans.push(Span::styled(" ", st));
    }
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let help = format!("{}=Help ", _app.keymap.label_for(crate::commands::Cmd::Help).unwrap_or_else(|| "F1".into()));
    let pad = (area.width as usize).saturating_sub(used);
    if help.width() + 2 <= pad {
        spans.push(Span::styled(" ".repeat(pad - help.width()), th.bar));
        spans.push(Span::styled(help, th.bar));
    } else {
        spans.push(Span::styled(" ".repeat(pad), th.bar));
    }
    f.render_widget(RParagraph::new(Line::from(clip_spans(spans, area.width as usize))), area);
}

fn draw_menu(f: &mut Frame, app: &mut App, area: Rect, th: &Theme, ch: &Chrome, menu: usize, item: usize) {
    let items = MENUS[menu].items;
    let (x0, w, first, shown) = menu_frame(app, menu, item);
    let h = shown as u16 + 2;
    if shown == 0 || area.width < w {
        return;
    }
    let r = Rect::new(x0, 1, w, h);
    clear(f, r, th);
    f.render_widget(Block::default().borders(Borders::ALL).border_style(th.border).style(th.menu), r);
    let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
    let mut lines: Vec<Line> = Vec::new();
    for (i, it) in items.iter().enumerate().skip(first).take(shown) {
        let y = inner.y + (i - first) as u16;
        match it {
            MenuItem::Sep => {
                // Extend the rule into the borders.
                let rule = format!("{}{}{}", ch.junction[0b1011], ch.h.repeat(inner.width as usize), ch.junction[0b0111]);
                f.render_widget(RParagraph::new(Line::from(Span::styled(rule, th.border))), Rect::new(r.x, y, r.width, 1));
                lines.push(Line::default());
            }
            MenuItem::Cmd(c) => {
                let key = app.keymap.label_for(*c).unwrap_or_default();
                let label = truncate(&menu_label(*c), (inner.width as usize).saturating_sub(key.width() + 5));
                let pad = (inner.width as usize).saturating_sub(2 + label.width() + key.width() + 1);
                let st = if i == item { th.menu_sel } else { th.menu };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}", label), st),
                    Span::styled(" ".repeat(pad), st),
                    Span::styled(format!("{} ", key), if i == item { st } else { st.fg(th.key) }),
                ]));
            }
        }
    }
    // Draw item rows one at a time so separators (already painted) are left alone.
    for (k, line) in lines.into_iter().enumerate() {
        if matches!(items[first + k], MenuItem::Sep) {
            continue;
        }
        f.render_widget(RParagraph::new(line), Rect::new(inner.x, inner.y + k as u16, inner.width, 1));
    }
    // More above / below: mark the border.
    if first > 0 {
        f.render_widget(RParagraph::new(Line::from(Span::styled(if ch.h == "-" { "^" } else { "▲" }, th.border))), Rect::new(r.x + r.width - 2, r.y, 1, 1));
    }
    if first + shown < items.len() {
        f.render_widget(RParagraph::new(Line::from(Span::styled(if ch.h == "-" { "v" } else { "▼" }, th.border))), Rect::new(r.x + r.width - 2, r.y + r.height - 1, 1, 1));
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
        "  Open dialog: type to filter, Tab completes, ←/→ leave/enter a folder,".into(),
        "  type a path with / to jump, Alt+A lists every file, not just documents.".into(),
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

