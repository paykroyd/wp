//! The document model: paragraphs of items, where an item is a character or a
//! formatting code. See DESIGN.md §3.

use serde::{Deserialize, Serialize};
use std::fmt;

/// 1/20 of a point. The native unit of `.docx` and of everything geometric here.
pub type Twips = i32;

pub const TWIPS_PER_INCH: Twips = 1440;
pub const TWIPS_PER_POINT: Twips = 20;

/// RGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
    pub fn parse_hex(s: &str) -> Option<Rgb> {
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Underline {
    /// Explicitly no underline (overrides a style).
    None,
    Single,
    Double,
    Words,
    Dotted,
    Dash,
    Wave,
    Thick,
}

impl Underline {
    pub fn docx_name(self) -> &'static str {
        match self {
            Underline::None => "none",
            Underline::Single => "single",
            Underline::Double => "double",
            Underline::Words => "words",
            Underline::Dotted => "dotted",
            Underline::Dash => "dash",
            Underline::Wave => "wave",
            Underline::Thick => "thick",
        }
    }
    pub fn from_docx(s: &str) -> Underline {
        match s {
            "none" => Underline::None,
            "double" => Underline::Double,
            "words" => Underline::Words,
            "dotted" | "dottedHeavy" => Underline::Dotted,
            "dash" | "dashLong" | "dashDotHeavy" | "dotDash" | "dotDotDash" => Underline::Dash,
            "wave" | "wavyDouble" | "wavyHeavy" => Underline::Wave,
            "thick" => Underline::Thick,
            _ => Underline::Single,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VertAlign {
    Baseline,
    Superscript,
    Subscript,
}

/// Highlight colours are a closed set in `.docx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Highlight {
    None,
    Yellow,
    Green,
    Cyan,
    Magenta,
    Blue,
    Red,
    DarkBlue,
    DarkCyan,
    DarkGreen,
    DarkMagenta,
    DarkRed,
    DarkYellow,
    DarkGray,
    LightGray,
    Black,
    White,
}

impl Highlight {
    pub fn docx_name(self) -> &'static str {
        match self {
            Highlight::None => "none",
            Highlight::Yellow => "yellow",
            Highlight::Green => "green",
            Highlight::Cyan => "cyan",
            Highlight::Magenta => "magenta",
            Highlight::Blue => "blue",
            Highlight::Red => "red",
            Highlight::DarkBlue => "darkBlue",
            Highlight::DarkCyan => "darkCyan",
            Highlight::DarkGreen => "darkGreen",
            Highlight::DarkMagenta => "darkMagenta",
            Highlight::DarkRed => "darkRed",
            Highlight::DarkYellow => "darkYellow",
            Highlight::DarkGray => "darkGray",
            Highlight::LightGray => "lightGray",
            Highlight::Black => "black",
            Highlight::White => "white",
        }
    }
    pub fn from_docx(s: &str) -> Option<Highlight> {
        Some(match s {
            "none" => Highlight::None,
            "yellow" => Highlight::Yellow,
            "green" => Highlight::Green,
            "cyan" => Highlight::Cyan,
            "magenta" => Highlight::Magenta,
            "blue" => Highlight::Blue,
            "red" => Highlight::Red,
            "darkBlue" => Highlight::DarkBlue,
            "darkCyan" => Highlight::DarkCyan,
            "darkGreen" => Highlight::DarkGreen,
            "darkMagenta" => Highlight::DarkMagenta,
            "darkRed" => Highlight::DarkRed,
            "darkYellow" => Highlight::DarkYellow,
            "darkGray" => Highlight::DarkGray,
            "lightGray" => Highlight::LightGray,
            "black" => Highlight::Black,
            "white" => Highlight::White,
            _ => return None,
        })
    }
    pub fn all() -> &'static [Highlight] {
        use Highlight::*;
        &[
            Yellow, Green, Cyan, Magenta, Blue, Red, DarkBlue, DarkCyan, DarkGreen, DarkMagenta,
            DarkRed, DarkYellow, DarkGray, LightGray, Black, White,
        ]
    }
}

/// The kind of a character attribute, independent of its value. Used to match
/// an `Off` code with its `On`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AttrKind {
    Bold,
    Italic,
    Underline,
    Strike,
    DoubleStrike,
    VertAlign,
    SmallCaps,
    AllCaps,
    Font,
    Size,
    Color,
    Highlight,
    CharStyle,
    /// Preserved run properties XML that wp does not model.
    Raw,
}

impl AttrKind {
    pub fn all() -> &'static [AttrKind] {
        use AttrKind::*;
        &[
            Bold, Italic, Underline, Strike, DoubleStrike, VertAlign, SmallCaps, AllCaps, Font,
            Size, Color, Highlight, CharStyle, Raw,
        ]
    }
    /// Kinds shown in Reveal Codes by default (Raw is hidden unless asked).
    pub fn is_visible(self) -> bool {
        self != AttrKind::Raw
    }
}

/// A character attribute with its value. Appears in the stream as `Code::On(attr)`
/// and is closed by `Code::Off(attr.kind())`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Attr {
    Bold(bool),
    Italic(bool),
    Underline(Underline),
    Strike(bool),
    DoubleStrike(bool),
    VertAlign(VertAlign),
    SmallCaps(bool),
    AllCaps(bool),
    Font(String),
    /// Half-points, as in `w:sz`.
    Size(u16),
    Color(Rgb),
    Highlight(Highlight),
    CharStyle(String),
    /// The complete `w:rPr` element of a run as read from a file. Carried
    /// alongside the modelled attributes so unmodelled properties survive.
    Raw(String),
}

impl Attr {
    pub fn kind(&self) -> AttrKind {
        match self {
            Attr::Bold(_) => AttrKind::Bold,
            Attr::Italic(_) => AttrKind::Italic,
            Attr::Underline(_) => AttrKind::Underline,
            Attr::Strike(_) => AttrKind::Strike,
            Attr::DoubleStrike(_) => AttrKind::DoubleStrike,
            Attr::VertAlign(_) => AttrKind::VertAlign,
            Attr::SmallCaps(_) => AttrKind::SmallCaps,
            Attr::AllCaps(_) => AttrKind::AllCaps,
            Attr::Font(_) => AttrKind::Font,
            Attr::Size(_) => AttrKind::Size,
            Attr::Color(_) => AttrKind::Color,
            Attr::Highlight(_) => AttrKind::Highlight,
            Attr::CharStyle(_) => AttrKind::CharStyle,
            Attr::Raw(_) => AttrKind::Raw,
        }
    }
}

/// XML we preserve but do not interpret. Stored as the exact serialized
/// element(s) so the writer can emit them verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpaqueKind {
    /// A complete element (a drawing, a field char, a footnote reference).
    Element,
    /// The opening half of a wrapper around runs (hyperlink, tracked change).
    Open(u32),
    /// The closing half; paired with the `Open` of the same id.
    Close(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpaqueXml {
    pub xml: String,
    /// Short label for Reveal Codes / placeholders, e.g. "Drawing", "Field".
    pub label: String,
    pub kind: OpaqueKind,
    /// Editing inside this wrapper is refused (tracked changes).
    pub protected: bool,
    /// Text inside is a tracked deletion (`w:delText` on write).
    pub deleted: bool,
}

impl OpaqueXml {
    pub fn element(xml: impl Into<String>, label: impl Into<String>) -> OpaqueXml {
        OpaqueXml { xml: xml.into(), label: label.into(), kind: OpaqueKind::Element, protected: false, deleted: false }
    }
}

/// A formatting code in the item stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Code {
    On(Attr),
    Off(AttrKind),
    Tab,
    /// `[Ln Brk]` — a line break within a paragraph.
    LineBreak,
    /// `[HPg]` — a hard page break.
    PageBreak,
    Bookmark(String),
    BookmarkEnd(String),
    /// Run-level XML that wp does not understand. Zero width in layout.
    Opaque(OpaqueXml),
}

impl Code {
    /// True for codes that are invisible/zero-width in ordinary editing.
    pub fn is_zero_width(&self) -> bool {
        !matches!(self, Code::Tab | Code::LineBreak | Code::PageBreak)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Item {
    Char(char),
    Code(Code),
}

impl Item {
    pub fn as_char(&self) -> Option<char> {
        match self {
            Item::Char(c) => Some(*c),
            _ => None,
        }
    }
    pub fn is_code(&self) -> bool {
        matches!(self, Item::Code(_))
    }
    pub fn code(&self) -> Option<&Code> {
        match self {
            Item::Code(c) => Some(c),
            _ => None,
        }
    }
    pub fn is_whitespace(&self) -> bool {
        matches!(self, Item::Char(c) if c.is_whitespace()) || matches!(self, Item::Code(Code::Tab))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub fn docx_name(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        }
    }
    pub fn from_docx(s: &str) -> Align {
        match s {
            "center" => Align::Center,
            "right" | "end" => Align::Right,
            "both" | "distribute" => Align::Justify,
            _ => Align::Left,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineSpacing {
    /// Multiple of single spacing, in 240ths (240 = single, 360 = 1.5, 480 = double).
    Auto(i32),
    Exact(Twips),
    AtLeast(Twips),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TabKind {
    #[default]
    Left,
    Center,
    Right,
    Decimal,
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TabLeader {
    #[default]
    None,
    Dot,
    Hyphen,
    Underscore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabStop {
    pub pos: Twips,
    pub kind: TabKind,
    pub leader: TabLeader,
    /// A `clear` tab removes an inherited stop at this position.
    pub clear: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BorderStyle {
    #[default]
    Single,
    Double,
    Dotted,
    Dashed,
    Thick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Border {
    pub style: BorderStyle,
    /// Eighths of a point, as in `w:sz`.
    pub size: u16,
    pub color: Option<Rgb>,
    pub space: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ParaBorders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
}

/// Run (character) properties. Every field is optional so the same struct
/// serves as a style definition (where unset means "inherit") and, once
/// resolved, as the effective properties of a run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct RunProps {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<Option<Underline>>,
    pub strike: Option<bool>,
    pub dstrike: Option<bool>,
    pub vert_align: Option<Option<VertAlign>>,
    pub small_caps: Option<bool>,
    pub all_caps: Option<bool>,
    pub font: Option<String>,
    pub size: Option<u16>,
    pub color: Option<Rgb>,
    pub highlight: Option<Option<Highlight>>,
    pub char_style: Option<String>,
    /// Unknown `w:rPr` children, preserved verbatim.
    pub opaque: Vec<String>,
}

impl RunProps {
    /// Overlay `other` on top of `self`: any field set in `other` wins.
    pub fn merge(&self, other: &RunProps) -> RunProps {
        RunProps {
            bold: other.bold.or(self.bold),
            italic: other.italic.or(self.italic),
            underline: other.underline.or(self.underline),
            strike: other.strike.or(self.strike),
            dstrike: other.dstrike.or(self.dstrike),
            vert_align: other.vert_align.or(self.vert_align),
            small_caps: other.small_caps.or(self.small_caps),
            all_caps: other.all_caps.or(self.all_caps),
            font: other.font.clone().or_else(|| self.font.clone()),
            size: other.size.or(self.size),
            color: other.color.or(self.color),
            highlight: other.highlight.or(self.highlight),
            char_style: other.char_style.clone().or_else(|| self.char_style.clone()),
            opaque: {
                let mut v = self.opaque.clone();
                v.extend(other.opaque.iter().cloned());
                v
            },
        }
    }

    /// Apply a single attribute on top of these props.
    pub fn apply(&mut self, attr: &Attr) {
        match attr {
            Attr::Bold(b) => self.bold = Some(*b),
            Attr::Italic(b) => self.italic = Some(*b),
            Attr::Underline(u) => self.underline = Some(if *u == Underline::None { None } else { Some(*u) }),
            Attr::Strike(b) => self.strike = Some(*b),
            Attr::DoubleStrike(b) => self.dstrike = Some(*b),
            Attr::VertAlign(v) => self.vert_align = Some(if *v == VertAlign::Baseline { None } else { Some(*v) }),
            Attr::SmallCaps(b) => self.small_caps = Some(*b),
            Attr::AllCaps(b) => self.all_caps = Some(*b),
            Attr::Font(f) => self.font = Some(f.clone()),
            Attr::Size(s) => self.size = Some(*s),
            Attr::Color(c) => self.color = Some(*c),
            Attr::Highlight(h) => self.highlight = Some(if *h == Highlight::None { None } else { Some(*h) }),
            Attr::CharStyle(s) => self.char_style = Some(s.clone()),
            Attr::Raw(_) => {}
        }
    }

    pub fn is_bold(&self) -> bool {
        self.bold.unwrap_or(false)
    }
    pub fn is_italic(&self) -> bool {
        self.italic.unwrap_or(false)
    }
    pub fn underline(&self) -> Option<Underline> {
        self.underline.flatten()
    }
    pub fn is_strike(&self) -> bool {
        self.strike.unwrap_or(false) || self.dstrike.unwrap_or(false)
    }
    pub fn vert_align(&self) -> Option<VertAlign> {
        self.vert_align.flatten()
    }
    pub fn highlight(&self) -> Option<Highlight> {
        self.highlight.flatten()
    }
    /// Size in half-points, defaulting to 11pt (Word's default).
    pub fn size_hp(&self) -> u16 {
        self.size.unwrap_or(22)
    }
    pub fn is_empty(&self) -> bool {
        *self == RunProps::default()
    }
}

/// List membership: numbering definition id and level (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListRef {
    pub num_id: i32,
    pub level: u8,
}

/// Paragraph properties. Optional fields inherit from the style chain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ParaProps {
    pub style: Option<String>,
    pub align: Option<Align>,
    pub indent_left: Option<Twips>,
    pub indent_right: Option<Twips>,
    pub first_line: Option<Twips>,
    pub hanging: Option<Twips>,
    pub space_before: Option<Twips>,
    pub space_after: Option<Twips>,
    pub line_spacing: Option<LineSpacing>,
    pub keep_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub widow_control: Option<bool>,
    pub page_break_before: Option<bool>,
    pub tabs: Vec<TabStop>,
    pub borders: Option<ParaBorders>,
    pub shading: Option<Rgb>,
    pub list: Option<ListRef>,
    /// Outline level (0 = Heading 1) when set directly.
    pub outline_level: Option<u8>,
    /// Properties of the paragraph mark (`w:pPr/w:rPr`), preserved.
    pub mark: RunProps,
    /// Unknown `w:pPr` children, preserved verbatim.
    pub opaque: Vec<String>,
    /// A section break attached to this paragraph (`w:pPr/w:sectPr`), verbatim.
    pub sect_break: Option<String>,
    /// The complete `w:pPr` as read, kept until the properties are changed.
    pub raw_ppr: Option<String>,
    /// This "paragraph" is a verbatim body-level block (table, content
    /// control, …) held in a single `Opaque` item. Not editable.
    pub raw_block: bool,
}

impl ParaProps {
    pub fn merge(&self, other: &ParaProps) -> ParaProps {
        let mut tabs = self.tabs.clone();
        for t in &other.tabs {
            tabs.retain(|x| x.pos != t.pos);
            if !t.clear {
                tabs.push(*t);
            }
        }
        tabs.sort_by_key(|t| t.pos);
        ParaProps {
            style: other.style.clone().or_else(|| self.style.clone()),
            align: other.align.or(self.align),
            indent_left: other.indent_left.or(self.indent_left),
            indent_right: other.indent_right.or(self.indent_right),
            first_line: other.first_line.or(self.first_line),
            hanging: other.hanging.or(self.hanging),
            space_before: other.space_before.or(self.space_before),
            space_after: other.space_after.or(self.space_after),
            line_spacing: other.line_spacing.or(self.line_spacing),
            keep_next: other.keep_next.or(self.keep_next),
            keep_lines: other.keep_lines.or(self.keep_lines),
            widow_control: other.widow_control.or(self.widow_control),
            page_break_before: other.page_break_before.or(self.page_break_before),
            tabs,
            borders: other.borders.clone().or_else(|| self.borders.clone()),
            shading: other.shading.or(self.shading),
            list: other.list.or(self.list),
            outline_level: other.outline_level.or(self.outline_level),
            mark: self.mark.merge(&other.mark),
            opaque: {
                let mut v = self.opaque.clone();
                v.extend(other.opaque.iter().cloned());
                v
            },
            sect_break: other.sect_break.clone().or_else(|| self.sect_break.clone()),
            raw_ppr: other.raw_ppr.clone(),
            raw_block: other.raw_block,
        }
    }

    /// Mark direct formatting as changed so preserved XML is regenerated.
    pub fn touch(&mut self) {
        self.raw_ppr = None;
    }

    pub fn align(&self) -> Align {
        self.align.unwrap_or_default()
    }
    pub fn indent_left(&self) -> Twips {
        self.indent_left.unwrap_or(0)
    }
    pub fn indent_right(&self) -> Twips {
        self.indent_right.unwrap_or(0)
    }
    /// Net first-line offset relative to the left indent (positive = indent,
    /// negative = hanging).
    pub fn first_line_offset(&self) -> Twips {
        if let Some(h) = self.hanging {
            -h
        } else {
            self.first_line.unwrap_or(0)
        }
    }
    pub fn space_before(&self) -> Twips {
        self.space_before.unwrap_or(0)
    }
    pub fn space_after(&self) -> Twips {
        self.space_after.unwrap_or(0)
    }
    pub fn line_spacing(&self) -> LineSpacing {
        self.line_spacing.unwrap_or(LineSpacing::Auto(240))
    }
    pub fn keep_next(&self) -> bool {
        self.keep_next.unwrap_or(false)
    }
    pub fn keep_lines(&self) -> bool {
        self.keep_lines.unwrap_or(false)
    }
    pub fn widow_control(&self) -> bool {
        self.widow_control.unwrap_or(true)
    }
    pub fn page_break_before(&self) -> bool {
        self.page_break_before.unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Page geometry for a section. v0.1 supports one section per document.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SectionProps {
    pub page_width: Twips,
    pub page_height: Twips,
    pub margin_top: Twips,
    pub margin_bottom: Twips,
    pub margin_left: Twips,
    pub margin_right: Twips,
    pub header_distance: Twips,
    pub footer_distance: Twips,
    pub gutter: Twips,
    pub orientation: Orientation,
    pub columns: u16,
    /// The full `w:sectPr` element as read, for verbatim re-emission of the
    /// parts we don't model (headers/footers refs, line numbering, etc.).
    pub opaque_children: Vec<String>,
}

impl Default for SectionProps {
    /// US Letter, 1" margins — Word's default.
    fn default() -> Self {
        SectionProps {
            page_width: 12240,
            page_height: 15840,
            margin_top: 1440,
            margin_bottom: 1440,
            margin_left: 1440,
            margin_right: 1440,
            header_distance: 720,
            footer_distance: 720,
            gutter: 0,
            orientation: Orientation::Portrait,
            columns: 1,
            opaque_children: Vec::new(),
        }
    }
}

impl SectionProps {
    pub fn a4() -> Self {
        SectionProps {
            page_width: 11906,
            page_height: 16838,
            ..Default::default()
        }
    }
    pub fn text_width(&self) -> Twips {
        (self.page_width - self.margin_left - self.margin_right - self.gutter).max(720)
    }
    pub fn text_height(&self) -> Twips {
        (self.page_height - self.margin_top - self.margin_bottom).max(720)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Paragraph {
    pub props: ParaProps,
    pub items: Vec<Item>,
}

impl Paragraph {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn from_text(s: &str) -> Self {
        Paragraph { props: ParaProps::default(), items: s.chars().map(Item::Char).collect() }
    }
    pub fn text(&self) -> String {
        self.items.iter().filter_map(Item::as_char).collect()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn char_count(&self) -> usize {
        self.items.iter().filter(|i| !i.is_code()).count()
    }
}

/// A position in the document: paragraph index and item index within it.
/// `idx == items.len()` is the position after the last item (before `[HRt]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct Pos {
    pub para: usize,
    pub idx: usize,
}

impl Pos {
    pub const fn new(para: usize, idx: usize) -> Pos {
        Pos { para, idx }
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.para, self.idx)
    }
}

/// An ordered range `[start, end)` of positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

impl Range {
    pub fn new(a: Pos, b: Pos) -> Range {
        if a <= b {
            Range { start: a, end: b }
        } else {
            Range { start: b, end: a }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
    pub fn contains(&self, p: Pos) -> bool {
        p >= self.start && p < self.end
    }
}
