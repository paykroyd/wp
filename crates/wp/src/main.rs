//! `wp` — a word processor for the terminal.

mod app;
mod commands;
mod config;
mod google;
mod keymap;
mod menu;
mod pageview;
mod palette;
mod ui;
#[cfg(test)]
mod tests;

use app::App;
use config::{Config, KeymapChoice};
use crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use ui::ScreenLen as _;

fn usage() {
    println!("wp {} — a word processor for the terminal\n", env!("CARGO_PKG_VERSION"));
    println!("Usage: wp [FILE.docx | FILE.md | FILE.txt]");
    println!("       wp gdoc:<id> | <docs.google.com URL>   open a Google Doc (needs [google] in config.toml)");
    println!("       wp --classic | --modern    choose the keyboard for this run");
    println!("       wp --text FILE.docx        dump a .docx as plain text and exit");
    println!("       wp --md FILE.docx          dump a .docx as Markdown and exit");
    println!("       wp --check FILE.docx       report unsupported content and page count, then exit");
    println!("       wp --check gdoc:<id>       the same for a Google Doc (also --text, --md, --json); never writes");
    println!("       wp --probe-keys            show what your terminal sends for each key, then Esc Esc Esc");
    println!("       wp --version");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file: Option<PathBuf> = None;
    let mut gdoc: Option<String> = None;
    let mut keymap_override: Option<KeymapChoice> = None;
    let mut mode = "edit";
    for a in &args {
        match a.as_str() {
            "-h" | "--help" => {
                usage();
                return;
            }
            "-V" | "--version" => {
                println!("wp {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--classic" => keymap_override = Some(KeymapChoice::Classic),
            "--modern" => keymap_override = Some(KeymapChoice::Modern),
            "--text" => mode = "text",
            "--md" => mode = "md",
            "--probe-keys" => mode = "probe",
            "--check" => mode = "check",
            "--json" => mode = "json",
            _ => match google::parse_doc_ref(a) {
                Some(id) => gdoc = Some(id),
                None => file = Some(PathBuf::from(a)),
            },
        }
    }

    if mode == "probe" {
        if let Err(e) = probe_keys() {
            eprintln!("wp: {}", e);
        }
        return;
    }
    if mode != "edit" {
        if let Some(id) = gdoc {
            // Read-only: sign in if needed, fetch, report. Nothing is written.
            let (cfg, _) = Config::load();
            if let Err(e) = check_gdoc(&cfg, &id, mode) {
                eprintln!("wp: {}", e);
                std::process::exit(1);
            }
            return;
        }
        let Some(f) = file else {
            eprintln!("--{} needs a file", mode);
            std::process::exit(2);
        };
        match wp_docx::read(&f) {
            Ok(l) => {
                if mode == "text" {
                    print!("{}", wp_core::text::to_text(&l.doc, None));
                } else if mode == "md" {
                    let rels = |id: &str| l.package.rel_target(id);
                    let e = wp_md::to_markdown(&l.doc, &rels);
                    print!("{}", e.text);
                    if let Some(w) = e.warning() {
                        eprintln!("{}", w);
                    }
                } else {

                    let warning = l.warning_line();
                    let mut ed = wp_core::Editor::new(l.doc);
                    println!("{}: {} paragraphs, {} words, {} pages", f.display(), ed.doc.paragraphs.len(), ed.doc.word_count(), ed.page_count());
                    match warning {
                        Some(w) => println!("{}", w),
                        None => println!("Nothing unsupported."),
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: {}", f.display(), e);
                std::process::exit(1);
            }
        }
        return;
    }

    let (mut cfg, existed) = Config::load();
    if let Some(k) = keymap_override {
        cfg.keymap = k;
    } else if !existed {
        match first_run_prompt() {
            Some(k) => {
                cfg.keymap = k;
                let _ = cfg.save();
            }
            None => return,
        }
    }

    let mut app = App::new(cfg);
    if let Some(f) = &file {
        if f.exists() {
            if let Err(e) = app.open_path(f) {
                eprintln!("{}: {}", f.display(), e);
                std::process::exit(1);
            }
        } else {
            app.path = Some(f.clone());
            let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            app.format = match ext.as_str() {
                "docx" | "" => app::Format::Docx,
                "md" | "markdown" => app::Format::Markdown,
                _ => app::Format::Text,
            };

            app.message(format!("New file: {}", f.display()));
        }
    }
    if let Some(id) = gdoc {
        // Opened (and recovery checked) once the terminal is up, so the
        // sign-in message has a screen to appear on.
        app.queue(app::Pending::Open { id, force: true });
    } else {
        app.check_recovery();
    }

    if let Err(e) = run(&mut app) {
        let _ = restore_terminal();
        eprintln!("wp: {}", e);
        std::process::exit(1);
    }
}

/// `--check` / `--text` / `--md` on a Google Doc: the same reports, over the
/// Docs API, with the sign-in done on the command line.
fn check_gdoc(cfg: &Config, id: &str, mode: &str) -> anyhow::Result<()> {
    if !cfg.google.is_set() {
        anyhow::bail!("Google Docs needs [google] client_id and client_secret in {}", config::config_path().display());
    }
    let mut client = google::Client::new(cfg.google.clone());
    if !client.signed_in() {
        let flow = client.begin_sign_in()?;
        let opened = google::open_in_browser(&flow.url);
        eprintln!("{}\n\n  {}\n", if opened { "A browser window has been opened to sign in to Google. If it did not appear, open:" } else { "Open this address in a browser to sign in to Google:" }, flow.url);
        eprintln!("Waiting for the sign-in to complete…");
        client.finish_sign_in(flow, || false)?;
        eprintln!("Signed in.");
    }
    let json = client.get_document(id)?;
    if mode == "json" {
        println!("{}", json);
        return Ok(());
    }
    let l = wp_gdoc::read(&json).map_err(anyhow::Error::msg)?;
    if mode == "text" {
        print!("{}", wp_core::text::to_text(&l.doc, None));
    } else if mode == "md" {
        let rels = |rid: &str| l.doc.extra_rels.iter().find(|r| r.id == rid).map(|r| r.target.clone());
        let e = wp_md::to_markdown(&l.doc, &rels);
        print!("{}", e.text);
        if let Some(w) = e.warning() {
            eprintln!("{}", w);
        }
    } else {
        let mut ed = wp_core::Editor::new(l.doc);
        println!("{} (Google Doc {}, revision {}): {} paragraphs, {} words, {} pages", l.baseline.title, l.baseline.document_id, l.baseline.revision_id, ed.doc.paragraphs.len(), ed.doc.word_count(), ed.page_count());
        let unchanged = wp_gdoc::diff(&l.baseline, &ed.doc).map_err(anyhow::Error::msg)?;
        println!("Diff of the unedited document: {} requests (expected 0).", unchanged.len());
        if l.warnings.is_empty() {
            println!("Nothing unsupported.");
        } else {
            for w in &l.warnings {
                println!("{}", w);
            }
        }
    }
    Ok(())
}

/// Print raw key events so users can see what their terminal delivers
/// (e.g. whether Cmd reaches the program).
fn probe_keys() -> anyhow::Result<()> {
    println!("Press keys to see what wp receives. Esc three times to stop.\r");
    enable_raw_mode()?;
    let mut out = io::stdout();
    let enhanced = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS)
    )
    .is_ok();
    print!("kitty keyboard protocol: {}\r\n", if enhanced { "requested (works if the terminal supports it)" } else { "not available" });
    let mut escapes = 0;
    loop {
        if let Event::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Release {
                continue;
            }
            let key = keymap::Key::from_event(&k);
            print!("{:<28} raw: {:?} {:?}\r\n", key.label(), k.code, k.modifiers);
            io::stdout().flush().ok();
            if k.code == KeyCode::Esc {
                escapes += 1;
                if escapes >= 3 {
                    break;
                }
            } else {
                escapes = 0;
            }
        }
    }
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    Ok(())
}

/// First launch: one question, one line of explanation.
fn first_run_prompt() -> Option<KeymapChoice> {
    println!("Welcome to wp.\n");
    println!("Which keyboard would you like?  (change any time: Ctrl+K → Keyboard)\n");
    println!("  1) Modern   Ctrl+S save, Ctrl+B bold, Ctrl+Z undo, Shift+arrows select.");
    println!("              WordPerfect F-keys also work underneath.");
    println!("  2) Classic  WordPerfect 5.1: F6 bold, F8 underline, F10 save, F7 exit,");
    println!("              Alt+F3 reveal codes, Alt+F4 block, Esc repeat count.\n");
    print!("Choose 1 or 2 [1]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return Some(KeymapChoice::Modern);
    }
    match line.trim() {
        "2" | "classic" | "c" => Some(KeymapChoice::Classic),
        "q" => None,
        _ => Some(KeymapChoice::Modern),
    }
}

fn setup_terminal(mouse: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        let _ = execute!(out, EnableMouseCapture);
    }
    // Kitty keyboard protocol where available: disambiguates Shift+F8, Ctrl+Enter, etc.
    let _ = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS)
    );
    Ok(())
}

fn restore_terminal() -> io::Result<()> {
    let mut out = io::stdout();
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste);
    execute!(out, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

/// Hand text to the terminal's clipboard with OSC 52 (through tmux's
/// passthrough when inside tmux). Terminals that disallow it ignore it.
fn osc52_copy(text: &str) {
    let b64 = base64(text.as_bytes());
    let seq = if std::env::var("TMUX").is_ok() {
        format!("\x1bPtmux;\x1b\x1b]52;c;{}\x07\x1b\\", b64)
    } else {
        format!("\x1b]52;c;{}\x07", b64)
    };
    let mut out = io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn run(app: &mut App) -> anyhow::Result<()> {
    setup_terminal(app.cfg.mouse)?;
    // Restore the terminal even if we panic, so the shell is usable.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let caps = ui::detect_caps();
    let size = terminal.size()?;
    app.resize(size.width, size.height);
    let _ = app.ed.layout_screen_len(0);

    loop {
        if app.needs_redraw {
            terminal.draw(|f| ui::draw(f, app, caps))?;
            app.needs_redraw = false;
        }
        if let Some(p) = app.pending.take() {
            app.run_pending(p);
            continue;
        }
        // Drive listings arrive on a channel; poll briefly while one is due.
        app.drive_tick();
        if app.needs_redraw {
            continue;
        }
        let wait = if app.drive_active() { 40 } else { 250 };
        if event::poll(Duration::from_millis(wait))? {
            match event::read()? {
                Event::Key(k) => {
                    if k.kind == KeyEventKind::Release {
                        continue;
                    }
                    // Ctrl+L redraws in any mode when unbound.
                    if k.code == KeyCode::Char('l') && k.modifiers.contains(event::KeyModifiers::CONTROL) && app.keymap.lookup(&keymap::Key::from_event(&k)).is_none() {
                        terminal.clear()?;
                        app.needs_redraw = true;
                        continue;
                    }
                    app.handle_key(k);
                    // Drain any queued keys (fast typing / paste) before redrawing.
                    while event::poll(Duration::from_millis(0))? {
                        match event::read()? {
                            Event::Key(k2) if k2.kind != KeyEventKind::Release => app.handle_key(k2),
                            Event::Resize(w, h) => app.resize(w, h),
                            Event::Paste(s) => app.paste_text(&s),
                            Event::Mouse(m) => app.handle_mouse(m),
                            _ => {}
                        }
                    }
                }
                Event::Mouse(m) => {
                    app.handle_mouse(m);
                    // Coalesce a burst of drag / wheel events.
                    while event::poll(Duration::from_millis(0))? {
                        match event::read()? {
                            Event::Mouse(m2) => app.handle_mouse(m2),
                            Event::Key(k2) if k2.kind != KeyEventKind::Release => app.handle_key(k2),
                            Event::Resize(w, h) => app.resize(w, h),
                            Event::Paste(s) => app.paste_text(&s),
                            _ => {}
                        }
                    }
                }
                Event::Resize(w, h) => app.resize(w, h),
                Event::Paste(s) => app.paste_text(&s),
                _ => {}
            }
        }
        if let Some(text) = app.clipboard_out.take() {
            osc52_copy(&text);
        }

        app.autosave_tick();
        if app.quit {
            break;
        }
    }
    restore_terminal()?;
    Ok(())
}
