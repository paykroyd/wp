//! Reveal Codes: labels for codes and for paragraph properties shown as codes.

use crate::model::*;

fn twips_str(t: Twips) -> String {
    let inches = t as f64 / TWIPS_PER_INCH as f64;
    let s = format!("{:.2}", inches);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{}\"", if s.is_empty() || s == "-" { "0" } else { s })
}

fn pt_str(t: Twips) -> String {
    let pt = t as f64 / TWIPS_PER_POINT as f64;
    let s = format!("{:.1}", pt);
    format!("{}pt", s.trim_end_matches(".0"))
}

pub fn attr_label(a: &Attr) -> String {
    match a {
        Attr::Bold(true) => "BOLD".into(),
        Attr::Bold(false) => "Bold Off".into(),
        Attr::Italic(true) => "ITALC".into(),
        Attr::Italic(false) => "Italic Off".into(),
        Attr::Underline(u) => match u {
            Underline::None => "Und Off".into(),
            Underline::Single => "UND".into(),
            Underline::Double => "DBL UND".into(),
            Underline::Words => "UND:Words".into(),
            Underline::Dotted => "UND:Dotted".into(),
            Underline::Dash => "UND:Dash".into(),
            Underline::Wave => "UND:Wave".into(),
            Underline::Thick => "UND:Thick".into(),
        },
        Attr::Strike(true) => "STKOUT".into(),
        Attr::Strike(false) => "Stkout Off".into(),
        Attr::DoubleStrike(true) => "DBL STKOUT".into(),
        Attr::DoubleStrike(false) => "Dbl Stkout Off".into(),
        Attr::VertAlign(VertAlign::Superscript) => "SUPRSCPT".into(),
        Attr::VertAlign(VertAlign::Subscript) => "SUBSCPT".into(),
        Attr::VertAlign(VertAlign::Baseline) => "Baseline".into(),
        Attr::SmallCaps(true) => "SM CAP".into(),
        Attr::SmallCaps(false) => "Sm Cap Off".into(),
        Attr::AllCaps(true) => "CAPS".into(),
        Attr::AllCaps(false) => "Caps Off".into(),
        Attr::Font(f) => format!("Font:{}", f),
        Attr::Size(s) => format!("Size:{}pt", half_pt(*s)),
        Attr::Color(c) => format!("Color:#{}", c.hex()),
        Attr::Highlight(h) => format!("Hilite:{}", h.docx_name()),
        Attr::CharStyle(s) => format!("Char Style:{}", s),
        Attr::Raw(_) => "Run Props".into(),
        Attr::RunAttrs(_) => "Run Attrs".into(),
    }
}

fn half_pt(hp: u16) -> String {
    if hp % 2 == 0 {
        format!("{}", hp / 2)
    } else {
        format!("{}.5", hp / 2)
    }
}

pub fn kind_label(k: AttrKind) -> &'static str {
    match k {
        AttrKind::Bold => "bold",
        AttrKind::Italic => "italc",
        AttrKind::Underline => "und",
        AttrKind::Strike => "stkout",
        AttrKind::DoubleStrike => "dbl stkout",
        AttrKind::VertAlign => "suprscpt/subscpt",
        AttrKind::SmallCaps => "sm cap",
        AttrKind::AllCaps => "caps",
        AttrKind::Font => "font",
        AttrKind::Size => "size",
        AttrKind::Color => "color",
        AttrKind::Highlight => "hilite",
        AttrKind::CharStyle => "char style",
        AttrKind::Raw => "run props",
        AttrKind::RunAttrs => "run attrs",

    }
}

/// The `[...]` label for a code.
pub fn code_label(c: &Code) -> String {
    match c {
        Code::On(a) => format!("[{}]", attr_label(a)),
        Code::Off(k) => format!("[{}]", kind_label(*k)),
        Code::Tab => "[Tab]".into(),
        Code::LineBreak => "[Ln Brk]".into(),
        Code::PageBreak => "[HPg]".into(),
        Code::Bookmark(n) => format!("[Bookmark:{}]", n),
        Code::BookmarkEnd(n) => format!("[bookmark:{}]", n),
        Code::Opaque(o) => format!("[{}]", o.label),
    }
}

/// Paragraph properties that are displayed (and deletable) as codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParaCode {
    Style,
    Align,
    IndentLeft,
    IndentRight,
    FirstLine,
    Hanging,
    SpaceBefore,
    SpaceAfter,
    LineSpacing,
    KeepNext,
    KeepLines,
    WidowControl,
    PageBreakBefore,
    Tabs,
    Borders,
    Shading,
    List,
    OutlineLevel,
    SectBreak,
    Mark,
    Opaque,
    RawBlock,
}

/// The codes to display at the start of a paragraph for its *direct*
/// properties.
pub fn para_codes(p: &ParaProps) -> Vec<(ParaCode, String)> {
    let mut v = Vec::new();
    if let Some(s) = &p.style {
        v.push((ParaCode::Style, format!("[Style:{}]", s)));
    }
    if let Some(a) = p.align {
        let n = match a {
            Align::Left => "Left",
            Align::Center => "Center",
            Align::Right => "Right",
            Align::Justify => "Full",
        };
        v.push((ParaCode::Align, format!("[Just:{}]", n)));
    }
    if let Some(t) = p.indent_left {
        v.push((ParaCode::IndentLeft, format!("[L Ind:{}]", twips_str(t))));
    }
    if let Some(t) = p.indent_right {
        v.push((ParaCode::IndentRight, format!("[R Ind:{}]", twips_str(t))));
    }
    if let Some(t) = p.first_line {
        v.push((ParaCode::FirstLine, format!("[First Ln:{}]", twips_str(t))));
    }
    if let Some(t) = p.hanging {
        v.push((ParaCode::Hanging, format!("[Hanging:{}]", twips_str(t))));
    }
    if let Some(t) = p.space_before {
        v.push((ParaCode::SpaceBefore, format!("[Sp Before:{}]", pt_str(t))));
    }
    if let Some(t) = p.space_after {
        v.push((ParaCode::SpaceAfter, format!("[Sp After:{}]", pt_str(t))));
    }
    if let Some(ls) = p.line_spacing {
        let s = match ls {
            LineSpacing::Auto(m) => {
                let x = m as f64 / 240.0;
                let s = format!("{:.2}", x);
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
            LineSpacing::Exact(t) => format!("Exactly {}", pt_str(t)),
            LineSpacing::AtLeast(t) => format!("At least {}", pt_str(t)),
        };
        v.push((ParaCode::LineSpacing, format!("[Ln Spacing:{}]", s)));
    }
    if let Some(b) = p.keep_next {
        v.push((ParaCode::KeepNext, format!("[Keep w/Next:{}]", onoff(b))));
    }
    if let Some(b) = p.keep_lines {
        v.push((ParaCode::KeepLines, format!("[Keep Lines:{}]", onoff(b))));
    }
    if let Some(b) = p.widow_control {
        v.push((ParaCode::WidowControl, format!("[W/O:{}]", onoff(b))));
    }
    if let Some(b) = p.page_break_before {
        if b {
            v.push((ParaCode::PageBreakBefore, "[Pg Brk Before]".into()));
        }
    }
    if !p.tabs.is_empty() {
        let s: Vec<String> = p.tabs.iter().map(|t| twips_str(t.pos)).collect();
        v.push((ParaCode::Tabs, format!("[Tab Set:{}]", s.join(","))));
    }
    if p.borders.is_some() {
        v.push((ParaCode::Borders, "[Par Border]".into()));
    }
    if let Some(c) = p.shading {
        v.push((ParaCode::Shading, format!("[Par Shade:#{}]", c.hex())));
    }
    if let Some(l) = p.list {
        v.push((ParaCode::List, format!("[List:{} Lvl {}]", l.num_id, l.level + 1)));
    }
    if let Some(l) = p.outline_level {
        v.push((ParaCode::OutlineLevel, format!("[Outline Lvl:{}]", l + 1)));
    }
    if p.sect_break.is_some() {
        v.push((ParaCode::SectBreak, "[Sect Brk]".into()));
    }
    if !p.opaque.is_empty() {
        v.push((ParaCode::Opaque, format!("[Par Props:{} preserved]", p.opaque.len())));
    }
    if p.raw_block {
        v.push((ParaCode::RawBlock, "[Block]".into()));
    }
    v
}

fn onoff(b: bool) -> &'static str {
    if b {
        "On"
    } else {
        "Off"
    }
}

/// Remove a paragraph property, as when its code is deleted in Reveal Codes.
pub fn clear_para_code(p: &mut ParaProps, which: ParaCode) {
    match which {
        ParaCode::Style => p.style = None,
        ParaCode::Align => p.align = None,
        ParaCode::IndentLeft => p.indent_left = None,
        ParaCode::IndentRight => p.indent_right = None,
        ParaCode::FirstLine => p.first_line = None,
        ParaCode::Hanging => p.hanging = None,
        ParaCode::SpaceBefore => p.space_before = None,
        ParaCode::SpaceAfter => p.space_after = None,
        ParaCode::LineSpacing => p.line_spacing = None,
        ParaCode::KeepNext => p.keep_next = None,
        ParaCode::KeepLines => p.keep_lines = None,
        ParaCode::WidowControl => p.widow_control = None,
        ParaCode::PageBreakBefore => p.page_break_before = None,
        ParaCode::Tabs => p.tabs.clear(),
        ParaCode::Borders => p.borders = None,
        ParaCode::Shading => p.shading = None,
        ParaCode::List => p.list = None,
        ParaCode::OutlineLevel => p.outline_level = None,
        ParaCode::SectBreak => p.sect_break = None,
        ParaCode::Mark => p.mark = RunProps::default(),
        ParaCode::Opaque => p.opaque.clear(),
        ParaCode::RawBlock => {}
    }
    p.raw_ppr = None;
}
