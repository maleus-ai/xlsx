//! Keeping a worksheet cursor alive across calls.
//!
//! `calamine` hands out its streaming cell reader as `XlsxCellReader<'a, RS>`,
//! borrowed from `&'a mut Xlsx<RS>`: the reader holds the archive's decoder for
//! the sheet entry, plus the workbook's shared-string and format tables. That
//! shape is exactly right for a `while let` loop over one stack frame, and
//! exactly wrong for a cursor that a consumer pulls from batch after batch —
//! the workbook and the reader borrowed from it must live in the same struct.
//!
//! So this module holds the pair, and the only `unsafe` in the crate is the
//! lifetime erasure that makes it expressible. Its invariants:
//!
//! * `workbook` is boxed, so its address survives every move of the cursor;
//! * `reader` is declared first and therefore dropped first, before the
//!   workbook it borrows from;
//! * nothing outside this module ever reaches the workbook. `XlsxReader` caches
//!   the sheet list at open time and reads it from that cache, so no shared
//!   reference is ever created alongside the reader's mutable borrow;
//! * `open_sheet` clears `reader` before touching the workbook, which ends the
//!   previous borrow before the next one starts.

use std::fs::File;
use std::io::BufReader;

use calamine::{Xlsx, XlsxCellReader, XlsxError};

pub(crate) type Backing = BufReader<File>;
pub(crate) type Workbook = Xlsx<Backing>;
pub(crate) type CellReader = XlsxCellReader<'static, Backing>;

pub(crate) struct SheetCursor {
    // Declaration order is load-bearing: `reader` borrows from `workbook`.
    reader: Option<CellReader>,
    workbook: Box<Workbook>,
}

impl SheetCursor {
    pub(crate) fn new(workbook: Workbook) -> Self {
        Self {
            reader: None,
            workbook: Box::new(workbook),
        }
    }

    /// Point the cursor at `name`, from the top of that sheet.
    pub(crate) fn open_sheet(&mut self, name: &str) -> Result<(), XlsxError> {
        // Ends the previous borrow of the workbook before the next one opens.
        self.reader = None;

        let workbook: *mut Workbook = &mut *self.workbook;

        // SAFETY: `workbook` points into a `Box` this struct owns, so the
        // address is live and stable for as long as `self` is. The borrow it
        // hands out is the only live borrow of the workbook — `self.reader` was
        // just cleared, and no code outside this module can reach the workbook.
        // The lifetime is erased to `'static` because the borrow's real scope is
        // the cursor's own lifetime, which Rust cannot name here; field order
        // guarantees the reader is dropped while the workbook is still alive.
        let reader = unsafe { (*workbook).worksheet_cells_reader(name) }?;
        let reader: CellReader =
            unsafe { std::mem::transmute::<XlsxCellReader<'_, Backing>, CellReader>(reader) };

        self.reader = Some(reader);
        Ok(())
    }

    /// The open sheet's cell reader, or `None` if no sheet has been selected.
    pub(crate) fn reader_mut(&mut self) -> Option<&mut CellReader> {
        self.reader.as_mut()
    }
}
