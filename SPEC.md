# `wp` — Product Specification

**Status:** Draft 1 · **Date:** 2026-08-28
**Engineering design:** [DESIGN.md](./DESIGN.md)

---

## 1. Summary

`wp` is a word processor that runs in a terminal.

It does the things people actually need a word processor for — styled documents,
real page layout, tables, footnotes, a table of contents, tracked page counts —
and it opens and saves `.docx` natively, so documents move between `wp` and Word,
Google Docs, and LibreOffice without a conversion step and without damage.

Its interaction model comes from WordPerfect 5.1: a blank screen, a complete
keyboard language, and *Reveal Codes* — the ability to see and directly edit the
formatting instructions inside a document. Its discoverability comes from modern
tools: a command palette, fuzzy navigation, live search.

**In one sentence:** the word processor for people who work in a terminal but
have to deliver a `.docx`.

---

## 2. The problem

There is a specific, common, and currently unserved situation: **your working
environment is a terminal, and your deliverable is a formatted document.**

A consultant edits over SSH on a client's jump box. A developer has to turn an
RFC into a document the legal team will redline. A contractor gets a statement of
work as `.docx` and has to return it as `.docx`. A novelist's publisher accepts
Word files and nothing else. In every case the person is fast and comfortable at
a keyboard in a terminal, and the moment formatting or `.docx` enters the picture
they have to leave it.

### 2.1 Why today's options don't close the gap

| Approach | Where it breaks |
|---|---|
| **Markdown + pandoc** | No page model — you cannot know what page you're on, control where a page breaks, or set margins. Tables can't merge cells. Footnote placement is whatever the backend decides. You cannot open the `.docx` a reviewer sends back; the round trip destroys the document. |
| **Word / Google Docs** | Requires leaving the terminal, requires a GUI, and doesn't work over SSH or on a constrained machine. Formatting is opaque: when a paragraph misbehaves there is no way to see *why*. Mouse-dependent for many operations. |
| **Terminal editors (vim, emacs, helix)** | These are text editors. No pagination, no styles, no `.docx`. Org-mode and AUCTeX get closer but are still authoring source that must be compiled. |
| **LaTeX** | A compile-and-preview loop, not direct manipulation. Excellent output, but its `.docx` export is poor and it cannot consume a `.docx` at all — so it fails the moment someone sends you one. |
| **WordPerfect for DOS (in DOSBox)** | People genuinely still do this. It works and its users love it. It cannot read `.docx`, cannot handle Unicode, and is a dead end. |

The gap is direct-manipulation word processing — keyboard-driven, in a terminal,
with `.docx` as a native format rather than an export target.

### 2.2 Why WordPerfect is the right model

Not nostalgia. Three of its design decisions solve problems that are still
unsolved in current word processors:

1. **Reveal Codes.** "Why is this paragraph indented?" is a question Word and
   Google Docs cannot answer. WordPerfect could: you opened the codes view and
   saw the exact instruction, and deleted it. This remains the single most-cited
   reason people never left WordPerfect, and it is why it survives in legal
   practice thirty years on.
2. **The blank screen.** The document filled the display. Everything else was
   summoned and dismissed. This is the correct default for a writing tool and it
   maps perfectly onto a terminal.
3. **A complete keyboard language.** Every operation had a key. Nothing required
   a mouse. A terminal app needs exactly this, and WordPerfect's is the most
   thoroughly worked-out one ever shipped.

What it got wrong — undiscoverable keys, no way to find a feature you couldn't
name — is precisely what a command palette fixes.

---

## 3. Users

### 3.1 Primary

**The terminal-resident professional.** Engineers, consultants, analysts, and
technical staff who spend the day in a terminal and periodically must produce or
revise a formatted document for someone who uses Word. They are fast at a
keyboard, allergic to context-switching, and often working on a remote machine.
They don't need every Word feature — they need the document to open correctly,
edit comfortably, and come back out intact.

*What success looks like for them:* they never open Word again for routine
document work.

**The long-form writer.** Novelists, journalists, technical authors. They want a
distraction-free drafting environment — which the terminal is, natively — but
their editor, agent, or publisher requires `.docx`. Today they draft in one tool
and convert in another, and the seam costs them.

*What success looks like for them:* draft, revise, and deliver in one place, with
an accurate word count and page count throughout.

**The formatting-control specialist.** Legal, policy, proposals, grants,
compliance. Documents with inherited formatting, strict layout requirements, and
real consequences when a page break lands wrong. They spend meaningful time
fighting formatting they didn't create. This is WordPerfect's surviving
constituency and they are underserved by everything currently shipping.

*What success looks like for them:* Reveal Codes lets them diagnose and fix any
formatting problem in under a minute, on any document, including ones produced by
someone else in Word.

### 3.2 Secondary

- **Academics and researchers** who need footnotes, cross-references, and a
  generated table of contents, and whose journals want `.docx`.
- **People on constrained or remote environments** — SSH sessions, low bandwidth,
  old hardware, single-board machines, Linux on a Chromebook.
- **WordPerfect users** still running DOSBox, who want their editing model on a
  modern machine with Unicode and `.docx`.

### 3.3 Not the target

Anyone whose documents are primarily visual — brochures, newsletters, posters,
anything where the layout *is* the product. `wp` renders in a character grid;
it can produce a correct page but not a beautiful one, and desktop publishing is
explicitly out of scope (§9.2).

---

## 4. Jobs to be done

The features in §7 exist to serve these. Anything that serves none of them is a
candidate for cutting.

| # | Job | Frequency |
|---|---|---|
| **J1** | *Someone sent me a `.docx`. I need to read it, change some things, and send it back without breaking it.* | Constant — the core job |
| **J2** | *I need to write a substantial document from scratch and deliver it as `.docx` or PDF.* | Common |
| **J3** | *This document's formatting is wrong and I can't tell why. I need to find the cause and fix it.* | Common, and currently very painful |
| **J4** | *I need to add scholarly apparatus — footnotes, a table of contents, cross-references, captions — and have it stay correct as I edit.* | Periodic, high-stakes |
| **J5** | *I have Markdown (notes, an RFC, a README) that now needs to be a real document.* | Periodic |
| **J6** | *I need to know exactly how long this is — words, pages — and where the page breaks fall.* | Continuous background need |
| **J7** | *I'm drafting and I want nothing on the screen but my words.* | Continuous background need |

---

## 5. Product principles

These are the tiebreakers. When a decision is contested, the earlier principle
wins.

1. **Never damage a document.** A file that goes through `wp` comes out
   intact — including the parts `wp` doesn't understand. Fidelity outranks
   features; we would rather not support something than support it lossily.
2. **The page count is the truth.** If `wp` says page 12, printing produces page
   12 and Word agrees. A word processor that guesses at pagination is a text
   editor with extra steps.
3. **The screen is for writing.** Everything that isn't the document is off by
   default and one key away.
4. **Nothing is mouse-only, nothing is menu-only.** Every capability is reachable
   from the keyboard, and every capability is findable by typing a plausible name
   into the palette.
5. **Formatting is never a mystery.** Any formatting the user can see, they can
   inspect and remove.
6. **Typing never stutters.** Responsiveness is a feature, and the first one
   users notice.

---

## 6. Experience

### 6.1 The default screen

Text and one status line. Nothing else.

```
                                                                              
   The quarterly results exceeded projections in every region except EMEA,     
   where currency effects reduced reported growth by roughly four points.      
                                                                              
   Regional detail                                                            
                                                                              
   North America grew 14% year over year, led by renewals in the mid-market    
   segment. The enterprise pipeline closed the quarter at $42M, up from        
   $31M, though weighted conversion remained flat.█                            
                                                                              
                                                                              
 Q3-REPORT.DOCX *                      Doc 1  Pg 4  Ln 2.7"  Pos 3.4"         
```

The status block is WordPerfect's, and it earns its place: it answers "where am
I in this document, in the terms that matter for a printed page" without
occupying any of the writing area. `*` marks unsaved changes.

Transient indicators appear alongside it as needed — `Select`, `Typeover`,
`Macro Rec`, `Spell`, live word count.

### 6.2 Finding things: the command palette

`Ctrl+K`. The answer to WordPerfect's one genuine failing.

```
┌─ Command ───────────────────────────────────────────────────────┐
│ > fooot                                                          │
├──────────────────────────────────────────────────────────────────┤
│   Insert Footnote                                    Ctrl+F7     │
│   Footnote Options…                                              │
│   Go to Next Footnote                                            │
│   Convert Footnotes to Endnotes                                  │
│   Format ▸ Footer ▸ Edit Footer                      Shift+F8    │
└──────────────────────────────────────────────────────────────────┘
```

Two product requirements here matter more than the search quality:

- **It always shows the keybinding.** The palette is the teaching mechanism for
  the keyboard. Users are expected to graduate off it for the things they do
  often, and it should actively push them there.
- **It is complete.** Every capability appears. If something is only reachable
  from a menu or a chord, that's a bug.

Typed prefixes switch modes: `>` commands, `@` jump to a heading, `#` jump to a
page, `/` find, `?` help.

### 6.3 Seeing the formatting: Reveal Codes

`Alt+F3`. Splits the screen; the lower pane shows the same text with the
formatting instructions made visible and **editable**.

```
┌─────────────────────────────────────────────────────────────────┐
│ The quarterly results exceeded projections in every region       │
│ except EMEA, where currency effects reduced reported growth.     │
├──────────────────────────────────────────────── Reveal Codes ───┤
│ [Style:Body]The quarterly results exceeded projections in every  │
│ region except [BOLD]EMEA[bold], where currency effects reduced   │
│ reported growth.[HRt]                                            │
│ [HRt]                                                            │
│ [Style:Heading 2][Bookmark:regional]Regional detail[HRt]         │
└─────────────────────────────────────────────────────────────────┘
```

The product promise is that **deleting a code removes its effect** — put the
cursor on `[BOLD]`, press Delete, the text is no longer bold and both halves of
the pair are gone. Codes can be selected, copied, and pasted to apply formatting
elsewhere. Codes the layout computed rather than the user inserted (soft returns
`[SRt]`, soft page breaks `[SPg]`) are shown but not editable — and seeing an
`[SRt]` where you expected an `[HRt]` explains most "why won't this line stay
put" confusion on sight.

This is the feature that makes J3 tractable, and it should be treated as the
product's differentiator, not as a power-user affordance.

### 6.4 Two ways to see the page

**Draft view** (default) is continuous text wrapped to the terminal — comfortable
for writing. Page boundaries appear as a labelled rule so you always know where
you are:

```
   the committee will reconvene in the spring to review the
   remaining proposals and issue a final recommendation.
─────────────────────────────── Page 4 ───────────────────────────────
   Appendix A — Methodology
```

**Page view** shows the actual page — margins, headers and footers in position,
footnotes at the bottom, real line breaks:

```
                                                       Draft 3
   ┌────────────────────────────────────────────────────────┐
   │  Regional detail                                        │
   │                                                         │
   │  The quarterly results exceeded projections in every    │
   │  region except EMEA, where currency effects reduced     │
   │  reported growth by roughly four points.¹               │
   │                                                         │
   │  ───────────────                                        │
   │  ¹ At constant currency, growth was 11.2%.              │
   │                                    12                   │
   └────────────────────────────────────────────────────────┘
```

Critically, **the page number is correct in both views** — `wp` computes real
pagination whether or not it's drawing the page. Draft view is a display choice,
not a fidelity compromise.

*Known limitation, stated up front:* a terminal has fixed-width character cells
and real documents use proportional fonts. Page view shows where lines truly
break, which means lines will look ragged — a line that fills the page in points
won't fill it in character cells. `wp` shows the truth and lets it look uneven,
because principle 2 outranks appearance. Users who prefer the screen and the page
to agree exactly can opt into monospace layout at the cost of print fidelity.

### 6.5 Two keyboards

The keymap is a setting, and both maps are complete.

**Classic** is WordPerfect 5.1: `F6` bold, `F8` underline, `F10` save, `F7` exit,
`Alt+F3` reveal codes, `Shift+F8` format, `Ctrl+F7` footnote, `Alt+F4` block.
Muscle memory from 1989 works.

**Modern** is what everyone else expects: `Ctrl+S`, `Ctrl+B`, `Ctrl+Z`,
`Ctrl+F`, `Shift+Arrows` to select. **The F-keys keep their classic meanings as a
second layer**, so the two maps coexist and a modern user can learn the classic
one gradually rather than choosing up front.

An on-screen F-key legend (off by default, one toggle away) reproduces the
plastic template card that shipped with every WordPerfect keyboard — the fastest
way to learn the classic map.

Everything is rebindable.

**Both maps ship in v1. Neither is an add-on or a plugin.** This is close to free
to build — bindings are a lookup table over a command registry that has to exist
anyway — and it decides who the product is for. Classic-only makes `wp` a
nostalgia artifact; modern-only throws away the audience most likely to adopt it
first. Shipping both, with classic F-keys layered underneath the modern map, is
what lets the two audiences share one product.

WordPerfect's *modal* behaviours — `Esc` as a repeat-count prefix (`Esc 8 ↓`),
and `Alt+F4`-then-arrows to extend a block — ship with the classic keymap only.
They are authentic and fast, but they surprise modern users badly enough that
they'd generate bug reports rather than converts. They are not independently
toggleable: a setting that granular is one nobody would find, and every
additional keyboard mode multiplies the states a bug report can arrive in.

### 6.6 First run

A new user's first thirty seconds decide whether they come back.

- Opening `wp` with no arguments shows a blank document and a single dismissable
  hint line: `Ctrl+K for commands · F1 for help · Alt+= for menus`.
- Opening `wp report.docx` shows the document immediately, before layout of the
  whole file has finished, with the page count filling in (`Pg 4 / ~120`).
- First launch asks one question — classic or modern keys — with a one-line
  explanation and a note that it's changeable later. No wizard, no account, no
  telemetry prompt.
- `wp --tutor` opens an interactive tutorial document that teaches by having the
  user edit it. Six lessons, each under three minutes.

---

## 7. Requirements

Written as user capabilities. Each has acceptance criteria that can be checked
without reference to how it's built.

### 7.1 P0 — required for v1

**Document exchange**

| ID | Capability | Acceptance |
|---|---|---|
| P0-1 | Open a `.docx` and see it rendered correctly | 95% of the test corpus (§10.2) renders with the same page count as Word, and no visible content missing |
| P0-2 | Save a `.docx` without damaging it | Open-then-save with no edits produces a file Word opens with zero differences; features `wp` doesn't support survive untouched |
| P0-3 | Be told what `wp` can't edit | **One** warning per document on open, summarizing every unsupported construct in a single line — e.g. *"7 comments and 3 tracked changes are preserved but not editable. Ctrl+K → Warnings for detail."* Never one warning per occurrence |
| P0-4 | Open and save Markdown | CommonMark + GFM tables and footnotes; saving a `.docx` as `.md` warns once, specifically, about what will be lost |
| P0-5 | Open and save plain text | Encoding detected automatically; configurable wrapping on save |
| P0-6 | Never lose work | Autosave every 30s; after a crash or lost SSH connection, reopening offers recovery to within one autosave interval |

**Writing and editing**

| ID | Capability | Acceptance |
|---|---|---|
| P0-7 | Type, select, cut, copy, paste, undo | Undo groups by word, not keystroke; paste offers formatted or plain; last 16 cuts retrievable |
| P0-8 | Apply character formatting | Bold, italic, underline (all styles), strikethrough, super/subscript, caps, font, size, color, highlight |
| P0-9 | Apply paragraph formatting | Alignment, all indent types, spacing, line spacing, tab stops incl. decimal and leaders, keep-with-next, widow/orphan control, borders, shading |
| P0-10 | Use named styles | Apply, create, modify, and inherit; a style browser shows what each style inherits and what direct formatting is overriding it at the cursor |
| P0-11 | Make lists | Bulleted and numbered, nine levels, all numbering formats, restart control; round-trips as a Word list, not as literal text |
| P0-12 | Find and replace | Live incremental find with match count; regex with capture groups; whole word and case options; **search by formatting** ("find bold text", "find text in Heading 2"); **search by code** ("find the next page break"); preview all matches before replace-all |
| P0-13 | Check spelling | Incremental, in the background, with squiggle underlines; suggestions inline; per-document, per-user, and per-project dictionaries; language is per-run so mixed-language documents check correctly |

**Structure and layout**

| ID | Capability | Acceptance |
|---|---|---|
| P0-14 | Know what page I'm on | `Pg`/`Ln`/`Pos` accurate at all times in both views; agrees with Word's page count on the corpus |
| P0-15 | Switch between draft and page view | One key; cursor position preserved; page numbers identical in both |
| P0-16 | Control the page | Size, orientation, margins, columns, headers and footers (first/odd/even variants), explicit page breaks, sections |
| P0-17 | Work with tables | Insert, navigate by Tab, insert/delete rows and columns, merge and split cells, resize, header row repeat across pages, alignment, borders, shading, sort by column, `SUM`/`AVERAGE` formula cells |
| P0-18 | See and fix formatting | Reveal Codes per §6.3, including editing codes |

**Reference apparatus**

| ID | Capability | Acceptance |
|---|---|---|
| P0-19 | Footnotes and endnotes | Auto-numbered, correctly placed at the page bottom, restartable per page or section, convertible between the two, renumber automatically on edit |
| P0-20 | Table of contents | Generated from headings or explicit marks; level selection, leaders, page numbers; regenerates on demand; Word regenerates it identically |
| P0-21 | Cross-references | To headings, bookmarks, figures, tables, footnotes, page numbers; broken references are listed and highlighted, never silently wrong |
| P0-22 | Captions | Auto-numbered by category (Figure, Table, Equation); inserting one mid-document renumbers the rest |
| P0-23 | Index | Mark entries and subentries; generate with page ranges and *see also* cross-references |
| P0-24 | Bookmarks | Named, navigable from the palette |

**Images**

| ID | Capability | Acceptance |
|---|---|---|
| P0-25 | See images in the document | Rendered inline on terminals that support graphics; a correctly-sized labelled placeholder box otherwise, never a blank gap |
| P0-26 | Insert, size, and position images | Inline or floating with text flow around; alt text; round-trips to `.docx` at the right size and position |

**Interaction**

| ID | Capability | Acceptance |
|---|---|---|
| P0-27 | Find any command by typing its name | Palette covers 100% of capabilities and shows each one's keybinding |
| P0-28 | Use either keyboard | Classic and modern maps both complete; F-keys work in both; everything rebindable |
| P0-29 | Work entirely without a mouse | No capability is mouse-only. Mouse support exists and is optional |
| P0-30 | Record and replay a sequence of actions | Record, stop, play; saved macros are human-readable, bindable to keys, and invocable from the palette |

### 7.2 P1 — v1.1

| ID | Capability |
|---|---|
| P1-1 | Export to PDF directly, with embedded fonts |
| P1-2 | Open and save RTF |
| P1-3 | Open and save ODT |
| P1-4 | Export to HTML |
| P1-5 | Outline pane — navigate and reorder the document by heading |
| P1-6 | Document comparison — show what changed between two files |
| P1-7 | Mail merge from CSV |

### 7.3 P2 — v2

| ID | Capability |
|---|---|
| P2-1 | Write and reply to comments |
| P2-2 | Author tracked changes; accept and reject them |
| P2-3 | Read legacy WordPerfect `.wpd` files |
| P2-4 | Equation editing |
| P2-5 | Right-to-left and complex-script editing |

### 7.4 Handling what we don't support (v1)

Comments and tracked changes are the most common things `wp` v1 can't author,
and they arrive in real documents constantly. The behaviour is specified rather
than left to chance:

- Both are **preserved perfectly** through a round trip.
- Both are **visible**: comments show a gutter marker and open in a read-only
  pane; tracked changes render as insertions and deletions when markup display
  is on.
- The user is **warned exactly once**, on open, in one line, covering everything
  unsupported in the document at once. A file with forty tracked changes
  produces one notice, not forty. Detail lives in the warnings pane for anyone
  who wants it; the default experience is a single sentence and then silence.
- Editing *inside* a tracked change or across a comment's anchor is **refused
  with an explanation**, not silently allowed. This is the one place where
  blocking an edit beats performing a lossy one, and users are told why. The
  refusal message is the exception to warn-once — it appears whenever the user
  attempts the edit, because it explains a thing that just happened.

---

## 8. Non-functional requirements

Stated as things the user perceives.

| Quality | Requirement |
|---|---|
| **Feels instant** | Typing never visibly lags, even in a 500-page document. Scrolling is smooth. No operation in normal editing blocks the cursor. |
| **Opens immediately** | A 500-page document is readable and editable in under a second; full pagination completes in the background with the page count filling in. |
| **Never loses work** | Autosave plus crash recovery. Killing the terminal mid-sentence loses at most 30 seconds. |
| **Never corrupts** | Zero file-corruption incidents is the standard, not a target. Any corruption bug is a release blocker. |
| **Works anywhere** | Any terminal from a bare Linux console to a modern GPU terminal. Degrades to 16 colors and ASCII box-drawing without breaking. Usable over a high-latency SSH link. |
| **Installs trivially** | One binary, no runtime, no dependencies. `brew install wp`, `cargo install wp`, or download and run. |
| **Modest footprint** | Under 250 MB resident on a 500-page document. Runs on a Raspberry Pi. |
| **Private** | No network access. No telemetry. No account. The program never phones home. |

---

## 9. Scope

### 9.1 In scope, stated plainly

Everything in §7. The shape of v1 is: **you can do your entire `.docx` workflow
without leaving the terminal** — receive, read, edit, format, structure,
reference, and return — plus Markdown in and out.

### 9.2 Out of scope, and why

| Not doing | Reason |
|---|---|
| **Real-time collaboration** | Requires a server, an account system, and a conflict-resolution model. It's a different product. Google Docs wins here and should. |
| **Desktop publishing** | Text boxes, wrapped shapes, precise image positioning, typographic control. A character grid can't show it and the target users don't need it. |
| **Charts, SmartArt, diagrams** | Authoring these in a terminal is worse than the alternative. They are preserved on round-trip; they are not created. |
| **A GUI** | The terminal is the point, not a limitation to be escaped. |
| **A macro programming language** | Record-and-replay covers what macros are actually used for. A scripting language is a large surface for a small return. |
| **Cloud storage integration** | `wp` edits files. Syncing them is the filesystem's job. |
| **AI writing features** | Not in v1. Possibly never. Not what this product is for. |

### 9.3 Release plan, by what the user can do

Sequenced so each release is genuinely usable, not so each is a convenient
engineering chunk.

| Release | The user can… | Ships |
|---|---|---|
| **0.1 Preview** | Write and format a document, save it as `.docx`, and have Word open it correctly | Editing, character and paragraph formatting, styles, `.docx` write, draft view, both keymaps, palette |
| **0.2 Round-trip** | Take a `.docx` someone sent them, edit it, and send it back safely | `.docx` read with full fidelity, lists, find/replace, undo, autosave, Markdown |
| **0.3 Documents** | Produce a real structured document | Tables, sections, headers/footers, page view, accurate pagination, Reveal Codes |
| **1.0** | Do everything the job requires | Footnotes, TOC, cross-references, captions, index, images, spell check, macros, tutorial |
| **1.1** | Deliver in more formats | PDF, RTF, ODT, HTML, outline pane, compare |
| **2.0** | Participate in review workflows | Comments, tracked changes, `.wpd` |

The 0.2 milestone is the real one. Until a document can make a full safe round
trip, `wp` isn't usable for the primary job and shouldn't be recommended to
anyone.

---

## 10. How we'll know it's working

### 10.1 The bar for v1

A user can be handed a `.docx` by a colleague, do a substantial revision entirely
in `wp`, and send it back — and the colleague cannot tell which tool was used.

### 10.2 Measurable

| Metric | Target |
|---|---|
| Corpus documents round-tripping with no detectable change | 100% of untouched files; **this is a release gate, not a goal** |
| Corpus documents whose page count matches Word exactly | ≥ 95% |
| Corpus documents rendering with no missing content | 100% |
| File corruption reports in the field | 0 |
| Cold open, 500-page document, to editable | < 1s |
| Time for a new user to complete the tutorial | < 20 min |
| Crash rate | < 1 per 1,000 sessions |

The test corpus is ~60 real `.docx` files spanning Word, Google Docs, and
LibreOffice output, plus deliberately pathological cases. It is the primary
quality instrument for the whole product.

### 10.3 Qualitative signals

- Users report having stopped opening Word for routine work.
- Reveal Codes is cited unprompted as a reason they use it — this validates the
  central product bet.
- Bug reports are about missing features, not about damaged files. Complaints
  about fidelity mean the foundation is wrong; complaints about scope mean it's
  right and incomplete.
- WordPerfect users say the keyboard feels correct.

---

## 11. Product bets

Assumptions this product rests on. Each could be wrong, and each is worth
testing early rather than discovering late.

1. **People will accept approximate visual fidelity in exchange for keyboard
   speed and staying in the terminal.** A terminal cannot show a document the way
   a GUI can. The bet is that the target user cares far more about being fast and
   in one place than about seeing proportional type. *Test:* put page view in
   front of ten target users in the 0.3 preview and watch whether the raggedness
   bothers them or they stop noticing.

2. **`.docx` fidelity is the make-or-break feature.** The bet is that one damaged
   document loses a user permanently, and that a tool which is *trustworthy* with
   files will be forgiven a lot of missing features. Everything in §5 principle 1
   follows from this. *Test:* it's the release gate; if fidelity slips, nothing
   else matters.

3. **Reveal Codes is a differentiator, not a nostalgia item.** The bet is that
   "see and delete the formatting instruction" solves a problem people have today
   in Word and Google Docs and will switch tools for. *Test:* whether it appears
   unprompted in user feedback.

4. **The two-keymap approach works, rather than splitting the product.** The bet
   is that layering classic F-keys under a modern map lets both audiences use one
   product and lets modern users migrate gradually. *Test:* whether modern-keymap
   users adopt any F-keys within a month.

5. **The market is big enough.** Terminal-resident people who must produce
   `.docx` is a real population but an unmeasured one. *Test:* the 0.1 preview's
   reception, particularly whether it spreads beyond the WordPerfect-nostalgia
   audience — which will show up first and is not the actual market.

---

## 12. Risks

| Risk | Impact | Response |
|---|---|---|
| **`.docx` fidelity is harder than estimated** | Fatal — the primary job fails | Build the corpus and the round-trip gate *before* building features. Treat the 0.2 milestone as the schedule's real long pole. |
| **Page counts don't match Word** | Undermines principle 2 and the product's credibility | Requires real font metrics and correct font substitution. Measure against the corpus continuously from 0.3. |
| **Page view looks bad enough that people won't use it** | Loses the "word processor, not editor" positioning | Bet 1. Prototype early, and keep a monospace layout mode as the fallback if raggedness proves intolerable. |
| **The audience is smaller than believed** | Product is fine, market isn't | Cheap to test with the 0.1 preview. Adjacent expansion (academics, legal) exists if the core is narrow. |
| **Scope for v1 is large** | Slipped dates, or a thin release | The release plan (§9.3) is explicitly sequenced so 0.2 and 0.3 are independently useful. If 1.0 must be cut, the index and macros go first. |
| **Terminal graphics fragmentation** | Images look bad or inconsistent | Placeholder boxes are a first-class supported experience, not a failure mode. Never leave a blank gap. |

---

## 13. Decisions and open questions

### 13.1 Decided

| # | Question | Decision |
|---|---|---|
| D1 | Ship one keymap or two? | **Two, both complete, both in v1** — classic and modern, with classic F-keys layered under the modern map. The cost is near zero; the audience consequence is large. §6.5 |
| D2 | How much WordPerfect modality to keep? | `Esc`-repeat and `Alt+F4` block ship **with the classic keymap only**, not as independent toggles. Follows from D1 — the keymap is the unit of choice, not the individual behaviour. §6.5 |
| D3 | How loudly to surface unsupported constructs? | **Warn once per document, on open, in one line**, covering everything at once. Detail in the warnings pane. A file with forty tracked changes produces one notice. §7.4 |
| D4 | Dedicated features for the legal segment? | **No.** Line numbering, pleading paper (the numbered-margin format US courts require), and auto-numbered contract paragraphs are cheap to build and matter enormously to law firms — but they serve document *conventions* we haven't validated demand for. Cut from scope; revisit only if that audience arrives on its own. **This does not cut the formatting-control persona (§3.1)**, who is served by Reveal Codes and precise layout control — capabilities already in P0 and shared with every user. We're declining the niche conventions, not the user. |

### 13.2 Still open

1. **Should the default view be draft or page?** Draft is more comfortable to
   write in; page makes the product's central claim — real pagination — visible
   in the first three seconds. Draft is currently specified, but a new user's
   first impression may argue for opening in page view and letting them settle
   into draft.
2. **Markdown lossiness UX.** Saving `.docx` as `.md` drops page geometry, fonts,
   and most styles. A one-time warning is specified. D3 leans this toward
   warn-once-and-proceed for consistency, but the asymmetry is real: an
   unsupported construct on *open* is informational, whereas a lossy *save*
   destroys something. Should `wp` require an explicit flag or confirmation
   instead?
3. **Pricing and licensing.** Unaddressed. Open source, open core with a paid
   tier, or commercial? Affects the roadmap and the contribution model, and
   should be settled before 1.0.
