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
