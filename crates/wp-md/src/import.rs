use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use wp_core::model::*;
use wp_core::numbering::ListKind;
use wp_core::style::Style;
use wp_core::Document;
use wp_docx::xml::escape_attr;

const MONO: &str = "Courier New";

struct ListCtx {
    num_id: i32,
    level: u8,
    /// A paragraph in this item has already taken the number.
    numbered: bool,
}

struct Builder {
    doc: Document,
    /// Items of the paragraph under construction, if any.
    cur: Option<Vec<Item>>,
    cur_props: ParaProps,
    /// Open inline attributes, innermost last.
    attrs: Vec<Attr>,
    lists: Vec<ListCtx>,
    quote_depth: usize,
    in_code_block: bool,
    code_buf: String,
    /// Table under construction: rows of cells (each cell a list of
    /// paragraphs), plus the cell being filled.
    table: Option<(Vec<Vec<Vec<Paragraph>>>, Vec<Paragraph>)>,

    in_table_cell: bool,
    footnote: Option<(String, Vec<Paragraph>)>,
    footnote_ids: Vec<String>,
    /// Hyperlink wrapper ids, so Open/Close pair up.
    next_wrapper: u32,
    next_rel: usize,
    bullet_num: Option<i32>,
    /// Task list marker to prefix the next text with.
    pending_prefix: Option<String>,
}

impl Builder {
    fn new() -> Builder {
        let mut doc = Document::new();
        doc.styles.dirty = true;
        if doc.styles.get("Hyperlink").is_none() {
            let mut h = Style::character("Hyperlink", "Hyperlink");
            h.run.color = Some(Rgb(0x05, 0x63, 0xC1));
            h.run.underline = Some(Some(Underline::Single));
            doc.styles.upsert(h);
        }
        doc.paragraphs.clear();
        Builder {
            doc,
            cur: None,
            cur_props: ParaProps::default(),
            attrs: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            in_code_block: false,
            code_buf: String::new(),
            table: None,
            in_table_cell: false,
            footnote: None,
            footnote_ids: Vec::new(),
            next_wrapper: 1,
            next_rel: 1,
            bullet_num: None,
            pending_prefix: None,
        }
    }

    fn footnote_id(&mut self, label: &str) -> i32 {
        match self.footnote_ids.iter().position(|l| l == label) {
            Some(i) => i as i32 + 1,
            None => {
                self.footnote_ids.push(label.to_string());
                self.footnote_ids.len() as i32
            }
        }
    }

    /// Paragraph properties for a block starting now: list membership,
    /// quote indent, in that order of precedence.
    fn block_props(&mut self) -> ParaProps {
        let mut p = ParaProps::default();
        if let Some(l) = self.lists.last_mut() {
            if l.numbered {
                // Continuation paragraph of the same item: indented, unnumbered.
                p.indent_left = Some(720 * (l.level as i32 + 1));
            } else {
                p.list = Some(ListRef { num_id: l.num_id, level: l.level });
                p.style = Some("ListParagraph".into());
                l.numbered = true;
            }
        }
        if self.quote_depth > 0 {
            let ind = 720 * self.quote_depth as i32;
            p.indent_left = Some(p.indent_left.unwrap_or(0) + ind);
            p.indent_right = Some(360);
            p.borders = Some(ParaBorders { left: Some(Border { style: BorderStyle::Single, size: 12, color: Some(Rgb(0xA0, 0xA0, 0xA0)), space: 8 }), ..Default::default() });
        }
        p
    }

    fn open_para(&mut self, props: ParaProps) {
        self.flush_para();
        self.cur = Some(Vec::new());
        self.cur_props = props;
    }

    fn ensure_para(&mut self) {
        if self.cur.is_none() {
            let props = self.block_props();
            self.open_para(props);
        }
    }

    fn flush_para(&mut self) {
        if let Some(items) = self.cur.take() {
            let mut props = std::mem::take(&mut self.cur_props);
            props.touch();
            let para = Paragraph { props, items };
            self.push_para(para);
        }
    }

    fn push_para(&mut self, para: Paragraph) {
        if let Some((_, cell)) = self.table.as_mut().filter(|_| self.in_table_cell) {
            cell.push(para);
        } else if let Some((_, paras)) = self.footnote.as_mut() {
            paras.push(para);
        } else {
            self.doc.paragraphs.push(para);
        }
    }

    fn push_text(&mut self, text: &str) {
        self.ensure_para();
        let attrs = self.attrs.clone();
        let items = self.cur.as_mut().unwrap();
        if let Some(prefix) = self.pending_prefix.take() {
            items.extend(prefix.chars().map(Item::Char));
        }
        for a in &attrs {
            items.push(Item::Code(Code::On(a.clone())));
        }
        for c in text.chars() {
            items.push(match c {
                '\t' => Item::Code(Code::Tab),
                '\n' => Item::Char(' '),
                c => Item::Char(c),
            });
        }
        for a in attrs.iter().rev() {
            items.push(Item::Code(Code::Off(a.kind())));
        }
    }

    fn push_code(&mut self, code: Code) {
        self.ensure_para();
        self.cur.as_mut().unwrap().push(Item::Code(code));
    }

    fn end_code_block(&mut self) {
        let text = std::mem::take(&mut self.code_buf);
        let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        for line in lines {
            let mut props = self.block_props();
            props.space_after = Some(0);
            props.line_spacing = Some(LineSpacing::Auto(240));
            props.shading = Some(Rgb(0xF2, 0xF2, 0xF2));
            self.open_para(props);
            self.attrs.push(Attr::Font(MONO.into()));
            self.attrs.push(Attr::Size(20));
            self.push_text(line);
            self.attrs.truncate(self.attrs.len() - 2);
            self.flush_para();
        }
        self.in_code_block = false;
    }

    fn start_list(&mut self, start: Option<u64>) {
        self.flush_para();
        let level = self.lists.len().min(8) as u8;
        let num_id = match start {
            Some(s) => {
                let id = self.doc.numbering.add_list(ListKind::Decimal);
                if s != 1 {
                    if let Some(a) = self.doc.numbering.abstract_nums.last_mut() {
                        if let Some(l0) = a.levels.get_mut(0) {
                            l0.start = s as i32;
                        }
                    }
                }
                id
            }
            None => match self.bullet_num {
                Some(id) => id,
                None => {
                    let id = self.doc.numbering.add_list(ListKind::Bullet);
                    self.bullet_num = Some(id);
                    id
                }
            },
        };
        self.lists.push(ListCtx { num_id, level, numbered: false });
    }

    fn finish_table(&mut self) {
        let Some((rows, _)) = self.table.take() else { return };
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
        let id = self.doc.next_table_id();
        let mut table = Table::new(rows.len().max(1), ncols, self.doc.section.text_width());
        table.rows[0].header = true;
        for (ri, row) in rows.iter().enumerate() {
            for ci in 0..ncols {
                let mut paras: Vec<Paragraph> = row.get(ci).cloned().unwrap_or_default();
                if paras.is_empty() {
                    paras.push(Paragraph::new());
                }
                for mut p in paras {
                    p.props.space_after = Some(0);
                    p.props.line_spacing = Some(LineSpacing::Auto(240));
                    p.props.cell = Some(CellRef::new(id, ri as u32, ci as u32));
                    if ri == 0 {
                        p.props.mark.bold = Some(true);
                        let mut items = vec![Item::Code(Code::On(Attr::Bold(true)))];
                        items.append(&mut p.items);
                        items.push(Item::Code(Code::Off(AttrKind::Bold)));
                        p.items = items;
                    }
                    self.doc.paragraphs.push(p);
                }
            }
        }
        self.doc.tables.insert(id, table);
    }

    fn open_link(&mut self, url: &str) {
        let id = self.next_wrapper;
        self.next_wrapper += 1;
        let (xml, label) = if let Some(anchor) = url.strip_prefix('#') {
            (format!("<w:hyperlink w:anchor=\"{}\" w:history=\"1\">", escape_attr(anchor)), "Hyperlink")
        } else {
            let rid = format!("rIdwp{}", self.next_rel);
            self.next_rel += 1;
            self.doc.extra_rels.push(ExtraRel { id: rid.clone(), kind: "hyperlink".into(), target: url.to_string(), external: true });
            (format!("<w:hyperlink r:id=\"{}\" w:history=\"1\">", rid), "Hyperlink")
        };
        self.push_code(Code::Opaque(OpaqueXml { xml, label: label.into(), kind: OpaqueKind::Open(id), protected: false, deleted: false, hint: false, level: OpaqueLevel::Para }));
        self.attrs.push(Attr::CharStyle("Hyperlink".into()));
    }

    fn close_link(&mut self) {
        self.attrs.retain(|a| !matches!(a, Attr::CharStyle(_)));
        let id = self.next_wrapper - 1;
        self.push_code(Code::Opaque(OpaqueXml { xml: "</w:hyperlink>".into(), label: "hyperlink".into(), kind: OpaqueKind::Close(id), protected: false, deleted: false, hint: false, level: OpaqueLevel::Para }));
    }
}

/// Build a document from Markdown text.
pub fn from_markdown(text: &str) -> Document {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    let mut b = Builder::new();
    let mut link_stack: Vec<bool> = Vec::new();
    for ev in Parser::new_ext(text, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    let props = b.block_props();
                    b.open_para(props);
                }
                Tag::Heading { level, .. } => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    let id = format!("Heading{}", n);
                    if b.doc.styles.get(&id).is_none() {
                        let mut st = Style::para(&id, &format!("heading {}", n));
                        st.based_on = Some("Normal".into());
                        st.next = Some("Normal".into());
                        st.para.keep_next = Some(true);
                        st.para.space_before = Some(40);
                        st.para.outline_level = Some(n - 1);
                        st.run.bold = Some(true);
                        b.doc.styles.upsert(st);
                    }
                    let mut props = b.block_props();
                    props.style = Some(id);
                    props.list = None;
                    b.open_para(props);
                }
                Tag::BlockQuote(_) => {
                    b.flush_para();
                    b.quote_depth += 1;
                }
                Tag::CodeBlock(kind) => {
                    b.flush_para();
                    b.in_code_block = true;
                    if let CodeBlockKind::Fenced(_) = kind {}
                }
                Tag::HtmlBlock => {
                    let props = b.block_props();
                    b.open_para(props);
                }
                Tag::List(start) => b.start_list(start),
                Tag::Item => {
                    b.flush_para();
                    if let Some(l) = b.lists.last_mut() {
                        l.numbered = false;
                    }
                }
                Tag::FootnoteDefinition(label) => {
                    b.flush_para();
                    b.footnote = Some((label.to_string(), Vec::new()));
                }
                Tag::Table(_) => {
                    b.flush_para();
                    b.table = Some((Vec::new(), Vec::new()));
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some((rows, _)) = b.table.as_mut() {
                        rows.push(Vec::new());
                    }
                }
                Tag::TableCell => {
                    b.in_table_cell = true;
                    b.open_para(ParaProps::default());
                }
                Tag::Emphasis => b.attrs.push(Attr::Italic(true)),
                Tag::Strong => b.attrs.push(Attr::Bold(true)),
                Tag::Strikethrough => b.attrs.push(Attr::Strike(true)),
                Tag::Superscript => b.attrs.push(Attr::VertAlign(VertAlign::Superscript)),
                Tag::Subscript => b.attrs.push(Attr::VertAlign(VertAlign::Subscript)),
                Tag::Link { dest_url, .. } => {
                    b.ensure_para();
                    b.open_link(&dest_url);
                    link_stack.push(true);
                }
                Tag::Image { dest_url, title, .. } => {
                    b.ensure_para();
                    b.open_link(&dest_url);
                    let t = if title.is_empty() { "image: ".to_string() } else { format!("image: {} ", title) };
                    b.push_text(&t);
                    link_stack.push(true);
                }
                Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition | Tag::MetadataBlock(_) => {
                    let props = b.block_props();
                    b.open_para(props);
                }
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::HtmlBlock => b.flush_para(),
                TagEnd::BlockQuote(_) => {
                    b.flush_para();
                    b.quote_depth = b.quote_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => b.end_code_block(),
                TagEnd::List(_) => {
                    b.flush_para();
                    b.lists.pop();
                }
                TagEnd::Item => b.flush_para(),
                TagEnd::FootnoteDefinition => {
                    b.flush_para();
                    if let Some((label, paras)) = b.footnote.take() {
                        let id = b.footnote_id(&label);
                        b.doc.footnotes.push(Footnote { id, paragraphs: paras });
                    }
                }
                TagEnd::Table => b.finish_table(),
                TagEnd::TableHead | TagEnd::TableRow => {}

                TagEnd::TableCell => {
                    b.flush_para();
                    b.in_table_cell = false;
                    if let Some((rows, cell)) = b.table.as_mut() {
                        let paras = std::mem::take(cell);
                        if rows.is_empty() {
                            rows.push(Vec::new());
                        }
                        rows.last_mut().unwrap().push(paras);
                    }
                }
                TagEnd::Emphasis => pop_attr(&mut b.attrs, AttrKind::Italic),
                TagEnd::Strong => pop_attr(&mut b.attrs, AttrKind::Bold),
                TagEnd::Strikethrough => pop_attr(&mut b.attrs, AttrKind::Strike),
                TagEnd::Superscript | TagEnd::Subscript => pop_attr(&mut b.attrs, AttrKind::VertAlign),
                TagEnd::Link | TagEnd::Image => {
                    if link_stack.pop().is_some() {
                        b.close_link();
                    }
                }
                TagEnd::DefinitionList | TagEnd::DefinitionListTitle | TagEnd::DefinitionListDefinition | TagEnd::MetadataBlock(_) => b.flush_para(),
            },
            Event::Text(t) => {
                if b.in_code_block {
                    b.code_buf.push_str(&t);
                } else {
                    b.push_text(&t);
                }
            }
            Event::Code(t) => {
                b.attrs.push(Attr::Font(MONO.into()));
                b.push_text(&t);
                pop_attr(&mut b.attrs, AttrKind::Font);
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => b.push_text(&t),
            Event::Html(t) | Event::InlineHtml(t) => {
                let s = t.trim();
                if s.eq_ignore_ascii_case("<!-- page break -->") || s.eq_ignore_ascii_case("<!-- pagebreak -->") {
                    b.push_code(Code::PageBreak);
                } else if s.eq_ignore_ascii_case("<br>") || s.eq_ignore_ascii_case("<br/>") || s.eq_ignore_ascii_case("<br />") {
                    b.push_code(Code::LineBreak);
                } else if !s.starts_with("<!--") {
                    b.push_text(&t);
                }
            }
            Event::FootnoteReference(label) => {
                let id = b.footnote_id(&label);
                b.ensure_para();
                let items = b.cur.as_mut().unwrap();
                items.push(Item::Code(Code::On(Attr::VertAlign(VertAlign::Superscript))));
                items.push(Item::Code(Code::Opaque(OpaqueXml::element(format!("<w:footnoteReference w:id=\"{}\"/>", id), "Footnote"))));
                items.push(Item::Code(Code::Off(AttrKind::VertAlign)));
            }
            Event::SoftBreak => b.push_text(" "),
            Event::HardBreak => b.push_code(Code::LineBreak),
            Event::Rule => {
                b.flush_para();
                let mut props = b.block_props();
                props.borders = Some(ParaBorders { bottom: Some(Border { style: BorderStyle::Single, size: 6, color: None, space: 1 }), ..Default::default() });
                b.open_para(props);
                b.flush_para();
            }
            Event::TaskListMarker(done) => {
                b.pending_prefix = Some(if done { "☑ ".into() } else { "☐ ".into() });
            }
        }
    }
    b.flush_para();
    b.finish_table();
    // Word wants a paragraph after a table that ends the body.
    if b.doc.paragraphs.last().map_or(false, |p| p.props.cell.is_some()) {
        b.doc.paragraphs.push(Paragraph::new());
    }
    if b.doc.paragraphs.is_empty() {
        b.doc.paragraphs.push(Paragraph::new());
    }
    // Footnote bodies are referenced by id; keep them in id order.
    b.doc.footnotes.sort_by_key(|f| f.id);
    b.doc
}

fn pop_attr(attrs: &mut Vec<Attr>, kind: AttrKind) {
    if let Some(i) = attrs.iter().rposition(|a| a.kind() == kind) {
        attrs.remove(i);
    }
}
