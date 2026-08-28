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

**Status: 0.1 Preview.** You can write and format a document, save it as
`.docx`, and Word opens it. Documents someone else sent you open and round-trip
safely — what `wp` doesn't understand yet (tables, comments, tracked changes,
fields, images) is shown as a labelled placeholder and preserved byte-for-byte
on save. See [SPEC.md](SPEC.md) for the product spec and [DESIGN.md](DESIGN.md)
for how it's built.

## Install

```
cargo install --path crates/wp
```

One static binary, no runtime, no network access, no telemetry.

## Use

```
wp                      blank document
wp report.docx          open a Word document
wp notes.txt            open plain text
wp --check report.docx  what's in it, page count, anything unsupported
wp --text report.docx   dump as text
```

On first run you pick a keyboard. Both are complete and you can switch later.

| | Modern | Classic (WordPerfect 5.1) |
|---|---|---|
| Command palette | `Ctrl+Shift+P` (`Cmd+Shift+P`, `Alt+=`) | `Ctrl+K` / `Alt+F10` |
| Reveal Codes | `Alt+F3` | `Alt+F3` / `F11` |
| Save / Open / Exit | `Ctrl+S` / `Ctrl+O` / `Ctrl+Q` (or `Cmd+…`) | `F10` / `F5` / `F7` |
| Bold / Italic / Underline | `Cmd+B/I/U` or `Ctrl+Shift+B` / `Ctrl+I` / `Ctrl+Shift+U` | `F6` / `Ctrl+F10` / `F8` |
| Move | emacs: `Ctrl+F/B/N/P`, `Ctrl+A/E`, `Alt+F/B` — and arrows | arrows |
| Delete | `Ctrl+D/H`, `Ctrl+K` to end of line, `Ctrl+U` to start, `Alt+D` word | `Del`, `Backspace` |
| Select | `Shift+arrows`, `Ctrl+Space` sets the mark | `Alt+F4` then move |
| Find | `Ctrl+Shift+F` / `Cmd+F` | `F2` |
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

Everything is rebindable in `~/.config/wp/config.toml`.

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
not editable.

## Fidelity

`corpus/` holds the round-trip test corpus. `cargo test -p wp-docx` reads and
writes every file and fails if the main document part differs semantically or
any other part differs by a byte. This is the release gate.

## Development

```
cargo test                # everything, including the corpus gate and headless UI tests
cargo build --release
python3 tools/fontgen.py  # regenerate embedded font metrics (see DESIGN.md §5.2)
```

Crates: `wp-core` (model, editing, layout — no terminal, no file formats),
`wp-docx` (`.docx` in and out), `wp` (the terminal app).

## Licence

MIT or Apache-2.0, at your option. Embedded font metrics derive from the
OFL-licensed Liberation, Carlito, and Caladea fonts.
