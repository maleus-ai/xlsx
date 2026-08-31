//! Writing errors.
//!
//! Same rule as [`ReadError`](crate::ReadError): the set is closed, and every
//! variant carries enough for a caller to build its own message. A writer that
//! says only "write failed" on row 300 000 of an export leaves nobody anywhere
//! to look.

use std::fmt;

/// Everything that can go wrong while writing a workbook.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// A cell held a timestamp that is not a date this crate can place.
    ///
    /// Carries the offending value, because on a large export the row number
    /// alone is not enough to find it.
    InvalidDateTime {
        /// The string as the caller supplied it.
        value: String,
        /// What could not be made of it.
        detail: String,
    },

    /// The sheet ran past the grid Excel defines: 1 048 576 rows by 16 384
    /// columns.
    ///
    /// This is a hard edge of the format, not a budget this crate chose. A
    /// caller that hits it has to split the export across sheets; no option
    /// makes the cell exist.
    SheetLimitExceeded {
        /// Rows the grid allows.
        max_rows: u32,
        /// Columns the grid allows.
        max_columns: u32,
        /// The row that did not fit, zero-based.
        row: u64,
        /// The column that did not fit, zero-based.
        column: u64,
    },

    /// The sheet name is not one Excel accepts.
    ///
    /// Names are at most 31 characters, cannot be empty, cannot hold
    /// `[ ] : * ? / \`, and cannot repeat within a workbook.
    InvalidSheetName {
        /// The name that was asked for.
        name: String,
        /// Why Excel would not take it.
        detail: String,
    },

    /// The output could not be written, or a temporary file could not be made.
    ///
    /// In constant-memory mode each row is flushed to a temporary file, so this
    /// covers a temporary directory that is missing, unwritable or full just as
    /// much as it covers the destination.
    Io {
        /// The underlying operating system message.
        detail: String,
    },

    /// The workbook could not be assembled, for a reason this crate does not
    /// model.
    ///
    /// Kept as an escape hatch so that a new failure in the underlying writer
    /// surfaces with its own words rather than being forced into a variant that
    /// misdescribes it.
    Failed {
        /// What the underlying writer reported.
        detail: String,
    },
}

impl WriteError {
    /// Stable, machine-readable discriminant. The binding maps this onto a JS
    /// error `code`, so it is part of the public contract: renaming one is a
    /// breaking change.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidDateTime { .. } => "INVALID_DATETIME",
            Self::SheetLimitExceeded { .. } => "SHEET_LIMIT_EXCEEDED",
            Self::InvalidSheetName { .. } => "INVALID_SHEET_NAME",
            Self::Io { .. } => "IO",
            Self::Failed { .. } => "WRITE_FAILED",
        }
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDateTime { value, detail } => {
                write!(f, "{value:?} is not a timestamp: {detail}")
            }
            Self::SheetLimitExceeded {
                max_rows,
                max_columns,
                row,
                column,
            } => write!(
                f,
                "cell at row {row} column {column} falls outside the Excel grid \
                 of {max_rows} rows by {max_columns} columns"
            ),
            Self::InvalidSheetName { name, detail } => {
                write!(f, "sheet name {name:?} is not usable: {detail}")
            }
            Self::Io { detail } => write!(f, "cannot write: {detail}"),
            Self::Failed { detail } => write!(f, "cannot build the workbook: {detail}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<std::io::Error> for WriteError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            detail: error.to_string(),
        }
    }
}

impl From<rust_xlsxwriter::XlsxError> for WriteError {
    /// Map the underlying writer's errors onto ours.
    ///
    /// Only the ones a caller can act on differently are picked out. The rest
    /// land in [`WriteError::Failed`] carrying their own text, which is better
    /// than filing them under a variant that would misdescribe them.
    fn from(error: rust_xlsxwriter::XlsxError) -> Self {
        use rust_xlsxwriter::XlsxError;

        match error {
            XlsxError::RowColumnLimitError => Self::SheetLimitExceeded {
                max_rows: super::MAX_ROWS,
                max_columns: super::MAX_COLUMNS,
                row: 0,
                column: 0,
            },
            XlsxError::SheetnameCannotBeBlank(_)
            | XlsxError::SheetnameLengthExceeded(_)
            | XlsxError::SheetnameContainsInvalidCharacter(_)
            | XlsxError::SheetnameReused(_)
            | XlsxError::SheetnameStartsOrEndsWithApostrophe(_) => Self::InvalidSheetName {
                name: String::new(),
                detail: error.to_string(),
            },
            XlsxError::IoError(inner) => Self::Io {
                detail: inner.to_string(),
            },
            other => Self::Failed {
                detail: other.to_string(),
            },
        }
    }
}
