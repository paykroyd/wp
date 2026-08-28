//! `wp` — a word processor for the terminal.

mod app;
mod commands;
mod config;
mod keymap;
mod palette;
mod ui;
#[cfg(test)]
mod tests;

use app::App;
use config::{Config, KeymapChoice};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use ui::ScreenLen as _;

fn usage() {
    println!("wp {} — a word processor for the terminal\n", env!("CARGO_PKG_VERSION"));
    println!("Usage: wp [FILE.docx | FILE.txt]");
    println!("       wp --classic | --modern    choose the keyboard for this run");
    println!("       wp --text FILE.docx        dump a .docx as plain text and exit");
    println!("       wp --check FILE.docx       report unsupported content and page count, then exit");
    println!("       wp --version");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file: Option<PathBuf> = None;
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
            "--check" => mode = "check",
            _ => file = Some(PathBuf::from(a)),
        }
    }

    if mode != "edit" {
        let Some(f) = file else {
            eprintln!("--{} needs a file", mode);
            std::process::exit(2);
        };
        match wp_docx::read(&f) {
            Ok(l) => {
                if mode == "text" {
                    print!("{}", wp_core::text::to_text(&l.doc, None));
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
            app.format = if ext == "docx" || ext.is_empty() { app::Format::Docx } else { app::Format::Text };
            app.message(format!("New file: {}", f.display()));
        }
    }
    app.check_recovery();

    if let Err(e) = run(&mut app) {
        let _ = restore_terminal();
        eprintln!("wp: {}", e);
        std::process::exit(1);
    }
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

fn setup_terminal() -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
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
    execute!(out, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

fn run(app: &mut App) -> anyhow::Result<()> {
    setup_terminal()?;
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
        if event::poll(Duration::from_millis(250))? {
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
                            Event::Paste(s) => app.ed.insert_str(&s),
                            _ => {}
                        }
                    }
                }
                Event::Resize(w, h) => app.resize(w, h),
                Event::Paste(s) => {
                    app.ed.insert_str(&s);
                    app.needs_redraw = true;
                }
                _ => {}
            }
        }
        app.autosave_tick();
        if app.quit {
            break;
        }
    }
    restore_terminal()?;
    Ok(())
}
