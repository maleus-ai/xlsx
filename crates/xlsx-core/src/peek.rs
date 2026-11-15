//! Peeking at the sheets without reading them.
//!
//! Opening a workbook charges the byte budget over the whole archive, because
//! that is the only hole-free way to count what leaves the inflater when the
//! inflater belongs to somebody else. For a read that is the right trade: the
//! archive is about to be read anyway. For a peek it is not — three sheet
//! names should not cost a walk of a hundred megabytes.
//!
//! So a peek takes its own path, and reads exactly two parts: the package
//! relationships, to find where the workbook lives, and the workbook itself.
//! Both are inflated through a counter and charged against the same budget, so
//! the bound is not skipped — it is simply applied to the two entries that get
//! read instead of to all of them.
//!
//! A full open reads its sheet list through the same parser, after charging
//! the whole archive, so the two paths cannot disagree about what a sheet is.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::encoding::Decoder;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use zip::ZipArchive;

use crate::budget::{inflate_entry_within, map_zip_error, Budget};
use crate::directory::enforce_entry_budget;
use crate::error::ReadError;
use crate::reader::{SheetInfo, SheetState};

const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

/// The sheets a workbook declares, without inflating anything else.
pub(crate) fn peek_sheets(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<Vec<SheetInfo>, ReadError> {
    enforce_entry_budget(path, max_decompressed_bytes)?;
    read_sheets(path, max_decompressed_bytes)
}

/// Read the sheet list, charging only the two parts it inflates.
///
/// The entry count is the caller's to enforce: a peek enforces it here, a full
/// open has already charged every entry before it gets this far.
pub(crate) fn read_sheets(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<Vec<SheetInfo>, ReadError> {
    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = entry_index(&archive)?;

    let mut budget = Budget::new(max_decompressed_bytes);

    let relationships = inflate_entry_within(
        &mut archive,
        entry_name(&entries, "_rels/.rels"),
        &mut budget,
    )?;
    let workbook_path = workbook_target(&relationships)?;

    let workbook = inflate_entry_within(
        &mut archive,
        entry_name(&entries, &workbook_path),
        &mut budget,
    )?;
    parse_sheets(&workbook)
}

/// The archive's entry names, keyed the way `calamine` looks a part up:
/// lowercase, with `\` read as `/`. Some generators write `xl\workbook.xml`, or
/// a case that differs from the target the relationships declare, and a part
/// that `calamine` opens must be one a peek finds too.
pub(crate) fn entry_index<R: std::io::Read + std::io::Seek>(
    archive: &ZipArchive<R>,
) -> Result<HashMap<String, String>, ReadError> {
    let mut index = HashMap::with_capacity(archive.len());
    for name in archive.file_names() {
        let name = name.map_err(map_zip_error)?;
        index.insert(
            name.replace('\\', "/").to_ascii_lowercase(),
            name.into_owned(),
        );
    }
    Ok(index)
}

/// The entry that holds `part`, or `part` itself when no entry matches, so
/// that the read reports the part as missing under the name it was asked by.
pub(crate) fn entry_name<'a>(index: &'a HashMap<String, String>, part: &'a str) -> &'a str {
    index
        .get(&part.to_ascii_lowercase())
        .map_or(part, String::as_str)
}

/// The archive entry of each declared sheet, in workbook order, as the archive
/// spells it — ready for an entry lookup by exact name.
///
/// Inflates three parts — the package relationships, the workbook and the
/// workbook's relationships — each charged against `max_decompressed_bytes`.
/// A sheet whose relationship is missing, or points outside the archive, makes
/// the workbook `CORRUPT`: a reader that skipped it would answer for a sheet
/// it never read.
pub(crate) fn read_sheet_parts(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<Vec<String>, ReadError> {
    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = entry_index(&archive)?;
    let mut budget = Budget::new(max_decompressed_bytes);

    let relationships = inflate_entry_within(
        &mut archive,
        entry_name(&entries, "_rels/.rels"),
        &mut budget,
    )?;
    let workbook_path = workbook_target(&relationships)?;
    let workbook = inflate_entry_within(
        &mut archive,
        entry_name(&entries, &workbook_path),
        &mut budget,
    )?;
    let ids = parse_sheet_relationship_ids(&workbook)?;

    let rels_path = relationships_of(&workbook_path);
    let rels = inflate_entry_within(&mut archive, entry_name(&entries, &rels_path), &mut budget)?;
    let relationships = crate::xml::parse_relationships(&rels, &rels_path)?;

    ids.into_iter()
        .map(|(name, id)| {
            let relationship = relationships
                .iter()
                .find(|candidate| !candidate.external && candidate.id == id)
                .ok_or_else(|| {
                    ReadError::corrupt(format!(
                        "sheet {name:?} refers to relationship {id:?}, which {rels_path} does not declare"
                    ))
                })?;
            let part = resolve_target(&workbook_path, &relationship.target)?;
            Ok(entry_name(&entries, &part).to_owned())
        })
        .collect()
}

/// The relationships part that sits beside `part`: `xl/workbook.xml` has its
/// relationships in `xl/_rels/workbook.xml.rels`.
pub(crate) fn relationships_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// Resolve a relationship target against the part that declares it, the way
/// OPC does: an absolute target is taken from the package root, a relative one
/// from the declaring part's directory.
///
/// A target that climbs out of the package (`../../etc/passwd`) names no part
/// and is refused.
pub(crate) fn resolve_target(source: &str, target: &str) -> Result<String, ReadError> {
    let mut segments: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())
            .unwrap_or_default()
    };
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(ReadError::corrupt(format!(
                        "relationship target {target:?} of {source:?} points outside the archive"
                    )));
                }
            }
            other => segments.push(other),
        }
    }
    Ok(segments.join("/"))
}

/// `(name, r:id)` of every `<sheet>` of a workbook part, in order.
pub(crate) fn parse_sheet_relationship_ids(
    workbook: &[u8],
) -> Result<Vec<(String, String)>, ReadError> {
    let mut xml = XmlReader::from_reader(workbook);
    let decoder = xml.decoder();
    let mut buf = Vec::new();
    let mut ids = Vec::new();

    loop {
        let event = xml.read_event_into(&mut buf).map_err(|error| {
            ReadError::corrupt(format!("the workbook part does not parse: {error}"))
        })?;

        match event {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"sheet" =>
            {
                let name = attribute(&element, b"name", decoder)
                    .ok_or_else(|| ReadError::corrupt("a sheet is declared without a name"))?;
                // `r:id`, whatever prefix the workbook binds to the
                // relationships namespace.
                let id = element
                    .attributes()
                    .flatten()
                    .find(|attr| {
                        attr.key.local_name().as_ref() == b"id" && attr.key.prefix().is_some()
                    })
                    .and_then(|attr| {
                        attr.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                            .ok()
                            .map(|value| value.into_owned())
                    })
                    .ok_or_else(|| {
                        ReadError::corrupt(format!(
                            "sheet {name:?} declares no relationship to its part"
                        ))
                    })?;
                ids.push((name, id));
            }
            Event::Eof => break,
            _ => {}
        }

        buf.clear();
    }

    Ok(ids)
}

/// Where `_rels/.rels` says the workbook part lives.
pub(crate) fn workbook_target(relationships: &[u8]) -> Result<String, ReadError> {
    let mut xml = XmlReader::from_reader(relationships);
    let decoder = xml.decoder();
    let mut buf = Vec::new();

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("_rels/.rels does not parse: {error}")))?;

        match event {
            Event::Start(element) | Event::Empty(element) => {
                if element.local_name().as_ref() != b"Relationship" {
                    buf.clear();
                    continue;
                }

                let kind = attribute(&element, b"Type", decoder);
                let target = attribute(&element, b"Target", decoder);

                if kind.as_deref() == Some(OFFICE_DOCUMENT) {
                    if let Some(target) = target {
                        // Targets are written both as `xl/workbook.xml` and as
                        // `/xl/workbook.xml`; archive entries never carry the
                        // leading slash.
                        return Ok(target.trim_start_matches('/').to_owned());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }

        buf.clear();
    }

    Err(ReadError::corrupt(
        "package relationships name no workbook part",
    ))
}

/// The `<sheet>` elements of a workbook part, in order.
pub(crate) fn parse_sheets(workbook: &[u8]) -> Result<Vec<SheetInfo>, ReadError> {
    let mut xml = XmlReader::from_reader(workbook);
    let decoder = xml.decoder();
    let mut buf = Vec::new();
    let mut sheets = Vec::new();

    loop {
        let event = xml.read_event_into(&mut buf).map_err(|error| {
            ReadError::corrupt(format!("the workbook part does not parse: {error}"))
        })?;

        match event {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"sheet" =>
            {
                let Some(name) = attribute(&element, b"name", decoder) else {
                    return Err(ReadError::corrupt("a sheet is declared without a name"));
                };
                let sheet_id = attribute(&element, b"sheetId", decoder)
                    .and_then(|id| id.parse::<u32>().ok())
                    .ok_or_else(|| {
                        ReadError::corrupt(format!(
                            "sheet {name:?} declares no numeric sheetId, which the format requires"
                        ))
                    })?;
                let state = match attribute(&element, b"state", decoder).as_deref() {
                    // Absent means visible: ST_SheetState defaults to it.
                    None | Some("visible") => SheetState::Visible,
                    Some("hidden") => SheetState::Hidden,
                    Some("veryHidden") => SheetState::VeryHidden,
                    Some(other) => {
                        return Err(ReadError::corrupt(format!(
                            "sheet {name:?} declares state {other:?}, which is not one of visible, hidden, veryHidden"
                        )))
                    }
                };
                sheets.push(SheetInfo {
                    name,
                    sheet_id,
                    state,
                });
            }
            Event::Eof => break,
            _ => {}
        }

        buf.clear();
    }

    Ok(sheets)
}

pub(crate) fn attribute(
    element: &quick_xml::events::BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
) -> Option<String> {
    let attribute = element.try_get_attribute(name).ok().flatten()?;
    let value = attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
        .ok()?;
    Some(value.into_owned())
}
