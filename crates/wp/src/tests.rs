//! Headless UI tests: drive the app with key events and inspect the rendered
//! buffer.

use crate::app::{App, Overlay};
use crate::config::{Config, KeymapChoice};
use crate::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

struct Harness {
    app: App,
    term: Terminal<TestBackend>,
}

impl Harness {
    fn new(keymap: KeymapChoice) -> Harness {
        let mut cfg = Config::default();
        cfg.keymap = keymap;
        cfg.show_hint = false;
        let mut app = App::new(cfg);
        app.resize(80, 24);
        let term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        Harness { app, term }
    }

    fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.app.handle_key(KeyEvent::new(code, mods));
    }

    fn type_str(&mut self, s: &str) {
        for c in s.chars() {
            self.key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn screen(&mut self) -> String {
        let caps = ui::Caps { ascii: false, colors: true };
        self.term.draw(|f| ui::draw(f, &mut self.app, caps)).unwrap();
        let buf = self.term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const ALT: KeyModifiers = KeyModifiers::ALT;
const NONE: KeyModifiers = KeyModifiers::NONE;

fn corpus(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus").join(name)
}

#[test]
fn typing_and_status() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("Hello, world");
    let s = h.screen();
    assert!(s.contains("Hello, world"), "{}", s);
    assert!(s.contains("Untitled *"), "{}", s);
    assert!(s.contains("Pg 1/1"), "{}", s);
    assert!(s.contains("Ln 1.00\""), "{}", s);
}

#[test]
fn bold_via_key_and_reveal_codes() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("plain ");
    h.key(KeyCode::Char('b'), CTRL | KeyModifiers::SHIFT);
    h.type_str("bold");
    h.key(KeyCode::Char('B'), CTRL); // uppercase form some terminals send
    h.type_str(" plain");
    assert_eq!(h.app.ed.doc.text(), "plain bold plain");
    assert_eq!(h.app.ed.doc.runs(0).len(), 3);
    h.key(KeyCode::F(3), ALT);
    let s = h.screen();
    assert!(s.contains("Reveal Codes"), "{}", s);
    assert!(s.contains("[BOLD]bold[bold]"), "{}", s);
    assert!(s.contains("[HRt]"), "{}", s);
    // Classic F6 also works under the modern map.
    h.key(KeyCode::End, NONE);
    h.key(KeyCode::F(6), NONE);
    h.type_str("X");
    assert!(h.app.ed.doc.run_props_at(h.app.ed.cursor).is_bold());
}

#[test]
fn delete_code_in_reveal_codes_removes_pair() {
    let mut h = Harness::new(KeymapChoice::Classic);
    h.type_str("ab");
    h.key(KeyCode::Home, NONE);
    h.key(KeyCode::Right, NONE);
    h.key(KeyCode::F(6), NONE); // [BOLD][bold] at cursor
    h.type_str("Q");
    h.key(KeyCode::F(3), ALT); // reveal codes
    // Cursor is between Q and [bold]; step left twice: over Q, onto [BOLD].
    h.key(KeyCode::Left, NONE);
    h.key(KeyCode::Left, NONE);
    let items_before = h.app.ed.doc.paragraphs[0].items.len();
    h.key(KeyCode::Delete, NONE);
    assert_eq!(h.app.ed.doc.paragraphs[0].items.len(), items_before - 2);
    assert_eq!(h.app.ed.doc.text(), "aQb");
    assert!(h.app.ed.doc.paragraphs[0].items.iter().all(|i| !i.is_code()));
}

#[test]
fn palette_finds_and_runs_commands() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("centre me");
    h.key(KeyCode::Char('p'), CTRL | KeyModifiers::SHIFT);
    h.type_str("cent");
    let s = h.screen();
    assert!(s.contains("Center"), "{}", s);
    assert!(s.contains("Ctrl+Shift+E"), "palette must show the key: {}", s);
    h.key(KeyCode::Enter, NONE);
    assert_eq!(h.app.ed.doc.paragraphs[0].props.align, Some(wp_core::Align::Center));
    // Every listed command must resolve and be findable by its own title.
    for c in crate::commands::COMMANDS.iter().filter(|c| c.listed) {
        let rows = h.app.palette_rows(c.title);
        assert!(rows.iter().any(|r| r.label == c.title), "palette can't find {}", c.title);
    }
}

#[test]
fn open_docx_render_and_page_rules() {
    let p = corpus("gen-report.docx");
    if !p.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_path(&p).unwrap();
    let s = h.screen();
    assert!(s.contains("Quarterly Report"), "{}", s);
    assert!(s.contains("table") || s.contains("Table"), "table placeholder: {}", s);
    let pages = h.app.ed.page_count();
    assert!(pages >= 2, "pages = {}", pages);
    h.key(KeyCode::End, CTRL);
    let s = h.screen();
    assert!(s.contains(&format!("Pg {}/{}", pages, pages)), "{}", s);
    // A page rule is drawn somewhere while scrolling through the document.
    h.key(KeyCode::Home, CTRL);
    let mut seen_rule = false;
    for _ in 0..80 {
        h.key(KeyCode::PageDown, NONE);
        if h.screen().contains("─ Page ") {
            seen_rule = true;
            break;
        }
    }
    assert!(seen_rule);
    // The one-line warning summary mentions the table.
    assert!(h.app.warnings.iter().any(|w| w.label == "table"));
}

#[test]
fn protected_content_refuses_edits() {
    let p = corpus("path-mixed.docx");
    if !p.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_path(&p).unwrap();
    // Paragraph 2 contains "inserted" inside a tracked change.
    let para = &h.app.ed.doc.paragraphs[1];
    let idx = para.items.iter().position(|i| matches!(i, wp_core::Item::Code(wp_core::Code::Opaque(o)) if o.label == "Inserted Text")).unwrap();
    h.app.ed.cursor = wp_core::Pos::new(1, idx + 2);
    let before = h.app.ed.doc.text();
    h.type_str("x");
    assert_eq!(h.app.ed.doc.text(), before);
    assert!(h.app.status_text().unwrap().contains("can't edit"));
    // Table block: cursor there, typing refused.
    let tp = h.app.ed.doc.paragraphs.iter().position(|p| p.props.raw_block).unwrap();
    h.app.ed.cursor = wp_core::Pos::new(tp, 0);
    h.type_str("x");
    assert_eq!(h.app.ed.doc.text(), before);
}

#[test]
fn save_as_docx_roundtrip_from_ui() {
    let dir = std::env::temp_dir().join(format!("wp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.docx");
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("Title line");
    h.key(KeyCode::Char('1'), CTRL | ALT);
    h.key(KeyCode::Enter, NONE);
    h.type_str("Body text with ");
    h.key(KeyCode::Char('i'), CTRL);
    h.type_str("italics");
    h.key(KeyCode::Char('i'), CTRL);
    h.type_str(".");
    h.app.save_to(&path, crate::app::Format::Docx).unwrap();
    assert!(!h.app.ed.dirty);
    let l = wp_docx::read(&path).unwrap();
    assert_eq!(l.doc.text(), "Title line\nBody text with italics.");
    assert_eq!(l.doc.paragraphs[0].props.style.as_deref(), Some("Heading1"));
    assert!(l.doc.runs(1).iter().any(|r| r.props.is_italic()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_incremental_and_undo_groups() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("one two three two");
    h.key(KeyCode::Home, CTRL);
    h.key(KeyCode::Char('f'), CTRL | KeyModifiers::SHIFT);
    h.type_str("two");
    assert!(matches!(h.app.overlay, Overlay::Prompt { .. }));
    assert_eq!(h.app.ed.selection().map(|r| r.start.idx), Some(4));
    h.key(KeyCode::Enter, NONE);
    h.key(KeyCode::F(3), NONE); // find next
    assert_eq!(h.app.ed.selection().map(|r| r.start.idx), Some(14));
    h.key(KeyCode::Esc, NONE);
    h.key(KeyCode::End, CTRL);
    h.key(KeyCode::Char('z'), CTRL);
    assert_eq!(h.app.ed.doc.text(), "one two three");
}

#[test]
fn emacs_and_cmd_keys() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("alpha beta gamma");
    h.key(KeyCode::Char('a'), CTRL); // line start
    assert_eq!(h.app.ed.cursor.idx, 0);
    h.key(KeyCode::Char('f'), ALT); // word right
    assert_eq!(h.app.ed.cursor.idx, 6);
    h.key(KeyCode::Char('b'), CTRL); // char left
    h.key(KeyCode::Char('f'), CTRL); // char right
    assert_eq!(h.app.ed.cursor.idx, 6);
    h.key(KeyCode::Char('d'), ALT); // delete word
    assert_eq!(h.app.ed.doc.text(), "alpha gamma");
    h.key(KeyCode::Char('k'), CTRL); // kill to end of line
    assert_eq!(h.app.ed.doc.text(), "alpha ");
    h.key(KeyCode::Char('u'), CTRL); // kill to start of line
    assert_eq!(h.app.ed.doc.text(), "");
    h.type_str("x");
    h.key(KeyCode::Char('b'), KeyModifiers::SUPER); // Cmd+B bold
    h.type_str("y");
    assert!(h.app.ed.doc.run_props_at(h.app.ed.cursor).is_bold());
    h.key(KeyCode::Backspace, KeyModifiers::SUPER); // Cmd+Backspace
    assert_eq!(h.app.ed.doc.text(), "");
    assert_eq!(crate::keymap::Key::parse("cmd+shift+z").map(|k| k.label()), Some("Cmd+Ctrl+Shift+Z".replace("Ctrl+", "")));
    assert_eq!(crate::keymap::Key::parse("ctrl++").map(|k| k.label()), Some("Ctrl++".into()));
}

#[test]
fn exit_prompts_when_dirty() {
    let mut h = Harness::new(KeymapChoice::Classic);
    h.type_str("x");
    h.key(KeyCode::F(7), NONE);
    assert!(matches!(h.app.overlay, Overlay::Confirm { .. }));
    h.key(KeyCode::Char('n'), NONE);
    assert!(h.app.quit);
}

#[test]
fn fkey_legend_and_help_render() {
    let mut h = Harness::new(KeymapChoice::Classic);
    h.app.cfg.fkey_legend = true;
    let s = h.screen();
    assert!(s.contains("F6"), "{}", s);
    assert!(s.contains("Bold"), "{}", s);
    h.key(KeyCode::F(3), NONE);
    let s = h.screen();
    assert!(s.contains("Help"), "{}", s);
}


#[test]
#[ignore]
fn perf_typing_latency_big_doc() {
    let p = std::path::PathBuf::from("/tmp/big.docx");
    if !p.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    let t0 = std::time::Instant::now();
    h.app.open_path(&p).unwrap();
    let _ = h.screen();
    eprintln!("open+first render: {:?}", t0.elapsed());
    h.key(KeyCode::End, CTRL);
    let _ = h.screen();
    let t1 = std::time::Instant::now();
    for c in "the quick brown fox ".repeat(5).chars() {
        h.key(KeyCode::Char(c), NONE);
        let _ = h.screen();
    }
    eprintln!("100 keystrokes+renders at end: {:?} ({:?}/key)", t1.elapsed(), t1.elapsed() / 100);
    h.key(KeyCode::Home, CTRL);
    h.key(KeyCode::Down, NONE);
    let t2 = std::time::Instant::now();
    for c in "inserted near the top ".repeat(5).chars() {
        h.key(KeyCode::Char(c), NONE);
        let _ = h.screen();
    }
    eprintln!("110 keystrokes+renders near top: {:?} ({:?}/key)", t2.elapsed(), t2.elapsed() / 110);
}

#[test]
fn lists_toggle_continue_demote_and_save() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("first");
    h.key(KeyCode::Char('o'), CTRL | KeyModifiers::SHIFT); // numbered list
    h.key(KeyCode::Enter, NONE);
    h.type_str("second");
    h.key(KeyCode::Enter, NONE);
    h.key(KeyCode::Tab, NONE); // demote at start of item
    h.type_str("nested");
    h.key(KeyCode::Enter, NONE);
    h.key(KeyCode::Enter, NONE); // empty item: ends the list
    h.type_str("plain");
    let s = h.screen();
    assert!(s.contains("1. first"), "{}", s);
    assert!(s.contains("2. second"), "{}", s);
    assert!(s.contains("a. nested"), "{}", s);
    assert!(h.app.ed.doc.list_ref(3).is_none());
    assert!(h.app.ed.doc.list_ref(2).map(|r| r.level) == Some(1));
    // Bullets: toggling on a numbered item switches kind; toggling again removes.
    h.app.ed.cursor = wp_core::Pos::new(0, 0);
    h.key(KeyCode::Char('l'), CTRL | KeyModifiers::SHIFT);
    assert!(h.app.ed.doc.numbering.is_bullet(h.app.ed.doc.list_ref(0).unwrap().num_id, 0));
    assert!(h.screen().contains("• first"));
    h.key(KeyCode::Char('l'), CTRL | KeyModifiers::SHIFT);
    assert!(h.app.ed.doc.list_ref(0).is_none());
    // Reveal Codes shows the list code; the file round-trips with numbering.xml.
    h.app.ed.cursor = wp_core::Pos::new(1, 0);
    h.key(KeyCode::F(3), ALT);
    assert!(h.screen().contains("[List:"), "{}", h.screen());
    let dir = std::env::temp_dir().join(format!("wp-list-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("list.docx");
    h.app.save_to(&path, crate::app::Format::Docx).unwrap();
    let l = wp_docx::read(&path).unwrap();
    let labels: Vec<String> = l.doc.list_labels().into_iter().map(|l| l.map(|l| l.text).unwrap_or_default()).collect();
    assert_eq!(labels, ["", "1.", "a.", ""]);
    assert!(l.package.has("word/numbering.xml"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn word_lists_render_labels() {
    let p = corpus("word-lists.docx");
    if !p.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_path(&p).unwrap();
    let text = wp_core::text::to_text(&h.app.ed.doc, None);
    for expect in ["• First bullet", "  ◦ Nested bullet", "    ▪ Deeper bullet", "1. One", "  a) Two a", "    i. Two b i", "      2.b.i.1 Level four", "3. Three", "1. Restarted at one", "I. Roman I", "  A. Roman I.A", "    01 Zero-padded", "      5th Ordinal", "        One Cardinal", "7. Override start", "  (A) Override level"] {
        assert!(text.contains(expect), "missing {:?} in:\n{}", expect, text);
    }
    assert!(!text.contains("numId 0 removes") || !text.contains("• numId 0"));
    let s = h.screen();
    assert!(s.contains("• First bullet"), "{}", s);
}

#[test]
fn find_regex_format_code_and_replace_preview() {
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("Order 12 and order 345 then ");
    h.key(KeyCode::Char('b'), CTRL | KeyModifiers::SHIFT);
    h.type_str("bold");
    h.key(KeyCode::Char('b'), CTRL | KeyModifiers::SHIFT);
    h.type_str(" end");
    h.key(KeyCode::Enter, CTRL); // page break
    h.type_str("page two");
    // Regex with capture groups, previewed then replaced in full.
    h.key(KeyCode::Char('h'), CTRL | KeyModifiers::SHIFT); // replace
    h.type_str("re:order (\\d+)");
    h.key(KeyCode::Enter, NONE);
    h.type_str("#$1");
    h.key(KeyCode::Enter, NONE);
    assert!(matches!(h.app.overlay, Overlay::ReplacePreview { .. }));
    let s = h.screen();
    assert!(s.contains("2 matches"), "{}", s);
    assert!(s.contains("→ #12"), "{}", s);
    h.key(KeyCode::Enter, NONE);
    assert!(h.app.ed.doc.paragraphs[0].text().starts_with("#12 and #345 then bold end"));
    // Format search: only the bold run.
    h.app.exec(crate::commands::Cmd::FindBold);
    assert_eq!(h.app.ed.selection().map(|r| (r.start.idx, r.end.idx)), Some((19, 23)));
    // Code search jumps to the page break.
    h.app.exec(crate::commands::Cmd::FindPageBreak);
    let c = h.app.ed.selection().unwrap().start;
    assert!(matches!(h.app.ed.doc.paragraphs[c.para].items.get(c.idx), Some(wp_core::Item::Code(wp_core::Code::PageBreak))));

    // Whole-word, case-sensitive one-at-a-time replacement: y, then n.
    h.app.ed.cursor = wp_core::Pos::new(0, 0);
    h.app.ed.anchor = None;
    h.app.find.whole_word = true;
    h.app.find.case_sensitive = true;
    h.key(KeyCode::Char('h'), CTRL | KeyModifiers::SHIFT);
    h.key(KeyCode::Char('u'), CTRL);
    h.type_str("and");
    h.key(KeyCode::Enter, NONE);
    h.type_str("&");
    h.key(KeyCode::Enter, NONE);
    h.key(KeyCode::Char('o'), NONE); // one at a time
    assert!(matches!(h.app.overlay, Overlay::ReplaceStep { .. }));
    h.key(KeyCode::Char('y'), NONE);
    assert!(h.app.ed.doc.paragraphs[0].text().starts_with("#12 & #345 then bold end"));
    assert!(h.app.status_text().unwrap().contains("Replaced 1 of 1"));
    // Undo restores the replacement as one step.
    h.key(KeyCode::Char('z'), CTRL);
    assert!(h.app.ed.doc.paragraphs[0].text().starts_with("#12 and #345 then bold end"));
}

#[test]
fn markdown_open_save_docx_and_back() {
    let dir = std::env::temp_dir().join(format!("wp-md-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("notes.md");
    std::fs::write(&md, "# Notes\n\nA [link](https://example.org/) with a footnote.[^a]\n\n- item one\n- item two\n\n1. first\n2. second\n\n| h1 | h2 |\n| --- | --- |\n| x | y |\n\n[^a]: Footnote body.\n").unwrap();
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_path(&md).unwrap();
    assert_eq!(h.app.format, crate::app::Format::Markdown);
    let s = h.screen();
    assert!(s.contains("Notes") && s.contains("• item one") && s.contains("2. second"), "{}", s);
    assert!(s.contains("Table 2×2") || s.contains("Table"), "{}", s);
    // To .docx: numbering, footnotes and the hyperlink relationship are all created.
    let docx = dir.join("notes.docx");
    h.app.save_to(&docx, crate::app::Format::Docx).unwrap();
    let l = wp_docx::read(&docx).unwrap();
    assert!(l.package.has("word/numbering.xml") && l.package.has("word/footnotes.xml"));
    assert_eq!(l.package.rel_target("rIdwp1").as_deref(), Some("https://example.org/"));
    assert_eq!(l.doc.footnotes.len(), 1);
    assert_eq!(l.doc.paragraphs[0].props.style.as_deref(), Some("Heading1"));
    assert!(l.package.get_str("word/document.xml").unwrap().contains("<w:footnoteReference w:id=\"1\"/>"));
    // And back to Markdown from the .docx, links resolved through the package.
    let mut h2 = Harness::new(KeymapChoice::Modern);
    h2.app.open_path(&docx).unwrap();
    let e = h2.app.markdown_export();
    assert!(e.text.contains("# Notes"), "{}", e.text);
    assert!(e.text.contains("[link](https://example.org/)"), "{}", e.text);
    assert!(e.text.contains("footnote.[^1]") && e.text.contains("[^1]: Footnote body."), "{}", e.text);
    assert!(e.text.contains("| h1 | h2 |"), "{}", e.text);
    assert!(e.text.contains("- item one\n- item two\n\n1. first\n2. second"), "{}", e.text);
    // Saving a formatted .docx as .md warns once about what was dropped.
    let rep = corpus("gen-report.docx");
    if rep.exists() {
        let mut h3 = Harness::new(KeymapChoice::Modern);
        h3.app.open_path(&rep).unwrap();
        h3.app.save_to(&dir.join("report.md"), crate::app::Format::Markdown).unwrap();
        let msg = h3.app.status_text().unwrap();
        assert!(msg.starts_with("Saved as Markdown — not carried over:"), "{}", msg);
        assert!(msg.contains("colours") || msg.contains("font sizes"), "{}", msg);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mouse_click_drag_and_wheel() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("first line of text");
    h.key(KeyCode::Enter, NONE);
    h.type_str("second line");
    let _ = h.screen();
    let m = |kind, col, row| MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE };
    // Text starts at column 1; click on the 'r' of "first" (column 3, row 0).
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 3, 0));
    h.app.handle_mouse(m(MouseEventKind::Up(MouseButton::Left), 3, 0));
    assert_eq!(h.app.ed.cursor, wp_core::Pos::new(0, 2));
    // Drag to the second line selects across the paragraph break.
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 1, 0));
    h.app.handle_mouse(m(MouseEventKind::Drag(MouseButton::Left), 7, 2)); // row 1 is the spacing gap
    h.app.handle_mouse(m(MouseEventKind::Up(MouseButton::Left), 7, 2));
    let sel = h.app.ed.selection().unwrap();
    assert_eq!((sel.start, sel.end), (wp_core::Pos::new(0, 0), wp_core::Pos::new(1, 6)));
    assert_eq!(h.app.ed.fragment(sel).text(), "first line of text\nsecond");
    // Wheel moves the cursor by lines; clicking past the end lands at the end.
    h.app.handle_mouse(m(MouseEventKind::ScrollUp, 0, 0));
    assert_eq!(h.app.ed.cursor.para, 0);
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 60, 2));
    assert_eq!(h.app.ed.cursor, wp_core::Pos::new(1, 11));
    // Mouse off: events are ignored.
    h.app.cfg.mouse = false;
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 1, 0));
    assert_eq!(h.app.ed.cursor, wp_core::Pos::new(1, 11));
}

#[test]
fn open_dialog_browses_filters_and_opens() {
    let dir = corpus("");
    if !corpus("gen-report.docx").exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.browse(&dir, false);
    let s = h.screen();
    assert!(s.contains("Open —"), "{}", s);
    assert!(s.contains("parent directory"), "{}", s);
    // Sorted, one screenful at a time — the first rows are the earliest names.
    assert!(s.contains("gen-breaks.docx"), "{}", s);

    // Typing narrows the list; Tab completes to the shared prefix.
    h.type_str("gen-r");
    h.key(KeyCode::Tab, NONE);
    match &h.app.overlay {
        Overlay::Browse { filter, .. } => assert!(filter.starts_with("gen-report"), "filter = {}", filter),
        o => panic!("{:?}", o),
    }
    let s = h.screen();
    assert!(s.contains("gen-report.docx"), "{}", s);
    assert!(!s.contains("attr-bold"), "filtered out: {}", s);

    // Enter on the highlighted row opens it.
    h.key(KeyCode::Enter, NONE);
    assert!(matches!(h.app.overlay, Overlay::None));
    assert_eq!(h.app.path, std::fs::canonicalize(corpus("gen-report.docx")).ok());
    assert!(h.screen().contains("Quarterly Report"));
}

#[test]
fn open_dialog_navigates_by_arrows_and_typed_paths() {
    let dir = corpus("");
    if !dir.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.browse(&dir, false);
    // Left goes to the parent, which lists the corpus directory again.
    h.key(KeyCode::Left, NONE);
    let parent = match &h.app.overlay {
        Overlay::Browse { dir, .. } => dir.clone(),
        o => panic!("{:?}", o),
    };
    assert!(parent.ends_with("wp"), "{}", parent.display());
    assert!(h.screen().contains("corpus/"), "{}", h.screen());

    // Typing a path with a slash hops to that directory and keeps the tail.
    h.type_str("corpus/gen");
    match &h.app.overlay {
        Overlay::Browse { dir, filter, .. } => {
            assert!(dir.ends_with("corpus"), "{}", dir.display());
            assert_eq!(filter, "gen");
        }
        o => panic!("{:?}", o),
    }

    // Non-documents are hidden until Alt+A, and dot-files until asked for.
    h.key(KeyCode::Char('u'), CTRL);
    let docs = h.screen();
    assert!(!docs.contains("make_corpus"), "{}", docs);
    h.key(KeyCode::Left, NONE);
    h.key(KeyCode::Char('a'), ALT);
    assert!(h.screen().contains("Cargo.toml"), "{}", h.screen());

    // Esc closes without touching the document.
    h.key(KeyCode::Esc, NONE);
    assert!(matches!(h.app.overlay, Overlay::None));
    assert!(h.app.path.is_none());
}

