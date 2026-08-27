//! Listing the sheets without reading them.
//!
//! Opening a workbook charges the byte budget over the whole archive, because
//! that is the only hole-free way to count what leaves the inflater when the
//! inflater belongs to somebody else. For a read that is the right trade: the
//! archive is about to be read anyway. For a listing it is not — three sheet
//! names should not cost a walk of a hundred megabytes.
//!
//! So listing takes its own path, and reads exactly two parts: the package
//! relationships, to find where the workbook lives, and the workbook itself.
//! Both are inflated through a counter and charged against the same budget, so
//! the bound is not skipped — it is simply applied to the two entries that get
//! read instead of to all of them.
//!
//! The obligation this creates is that the two paths must agree about what a
//! sheet is. `lists_the_same_sheets_as_a_full_open` holds them to it, on every
//! fixture in the repository.

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
use crate::reader::SheetInfo;

const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

/// The sheets a workbook declares, without inflating anything else.
pub(crate) fn list_sheets(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<Vec<SheetInfo>, ReadError> {
    enforce_entry_budget(path, max_decompressed_bytes)?;

    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;

    let mut budget = Budget::new(max_decompressed_bytes);

    let relationships = inflate_entry_within(&mut archive, "_rels/.rels", &mut budget)?;
    let workbook_path = workbook_target(&relationships)?;

    let workbook = inflate_entry_within(&mut archive, &workbook_path, &mut budget)?;
    parse_sheets(&workbook)
}

/// Where `_rels/.rels` says the workbook part lives.
fn workbook_target(relationships: &[u8]) -> Result<String, ReadError> {
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
fn parse_sheets(workbook: &[u8]) -> Result<Vec<SheetInfo>, ReadError> {
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
                // Absent means visible; `hidden` and `veryHidden` do not.
                let state = attribute(&element, b"state", decoder);
                sheets.push(SheetInfo {
                    name,
                    visible: matches!(state.as_deref(), None | Some("visible")),
                });
            }
            Event::Eof => break,
            _ => {}
        }

        buf.clear();
    }

    Ok(sheets)
}

fn attribute(
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
