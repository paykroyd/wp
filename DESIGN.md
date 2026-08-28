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

No network crate exists in the dependency tree. "No network access" (§8) is
enforced by construction, not by policy.

---

## 2. Crate layout

```
wp/
├── Cargo.toml                 workspace
├── crates/
│   ├── wp-core/               document model, codes, styles, editing, undo,
│   │                          layout & pagination, font metrics, plain text
│   ├── wp-docx/               .docx read/write with opaque preservation
│   └── wp/                    the binary: terminal UI, keymaps, commands,
│                              palette, views, config, autosave
└── tools/
    └── fontgen.py             generates embedded font-metric tables
```

`wp-core` has no terminal dependency and no knowledge of `.docx`. Everything in
it is testable with plain `cargo test` and no TTY. `wp-docx` depends on
`wp-core` only. The binary depends on both.

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
then parses `styles.xml`, `numbering.xml`, `settings.xml`, and the main part
with a streaming pull parser. Everything it recognises becomes model; every
element it does not is captured as `Opaque` with its exact bytes. All other zip
entries are stored untouched as `(name, bytes)` on the `DocxPackage` that
accompanies the `Document` in memory.

### 6.2 Writing

Writing an opened file re-emits the main part from the model and copies every
other entry from the stored package verbatim, in the original order, with the
original compression. Writing a new file emits a minimal package (content types,
rels, document, styles, settings) that Word 2007+ opens without a repair
prompt.

### 6.3 The round-trip gate

`cargo test -p wp-docx` runs every file in `corpus/` through read → write and
asserts that the resulting main part is *semantically identical* (canonicalised
XML, ignoring attribute order and insignificant whitespace) and every other
entry is *byte-identical*. This test is the release gate named in spec §10.2. It
is present from the first commit, with an empty corpus, so it is never
"something we'll add".

---

## 7. The terminal application

### 7.1 Structure

```
App
├── Editor            document + cursor + selection + undo + layout caches
├── Commands          registry: id, title, category, handler
├── Keymap            two tables (classic, modern); modern includes classic F-keys
├── Views             draft, page, reveal-codes pane, palette, help, prompts
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
else — font, size, spacing — rendered honestly as nothing, since a terminal
can't show it. Reveal Codes is the place those are seen.

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
| 0.2 Round-trip | Opaque preservation exhaustively tested against the corpus, lists, find/replace, autosave, Markdown |
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

## 11. Status (2026-08-28)

**0.1 Preview is implemented** against this design. What exists:

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
  command palette with `> @ # / ?` modes and key labels, both keymaps with the
  classic F-key layer, rebinding from config, incremental find, replace-all,
  go-to page/heading/bookmark, style browser with inheritance and overrides at
  the cursor, character and paragraph formatting commands, page setup, autosave
  and crash recovery, first-run keyboard prompt, F-key legend, help,
  `--check` / `--text` CLI modes. Headless `TestBackend` tests drive the UI.

Measured on a 330-page, 142 k-word `.docx`: cold open with full pagination
70 ms; ~0.2 ms per keystroke including repagination and render; 80 MB RSS.

**Not yet (by milestone):**

- 0.2: exhaustive corpus (currently 3 files, target ~60), list rendering from
  `numbering.xml` (lists round-trip but draft view shows a generic bullet),
  regex / format / code search, replace preview, Markdown, OSC 52 system
  clipboard, mouse.
- 0.3: page view (the command exists and toggles a status indicator only),
  tables as editable objects, headers/footers, multiple sections.
- 1.0: footnotes, TOC, cross-references, captions, index, images, spelling,
  macros, tutorial.

**Known fidelity gaps** to fix before 0.2 ships: `w:p` and `w:sectPr` element
attributes (`w:rsid*`, `w14:paraId`) are dropped on write — Word regenerates
them, and the canonical comparison ignores them, but a byte-diff will show
them; `w:lastRenderedPageBreak` and `w:proofErr` are dropped for the same
reason; bookmarks that sat between paragraphs at body level are re-emitted at
the start of the following paragraph.
