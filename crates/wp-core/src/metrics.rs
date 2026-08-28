//! Embedded font metrics. See DESIGN.md §5.2.

use crate::model::{RunProps, Twips};

pub struct FontTable {
    pub name: &'static str,
    /// (winAscent + winDescent) / unitsPerEm — the height of "single" spacing.
    pub line_height: f32,
    pub avg_width: u16,
    pub avg_width_bold: u16,
    /// Sorted codepoints.
    pub codepoints: &'static [u32],
    pub widths: &'static [u16],
    pub widths_bold: &'static [u16],
}

impl FontTable {
    /// Advance width of `c` in 1/1000 em.
    pub fn width(&self, c: char, bold: bool) -> u16 {
        let table = if bold { self.widths_bold } else { self.widths };
        match self.codepoints.binary_search(&(c as u32)) {
            Ok(i) if table[i] != 0 => table[i],
            _ => {
                // East Asian wide characters are typically a full em.
                if unicode_width::UnicodeWidthChar::width(c) == Some(2) {
                    1000
                } else if bold {
                    self.avg_width_bold
                } else {
                    self.avg_width
                }
            }
        }
    }
}

/// Pick the metrics table for a font family name.
pub fn table_for(font: &str) -> &'static FontTable {
    use crate::metrics_tables::*;
    let f = font.trim().to_ascii_lowercase();
    for t in ALL {
        if t.name.eq_ignore_ascii_case(&f) {
            return t;
        }
    }
    let has = |s: &str| f.contains(s);
    if has("mono") || has("courier") || has("consolas") || has("menlo") || has("lucida console") {
        &COURIER_NEW
    } else if has("calibri") || has("carlito") || has("aptos") || has("segoe") || has("candara") || has("corbel") {
        &CALIBRI
    } else if has("cambria") || has("caladea") {
        &CAMBRIA
    } else if has("times")
        || has("serif")
        || has("georgia")
        || has("garamond")
        || has("book antiqua")
        || has("palatino")
        || has("baskerville")
        || has("century")
        || has("bookman")
        || has("minion")
        || has("liberation serif")
    {
        &TIMES_NEW_ROMAN
    } else {
        &ARIAL
    }
}

/// Advance width of `c` in twips for the given run properties.
pub fn advance(props: &RunProps, c: char) -> Twips {
    let table = table_for(props.font.as_deref().unwrap_or("Calibri"));
    let w = table.width(c, props.is_bold()) as i32;
    let size_hp = props.size_hp() as i32;
    // width/1000 em * size_hp/2 pt * 20 twips/pt = width * size_hp / 100
    let mut tw = w * size_hp / 100;
    if props.vert_align().is_some() {
        tw = tw * 2 / 3; // Word renders super/subscript at ~65%
    }
    tw
}

/// Height of a single-spaced line in twips for the given run properties.
pub fn line_height(props: &RunProps) -> Twips {
    let table = table_for(props.font.as_deref().unwrap_or("Calibri"));
    let size_hp = props.size_hp() as f32;
    // size_hp/2 pt * 20 twips/pt * factor
    (size_hp * 10.0 * table.line_height).round() as Twips
}
