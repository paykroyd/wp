//! Live verification of the Google Docs path (DESIGN.md §6a) against the
//! real API with the user's cached token. It creates a scratch document in
//! the account, seeds it with every construct the reader models, then runs
//! each kind of edit through the `Editor`, saves it as a diff and reads it
//! back, checking that the re-read document diffs to nothing against the
//! edited one — so the *next* save would send no requests. The document is
//! left in Drive (the client has no delete scope); its title starts with
//! "wp live test".
//!
//!     cargo test -p wp live_roundtrip -- --ignored --nocapture

use super::Client;
use serde_json::{json, Value};
use std::time::Instant;
use wp_core::model::*;
use wp_core::numbering::ListKind;
use wp_core::{Document, Editor};
use wp_gdoc::Loaded;

struct Live {
    c: Client,
    id: String,
}

impl Live {
    fn fetch(&mut self) -> (Loaded, Value) {
        let json = self.c.get_document(&self.id).expect("documents.get");
        let v: Value = serde_json::from_str(&json).unwrap();
        let l = wp_gdoc::read(&json).unwrap_or_else(|e| panic!("read: {}", e));
        (l, v)
    }

    /// Requests applied without the revision guard (seeding).
    fn apply(&mut self, reqs: Vec<Value>) -> Value {
        self.c
            .batch_update(&self.id, &json!({ "requests": reqs }))
            .unwrap_or_else(|e| panic!("seed batchUpdate failed: {}\n{}", e, serde_json::to_string_pretty(&reqs).unwrap()))
    }
}

/// Every text character of the body with its Docs index, in order.
fn body_chars(v: &Value) -> Vec<(i64, char)> {
    fn walk(content: &Value, out: &mut Vec<(i64, char)>) {
        for el in content.as_array().into_iter().flatten() {
            if let Some(p) = el.get("paragraph") {
                for e in p["elements"].as_array().into_iter().flatten() {
                    if let Some(tr) = e.get("textRun") {
                        let mut i = e["startIndex"].as_i64().unwrap();
                        for c in tr["content"].as_str().unwrap_or("").chars() {
                            out.push((i, c));
                            i += c.len_utf16() as i64;
                        }
                    }
                }
            } else if let Some(t) = el.get("table") {
                for row in t["tableRows"].as_array().into_iter().flatten() {
                    for cell in row["tableCells"].as_array().into_iter().flatten() {
                        walk(&cell["content"], out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&body_content(v), &mut out);
    out
}

fn body_content(v: &Value) -> Value {
    if v.get("body").is_some() {
        v["body"]["content"].clone()
    } else {
        v["tabs"][0]["documentTab"]["body"]["content"].clone()
    }
}

/// Docs index of the first character of `needle` in the body.
fn index_of(v: &Value, needle: &str) -> i64 {
    let chars = body_chars(v);
    let n: Vec<char> = needle.chars().collect();
    let at = chars.windows(n.len()).position(|w| w.iter().map(|c| c.1).eq(n.iter().copied())).unwrap_or_else(|| panic!("{:?} not in document", needle));
    chars[at].0
}

/// Docs index just past `needle`.
fn index_after(v: &Value, needle: &str) -> i64 {
    index_of(v, needle) + needle.encode_utf16().count() as i64
}

fn texts(doc: &Document) -> Vec<String> {
    doc.paragraphs.iter().map(|p| p.text()).collect()
}

/// Model position of the first character of `needle` (skipping codes).
fn pos_of(doc: &Document, needle: &str) -> Pos {
    let n: Vec<char> = needle.chars().collect();
    for (pi, p) in doc.paragraphs.iter().enumerate() {
        let chars: Vec<(usize, char)> = p.items.iter().enumerate().filter_map(|(i, it)| match it {
            Item::Char(c) => Some((i, *c)),
            _ => None,
        }).collect();
        if let Some(at) = chars.windows(n.len()).position(|w| w.iter().map(|c| c.1).eq(n.iter().copied())) {
            return Pos::new(pi, chars[at].0);
        }
    }
    panic!("{:?} not in model: {:?}", needle, texts(doc))
}

/// Model position just past the last character of `needle`.
fn pos_after(doc: &Document, needle: &str) -> Pos {
    let p = pos_of(doc, needle);
    let items = &doc.paragraphs[p.para].items;
    let mut left = needle.chars().count();
    let mut i = p.idx;
    while left > 0 {
        if matches!(items[i], Item::Char(_)) {
            left -= 1;
        }
        i += 1;
    }
    Pos::new(p.para, i)
}

fn select(ed: &mut Editor, needle: &str) {
    let (a, b) = (pos_of(&ed.doc, needle), pos_after(&ed.doc, needle));
    ed.move_to(a, false);
    ed.move_to(b, true);
}

const SEED: &str = "Live Test Heading\nThe results were published.\nFirst bullet\nSecond bullet\nStep one\nStep two\nTable intro\nNote here.\nBefore breakafter break\nCafé — naïve 😀 end.";

fn seed(live: &mut Live) -> Loaded {
    live.apply(vec![json!({ "insertText": { "location": { "index": 1 }, "text": SEED } })]);
    let (_, v) = live.fetch();
    // Descending index order, so every index is still valid when its
    // request runs.
    let reply = live.apply(vec![
        json!({ "insertPageBreak": { "location": { "index": index_after(&v, "Before break") } } }),
        json!({ "createFootnote": { "location": { "index": index_after(&v, "Note here.") } } }),
        json!({ "insertTable": { "rows": 2, "columns": 2, "location": { "index": index_after(&v, "Table intro") } } }),
        json!({ "createParagraphBullets": { "range": { "startIndex": index_of(&v, "Step one"), "endIndex": index_after(&v, "Step two") + 1 }, "bulletPreset": "NUMBERED_DECIMAL_ALPHA_ROMAN" } }),
        json!({ "createParagraphBullets": { "range": { "startIndex": index_of(&v, "First bullet"), "endIndex": index_after(&v, "Second bullet") + 1 }, "bulletPreset": "BULLET_DISC_CIRCLE_SQUARE" } }),
        json!({ "updateTextStyle": { "range": { "startIndex": index_of(&v, "published"), "endIndex": index_after(&v, "published") }, "textStyle": { "italic": true }, "fields": "italic" } }),
        json!({ "updateTextStyle": { "range": { "startIndex": index_of(&v, "results"), "endIndex": index_after(&v, "results") }, "textStyle": { "bold": true }, "fields": "bold" } }),
        json!({ "updateParagraphStyle": { "range": { "startIndex": index_of(&v, "Live Test Heading"), "endIndex": index_of(&v, "Live Test Heading") + 1 }, "paragraphStyle": { "namedStyleType": "HEADING_1" }, "fields": "namedStyleType" } }),
    ]);
    let fid = reply["replies"][1]["createFootnote"]["footnoteId"].as_str().expect("footnoteId in reply").to_string();
    // The footnote body and the cells: their indexes come from a fresh read.
    let (_, v) = live.fetch();
    let fn_start = v["footnotes"][&fid]["content"][0]["startIndex"].as_i64().unwrap_or(0);
    println!("footnote {} first paragraph starts at {}", fid, fn_start);
    // A new footnote holds one placeholder space; write the body before it,
    // then take the space out (a delete inside a footnote segment).
    let body_len = "Footnote body.".encode_utf16().count() as i64;
    let mut reqs = vec![
        json!({ "insertText": { "location": { "segmentId": fid, "index": fn_start }, "text": "Footnote body." } }),
        json!({ "deleteContentRange": { "range": { "segmentId": fid, "startIndex": fn_start + body_len, "endIndex": fn_start + body_len + 1 } } }),
    ];
    let body = body_content(&v);
    let table = body.as_array().unwrap().iter().find(|el| el.get("table").is_some()).expect("table in body");
    let mut cells: Vec<(i64, String)> = Vec::new();
    for (r, row) in table["table"]["tableRows"].as_array().unwrap().iter().enumerate() {
        for (c, cell) in row["tableCells"].as_array().unwrap().iter().enumerate() {
            cells.push((cell["content"][0]["startIndex"].as_i64().unwrap(), format!("{}{}", ['A', 'B'][c], r + 1)));
        }
    }
    cells.sort_by(|a, b| b.0.cmp(&a.0));
    for (i, t) in cells {
        reqs.push(json!({ "insertText": { "location": { "index": i }, "text": t } }));
    }
    live.apply(reqs);
    let (l, _) = live.fetch();
    l
}

fn check_seed(l: &Loaded) -> Vec<String> {
    let d = &l.doc;
    let mut bad = Vec::new();
    let t = texts(d);
    println!("seeded paragraphs: {:?}", t);
    println!("warnings: {:?}", l.warnings);
    // insertPageBreak adds a newline after the break, so "Before break"
    // ends its paragraph.
    let want = ["Live Test Heading", "The results were published.", "First bullet", "Second bullet", "Step one", "Step two", "Table intro", "A1", "B1", "A2", "B2", "", "Note here.", "Before break", "after break", "Café — naïve 😀 end."];
    if t != want {
        bad.push(format!("seed texts: got {:?}", t));
    }
    let mut expect = |ok: bool, what: &str| {
        if !ok {
            bad.push(format!("seed: {}", what));
        }
    };
    expect(d.paragraphs[0].props.style.as_deref() == Some("Heading1"), "heading style");
    let p1 = &d.paragraphs[1];
    expect(p1.items.iter().any(|it| matches!(it, Item::Code(Code::On(Attr::Bold(true))))), "bold on results");
    expect(p1.items.iter().any(|it| matches!(it, Item::Code(Code::On(Attr::Italic(true))))), "italic on published");
    let l2 = d.paragraphs[2].props.list;
    expect(l2.is_some() && d.numbering.is_bullet(l2.unwrap().num_id, 0), "bullet list");
    expect(d.paragraphs[3].props.list == l2, "second bullet in the same list");
    let l4 = d.paragraphs[4].props.list;
    expect(l4.is_some() && !d.numbering.is_bullet(l4.unwrap().num_id, 0), "numbered list");
    expect(d.paragraphs.get(7).and_then(|p| p.props.cell).is_some(), "cell tags");
    let fns: Vec<String> = d.footnotes.iter().map(|f| f.paragraphs.iter().map(|p| p.text()).collect::<Vec<_>>().join("|")).collect();
    expect(fns == ["Footnote body."], &format!("footnote body: {:?}", fns));
    expect(d.paragraphs[12].items.iter().any(|it| matches!(it, Item::Code(Code::Opaque(o)) if o.label == "Footnote")), "footnote reference");
    expect(d.paragraphs[13].items.last().map_or(false, |it| matches!(it, Item::Code(Code::PageBreak))), "page break ends its paragraph");
    bad
}

struct Round {
    name: &'static str,
    edit: fn(&mut Editor),
    /// The diff is expected to refuse the edit with this in its message.
    refused: Option<&'static str>,
}

fn rounds() -> Vec<Round> {
    vec![
        Round { name: "type in the middle of a paragraph", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "The results");
            ed.move_to(p, false);
            ed.insert_str(" really");
        } },
        Round { name: "type after an emoji", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "😀");
            ed.move_to(p, false);
            ed.insert_str("!");
        } },
        Round { name: "delete a range", refused: None, edit: |ed| {
            let r = Range::new(pos_of(&ed.doc, " really"), pos_after(&ed.doc, " really"));
            ed.delete_range(r);
        } },
        Round { name: "bold a word", refused: None, edit: |ed| {
            select(ed, "published");
            ed.toggle_attr(Attr::Bold(true));
        } },
        Round { name: "unbold a word", refused: None, edit: |ed| {
            select(ed, "results");
            ed.set_attr(AttrKind::Bold, None);
        } },
        Round { name: "italic, size and colour", refused: None, edit: |ed| {
            select(ed, "Table intro");
            ed.toggle_attr(Attr::Italic(true));
            ed.set_attr(AttrKind::Size, Some(Attr::Size(28)));
            ed.set_attr(AttrKind::Color, Some(Attr::Color(Rgb(255, 0, 0))));
        } },
        Round { name: "centre a paragraph", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Table intro");
            ed.move_to(p, false);
            ed.update_para_props(|p| p.align = Some(Align::Center));
        } },
        Round { name: "change the paragraph style", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Note here");
            ed.move_to(p, false);
            ed.set_style("Heading2");
        } },
        Round { name: "remove a bullet", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Second bullet");
            ed.move_to(p, false);
            ed.update_para_props(|p| p.list = None);
        } },
        Round { name: "nest a bullet", refused: Some("level"), edit: |ed| {
            let p = pos_of(&ed.doc, "First bullet");
            ed.move_to(p, false);
            ed.update_para_props(|p| p.list = p.list.map(|l| ListRef { num_id: l.num_id, level: 1 }));
        } },
        Round { name: "start a new bulleted list", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Second bullet");
            ed.move_to(p, false);
            let id = ed.doc.numbering.add_list(ListKind::Bullet);
            ed.update_para_props(move |p| p.list = Some(ListRef { num_id: id, level: 0 }));
        } },
        Round { name: "start a new numbered list", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Café");
            ed.move_to(p, false);
            let id = ed.doc.numbering.add_list(ListKind::Decimal);
            ed.update_para_props(move |p| p.list = Some(ListRef { num_id: id, level: 0 }));
        } },
        Round { name: "split a paragraph", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "The results");
            ed.move_to(p, false);
            ed.newline();
        } },
        Round { name: "join two paragraphs", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Step two");
            ed.move_to(Pos::new(p.para, 0), false);
            ed.backspace(false);
        } },
        Round { name: "append a paragraph at the end", refused: None, edit: |ed| {
            let e = ed.doc.end_pos();
            ed.move_to(e, false);
            ed.newline();
            ed.insert_str("Fin.");
        } },
        Round { name: "insert a paragraph at the start", refused: None, edit: |ed| {
            ed.move_to(Pos::new(0, 0), false);
            ed.insert_str("Top");
            ed.newline();
        } },
        Round { name: "type in a table cell", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "B2");
            ed.move_to(p, false);
            ed.insert_str("!");
        } },
        Round { name: "edit a footnote body", refused: None, edit: |ed| {
            ed.doc.footnotes[0].paragraphs[0].items.insert(0, Item::Char('*'));
        } },
        Round { name: "delete a whole paragraph", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Step one");
            ed.delete_range(Range::new(Pos::new(p.para, 0), Pos::new(p.para + 1, 0)));
        } },
        Round { name: "type before a page break", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "Before break");
            ed.move_to(p, false);
            ed.insert_str(" x");
        } },
        Round { name: "insert a page break", refused: None, edit: |ed| {
            let p = pos_after(&ed.doc, "Table intro");
            ed.move_to(p, false);
            ed.insert_code(Code::PageBreak);
        } },
        Round { name: "delete a page break", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Before break");
            let i = ed.doc.paragraphs[p.para].items.iter().position(|it| matches!(it, Item::Code(Code::PageBreak))).expect("page break after 'Before break'");
            ed.delete_item_at(Pos::new(p.para, i));
        } },
        Round { name: "delete the last paragraph", refused: None, edit: |ed| {
            let p = pos_of(&ed.doc, "Fin.");
            let prev_end = Pos::new(p.para - 1, ed.doc.paragraphs[p.para - 1].items.len());
            let e = ed.doc.end_pos();
            ed.delete_range(Range::new(prev_end, e));
        } },
    ]
}

/// Against the real API with the user's cached token:
/// `cargo test -p wp live_roundtrip -- --ignored --nocapture`.
#[test]
#[ignore]
fn live_roundtrip() {
    let (cfg, _) = crate::config::Config::load();
    let mut c = Client::new(cfg.google);
    assert!(c.signed_in(), "not signed in");
    let title = format!("wp live test {}", super::now());
    let id = c.create_document(&title).expect("documents.create");
    println!("created https://docs.google.com/document/d/{}/edit", id);
    let mut live = Live { c, id };
    let t = Instant::now();
    let l = seed(&mut live);
    println!("seeded in {:?}", t.elapsed());
    let mut failures = check_seed(&l);

    for r in rounds() {
        let t = Instant::now();
        let (l, _) = live.fetch();
        let mut ed = Editor::new(l.doc.clone());
        (r.edit)(&mut ed);
        ed.commit();
        let reqs = match (wp_gdoc::diff(&l.baseline, &ed.doc), r.refused) {
            (Err(e), Some(want)) if e.contains(want) => {
                println!("ok   {} (refused as designed: {})", r.name, e);
                continue;
            }
            (Ok(_), Some(want)) => {
                println!("FAIL {}: expected a refusal mentioning {:?}", r.name, want);
                failures.push(format!("{}: not refused", r.name));
                continue;
            }
            (Ok(r), None) => r,
            (Err(e), _) => {
                println!("FAIL {}: diff refused: {}", r.name, e);
                failures.push(format!("{}: diff refused: {}", r.name, e));
                continue;
            }
        };
        if reqs.is_empty() {
            println!("FAIL {}: the edit produced no requests", r.name);
            failures.push(format!("{}: no requests", r.name));
            continue;
        }
        let pretty = serde_json::to_string_pretty(&reqs).unwrap();
        let body = wp_gdoc::batch_update(&l.baseline, reqs);
        if let Err(e) = live.c.batch_update(&live.id, &body) {
            println!("FAIL {}: batchUpdate: {}\n{}", r.name, e, pretty);
            failures.push(format!("{}: batchUpdate: {}", r.name, e));
            continue;
        }
        let (l2, _) = live.fetch();
        let (got, want) = (texts(&l2.doc), texts(&ed.doc));
        if got != want {
            println!("FAIL {}: texts after save\n  got  {:?}\n  want {:?}\n{}", r.name, got, want, pretty);
            failures.push(format!("{}: texts differ", r.name));
            continue;
        }
        let fn_got: Vec<String> = l2.doc.footnotes.iter().map(|f| f.paragraphs[0].text()).collect();
        let fn_want: Vec<String> = ed.doc.footnotes.iter().map(|f| f.paragraphs[0].text()).collect();
        if fn_got != fn_want {
            println!("FAIL {}: footnotes after save: got {:?} want {:?}", r.name, fn_got, fn_want);
            failures.push(format!("{}: footnotes differ", r.name));
            continue;
        }
        // The app keeps the edited document only when the re-read agrees
        // with it in shape (list, footnote and header ids, paragraph count)
        // and otherwise reloads; a new list read back gets a different
        // `num_id`, so only the same-shape case can be checked for a quiet
        // second save.
        let same_shape = l2.baseline.lists == l.baseline.lists
            && l2.baseline.footnote_ids == l.baseline.footnote_ids
            && l2.baseline.header_ids == l.baseline.header_ids
            && l2.doc.paragraphs.len() == ed.doc.paragraphs.len()
            && l2.doc.paragraphs.iter().zip(&ed.doc.paragraphs).all(|(a, b)| a.props.list == b.props.list);
        if !same_shape {
            for (i, (a, b)) in l2.doc.paragraphs.iter().zip(&ed.doc.paragraphs).enumerate() {
                if a.props.list != b.props.list {
                    println!("     para {} list: docs {:?} (indent {:?}/{:?}) vs wp {:?} (indent {:?}/{:?})", i, a.props.list, a.props.indent_left, a.props.hanging, b.props.list, b.props.indent_left, b.props.hanging);
                }
            }
            if l2.baseline.lists != l.baseline.lists {
                println!("     list map changed: {:?} -> {:?}", l.baseline.lists, l2.baseline.lists);
            }
            let kinds = |d: &Document| -> Vec<Option<(bool, u8)>> { d.paragraphs.iter().map(|p| p.props.list.map(|l| (d.numbering.is_bullet(l.num_id, l.level), l.level))).collect() };
            if kinds(&l2.doc) != kinds(&ed.doc) {
                println!("FAIL {}: lists after save\n  got  {:?}\n  want {:?}\n{}", r.name, kinds(&l2.doc), kinds(&ed.doc), pretty);
                failures.push(format!("{}: lists differ", r.name));
            } else {
                println!("ok   {} ({:?}; the app would reload: shape changed)", r.name, t.elapsed());
            }
            continue;
        }
        match wp_gdoc::diff(&l2.baseline, &ed.doc) {
            Ok(left) if left.is_empty() => println!("ok   {} ({:?})", r.name, t.elapsed()),
            Ok(left) => {
                println!("FAIL {}: a second save would still send:\n{}\nafter sending:\n{}", r.name, serde_json::to_string_pretty(&left).unwrap(), pretty);
                failures.push(format!("{}: not idempotent ({} leftover)", r.name, left.len()));
            }
            Err(e) => {
                println!("FAIL {}: re-diff refused: {}", r.name, e);
                failures.push(format!("{}: re-diff refused: {}", r.name, e));
            }
        }
    }
    println!("document: https://docs.google.com/document/d/{}/edit", live.id);
    assert!(failures.is_empty(), "{} failure(s):\n  {}", failures.len(), failures.join("\n  "));
}

/// Sign in through the browser: prints the URL, waits for the redirect and
/// caches the token. `cargo test -p wp live_sign_in -- --ignored --nocapture`
#[test]
#[ignore]
fn live_sign_in() {
    let (cfg, _) = crate::config::Config::load();
    assert!(cfg.google.is_set(), "no [google] client in config.toml");
    let mut c = Client::new(cfg.google);
    let flow = c.begin_sign_in().expect("begin_sign_in");
    println!("SIGN_IN_URL {}", flow.url);
    c.finish_sign_in(flow, || false).expect("finish_sign_in");
    assert!(c.signed_in());
    println!("signed in");
}
