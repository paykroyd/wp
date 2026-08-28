use wp_core::model::*;
use wp_core::numbering::NumFmt;
use wp_core::Document;

/// The Markdown text plus what could not be expressed in it.
pub struct Export {
    pub text: String,
    /// Human-readable losses, e.g. "page setup", "3 comments".
    pub losses: Vec<String>,
}

impl Export {
    /// The one-line warning shown once when saving (SPEC §7.1 P0-4).
    pub fn warning(&self) -> Option<String> {
        if self.losses.is_empty() {
            return None;
        }
        Some(format!("Saved as Markdown — not carried over: {}.", self.losses.join(", ")))
    }
}

fn is_mono(font: Option<&str>) -> bool {
    matches!(font, Some(f) if f.contains("Courier") || f.contains("Mono") || f.contains("Consolas") || f.contains("Menlo"))
}

fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '*' | '_' | '`' | '\\' | '[' | ']' | '<' | '>' | '~' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn escape_line_start(s: &str) -> String {
    let t = s.trim_start();
    let needs = t.starts_with('#')
        || t.starts_with('>')
        || t.starts_with("- ")
        || t.starts_with("+ ")
        || t.starts_with("* ")
        || t == "-"
        || t.starts_with("---")
        || t.chars().take_while(|c| c.is_ascii_digit()).count() > 0 && t.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with(". ");
    if needs {
        format!("\\{}", s)
    } else {
        s.to_string()
    }
}

/// Inline Markdown for one paragraph's items.
fn inline(doc: &Document, para: usize, rels: &dyn Fn(&str) -> Option<String>, losses: &mut Losses) -> String {
    let runs = doc.runs(para);
    let p = &doc.paragraphs[para];
    let base = doc.base_run_props(para);
    let mut out = String::new();
    let mut open: Vec<&str> = Vec::new(); // markers currently open, in order
    let mut ri = 0;
    let mut link: Option<String> = None;
    let mut link_text_start: Option<usize> = None;
    let mut pending_text = String::new();
    let flush_text = |out: &mut String, t: &mut String| {
        if !t.is_empty() {
            out.push_str(&escape_inline(t));
            t.clear();
        }
    };
    for (i, it) in p.items.iter().enumerate() {
        while ri + 1 < runs.len() && runs[ri].end <= i {
            ri += 1;
        }
        let props = &runs[ri].props;
        // Desired markers for this run.
        let mut want: Vec<&str> = Vec::new();
        if props.is_bold() && !base.is_bold() {
            want.push("**");
        }
        if props.is_italic() && !base.is_italic() {
            want.push("*");
        }
        if props.is_strike() {
            want.push("~~");
        }
        if is_mono(props.font.as_deref()) && !is_mono(base.font.as_deref()) {
            want.push("`");
        }
        let is_content = matches!(it, Item::Char(_) | Item::Code(Code::Tab) | Item::Code(Code::LineBreak));
        if is_content && want != open {
            flush_text(&mut out, &mut pending_text);
            // Close everything that changed (innermost first), reopen in order.
            let common = open.iter().zip(want.iter()).take_while(|(a, b)| a == b).count();
            for m in open[common..].iter().rev() {
                out.push_str(m);
            }
            for m in &want[common..] {
                out.push_str(m);
            }
            open = want.clone();
        }
        match it {
            Item::Char(c) => pending_text.push(*c),
            Item::Code(Code::Tab) => pending_text.push('\t'),
            Item::Code(Code::LineBreak) => {
                flush_text(&mut out, &mut pending_text);
                out.push_str("  \n");
            }
            Item::Code(Code::PageBreak) => {
                flush_text(&mut out, &mut pending_text);
                for m in open.iter().rev() {
                    out.push_str(m);
                }
                open.clear();
                out.push_str("\n\n<!-- page break -->\n\n");
            }
            Item::Code(Code::Opaque(o)) => match o.kind {
                OpaqueKind::Open(_) if o.xml.starts_with("<w:hyperlink") => {
                    flush_text(&mut out, &mut pending_text);
                    let target = attr_of(&o.xml, "r:id").and_then(|id| rels(&id)).or_else(|| attr_of(&o.xml, "w:anchor").map(|a| format!("#{}", a)));
                    link = target;
                    link_text_start = Some(out.len());
                    out.push('[');
                }
                OpaqueKind::Close(_) if o.xml == "</w:hyperlink>" => {
                    flush_text(&mut out, &mut pending_text);
                    for m in open.iter().rev() {
                        out.push_str(m);
                    }
                    open.clear();
                    match (link.take(), link_text_start.take()) {
                        (Some(url), Some(_)) => out.push_str(&format!("]({})", url)),
                        (None, Some(start)) => {
                            out.remove(start);
                        }
                        _ => {}
                    }
                }
                OpaqueKind::Element if o.label == "Footnote" => {
                    flush_text(&mut out, &mut pending_text);
                    if let Some(id) = attr_of(&o.xml, "w:id") {
                        out.push_str(&format!("[^{}]", id));
                    }
                }
                OpaqueKind::Element if o.hint => {}
                OpaqueKind::Element => losses.opaque(&o.label),
                _ => losses.opaque(&o.label),
            },
            _ => {}
        }
        // Colours, sizes, highlights, underline are not Markdown.
        if is_content {
            if props.color.is_some() && base.color != props.color {
                losses.flag("text colours");
            }
            if props.highlight().is_some() {
                losses.flag("highlighting");
            }
            if props.underline().is_some() && !props.underline().is_some_and(|_| link.is_some()) {
                losses.flag("underlining");
            }
            if props.size.is_some() && props.size != base.size {
                losses.flag("font sizes");
            }
            if props.font.is_some() && props.font != base.font && !is_mono(props.font.as_deref()) {
                losses.flag("fonts");
            }
        }
    }
    flush_text(&mut out, &mut pending_text);
    for m in open.iter().rev() {
        out.push_str(m);
    }
    if link.is_some() {
        out.push(']');
    }
    out
}

fn attr_of(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("{}=\"", name))? + name.len() + 2;
    let end = xml[start..].find('"')? + start;
    Some(xml[start..end].replace("&amp;", "&").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">"))
}

#[derive(Default)]
struct Losses {
    flags: Vec<String>,
    counts: std::collections::BTreeMap<String, usize>,
}

impl Losses {
    fn flag(&mut self, what: &str) {
        if !self.flags.iter().any(|f| f == what) {
            self.flags.push(what.to_string());
        }
    }
    fn opaque(&mut self, label: &str) {
        let key = match label {
            "Comment" => "comments",
            "Inserted Text" | "Deleted Text" | "Moved Text (from)" | "Moved Text (to)" => "tracked changes",
            "Drawing" | "Picture" | "Object" => "images",
            "Field" | "Field Code" => "fields",
            "Content Control" => "content controls",
            "Endnote" => "endnotes",
            "Equation" => "equations",
            l if l.starts_with("Bookmark") || l.eq_ignore_ascii_case("bookmark end") => return,
            l if l.to_lowercase().contains("hyperlink") || l == "Proof Mark" || l == "Rendered Pg Brk" || l == "Empty Run" || l == "Note Ref" => return,
            _ => "other preserved content",
        };
        *self.counts.entry(key.to_string()).or_insert(0) += 1;
    }
    fn into_list(self) -> Vec<String> {
        let mut v = self.flags;
        for (k, n) in self.counts {
            let label = if n == 1 { k.trim_end_matches('s').to_string() } else { k.clone() };
            v.push(format!("{} {}", n, label));
        }
        v
    }
}

/// Render a document as Markdown. `rels` resolves hyperlink relationship
/// ids to targets (from the package the document was read from).
pub fn to_markdown(doc: &Document, rels: &dyn Fn(&str) -> Option<String>) -> Export {
    let mut out = String::new();
    let mut losses = Losses::default();
    let labels = doc.list_labels();
    if doc.section != SectionProps::default() {
        losses.flag("page setup");
    }
    let mut prev_code_block = false;
    let mut i = 0;
    let n = doc.paragraphs.len();
    while i < n {
        let p = &doc.paragraphs[i];
        let pp = doc.para_props(i);
        // Tables become GFM tables: one row per row, cell paragraphs joined
        // by spaces. Merged cells and nested tables cannot be expressed.
        if let Some(c) = p.props.cell {
            let (_, end) = doc.table_bounds(i).unwrap();
            let rows = doc.table_paras(i).unwrap();
            let table = doc.tables.get(&c.table);
            let mut lossy = false;
            if prev_code_block {
                out.push_str("```\n\n");
                prev_code_block = false;
            }
            let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
            for (ri, row) in rows.iter().enumerate() {
                out.push('|');
                for ci in 0..ncols {
                    let mut cell = String::new();
                    if let Some(paras) = row.get(ci) {
                        for &pi in paras {
                            if doc.paragraphs[pi].props.raw_block {
                                lossy = true;
                                continue;
                            }
                            let text = inline(doc, pi, rels, &mut losses);
                            let mut text = text.trim();
                            // GFM's header row is bold by convention; an
                            // explicit bold around the whole cell is redundant.
                            if ri == 0 && text.len() > 4 && text.starts_with("**") && text.ends_with("**") && !text[2..text.len() - 2].contains("**") {
                                text = &text[2..text.len() - 2];
                            }
                            if !text.is_empty() {
                                if !cell.is_empty() {
                                    cell.push(' ');
                                }
                                cell.push_str(text);
                            }
                        }
                    }
                    let cell = cell.replace('|', "\\|").replace('\n', " ");
                    out.push_str(&format!(" {} |", cell));
                }
                if table.and_then(|t| t.rows.get(ri)).map_or(false, |r| r.cells.iter().any(|x| x.span() > 1 || x.vmerge.is_some())) {
                    lossy = true;
                }
                out.push('\n');
                if ri == 0 {
                    out.push('|');
                    for _ in 0..ncols {
                        out.push_str(" --- |");
                    }
                    out.push('\n');
                }
            }
            if lossy {
                losses.flag("merged or nested table cells");
            }
            out.push('\n');
            i = end;
            continue;
        }
        // Preserved blocks: tables become GFM tables, the rest is dropped.
        if p.props.raw_block {
            if let Some(Item::Code(Code::Opaque(o))) = p.items.first() {
                if o.xml.starts_with("<w:tbl") {
                    let (rows, lossy) = wp_docx::table_cells(&o.xml);
                    if lossy {
                        losses.flag("merged or nested table cells");
                    }
                    if prev_code_block {
                        out.push_str("```\n\n");
                        prev_code_block = false;
                    }
                    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                    for (ri, row) in rows.iter().enumerate() {
                        out.push('|');
                        for ci in 0..ncols {
                            let cell = row.get(ci).map(|s| s.replace('|', "\\|").replace('\n', " ")).unwrap_or_default();
                            out.push_str(&format!(" {} |", cell));
                        }
                        out.push('\n');
                        if ri == 0 {
                            out.push('|');
                            for _ in 0..ncols {
                                out.push_str(" --- |");
                            }
                            out.push('\n');
                        }
                    }
                    out.push('\n');
                } else {
                    losses.opaque(&o.label);
                }
            }
            i += 1;
            continue;
        }
        // Code block: monospace paragraphs, merged while they continue.
        let content_runs = doc.runs(i).into_iter().filter(|r| p.items[r.start..r.end].iter().any(|it| !it.is_code())).collect::<Vec<_>>();
        let mono = !content_runs.is_empty() && content_runs.iter().all(|r| is_mono(r.props.font.as_deref())) && labels[i].is_none() && pp.outline_level.is_none();

        if mono {
            if !prev_code_block {
                out.push_str("```\n");
                prev_code_block = true;
            }
            out.push_str(&p.text());
            out.push('\n');
            i += 1;
            continue;
        } else if prev_code_block {
            out.push_str("```\n\n");
            prev_code_block = false;
        }
        let body = inline(doc, i, rels, &mut losses).trim_end_matches('\n').to_string();
        let mut prefix = String::new();

        if let Some(l) = pp.outline_level.filter(|l| *l < 6 && labels[i].is_none()) {
            prefix = format!("{} ", "#".repeat(l as usize + 1));
        } else if let Some(label) = &labels[i] {
            let indent = "   ".repeat(label.level as usize);
            let marker = if label.fmt == NumFmt::Bullet || label.fmt == NumFmt::None { "-".to_string() } else { format!("{}.", label.number) };
            if !matches!(label.fmt, NumFmt::Bullet | NumFmt::None | NumFmt::Decimal) {
                losses.flag("list numbering styles");
            }
            prefix = format!("{}{} ", indent, marker);
        } else if pp.indent_left() > 0 && pp.indent_right() > 0 && pp.borders.as_ref().map_or(false, |b| b.left.is_some()) {
            prefix = "> ".into();
        } else if pp.borders.as_ref().map_or(false, |b| b.bottom.is_some() && b.left.is_none()) && p.items.is_empty() {
            out.push_str("---\n\n");
            i += 1;
            continue;
        } else {
            if pp.align != None && pp.align() != Align::Left {
                losses.flag("alignment");
            }
            if p.props.indent_left.is_some() || p.props.first_line.is_some() || p.props.hanging.is_some() {
                losses.flag("indents");
            }
            if p.props.space_before.is_some() || p.props.space_after.is_some() || p.props.line_spacing.is_some() {
                losses.flag("paragraph spacing");
            }
        }
        if p.props.sect_break.is_some() {
            losses.flag("section breaks");
        }
        let text = if prefix.is_empty() { escape_line_start(&body) } else { body };
        if prefix.starts_with("> ") {
            out.push_str(&format!("> {}\n\n", text.replace("  \n", "  \n> ")));
        } else if prefix.ends_with(". ") || prefix.ends_with("- ") {
            let cont = format!("\n{}", " ".repeat(prefix.len()));
            out.push_str(&format!("{}{}\n", prefix, text.replace("  \n", &format!("  {}", cont))));
            // Blank line after the last item of a list: the next paragraph
            // is not an item of this list (or a nested one).
            let this = doc.list_ref(i);
            let next_is_item = i + 1 < n
                && labels[i + 1].as_ref().map_or(false, |l| l.level > 0 || doc.list_ref(i + 1).map(|r| r.num_id) == this.map(|r| r.num_id));

            if !next_is_item {
                out.push('\n');
            }
        } else if text.is_empty() && prefix.is_empty() {
            // Empty paragraph: keep the vertical gap.
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        } else {
            out.push_str(&format!("{}{}\n\n", prefix, text));
        }
        i += 1;
    }
    if prev_code_block {
        out.push_str("```\n\n");
    }
    if !doc.footnotes.is_empty() {
        for f in &doc.footnotes {
            let mut tmp = Document::new();
            tmp.styles = doc.styles.clone();
            tmp.numbering = doc.numbering.clone();
            tmp.paragraphs = f.paragraphs.clone();
            let mut body = Vec::new();
            for j in 0..tmp.paragraphs.len() {
                body.push(inline(&tmp, j, rels, &mut losses).trim().to_string());
            }
            out.push_str(&format!("[^{}]: {}\n\n", f.id, body.join("\n    ")));
        }
    }
    let text = out.trim_end().to_string() + "\n";
    Export { text, losses: losses.into_list() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown;

    const SAMPLE: &str = "# Title\n\nSome *emphasis* and **strong** and `code` and ~~gone~~.\n\n- one\n- two\n   - nested\n\n1. first\n2. second\n\n> quoted\n\n```\nlet x = 1;\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nA link to [example](https://example.com/) and a note.[^1]\n\n[^1]: The note.\n";

    #[test]
    fn round_trip_keeps_structure() {
        let doc = from_markdown(SAMPLE);
        assert_eq!(doc.paragraphs[0].props.style.as_deref(), Some("Heading1"));
        assert!(doc.runs(1).iter().any(|r| r.props.is_italic()));
        assert!(doc.runs(1).iter().any(|r| r.props.is_bold()));
        assert!(doc.runs(1).iter().any(|r| r.props.is_strike()));
        let labels: Vec<String> = doc.list_labels().into_iter().map(|l| l.map(|l| l.text).unwrap_or_default()).collect();
        assert_eq!(&labels[2..7], &["•", "•", "◦", "1.", "2."]);
        assert_eq!(doc.tables.len(), 1);
        assert!(doc.paragraphs.iter().any(|p| p.props.cell.is_some()));
        assert_eq!(doc.footnotes.len(), 1);
        assert_eq!(doc.extra_rels.len(), 1);
        let rels = |id: &str| doc.extra_rels.iter().find(|r| r.id == id).map(|r| r.target.clone());
        let md = to_markdown(&doc, &rels);
        assert!(md.text.contains("# Title"), "{}", md.text);
        assert!(md.text.contains("*emphasis*") && md.text.contains("**strong**") && md.text.contains("`code`") && md.text.contains("~~gone~~"), "{}", md.text);
        assert!(md.text.contains("- one\n- two\n   - nested\n"), "{}", md.text);
        assert!(md.text.contains("1. first\n2. second\n"), "{}", md.text);
        assert!(md.text.contains("> quoted"), "{}", md.text);
        assert!(md.text.contains("```\nlet x = 1;\n```"), "{}", md.text);
        assert!(md.text.contains("| a | b |\n| --- | --- |\n| 1 | 2 |"), "{}", md.text);
        assert!(md.text.contains("[example](https://example.com/)"), "{}", md.text);
        assert!(md.text.contains("note.[^1]") && md.text.contains("[^1]: The note."), "{}", md.text);
        // A second pass is stable.
        let again = to_markdown(&from_markdown(&md.text), &rels);
        assert_eq!(again.text, md.text);
    }

    #[test]
    fn losses_are_reported_once() {
        let mut doc = from_markdown("plain\n");
        doc.section.margin_left = 720;
        doc.paragraphs[0].items.insert(0, Item::Code(Code::On(Attr::Color(Rgb(255, 0, 0)))));
        doc.paragraphs[0].items.push(Item::Code(Code::Off(AttrKind::Color)));
        let e = to_markdown(&doc, &|_| None);
        let w = e.warning().unwrap();
        assert!(w.contains("page setup") && w.contains("text colours"), "{}", w);
    }
}
