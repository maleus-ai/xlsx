//! What a sheet declares besides its cells, restored as declared.

mod common;

use common::fixture;
use xlsx_core::{
    inspect_sheet, Color, ColumnRange, Comment, ConditionalFormatRule, DataValidation, Hyperlink,
    Pane, SheetInspection, Table, TableColumn,
};

fn anatomy() -> SheetInspection {
    inspect_sheet(&fixture("sheet-anatomy"), "Data", 1 << 30).expect("inspect")
}

#[test]
fn restores_merged_ranges_in_a1_notation() {
    assert_eq!(anatomy().merge_cells, vec!["A1:B1", "C2:C4"]);
}

#[test]
fn merges_standard_and_x14_validations_in_one_list_marking_the_extension() {
    let validations = anatomy().data_validations;

    assert_eq!(validations.len(), 2);
    assert_eq!(
        validations[0],
        DataValidation {
            sqref: vec!["B2:B6".into(), "D2".into()],
            extension: false,
            validation_type: Some("list".into()),
            allow_blank: true,
            show_input_message: true,
            show_error_message: true,
            error_style: Some("warning".into()),
            error_title: Some("Statut".into()),
            error: Some("Choisir dans la liste".into()),
            formula1: Some("\"Ouvert,Fermé\"".into()),
            ..DataValidation::default()
        }
    );
    assert!(validations[1].extension);
    assert_eq!(validations[1].sqref, vec!["E2:E6"]);
    assert_eq!(validations[1].formula1.as_deref(), Some("Lists!$A$1:$A$3"));
}

#[test]
fn splits_a_multiple_sqref_into_a_list_of_ranges() {
    assert_eq!(
        anatomy().conditional_formatting[0].sqref,
        vec!["A1:A6", "C1:C6"]
    );
}

#[test]
fn restores_conditional_formatting_rules_with_their_priority_and_formulas() {
    let rules = &anatomy().conditional_formatting[0].rules;

    // The third rule declares a type the format does not define: dropped.
    assert_eq!(rules.len(), 2);
    assert_eq!(
        rules[0],
        ConditionalFormatRule {
            rule_type: "cellIs".into(),
            priority: 2,
            operator: Some("greaterThan".into()),
            stop_if_true: false,
            text: None,
            time_period: None,
            formulas: vec!["10".into()],
            dxf_id: Some(0),
        }
    );
    assert_eq!(rules[1].time_period.as_deref(), Some("last7Days"));
    assert!(rules[1].stop_if_true);
}

#[test]
fn merges_standard_and_x14_conditional_formats_without_restoring_an_extended_rule_twice() {
    let formatting = anatomy().conditional_formatting;

    // The data bar is written in both: standard, and an x14 rule extending it.
    assert_eq!(formatting.len(), 3);
    assert!(!formatting[1].extension);
    assert_eq!(formatting[1].sqref, vec!["B2:B6"]);
    assert_eq!(formatting[1].rules.len(), 1);
    assert_eq!(formatting[1].rules[0].rule_type, "dataBar");

    let x14 = &formatting[2];
    assert!(x14.extension);
    assert_eq!(x14.sqref, vec!["F2:F6", "F8"]);
    assert_eq!(
        x14.rules
            .iter()
            .map(|r| (r.rule_type.as_str(), r.priority, r.dxf_id))
            .collect::<Vec<_>>(),
        vec![("expression", 5, None), ("iconSet", 6, None)]
    );
    assert_eq!(x14.rules[0].formulas, vec!["Lists!$A$1=\"Ouvert\""]);
    // The thresholds of the icon set are not formulas of the rule.
    assert!(x14.rules[1].formulas.is_empty());
}

#[test]
fn restores_a_table_with_its_columns_and_its_totals_row() {
    assert_eq!(
        anatomy().tables,
        vec![Table {
            name: "Suivi".into(),
            display_name: "Suivi_2024".into(),
            reference: "A1:C6".into(),
            header_row_count: 1,
            totals_row_count: 1,
            columns: vec![
                TableColumn {
                    name: "Client".into(),
                    totals_row_function: None,
                    totals_row_label: Some("Total".into()),
                    totals_row_formula: None,
                    calculated_column_formula: None,
                },
                TableColumn {
                    name: "Montant".into(),
                    totals_row_function: Some("sum".into()),
                    totals_row_label: None,
                    totals_row_formula: None,
                    calculated_column_formula: None,
                },
                TableColumn {
                    name: "TTC".into(),
                    totals_row_function: None,
                    totals_row_label: None,
                    totals_row_formula: None,
                    calculated_column_formula: Some("Suivi_2024[[#This Row],[Montant]]*1.2".into()),
                },
            ],
        }]
    );
}

#[test]
fn restores_a_comment_with_its_author_and_its_cell() {
    let inspection = anatomy();

    assert_eq!(inspection.comment_authors, vec!["Ada", "Grace"]);
    assert_eq!(
        inspection.comments,
        vec![
            Comment {
                reference: "B2".into(),
                author_ref: Some(1),
                text: "Grace: à vérifier".into(),
            },
            Comment {
                reference: "A6".into(),
                author_ref: Some(0),
                text: "Simple".into(),
            },
            // Names an author the part does not list.
            Comment {
                reference: "C3".into(),
                author_ref: None,
                text: "Orphan".into(),
            },
        ]
    );
}

#[test]
fn resolves_the_target_of_an_external_link_through_the_sheet_relationships() {
    assert_eq!(
        anatomy().hyperlinks,
        vec![
            Hyperlink {
                reference: "A1".into(),
                target: Some("https://github.com/maleus-ai/xlsx".into()),
                location: None,
                tooltip: Some("Le site".into()),
                display: Some("maleus-ai/xlsx".into()),
            },
            Hyperlink {
                reference: "A6".into(),
                target: None,
                location: Some("'Data'!A1".into()),
                tooltip: None,
                display: None,
            },
        ]
    );
}

#[test]
fn restores_frozen_panes_hidden_columns_0_based_and_hidden_rows() {
    let inspection = anatomy();

    assert_eq!(
        inspection.pane,
        Some(Pane {
            x_split: Some(1.0),
            y_split: Some(2.0),
            top_left_cell: Some("B3".into()),
            state: "frozen".into(),
        })
    );
    assert_eq!(
        inspection.cols,
        vec![
            ColumnRange {
                first_col: 1,
                last_col: 1,
                width: Some(0.0),
                hidden: true,
                outline_level: None,
            },
            ColumnRange {
                first_col: 3,
                last_col: 5,
                width: Some(18.5),
                hidden: false,
                outline_level: Some(1),
            },
        ]
    );
    // Row 3 by its `r`; the next row declares none and follows it.
    assert_eq!(inspection.hidden_rows, vec![2, 3]);
    assert_eq!(inspection.dimension.as_deref(), Some("A1:F6"));
    assert_eq!(inspection.auto_filter.as_deref(), Some("A1:C6"));
    assert_eq!(
        inspection.tab_color,
        Some(Color {
            rgb: Some("FFFF0000".into()),
            ..Color::default()
        })
    );
}

#[test]
fn reports_that_a_protection_has_a_password_without_ever_restoring_its_hash() {
    let protection = anatomy().sheet_protection.expect("protected");

    assert!(protection.password_protected);
    assert!(protection.sheet);
    assert!(!protection.format_cells, "declared 0");
    assert!(!protection.sort, "declared 0");
    assert!(protection.format_columns, "the format's default");
    assert!(!protection.select_locked_cells, "the format's default");
    assert!(!format!("{protection:?}").contains("CC1A"));
}

#[test]
fn counts_the_pictures_of_the_sheet_drawing() {
    assert_eq!(anatomy().picture_count, 2);
}

#[test]
fn traverses_the_sheet_data_without_allocating_per_cell() {
    // Inspecting a 200 000 row sheet finds nothing to report and holds nothing
    // of its cells: what is measured here is that it reads through.
    let inspection = inspect_sheet(&fixture("large-200000"), "Data", 1 << 30).expect("inspect");
    assert!(inspection.hidden_rows.is_empty());
    assert!(inspection.merge_cells.is_empty());
}

#[test]
fn refuses_a_sheet_relationship_that_leaves_the_archive_or_names_a_missing_part() {
    for name in ["sheet-missing-part", "sheet-escaping-relationship"] {
        let error = inspect_sheet(&fixture(name), "Data", 1 << 30).expect_err("must refuse");
        assert_eq!(error.code(), "CORRUPT", "{name}");
    }
}

#[test]
fn an_unknown_sheet_names_the_alternatives() {
    let error = inspect_sheet(&fixture("sheets-2"), "Nope", 1 << 30).expect_err("must refuse");
    assert_eq!(error.code(), "SHEET_NOT_FOUND");
    assert!(error.to_string().contains("\"Sheet1\""));
}

#[test]
fn finds_the_sheet_part_when_entries_use_backslashes_and_another_case() {
    // The sheet streams from `xl\worksheets\sheet2.xml`, declared as
    // `worksheets/sheet2.xml`.
    let inspection =
        inspect_sheet(&fixture("backslash-entries"), "Notes", 1 << 30).expect("inspect");

    assert!(inspection.merge_cells.is_empty());
    assert!(inspection.hidden_rows.is_empty());
}
