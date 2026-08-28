//! Edits as `batchUpdate` requests: the difference between the baseline the
//! reader recorded and the document as it is now, expressed as the minimal
//! text, style and paragraph operations, in descending index order so no
//! request shifts the indexes of the ones after it. Content nobody touched
//! produces no request and therefore keeps everything Docs holds for it.

use crate::project::*;
use crate::read::{BBlock, BPara, Baseline};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use wp_core::model::*;
use wp_core::Document;

/// Requests generated in one group are applied together, groups in
/// descending `key` order (then ascending `rank`, for groups at one index).
struct Group {
    key: i64,
    rank: u8,
    reqs: Vec<Value>,
}

struct Writer<'a> {
    doc: &'a Document,
    ctx: Ctx<'a>,
    tab_id: Option<&'a str>,
    groups: Vec<Group>,
    /// Footnotes still referenced from the body.
    referenced: BTreeSet<String>,
}

enum MBlock<'a> {
    Paras(Vec<&'a Paragraph>),
    Table(Vec<Vec<Vec<&'a Paragraph>>>),
}

const UNSUPPORTED_TABLES: &str = "adding, removing or reshaping a table is not supported for Google Docs yet";

/// The requests that turn the document Docs has (the baseline) into `doc`.
/// Empty when nothing changed.
pub fn diff(base: &Baseline, doc: &Document) -> Result<Vec<Value>, String> {
    let mut w = Writer { doc, ctx: Ctx { rels: &doc.extra_rels, footnote_ids: &base.footnote_ids }, tab_id: base.tab_id.as_deref(), groups: Vec::new(), referenced: BTreeSet::new() };
    // Body.
    let body = base.segments.first().ok_or("baseline has no body")?;
    let mblocks = model_blocks(&doc.paragraphs);
    if mblocks.len() != body.blocks.len() {
        return Err(UNSUPPORTED_TABLES.into());
    }
    for (bb, mb) in body.blocks.iter().zip(&mblocks) {
        match (bb, mb) {
            (BBlock::Paras(bp), MBlock::Paras(mp)) => w.region(bp, mp, None)?,
            (BBlock::Table { cells: bc, .. }, MBlock::Table(mc)) => {
                if bc.len() != mc.len() || bc.iter().zip(mc).any(|(r, s)| r.len() != s.len()) {
                    return Err(UNSUPPORTED_TABLES.into());
                }
                for (br, mr) in bc.iter().zip(mc) {
                    for (bcell, mcell) in br.iter().zip(mr) {
                        w.region(bcell, mcell, None)?;
                    }
                }
            }
            _ => return Err(UNSUPPORTED_TABLES.into()),
        }
    }
    // Footnote bodies whose reference survived.
    for f in &doc.footnotes {
        let gid = base.footnote_ids.get((f.id - 1).max(0) as usize).ok_or("a footnote created in wp (not yet supported for Google Docs)")?;
        if !w.referenced.contains(gid) {
            continue;
        }
        let Some(seg) = base.segments.iter().find(|s| s.id.as_deref() == Some(gid)) else { continue };
        let bp: Vec<BPara> = seg
            .blocks
            .iter()
            .flat_map(|b| match b {
                BBlock::Paras(v) => v.clone(),
                BBlock::Table { .. } => Vec::new(),
            })
            .collect();
        let mp: Vec<&Paragraph> = f.paragraphs.iter().collect();
        w.region(&bp, &mp, Some(gid))?;
    }
    let mut groups = std::mem::take(&mut w.groups);
    groups.sort_by(|a, b| b.key.cmp(&a.key).then(a.rank.cmp(&b.rank)));
    Ok(groups.into_iter().flat_map(|g| g.reqs).collect())
}

/// The `batchUpdate` request body, pinned to the revision that was read so
/// a concurrent edit is refused rather than clobbered.
pub fn batch_update(base: &Baseline, requests: Vec<Value>) -> Value {
    let mut body = json!({ "requests": requests });
    if !base.revision_id.is_empty() {
        body["writeControl"] = json!({ "requiredRevisionId": base.revision_id });
    }
    body
}

fn model_blocks(paras: &[Paragraph]) -> Vec<MBlock<'_>> {
    let mut out: Vec<MBlock> = Vec::new();
    let mut cur_table: Option<u32> = None;
    for p in paras {
        match p.props.cell {
            Some(c) => {
                if cur_table != Some(c.table) || !matches!(out.last(), Some(MBlock::Table(_))) {
                    out.push(MBlock::Table(Vec::new()));
                    cur_table = Some(c.table);
                }
                let Some(MBlock::Table(rows)) = out.last_mut() else { unreachable!() };
                while rows.len() <= c.row as usize {
                    rows.push(Vec::new());
                }
                let row = &mut rows[c.row as usize];
                while row.len() <= c.col as usize {
                    row.push(Vec::new());
                }
                row[c.col as usize].push(p);
            }
            None => {
                cur_table = None;
                match out.last_mut() {
                    Some(MBlock::Paras(v)) => v.push(p),
                    _ => out.push(MBlock::Paras(vec![p])),
                }
            }
        }
    }
    out
}

/// Longest common subsequence as index pairs, after trimming the common
/// prefix and suffix. Very large middles fall back to the trimmed matches
/// only (the caller pairs the rest positionally).
fn lcs_pairs<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let mut pairs: Vec<(usize, usize)> = (0..pre).map(|i| (i, i)).collect();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if !am.is_empty() && !bm.is_empty() && am.len() * bm.len() <= 4_000_000 {
        let (n, m) = (am.len(), bm.len());
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * (m + 1) + j] = if am[i] == bm[j] { t[(i + 1) * (m + 1) + j + 1] + 1 } else { t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                pairs.push((pre + i, pre + j));
                i += 1;
                j += 1;
            } else if t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.extend((0..suf).rev().map(|k| (a.len() - 1 - k, b.len() - 1 - k)));
    pairs
}

impl<'a> Writer<'a> {
    fn loc(&self, seg: Option<&str>, index: i64) -> Value {
        let mut l = json!({ "index": index });
        if let Some(s) = seg {
            l["segmentId"] = s.into();
        }
        if let Some(t) = self.tab_id {
            l["tabId"] = t.into();
        }
        l
    }

    fn range(&self, seg: Option<&str>, start: i64, end: i64) -> Value {
        let mut r = json!({ "startIndex": start, "endIndex": end });
        if let Some(s) = seg {
            r["segmentId"] = s.into();
        }
        if let Some(t) = self.tab_id {
            r["tabId"] = t.into();
        }
        r
    }

    fn group(&mut self, key: i64, rank: u8) -> &mut Vec<Value> {
        self.groups.push(Group { key, rank, reqs: Vec::new() });
        &mut self.groups.last_mut().unwrap().reqs
    }

    /// Diff one run of paragraphs that share a container: a stretch of body
    /// between tables, a table cell, or a footnote. The last paragraph's
    /// newline belongs to the container and is never deleted.
    fn region(&mut self, b: &[BPara], m: &[&Paragraph], seg: Option<&str>) -> Result<(), String> {
        if b.is_empty() || m.is_empty() {
            return Err(UNSUPPORTED_TABLES.into());
        }
        let bp: Vec<Proj> = b.iter().map(|p| project(&self.ctx, &p.para)).collect::<Result<_, _>>()?;
        let mp: Vec<Proj> = m.iter().map(|p| project(&self.ctx, p)).collect::<Result<_, _>>()?;
        for p in &mp {
            for u in &p.units {
                if let UnitKind::Footnote(g) = &u.kind {
                    self.referenced.insert(g.clone());
                }
            }
        }
        // Align paragraphs: equal ones first, then modified ones positionally.
        let mut pair: Vec<Option<usize>> = vec![None; b.len()];
        let mut inserts: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        let mut matches = lcs_pairs(&bp, &mp);
        matches.push((b.len(), m.len()));
        let (mut pb, mut pm) = (0usize, 0usize);
        let mut last_survivor: i64 = -1;
        for (bi, mi) in matches {
            let k = (bi - pb).min(mi - pm);
            for j in 0..k {
                let (bx, mx) = (pb + j, pm + j);
                let compatible = match (&bp[bx].raw, &mp[mx].raw) {
                    (None, None) => true,
                    (Some(x), Some(y)) => x == y,
                    _ => false,
                };
                if compatible {
                    pair[bx] = Some(mx);
                    last_survivor = bx as i64;
                } else {
                    inserts.entry(last_survivor).or_default().push(mx);
                }
            }
            for mx in pm + k..mi {
                inserts.entry(last_survivor).or_default().push(mx);
            }
            if bi < b.len() {
                pair[bi] = Some(mi);
                last_survivor = bi as i64;
            }
            pb = bi + 1;
            pm = mi + 1;
        }
        // Deletions, as runs. A run reaching the end of the region takes the
        // newline of the surviving paragraph before it instead of its own.
        let last = b.len() - 1;
        let mut restyle = vec![false; b.len()];
        let mut i = 0;
        while i < b.len() {
            if pair[i].is_some() {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < b.len() && pair[j].is_none() {
                j += 1;
            }
            if j == b.len() && bp[last].raw.is_none() {
                if i == 0 {
                    return Err("deleting every paragraph of a table cell or footnote is not supported".into());
                }
                if bp[i - 1].raw.is_some() {
                    return Err("deleting the paragraph after a preserved block at the end of its container is not supported".into());
                }
                let start = b[i - 1].end - 1;
                let r = self.range(seg, start, b[last].end - 1);
                self.group(b[i].start, 0).push(json!({ "deleteContentRange": { "range": r } }));
                restyle[i - 1] = true;
            } else {
                let r = self.range(seg, b[i].start, b[j - 1].end);
                self.group(b[i].start, 0).push(json!({ "deleteContentRange": { "range": r } }));
            }
            i = j;
        }
        // Insertions.
        for (anchor, mis) in &inserts {
            let (key, index, leading, src_list) = if *anchor < 0 {
                (b[0].start, b[0].start, false, bp[0].list)
            } else {
                let s = *anchor as usize;
                if bp[s].raw.is_some() {
                    (b[s].end, b[s].end, false, bp.get(s + 1).and_then(|p| p.list))
                } else {
                    let cur = if restyle[s] { bp[last].list } else { bp[s].list };
                    (b[s].end - 1, b[s].end - 1, true, cur)
                }
            };
            let mut texts: Vec<String> = Vec::new();
            for &mi in mis {
                if mp[mi].raw.is_some() {
                    return Err("inserting a preserved block is not supported".into());
                }
                let mut t = String::new();
                for u in &mp[mi].units {
                    match &u.kind {
                        UnitKind::Char(c) => t.push(*c),
                        UnitKind::PageBreak => return Err("a page break in a new paragraph is not supported for Google Docs yet (put it in an existing paragraph)".into()),
                        UnitKind::Footnote(_) => return Err("moving a footnote reference into a new paragraph is not supported for Google Docs yet".into()),
                        UnitKind::Object(_) => return Err("moving an image or other element into a new paragraph is not supported for Google Docs yet".into()),
                    }
                }
                texts.push(t);
            }
            let text = if leading { format!("\n{}", texts.join("\n")) } else { format!("{}\n", texts.join("\n")) };
            let mut reqs = vec![json!({ "insertText": { "location": self.loc(seg, index), "text": text } })];
            let mut start = if leading { index + 1 } else { index };
            for &mi in mis {
                let p = &mp[mi];
                for (off, len, sty) in style_runs(&p.units) {
                    reqs.push(json!({ "updateTextStyle": { "range": self.range(seg, start + off, start + off + len), "textStyle": sty.to_json(F_ALL), "fields": field_mask(F_ALL) } }));
                }
                let (ps, fields) = p.para.to_json(None).unwrap_or((json!({}), P_ALL.into()));
                reqs.push(json!({ "updateParagraphStyle": { "range": self.range(seg, start, start + 1), "paragraphStyle": ps, "fields": fields } }));
                self.bullets(&mut reqs, seg, start, src_list, p.list);
                start += p.text_len() + 1;
            }
            self.group(key, 1).extend(reqs);
        }
        // Paired paragraphs.
        for bi in 0..b.len() {
            let Some(mi) = pair[bi] else { continue };
            let (bpp, mpp) = (&bp[bi], &mp[mi]);
            if bpp.raw.is_some() {
                continue;
            }
            let base = b[bi].start;
            let mut reqs = Vec::new();
            let mut mapped: Vec<Option<usize>> = vec![None; mpp.units.len()];
            if bpp.units != mpp.units {
                // Text: align on unit kinds, ignoring style.
                let bk: Vec<&UnitKind> = bpp.units.iter().map(|u| &u.kind).collect();
                let mk: Vec<&UnitKind> = mpp.units.iter().map(|u| &u.kind).collect();
                let mut pairs = lcs_pairs(&bk, &mk);
                for (x, y) in &pairs {
                    mapped[*y] = Some(*x);
                }
                pairs.push((bk.len(), mk.len()));
                let boff: Vec<i64> = std::iter::once(0).chain(bpp.units.iter().scan(0, |a, u| { *a += u.len; Some(*a) })).collect();
                let mut hunks: Vec<(usize, usize, usize, usize)> = Vec::new();
                let (mut pbu, mut pmu) = (0, 0);
                for (x, y) in pairs {
                    if x > pbu || y > pmu {
                        hunks.push((pbu, x, pmu, y));
                    }
                    pbu = x + 1;
                    pmu = y + 1;
                }
                for (bs, be, ms, me) in hunks.into_iter().rev() {
                    let idx = base + boff[bs];
                    if be > bs {
                        reqs.push(json!({ "deleteContentRange": { "range": self.range(seg, idx, base + boff[be]) } }));
                    }
                    // Insert the pieces back to front so they land in order.
                    let mut pieces: Vec<Result<String, &'static str>> = Vec::new();
                    for u in &mpp.units[ms..me] {
                        match &u.kind {
                            UnitKind::Char(c) => match pieces.last_mut() {
                                Some(Ok(s)) => s.push(*c),
                                _ => pieces.push(Ok(c.to_string())),
                            },
                            UnitKind::PageBreak => pieces.push(Err("pb")),
                            UnitKind::Footnote(_) => return Err("moving a footnote reference is not supported for Google Docs yet".into()),
                            UnitKind::Object(_) => return Err("moving an image or other element is not supported for Google Docs yet".into()),
                        }
                    }
                    for piece in pieces.into_iter().rev() {
                        match piece {
                            Ok(s) => reqs.push(json!({ "insertText": { "location": self.loc(seg, idx), "text": s } })),
                            Err(_) => reqs.push(json!({ "insertPageBreak": { "location": self.loc(seg, idx) } })),
                        }
                    }
                }
                // Character formatting, on the paragraph as it now reads.
                let mut runs: Vec<(i64, i64, Fields, Sty)> = Vec::new();
                let mut off = 0;
                for (k, u) in mpp.units.iter().enumerate() {
                    if u.is_text() {
                        let need = match mapped[k] {
                            Some(j) => bpp.units[j].sty.diff(&u.sty),
                            None => F_ALL,
                        };
                        if need != 0 {
                            match runs.last_mut() {
                                Some((o, l, n, s)) if *o + *l == off && *n == need && *s == u.sty => *l += u.len,
                                _ => runs.push((off, u.len, need, u.sty.clone())),
                            }
                        }
                    }
                    off += u.len;
                }
                for (off, len, need, sty) in runs {
                    reqs.push(json!({ "updateTextStyle": { "range": self.range(seg, base + off, base + off + len), "textStyle": sty.to_json(need), "fields": field_mask(need) } }));
                }
            }
            // Paragraph formatting and bullets.
            let have = if restyle[bi] { None } else { Some(&bpp.para) };
            if let Some((ps, fields)) = mpp.para.to_json(have) {
                reqs.push(json!({ "updateParagraphStyle": { "range": self.range(seg, base, base + 1), "paragraphStyle": ps, "fields": fields } }));
            }
            let cur = if restyle[bi] { bp[last].list } else { bpp.list };
            self.bullets(&mut reqs, seg, base, cur, mpp.list);
            if !reqs.is_empty() {
                self.group(base, 2).extend(reqs);
            }
        }
        Ok(())
    }

    fn bullets(&self, reqs: &mut Vec<Value>, seg: Option<&str>, index: i64, cur: Option<ListRef>, want: Option<ListRef>) {
        let range = self.range(seg, index, index + 1);
        match want {
            Some(l) if cur != want => reqs.push(json!({ "createParagraphBullets": { "range": range, "bulletPreset": bullet_preset(self.doc, l) } })),
            None if cur.is_some() => reqs.push(json!({ "deleteParagraphBullets": { "range": range } })),
            _ => {}
        }
    }
}
