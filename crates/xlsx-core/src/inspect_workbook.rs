//! What a workbook declares about itself, apart from its sheets' contents:
//! document properties, defined names, date system, which kinds of parts the
//! package carries, and the cell styles with the number formats, fonts and
//! fills they refer to.
//!
//! Each of those tables is restored once and a style holds references into
//! them — 0-based indexes, as `styles.xml` does for fonts and fills. A style that copied its font would make the
//! output grow as the product of two tables — a long font name times many
//! styles — where the archive only pays for their sum.
//!
//! Every part is inflated through the same counted path as a peek, so the
//! inspection is bounded by `max_decompressed_bytes` and never walks a sheet.
//! Types follow ECMA-376: attribute names as the format spells them, the one-
//! or two-letter abbreviations of fonts spelled out (`b` is `bold`), and an
//! enumerated value outside its `ST_*` dropped rather than passed on.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesStart, Event};
use zip::ZipArchive;

use crate::budget::{inflate_entry_within, map_zip_error, Budget};
use crate::directory::enforce_entry_budget;
use crate::error::ReadError;
use crate::peek::{
    attribute, entry_index, entry_name, relationships_of, resolve_target, workbook_target,
};
use crate::reader::SheetInfo;
use crate::xml::{
    append_text, enum_attribute, is_kind, number_attribute, parse_bool, parse_relationships,
    reader, Relationship,
};

/// A colour — `CT_Color`. The attributes are kept as declared: a theme colour
/// is not resolved to RGB, which would need the theme and is a rendering
/// decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Color {
    /// `AARRGGBB`, as declared.
    pub rgb: Option<String>,
    /// Index into the theme's colour scheme.
    pub theme: Option<u32>,
    /// Index into the legacy indexed palette.
    pub indexed: Option<u32>,
    /// The application's automatic colour.
    pub auto: bool,
    /// Lightening (positive) or darkening (negative) applied, from -1 to 1.
    pub tint: Option<f64>,
}

/// A number format — `CT_NumFmt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumFmt {
    /// The identifier cells refer to it by.
    pub num_fmt_id: u32,
    /// The format code. A built-in identifier is resolved to the code the
    /// format defines for it; one whose code depends on the locale has none.
    pub format_code: Option<String>,
}

/// A font — `CT_Font`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Font {
    /// Typeface name.
    pub name: Option<String>,
    /// Size, in points.
    pub size: Option<f64>,
    /// `b`.
    pub bold: bool,
    /// `i`.
    pub italic: bool,
    /// `strike`.
    pub strike: bool,
    /// `u` — `ST_UnderlineValues`.
    pub underline: Option<String>,
    /// `vertAlign` — `ST_VerticalAlignRun`.
    pub vert_align: Option<String>,
    /// Text colour.
    pub color: Option<Color>,
}

/// A pattern fill — `CT_PatternFill`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatternFill {
    /// `ST_PatternType`.
    pub pattern_type: Option<String>,
    /// Foreground colour: the colour of a solid fill.
    pub fg_color: Option<Color>,
    /// Background colour.
    pub bg_color: Option<Color>,
}

/// One stop of a gradient — `CT_GradientStop`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient, from 0 to 1.
    pub position: f64,
    /// Colour at that position.
    pub color: Color,
}

/// A gradient fill — `CT_GradientFill`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GradientFill {
    /// `linear` or `path` — `ST_GradientType`.
    pub gradient_type: String,
    /// Angle of a linear gradient, in degrees.
    pub degree: Option<f64>,
    /// The stops, in order.
    pub stops: Vec<GradientStop>,
}

/// A fill — `CT_Fill`, which holds one or the other.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fill {
    /// The fill, when it is a pattern.
    pub pattern_fill: Option<PatternFill>,
    /// The fill, when it is a gradient.
    pub gradient_fill: Option<GradientFill>,
}

/// Alignment — `CT_CellAlignment`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alignment {
    /// `ST_HorizontalAlignment`.
    pub horizontal: Option<String>,
    /// `ST_VerticalAlignment`.
    pub vertical: Option<String>,
    /// Whether text wraps within the cell.
    pub wrap_text: bool,
    /// Whether text shrinks to fit the cell.
    pub shrink_to_fit: bool,
    /// Indentation, in steps.
    pub indent: Option<u32>,
    /// Rotation, in degrees, or `255` for vertical text.
    pub text_rotation: Option<u32>,
}

/// The style of a cell — an entry of `cellXfs`. It refers to its number
/// format, font and fill by reference: each is restored once in its own table
/// of the [`WorkbookInspection`], whatever the number of styles sharing it.
///
/// A reference is a 0-based index into that table, not an identifier the
/// format declares: the `numFmtId` a `xf` declares is kept on the entry of
/// [`WorkbookInspection::num_fmts`] that `num_fmt_ref` points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellStyle {
    /// Index in [`WorkbookInspection::num_fmts`].
    pub num_fmt_ref: u32,
    /// Index in [`WorkbookInspection::fonts`]; absent when the style refers
    /// to a font the table does not declare, which leaves the default font.
    pub font_ref: Option<u32>,
    /// Index in [`WorkbookInspection::fills`]; absent when the style refers
    /// to a fill the table does not declare, which leaves no fill.
    pub fill_ref: Option<u32>,
    /// Alignment.
    pub alignment: Alignment,
}

/// A defined name — `CT_DefinedName`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinedName {
    /// The name.
    pub name: String,
    /// The formula it stands for, as written. A broken reference stays as the
    /// file spells it, `#REF!` included.
    pub formula: String,
    /// 0-based position of the sheet the name is scoped to; absent for a name
    /// scoped to the workbook.
    pub local_sheet_id: Option<u32>,
    /// Whether the name is hidden from the interface.
    pub hidden: bool,
    /// The comment attached to it.
    pub comment: Option<String>,
}

/// Core and extended document properties.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentProperties {
    /// `dc:creator`.
    pub creator: Option<String>,
    /// `cp:lastModifiedBy`.
    pub last_modified_by: Option<String>,
    /// `dcterms:created`, as written (W3C date-time).
    pub created: Option<String>,
    /// `dcterms:modified`, as written.
    pub modified: Option<String>,
    /// `dc:title`.
    pub title: Option<String>,
    /// `dc:subject`.
    pub subject: Option<String>,
    /// `dc:description`.
    pub description: Option<String>,
    /// `cp:keywords`.
    pub keywords: Option<String>,
    /// `cp:category`.
    pub category: Option<String>,
    /// `Application`: the program that wrote the file, as it names itself.
    pub application: Option<String>,
    /// `AppVersion`.
    pub app_version: Option<String>,
    /// `Company`.
    pub company: Option<String>,
}

/// Which kinds of parts the package carries, besides sheets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkbookParts {
    /// A VBA project: the workbook carries macros.
    pub vba_project: bool,
    /// Pivot tables or pivot caches.
    pub pivot_tables: bool,
    /// Links to other workbooks.
    pub external_links: bool,
    /// Drawings: images, charts, shapes.
    pub drawings: bool,
}

/// What a workbook declares about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkbookInspection {
    /// The sheets, as a peek returns them.
    pub sheets: Vec<SheetInfo>,
    /// The defined names, in order.
    pub defined_names: Vec<DefinedName>,
    /// Document properties.
    pub properties: DocumentProperties,
    /// Whether dates count from 1904 rather than 1900.
    pub date1904: bool,
    /// Which kinds of parts the package carries.
    pub parts: WorkbookParts,
    /// The number formats the workbook declares and the built-in ones its
    /// styles refer to, one per `numFmtId`, by ascending `numFmtId`; a style
    /// points into it with [`CellStyle::num_fmt_ref`].
    pub num_fmts: Vec<NumFmt>,
    /// The font table, in order; a style points into it with
    /// [`CellStyle::font_ref`].
    pub fonts: Vec<Font>,
    /// The fill table, in order; a style points into it with
    /// [`CellStyle::fill_ref`].
    pub fills: Vec<Fill>,
    /// Cell styles, indexed as cells refer to them.
    pub styles: Vec<CellStyle>,
}

const UNDERLINE: &[&str] = &[
    "single",
    "double",
    "singleAccounting",
    "doubleAccounting",
    "none",
];
const VERT_ALIGN: &[&str] = &["baseline", "superscript", "subscript"];
const PATTERN: &[&str] = &[
    "none",
    "solid",
    "mediumGray",
    "darkGray",
    "lightGray",
    "darkHorizontal",
    "darkVertical",
    "darkDown",
    "darkUp",
    "darkGrid",
    "darkTrellis",
    "lightHorizontal",
    "lightVertical",
    "lightDown",
    "lightUp",
    "lightGrid",
    "lightTrellis",
    "gray125",
    "gray0625",
];
const HORIZONTAL: &[&str] = &[
    "general",
    "left",
    "center",
    "right",
    "fill",
    "justify",
    "centerContinuous",
    "distributed",
];
const VERTICAL: &[&str] = &["top", "center", "bottom", "justify", "distributed"];

/// The code ECMA-376 assigns a built-in number format, `None` for an
/// identifier whose code depends on the locale or that the format leaves free.
fn builtin_format_code(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// Inspect a workbook: everything but the contents of its sheets.
///
/// Inflates the package and workbook relationships, the workbook, the two
/// properties parts and the styles, each charged against
/// `max_decompressed_bytes`; no sheet is read.
pub fn inspect_workbook(
    path: &Path,
    max_decompressed_bytes: u64,
) -> Result<WorkbookInspection, ReadError> {
    enforce_entry_budget(path, max_decompressed_bytes)?;

    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = entry_index(&archive)?;
    let has = |part: &str| entries.contains_key(&part.to_ascii_lowercase());
    let mut budget = Budget::new(max_decompressed_bytes);

    let package_rels_bytes = inflate_entry_within(
        &mut archive,
        entry_name(&entries, "_rels/.rels"),
        &mut budget,
    )?;
    let package_rels = parse_relationships(&package_rels_bytes, "_rels/.rels")?;
    let workbook_path = workbook_target(&package_rels_bytes)?;

    let workbook = inflate_entry_within(
        &mut archive,
        entry_name(&entries, &workbook_path),
        &mut budget,
    )?;
    let sheets = crate::peek::parse_sheets(&workbook)?;
    let (defined_names, date1904) = parse_workbook(&workbook, &workbook_path)?;

    let workbook_rels_path = relationships_of(&workbook_path);
    let workbook_rels = match has(&workbook_rels_path) {
        true => {
            let bytes = inflate_entry_within(
                &mut archive,
                entry_name(&entries, &workbook_rels_path),
                &mut budget,
            )?;
            parse_relationships(&bytes, &workbook_rels_path)?
        }
        false => Vec::new(),
    };

    let mut properties = DocumentProperties::default();
    if let Some(core) = internal_part(&package_rels, "core-properties", "")? {
        if has(&core) {
            let bytes =
                inflate_entry_within(&mut archive, entry_name(&entries, &core), &mut budget)?;
            parse_core_properties(&bytes, &core, &mut properties)?;
        }
    }
    if let Some(app) = internal_part(&package_rels, "extended-properties", "")? {
        if has(&app) {
            let bytes =
                inflate_entry_within(&mut archive, entry_name(&entries, &app), &mut budget)?;
            parse_app_properties(&bytes, &app, &mut properties)?;
        }
    }

    let Styles {
        num_fmts,
        fonts,
        fills,
        styles,
    } = match internal_part(&workbook_rels, "styles", &workbook_path)? {
        Some(styles) if has(&styles) => {
            let bytes =
                inflate_entry_within(&mut archive, entry_name(&entries, &styles), &mut budget)?;
            parse_styles(&bytes, &styles)?
        }
        _ => Styles::default_only(),
    };

    // Matched on the index's keys: lowercase, `/` as the separator.
    let keys = || entries.keys();
    let parts = WorkbookParts {
        vba_project: workbook_rels.iter().any(|r| is_kind(r, "vbaProject"))
            || keys().any(|n| n.ends_with("vbaproject.bin")),
        pivot_tables: workbook_rels
            .iter()
            .any(|r| is_kind(r, "pivotCacheDefinition"))
            || keys().any(|n| n.contains("/pivottables/") || n.contains("/pivotcache/")),
        external_links: workbook_rels.iter().any(|r| is_kind(r, "externalLink")),
        drawings: keys().any(|n| n.contains("/drawings/") && n.ends_with(".xml")),
    };

    Ok(WorkbookInspection {
        sheets,
        defined_names,
        properties,
        date1904,
        parts,
        num_fmts,
        fonts,
        fills,
        styles,
    })
}

/// The part an internal relationship of `kind` points to, resolved against
/// the part that declares it (`""` for the package itself).
fn internal_part(
    relationships: &[Relationship],
    kind: &str,
    source: &str,
) -> Result<Option<String>, ReadError> {
    relationships
        .iter()
        .find(|r| !r.external && is_kind(r, kind))
        .map(|r| {
            if source.is_empty() {
                resolve_target("", &format!("/{}", r.target.trim_start_matches('/')))
            } else {
                resolve_target(source, &r.target)
            }
        })
        .transpose()
}

/// Defined names and the date system, from the workbook part.
fn parse_workbook(bytes: &[u8], part: &str) -> Result<(Vec<DefinedName>, bool), ReadError> {
    let mut xml = reader(bytes);
    let decoder = xml.decoder();
    let mut buf = Vec::new();
    let mut names = Vec::new();
    let mut date1904 = false;
    let mut current: Option<DefinedName> = None;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match &event {
            Event::Start(element) if element.local_name().as_ref() == b"workbookPr" => {
                date1904 = parse_bool(attribute(element, b"date1904", decoder).as_deref())
                    .unwrap_or(false);
            }
            Event::Start(element) if element.local_name().as_ref() == b"definedName" => {
                let Some(name) = attribute(element, b"name", decoder) else {
                    return Err(ReadError::corrupt("a defined name has no name"));
                };
                current = Some(DefinedName {
                    name,
                    formula: String::new(),
                    local_sheet_id: number_attribute(element, b"localSheetId", decoder),
                    hidden: parse_bool(attribute(element, b"hidden", decoder).as_deref())
                        .unwrap_or(false),
                    comment: attribute(element, b"comment", decoder),
                });
            }
            Event::End(element) if element.local_name().as_ref() == b"definedName" => {
                names.extend(current.take());
            }
            Event::Eof => break,
            other => {
                if let Some(name) = current.as_mut() {
                    append_text(&mut name.formula, other, part)?;
                }
            }
        }
        buf.clear();
    }

    Ok((names, date1904))
}

/// The text of the elements a properties part declares, by local name.
fn element_texts(bytes: &[u8], part: &str) -> Result<HashMap<String, String>, ReadError> {
    let mut xml = reader(bytes);
    let mut buf = Vec::new();
    let mut texts = HashMap::new();
    let mut depth = 0usize;
    let mut current: Option<(String, String)> = None;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match &event {
            Event::Start(element) => {
                depth += 1;
                // The properties are the root's direct children.
                if depth == 2 {
                    let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                    current = Some((name, String::new()));
                }
            }
            Event::End(_) => {
                if depth == 2 {
                    if let Some((name, text)) = current.take() {
                        texts.entry(name).or_insert(text);
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            other => {
                if let Some((_, text)) = current.as_mut() {
                    append_text(text, other, part)?;
                }
            }
        }
        buf.clear();
    }

    Ok(texts)
}

fn parse_core_properties(
    bytes: &[u8],
    part: &str,
    properties: &mut DocumentProperties,
) -> Result<(), ReadError> {
    let mut texts = element_texts(bytes, part)?;
    let mut take = |name: &str| texts.remove(name).filter(|value| !value.is_empty());
    properties.creator = take("creator");
    properties.last_modified_by = take("lastModifiedBy");
    properties.created = take("created");
    properties.modified = take("modified");
    properties.title = take("title");
    properties.subject = take("subject");
    properties.description = take("description");
    properties.keywords = take("keywords");
    properties.category = take("category");
    Ok(())
}

fn parse_app_properties(
    bytes: &[u8],
    part: &str,
    properties: &mut DocumentProperties,
) -> Result<(), ReadError> {
    let mut texts = element_texts(bytes, part)?;
    let mut take = |name: &str| texts.remove(name).filter(|value| !value.is_empty());
    properties.application = take("Application");
    properties.app_version = take("AppVersion");
    properties.company = take("Company");
    Ok(())
}

/// The tables of the styles part, as [`WorkbookInspection`] restores them.
struct Styles {
    num_fmts: Vec<NumFmt>,
    fonts: Vec<Font>,
    fills: Vec<Fill>,
    styles: Vec<CellStyle>,
}

impl Styles {
    /// What a workbook without cell formats gets: one style, `General`, the
    /// default font and no fill.
    fn default_only() -> Self {
        Self {
            num_fmts: vec![NumFmt {
                num_fmt_id: 0,
                format_code: builtin_format_code(0).map(str::to_owned),
            }],
            fonts: Vec::new(),
            fills: Vec::new(),
            styles: vec![CellStyle {
                num_fmt_ref: 0,
                font_ref: None,
                fill_ref: None,
                alignment: Alignment::default(),
            }],
        }
    }
}

fn color_of(element: &BytesStart<'_>, decoder: Decoder) -> Color {
    Color {
        rgb: attribute(element, b"rgb", decoder),
        theme: number_attribute(element, b"theme", decoder),
        indexed: number_attribute(element, b"indexed", decoder),
        auto: parse_bool(attribute(element, b"auto", decoder).as_deref()).unwrap_or(false),
        tint: number_attribute(element, b"tint", decoder),
    }
}

/// A font flag such as `<b/>`: present means on unless `val` says otherwise.
fn flag(element: &BytesStart<'_>, decoder: Decoder) -> bool {
    parse_bool(attribute(element, b"val", decoder).as_deref()).unwrap_or(true)
}

/// One `cellXfs` entry before it is resolved.
struct Xf {
    num_fmt_id: u32,
    font_id: usize,
    fill_id: usize,
    alignment: Alignment,
}

/// Stream through the styles part into its tables.
fn parse_styles(bytes: &[u8], part: &str) -> Result<Styles, ReadError> {
    let mut xml = reader(bytes);
    let decoder = xml.decoder();
    let mut buf = Vec::new();
    // Local names from the root down: `font` under `fonts` is a font of the
    // table; under `dxf` it is a differential format, which is not read.
    let mut path: Vec<Vec<u8>> = Vec::new();

    let mut num_fmts: BTreeMap<u32, Option<String>> = BTreeMap::new();
    let mut fonts: Vec<Font> = Vec::new();
    let mut fills: Vec<Fill> = Vec::new();
    let mut xfs: Vec<Xf> = Vec::new();

    let mut font: Option<Font> = None;
    let mut fill: Option<Fill> = None;
    let mut stop: Option<GradientStop> = None;
    let mut xf: Option<Xf> = None;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match &event {
            Event::Start(element) => {
                let name = element.local_name().as_ref().to_vec();
                let parent = path.last().map(Vec::as_slice);
                let grandparent = path.len().checked_sub(2).map(|i| path[i].as_slice());
                match (grandparent, parent, name.as_slice()) {
                    (_, Some(b"numFmts"), b"numFmt") => {
                        if let (Some(id), Some(code)) = (
                            number_attribute::<u32>(element, b"numFmtId", decoder),
                            attribute(element, b"formatCode", decoder),
                        ) {
                            num_fmts.insert(id, Some(code));
                        }
                    }
                    (_, Some(b"fonts"), b"font") => font = Some(Font::default()),
                    (Some(b"fonts"), Some(b"font"), field) => {
                        if let Some(font) = font.as_mut() {
                            match field {
                                b"b" => font.bold = flag(element, decoder),
                                b"i" => font.italic = flag(element, decoder),
                                b"strike" => font.strike = flag(element, decoder),
                                b"u" => {
                                    font.underline = Some(
                                        attribute(element, b"val", decoder)
                                            .unwrap_or_else(|| "single".to_owned()),
                                    )
                                    .filter(|value| UNDERLINE.contains(&value.as_str()))
                                }
                                b"vertAlign" => {
                                    font.vert_align =
                                        enum_attribute(element, b"val", decoder, VERT_ALIGN)
                                }
                                b"sz" => font.size = number_attribute(element, b"val", decoder),
                                b"name" => font.name = attribute(element, b"val", decoder),
                                b"color" => font.color = Some(color_of(element, decoder)),
                                _ => {}
                            }
                        }
                    }
                    (_, Some(b"fills"), b"fill") => fill = Some(Fill::default()),
                    (Some(b"fills"), Some(b"fill"), b"patternFill") => {
                        if let Some(fill) = fill.as_mut() {
                            fill.pattern_fill = Some(PatternFill {
                                pattern_type: enum_attribute(
                                    element,
                                    b"patternType",
                                    decoder,
                                    PATTERN,
                                ),
                                ..PatternFill::default()
                            });
                        }
                    }
                    (Some(b"fill"), Some(b"patternFill"), b"fgColor" | b"bgColor") => {
                        if let Some(pattern) = fill.as_mut().and_then(|f| f.pattern_fill.as_mut()) {
                            let color = Some(color_of(element, decoder));
                            if name == b"fgColor" {
                                pattern.fg_color = color;
                            } else {
                                pattern.bg_color = color;
                            }
                        }
                    }
                    (Some(b"fills"), Some(b"fill"), b"gradientFill") => {
                        if let Some(fill) = fill.as_mut() {
                            fill.gradient_fill = Some(GradientFill {
                                gradient_type: enum_attribute(
                                    element,
                                    b"type",
                                    decoder,
                                    &["linear", "path"],
                                )
                                .unwrap_or_else(|| "linear".to_owned()),
                                degree: number_attribute(element, b"degree", decoder),
                                stops: Vec::new(),
                            });
                        }
                    }
                    (Some(b"fill"), Some(b"gradientFill"), b"stop") => {
                        stop = Some(GradientStop {
                            position: number_attribute(element, b"position", decoder)
                                .unwrap_or(0.0),
                            color: Color::default(),
                        });
                    }
                    (Some(b"gradientFill"), Some(b"stop"), b"color") => {
                        if let Some(stop) = stop.as_mut() {
                            stop.color = color_of(element, decoder);
                        }
                    }
                    (_, Some(b"cellXfs"), b"xf") => {
                        xf = Some(Xf {
                            num_fmt_id: number_attribute(element, b"numFmtId", decoder)
                                .unwrap_or(0),
                            font_id: number_attribute(element, b"fontId", decoder).unwrap_or(0),
                            fill_id: number_attribute(element, b"fillId", decoder).unwrap_or(0),
                            alignment: Alignment::default(),
                        });
                    }
                    (Some(b"cellXfs"), Some(b"xf"), b"alignment") => {
                        if let Some(xf) = xf.as_mut() {
                            xf.alignment = Alignment {
                                horizontal: enum_attribute(
                                    element,
                                    b"horizontal",
                                    decoder,
                                    HORIZONTAL,
                                ),
                                vertical: enum_attribute(element, b"vertical", decoder, VERTICAL),
                                wrap_text: parse_bool(
                                    attribute(element, b"wrapText", decoder).as_deref(),
                                )
                                .unwrap_or(false),
                                shrink_to_fit: parse_bool(
                                    attribute(element, b"shrinkToFit", decoder).as_deref(),
                                )
                                .unwrap_or(false),
                                indent: number_attribute(element, b"indent", decoder),
                                text_rotation: number_attribute(element, b"textRotation", decoder),
                            };
                        }
                    }
                    _ => {}
                }
                path.push(name);
            }
            Event::End(element) => {
                let name = element.local_name();
                let parent = path.len().checked_sub(2).map(|i| path[i].as_slice());
                match (parent, name.as_ref()) {
                    (Some(b"fonts"), b"font") => fonts.extend(font.take()),
                    (Some(b"fills"), b"fill") => fills.extend(fill.take()),
                    (Some(b"gradientFill"), b"stop") => {
                        if let (Some(stop), Some(gradient)) = (
                            stop.take(),
                            fill.as_mut().and_then(|f| f.gradient_fill.as_mut()),
                        ) {
                            gradient.stops.push(stop);
                        }
                    }
                    (Some(b"cellXfs"), b"xf") => xfs.extend(xf.take()),
                    _ => {}
                }
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Without cell formats, the one default style is `General`.
    let referenced = if xfs.is_empty() {
        vec![0]
    } else {
        xfs.iter().map(|xf| xf.num_fmt_id).collect()
    };
    for id in referenced {
        num_fmts
            .entry(id)
            .or_insert_with(|| builtin_format_code(id).map(str::to_owned));
    }
    let num_fmts: Vec<NumFmt> = num_fmts
        .into_iter()
        .map(|(num_fmt_id, format_code)| NumFmt {
            num_fmt_id,
            format_code,
        })
        .collect();
    // Every identifier a style declares has its entry, and the table holds
    // at most one entry per `u32` identifier, so the index fits a `u32`.
    let num_fmt_ref = |id: u32| num_fmts.partition_point(|num_fmt| num_fmt.num_fmt_id < id) as u32;
    let within =
        |index: usize, len: usize| (index < len).then(|| u32::try_from(index).ok()).flatten();

    let mut styles: Vec<CellStyle> = xfs
        .into_iter()
        .map(|xf| CellStyle {
            num_fmt_ref: num_fmt_ref(xf.num_fmt_id),
            font_ref: within(xf.font_id, fonts.len()),
            fill_ref: within(xf.fill_id, fills.len()),
            alignment: xf.alignment,
        })
        .collect();
    if styles.is_empty() {
        styles.push(CellStyle {
            num_fmt_ref: num_fmt_ref(0),
            font_ref: None,
            fill_ref: None,
            alignment: Alignment::default(),
        });
    }

    Ok(Styles {
        num_fmts,
        fonts,
        fills,
        styles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_built_in_formats_the_specification_fixes() {
        assert_eq!(builtin_format_code(14), Some("mm-dd-yy"));
        assert_eq!(builtin_format_code(49), Some("@"));
        // Locale-dependent: the specification gives no code.
        assert_eq!(builtin_format_code(5), None);
        assert_eq!(builtin_format_code(27), None);
    }
}
