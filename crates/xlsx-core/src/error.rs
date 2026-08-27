//! Reading errors.
//!
//! The set is closed and every variant carries what a caller needs to build a
//! message on its own: a reader that says only "corrupt" forces the caller to
//! re-open the file to find out anything.

use std::fmt;

/// Everything that can go wrong while reading a workbook.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadError {
    /// The file is not a ZIP archive, or is too damaged for its central
    /// directory to be read.
    NotAnArchive,

    /// No sheet by that name. `available` is the full list, in workbook order,
    /// so the caller can name the alternatives without re-opening the file.
    SheetNotFound {
        /// Sheet name that was asked for.
        name: String,
        /// Sheet names the workbook actually declares.
        available: Vec<String>,
    },

    /// The archive expands past `max_decompressed_bytes`.
    DecompressedBudgetExceeded {
        /// Budget the caller set, in bytes.
        limit: u64,
        /// What was being read when the budget ran out: an archive member, or
        /// the sheet whose own geometry asked for more blank values than the
        /// budget allowed.
        entry: String,
    },

    /// The archive holds so many entries that indexing them alone would cost
    /// more than the byte budget allows.
    ///
    /// Entries cost memory before any of them is inflated, so this is checked
    /// on the archive's own directory, before it is opened. `limit` is what the
    /// byte budget works out to in entries.
    TooManyEntries {
        /// Entries the budget allows.
        limit: u64,
        /// Entries the archive declares.
        count: u64,
    },

    /// The selected sheet holds more rows than `max_rows`. Raised on the row
    /// that crosses the budget, not at the end of the sheet.
    RowBudgetExceeded {
        /// Budget the caller set, in rows.
        limit: u64,
    },

    /// The archive is a ZIP but not a readable workbook: a missing part, XML
    /// that does not parse, a cell reference that makes no sense.
    Corrupt {
        /// What was being read, and what was wrong with it.
        detail: String,
    },

    /// The file could not be read at all: it does not exist, or the process
    /// cannot reach it.
    ///
    /// Kept apart from [`ReadError::Corrupt`] on purpose. A missing path is a
    /// bug in the calling program; a corrupt archive is a bad upload. Folding
    /// the two together would push a caller to answer both the same way.
    Io {
        /// The underlying operating system message.
        detail: String,
    },
}

impl ReadError {
    /// Stable, machine-readable discriminant. The binding maps this onto a JS
    /// error `code`, so it is part of the public contract: renaming one is a
    /// breaking change.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAnArchive => "NOT_AN_ARCHIVE",
            Self::SheetNotFound { .. } => "SHEET_NOT_FOUND",
            Self::DecompressedBudgetExceeded { .. } => "DECOMPRESSED_BUDGET_EXCEEDED",
            Self::TooManyEntries { .. } => "TOO_MANY_ENTRIES",
            Self::RowBudgetExceeded { .. } => "ROW_BUDGET_EXCEEDED",
            Self::Corrupt { .. } => "CORRUPT",
            Self::Io { .. } => "IO",
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnArchive => write!(f, "not a ZIP archive"),
            Self::SheetNotFound { name, available } => write!(
                f,
                "sheet {name:?} not found; workbook declares [{}]",
                available
                    .iter()
                    .map(|n| format!("{n:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::DecompressedBudgetExceeded { limit, entry } => {
                write!(f, "archive expands past the {limit} byte budget at {entry}")
            }
            Self::TooManyEntries { limit, count } => write!(
                f,
                "archive declares {count} entries and the budget allows {limit}; \
                 indexing them costs memory before a single one is read"
            ),
            Self::RowBudgetExceeded { limit } => {
                write!(f, "sheet holds more than {limit} rows")
            }
            Self::Corrupt { detail } => write!(f, "corrupt workbook: {detail}"),
            Self::Io { detail } => write!(f, "cannot read the file: {detail}"),
        }
    }
}

impl std::error::Error for ReadError {}

impl ReadError {
    pub(crate) fn corrupt(detail: impl Into<String>) -> Self {
        Self::Corrupt {
            detail: detail.into(),
        }
    }
}
