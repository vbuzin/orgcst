//! Tables, rows and cells.
//!
//! Tree shape:
//!
//! ```text
//!   TABLE
//!     (TABLE_ROW | TABLE_RULE_ROW)+
//!     KEYWORD*                 // directly-consecutive #+TBLFM:
//!     [post-blank]
//!   TABLE_ROW
//!     [INDENT] PIPE TABLE_CELL* [NEWLINE]
//!   TABLE_CELL
//!     [WHITESPACE] objects [WHITESPACE] [PIPE]
//! ```
//!
//! The closing pipe belongs to the cell; the leading pipe belongs to the
//! row. A rule row is opaque and has no cells. [`Table`] does not implement
//! `HasParts` or `HasElements`.

use super::keyword::Keyword;
use crate::TextRange;
use crate::TextSize;
use crate::ast::layout::{TableCellFormulaMark, table_cell_content, table_cell_formula_mark};
use crate::ast::left_biased_child_at;
use crate::ast::support::{child_views, dest};
use crate::ast::{AstNode, ElementView, HasObjects, SplitPoint, split_span};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A table: its rows, then any directly following `#+TBLFM:` lines.
    Table,
    OrgSyntaxKind::TABLE
);
crate::ast_node!(
    /// A table row: a standard `| ... |` row or a `|---+---|` rule row.
    TableRow,
    [OrgSyntaxKind::TABLE_ROW, OrgSyntaxKind::TABLE_RULE_ROW]
);
crate::ast_node!(
    /// A table cell, with its closing pipe; it holds objects.
    TableCell,
    OrgSyntaxKind::TABLE_CELL
);

impl HasObjects for TableCell {}

/// Which table syntax a [`Table`] uses.
///
/// Only Org pipe tables are parsed. The enum is non-exhaustive so that
/// table.el tables can be added later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TableKind {
    /// Org pipe table.
    Org,
}

/// 0-based grid address: `row` indexes [`Table::rows`], `col` indexes
/// [`TableRow::cells`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellAddr {
    row: usize,
    col: usize,
}

impl CellAddr {
    /// Address of the cell at `row`, `col`.
    #[must_use]
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }

    /// 0-based row among all rows, including rule rows.
    #[must_use]
    pub const fn row(self) -> usize {
        self.row
    }

    /// 0-based cell among [`TableRow::cells`] of that row.
    #[must_use]
    pub const fn col(self) -> usize {
        self.col
    }
}

/// Kind of a leading in-field table formula mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellFormulaKind {
    /// `=`: this field only.
    Field,
    /// `:=`: the whole column.
    Column,
}

/// The leading `=` or `:=` that makes a cell's text a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellFormula {
    kind: CellFormulaKind,
    range: TextRange,
}

impl From<TableCellFormulaMark> for CellFormula {
    fn from(mark: TableCellFormulaMark) -> Self {
        let kind = match mark {
            TableCellFormulaMark::Field(_) => CellFormulaKind::Field,
            TableCellFormulaMark::Column(_) => CellFormulaKind::Column,
        };
        Self::new(kind, mark.range())
    }
}

impl CellFormula {
    pub(crate) const fn new(kind: CellFormulaKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// Field or column mark.
    #[must_use]
    pub const fn kind(self) -> CellFormulaKind {
        self.kind
    }

    /// Span of the `=` or `:=` bytes, not the rest of the formula.
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }
}

impl Table {
    /// Always [`TableKind::Org`]; this crate does not parse table.el `+-`
    /// grids.
    #[must_use]
    pub fn kind(&self) -> TableKind {
        TableKind::Org
    }

    /// Data and rule rows in source order; `#+TBLFM:` lines are not rows.
    pub fn rows(&self) -> impl Iterator<Item = TableRow> + '_ {
        child_views(self.syntax())
    }

    /// Data rows, without rule rows.
    pub fn data_rows(&self) -> impl Iterator<Item = TableRow> + '_ {
        self.rows().filter(|r| !r.is_rule())
    }

    /// Rule rows, each with its 1-based ordinal as used in formula
    /// references (`@I`, `@II`, ...).
    pub fn rule_rows(&self) -> impl Iterator<Item = (usize, TableRow)> + '_ {
        self.rows()
            .filter(TableRow::is_rule)
            .enumerate()
            .map(|(i, r)| (i + 1, r))
    }

    /// Cell at `index` in each data row that has one. Rule rows and short
    /// rows are skipped.
    pub fn column(&self, index: usize) -> impl Iterator<Item = TableCell> + '_ {
        self.data_rows()
            .filter_map(move |row| row.cells().nth(index))
    }

    /// Cell at a structural address. `None` on a rule row or a missing
    /// (ragged) cell.
    #[must_use]
    pub fn cell(&self, at: CellAddr) -> Option<TableCell> {
        self.rows().nth(at.row())?.cells().nth(at.col())
    }

    /// The cell containing `offset`, if any.
    ///
    /// Where two cells of a row meet, the left one wins, as in
    /// [`crate::ast::HasElements::element_at`]. Cells of different rows never
    /// share a boundary.
    #[must_use]
    pub fn cell_at(&self, offset: TextSize) -> Option<TableCell> {
        for r in self.rows() {
            if let Some(cell) = left_biased_child_at(r.cells(), offset) {
                return Some(cell);
            }
        }
        None
    }

    /// `(row count including rules, max cells on any data row)`.
    #[must_use]
    pub fn dimensions(&self) -> (usize, usize) {
        let mut n_rows = 0;
        let mut n_cols = 0;
        for r in self.rows() {
            n_rows += 1;
            n_cols = n_cols.max(r.cells().count());
        }
        (n_rows, n_cols)
    }

    /// The `#+TBLFM:` lines directly after the table.
    ///
    /// They belong to the table, so [`crate::ast::Element::cast`] refuses
    /// them.
    pub fn formulas(&self) -> impl Iterator<Item = Keyword> + '_ {
        child_views(self.syntax())
    }
}

impl TableRow {
    /// The table this row belongs to.
    ///
    /// # Panics
    ///
    /// Panics if the row's parent is not `TABLE`, which the parser never
    /// produces.
    #[must_use]
    pub fn table(&self) -> Table {
        self.syntax()
            .parent()
            .and_then(Table::cast)
            .expect("grammar: TABLE_ROW is always a child of TABLE (forms/table.rs::parse)")
    }

    /// Whether this is a rule row (`|---+---|`), which has no cells.
    #[must_use]
    pub fn is_rule(&self) -> bool {
        self.syntax().kind() == OrgSyntaxKind::TABLE_RULE_ROW
    }

    /// The cells, in order. Empty for a rule row and for a data row that is
    /// only `|`.
    pub fn cells(&self) -> impl Iterator<Item = TableCell> + '_ {
        child_views(self.syntax())
    }

    /// 0-based index among [`Table::rows`].
    ///
    /// # Panics
    ///
    /// Panics if the row's parent is not `TABLE`, which the parser never
    /// produces.
    #[must_use]
    pub fn index(&self) -> usize {
        self.table()
            .rows()
            .position(|r| r.syntax() == self.syntax())
            .expect("grammar: TABLE_ROW is a child of TABLE (forms/table.rs::parse)")
    }
}

impl TableCell {
    /// The row this cell belongs to.
    ///
    /// # Panics
    ///
    /// Panics if the cell's parent is not `TABLE_ROW`, which the parser never
    /// produces.
    #[must_use]
    pub fn row(&self) -> TableRow {
        self.syntax().parent().and_then(TableRow::cast).expect(
            "grammar: TABLE_CELL is always a child of TABLE_ROW (forms/table.rs::parse_row)",
        )
    }

    /// Structural address of this cell.
    ///
    /// # Panics
    ///
    /// Panics if the cell is not in a `TABLE_ROW` inside a `TABLE`, which the
    /// parser never produces.
    #[must_use]
    pub fn address(&self) -> CellAddr {
        let row = self.row();
        let col = row
            .cells()
            .position(|c| c.syntax() == self.syntax())
            .expect("grammar: TABLE_CELL is a child of TABLE_ROW (forms/table.rs::parse_row)");
        CellAddr::new(row.index(), col)
    }

    /// The cell's text, without the pipes and the padding spaces.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        table_cell_content(self.syntax())
    }

    /// Cut the cell's text at `offset`, moving the rest to a new row after
    /// this one.
    ///
    /// `None` when `offset` is outside the content's half-open `[start, end)`.
    #[must_use]
    pub fn split_at(&self, offset: TextSize) -> Option<SplitPoint> {
        split_span(
            self.content_range(),
            offset,
            dest(self.syntax(), self.row().range().end()),
        )
    }

    /// The leading `=` or `:=` of a formula cell. `None` otherwise, including
    /// when `:=` appears later in the text.
    #[must_use]
    pub fn formula_mark(&self) -> Option<CellFormula> {
        table_cell_formula_mark(self.syntax()).map(CellFormula::from)
    }
}

impl ElementView for Table {
    fn content_range(&self) -> Option<TextRange> {
        Some(row_run(self.syntax()))
    }

    fn is_greater(&self) -> bool {
        true
    }
}

fn row_run(table: &crate::SyntaxNode) -> TextRange {
    let mut rows = child_views::<TableRow>(table);
    let Some(first) = rows.next() else {
        return TextRange::empty(table.text_range().start());
    };
    let end = rows
        .last()
        .map_or_else(|| first.range().end(), |last| last.range().end());
    TextRange::new(first.range().start(), end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_token_of_kind;
    use crate::ast::{AstNode, Element, ElementView, Padding};
    use crate::parse;
    use crate::tests::{nodes_of_kind, source_slice};

    fn first_table(src: &str) -> (crate::Document, Table) {
        let doc = parse(src);
        let t = Table::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::TABLE)
                .into_iter()
                .next()
                .expect("table"),
        )
        .expect("cast");
        (doc, t)
    }

    #[test]
    fn content_range_is_row_run_excluding_tblfm() {
        let src = "| a |\n#+TBLFM: $1=$1\n";
        let (doc, t) = first_table(src);
        assert!(t.is_greater());
        assert_eq!(t.kind(), TableKind::Org);
        let content = t.content_range().unwrap();
        assert_eq!(source_slice(&doc, content), "| a |\n");
        assert!(t.range().end() > content.end());
    }

    #[test]
    fn data_row_vs_rule_row_rule_has_no_cells() {
        let src = "| a | b |\n|---+---|\n| c | d |\n";
        let (_, t) = first_table(src);
        let rows: Vec<_> = t.rows().collect();
        assert_eq!(rows.len(), 3);
        assert!(!rows[0].is_rule());
        assert!(rows[1].is_rule());
        assert!(!rows[2].is_rule());
        assert_eq!(rows[0].cells().count(), 2);
        assert_eq!(rows[1].cells().count(), 0);
        assert_eq!(rows[2].cells().count(), 2);
        assert_eq!(t.data_rows().count(), 2);
        let rules: Vec<_> = t.rule_rows().collect();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].0, 1);
        assert_eq!(rules[0].1.index(), 1);
        assert_eq!(rows[0].index(), 0);
        assert_eq!(rows[2].index(), 2);

        let (_, t) = first_table("|---|\n| a |\n|---|\n");
        let rules: Vec<_> = t.rule_rows().collect();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].0, 1);
        assert_eq!(rules[0].1.index(), 0);
        assert_eq!(rules[1].0, 2);
        assert_eq!(rules[1].1.index(), 2);
        assert_eq!(t.data_rows().next().unwrap().index(), 1);
    }

    #[test]
    fn closing_pipe_on_cell_leading_pipe_on_row() {
        let src = "| a | b |\n";
        let (doc, t) = first_table(src);
        let row = t.rows().next().expect("row");
        let pipe = first_token_of_kind(row.syntax(), OrgSyntaxKind::PIPE).expect("leading pipe");
        assert_eq!(source_slice(&doc, pipe.text_range()), "|");
        assert_eq!(pipe.text_range().start(), row.range().start());
        let cells: Vec<_> = row.cells().collect();
        assert_eq!(cells.len(), 2);
        for cell in &cells {
            let carved = source_slice(&doc, cell.range());
            assert!(
                carved.ends_with('|'),
                "closing pipe belongs to the cell: {carved:?}"
            );
            assert!(
                !carved.starts_with('|'),
                "leading pipe belongs to the row: {carved:?}"
            );
            assert!(!source_slice(&doc, cell.content_range()).contains('|'));
            assert!(cell.range().start() > pipe.text_range().start());
        }
        assert_eq!(source_slice(&doc, cells[0].range()), " a |");
        assert_eq!(source_slice(&doc, cells[1].range()), " b |");
    }

    #[test]
    fn indented_row_still_owns_the_leading_pipe() {
        let src = "  | a |\n";
        let (doc, t) = first_table(src);
        let row = t.rows().next().expect("row");
        let pipe = first_token_of_kind(row.syntax(), OrgSyntaxKind::PIPE).expect("pipe");
        let cell = row.cells().next().expect("cell");
        assert!(pipe.text_range().start() > row.range().start());
        assert!(cell.range().start() > pipe.text_range().start());
        assert_eq!(source_slice(&doc, pipe.text_range()), "|");
        assert!(!source_slice(&doc, cell.range()).starts_with('|'));
    }

    #[test]
    fn ragged_rows_column_skips_missing_and_rules() {
        let src = "| a | b | c |\n| d |\n|---+---|\n| e | f |\n";
        let (doc, t) = first_table(src);
        let col0: Vec<_> = t
            .column(0)
            .map(|c| source_slice(&doc, c.content_range()).to_string())
            .collect();
        let col1: Vec<_> = t
            .column(1)
            .map(|c| source_slice(&doc, c.content_range()).to_string())
            .collect();
        let col2: Vec<_> = t
            .column(2)
            .map(|c| source_slice(&doc, c.content_range()).to_string())
            .collect();
        assert_eq!(col0, ["a", "d", "e"]);
        assert_eq!(col1, ["b", "f"]);
        assert_eq!(col2, ["c"]);
        assert_eq!(t.column(5).count(), 0);
        assert!(t.cell(CellAddr::new(1, 1)).is_none());
        assert!(t.cell(CellAddr::new(2, 0)).is_none());
        let below = t.cell(CellAddr::new(3, 1)).unwrap();
        assert_eq!(source_slice(&doc, below.content_range()), "f");
        assert_eq!(below.address(), CellAddr::new(3, 1));
        assert_eq!(below.address().row(), below.row().index());
    }

    #[test]
    fn table_cell_objects() {
        use crate::ast::{HasObjects, Object};
        let (_, t) = first_table("| *b* |\n");
        let cell = t
            .data_rows()
            .next()
            .and_then(|r| r.cells().next())
            .expect("cell");
        let objs: Vec<_> = cell.objects().collect();
        assert_eq!(objs.len(), 1);
        assert!(matches!(objs[0], Object::Markup(_)));
    }

    #[test]
    fn cell_addr_and_cell_at_left_bias() {
        let src = "| a | b |\n";
        let (doc, t) = first_table(src);
        let a = t.cell(CellAddr::new(0, 0)).expect("a");
        let b = t.cell(CellAddr::new(0, 1)).expect("b");
        assert_eq!(a.address(), CellAddr::new(0, 0));
        assert_eq!(b.address(), CellAddr::new(0, 1));
        assert_eq!(source_slice(&doc, a.content_range()), "a");
        assert_eq!(source_slice(&doc, b.content_range()), "b");

        assert_eq!(
            t.cell_at(a.range().start()).as_ref().map(AstNode::syntax),
            Some(a.syntax())
        );
        let seam = a.range().end();
        assert_eq!(seam, b.range().start());
        assert_eq!(
            t.cell_at(seam).as_ref().map(AstNode::syntax),
            Some(a.syntax()),
            "left-biased at the shared pipe boundary"
        );
        let interior = b.content_range().start();
        assert_eq!(
            t.cell_at(interior).as_ref().map(AstNode::syntax),
            Some(b.syntax())
        );
        assert_eq!(
            t.cell_at(b.range().end()).as_ref().map(AstNode::syntax),
            Some(b.syntax())
        );
        let pipe = first_token_of_kind(t.rows().next().unwrap().syntax(), OrgSyntaxKind::PIPE)
            .expect("leading pipe");
        assert!(
            t.cell_at(pipe.text_range().start()).is_none(),
            "leading pipe is row-owned, not a cell"
        );
    }

    #[test]
    fn dimensions_include_rule_rows() {
        let src = "| a | b | c |\n|---+---|\n| d |\n";
        let (_, t) = first_table(src);
        assert_eq!(t.dimensions(), (3, 3));
        let (_, t) = first_table("|---|\n");
        assert_eq!(t.dimensions(), (1, 0));
        assert!(t.data_rows().next().is_none());
        assert!(t.cell(CellAddr::new(0, 0)).is_none());

        let (_, t) = first_table("|\n");
        let row = t.rows().next().expect("pipe-only data row");
        assert!(!row.is_rule());
        assert_eq!(row.cells().count(), 0);
        assert_eq!(t.data_rows().count(), 1);
        assert_eq!(t.rule_rows().count(), 0);
        assert!(t.cell(CellAddr::new(0, 0)).is_none());
        assert_eq!(t.dimensions(), (1, 0));
    }

    #[test]
    fn formulas_consecutive_tblfm_hidden_from_elements() {
        let src = "| a |\n#+TBLFM: $1=$1\n#+tblfm: $2=$1\n";
        let (doc, t) = first_table(src);
        let fml: Vec<_> = t.formulas().collect();
        assert_eq!(fml.len(), 2);
        for kw in &fml {
            assert!(Keyword::cast(kw.syntax().clone()).is_some());
            assert!(
                Element::cast(kw.syntax().clone()).is_none(),
                "TBLFM is a table child, not an element"
            );
        }
        let kinds: Vec<_> = doc.elements().map(|e| e.kind()).collect();
        assert!(kinds.contains(&OrgSyntaxKind::TABLE));
        assert!(
            !kinds.contains(&OrgSyntaxKind::KEYWORD),
            "Document::elements hides TBLFM"
        );
    }

    #[test]
    fn blank_separated_formula_is_a_sibling_element() {
        let src = "| a |\n\n#+TBLFM: $1=2\n";
        let (doc, t) = first_table(src);
        assert_eq!(t.formulas().count(), 0);
        let els: Vec<_> = doc.elements().collect();
        assert!(els.iter().any(|e| matches!(e, Element::Table(_))));
        assert!(
            els.iter().any(|e| matches!(e, Element::Keyword(_))),
            "blank line ends the TBLFM run; the keyword is a sibling element"
        );
    }

    #[test]
    fn split_at_inside_cell_content_vs_outside() {
        let src = "| hello world |\n";
        let (doc, t) = first_table(src);
        let cell = t.cell(CellAddr::new(0, 0)).expect("cell");
        let content = cell.content_range();
        assert_eq!(source_slice(&doc, content), "hello world");
        let offset = content.start() + TextSize::from(6);
        let sp = cell.split_at(offset).expect("inside");
        assert_eq!(source_slice(&doc, sp.keep()), "hello ");
        assert_eq!(source_slice(&doc, sp.moved()), "world");
        assert_eq!(sp.dest().offset(), cell.row().range().end());

        let sp = cell.split_at(content.start()).expect("start");
        assert!(sp.keep().is_empty());
        assert_eq!(source_slice(&doc, sp.moved()), "hello world");

        assert!(cell.split_at(content.end()).is_none());
        assert!(cell.split_at(cell.range().end()).is_none());
        let pipe = first_token_of_kind(cell.row().syntax(), OrgSyntaxKind::PIPE).unwrap();
        assert!(cell.split_at(pipe.text_range().start()).is_none());

        let (doc, t) = first_table("| hello world |\n| next |\n");
        let cell = t.cell(CellAddr::new(0, 0)).expect("cell");
        let offset = cell.content_range().start() + TextSize::from(6);
        let sp = cell.split_at(offset).expect("inside");
        assert_eq!(sp.dest().offset(), cell.row().range().end());
        assert!(sp.dest().offset() < t.range().end());
        assert_eq!(sp.dest().lead(), Padding::None);
        assert_eq!(source_slice(&doc, sp.keep()), "hello ");

        let (_, t) = first_table("| hello world |\n#+TBLFM: $1=$1\n");
        let cell = t.cell(CellAddr::new(0, 0)).expect("cell");
        let offset = cell.content_range().start() + TextSize::from(6);
        let sp = cell.split_at(offset).expect("inside");
        assert_eq!(sp.dest().offset(), cell.row().range().end());
        assert!(sp.dest().offset() < t.range().end());
        assert_eq!(sp.dest().lead(), Padding::None);
    }

    #[test]
    fn formula_mark_delegates_to_layout() {
        let (doc, t) = first_table("| =$1+$2 |\n");
        let cell = t.cell(CellAddr::new(0, 0)).unwrap();
        let mark = cell.formula_mark().expect("field");
        assert_eq!(mark.kind(), CellFormulaKind::Field);
        assert_eq!(source_slice(&doc, mark.range()), "=");

        let (doc, t) = first_table("| :=$1 |\n");
        let cell = t.cell(CellAddr::new(0, 0)).unwrap();
        let mark = cell.formula_mark().expect("column");
        assert_eq!(mark.kind(), CellFormulaKind::Column);
        assert_eq!(source_slice(&doc, mark.range()), ":=");

        let src = "| =foo= |\n";
        let (doc, t) = first_table(src);
        let cell = t.cell(CellAddr::new(0, 0)).unwrap();
        assert!(
            cell.syntax()
                .children()
                .any(|c| c.kind() == OrgSyntaxKind::VERBATIM),
            "fixture must be verbatim markup, not a formula: {}",
            source_slice(&doc, cell.range())
        );
        assert!(cell.formula_mark().is_none());
    }

    #[test]
    fn cell_swap_and_rectangle_are_content_spans() {
        let src = "| a | b |\n|---+---|\n| c | d |\n| e | f |\n";
        let (doc, t) = first_table(src);
        let left = t.cell(CellAddr::new(0, 0)).unwrap();
        let right = t.cell(CellAddr::new(2, 1)).unwrap();
        assert_eq!(right.address(), CellAddr::new(2, 1));
        assert_eq!(right.address().row(), right.row().index());
        assert_eq!(source_slice(&doc, left.content_range()), "a");
        assert_eq!(source_slice(&doc, right.content_range()), "d");
        assert!(!source_slice(&doc, left.content_range()).contains('|'));
        assert!(!source_slice(&doc, right.content_range()).contains('|'));

        // Rectangle covering the two data rows below the hline, first two cols.
        let mut rect = Vec::new();
        for row in t.data_rows() {
            let i = row.index();
            if i < 2 {
                continue;
            }
            for col in 0..2 {
                if let Some(cell) = t.cell(CellAddr::new(i, col)) {
                    rect.push(source_slice(&doc, cell.content_range()).to_string());
                }
            }
        }
        assert_eq!(rect, ["c", "d", "e", "f"]);
        assert!(
            t.rule_rows()
                .all(|(_, r)| t.cell_at(r.range().start()).is_none()),
            "hline offsets are not cells"
        );
    }

    #[test]
    fn whitespace_perturbed_cell_interiors() {
        let src = "|  alpha  |  beta |\n";
        let (doc, t) = first_table(src);
        let cells: Vec<_> = t.rows().next().unwrap().cells().collect();
        assert_eq!(source_slice(&doc, cells[0].content_range()), "alpha");
        assert_eq!(source_slice(&doc, cells[1].content_range()), "beta");
        assert_eq!(source_slice(&doc, cells[0].range()), "  alpha  |");

        let src = "| a | b\n";
        let (doc, t) = first_table(src);
        let cells: Vec<_> = t.rows().next().unwrap().cells().collect();
        assert_eq!(source_slice(&doc, cells[1].content_range()), "b");
        assert!(
            !source_slice(&doc, cells[1].range()).ends_with('|'),
            "missing trailing pipe stays off the last cell"
        );

        let src = "|   |\n";
        let (_, t) = first_table(src);
        let cell = t.cell(CellAddr::new(0, 0)).unwrap();
        assert!(cell.content_range().is_empty());
        assert!(cell.split_at(cell.content_range().start()).is_none());
        assert!(cell.formula_mark().is_none());
    }
}
