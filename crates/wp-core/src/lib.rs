//! wp-core: the document model, editing, and layout engine for `wp`.
//!
//! No terminal, no file formats — everything here is testable in isolation.

pub mod document;
pub mod edit;
pub mod editor;
pub mod layout;
pub mod metrics;
pub mod metrics_tables;
pub mod model;
pub mod numbering;
pub mod reveal;
pub mod search;

pub mod style;
pub mod text;

pub use document::Document;
pub use editor::{Editor, Fragment};
pub use model::*;
pub use numbering::{ListKind, ListLabel, Numbering};

pub use style::{Style, StyleKind, StyleSheet};
