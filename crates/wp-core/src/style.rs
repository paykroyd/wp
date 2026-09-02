//! Named styles and property resolution.

use crate::model::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StyleKind {
    Paragraph,
    Character,
    Table,
    Numbering,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub kind: StyleKind,
    pub based_on: Option<String>,
    pub next: Option<String>,
    pub para: ParaProps,
    pub run: RunProps,
    pub is_default: bool,
    pub hidden: bool,
    /// The `w:style` element exactly as read, if it came from a file and has
    /// not been modified. The writer emits this verbatim when present.
    pub raw_xml: Option<String>,
}

impl Style {
    pub fn para(id: &str, name: &str) -> Style {
        Style {
            id: id.to_string(),
            name: name.to_string(),
            kind: StyleKind::Paragraph,
            based_on: None,
            next: None,
            para: ParaProps::default(),
            run: RunProps::default(),
            is_default: false,
            hidden: false,
            raw_xml: None,
        }
    }
    pub fn character(id: &str, name: &str) -> Style {
        Style { kind: StyleKind::Character, ..Style::para(id, name) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSheet {
    pub styles: Vec<Style>,
    pub default_run: RunProps,
    pub default_para: ParaProps,
    /// Unknown children of `w:docDefaults`/`w:styles` preserved verbatim
    /// (e.g. `w:latentStyles`).
    pub opaque: Vec<String>,
    /// Set when a style has been added or changed since the sheet was read;
    /// the writer then regenerates `styles.xml` instead of copying it.
    pub dirty: bool,
}

impl Default for StyleSheet {
    fn default() -> Self {
        StyleSheet::builtin()
    }
}

impl StyleSheet {
    pub fn empty() -> StyleSheet {
        StyleSheet {
            styles: Vec::new(),
            default_run: RunProps::default(),
            default_para: ParaProps::default(),
            opaque: Vec::new(),
            dirty: false,
        }
    }

    /// The style set a new document starts with — Word's defaults, so that a
    /// document created in `wp` looks native when opened in Word.
    pub fn builtin() -> StyleSheet {
        let heading_color = Rgb(0x2F, 0x54, 0x96);
        let mut s = StyleSheet::empty();
        s.default_run = RunProps {
            font: Some("Calibri".into()),
            size: Some(22),
            ..Default::default()
        };
        s.default_para = ParaProps {
            space_after: Some(160),
            line_spacing: Some(LineSpacing::Auto(259)),
            widow_control: Some(true),
            ..Default::default()
        };

        let mut normal = Style::para("Normal", "Normal");
        normal.is_default = true;
        s.styles.push(normal);

        let h = |id: &str, name: &str, size: u16, before: Twips, color: Rgb, lvl: u8| {
            let mut st = Style::para(id, name);
            st.based_on = Some("Normal".into());
            st.next = Some("Normal".into());
            st.para.keep_next = Some(true);
            st.para.keep_lines = Some(true);
            st.para.space_before = Some(before);
            st.para.space_after = Some(0);
            st.para.outline_level = Some(lvl);
            st.run.font = Some("Calibri Light".into());
            st.run.size = Some(size);
            st.run.color = Some(color);
            st
        };
        s.styles.push(h("Heading1", "heading 1", 32, 240, heading_color, 0));
        s.styles.push(h("Heading2", "heading 2", 26, 40, heading_color, 1));
        s.styles.push(h("Heading3", "heading 3", 24, 40, Rgb(0x1F, 0x37, 0x63), 2));
        s.styles.push(h("Heading4", "heading 4", 22, 40, heading_color, 3));

        let mut title = Style::para("Title", "Title");
        title.based_on = Some("Normal".into());
        title.next = Some("Normal".into());
        title.para.space_after = Some(0);
        title.para.line_spacing = Some(LineSpacing::Auto(240));
        title.run.font = Some("Calibri Light".into());
        title.run.size = Some(56);
        s.styles.push(title);

        let mut subtitle = Style::para("Subtitle", "Subtitle");
        subtitle.based_on = Some("Normal".into());
        subtitle.next = Some("Normal".into());
        subtitle.para.space_after = Some(160);
        subtitle.run.size = Some(28);
        subtitle.run.color = Some(Rgb(0x59, 0x59, 0x59));
        s.styles.push(subtitle);

        let mut quote = Style::para("Quote", "Quote");
        quote.based_on = Some("Normal".into());
        quote.next = Some("Normal".into());
        quote.para.align = Some(Align::Center);
        quote.para.space_before = Some(200);
        quote.para.space_after = Some(160);
        quote.run.italic = Some(true);
        quote.run.color = Some(Rgb(0x40, 0x40, 0x40));
        s.styles.push(quote);

        let mut lp = Style::para("ListParagraph", "List Paragraph");
        lp.based_on = Some("Normal".into());
        lp.para.indent_left = Some(720);
        s.styles.push(lp);

        let mut ns = Style::para("NoSpacing", "No Spacing");
        ns.para.space_after = Some(0);
        ns.para.line_spacing = Some(LineSpacing::Auto(240));
        s.styles.push(ns);

        // Word's Header and Footer: no spacing, a centre tab and a right tab.
        for (id, name) in [("Header", "header"), ("Footer", "footer")] {
            let mut st = Style::para(id, name);
            st.based_on = Some("Normal".into());
            st.para.space_after = Some(0);
            st.para.line_spacing = Some(LineSpacing::Auto(240));
            st.para.tabs = vec![
                TabStop { pos: 4680, kind: TabKind::Center, leader: TabLeader::None, clear: false },
                TabStop { pos: 9360, kind: TabKind::Right, leader: TabLeader::None, clear: false },
            ];
            s.styles.push(st);
        }

        let mut strong = Style::character("Strong", "Strong");
        strong.run.bold = Some(true);
        s.styles.push(strong);
        let mut em = Style::character("Emphasis", "Emphasis");
        em.run.italic = Some(true);
        s.styles.push(em);

        let mut dpf = Style::character("DefaultParagraphFont", "Default Paragraph Font");
        dpf.is_default = true;
        dpf.hidden = true;
        s.styles.push(dpf);
        s
    }

    pub fn get(&self, id: &str) -> Option<&Style> {
        self.styles.iter().find(|s| s.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Style> {
        self.styles.iter_mut().find(|s| s.id == id)
    }
    /// Look a style up by id, or by display name (case-insensitive).
    pub fn find(&self, id_or_name: &str) -> Option<&Style> {
        self.get(id_or_name).or_else(|| {
            let n = id_or_name.to_ascii_lowercase();
            self.styles.iter().find(|s| s.name.to_ascii_lowercase() == n)
        })
    }
    pub fn default_para_style(&self) -> Option<&Style> {
        self.styles
            .iter()
            .find(|s| s.kind == StyleKind::Paragraph && s.is_default)
            .or_else(|| self.styles.iter().find(|s| s.kind == StyleKind::Paragraph))
    }
    pub fn paragraph_styles(&self) -> impl Iterator<Item = &Style> {
        self.styles.iter().filter(|s| s.kind == StyleKind::Paragraph && !s.hidden)
    }
    pub fn character_styles(&self) -> impl Iterator<Item = &Style> {
        self.styles.iter().filter(|s| s.kind == StyleKind::Character && !s.hidden)
    }

    /// The inheritance chain for a style, root first. Guards against cycles.
    pub fn chain(&self, id: &str) -> Vec<&Style> {
        let mut out = Vec::new();
        let mut cur = self.get(id);
        let mut guard = 0;
        while let Some(s) = cur {
            out.push(s);
            guard += 1;
            if guard > 32 {
                break;
            }
            cur = s.based_on.as_deref().and_then(|b| self.get(b));
            if let Some(c) = cur {
                if out.iter().any(|x| x.id == c.id) {
                    break;
                }
            }
        }
        out.reverse();
        out
    }

    /// Effective paragraph properties from the style chain (without the
    /// paragraph's own direct formatting).
    pub fn resolve_para_style(&self, style_id: Option<&str>) -> ParaProps {
        let mut p = self.default_para.clone();
        let id = style_id.map(str::to_string).or_else(|| self.default_para_style().map(|s| s.id.clone()));
        if let Some(id) = id {
            for s in self.chain(&id) {
                p = p.merge(&s.para);
            }
        }
        p
    }

    /// Effective run properties contributed by a paragraph style chain.
    pub fn resolve_para_style_run(&self, style_id: Option<&str>) -> RunProps {
        let mut r = self.default_run.clone();
        let id = style_id.map(str::to_string).or_else(|| self.default_para_style().map(|s| s.id.clone()));
        if let Some(id) = id {
            for s in self.chain(&id) {
                r = r.merge(&s.run);
            }
        }
        r
    }

    /// Run properties contributed by a character style chain.
    pub fn resolve_char_style(&self, style_id: &str) -> RunProps {
        let mut r = RunProps::default();
        for s in self.chain(style_id) {
            r = r.merge(&s.run);
        }
        r
    }

    pub fn upsert(&mut self, style: Style) {
        self.dirty = true;
        if let Some(existing) = self.styles.iter_mut().find(|s| s.id == style.id) {
            *existing = style;
        } else {
            self.styles.push(style);
        }
    }
}
