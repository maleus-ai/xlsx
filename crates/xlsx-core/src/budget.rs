//! The decompressed-byte budget.
//!
//! Why this runs before anything else opens the workbook: a bound placed *above*
//! a reader protects nothing during the phases the reader runs before handing
//! back its first row, and for an XLSX those phases build four tables — package
//! relationships, shared strings, styles, workbook — every one of them sized by
//! the file and none of them by the number of rows. Measured on `exceljs`, on
//! workbooks carrying a single data row: 324 KB of upload reach 423 MB of RSS
//! through `sharedStrings`, 0.50 MB reach 2.5 GB through the workbook model.
//!
//! So the archive is walked once, up front, and every entry is inflated into a
//! sink through a fixed buffer. Nothing is retained, the peak stays flat, and a
//! hostile archive is refused before a single table is allocated.
//!
//! The count is taken on the bytes that actually leave the inflater. The sizes
//! an archive declares are not evidence: `zip` checks an entry's CRC but never
//! caps its output at the declared length, so an archive is free to announce 64
//! bytes for an entry that deploys 64 MB. Only the real expansion is counted.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use zip::result::ZipError;
use zip::ZipArchive;

use crate::directory::enforce_entry_budget;
use crate::error::ReadError;

/// Fixed inflate buffer. The point of the walk is that peak memory is this
/// buffer, whatever the archive claims to hold.
const CHUNK_BYTES: usize = 64 * 1024;

/// A running total against a ceiling.
pub(crate) struct Budget {
    limit: u64,
    used: u64,
}

impl Budget {
    pub(crate) fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }

    fn charge(&mut self, bytes: u64, entry: &str) -> Result<(), ReadError> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit {
            return Err(ReadError::DecompressedBudgetExceeded {
                limit: self.limit,
                entry: format!("{entry:?}"),
            });
        }
        Ok(())
    }
}

/// Inflate one entry into memory, charging every byte that leaves the inflater
/// and stopping the moment the budget is crossed.
///
/// Used by the listing path, which reads two small parts rather than the whole
/// archive. The entry is materialised because it has to be parsed; the budget
/// is what keeps that bounded.
pub(crate) fn inflate_entry_within<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    budget: &mut Budget,
) -> Result<Vec<u8>, ReadError> {
    let mut entry = archive.by_name(name).map_err(|error| match error {
        ZipError::FileNotFound => ReadError::corrupt(format!("the archive has no {name:?} part")),
        other => map_zip_error(other),
    })?;

    let mut content = Vec::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];

    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|error| ReadError::Corrupt {
                detail: format!("cannot inflate {name:?}: {error}"),
            })?;
        if read == 0 {
            break;
        }

        budget.charge(read as u64, name)?;
        content.extend_from_slice(&buffer[..read]);
    }

    Ok(content)
}

/// Walk every entry of the archive, inflating each into a sink, and stop the
/// moment the cumulative output crosses `limit`.
///
/// Returns the total number of bytes the archive expands to, which is only ever
/// reached for archives that stay within budget.
pub(crate) fn enforce_decompressed_budget(path: &Path, limit: u64) -> Result<u64, ReadError> {
    // Before the archive is opened at all: opening it indexes every entry it
    // declares, and that cost is neither inflated nor bounded by anything below.
    enforce_entry_budget(path, limit)?;

    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;

    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;

    let mut total: u64 = 0;
    let mut buffer = vec![0u8; CHUNK_BYTES];

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(map_zip_error)?;
        let name = entry.name().to_owned();

        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| ReadError::Corrupt {
                    detail: format!("cannot inflate {name:?}: {error}"),
                })?;
            if read == 0 {
                break;
            }

            total += read as u64;
            if total > limit {
                return Err(ReadError::DecompressedBudgetExceeded { limit, entry: name });
            }
        }
    }

    Ok(total)
}

pub(crate) fn map_zip_error(error: ZipError) -> ReadError {
    match error {
        ZipError::InvalidArchive(_) | ZipError::UnsupportedArchive(_) => ReadError::NotAnArchive,
        ZipError::FileNotFound => ReadError::corrupt("archive is missing a part it declares"),
        ZipError::Io(error) => ReadError::Io {
            detail: error.to_string(),
        },
        other => ReadError::corrupt(other.to_string()),
    }
}
