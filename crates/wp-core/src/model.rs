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
    /// Preserved attributes of the `w:r` element itself (revision ids).
    RunAttrs,
}

impl AttrKind {
    pub fn all() -> &'static [AttrKind] {
        use AttrKind::*;
        &[
            Bold, Italic, Underline, Strike, DoubleStrike, VertAlign, SmallCaps, AllCaps, Font,
            Size, Color, Highlight, CharStyle, Raw, RunAttrs,
        ]
    }
    /// Kinds shown in Reveal Codes by default (preserved XML is hidden unless asked).
    pub fn is_visible(self) -> bool {
        !matches!(self, AttrKind::Raw | AttrKind::RunAttrs)
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
    /// The attribute text of the `w:r` start tag as read (` w:rsidR="…"`),
    /// re-emitted verbatim so a byte-diff of the saved file stays quiet.
    RunAttrs(String),
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
            Attr::RunAttrs(_) => AttrKind::RunAttrs,
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

/// Where an opaque element sat in the file, so it goes back to the same place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum OpaqueLevel {
    /// Inside a `w:r`.
    #[default]
    Run,
    /// A direct child of `w:p`, outside any run.
    Para,
    /// A direct child of `w:body`, next to the paragraph rather than in it.
    Body,
    /// A direct child of `w:tc`, next to the paragraph inside its cell.
    Cell,
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
    /// A rendering hint Word regenerates itself (`w:lastRenderedPageBreak`,
    /// `w:proofErr`): preserved, but hidden in Reveal Codes unless asked.
    pub hint: bool,
    pub level: OpaqueLevel,
}

impl OpaqueXml {
    pub fn element(xml: impl Into<String>, label: impl Into<String>) -> OpaqueXml {
        OpaqueXml { xml: xml.into(), label: label.into(), kind: OpaqueKind::Element, protected: false, deleted: false, hint: false, level: OpaqueLevel::Run }
    }
    pub fn hint(xml: impl Into<String>, label: impl Into<String>) -> OpaqueXml {
        OpaqueXml { hint: true, ..OpaqueXml::element(xml, label) }
    }
    pub fn at(mut self, level: OpaqueLevel) -> OpaqueXml {
        self.level = level;
        self
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
    /// `[Col Brk]` — the rest of the text column is left empty.
    ColumnBreak,
    Bookmark(String),
    BookmarkEnd(String),
    /// Run-level XML that wp does not understand. Zero width in layout.
    Opaque(OpaqueXml),
}

impl Code {
    /// True for codes that are invisible/zero-width in ordinary editing.
    pub fn is_zero_width(&self) -> bool {
        !matches!(self, Code::Tab | Code::LineBreak | Code::PageBreak | Code::ColumnBreak)
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
    /// Explicitly no line (`w:val="nil"`), overriding a style's.
    None,
}

impl BorderStyle {
    pub fn docx_name(self) -> &'static str {
        match self {
            BorderStyle::Single => "single",
            BorderStyle::Double => "double",
            BorderStyle::Dotted => "dotted",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Thick => "thick",
            BorderStyle::None => "nil",
        }
    }
    pub fn from_docx(s: &str) -> BorderStyle {
        match s {
            "double" => BorderStyle::Double,
            "dotted" => BorderStyle::Dotted,
            "dashed" => BorderStyle::Dashed,
            "thick" => BorderStyle::Thick,
            "nil" | "none" => BorderStyle::None,
            _ => BorderStyle::Single,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Border {
    pub style: BorderStyle,
    /// Eighths of a point, as in `w:sz`.
    pub size: u16,
    pub color: Option<Rgb>,
    pub space: u16,
}

impl Border {
    /// Word's default table line: single, ½ pt, automatic colour.
    pub fn single() -> Border {
        Border { style: BorderStyle::Single, size: 4, color: None, space: 0 }
    }
    pub fn none() -> Border {
        Border { style: BorderStyle::None, size: 0, color: None, space: 0 }
    }
    pub fn is_visible(&self) -> bool {
        self.style != BorderStyle::None
    }
}

/// A table's lines (`w:tblBorders`): the outside edges and the lines
/// between cells. `None` on a side means the table style decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TableBorders {
    pub top: Option<Border>,
    pub left: Option<Border>,
    pub bottom: Option<Border>,
    pub right: Option<Border>,
    pub inside_h: Option<Border>,
    pub inside_v: Option<Border>,
}

impl TableBorders {
    pub fn all(b: Border) -> TableBorders {
        TableBorders { top: Some(b), left: Some(b), bottom: Some(b), right: Some(b), inside_h: Some(b), inside_v: Some(b) }
    }
    /// Outside lines only.
    pub fn outside(b: Border) -> TableBorders {
        TableBorders { top: Some(b), left: Some(b), bottom: Some(b), right: Some(b), inside_h: Some(Border::none()), inside_v: Some(Border::none()) }
    }
    /// Inside lines only.
    pub fn inside(b: Border) -> TableBorders {
        TableBorders { top: Some(Border::none()), left: Some(Border::none()), bottom: Some(Border::none()), right: Some(Border::none()), inside_h: Some(b), inside_v: Some(b) }
    }
    /// True when every line is explicitly off.
    pub fn is_none(&self) -> bool {
        [self.top, self.left, self.bottom, self.right, self.inside_h, self.inside_v].iter().all(|b| b.map_or(false, |b| !b.is_visible()))
    }
}

/// One cell's own lines (`w:tcBorders`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CellBorders {
    pub top: Option<Border>,
    pub left: Option<Border>,
    pub bottom: Option<Border>,
    pub right: Option<Border>,
}

impl CellBorders {
    pub fn all(b: Border) -> CellBorders {
        CellBorders { top: Some(b), left: Some(b), bottom: Some(b), right: Some(b) }
    }
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
            Attr::Raw(_) | Attr::RunAttrs(_) => {}
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
    /// A section break: this paragraph is the last of a section whose page
    /// setup is these properties (`w:pPr/w:sectPr`). The section after the
    /// last break is `Document::section`.
    pub sect_break: Option<SectionProps>,
    /// The complete `w:pPr` as read, kept until the properties are changed.
    pub raw_ppr: Option<String>,
    /// Attribute text of the `w:p` start tag as read (` w:rsidR="…"
    /// w14:paraId="…"`). Never copied to a new paragraph: paragraph ids must
    /// stay unique.
    pub p_attrs: Option<String>,
    /// This "paragraph" is a verbatim body-level block (table, content
    /// control, …) held in a single `Opaque` item. Not editable.
    pub raw_block: bool,
    /// The table cell this paragraph belongs to, if any. Cell paragraphs are
    /// contiguous in the document, in row-major order; the grid itself lives
    /// in `Document::tables` (DESIGN.md §3.7).
    pub cell: Option<CellRef>,
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
            p_attrs: other.p_attrs.clone(),
            raw_block: other.raw_block,
            cell: other.cell,
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


/// Which table cell a paragraph belongs to. `col` is the cell's index within
/// its row (not the grid column: a cell may span several grid columns).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CellRef {
    pub table: u32,
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub const fn new(table: u32, row: u32, col: u32) -> CellRef {
        CellRef { table, row, col }
    }
    /// Spreadsheet-style name, `A1` = first column of the first row.
    pub fn name(&self) -> String {
        format!("{}{}", column_letters(self.col), self.row + 1)
    }
}

/// `0 → A`, `25 → Z`, `26 → AA`.
pub fn column_letters(mut col: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s.iter().rev().collect()
}

/// Vertical merge state of a cell (`w:vMerge`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VMerge {
    /// The top cell of a vertically merged region.
    Restart,
    /// Continues the region begun above; its content is not shown.
    Continue,
}

/// Default cell margin on each side (Word's `TableNormal`: 108 twips).
pub const DEFAULT_CELL_MARGIN: Twips = 108;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TableCell {
    /// Grid columns spanned (`w:gridSpan`), at least 1.
    pub span: u16,
    pub vmerge: Option<VMerge>,
    /// Preferred width in twips (`w:tcW` of type `dxa`), when known.
    pub width: Option<Twips>,
    /// Cell shading fill (`w:shd/@w:fill`), when a plain colour.
    pub shading: Option<Rgb>,
    /// The cell's own lines, when set.
    pub borders: Option<CellBorders>,
    /// The complete `w:tcPr` as read (empty when the cell had none);
    /// `None` means it is regenerated from the fields on write.
    pub raw_tcpr: Option<String>,
    /// Attribute text of the `w:tc` start tag.
    pub attrs: String,
}

impl TableCell {
    pub fn new() -> TableCell {
        TableCell { span: 1, ..Default::default() }
    }
    pub fn span(&self) -> usize {
        self.span.max(1) as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    /// Repeat as a header row at the top of each page (`w:tblHeader`).
    pub header: bool,
    /// Don't break this row across pages (`w:cantSplit`).
    pub cant_split: bool,
    /// Row height in twips (`w:trHeight`), when set: a minimum, or exact
    /// when `height_exact` (`w:hRule="exact"`).
    pub height: Option<Twips>,
    pub height_exact: bool,
    /// Everything before the first `w:tc` (`w:tblPrEx`, `w:trPr`), verbatim.
    pub raw_trpr: Option<String>,
    /// Attribute text of the `w:tr` start tag.
    pub attrs: String,
}

/// A table's grid and properties. The cell *contents* are the paragraphs
/// tagged with a `CellRef` naming this table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Table {
    /// Grid column widths in twips (`w:tblGrid`).
    pub grid: Vec<Twips>,
    pub rows: Vec<TableRow>,
    /// Table style id (`w:tblStyle`).
    pub style: Option<String>,
    /// The table's lines (`w:tblBorders`), when set directly.
    pub borders: Option<TableBorders>,
    /// Left/right cell margins (`w:tblCellMar`), defaulting to Word's 108.
    pub cell_margin_left: Twips,
    pub cell_margin_right: Twips,
    /// The complete `w:tblPr` as read, re-emitted verbatim.
    pub raw_tblpr: Option<String>,
    /// The complete `w:tblGrid` as read; regenerated when columns change.
    pub raw_grid: Option<String>,
    /// Attribute text of the `w:tbl` start tag.
    pub attrs: String,
}

impl Table {
    /// A new `rows × cols` table filling `width`, with single borders.
    pub fn new(rows: usize, cols: usize, width: Twips) -> Table {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let w = (width / cols as Twips).max(360);
        Table {
            grid: vec![w; cols],
            rows: (0..rows).map(|_| TableRow { cells: (0..cols).map(|_| TableCell { span: 1, width: Some(w), ..Default::default() }).collect(), ..Default::default() }).collect(),
            style: Some("TableGrid".into()),
            borders: Some(TableBorders::all(Border::single())),
            cell_margin_left: DEFAULT_CELL_MARGIN,
            cell_margin_right: DEFAULT_CELL_MARGIN,
            raw_tblpr: None,
            raw_grid: None,
            attrs: String::new(),
        }
    }
    pub fn cols(&self) -> usize {
        self.grid.len().max(1)
    }
    pub fn width(&self) -> Twips {
        self.grid.iter().sum()
    }
    /// The first grid column a cell starts at.
    pub fn grid_col(&self, row: usize, col: usize) -> usize {
        self.rows.get(row).map(|r| r.cells.iter().take(col).map(|c| c.span()).sum()).unwrap_or(0)
    }
    /// Twips from the table's left edge to the cell's left edge, and the
    /// cell's full width (before margins).
    pub fn cell_extent(&self, row: usize, col: usize) -> (Twips, Twips) {
        let g = self.grid_col(row, col);
        let span = self.rows.get(row).and_then(|r| r.cells.get(col)).map(|c| c.span()).unwrap_or(1);
        let x: Twips = self.grid.iter().take(g).sum();
        let w: Twips = self.grid.iter().skip(g).take(span).sum();
        (x, w.max(360))
    }
    /// Width available to text inside a cell.
    pub fn cell_text_width(&self, row: usize, col: usize) -> Twips {
        let (_, w) = self.cell_extent(row, col);
        (w - self.cell_margin_left - self.cell_margin_right).max(360)
    }
    /// The cell index in `row` that covers grid column `g`.
    pub fn cell_at_grid(&self, row: usize, g: usize) -> usize {
        let Some(r) = self.rows.get(row) else { return 0 };
        let mut acc = 0;
        for (i, c) in r.cells.iter().enumerate() {
            acc += c.span();
            if g < acc {
                return i;
            }
        }
        r.cells.len().saturating_sub(1)
    }
    /// Whether the table draws lines between cells (the grid in draft view
    /// is dotted when it does not).
    pub fn lines_visible(&self) -> bool {
        match &self.borders {
            Some(b) => !b.is_none(),
            // No direct borders: the style decides; TableNormal has none,
            // TableGrid and everything else wp knows draw lines.
            None => self.style.as_deref().map_or(false, |s| s != "TableNormal"),
        }
    }
    /// Mark the grid as changed so it is regenerated on write.
    pub fn touch_grid(&mut self) {
        self.raw_grid = None;
        for r in &mut self.rows {
            for c in &mut r.cells {
                c.raw_tcpr = None;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// How a section begins relative to the one before it (`w:type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SectionStart {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
}

impl SectionStart {
    pub fn docx_name(self) -> &'static str {
        match self {
            SectionStart::NextPage => "nextPage",
            SectionStart::Continuous => "continuous",
            SectionStart::EvenPage => "evenPage",
            SectionStart::OddPage => "oddPage",
        }
    }
    pub fn from_docx(s: &str) -> SectionStart {
        match s {
            "continuous" => SectionStart::Continuous,
            "evenPage" => SectionStart::EvenPage,
            "oddPage" => SectionStart::OddPage,
            _ => SectionStart::NextPage,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            SectionStart::NextPage => "New Page",
            SectionStart::Continuous => "Continuous",
            SectionStart::EvenPage => "Even Page",
            SectionStart::OddPage => "Odd Page",
        }
    }
}

/// Header or footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HfKind {
    Header,
    Footer,
}

impl HfKind {
    pub fn title(self) -> &'static str {
        match self {
            HfKind::Header => "Header",
            HfKind::Footer => "Footer",
        }
    }
}

/// Which pages of a section a header or footer applies to (`w:type` of a
/// `w:headerReference`). `First` needs the section's `title_page`; `Even`
/// needs the document's `even_odd_headers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HfPages {
    Default,
    First,
    Even,
}

impl HfPages {
    pub fn docx_name(self) -> &'static str {
        match self {
            HfPages::Default => "default",
            HfPages::First => "first",
            HfPages::Even => "even",
        }
    }
    pub fn from_docx(s: &str) -> HfPages {
        match s {
            "first" => HfPages::First,
            "even" => HfPages::Even,
            _ => HfPages::Default,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            HfPages::Default => "every page",
            HfPages::First => "first page",
            HfPages::Even => "even pages",
        }
    }
}

/// A section's reference to a header or footer body, by the id it has in
/// `Document::headers` (the main part's relationship id in a `.docx`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HfRef {
    pub kind: HfKind,
    pub pages: HfPages,
    pub id: String,
}

/// The body of a header or footer: paragraphs laid out against the
/// section's text width. Kept as its own part in a `.docx`; `raw` is that
/// part verbatim until the body is edited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeaderFooter {
    pub kind: Option<HfKind>,
    pub paragraphs: Vec<Paragraph>,
    /// The part's XML as read (`None` for a body created in wp, or once
    /// edited — the writer then regenerates it).
    pub raw: Option<String>,
    /// The root start tag of the part as read (`<w:hdr …>`), for the
    /// namespace declarations.
    pub root_tag: Option<String>,
    /// The part's name in the package, once it has one.
    pub part: Option<String>,
}

/// Page geometry and page furniture for a section.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SectionProps {
    /// How the section begins (`w:type`).
    pub start: SectionStart,
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
    /// Space between columns (`w:cols/@w:space`).
    pub column_space: Twips,
    /// The first page has its own header/footer (`w:titlePg`).
    pub title_page: bool,
    /// Headers and footers that apply in this section.
    pub hf: Vec<HfRef>,
    /// Page numbering restarts at this number (`w:pgNumType/@w:start`).
    pub page_start: Option<i32>,
    /// Every child of `w:sectPr` as read, for verbatim re-emission of the
    /// parts we don't model (line numbering, page borders, etc.). The
    /// modelled children are replaced on write only when they changed.
    pub opaque_children: Vec<String>,
    /// Attribute text of the `w:sectPr` start tag as read.
    pub attrs: String,
}

impl Default for SectionProps {
    /// US Letter, 1" margins — Word's default.
    fn default() -> Self {
        SectionProps {
            start: SectionStart::NextPage,
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
            column_space: 720,
            title_page: false,
            hf: Vec::new(),
            page_start: None,
            opaque_children: Vec::new(),
            attrs: String::new(),
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
    /// Columns per page, at least 1.
    pub fn column_count(&self) -> usize {
        self.columns.max(1) as usize
    }
    /// Width of one text column.
    pub fn column_width(&self) -> Twips {
        let n = self.column_count() as Twips;
        ((self.text_width() - self.column_space.max(0) * (n - 1)) / n).max(360)
    }
    /// The header/footer id for `kind` on pages of type `pages`, if the
    /// section has one.
    pub fn hf_id(&self, kind: HfKind, pages: HfPages) -> Option<&str> {
        self.hf.iter().find(|r| r.kind == kind && r.pages == pages).map(|r| r.id.as_str())
    }
    /// Which header/footer body a page uses: the first-page one on the first
    /// page when the section has a title page, the even one on even pages
    /// when the document distinguishes them, else the default.
    pub fn hf_for_page(&self, kind: HfKind, first: bool, even: bool, even_odd: bool) -> Option<&str> {
        if first && self.title_page {
            if let Some(id) = self.hf_id(kind, HfPages::First) {
                return Some(id);
            }
        }
        if even && even_odd {
            if let Some(id) = self.hf_id(kind, HfPages::Even) {
                return Some(id);
            }
        }
        self.hf_id(kind, HfPages::Default)
    }
    /// The page setup is the same (headers and preserved XML aside).
    pub fn same_geometry(&self, o: &SectionProps) -> bool {
        self.page_width == o.page_width
            && self.page_height == o.page_height
            && self.orientation == o.orientation
            && self.margin_top == o.margin_top
            && self.margin_bottom == o.margin_bottom
            && self.margin_left == o.margin_left
            && self.margin_right == o.margin_right
            && self.header_distance == o.header_distance
            && self.footer_distance == o.footer_distance
            && self.gutter == o.gutter
            && self.columns == o.columns
            && self.column_space == o.column_space
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Paragraph {
    pub props: ParaProps,
    pub items: Vec<Item>,
}

/// A footnote body. Read from `footnotes.xml` for export, or created by a
/// Markdown import; the `.docx` writer only generates the part when the
/// package has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footnote {
    pub id: i32,
    pub paragraphs: Vec<Paragraph>,
}

/// A relationship the document needs that the package does not have yet
/// (a hyperlink target created by a Markdown import).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraRel {
    pub id: String,
    /// Relationship type suffix, e.g. `hyperlink`.
    pub kind: String,
    pub target: String,
    pub external: bool,
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
