//! Bounded streaming reader for XLSX worksheets.
//!
//! Three properties, none of them negotiable, and each one owed to an incident
//! or a measurement:
//!
//! 1. **Streaming at bounded memory.** A client application died in production
//!    on an import. On 200 000 rows by 20 columns a buffered read asks for 2.1
//!    to 2.5 GB of peak RSS; the production VMs have 4 GB.
//! 2. **Correct date typing.** In an XLSX a date is a number plus a `numFmt`
//!    declared in `xl/styles.xml`. Without that table `45376` cannot be told
//!    apart from a quantity, and a reader that does not expose it writes numbers
//!    where the business expects dates — a wrong value in the database, not an
//!    exception.
//! 3. **Multiple sheets.** A business workbook has tabs: data, parameters,
//!    legend, notes.
//!
//! What this crate deliberately does not do is decide anything about your data.
//! It knows nothing of headers, of records, of which sheet a given import is
//! allowed to use. It opens an archive, applies bounds, and hands back typed
//! rows; everything above that is product policy and belongs where it can be
//! read and tested as product policy.
//!
//! ```no_run
//! use std::path::Path;
//! use xlsx_core::{ReaderOptions, XlsxReader};
//!
//! let mut reader = XlsxReader::open(
//!     Path::new("upload.xlsx"),
//!     ReaderOptions {
//!         max_decompressed_bytes: 512 * 1024 * 1024,
//!         max_rows: 1_000_000,
//!     },
//! )?;
//!
//! let sheet = reader.sheets()[0].name.clone();
//! reader.select(&sheet)?;
//!
//! while let Some(rows) = reader.next_batch(1_000)? {
//!     for row in rows {
//!         // row: Vec<CellValue>, values at their column index
//!         let _ = row;
//!     }
//! }
//! # Ok::<(), xlsx_core::ReadError>(())
//! ```

#![deny(missing_docs)]
#![warn(clippy::all)]

mod budget;
mod cursor;
mod directory;
mod error;
mod list;
mod reader;
mod value;
mod writer;

pub use error::ReadError;
pub use reader::{ReaderOptions, SheetInfo, XlsxReader};
pub use value::CellValue;
pub use writer::{
    validate_sheet_name, WriteError, WriterOptions, XlsxWriter, MAX_COLUMNS, MAX_ROWS, MAX_SHEETS,
};
