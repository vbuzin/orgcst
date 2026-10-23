//! Fixed-width element: a folded run of `: ` lines.
//!
//! `value()` is verbatim after `: `; `ElementView::content_range()` stays
//! `None`.

use crate::ast::AstNode;
use crate::ast::layout::marker_stripped_value;
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A fixed-width element: a run of consecutive `: ` lines.
    FixedWidth,
    OrgSyntaxKind::FIXED_WIDTH
);
crate::ast::impl_leaf_element_view!(FixedWidth);

impl FixedWidth {
    /// Text verbatim after `: ` on each line.
    #[must_use]
    pub fn value(&self) -> String {
        marker_stripped_value(self.syntax(), OrgSyntaxKind::COLON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ElementView;
    use crate::parse;
    use crate::tests::nodes_of_kind;

    fn fixed_width(src: &str) -> FixedWidth {
        FixedWidth::cast(
            nodes_of_kind(parse(src).root(), OrgSyntaxKind::FIXED_WIDTH)
                .into_iter()
                .next()
                .expect("fixed-width"),
        )
        .expect("cast")
    }

    #[test]
    fn fixed_width_is_a_leaf() {
        let f = fixed_width(": verbatim\n");
        assert!(f.content_range().is_none());
        assert!(!f.is_greater());
    }

    #[test]
    fn value_strips_marker_and_one_space() {
        assert_eq!(fixed_width(": hello\n").value(), "hello");
        assert_eq!(fixed_width(": hello").value(), "hello");
        assert_eq!(fixed_width(":  two spaces\n").value(), " two spaces");
        assert_eq!(fixed_width(":\n").value(), "");
        assert_eq!(fixed_width(": \n").value(), "");
        assert_eq!(fixed_width(": key: value\n").value(), "key: value");
        assert!(fixed_width(": hello\n").content_range().is_none());
    }

    #[test]
    fn value_joins_folded_lines_and_drops_owned_blank() {
        assert_eq!(fixed_width(": a\n: b\n").value(), "a\nb");
        assert_eq!(fixed_width(": a\n: b").value(), "a\nb");
        assert_eq!(fixed_width(": a\n: b\n\n").value(), "a\nb");
        assert_eq!(fixed_width(": a\n:\n: c\n").value(), "a\n\nc");
    }

    #[test]
    fn value_skips_indent_and_keeps_body_spaces() {
        assert_eq!(fixed_width("  : hello\n").value(), "hello");
        assert_eq!(fixed_width("\t: hello\n").value(), "hello");
        assert_eq!(
            fixed_width(": first\n  : second\n").value(),
            "first\nsecond"
        );
        assert_eq!(fixed_width(": hello  \n").value(), "hello  ");
    }
}
