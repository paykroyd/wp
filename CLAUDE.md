# wp — notes for working in this repo

A terminal word processor in Rust with native `.docx` round-trip and
WordPerfect-style Reveal Codes. Read in this order: `SPEC.md` (product
requirements — treat as the source of truth for *what*), `DESIGN.md` (how it's
built and why; §11 is the current status and gap list), `KEYBINDINGS.md`.

## Toolchain

- Rust stable via rustup, installed to `~/.cargo`. Non-login shells may need
  `export PATH="$HOME/.cargo/bin:$PATH"` before `cargo` works.
- `cargo test` runs everything: core unit tests, the `.docx` corpus gate, and
  headless UI tests (ratatui `TestBackend`). It must be green before a commit.
- `cargo build --release` → `target/release/wp`. Try it with
  `./target/release/wp corpus/gen-report.docx`; `wp --probe-keys` shows what the
  terminal delivers for each key.
- Corpus: `python3 tools/make_corpus.py corpus` regenerates all ~60 fixtures (needs `python-docx`;
  use a venv, the system Python is PEP 668-locked). `tools/fontgen.py`
  regenerates embedded font metrics; `tools/gen_keybindings.py` regenerates
  `KEYBINDINGS.md` — run it after touching `keymap.rs` or `commands.rs`.

## Layout

- `crates/wp-core` — model, editing, undo, layout/pagination, metrics. No
  terminal, no file formats. Everything here is unit-testable.
- `crates/wp-docx` — `.docx` reader/writer. `tests/roundtrip.rs` is the
  **release gate**: every file in `corpus/` must round-trip with the main part
  semantically identical and every other part byte-identical.
- `crates/wp-md` — Markdown import/export on top of `pulldown-cmark`; emits
  table blocks and footnotes as WordprocessingML, so it depends on `wp-docx`.
- `crates/wp` — the binary: `app.rs` (state + command execution), `ui.rs`

  (rendering), `commands.rs` (registry — every capability is a command),
  `keymap.rs` (classic / modern / Cmd tables), `tests.rs` (headless UI tests).

## Rules that keep the product promises

- **Never damage a document.** Anything the reader doesn't model must be kept
  verbatim: run properties as `Attr::Raw`, paragraph properties in `raw_ppr`,
  unknown elements as `Code::Opaque`, body blocks as `raw_block` paragraphs,
  other zip parts untouched. When modelling something new, update
  `parse_rpr`/`render_rpr_attrs` (or `parse_ppr`/`render_ppr_body`) together
  and add a corpus fixture that exercises it.
- **Formatting is never a mystery.** Character formatting is only ever paired
  codes in the item stream; change attributes through `rewrite_attrs`, never
  by splicing codes by hand. Paragraph properties are struct fields shown as
  pseudo-codes in Reveal Codes.
- **Nothing is key-only.** Add a command to `commands.rs` first; keys and the
  palette both resolve to it. Palette completeness is tested.
- Geometry is twips (`i32`), never floats. Layout is synchronous per keystroke
  by design (≈0.2 ms on 330 pages); don't add threads without a profile.

## Conventions

- Modern keymap follows emacs/macOS readline movement; the palette is
  `Ctrl+Shift+P` there (`Ctrl+K` in classic) — a deliberate spec deviation,
  see DESIGN.md §10a.
- Commit messages end with the Co-Authored-By / Claude-Session trailers the
  session provides. Don't push; there is no remote yet.
