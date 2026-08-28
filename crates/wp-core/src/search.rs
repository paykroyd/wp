//! Find and replace: plain text, regular expressions with capture groups,
//! case and whole-word options, formatting filters ("bold text in Heading
//! 2"), and code search ("the next page break"). See SPEC §7.1 P0-12.

use crate::document::Document;
use crate::model::*;
use crate::reveal;
use regex::Regex;

/// A formatting condition every character of a match must satisfy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatFilter {
    Bold,
    Italic,
    Underline,
    Strike,
    Highlight,
    Superscript,
    Subscript,
    /// Paragraph style id or name (case-insensitive).
    Style(String),
    Font(String),
    /// Half-points.
    Size(u16),
    Color(Rgb),
}

impl FormatFilter {
    pub fn label(&self) -> String {
        match self {
            FormatFilter::Bold => "bold".into(),
            FormatFilter::Italic => "italic".into(),
            FormatFilter::Underline => "underlined".into(),
            FormatFilter::Strike => "struck".into(),
            FormatFilter::Highlight => "highlighted".into(),
            FormatFilter::Superscript => "superscript".into(),
            FormatFilter::Subscript => "subscript".into(),
            FormatFilter::Style(s) => format!("in style {}", s),
            FormatFilter::Font(f) => format!("in font {}", f),
            FormatFilter::Size(s) => format!("at {}pt", *s as f32 / 2.0),
            FormatFilter::Color(c) => format!("in color #{}", c.hex()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The text (or pattern) to find; may be empty when filters or a code
    /// are given.
    pub text: String,
    pub regex: bool,
    /// `None` = smart case (sensitive when the text has an uppercase letter).
    pub case_sensitive: Option<bool>,
    pub whole_word: bool,
    pub filters: Vec<FormatFilter>,
    /// Find a code by its Reveal Codes label, e.g. `HPg`, `Tab`, `BOLD`,
    /// `Style:Heading1` (prefix match, case-insensitive).
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub range: Range,
    /// Capture groups (0 = whole match) when the query was a regex.
    pub captures: Vec<Option<String>>,
}

impl Query {
    /// Parse the search box syntax:
    ///
    /// - `[HPg]`, `[Tab]`, `[BOLD]`, `[Style:Heading1]` — find that code
    /// - `bold:`, `italic:`, `underline:`, `strike:`, `highlight:`,
    ///   `super:`, `sub:`, `style:Name` (or `style:"Two words"`),
    ///   `font:Name`, `size:12`, `color:FF0000` — formatting filters,
    ///   followed by optional text
    /// - `re:` — the rest is a regular expression
    /// - anything else is literal text
    pub fn parse(input: &str) -> Query {
        let mut q = Query::default();
        let s = input.trim_start();
        if s.starts_with('[') && s.ends_with(']') && s.len() > 2 {
            q.code = Some(s[1..s.len() - 1].to_string());
            return q;
        }
        let mut rest = s;
        loop {
            let t = rest.trim_start();
            let lower = t.to_ascii_lowercase();
            let simple = [
                ("bold:", FormatFilter::Bold),
                ("italic:", FormatFilter::Italic),
                ("underline:", FormatFilter::Underline),
                ("underlined:", FormatFilter::Underline),
                ("strike:", FormatFilter::Strike),
                ("highlight:", FormatFilter::Highlight),
                ("super:", FormatFilter::Superscript),
                ("sub:", FormatFilter::Subscript),
            ];
            if let Some((p, f)) = simple.iter().find(|(p, _)| lower.starts_with(p)) {
                q.filters.push(f.clone());
                rest = &t[p.len()..];
                continue;
            }
            if lower.starts_with("re:") {
                q.regex = true;
                rest = &t[3..];
                continue;
            }
            let valued = ["style:", "font:", "size:", "color:"];
            if let Some(p) = valued.iter().find(|p| lower.starts_with(*p)) {
                let after = &t[p.len()..];
                let (val, remainder) = if let Some(inner) = after.strip_prefix('"') {
                    match inner.find('"') {
                        Some(i) => (&inner[..i], &inner[i + 1..]),
                        None => (inner, ""),
                    }
                } else {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    (&after[..end], &after[end..])
                };
                let f = match *p {
                    "style:" => Some(FormatFilter::Style(val.to_string())),
                    "font:" => Some(FormatFilter::Font(val.to_string())),
                    "size:" => val.parse::<f32>().ok().map(|v| FormatFilter::Size((v * 2.0).round() as u16)),
                    _ => Rgb::parse_hex(val).map(FormatFilter::Color),
                };
                if let Some(f) = f {
                    q.filters.push(f);
                }
                rest = remainder;
                continue;
            }
            break;
        }
        q.text = rest.trim_start().to_string();
        q
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.filters.is_empty() && self.code.is_none()
    }

    /// One line describing the query, for the status line.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(c) = &self.code {
            parts.push(format!("code [{}]", c));
        }
        if !self.text.is_empty() {
            parts.push(if self.regex { format!("/{}/", self.text) } else { format!("“{}”", self.text) });
        }
        for f in &self.filters {
            parts.push(f.label());
        }
        if self.whole_word {
            parts.push("whole word".into());
        }
        if self.case_sensitive == Some(true) {
            parts.push("match case".into());
        }
        parts.join(" ")
    }

    fn compile(&self) -> Option<Regex> {
        if self.text.is_empty() {
            return None;
        }
        let sensitive = self.case_sensitive.unwrap_or_else(|| self.text.chars().any(|c| c.is_uppercase()));
        let body = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        let body = if self.whole_word { format!(r"\b(?:{})\b", body) } else { body };
        let pat = if sensitive { body } else { format!("(?i){}", body) };
        Regex::new(&pat).ok()
    }
}

/// A paragraph flattened for matching: text with a map back to item indices.
struct Flat {
    text: String,
    /// Item index of each char in `text`, plus one past the end.
    idx: Vec<usize>,
    /// Byte offset → char position.
    byte_to_char: Vec<usize>,
}

fn flatten(p: &Paragraph) -> Flat {
    let mut text = String::with_capacity(p.items.len());
    let mut idx = Vec::with_capacity(p.items.len() + 1);
    for (i, it) in p.items.iter().enumerate() {
        let c = match it {
            Item::Char(c) => *c,
            Item::Code(Code::Tab) => '\t',
            Item::Code(Code::LineBreak) => '\n',
            _ => continue,
        };
        text.push(c);
        idx.push(i);
    }
    idx.push(p.items.len());
    let mut byte_to_char = vec![0; text.len() + 1];
    for (ci, (bi, _)) in text.char_indices().enumerate() {
        byte_to_char[bi] = ci;
    }
    byte_to_char[text.len()] = idx.len() - 1;
    Flat { text, idx, byte_to_char }
}

fn style_matches(doc: &Document, para: usize, want: &str) -> bool {
    let pp = &doc.paragraphs[para].props;
    let id = pp.style.clone().or_else(|| doc.styles.default_para_style().map(|s| s.id.clone())).unwrap_or_default();
    if id.eq_ignore_ascii_case(want) {
        return true;
    }
    doc.styles.get(&id).map(|s| s.name.eq_ignore_ascii_case(want)).unwrap_or(false)
}

fn run_matches(props: &RunProps, f: &FormatFilter) -> bool {
    match f {
        FormatFilter::Bold => props.is_bold(),
        FormatFilter::Italic => props.is_italic(),
        FormatFilter::Underline => props.underline().is_some(),
        FormatFilter::Strike => props.is_strike(),
        FormatFilter::Highlight => props.highlight().is_some(),
        FormatFilter::Superscript => props.vert_align() == Some(VertAlign::Superscript),
        FormatFilter::Subscript => props.vert_align() == Some(VertAlign::Subscript),
        FormatFilter::Style(_) => true,
        FormatFilter::Font(f) => props.font.as_deref().map_or(false, |x| x.eq_ignore_ascii_case(f)),
        FormatFilter::Size(s) => props.size_hp() == *s,
        FormatFilter::Color(c) => props.color == Some(*c),
    }
}

/// Which chars of a paragraph satisfy every run-level filter.
fn char_ok(doc: &Document, para: usize, flat: &Flat, q: &Query) -> Vec<bool> {
    let runs = doc.runs(para);
    let filters: Vec<&FormatFilter> = q.filters.iter().filter(|f| !matches!(f, FormatFilter::Style(_))).collect();
    let mut ok = vec![true; flat.idx.len() - 1];
    if filters.is_empty() {
        return ok;
    }
    let mut ri = 0;
    for (ci, &item_idx) in flat.idx[..flat.idx.len() - 1].iter().enumerate() {
        while ri + 1 < runs.len() && runs[ri].end <= item_idx {
            ri += 1;
        }
        ok[ci] = filters.iter().all(|f| run_matches(&runs[ri].props, f));
    }
    ok
}

/// All matches within one paragraph, in order.
fn matches_in(doc: &Document, para: usize, q: &Query) -> Vec<Match> {
    let p = &doc.paragraphs[para];
    if p.props.raw_block {
        return Vec::new();
    }
    for f in &q.filters {
        if let FormatFilter::Style(s) = f {
            if !style_matches(doc, para, s) {
                return Vec::new();
            }
        }
    }
    if let Some(code) = &q.code {
        let want = code.to_ascii_lowercase();
        let mut out = Vec::new();
        for (_, label) in reveal::para_codes(&p.props) {
            if label.to_ascii_lowercase().trim_start_matches('[').starts_with(&want) {
                out.push(Match { range: Range { start: Pos::new(para, 0), end: Pos::new(para, 0) }, captures: Vec::new() });
                break;
            }
        }
        for (i, it) in p.items.iter().enumerate() {
            if let Item::Code(c) = it {
                let label = reveal::code_label(c).to_ascii_lowercase();
                if label.trim_start_matches('[').starts_with(&want) || (want == "hrt" && false) {
                    out.push(Match { range: Range { start: Pos::new(para, i), end: Pos::new(para, i + 1) }, captures: Vec::new() });
                }
            }
        }
        return out;
    }
    let flat = flatten(p);
    let ok = char_ok(doc, para, &flat, q);
    let mut out = Vec::new();
    match q.compile() {
        Some(re) => {
            for caps in re.captures_iter(&flat.text) {
                let m = caps.get(0).unwrap();
                let (cs, ce) = (flat.byte_to_char[m.start()], flat.byte_to_char[m.end()]);
                if cs == ce {
                    continue;
                }
                if !ok[cs..ce].iter().all(|&b| b) {
                    continue;
                }
                let captures = caps.iter().map(|c| c.map(|c| c.as_str().to_string())).collect();
                // End right after the last matched char, so codes that follow stay outside.
                out.push(Match { range: Range { start: Pos::new(para, flat.idx[cs]), end: Pos::new(para, flat.idx[ce - 1] + 1) }, captures });
            }
        }
        None => {
            // No text: every maximal stretch of chars that satisfies the
            // filters is a match (a whole paragraph for a pure style filter).
            let style_only = q.filters.iter().all(|f| matches!(f, FormatFilter::Style(_)));
            if q.filters.is_empty() {
                return out;
            }
            if style_only {
                if !p.items.is_empty() {
                    out.push(Match { range: Range { start: Pos::new(para, 0), end: Pos::new(para, p.items.len()) }, captures: Vec::new() });
                }
                return out;
            }
            let mut start: Option<usize> = None;
            for (ci, &b) in ok.iter().enumerate() {
                match (b, start) {
                    (true, None) => start = Some(ci),
                    (false, Some(s)) => {
                        out.push(Match { range: Range { start: Pos::new(para, flat.idx[s]), end: Pos::new(para, flat.idx[ci - 1] + 1) }, captures: Vec::new() });
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                out.push(Match { range: Range { start: Pos::new(para, flat.idx[s]), end: Pos::new(para, flat.idx[ok.len() - 1] + 1) }, captures: Vec::new() });
            }

        }
    }
    out
}

/// The next match from `from` (exclusive of matches starting before it when
/// searching forward, at or after it when backward), wrapping if asked.
pub fn find(doc: &Document, q: &Query, from: Pos, backward: bool, wrap: bool) -> Option<Match> {
    if q.is_empty() {
        return None;
    }
    let n = doc.paragraphs.len();
    let order: Vec<usize> = if backward {
        (0..=from.para.min(n - 1)).rev().chain(if wrap { (from.para + 1..n).rev().collect::<Vec<_>>() } else { vec![] }).collect()
    } else {
        (from.para..n).chain(if wrap { 0..from.para.min(n) } else { 0..0 }).collect()
    };
    for (k, pi) in order.iter().enumerate() {
        let ms = matches_in(doc, *pi, q);
        let first_para = k == 0 && *pi == from.para;
        if backward {
            for m in ms.into_iter().rev() {
                if first_para && m.range.start.idx >= from.idx {
                    continue;
                }
                return Some(m);
            }
        } else {
            for m in ms {
                if first_para && m.range.start.idx < from.idx {
                    continue;
                }
                return Some(m);
            }
        }
    }
    None
}

/// Every match in the document, capped.
pub fn find_all(doc: &Document, q: &Query, cap: usize) -> Vec<Match> {
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    for pi in 0..doc.paragraphs.len() {
        out.extend(matches_in(doc, pi, q));
        if out.len() >= cap {
            out.truncate(cap);
            break;
        }
    }
    out
}

/// Expand `$1`, `${2}`, `$0`/`$&` and `$$` in a replacement when the query
/// was a regex; literal otherwise.
pub fn expand_replacement(template: &str, m: &Match, regex: bool) -> String {
    if !regex {
        return template.to_string();
    }
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                out.push_str(m.captures.first().and_then(|c| c.as_deref()).unwrap_or(""));
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                for ch in chars.by_ref() {
                    if ch == '}' {
                        break;
                    }
                    name.push(ch);
                }
                if let Ok(n) = name.parse::<usize>() {
                    out.push_str(m.captures.get(n).and_then(|c| c.as_deref()).unwrap_or(""));
                }
            }
            Some(d) if d.is_ascii_digit() => {
                let mut n = 0usize;
                while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                    chars.next();
                    n = n * 10 + d as usize;
                }
                out.push_str(m.captures.get(n).and_then(|c| c.as_deref()).unwrap_or(""));
            }
            _ => out.push('$'),
        }
    }
    out
}

/// A short context line around a match, with the match itself marked.
pub fn context(doc: &Document, m: &Match, width: usize) -> String {
    let p = &doc.paragraphs[m.range.start.para];
    let text = |a: usize, b: usize| -> String { p.items[a.min(p.items.len())..b.min(p.items.len())].iter().filter_map(|i| i.as_char()).collect() };
    let before = text(0, m.range.start.idx);
    let hit = text(m.range.start.idx, m.range.end.idx);
    let after = text(m.range.end.idx, p.items.len());
    let side = width.saturating_sub(hit.chars().count() + 4) / 2;
    let b: String = before.chars().rev().take(side).collect::<Vec<_>>().into_iter().rev().collect();
    let a: String = after.chars().take(side).collect();
    let hit = if hit.is_empty() {
        match p.items.get(m.range.start.idx) {
            Some(Item::Code(c)) => reveal::code_label(c),
            _ => "¶".into(),
        }
    } else {
        hit
    };
    format!("{}{}«{}»{}{}", if before.chars().count() > side { "…" } else { "" }, b, hit, a, if after.chars().count() > side { "…" } else { "" }).replace('\n', "↵")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::from_text;

    #[test]
    fn parse_syntax() {
        let q = Query::parse("bold: style:\"Heading 2\" re:fo+");
        assert!(q.regex);
        assert_eq!(q.text, "fo+");
        assert_eq!(q.filters, vec![FormatFilter::Bold, FormatFilter::Style("Heading 2".into())]);
        assert_eq!(Query::parse("[HPg]").code.as_deref(), Some("HPg"));
        assert_eq!(Query::parse("plain words").text, "plain words");
    }

    #[test]
    fn regex_captures_and_replacement() {
        let doc = from_text("call 555-1234 or 555-9876", false);
        let q = Query { text: r"(\d{3})-(\d{4})".into(), regex: true, ..Default::default() };
        let all = find_all(&doc, &q, 100);
        assert_eq!(all.len(), 2);
        assert_eq!(expand_replacement("$2/$1 ($&)", &all[1], true), "9876/555 (555-9876)");
        assert_eq!(all[0].range.start.idx, 5);
        let back = find(&doc, &q, Pos::new(0, 20), true, false).unwrap();
        assert_eq!(back.range.start.idx, 17);
    }

    #[test]
    fn smart_case_and_whole_word() {
        let doc = from_text("Cat cat concatenate CAT", false);
        assert_eq!(find_all(&doc, &Query::parse("cat"), 100).len(), 4);
        assert_eq!(find_all(&doc, &Query::parse("Cat"), 100).len(), 1);
        let ww = Query { text: "cat".into(), whole_word: true, ..Default::default() };
        assert_eq!(find_all(&doc, &ww, 100).len(), 3);
    }

    #[test]
    fn format_and_code_search() {
        let mut doc = from_text("plain bold plain", false);
        doc.paragraphs[0].items.insert(6, Item::Code(Code::On(Attr::Bold(true))));
        doc.paragraphs[0].items.insert(11, Item::Code(Code::Off(AttrKind::Bold)));
        doc.paragraphs.push(Paragraph::from_text("second"));
        doc.paragraphs[1].props.style = Some("Heading1".into());
        doc.paragraphs[1].items.push(Item::Code(Code::PageBreak));
        let bold = find_all(&doc, &Query::parse("bold:"), 100);
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0].range, Range { start: Pos::new(0, 7), end: Pos::new(0, 11) });
        assert_eq!(find_all(&doc, &Query::parse("bold: plain"), 100).len(), 0);
        assert_eq!(find_all(&doc, &Query::parse("style:heading1"), 100).len(), 1);
        assert_eq!(find_all(&doc, &Query::parse("style:\"heading 1\" sec"), 100).len(), 1);
        let pg = find_all(&doc, &Query::parse("[HPg]"), 100);
        assert_eq!(pg.len(), 1);
        assert_eq!(pg[0].range.start, Pos::new(1, 6));
        assert!(context(&doc, &pg[0], 40).contains("[HPg]"));
        assert_eq!(find_all(&doc, &Query::parse("[Style:Heading1]"), 100).len(), 1);
    }
}
