//! Texts laid out at a width: columns, hanging indents, stacking, and none at all.

use super::*;

const ROWS: &[Row] = &[
    row(
        "Find",
        "ready [ID]",
        "issues nobody holds, the subtree of ID",
    ),
    row("", "list [-s STATUS] [-a]", "issues"),
    row("", "search TEXT   blocked   dep tree ID   dep cycles", ""),
    BLANK,
    row("Flags", "", "--json anywhere for machine output"),
];

#[test]
fn a_table_wraps_each_description_under_its_column_and_a_long_term_has_its_line() {
    let expected = "\
Find   ready [ID]  issues nobody holds, the
                   subtree of ID
       list [-s STATUS] [-a]
                   issues
       search TEXT   blocked   dep tree ID
         dep cycles

Flags  --json anywhere for machine output
";
    assert_eq!(lay_out(&[Block::Rows(ROWS)], Some(50)), expected);
}

#[test]
fn a_narrow_width_stacks_the_rows() {
    let expected = "\
Find
  ready [ID]
    issues nobody holds, the
    subtree of ID
  list [-s STATUS] [-a]
    issues
  search TEXT   blocked
    dep tree ID   dep cycles

Flags
  --json anywhere for machine
  output
";
    assert_eq!(lay_out(&[Block::Rows(ROWS)], Some(30)), expected);
}

#[test]
fn without_a_width_nothing_wraps() {
    let text = "a paragraph long enough to wrap anywhere narrower than itself";
    let laid = lay_out(&[Block::Text(text), Block::Rows(ROWS)], None);
    assert_eq!(
        laid.lines().collect::<Vec<_>>(),
        [
            text,
            "",
            "Find   ready [ID]             issues nobody holds, the subtree of ID",
            "       list [-s STATUS] [-a]  issues",
            "       search TEXT   blocked   dep tree ID   dep cycles",
            "",
            "Flags  --json anywhere for machine output",
        ]
    );
}

#[test]
fn a_long_term_has_its_line_and_brackets_and_hyphens_hold() {
    let rows = &[
        row("", "short", "fits"),
        row(
            "",
            "create TITLE [--parent P] [--deps blocks:ID]",
            "a long one",
        ),
    ];
    assert_eq!(
        table(rows, Some(40)),
        [
            "short  fits",
            "create TITLE [--parent P]",
            "  [--deps blocks:ID]",
            "       a long one",
        ]
    );
    // no break after a hyphen, inside a word or inside brackets
    assert_eq!(
        paragraph("parent-child [-l LABEL] supercalifragilistic", Some(12)),
        "parent-child\n[-l LABEL]\nsupercalifragilistic"
    );
}

#[test]
fn wide_characters_take_two_columns() {
    let rows = &[row("", "寛寛", "x"), row("", "ab", "y")];
    assert_eq!(table(rows, None), ["寛寛  x", "ab    y"]);
    assert_eq!(paragraph("寛寛 寛寛", Some(6)), "寛寛\n寛寛");
}

#[test]
fn the_width_is_columns_at_most_a_hundred() {
    assert_eq!(width(Some(OsStr::new("72"))), Some(72));
    assert_eq!(width(Some(OsStr::new("300"))), Some(READABLE));
}
