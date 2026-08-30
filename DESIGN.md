# `wp` — Engineering Design

**Status:** Draft 1 · **Date:** 2026-08-28 · **Spec:** [SPEC.md](./SPEC.md)

This document records the technical decisions behind `wp`. The spec says what
the product does; this says how it is built and why it is built that way. Where
a decision follows from a product principle, the principle is cited.

---

## 1. Language, runtime, and dependencies

**Rust, single static binary.** The spec's non-functional requirements — one
binary, no runtime, runs on a Raspberry Pi, under 250 MB on a 500-page document,
typing never stutters — are Rust's home ground, and `cargo install wp` is named
in the spec.

Dependencies are kept deliberately few. Every crate is a maintenance liability
in a project whose first principle is *never damage a document*.

| Purpose | Crate | Why this one |
|---|---|---|
| Terminal I/O | `crossterm` | Cross-platform, supports the kitty keyboard protocol (needed to distinguish `Shift+F8` from `F8` on modern terminals) |
| Screen buffer / diffing | `ratatui` | Cell-level diffing means only changed cells are written — matters over high-latency SSH |
| `.docx` container | `zip` | Zip read/write; we drive it ourselves to preserve untouched parts byte-for-byte |
| XML | `quick-xml` | Streaming pull parser; fast and doesn't build a DOM for the parts we skip |
| Config | `serde` + `toml` | Config file and macro files are TOML — human-readable per P0-30 |
| Unicode | `unicode-width`, `unicode-segmentation` | Cell width for display; grapheme/word boundaries for cursor movement and undo grouping |

Explicitly **not** used: any `.docx` library. Every one we evaluated reads a
document into its own model and writes it back from that model, which is
exactly the lossy round trip principle 1 forbids. We own the container and the
XML.

| JSON | `serde_json` | The Google Docs API speaks JSON (§6a) |

| HTTPS | `ureq` (rustls) | Blocking; used only by the Google Docs client (§6a.4) on the open/save path |

"No network access" (§8) holds by construction for everything but Google
Docs, which the user opts into by opening one.

---

## 2. Crate layout

```
wp/
├── Cargo.toml                 workspace
├── crates/
│   ├── wp-core/               document model, codes, styles, editing, undo,
│   │                          layout & pagination, font metrics, plain text
│   ├── wp-docx/               .docx read/write with opaque preservation
│   ├── wp-md/                 Markdown import/export (pulldown-cmark)
│   ├── wp-gdoc/               Google Docs API JSON reader and minimal-edit
│   │                          batchUpdate writer (§6a)
│   └── wp/                    the binary: terminal UI, keymaps, commands,
│                              palette, views, config, autosave
└── tools/
    ├── fontgen.py             generates embedded font-metric tables
    ├── make_corpus.py         generates the round-trip corpus
    └── gen_keybindings.py     regenerates KEYBINDINGS.md from the keymaps
```

`wp-core` has no terminal dependency and no knowledge of `.docx`. Everything in
it is testable with plain `cargo test` and no TTY (this includes lists,
numbering, search, and tables). `wp-docx` depends on `wp-core` only; `wp-md`
depends on both (footnotes are built as model paragraphs; a preserved table
block still needs `wp-docx` to read its cells for export). The binary depends
on all three.

---

## 3. The document model

This is the decision everything else rests on. Three requirements pull in
different directions:

1. **Reveal Codes** (P0-18, principle 5) wants formatting to be *things in the
   text* that can be pointed at and deleted.
2. **`.docx` fidelity** (P0-1/2, principle 1) wants a model that maps 1:1 onto
   WordprocessingML's paragraph/run structure, so nothing is lost translating.
3. **Pagination** (P0-14, principle 2) wants per-paragraph caching so an edit
   only re-lays-out the paragraph it touched.

### 3.1 Paragraphs of items

```rust
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    pub styles: StyleSheet,
    pub section: SectionProps,       // page size, margins, orientation (v0.1: one section)
    pub defaults: RunProps,          // document defaults (docDefaults in .docx)
}

pub struct Paragraph {
    pub props: ParaProps,            // style, alignment, indents, spacing, keep-with-next…
    pub items: Vec<Item>,
}

pub enum Item {
    Char(char),
    Code(Code),
}
```

A document is a list of paragraphs; a paragraph is a list of items; an item is
a character or a code. The cursor is `(paragraph index, item index)`. This is
WordPerfect's stream model with one structural concession to `.docx`: the hard
return is the paragraph boundary rather than a code in the stream. Reveal Codes
still draws it as `[HRt]`, and deleting it joins paragraphs, exactly as in WP.

### 3.2 Two kinds of code

**Character codes are paired spans**, WordPerfect-style: `[BOLD]…[bold]`,
`[Font:Arial]…[font]`, `[Size:14pt]…[size]`. They live in the item stream.
A run in `.docx` terms is simply a maximal stretch of characters with the same
set of open spans — computed by a single forward scan over the paragraph, which
is how both the layout engine and the `.docx` writer consume it. Spans never
cross a paragraph boundary; bold text across three paragraphs is three pairs.
This matches `.docx` (run properties are per-run, runs are per-paragraph) and
keeps every paragraph independently editable and cacheable.

**Paragraph codes are properties**, stored in `ParaProps`, and *displayed* in
Reveal Codes as codes at the start of the paragraph: `[Style:Heading 2]`,
`[Just:Center]`, `[Ln Spacing:1.5]`. Deleting one in Reveal Codes clears that
property. This is the one place the display lies about storage, and it lies in
the user's favour: `.docx` paragraph properties are per-paragraph, WordPerfect's
were "from here until changed", and per-paragraph is the semantics that survives
a round trip.

Codes the layout computes — `[SRt]` soft return, `[SPg]` soft page — are never
stored. Reveal Codes draws them from the layout result and refuses to edit them
(§6.3 of the spec).

### 3.3 The invariant: delete a code, its effect is gone

Every character code carries an `on: bool`. Deleting an `on` code deletes the
matching `off` (found by forward scan with depth counting), and vice versa.
There is no other way an attribute can be applied, so there is no way for
formatting to persist with its code removed. This is the whole of principle 5
and it is enforced by the model, not the UI.

### 3.4 Round-trip preservation

`Code::Opaque(OpaqueXml)` holds any run-level XML `wp` doesn't understand
(fields, comments anchors, tracked-change wrappers, drawings, math). It is
inert: layout treats it as zero width (or as a labelled placeholder box for
drawings, P0-25), editing moves it but never alters it, and the writer emits it
verbatim. `ParaProps` and `RunProps` each carry an `opaque: Vec<OpaqueXml>` for
the same purpose at the property level. Zip parts the reader doesn't parse
(media, fonts, `customXml/`, `comments.xml`…) are carried through byte-for-byte
by `wp-docx`. This is how "features `wp` doesn't support survive untouched"
(P0-2) is achieved without supporting them.

### 3.5 Units

Everything geometric is in **twips** (1/20 pt, `.docx`'s native unit) as `i32`.
Fixed-point avoids float drift across a 500-page pagination pass, and using the
file's unit means read and write are lossless by construction.

### 3.6 Memory budget

`Item` is 16 bytes. A 500-page document is ~1.5 M characters → 24 MB for the
stream, plus layout caches. Well under the 250 MB ceiling with room for undo.

### 3.7 Tables: cells are paragraphs, the grid is a property

A table is not a nested block. Its cells' paragraphs sit in the same flat
`Vec<Paragraph>` as everything else, in row-major order, each carrying
`props.cell = Some(CellRef { table, row, col })`. The grid — column widths,
row properties, per-cell span / vertical merge / width / shading, and the
verbatim `tblPr` / `trPr` / `tcPr` — lives in `Document::tables`, keyed by the
table id. This is WordPerfect's own layout of a table in the code stream
(`[Tbl Def]`, `[Row]`, `[Cell]`… `[Tbl Off]`), and it is what `.docx` writes
too once the `w:tbl`/`w:tr`/`w:tc` wrappers are peeled off.

What this buys:

- `Pos` is still `(paragraph, item)`. Cursor movement, selection, search and
  replace, undo, autosave, Reveal Codes and the `.docx` writer needed no new
  addressing scheme. Reveal Codes draws `[Tbl Def:3×4]`, `[Row]` and
  `[Cell:B2]` as pseudo-codes on the first paragraph of each cell, from the
  tags alone.
- Structural edits are sequences of the primitive ops: insert / delete a row
  or column is `InsertPara` / `RemovePara` plus `SetParaProps` to renumber
  the cells after it and one `SetTable` for the grid. Undo is therefore the
  same word-grouped history as typing; there is no table-specific undo.
- The editor enforces the invariants the model cannot: a cell always keeps at
  least one paragraph; Backspace / Delete never join across a cell boundary
  (the boundary is not a character, as in WordPerfect); a range that crosses
  cells clears them rather than joining them, and removes a table only when
  the range covers all of it; paste into a cell tags the pasted paragraphs
  with that cell; Enter in a cell adds a paragraph to the cell.
- Layout needs one extra input: a cell paragraph wraps to its column's width
  (`Table::cell_text_width`) rather than the page's. Pagination places a row
  as a unit — all cells start at the same y, the row is as tall as its
  tallest cell, a row that doesn't fit moves to the next page whole, and only
  a row taller than a page is split line by line. Draft view scales the grid
  to the terminal width (three cells minimum per column) and draws the box.

Nested tables, content controls and anything else inside a cell that is not a
paragraph stay preserved blocks *inside* the cell (`raw_block` with a cell
tag). A table whose XML has structure the reader does not expect — a content
control wrapping rows, an unknown element between cells — falls back to a
single preserved block, exactly the 0.2 behaviour, so nothing is ever
rewritten into a shape the reader did not understand.

---

## 4. Editing and undo

All mutation goes through `Document::apply(&mut self, op: Op) -> Op` which
returns the inverse op. `Op` is a small closed set: `InsertItems`,
`DeleteItems`, `SplitParagraph`, `JoinParagraphs`, `SetParaProps`,
`ReplaceRange`. Toggling bold on a selection is expressed as inserting a code
pair, so it is undoable with no special casing.

The undo stack holds *groups* of inverse ops. Grouping (P0-7: "undo groups by
word, not keystroke"): a new group starts when the op is not a single-char
insert, when the inserted char is whitespace after non-whitespace, when the
cursor moved between ops, or when more than 1 s has elapsed. Redo is the same
stack in reverse.

Cut history (P0-7, "last 16 cuts retrievable") is a ring of 16 item-vectors,
formatted; paste-plain strips codes.

---

## 5. Layout and pagination

### 5.1 Two layouts, one truth

`wp` maintains **two** layouts of every paragraph:

- **Print layout** — lines broken against the real page width using real font
  metrics, in twips. This is the source of page numbers, `Ln`, and `Pos`. It
  exists whether or not the user is looking at it (spec §6.4: "computes real
  pagination whether or not it's drawing the page").
- **Screen layout** — lines wrapped to the terminal's column count for draft
  view.

Draft view draws the screen layout and overlays page boundaries from the print
layout: the pagination pass records the `(paragraph, item)` position at which
each page begins, and draft view draws the `─── Page N ───` rule above the
screen line containing that position. Page view draws the print layout
directly, one character cell per… nothing in particular — cells are placed by
rounding each glyph's twip x-position to a column, which is why page view is
ragged, and honest (spec §6.4's stated limitation).

### 5.2 Font metrics

Pagination that matches Word (P0-14, ≥95% of corpus) needs per-glyph advance
widths for the fonts real documents use. `wp` embeds width tables generated at
development time by `tools/fontgen.py` from metric-compatible open fonts:

| Document font | Metrics source | Licence |
|---|---|---|
| Times New Roman | Liberation Serif | OFL |
| Arial, Helvetica | Liberation Sans | OFL |
| Courier New | Liberation Mono | OFL |
| Calibri | Carlito | OFL |
| Cambria | Caladea | OFL |

Each table covers U+0020–U+024F plus common punctuation, in 1/1000 em, for
regular and bold weights, plus the font's line-height factor
(`winAscent + winDescent`, over `unitsPerEm` — what Word uses for "single"
spacing). Unknown fonts fall back by family class (serif → Times metrics,
sans → Arial, mono → Courier); unknown glyphs use the font's average width.
Kerning is ignored — Word ignores it by default too.

### 5.3 Incremental layout

Each paragraph caches its print lines keyed on a content version and the
available width. An edit bumps one paragraph's version. Re-pagination walks all
paragraphs summing cached line heights — ~10 k paragraphs for a 500-page
document, sub-millisecond — so v0.1 does it synchronously on every keystroke.
The interface (`Layout::paginate(&Document) -> Pagination`) is pure so it can
move to a background thread when a profile says it must, without changing
callers. Keep-with-next, keep-lines, widow/orphan, and page-break-before are
handled in the pagination pass by pulling lines back to the next page.

---

## 6. `.docx`

### 6.1 Reading

`wp-docx` opens the zip, reads `[Content_Types].xml` and the package
relationships to find the main part (not assumed to be `word/document.xml`),
then parses `styles.xml`, `numbering.xml`, `footnotes.xml` (bodies, for
Markdown export), and the main part with a streaming pull parser. Everything it
recognises becomes model; every element it does not is captured as `Opaque`
with its exact bytes and the level it sat at (run, paragraph, or body), so it
goes back to the same place. Start-tag attributes of `w:p`, `w:r`, and
`w:sectPr` (revision ids, paragraph ids) are kept as text and re-emitted;
`w:proofErr` and `w:lastRenderedPageBreak` are kept as hidden hint opaques;
bookmarks keep their original ids, and any bookmark that is not the plain kind
(duplicate name, table-column attributes) is kept verbatim. Body-level tables
become cell-tagged paragraphs (§3.7) with `tblPr`, `trPr` and `tcPr` — and the
absence of a `tcPr` — recorded verbatim, so an untouched table writes back
token-identical; the grid is regenerated only when a column is added,
removed or resized. All other zip entries are stored untouched as
`(name, bytes)` on the `DocxPackage` that accompanies the `Document` in
memory.

### 6.2 Writing

Writing an opened file re-emits the main part from the model and copies every
other entry from the stored package verbatim, in the original order, with the
original compression. Writing a new file emits a minimal package (content types,
rels, document, styles, settings) that Word 2007+ opens without a repair
prompt.

### 6.3 The round-trip gate

`cargo test -p wp-docx` runs every file in `corpus/` through read → write and
asserts that the resulting main part is *semantically identical* and every
other entry is *byte-identical*. This test is the release gate named in spec
§10.2. "Semantically identical" tolerates only what the format itself cannot
distinguish: attribute order, `xml:space="preserve"`, empty `w:t`, adjacent
runs with identical properties at the same nesting depth, `w:cr` versus a plain
`w:br`, the hyphen elements versus their characters, and CDATA. Revision ids,
paragraph ids, proofing marks, and rendered page-break hints must all survive.
Bookmarks are compared as a set because a body-level bookmark and one at the
start of the next paragraph mean the same place.

The corpus (62 files, `tools/make_corpus.py`) spans python-docx output,
hand-built files that mimic Word 365, Google Docs, and LibreOffice export, and
deliberately pathological cases. Growing it to that size is what found every
0.2 fidelity bug.

---

## 6a. Google Docs (`wp-gdoc`)

Google Docs is a third native format, next to `.docx` and Markdown — not a
`.docx` export. Drive's "export as Word" and "upload as Google Doc" both run
Google's converter, which drops what it does not model; going through it
would break principle 1 on every save. The Docs API works on the document
itself: `documents.get` returns the whole thing as JSON, and
`documents.batchUpdate` applies *operations* (insert text here, delete this
range, set these style fields there), addressed by UTF-16 index. `wp-gdoc`
reads the former and writes the latter. No networking lives in the crate;
the binary fetches and posts.

### 6a.1 Reading

The JSON is a closed, documented schema, so there is no "unknown XML". Text
runs become characters with paired codes for the direct `TextStyle` fields
`wp` models (bold, italic, underline, strikethrough, small caps, size, font,
colour, background when it is one of Word's highlight colours, baseline
offset); a `link` becomes the same hyperlink wrapper a Markdown import makes
(`<w:hyperlink r:id>` + `extra_rels`), so `.docx` and Markdown export see an
ordinary hyperlink. Paragraph style fields become `ParaProps`; `namedStyleType`
becomes the style id (`HEADING_2` → `Heading2`); `namedStyles` fill the style
sheet so layout has real sizes; `documentStyle` sets the page. A `bullet`
becomes a `ListRef` on a numbering definition created per Docs list id,
its levels' formats and indents taken from the list's `nestingLevels`; the
indent Docs also stores on each list paragraph is dropped when it is the
level's own, so in `wp` the indent lives on the level and follows the
paragraph out of the list or to another level, as it does for `.docx`. Footnote references become `<w:footnoteReference>` items
numbered in document order, with the bodies read into `Document::footnotes`.
Tables with no spans become cell-tagged paragraphs (§3.7) with the grid from
`tableColumnProperties`; a table with spans, a nested table, a table of
contents and a section break after the first are preserved blocks.
Everything else in a paragraph — inline image, equation, person chip, rich
link, date, auto text, column break, horizontal rule — is a `Code::Opaque`
whose `xml` is the element's JSON plus its index length. Suggestions
(`suggestedInsertionIds` / `suggestedDeletionIds`) are wrapped as protected
`w:ins` / `w:del` tracked changes, which is what they are.

Alongside the `Document`, the reader returns a `Baseline`: the paragraphs
exactly as read, each with its Docs index range, grouped by container
(body stretch between tables, table cell, footnote), plus the revision id
and the list / footnote id maps.

### 6a.2 Writing: the diff

The writer never rebuilds the document. It compares the baseline with the
document as edited and emits only what changed, so content nobody touched
produces no request and therefore keeps everything Docs holds for it —
comments anchored to it, suggestions, chips, named ranges, the fields `wp`
does not model. Both sides are put through one projection (`project.rs`):
a paragraph becomes a list of *units* (a character, tab or soft line break
with its direct formatting; a page break; a footnote reference by Docs id;
any other element by its JSON), each carrying its UTF-16 length, plus the
paragraph formatting Docs can hold. Because the baseline is projected from
the reader's own paragraphs, an untouched paragraph projects identically on
both sides by construction.

Within each container, paragraphs are aligned by longest common subsequence
on the full projection, and the gaps paired positionally as modified
paragraphs; leftover baseline paragraphs are deleted, leftover model
paragraphs inserted after the nearest surviving paragraph. A modified
paragraph is diffed again at the unit level (by kind, ignoring style) into
delete / insert hunks; then its characters are compared unit by unit and
`updateTextStyle` is sent only for the ranges and fields that differ
(inserted text gets every modelled field, since Docs gives it its
neighbour's). Paragraph formatting is an `updateParagraphStyle` with a
field mask of what changed, the indent being the *effective* one (the list
level's unless set directly), because Docs keeps a list paragraph's indent
on the paragraph and derives its nesting level from it. So a level change
within a list is just an indent change; a paragraph joining a list gets
`createParagraphBullets` after leading tabs for its level (which the
request counts and removes); leaving one gets `deleteParagraphBullets`
plus the indent reset.

Two Docs rules shape the edit script. A container's final newline cannot be
deleted, so deleting the last paragraph deletes from the previous
paragraph's newline instead, and the survivor — which now ends with the
deleted paragraph's newline, and with it that paragraph's style — is restyled
in full. And a newline inserted into a paragraph creates a paragraph with
that paragraph's style and bullets, so every inserted paragraph is set in
full. Requests are generated in groups keyed by baseline index and emitted
in descending order, so no request shifts the indexes of the ones after it;
within a paragraph, text operations come first (style ranges are in the
paragraph's post-edit coordinates), then the paragraph style and bullets,
then character styles — in that order because applying a named style to a
paragraph resets its character formatting, verified live: text styles sent
before the paragraph style were silently undone.

The `batchUpdate` body carries `writeControl.requiredRevisionId` from the
read, so a save over someone else's concurrent edit is refused by Google
rather than merged blind; the app then re-reads and asks. Not yet expressible
as a diff (the writer returns an error naming the reason, and the document
can still be saved as `.docx`): adding, removing or reshaping a table;
creating a footnote; moving an image, footnote reference or page break into
a new paragraph; deleting every paragraph of a cell or footnote.

### 6a.3 What only Docs can hold

`Code::Opaque` items whose `xml` is JSON have no meaning in a `.docx`.
`wp_gdoc::detach` strips them (returning their labels for the save warning)
before a Drive-native document is written as `.docx`, Markdown or text;
hyperlinks, footnotes and tracked changes are ordinary `.docx` shapes and
stay.

### 6a.4 Privacy and the binary

SPEC §8 says *no network access*. That stays true by default: nothing in
`wp` opens a socket unless the user opens or saves a Google Doc. The client
(`google.rs`) is a blocking `ureq` call on the open / save / list path only
— no background sync, no token refresh on a timer, no telemetry.
Authentication is OAuth 2.0 with the loopback redirect (`wp` listens on a
random `127.0.0.1` port for the one redirect, then closes it), scopes
`documents` and `drive.readonly`, against a "Desktop app" client the user
creates in the Google Cloud console and puts in `config.toml` under
`[google]`. The refresh token is cached, mode 0600, in the state directory;
`Sign Out of Google` deletes it.

Network calls run from a small queue (`App::pending`) that the main loop
drains *after* drawing, so the screen shows "Contacting Google…" or the
sign-in URL while the call blocks; Esc cancels a sign-in. Opening is
`Open from Google Drive…`, the `gdoc:<id>` / URL argument on the command
line, and `--check` / `--text` / `--md` on a `gdoc:` reference for a
read-only look.

`Open from Google Drive…` is a modal like the local Open dialog
(`Overlay::Drive`), and it is the one place `wp` does network work off the
main thread. It opens at once on **Recent** — Google Docs ordered by
Drive's `recency` (last viewed, edited, or shared), the same list the Drive
web app calls Recent — from a copy cached on disk (`drive-recent.json` in
the state directory), while a worker thread fetches a fresh listing. Typing
filters those rows locally, so the common case (a doc you had open lately)
never waits on the network; a 300 ms pause in typing sends one
`name contains` search to Drive and shows any hits not already listed
under a "more from Drive" divider. Every fetch carries a sequence number
and the dialog only takes the reply it is waiting for, so a slow answer to
an earlier keystroke can't overwrite a later one. Tab switches to
**Folders**: My Drive / Shared with me / Shared drives browsed as a tree,
one `files.list` per folder (`'id' in parents`), cached for the session so
going back is instant. Enter on a document opens it (blocking, as before);
a pasted Docs URL opens directly. The main loop polls the terminal at 40 ms
instead of 250 ms while a listing or search is outstanding. Listings are
the *only* thing that runs on a worker: the worker gets a clone of the
client, and a token it refreshes is written to the same file. Saving is `Save`: the
diff is posted, then the document is re-read so the next save diffs
against what Google now has (the editor keeps its undo history when the
re-read has the same shape). A revision conflict is reported, not merged;
`Save As .docx` keeps the local version. Autosave for a Google Doc writes
the model *and* the baseline as JSON to the recovery directory, so a
recovered document can still be saved back as a diff.

---

## 7. The terminal application

### 7.1 Structure

```
App
├── Editor            document + cursor + selection + undo + layout caches
├── Commands          registry: id, title, category, handler
├── Keymap            two tables (classic, modern); modern includes classic F-keys
├── Views             draft, page, reveal-codes pane, palette, menus, open
│                     dialog, help, prompts
└── Status            filename, dirty flag, Doc/Pg/Ln/Pos, transient indicators
```

The main loop is single-threaded: read event → dispatch → relayout dirty
paragraphs → render diff. Rendering is skipped when nothing changed. Key events
are translated through the active keymap into command ids; commands are the
only thing that touches the editor. This gives P0-27 ("palette covers 100% of
capabilities") for free — the palette lists the registry, and every binding is
a registry entry, so nothing can be reachable by key but absent from the
palette.

### 7.2 Key handling

`crossterm` with the kitty keyboard protocol enabled where the terminal
supports it, so `Shift+F8`, `Ctrl+F7`, and `Alt+F3` arrive as themselves. On
terminals without it, `wp` still works: the palette reaches everything, and the
modern map avoids the ambiguous chords. Rebinding is a TOML table in the config
file mapping key strings to command ids.

### 7.3 Rendering

The document region is drawn from the screen layout with visible attributes
(bold, italic, underline, colour) mapped to terminal attributes and everything
else — font, spacing — rendered honestly as nothing, since a terminal can't
show it. Size gets WordPerfect 5.1's treatment: WP couldn't show sizes either,
so its display setup mapped each size attribute to a screen attribute. `wp`
classes a run's size against the body text — Large (≥ 120 %) is bold, Very
Large and above (≥ 150 %) bold in the theme's size colour, Fine / Small
(≤ 85 %) dim — so a Google Docs title, heading and body read as three tiers
even though none of them is bold. The status line names the paragraph style
at the cursor. Reveal Codes is the place the real points and fonts are seen.

Box drawing degrades to ASCII and colours to the 16-colour palette when
`TERM`/`COLORTERM` indicate a basic terminal (§8, "works anywhere").

### 7.4 Autosave and recovery

Every 30 s if dirty, the document is serialised to
`$XDG_STATE_HOME/wp/recovery/<hash-of-path>.wpr` in `wp`'s own compact format
(the model, not `.docx` — faster and lossless). On open, a recovery file newer
than the target file triggers the recovery prompt. The file is removed on a
clean save or exit.

---

## 8. Milestone mapping

| Spec release | What this design delivers |
|---|---|
| **0.1 Preview** | Model, editing, undo, character/paragraph formatting, styles, draft view with true page boundaries, `.docx` write, basic `.docx` read, plain text, both keymaps, palette, Reveal Codes (the model makes it nearly free, so it comes early) |
| **0.2 Round-trip** | Corpus of 62 files with a stricter gate and the fixes it forced; lists from `numbering.xml` with real labels and list commands; regex / format / code search with replace preview; Markdown in and out; mouse; OSC 52 clipboard |
| 0.3 Documents | Tables, sections, headers/footers, page view |
| 1.0 | Footnotes, TOC, cross-refs, captions, index, images, spelling, macros, tutor |

---

## 9. Decisions

| # | Decision | Alternative rejected | Why |
|---|---|---|---|
| E1 | Paragraphs-of-items model | Flat item stream with `[HRt]` in-stream (pure WP) | Per-paragraph caching and 1:1 `.docx` mapping; Reveal Codes can still *show* a flat stream |
| E2 | Paired character codes | Run-attribute model (`.docx`-native) | Reveal Codes' "delete the code" needs codes to be real objects; runs are derivable, codes are not |
| E3 | Paragraph props as props, displayed as codes | Paragraph codes in-stream | `.docx` semantics are per-paragraph; in-stream codes would need "until next change" semantics that don't round-trip |
| E4 | Own `.docx` reader/writer | `docx-rs` or similar | Every library round-trips through its own model; principle 1 forbids that |
| E5 | Embedded metrics from OFL metric-compatible fonts | Reading system fonts at runtime | One binary, works on a bare console with no fonts installed, identical pagination on every machine |
| E6 | Synchronous layout in v0.1 | Background layout thread | Measured cost is negligible at 500 pages; keep the interface pure so it can move later |
| E7 | Twips as `i32` | Points as `f32` | Lossless with the file format; no accumulated drift over a long document |
| E9 | Google Docs as a native format via the Docs API, written as a minimal diff of `batchUpdate` operations (§6a) | Drive's `.docx` export/import; or delete-all-and-reinsert via the API | Both run everything through a converter or a rebuild and lose what `wp` does not model; a diff touches only what the user changed |
| E8 | Table cells as tagged paragraphs in the flat stream, grid as a property (§3.7) | Nested `Block::Table { rows: Vec<Vec<Vec<Paragraph>>> }` with a path-shaped cursor | Keeps `Pos`, undo, search and the writer unchanged; matches both WordPerfect's stream and `.docx`'s serialisation; structural edits reduce to existing ops |

---

## 10. Open engineering questions

1. **Aptos.** Word's default font since 2023 has no metric-compatible open
   equivalent. Documents in Aptos will paginate with Carlito metrics until one
   exists. Track corpus page-count deviation for Aptos documents separately.
2. **Hyphenation.** Word hyphenates only when asked; v1 does not hyphenate.
   Documents with `w:autoHyphenation` will paginate long.
3. **Justified text and Word's line-breaking quirks.** Word's break algorithm
   has known deviations from a greedy fit (compressed spaces, trailing-space
   handling). The corpus will tell us how much this matters.

---

## 10a. Keymap deviation from the spec (2026-08-28)

Spec §6.2 names `Ctrl+K` as the palette key. In the **modern** map `Ctrl+K`
is kill-to-end-of-line instead, because the modern map follows emacs / macOS
readline movement and deletion (`Ctrl+F/B/N/P/A/E/D/H/K/U`, `Alt+F/B/D`),
which the primary user asked for out of the box. The palette is
`Ctrl+Shift+P` (VS Code muscle memory), `Cmd+P`, `Cmd+Shift+P`, and `Alt+=`;
the classic map keeps `Ctrl+K`. `Cmd+P` earns its place because Ghostty 1.2
took `Cmd+Shift+P` for its own palette, and `wp` has no Print to collide
with. On macOS the *advertised* label never picks an `Alt+` binding when the
command has another: Option is not Alt unless the terminal is configured to
send it, so `Alt+=` was being shown for a key most Mac users cannot press (it is now
the menu key, and the modern map's `F10` twin is what gets advertised).
A Cmd/Super layer rides on top of the modern map for terminals that deliver
it via the kitty keyboard protocol; every Cmd binding has a Ctrl or F-key
twin, so nothing depends on it.

## 11. Status (2026-08-28)

**0.2 Round-trip is implemented.** On top of 0.1:

- `wp-docx`: the fidelity work in §6.1/§6.3 (element levels, start-tag
  attributes, hints, bookmark ids, verbatim fallbacks), `numbering.xml` read
  and regenerated only when a list is added, `footnotes.xml` read for export
  and generated for Markdown imports, relationships and content types added
  for parts and hyperlinks the document creates, no styles part or `sectPr`
  invented for files that had none.
- `wp-core`: `numbering` (definitions, every `numFmt`, per-abstract counters
  with overrides, level indents merged between style and direct formatting,
  labels placed in print and draft layout), `search` (regex with captures,
  smart case / match case / whole word, formatting and style filters, code
  search, context lines), `Editor::replace_range` as one undo unit.
- `wp-md`: CommonMark + GFM tables, strikethrough, task lists, footnotes,
  both directions, with a one-line loss report on export.
- `wp`: list commands (toggle bullets / numbering, format picker, Tab /
  Shift+Tab levels, restart, continue, remove; Enter on an empty item ends the
  list), find prompt option toggles (Alt+R/C/W), palette find commands,
  replace preview with one-at-a-time mode, `.md` open/save, `--md`, mouse
  (click, drag, double-click, wheel), bracketed paste, OSC 52 copy.

Known gaps carried into 0.3: a document that declares WordprocessingML as the
*default* namespace (no `w:` prefix) opens as preserved blocks only; Markdown
images become links; right-aligned list labels (`lvlJc="right"`) are placed
left-aligned; `[List:…]` in Reveal Codes shows the instance id rather than
the format.

**Google Docs, in progress (2026-08-28).** `wp-gdoc` (§6a) reads
`documents.get` JSON into the model and turns edits into `batchUpdate`
requests, tested against recorded fixtures (`tools/make_gdoc_fixtures.py`):
typing, deleting, bolding, paragraph formatting, bullets, splitting and
joining paragraphs, appending, table-cell and footnote edits, page breaks,
UTF-16 surrogates, tabbed documents, suggestions. The binary's OAuth
client, open / save / recovery and `--check` (2026-08-29, §6a.4); the Open
from Drive modal — cached recents, type-to-filter with a paused-typing
server search, and a folder view — with its listings on a worker thread
(2026-08-30). Still to do: live verification against the API — the request
shapes follow the reference and the newline / paragraph-style semantics in
§6a.2 follow its documentation, but have not been exercised on a real
document yet.

**0.3 Documents, in progress — tables (2026-08-28).** The model of §3.7:
`.docx` tables read into editable cells and write back verbatim (the full
corpus gate passes with every table fixture parsed, not preserved); insert a
table (`Table: Insert…`, Alt+F7 in classic), Tab / Shift+Tab between cells
(Tab at the last cell adds a row), ↑/↓ across rows by column, insert and
delete rows and columns, delete table, convert to tab-separated text (also
what deleting `[Tbl Def]` does), column width, header-row flag; draft view
draws the grid with spans and vertical merges; Reveal Codes shows the
structure; the status line shows the cell; Markdown tables import as real
tables and export from them. Still to do for P0-17: merge and split cells
(spans and merges are preserved and drawn, not created), borders and shading
as editable properties, sort by column, `SUM`/`AVERAGE` formulas, header rows
actually repeating in pagination, `cantSplit`, and row heights; a literal tab
inside a cell (Tab navigates; insert one from the palette outside a cell and
paste it); page view.

**0.1 Preview** provided:

- `wp-core`: model, paired-code attribute rewriting, invertible ops with
  word-grouped undo, 16-entry cut ring, print layout with embedded metrics for
  five font families, pagination with keep-with-next / keep-lines /
  widow-orphan / hard breaks, draft wrap, Reveal Codes labels, plain text.
- `wp-docx`: package with verbatim entry preservation, streaming reader that
  keeps unknown run/paragraph properties (`Attr::Raw`, `raw_ppr`), wrappers for
  hyperlinks / tracked changes / fields / content controls as paired opaque
  markers, body-level tables and SDTs as raw blocks, style sheet parsing with
  raw re-emission, section geometry, theme font resolution. Writer regenerates
  only what changed. The corpus gate test passes on three fixtures (a
  python-docx document, an empty document, and a hand-built pathological one
  with `w:ins`/`w:del`, comments, fields, hyperlinks, SDTs, and a table).
- `wp`: draft view with true page rules and paragraph-spacing gaps, status
  block, Reveal Codes pane with editable codes and paragraph pseudo-codes,
  command palette with `> @ # / ?` modes and key labels, WordPerfect-style
  pull-down menus (`F10` / `Alt+=`; the bar is on by default and View ▸ Menu
  Bar makes it 5.1-style, shown only while open; every item is a registry
  command and the test checks it), a classic blue-screen theme
  (`theme = "classic"`, colours sampled from a WP 5.1 screenshot: CGA blue
  ground, light-grey text, bright-white bold and status, red mnemonics, with
  a 16-colour fallback), an Open dialog that
  browses directories (type to filter, Tab to complete, `/` to jump to a typed
  path), both keymaps with the classic F-key layer, rebinding from config,
  incremental find, replace-all,
  go-to page/heading/bookmark, style browser with inheritance and overrides at
  the cursor, character and paragraph formatting commands, page setup, autosave
  and crash recovery, first-run keyboard prompt, F-key legend, help,
  `--check` / `--text` CLI modes. Headless `TestBackend` tests drive the UI.

Measured on a 330-page, 142 k-word `.docx`: cold open with full pagination
70 ms; ~0.2 ms per keystroke including repagination and render; 80 MB RSS.

**Not yet (by milestone):**

- 0.3: page view (the command exists and toggles a status indicator only),
  the rest of tables (above), headers/footers, multiple sections.
- 1.0: footnotes, TOC, cross-references, captions, index, images, spelling,
  macros, tutorial.

The 0.1 fidelity gaps (dropped rsids and paragraph ids, dropped
`lastRenderedPageBreak` / `proofErr`, body-level bookmarks moved into the next
paragraph) are closed in 0.2; see §6.1.

