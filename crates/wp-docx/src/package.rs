//! The zip container. Every entry is kept in memory in its original order so
//! untouched parts can be written back byte-for-byte.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageEntry {
    pub name: String,
    pub data: Vec<u8>,
    pub deflated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocxPackage {
    pub entries: Vec<PackageEntry>,
    /// Path of the main document part, e.g. `word/document.xml`.
    pub main_part: String,
    /// Path of the styles part, if any.
    pub styles_part: Option<String>,
    pub numbering_part: Option<String>,
    /// Text before the root element of the main part (XML declaration).
    pub prolog: String,
    /// The `w:document` start tag verbatim (namespace declarations).
    pub root_tag: String,
    /// Anything between `<w:document>` and `<w:body>` (e.g. `w:background`).
    pub pre_body: String,
    pub theme_major: Option<String>,
    pub theme_minor: Option<String>,
    /// The body had no paragraphs at all when read (only a `w:sectPr`).
    pub empty_body: bool,
    /// Original `w:id` of each bookmark by name, so they are written back
    /// unchanged.
    pub bookmark_ids: HashMap<String, u32>,
}


impl DocxPackage {
    pub fn from_bytes(bytes: &[u8]) -> Result<DocxPackage> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("not a zip file")?;
        let mut entries = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            let mut data = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut data)?;
            entries.push(PackageEntry {
                name: f.name().to_string(),
                data,
                deflated: f.compression() != zip::CompressionMethod::Stored,
            });
        }
        let mut pkg = DocxPackage { entries, ..Default::default() };
        pkg.resolve_parts()?;
        Ok(pkg)
    }

    pub fn get(&self, name: &str) -> Option<&PackageEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        self.get(name).map(|e| String::from_utf8_lossy(&e.data).into_owned())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Replace or add an entry.
    pub fn put(&mut self, name: &str, data: Vec<u8>) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name) {
            e.data = data;
        } else {
            self.entries.push(PackageEntry { name: name.to_string(), data, deflated: true });
        }
    }

    /// Find the main part through the package relationships, falling back to
    /// the conventional name.
    fn resolve_parts(&mut self) -> Result<()> {
        let rels = self.get_str("_rels/.rels").unwrap_or_default();
        let main = find_rel_target(&rels, "officeDocument", "")
            .filter(|t| self.has(t))
            .unwrap_or_else(|| "word/document.xml".to_string());
        if !self.has(&main) {
            return Err(anyhow!("no main document part in package"));
        }
        let dir = main.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        let file = main.rsplit('/').next().unwrap_or(&main).to_string();
        let rels_name = if dir.is_empty() { format!("_rels/{}.rels", file) } else { format!("{}/_rels/{}.rels", dir, file) };
        let doc_rels = self.get_str(&rels_name).unwrap_or_default();
        self.styles_part = find_rel_target(&doc_rels, "/styles", &dir).filter(|t| self.has(t));
        self.numbering_part = find_rel_target(&doc_rels, "/numbering", &dir).filter(|t| self.has(t));
        self.main_part = main;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            for e in &self.entries {
                let method = if e.deflated { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
                let opts = zip::write::SimpleFileOptions::default().compression_method(method);
                zip.start_file(e.name.clone(), opts)?;
                zip.write_all(&e.data)?;
            }
            zip.finish()?;
        }
        Ok(buf.into_inner())
    }
}

/// Very small relationship lookup: finds the `Target` of the first
/// `Relationship` whose `Type` ends with `type_suffix`.
fn find_rel_target(rels_xml: &str, type_suffix: &str, base_dir: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(rels_xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Empty(e)) | Ok(quick_xml::events::Event::Start(e)) => {
                if e.local_name().as_ref() == b"Relationship" {
                    let mut ty = String::new();
                    let mut target = String::new();
                    let mut mode = String::new();
                    for a in e.attributes().flatten() {
                        let v = a.unescape_value().ok()?.into_owned();
                        match a.key.as_ref() {
                            b"Type" => ty = v,
                            b"Target" => target = v,
                            b"TargetMode" => mode = v,
                            _ => {}
                        }
                    }
                    if ty.ends_with(type_suffix) && mode != "External" {
                        let t = target.trim_start_matches('/');
                        return Some(if base_dir.is_empty() || target.starts_with('/') {
                            t.to_string()
                        } else {
                            format!("{}/{}", base_dir, t)
                        });
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}
