//! Table cell content and in-field formula marks.

use super::is_horizontal_ws;
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
// Table cell content and in-field formula marks
// ---------------------------------------------------------------------------

/// Object-region span of a `TABLE_CELL`: leading/trailing horizontal
/// whitespace and the closing pipe excluded.
///
/// A rewrite of this span leaves the grid pipes in place. Empty and
/// whitespace-only cells are a zero-width slot after any leading padding
/// (and before the closing pipe).
#[must_use]
pub(crate) fn table_cell_content(cell: &SyntaxNode) -> TextRange {
    debug_assert_eq!(cell.kind(), OrgSyntaxKind::TABLE_CELL);
    let mut slot = cell.text_range().start();
    let mut first_non_ws: Option<TextSize> = None;
    let mut last_non_ws: Option<TextSize> = None;
    for e in cell.children_with_tokens() {
        if e.kind() == OrgSyntaxKind::PIPE {
            break;
        }
        if is_horizontal_ws(&e) {
            if first_non_ws.is_none() {
                slot = e.text_range().end();
            }
            continue;
        }
        let r = e.text_range();
        if first_non_ws.is_none() {
            first_non_ws = Some(r.start());
        }
        last_non_ws = Some(r.end());
    }
    match (first_non_ws, last_non_ws) {
        (Some(s), Some(e)) => TextRange::new(s, e),
        _ => TextRange::empty(slot),
    }
}

/// Leading in-field formula mark inside a `TABLE_CELL` object region.
///
/// [`Self::Field`] is `=`, [`Self::Column`] is `:=`. Only a mark at the
/// start of the object region counts: `:=` later in the cell, or a
/// `VERBATIM` node that absorbed `=`, is not a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableCellFormulaMark {
    Field(TextRange),
    Column(TextRange),
}

impl TableCellFormulaMark {
    #[must_use]
    pub(crate) const fn range(self) -> TextRange {
        match self {
            Self::Field(r) | Self::Column(r) => r,
        }
    }
}

/// Scan the cell object region for a leading `=` or `:=`. No regex.
#[must_use]
pub(crate) fn table_cell_formula_mark(cell: &SyntaxNode) -> Option<TableCellFormulaMark> {
    if cell.kind() != OrgSyntaxKind::TABLE_CELL {
        return None;
    }
    let mut elems = cell.children_with_tokens().peekable();
    while elems.peek().is_some_and(is_horizontal_ws) {
        elems.next();
    }
    let first = elems.next()?.into_token()?;
    match first.kind() {
        OrgSyntaxKind::EQUALS => Some(TableCellFormulaMark::Field(first.text_range())),
        OrgSyntaxKind::COLON => {
            let second = elems.next()?.into_token()?;
            (second.kind() == OrgSyntaxKind::EQUALS).then(|| {
                TableCellFormulaMark::Column(TextRange::new(
                    first.text_range().start(),
                    second.text_range().end(),
                ))
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;

    #[test]
    fn table_cell_content_strips_padding_and_closing_pipe() {
        let src = "|  a  |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        let content = table_cell_content(&cell);
        assert_eq!(&doc.source()[content], "a");
        assert!(cell.text().to_string().ends_with('|'));
        assert!(!doc.source()[content].contains('|'));

        let src = "| a | b\n";
        let doc = parse(src);
        let cells = nodes_of_kind(doc.root(), OrgSyntaxKind::TABLE_CELL);
        assert_eq!(&doc.source()[table_cell_content(&cells[1])], "b");

        let src = "|   |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let c = table_cell_content(&cell);
        assert!(c.is_empty());
        let pipe = cell
            .children_with_tokens()
            .find(|e| e.kind() == OrgSyntaxKind::PIPE)
            .expect("pipe");
        assert_eq!(
            c.start(),
            pipe.text_range().start(),
            "empty interior sits at the closing pipe"
        );

        let src = "|a|\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        assert_eq!(&doc.source()[table_cell_content(&cell)], "a");
    }
    #[test]
    fn table_cell_formula_mark_only_at_object_region_start() {
        let src = "| =$1+$2 |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        let mark = table_cell_formula_mark(&cell).expect("field");
        assert!(matches!(mark, TableCellFormulaMark::Field(_)));
        assert_eq!(&doc.source()[mark.range()], "=");

        let src = "| :=$1 |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        let mark = table_cell_formula_mark(&cell).expect("column");
        assert!(matches!(mark, TableCellFormulaMark::Column(_)));
        assert_eq!(&doc.source()[mark.range()], ":=");

        let cell = first_kind("| x:=y |\n", OrgSyntaxKind::TABLE_CELL);
        assert!(
            table_cell_formula_mark(&cell).is_none(),
            ":= after other text is not a mark"
        );

        let src = "| =foo= |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        assert!(
            cell.children().any(|c| c.kind() == OrgSyntaxKind::VERBATIM),
            "verbatim fixture"
        );
        assert!(table_cell_formula_mark(&cell).is_none());

        let para = first_kind("plain\n", OrgSyntaxKind::PARAGRAPH);
        assert!(table_cell_formula_mark(&para).is_none());

        let src = "| :foo |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        assert!(table_cell_formula_mark(&cell).is_none());
        assert_eq!(&doc.source()[table_cell_content(&cell)], ":foo");

        let src = "| : |\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        assert!(table_cell_formula_mark(&cell).is_none());
        assert_eq!(&doc.source()[table_cell_content(&cell)], ":");

        let src = "|=$1|\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        let mark = table_cell_formula_mark(&cell).expect("unpadded field");
        assert!(matches!(mark, TableCellFormulaMark::Field(_)));
        assert_eq!(&doc.source()[mark.range()], "=");
        assert_eq!(&doc.source()[table_cell_content(&cell)], "=$1");

        let src = "|:=$1|\n";
        let cell = first_kind(src, OrgSyntaxKind::TABLE_CELL);
        let doc = parse(src);
        let mark = table_cell_formula_mark(&cell).expect("unpadded column");
        assert!(matches!(mark, TableCellFormulaMark::Column(_)));
        assert_eq!(&doc.source()[mark.range()], ":=");
    }
}
