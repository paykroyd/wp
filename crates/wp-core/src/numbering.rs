//! Lists: numbering definitions (`numbering.xml`), label formatting, and the
//! counters that turn "item at level 2 of list 5" into "2.1.".
//!
//! A paragraph belongs to a list through `ParaProps.list` (`w:numPr`). The
//! `num_id` names an instance (`w:num`), which points at an abstract
//! definition (`w:abstractNum`) with up to nine levels. Counters run per
//! abstract definition, which is how Word continues numbering across
//! instances; an instance with a start override resets its level the first
//! time it is used, which is how "restart numbering" works.

use crate::document::Document;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum NumFmt {
    #[default]
    Decimal,
    DecimalZero,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    Ordinal,
    CardinalText,
    OrdinalText,
    Bullet,
    None,
    /// Any other `w:numFmt` (chicago, aiueo, …): shown as decimal.
    Other(String),
}

impl NumFmt {
    pub fn docx_name(&self) -> &str {
        match self {
            NumFmt::Decimal => "decimal",
            NumFmt::DecimalZero => "decimalZero",
            NumFmt::LowerLetter => "lowerLetter",
            NumFmt::UpperLetter => "upperLetter",
            NumFmt::LowerRoman => "lowerRoman",
            NumFmt::UpperRoman => "upperRoman",
            NumFmt::Ordinal => "ordinal",
            NumFmt::CardinalText => "cardinalText",
            NumFmt::OrdinalText => "ordinalText",
            NumFmt::Bullet => "bullet",
            NumFmt::None => "none",
            NumFmt::Other(s) => s,
        }
    }
    pub fn from_docx(s: &str) -> NumFmt {
        match s {
            "decimal" => NumFmt::Decimal,
            "decimalZero" => NumFmt::DecimalZero,
            "lowerLetter" => NumFmt::LowerLetter,
            "upperLetter" => NumFmt::UpperLetter,
            "lowerRoman" => NumFmt::LowerRoman,
            "upperRoman" => NumFmt::UpperRoman,
            "ordinal" => NumFmt::Ordinal,
            "cardinalText" => NumFmt::CardinalText,
            "ordinalText" => NumFmt::OrdinalText,
            "bullet" => NumFmt::Bullet,
            "none" => NumFmt::None,
            other => NumFmt::Other(other.to_string()),
        }
    }
    pub fn is_bullet(&self) -> bool {
        *self == NumFmt::Bullet
    }
}

/// What follows the label: a tab to the text indent (Word's default), a
/// single space, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Suffix {
    #[default]
    Tab,
    Space,
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Level {
    pub start: i32,
    pub fmt: NumFmt,
    /// `w:lvlText`: literal text with `%1`…`%9` placeholders.
    pub text: String,
    pub align: Align,
    pub suffix: Suffix,
    /// Paragraph properties the level contributes (indent, tabs).
    pub para: ParaProps,
    /// Run properties for the label itself (bullet font).
    pub run: RunProps,
    /// The `w:lvl` element exactly as read; emitted verbatim while unchanged.
    pub raw: Option<String>,
}

impl Level {
    pub fn new(fmt: NumFmt, text: &str, ilvl: u8) -> Level {
        let left = 720 * (ilvl as i32 + 1);
        Level {
            start: 1,
            fmt,
            text: text.to_string(),
            align: Align::Left,
            suffix: Suffix::Tab,
            para: ParaProps { indent_left: Some(left), hanging: Some(360), ..Default::default() },
            run: RunProps::default(),
            raw: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbstractNum {
    pub id: i32,
    pub levels: Vec<Level>,
    pub raw: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelOverride {
    pub ilvl: u8,
    pub start: Option<i32>,
    pub level: Option<Level>,
    pub raw: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumInstance {
    pub id: i32,
    pub abstract_id: i32,
    pub overrides: Vec<LevelOverride>,
    pub raw: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Numbering {
    pub abstract_nums: Vec<AbstractNum>,
    pub nums: Vec<NumInstance>,
    /// The `w:numbering` start tag as read (namespaces).
    pub root_tag: Option<String>,
    /// Other children of `w:numbering` (`w:numPicBullet`, `w:numIdMacAtCleanup`), verbatim.
    pub opaque: Vec<String>,
    /// Something was added or changed; the part must be regenerated.
    pub dirty: bool,
}

/// The label of one list paragraph, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListLabel {
    pub text: String,
    pub level: u8,
    pub fmt: NumFmt,
    pub suffix: Suffix,
    pub align: Align,
    pub run: RunProps,
}

/// The kind of list a user asks for; each maps to a nine-level definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ListKind {
    Bullet,
    Decimal,
    DecimalParen,
    LowerLetter,
    LowerLetterParen,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    Dash,
    Outline,
}

impl ListKind {
    pub fn all() -> &'static [ListKind] {
        use ListKind::*;
        &[Bullet, Decimal, DecimalParen, LowerLetter, LowerLetterParen, UpperLetter, LowerRoman, UpperRoman, Dash, Outline]
    }
    pub fn title(self) -> &'static str {
        match self {
            ListKind::Bullet => "• Bullet",
            ListKind::Decimal => "1. 2. 3.",
            ListKind::DecimalParen => "1) 2) 3)",
            ListKind::LowerLetter => "a. b. c.",
            ListKind::LowerLetterParen => "a) b) c)",
            ListKind::UpperLetter => "A. B. C.",
            ListKind::LowerRoman => "i. ii. iii.",
            ListKind::UpperRoman => "I. II. III.",
            ListKind::Dash => "– Dash",
            ListKind::Outline => "1. 1.1. 1.1.1. (outline)",
        }
    }
    pub fn is_bullet(self) -> bool {
        matches!(self, ListKind::Bullet | ListKind::Dash)
    }

    fn levels(self) -> Vec<Level> {
        (0..9u8)
            .map(|i| match self {
                ListKind::Bullet => {
                    let glyphs = ["•", "◦", "▪"];
                    Level::new(NumFmt::Bullet, glyphs[(i % 3) as usize], i)
                }
                ListKind::Dash => Level::new(NumFmt::Bullet, "–", i),
                ListKind::Outline => {
                    let text: Vec<String> = (1..=i + 1).map(|n| format!("%{}", n)).collect();
                    let mut l = Level::new(NumFmt::Decimal, &format!("{}.", text.join(".")), i);
                    l.para.indent_left = Some(360 * (i as i32 + 2));
                    l.para.hanging = Some(360 * (i as i32 + 1).min(3));
                    l
                }
                _ => {
                    // Word's hybrid multilevel: the chosen format at level 0, then a., i., repeating.
                    let (fmt, close) = match self {
                        ListKind::Decimal => (NumFmt::Decimal, "."),
                        ListKind::DecimalParen => (NumFmt::Decimal, ")"),
                        ListKind::LowerLetter => (NumFmt::LowerLetter, "."),
                        ListKind::LowerLetterParen => (NumFmt::LowerLetter, ")"),
                        ListKind::UpperLetter => (NumFmt::UpperLetter, "."),
                        ListKind::LowerRoman => (NumFmt::LowerRoman, "."),
                        ListKind::UpperRoman => (NumFmt::UpperRoman, "."),
                        _ => (NumFmt::Decimal, "."),
                    };
                    let (f, c) = match i % 3 {
                        0 => (fmt, close),
                        1 => (NumFmt::LowerLetter, close),
                        _ => (NumFmt::LowerRoman, "."),
                    };
                    let mut l = Level::new(f, &format!("%{}{}", i + 1, c), i);
                    if i % 3 == 2 {
                        l.align = Align::Right;
                        l.para.hanging = Some(180);
                    }
                    l
                }
            })
            .collect()
    }
}

impl Numbering {
    pub fn abstract_num(&self, id: i32) -> Option<&AbstractNum> {
        self.abstract_nums.iter().find(|a| a.id == id)
    }
    pub fn num(&self, id: i32) -> Option<&NumInstance> {
        self.nums.iter().find(|n| n.id == id)
    }

    /// The effective level definition for an instance, overrides applied.
    pub fn level(&self, num_id: i32, ilvl: u8) -> Option<Level> {
        let num = self.num(num_id)?;
        if let Some(o) = num.overrides.iter().find(|o| o.ilvl == ilvl) {
            if let Some(l) = &o.level {
                let mut l = l.clone();
                if let Some(s) = o.start {
                    l.start = s;
                }
                return Some(l);
            }
        }
        let a = self.abstract_num(num.abstract_id)?;
        let mut l = a.levels.get(ilvl as usize)?.clone();
        if let Some(o) = num.overrides.iter().find(|o| o.ilvl == ilvl) {
            if let Some(s) = o.start {
                l.start = s;
            }
        }
        Some(l)
    }

    /// The kind of an instance's top level (bullet or numbered), for toggles.
    pub fn is_bullet(&self, num_id: i32, ilvl: u8) -> bool {
        self.level(num_id, ilvl).map(|l| l.fmt.is_bullet()).unwrap_or(false)
    }

    fn next_abstract_id(&self) -> i32 {
        self.abstract_nums.iter().map(|a| a.id).max().map(|m| m + 1).unwrap_or(0)
    }
    fn next_num_id(&self) -> i32 {
        self.nums.iter().map(|n| n.id).max().map(|m| m + 1).unwrap_or(1)
    }

    /// Create a new list definition and instance; returns the `num_id`.
    pub fn add_list(&mut self, kind: ListKind) -> i32 {
        let aid = self.next_abstract_id();
        self.abstract_nums.push(AbstractNum { id: aid, levels: kind.levels(), raw: None });
        let nid = self.next_num_id();
        self.nums.push(NumInstance { id: nid, abstract_id: aid, overrides: Vec::new(), raw: None });
        self.dirty = true;
        nid
    }

    /// A new instance of an existing list that restarts level 0 at `start`.
    pub fn restart(&mut self, num_id: i32, start: i32) -> Option<i32> {
        let abstract_id = self.num(num_id)?.abstract_id;
        let nid = self.next_num_id();
        self.nums.push(NumInstance {
            id: nid,
            abstract_id,
            overrides: vec![LevelOverride { ilvl: 0, start: Some(start), level: None, raw: None }],
            raw: None,
        });
        self.dirty = true;
        Some(nid)
    }

    /// Find an existing instance whose level-0 definition matches `kind`
    /// exactly as `add_list` would create it (so repeated toggles reuse one
    /// definition instead of growing numbering.xml).
    pub fn find_kind(&self, kind: ListKind) -> Option<i32> {
        let want = kind.levels();
        self.nums
            .iter()
            .filter(|n| n.overrides.is_empty())
            .find(|n| self.abstract_num(n.abstract_id).map(|a| a.levels == want).unwrap_or(false))
            .map(|n| n.id)
    }
}

// ---------------------------------------------------------------------------
// Number formatting
// ---------------------------------------------------------------------------

pub fn format_number(fmt: &NumFmt, n: i32) -> String {
    match fmt {
        NumFmt::Decimal | NumFmt::Other(_) => n.to_string(),
        NumFmt::DecimalZero => {
            if (0..10).contains(&n) {
                format!("0{}", n)
            } else {
                n.to_string()
            }
        }
        NumFmt::LowerLetter => letters(n, false),
        NumFmt::UpperLetter => letters(n, true),
        NumFmt::LowerRoman => roman(n).to_lowercase(),
        NumFmt::UpperRoman => roman(n),
        NumFmt::Ordinal => ordinal(n),
        NumFmt::CardinalText => capitalize(&cardinal_text(n)),
        NumFmt::OrdinalText => capitalize(&ordinal_text(n)),
        NumFmt::Bullet | NumFmt::None => String::new(),
    }
}

/// a, b, …, z, aa, bb, … (Word's letter numbering repeats the letter).
fn letters(n: i32, upper: bool) -> String {
    if n < 1 {
        return n.to_string();
    }
    let idx = ((n - 1) % 26) as u8;
    let reps = ((n - 1) / 26 + 1) as usize;
    let c = (if upper { b'A' } else { b'a' } + idx) as char;
    std::iter::repeat(c).take(reps).collect()
}

fn roman(n: i32) -> String {
    if n < 1 || n > 3999 {
        return n.to_string();
    }
    let table = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut n = n;
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn ordinal(n: i32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

const ONES: [&str; 20] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

fn cardinal_text(n: i32) -> String {
    if n < 0 {
        return format!("minus {}", cardinal_text(-n));
    }
    if n < 20 {
        return ONES[n as usize].into();
    }
    if n < 100 {
        let t = TENS[(n / 10) as usize];
        return if n % 10 == 0 { t.into() } else { format!("{}-{}", t, ONES[(n % 10) as usize]) };
    }
    if n < 1000 {
        let h = format!("{} hundred", ONES[(n / 100) as usize]);
        return if n % 100 == 0 { h } else { format!("{} {}", h, cardinal_text(n % 100)) };
    }
    if n < 1_000_000 {
        let t = format!("{} thousand", cardinal_text(n / 1000));
        return if n % 1000 == 0 { t } else { format!("{} {}", t, cardinal_text(n % 1000)) };
    }
    n.to_string()
}

fn ordinal_text(n: i32) -> String {
    let c = cardinal_text(n);
    let (head, last) = match c.rfind(|ch: char| ch == ' ' || ch == '-') {
        Some(i) => (&c[..=i], &c[i + 1..]),
        None => ("", c.as_str()),
    };
    let last_ord = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{}th", w),
    };
    format!("{}{}", head, last_ord)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Bullets in symbol fonts arrive as private-use characters; show the glyph
/// they stand for.
pub fn bullet_glyph(text: &str, font: Option<&str>) -> String {
    let mut out = String::new();
    for c in text.chars() {
        out.push(match c {
            '\u{F0B7}' | '\u{F0A7}' if font.map_or(false, |f| f.starts_with("Wingdings")) => '▪',
            '\u{F0B7}' => '•',
            '\u{F0A7}' => '▪',
            '\u{F0D8}' => '➢',
            '\u{F0FC}' => '✓',
            '\u{F0A8}' => '□',
            '\u{F076}' => '❖',
            '\u{F0B2}' => '•',
            '\u{F0AB}' => '❖',
            '\u{F0D5}' => '◊',
            'o' if font.map_or(false, |f| f == "Courier New") => '◦',
            c if ('\u{F000}'..='\u{F0FF}').contains(&c) => '•',
            c => c,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Counters over a document
// ---------------------------------------------------------------------------

struct Counters {
    /// Per abstract definition: the current value at each level, or None
    /// before the level first appears.
    by_abstract: HashMap<i32, [Option<i32>; 9]>,
    seen_nums: std::collections::HashSet<i32>,
}

impl Document {
    /// Effective list reference of a paragraph (direct beats style); `None`
    /// for `numId 0`, which removes inherited numbering.
    pub fn list_ref(&self, para: usize) -> Option<ListRef> {
        let p = &self.paragraphs[para].props;
        if p.raw_block {
            return None;
        }
        let r = match p.list {
            Some(l) => Some(l),
            None => self.styles.resolve_para_style(p.style.as_deref()).list,
        };
        r.filter(|l| l.num_id > 0)
    }

    /// Labels for every paragraph, in document order.
    pub fn list_labels(&self) -> Vec<Option<ListLabel>> {
        let mut c = Counters { by_abstract: HashMap::new(), seen_nums: Default::default() };
        let mut out = Vec::with_capacity(self.paragraphs.len());
        for i in 0..self.paragraphs.len() {
            out.push(self.list_ref(i).and_then(|r| self.label_for(&mut c, r)));
        }
        out
    }

    fn label_for(&self, c: &mut Counters, r: ListRef) -> Option<ListLabel> {
        let num = self.numbering.num(r.num_id)?;
        let lvl = (r.level as usize).min(8);
        let level = self.numbering.level(r.num_id, lvl as u8)?;
        let counters = c.by_abstract.entry(num.abstract_id).or_insert([None; 9]);
        if c.seen_nums.insert(r.num_id) {
            // First use of this instance: its start overrides take effect.
            for o in &num.overrides {
                if let Some(s) = o.start {
                    counters[(o.ilvl as usize).min(8)] = Some(s - 1);
                }
            }
        }
        // Levels above this one that never appeared count as their start value.
        for (j, slot) in counters.iter_mut().enumerate().take(lvl) {
            if slot.is_none() {
                *slot = Some(self.numbering.level(r.num_id, j as u8).map(|l| l.start).unwrap_or(1));
            }
        }
        counters[lvl] = Some(counters[lvl].map(|v| v + 1).unwrap_or(level.start));
        for slot in counters.iter_mut().skip(lvl + 1) {
            *slot = None;
        }
        let text = if level.fmt.is_bullet() {
            bullet_glyph(&level.text, level.run.font.as_deref())
        } else {
            let mut s = String::new();
            let mut chars = level.text.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '%' {
                    if let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                        chars.next();
                        let j = (d as usize).saturating_sub(1).min(8);
                        let fmt = if j == lvl { level.fmt.clone() } else { self.numbering.level(r.num_id, j as u8).map(|l| l.fmt).unwrap_or(NumFmt::Decimal) };
                        let v = counters[j].unwrap_or(1);
                        s.push_str(&format_number(&fmt, v));
                        continue;
                    }
                }
                s.push(ch);
            }
            s
        };
        Some(ListLabel { text, level: lvl as u8, fmt: level.fmt.clone(), suffix: level.suffix, align: level.align, run: level.run.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {

        assert_eq!(format_number(&NumFmt::LowerLetter, 27), "aa");
        assert_eq!(format_number(&NumFmt::UpperRoman, 1994), "MCMXCIV");
        assert_eq!(format_number(&NumFmt::LowerRoman, 4), "iv");
        assert_eq!(format_number(&NumFmt::Ordinal, 22), "22nd");
        assert_eq!(format_number(&NumFmt::Ordinal, 13), "13th");
        assert_eq!(format_number(&NumFmt::CardinalText, 21), "Twenty-one");
        assert_eq!(format_number(&NumFmt::OrdinalText, 12), "Twelfth");
        assert_eq!(format_number(&NumFmt::OrdinalText, 40), "Fortieth");
        assert_eq!(format_number(&NumFmt::DecimalZero, 7), "07");
    }

    #[test]
    fn counters_nest_restart_and_continue() {
        let mut doc = Document::new();
        let n = doc.numbering.add_list(ListKind::Decimal);
        let outline = doc.numbering.add_list(ListKind::Outline);
        let restart = doc.numbering.restart(n, 1).unwrap();
        let items = [(n, 0), (n, 0), (n, 1), (n, 1), (n, 2), (n, 0), (outline, 0), (outline, 1), (outline, 1), (outline, 2), (restart, 0), (n, 0)];
        doc.paragraphs = items
            .iter()
            .map(|(id, lvl)| {
                let mut p = Paragraph::from_text("x");
                p.props.list = Some(ListRef { num_id: *id, level: *lvl });
                p
            })
            .collect();
        let labels: Vec<String> = doc.list_labels().into_iter().map(|l| l.unwrap().text).collect();
        assert_eq!(labels, ["1.", "2.", "a.", "b.", "i.", "3.", "1.", "1.1.", "1.2.", "1.2.1.", "1.", "2."]);
    }

    #[test]
    fn bullets_and_style_lists() {
        let mut doc = Document::new();
        let b = doc.numbering.add_list(ListKind::Bullet);
        let mut p = Paragraph::from_text("x");
        p.props.list = Some(ListRef { num_id: b, level: 1 });
        doc.paragraphs = vec![p.clone(), Paragraph::from_text("plain")];
        let l = doc.list_labels();
        assert_eq!(l[0].as_ref().unwrap().text, "◦");
        assert!(l[1].is_none());
        // numId 0 removes a style-supplied list.
        let mut st = crate::style::Style::para("ListBullet", "List Bullet");
        st.para.list = Some(ListRef { num_id: b, level: 0 });
        doc.styles.upsert(st);
        doc.paragraphs[1].props.style = Some("ListBullet".into());
        assert!(doc.list_labels()[1].is_some());
        doc.paragraphs[1].props.list = Some(ListRef { num_id: 0, level: 0 });
        assert!(doc.list_labels()[1].is_none());
    }
}
