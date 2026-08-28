//! Application state and command execution.

use crate::commands::{info, Cmd, COMMANDS};
use crate::config::{state_dir, Config, KeymapChoice};
use crate::keymap::{Key, Keymap};
use crate::palette;
use crossterm::event::{KeyCode, KeyEvent};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wp_core::model::*;
use wp_core::reveal::{self, ParaCode};
use wp_core::{Document, Editor, Fragment, ListKind};
use wp_docx::{DocxPackage, Warning};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Draft,
    Page,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Open,
    SaveAs(Format),
    Find { backward: bool },
    ReplaceFind,
    ReplaceWith { find: String },
    GoToPage,
    Bookmark,
    FontName,
    FontSize,
    SpaceBefore,
    SpaceAfter,
    FirstLine,
    Margins,
    TabSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    ExitSave,
    NewDiscard,
    OpenDiscard(PathBuf),
    Recover(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListAction {
    ApplyStyle,
    PasteRing,
    GoToHeading,
    GoToBookmark,
    FontColor,
    HighlightColor,
    ListFormat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub label: String,
    pub detail: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette { input: String, selected: usize },
    Prompt { kind: PromptKind, label: String, input: String },
    List { title: String, items: Vec<ListItem>, selected: usize, action: ListAction, filter: String },
    Confirm { question: String, action: ConfirmAction },
    Help,
    Message { title: String, lines: Vec<String> },
}

/// One palette result.
#[derive(Clone, Debug)]
pub struct PaletteRow {
    pub label: String,
    pub detail: String,
    pub key: String,
    pub action: PaletteAction,
}

#[derive(Clone, Debug)]
pub enum PaletteAction {
    Cmd(Cmd),
    GoTo(Pos),
    Page(usize),
    Help,
}

#[derive(Default, Clone)]
pub struct FindState {
    pub query: String,
    pub backward: bool,
    pub origin: Pos,
    pub origin_anchor: Option<Pos>,
}

pub struct App {
    pub ed: Editor,
    pub path: Option<PathBuf>,
    pub format: Format,
    pub package: Option<DocxPackage>,
    pub warnings: Vec<Warning>,
    pub cfg: Config,
    pub keymap: Keymap,
    pub overlay: Overlay,
    pub reveal: bool,
    pub reveal_all: bool,
    /// In Reveal Codes, the cursor may rest on one of the paragraph's
    /// property codes (index into `reveal::para_codes`).
    pub reveal_para_code: Option<usize>,
    pub view: View,
    pub status: Option<(String, Instant)>,
    pub sticky_status: Option<String>,
    pub scroll: (usize, usize),
    pub clipboard: Option<Fragment>,
    pub quit: bool,
    pub last_autosave: Instant,
    pub find: FindState,
    pub repeat: Option<u32>,
    pub repeat_armed: bool,
    pub hint: bool,
    pub size: (u16, u16),
    pub needs_redraw: bool,
    pub block_mode: bool,
    pub quit_after_save: bool,
}

const UNTITLED: &str = "Untitled";

impl App {
    pub fn new(cfg: Config) -> App {
        let keymap = Keymap::build(&cfg);
        let hint = cfg.show_hint;
        App {
            ed: Editor::new(Document::new()),
            path: None,
            format: Format::Docx,
            package: None,
            warnings: Vec::new(),
            cfg,
            keymap,
            overlay: Overlay::None,
            reveal: false,
            reveal_all: false,
            reveal_para_code: None,
            view: View::Draft,
            status: None,
            sticky_status: None,
            scroll: (0, 0),
            clipboard: None,
            quit: false,
            last_autosave: Instant::now(),
            find: FindState::default(),
            repeat: None,
            repeat_armed: false,
            hint,
            size: (80, 24),
            needs_redraw: true,
            block_mode: false,
            quit_after_save: false,
        }
    }

    pub fn title(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| UNTITLED.into())
    }

    pub fn message(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
        self.needs_redraw = true;
    }

    pub fn status_text(&self) -> Option<String> {
        if let Some((s, t)) = &self.status {
            if t.elapsed() < Duration::from_secs(6) {
                return Some(s.clone());
            }
        }
        self.sticky_status.clone()
    }

    // ------------------------------------------------------------------
    // Files
    // ------------------------------------------------------------------

    pub fn open_path(&mut self, path: &Path) -> anyhow::Result<()> {
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "docx" {
            let loaded = wp_docx::read(path)?;
            self.warnings = loaded.warnings;
            let line = wp_docx::Loaded { doc: Document::new(), package: DocxPackage::default(), warnings: self.warnings.clone() }.warning_line();
            self.ed.replace_document(loaded.doc);
            self.package = Some(loaded.package);
            self.format = Format::Docx;
            self.sticky_status = None;
            if let Some(l) = line {
                self.message(l);
            }
        } else {
            let bytes = std::fs::read(path)?;
            let text = decode_text(&bytes);
            self.ed.replace_document(wp_core::text::from_text(&text, false));
            self.package = None;
            self.format = Format::Text;
            self.warnings.clear();
        }
        self.path = Some(path.to_path_buf());
        self.scroll = (0, 0);
        self.reveal_para_code = None;
        self.needs_redraw = true;
        self.ed.set_cols(self.doc_cols());
        Ok(())
    }

    pub fn save_to(&mut self, path: &Path, format: Format) -> anyhow::Result<()> {
        self.ed.commit();
        match format {
            Format::Docx => {
                wp_docx::write(&self.ed.doc, self.package.as_ref(), path)?;
                // Re-read the package so preserved parts reflect the saved file.
                if let Ok(l) = wp_docx::read(path) {
                    self.package = Some(l.package);
                }
            }
            Format::Text => {
                let wrap = if self.cfg.text_wrap > 0 { Some(self.cfg.text_wrap) } else { None };
                std::fs::write(path, wp_core::text::to_text(&self.ed.doc, wrap))?;
            }
        }
        self.path = Some(path.to_path_buf());
        self.format = format;
        self.ed.dirty = false;
        self.remove_recovery();
        self.message(format!("Saved {}", path.display()));
        Ok(())
    }

    pub fn save(&mut self) {
        match self.path.clone() {
            Some(p) => {
                let f = self.format;
                if let Err(e) = self.save_to(&p, f) {
                    self.message(format!("Save failed: {}", e));
                }
            }
            None => self.prompt(PromptKind::SaveAs(Format::Docx), "Save as (.docx or .txt): ", ""),
        }
    }

    fn recovery_path(&self) -> PathBuf {
        let key = self.path.as_ref().map(|p| p.canonicalize().unwrap_or(p.clone()).to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
        let mut h: u64 = 0xcbf29ce484222325;
        for b in key.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        let ext = if self.format == Format::Docx { "docx" } else { "txt" };
        state_dir().join("recovery").join(format!("{:016x}.{}", h, ext))
    }

    pub fn autosave_tick(&mut self) {
        if !self.ed.dirty || self.last_autosave.elapsed() < Duration::from_secs(self.cfg.autosave_seconds.max(5)) {
            return;
        }
        self.last_autosave = Instant::now();
        let p = self.recovery_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        self.ed.commit();
        let res = match self.format {
            Format::Docx => wp_docx::write(&self.ed.doc, self.package.as_ref(), &p).map_err(|e| e.to_string()),
            Format::Text => std::fs::write(&p, wp_core::text::to_text(&self.ed.doc, None)).map_err(|e| e.to_string()),
        };
        if let Err(e) = res {
            self.message(format!("Autosave failed: {}", e));
        }
    }

    pub fn remove_recovery(&self) {
        let _ = std::fs::remove_file(self.recovery_path());
    }

    /// If a recovery file newer than the document exists, offer it.
    pub fn check_recovery(&mut self) {
        let p = self.recovery_path();
        let Ok(meta) = std::fs::metadata(&p) else { return };
        let newer = match self.path.as_ref().and_then(|d| std::fs::metadata(d).ok()) {
            Some(dm) => meta.modified().ok() > dm.modified().ok(),
            None => true,
        };
        if newer {
            self.overlay = Overlay::Confirm {
                question: "Unsaved changes from a previous session were found. Recover them? (y/n)".into(),
                action: ConfirmAction::Recover(p),
            };
        }
    }

    // ------------------------------------------------------------------
    // Layout helpers
    // ------------------------------------------------------------------

    pub fn doc_cols(&self) -> u16 {
        self.size.0.saturating_sub(2).max(20)
    }

    pub fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.ed.set_cols(self.doc_cols());
        self.needs_redraw = true;
    }

    pub fn doc_rows(&self) -> u16 {
        let mut h = self.size.1.saturating_sub(1); // status line
        if self.cfg.fkey_legend {
            h = h.saturating_sub(5);
        }
        if self.hint {
            h = h.saturating_sub(1);
        }
        if self.reveal {
            h = h.saturating_sub(h * 2 / 5 + 1);
        }
        h.max(3)
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    pub fn handle_key(&mut self, ev: KeyEvent) {
        self.needs_redraw = true;
        self.hint = false;
        if !matches!(self.overlay, Overlay::None) {
            let overlay = std::mem::replace(&mut self.overlay, Overlay::None);
            self.handle_overlay_key(overlay, ev);
            return;
        }
        let key = Key::from_event(&ev);
        // Classic repeat-count prefix: Esc, digits, then a key.
        if self.repeat_armed {
            if let KeyCode::Char(c) = key.code {
                if c.is_ascii_digit() && !key.ctrl && !key.alt {
                    let r = self.repeat.unwrap_or(0) * 10 + c.to_digit(10).unwrap();
                    self.repeat = Some(r.min(9999));
                    self.message(format!("Repeat: {}", r));
                    return;
                }
            }
            self.repeat_armed = false;
        }
        let n = self.repeat.take().unwrap_or(1).max(1);
        match self.keymap.lookup(&key) {
            Some(cmd) => {
                for _ in 0..n {
                    self.exec(cmd);
                    if !matches!(self.overlay, Overlay::None) {
                        break;
                    }
                }
            }
            None => {
                if let KeyCode::Char(c) = key.code {
                    if !key.ctrl && !key.alt && !key.sup {
                        // Typing with a plain uppercase Char: use the event's char.
                        let ch = if let KeyCode::Char(orig) = ev.code { orig } else { c };
                        for _ in 0..n {
                            self.type_char(ch);
                        }
                    }
                }
            }
        }
    }

    fn type_char(&mut self, c: char) {
        if let Some(msg) = self.ed.selection_protected() {
            self.message(msg);
            return;
        }
        self.reveal_para_code = None;
        self.ed.insert_char(c);
        self.block_mode = false;
    }

    fn guard_edit(&mut self) -> bool {
        if let Some(msg) = self.ed.selection_protected() {
            self.message(msg);
            return false;
        }
        true
    }

    pub fn prompt(&mut self, kind: PromptKind, label: &str, initial: &str) {
        self.overlay = Overlay::Prompt { kind, label: label.into(), input: initial.into() };
    }

    fn list(&mut self, title: &str, items: Vec<ListItem>, action: ListAction) {
        self.overlay = Overlay::List { title: title.into(), items, selected: 0, action, filter: String::new() };
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    pub fn exec(&mut self, cmd: Cmd) {
        let sel = self.block_mode;
        let codes = self.reveal;
        match cmd {
            Cmd::New => {
                if self.ed.dirty {
                    self.overlay = Overlay::Confirm { question: "Discard unsaved changes and start a new document? (y/n)".into(), action: ConfirmAction::NewDiscard };
                } else {
                    self.new_document();
                }
            }
            Cmd::Open => {
                let dir = self.path.as_ref().and_then(|p| p.parent()).map(|d| format!("{}/", d.display())).unwrap_or_default();
                self.prompt(PromptKind::Open, "Open: ", &dir);
            }
            Cmd::Save => self.save(),
            Cmd::SaveAs => {
                let init = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                self.prompt(PromptKind::SaveAs(self.format), "Save as: ", &init);
            }
            Cmd::SaveAsDocx => {
                let init = self.path.as_ref().map(|p| p.with_extension("docx").display().to_string()).unwrap_or_default();
                self.prompt(PromptKind::SaveAs(Format::Docx), "Save as .docx: ", &init);
            }
            Cmd::SaveAsText => {
                let init = self.path.as_ref().map(|p| p.with_extension("txt").display().to_string()).unwrap_or_default();
                if self.format == Format::Docx && !self.ed.doc.paragraphs.iter().all(|p| p.items.iter().all(|i| !i.is_code())) {
                    self.message("Saving as plain text drops all formatting, styles, and page setup.");
                }
                self.prompt(PromptKind::SaveAs(Format::Text), "Save as .txt: ", &init);
            }
            Cmd::Exit => {
                if self.ed.dirty {
                    self.overlay = Overlay::Confirm { question: "Save changes before exiting? (y = save, n = discard, Esc = cancel)".into(), action: ConfirmAction::ExitSave };
                } else {
                    self.remove_recovery();
                    self.quit = true;
                }
            }
            Cmd::Warnings => {
                let mut lines: Vec<String> = Vec::new();
                if self.warnings.is_empty() {
                    lines.push("Nothing in this document is unsupported. Everything you see can be edited.".into());
                } else {
                    lines.push("Preserved exactly on save, but not editable in this version of wp:".into());
                    lines.push(String::new());
                    for w in &self.warnings {
                        lines.push(format!("  {:>4}  {}{}", w.count, w.label, if w.count == 1 { "" } else { "s" }));
                    }
                    lines.push(String::new());
                    lines.push("Tables, comments, tracked changes, fields, and drawings show as labelled".into());
                    lines.push("placeholders. Editing inside a tracked change is refused with a message.".into());
                }
                self.overlay = Overlay::Message { title: "Warnings".into(), lines };
            }
            Cmd::Undo => {
                if !self.ed.undo() {
                    self.message("Nothing to undo");
                }
            }
            Cmd::Redo => {
                if !self.ed.redo() {
                    self.message("Nothing to redo");
                }
            }
            Cmd::Cut => {
                if self.guard_edit() {
                    match self.ed.cut() {
                        Some(f) => {
                            self.clipboard = Some(f);
                            self.block_mode = false;
                        }
                        None => self.message("Nothing selected — Shift+arrows or Alt+F4 to select"),
                    }
                }
            }
            Cmd::Copy => match self.ed.copy() {
                Some(f) => {
                    self.clipboard = Some(f);
                    self.ed.clear_selection();
                    self.block_mode = false;
                    self.message("Copied");
                }
                None => self.message("Nothing selected"),
            },
            Cmd::Paste => {
                if self.guard_edit() {
                    match self.clipboard.clone() {
                        Some(f) => self.ed.paste(&f),
                        None => self.message("Clipboard is empty"),
                    }
                }
            }
            Cmd::PastePlain => {
                if self.guard_edit() {
                    match self.clipboard.clone() {
                        Some(f) => self.ed.paste(&f.plain()),
                        None => self.message("Clipboard is empty"),
                    }
                }
            }
            Cmd::PasteFromRing => {
                let items: Vec<ListItem> = self
                    .ed
                    .cut_ring()
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let t = f.text().replace('\n', " ¶ ");
                        let t: String = t.chars().take(60).collect();
                        ListItem { label: format!("{}: {}", i + 1, t), detail: String::new(), value: i.to_string() }
                    })
                    .collect();
                if items.is_empty() {
                    self.message("Cut history is empty");
                } else {
                    self.list("Paste from cut history", items, ListAction::PasteRing);
                }
            }
            Cmd::SelectAll => {
                self.ed.select_all();
                self.block_mode = true;
            }
            Cmd::Block => {
                if self.block_mode {
                    self.block_mode = false;
                    self.ed.clear_selection();
                } else {
                    self.block_mode = true;
                    self.ed.start_selection();
                    self.message("Block on — move to extend, then Ctrl+F4/Ctrl+X to cut, F6 for bold…");
                }
            }
            Cmd::DeleteWord => {
                if self.guard_edit() {
                    let c = self.ed.cursor;
                    self.ed.word_right(false);
                    let e = self.ed.cursor;
                    self.ed.commit();
                    self.ed.delete_range(Range::new(c, e));
                    self.ed.commit();
                }
            }
            Cmd::DeleteToEndOfLine => {
                if self.guard_edit() {
                    let c = self.ed.cursor;
                    self.ed.move_end(false);
                    let e = self.ed.cursor;
                    self.ed.commit();
                    self.ed.delete_range(Range::new(c, e));
                    self.ed.commit();
                }
            }
            Cmd::DeleteToStartOfLine => {
                if self.guard_edit() {
                    let c = self.ed.cursor;
                    self.ed.move_home(false);
                    let s = self.ed.cursor;
                    self.ed.commit();
                    self.ed.delete_range(Range::new(s, c));
                    self.ed.commit();
                }
            }
            Cmd::Typeover => {
                self.ed.typeover = !self.ed.typeover;
            }
            Cmd::Cancel => {
                if self.block_mode || self.ed.has_selection() {
                    self.block_mode = false;
                    self.ed.clear_selection();
                } else {
                    self.reveal_para_code = None;
                    self.message("");
                }
            }
            Cmd::RepeatPrefix => {
                self.repeat_armed = true;
                self.repeat = None;
                self.message("Repeat count: type a number, then a key");
            }
            Cmd::Bold => self.toggle(Attr::Bold(true)),
            Cmd::Italic => self.toggle(Attr::Italic(true)),
            Cmd::Underline => self.toggle(Attr::Underline(Underline::Single)),
            Cmd::DoubleUnderline => self.toggle(Attr::Underline(Underline::Double)),
            Cmd::Strikethrough => self.toggle(Attr::Strike(true)),
            Cmd::Superscript => self.toggle(Attr::VertAlign(VertAlign::Superscript)),
            Cmd::Subscript => self.toggle(Attr::VertAlign(VertAlign::Subscript)),
            Cmd::SmallCaps => self.toggle(Attr::SmallCaps(true)),
            Cmd::AllCaps => self.toggle(Attr::AllCaps(true)),
            Cmd::Font => {
                let cur = self.ed.doc.run_props_at(self.ed.cursor).font.unwrap_or_default();
                self.prompt(PromptKind::FontName, "Font (Calibri, Times New Roman, Arial, Courier New, Cambria…): ", &cur);
            }
            Cmd::FontSize => {
                let cur = self.ed.doc.run_props_at(self.ed.cursor).size_hp();
                let s = if cur % 2 == 0 { format!("{}", cur / 2) } else { format!("{}.5", cur / 2) };
                self.prompt(PromptKind::FontSize, "Font size (points): ", &s);
            }
            Cmd::FontColor => {
                let mut items: Vec<ListItem> = [
                    ("Automatic (remove color)", ""),
                    ("Black", "000000"),
                    ("Dark Red", "C00000"),
                    ("Red", "FF0000"),
                    ("Orange", "FFC000"),
                    ("Yellow", "FFFF00"),
                    ("Green", "00B050"),
                    ("Dark Green", "00602B"),
                    ("Blue", "0070C0"),
                    ("Dark Blue", "002060"),
                    ("Purple", "7030A0"),
                    ("Gray", "808080"),
                ]
                .iter()
                .map(|(n, v)| ListItem { label: n.to_string(), detail: if v.is_empty() { String::new() } else { format!("#{}", v) }, value: v.to_string() })
                .collect();
                items.push(ListItem { label: "Custom hex…".into(), detail: "type #RRGGBB in the filter and press Enter".into(), value: "custom".into() });
                self.list("Text color", items, ListAction::FontColor);
            }
            Cmd::Highlight => {
                let mut items = vec![ListItem { label: "None (remove highlight)".into(), detail: String::new(), value: "none".into() }];
                for h in Highlight::all() {
                    items.push(ListItem { label: h.docx_name().to_string(), detail: String::new(), value: h.docx_name().to_string() });
                }
                self.list("Highlight", items, ListAction::HighlightColor);
            }
            Cmd::RemoveFormatting => {
                if self.ed.has_selection() {
                    if self.guard_edit() {
                        self.ed.clear_char_formatting();
                    }
                } else {
                    self.message("Select text first");
                }
            }
            Cmd::AlignLeft => self.para(|p| p.align = Some(Align::Left)),
            Cmd::AlignCenter => self.para(|p| p.align = Some(Align::Center)),
            Cmd::AlignRight => self.para(|p| p.align = Some(Align::Right)),
            Cmd::AlignJustify => self.para(|p| p.align = Some(Align::Justify)),
            Cmd::Indent => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).indent_left();
                self.para(move |p| p.indent_left = Some(cur + 720));
            }
            Cmd::Outdent => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).indent_left();
                self.para(move |p| p.indent_left = Some((cur - 720).max(0)));
            }
            Cmd::IndentLeftRight => {
                let pp = self.ed.doc.para_props(self.ed.cursor.para);
                let (l, r) = (pp.indent_left() + 720, pp.indent_right() + 720);
                self.para(move |p| {
                    p.indent_left = Some(l);
                    p.indent_right = Some(r);
                });
            }
            Cmd::HangingIndent => {
                let pp = self.ed.doc.para_props(self.ed.cursor.para);
                let on = pp.hanging.unwrap_or(0) > 0;
                let l = pp.indent_left();
                self.para(move |p| {
                    if on {
                        p.hanging = None;
                        p.indent_left = Some((l - 720).max(0));
                    } else {
                        p.hanging = Some(720);
                        p.first_line = None;
                        p.indent_left = Some(l + 720);
                    }
                });
            }
            Cmd::FirstLineIndent => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).first_line.unwrap_or(0);
                self.prompt(PromptKind::FirstLine, "First line indent (inches, e.g. 0.5): ", &inches(cur));
            }
            Cmd::SpacingSingle => self.para(|p| p.line_spacing = Some(LineSpacing::Auto(240))),
            Cmd::SpacingOneHalf => self.para(|p| p.line_spacing = Some(LineSpacing::Auto(360))),
            Cmd::SpacingDouble => self.para(|p| p.line_spacing = Some(LineSpacing::Auto(480))),
            Cmd::SpaceBefore => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).space_before();
                self.prompt(PromptKind::SpaceBefore, "Space before (points): ", &points(cur));
            }
            Cmd::SpaceAfter => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).space_after();
                self.prompt(PromptKind::SpaceAfter, "Space after (points): ", &points(cur));
            }
            Cmd::KeepWithNext => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).keep_next();
                self.para(move |p| p.keep_next = Some(!cur));
                self.message(if cur { "Keep with next: off" } else { "Keep with next: on" });
            }
            Cmd::KeepLinesTogether => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).keep_lines();
                self.para(move |p| p.keep_lines = Some(!cur));
                self.message(if cur { "Keep lines together: off" } else { "Keep lines together: on" });
            }
            Cmd::PageBreakBefore => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).page_break_before();
                self.para(move |p| p.page_break_before = Some(!cur));
            }
            Cmd::WidowOrphan => {
                let cur = self.ed.doc.para_props(self.ed.cursor.para).widow_control();
                self.para(move |p| p.widow_control = Some(!cur));
                self.message(if cur { "Widow/orphan control: off" } else { "Widow/orphan control: on" });
            }
            Cmd::TabSet => {
                let pp = self.ed.doc.para_props(self.ed.cursor.para);
                let cur: Vec<String> = pp.tabs.iter().filter(|t| !t.clear).map(|t| {
                    let k = match t.kind {
                        TabKind::Left => "",
                        TabKind::Center => "c",
                        TabKind::Right => "r",
                        TabKind::Decimal => "d",
                        TabKind::Bar => "|",
                    };
                    let l = match t.leader {
                        TabLeader::None => "",
                        TabLeader::Dot => ".",
                        TabLeader::Hyphen => "-",
                        TabLeader::Underscore => "_",
                    };
                    format!("{}{}{}", inches(t.pos), k, l)
                }).collect();
                self.prompt(PromptKind::TabSet, "Tab stops in inches (e.g. 1 2.5r 6.5r. — r/c/d = right/center/decimal, . = dot leader): ", &cur.join(" "));
            }
            Cmd::ApplyStyle | Cmd::StyleBrowser => {
                let items = self.style_items(cmd == Cmd::StyleBrowser);
                self.list(if cmd == Cmd::StyleBrowser { "Style browser — Enter applies" } else { "Apply style" }, items, ListAction::ApplyStyle);
            }
            Cmd::StyleNormal => self.apply_style_named("Normal"),
            Cmd::StyleHeading1 => self.apply_style_named("Heading1"),
            Cmd::StyleHeading2 => self.apply_style_named("Heading2"),
            Cmd::StyleHeading3 => self.apply_style_named("Heading3"),
            Cmd::StyleTitle => self.apply_style_named("Title"),
            Cmd::PageBreak => {
                if self.guard_edit() {
                    self.ed.insert_code(Code::PageBreak);
                }
            }
            Cmd::LineBreak => {
                if self.guard_edit() {
                    self.ed.insert_code(Code::LineBreak);
                }
            }
            Cmd::InsertTab => {
                if self.guard_edit() {
                    // Tab at the start of a list item demotes it, as in Word.
                    let c = self.ed.cursor;
                    let at_start = self.ed.doc.paragraphs[c.para].items[..c.idx].iter().all(|i| i.is_code());
                    if at_start && !self.ed.has_selection() && self.ed.doc.list_ref(c.para).is_some() {
                        self.list_level(1);
                    } else {
                        self.ed.insert_code(Code::Tab);
                    }
                }
            }
            Cmd::ListBullet => self.toggle_list(ListKind::Bullet),
            Cmd::ListNumber => self.toggle_list(ListKind::Decimal),
            Cmd::ListFormat => {
                let items: Vec<ListItem> = ListKind::all()
                    .iter()
                    .enumerate()
                    .map(|(i, k)| ListItem { label: k.title().to_string(), detail: String::new(), value: i.to_string() })
                    .collect();
                self.list("List numbering format", items, ListAction::ListFormat);
            }
            Cmd::ListIndent => self.list_level(1),
            Cmd::ListOutdent => {
                if self.ed.doc.list_ref(self.ed.cursor.para).is_some() {
                    self.list_level(-1);
                } else {
                    self.exec(Cmd::Outdent);
                }
            }
            Cmd::ListRestart => {
                let para = self.ed.cursor.para;
                match self.ed.doc.list_ref(para) {
                    Some(r) => {
                        if let Some(id) = self.ed.doc.numbering.restart(r.num_id, 1) {
                            self.ed.dirty = true;
                            self.para(move |p| p.list = Some(ListRef { num_id: id, level: r.level }));
                        }
                    }
                    None => self.message("Not in a list"),
                }
            }
            Cmd::ListContinue => {
                let para = self.ed.cursor.para;
                let prev = (0..para).rev().find_map(|i| self.ed.doc.list_ref(i));
                match prev {
                    Some(r) => {
                        let level = self.ed.doc.list_ref(para).map(|l| l.level).unwrap_or(r.level);
                        self.apply_list(r.num_id, Some(level));
                    }
                    None => self.message("No list above this paragraph to continue"),
                }
            }
            Cmd::ListRemove => self.remove_list(),
            Cmd::Bookmark => self.prompt(PromptKind::Bookmark, "Bookmark name: ", ""),
            Cmd::Date => {
                if self.guard_edit() {
                    self.ed.insert_str(&today());
                }
            }
            Cmd::PageSetup | Cmd::Margins => {
                let s = &self.ed.doc.section;
                let cur = format!("{} {} {} {}", inches(s.margin_top), inches(s.margin_bottom), inches(s.margin_left), inches(s.margin_right));
                self.prompt(PromptKind::Margins, "Margins in inches — top bottom left right: ", &cur);
            }
            Cmd::PaperLetter => self.section(|s| {
                s.page_width = 12240;
                s.page_height = 15840;
                if s.orientation == Orientation::Landscape {
                    std::mem::swap(&mut s.page_width, &mut s.page_height);
                }
            }),
            Cmd::PaperA4 => self.section(|s| {
                s.page_width = 11906;
                s.page_height = 16838;
                if s.orientation == Orientation::Landscape {
                    std::mem::swap(&mut s.page_width, &mut s.page_height);
                }
            }),
            Cmd::Landscape => self.section(|s| {
                if s.orientation != Orientation::Landscape {
                    s.orientation = Orientation::Landscape;
                    std::mem::swap(&mut s.page_width, &mut s.page_height);
                }
            }),
            Cmd::Portrait => self.section(|s| {
                if s.orientation != Orientation::Portrait {
                    s.orientation = Orientation::Portrait;
                    std::mem::swap(&mut s.page_width, &mut s.page_height);
                }
            }),
            Cmd::RevealCodes => {
                self.reveal = !self.reveal;
                self.reveal_para_code = None;
                self.ed.set_cols(self.doc_cols());
            }
            Cmd::RevealAllCodes => {
                self.reveal_all = !self.reveal_all;
                if !self.reveal {
                    self.reveal = true;
                }
            }
            Cmd::ToggleView => {
                self.view = if self.view == View::Draft { View::Page } else { View::Draft };
                self.message(if self.view == View::Page { "Page view" } else { "Draft view" });
            }
            Cmd::FkeyLegend => {
                self.cfg.fkey_legend = !self.cfg.fkey_legend;
                let _ = self.cfg.save();
            }
            Cmd::WordCount => {
                let words = self.ed.doc.word_count();
                let chars = self.ed.doc.char_count();
                let paras = self.ed.doc.paragraphs.len();
                let pages = self.ed.page_count();
                let sel = self.ed.selection().map(|r| self.ed.fragment(r));
                let mut lines = vec![
                    format!("Words        {:>8}", words),
                    format!("Characters   {:>8}", chars),
                    format!("Paragraphs   {:>8}", paras),
                    format!("Pages        {:>8}", pages),
                ];
                if let Some(f) = sel {
                    let d = Document::from_paragraphs(f.paragraphs);
                    lines.push(String::new());
                    lines.push(format!("Selection: {} words, {} characters", d.word_count(), d.char_count()));
                }
                self.overlay = Overlay::Message { title: "Word count".into(), lines };
            }
            Cmd::Redraw => {}
            Cmd::Palette => self.overlay = Overlay::Palette { input: String::new(), selected: 0 },
            Cmd::GoToPage => {
                let n = self.ed.page_count();
                self.prompt(PromptKind::GoToPage, &format!("Go to page (1–{}): ", n), "");
            }
            Cmd::GoToHeading => {
                let items: Vec<ListItem> = self
                    .ed
                    .doc
                    .headings()
                    .into_iter()
                    .map(|(pi, lvl, text)| ListItem { label: format!("{}{}", "  ".repeat(lvl as usize), text), detail: format!("¶ {}", pi + 1), value: pi.to_string() })
                    .collect();
                if items.is_empty() {
                    self.message("No headings in this document");
                } else {
                    self.list("Go to heading", items, ListAction::GoToHeading);
                }
            }
            Cmd::GoToBookmark => {
                let items: Vec<ListItem> = self
                    .ed
                    .doc
                    .bookmarks()
                    .into_iter()
                    .map(|(n, p)| ListItem { label: n, detail: format!("¶ {}", p.para + 1), value: format!("{}:{}", p.para, p.idx) })
                    .collect();
                if items.is_empty() {
                    self.message("No bookmarks in this document");
                } else {
                    self.list("Go to bookmark", items, ListAction::GoToBookmark);
                }
            }
            Cmd::Find | Cmd::FindBackward => {
                self.find.origin = self.ed.cursor;
                self.find.origin_anchor = self.ed.anchor;
                self.find.backward = cmd == Cmd::FindBackward;
                let q = self.find.query.clone();
                self.prompt(PromptKind::Find { backward: cmd == Cmd::FindBackward }, if cmd == Cmd::FindBackward { "Find backward: " } else { "Find: " }, &q);
            }
            Cmd::FindNext => self.find_step(false),
            Cmd::FindPrev => self.find_step(true),
            Cmd::Replace => {
                let q = self.find.query.clone();
                self.prompt(PromptKind::ReplaceFind, "Replace — find: ", &q);
            }
            Cmd::MoveLeft => self.move_left(sel),
            Cmd::MoveRight => self.move_right(sel),
            Cmd::MoveUp => self.ed.move_up(sel),
            Cmd::MoveDown => self.ed.move_down(sel),
            Cmd::WordLeft => self.ed.word_left(sel),
            Cmd::WordRight => self.ed.word_right(sel),
            Cmd::LineStart => self.ed.move_home(sel),
            Cmd::LineEnd => self.ed.move_end(sel),
            Cmd::ParaUp => self.ed.move_para_up(sel),
            Cmd::ParaDown => self.ed.move_para_down(sel),
            Cmd::PageUp => self.ed.move_lines(-(self.doc_rows() as i32 - 1), sel),
            Cmd::PageDown => self.ed.move_lines(self.doc_rows() as i32 - 1, sel),
            Cmd::DocStart => self.ed.move_doc_start(sel),
            Cmd::DocEnd => self.ed.move_doc_end(sel),
            Cmd::NextPage | Cmd::PrevPage => {
                let (pg, _, _) = self.ed.cursor_page_ln_pos();
                let target = if cmd == Cmd::NextPage { pg + 1 } else { pg.saturating_sub(1) };
                if let Some(p) = self.ed.page_start_pos(target) {
                    self.ed.move_to(p, sel);
                } else {
                    self.message(if cmd == Cmd::NextPage { "Last page" } else { "First page" });
                }
            }
            Cmd::SelectLeft => self.move_left(true),
            Cmd::SelectRight => self.move_right(true),
            Cmd::SelectUp => self.ed.move_up(true),
            Cmd::SelectDown => self.ed.move_down(true),
            Cmd::SelectWordLeft => self.ed.word_left(true),
            Cmd::SelectWordRight => self.ed.word_right(true),
            Cmd::SelectLineStart => self.ed.move_home(true),
            Cmd::SelectLineEnd => self.ed.move_end(true),
            Cmd::SelectDocStart => self.ed.move_doc_start(true),
            Cmd::SelectDocEnd => self.ed.move_doc_end(true),
            Cmd::SelectPageUp => self.ed.move_lines(-(self.doc_rows() as i32 - 1), true),
            Cmd::SelectPageDown => self.ed.move_lines(self.doc_rows() as i32 - 1, true),
            Cmd::KeyboardClassic => self.set_keymap(KeymapChoice::Classic),
            Cmd::KeyboardModern => self.set_keymap(KeymapChoice::Modern),
            Cmd::Help => self.overlay = Overlay::Help,
            Cmd::About => {
                self.overlay = Overlay::Message {
                    title: "About wp".into(),
                    lines: vec![
                        format!("wp {} — a word processor for the terminal", env!("CARGO_PKG_VERSION")),
                        String::new(),
                        "Opens and saves .docx natively. Reveal Codes shows why formatting".into(),
                        "is the way it is. Every command is in the palette (Ctrl+K).".into(),
                        String::new(),
                        "No network. No telemetry. One binary.".into(),
                    ],
                };
            }
            Cmd::Backspace => {
                if self.reveal {
                    if let Some(i) = self.reveal_para_code {
                        self.delete_para_code(i);
                        return;
                    }
                }
                if self.guard_edit() {
                    // Backspace at the start of a list item removes its number first.
                    let c = self.ed.cursor;
                    if c.idx == 0 && !self.ed.has_selection() && !self.reveal && self.ed.doc.list_ref(c.para).is_some() {
                        self.remove_list();
                        return;
                    }
                    self.ed.backspace(codes);
                    self.block_mode = false;
                }
            }
            Cmd::Delete => {
                if self.reveal {
                    if let Some(i) = self.reveal_para_code {
                        self.delete_para_code(i);
                        return;
                    }
                }
                if self.guard_edit() {
                    if self.reveal && !self.ed.has_selection() {
                        let c = self.ed.cursor;
                        if c.idx < self.ed.doc.paragraphs[c.para].items.len() {
                            self.ed.commit();
                            self.ed.delete_item_at(c);
                            self.ed.commit();
                            return;
                        }
                    }
                    self.ed.delete_forward(codes);
                    self.block_mode = false;
                }
            }
            Cmd::Enter => {
                if self.guard_edit() {
                    // Enter on an empty list item ends the list.
                    let c = self.ed.cursor;
                    if self.ed.doc.paragraphs[c.para].char_count() == 0 && !self.ed.has_selection() && self.ed.doc.list_ref(c.para).is_some() {
                        self.remove_list();
                        return;
                    }
                    self.ed.newline();
                    self.block_mode = false;
                    self.reveal_para_code = None;
                }
            }
        }
        if !matches!(cmd, Cmd::MoveLeft | Cmd::MoveRight | Cmd::Backspace | Cmd::Delete) {
            self.reveal_para_code = None;
        }
    }

    fn move_left(&mut self, sel: bool) {
        if self.reveal && !sel {
            let c = self.ed.cursor;
            let n = reveal::para_codes(&self.ed.doc.paragraphs[c.para].props).len();
            match self.reveal_para_code {
                Some(i) if i > 0 => {
                    self.reveal_para_code = Some(i - 1);
                    return;
                }
                Some(_) => {
                    // Move to previous paragraph's end.
                    if c.para > 0 {
                        let p = Pos::new(c.para - 1, self.ed.doc.paragraphs[c.para - 1].items.len());
                        self.ed.move_to(p, false);
                    }
                    self.reveal_para_code = None;
                    return;
                }
                None => {
                    if c.idx == 0 && n > 0 {
                        self.reveal_para_code = Some(n - 1);
                        return;
                    }
                }
            }
        }
        self.ed.move_left(sel, self.reveal);
    }

    fn move_right(&mut self, sel: bool) {
        if self.reveal && !sel {
            if let Some(i) = self.reveal_para_code {
                let c = self.ed.cursor;
                let n = reveal::para_codes(&self.ed.doc.paragraphs[c.para].props).len();
                self.reveal_para_code = if i + 1 < n { Some(i + 1) } else { None };
                return;
            }
        }
        let c = self.ed.cursor;
        let at_end = c.idx >= self.ed.doc.paragraphs[c.para].items.len();
        if self.reveal && !sel && at_end && c.para + 1 < self.ed.doc.paragraphs.len() {
            // Step onto the next paragraph's property codes, if any.
            let n = reveal::para_codes(&self.ed.doc.paragraphs[c.para + 1].props).len();
            self.ed.move_to(Pos::new(c.para + 1, 0), false);
            self.reveal_para_code = if n > 0 { Some(0) } else { None };
            return;
        }
        self.ed.move_right(sel, self.reveal);
    }

    fn delete_para_code(&mut self, i: usize) {
        let para = self.ed.cursor.para;
        let codes = reveal::para_codes(&self.ed.doc.paragraphs[para].props);
        if let Some((which, label)) = codes.get(i) {
            if *which == ParaCode::RawBlock {
                self.message("This block is preserved as a whole and can't be removed here");
                return;
            }
            self.ed.clear_para_code(para, *which);
            self.message(format!("Deleted {}", label));
            let n = codes.len() - 1;
            self.reveal_para_code = if n == 0 { None } else { Some(i.min(n - 1)) };
        }
    }

    fn toggle(&mut self, attr: Attr) {
        if self.guard_edit() {
            self.ed.toggle_attr(attr);
        }
    }

    // ------------------------------------------------------------------
    // Lists
    // ------------------------------------------------------------------

    /// Make the selected paragraphs items of `kind`, continuing the list
    /// directly above when it is the same kind; toggles off when they
    /// already are that kind.
    fn toggle_list(&mut self, kind: ListKind) {
        let para = *self.ed.selected_paras().start();
        let doc = &self.ed.doc;
        let current = doc.list_ref(para);
        if let Some(r) = current {
            if doc.numbering.is_bullet(r.num_id, r.level) == kind.is_bullet() {
                self.remove_list();
                return;
            }
        }
        let above = para.checked_sub(1).and_then(|i| doc.list_ref(i)).filter(|r| doc.numbering.is_bullet(r.num_id, 0) == kind.is_bullet());
        let num_id = match above {
            Some(r) => r.num_id,
            None => {
                let existing = doc.numbering.find_kind(kind);
                match existing {
                    Some(id) if kind.is_bullet() => id,
                    Some(id) => self.ed.doc.numbering.restart(id, 1).unwrap_or(id),
                    None => self.ed.doc.numbering.add_list(kind),
                }
            }
        };
        self.ed.dirty = true;
        self.apply_list(num_id, None);
    }

    fn apply_list(&mut self, num_id: i32, level: Option<u8>) {
        let has_lp = self.ed.doc.styles.get("ListParagraph").is_some();
        self.para(move |p| {
            let lvl = level.or_else(|| p.list.map(|l| l.level)).unwrap_or(0);
            p.list = Some(ListRef { num_id, level: lvl });
            if has_lp && p.style.is_none() {
                p.style = Some("ListParagraph".into());
            }
        });
    }

    fn remove_list(&mut self) {
        let styles = self.ed.doc.styles.clone();
        self.para(move |p| {
            let from_style = styles.resolve_para_style(p.style.as_deref()).list.map_or(false, |l| l.num_id > 0);
            p.list = if from_style { Some(ListRef { num_id: 0, level: 0 }) } else { None };
            if p.style.as_deref() == Some("ListParagraph") {
                p.style = None;
            }
        });
    }

    fn list_level(&mut self, delta: i8) {
        let para = self.ed.cursor.para;
        if self.ed.doc.list_ref(para).is_none() {
            self.message("Not in a list — Ctrl+Shift+L for bullets, Ctrl+Shift+O for numbering");
            return;
        }
        let doc_list = |p: &ParaProps, styles: &wp_core::StyleSheet| p.list.or(styles.resolve_para_style(p.style.as_deref()).list);
        let styles = self.ed.doc.styles.clone();
        self.para(move |p| {
            if let Some(r) = doc_list(p, &styles).filter(|r| r.num_id > 0) {
                let level = (r.level as i8 + delta).clamp(0, 8) as u8;
                p.list = Some(ListRef { num_id: r.num_id, level });
            }
        });
    }

    fn para(&mut self, f: impl Fn(&mut ParaProps)) {
        self.ed.update_para_props(f);
    }

    fn section(&mut self, f: impl Fn(&mut SectionProps)) {
        let mut s = self.ed.doc.section.clone();
        f(&mut s);
        self.ed.set_section(s);
    }

    fn apply_style_named(&mut self, id: &str) {
        let real = self.ed.doc.styles.find(id).map(|s| s.id.clone());
        match real {
            Some(r) => self.ed.set_style(&r),
            None => {
                // Add it from the built-in set so the document gains the style.
                if let Some(st) = wp_core::StyleSheet::builtin().get(id).cloned() {
                    let mut st = st;
                    st.raw_xml = None;
                    self.ed.doc.styles.upsert(st);
                    self.ed.set_style(id);
                } else {
                    self.message(format!("No style named {}", id));
                }
            }
        }
    }

    fn style_items(&self, browser: bool) -> Vec<ListItem> {
        let doc = &self.ed.doc;
        let cur_para = &doc.paragraphs[self.ed.cursor.para];
        let cur_style = cur_para.props.style.clone().or_else(|| doc.styles.default_para_style().map(|s| s.id.clone()));
        let mut items = Vec::new();
        for s in doc.styles.paragraph_styles() {
            let chain: Vec<String> = doc.styles.chain(&s.id).iter().rev().skip(1).map(|c| c.name.clone()).collect();
            let mut detail = if chain.is_empty() { String::new() } else { format!("← {}", chain.join(" ← ")) };
            if browser && cur_style.as_deref() == Some(&s.id) {
                let mut over: Vec<String> = reveal::para_codes(&cur_para.props).into_iter().filter(|(k, _)| *k != ParaCode::Style).map(|(_, l)| l).collect();
                let attrs = doc.attrs_at(self.ed.cursor);
                over.extend(attrs.values().filter(|a| a.kind().is_visible()).map(|a| format!("[{}]", reveal::attr_label(a))));
                detail = format!("{}  (current{})", detail, if over.is_empty() { ", no direct overrides".to_string() } else { format!("; overridden by {}", over.join(" ")) });
            }
            items.push(ListItem { label: s.name.clone(), detail, value: s.id.clone() });
        }
        items.sort_by(|a, b| {
            let rank = |l: &str| if l == "Normal" { 0 } else if l.starts_with("heading") || l.starts_with("Heading") { 1 } else { 2 };
            rank(&a.label).cmp(&rank(&b.label)).then(a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
        items
    }

    fn set_keymap(&mut self, k: KeymapChoice) {
        self.cfg.keymap = k;
        self.keymap = Keymap::build(&self.cfg);
        let _ = self.cfg.save();
        self.message(match k {
            KeymapChoice::Classic => "Classic keyboard: F6 bold, F8 underline, F10 save, F7 exit, Alt+F3 reveal codes, F1 cancel, Esc repeat",
            KeymapChoice::Modern => "Modern keyboard: emacs movement on Ctrl/Alt, Cmd+B/S/Z where the terminal delivers Cmd, Ctrl+Shift+P palette — F-keys keep their classic meanings",
        });
    }

    fn new_document(&mut self) {
        self.ed.replace_document(Document::new());
        self.path = None;
        self.package = None;
        self.format = Format::Docx;
        self.warnings.clear();
        self.scroll = (0, 0);
        self.sticky_status = None;
        self.ed.set_cols(self.doc_cols());
    }

    // ------------------------------------------------------------------
    // Find
    // ------------------------------------------------------------------

    /// Search from `from` for `q`. Returns the match range.
    pub fn search(&self, q: &str, from: Pos, backward: bool, wrap: bool) -> Option<Range> {
        if q.is_empty() {
            return None;
        }
        let smart_case = q.chars().any(|c| c.is_uppercase());
        let qn: Vec<char> = if smart_case { q.chars().collect() } else { q.to_lowercase().chars().collect() };
        let doc = &self.ed.doc;
        let n = doc.paragraphs.len();
        let order: Vec<usize> = if backward {
            (0..=from.para).rev().chain(if wrap { (from.para + 1..n).rev().collect::<Vec<_>>() } else { vec![] }).collect()
        } else {
            (from.para..n).chain(if wrap { 0..from.para } else { 0..0 }).collect()
        };
        for (k, pi) in order.iter().enumerate() {
            let p = &doc.paragraphs[*pi];
            // chars with item indices
            let mut chars: Vec<(char, usize)> = Vec::with_capacity(p.items.len());
            for (i, it) in p.items.iter().enumerate() {
                match it {
                    Item::Char(c) => chars.push((if smart_case { *c } else { c.to_lowercase().next().unwrap_or(*c) }, i)),
                    Item::Code(Code::Tab) => chars.push((' ', i)),
                    _ => {}
                }
            }
            if chars.len() < qn.len() {
                continue;
            }
            let first_para = k == 0;
            let positions: Vec<usize> = (0..=chars.len() - qn.len()).collect();
            let iter: Box<dyn Iterator<Item = usize>> = if backward { Box::new(positions.into_iter().rev()) } else { Box::new(positions.into_iter()) };
            for ci in iter {
                let item_start = chars[ci].1;
                if first_para && *pi == from.para {
                    if !backward && item_start < from.idx {
                        continue;
                    }
                    if backward && item_start >= from.idx {
                        continue;
                    }
                }
                if chars[ci..ci + qn.len()].iter().zip(qn.iter()).all(|((c, _), q)| c == q) {
                    let end_item = chars[ci + qn.len() - 1].1 + 1;
                    return Some(Range { start: Pos::new(*pi, item_start), end: Pos::new(*pi, end_item) });
                }
            }
        }
        None
    }

    fn find_step(&mut self, reverse: bool) {
        if self.find.query.is_empty() {
            self.exec(Cmd::Find);
            return;
        }
        let backward = self.find.backward ^ reverse;
        let from = if backward {
            self.ed.selection().map(|r| r.start).unwrap_or(self.ed.cursor)
        } else {
            self.ed.selection().map(|r| r.end).unwrap_or(self.ed.cursor)
        };
        let q = self.find.query.clone();
        match self.search(&q, from, backward, true) {
            Some(r) => {
                self.ed.anchor = Some(r.start);
                self.ed.cursor = r.end;
                self.block_mode = false;
            }
            None => self.message(format!("Not found: {}", q)),
        }
    }

    fn incremental_find(&mut self, q: &str, backward: bool) {
        let from = self.find.origin;
        match self.search(q, from, backward, true) {
            Some(r) => {
                self.ed.anchor = Some(r.start);
                self.ed.cursor = r.end;
                let count = self.count_matches(q);
                self.sticky_status = Some(format!("{} match{}", count, if count == 1 { "" } else { "es" }));
            }
            None => {
                self.ed.cursor = from;
                self.ed.anchor = self.find.origin_anchor;
                self.sticky_status = Some(if q.is_empty() { String::new() } else { "No matches".into() });
            }
        }
    }

    fn count_matches(&self, q: &str) -> usize {
        let mut n = 0;
        let mut from = Pos::default();
        while let Some(r) = self.search(q, from, false, false) {
            n += 1;
            from = r.end;
            if n > 9999 {
                break;
            }
        }
        n
    }

    fn replace_all(&mut self, find: &str, with: &str) {
        let mut n = 0;
        let mut from = Pos::default();
        self.ed.commit();
        while let Some(r) = self.search(find, from, false, false) {
            self.ed.cursor = r.start;
            self.ed.anchor = Some(r.end);
            if self.ed.selection_protected().is_some() {
                from = r.end;
                continue;
            }
            self.ed.delete_range(r);
            let start = self.ed.cursor;
            let items: Vec<Item> = with.chars().map(Item::Char).collect();
            let len = items.len();
            if len > 0 {
                self.ed.paste(&Fragment { paragraphs: vec![Paragraph { props: ParaProps::default(), items }] });
            }
            from = Pos::new(start.para, start.idx + len);
            n += 1;
            if n > 100_000 {
                break;
            }
        }
        self.ed.commit();
        self.ed.anchor = None;
        self.message(format!("Replaced {} occurrence{}", n, if n == 1 { "" } else { "s" }));
    }

    // ------------------------------------------------------------------
    // Overlays
    // ------------------------------------------------------------------

    pub fn palette_rows(&mut self, input: &str) -> Vec<PaletteRow> {
        let (mode, q) = match input.chars().next() {
            Some('@') => ('@', &input[1..]),
            Some('#') => ('#', &input[1..]),
            Some('/') => ('/', &input[1..]),
            Some('?') => ('?', &input[1..]),
            Some('>') => ('>', &input[1..]),
            _ => ('>', input),
        };
        let q = q.trim();
        match mode {
            '@' => {
                let mut rows: Vec<(i32, PaletteRow)> = Vec::new();
                for (pi, lvl, text) in self.ed.doc.headings() {
                    if let Some(s) = palette::score(q, &text) {
                        rows.push((s, PaletteRow { label: format!("{}{}", "  ".repeat(lvl as usize), text), detail: String::new(), key: format!("¶{}", pi + 1), action: PaletteAction::GoTo(Pos::new(pi, 0)) }));
                    }
                }
                if !q.is_empty() {
                    rows.sort_by(|a, b| b.0.cmp(&a.0));
                }
                rows.into_iter().map(|r| r.1).collect()
            }
            '#' => {
                let n = self.ed.page_count();
                let want: Option<usize> = q.parse().ok();
                (1..=n)
                    .filter(|p| want.map_or(true, |w| p.to_string().starts_with(&w.to_string())))
                    .take(50)
                    .map(|p| PaletteRow { label: format!("Page {}", p), detail: String::new(), key: String::new(), action: PaletteAction::Page(p) })
                    .collect()
            }
            '/' => {
                let count = if q.is_empty() { 0 } else { self.count_matches(q) };
                vec![PaletteRow {
                    label: if q.is_empty() { "Type to search".into() } else { format!("Find “{}” — {} match{}", q, count, if count == 1 { "" } else { "es" }) },
                    detail: "Enter jumps to the next match; F3 / Shift+F3 continue".into(),
                    key: String::new(),
                    action: PaletteAction::Cmd(Cmd::FindNext),
                }]
            }
            '?' => {
                let topics: &[(&str, &str)] = &[
                    ("Keys: the palette shows every command's key", "palette"),
                    ("Reveal Codes: Alt+F3 — see and delete formatting codes", "reveal"),
                    ("Selecting: Shift+arrows, or Alt+F4 then arrows (classic)", "select"),
                    ("Prefixes here: @ heading, # page, / find, > command", "prefix"),
                    ("Switch keyboards: Ctrl+K → Keyboard: Classic / Modern", "keyboard"),
                    ("Config file: ~/.config/wp/config.toml (rebind any key)", "config"),
                ];
                topics
                    .iter()
                    .filter(|(t, _)| palette::score(q, t).is_some())
                    .map(|(t, _k)| PaletteRow { label: t.to_string(), detail: String::new(), key: String::new(), action: PaletteAction::Help })
                    .collect()
            }
            _ => {
                let mut rows: Vec<(i32, PaletteRow)> = Vec::new();
                for c in COMMANDS.iter().filter(|c| c.listed) {
                    let hay = format!("{} {} {}", c.title, c.category, c.aliases);
                    if let Some(s) = palette::score(q, &hay) {
                        let title_bonus = palette::score(q, c.title).unwrap_or(-50);
                        let key = self.keymap.label_for(c.id).unwrap_or_default();
                        rows.push((s + title_bonus, PaletteRow { label: c.title.to_string(), detail: c.category.to_string(), key, action: PaletteAction::Cmd(c.id) }));
                    }
                }
                if q.is_empty() {
                    rows.sort_by(|a, b| a.1.detail.cmp(&b.1.detail));
                } else {
                    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
                }
                rows.into_iter().map(|r| r.1).collect()
            }
        }
    }

    fn handle_overlay_key(&mut self, overlay: Overlay, ev: KeyEvent) {
        let key = Key::from_event(&ev);
        match overlay {
            Overlay::None => {}
            Overlay::Palette { mut input, mut selected } => match ev.code {
                KeyCode::Esc => {
                    if input.starts_with('/') {
                        self.ed.cursor = self.find.origin;
                        self.ed.anchor = self.find.origin_anchor;
                        self.sticky_status = None;
                    }
                }
                KeyCode::Enter => {
                    let rows = self.palette_rows(&input);
                    if let Some(row) = rows.get(selected.min(rows.len().saturating_sub(1))) {
                        match row.action.clone() {
                            PaletteAction::Cmd(c) => {
                                if input.starts_with('/') {
                                    self.find.query = input[1..].trim().to_string();
                                    self.sticky_status = None;
                                    self.find_step(false);
                                } else {
                                    self.exec(c);
                                }
                            }
                            PaletteAction::GoTo(p) => self.ed.move_to(p, false),
                            PaletteAction::Page(n) => {
                                if let Some(p) = self.ed.page_start_pos(n) {
                                    self.ed.move_to(p, false);
                                }
                            }
                            PaletteAction::Help => self.overlay = Overlay::Help,
                        }
                    }
                }
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    self.overlay = Overlay::Palette { input, selected };
                }
                KeyCode::Down => {
                    selected += 1;
                    self.overlay = Overlay::Palette { input, selected };
                }
                KeyCode::Backspace => {
                    input.pop();
                    if input.starts_with('/') {
                        let q = input[1..].to_string();
                        self.incremental_find(&q, false);
                    }
                    self.overlay = Overlay::Palette { input, selected: 0 };
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt && !key.sup => {
                    if input.is_empty() && c == '/' {
                        self.find.origin = self.ed.cursor;
                        self.find.origin_anchor = self.ed.anchor;
                    }
                    input.push(c);
                    if input.starts_with('/') {
                        let q = input[1..].to_string();
                        self.incremental_find(&q, false);
                    }
                    self.overlay = Overlay::Palette { input, selected: 0 };
                }
                KeyCode::Char('k') if key.ctrl => {}
                _ => self.overlay = Overlay::Palette { input, selected },
            },
            Overlay::Prompt { kind, label, mut input } => match ev.code {
                KeyCode::Esc => {
                    if let PromptKind::Find { .. } = kind {
                        self.ed.cursor = self.find.origin;
                        self.ed.anchor = self.find.origin_anchor;
                        self.sticky_status = None;
                    }
                }
                KeyCode::Enter => self.finish_prompt(kind, input),
                KeyCode::Backspace => {
                    input.pop();
                    if let PromptKind::Find { backward } = kind {
                        self.incremental_find(&input, backward);
                    }
                    self.overlay = Overlay::Prompt { kind, label, input };
                }
                KeyCode::Char('u') if key.ctrl => {
                    input.clear();
                    self.overlay = Overlay::Prompt { kind, label, input };
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt && !key.sup => {
                    input.push(c);
                    if let PromptKind::Find { backward } = kind {
                        self.incremental_find(&input, backward);
                    }
                    self.overlay = Overlay::Prompt { kind, label, input };
                }
                KeyCode::Tab => {
                    if matches!(kind, PromptKind::Open | PromptKind::SaveAs(_)) {
                        input = complete_path(&input);
                    }
                    self.overlay = Overlay::Prompt { kind, label, input };
                }
                _ => self.overlay = Overlay::Prompt { kind, label, input },
            },
            Overlay::List { title, items, mut selected, action, mut filter } => {
                let visible: Vec<usize> = items.iter().enumerate().filter(|(_, it)| palette::score(&filter, &format!("{} {}", it.label, it.detail)).is_some()).map(|(i, _)| i).collect();
                match ev.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        let custom = action == ListAction::FontColor && filter.starts_with('#');
                        if custom {
                            self.pick_list(action, &ListItem { label: filter.clone(), detail: String::new(), value: filter.trim_start_matches('#').to_string() });
                        } else if let Some(&i) = visible.get(selected.min(visible.len().saturating_sub(1))) {
                            let item = items[i].clone();
                            self.pick_list(action, &item);
                        }
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Overlay::List { title, items, selected, action, filter };
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(visible.len().saturating_sub(1));
                        self.overlay = Overlay::List { title, items, selected, action, filter };
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        self.overlay = Overlay::List { title, items, selected: 0, action, filter };
                    }
                    KeyCode::Char(c) if !key.ctrl && !key.alt && !key.sup => {
                        filter.push(c);
                        self.overlay = Overlay::List { title, items, selected: 0, action, filter };
                    }
                    _ => self.overlay = Overlay::List { title, items, selected, action, filter },
                }
            }
            Overlay::Confirm { question, action } => match ev.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm(action, true),
                KeyCode::Char('n') | KeyCode::Char('N') => self.confirm(action, false),
                KeyCode::Esc => {}
                _ => self.overlay = Overlay::Confirm { question, action },
            },
            Overlay::Help | Overlay::Message { .. } => {}
        }
    }

    fn pick_list(&mut self, action: ListAction, item: &ListItem) {
        match action {
            ListAction::ApplyStyle => {
                let id = item.value.clone();
                self.ed.set_style(&id);
            }
            ListAction::PasteRing => {
                if let Ok(i) = item.value.parse::<usize>() {
                    if let Some(f) = self.ed.cut_ring().get(i).cloned() {
                        if self.guard_edit() {
                            self.ed.paste(&f);
                            self.clipboard = Some(f);
                        }
                    }
                }
            }
            ListAction::GoToHeading => {
                if let Ok(pi) = item.value.parse::<usize>() {
                    self.ed.move_to(Pos::new(pi, 0), false);
                }
            }
            ListAction::GoToBookmark => {
                if let Some((a, b)) = item.value.split_once(':') {
                    if let (Ok(p), Ok(i)) = (a.parse(), b.parse()) {
                        self.ed.move_to(Pos::new(p, i), false);
                    }
                }
            }
            ListAction::FontColor => {
                if !self.guard_edit() {
                    return;
                }
                if item.value.is_empty() {
                    self.ed.set_attr(AttrKind::Color, None);
                } else if item.value == "custom" {
                    self.message("Type #RRGGBB in the filter box and press Enter");
                    self.exec(Cmd::FontColor);
                } else if let Some(c) = Rgb::parse_hex(&item.value) {
                    self.ed.set_attr(AttrKind::Color, Some(Attr::Color(c)));
                } else {
                    self.message("Not a color: use #RRGGBB");
                }
            }
            ListAction::HighlightColor => {
                if !self.guard_edit() {
                    return;
                }
                if item.value == "none" {
                    self.ed.set_attr(AttrKind::Highlight, None);
                } else if let Some(h) = Highlight::from_docx(&item.value) {
                    self.ed.set_attr(AttrKind::Highlight, Some(Attr::Highlight(h)));
                }
            }
            ListAction::ListFormat => {
                if let Some(kind) = item.value.parse::<usize>().ok().and_then(|i| ListKind::all().get(i).copied()) {
                    let num_id = self.ed.doc.numbering.add_list(kind);
                    self.ed.dirty = true;
                    self.apply_list(num_id, None);
                }
            }

        }
    }

    fn confirm(&mut self, action: ConfirmAction, yes: bool) {
        match action {
            ConfirmAction::ExitSave => {
                if yes {
                    match self.path.clone() {
                        Some(p) => {
                            let f = self.format;
                            match self.save_to(&p, f) {
                                Ok(()) => self.quit = true,
                                Err(e) => self.message(format!("Save failed: {}", e)),
                            }
                        }
                        None => {
                            self.prompt(PromptKind::SaveAs(Format::Docx), "Save as (.docx or .txt), then exit: ", "");
                            self.quit_after_save = true;
                        }
                    }
                } else {
                    self.remove_recovery();
                    self.quit = true;
                }
            }
            ConfirmAction::NewDiscard => {
                if yes {
                    self.remove_recovery();
                    self.new_document();
                }
            }
            ConfirmAction::OpenDiscard(p) => {
                if yes {
                    self.remove_recovery();
                    if let Err(e) = self.open_path(&p) {
                        self.message(format!("Could not open {}: {}", p.display(), e));
                    }
                }
            }
            ConfirmAction::Recover(p) => {
                if yes {
                    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                    let keep_path = self.path.clone();
                    let keep_format = self.format;
                    match if ext == "docx" {
                        wp_docx::read(&p).map(|l| {
                            self.ed.replace_document(l.doc);
                            self.package = Some(l.package);
                        })
                    } else {
                        std::fs::read(&p).map(|b| self.ed.replace_document(wp_core::text::from_text(&decode_text(&b), false))).map_err(|e| e.into())
                    } {
                        Ok(()) => {
                            self.path = keep_path;
                            self.format = keep_format;
                            self.ed.dirty = true;
                            self.ed.set_cols(self.doc_cols());
                            self.message("Recovered unsaved changes — save to keep them");
                        }
                        Err(e) => self.message(format!("Recovery failed: {}", e)),
                    }
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    fn finish_prompt(&mut self, kind: PromptKind, input: String) {
        let v = input.trim().to_string();
        match kind {
            PromptKind::Open => {
                if v.is_empty() {
                    return;
                }
                let p = expand_path(&v);
                if self.ed.dirty {
                    self.overlay = Overlay::Confirm { question: "Discard unsaved changes and open another file? (y/n)".into(), action: ConfirmAction::OpenDiscard(p) };
                } else if let Err(e) = self.open_path(&p) {
                    self.message(format!("Could not open {}: {}", p.display(), e));
                } else {
                    self.check_recovery();
                }
            }
            PromptKind::SaveAs(fmt) => {
                if v.is_empty() {
                    return;
                }
                let mut p = expand_path(&v);
                let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
                let fmt = match ext.as_deref() {
                    Some("txt") | Some("text") | Some("md") => Format::Text,
                    Some("docx") => Format::Docx,
                    _ => {
                        p.set_extension(if fmt == Format::Text { "txt" } else { "docx" });
                        fmt
                    }
                };
                if fmt == Format::Text && self.format == Format::Docx && self.package.is_some() {
                    self.message("Saved as plain text — formatting, styles, and page setup were dropped from the .txt copy.");
                }
                match self.save_to(&p, fmt) {
                    Ok(()) => {
                        if self.quit_after_save {
                            self.quit = true;
                        }
                    }
                    Err(e) => self.message(format!("Save failed: {}", e)),
                }
                self.quit_after_save = false;
            }
            PromptKind::Find { backward } => {
                self.find.query = v.clone();
                self.find.backward = backward;
                self.sticky_status = None;
                if self.ed.selection().is_none() {
                    self.find_step(false);
                }
            }
            PromptKind::ReplaceFind => {
                if v.is_empty() {
                    return;
                }
                self.find.query = v.clone();
                self.prompt(PromptKind::ReplaceWith { find: v }, "Replace with: ", "");
            }
            PromptKind::ReplaceWith { find } => {
                let n = self.count_matches(&find);
                if n == 0 {
                    self.message(format!("Not found: {}", find));
                } else {
                    self.replace_all(&find, &v);
                }
            }
            PromptKind::GoToPage => match v.parse::<usize>() {
                Ok(n) => match self.ed.page_start_pos(n) {
                    Some(p) => self.ed.move_to(p, false),
                    None => self.message(format!("No page {}", n)),
                },
                Err(_) => self.message("Enter a page number"),
            },
            PromptKind::Bookmark => {
                if v.is_empty() || !self.guard_edit() {
                    return;
                }
                let name: String = v.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
                match self.ed.selection() {
                    Some(r) if r.start.para == r.end.para => {
                        self.ed.commit();
                        let c = self.ed.cursor;
                        self.ed.anchor = None;
                        self.ed.cursor = r.end;
                        self.ed.insert_code(Code::BookmarkEnd(name.clone()));
                        self.ed.cursor = r.start;
                        self.ed.insert_code(Code::Bookmark(name.clone()));
                        self.ed.cursor = if c > r.start { Pos::new(c.para, c.idx + 1) } else { c };
                        self.ed.commit();
                    }
                    _ => {
                        self.ed.commit();
                        self.ed.insert_code(Code::Bookmark(name.clone()));
                        self.ed.insert_code(Code::BookmarkEnd(name.clone()));
                        self.ed.commit();
                    }
                }
                self.message(format!("Bookmark “{}” set — Ctrl+K → Go to Bookmark", name));
            }
            PromptKind::FontName => {
                if !self.guard_edit() {
                    return;
                }
                if v.is_empty() {
                    self.ed.set_attr(AttrKind::Font, None);
                } else {
                    self.ed.set_attr(AttrKind::Font, Some(Attr::Font(v)));
                }
            }
            PromptKind::FontSize => {
                if !self.guard_edit() {
                    return;
                }
                match v.parse::<f32>() {
                    Ok(pt) if pt >= 1.0 && pt <= 400.0 => self.ed.set_attr(AttrKind::Size, Some(Attr::Size((pt * 2.0).round() as u16))),
                    _ => self.message("Enter a size in points, e.g. 12"),
                }
            }
            PromptKind::SpaceBefore => match parse_points(&v) {
                Some(t) => self.para(move |p| p.space_before = Some(t)),
                None => self.message("Enter points, e.g. 6"),
            },
            PromptKind::SpaceAfter => match parse_points(&v) {
                Some(t) => self.para(move |p| p.space_after = Some(t)),
                None => self.message("Enter points, e.g. 8"),
            },
            PromptKind::FirstLine => match parse_inches(&v) {
                Some(t) => self.para(move |p| {
                    p.first_line = Some(t);
                    p.hanging = None;
                }),
                None => self.message("Enter inches, e.g. 0.5"),
            },
            PromptKind::Margins => {
                let parts: Vec<Option<Twips>> = v.split_whitespace().map(parse_inches).collect();
                if parts.len() == 4 && parts.iter().all(|p| p.is_some()) {
                    let m: Vec<Twips> = parts.into_iter().map(|p| p.unwrap()).collect();
                    self.section(move |s| {
                        s.margin_top = m[0];
                        s.margin_bottom = m[1];
                        s.margin_left = m[2];
                        s.margin_right = m[3];
                    });
                } else {
                    self.message("Enter four values in inches: top bottom left right");
                }
            }
            PromptKind::TabSet => {
                let mut tabs = Vec::new();
                for tok in v.split_whitespace() {
                    let num: String = tok.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    let rest = &tok[num.len()..];
                    let Some(pos) = parse_inches(&num) else {
                        self.message(format!("Not a tab position: {}", tok));
                        return;
                    };
                    let kind = if rest.contains('r') {
                        TabKind::Right
                    } else if rest.contains('c') {
                        TabKind::Center
                    } else if rest.contains('d') {
                        TabKind::Decimal
                    } else {
                        TabKind::Left
                    };
                    let leader = if rest.contains('.') {
                        TabLeader::Dot
                    } else if rest.contains('-') {
                        TabLeader::Hyphen
                    } else if rest.contains('_') {
                        TabLeader::Underscore
                    } else {
                        TabLeader::None
                    };
                    tabs.push(TabStop { pos, kind, leader, clear: false });
                }
                self.para(move |p| p.tabs = tabs.clone());
            }
        }
    }
}

fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let be = bytes[0] == 0xFE;
        let u16s: Vec<u16> = bytes[2..].chunks(2).filter(|c| c.len() == 2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16_lossy(&u16s);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(), // Latin-1
    }
}

pub fn expand_path(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join(rest);
        }
    }
    PathBuf::from(s)
}

fn complete_path(input: &str) -> String {
    let p = expand_path(input);
    let (dir, prefix) = if input.ends_with('/') {
        (p.clone(), String::new())
    } else {
        (p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")), p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
    };
    let dir_read = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir.clone() };
    let Ok(rd) = std::fs::read_dir(&dir_read) else { return input.to_string() };
    let mut matches: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && !name.starts_with('.') {
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                Some(if is_dir { format!("{}/", name) } else { name })
            } else {
                None
            }
        })
        .collect();
    matches.sort();
    if matches.is_empty() {
        return input.to_string();
    }
    // Longest common prefix.
    let mut lcp = matches[0].clone();
    for m in &matches[1..] {
        while !m.starts_with(&lcp) {
            lcp.pop();
        }
    }
    let base = if input.ends_with('/') { input.to_string() } else { input[..input.len() - prefix.len()].to_string() };
    format!("{}{}", base, lcp)
}

fn today() -> String {
    // Civil date from the system clock without a date crate.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    format!("{} {}, {}", months[(m - 1) as usize], d, y)
}

pub fn inches(t: Twips) -> String {
    let s = format!("{:.2}", t as f64 / 1440.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn points(t: Twips) -> String {
    let s = format!("{:.1}", t as f64 / 20.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn parse_inches(s: &str) -> Option<Twips> {
    let s = s.trim().trim_end_matches('"');
    s.parse::<f64>().ok().map(|v| (v * 1440.0).round() as Twips)
}

fn parse_points(s: &str) -> Option<Twips> {
    let s = s.trim().trim_end_matches("pt");
    s.parse::<f64>().ok().map(|v| (v * 20.0).round() as Twips)
}

pub fn cmd_title(c: Cmd) -> &'static str {
    info(c).title
}
