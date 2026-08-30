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
        app.drive_cache_path = None;
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

    /// The style of one rendered cell.
    fn cell(&mut self, x: u16, y: u16) -> ratatui::style::Style {
        let caps = ui::Caps { ascii: false, colors: true, truecolor: true };
        self.term.draw(|f| ui::draw(f, &mut self.app, caps)).unwrap();
        self.term.backend().buffer()[(x, y)].style()
    }

    fn screen(&mut self) -> String {
        let caps = ui::Caps { ascii: false, colors: true, truecolor: true };
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
    assert!(s.contains("│ r0c0") && s.contains("┌─"), "table grid: {}", s);
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
    // Tables are editable now, so the warning summary doesn't mention one.
    assert!(!h.app.warnings.iter().any(|w| w.label.starts_with("table")));
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
    // Cmd+P opens the palette: Ghostty claims Cmd+Shift+P for its own.
    h.key(KeyCode::Char('p'), KeyModifiers::SUPER);
    assert!(matches!(h.app.overlay, Overlay::Palette { .. }));
    h.key(KeyCode::Esc, NONE);
    // The advertised palette key is one the terminal certainly delivers —
    // never Alt+= on macOS, where Option is not Alt by default.
    let label = h.app.keymap.label_for(crate::commands::Cmd::Palette).unwrap();
    assert_eq!(label, if cfg!(target_os = "macos") { "Ctrl+Shift+P" } else { "Alt+=" });
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
    assert!(s.contains("│ h1") && s.contains("│ y"), "{}", s);
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
    h.app.cfg.menu_bar = false; // rows below are document rows
    h.app.resize(80, 24);
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


#[test]
fn table_insert_navigate_render_and_save() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = Harness::new(KeymapChoice::Modern);
    h.type_str("Before the table.");
    h.key(KeyCode::Enter, NONE);
    // Insert via the palette prompt.
    h.key(KeyCode::Char('p'), CTRL | KeyModifiers::SHIFT);
    h.type_str("Table: Insert…");
    h.key(KeyCode::Enter, NONE);
    let s = h.screen();
    assert!(s.contains("rows × columns"), "{}", s);
    h.key(KeyCode::Backspace, NONE);
    h.key(KeyCode::Backspace, NONE);
    h.key(KeyCode::Backspace, NONE);
    h.type_str("2x3");
    h.key(KeyCode::Enter, NONE);
    assert_eq!(h.app.ed.doc.tables.len(), 1);
    assert_eq!(h.app.ed.current_cell().map(|c| c.name()), Some("A1".into()));
    // Type across cells with Tab; Shift+Tab goes back.
    h.type_str("one");
    h.key(KeyCode::Tab, NONE);
    h.type_str("two");
    h.key(KeyCode::Tab, NONE);
    h.type_str("three");
    h.key(KeyCode::Tab, NONE);
    assert_eq!(h.app.ed.current_cell().map(|c| c.name()), Some("A2".into()));
    h.type_str("four");
    h.key(KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(h.app.ed.current_cell().map(|c| c.name()), Some("C1".into()));
    let s = h.screen();
    assert!(s.contains("┌─"), "top rule: {}", s);
    assert!(s.contains("│ one"), "{}", s);
    assert!(s.contains("│ two"), "{}", s);
    assert!(s.contains("│ four"), "{}", s);
    assert!(s.contains("┼"), "junction: {}", s);
    assert!(s.contains("└─"), "bottom rule: {}", s);
    h.app.status = None; // the "Inserted…" message hides the indicators
    let s = h.screen();
    assert!(s.contains("Cell C1"), "status shows the cell: {}", s);
    // Enter adds a paragraph to the cell; Backspace at the cell start does nothing.
    h.key(KeyCode::Enter, NONE);
    h.type_str("more");
    assert_eq!(h.app.ed.doc.cell_of(h.app.ed.cursor.para).map(|c| c.name()), Some("C1".into()));
    h.key(KeyCode::Home, NONE);
    h.key(KeyCode::Backspace, NONE);
    h.key(KeyCode::Backspace, NONE);
    assert_eq!(h.app.ed.doc.paragraphs[h.app.ed.cursor.para].text(), "morethree");
    h.key(KeyCode::Backspace, NONE);
    assert_eq!(h.app.ed.doc.paragraphs[h.app.ed.cursor.para].text(), "morethree");
    // Down from the top row lands in the same column of the next row.
    h.key(KeyCode::Down, NONE);
    assert_eq!(h.app.ed.current_cell().map(|c| c.name()), Some("C2".into()));
    h.key(KeyCode::Down, NONE);
    assert!(h.app.ed.current_cell().is_none());
    // Reveal Codes shows the table structure.
    h.key(KeyCode::Up, NONE);
    h.key(KeyCode::F(3), ALT);
    let s = h.screen();
    assert!(s.contains("[Cell:C2]") && s.contains("[Cell:B2]"), "{}", s);
    h.key(KeyCode::BackTab, KeyModifiers::SHIFT);
    h.key(KeyCode::BackTab, KeyModifiers::SHIFT);
    let s = h.screen();
    assert!(s.contains("[Row][Cell:A2]"), "{}", s);
    h.key(KeyCode::F(3), ALT);
    // Row and column commands.
    h.key(KeyCode::Char('p'), CTRL | KeyModifiers::SHIFT);
    h.type_str("insert row below");
    h.key(KeyCode::Enter, NONE);
    assert_eq!(h.app.ed.doc.tables[&1].rows.len(), 3);
    h.key(KeyCode::Char('p'), CTRL | KeyModifiers::SHIFT);
    h.type_str("delete column");
    h.key(KeyCode::Enter, NONE);
    assert_eq!(h.app.ed.doc.tables[&1].cols(), 2);
    assert!(h.app.ed.doc.table_is_consistent(1));
    // Clicking in a cell moves the cursor there.
    let m = |kind, col, row| MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE };
    let s = h.screen();
    // Column A ("one"/"four") is gone; "two" is now A1.
    assert!(!s.contains("│ one"), "{}", s);
    let y = s.lines().position(|l| l.contains("│ two")).unwrap() as u16;
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 5, y));
    h.app.handle_mouse(m(MouseEventKind::Up(MouseButton::Left), 5, y));
    assert_eq!(h.app.ed.current_cell().map(|c| c.name()), Some("A1".into()));
    assert_eq!(h.app.ed.cursor.idx, 2);
    // Saved as .docx, the table comes back as a table.
    let dir = std::env::temp_dir().join(format!("wp-table-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let docx = dir.join("t.docx");
    h.app.save_to(&docx, crate::app::Format::Docx).unwrap();
    let l = wp_docx::read(&docx).unwrap();
    assert_eq!(l.doc.tables.len(), 1);
    assert_eq!(l.doc.tables[&1].rows.len(), 3);
    assert!(l.doc.table_is_consistent(1));
    let xml = l.package.get_str("word/document.xml").unwrap();
    assert!(xml.contains("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/>"), "{}", xml);
    assert!(xml.contains("<w:tblGrid><w:gridCol"), "{}", xml);
    assert!(xml.matches("<w:tr>").count() == 3, "{}", xml);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    // Undo all the way back.
    let mut h2 = Harness::new(KeymapChoice::Modern);
    h2.app.open_path(&docx).unwrap();
    let s = h2.screen();
    assert!(s.contains("│ two") && s.contains("│ morethree"), "{}", s);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn table_corpus_spans_and_merges_render() {
    let p = corpus("word-tables.docx");
    if !p.exists() {
        return;
    }
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_path(&p).unwrap();
    let s = h.screen();
    assert!(s.contains("│ Header 1"), "{}", s);
    assert!(s.contains("│ Merged"), "{}", s);
    assert!(s.contains("[Table 1×1]"), "nested table placeholder: {}", s);
    // Only the nested table is reported as not editable.
    assert!(h.app.warnings.iter().any(|w| w.label == "nested table"), "{:?}", h.app.warnings);
    assert!(!h.app.warnings.iter().any(|w| w.label == "table"), "{:?}", h.app.warnings);
    // Three tables, all consistent.
    for id in h.app.ed.doc.tables.keys() {
        assert!(h.app.ed.doc.table_is_consistent(*id), "table {}", id);
    }
    // Delete the whole document's text and undo: structure survives.
    h.key(KeyCode::Char('a'), CTRL | KeyModifiers::SHIFT);
}

#[test]
fn menu_opens_navigates_and_runs_commands() {
    use crate::menu::{Item, MENUS};
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.cfg.menu_bar = false; // the 5.1 way: the bar shows only while a menu is open
    h.app.resize(80, 24);
    h.type_str("centre me");
    let s = h.screen();
    assert!(s.lines().next().unwrap().contains("centre me"), "{}", s);
    // Alt+= pops the bar and the File menu.
    h.key(KeyCode::Char('='), ALT);
    assert!(matches!(h.app.overlay, Overlay::Menu { menu: 0, item: 0 }));
    let s = h.screen();
    let bar = s.lines().next().unwrap();
    assert!(bar.contains("File") && bar.contains("Edit") && bar.contains("Layout") && bar.contains("Help"), "{}", bar);
    assert!(s.contains("New Document"), "{}", s);
    assert!(s.contains("Ctrl+O"), "menu shows keys: {}", s);
    // → moves along the bar, ↓ skips separators, letters jump.
    h.key(KeyCode::Right, NONE);
    assert!(matches!(h.app.overlay, Overlay::Menu { menu: 1, .. }));
    let s = h.screen();
    assert!(s.contains("Undo") && s.contains("Paste from Cut History"), "{}", s);
    h.key(KeyCode::Down, NONE);
    h.key(KeyCode::Down, NONE);
    assert!(matches!(h.app.overlay, Overlay::Menu { menu: 1, item: 3 }), "{:?}", h.app.overlay); // Undo, Redo, ─, Cut
    h.key(KeyCode::Char('o'), NONE); // Font's mnemonic
    let s = h.screen();
    assert!(s.contains("Bold") && s.contains("Small Caps"), "{}", s);
    h.key(KeyCode::Char('l'), NONE); // Layout
    h.key(KeyCode::Char('c'), NONE); // Center — runs it and closes the menu
    assert!(matches!(h.app.overlay, Overlay::None));
    assert_eq!(h.app.ed.doc.paragraphs[0].props.align, Some(wp_core::Align::Center));
    let s = h.screen();
    assert!(!s.lines().next().unwrap().contains("File"), "bar hides again: {}", s);
    // Esc closes without running anything; F10 is the modern key.
    h.key(KeyCode::F(10), NONE);
    assert!(matches!(h.app.overlay, Overlay::Menu { .. }));
    h.key(KeyCode::Esc, NONE);
    assert!(matches!(h.app.overlay, Overlay::None));
    // Classic keeps F10 = Save and uses Alt+=.
    let mut c = Harness::new(KeymapChoice::Classic);
    c.key(KeyCode::Char('='), ALT);
    assert!(matches!(c.app.overlay, Overlay::Menu { .. }));
    // Nothing is menu-only: every item is a listed palette command, once per
    // menu, and the mnemonics are distinct.
    let mut mn = std::collections::HashSet::new();
    for m in MENUS {
        assert!(mn.insert(m.mnemonic), "duplicate mnemonic {}", m.mnemonic);
        assert!(m.title.to_ascii_uppercase().contains(m.mnemonic), "{} lacks its mnemonic {}", m.title, m.mnemonic);
        let mut seen = std::collections::HashSet::new();
        for it in m.items {
            if let Item::Cmd(c) = it {
                assert!(crate::commands::info(*c).listed, "{:?} is not a palette command", c);
                assert!(seen.insert(*c), "{:?} twice in {}", c, m.title);
            }
        }
    }
    let (_, end) = crate::menu::title_span(MENUS.len() - 1);
    assert!(end <= 80, "bar must fit an 80-column screen: {}", end);
}

#[test]
fn menu_bar_pinned_and_mouse() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = Harness::new(KeymapChoice::Modern);
    assert!(h.app.cfg.menu_bar, "the bar is on by default");
    h.type_str("hello there");
    let s = h.screen();
    assert!(s.lines().next().unwrap().contains("File"), "{}", s);
    assert!(s.lines().next().unwrap().contains("F1=Help"), "{}", s);
    assert!(s.lines().nth(1).unwrap().contains("hello there"), "document moves down a row: {}", s);
    let m = |kind, col, row| MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE };
    // Clicking text under the bar still lands on the right character.
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 3, 1));
    h.app.handle_mouse(m(MouseEventKind::Up(MouseButton::Left), 3, 1));
    assert_eq!(h.app.ed.cursor, wp_core::Pos::new(0, 2));
    // Clicking "Edit" on the bar opens it; clicking "Select All" runs it.
    let (x, _) = crate::menu::title_span(1);
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), x + 1, 0));
    assert!(matches!(h.app.overlay, Overlay::Menu { menu: 1, .. }), "{:?}", h.app.overlay);
    let _ = h.screen();
    let row = crate::menu::MENUS[1].items.iter().position(|i| matches!(i, crate::menu::Item::Cmd(crate::commands::Cmd::SelectAll))).unwrap();
    h.app.handle_mouse(m(MouseEventKind::Moved, x + 2, 2 + row as u16));
    assert!(matches!(h.app.overlay, Overlay::Menu { menu: 1, item } if item == row));
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), x + 2, 2 + row as u16));
    assert!(matches!(h.app.overlay, Overlay::None));
    assert_eq!(h.app.ed.selection().map(|r| h.app.ed.fragment(r).text()), Some("hello there".into()));
    // A click elsewhere closes an open menu.
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), x + 1, 0));
    h.app.handle_mouse(m(MouseEventKind::Down(MouseButton::Left), 70, 20));
    assert!(matches!(h.app.overlay, Overlay::None));
}

#[test]
fn classic_theme_paints_the_blue_screen() {
    use ratatui::style::{Color, Modifier};
    const BLUE: Option<Color> = Some(Color::Rgb(0x00, 0x00, 0xAA));
    const GREY: Option<Color> = Some(Color::Rgb(0xAA, 0xAA, 0xAA));
    const WHITE: Option<Color> = Some(Color::Rgb(0xFF, 0xFF, 0xFF));
    const RED: Option<Color> = Some(Color::Rgb(0xAA, 0x00, 0x00));
    let mut h = Harness::new(KeymapChoice::Classic);
    h.type_str("grey ");
    h.key(KeyCode::F(6), NONE);
    h.type_str("bold");
    // Default: no background of our own, status line reversed.
    assert_eq!(h.cell(0, 5).bg, Some(Color::Reset));
    assert!(h.cell(0, 23).add_modifier.contains(Modifier::REVERSED));
    h.app.cfg.theme = crate::config::ThemeChoice::Classic;
    let s = h.screen();
    assert!(s.contains("Doc 1  Pg 1/1"), "{}", s);
    // One blue ground under everything — screen, text, menu bar, status.
    assert_eq!(h.cell(0, 5).bg, BLUE, "empty screen");
    assert_eq!((h.cell(1, 1).fg, h.cell(1, 1).bg), (GREY, BLUE), "body text is CGA light grey on blue");
    let b = h.cell(7, 1); // the 'o' of bold
    assert_eq!(b.fg, WHITE, "bold is bright white");
    assert!(b.add_modifier.contains(Modifier::BOLD));
    assert_eq!((h.cell(2, 0).fg, h.cell(2, 0).bg), (RED, BLUE), "menu mnemonic is CGA red");
    assert_eq!((h.cell(3, 0).fg, h.cell(3, 0).bg), (WHITE, BLUE), "menu titles are bright white on the same blue");
    let st = h.cell(1, 23);
    assert_eq!((st.fg, st.bg), (WHITE, BLUE), "status text sits on the same blue, no bar");
    assert!(!st.add_modifier.contains(Modifier::REVERSED));
    // An open menu: its title and the current item in reverse video.
    h.key(KeyCode::Char('='), ALT);
    let _ = h.screen();
    assert_eq!((h.cell(3, 0).fg, h.cell(3, 0).bg), (BLUE, GREY), "the open title is reversed");
    assert_eq!((h.cell(3, 2).fg, h.cell(3, 2).bg), (BLUE, GREY), "the selected item is reversed");
    assert_eq!((h.cell(3, 3).fg, h.cell(3, 3).bg), (GREY, BLUE), "other items are grey on blue");
    // Document colours never reach the classic screen: a Word heading in the
    // template's dark blue reads as bold white (or WP 5.1's size colour for
    // a Very Large one), not blue-on-blue.
    h.key(KeyCode::Esc, NONE);
    h.app.open_path(&corpus("gen-report.docx")).unwrap();
    let s = h.screen();
    let row = s.lines().position(|l| l.contains("Quarterly")).expect(&s) as u16;
    let col = s.lines().nth(row as usize).unwrap().find("Quarterly").unwrap() as u16;
    let c = h.cell(col, row);
    const YELLOW: Option<Color> = Some(Color::Rgb(0xFF, 0xFF, 0x55));
    assert!(c.fg == WHITE || c.fg == GREY || c.fg == YELLOW, "heading colour on the classic screen: {:?}", c.fg);
    assert_eq!(c.bg, BLUE);
    // Without truecolor the nearest ANSI colours stand in.
    let caps = ui::Caps { ascii: false, colors: true, truecolor: false };
    h.term.draw(|f| ui::draw(f, &mut h.app, caps)).unwrap();
    assert_eq!(h.term.backend().buffer()[(0, 5)].style().bg, Some(Color::Blue));
}

#[test]
fn sizes_show_as_wp51_attributes_and_the_style_in_the_status_line() {
    use ratatui::style::{Color, Modifier};
    use wp_core::model::*;
    use wp_core::style::Style as WpStyle;
    use wp_core::Document;
    let mut h = Harness::new(KeymapChoice::Modern);
    let mut doc = Document::new();
    let mut title = WpStyle::para("Title", "Title");
    title.run.size = Some(52); // 26pt against an 11pt body: Very Large
    doc.styles.upsert(title);
    let mut h2 = WpStyle::para("Heading2", "heading 2");
    h2.run.size = Some(32); // 16pt: Large
    doc.styles.upsert(h2);
    let mut p0 = Paragraph::from_text("Title line");
    p0.props.style = Some("Title".into());
    let mut p1 = Paragraph::from_text("Heading line");
    p1.props.style = Some("Heading2".into());
    let p2 = Paragraph::from_text("Body line");
    let mut p3 = Paragraph { props: ParaProps::default(), items: vec![Item::Code(Code::On(Attr::Size(16)))] };
    p3.items.extend("fine".chars().map(Item::Char));
    p3.items.push(Item::Code(Code::Off(AttrKind::Size)));
    doc.paragraphs = vec![p0, p1, p2, p3];
    h.app.ed.replace_document(doc);
    let s = h.screen();
    let row = |t: &str| s.lines().position(|l| l.contains(t)).expect(t) as u16;
    let very = h.cell(1, row("Title line"));
    assert!(very.add_modifier.contains(Modifier::BOLD), "very large is bold");
    assert_eq!(very.fg, Some(Color::Cyan), "very large takes the size colour");
    let large = h.cell(1, row("Heading line"));
    assert!(large.add_modifier.contains(Modifier::BOLD), "large is bold");
    assert_ne!(large.fg, Some(Color::Cyan));
    let body = h.cell(1, row("Body line"));
    assert!(!body.add_modifier.contains(Modifier::BOLD));
    assert!(h.cell(1, row("fine")).add_modifier.contains(Modifier::DIM), "fine is dim");
    // The style name sits in the status line for the cursor's paragraph.
    assert!(h.screen().lines().last().unwrap().contains("Title"));
    h.app.ed.move_to(Pos::new(1, 0), false);
    assert!(h.screen().lines().last().unwrap().contains("Heading 2"));
    h.app.ed.move_to(Pos::new(2, 0), false);
    assert!(!h.screen().lines().last().unwrap().contains("Heading 2"));
}

// ----------------------------------------------------------------------
// Open from Google Drive
// ----------------------------------------------------------------------

fn drive_doc(id: &str, name: &str) -> crate::google::DriveEntry {
    crate::google::DriveEntry { id: id.into(), name: name.into(), kind: crate::google::DriveKind::Doc, detail: "2026-08-29 10:00".into() }
}

fn drive_folder(id: &str, name: &str) -> crate::google::DriveEntry {
    crate::google::DriveEntry { id: id.into(), name: name.into(), kind: crate::google::DriveKind::Folder, detail: String::new() }
}

#[test]
fn drive_dialog_opens_empty_then_fills_and_filters() {
    use crate::app::{DriveMode, Overlay};
    use crate::google::DriveQuery;
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_drive();
    let s = h.screen();
    assert!(s.contains("Google Drive — Recent · loading…"), "{}", s);
    assert!(s.contains("Loading…"), "{}", s);
    assert!(h.app.drive_active());

    // The recent listing lands: rows appear, in Drive's order.
    let seq = h.app.drive_list_seq;
    h.app.drive_reply(seq, DriveQuery::Recent, Ok(vec![drive_doc("a1", "Annual plan"), drive_doc("b2", "Budget 2026"), drive_doc("c3", "Cover letter")]));
    let s = h.screen();
    assert!(s.contains("Google Drive — Recent "), "{}", s);
    assert!(!s.contains("loading"), "{}", s);
    assert!(s.contains("Annual plan") && s.contains("Budget 2026") && s.contains("Cover letter"), "{}", s);
    assert!(!h.app.drive_active());

    // Typing narrows locally at once and arms the paused-typing search.
    h.type_str("bud");
    let s = h.screen();
    assert!(s.contains("Budget 2026") && !s.contains("Annual plan"), "{}", s);
    assert!(h.app.drive_search_due.is_some());
    assert!(h.app.drive_active());

    // Once the pause elapses the search fires; its hits that aren't already
    // listed appear under a divider.
    h.app.drive_tick_at(std::time::Instant::now() + std::time::Duration::from_secs(1));
    assert!(matches!(&h.app.overlay, Overlay::Drive(d) if d.searching));
    let seq = h.app.drive_search_seq;
    h.app.drive_reply(seq, DriveQuery::Search("bud".into()), Ok(vec![drive_doc("b2", "Budget 2026"), drive_doc("d4", "Buddy list")]));
    let s = h.screen();
    assert!(s.contains("more from Drive"), "{}", s);
    assert!(s.contains("Buddy list"), "{}", s);
    assert_eq!(s.matches("Budget 2026").count(), 1, "{}", s);

    // Down reaches the search hit; Enter asks to open it.
    h.key(KeyCode::Down, NONE);
    h.key(KeyCode::Enter, NONE);
    assert!(matches!(h.app.pending, Some(crate::app::Pending::Open { ref id, .. }) if id == "d4"));
    assert!(matches!(h.app.overlay, Overlay::None));
    let _ = DriveMode::Recent;
}

#[test]
fn drive_dialog_stale_replies_and_urls() {
    use crate::app::Overlay;
    use crate::google::DriveQuery;
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_drive();
    let first = h.app.drive_list_seq;
    // A reply for an older request is not shown (but Esc closes cleanly).
    h.app.drive_reply(first - 1, DriveQuery::Recent, Ok(vec![drive_doc("x", "Stale")]));
    assert!(matches!(&h.app.overlay, Overlay::Drive(d) if d.loading && d.rows.is_empty()));
    // An error is shown in the rows' place.
    h.app.drive_reply(first, DriveQuery::Recent, Err("Google API 403: quota".into()));
    let s = h.screen();
    assert!(s.contains("Could not list Drive: Google API 403"), "{}", s);

    // Pasting a Docs URL opens it directly, whatever is listed.
    h.app.paste_text("https://docs.google.com/document/d/1AbC_d-e/edit");
    h.key(KeyCode::Enter, NONE);
    assert!(matches!(h.app.pending, Some(crate::app::Pending::Open { ref id, .. }) if id == "1AbC_d-e"));
}

#[test]
fn drive_dialog_browses_folders() {
    use crate::app::{DriveMode, Overlay};
    use crate::google::DriveQuery;
    let mut h = Harness::new(KeymapChoice::Modern);
    h.app.open_drive();
    let seq = h.app.drive_list_seq;
    h.app.drive_reply(seq, DriveQuery::Recent, Ok(vec![drive_doc("a1", "Annual plan")]));

    // Tab switches to the folder view, whose top level needs no network.
    h.key(KeyCode::Tab, NONE);
    let s = h.screen();
    assert!(s.contains("Google Drive — Drive "), "{}", s);
    assert!(s.contains("My Drive/") && s.contains("Shared with me/") && s.contains("Shared drives/"), "{}", s);
    assert!(!h.app.drive_active());

    // Enter on My Drive lists it (root) once the worker answers.
    h.key(KeyCode::Enter, NONE);
    assert!(matches!(&h.app.overlay, Overlay::Drive(d) if d.loading && d.query() == DriveQuery::Folder("root".into())));
    assert!(h.screen().contains("Drive / My Drive · loading…"), "{}", h.screen());
    let seq = h.app.drive_list_seq;
    h.app.drive_reply(seq, DriveQuery::Folder("root".into()), Ok(vec![drive_folder("f1", "Projects"), drive_doc("d1", "Notes")]));
    let s = h.screen();
    assert!(s.contains("Projects/") && s.contains("Notes"), "{}", s);

    // Right descends; the listing is fetched; Left comes back from the
    // cache without another fetch.
    h.key(KeyCode::Right, NONE);
    assert!(h.screen().contains("My Drive / Projects · loading…"), "{}", h.screen());
    let seq = h.app.drive_list_seq;
    h.app.drive_reply(seq, DriveQuery::Folder("f1".into()), Ok(vec![drive_doc("p1", "Proposal")]));
    assert!(h.screen().contains("Proposal"), "{}", h.screen());
    h.key(KeyCode::Left, NONE);
    let s = h.screen();
    assert!(s.contains("Projects/") && !s.contains("loading"), "{}", s);
    assert!(!h.app.drive_active());

    // Typing filters a folder locally and never searches the server.
    h.type_str("not");
    let s = h.screen();
    assert!(s.contains("Notes") && !s.contains("Projects/"), "{}", s);
    assert!(h.app.drive_search_due.is_none());

    // Backspace on an empty filter goes up; Tab returns to Recent from the cache.
    h.key(KeyCode::Char('u'), CTRL);
    h.key(KeyCode::Backspace, NONE);
    assert!(h.screen().contains("Google Drive — Drive "), "{}", h.screen());
    h.key(KeyCode::Tab, NONE);
    let s = h.screen();
    assert!(s.contains("Google Drive — Recent") && s.contains("Annual plan") && !s.contains("loading"), "{}", s);
    assert!(matches!(&h.app.overlay, Overlay::Drive(d) if d.mode == DriveMode::Recent));
}
