//! Line break objects: `\\`, trailing spaces, and the terminator.

use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A line break object: `\\` at the end of a line, with any trailing
    /// spaces and the line terminator.
    LineBreak,
    OrgSyntaxKind::LINE_BREAK
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AstNode;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, source_slice};

    #[test]
    fn line_break_owns_terminator() {
        let doc = parse("hello\\\\\nnext\n");
        let lb = LineBreak::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LINE_BREAK)
                .into_iter()
                .next()
                .expect("break"),
        )
        .expect("cast");
        let text = source_slice(&doc, lb.range());
        assert!(text.starts_with("\\\\"));
        assert!(text.ends_with('\n'), "terminator is inside the object");
    }

    #[test]
    fn line_break_in_heading_title_is_not_carved() {
        let doc = parse("* hello\\\\\n");
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LINE_BREAK).is_empty(),
            "STANDARD_NO_LINE_BREAK on HEADING_TITLE"
        );
    }
}
