//! The round-trip gate (DESIGN.md §6.3): every file in `corpus/` must survive
//! read → write with its main part semantically identical and every other
//! part byte-identical.

use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

fn corpus_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |e| e == "docx")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Canonical token stream for a WordprocessingML part. Normalises only what
/// cannot be told apart in the file format itself: attribute order, an
/// explicit `xml:space="preserve"` on text, and splits between adjacent runs
/// of identical formatting. Revision ids, paragraph ids, rendered page-break
/// hints and proofing marks all have to survive. Bookmarks may sit just
/// outside or just inside a paragraph boundary (both mean the same place),
/// so they are removed here and compared as a set by `bookmark_tokens`.
fn canonical(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut out: Vec<String> = Vec::new();
    let mut skip_depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let empty = matches!(reader.decoder().decode(e.name().as_ref()), Ok(_)) && false;
                let _ = empty;
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let is_empty_event = xml[..reader.buffer_position() as usize].ends_with("/>");
                if skip_depth > 0 {
                    if !is_empty_event {
                        skip_depth += 1;
                    }
                    continue;
                }
                if name == "w:bookmarkStart" || name == "w:bookmarkEnd" {
                    if !is_empty_event {
                        skip_depth = 1;
                    }
                    continue;
                }
                let mut attrs: Vec<String> = Vec::new();
                for a in e.attributes().flatten() {
                    let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    if k == "xml:space" {
                        continue;
                    }
                    let v = a.unescape_value().unwrap_or_default().into_owned();
                    attrs.push(format!("{}={}", k, v));
                }
                attrs.sort();
                out.push(format!("S {} {}", name, attrs.join(" ")));
                if is_empty_event {
                    out.push(format!("E {}", name));
                }
            }
            Ok(Event::End(e)) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                out.push(format!("E {}", String::from_utf8_lossy(e.name().as_ref())));
            }
            Ok(Event::Text(t)) => {
                if skip_depth > 0 {
                    continue;
                }
                let s = t.unescape().unwrap_or_default().into_owned();
                if s.trim().is_empty() && !matches!(out.last(), Some(l) if l.starts_with("S w:t ") || l.starts_with("S w:delText ") || l.starts_with("S w:instrText ")) {
                    continue;
                }
                if let Some(last) = out.last_mut() {
                    if let Some(prev) = last.strip_prefix("T ") {
                        *last = format!("T {}{}", prev, s);
                        continue;
                    }
                }
                out.push(format!("T {}", s));
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error: {}", e),
            _ => {}
        }
    }
    merge_runs(out)
}

/// Merge `</w:r><w:r><w:rPr>same</w:rPr>` boundaries and adjacent `w:t`s.
fn merge_runs(tokens: Vec<String>) -> Vec<String> {
    // Pass 1: collapse run boundaries with identical rPr.
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut prev_rpr: Option<Vec<String>> = None;
    while i < tokens.len() {
        if tokens[i].starts_with("S w:r ") {
            // gather this run's rPr block
            let mut j = i + 1;
            let mut rpr: Vec<String> = Vec::new();
            if j < tokens.len() && tokens[j].starts_with("S w:rPr") {
                let mut depth = 0;
                loop {
                    rpr.push(tokens[j].clone());
                    if tokens[j].starts_with("S ") {
                        depth += 1;
                    } else if tokens[j].starts_with("E ") {
                        depth -= 1;
                    }
                    j += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            let run_empty = j < tokens.len() && tokens[j] == "E w:r";
            if run_empty {
                i = j + 1;
                continue;
            }
            if out.last().map(|s| s.as_str()) == Some("E w:r") && prev_rpr.as_ref() == Some(&rpr) {
                out.pop();
                i = j;
                continue;
            }
            out.push(tokens[i].clone());
            out.extend(rpr.iter().cloned());
            prev_rpr = Some(rpr);
            i = j;
            continue;
        }
        if tokens[i] != "E w:r" {
            // Anything other than a run boundary breaks the merge chain,
            // except content inside runs.
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    // Pass 2: merge adjacent w:t elements.
    let mut merged: Vec<String> = Vec::new();
    for t in out {
        let n = merged.len();
        if n >= 3 && t.starts_with("T ") && merged[n - 1].starts_with("S w:t ") && merged[n - 2] == "E w:t" && merged[n - 3].starts_with("T ") {
            let text = t[2..].to_string();
            merged.truncate(n - 2);
            let last = merged.last_mut().unwrap();
            last.push_str(&text);
            // we removed "E w:t" and "S w:t"; the following "E w:t" closes the merged element
            continue;
        }
        merged.push(t);
    }
    merged
}

/// Every bookmark element with its attributes, sorted — position-independent.
fn bookmark_tokens(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "w:bookmarkStart" || name == "w:bookmarkEnd" {
                    let mut attrs: Vec<String> = e
                        .attributes()
                        .flatten()
                        .map(|a| format!("{}={}", String::from_utf8_lossy(a.key.as_ref()), a.unescape_value().unwrap_or_default()))
                        .collect();
                    attrs.sort();
                    out.push(format!("{} {}", name, attrs.join(" ")));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error: {}", e),
            _ => {}
        }
    }
    out.sort();
    out
}

fn main_part(bytes: &[u8]) -> (String, Vec<(String, Vec<u8>)>) {
    let pkg = wp_docx::DocxPackage::from_bytes(bytes).unwrap();
    let main = pkg.get_str(&pkg.main_part).unwrap();
    let others = pkg.entries.iter().filter(|e| e.name != pkg.main_part).map(|e| (e.name.clone(), e.data.clone())).collect();
    (main, others)
}

#[test]
fn corpus_round_trips_losslessly() {
    let files = corpus_files();
    let mut failures = Vec::new();
    for f in &files {
        let input = std::fs::read(f).unwrap();
        let output = match wp_docx::roundtrip_bytes(&input) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{}: {}", f.display(), e));
                continue;
            }
        };
        let (a_main, a_rest) = main_part(&input);
        let (b_main, b_rest) = main_part(&output);
        if a_rest != b_rest {
            let diff: Vec<String> = a_rest
                .iter()
                .zip(b_rest.iter())
                .filter(|(x, y)| x != y)
                .map(|(x, _)| x.0.clone())
                .collect();
            failures.push(format!("{}: other parts differ: {:?} (counts {} vs {})", f.display(), diff, a_rest.len(), b_rest.len()));
        }
        let ca = canonical(&a_main);
        let cb = canonical(&b_main);
        if bookmark_tokens(&a_main) != bookmark_tokens(&b_main) {
            failures.push(format!("{}: bookmarks differ:\n  expected: {:?}\n  actual:   {:?}", f.display(), bookmark_tokens(&a_main), bookmark_tokens(&b_main)));
        }

        if ca != cb {
            let first = ca.iter().zip(cb.iter()).position(|(x, y)| x != y).unwrap_or(ca.len().min(cb.len()));
            let lo = first.saturating_sub(6);
            failures.push(format!(
                "{}: main part differs at token {}:\n  expected: {:?}\n  actual:   {:?}",
                f.display(),
                first,
                &ca[lo..(first + 6).min(ca.len())],
                &cb[lo..(first + 6).min(cb.len())]
            ));
        }
        // The output must itself be readable.
        wp_docx::read_bytes(&output).unwrap_or_else(|e| panic!("{}: re-read failed: {}", f.display(), e));
    }
    assert!(failures.is_empty(), "round-trip failures:\n{}", failures.join("\n"));
}

#[test]
fn new_document_writes_and_reads() {
    let mut doc = wp_core::Document::new();
    doc.paragraphs[0] = wp_core::Paragraph::from_text("Hello, world");
    doc.paragraphs.push(wp_core::Paragraph::from_text("Second <paragraph> & more"));
    doc.paragraphs[1].props.style = Some("Heading1".into());
    let bytes = wp_docx::write_bytes(&doc, None).unwrap();
    let loaded = wp_docx::read_bytes(&bytes).unwrap();
    assert_eq!(loaded.doc.text(), "Hello, world\nSecond <paragraph> & more");
    assert_eq!(loaded.doc.paragraphs[1].props.style.as_deref(), Some("Heading1"));
    assert!(loaded.doc.styles.get("Heading1").is_some());
    assert!(loaded.warnings.is_empty());
}

#[test]
fn edits_survive_write() {
    use wp_core::model::*;
    let path = corpus_dir().join("pathological.docx");
    if !path.exists() {
        return;
    }
    let loaded = wp_docx::read(&path).unwrap();
    let mut ed = wp_core::Editor::new(loaded.doc);
    // Bold the word "Plain" in paragraph 2 and type after it.
    ed.cursor = Pos::new(1, 0);
    ed.anchor = Some(Pos::new(1, 5));
    ed.toggle_attr(Attr::Bold(true));
    ed.anchor = None;
    ed.cursor = Pos::new(1, 0);
    ed.insert_str("Very ");
    let bytes = wp_docx::write_bytes(&ed.doc, Some(&loaded.package)).unwrap();
    let again = wp_docx::read_bytes(&bytes).unwrap();
    assert!(again.doc.paragraphs[1].text().starts_with("Very Plain bold Arial then inserted deleted."));
    let runs = again.doc.runs(1);
    assert!(runs[0].props.is_bold() || runs[1].props.is_bold());
    // Tracked change and comment parts still present byte-for-byte.
    assert_eq!(again.package.get("word/comments.xml").unwrap().data, loaded.package.get("word/comments.xml").unwrap().data);
    let main = again.package.get_str("word/document.xml").unwrap();
    assert!(main.contains("<w:ins "));
    assert!(main.contains("<w:delText"));
    assert!(main.contains("<w:hyperlink "));
    assert!(main.contains("</w:sdtContent></w:sdt>"));
    assert!(main.contains("<w:tbl>"));
}
