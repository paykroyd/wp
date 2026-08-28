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
}

impl Key {
    pub fn from_event(ev: &KeyEvent) -> Key {
        let mut code = ev.code;
        let mut ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let mut alt = ev.modifiers.contains(KeyModifiers::ALT);
        let mut shift = ev.modifiers.contains(KeyModifiers::SHIFT);
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
            if c.is_ascii_uppercase() && (ctrl || alt) {
                code = KeyCode::Char(c.to_ascii_lowercase());
                shift = true;
            } else if c.is_ascii_uppercase() {
                // plain typed capital: leave as text, not a binding
                shift = false;
            } else if ctrl || alt {
                // some terminals send lowercase with SHIFT set
            } else {
                shift = false;
            }
        }
        if matches!(code, KeyCode::BackTab) {
            code = KeyCode::Tab;
            shift = true;
        }
        Key { code, ctrl, alt, shift }
    }

    /// Parse "ctrl+shift+f8", "alt+f3", "f6", "ctrl+k", "enter", "esc".
    pub fn parse(s: &str) -> Option<Key> {
        let mut k = Key { code: KeyCode::Null, ctrl: false, alt: false, shift: false };
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
        let (mods, last) = parts.split_at(parts.len() - 1);
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => k.ctrl = true,
                "alt" | "meta" | "option" | "m" => k.alt = true,
                "shift" | "s" => k.shift = true,
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
    ("insert", "typeover"),
    ("ctrl+k", "palette"),
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

/// What everyone else expects; the classic F-keys sit underneath.
const MODERN: &[(&str, &str)] = &[
    ("esc", "cancel"),
    ("f1", "help"),
    ("ctrl+s", "save"),
    ("ctrl+shift+s", "save-as"),
    ("ctrl+o", "open"),
    ("ctrl+n", "new"),
    ("ctrl+q", "exit"),
    ("ctrl+w", "exit"),
    ("ctrl+z", "undo"),
    ("ctrl+y", "redo"),
    ("ctrl+shift+z", "redo"),
    ("ctrl+x", "cut"),
    ("ctrl+c", "copy"),
    ("ctrl+v", "paste"),
    ("ctrl+shift+v", "paste-plain"),
    ("ctrl+a", "select-all"),
    ("ctrl+b", "bold"),
    ("ctrl+i", "italic"),
    ("ctrl+u", "underline"),
    ("ctrl+d", "font"),
    ("ctrl+shift+d", "double-underline"),
    ("ctrl+shift+x", "strikethrough"),
    ("ctrl+shift+=", "superscript"),
    ("ctrl+=", "subscript"),
    ("ctrl+shift+k", "small-caps"),
    ("ctrl+shift+a", "all-caps"),
    ("ctrl+space", "remove-formatting"),
    ("ctrl+f", "find"),
    ("ctrl+h", "replace"),
    ("f3", "find-next"),
    ("shift+f3", "find-prev"),
    ("ctrl+g", "goto-page"),
    ("ctrl+e", "align-center"),
    ("ctrl+l", "align-left"),
    ("ctrl+r", "align-right"),
    ("ctrl+j", "align-justify"),
    ("ctrl+m", "indent"),
    ("ctrl+shift+m", "outdent"),
    ("ctrl+t", "hanging-indent"),
    ("ctrl+1", "spacing-single"),
    ("ctrl+5", "spacing-1.5"),
    ("ctrl+2", "spacing-double"),
    ("ctrl+alt+1", "style-heading-1"),
    ("ctrl+alt+2", "style-heading-2"),
    ("ctrl+alt+3", "style-heading-3"),
    ("ctrl+shift+n", "style-normal"),
    ("ctrl+p", "word-count"),
    ("alt+=", "palette"),
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
        labels.sort_by_key(|l| (matches!(l.as_str(), "F1" | "Esc") as u8, l.starts_with('F') as u8, l.len()));
        Some(labels.remove(0))
    }

    /// Verify every listed command is still reachable somehow (palette
    /// completeness is by construction; this is for the F-key legend).
    pub fn fkey_row(&self, n: u8) -> [Option<Cmd>; 4] {
        let f = |ctrl, alt, shift| self.map.get(&Key { code: KeyCode::F(n), ctrl, alt, shift }).copied();
        [f(true, false, false), f(false, true, false), f(false, false, true), f(false, false, false)]
    }
}
