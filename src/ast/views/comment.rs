//! Comment element: a folded run of `#` lines.
//!
//! `value()` strips markers per line; `ElementView::content_range()` stays
//! `None`.

use crate::ast::AstNode;
use crate::ast::layout::marker_stripped_value;
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A comment element: a run of consecutive `#` comment lines.
    Comment,
    OrgSyntaxKind::COMMENT
);
crate::ast::impl_leaf_element_view!(Comment);

impl Comment {
    /// Text with `# ` markers stripped per line.
    #[must_use]
    pub fn value(&self) -> String {
        marker_stripped_value(self.syntax(), OrgSyntaxKind::HASH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ElementView;
    use crate::parse;
    use crate::tests::nodes_of_kind;

    fn comment(src: &str) -> Comment {
        Comment::cast(
            nodes_of_kind(parse(src).root(), OrgSyntaxKind::COMMENT)
                .into_iter()
                .next()
                .expect("comment"),
        )
        .expect("cast")
    }

    #[test]
    fn comment_is_a_leaf() {
        let c = comment("# a comment\n");
        assert!(c.content_range().is_none());
        assert!(!c.is_greater());
    }

    #[test]
    fn value_strips_marker_and_one_space() {
        assert_eq!(comment("# hello\n").value(), "hello");
        assert_eq!(comment("# hello").value(), "hello");
        assert_eq!(comment("#  two spaces\n").value(), " two spaces");
        assert_eq!(comment("#\n").value(), "");
        assert_eq!(comment("# \n").value(), "");
        assert_eq!(comment("# see #tag\n").value(), "see #tag");
        assert!(comment("# hello\n").content_range().is_none());
    }

    #[test]
    fn value_joins_folded_lines_and_drops_owned_blank() {
        assert_eq!(comment("# a\n# b\n").value(), "a\nb");
        assert_eq!(comment("# a\n# b").value(), "a\nb");
        assert_eq!(comment("# a\n# b\n\n").value(), "a\nb");
        assert_eq!(comment("# a\n#\n# c\n").value(), "a\n\nc");
    }

    #[test]
    fn value_skips_indent_and_keeps_body_spaces() {
        assert_eq!(comment("  # hello\n").value(), "hello");
        assert_eq!(comment("\t# hello\n").value(), "hello");
        assert_eq!(comment("# first\n  # second\n").value(), "first\nsecond");
        assert_eq!(comment("# hello  \n").value(), "hello  ");
        assert_eq!(comment("# \\alpha\n").value(), "\\alpha");
    }
}
