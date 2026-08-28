//! Key bindings: two complete maps, the classic F-keys layered under the
//! modern one, everything rebindable from the config file.

use crate::commands::{by_key, Cmd};
use crate::config::{Config, KeymapChoice};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// A normalised key: code plus ctrl/alt/shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS / Super elsewhere. Only delivered by terminals that
    /// speak the kitty keyboard protocol (Ghostty, kitty, WezTerm…).
    pub sup: bool,
}

impl Key {
    pub fn from_event(ev: &KeyEvent) -> Key {
        let mut code = ev.code;
        let mut ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let mut alt = ev.modifiers.contains(KeyModifiers::ALT);
        let mut shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let sup = ev.modifiers.contains(KeyModifiers::SUPER) || ev.modifiers.contains(KeyModifiers::META);
        // xterm reports modified F-keys as higher F numbers.
        if let KeyCode::F(n) = code {
            match n {
                13..=24 => {
                    code = KeyCode::F(n - 12);
                    shift = true;
                }
                25..=36 => {
                    code = KeyCode::F(n - 24);
                    ctrl = true;
                }
                37..=48 => {
                    code = KeyCode::F(n - 36);
                    ctrl = true;
                    shift = true;
                }
                49..=60 => {
                    code = KeyCode::F(n - 48);
                    alt = true;
                }
                _ => {}
            }
        }
        // Letters: normalise case so Shift+b and B match "shift+b".
        if let KeyCode::Char(c) = code {
            let modded = ctrl || alt || sup;
            if c.is_ascii_uppercase() && modded {
                code = KeyCode::Char(c.to_ascii_lowercase());
                shift = true;
            } else if c.is_ascii_alphabetic() && !modded {
                // plain typed letter: text, not a binding
                shift = false;
            } else if !c.is_ascii_alphabetic() {
                // punctuation already encodes shift ('<' vs ','): drop the flag
                shift = false;
            }
        }
        if matches!(code, KeyCode::BackTab) {
            code = KeyCode::Tab;
            shift = true;
        }
        Key { code, ctrl, alt, shift, sup }
    }

    /// Parse "ctrl+shift+f8", "alt+f3", "f6", "ctrl+k", "enter", "esc".
    pub fn parse(s: &str) -> Option<Key> {
        let mut k = Key { code: KeyCode::Null, ctrl: false, alt: false, shift: false, sup: false };
        // "ctrl++" means Ctrl and the '+' key.
        let (body, last_char) = match s.strip_suffix("++") {
            Some(b) => (b, Some("+")),
            None => (s, None),
        };
        let mut parts: Vec<&str> = body.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
        if let Some(c) = last_char {
            parts.push(c);
        }
        if parts.is_empty() {
            return None;
        }
        let (mods, last) = parts.split_at(parts.len() - 1);
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => k.ctrl = true,
                "alt" | "meta" | "option" | "m" => k.alt = true,
                "shift" | "s" => k.shift = true,
                "cmd" | "super" | "win" | "command" => k.sup = true,
                _ => return None,
            }
        }
        let l = last[0].to_ascii_lowercase();
        k.code = match l.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "space" => KeyCode::Char(' '),
            _ => {
                if let Some(n) = l.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    KeyCode::F(n)
                } else if l.chars().count() == 1 {
                    KeyCode::Char(l.chars().next().unwrap())
                } else {
                    return None;
                }
            }
        };
        Some(k)
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.sup {
            s.push_str("Cmd+");
        }
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(&match self.code {
            KeyCode::F(n) => format!("F{}", n),
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Insert => "Ins".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            other => format!("{:?}", other),
        });
        s
    }
}

/// Bindings shared by both maps: cursor keys and editing keys.
const COMMON: &[(&str, &str)] = &[
    ("left", "left"),
    ("right", "right"),
    ("up", "up"),
    ("down", "down"),
    ("ctrl+left", "word-left"),
    ("ctrl+right", "word-right"),
    ("alt+left", "word-left"),
    ("alt+right", "word-right"),
    ("home", "line-start"),
    ("end", "line-end"),
    ("ctrl+home", "doc-start"),
    ("ctrl+end", "doc-end"),
    ("ctrl+up", "para-up"),
    ("ctrl+down", "para-down"),
    ("pageup", "page-up"),
    ("pagedown", "page-down"),
    ("shift+left", "select-left"),
    ("shift+right", "select-right"),
    ("shift+up", "select-up"),
    ("shift+down", "select-down"),
    ("ctrl+shift+left", "select-word-left"),
    ("ctrl+shift+right", "select-word-right"),
    ("shift+home", "select-line-start"),
    ("shift+end", "select-line-end"),
    ("ctrl+shift+home", "select-doc-start"),
    ("ctrl+shift+end", "select-doc-end"),
    ("shift+pageup", "select-page-up"),
    ("shift+pagedown", "select-page-down"),
    ("backspace", "backspace"),
    ("ctrl+backspace", "delete-word"),
    ("alt+backspace", "delete-word"),
    ("delete", "delete"),
    ("enter", "enter"),
    ("tab", "tab"),
    ("shift+tab", "list-outdent"),
    ("insert", "typeover"),
    ("alt+f3", "reveal-codes"),
    ("f11", "reveal-codes"),
    ("ctrl+pagedown", "next-page"),
    ("ctrl+pageup", "prev-page"),
    ("ctrl+enter", "page-break"),
    ("shift+enter", "line-break"),
];

/// WordPerfect 5.1.
const CLASSIC: &[(&str, &str)] = &[
    ("f1", "cancel"),
    ("esc", "repeat"),
    ("ctrl+k", "palette"),
    ("shift+f1", "page-setup"),
    ("f2", "find"),
    ("shift+f2", "find-backward"),
    ("alt+f2", "replace"),
    ("f3", "help"),
    ("shift+f3", "toggle-view"),
    ("ctrl+f3", "redraw"),
    ("f4", "indent"),
    ("shift+f4", "indent-both"),
    ("alt+f4", "block"),
    ("f12", "block"),
    ("ctrl+f4", "cut"),
    ("f5", "open"),
    ("shift+f5", "date"),
    ("alt+f5", "bookmark"),
    ("ctrl+f5", "save-as-text"),
    ("f6", "bold"),
    ("shift+f6", "align-center"),
    ("alt+f6", "align-right"),
    ("f7", "exit"),
    ("shift+f7", "word-count"),
    ("ctrl+f7", "page-break"),
    ("alt+f7", "table-insert"),
    ("f8", "underline"),
    ("shift+f8", "spacing-double"),
    ("alt+f8", "style"),
    ("ctrl+f8", "font"),
    ("ctrl+f9", "goto-page"),
    ("f10", "save"),
    ("shift+f10", "open"),
    ("alt+f10", "palette"),
    ("ctrl+f10", "italic"),
    ("ctrl+f2", "find-next"),
    ("ctrl+f1", "about"),
    ("ctrl+z", "undo"),
];

/// What everyone else expects, plus emacs / macOS-readline movement and
/// deletion on the Ctrl and Alt (Meta) keys. The classic F-keys sit underneath.
const MODERN: &[(&str, &str)] = &[
    ("esc", "cancel"),
    ("f1", "help"),
    ("f3", "find-next"),
    ("shift+f3", "find-prev"),
    // emacs movement
    ("ctrl+f", "right"),
    ("ctrl+b", "left"),
    ("ctrl+n", "down"),
    ("ctrl+p", "up"),
    ("ctrl+a", "line-start"),
    ("ctrl+e", "line-end"),
    ("alt+f", "word-right"),
    ("alt+b", "word-left"),
    ("alt+v", "page-up"),
    ("alt+<", "doc-start"),
    ("alt+>", "doc-end"),
    ("ctrl+space", "block"),
    // emacs / readline deletion
    ("ctrl+d", "delete"),
    ("ctrl+h", "backspace"),
    ("ctrl+k", "delete-eol"),
    ("ctrl+u", "delete-bol"),
    ("alt+d", "delete-word"),
    ("ctrl+w", "cut"),
    ("alt+w", "copy"),
    ("ctrl+y", "paste"),
    // everything else
    ("ctrl+shift+p", "palette"),
    ("alt+=", "palette"),
    ("ctrl+s", "save"),
    ("ctrl+shift+s", "save-as"),
    ("ctrl+o", "open"),
    ("ctrl+shift+n", "new"),
    ("ctrl+q", "exit"),
    ("ctrl+z", "undo"),
    ("ctrl+shift+z", "redo"),
    ("ctrl+x", "cut"),
    ("ctrl+c", "copy"),
    ("ctrl+v", "paste"),
    ("ctrl+shift+v", "paste-plain"),
    ("ctrl+shift+a", "select-all"),
    ("ctrl+shift+b", "bold"),
    ("ctrl+i", "italic"),
    ("ctrl+shift+u", "underline"),
    ("ctrl+alt+u", "double-underline"),
    ("ctrl+shift+d", "font"),
    ("ctrl+shift+x", "strikethrough"),
    ("ctrl++", "superscript"),
    ("ctrl+=", "subscript"),
    ("ctrl+shift+k", "small-caps"),
    ("ctrl+alt+a", "all-caps"),
    ("ctrl+\\", "remove-formatting"),
    ("ctrl+shift+f", "find"),
    ("ctrl+shift+h", "replace"),
    ("ctrl+g", "goto-page"),
    ("ctrl+shift+e", "align-center"),
    ("ctrl+l", "align-left"),
    ("ctrl+r", "align-right"),
    ("ctrl+j", "align-justify"),
    ("alt+]", "indent"),
    ("alt+[", "outdent"),
    ("ctrl+t", "hanging-indent"),
    ("ctrl+shift+l", "list-bullet"),
    ("ctrl+shift+o", "list-number"),

    ("ctrl+1", "spacing-single"),
    ("ctrl+5", "spacing-1.5"),
    ("ctrl+2", "spacing-double"),
    ("ctrl+alt+1", "style-heading-1"),
    ("ctrl+alt+2", "style-heading-2"),
    ("ctrl+alt+3", "style-heading-3"),
    ("ctrl+alt+0", "style-normal"),
    ("ctrl+shift+w", "word-count"),
];

/// The Cmd (Super) layer, added on top of the modern map. Only terminals
/// that report Cmd deliver these (Ghostty, kitty, WezTerm with the kitty
/// keyboard protocol), and the terminal's own Cmd shortcuts win; every command
/// here also has a Ctrl or F-key binding.
const MAC: &[(&str, &str)] = &[
    ("cmd+s", "save"),
    ("cmd+shift+s", "save-as"),
    ("cmd+o", "open"),
    ("cmd+n", "new"),
    ("cmd+q", "exit"),
    ("cmd+z", "undo"),
    ("cmd+shift+z", "redo"),
    ("cmd+x", "cut"),
    ("cmd+c", "copy"),
    ("cmd+v", "paste"),
    ("cmd+shift+v", "paste-plain"),
    ("cmd+a", "select-all"),
    ("cmd+b", "bold"),
    ("cmd+i", "italic"),
    ("cmd+u", "underline"),
    ("cmd+d", "font"),
    ("cmd+f", "find"),
    ("cmd+g", "find-next"),
    ("cmd+shift+g", "find-prev"),
    ("cmd+shift+h", "replace"),
    ("cmd+e", "align-center"),
    ("cmd+l", "align-left"),
    ("cmd+r", "align-right"),
    ("cmd+j", "align-justify"),
    ("cmd+]", "indent"),
    ("cmd+[", "outdent"),
    ("cmd+shift+p", "palette"),
    // Cmd+P too: newer Ghostty keeps Cmd+Shift+P for its own palette, and
    // there is no Print command to collide with.
    ("cmd+p", "palette"),
    ("cmd+left", "line-start"),
    ("cmd+right", "line-end"),
    ("cmd+up", "doc-start"),
    ("cmd+down", "doc-end"),
    ("cmd+shift+left", "select-line-start"),
    ("cmd+shift+right", "select-line-end"),
    ("cmd+shift+up", "select-doc-start"),
    ("cmd+shift+down", "select-doc-end"),
    ("cmd+backspace", "delete-bol"),
    ("cmd+delete", "delete-eol"),
    ("cmd+alt+1", "style-heading-1"),
    ("cmd+alt+2", "style-heading-2"),
    ("cmd+alt+3", "style-heading-3"),
    ("cmd+alt+0", "style-normal"),
];

pub struct Keymap {
    map: HashMap<Key, Cmd>,
    pub choice: KeymapChoice,
}

impl Keymap {
    pub fn build(cfg: &Config) -> Keymap {
        let mut map = HashMap::new();
        let mut add = |table: &[(&str, &str)]| {
            for (k, c) in table {
                if let (Some(key), Some(cmd)) = (Key::parse(k), by_key(c)) {
                    map.insert(key, cmd);
                }
            }
        };
        add(COMMON);
        match cfg.keymap {
            KeymapChoice::Classic => add(CLASSIC),
            KeymapChoice::Modern => {
                add(CLASSIC);
                add(MODERN);
                add(MAC);
            }
        }
        for (k, c) in &cfg.bindings {
            if let Some(key) = Key::parse(k) {
                match by_key(c) {
                    Some(cmd) => {
                        map.insert(key, cmd);
                    }
                    None => {
                        map.remove(&key);
                    }
                }
            }
        }
        Keymap { map, choice: cfg.keymap }
    }

    pub fn lookup(&self, key: &Key) -> Option<Cmd> {
        self.map.get(key).copied()
    }

    /// The primary key label for a command (shortest, preferring modern keys).
    pub fn label_for(&self, cmd: Cmd) -> Option<String> {
        let mut labels: Vec<String> = self.map.iter().filter(|(_, c)| **c == cmd).map(|(k, _)| k.label()).collect();
        if labels.is_empty() {
            return None;
        }
        // Prefer keys every terminal delivers: Ctrl/Alt over Cmd, then F-keys, then short.
        let is_fkey = |l: &str| {
            let last = l.rsplit('+').next().unwrap_or("");
            last.len() >= 2 && last.starts_with('F') && last[1..].chars().all(|c| c.is_ascii_digit())
        };
        // On macOS, Option is not Alt unless the terminal is told to send it
        // (Ghostty's `macos-option-as-alt`), so an Alt label is the one we are
        // least sure the user can press. Rank it last where a twin exists.
        let alt_last = |l: &str| cfg!(target_os = "macos") && l.starts_with("Alt+");
        labels.sort_by_key(|l| (alt_last(l) as u8, l.starts_with("Cmd+") as u8, matches!(l.as_str(), "F1" | "Esc") as u8, is_fkey(l) as u8, l.len()));
        Some(labels.remove(0))
    }

    /// Verify every listed command is still reachable somehow (palette
    /// completeness is by construction; this is for the F-key legend).
    pub fn fkey_row(&self, n: u8) -> [Option<Cmd>; 4] {
        let f = |ctrl, alt, shift| self.map.get(&Key { code: KeyCode::F(n), ctrl, alt, shift, sup: false }).copied();
        [f(true, false, false), f(false, true, false), f(false, false, true), f(false, false, false)]
    }
}
