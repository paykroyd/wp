# Changelog

Releases follow the plan in SPEC.md §9.3: each one is named for what the
user can do with it. DESIGN.md §11 has the current status and gaps.

## 0.3.0 — Documents (2026-09-02)

Produce a real structured document.

### Tables (the rest of P0-17)
- Merge selected cells (Block, then Tab or arrows to the last cell) and
  split a cell: unmerge a vertical region, one cell per column, or divide a
  plain cell in two.
- Lines — All / None / Outside Only / Inside Only for the table, Cell Lines
  and Cell Shading for the selected cells. A table without lines shows a
  dotted grid on screen and prints nothing. Only the changed child of a
  verbatim `tblPr` / `tcPr` is rewritten; everything else Word wrote comes
  back untouched.
- Sort rows by the cursor's column, ascending or descending, numerically
  when every key is a number; header rows stay put.
- Formula… inserts `=SUM(ABOVE)`, `AVERAGE(LEFT)`, `MAX(B2:C9)`, `COUNT`,
  `MIN` or `PRODUCT` as a field with its value; Recalculate Formulas
  updates them. Formulas read from a file are recognised as such, not
  reported as unsupported fields.
- Row Height… (at least / exact / fit the text), Row Can't Split Across
  Pages, Insert Tab Character in Cell.
- Pagination follows Word: rows run on past the page bottom unless
  `cantSplit`, header rows repeat at the top of every page the table
  continues onto, row heights are honoured.

### Sections
- Insert Section Break (New Page / Continuous / Odd Page / Even Page), with
  a blank page where an odd or even start needs one.
- Page setup (paper, orientation, margins) applies to the section at the
  cursor; the status line shows `Sec n/m`; Reveal Codes shows
  `[Sect Brk:New Page]` and deleting it merges the sections as Word does.
- Columns: One / Two / Three, and Insert Column Break. Text flows column by
  column; draft view marks where a column begins.
- Page numbering restarts (`w:pgNumType`) and section starts are read and
  written; a section read from a file writes back token-identical unless
  changed.

### Headers and footers
- Header / Footer: Edit for every page, the first page or even pages, each
  edited on its own screen as WordPerfect 5.1 did; Exit (F7) returns.
  Saving while a header is open saves the document with the header synced.
- Header / Footer: Remove from This Section; Different First Page and
  Different Odd and Even toggles.
- Insert Page Number and Insert Page Count (`PAGE` / `NUMPAGES` fields),
  shown as `[Page Num]` in Reveal Codes.
- Header and footer parts are read from a `.docx`, kept verbatim until
  edited, and written back as new or regenerated parts with their
  relationships and content types. A tall header shortens the text area so
  page counts match Word's.
- Built-in Header and Footer styles with Word's centre and right tabs.

### Page view
- Switch Draft / Page View draws the page: a box as wide as the paper with
  margins, header and footer in position, columns side by side, table cell
  edges, repeated header rows, blank pages and live page numbers. Glyphs
  sit where their twip position rounds to, never overwriting the previous
  one, so proportional text is ragged and honest.
- Typing, selection and clicking work on the page; the view follows the
  cursor when it moves and the wheel scrolls the pages freely. Measured at
  0.7 ms per keystroke and render on an 84-page document.

### Also
- Right-aligned list labels (`lvlJc="right"`) end at the first-line
  position; Reveal Codes names a list's format
  (`[List:Decimal "%1." Lvl 1]`) rather than its numbering instance id.
- Enter at the end of a section's last paragraph keeps the break at the
  end of the section.
- Markdown export reports headers and footers as a loss.
- A `.docx` that puts WordprocessingML in the default namespace stays
  preserved blocks by decision (DESIGN.md E12).

## 0.2.0 — Round-trip (2026-08-28)

Take a `.docx` someone sent you, edit it, and send it back safely.

- Round-trip corpus of 62 files (python-docx, Word, Google Docs and
  LibreOffice shapes, and pathological cases) with a stricter gate;
  revision ids, paragraph ids, proofing marks, rendered page breaks and
  bookmark ids all survive.
- Lists from `numbering.xml` with real labels, every number format,
  restart and continue; list commands (bullets, numbering, format picker,
  Tab / Shift+Tab levels, remove).
- Find and replace: live incremental find, regular expressions with
  captures, case and whole-word options, formatting search ("bold text in
  Heading 2"), code search ("the next page break"), a preview before
  replace-all and one-at-a-time replacement.
- Markdown in and out: CommonMark and GFM tables, strikethrough, task
  lists and footnotes, with one line about what a save cannot carry.
- Tables as editable cells in the flat stream: insert, Tab between cells,
  rows and columns, delete, convert to text, column width, header-row flag;
  round-tripped verbatim.
- Mouse (click, drag, double-click, wheel), bracketed paste, the system
  clipboard through OSC 52.
- Pull-down menu bar, the WordPerfect 5.1 classic blue theme, the Open
  dialog as a file browser, `Cmd+P` for the palette.
- Google Docs as a native format: open from Drive (recents, search,
  folders), save as a `batchUpdate` diff guarded by the revision id,
  OAuth sign-in, recovery.
- Draft view wraps at the printed line breaks by default; sizes shown as
  WP 5.1 attributes; the paragraph style in the status line.
- macOS distribution: `wp.app` launcher, universal binary, release
  workflow.

## 0.1.0 — Preview (2026-08-28)

Write and format a document, save it as `.docx`, and have Word open it.

- The model: paragraphs of items, paired character codes, paragraph
  properties shown as codes, everything unknown preserved verbatim.
- Print layout with embedded metrics for five font families, pagination
  with keep-with-next, keep-lines, widow/orphan control and hard breaks;
  draft view with true page rules.
- `.docx` writer and basic reader with verbatim preservation of unknown
  parts; the round-trip gate on the first fixtures.
- Reveal Codes with editable codes, the command palette, both keymaps
  (classic WordPerfect 5.1 F-keys and a modern emacs/macOS map), style
  browser, character and paragraph formatting, page setup, autosave and
  crash recovery, `--check` and `--text`.
