//! Application state and command execution.

use crate::commands::{info, Cmd, COMMANDS};
use crate::config::{state_dir, Config, KeymapChoice, ThemeChoice, WrapChoice};
use crate::google::{self, DriveEntry, DriveKind, DriveQuery};
use crate::keymap::{Key, Keymap};
use crate::palette;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use wp_core::model::*;
use wp_core::reveal::{self, ParaCode};
use wp_core::search::{self, Match, Query};
use wp_core::{Document, Editor, Fragment, ListKind};
use wp_docx::{DocxPackage, Warning};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    Text,
    Markdown,
    /// A Google Doc opened through the Docs API; saves are `batchUpdate`
    /// diffs against `App::gdoc` (DESIGN.md §6a).
    GoogleDoc,
}

/// The Google Doc the editor holds, and the baseline its edits are diffed against.
pub struct GdocState {
    pub id: String,
    pub title: String,
    pub baseline: wp_gdoc::Baseline,
}

/// A network action queued to run after the next draw, so what the screen
/// says while it blocks (a sign-in URL, "Contacting Google…") is visible.
pub enum Pending {
    Open { id: String, force: bool },
    Save,
    /// Show the Open from Drive dialog (after signing in).
    Drive,
    SignIn { flow: google::SignIn, then: Box<Pending> },
}

/// A Drive listing that came back from a worker thread.
pub type DriveReply = (u64, DriveQuery, Result<Vec<DriveEntry>, String>);

/// How long typing must pause before the dialog asks Drive to search.
pub const DRIVE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Draft,
    Page,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    SaveAs(Format),
    Find { backward: bool },
    FindStyle,
    FindCode,
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
    TableInsert,
    ColumnWidth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    ExitSave,
    NewDiscard,
    OpenDiscard(PathBuf),
    OpenDriveDiscard(String),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveMode {
    /// Google Docs by recency, filtered as you type; a pause asks Drive to
    /// search by name too.
    Recent,
    /// My Drive / Shared with me / Shared drives as a directory tree.
    Folders,
}

/// One step of the folder view's breadcrumb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveFolder {
    pub name: String,
    pub query: DriveQuery,
}

/// The Open from Drive dialog. The rows shown are the listing for the
/// current place (recents, or a folder's contents) narrowed by `filter`,
/// then — in Recent mode — whatever a server-side name search added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveDialog {
    pub mode: DriveMode,
    /// Folder mode's breadcrumb; `path[0]` is the roots listing.
    pub path: Vec<DriveFolder>,
    pub filter: String,
    pub rows: Vec<DriveEntry>,
    /// Search hits that aren't already in `rows`.
    pub extra: Vec<DriveEntry>,
    pub selected: usize,
    /// The listing for `rows` is in flight.
    pub loading: bool,
    /// A name search is in flight.
    pub searching: bool,
    /// Why the listing failed, shown in place of the rows.
    pub error: Option<String>,
}

impl DriveDialog {
    fn new() -> DriveDialog {
        DriveDialog { mode: DriveMode::Recent, path: vec![DriveFolder { name: "Drive".into(), query: DriveQuery::Roots }], filter: String::new(), rows: Vec::new(), extra: Vec::new(), selected: 0, loading: false, searching: false, error: None }
    }

    /// The listing this view shows.
    pub fn query(&self) -> DriveQuery {
        match self.mode {
            DriveMode::Recent => DriveQuery::Recent,
            DriveMode::Folders => self.path.last().map(|f| f.query.clone()).unwrap_or(DriveQuery::Roots),
        }
    }

    /// The rows for the filter: local matches, then the search's extras
    /// (flagged true).
    pub fn visible(&self) -> Vec<(&DriveEntry, bool)> {
        let mut v: Vec<(&DriveEntry, bool)> = self.rows.iter().filter(|e| palette::score(&self.filter, &e.name).is_some()).map(|e| (e, false)).collect();
        v.extend(self.extra.iter().map(|e| (e, true)));
        v
    }

    pub fn title(&self) -> String {
        let mut t = match self.mode {
            DriveMode::Recent => "Google Drive — Recent".to_string(),
            DriveMode::Folders => format!("Google Drive — {}", self.path.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(" / ")),
        };
        if self.loading {
            t.push_str(if self.rows.is_empty() { " · loading…" } else { " · updating…" });
        } else if self.searching {
            t.push_str(" · searching…");
        }
        t
    }
}

/// One row in the Open dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    /// A format wp opens as a document; anything else is read as plain text.
    pub is_doc: bool,
    /// Size and modified date, shown greyed to the right.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette { input: String, selected: usize },
    Prompt { kind: PromptKind, label: String, input: String },
    List { title: String, items: Vec<ListItem>, selected: usize, action: ListAction, filter: String },
    /// The Open dialog: a browsable listing of `dir`. `filter` narrows the rows
    /// as you type; typing a `/` navigates instead. `all` shows every file, not
    /// just the ones wp opens as documents.
    Browse { dir: PathBuf, entries: Vec<FileEntry>, selected: usize, filter: String, all: bool },
    /// The Open from Google Drive dialog (DESIGN.md §6a.4).
    Drive(DriveDialog),
    Confirm { question: String, action: ConfirmAction },
    Help,
    Message { title: String, lines: Vec<String> },
    /// A pull-down menu is open: `menu` on the bar, `item` highlighted.
    Menu { menu: usize, item: usize },
    /// Every match listed before a replace-all; ↑↓ previews each in place.
    ReplacePreview { find: String, with: String, matches: Vec<Match>, selected: usize },
    /// One-at-a-time replacement: the current match is selected in the document.
    ReplaceStep { find: String, with: String, done: usize, total: usize },
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
    /// Options toggled in the find prompt (Alt+R / Alt+C / Alt+W) or from the palette.
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl FindState {
    /// The query for a search-box string, with the toggled options applied.
    pub fn build(&self, text: &str) -> Query {
        let mut q = Query::parse(text);
        q.regex |= self.regex;
        q.whole_word |= self.whole_word;
        if self.case_sensitive {
            q.case_sensitive = Some(true);
        }
        q
    }
    pub fn flags(&self) -> String {
        let mut f = Vec::new();
        if self.regex {
            f.push("re");
        }
        if self.case_sensitive {
            f.push("Aa");
        }
        if self.whole_word {
            f.push("W");
        }
        if f.is_empty() {
            String::new()
        } else {
            format!(" [{}]", f.join("·"))
        }
    }
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
    /// Text to hand to the terminal's clipboard (OSC 52) after this event.
    pub clipboard_out: Option<String>,
    /// Mouse state: a button is held (dragging selects), and the last click
    /// for double-click detection.
    mouse_down: bool,
    last_click: Option<(Pos, Instant)>,
    pub gdoc: Option<GdocState>,
    google: Option<google::Client>,
    pub pending: Option<Pending>,
    /// Drive listings come back from worker threads on this channel.
    drive_tx: mpsc::Sender<DriveReply>,
    drive_rx: mpsc::Receiver<DriveReply>,
    /// Sequence numbers: the next to hand out, and the ones whose replies
    /// the dialog is still waiting for (anything else is stale).
    drive_seq: u64,
    pub(crate) drive_list_seq: u64,
    pub(crate) drive_search_seq: u64,
    /// When the paused-typing search fires.
    pub(crate) drive_search_due: Option<Instant>,
    /// The words the last search asked for, so a pause doesn't repeat it.
    drive_search_sent: String,
    /// Listings fetched this session; Recent is also kept on disk so the
    /// dialog opens with rows before Drive answers.
    drive_cache: HashMap<DriveQuery, Vec<DriveEntry>>,
    pub drive_cache_path: Option<PathBuf>,
}

const UNTITLED: &str = "Untitled";

impl App {
    pub fn new(cfg: Config) -> App {
        let keymap = Keymap::build(&cfg);
        let hint = cfg.show_hint;
        let (drive_tx, drive_rx) = mpsc::channel();
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
            clipboard_out: None,
            mouse_down: false,
            last_click: None,
            gdoc: None,
            google: None,
            pending: None,
            drive_tx,
            drive_rx,
            drive_seq: 0,
            drive_list_seq: 0,
            drive_search_seq: 0,
            drive_search_due: None,
            drive_search_sent: String::new(),
            drive_cache: HashMap::new(),
            drive_cache_path: Some(state_dir().join("drive-recent.json")),
        }
    }

    /// Mouse: click places the cursor, drag selects, double-click selects a
    /// word, the wheel scrolls; lists and the palette follow the wheel.
    pub fn handle_mouse(&mut self, ev: MouseEvent) {
        if !self.cfg.mouse {
            return;
        }
        match &self.overlay {
            Overlay::None => {
                // A click on the pinned menu bar opens that menu.
                if self.cfg.menu_bar && ev.row == 0 && matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
                    if let Some(m) = crate::menu::menu_at(ev.column) {
                        self.needs_redraw = true;
                        self.overlay = Overlay::Menu { menu: m, item: crate::menu::first_item(m) };
                    }
                    return;
                }
            }
            Overlay::Menu { menu, item } => {
                // Taken out first, like keys: a handler that runs a command
                // leaves the menu closed unless it puts it back.
                let (menu, item) = (*menu, *item);
                self.overlay = Overlay::None;
                self.menu_mouse(menu, item, ev);
                return;
            }
            Overlay::Palette { .. } | Overlay::List { .. } | Overlay::Drive(_) | Overlay::ReplacePreview { .. } => {
                let code = match ev.kind {
                    MouseEventKind::ScrollUp => Some(KeyCode::Up),
                    MouseEventKind::ScrollDown => Some(KeyCode::Down),
                    _ => None,
                };
                if let Some(c) = code {
                    self.handle_key(KeyEvent::new(c, crossterm::event::KeyModifiers::NONE));
                }
                return;
            }
            _ => return,
        }
        self.needs_redraw = true;
        match ev.kind {
            MouseEventKind::ScrollUp => self.ed.move_lines(-3, false),
            MouseEventKind::ScrollDown => self.ed.move_lines(3, false),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(p) = crate::ui::pos_at(self, ev.column, ev.row) else { return };
                self.mouse_down = true;
                self.reveal_para_code = None;
                let double = self.last_click.map_or(false, |(lp, t)| lp == p && t.elapsed() < Duration::from_millis(450));
                self.last_click = Some((p, Instant::now()));
                if double {
                    self.ed.move_to(p, false);
                    self.ed.word_left(false);
                    self.ed.word_right(true);
                    self.block_mode = self.ed.has_selection();
                } else {
                    let shift = ev.modifiers.contains(crossterm::event::KeyModifiers::SHIFT);
                    self.ed.move_to(p, shift);
                    if !shift {
                        self.block_mode = false;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.mouse_down {
                    if let Some(p) = crate::ui::pos_at(self, ev.column, ev.row) {
                        self.ed.move_to(p, true);
                        self.block_mode = self.ed.has_selection();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.mouse_down = false,
            _ => {}
        }
    }

    /// Bracketed paste from the terminal: typed into the document (or into
    /// a prompt), never interpreted as keys.
    pub fn paste_text(&mut self, s: &str) {
        self.needs_redraw = true;
        match &mut self.overlay {
            Overlay::None => {
                if self.guard_edit() {
                    let s = s.replace("\r\n", "\n").replace('\r', "\n");
                    self.ed.insert_str(&s);
                    self.block_mode = false;
                }
            }
            Overlay::Prompt { input, .. } => input.push_str(s.lines().next().unwrap_or("")),
            Overlay::Palette { input, .. } => input.push_str(s.lines().next().unwrap_or("")),
            Overlay::List { filter, .. } => filter.push_str(s.lines().next().unwrap_or("")),
            Overlay::Browse { filter, .. } => filter.push_str(s.lines().next().unwrap_or("")),
            Overlay::Drive(d) => {
                d.filter.push_str(s.lines().next().unwrap_or(""));
                d.selected = 0;
                self.drive_filter_changed();
            }
            _ => {}
        }
        self.browse_retarget();
    }

    pub fn title(&self) -> String {
        if let Some(g) = &self.gdoc {
            return format!("{} (Google Docs)", g.title);
        }
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
        } else if ext == "md" || ext == "markdown" {
            let bytes = std::fs::read(path)?;
            let text = decode_text(&bytes);
            self.ed.replace_document(wp_md::from_markdown(&text));
            self.package = None;
            self.format = Format::Markdown;
            self.warnings.clear();
            self.sticky_status = None;
        } else {
            let bytes = std::fs::read(path)?;
            let text = decode_text(&bytes);
            self.ed.replace_document(wp_core::text::from_text(&text, false));
            self.package = None;
            self.format = Format::Text;
            self.warnings.clear();
        }
        self.path = Some(path.to_path_buf());
        self.gdoc = None;
        self.scroll = (0, 0);
        self.reveal_para_code = None;
        self.needs_redraw = true;
        self.sync_editor_layout();
        Ok(())
    }

    /// Show the Open dialog listing `dir`. Keeps the current overlay (with a
    /// message) if the directory can't be read.
    pub fn browse(&mut self, dir: &Path, all: bool) {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        match read_entries(&dir) {
            Ok(entries) => self.overlay = Overlay::Browse { dir, entries, selected: 0, filter: String::new(), all },
            Err(e) => self.message(format!("Could not read {}: {}", dir.display(), e)),
        }
        self.needs_redraw = true;
    }

    /// If the Open dialog's filter names a directory in everything up to its
    /// last `/`, move there and keep the rest as the filter. False when no such
    /// directory exists, leaving the overlay untouched.
    fn browse_retarget(&mut self) -> bool {
        let Overlay::Browse { dir, filter, all, .. } = &self.overlay else { return false };
        let Some(i) = filter.rfind('/') else { return false };
        let (head, tail) = (filter[..i + 1].to_string(), filter[i + 1..].to_string());
        let target = if head.starts_with('/') || head.starts_with('~') { expand_path(&head) } else { dir.join(&head) };
        if !target.is_dir() {
            return false;
        }
        let all = *all;
        self.browse(&target, all);
        if let Overlay::Browse { filter, .. } = &mut self.overlay {
            *filter = tail;
        }
        true
    }

    /// Open `path`, confirming first if the current document has unsaved edits.
    /// On failure the Open dialog stays up so another file can be picked.
    fn open_file(&mut self, path: &Path, fallback: Option<(&Path, bool)>) {
        if self.ed.dirty {
            self.overlay = Overlay::Confirm { question: "Discard unsaved changes and open another file? (y/n)".into(), action: ConfirmAction::OpenDiscard(path.to_path_buf()) };
        } else if let Err(e) = self.open_path(path) {
            self.message(format!("Could not open {}: {}", path.display(), e));
            if let Some((dir, all)) = fallback {
                self.browse(dir, all);
            }
        } else {
            self.check_recovery();
        }
    }

    pub fn save_to(&mut self, path: &Path, format: Format) -> anyhow::Result<()> {
        self.ed.commit();
        if format == Format::GoogleDoc {
            self.queue(Pending::Save);
            return Ok(());
        }
        // Leaving Google Docs for a file: what only Docs can hold goes.
        let mut dropped = Vec::new();
        if self.gdoc.is_some() {
            dropped = wp_gdoc::detach(&mut self.ed.doc);
        }
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
            Format::GoogleDoc => unreachable!("handled above"),
            Format::Markdown => {
                let export = self.markdown_export();
                std::fs::write(path, &export.text)?;
                if let Some(w) = export.warning() {
                    self.path = Some(path.to_path_buf());
                    self.format = format;
                    self.ed.dirty = false;
                    self.remove_recovery();
                    self.message(w);
                    return Ok(());
                }
            }
        }
        self.remove_recovery();
        self.path = Some(path.to_path_buf());
        self.format = format;
        self.gdoc = None;
        self.ed.dirty = false;
        if dropped.is_empty() {
            self.message(format!("Saved {}", path.display()));
        } else {
            self.message(format!("Saved {} — not carried over from Google Docs: {}", path.display(), dropped.join(", ")));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Google Docs (DESIGN.md §6a)
    // ------------------------------------------------------------------

    /// Run `p` after the next draw.
    pub fn queue(&mut self, p: Pending) {
        if !matches!(p, Pending::SignIn { .. } | Pending::Drive) {
            self.message("Contacting Google…");
        }
        self.pending = Some(p);
        self.needs_redraw = true;
    }

    /// The queued network action. Signs in first when there is no token,
    /// showing the URL while it waits for the browser to come back.
    pub fn run_pending(&mut self, p: Pending) {
        if !self.ensure_google() {
            return;
        }
        if let Pending::SignIn { flow, then } = p {
            let res = self.google.as_mut().unwrap().finish_sign_in(flow, cancel_pressed);
            self.overlay = Overlay::None;
            self.needs_redraw = true;
            match res {
                Ok(()) => self.run_pending(*then),
                Err(e) => self.message(format!("Google sign-in failed: {}", e)),
            }
            return;
        }
        if !self.google.as_ref().unwrap().signed_in() {
            match self.google.as_ref().unwrap().begin_sign_in() {
                Ok(flow) => {
                    let opened = google::open_in_browser(&flow.url);
                    let mut lines = vec![if opened { "A browser window has been opened for you to sign in." } else { "Open this address in a browser to sign in:" }.to_string(), String::new()];
                    let url = flow.url.clone();
                    let mut rest = url.as_str();
                    while !rest.is_empty() {
                        let n = rest.char_indices().nth(72).map(|(i, _)| i).unwrap_or(rest.len());
                        lines.push(rest[..n].to_string());
                        rest = &rest[n..];
                    }
                    lines.push(String::new());
                    lines.push("Waiting for the sign-in to complete… Esc cancels.".into());
                    self.overlay = Overlay::Message { title: "Sign in to Google".into(), lines };
                    self.pending = Some(Pending::SignIn { flow, then: Box::new(p) });
                    self.needs_redraw = true;
                }
                Err(e) => self.message(format!("Google sign-in failed: {}", e)),
            }
            return;
        }
        match p {
            Pending::Open { id, force } => self.open_gdoc(&id, force),
            Pending::Save => self.save_gdoc(),
            Pending::Drive => self.open_drive(),
            Pending::SignIn { .. } => {}
        }
    }

    /// The client, created from config on first use. False (with a message)
    /// when there is no OAuth client to create it from.
    fn ensure_google(&mut self) -> bool {
        if self.google.is_none() {
            if !self.cfg.google.is_set() {
                self.message(format!("Google Docs needs an OAuth desktop client: add [google] client_id and client_secret to {}", crate::config::config_path().display()));
                return false;
            }
            self.google = Some(google::Client::new(self.cfg.google.clone()));
        }
        true
    }

    fn open_gdoc(&mut self, id: &str, force: bool) {
        if self.ed.dirty && !force {
            self.overlay = Overlay::Confirm { question: "Discard unsaved changes and open the Google Doc? (y/n)".into(), action: ConfirmAction::OpenDriveDiscard(id.to_string()) };
            self.needs_redraw = true;
            return;
        }
        let json = match self.google.as_mut().unwrap().get_document(id) {
            Ok(j) => j,
            Err(e) => return self.message(format!("Could not open the Google Doc: {}", e)),
        };
        match wp_gdoc::read(&json) {
            Ok(l) => {
                self.load_gdoc(id, l);
                self.check_recovery();
            }
            Err(e) => self.message(format!("Could not read the Google Doc: {}", e)),
        }
    }

    /// Install a document read from Google Docs.
    pub fn load_gdoc(&mut self, id: &str, l: wp_gdoc::Loaded) {
        self.ed.replace_document(l.doc);
        self.package = None;
        self.path = None;
        self.format = Format::GoogleDoc;
        self.warnings.clear();
        self.sticky_status = None;
        let title = l.baseline.title.clone();
        self.gdoc = Some(GdocState { id: id.to_string(), title: title.clone(), baseline: l.baseline });
        self.scroll = (0, 0);
        self.reveal_para_code = None;
        self.sync_editor_layout();
        self.needs_redraw = true;
        if l.warnings.is_empty() {
            self.message(format!("Opened “{}” from Google Docs", title));
        } else {
            self.message(format!("Opened “{}” from Google Docs — {}", title, l.warnings.join("; ")));
        }
    }

    fn save_gdoc(&mut self) {
        self.ed.commit();
        let Some(g) = &self.gdoc else { return };
        let reqs = match wp_gdoc::diff(&g.baseline, &self.ed.doc) {
            Ok(r) => r,
            Err(e) => return self.message(format!("Can't save this edit to Google Docs yet: {} — Save As .docx keeps a copy", e)),
        };
        if reqs.is_empty() {
            self.ed.dirty = false;
            self.remove_recovery();
            self.message("No changes to save");
            return;
        }
        let n = reqs.len();
        let body = wp_gdoc::batch_update(&g.baseline, reqs);
        let id = g.id.clone();
        let client = self.google.as_mut().unwrap();
        match client.batch_update(&id, &body) {
            Ok(_) => {
                // Re-read so the next save diffs against what Google now has.
                match client.get_document(&id).and_then(|j| wp_gdoc::read(&j).map_err(anyhow::Error::msg)) {
                    Ok(l) => self.adopt_baseline(&id, l),
                    Err(e) => self.message(format!("Saved to Google Docs, but could not re-read it ({}); reopen before saving again", e)),
                }
                self.ed.dirty = false;
                self.remove_recovery();
                self.message(format!("Saved to Google Docs ({} change{})", n, if n == 1 { "" } else { "s" }));
                if self.quit_after_save {
                    self.quit = true;
                }
            }
            Err(e) => {
                let conflict = e.downcast_ref::<google::ApiError>().map_or(false, |a| a.is_conflict());
                if conflict {
                    self.message("Not saved: the document changed on Google Docs since you opened it. Save As .docx to keep your version, then reopen it.");
                } else {
                    self.message(format!("Save to Google Docs failed: {}", e));
                }
            }
        }
        self.quit_after_save = false;
    }

    /// After a successful save: keep the edited document (and its undo
    /// history) when the re-read agrees with it in shape, else reload.
    fn adopt_baseline(&mut self, id: &str, l: wp_gdoc::Loaded) {
        let same_shape = self.gdoc.as_ref().map_or(false, |g| g.baseline.lists == l.baseline.lists && g.baseline.footnote_ids == l.baseline.footnote_ids) && l.doc.paragraphs.len() == self.ed.doc.paragraphs.len();
        if same_shape {
            if let Some(g) = &mut self.gdoc {
                g.baseline = l.baseline;
                g.title = g.baseline.title.clone();
            }
        } else {
            let cursor = self.ed.cursor;
            self.load_gdoc(id, l);
            let c = self.ed.doc.clamp(cursor);
            self.ed.move_to(c, false);
        }
    }

    // ------------------------------------------------------------------
    // Open from Drive (DESIGN.md §6a.4)
    // ------------------------------------------------------------------

    /// Show the Drive dialog on Recent, from the cached rows, and ask Drive
    /// for a fresh listing.
    pub fn open_drive(&mut self) {
        if !self.drive_cache.contains_key(&DriveQuery::Recent) {
            if let Some(rows) = self.drive_cache_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<Vec<DriveEntry>>(&s).ok()) {
                self.drive_cache.insert(DriveQuery::Recent, rows);
            }
        }
        self.drive_show(DriveDialog::new(), true);
    }

    /// Put `d` up showing the listing for its current place: from the cache
    /// when there is one (fetched again anyway if `refresh`), otherwise
    /// empty while a worker fetches it.
    fn drive_show(&mut self, mut d: DriveDialog, refresh: bool) {
        let q = d.query();
        d.extra.clear();
        d.searching = false;
        d.error = None;
        d.selected = 0;
        self.drive_search_due = None;
        self.drive_search_sent.clear();
        let cached = if q == DriveQuery::Roots { Some(google::drive_roots()) } else { self.drive_cache.get(&q).cloned() };
        d.loading = false;
        match cached {
            Some(rows) => {
                d.rows = rows;
                if refresh && q != DriveQuery::Roots {
                    d.loading = true;
                    self.drive_list_seq = self.drive_fetch(q);
                }
            }
            None => {
                d.rows.clear();
                d.loading = true;
                self.drive_list_seq = self.drive_fetch(q);
            }
        }
        self.overlay = Overlay::Drive(d);
        self.needs_redraw = true;
    }

    /// Ask a worker thread for a listing; the reply carries the sequence
    /// number returned. Without a client (headless tests) nothing runs and
    /// the test answers through `drive_reply`.
    fn drive_fetch(&mut self, q: DriveQuery) -> u64 {
        self.drive_seq += 1;
        let seq = self.drive_seq;
        if let Some(client) = &self.google {
            let mut client = client.clone();
            let tx = self.drive_tx.clone();
            std::thread::spawn(move || {
                let r = client.list(&q).map_err(|e| e.to_string());
                let _ = tx.send((seq, q, r));
            });
        }
        seq
    }

    /// Descend into a folder-like entry.
    fn drive_enter(&mut self, mut d: DriveDialog, e: &DriveEntry) {
        let Some(q) = google::query_for(e) else { return self.overlay = Overlay::Drive(d) };
        d.mode = DriveMode::Folders;
        d.path.push(DriveFolder { name: e.name.clone(), query: q });
        d.filter.clear();
        self.drive_show(d, false);
    }

    /// Up one level in the folder view; at the top, or in Recent, nothing.
    fn drive_up(&mut self, mut d: DriveDialog) {
        if d.mode == DriveMode::Folders && d.path.len() > 1 {
            d.path.pop();
            d.filter.clear();
            self.drive_show(d, false);
        } else {
            self.overlay = Overlay::Drive(d);
        }
    }

    fn drive_toggle_mode(&mut self, mut d: DriveDialog) {
        d.mode = match d.mode {
            DriveMode::Recent => DriveMode::Folders,
            DriveMode::Folders => DriveMode::Recent,
        };
        d.filter.clear();
        self.drive_show(d, false);
    }

    /// The filter changed: in Recent mode, arm the paused-typing search (or
    /// drop the search when the box is empty again).
    fn drive_filter_changed(&mut self) {
        let Overlay::Drive(d) = &mut self.overlay else { return };
        if d.mode != DriveMode::Recent {
            return;
        }
        if d.filter.trim().is_empty() {
            d.extra.clear();
            d.searching = false;
            self.drive_search_due = None;
            self.drive_search_sent.clear();
            self.drive_search_seq = 0;
        } else {
            self.drive_search_due = Some(Instant::now() + DRIVE_SEARCH_DEBOUNCE);
        }
    }

    /// Something the main loop should poll for quickly: a listing or search
    /// in flight, or a search about to fire.
    pub fn drive_active(&self) -> bool {
        match &self.overlay {
            Overlay::Drive(d) => d.loading || d.searching || self.drive_search_due.is_some(),
            _ => false,
        }
    }

    /// Called every pass of the main loop: take in worker replies and fire
    /// a search once typing has paused.
    pub fn drive_tick(&mut self) {
        self.drive_tick_at(Instant::now());
    }

    pub fn drive_tick_at(&mut self, now: Instant) {
        while let Ok((seq, q, r)) = self.drive_rx.try_recv() {
            self.drive_reply(seq, q, r);
        }
        if self.drive_search_due.map_or(false, |due| now >= due) {
            self.drive_search_due = None;
            let Overlay::Drive(d) = &mut self.overlay else { return };
            let words = d.filter.trim().to_string();
            if d.mode == DriveMode::Recent && !words.is_empty() && google::parse_doc_ref(&words).is_none() && words != self.drive_search_sent {
                d.searching = true;
                self.needs_redraw = true;
                self.drive_search_sent = words.clone();
                self.drive_search_seq = self.drive_fetch(DriveQuery::Search(words));
            }
        }
    }

    /// A listing came back. Successful listings are cached whatever the
    /// dialog is doing now; the dialog itself only takes the reply it is
    /// waiting for.
    pub fn drive_reply(&mut self, seq: u64, q: DriveQuery, r: Result<Vec<DriveEntry>, String>) {
        if let Ok(rows) = &r {
            if !matches!(q, DriveQuery::Search(_)) {
                self.drive_cache.insert(q.clone(), rows.clone());
                if q == DriveQuery::Recent {
                    if let Some(p) = &self.drive_cache_path {
                        if let Some(dir) = p.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(p, serde_json::to_string(rows).unwrap_or_default());
                    }
                }
            }
        }
        let Overlay::Drive(d) = &mut self.overlay else { return };
        if seq == self.drive_list_seq && q == d.query() {
            d.loading = false;
            match r {
                Ok(rows) => d.rows = rows,
                Err(e) => d.error = Some(e),
            }
            self.needs_redraw = true;
        } else if d.searching && seq == self.drive_search_seq && d.mode == DriveMode::Recent {
            d.searching = false;
            match r {
                Ok(rows) => {
                    let local = d.visible().iter().filter(|(_, x)| !*x).map(|(e, _)| e.id.clone()).collect::<Vec<_>>();
                    d.extra = rows.into_iter().filter(|e| !local.contains(&e.id)).collect();
                }
                Err(e) => self.message(format!("Drive search failed: {}", e)),
            }
            self.needs_redraw = true;
        }
    }

    /// The document as Markdown, hyperlink targets resolved through the
    /// package it came from (or the relationships a Markdown import created).
    pub fn markdown_export(&self) -> wp_md::Export {
        let doc = &self.ed.doc;
        let pkg = self.package.as_ref();
        let rels = |id: &str| -> Option<String> {
            doc.extra_rels.iter().find(|r| r.id == id).map(|r| r.target.clone()).or_else(|| pkg.and_then(|p| p.rel_target(id)))
        };
        wp_md::to_markdown(doc, &rels)
    }

    pub fn save(&mut self) {
        if self.format == Format::GoogleDoc && self.gdoc.is_some() {
            self.queue(Pending::Save);
            return;
        }
        match self.path.clone() {
            Some(p) => {
                let f = self.format;
                if let Err(e) = self.save_to(&p, f) {
                    self.message(format!("Save failed: {}", e));
                }
            }
            None => self.prompt(PromptKind::SaveAs(Format::Docx), "Save as (.docx, .md or .txt): ", ""),

        }
    }

    fn recovery_path(&self) -> PathBuf {
        let key = match &self.gdoc {
            Some(g) => format!("gdoc:{}", g.id),
            None => self.path.as_ref().map(|p| p.canonicalize().unwrap_or(p.clone()).to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into()),
        };
        let mut h: u64 = 0xcbf29ce484222325;
        for b in key.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        let ext = match self.format {
            Format::Docx => "docx",
            Format::Text => "txt",
            Format::Markdown => "md",
            Format::GoogleDoc => "gdoc.json",
        };
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
            Format::Markdown => std::fs::write(&p, self.markdown_export().text).map_err(|e| e.to_string()),
            // The model itself, with the baseline, so nothing is lost and a
            // recovered document can still be saved back as a diff.
            Format::GoogleDoc => match &self.gdoc {
                Some(g) => serde_json::to_string(&serde_json::json!({ "id": g.id, "title": g.title, "baseline": g.baseline, "doc": self.ed.doc })).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string())),
                None => Ok(()),
            },
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
        self.sync_editor_layout();
        self.needs_redraw = true;
    }

    /// The menu bar is on screen when pinned in the config or while a menu
    /// is open.
    pub fn menu_bar_visible(&self) -> bool {
        self.cfg.menu_bar || matches!(self.overlay, Overlay::Menu { .. })
    }

    /// The first screen row of the document area.
    pub fn doc_top(&self) -> u16 {
        if self.menu_bar_visible() { 1 } else { 0 }
    }

    pub fn theme(&self) -> ThemeChoice {
        self.cfg.theme
    }

    pub fn doc_rows(&self) -> u16 {
        let mut h = self.size.1.saturating_sub(1 + self.doc_top()); // status line, menu bar
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
                let dir = self.path.as_ref().and_then(|p| p.parent()).filter(|d| !d.as_os_str().is_empty()).map(|d| d.to_path_buf()).unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                self.browse(&dir, false);
            }
            Cmd::Save => self.save(),
            Cmd::SaveAs => {
                let init = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                let f = if self.format == Format::GoogleDoc { Format::Docx } else { self.format };
                self.prompt(PromptKind::SaveAs(f), "Save as: ", &init);
            }
            Cmd::OpenFromDrive => {
                if self.ensure_google() {
                    if self.google.as_ref().unwrap().signed_in() {
                        self.open_drive();
                    } else {
                        self.queue(Pending::Drive);
                    }
                }
            }
            Cmd::GoogleSignOut => {
                if let Some(c) = &mut self.google {
                    c.sign_out();
                } else {
                    google::Client::new(self.cfg.google.clone()).sign_out();
                }
                self.message("Signed out of Google; the next Drive open or save signs in again");
            }
            Cmd::SaveAsDocx => {
                let init = self.path.as_ref().map(|p| p.with_extension("docx").display().to_string()).unwrap_or_default();
                self.prompt(PromptKind::SaveAs(Format::Docx), "Save as .docx: ", &init);
            }
            Cmd::SaveAsMarkdown => {
                let init = self.path.as_ref().map(|p| p.with_extension("md").display().to_string()).unwrap_or_default();
                self.prompt(PromptKind::SaveAs(Format::Markdown), "Save as .md: ", &init);
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
                            if self.cfg.system_clipboard {
                                self.clipboard_out = Some(f.text());
                            }
                            self.clipboard = Some(f);
                            self.block_mode = false;
                        }
                        None => self.message("Nothing selected — Shift+arrows or Alt+F4 to select"),
                    }
                }
            }
            Cmd::Copy => match self.ed.copy() {
                Some(f) => {
                    if self.cfg.system_clipboard {
                        self.clipboard_out = Some(f.text());
                    }
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
            Cmd::DeleteWordBack => {
                if self.guard_edit() {
                    let c = self.ed.cursor;
                    self.ed.word_left(false);
                    let s = self.ed.cursor;
                    self.ed.commit();
                    self.ed.delete_range(Range::new(s, c));
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
            Cmd::ClearIndent => self.para(|p| {
                p.indent_left = None;
                p.indent_right = None;
                p.first_line = None;
                p.hanging = None;
            }),
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
                    // Tab at the start of a list item demotes it, as in Word;
                    // in a table cell it moves to the next cell.
                    let c = self.ed.cursor;
                    let at_start = self.ed.doc.paragraphs[c.para].items[..c.idx].iter().all(|i| i.is_code());
                    if self.ed.current_cell().is_some() && !self.ed.has_selection() {
                        self.exec(Cmd::TableNextCell);
                    } else if at_start && !self.ed.has_selection() && self.ed.doc.list_ref(c.para).is_some() {
                        self.list_level(1);
                    } else {
                        self.ed.insert_code(Code::Tab);
                    }
                }
            }
            Cmd::TableInsert => {
                if self.guard_edit() {
                    if self.ed.current_cell().is_some() {
                        self.message("Already in a table — nested tables aren't editable in this version");
                    } else {
                        self.prompt(PromptKind::TableInsert, "Table size, rows × columns (e.g. 3x4): ", "3x3");
                    }
                }
            }
            Cmd::TableNextCell => {
                if self.ed.current_cell().is_none() {
                    self.message("Not in a table");
                } else if self.guard_edit() {
                    let rows = self.ed.doc.tables.get(&self.ed.current_cell().unwrap().table).map(|t| t.rows.len());
                    self.ed.next_cell();
                    let now = self.ed.doc.tables.get(&self.ed.current_cell().unwrap().table).map(|t| t.rows.len());
                    if now > rows {
                        self.message("New row added");
                    }
                }
            }
            Cmd::TablePrevCell => {
                if !self.ed.prev_cell() {
                    self.message("Not in a table");
                }
            }
            Cmd::TableInsertRowBelow => self.table_op(|ed| ed.insert_row(true), "Row inserted below"),
            Cmd::TableInsertRowAbove => self.table_op(|ed| ed.insert_row(false), "Row inserted above"),
            Cmd::TableInsertColRight => self.table_op(|ed| ed.insert_column(true), "Column inserted right"),
            Cmd::TableInsertColLeft => self.table_op(|ed| ed.insert_column(false), "Column inserted left"),
            Cmd::TableDeleteRow => self.table_op(|ed| ed.delete_row(), "Row deleted"),
            Cmd::TableDeleteCol => self.table_op(|ed| ed.delete_column(), "Column deleted"),
            Cmd::TableDelete => self.table_op(|ed| ed.delete_table(), "Table deleted"),
            Cmd::TableToText => self.table_op(|ed| ed.table_to_text(), "Table converted to tab-separated text"),
            Cmd::TableHeaderRow => {
                if self.ed.current_cell().is_none() {
                    self.message("Not in a table");
                } else if self.guard_edit() {
                    match self.ed.toggle_header_row() {
                        Some(true) => self.message("Row repeats as a header at the top of each page"),
                        Some(false) => self.message("Row no longer repeats as a header"),
                        None => self.message("Not in a table"),
                    }
                }
            }
            Cmd::TableColWidth => {
                if self.ed.current_cell().is_none() {
                    self.message("Not in a table");
                } else if self.guard_edit() {
                    let cur = self.ed.current_column_width().unwrap_or(1440);
                    self.prompt(PromptKind::ColumnWidth, "Column width in inches: ", &format!("{:.2}", cur as f64 / 1440.0));
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
            Cmd::ListOutdent if self.ed.current_cell().is_some() && self.ed.doc.list_ref(self.ed.cursor.para).is_none() => {
                self.exec(Cmd::TablePrevCell);
            }
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
                self.sync_editor_layout();
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
            Cmd::ToggleWrap => {
                self.cfg.draft_wrap = match self.cfg.draft_wrap {
                    WrapChoice::Page => WrapChoice::Terminal,
                    WrapChoice::Terminal => WrapChoice::Page,
                };
                let _ = self.cfg.save();
                self.sync_editor_layout();
                self.message(match self.cfg.draft_wrap {
                    WrapChoice::Page => "Lines wrap where they do on the page",
                    WrapChoice::Terminal => "Lines wrap to the terminal width",
                });
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
            Cmd::Menu => self.overlay = Overlay::Menu { menu: 0, item: crate::menu::first_item(0) },
            Cmd::MenuBar => {
                self.cfg.menu_bar = !self.cfg.menu_bar;
                let _ = self.cfg.save();
                self.resize(self.size.0, self.size.1);
            }
            Cmd::ThemeDefault => self.set_theme(ThemeChoice::Default),
            Cmd::ThemeClassic => self.set_theme(ThemeChoice::Classic),
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
                let label = self.find_label(cmd == Cmd::FindBackward);
                self.prompt(PromptKind::Find { backward: cmd == Cmd::FindBackward }, &label, &q);
            }
            Cmd::FindNext => self.find_step(false),
            Cmd::FindPrev => self.find_step(true),
            Cmd::FindRegex => {
                self.find.regex = true;
                self.exec(Cmd::Find);
            }
            Cmd::FindToggleCase => {
                self.find.case_sensitive = !self.find.case_sensitive;
                self.message(if self.find.case_sensitive { "Find: match case" } else { "Find: ignore case (smart: capitals in the query match case)" });
            }
            Cmd::FindToggleWord => {
                self.find.whole_word = !self.find.whole_word;
                self.message(if self.find.whole_word { "Find: whole words only" } else { "Find: partial words too" });
            }
            Cmd::FindToggleRegex => {
                self.find.regex = !self.find.regex;
                self.message(if self.find.regex { "Find: regular expressions on ($1… in replacements)" } else { "Find: plain text" });
            }
            Cmd::FindBold => self.find_with("bold:"),
            Cmd::FindItalic => self.find_with("italic:"),
            Cmd::FindUnderlined => self.find_with("underline:"),
            Cmd::FindHighlighted => self.find_with("highlight:"),
            Cmd::FindPageBreak => self.find_with("[HPg]"),
            Cmd::FindTab => self.find_with("[Tab]"),
            Cmd::FindLineBreak => self.find_with("[Ln Brk]"),
            Cmd::FindInStyle => {
                let cur = self.ed.doc.paragraphs[self.ed.cursor.para].props.style.clone().unwrap_or_default();
                self.prompt(PromptKind::FindStyle, "Find text in style (name or id, then optional text): ", &cur);
            }
            Cmd::FindCode => self.prompt(PromptKind::FindCode, "Find code (as shown in Reveal Codes, e.g. HPg, Tab, BOLD, Style:Heading1): ", ""),
            Cmd::Replace => {
                let q = self.find.query.clone();
                let label = format!("Replace — find{}: ", self.find.flags());
                self.prompt(PromptKind::ReplaceFind, &label, &q);
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
            let n = reveal::para_codes_at(&self.ed.doc, c.para).len();
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
                let n = reveal::para_codes_at(&self.ed.doc, c.para).len();
                self.reveal_para_code = if i + 1 < n { Some(i + 1) } else { None };
                return;
            }
        }
        let c = self.ed.cursor;
        let at_end = c.idx >= self.ed.doc.paragraphs[c.para].items.len();
        if self.reveal && !sel && at_end && c.para + 1 < self.ed.doc.paragraphs.len() {
            // Step onto the next paragraph's property codes, if any.
            let n = reveal::para_codes_at(&self.ed.doc, c.para + 1).len();
            self.ed.move_to(Pos::new(c.para + 1, 0), false);
            self.reveal_para_code = if n > 0 { Some(0) } else { None };
            return;
        }
        self.ed.move_right(sel, self.reveal);
    }

    /// Run a table command that needs the cursor in a table.
    fn table_op(&mut self, f: impl FnOnce(&mut wp_core::Editor) -> bool, done: &str) {
        if self.ed.current_cell().is_none() {
            self.message("Not in a table — Table: Insert… creates one");
            return;
        }
        if !self.guard_edit() {
            return;
        }
        if f(&mut self.ed) {
            self.message(done);
        } else {
            self.message("Couldn't do that here");
        }
        self.block_mode = false;
        self.reveal_para_code = None;
    }

    fn delete_para_code(&mut self, i: usize) {
        let para = self.ed.cursor.para;
        let codes = reveal::para_codes_at(&self.ed.doc, para);
        if let Some((which, label)) = codes.get(i) {
            if *which == ParaCode::RawBlock {
                self.message("This block is preserved as a whole and can't be removed here");
                return;
            }
            match which {
                ParaCode::TableDef => {
                    // As in WordPerfect: deleting [Tbl Def] leaves the text.
                    if self.guard_edit() && self.ed.table_to_text() {
                        self.message("Deleted [Tbl Def] — the table is now tab-separated text (Undo restores it)");
                        self.reveal_para_code = None;
                    }
                    return;
                }
                ParaCode::Row => {
                    self.message("Use Table: Delete Row to remove a row");
                    return;
                }
                ParaCode::Cell => {
                    self.message("Use Table: Delete Column to remove a column");
                    return;
                }
                _ => {}
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

    /// Give the editor the draft-view geometry: the text column count and
    /// how lines wrap (`draft_wrap` in config).
    pub fn sync_editor_layout(&mut self) {
        self.ed.set_cols(self.doc_cols());
        self.ed.set_wrap(self.cfg.draft_wrap.mode());
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
        self.gdoc = None;
        self.format = Format::Docx;
        self.warnings.clear();
        self.scroll = (0, 0);
        self.sticky_status = None;
        self.sync_editor_layout();
    }

    // ------------------------------------------------------------------
    // Find
    // ------------------------------------------------------------------

    fn find_label(&self, backward: bool) -> String {
        format!("{}{}: ", if backward { "Find backward" } else { "Find" }, self.find.flags())
    }

    /// Search from `from` for the search-box string `q`.
    pub fn search(&self, q: &str, from: Pos, backward: bool, wrap: bool) -> Option<Match> {
        let query = self.find.build(q);
        search::find(&self.ed.doc, &query, from, backward, wrap)
    }

    /// Run a canned query (from a palette command) as the next find.
    fn find_with(&mut self, q: &str) {
        self.find.query = q.to_string();
        self.find.backward = false;
        self.sticky_status = None;
        self.find_step(false);
    }

    fn select_match(&mut self, m: &Match) {
        if m.range.is_empty() {
            self.ed.anchor = None;
            self.ed.cursor = m.range.start;
        } else {
            self.ed.anchor = Some(m.range.start);
            self.ed.cursor = m.range.end;
        }
        self.block_mode = false;
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
            match self.ed.selection() {
                Some(r) => r.end,
                // An empty match (a code) must not be found again in place.
                None => Pos::new(self.ed.cursor.para, self.ed.cursor.idx + 1),
            }
        };
        let q = self.find.query.clone();
        match self.search(&q, from, backward, true) {
            Some(m) => {
                self.select_match(&m);
                let desc = self.find.build(&q).describe();
                self.sticky_status = Some(format!("Found {}", desc));
            }
            None => self.message(format!("Not found: {}", self.find.build(&q).describe())),
        }
    }

    fn incremental_find(&mut self, q: &str, backward: bool) {
        let from = self.find.origin;
        match self.search(q, from, backward, true) {
            Some(m) => {
                self.select_match(&m);
                let count = self.count_matches(q);
                self.sticky_status = Some(format!("{} match{}", count, if count == 1 { "" } else { "es" }));
            }
            None => {
                self.ed.cursor = from;
                self.ed.anchor = self.find.origin_anchor;
                self.sticky_status = Some(if q.trim().is_empty() { String::new() } else { "No matches".into() });
            }
        }
    }

    fn count_matches(&self, q: &str) -> usize {
        search::find_all(&self.ed.doc, &self.find.build(q), 10_000).len()
    }

    /// Replace one match at its range with the expanded replacement; the
    /// cursor ends after the inserted text. Returns false if it was protected.
    fn replace_match(&mut self, m: &Match, with: &str, regex: bool) -> bool {
        self.ed.cursor = m.range.start;
        self.ed.anchor = Some(m.range.end);
        if self.ed.selection_protected().is_some() {
            self.ed.anchor = None;
            return false;
        }
        let text = search::expand_replacement(with, m, regex);
        let items: Vec<Item> = text.chars().map(|c| if c == '\t' { Item::Code(Code::Tab) } else { Item::Char(c) }).collect();
        self.ed.replace_range(m.range, items);
        true

    }

    /// Replace every match from `from` onward.
    fn replace_all_from(&mut self, find: &str, with: &str, from: Pos) -> usize {
        let query = self.find.build(find);
        let mut n = 0;
        let mut from = from;
        self.ed.commit();
        while let Some(m) = search::find(&self.ed.doc, &query, from, false, false) {
            let end_para = m.range.start.para;
            if self.replace_match(&m, with, query.regex) {
                n += 1;
                from = self.ed.cursor;
            } else {
                from = Pos::new(end_para, m.range.end.idx.max(m.range.start.idx + 1));
            }
            if n > 100_000 {
                break;
            }
        }
        self.ed.commit();
        self.ed.anchor = None;
        n
    }


    /// Advance the one-at-a-time replacement to the next match after the cursor.
    fn replace_step_next(&mut self, find: String, with: String, done: usize, total: usize) {
        let query = self.find.build(&find);
        let from = self.ed.selection().map(|r| r.end).unwrap_or(self.ed.cursor);
        match search::find(&self.ed.doc, &query, from, false, false) {
            Some(m) => {
                self.select_match(&m);
                self.overlay = Overlay::ReplaceStep { find, with, done, total };
            }
            None => {
                self.ed.anchor = None;
                self.message(format!("Replaced {} of {}", done, total));
            }
        }
    }

    // ------------------------------------------------------------------
    // Overlays
    // ------------------------------------------------------------------

    fn set_theme(&mut self, t: ThemeChoice) {
        self.cfg.theme = t;
        let _ = self.cfg.save();
        self.message(match t {
            ThemeChoice::Default => "Theme: terminal default",
            ThemeChoice::Classic => "Theme: classic WordPerfect",
        });
    }

    /// Keys while a menu is open: ←/→ change menu, ↑/↓ move, Enter runs,
    /// a letter jumps to the item (or, with Alt, the menu) it starts with.
    fn menu_key(&mut self, menu: usize, item: usize, ev: KeyEvent) {
        use crate::menu::{self, Item, MENUS};
        let key = Key::from_event(&ev);
        let n = MENUS.len();
        // Alt+= / F10 again, Esc, or the keymap's Cancel key closes the menu.
        if matches!(self.keymap.lookup(&key), Some(Cmd::Menu) | Some(Cmd::Cancel)) {
            return;
        }
        match ev.code {
            KeyCode::Esc => {}
            KeyCode::Left => self.overlay = Overlay::Menu { menu: (menu + n - 1) % n, item: menu::first_item((menu + n - 1) % n) },
            KeyCode::Right | KeyCode::Tab => self.overlay = Overlay::Menu { menu: (menu + 1) % n, item: menu::first_item((menu + 1) % n) },
            KeyCode::Up => self.overlay = Overlay::Menu { menu, item: menu::prev_item(menu, item) },
            KeyCode::Down => self.overlay = Overlay::Menu { menu, item: menu::next_item(menu, item) },
            KeyCode::Home => self.overlay = Overlay::Menu { menu, item: menu::first_item(menu) },
            KeyCode::End => self.overlay = Overlay::Menu { menu, item: menu::prev_item(menu, menu::first_item(menu)) },
            KeyCode::Enter => {
                if let Some(Item::Cmd(c)) = MENUS[menu].items.get(item) {
                    self.exec(*c);
                }
            }
            KeyCode::Char(c) if key.alt => {
                if let Some(m) = menu::menu_by_letter(c) {
                    self.overlay = Overlay::Menu { menu: m, item: menu::first_item(m) };
                } else {
                    self.overlay = Overlay::Menu { menu, item };
                }
            }
            KeyCode::Char(c) if !key.ctrl && !key.sup => {
                // An item's first letter runs it; a menu mnemonic switches menus.
                if let Some(i) = menu::item_by_letter(menu, item, c) {
                    if let Some(Item::Cmd(cmd)) = MENUS[menu].items.get(i) {
                        self.exec(*cmd);
                    }
                } else if let Some(m) = menu::menu_by_letter(c) {
                    self.overlay = Overlay::Menu { menu: m, item: menu::first_item(m) };
                } else {
                    self.overlay = Overlay::Menu { menu, item };
                }
            }
            _ => self.overlay = Overlay::Menu { menu, item },
        }
    }

    /// Mouse while a menu is open: click a title to switch, an item to run
    /// it, anywhere else to close. Moving over the bar or the list follows.
    fn menu_mouse(&mut self, menu: usize, item: usize, ev: MouseEvent) {
        use crate::menu::{self, Item, MENUS};
        self.needs_redraw = true;
        let (x0, w, first, shown) = crate::ui::menu_frame(self, menu, item);
        let items = MENUS[menu].items;
        // The drop-down: rows 2.. (row 1 is its top border), columns x0..x0+w.
        let in_list = |col: u16, row: u16| -> Option<usize> {
            let r = row.checked_sub(2)? as usize + first;
            if r < first + shown && col >= x0 && col < x0 + w && matches!(items[r], Item::Cmd(_)) {
                Some(r)
            } else {
                None
            }
        };
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if ev.row == 0 {
                    match menu::menu_at(ev.column) {
                        Some(m) if m != menu => self.overlay = Overlay::Menu { menu: m, item: menu::first_item(m) },
                        Some(_) => {} // clicking the open title closes it
                        None => {}
                    }
                } else if let Some(i) = in_list(ev.column, ev.row) {
                    if let Item::Cmd(c) = items[i] {
                        self.exec(c);
                    }
                }
                // anywhere else: closed
            }
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                if ev.row == 0 {
                    if let Some(m) = menu::menu_at(ev.column) {
                        self.overlay = Overlay::Menu { menu: m, item: if m == menu { item } else { menu::first_item(m) } };
                        return;
                    }
                }
                let i = in_list(ev.column, ev.row).unwrap_or(item);
                self.overlay = Overlay::Menu { menu, item: i };
            }
            MouseEventKind::ScrollUp => self.overlay = Overlay::Menu { menu, item: menu::prev_item(menu, item) },
            MouseEventKind::ScrollDown => self.overlay = Overlay::Menu { menu, item: menu::next_item(menu, item) },
            _ => self.overlay = Overlay::Menu { menu, item },
        }
    }

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
                    label: if q.is_empty() { "Type to search (bold: italic: style:Name re: [HPg] …)".into() } else { format!("Find {} — {} match{}", self.find.build(q).describe(), count, if count == 1 { "" } else { "es" }) },
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
                        let repeat_bonus = 15 * palette::extra_occurrences(q, &hay);
                        let key = self.keymap.label_for(c.id).unwrap_or_default();
                        rows.push((s + title_bonus + repeat_bonus, PaletteRow { label: c.title.to_string(), detail: c.category.to_string(), key, action: PaletteAction::Cmd(c.id) }));
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
                KeyCode::Char(c @ ('r' | 'c' | 'w')) if key.alt && matches!(kind, PromptKind::Find { .. } | PromptKind::ReplaceFind) => {
                    match c {
                        'r' => self.find.regex = !self.find.regex,
                        'c' => self.find.case_sensitive = !self.find.case_sensitive,
                        _ => self.find.whole_word = !self.find.whole_word,
                    }
                    let label = match kind {
                        PromptKind::Find { backward } => {
                            self.incremental_find(&input, backward);
                            self.find_label(backward)
                        }
                        _ => format!("Replace — find{}: ", self.find.flags()),
                    };
                    self.overlay = Overlay::Prompt { kind, label, input };
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
                    if matches!(kind, PromptKind::SaveAs(_)) {
                        let (completed, rest) = complete_path(&input);
                        input = completed;
                        if let Some(names) = rest {
                            self.message(format!("{} matches: {}", names.len(), names.join("  ")));
                        }
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
            Overlay::Browse { dir, entries, mut selected, mut filter, mut all } => {
                let rows = browse_rows(&entries, &filter, all);
                let n = rows.len();
                let chosen = rows.get(selected.min(n.saturating_sub(1))).map(|e| (*e).clone());
                // Completion is prefix-based even though the filter is fuzzy:
                // a shared prefix is the only thing Tab can meaningfully extend.
                let lower = filter.to_lowercase();
                let pfx: Vec<&FileEntry> = rows.iter().copied().filter(|e| e.name.to_lowercase().starts_with(&lower)).collect();
                let cand = if pfx.is_empty() { &rows } else { &pfx };
                let tab_dir = (cand.len() == 1 && cand[0].is_dir).then(|| cand[0].name.clone());
                let tab_lcp = common_prefix(cand.iter().map(|e| e.name.as_str())).filter(|l| l.len() > filter.len() && l.to_lowercase().starts_with(&lower));
                match ev.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter | KeyCode::Right => match chosen {
                        Some(e) if e.is_dir => self.browse(&dir.join(&e.name), all),
                        Some(e) if ev.code == KeyCode::Enter => self.open_file(&dir.join(&e.name), Some((&dir, all))),
                        // Right on a file, or Enter with nothing listed, keeps the dialog.
                        _ => self.overlay = Overlay::Browse { dir, entries, selected, filter, all },
                    },
                    KeyCode::Left => self.browse(&dir.join(".."), all),
                    KeyCode::Backspace if filter.is_empty() => self.browse(&dir.join(".."), all),
                    KeyCode::Tab => match tab_dir {
                        // One candidate left: Tab walks into it, as a shell would.
                        Some(name) => self.browse(&dir.join(name), all),
                        None => {
                            if let Some(lcp) = tab_lcp {
                                filter = lcp;
                                selected = 0;
                            }
                            self.overlay = Overlay::Browse { dir, entries, selected, filter, all };
                        }
                    },
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Overlay::Browse { dir, entries, selected, filter, all };
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(n.saturating_sub(1));
                        self.overlay = Overlay::Browse { dir, entries, selected, filter, all };
                    }
                    KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                        selected = match ev.code {
                            KeyCode::PageUp => selected.saturating_sub(BROWSE_ROWS),
                            KeyCode::PageDown => (selected + BROWSE_ROWS).min(n.saturating_sub(1)),
                            KeyCode::Home => 0,
                            _ => n.saturating_sub(1),
                        };
                        self.overlay = Overlay::Browse { dir, entries, selected, filter, all };
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        self.overlay = Overlay::Browse { dir, entries, selected: 0, filter, all };
                    }
                    KeyCode::Char('u') if key.ctrl => {
                        filter.clear();
                        self.overlay = Overlay::Browse { dir, entries, selected: 0, filter, all };
                    }
                    KeyCode::Char('a') if key.alt => {
                        all = !all;
                        self.overlay = Overlay::Browse { dir, entries, selected: 0, filter, all };
                    }
                    KeyCode::Char(c) if !key.ctrl && !key.alt && !key.sup => {
                        filter.push(c);
                        self.overlay = Overlay::Browse { dir, entries, selected: 0, filter, all };
                        // A slash means the typed text is a path, not a filter.
                        if c == '/' && !self.browse_retarget() {
                            if let Overlay::Browse { filter, .. } = &mut self.overlay {
                                filter.pop(); // nowhere to go; drop the slash
                            }
                        }
                    }
                    _ => self.overlay = Overlay::Browse { dir, entries, selected, filter, all },
                }
            }
            Overlay::Drive(mut d) => {
                let n = d.visible().len();
                let chosen = d.visible().get(d.selected.min(n.saturating_sub(1))).map(|(e, _)| (*e).clone());
                match ev.code {
                    KeyCode::Esc => self.drive_search_due = None,
                    KeyCode::Enter => match google::parse_doc_ref(&d.filter) {
                        Some(id) => self.queue(Pending::Open { id, force: false }),
                        None => match chosen {
                            Some(e) if e.kind == DriveKind::Doc => self.queue(Pending::Open { id: e.id, force: false }),
                            Some(e) => self.drive_enter(d, &e),
                            None => self.overlay = Overlay::Drive(d),
                        },
                    },
                    KeyCode::Right => match chosen {
                        Some(e) if e.kind != DriveKind::Doc => self.drive_enter(d, &e),
                        _ => self.overlay = Overlay::Drive(d),
                    },
                    KeyCode::Left => self.drive_up(d),
                    KeyCode::Backspace if d.filter.is_empty() => self.drive_up(d),
                    KeyCode::Tab => self.drive_toggle_mode(d),
                    KeyCode::Char('f') if key.alt => self.drive_toggle_mode(d),
                    KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                        d.selected = match ev.code {
                            KeyCode::Up => d.selected.saturating_sub(1),
                            KeyCode::Down => (d.selected + 1).min(n.saturating_sub(1)),
                            KeyCode::PageUp => d.selected.saturating_sub(BROWSE_ROWS),
                            KeyCode::PageDown => (d.selected + BROWSE_ROWS).min(n.saturating_sub(1)),
                            KeyCode::Home => 0,
                            _ => n.saturating_sub(1),
                        };
                        self.overlay = Overlay::Drive(d);
                    }
                    KeyCode::Backspace => {
                        d.filter.pop();
                        d.selected = 0;
                        self.overlay = Overlay::Drive(d);
                        self.drive_filter_changed();
                    }
                    KeyCode::Char('u') if key.ctrl => {
                        d.filter.clear();
                        d.selected = 0;
                        self.overlay = Overlay::Drive(d);
                        self.drive_filter_changed();
                    }
                    KeyCode::Char(c) if !key.ctrl && !key.alt && !key.sup => {
                        d.filter.push(c);
                        d.selected = 0;
                        self.overlay = Overlay::Drive(d);
                        self.drive_filter_changed();
                    }
                    _ => self.overlay = Overlay::Drive(d),
                }
            }
            Overlay::Confirm { question, action } => match ev.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm(action, true),
                KeyCode::Char('n') | KeyCode::Char('N') => self.confirm(action, false),
                KeyCode::Esc => {}
                _ => self.overlay = Overlay::Confirm { question, action },
            },
            Overlay::Help | Overlay::Message { .. } => {}
            Overlay::Menu { menu, item } => self.menu_key(menu, item, ev),
            Overlay::ReplacePreview { find, with, matches, mut selected } => match ev.code {
                KeyCode::Esc => {
                    self.ed.anchor = None;
                    self.ed.cursor = self.find.origin;
                }
                KeyCode::Enter | KeyCode::Char('a') | KeyCode::Char('A') => {
                    let n = self.replace_all_from(&find, &with, Pos::default());
                    self.message(format!("Replaced {} occurrence{}", n, if n == 1 { "" } else { "s" }));
                }
                KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let total = matches.len();
                    if let Some(m) = matches.get(selected) {
                        self.select_match(m);
                        self.overlay = Overlay::ReplaceStep { find, with, done: 0, total };
                    }
                }
                KeyCode::Up | KeyCode::Down => {
                    selected = if ev.code == KeyCode::Up { selected.saturating_sub(1) } else { (selected + 1).min(matches.len().saturating_sub(1)) };
                    if let Some(m) = matches.get(selected) {
                        self.select_match(m);
                    }
                    self.overlay = Overlay::ReplacePreview { find, with, matches, selected };
                }
                _ => self.overlay = Overlay::ReplacePreview { find, with, matches, selected },
            },
            Overlay::ReplaceStep { find, with, done, total } => match ev.code {
                KeyCode::Esc => {
                    self.ed.anchor = None;
                    self.message(format!("Replaced {} of {}", done, total));
                }
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter | KeyCode::Char(' ') => {

                    let query = self.find.build(&find);
                    let sel = self.ed.selection().unwrap_or(Range { start: self.ed.cursor, end: self.ed.cursor });
                    let m = search::find(&self.ed.doc, &query, sel.start, false, false).filter(|m| m.range.start == sel.start);
                    let mut done = done;
                    if let Some(m) = m {
                        self.ed.commit();
                        if self.replace_match(&m, &with, query.regex) {
                            done += 1;
                        }
                        self.ed.commit();
                    }
                    self.replace_step_next(find, with, done, total);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => self.replace_step_next(find, with, done, total),
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    let from = self.ed.selection().map(|r| r.start).unwrap_or(self.ed.cursor);
                    let n = self.replace_all_from(&find, &with, from);
                    self.message(format!("Replaced {} of {}", done + n, total));
                }
                _ => self.overlay = Overlay::ReplaceStep { find, with, done, total },
            },
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
                if yes && self.format == Format::GoogleDoc && self.gdoc.is_some() {
                    self.quit_after_save = true;
                    self.queue(Pending::Save);
                } else if yes {
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
            ConfirmAction::OpenDriveDiscard(id) => {
                if yes {
                    self.remove_recovery();
                    self.queue(Pending::Open { id, force: true });
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
                    } else if ext == "md" {
                        std::fs::read(&p).map(|b| self.ed.replace_document(wp_md::from_markdown(&decode_text(&b)))).map_err(|e| e.into())
                    } else if ext == "json" {
                        std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|s| {
                            let v: serde_json::Value = serde_json::from_str(&s)?;
                            let doc: Document = serde_json::from_value(v["doc"].clone())?;
                            let baseline: wp_gdoc::Baseline = serde_json::from_value(v["baseline"].clone())?;
                            let id = v["id"].as_str().unwrap_or("").to_string();
                            let title = v["title"].as_str().unwrap_or("").to_string();
                            self.ed.replace_document(doc);
                            self.gdoc = Some(GdocState { id, title, baseline });
                            Ok(())
                        })
                    } else {
                        std::fs::read(&p).map(|b| self.ed.replace_document(wp_core::text::from_text(&decode_text(&b), false))).map_err(|e| e.into())
                    } {
                        Ok(()) => {
                            self.path = keep_path;
                            self.format = keep_format;
                            self.ed.dirty = true;
                            self.sync_editor_layout();
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
            PromptKind::SaveAs(fmt) => {
                if v.is_empty() {
                    return;
                }
                let mut p = expand_path(&v);
                let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
                let fmt = match ext.as_deref() {
                    Some("txt") | Some("text") => Format::Text,
                    Some("md") | Some("markdown") => Format::Markdown,
                    Some("docx") => Format::Docx,
                    _ => {
                        p.set_extension(match fmt {
                            Format::Text => "txt",
                            Format::Markdown => "md",
                            Format::Docx | Format::GoogleDoc => "docx",
                        });
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
                let query = self.find.build(&find);
                let matches = search::find_all(&self.ed.doc, &query, 10_000);
                if matches.is_empty() {
                    self.message(format!("Not found: {}", query.describe()));
                } else {
                    self.find.query = find.clone();
                    self.find.origin = self.ed.cursor;
                    self.select_match(&matches[0]);
                    self.overlay = Overlay::ReplacePreview { find, with: v, matches, selected: 0 };
                }
            }
            PromptKind::FindStyle => {
                if v.is_empty() {
                    return;
                }
                let (style, text) = match v.split_once("  ") {
                    Some((s, t)) => (s.trim(), t.trim()),
                    None => (v.as_str(), ""),
                };
                let q = if style.contains(' ') { format!("style:\"{}\" {}", style, text) } else { format!("style:{} {}", style, text) };
                self.find_with(q.trim());
            }
            PromptKind::FindCode => {
                if v.is_empty() {
                    return;
                }
                let q = format!("[{}]", v.trim_matches(|c| c == '[' || c == ']'));
                self.find_with(&q);
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
            PromptKind::TableInsert => {
                let nums: Vec<usize> = v.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty()).filter_map(|t| t.parse().ok()).collect();
                let (rows, cols) = match nums.as_slice() {
                    [r, c, ..] => (*r, *c),
                    [r] => (*r, *r),
                    _ => {
                        self.message("Enter rows and columns, e.g. 3x4");
                        return;
                    }
                };
                if rows == 0 || cols == 0 || rows > 1000 || cols > 63 {
                    self.message("Tables can have 1–1000 rows and 1–63 columns");
                    return;
                }
                if self.ed.insert_table(rows, cols) {
                    self.message(format!("Inserted a {}×{} table — Tab moves between cells", rows, cols));
                } else {
                    self.message("Can't insert a table here");
                }
            }
            PromptKind::ColumnWidth => match parse_inches(&v) {
                Some(w) if w >= 360 => {
                    if self.ed.set_column_width(w) {
                        self.message(format!("Column width {:.2}\"", w as f64 / 1440.0));
                    }
                }
                _ => self.message("Enter a width in inches (at least 0.25)"),
            },
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

/// How many rows the Open dialog shows at once (also the PgUp/PgDn step).
pub const BROWSE_ROWS: usize = 16;

/// Documents wp opens with full fidelity; everything else is read as text.
fn is_doc(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "docx" | "md" | "markdown" | "txt" | "text")
}

/// Directories first, then files, each sorted case-insensitively.
fn read_entries(dir: &Path) -> std::io::Result<Vec<FileEntry>> {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // Follow symlinks, so a linked directory browses as one.
        let md = std::fs::metadata(e.path()).or_else(|_| e.metadata())?;
        if md.is_dir() {
            dirs.push(FileEntry { name, is_dir: true, is_doc: false, detail: String::new() });
        } else {
            let when = md.modified().ok().map(stamp).unwrap_or_default();
            files.push(FileEntry { is_doc: is_doc(&name), name, is_dir: false, detail: format!("{}  {}", size(md.len()), when) });
        }
    }
    let key = |e: &FileEntry| e.name.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    let mut out = Vec::with_capacity(dirs.len() + files.len() + 1);
    if dir.parent().is_some() {
        out.push(FileEntry { name: "..".into(), is_dir: true, is_doc: false, detail: "parent directory".into() });
    }
    out.append(&mut dirs);
    out.append(&mut files);
    Ok(out)
}

/// The rows the Open dialog shows: dot-files only when asked for by name or by
/// `all`, non-document files only when `all`.
pub fn browse_rows<'a>(entries: &'a [FileEntry], filter: &str, all: bool) -> Vec<&'a FileEntry> {
    let hidden_ok = all || filter.starts_with('.');
    entries
        .iter()
        .filter(|e| {
            let hidden = e.name.starts_with('.') && e.name != "..";
            (hidden_ok || !hidden) && (all || e.is_dir || e.is_doc) && subsequence(filter, &e.name)
        })
        .collect()
}

/// Case-insensitive subsequence: the filter's letters in order. Deliberately
/// not `palette::score`, whose one-typo tolerance would let `gen-l` match every
/// `gen-` file — in a file list a near-miss should narrow, not widen.
fn subsequence(filter: &str, name: &str) -> bool {
    let mut n = name.chars().flat_map(|c| c.to_lowercase());
    filter.chars().flat_map(|c| c.to_lowercase()).all(|c| n.any(|x| x == c))
}

fn common_prefix<'a>(mut names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut lcp: String = names.next()?.to_string();
    for n in names {
        while !n.to_lowercase().starts_with(&lcp.to_lowercase()) {
            lcp.pop();
        }
    }
    Some(lcp)
}

fn size(n: u64) -> String {
    match n {
        0..=1023 => format!("{} B", n),
        1024..=1048575 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1048576.0),
    }
}

fn stamp(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (y, m, d) = civil(secs.div_euclid(86400));
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Returns `(input, Some(candidates))` when the completion is ambiguous, so the
/// caller can show what a bare Tab could not decide between.
fn complete_path(input: &str) -> (String, Option<Vec<String>>) {
    let p = expand_path(input);
    let (dir, prefix) = if input.ends_with('/') {
        (p.clone(), String::new())
    } else {
        (p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")), p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
    };
    let dir_read = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir.clone() };
    let Ok(rd) = std::fs::read_dir(&dir_read) else { return (input.to_string(), None) };
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
        return (input.to_string(), None);
    }
    // Longest common prefix.
    let mut lcp = matches[0].clone();
    for m in &matches[1..] {
        while !m.starts_with(&lcp) {
            lcp.pop();
        }
    }
    let base = if input.ends_with('/') { input.to_string() } else { input[..input.len() - prefix.len()].to_string() };
    let ambiguous = (matches.len() > 1 && lcp.len() == prefix.len()).then(|| matches.into_iter().take(12).collect());
    (format!("{}{}", base, lcp), ambiguous)
}

fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (y, m, d) = civil(secs.div_euclid(86400));
    let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    format!("{} {}, {}", months[(m - 1) as usize], d, y)
}

/// Civil (y, m, d) from a Unix day number, without a date crate.
fn civil(days: i64) -> (i64, i64, i64) {
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
    (y, m, d)
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

/// While a sign-in blocks the main loop: has the user pressed Esc?
fn cancel_pressed() -> bool {
    while crossterm::event::poll(Duration::from_millis(0)).unwrap_or(false) {
        if let Ok(crossterm::event::Event::Key(k)) = crossterm::event::read() {
            if k.code == KeyCode::Esc {
                return true;
            }
        }
    }
    false
}
