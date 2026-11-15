//! Small pieces every structural reader needs: a reader configured the same
//! way everywhere, text with its entities resolved, relationships with their
//! types, attribute values parsed as the format spells them.

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::Reader as XmlReader;

use crate::error::ReadError;
use crate::peek::attribute;

/// A reader over an inflated part. Empty elements read as a start and an end,
/// so `<b/>` and `<b></b>` take the same path through every parser.
pub(crate) fn reader(bytes: &[u8]) -> XmlReader<&[u8]> {
    let mut xml = XmlReader::from_reader(bytes);
    xml.config_mut().expand_empty_elements = true;
    xml
}

/// Append a text event to `out`. Only the five predefined entities and
/// character references are resolved: a part that declares its own entities is
/// refused, as everywhere else in the crate.
pub(crate) fn append_text(
    out: &mut String,
    event: &Event<'_>,
    part: &str,
) -> Result<(), ReadError> {
    match event {
        Event::Text(text) => out.push_str(
            &text
                .xml10_content()
                .map_err(|error| ReadError::corrupt(format!("{part} does not decode: {error}")))?,
        ),
        Event::CData(data) => out.push_str(
            &data
                .decode()
                .map_err(|error| ReadError::corrupt(format!("{part} does not decode: {error}")))?,
        ),
        Event::GeneralRef(reference) => out.push(resolve_reference(reference, part)?),
        _ => {}
    }
    Ok(())
}

fn resolve_reference(reference: &BytesRef<'_>, part: &str) -> Result<char, ReadError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| ReadError::corrupt(format!("{part}: {error}")))?
    {
        return Ok(character);
    }
    let name = reference
        .decode()
        .map_err(|error| ReadError::corrupt(format!("{part} does not decode: {error}")))?;
    match name.as_ref() {
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "amp" => Ok('&'),
        "apos" => Ok('\''),
        "quot" => Ok('"'),
        other => Err(ReadError::corrupt(format!(
            "{part} refers to entity {other:?}; only the predefined ones are read"
        ))),
    }
}

/// `xsd:boolean`, as the format writes it.
pub(crate) fn parse_bool(value: Option<&str>) -> Option<bool> {
    match value {
        Some("1") | Some("true") => Some(true),
        Some("0") | Some("false") => Some(false),
        _ => None,
    }
}

/// An attribute parsed as a number, `None` when absent or not one.
pub(crate) fn number_attribute<T: std::str::FromStr>(
    element: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
) -> Option<T> {
    attribute(element, name, decoder).and_then(|value| value.trim().parse().ok())
}

/// An attribute kept only when its value is one the format enumerates.
pub(crate) fn enum_attribute(
    element: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
    allowed: &[&str],
) -> Option<String> {
    attribute(element, name, decoder).filter(|value| allowed.contains(&value.as_str()))
}

/// One relationship of a relationships part.
#[derive(Debug, Clone)]
pub(crate) struct Relationship {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) external: bool,
}

/// Every relationship a relationships part declares.
pub(crate) fn parse_relationships(
    bytes: &[u8],
    part: &str,
) -> Result<Vec<Relationship>, ReadError> {
    let mut xml = reader(bytes);
    let decoder = xml.decoder();
    let mut buf = Vec::new();
    let mut relationships = Vec::new();

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match event {
            Event::Start(element) if element.local_name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(kind), Some(target)) = (
                    attribute(&element, b"Id", decoder),
                    attribute(&element, b"Type", decoder),
                    attribute(&element, b"Target", decoder),
                ) {
                    let external =
                        attribute(&element, b"TargetMode", decoder).as_deref() == Some("External");
                    relationships.push(Relationship {
                        id,
                        kind,
                        target,
                        external,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(relationships)
}

/// Whether a relationship type ends with `/<name>`. Strict and transitional
/// OOXML use different namespace URIs for the same relationship types, so a
/// type is recognised by its last segment.
pub(crate) fn is_kind(relationship: &Relationship, name: &str) -> bool {
    relationship
        .kind
        .rsplit('/')
        .next()
        .is_some_and(|last| last == name)
}
