//! The archive's own directory, bounded before it is read.
//!
//! [`crate::budget`] counts what leaves the inflater, which covers every vector
//! that turns a small archive into a large expansion. It does not cover the one
//! that turns a small archive into a large *directory*: a ZIP holding hundreds
//! of thousands of tiny entries costs nothing to inflate and a great deal to
//! index, and that indexing happens inside `ZipArchive::new`, before any byte
//! budget has anything to say.
//!
//! Measured: a 15 MB archive of 65 000 one-byte entries peaks at 57 MB before a
//! single row is read — about 880 bytes an entry, for the entry record, its
//! name, and the lookup table. Below 65 536 entries a plain ZIP cannot go any
//! further, because the count is a `u16`; a ZIP64 archive has no such ceiling,
//! and ten million entries would ask for gigabytes.
//!
//! So the count is read from the end-of-central-directory record — 22 bytes,
//! plus the ZIP64 record when the count is at its sentinel — and charged
//! against the budget the caller already set, before the archive is opened.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::ReadError;

/// Process memory an archive entry costs to index, rounded up from the 880
/// bytes measured. Expressed in the same unit as the caller's byte budget so
/// that one number governs both, rather than a constant nobody chose.
const DIRECTORY_COST_PER_ENTRY: u64 = 1024;

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0201_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;

const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_LEN: usize = 20;
const MAX_COMMENT_LEN: usize = u16::MAX as usize;

/// How many entries the archive declares, or `None` when its trailer cannot be
/// made sense of at all.
///
/// A `None` is fail-closed: the caller refuses the file. It has to be. The
/// tolerance for what counts as a trailer is not the same on both sides — `zip`
/// accepts a comment that stops short of the end of the file, on purpose, for
/// "garbage-after-comment Python files" — and if the reader that decides the
/// bound is the stricter of the two, five arbitrary bytes appended to an archive
/// make the bound disappear while the archive still opens.
fn declared_entry_count<R: Read + Seek>(reader: &mut R, length: u64) -> Option<u64> {
    let trailer_len = (EOCD_LEN + MAX_COMMENT_LEN + ZIP64_LOCATOR_LEN).min(length as usize);
    let trailer_start = length - trailer_len as u64;

    let mut trailer = vec![0u8; trailer_len];
    reader.seek(SeekFrom::Start(trailer_start)).ok()?;
    reader.read_exact(&mut trailer).ok()?;

    // The comment only has to *fit*, not to end exactly on the last byte —
    // matching what `zip` will accept when it reads the same bytes.
    let candidates = (0..=trailer.len().checked_sub(EOCD_LEN)?).filter(|&at| {
        read_u32(&trailer, at) == Some(EOCD_SIGNATURE)
            && read_u16(&trailer, at + 20)
                .is_some_and(|comment_len| at + EOCD_LEN + comment_len as usize <= trailer.len())
    });

    // A comment is free to contain the signature, an archive is free to carry a
    // comment shaped like a second trailer, and once the comment length is only
    // an upper bound, four bytes of compressed data can look like one too.
    // Rather than guess which record the ZIP reader will settle on, every
    // candidate that survives corroboration is read and the largest count wins:
    // that cannot be talked into reading a small count off a decoy planted after
    // a large one, and it can only ever refuse an archive that is deliberately
    // ambiguous about how many entries it holds.
    let mut highest: Option<u64> = None;
    for at in candidates {
        let Some(count) = read_u16(&trailer, at + 10) else {
            continue;
        };
        let Some(comment_len) = read_u16(&trailer, at + 20) else {
            continue;
        };

        let ends_the_file = at + EOCD_LEN + comment_len as usize == trailer.len();

        let count = if count < u16::MAX {
            // Only a record whose directory pointer lands on a real directory
            // header is believed — unless it sits exactly at the end of the
            // file, which is where a well-formed trailer lives.
            if !ends_the_file && !points_at_a_directory(reader, &trailer, at, length, count) {
                continue;
            }
            u64::from(count)
        } else {
            // At the sentinel the real count lives in the ZIP64 record, which
            // the locator immediately before the trailer points at. Finding and
            // parsing that record is itself the corroboration.
            match zip64_entry_count(reader, &trailer, at) {
                Some(count) => count,
                None if ends_the_file => u64::from(count),
                None => continue,
            }
        };

        highest = Some(highest.map_or(count, |previous: u64| previous.max(count)));
    }

    highest
}

/// Does this record's central directory pointer land on a directory header?
///
/// Four bytes matching the trailer signature by accident is a one-in-four-
/// billion event per position, which over a 64 kB window and a lot of uploads is
/// not never. A record that also points at a real central directory header is.
fn points_at_a_directory<R: Read + Seek>(
    reader: &mut R,
    trailer: &[u8],
    at: usize,
    length: u64,
    count: u16,
) -> bool {
    if count == 0 {
        // Nothing to point at, and an empty archive is not a directory bomb.
        return true;
    }

    let Some(offset) = read_u32(trailer, at + 16).map(u64::from) else {
        return false;
    };
    if offset.saturating_add(4) > length {
        return false;
    }

    let mut magic = [0u8; 4];
    reader.seek(SeekFrom::Start(offset)).is_ok()
        && reader.read_exact(&mut magic).is_ok()
        && u32::from_le_bytes(magic) == CENTRAL_DIRECTORY_SIGNATURE
}

fn zip64_entry_count<R: Read + Seek>(
    reader: &mut R,
    trailer: &[u8],
    eocd_at: usize,
) -> Option<u64> {
    let locator_at = eocd_at.checked_sub(ZIP64_LOCATOR_LEN)?;
    if read_u32(trailer, locator_at) != Some(ZIP64_LOCATOR_SIGNATURE) {
        return None;
    }

    let zip64_eocd_at = read_u64(trailer, locator_at + 8)?;

    let mut record = [0u8; 40];
    reader.seek(SeekFrom::Start(zip64_eocd_at)).ok()?;
    reader.read_exact(&mut record).ok()?;

    if read_u32(&record, 0) != Some(ZIP64_EOCD_SIGNATURE) {
        return None;
    }

    read_u64(&record, 32)
}

/// Refuse an archive whose directory alone would cost more than the budget.
pub(crate) fn enforce_entry_budget(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<(), ReadError> {
    let mut file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;

    let Some(length) = file.metadata().ok().map(|meta| meta.len()) else {
        return Ok(());
    };

    // Fail closed. A file whose trailer cannot be read is not an archive this
    // reader is willing to bound, and therefore not one it is willing to open.
    let Some(count) = declared_entry_count(&mut file, length) else {
        return Err(ReadError::NotAnArchive);
    };

    let limit = max_decompressed_bytes / DIRECTORY_COST_PER_ENTRY;
    if count > limit {
        return Err(ReadError::TooManyEntries { limit, count });
    }

    Ok(())
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn eocd(entries: u16, comment: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number
        bytes.extend_from_slice(&0u16.to_le_bytes()); // central directory disk
        bytes.extend_from_slice(&entries.to_le_bytes()); // entries on this disk
        bytes.extend_from_slice(&entries.to_le_bytes()); // entries in total
        bytes.extend_from_slice(&0u32.to_le_bytes()); // directory size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // directory offset
        bytes.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);
        bytes
    }

    fn count_of(bytes: Vec<u8>) -> Option<u64> {
        let length = bytes.len() as u64;
        declared_entry_count(&mut Cursor::new(bytes), length)
    }

    #[test]
    fn reads_the_count_from_a_plain_trailer() {
        assert_eq!(count_of(eocd(7, b"")), Some(7));
    }

    #[test]
    fn a_trailer_planted_in_a_comment_cannot_shrink_the_count() {
        // Both records are structurally valid, and which one a ZIP reader
        // settles on is not something to bet the bound on. The larger count
        // wins, so hiding a small one after a large one buys nothing.
        let decoy = eocd(7, b"");
        let bytes = eocd(9_999, &decoy);
        assert_eq!(count_of(bytes), Some(9_999));
    }

    #[test]
    fn the_sentinel_sends_the_count_to_the_zip64_record() {
        let mut bytes = vec![0u8; 8];
        let zip64_eocd_at = bytes.len() as u64;

        // ZIP64 end of central directory record.
        bytes.extend_from_slice(&ZIP64_EOCD_SIGNATURE.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes()); // size of the record
        bytes.extend_from_slice(&45u16.to_le_bytes()); // version made by
        bytes.extend_from_slice(&45u16.to_le_bytes()); // version needed
        bytes.extend_from_slice(&0u32.to_le_bytes()); // disk number
        bytes.extend_from_slice(&0u32.to_le_bytes()); // central directory disk
        bytes.extend_from_slice(&10_000_000u64.to_le_bytes()); // entries on disk
        bytes.extend_from_slice(&10_000_000u64.to_le_bytes()); // entries in total

        // ZIP64 locator, then the classic trailer carrying the sentinel.
        bytes.extend_from_slice(&ZIP64_LOCATOR_SIGNATURE.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // disk of the record
        bytes.extend_from_slice(&zip64_eocd_at.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // total disks
        bytes.extend_from_slice(&eocd(u16::MAX, b""));

        assert_eq!(count_of(bytes), Some(10_000_000));
    }

    #[test]
    fn a_file_with_no_trailer_is_left_to_the_zip_reader() {
        // Not this module's job to diagnose: `ZipArchive::new` reads the same
        // bytes next and says something far more useful.
        assert_eq!(count_of(b"pas une archive".to_vec()), None);
    }
}
