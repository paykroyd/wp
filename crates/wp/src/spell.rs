//! Spelling (SPEC.md P0-13, the first cut; DESIGN.md §7.5): misspellings
//! found against a plain word list and underlined in both views as you
//! write, with suggestions, Ignore and Add, and user / project word lists.
//!
//! The word list is whatever the machine has — macOS and most Linux
//! systems ship Webster's Second at `/usr/share/dict/words` — so nothing
//! is downloaded and nothing is bundled. A plain list has no affix rules,
//! so a few regular English suffixes (-s, -ed, -ing, -ly …) are stripped
//! before a word is called wrong; the cost is that a non-word made of a
//! real stem and a real suffix passes. Hunspell dictionaries, with real
//! affix rules and other languages, are the planned second cut.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use wp_core::model::Item;

/// Contractions, which Webster's Second does not list.
const CONTRACTIONS: &[&str] = &[
    "aren't", "can't", "couldn't", "didn't", "doesn't", "don't", "hadn't", "hasn't", "haven't", "he'd", "he'll", "he's", "i'd", "i'll", "i'm", "i've", "isn't", "it'd", "it'll", "it's", "let's", "mightn't", "mustn't", "needn't", "o'clock", "shan't", "she'd", "she'll", "she's", "shouldn't", "that'd", "that'll", "that's", "there'd", "there'll", "there's", "they'd", "they'll", "they're", "they've", "wasn't", "we'd", "we'll", "we're", "we've", "weren't", "what's", "where's", "who'd", "who'll", "who's", "won't", "wouldn't", "you'd", "you'll", "you're", "you've",
];

/// Regular suffixes tried, in order, when a word is not listed: the suffix
/// and what replaces it (`""` = nothing; `"y"` puts back an *-ies* stem).
const SUFFIXES: &[(&str, &[&str])] = &[
    ("iest", &["y"]),
    ("ier", &["y"]),
    ("ies", &["y"]),
    ("ied", &["y"]),
    ("ily", &["y"]),
    ("ing", &["", "e"]),
    ("ness", &[""]),
    ("ment", &[""]),
    ("est", &["", "e"]),
    ("ed", &["", "e"]),
    ("es", &[""]),
    ("er", &["", "e"]),
    ("ly", &[""]),
    ("s", &[""]),
];

const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz'";

/// A word list: the words as listed (proper nouns keep their capital) and
/// lowercased, so `The` and `paris` both pass.
pub struct Dictionary {
    exact: HashSet<String>,
    lower: HashSet<String>,
    /// Words listed (the built-in contractions not counted).
    count: usize,
    /// Where it came from, for the message.
    pub source: String,
}

impl Dictionary {
    pub fn from_words<I, S>(words: I, source: &str) -> Dictionary
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut d = Dictionary { exact: HashSet::new(), lower: HashSet::new(), count: 0, source: source.to_string() };
        d.extend(words);
        d.lower.extend(CONTRACTIONS.iter().map(|c| c.to_string()));
        d
    }

    pub fn extend<I, S>(&mut self, words: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for w in words {
            let w = w.as_ref().trim();
            if w.is_empty() || w.starts_with('#') {
                continue;
            }
            self.lower.insert(w.to_lowercase());
            if self.exact.insert(w.to_string()) {
                self.count += 1;
            }
        }
    }

    /// The first word list found: `override_path` from the config, then
    /// `dictionary.txt` in the config directory, then the system's. The
    /// system's proper names come along when there are some.
    pub fn load(override_path: &str) -> Result<Dictionary, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !override_path.trim().is_empty() {
            candidates.push(crate::app::expand_path(override_path.trim()));
        }
        let own = crate::config::config_dir().join("dictionary.txt");
        candidates.push(own.clone());
        candidates.push(PathBuf::from("/usr/share/dict/words"));
        candidates.push(PathBuf::from("/usr/dict/words"));
        for p in &candidates {
            if let Ok(text) = std::fs::read_to_string(p) {
                let mut d = Dictionary::from_words(text.lines(), &p.display().to_string());
                if let Ok(names) = std::fs::read_to_string("/usr/share/dict/propernames") {
                    d.extend(names.lines());
                }
                return Ok(d);
            }
        }
        Err(format!("no word list found — put one word per line at {} (or set [spell] dictionary in config.toml)", own.display()))
    }

    pub fn len(&self) -> usize {
        self.count
    }

    /// Listed as is, or lowercased.
    fn listed(&self, w: &str) -> bool {
        self.exact.contains(w) || self.lower.contains(&w.to_lowercase())
    }

    /// Listed, or a listed word plus a regular suffix or a possessive.
    pub fn knows(&self, w: &str) -> bool {
        if self.listed(w) {
            return true;
        }
        let lw = w.to_lowercase();
        let lw = lw.trim_end_matches('\'');
        if let Some(base) = lw.strip_suffix("'s") {
            return self.listed(base) || self.by_suffix(base);
        }
        self.by_suffix(lw)
    }

    fn by_suffix(&self, lw: &str) -> bool {
        for (suffix, replacements) in SUFFIXES {
            let Some(base) = lw.strip_suffix(suffix) else { continue };
            for r in replacements.iter() {
                let cand = format!("{}{}", base, r);
                if cand.chars().count() >= 3 && self.listed(&cand) {
                    return true;
                }
            }
            // A doubled final consonant: stopped → stop, running → run.
            let b: Vec<char> = base.chars().collect();
            if b.len() >= 4 && b[b.len() - 1] == b[b.len() - 2] && !"aeiou".contains(b[b.len() - 1]) {
                let short: String = b[..b.len() - 1].iter().collect();
                if self.listed(&short) {
                    return true;
                }
            }
        }
        false
    }

    /// Up to `max` words one edit away that `known` accepts, then — when
    /// that gives fewer than three — listed words two edits away. Nearest
    /// first: same first letter, same length, then alphabetical. The
    /// original's capital is kept.
    pub fn suggest(&self, w: &str, max: usize, known: &dyn Fn(&str) -> bool) -> Vec<String> {
        let lw = w.to_lowercase();
        let first = lw.chars().next();
        let rank = |c: &String| (c.chars().next() != first, (c.chars().count() as i64 - lw.chars().count() as i64).abs(), c.clone());
        let e1 = edits1(&lw);
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = e1.iter().filter(|c| **c != lw && known(c) && seen.insert((*c).clone())).cloned().collect();
        out.sort_by_key(rank);
        if out.len() < 3 && lw.chars().count() <= 12 {
            let mut second: Vec<String> = Vec::new();
            'outer: for c in &e1 {
                for c2 in edits1(c) {
                    if c2 != lw && !seen.contains(&c2) && self.listed(&c2) {
                        seen.insert(c2.clone());
                        second.push(c2);
                        if second.len() >= 40 {
                            break 'outer;
                        }
                    }
                }
            }
            second.sort_by_key(rank);
            out.extend(second);
        }
        out.truncate(max);
        if w.chars().next().map_or(false, |c| c.is_uppercase()) {
            for s in &mut out {
                let mut cs = s.chars();
                *s = cs.next().map(|c| c.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default();
            }
        }
        out
    }
}

/// Every string one deletion, transposition, replacement or insertion away.
fn edits1(w: &str) -> Vec<String> {
    let cs: Vec<char> = w.chars().collect();
    let n = cs.len();
    let mut out = Vec::with_capacity(54 * n + 25);
    let s = |v: &[char]| v.iter().collect::<String>();
    for i in 0..n {
        let mut v = cs.clone();
        v.remove(i);
        out.push(s(&v));
    }
    for i in 0..n.saturating_sub(1) {
        let mut v = cs.clone();
        v.swap(i, i + 1);
        out.push(s(&v));
    }
    for i in 0..n {
        for l in LETTERS.chars() {
            if l != cs[i] {
                let mut v = cs.clone();
                v[i] = l;
                out.push(s(&v));
            }
        }
    }
    for i in 0..=n {
        for l in LETTERS.chars() {
            let mut v = cs.clone();
            v.insert(i, l);
            out.push(s(&v));
        }
    }
    out
}

/// The words of a paragraph as `(start, end, word)` item ranges, `end`
/// exclusive. Letters and inner apostrophes make a word; zero-width codes
/// (formatting, bookmarks, opaque XML) are transparent, so a word bolded
/// in the middle is still one word; anything else separates.
pub fn words(items: &[Item]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    // The word so far: its text and the item index of each character.
    let mut cur: Option<(String, Vec<usize>)> = None;
    let finish = |cur: &mut Option<(String, Vec<usize>)>, out: &mut Vec<(usize, usize, String)>| {
        if let Some((mut w, mut idx)) = cur.take() {
            while w.ends_with('\'') {
                w.pop();
                idx.pop();
            }
            if let (Some(&a), Some(&b)) = (idx.first(), idx.last()) {
                out.push((a, b + 1, w));
            }
        }
    };
    for (i, it) in items.iter().enumerate() {
        match it {
            Item::Char(c) if c.is_alphabetic() => {
                let (w, idx) = cur.get_or_insert((String::new(), Vec::new()));
                w.push(*c);
                idx.push(i);
            }
            Item::Char(c) if (*c == '\'' || *c == '’') && cur.is_some() => {
                let (w, idx) = cur.as_mut().unwrap();
                w.push('\'');
                idx.push(i);
            }
            Item::Code(code) if code.is_zero_width() => {}
            _ => finish(&mut cur, &mut out),
        }
    }
    finish(&mut cur, &mut out);
    out
}

/// The text of a word's items (the characters, codes dropped).
pub fn word_text(items: &[Item]) -> String {
    items.iter().filter_map(|it| it.as_char()).map(|c| if c == '’' { '\'' } else { c }).collect()
}

fn hash_items(items: &[Item]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.hash(&mut h);
    h.finish()
}

fn read_list(p: &Path) -> HashSet<String> {
    std::fs::read_to_string(p).map(|t| t.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()).unwrap_or_default()
}

/// The checker the app holds: the dictionary (loaded on first use), the
/// user's and the project's word lists, the words ignored this session,
/// and a per-paragraph cache of what is wrong.
pub struct Checker {
    /// Misspellings are looked for and underlined.
    pub enabled: bool,
    dictionary_path: String,
    dict: Option<Dictionary>,
    tried: bool,
    /// Why there is no dictionary, once loading was tried.
    pub error: Option<String>,
    /// The user's own words, one per line; None keeps additions in memory
    /// (headless tests).
    pub user_file: Option<PathBuf>,
    user: HashSet<String>,
    /// `.wp-words` in the document's directory or one above it.
    project_file: Option<PathBuf>,
    project: HashSet<String>,
    ignored: HashSet<String>,
    /// Paragraph index → (hash of its items, misspelled ranges).
    cache: HashMap<usize, (u64, Vec<(usize, usize)>)>,
}

impl Checker {
    pub fn new(enabled: bool, dictionary_path: &str) -> Checker {
        Checker {
            enabled,
            dictionary_path: dictionary_path.to_string(),
            dict: None,
            tried: false,
            error: None,
            user_file: Some(crate::config::config_dir().join("words.txt")),
            user: HashSet::new(),
            project_file: None,
            project: HashSet::new(),
            ignored: HashSet::new(),
            cache: HashMap::new(),
        }
    }

    /// Use `d` instead of the machine's word list (tests).
    #[cfg(test)]
    pub fn set_dictionary(&mut self, d: Dictionary) {
        self.dict = Some(d);
        self.tried = true;
        self.error = None;
        self.cache.clear();
    }

    /// Load the dictionary and the user's list the first time; whether
    /// there is a dictionary to check against.
    pub fn ready(&mut self) -> bool {
        if !self.tried {
            self.tried = true;
            match Dictionary::load(&self.dictionary_path) {
                Ok(d) => self.dict = Some(d),
                Err(e) => self.error = Some(e),
            }
            if let Some(p) = &self.user_file {
                self.user = read_list(p);
            }
        }
        self.dict.is_some()
    }

    /// How many words the dictionary has, and where it came from.
    pub fn describe(&self) -> String {
        match &self.dict {
            Some(d) => format!("{} words from {}", d.len(), d.source),
            None => "no dictionary".into(),
        }
    }

    /// The document moved: find its project's `.wp-words`, walking up from
    /// `dir`, and reload the list if it is a different file.
    pub fn set_project_dir(&mut self, dir: Option<&Path>) {
        let mut found = None;
        let mut d = dir.map(|d| d.to_path_buf());
        while let Some(cur) = d {
            let p = cur.join(".wp-words");
            if p.is_file() {
                found = Some(p);
                break;
            }
            d = cur.parent().map(|p| p.to_path_buf());
        }
        if found != self.project_file {
            self.project = found.as_ref().map(|p| read_list(p)).unwrap_or_default();
            self.project_file = found;
            self.cache.clear();
        }
    }

    /// In the dictionary or one of the lists.
    pub fn known(&self, w: &str) -> bool {
        let lw = w.to_lowercase();
        self.user.contains(&lw) || self.project.contains(&lw) || self.ignored.contains(&lw) || self.dict.as_ref().map_or(true, |d| d.knows(w))
    }

    /// The misspelled ranges of paragraph `pi`, from the cache when its
    /// items are unchanged. Empty when checking is off or has no
    /// dictionary.
    pub fn ranges(&mut self, pi: usize, items: &[Item]) -> Vec<(usize, usize)> {
        if !self.enabled || !self.ready() {
            return Vec::new();
        }
        let h = hash_items(items);
        if let Some((hh, r)) = self.cache.get(&pi) {
            if *hh == h {
                return r.clone();
            }
        }
        let r = self.flag(items);
        self.cache.insert(pi, (h, r.clone()));
        r
    }

    /// What is wrong in `items`: words that are not known, leaving alone
    /// single letters, words in capitals or CamelCase, and anything glued
    /// to a digit, an `@`, a `/` or a dot — a URL, an address, `3rd`.
    fn flag(&self, items: &[Item]) -> Vec<(usize, usize)> {
        let before = |i: usize| items[..i].iter().rev().find_map(|it| it.as_char());
        let after = |i: usize| items[i..].iter().find_map(|it| it.as_char());
        let after2 = |i: usize| items[i..].iter().filter_map(|it| it.as_char()).nth(1);
        let mut out = Vec::new();
        for (a, b, w) in words(items) {
            if w.chars().count() < 2 || w.chars().skip(1).any(|c| c.is_uppercase()) {
                continue;
            }
            let glued = |c: Option<char>| c.map_or(false, |c| c.is_ascii_digit() || matches!(c, '@' | '/' | '.'));
            if glued(before(a)) || after(b).map_or(false, |c| c.is_ascii_digit() || matches!(c, '@' | '/')) || (after(b) == Some('.') && after2(b).map_or(false, |c| c.is_alphabetic())) {
                continue;
            }
            if !self.known(&w) {
                out.push((a, b));
            }
        }
        out
    }

    /// Accept `w` for the rest of the session.
    pub fn ignore(&mut self, w: &str) {
        self.ignored.insert(w.to_lowercase());
        self.cache.clear();
    }

    /// Put `w` on the user's list, and in the list's file.
    pub fn add(&mut self, w: &str) -> Result<(), String> {
        self.user.insert(w.to_lowercase());
        self.cache.clear();
        if let Some(p) = &self.user_file {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let mut text = std::fs::read_to_string(p).unwrap_or_default();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(w);
            text.push('\n');
            std::fs::write(p, text).map_err(|e| format!("could not write {}: {}", p.display(), e))?;
        }
        Ok(())
    }

    pub fn suggest(&self, w: &str, max: usize) -> Vec<String> {
        match &self.dict {
            Some(d) => d.suggest(w, max, &|c| self.known(c)),
            None => Vec::new(),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_core::model::{Attr, AttrKind, Code};

    fn dict() -> Dictionary {
        Dictionary::from_words(["the", "cat", "cats", "run", "stop", "make", "happy", "quick", "test", "spelling", "Paris", "use", "this", "is", "of", "a", "item", "at", "and"], "test")
    }

    fn items(s: &str) -> Vec<Item> {
        s.chars().map(Item::Char).collect()
    }

    #[test]
    fn suffixes_possessives_and_contractions() {
        let d = dict();
        for w in ["cat", "Cat", "cats", "The", "paris", "running", "stopped", "making", "happily", "happiest", "quicker", "quickly", "cat's", "cats'", "don't", "Don't", "used"] {
            assert!(d.knows(w), "{}", w);
        }
        for w in ["teh", "runnning", "catz", "xyz"] {
            assert!(!d.knows(w), "{}", w);
        }
        // A real stem plus a real suffix passes: the price of a plain list.
        assert!(d.knows("runing"));
        // The stem must be a real word of some length: "ing" and "es" are not.
        assert!(!d.knows("ing"));
    }

    /// The machine's own word list: `cargo test -p wp system_dictionary --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn system_dictionary() {
        let t = std::time::Instant::now();
        let d = Dictionary::load("").unwrap();
        println!("{} words from {} in {:?}", d.len(), d.source, t.elapsed());
        for w in ["the", "running", "computers", "doesn't", "documents", "Paris", "quickly", "happiest", "stopped", "terminal's", "terminal", "paragraphs"] {
            assert!(d.knows(w), "{}", w);
        }
        for w in ["teh", "recieve", "seperate", "definately", "occurence"] {
            assert!(!d.knows(w), "{}", w);
        }
        let known = |w: &str| d.knows(w);
        let t = std::time::Instant::now();
        let s = d.suggest("recieve", 5, &known);
        println!("recieve → {:?} in {:?}", s, t.elapsed());
        assert!(s.contains(&"receive".to_string()));
        let t = std::time::Instant::now();
        let s = d.suggest("definately", 5, &known);
        println!("definately → {:?} in {:?}", s, t.elapsed());
        assert!(s.contains(&"definitely".to_string()));
    }

    #[test]
    fn suggestions_are_near_and_keep_the_capital() {
        let d = dict();
        let known = |w: &str| d.knows(w);
        assert_eq!(d.suggest("teh", 5, &known)[0], "the");
        assert_eq!(d.suggest("Tset", 5, &known)[0], "Test");
        assert!(d.suggest("speling", 5, &known).contains(&"spelling".to_string()));
        // Two edits away, found when one edit gives nothing.
        assert!(d.suggest("spelnig", 5, &known).contains(&"spelling".to_string()));
        assert!(d.suggest("qqqqqq", 5, &known).is_empty());
    }

    #[test]
    fn words_span_codes_and_stop_at_everything_else() {
        let mut it = items("It's a ");
        it.push(Item::Char('b'));
        it.push(Item::Code(Code::On(Attr::Bold(true))));
        it.push(Item::Char('o'));
        it.push(Item::Code(Code::Off(AttrKind::Bold)));
        it.extend(items("ld-faced dogs' tab"));
        it.push(Item::Code(Code::Tab));
        it.extend(items("end’s"));
        let w = words(&it);
        let texts: Vec<&str> = w.iter().map(|(_, _, t)| t.as_str()).collect();
        assert_eq!(texts, ["It's", "a", "bold", "faced", "dogs", "tab", "end's"]);
        // The bolded word's range covers the codes inside it.
        let (a, b, _) = w[2];
        assert_eq!(word_text(&it[a..b]), "bold");
        // A trailing apostrophe is left out of the range.
        let (a, b, _) = w[4];
        assert_eq!(word_text(&it[a..b]), "dogs");
    }

    #[test]
    fn flags_unknown_words_but_not_names_numbers_or_addresses() {
        let mut c = Checker::new(true, "");
        c.user_file = None;
        c.set_dictionary(dict());
        let it = items("This is a tset of speling, NASA's CamelCase 3rd item at a@b.cat and www.cat.com; the catz.");
        let flagged: Vec<String> = c.ranges(0, &it).into_iter().map(|(a, b)| word_text(&it[a..b])).collect();
        assert_eq!(flagged, ["tset", "speling", "catz"]);
        // Ignore and Add take effect at once; the cache follows the text.
        c.ignore("tset");
        c.add("speling").unwrap();
        let flagged: Vec<String> = c.ranges(0, &it).into_iter().map(|(a, b)| word_text(&it[a..b])).collect();
        assert_eq!(flagged, ["catz"]);
        assert!(c.suggest("catz", 3).contains(&"cats".to_string()));
        c.enabled = false;
        assert!(c.ranges(0, &it).is_empty());
    }
}
