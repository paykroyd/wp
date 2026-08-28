//! The projection both sides of a diff are compared in: a paragraph as a
//! list of index-carrying units with the character formatting Docs can hold,
//! plus the paragraph formatting Docs can hold.
//!
//! The baseline is projected from the paragraphs the reader built, and the
//! current document is projected from the paragraphs the user edited, with
//! the same function — so a paragraph nobody touched projects identically on
//! both sides and produces no request.

use crate::json::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use wp_core::model::*;
use wp_core::Document;

/// Character formatting Docs can hold and `wp` models. Every field is the
/// *direct* formatting: `None` means "not set here" (inherited).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Sty {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub small_caps: Option<bool>,
    pub size_hp: Option<u16>,
    pub font: Option<String>,
    pub color: Option<Rgb>,
    pub bg: Option<Highlight>,
    pub vert: Option<VertAlign>,
    pub link: Option<String>,
}

/// Bit set of `Sty` fields, in `updateTextStyle` mask order.
pub type Fields = u16;
pub const F_BOLD: Fields = 1;
pub const F_ITALIC: Fields = 2;
pub const F_UNDERLINE: Fields = 4;
pub const F_STRIKE: Fields = 8;
pub const F_SMALL_CAPS: Fields = 16;
pub const F_SIZE: Fields = 32;
pub const F_FONT: Fields = 64;
pub const F_COLOR: Fields = 128;
pub const F_BG: Fields = 256;
pub const F_VERT: Fields = 512;
pub const F_LINK: Fields = 1024;
pub const F_ALL: Fields = 2047;

const FIELD_NAMES: [(Fields, &str); 11] = [
    (F_BOLD, "bold"),
    (F_ITALIC, "italic"),
    (F_UNDERLINE, "underline"),
    (F_STRIKE, "strikethrough"),
    (F_SMALL_CAPS, "smallCaps"),
    (F_SIZE, "fontSize"),
    (F_FONT, "weightedFontFamily"),
    (F_COLOR, "foregroundColor"),
    (F_BG, "backgroundColor"),
    (F_VERT, "baselineOffset"),
    (F_LINK, "link"),
];

pub fn field_mask(f: Fields) -> String {
    FIELD_NAMES.iter().filter(|(b, _)| f & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(",")
}

impl Sty {
    pub fn from_attrs(stack: &[Attr], link: Option<&str>) -> Sty {
        let mut s = Sty { link: link.map(str::to_string), ..Default::default() };
        // Innermost wins, as in `Document::runs`.
        for a in stack {
            match a {
                Attr::Bold(b) => s.bold = Some(*b),
                Attr::Italic(b) => s.italic = Some(*b),
                Attr::Underline(u) => s.underline = Some(*u != Underline::None),
                Attr::Strike(b) | Attr::DoubleStrike(b) => s.strike = Some(*b),
                Attr::VertAlign(v) => s.vert = Some(*v),
                Attr::SmallCaps(b) => s.small_caps = Some(*b),
                Attr::Font(f) => s.font = Some(f.clone()),
                Attr::Size(n) => s.size_hp = Some(*n),
                Attr::Color(c) => s.color = Some(*c),
                Attr::Highlight(h) => s.bg = if *h == Highlight::None { None } else { Some(*h) },
                Attr::AllCaps(_) | Attr::CharStyle(_) | Attr::Raw(_) | Attr::RunAttrs(_) => {}
            }
        }
        s
    }

    /// Fields that differ between `self` (what the document has) and `want`.
    pub fn diff(&self, want: &Sty) -> Fields {
        let mut f = 0;
        if self.bold != want.bold {
            f |= F_BOLD;
        }
        if self.italic != want.italic {
            f |= F_ITALIC;
        }
        if self.underline != want.underline {
            f |= F_UNDERLINE;
        }
        if self.strike != want.strike {
            f |= F_STRIKE;
        }
        if self.small_caps != want.small_caps {
            f |= F_SMALL_CAPS;
        }
        if self.size_hp != want.size_hp {
            f |= F_SIZE;
        }
        if self.font != want.font {
            f |= F_FONT;
        }
        if self.color != want.color {
            f |= F_COLOR;
        }
        if self.bg != want.bg {
            f |= F_BG;
        }
        if self.vert != want.vert {
            f |= F_VERT;
        }
        if self.link != want.link {
            f |= F_LINK;
        }
        f
    }

    /// The `TextStyle` object for the fields in `mask`. A field in the mask
    /// with no value is left out, which `updateTextStyle` reads as "reset".
    pub fn to_json(&self, mask: Fields) -> Value {
        let mut m = serde_json::Map::new();
        if mask & F_BOLD != 0 {
            if let Some(b) = self.bold {
                m.insert("bold".into(), b.into());
            }
        }
        if mask & F_ITALIC != 0 {
            if let Some(b) = self.italic {
                m.insert("italic".into(), b.into());
            }
        }
        if mask & F_UNDERLINE != 0 {
            if let Some(b) = self.underline {
                m.insert("underline".into(), b.into());
            }
        }
        if mask & F_STRIKE != 0 {
            if let Some(b) = self.strike {
                m.insert("strikethrough".into(), b.into());
            }
        }
        if mask & F_SMALL_CAPS != 0 {
            if let Some(b) = self.small_caps {
                m.insert("smallCaps".into(), b.into());
            }
        }
        if mask & F_SIZE != 0 {
            if let Some(hp) = self.size_hp {
                m.insert("fontSize".into(), json!({ "magnitude": hp as f64 / 2.0, "unit": "PT" }));
            }
        }
        if mask & F_FONT != 0 {
            if let Some(f) = &self.font {
                m.insert("weightedFontFamily".into(), json!({ "fontFamily": f, "weight": 400 }));
            }
        }
        if mask & F_COLOR != 0 {
            if let Some(c) = self.color {
                m.insert("foregroundColor".into(), rgb_color(c));
            }
        }
        if mask & F_BG != 0 {
            if let Some(c) = self.bg.and_then(highlight_rgb) {
                m.insert("backgroundColor".into(), rgb_color(c));
            }
        }
        if mask & F_VERT != 0 {
            if let Some(v) = self.vert {
                let name = match v {
                    VertAlign::Baseline => "NONE",
                    VertAlign::Superscript => "SUPERSCRIPT",
                    VertAlign::Subscript => "SUBSCRIPT",
                };
                m.insert("baselineOffset".into(), name.into());
            }
        }
        if mask & F_LINK != 0 {
            if let Some(u) = &self.link {
                m.insert("link".into(), json!({ "url": u }));
            }
        }
        Value::Object(m)
    }
}

/// Paragraph formatting Docs can hold and `wp` models (direct formatting).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct PSty {
    pub named: Option<String>,
    pub align: Option<Align>,
    pub indent_start: Option<Twips>,
    pub indent_end: Option<Twips>,
    /// Absolute first-line indent, as Docs stores it.
    pub indent_first: Option<Twips>,
    pub space_above: Option<Twips>,
    pub space_below: Option<Twips>,
    /// Percent of single spacing.
    pub line_pct: Option<i32>,
    pub keep_lines: Option<bool>,
    pub keep_next: Option<bool>,
    pub widow: Option<bool>,
    pub page_break_before: Option<bool>,
    pub shading: Option<Rgb>,
}

pub const P_ALL: &str = "namedStyleType,alignment,indentStart,indentEnd,indentFirstLine,spaceAbove,spaceBelow,lineSpacing,keepLinesTogether,keepWithNext,avoidWidowAndOrphan,pageBreakBefore,shading.backgroundColor";

impl PSty {
    pub fn from_props(p: &ParaProps) -> PSty {
        let indent_first = if p.first_line.is_some() || p.hanging.is_some() {
            Some(p.indent_left.unwrap_or(0) + p.first_line_offset())
        } else {
            None
        };
        PSty {
            named: p.style.clone(),
            align: p.align,
            indent_start: p.indent_left,
            indent_end: p.indent_right,
            indent_first,
            space_above: p.space_before,
            space_below: p.space_after,
            line_pct: match p.line_spacing {
                Some(LineSpacing::Auto(n)) => Some(((n as f64) * 100.0 / 240.0).round() as i32),
                _ => None,
            },
            keep_lines: p.keep_lines,
            keep_next: p.keep_next,
            widow: p.widow_control,
            page_break_before: p.page_break_before,
            shading: p.shading,
        }
    }

    /// `(paragraphStyle, fields)` for the fields that differ from `have`, or
    /// for everything when `have` is `None`.
    pub fn to_json(&self, have: Option<&PSty>) -> Option<(Value, String)> {
        let mut m = serde_json::Map::new();
        let mut fields: Vec<&str> = Vec::new();
        macro_rules! field {
            ($name:expr, $get:expr, $render:expr) => {
                if have.map_or(true, |h| $get(h) != $get(self)) {
                    fields.push($name);
                    if let Some(v) = $get(self) {
                        m.insert($name.into(), $render(v));
                    }
                }
            };
        }
        if have.map_or(true, |h| h.named != self.named) {
            fields.push("namedStyleType");
            m.insert("namedStyleType".into(), named_style_of(self.named.as_deref()).into());
        }
        field!("alignment", |p: &PSty| p.align, |a| Value::from(align_name(a)));
        field!("indentStart", |p: &PSty| p.indent_start, twips_dim);
        field!("indentEnd", |p: &PSty| p.indent_end, twips_dim);
        field!("indentFirstLine", |p: &PSty| p.indent_first, twips_dim);
        field!("spaceAbove", |p: &PSty| p.space_above, twips_dim);
        field!("spaceBelow", |p: &PSty| p.space_below, twips_dim);
        field!("lineSpacing", |p: &PSty| p.line_pct, |n: i32| Value::from(n as f64));
        field!("keepLinesTogether", |p: &PSty| p.keep_lines, Value::from);
        field!("keepWithNext", |p: &PSty| p.keep_next, Value::from);
        field!("avoidWidowAndOrphan", |p: &PSty| p.widow, Value::from);
        field!("pageBreakBefore", |p: &PSty| p.page_break_before, Value::from);
        if have.map_or(true, |h| h.shading != self.shading) {
            fields.push("shading.backgroundColor");
            if let Some(c) = self.shading {
                m.insert("shading".into(), json!({ "backgroundColor": rgb_color(c) }));
            }
        }
        if fields.is_empty() {
            return None;
        }
        Some((Value::Object(m), fields.join(",")))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnitKind {
    /// A character, tab (`\t`) or line break (`\u{b}`) — text Docs stores as text.
    Char(char),
    /// A page break, insertable.
    PageBreak,
    /// A footnote reference, by Docs footnote id.
    Footnote(String),
    /// Anything else Docs has in a paragraph, by its preserved JSON.
    Object(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Unit {
    pub kind: UnitKind,
    /// UTF-16 code units this occupies in Docs.
    pub len: i64,
    pub sty: Sty,
}

impl Unit {
    pub fn is_text(&self) -> bool {
        matches!(self.kind, UnitKind::Char(_))
    }
}

/// One paragraph, projected.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Proj {
    pub units: Vec<Unit>,
    pub para: PSty,
    /// List membership as the model's `num_id`; the writer maps it to a preset.
    pub list: Option<ListRef>,
    /// A preserved block (`raw_block`): its JSON and length. Such a paragraph
    /// has no text and no newline of its own.
    pub raw: Option<(String, i64)>,
}

impl Proj {
    pub fn text_len(&self) -> i64 {
        self.units.iter().map(|u| u.len).sum()
    }
}

/// Context for projecting the current document.
pub struct Ctx<'a> {
    pub rels: &'a [ExtraRel],
    /// Model footnote id (1-based) → Docs footnote id.
    pub footnote_ids: &'a [String],
}

/// Project a paragraph. Errors name what the paragraph holds that has no
/// place in a Docs document (a footnote created in `wp`, for instance).
pub fn project(ctx: &Ctx, p: &Paragraph) -> Result<Proj, String> {
    let para = PSty::from_props(&p.props);
    if p.props.raw_block {
        let raw = p.items.iter().find_map(|i| match i {
            Item::Code(Code::Opaque(o)) => Some(o),
            _ => None,
        });
        let Some(o) = raw else { return Err("a preserved block with no content".into()) };
        let len = opaque_len(&o.xml).ok_or("a preserved block that did not come from Google Docs")?;
        return Ok(Proj { units: Vec::new(), para, list: p.props.list, raw: Some((o.xml.clone(), len)) });
    }
    let mut units = Vec::new();
    let mut stack: Vec<Attr> = Vec::new();
    let mut link: Option<String> = None;
    let mut sty = Sty::default();
    for it in &p.items {
        match it {
            Item::Char(c) => {
                let c = if *c == '\n' { ' ' } else { *c };
                units.push(Unit { kind: UnitKind::Char(c), len: utf16_len(c), sty: sty.clone() });
            }
            Item::Code(code) => match code {
                Code::On(a) => {
                    stack.push(a.clone());
                    sty = Sty::from_attrs(&stack, link.as_deref());
                }
                Code::Off(k) => {
                    if let Some(i) = stack.iter().rposition(|a| a.kind() == *k) {
                        stack.remove(i);
                    }
                    sty = Sty::from_attrs(&stack, link.as_deref());
                }
                Code::Tab => units.push(Unit { kind: UnitKind::Char('\t'), len: 1, sty: sty.clone() }),
                Code::LineBreak => units.push(Unit { kind: UnitKind::Char('\u{b}'), len: 1, sty: sty.clone() }),
                Code::PageBreak => units.push(Unit { kind: UnitKind::PageBreak, len: 1, sty: Sty::default() }),
                Code::Bookmark(_) | Code::BookmarkEnd(_) => {}
                Code::Opaque(o) => {
                    if o.xml.starts_with("<w:hyperlink") {
                        link = hyperlink_url(&o.xml, ctx.rels);
                        sty = Sty::from_attrs(&stack, link.as_deref());
                    } else if o.xml.starts_with("</w:hyperlink") {
                        link = None;
                        sty = Sty::from_attrs(&stack, link.as_deref());
                    } else if let Some(id) = footnote_ref_id(&o.xml) {
                        let gid = ctx.footnote_ids.get(id.saturating_sub(1)).ok_or("a footnote created in wp (not yet supported for Google Docs)")?;
                        units.push(Unit { kind: UnitKind::Footnote(gid.clone()), len: 1, sty: Sty::default() });
                    } else if o.xml.starts_with('{') {
                        let len = opaque_len(&o.xml).unwrap_or(1);
                        units.push(Unit { kind: UnitKind::Object(o.xml.clone()), len, sty: Sty::default() });
                    } else if o.xml.starts_with("<w:ins") || o.xml.starts_with("</w:ins") || o.xml.starts_with("<w:del") || o.xml.starts_with("</w:del") {
                        // Suggestion wrappers: the text inside is ordinary units.
                    } else {
                        return Err(format!("{} (not from Google Docs)", if o.label.is_empty() { "an element" } else { &o.label }));
                    }
                }
            },
        }
    }
    Ok(Proj { units, para, list: p.props.list, raw: None })
}

/// The length recorded in a preserved element's JSON (`"wpLen"`).
pub fn opaque_len(xml: &str) -> Option<i64> {
    let v: Value = serde_json::from_str(xml).ok()?;
    v.get("wpLen").and_then(Value::as_i64)
}

/// `w:id` of a `<w:footnoteReference w:id="N"/>`.
pub fn footnote_ref_id(xml: &str) -> Option<usize> {
    let rest = xml.strip_prefix("<w:footnoteReference")?;
    let i = rest.find("w:id=\"")? + 6;
    let j = rest[i..].find('"')? + i;
    rest[i..j].parse().ok()
}

fn hyperlink_url(xml: &str, rels: &[ExtraRel]) -> Option<String> {
    let i = xml.find("r:id=\"")? + 6;
    let j = xml[i..].find('"')? + i;
    let id = &xml[i..j];
    rels.iter().find(|r| r.id == id).map(|r| r.target.clone())
}

/// `num_id` → bullet preset, from the list's level-0 format.
pub fn bullet_preset(doc: &Document, list: ListRef) -> &'static str {
    use wp_core::numbering::NumFmt;
    match doc.numbering.level(list.num_id, 0).map(|l| l.fmt) {
        Some(NumFmt::Bullet) | None => "BULLET_DISC_CIRCLE_SQUARE",
        Some(NumFmt::UpperLetter) => "NUMBERED_UPPERALPHA_ALPHA_ROMAN",
        Some(NumFmt::UpperRoman) => "NUMBERED_UPPERROMAN_UPPERALPHA_DECIMAL_PARENS",
        Some(NumFmt::DecimalZero) => "NUMBERED_ZERODECIMAL_ALPHA_ROMAN",
        Some(_) => "NUMBERED_DECIMAL_ALPHA_ROMAN",
    }
}

/// Coalesce consecutive text units with the same style into `(offset, len, sty)`.
pub fn style_runs(units: &[Unit]) -> Vec<(i64, i64, Sty)> {
    let mut out: Vec<(i64, i64, Sty)> = Vec::new();
    let mut off = 0;
    for u in units {
        if u.is_text() {
            match out.last_mut() {
                Some((o, l, s)) if *o + *l == off && *s == u.sty => *l += u.len,
                _ => out.push((off, u.len, u.sty.clone())),
            }
        }
        off += u.len;
    }
    out
}

/// A map used by the reader and writer alike: Docs list id ↔ `num_id`.
pub type ListMap = BTreeMap<String, i32>;
