//! Table grammar: org `|`-delimited tables with rows, cells, rule rows, and
//! trailing `#+TBLFM:` formula lines.
//!
//! Evidence (org-element.el):
//! - `org-element-table-parser`: a table is the maximal run of lines whose
//!   first non-whitespace byte is `|` (extent regexp `^[ \t]*($|[^| \t])`
//!   ends it: a blank line or any other content). After the last row,
//!   consecutive `[ \t]*#\+TBLFM: +VALUE` lines (case-insensitive; at least
//!   one space after the colon) are PART of the table element; a blank line
//!   stops the run.
//! - **table.el `+-` grids are not supported.** org-element also recognises
//!   legacy Emacs `table.el` grids (border lines starting `+-` then `+`/`-`,
//!   body often with `|` cells) as **one** table of type `table.el`. This
//!   parser handles org `|` tables only: a `+-` line never opens a table and
//!   falls through to a paragraph or another element. Interior `|` lines
//!   still form ordinary org `TABLE`s when they start an element, which is
//!   correct org grammar rather than table.el support. There is no
//!   `table.el` type and no single border-plus-body element
//!   (`table_el_plus_minus_is_not_table` pins this). Supporting table.el
//!   would be a new construct.
//! - `org-element-table-row-parser`: a row is one line; type `rule` when it
//!   starts `|-`, else `standard`. Rule rows have no cells.
//! - `org-element-table-cell-parser` (`[ \t]*(.*?)[ \t]*(?:|\|$)`): each
//!   cell spans from after the previous pipe through ITS closing pipe (or
//!   the line end when the final pipe is missing); contents are the
//!   whitespace-trimmed interior, parsed as objects under the table-cell
//!   restriction.
//! - Paragraph separation: a `|` line interrupts a paragraph.
//!
//! Emits:
//! ```text
//! TABLE
//!   (TABLE_ROW | TABLE_RULE_ROW)+
//!   KEYWORD*                   // directly-consecutive #+TBLFM: lines
//!   [post-blank tokens]
//! TABLE_ROW
//!   [INDENT] PIPE TABLE_CELL* [NEWLINE]
//! TABLE_CELL
//!   [WHITESPACE] objects [WHITESPACE] [PIPE]
//! ```
//!
//! Contract: [`probe`] is the one-line `|` shape (dispatcher row and
//! paragraph interruption); [`parse`] consumes the whole run.

use crate::parser::{LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

use super::keyword;
use crate::parser::grammar::drive::{atoms, region};

/// A table starts at any line whose content token is `|`.
pub(crate) fn probe(line: LineView) -> Option<()> {
    (line.kind(0) == OrgSyntaxKind::PIPE).then_some(())
}

/// Whether this table line is a rule row (`|-` prefix).
fn is_rule_row(line: LineView) -> bool {
    line.kind(0) == OrgSyntaxKind::PIPE
        && line.kind(1) == OrgSyntaxKind::TEXT
        && line.text(1).starts_with('-')
}

/// Whether `line` is a `#+TBLFM:` formula line (case-insensitive key and at
/// least one space after the colon, per org's `#\+TBLFM: +` collection).
fn is_tblfm_line(line: LineView) -> bool {
    atoms::line_keyword_prefix(line).is_some_and(|key| {
        line.text_eq_caseless(key, "tblfm")
            && line.kind(key + 1) == OrgSyntaxKind::COLON
            && line.kind(key + 2) == OrgSyntaxKind::WHITESPACE
    })
}

/// Parse the whole table: row run, formula lines, post-blank.
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser.line().is_some_and(|l| probe(l).is_some()),
        "table::parse called without probe (dispatcher contract)"
    );

    let marker = parser.start();
    while parser.line().is_some_and(|l| probe(l).is_some()) {
        parse_row(parser);
    }

    // Directly-consecutive #+TBLFM: lines attach to the table; a blank line
    // stops the run (keyword::parse_line does not claim post-blank).
    loop {
        let shape = parser
            .line()
            .filter(|l| is_tblfm_line(*l))
            .and_then(keyword::probe);
        match shape {
            Some(shape) => keyword::parse_line(parser, shape),
            None => break,
        }
    }

    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::TABLE);
}

/// Parse one row line (standard with cells, or an opaque rule row).
fn parse_row(parser: &mut Parser) {
    let rule = parser.line().is_some_and(|l| is_rule_row(l));
    let marker = parser.start();

    if rule {
        parser.eat_line();
        marker.complete(parser, OrgSyntaxKind::TABLE_RULE_ROW);
        return;
    }

    parser.eat(OrgSyntaxKind::INDENT);
    parser.eat(OrgSyntaxKind::PIPE);
    while !parser.at_line_term() {
        parse_cell(parser);
    }
    parser.eat(OrgSyntaxKind::NEWLINE);
    marker.complete(parser, OrgSyntaxKind::TABLE_ROW);
}

/// Parse one cell: `[ws] contents [ws] [|]` (the closing pipe is inside the
/// cell; the last cell of a pipe-less row ends at the line terminator).
fn parse_cell(parser: &mut Parser) {
    let marker = parser.start();
    parser.eat(OrgSyntaxKind::WHITESPACE);

    if let Some(end) = cell_content_end(parser) {
        region::inline_in(parser, end, OrgSyntaxKind::TABLE_CELL);
    }
    parser.eat(OrgSyntaxKind::WHITESPACE);
    parser.eat(OrgSyntaxKind::PIPE);
    marker.complete(parser, OrgSyntaxKind::TABLE_CELL);
}

/// Exclusive end of the trimmed cell interior from the cursor: after the
/// last non-whitespace token before the next `|` or the line end. `None`
/// when the interior is empty.
fn cell_content_end(parser: &Parser) -> Option<Pos> {
    let mut last = None;
    for t in parser.tokens_from_cursor() {
        match t.kind() {
            OrgSyntaxKind::PIPE | OrgSyntaxKind::NEWLINE => break,
            OrgSyntaxKind::WHITESPACE => {}
            _ => last = Some(t),
        }
    }
    last.map(|t| t.end())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{format_tree, nodes_of_kind, parse_document_tree, probe_line, tree_text};

    fn tables(root: &crate::syntax::SyntaxNode) -> Vec<crate::syntax::SyntaxNode> {
        nodes_of_kind(root, OrgSyntaxKind::TABLE)
    }

    // --- Recognition --------------------------------------------------------

    #[test]
    fn pipe_lines_recognised() {
        assert!(probe_line("| a |\n", probe).is_some());
        assert!(probe_line("|a|\n", probe).is_some());
        assert!(probe_line("  | indented |\n", probe).is_some());
        assert!(probe_line("|-\n", probe).is_some());
        assert!(probe_line("|\n", probe).is_some());
        assert!(probe_line("not | a table\n", probe).is_none());
        assert!(
            probe_line("+-+-+\n", probe).is_none(),
            "table.el border is not an org table opener"
        );
    }

    /// Legacy Emacs `table.el` grids are not a table type here. org-element
    /// would emit one `table` of type `table.el` covering borders and body.
    /// This parser:
    /// - never treats a `+-` border line as a table opener (probe rejects);
    /// - has no `table.el` type or opaque table.el body;
    /// - may still emit org `TABLE` nodes for interior `|` rows (those are
    ///   genuine org table lines when seen at element start).
    #[test]
    fn table_el_plus_minus_is_not_table() {
        // Border-only: must not be TABLE (the structural claim for +-).
        let border = "+------+-----+\n";
        let root_b = parse_document_tree(border);
        assert_eq!(tree_text(&root_b), border);
        assert!(
            tables(&root_b).is_empty(),
            "table.el +- border alone is not an org TABLE"
        );
        assert!(probe_line(border, probe).is_none());

        // Full legacy grid: lossless; +- never opens a table; interior |
        // lines may still form org TABLEs, since only | tables are supported.
        let src = "\
+------+-----+
|Name  |Phone|
+------+-----+
|Peter |1234 |
+------+-----+
";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "lossless without table.el support");
        // First line is +- so the document does not open as one table.el
        // element. Interior | rows are free to form org tables.
        assert!(
            !tables(&root).is_empty(),
            "interior | rows still form org TABLEs"
        );
        // No row is parented under a construct that started at +-: every
        // TABLE's source slice must start with | (optional indent), not +.
        for t in tables(&root) {
            let text = tree_text(&t);
            let first = text.trim_start_matches([' ', '\t']);
            assert!(
                first.starts_with('|'),
                "TABLE must open on |, not table.el border: {text:?}"
            );
        }
    }

    // --- Structure ----------------------------------------------------------

    #[test]
    fn minimal_and_multi_row_tables() {
        let src = "| a |\n";
        let root = parse_document_tree(src);
        let t = tables(&root);
        assert_eq!(t.len(), 1);
        assert_eq!(nodes_of_kind(&t[0], OrgSyntaxKind::TABLE_ROW).len(), 1);
        let cells = nodes_of_kind(&t[0], OrgSyntaxKind::TABLE_CELL);
        assert_eq!(cells.len(), 1);
        assert_eq!(tree_text(&cells[0]), " a |");

        let src2 = "| a | b |\n|---+---|\n| c | d |\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let t2 = &tables(&root2)[0];
        assert_eq!(nodes_of_kind(t2, OrgSyntaxKind::TABLE_ROW).len(), 2);
        assert_eq!(nodes_of_kind(t2, OrgSyntaxKind::TABLE_RULE_ROW).len(), 1);
        assert_eq!(nodes_of_kind(t2, OrgSyntaxKind::TABLE_CELL).len(), 4);
    }

    #[test]
    fn missing_trailing_pipe_and_uneven_columns() {
        let src = "| a | b\n| c |\n";
        let root = parse_document_tree(src);
        let t = &tables(&root)[0];
        let rows = nodes_of_kind(t, OrgSyntaxKind::TABLE_ROW);
        assert_eq!(rows.len(), 2);
        let row0_cells = nodes_of_kind(&rows[0], OrgSyntaxKind::TABLE_CELL);
        assert_eq!(row0_cells.len(), 2);
        assert_eq!(
            tree_text(&row0_cells[1]),
            " b",
            "no closing pipe on last cell"
        );
        assert_eq!(
            nodes_of_kind(&rows[1], OrgSyntaxKind::TABLE_CELL).len(),
            1,
            "uneven column counts are fine"
        );
    }

    #[test]
    fn empty_cells_and_rule_only_table() {
        let src = "|| a ||\n";
        let root = parse_document_tree(src);
        let cells = nodes_of_kind(&tables(&root)[0], OrgSyntaxKind::TABLE_CELL);
        assert_eq!(cells.len(), 3, "empty | a | empty");

        let src2 = "|---|\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let t2 = &tables(&root2)[0];
        assert_eq!(nodes_of_kind(t2, OrgSyntaxKind::TABLE_RULE_ROW).len(), 1);
        assert_eq!(nodes_of_kind(t2, OrgSyntaxKind::TABLE_CELL).len(), 0);
    }

    #[test]
    fn cells_with_objects_and_restriction() {
        let src = "| *bold* | <2026-06-24 Wed> | [[https://x][d]] |\n";
        let root = parse_document_tree(src);
        let t = &tables(&root)[0];
        assert!(
            !nodes_of_kind(t, OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            nodes_of_kind(t, OrgSyntaxKind::BOLD)
        );
        assert!(
            !nodes_of_kind(t, OrgSyntaxKind::TIMESTAMP).is_empty(),
            "{:?}",
            nodes_of_kind(t, OrgSyntaxKind::TIMESTAMP)
        );
        assert!(
            !nodes_of_kind(t, OrgSyntaxKind::LINK).is_empty(),
            "{:?}",
            nodes_of_kind(t, OrgSyntaxKind::LINK)
        );

        // Statistics cookies are excluded from cells (restriction).
        let src2 = "| [1/2] |\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            0,
            "no cookies in table cells"
        );
    }

    #[test]
    fn code_markup_pipe_stays_cell_boundary() {
        // A `|` inside code markup is still a cell boundary: the cell region
        // is measured before objects parse (org cell-first taxonomy).
        let src = "| ~a|b~ |\n";
        let root = parse_document_tree(src);
        let cells = nodes_of_kind(&tables(&root)[0], OrgSyntaxKind::TABLE_CELL);
        assert_eq!(cells.len(), 2, "pipe splits even inside would-be markup");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CODE).len(), 0);
    }

    // --- TBLFM --------------------------------------------------------------

    #[test]
    fn tblfm_lines_attach_to_table() {
        let src = "| a |\n#+TBLFM: $2=$1*2\n#+tblfm: $3=$1\n";
        let root = parse_document_tree(src);
        let t = &tables(&root)[0];
        assert_eq!(tree_text(t), src, "formulas are table children");
        assert_eq!(nodes_of_kind(t, OrgSyntaxKind::KEYWORD).len(), 2);
    }

    #[test]
    fn tblfm_after_blank_is_separate_keyword() {
        let src = "| a |\n\n#+TBLFM: $1=2\n";
        let root = parse_document_tree(src);
        let t = &tables(&root)[0];
        assert_eq!(
            nodes_of_kind(t, OrgSyntaxKind::KEYWORD).len(),
            0,
            "blank line stops the formula run"
        );
        assert_eq!(tree_text(t), "| a |\n\n", "table owns its post-blank");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(),
            1,
            "the formula line is an ordinary keyword element"
        );
    }

    #[test]
    fn tblfm_without_space_is_ordinary_keyword() {
        // org collects `#+TBLFM: +` (at least one space); `#+TBLFM:x` is not
        // part of the table.
        let src = "| a |\n#+TBLFM:x\n";
        let root = parse_document_tree(src);
        let t = &tables(&root)[0];
        assert_eq!(nodes_of_kind(t, OrgSyntaxKind::KEYWORD).len(), 0);
    }

    // --- Interruption / placement ------------------------------------------

    #[test]
    fn table_interrupts_paragraph() {
        let src = "text\n| a |\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert!(paras.iter().any(|p| tree_text(p) == "text\n"));
        assert_eq!(tables(&root).len(), 1);
    }

    #[test]
    fn table_in_list_item_and_quote_block() {
        let src = "- item\n  | a |\n";
        let root = parse_document_tree(src);
        let item = &nodes_of_kind(&root, OrgSyntaxKind::ITEM)[0];
        assert_eq!(tables(item).len(), 1, "indented table inside item");

        let src2 = "#+begin_quote\n| q |\n#+end_quote\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let quote = &nodes_of_kind(&root2, OrgSyntaxKind::QUOTE_BLOCK)[0];
        assert_eq!(tables(quote).len(), 1);
    }

    #[test]
    fn table_at_section_end_owns_post_blank() {
        let src = "* H\n| a |\n\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&tables(&root)[0]), "| a |\n\n");
    }

    #[test]
    fn affiliated_keywords_attach_to_table() {
        let src = "#+name: tbl\n#+caption: c\n| a |\n";
        let root = parse_document_tree(src);
        // The run node sits before TABLE; the table absorbs nothing.
        assert_eq!(tables(&root).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(), 2);
        let run = tables(&root)[0]
            .prev_sibling()
            .expect("affiliated run above the table");
        assert_eq!(run.kind(), OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(tree_text(&run), "#+name: tbl\n#+caption: c\n");
    }

    /// Shape fixtures: `(source, TABLE count, TABLE_ROW count, TABLE_CELL count)`.
    ///
    /// Losslessness is not asserted: `builder::build` already checks gap-free
    /// tiling on every parse, in release builds too. These fixtures pin row
    /// and cell carving.
    const SHAPE_FIXTURES: &[(&str, usize, usize, usize)] = &[
        ("| a | b |\n|---+---|\n| c | d |\n", 1, 3, 4), // 2 data rows + rule row (no cells)
        ("| a\n", 1, 1, 1),                             // unterminated row
        ("|\n", 1, 1, 0),
        ("|   |\n", 1, 1, 1), // whitespace-only cell
        ("| a |\n#+TBLFM: $1=1\nafter\n", 1, 1, 1),
        ("| x |\r\n| y |\r\n", 1, 2, 2), // CRLF
        ("  | ind |\n", 1, 1, 1),        // indented
        ("para\n| t |\npara2\n", 1, 1, 1),
    ];

    #[test]
    fn tiling_fixtures_produce_expected_table_shape() {
        for &(src, tables_n, rows, cells) in SHAPE_FIXTURES {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::TABLE).len(),
                tables_n,
                "TABLE count for {src:?}\n{}",
                format_tree(&root)
            );
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::TABLE_ROW).len()
                    + nodes_of_kind(&root, OrgSyntaxKind::TABLE_RULE_ROW).len(),
                rows,
                "row count for {src:?}\n{}",
                format_tree(&root)
            );
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::TABLE_CELL).len(),
                cells,
                "TABLE_CELL count for {src:?}\n{}",
                format_tree(&root)
            );
        }
    }
}
