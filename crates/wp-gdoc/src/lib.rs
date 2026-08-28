//! Google Docs as a native format for `wp` (DESIGN.md §6a).
//!
//! `read` turns a `documents.get` response into a `Document` plus a
//! `Baseline`; `diff` turns the edits made since into the minimal
//! `documents.batchUpdate` requests; `batch_update` wraps them with the
//! revision guard. No networking lives here — the binary fetches and posts.

pub mod json;
pub mod project;
pub mod read;
pub mod write;

pub use read::{read, Baseline, Loaded};
pub use write::{batch_update, diff};

use wp_core::model::*;
use wp_core::Document;

/// Remove what only Google Docs can hold — preserved elements kept as JSON —
/// so the document can be written as `.docx`, Markdown or text. Returns a
/// label for each thing dropped.
pub fn detach(doc: &mut Document) -> Vec<String> {
    let mut dropped = Vec::new();
    let mut i = 0;
    while i < doc.paragraphs.len() {
        let n = doc.paragraphs.len();
        let p = &mut doc.paragraphs[i];
        let is_gdoc_block = p.props.raw_block && p.items.iter().any(|it| matches!(it, Item::Code(Code::Opaque(o)) if o.xml.starts_with('{')));
        if is_gdoc_block {
            if let Some(Item::Code(Code::Opaque(o))) = p.items.first() {
                dropped.push(o.label.clone());
            }
            if n > 1 {
                doc.paragraphs.remove(i);
                continue;
            }
            p.props.raw_block = false;
            p.items.clear();
        } else {
            p.items.retain(|it| match it {
                Item::Code(Code::Opaque(o)) if o.xml.starts_with('{') => {
                    dropped.push(o.label.clone());
                    false
                }
                _ => true,
            });
        }
        i += 1;
    }
    for f in &mut doc.footnotes {
        for p in &mut f.paragraphs {
            p.items.retain(|it| match it {
                Item::Code(Code::Opaque(o)) if o.xml.starts_with('{') => {
                    dropped.push(o.label.clone());
                    false
                }
                _ => true,
            });
        }
    }
    dropped
}
