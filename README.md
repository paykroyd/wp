# wp

A word processor that runs in a terminal. It opens and saves `.docx` natively,
paginates like Word does, and borrows its editing model from WordPerfect 5.1 —
a blank screen, a complete keyboard, and **Reveal Codes** — with a modern
command palette so nothing has to be memorised.

```
   The quarterly results exceeded projections in every region except EMEA,
   where currency effects reduced reported growth by roughly four points.

   Regional detail

   North America grew 14% year over year, led by renewals in the mid-market
   segment.█

 Q3-REPORT.DOCX *                      Doc 1  Pg 4/12  Ln 2.70"  Pos 3.40"
```

**Status: 0.3 Documents.** Produce a real structured document: tables
(insert, Tab between cells, rows and columns, merge and split, lines and
shading, sort, `SUM(ABOVE)` formulas, header rows that repeat on each page),
sections with their own paper, margins and columns, headers and footers with
page numbers (edited on their own screen, as WordPerfect did), and a page
view that draws the page — margins, header, footer and columns in place —
with the page number correct in both views. Take a `.docx` someone sent you,
edit it, and send it back: what `wp` doesn't edit yet (comments, tracked
changes, images, footnotes, nested tables) is shown as a labelled placeholder
and preserved byte-for-byte — down to Word's revision ids — across a 62-file
test corpus. Lists are real Word lists with their numbering, find and replace
does regular expressions, formatting ("find bold text in Heading 2") and codes
("find the next page break") with a preview before replacing, and Markdown
opens and saves with one honest line about what it can't carry. See
[SPEC.md](SPEC.md) for the product spec and [DESIGN.md](DESIGN.md) for how
it's built.

## Install

```
cargo install --path crates/wp
```

One static binary, no runtime, no network access, no telemetry.

## Use

```
wp                      blank document
wp report.docx          open a Word document
wp notes.md             open Markdown (CommonMark + tables, footnotes, task lists)
wp notes.txt            open plain text
wp --check report.docx  what's in it, page count, anything unsupported
wp --text report.docx   dump as text
wp --md report.docx     dump as Markdown
```

Save As picks the format from the extension: `.docx`, `.md`, `.txt`. Saving a
Word document as Markdown tells you, once, exactly what was dropped.

On first run you pick a keyboard. Both are complete and you can switch later.

| | Modern | Classic (WordPerfect 5.1) |
|---|---|---|
| Command palette | `Ctrl+Shift+P` (`Cmd+Shift+P`, `Cmd+P`) | `Ctrl+K` / `Alt+F10` |
| Pull-down menus | `Alt+=` / `F10` | `Alt+=` |
| Reveal Codes | `Alt+F3` | `Alt+F3` / `F11` |
| Save / Open / Exit | `Ctrl+S` / `Ctrl+O` / `Ctrl+Q` (or `Cmd+…`) | `F10` / `F5` / `F7` |
| Bold / Italic / Underline | `Cmd+B/I/U` or `Ctrl+Shift+B` / `Ctrl+I` / `Ctrl+Shift+U` | `F6` / `Ctrl+F10` / `F8` |
| Move | emacs: `Ctrl+F/B/N/P`, `Ctrl+A/E`, `Alt+F/B` — and arrows | arrows |
| Delete | `Ctrl+D/H`, `Ctrl+K` to end of line, `Ctrl+U` to start, `Alt+D` word | `Del`, `Backspace` |
| Select | `Shift+arrows`, `Ctrl+Space` sets the mark | `Alt+F4` then move |
| Find / Replace | `Ctrl+Shift+F` / `Ctrl+Shift+H` (`Cmd+F` / `Cmd+Shift+H`) | `F2` / `Alt+F2` |
| Lists | `Ctrl+Shift+L` bullets, `Ctrl+Shift+O` numbers, `Tab` / `Shift+Tab` level | palette |
| Undo | `Ctrl+Z` | `Ctrl+Z` |
| Center / flush right | `Ctrl+Shift+E` / `Ctrl+R` | `Shift+F6` / `Alt+F6` |
| Repeat count | — | `Esc` `8` `↓` |

The full list for both keyboards is in [KEYBINDINGS.md](KEYBINDINGS.md). The
F-keys keep their classic meanings under the modern map, so you can learn them
gradually. `F1` (modern) or `F3` (classic) shows help; the palette's *F-Key
Legend* shows the keyboard template card. `Cmd+…` bindings work in terminals
that report the Cmd key (Ghostty, kitty, WezTerm); `wp --probe-keys` shows
what yours sends.

The palette prefixes: `>` commands (default), `@` jump to a heading, `#` jump to
a page, `/` incremental find, `?` help.

The mouse works too — click, drag, double-click a word, wheel to scroll — but
nothing needs it. Copy and cut also reach the system clipboard through the
terminal (OSC 52), including over SSH where the terminal allows it.

The menu bar across the top lists every command by group with its key: `F10`
(or `Alt+=`, as in WordPerfect 5.1) opens it, `←`/`→` change menu, a letter
picks an item, `Esc` closes. *View ▸ Menu Bar* hides it the 5.1 way, where it
appears only while a menu is open. Also under View: *Theme: Classic
WordPerfect* — the 5.1 blue screen with its exact CGA colours, status text
bottom right — or `theme = "classic"` in the config.

Everything is rebindable in `~/.config/wp/config.toml`.

## Google Docs

`wp` opens and saves Google Docs directly — *File ▸ Open from Google Drive…*
(recents, search-as-you-type, folders), or from the shell:

```
wp gdoc:<id>                       open a Google Doc
wp https://docs.google.com/...     the same, from a URL
wp --check gdoc:<id>               inspect it (also --text, --md, --json); never writes
```

Saves are diffs: only the ranges you changed go back, guarded by the revision
you opened, so a concurrent edit in the browser is caught rather than
overwritten. An edit the API can't express is reported by name and the
document stays saveable as `.docx`.

There is no shared app credential — you register your own once, which takes
about five minutes:

1. In [Google Cloud Console](https://console.cloud.google.com/) create a
   project (or pick one), then under **APIs & Services ▸ Library** enable the
   **Google Docs API** and the **Google Drive API**.
2. **APIs & Services ▸ OAuth consent screen**: External, add yourself as a
   test user. The app can stay in "Testing" — it never needs verification for
   your own use.
3. **APIs & Services ▸ Credentials ▸ Create credentials ▸ OAuth client ID**,
   application type **Desktop app**. Copy the client ID and secret into
   `~/.config/wp/config.toml`:

   ```toml
   [google]
   client_id = "1234567890-abc.apps.googleusercontent.com"
   client_secret = "GOCSPX-..."
   ```

The first Docs command opens your browser to sign in; `wp` listens on a
loopback port for the redirect. It asks for the `documents` and
`drive.readonly` scopes only. The refresh token is cached, mode 0600, in
`~/.local/state/wp/`; *File ▸ Sign Out of Google* deletes it. Nothing runs in
the background and nothing else is sent anywhere.

## Find and replace

The find box takes plain text (smart case), or:

```
re:colou?r              regular expression; $1 … in the replacement
bold:                   every stretch of bold text
italic: draft           italic text containing "draft"
style:"Heading 2"       paragraphs in a style (id or name), with optional text
[HPg]  [Tab]  [BOLD]    the next code, by its Reveal Codes label
```

`Alt+R` / `Alt+C` / `Alt+W` in the box toggle regex, match case, whole word.
Replace lists every match with its context and the expanded replacement
before anything changes; `Enter` replaces all, `O` steps through one at a time.

## Reveal Codes

`Alt+F3` splits the screen. The lower pane shows the text with every formatting
instruction visible:

```
[Style:Heading2]Regional detail[HRt]
[HRt]
North America grew [BOLD]14%[bold] year over year.[SRt]
```

Put the cursor on a code and press `Delete`: the code *and its pair* are gone
and the formatting with them. Paragraph codes (`[Style:…]`, `[Just:Center]`,
`[L Ind:0.5"]`) sit at the start of the paragraph and delete the same way.
`[SRt]` and `[SPg]` are where the layout broke a line or a page; they're shown,
not editable. Tables show as `[Tbl Def:3×4]`, `[Row]` and `[Cell:B2]` on the
first paragraph of each cell; deleting `[Tbl Def]` turns the table into
tab-separated text, as it did in WordPerfect.

## Fidelity

`corpus/` holds the round-trip test corpus: 62 files generated by
`tools/make_corpus.py` in the styles of Word 365, Google Docs, LibreOffice and
python-docx, plus deliberately broken ones. `cargo test -p wp-docx` reads and
writes every file and fails if the main document part differs in anything the
file format can express or any other part differs by a byte. This is the
release gate.

## Development

```
cargo test                     # everything, including the corpus gate and headless UI tests
cargo build --release
python3 tools/make_corpus.py corpus   # regenerate the corpus (needs python-docx in a venv)
python3 tools/fontgen.py       # regenerate embedded font metrics (see DESIGN.md §5.2)
python3 tools/gen_keybindings.py      # regenerate KEYBINDINGS.md after touching keymaps
```

Crates: `wp-core` (model, editing, layout, lists, search — no terminal, no file
formats), `wp-docx` (`.docx` in and out), `wp-md` (Markdown in and out), `wp`
(the terminal app).


## Licence

MIT or Apache-2.0, at your option. Embedded font metrics derive from the
OFL-licensed Liberation, Carlito, and Caladea fonts.
