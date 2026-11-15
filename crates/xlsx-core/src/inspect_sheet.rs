//! What a sheet declares besides its cells: dimension, tab colour, panes,
//! columns, hidden rows, merged ranges, hyperlinks, tables, data validations
//! (the `x14` extension included), conditional formats, protection, autofilter,
//! comments, and how many pictures its drawing holds.
//!
//! The sheet part is streamed — its metadata sits on both sides of the cell
//! data in the XML, so the whole part is read, cells skipped — and every byte
//! inflated, the parts it links to included, is charged against
//! `max_decompressed_bytes`, and so is the memory every record built from
//! them takes beyond the text it holds. References stay in A1 notation as declared; a
//! position the format numbers from 1 (`col@min`, `row@r`) is given 0-based
//! under a name of its own.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use zip::ZipArchive;

use crate::budget::{inflate_entry_within, map_zip_error, Budget};
use crate::cells::{open_part_stream, Charged};
use crate::directory::enforce_entry_budget;
use crate::error::ReadError;
use crate::inspect_workbook::Color;
use crate::peek::{
    attribute, entry_index, entry_name, parse_sheet_relationship_ids, relationships_of,
    resolve_target, workbook_target,
};
use crate::xml::{
    append_text, enum_attribute, is_kind, number_attribute, parse_bool, parse_relationships,
    reader, Relationship,
};

/// The frozen or split pane of the sheet's first view — `CT_Pane`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    /// Columns left of the split, for a frozen pane; a width otherwise.
    pub x_split: Option<f64>,
    /// Rows above the split, for a frozen pane; a height otherwise.
    pub y_split: Option<f64>,
    /// First visible cell of the bottom-right pane, in A1 notation.
    pub top_left_cell: Option<String>,
    /// `split`, `frozen` or `frozenSplit` — `ST_PaneState`.
    pub state: String,
}

/// A run of columns sharing a definition — `CT_Col`, positions 0-based.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRange {
    /// First column of the run, 0-based (`min` in the file, 1-based).
    pub first_col: u32,
    /// Last column of the run, 0-based (`max` in the file, 1-based).
    pub last_col: u32,
    /// Width, in characters.
    pub width: Option<f64>,
    /// Whether the columns are hidden.
    pub hidden: bool,
    /// Outline (grouping) level.
    pub outline_level: Option<u32>,
}

/// A hyperlink — `CT_Hyperlink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    /// The cell or range it is anchored to, in A1 notation.
    pub reference: String,
    /// Where it leads, resolved through the sheet's relationships.
    pub target: Option<String>,
    /// A location inside the workbook (`Sheet2!A1`).
    pub location: Option<String>,
    /// The tooltip.
    pub tooltip: Option<String>,
    /// The text displayed.
    pub display: Option<String>,
}

/// A column of a table — `CT_TableColumn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumn {
    /// Header of the column.
    pub name: String,
    /// `ST_TotalsRowFunction`.
    pub totals_row_function: Option<String>,
    /// Label of the totals row.
    pub totals_row_label: Option<String>,
    /// Custom formula of the totals row.
    pub totals_row_formula: Option<String>,
    /// Formula every cell of the column computes.
    pub calculated_column_formula: Option<String>,
}

/// A table — `CT_Table`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The name formulas use.
    pub name: String,
    /// The name the interface shows.
    pub display_name: String,
    /// The range it covers, header and totals included, in A1 notation.
    pub reference: String,
    /// Header rows, `1` unless declared otherwise.
    pub header_row_count: u32,
    /// Totals rows, `0` unless declared otherwise.
    pub totals_row_count: u32,
    /// The columns, in order.
    pub columns: Vec<TableColumn>,
}

/// A data validation rule — `CT_DataValidation`, or its `x14` extension.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataValidation {
    /// The ranges it applies to, in A1 notation.
    pub sqref: Vec<String>,
    /// Whether it comes from the `x14` extension (Excel 2010 and later), which
    /// is where a list drawn from another sheet is written.
    pub extension: bool,
    /// `ST_DataValidationType`.
    pub validation_type: Option<String>,
    /// `ST_DataValidationOperator`.
    pub operator: Option<String>,
    /// Whether a blank passes.
    pub allow_blank: bool,
    /// As the format defines it: `true` *hides* the in-cell dropdown.
    pub show_drop_down: bool,
    /// Whether the input message shows.
    pub show_input_message: bool,
    /// Whether the error message shows.
    pub show_error_message: bool,
    /// `stop`, `warning` or `information` — `ST_DataValidationErrorStyle`.
    pub error_style: Option<String>,
    /// Title of the error message.
    pub error_title: Option<String>,
    /// The error message.
    pub error: Option<String>,
    /// Title of the input message.
    pub prompt_title: Option<String>,
    /// The input message.
    pub prompt: Option<String>,
    /// First formula, as written.
    pub formula1: Option<String>,
    /// Second formula, for `between` and `notBetween`.
    pub formula2: Option<String>,
}

/// A conditional formatting rule — `CT_CfRule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalFormatRule {
    /// `ST_CfType`.
    pub rule_type: String,
    /// Evaluation order, lowest first.
    pub priority: u32,
    /// `ST_ConditionalFormattingOperator`.
    pub operator: Option<String>,
    /// Whether rules of lower priority stop applying once this one does.
    pub stop_if_true: bool,
    /// Text a text rule looks for.
    pub text: Option<String>,
    /// `ST_TimePeriod`.
    pub time_period: Option<String>,
    /// The rule's formulas, as written.
    pub formulas: Vec<String>,
    /// Index of the differential format it applies. Absent for a rule of the
    /// `x14` extension, which writes its format inline, and that format is
    /// not restored.
    pub dxf_id: Option<u32>,
}

/// Conditional formatting over some ranges — `CT_ConditionalFormatting`, or
/// its `x14` extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalFormatting {
    /// The ranges, in A1 notation.
    pub sqref: Vec<String>,
    /// Whether it comes from the `x14` extension (Excel 2010 and later), which
    /// is where a rule drawing on another sheet, or a custom icon set, is
    /// written. A rule of the extension that extends a standard rule — the
    /// extra properties of a data bar — is not restored: the standard rule
    /// stands for it.
    pub extension: bool,
    /// The rules, in order.
    pub rules: Vec<ConditionalFormatRule>,
}

/// Sheet protection — the flags of `CT_SheetProtection`, with the defaults
/// the format gives them. For an action flag, `true` means the action is
/// *prevented* while the sheet is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)] // one field per attribute of the format, named after it
pub struct SheetProtection {
    /// Whether a password is set. Its hash is never restored.
    pub password_protected: bool,
    pub sheet: bool,
    pub objects: bool,
    pub scenarios: bool,
    pub format_cells: bool,
    pub format_columns: bool,
    pub format_rows: bool,
    pub insert_columns: bool,
    pub insert_rows: bool,
    pub insert_hyperlinks: bool,
    pub delete_columns: bool,
    pub delete_rows: bool,
    pub select_locked_cells: bool,
    pub sort: bool,
    pub auto_filter: bool,
    pub pivot_tables: bool,
    pub select_unlocked_cells: bool,
}

/// A comment, in the classic comments part. A threaded comment of recent
/// versions of Excel is always written there too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The cell it is attached to, in A1 notation.
    pub reference: String,
    /// 0-based index of its author in [`SheetInspection::comment_authors`];
    /// absent when the comment names an author the part does not list.
    pub author_ref: Option<u32>,
    /// The text, runs concatenated.
    pub text: String,
}

/// What a sheet declares besides its cells.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SheetInspection {
    /// The range the sheet declares it uses, in A1 notation. Written by the
    /// application that saved the file, and not checked against the cells.
    pub dimension: Option<String>,
    /// Colour of the tab.
    pub tab_color: Option<Color>,
    /// Frozen or split pane of the first view.
    pub pane: Option<Pane>,
    /// Column definitions.
    pub cols: Vec<ColumnRange>,
    /// 0-based positions of the hidden rows.
    pub hidden_rows: Vec<u32>,
    /// Merged ranges, in A1 notation.
    pub merge_cells: Vec<String>,
    /// Hyperlinks.
    pub hyperlinks: Vec<Hyperlink>,
    /// Tables.
    pub tables: Vec<Table>,
    /// Data validations, standard and `x14`, in document order.
    pub data_validations: Vec<DataValidation>,
    /// Conditional formatting.
    pub conditional_formatting: Vec<ConditionalFormatting>,
    /// Protection, when the sheet declares it.
    pub sheet_protection: Option<SheetProtection>,
    /// Range of the autofilter, in A1 notation.
    pub auto_filter: Option<String>,
    /// Authors of the comments, each held once, as the comments part lists
    /// them; a comment refers to its author by index.
    pub comment_authors: Vec<String>,
    /// Comments.
    pub comments: Vec<Comment>,
    /// Pictures in the sheet's drawing.
    pub picture_count: u32,
}

const PANE_STATE: &[&str] = &["split", "frozen", "frozenSplit"];
const DV_TYPE: &[&str] = &[
    "none",
    "whole",
    "decimal",
    "list",
    "date",
    "time",
    "textLength",
    "custom",
];
const DV_OPERATOR: &[&str] = &[
    "between",
    "notBetween",
    "equal",
    "notEqual",
    "lessThan",
    "lessThanOrEqual",
    "greaterThan",
    "greaterThanOrEqual",
];
const DV_ERROR_STYLE: &[&str] = &["stop", "warning", "information"];
const CF_TYPE: &[&str] = &[
    "expression",
    "cellIs",
    "colorScale",
    "dataBar",
    "iconSet",
    "top10",
    "uniqueValues",
    "duplicateValues",
    "containsText",
    "notContainsText",
    "beginsWith",
    "endsWith",
    "containsBlanks",
    "notContainsBlanks",
    "containsErrors",
    "notContainsErrors",
    "timePeriod",
    "aboveAverage",
];
const CF_OPERATOR: &[&str] = &[
    "lessThan",
    "lessThanOrEqual",
    "equal",
    "notEqual",
    "greaterThanOrEqual",
    "greaterThan",
    "between",
    "notBetween",
    "containsText",
    "notContains",
    "beginsWith",
    "endsWith",
];
const TIME_PERIOD: &[&str] = &[
    "today",
    "yesterday",
    "tomorrow",
    "last7Days",
    "thisMonth",
    "lastMonth",
    "nextMonth",
    "thisWeek",
    "lastWeek",
    "nextWeek",
];
const TOTALS_ROW_FUNCTION: &[&str] = &[
    "none",
    "sum",
    "min",
    "max",
    "average",
    "count",
    "countNums",
    "stdDev",
    "var",
    "custom",
];

/// Inspect one sheet: everything it declares besides its cells.
///
/// # Errors
///
/// `SHEET_NOT_FOUND` when the workbook has no sheet by that name;
/// `DECOMPRESSED_BUDGET_EXCEEDED` or `TOO_MANY_ENTRIES` when the parts read
/// expand past the budget; `CORRUPT` when a part does not parse or a
/// relationship points to a part the archive does not hold.
pub fn inspect_sheet(
    path: &Path,
    sheet: &str,
    max_decompressed_bytes: u64,
) -> Result<SheetInspection, ReadError> {
    enforce_entry_budget(path, max_decompressed_bytes)?;

    let file = File::open(path).map_err(|error| ReadError::Io {
        detail: format!("cannot open {}: {error}", path.display()),
    })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = entry_index(&archive)?;
    let mut budget = Budget::new(max_decompressed_bytes);

    let package_rels = inflate_entry_within(
        &mut archive,
        entry_name(&entries, "_rels/.rels"),
        &mut budget,
    )?;
    let workbook_path = workbook_target(&package_rels)?;
    let workbook = inflate_entry_within(
        &mut archive,
        entry_name(&entries, &workbook_path),
        &mut budget,
    )?;
    let ids = parse_sheet_relationship_ids(&workbook)?;
    let Some((_, sheet_id)) = ids.iter().find(|(name, _)| name == sheet) else {
        return Err(ReadError::SheetNotFound {
            name: sheet.to_owned(),
            available: ids.into_iter().map(|(name, _)| name).collect(),
        });
    };
    let workbook_rels_path = relationships_of(&workbook_path);
    let workbook_rels = inflate_entry_within(
        &mut archive,
        entry_name(&entries, &workbook_rels_path),
        &mut budget,
    )?;
    let workbook_rels = parse_relationships(&workbook_rels, &workbook_rels_path)?;
    let sheet_part = internal_target(
        &by_id(&workbook_rels),
        sheet_id,
        &workbook_path,
        &workbook_rels_path,
    )?;

    let mut inspection = SheetInspection::default();
    let references = stream_sheet(
        path,
        entry_name(&entries, &sheet_part),
        &mut budget,
        &mut inspection,
    )?;

    let sheet_rels_path = relationships_of(&sheet_part);
    let sheet_rels = if entries.contains_key(&sheet_rels_path.to_ascii_lowercase()) {
        let bytes = inflate_entry_within(
            &mut archive,
            entry_name(&entries, &sheet_rels_path),
            &mut budget,
        )?;
        parse_relationships(&bytes, &sheet_rels_path)?
    } else {
        Vec::new()
    };

    let rels_by_id = by_id(&sheet_rels);

    // Every hyperlink receives its own copy of its target, and any number of
    // them may name the same relationship: each copy is charged, or a few
    // bytes of links would multiply one long target past the budget.
    let copied_targets = format!("hyperlink targets copied from {sheet_rels_path}");
    for (index, id) in references.hyperlinks {
        let relationship = find(&rels_by_id, &id, &sheet_rels_path)?;
        budget.charge(relationship.target.len() as u64, &copied_targets)?;
        inspection.hyperlinks[index].target = Some(relationship.target.clone());
    }

    for id in references.tables {
        let part = internal_target(&rels_by_id, &id, &sheet_part, &sheet_rels_path)?;
        let bytes = inflate_entry_within(&mut archive, entry_name(&entries, &part), &mut budget)?;
        inspection
            .tables
            .push(parse_table(&bytes, &part, &mut budget)?);
    }

    if let Some(id) = references.drawing {
        let part = internal_target(&rels_by_id, &id, &sheet_part, &sheet_rels_path)?;
        let bytes = inflate_entry_within(&mut archive, entry_name(&entries, &part), &mut budget)?;
        inspection.picture_count = count_pictures(&bytes, &part)?;
    }

    if let Some(relationship) = sheet_rels
        .iter()
        .find(|r| !r.external && is_kind(r, "comments"))
    {
        let part = resolve_target(&sheet_part, &relationship.target)?;
        let bytes = inflate_entry_within(&mut archive, entry_name(&entries, &part), &mut budget)?;
        (inspection.comment_authors, inspection.comments) =
            parse_comments(&bytes, &part, &mut budget)?;
    }

    Ok(inspection)
}

/// Relationships by id, so that resolving many references costs one lookup
/// each rather than a scan of the part. An id declared twice resolves to its
/// first declaration.
fn by_id(relationships: &[Relationship]) -> HashMap<&str, &Relationship> {
    let mut index = HashMap::with_capacity(relationships.len());
    for relationship in relationships {
        index
            .entry(relationship.id.as_str())
            .or_insert(relationship);
    }
    index
}

fn find<'a>(
    relationships: &HashMap<&str, &'a Relationship>,
    id: &str,
    part: &str,
) -> Result<&'a Relationship, ReadError> {
    relationships
        .get(id)
        .copied()
        .ok_or_else(|| ReadError::corrupt(format!("{part} declares no relationship {id:?}")))
}

fn internal_target(
    relationships: &HashMap<&str, &Relationship>,
    id: &str,
    source: &str,
    part: &str,
) -> Result<String, ReadError> {
    let relationship = find(relationships, id, part)?;
    if relationship.external {
        return Err(ReadError::corrupt(format!(
            "{part}: relationship {id:?} points outside the package where a part is expected"
        )));
    }
    resolve_target(source, &relationship.target)
}

/// The `r:id` of an element, whatever prefix the part binds to the
/// relationships namespace.
fn relationship_id(element: &BytesStart<'_>, decoder: Decoder) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| attr.key.local_name().as_ref() == b"id" && attr.key.prefix().is_some())
        .and_then(|attr| {
            attr.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                .ok()
                .map(|value| value.into_owned())
        })
}

fn bool_attribute(element: &BytesStart<'_>, name: &[u8], decoder: Decoder, default: bool) -> bool {
    parse_bool(attribute(element, name, decoder).as_deref()).unwrap_or(default)
}

fn sqref_of(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

/// Memory a record of type `T` takes beyond the text it holds, which the bytes
/// it is read from already pay for. An empty element of a few bytes builds a
/// record many times its size: left uncharged, a part well within the budget
/// would build records far past it.
const fn footprint<T>() -> u64 {
    std::mem::size_of::<T>() as u64
}

/// [`footprint`] of a list of `count` strings, beyond their text.
fn strings(count: usize) -> u64 {
    footprint::<String>() * count as u64
}

/// The entry a refusal names when the records built from `part` cross the
/// budget.
fn records_of(part: &str) -> String {
    format!("records produced from {part}")
}

/// Relationship ids the sheet part refers to, resolved once it is read.
#[derive(Default)]
struct References {
    /// Index in the hyperlink list, and the relationship of its target.
    hyperlinks: Vec<(usize, String)>,
    tables: Vec<String>,
    drawing: Option<String>,
}

/// Which text is being collected, and from which depth.
enum Capture {
    Formula1,
    Formula2,
    Sqref,
    CfFormula,
    CfSqref,
    CfLinkId,
}

fn stream_sheet(
    path: &Path,
    part: &str,
    budget: &mut Budget,
    inspection: &mut SheetInspection,
) -> Result<References, ReadError> {
    let already = budget.used();
    let charged = Charged {
        inner: open_part_stream(path, part)?,
        used: already,
        limit: budget.limit(),
        part: part.to_owned(),
        exceeded: false,
    };
    let mut xml = XmlReader::from_reader(BufReader::new(charged));
    xml.config_mut().expand_empty_elements = true;
    let decoder = xml.decoder();

    let mut references = References::default();
    let mut buf = Vec::new();
    let mut path_stack: Vec<Vec<u8>> = Vec::new();
    let mut row_index: u32 = 0;
    let mut sheet_views = 0u32;
    let mut validation: Option<DataValidation> = None;
    let mut formatting: Option<ConditionalFormatting> = None;
    let mut rule: Option<ConditionalFormatRule> = None;
    // Ids of the `x14` rules that standard rules declare as their extension:
    // the extension block comes last in the part, so they are all known by
    // the time it is read.
    let mut extended: HashSet<String> = HashSet::new();
    let mut dropped_extension = false;
    let mut capture: Option<(Capture, usize, String)> = None;
    let records = records_of(part);
    let mut produced = 0u64;

    loop {
        let event = match xml.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(error) => {
                let charged = xml.get_ref().get_ref();
                if charged.exceeded {
                    return Err(ReadError::DecompressedBudgetExceeded {
                        limit: charged.limit,
                        entry: format!("{part:?}"),
                    });
                }
                return Err(ReadError::corrupt(format!(
                    "{part} does not parse: {error}"
                )));
            }
        };

        match &event {
            Event::Start(element) => {
                let name = element.local_name().as_ref().to_vec();
                let parent = path_stack.last().map(Vec::as_slice);
                let grandparent = path_stack
                    .len()
                    .checked_sub(2)
                    .map(|i| path_stack[i].as_slice());

                match (grandparent, parent, name.as_slice()) {
                    (_, Some(b"sheetData"), b"row") => {
                        if let Some(r) = number_attribute::<u32>(element, b"r", decoder) {
                            if r == 0 {
                                return Err(ReadError::corrupt("a row declares number 0"));
                            }
                            row_index = r - 1;
                        }
                        if bool_attribute(element, b"hidden", decoder, false) {
                            inspection.hidden_rows.push(row_index);
                            produced += footprint::<u32>();
                        }
                    }
                    (_, Some(b"sheetData"), _) => {}
                    (Some(b"sheetData"), _, _) => {}
                    (_, Some(b"sheetPr"), b"tabColor") => {
                        inspection.tab_color = Some(color_of(element, decoder));
                    }
                    (_, Some(b"worksheet"), b"dimension") => {
                        inspection.dimension = attribute(element, b"ref", decoder);
                    }
                    (_, Some(b"sheetViews"), b"sheetView") => sheet_views += 1,
                    (_, Some(b"sheetView"), b"pane") if sheet_views == 1 => {
                        inspection.pane = Some(Pane {
                            x_split: number_attribute(element, b"xSplit", decoder),
                            y_split: number_attribute(element, b"ySplit", decoder),
                            top_left_cell: attribute(element, b"topLeftCell", decoder),
                            state: enum_attribute(element, b"state", decoder, PANE_STATE)
                                .unwrap_or_else(|| "split".to_owned()),
                        });
                    }
                    (_, Some(b"cols"), b"col") => {
                        let min = number_attribute::<u32>(element, b"min", decoder);
                        let max = number_attribute::<u32>(element, b"max", decoder);
                        if let (Some(min), Some(max)) = (min, max) {
                            if min >= 1 && max >= min {
                                inspection.cols.push(ColumnRange {
                                    first_col: min - 1,
                                    last_col: max - 1,
                                    width: number_attribute(element, b"width", decoder),
                                    hidden: bool_attribute(element, b"hidden", decoder, false),
                                    outline_level: number_attribute(
                                        element,
                                        b"outlineLevel",
                                        decoder,
                                    ),
                                });
                                produced += footprint::<ColumnRange>();
                            }
                        }
                    }
                    (_, Some(b"worksheet"), b"sheetProtection") => {
                        let flag = |name: &[u8], default: bool| {
                            bool_attribute(element, name, decoder, default)
                        };
                        inspection.sheet_protection = Some(SheetProtection {
                            password_protected: attribute(element, b"password", decoder).is_some()
                                || attribute(element, b"hashValue", decoder).is_some(),
                            sheet: flag(b"sheet", false),
                            objects: flag(b"objects", false),
                            scenarios: flag(b"scenarios", false),
                            format_cells: flag(b"formatCells", true),
                            format_columns: flag(b"formatColumns", true),
                            format_rows: flag(b"formatRows", true),
                            insert_columns: flag(b"insertColumns", true),
                            insert_rows: flag(b"insertRows", true),
                            insert_hyperlinks: flag(b"insertHyperlinks", true),
                            delete_columns: flag(b"deleteColumns", true),
                            delete_rows: flag(b"deleteRows", true),
                            select_locked_cells: flag(b"selectLockedCells", false),
                            sort: flag(b"sort", true),
                            auto_filter: flag(b"autoFilter", true),
                            pivot_tables: flag(b"pivotTables", true),
                            select_unlocked_cells: flag(b"selectUnlockedCells", false),
                        });
                    }
                    (_, Some(b"worksheet"), b"autoFilter") => {
                        inspection.auto_filter = attribute(element, b"ref", decoder);
                    }
                    (_, Some(b"mergeCells"), b"mergeCell") => {
                        if let Some(reference) = attribute(element, b"ref", decoder) {
                            inspection.merge_cells.push(reference);
                            produced += strings(1);
                        }
                    }
                    (_, Some(b"worksheet"), b"conditionalFormatting") => {
                        let sqref =
                            sqref_of(&attribute(element, b"sqref", decoder).unwrap_or_default());
                        produced += footprint::<ConditionalFormatting>() + strings(sqref.len());
                        formatting = Some(ConditionalFormatting {
                            sqref,
                            extension: false,
                            rules: Vec::new(),
                        });
                    }
                    (Some(b"ext"), Some(b"conditionalFormattings"), b"conditionalFormatting") => {
                        produced += footprint::<ConditionalFormatting>();
                        dropped_extension = false;
                        formatting = Some(ConditionalFormatting {
                            sqref: Vec::new(),
                            extension: true,
                            rules: Vec::new(),
                        });
                    }
                    (Some(b"conditionalFormattings"), Some(b"conditionalFormatting"), b"sqref")
                        if formatting.is_some() =>
                    {
                        capture = Some((Capture::CfSqref, path_stack.len() + 1, String::new()));
                    }
                    (Some(owner), Some(b"conditionalFormatting"), b"cfRule")
                        if owner == b"conditionalFormattings"
                            && attribute(element, b"id", decoder)
                                .is_some_and(|id| extended.contains(&id)) =>
                    {
                        dropped_extension = true;
                    }
                    (Some(owner), Some(b"conditionalFormatting"), b"cfRule")
                        if owner == b"worksheet" || owner == b"conditionalFormattings" =>
                    {
                        rule =
                            enum_attribute(element, b"type", decoder, CF_TYPE).map(|rule_type| {
                                ConditionalFormatRule {
                                    rule_type,
                                    priority: number_attribute(element, b"priority", decoder)
                                        .unwrap_or(0),
                                    operator: enum_attribute(
                                        element,
                                        b"operator",
                                        decoder,
                                        CF_OPERATOR,
                                    ),
                                    stop_if_true: bool_attribute(
                                        element,
                                        b"stopIfTrue",
                                        decoder,
                                        false,
                                    ),
                                    text: attribute(element, b"text", decoder),
                                    time_period: enum_attribute(
                                        element,
                                        b"timePeriod",
                                        decoder,
                                        TIME_PERIOD,
                                    ),
                                    formulas: Vec::new(),
                                    dxf_id: number_attribute(element, b"dxfId", decoder),
                                }
                            });
                        if rule.is_some() {
                            produced += footprint::<ConditionalFormatRule>();
                        }
                    }
                    // `formula` in the standard part, `xm:f` in the extension;
                    // an `xm:f` deeper down is a threshold of a data bar or an
                    // icon set, not a formula of the rule.
                    (Some(b"conditionalFormatting"), Some(b"cfRule"), b"formula" | b"f")
                        if rule.is_some() =>
                    {
                        capture = Some((Capture::CfFormula, path_stack.len() + 1, String::new()));
                    }
                    (Some(b"extLst"), Some(b"ext"), b"id")
                        if rule.is_some() && formatting.as_ref().is_some_and(|f| !f.extension) =>
                    {
                        capture = Some((Capture::CfLinkId, path_stack.len() + 1, String::new()));
                    }
                    (Some(owner), Some(b"dataValidations"), b"dataValidation")
                        if owner == b"worksheet" || owner == b"ext" =>
                    {
                        let sqref =
                            sqref_of(&attribute(element, b"sqref", decoder).unwrap_or_default());
                        produced += footprint::<DataValidation>() + strings(sqref.len());
                        validation = Some(DataValidation {
                            sqref,
                            extension: owner == b"ext",
                            validation_type: Some(
                                enum_attribute(element, b"type", decoder, DV_TYPE)
                                    .unwrap_or_else(|| "none".to_owned()),
                            ),
                            operator: enum_attribute(element, b"operator", decoder, DV_OPERATOR),
                            allow_blank: bool_attribute(element, b"allowBlank", decoder, false),
                            show_drop_down: bool_attribute(
                                element,
                                b"showDropDown",
                                decoder,
                                false,
                            ),
                            show_input_message: bool_attribute(
                                element,
                                b"showInputMessage",
                                decoder,
                                false,
                            ),
                            show_error_message: bool_attribute(
                                element,
                                b"showErrorMessage",
                                decoder,
                                false,
                            ),
                            error_style: enum_attribute(
                                element,
                                b"errorStyle",
                                decoder,
                                DV_ERROR_STYLE,
                            ),
                            error_title: attribute(element, b"errorTitle", decoder),
                            error: attribute(element, b"error", decoder),
                            prompt_title: attribute(element, b"promptTitle", decoder),
                            prompt: attribute(element, b"prompt", decoder),
                            formula1: None,
                            formula2: None,
                        });
                    }
                    (_, Some(b"dataValidation"), b"formula1") if validation.is_some() => {
                        capture = Some((Capture::Formula1, path_stack.len() + 1, String::new()));
                    }
                    (_, Some(b"dataValidation"), b"formula2") if validation.is_some() => {
                        capture = Some((Capture::Formula2, path_stack.len() + 1, String::new()));
                    }
                    (_, Some(b"dataValidation"), b"sqref") if validation.is_some() => {
                        capture = Some((Capture::Sqref, path_stack.len() + 1, String::new()));
                    }
                    (_, Some(b"hyperlinks"), b"hyperlink") => {
                        if let Some(reference) = attribute(element, b"ref", decoder) {
                            if let Some(id) = relationship_id(element, decoder) {
                                references
                                    .hyperlinks
                                    .push((inspection.hyperlinks.len(), id));
                                produced += footprint::<(usize, String)>();
                            }
                            inspection.hyperlinks.push(Hyperlink {
                                reference,
                                target: None,
                                location: attribute(element, b"location", decoder),
                                tooltip: attribute(element, b"tooltip", decoder),
                                display: attribute(element, b"display", decoder),
                            });
                            produced += footprint::<Hyperlink>();
                        }
                    }
                    (_, Some(b"worksheet"), b"drawing") => {
                        references.drawing = relationship_id(element, decoder);
                    }
                    (_, Some(b"tableParts"), b"tablePart") => {
                        if let Some(id) = relationship_id(element, decoder) {
                            references.tables.push(id);
                            produced += strings(1);
                        }
                    }
                    _ => {}
                }
                path_stack.push(name);
            }
            Event::End(element) => {
                let name = element.local_name();
                let parent = path_stack
                    .len()
                    .checked_sub(2)
                    .map(|i| path_stack[i].as_slice());

                if let Some((kind, depth, text)) = capture.take() {
                    if depth == path_stack.len() {
                        match kind {
                            Capture::Formula1 => {
                                if let Some(v) = validation.as_mut() {
                                    v.formula1 = Some(text);
                                }
                            }
                            Capture::Formula2 => {
                                if let Some(v) = validation.as_mut() {
                                    v.formula2 = Some(text);
                                }
                            }
                            Capture::Sqref => {
                                if let Some(v) = validation.as_mut() {
                                    v.sqref = sqref_of(&text);
                                    produced += strings(v.sqref.len());
                                }
                            }
                            Capture::CfFormula => {
                                if let Some(r) = rule.as_mut() {
                                    r.formulas.push(text);
                                    produced += strings(1);
                                }
                            }
                            Capture::CfSqref => {
                                if let Some(f) = formatting.as_mut() {
                                    f.sqref = sqref_of(&text);
                                    produced += strings(f.sqref.len());
                                }
                            }
                            Capture::CfLinkId => {
                                extended.insert(text.trim().to_owned());
                                produced += strings(1);
                            }
                        }
                    } else {
                        capture = Some((kind, depth, text));
                    }
                }

                match (parent, name.as_ref()) {
                    (Some(b"sheetData"), b"row") => row_index = row_index.saturating_add(1),
                    (Some(b"conditionalFormatting"), b"cfRule") => {
                        if let (Some(r), Some(f)) = (rule.take(), formatting.as_mut()) {
                            f.rules.push(r);
                        }
                    }
                    (Some(b"worksheet"), b"conditionalFormatting") => {
                        inspection.conditional_formatting.extend(formatting.take());
                    }
                    // An extension block left with no rule once the rules it
                    // only extended are dropped goes with them.
                    (Some(b"conditionalFormattings"), b"conditionalFormatting") => {
                        inspection.conditional_formatting.extend(
                            formatting
                                .take()
                                .filter(|f| !(dropped_extension && f.rules.is_empty())),
                        );
                    }
                    (Some(b"dataValidations"), b"dataValidation") => {
                        inspection.data_validations.extend(validation.take());
                    }
                    _ => {}
                }
                path_stack.pop();
            }
            Event::Eof => break,
            other => {
                if let Some((_, _, text)) = capture.as_mut() {
                    append_text(text, other, part)?;
                }
            }
        }
        buf.clear();

        if xml.get_ref().get_ref().used.saturating_add(produced) > budget.limit() {
            return Err(ReadError::DecompressedBudgetExceeded {
                limit: budget.limit(),
                entry: format!("{records:?}"),
            });
        }
    }

    let streamed = xml.get_ref().get_ref().used - already;
    budget.charge(streamed, part)?;
    budget.charge(produced, &records)?;
    Ok(references)
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

fn parse_table(bytes: &[u8], part: &str, budget: &mut Budget) -> Result<Table, ReadError> {
    let mut xml = reader(bytes);
    let decoder = xml.decoder();
    let records = records_of(part);
    let mut buf = Vec::new();
    let mut table: Option<Table> = None;
    let mut column: Option<TableColumn> = None;
    // 1 for the calculated formula, 2 for the totals one.
    let mut capture: Option<(u8, String)> = None;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match &event {
            Event::Start(element) => match element.local_name().as_ref() {
                b"table" if table.is_none() => {
                    budget.charge(footprint::<Table>(), &records)?;
                    let name = attribute(element, b"name", decoder).unwrap_or_default();
                    table = Some(Table {
                        display_name: attribute(element, b"displayName", decoder)
                            .unwrap_or_else(|| name.clone()),
                        name,
                        reference: attribute(element, b"ref", decoder).ok_or_else(|| {
                            ReadError::corrupt(format!("{part}: a table declares no range"))
                        })?,
                        header_row_count: number_attribute(element, b"headerRowCount", decoder)
                            .unwrap_or(1),
                        totals_row_count: number_attribute(element, b"totalsRowCount", decoder)
                            .unwrap_or(0),
                        columns: Vec::new(),
                    });
                }
                b"tableColumn" => {
                    budget.charge(footprint::<TableColumn>(), &records)?;
                    column = Some(TableColumn {
                        name: attribute(element, b"name", decoder).unwrap_or_default(),
                        totals_row_function: enum_attribute(
                            element,
                            b"totalsRowFunction",
                            decoder,
                            TOTALS_ROW_FUNCTION,
                        ),
                        totals_row_label: attribute(element, b"totalsRowLabel", decoder),
                        totals_row_formula: None,
                        calculated_column_formula: None,
                    });
                }
                b"calculatedColumnFormula" if column.is_some() => {
                    capture = Some((1, String::new()));
                }
                b"totalsRowFormula" if column.is_some() => capture = Some((2, String::new())),
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                b"calculatedColumnFormula" | b"totalsRowFormula" => {
                    if let (Some((kind, text)), Some(c)) = (capture.take(), column.as_mut()) {
                        if kind == 1 {
                            c.calculated_column_formula = Some(text);
                        } else {
                            c.totals_row_formula = Some(text);
                        }
                    }
                }
                b"tableColumn" => {
                    if let (Some(c), Some(t)) = (column.take(), table.as_mut()) {
                        t.columns.push(c);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            other => {
                if let Some((_, text)) = capture.as_mut() {
                    append_text(text, other, part)?;
                }
            }
        }
        buf.clear();
    }

    table.ok_or_else(|| ReadError::corrupt(format!("{part} declares no table")))
}

/// The authors of a comments part, and its comments referring to them by
/// index — never a copy of the name per comment, which would grow as the
/// product of the two lists while the byte budget only pays for their sum.
fn parse_comments(
    bytes: &[u8],
    part: &str,
    budget: &mut Budget,
) -> Result<(Vec<String>, Vec<Comment>), ReadError> {
    let mut xml = reader(bytes);
    let decoder = xml.decoder();
    let records = records_of(part);
    let mut buf = Vec::new();
    let mut authors: Vec<String> = Vec::new();
    let mut author: Option<String> = None;
    let mut comments = Vec::new();
    let mut comment: Option<(Comment, Option<usize>)> = None;
    let mut in_text = 0usize;
    let mut in_run_text = false;
    let mut in_phonetic = false;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?;
        match &event {
            Event::Start(element) => match element.local_name().as_ref() {
                b"author" => {
                    budget.charge(strings(1), &records)?;
                    author = Some(String::new());
                }
                b"comment" => {
                    budget.charge(footprint::<Comment>(), &records)?;
                    comment = Some((
                        Comment {
                            reference: attribute(element, b"ref", decoder).unwrap_or_default(),
                            author_ref: None,
                            text: String::new(),
                        },
                        number_attribute(element, b"authorId", decoder),
                    ));
                }
                b"text" if comment.is_some() => in_text += 1,
                // Phonetic guides of East Asian text are not part of the text.
                b"rPh" => in_phonetic = true,
                b"t" if in_text > 0 && !in_phonetic => in_run_text = true,
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                b"author" => authors.extend(author.take()),
                b"comment" => {
                    if let Some((mut c, id)) = comment.take() {
                        c.author_ref = id
                            .filter(|&id| id < authors.len())
                            .and_then(|id| u32::try_from(id).ok());
                        comments.push(c);
                    }
                }
                b"text" => in_text = in_text.saturating_sub(1),
                b"rPh" => in_phonetic = false,
                b"t" => in_run_text = false,
                _ => {}
            },
            Event::Eof => break,
            other => {
                if let Some(name) = author.as_mut() {
                    append_text(name, other, part)?;
                } else if in_run_text {
                    if let Some((c, _)) = comment.as_mut() {
                        append_text(&mut c.text, other, part)?;
                    }
                }
            }
        }
        buf.clear();
    }

    Ok((authors, comments))
}

fn count_pictures(bytes: &[u8], part: &str) -> Result<u32, ReadError> {
    let mut xml = reader(bytes);
    let mut buf = Vec::new();
    let mut count = 0u32;
    loop {
        match xml
            .read_event_into(&mut buf)
            .map_err(|error| ReadError::corrupt(format!("{part} does not parse: {error}")))?
        {
            Event::Start(element) if element.local_name().as_ref() == b"pic" => {
                count = count.saturating_add(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(count)
}
