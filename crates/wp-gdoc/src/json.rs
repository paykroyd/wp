//! Small accessors over the Docs API JSON: dimensions, colours, enums.

use serde_json::{json, Value};
use wp_core::model::{Align, Highlight, Rgb, Twips};

pub fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub fn i64_of(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

pub fn bool_of(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// A `Dimension` (`{"magnitude": 36, "unit": "PT"}`) in twips. A dimension
/// with no magnitude is zero.
pub fn dim_twips(v: Option<&Value>) -> Option<Twips> {
    let o = v?.as_object()?;
    let mag = o.get("magnitude").and_then(Value::as_f64).unwrap_or(0.0);
    Some((mag * 20.0).round() as Twips)
}

pub fn twips_dim(t: Twips) -> Value {
    json!({ "magnitude": t as f64 / 20.0, "unit": "PT" })
}

/// An `OptionalColor`. `{}` (transparent) and a missing value are both `None`.
pub fn color_rgb(v: Option<&Value>) -> Option<Rgb> {
    let rgb = v?.get("color")?.get("rgbColor")?.as_object()?;
    let ch = |k: &str| (rgb.get(k).and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Rgb(ch("red"), ch("green"), ch("blue")))
}

pub fn rgb_color(c: Rgb) -> Value {
    let mut m = serde_json::Map::new();
    let f = |x: u8| Value::from(x as f64 / 255.0);
    if c.0 != 0 {
        m.insert("red".into(), f(c.0));
    }
    if c.1 != 0 {
        m.insert("green".into(), f(c.1));
    }
    if c.2 != 0 {
        m.insert("blue".into(), f(c.2));
    }
    json!({ "color": { "rgbColor": Value::Object(m) } })
}

/// Word's highlight palette, which is what `wp` can model as a background.
pub fn highlight_rgb(h: Highlight) -> Option<Rgb> {
    use Highlight::*;
    Some(match h {
        None => return Option::None,
        Yellow => Rgb(0xFF, 0xFF, 0),
        Green => Rgb(0, 0xFF, 0),
        Cyan => Rgb(0, 0xFF, 0xFF),
        Magenta => Rgb(0xFF, 0, 0xFF),
        Blue => Rgb(0, 0, 0xFF),
        Red => Rgb(0xFF, 0, 0),
        DarkBlue => Rgb(0, 0, 0x80),
        DarkCyan => Rgb(0, 0x80, 0x80),
        DarkGreen => Rgb(0, 0x80, 0),
        DarkMagenta => Rgb(0x80, 0, 0x80),
        DarkRed => Rgb(0x80, 0, 0),
        DarkYellow => Rgb(0x80, 0x80, 0),
        DarkGray => Rgb(0x80, 0x80, 0x80),
        LightGray => Rgb(0xC0, 0xC0, 0xC0),
        Black => Rgb(0, 0, 0),
        White => Rgb(0xFF, 0xFF, 0xFF),
    })
}

pub fn rgb_highlight(c: Rgb) -> Option<Highlight> {
    Highlight::all().iter().copied().find(|h| highlight_rgb(*h) == Some(c))
}

pub fn align_from(s: &str) -> Option<Align> {
    Some(match s {
        "START" => Align::Left,
        "CENTER" => Align::Center,
        "END" => Align::Right,
        "JUSTIFIED" => Align::Justify,
        _ => return None,
    })
}

pub fn align_name(a: Align) -> &'static str {
    match a {
        Align::Left => "START",
        Align::Center => "CENTER",
        Align::Right => "END",
        Align::Justify => "JUSTIFIED",
    }
}

/// Docs named style type ↔ `wp` style id. `NORMAL_TEXT` is no style.
pub fn style_id_from(named: &str) -> Option<String> {
    Some(match named {
        "TITLE" => "Title".into(),
        "SUBTITLE" => "Subtitle".into(),
        s if s.starts_with("HEADING_") => format!("Heading{}", &s[8..]),
        _ => return None,
    })
}

pub fn named_style_of(style: Option<&str>) -> &'static str {
    match style {
        Some("Title") => "TITLE",
        Some("Subtitle") => "SUBTITLE",
        Some("Heading1") => "HEADING_1",
        Some("Heading2") => "HEADING_2",
        Some("Heading3") => "HEADING_3",
        Some("Heading4") => "HEADING_4",
        Some("Heading5") => "HEADING_5",
        Some("Heading6") => "HEADING_6",
        _ => "NORMAL_TEXT",
    }
}

pub fn utf16_len(c: char) -> i64 {
    c.len_utf16() as i64
}
