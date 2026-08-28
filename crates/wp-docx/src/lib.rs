//! `.docx` reading and writing for `wp`, built for lossless round trips.
//!
//! The reader converts what it understands into the `wp-core` model and keeps
//! everything else verbatim: unknown run/paragraph properties, unknown
//! elements, whole body-level blocks, and every zip entry it doesn't parse.
//! The writer emits the model and copies the preserved material back
//! unchanged. See DESIGN.md §6.

pub mod package;
pub mod read;
pub mod table;
pub mod write;
pub mod xml;

pub use package::{DocxPackage, PackageEntry};
pub use read::{read, read_bytes, table_cells, Ctx, Loaded, Warning};
pub use write::{render_paragraph_xml, write, write_bytes};


/// Round-trip helper for tests and the corpus gate: read then write, returning
/// the resulting bytes.
pub fn roundtrip_bytes(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let loaded = read_bytes(input)?;
    write_bytes(&loaded.doc, Some(&loaded.package))
}
