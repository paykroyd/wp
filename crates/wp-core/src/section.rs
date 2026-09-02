//! Sections: which page setup governs each paragraph.
//!
//! A section ends at a paragraph carrying `props.sect_break` (the `.docx`
//! convention: the `w:sectPr` sits in the last paragraph's properties); the
//! paragraphs after the last break belong to `Document::section`. Nothing
//! is stored per paragraph beyond that, so this module computes the map.

use crate::document::Document;
use crate::layout::{self, ParaLayout};
use crate::model::*;

/// Every section of a document in order, and the section of each paragraph.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Sections {
    pub list: Vec<SectionProps>,
    /// Index into `list` for each paragraph.
    pub of: Vec<u32>,
}

impl Sections {
    pub fn build(doc: &Document) -> Sections {
        let mut list = Vec::new();
        let mut of = Vec::with_capacity(doc.paragraphs.len());
        for p in &doc.paragraphs {
            of.push(list.len() as u32);
            if let Some(s) = &p.props.sect_break {
                list.push(s.clone());
            }
        }
        list.push(doc.section.clone());
        Sections { list, of }
    }

    pub fn section_of(&self, para: usize) -> &SectionProps {
        let i = self.of.get(para).copied().unwrap_or(self.list.len() as u32 - 1) as usize;
        &self.list[i.min(self.list.len() - 1)]
    }

    pub fn index_of(&self, para: usize) -> usize {
        self.of.get(para).copied().unwrap_or(self.list.len() as u32 - 1) as usize
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

impl Document {
    /// The section properties governing `para`: those of the next section
    /// break at or after it, else the document's final section. Scans
    /// forward; callers that need this for every paragraph should build a
    /// [`Sections`] instead.
    pub fn section_at(&self, para: usize) -> &SectionProps {
        for p in self.paragraphs.iter().skip(para) {
            if let Some(s) = &p.props.sect_break {
                return s;
            }
        }
        &self.section
    }

    /// The paragraph whose `sect_break` governs `para`, or `None` when the
    /// document's final section does.
    pub fn section_owner(&self, para: usize) -> Option<usize> {
        (para..self.paragraphs.len()).find(|&i| self.paragraphs[i].props.sect_break.is_some())
    }

    /// 1-based section number of `para`.
    pub fn section_number(&self, para: usize) -> usize {
        1 + self.paragraphs.iter().take(para).filter(|p| p.props.sect_break.is_some()).count()
    }

    pub fn section_count(&self) -> usize {
        1 + self.paragraphs.iter().filter(|p| p.props.sect_break.is_some()).count()
    }

    /// Paragraph indices that end a section.
    pub fn section_breaks(&self) -> Vec<usize> {
        self.paragraphs.iter().enumerate().filter(|(_, p)| p.props.sect_break.is_some()).map(|(i, _)| i).collect()
    }
}

/// A document holding `paragraphs` with `doc`'s styles and numbering, for
/// laying out a header or footer body (or editing one) as if it were a
/// document of its own.
pub fn scratch_doc(doc: &Document, paragraphs: &[Paragraph]) -> Document {
    let mut d = Document::new();
    d.styles = doc.styles.clone();
    d.numbering = doc.numbering.clone();
    d.section = doc.section.clone();
    if !paragraphs.is_empty() {
        d.paragraphs = paragraphs.to_vec();
    }
    d
}

/// Lay out a header or footer body against the text width of `sect`.
pub fn layout_body(doc: &Document, hf: &HeaderFooter, sect: &SectionProps) -> Vec<ParaLayout> {
    let d = scratch_doc(doc, &hf.paragraphs);
    let labels = d.list_labels();
    let mut plain = sect.clone();
    plain.columns = 1;
    (0..d.paragraphs.len()).map(|i| layout::layout_paragraph_in(&d, i, labels.get(i).and_then(|l| l.as_ref()), &plain)).collect()
}

/// Height of a laid-out body: every line plus paragraph spacing.
pub fn body_height(layouts: &[ParaLayout]) -> Twips {
    layouts.iter().map(|l| l.space_before + l.height() + l.space_after).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_map_paragraphs() {
        let mut d = Document::from_paragraphs(vec![Paragraph::from_text("a"), Paragraph::from_text("b"), Paragraph::from_text("c")]);
        let mut s = SectionProps::default();
        s.margin_left = 2000;
        d.paragraphs[1].props.sect_break = Some(s);
        let secs = Sections::build(&d);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs.of, vec![0, 0, 1]);
        assert_eq!(secs.section_of(0).margin_left, 2000);
        assert_eq!(secs.section_of(2).margin_left, 1440);
        assert_eq!(d.section_at(1).margin_left, 2000);
        assert_eq!(d.section_owner(0), Some(1));
        assert_eq!(d.section_owner(2), None);
        assert_eq!(d.section_number(2), 2);
        assert_eq!(d.section_count(), 2);
    }
}
