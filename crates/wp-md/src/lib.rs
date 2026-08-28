//! Markdown in and out (SPEC §7.1 P0-4): CommonMark plus GFM tables,
//! strikethrough, task lists and footnotes.
//!
//! Import builds a real document: headings become heading styles, lists
//! become Word lists with numbering definitions, links become hyperlinks
//! with relationships, footnotes become footnote references and bodies,
//! tables become (for now) preserved table blocks. Export is the reverse,
//! and reports in one line what the Markdown cannot carry.

pub mod export;
pub mod import;

pub use export::{to_markdown, Export};
pub use import::from_markdown;
