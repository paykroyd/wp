//! Plain-text import and export.

use crate::document::Document;
use crate::model::*;

/// Build a document from plain text. Blank-line separated blocks become
/// paragraphs when `reflow` is set; otherwise every line is a paragraph.
pub fn from_text(text: &str, reflow: bool) -> Document {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut paras: Vec<Paragraph> = Vec::new();
    if reflow {
        let mut cur = String::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                if !cur.is_empty() {
                    paras.push(para_from_str(&cur));
                    cur.clear();
                }
                paras.push(Paragraph::new());
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(line.trim());
            }
        }
        if !cur.is_empty() {
            paras.push(para_from_str(&cur));
        }
    } else {
        for line in text.split('\n') {
            paras.push(para_from_str(line.trim_end_matches('\r')));
        }
        if text.ends_with('\n') && paras.len() > 1 {
            paras.pop();
        }
    }
    let mut doc = Document::from_paragraphs(paras);
    // Plain text has no formatting; use a monospace default to match.
    doc.styles.dirty = true;
    doc
}

fn para_from_str(s: &str) -> Paragraph {
    let mut p = Paragraph::new();
    for c in s.chars() {
        if c == '\t' {
            p.items.push(Item::Code(Code::Tab));
        } else {
            p.items.push(Item::Char(c));
        }
    }
    p
}

/// Export as plain text. `wrap` wraps paragraphs at that many columns.
pub fn to_text(doc: &Document, wrap: Option<usize>) -> String {
    let mut out = String::new();
    for (i, p) in doc.paragraphs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut line = String::new();
        for it in &p.items {
            match it {
                Item::Char(c) => line.push(*c),
                Item::Code(Code::Tab) => line.push('\t'),
                Item::Code(Code::LineBreak) => line.push('\n'),
                Item::Code(Code::PageBreak) => line.push('\u{c}'),
                _ => {}
            }
        }
        match wrap {
            Some(w) if w > 10 => out.push_str(&wrap_text(&line, w)),
            _ => out.push_str(&line),
        }
    }
    out.push('\n');
    out
}

fn wrap_text(s: &str, width: usize) -> String {
    let mut out = String::new();
    for (i, para) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut col = 0;
        for word in para.split(' ') {
            let w = unicode_width::UnicodeWidthStr::width(word);
            if col > 0 && col + 1 + w > width {
                out.push('\n');
                col = 0;
            } else if col > 0 {
                out.push(' ');
                col += 1;
            }
            out.push_str(word);
            col += w;
        }
    }
    out
}
