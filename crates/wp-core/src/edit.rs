//! Primitive, invertible edit operations. All mutation of a `Document` goes
//! through `Document::apply`, which returns the inverse operation.

use crate::document::Document;
use crate::model::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Insert { at: Pos, items: Vec<Item> },
    Delete { at: Pos, len: usize },
    /// Split the paragraph at `at`; the tail becomes paragraph `at.para + 1`
    /// with `props` (or a copy of the original's if None).
    Split { at: Pos, props: Option<ParaProps> },
    /// Join paragraph `para + 1` onto the end of `para`.
    Join { para: usize },
    SetParaProps { para: usize, props: ParaProps },
    /// Replace the whole paragraph's items (used by attribute rewrites).
    ReplaceItems { para: usize, items: Vec<Item> },
    SetSection(SectionProps),
}

impl Document {
    pub fn apply(&mut self, op: Op) -> Op {
        match op {
            Op::Insert { at, items } => {
                let p = &mut self.paragraphs[at.para];
                let n = items.len();
                let idx = at.idx.min(p.items.len());
                p.items.splice(idx..idx, items);
                Op::Delete { at: Pos::new(at.para, idx), len: n }
            }
            Op::Delete { at, len } => {
                let p = &mut self.paragraphs[at.para];
                let end = (at.idx + len).min(p.items.len());
                let removed: Vec<Item> = p.items.drain(at.idx..end).collect();
                Op::Insert { at, items: removed }
            }
            Op::Split { at, props } => {
                let p = &mut self.paragraphs[at.para];
                let idx = at.idx.min(p.items.len());
                let tail: Vec<Item> = p.items.drain(idx..).collect();
                let props = props.unwrap_or_else(|| p.props.clone());
                self.paragraphs.insert(at.para + 1, Paragraph { props, items: tail });
                Op::Join { para: at.para }
            }
            Op::Join { para } => {
                let next = self.paragraphs.remove(para + 1);
                let p = &mut self.paragraphs[para];
                let at = Pos::new(para, p.items.len());
                p.items.extend(next.items);
                Op::Split { at, props: Some(next.props) }
            }
            Op::SetParaProps { para, props } => {
                let old = std::mem::replace(&mut self.paragraphs[para].props, props);
                Op::SetParaProps { para, props: old }
            }
            Op::ReplaceItems { para, items } => {
                let old = std::mem::replace(&mut self.paragraphs[para].items, items);
                Op::ReplaceItems { para, items: old }
            }
            Op::SetSection(s) => {
                let old = std::mem::replace(&mut self.section, s);
                Op::SetSection(old)
            }
        }
    }
}
