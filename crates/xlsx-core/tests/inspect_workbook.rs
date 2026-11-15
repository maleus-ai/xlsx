//! What a workbook declares about itself, restored as declared.

mod common;

use common::fixture;
use xlsx_core::{
    inspect_workbook, Alignment, CellStyle, Color, DefinedName, GradientFill, GradientStop, NumFmt,
    PatternFill, SheetState, WorkbookInspection, WorkbookParts,
};

fn anatomy() -> WorkbookInspection {
    inspect_workbook(&fixture("workbook-anatomy"), 1 << 30).expect("inspect")
}

#[test]
fn restores_core_and_app_properties_with_dates_as_written() {
    let properties = anatomy().properties;

    assert_eq!(properties.creator.as_deref(), Some("Ada & Grace"));
    assert_eq!(properties.last_modified_by.as_deref(), Some("Alan"));
    assert_eq!(properties.created.as_deref(), Some("2024-03-25T09:30:00Z"));
    assert_eq!(properties.modified.as_deref(), Some("2024-04-01T17:00:00Z"));
    assert_eq!(properties.title.as_deref(), Some("Planning"));
    assert_eq!(properties.keywords.as_deref(), Some("budget, 2024"));
    assert_eq!(properties.subject, None);
    assert_eq!(properties.application.as_deref(), Some("Microsoft Excel"));
    assert_eq!(properties.app_version.as_deref(), Some("16.0300"));
    assert_eq!(properties.company.as_deref(), Some("Contoso"));
}

#[test]
fn restores_a_workbook_name_and_a_sheet_scoped_name_with_its_scope() {
    let names = anatomy().defined_names;

    assert_eq!(
        names[0],
        DefinedName {
            name: "Statuses".into(),
            formula: "Lists!$A$1:$A$3".into(),
            local_sheet_id: None,
            hidden: false,
            comment: None,
        }
    );
    assert_eq!(
        names[1],
        DefinedName {
            name: "Local".into(),
            formula: "Data!$B$2".into(),
            local_sheet_id: Some(0),
            hidden: true,
            comment: Some("scoped".into()),
        }
    );
}

#[test]
fn restores_a_defined_name_referring_to_ref_without_interpreting_it() {
    assert_eq!(anatomy().defined_names[2].formula, "#REF!$A$1");
}

#[test]
fn reports_a_vba_project_pivot_tables_external_links_and_drawings() {
    let inspection = anatomy();

    assert!(inspection.date1904);
    assert_eq!(
        inspection.parts,
        WorkbookParts {
            vba_project: true,
            pivot_tables: true,
            external_links: true,
            drawings: true,
        }
    );
    assert_eq!(
        inspection
            .sheets
            .iter()
            .map(|s| (s.name.as_str(), s.state))
            .collect::<Vec<_>>(),
        vec![("Data", SheetState::Visible), ("Lists", SheetState::Hidden)]
    );
}

#[test]
fn resolves_built_in_number_formats_and_restores_custom_ones_as_declared() {
    let inspection = anatomy();

    assert_eq!(
        inspection.num_fmts,
        vec![
            NumFmt {
                num_fmt_id: 0,
                format_code: Some("General".into())
            },
            NumFmt {
                num_fmt_id: 14,
                format_code: Some("mm-dd-yy".into())
            },
            // 27 is a locale-dependent date format: the specification gives no code.
            NumFmt {
                num_fmt_id: 27,
                format_code: None
            },
            NumFmt {
                num_fmt_id: 164,
                format_code: Some("#,##0.00 \"€\"".into())
            },
        ]
    );
    // A reference is an index into the table, not the `numFmtId` it holds.
    assert_eq!(
        inspection
            .styles
            .iter()
            .map(|style| style.num_fmt_ref)
            .collect::<Vec<_>>(),
        vec![0, 3, 1, 2, 0]
    );
    assert_eq!(
        inspection
            .styles
            .iter()
            .map(|style| inspection.num_fmts[style.num_fmt_ref as usize].num_fmt_id)
            .collect::<Vec<_>>(),
        vec![0, 164, 14, 27, 0]
    );
}

#[test]
fn refers_to_each_font_and_fill_by_its_index_in_its_table() {
    let inspection = anatomy();

    assert_eq!(inspection.fonts.len(), 2);
    assert_eq!(inspection.fills.len(), 3);
    assert_eq!(
        inspection
            .styles
            .iter()
            .map(|style| (style.font_ref, style.fill_ref))
            .collect::<Vec<_>>(),
        vec![
            (Some(0), Some(0)),
            (Some(1), Some(1)),
            (Some(0), Some(2)),
            (Some(0), Some(0)),
            (None, Some(0)),
        ]
    );
}

#[test]
fn restores_a_font_with_its_theme_colour_and_tint_unconverted() {
    let font = &anatomy().fonts[1];

    assert!(font.bold);
    assert!(!font.italic, "<i val=\"0\"/> is off");
    assert_eq!(font.underline.as_deref(), Some("double"));
    assert_eq!(font.vert_align.as_deref(), Some("superscript"));
    assert_eq!(font.size, Some(14.0));
    assert_eq!(font.name.as_deref(), Some("Arial"));
    assert_eq!(
        font.color,
        Some(Color {
            theme: Some(1),
            tint: Some(-0.25),
            ..Color::default()
        })
    );
}

#[test]
fn restores_a_solid_fill_and_a_gradient_fill_with_its_stops() {
    let fills = anatomy().fills;

    assert_eq!(
        fills[1].pattern_fill,
        Some(PatternFill {
            pattern_type: Some("solid".into()),
            fg_color: Some(Color {
                rgb: Some("FFFFFF00".into()),
                ..Color::default()
            }),
            bg_color: Some(Color {
                indexed: Some(64),
                ..Color::default()
            }),
        })
    );
    assert_eq!(
        fills[2].gradient_fill,
        Some(GradientFill {
            gradient_type: "linear".into(),
            degree: Some(90.0),
            stops: vec![
                GradientStop {
                    position: 0.0,
                    color: Color {
                        rgb: Some("FF0000FF".into()),
                        ..Color::default()
                    },
                },
                GradientStop {
                    position: 1.0,
                    color: Color {
                        theme: Some(4),
                        ..Color::default()
                    },
                },
            ],
        })
    );
}

#[test]
fn restores_alignment_and_drops_a_value_outside_the_format() {
    let styles = anatomy().styles;

    assert_eq!(
        styles[1].alignment,
        Alignment {
            horizontal: Some("center".into()),
            vertical: Some("top".into()),
            wrap_text: true,
            shrink_to_fit: false,
            indent: Some(2),
            text_rotation: Some(45),
        }
    );
    assert_eq!(
        styles[3].alignment.horizontal, None,
        "\"sideways\" is not an ST_HorizontalAlignment"
    );
}

#[test]
fn a_differential_font_does_not_land_in_the_font_table() {
    // The last cell format refers to a third font the table does not have. Read
    // as a table font, the dxf's bold font would have become that third font.
    let inspection = anatomy();
    assert_eq!(inspection.fonts.len(), 2);
    assert_eq!(inspection.styles[4].font_ref, None);
}

#[test]
fn restores_a_workbook_without_styles_with_a_default_style() {
    let inspection = inspect_workbook(&fixture("bare-workbook"), 1 << 30).expect("inspect");

    assert_eq!(
        inspection.styles,
        vec![CellStyle {
            num_fmt_ref: 0,
            font_ref: None,
            fill_ref: None,
            alignment: Alignment::default(),
        }]
    );
    assert_eq!(
        inspection.num_fmts,
        vec![NumFmt {
            num_fmt_id: 0,
            format_code: Some("General".into())
        }]
    );
    assert!(inspection.fonts.is_empty());
    assert!(inspection.fills.is_empty());
    assert!(inspection.defined_names.is_empty());
    assert_eq!(inspection.properties.application, None);
    assert!(!inspection.date1904);
    assert_eq!(inspection.parts, WorkbookParts::default());
}

#[test]
fn finds_its_parts_when_entries_use_backslashes_and_another_case() {
    let inspection = inspect_workbook(&fixture("backslash-entries"), 1 << 30).expect("inspect");

    assert_eq!(inspection.sheets.len(), 2);
    // Four cell formats in `xl\styles.xml`: the table was found, not defaulted.
    assert_eq!(inspection.styles.len(), 4);
    let num_fmt = &inspection.num_fmts[inspection.styles[1].num_fmt_ref as usize];
    assert_eq!(num_fmt.num_fmt_id, 14);
}

#[test]
fn reads_no_sheet() {
    // The sheets of `bomb-sharedstrings` are fine; its shared strings are not,
    // and an inspection never reaches them.
    let inspection =
        inspect_workbook(&fixture("bomb-sharedstrings"), 8 * 1024 * 1024).expect("inspect");
    assert_eq!(inspection.sheets.len(), 1);
}
