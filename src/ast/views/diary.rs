//! Diary sexp. The same kind under `TIMESTAMP` is not an element.

use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A diary sexp: the `%%(...)` element, or the sexp inside a
    /// `<%%(...)>` timestamp.
    DiarySexp,
    OrgSyntaxKind::DIARY_SEXP
);
crate::ast::impl_leaf_element_view!(DiarySexp);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Element, ElementView};
    use crate::parse;
    use crate::tests::{nodes_of_kind, source_slice};

    #[test]
    fn diary_sexp_cast_is_kind_only_element_cast_is_parent_aware() {
        let src = "%%(diary-float t 4 2)\n";
        let doc = parse(src);
        let d = DiarySexp::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::DIARY_SEXP)
                .into_iter()
                .next()
                .expect("diary"),
        )
        .expect("view is kind-only");
        assert!(d.content_range().is_none());
        assert!(!d.is_greater());
        assert!(Element::cast(d.syntax().clone()).is_some());
        assert_eq!(source_slice(&doc, d.range()), src);
        assert!(source_slice(&doc, d.range()).starts_with("%%"));

        let src = "<%%(diary-float t 4 2)>\n";
        let doc = parse(src);
        let d = DiarySexp::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::DIARY_SEXP)
                .into_iter()
                .next()
                .expect("stamp child"),
        )
        .expect("view is kind-only");
        assert!(d.content_range().is_none());
        assert!(!d.is_greater());
        assert!(
            d.syntax()
                .parent()
                .is_some_and(|p| p.kind() == OrgSyntaxKind::TIMESTAMP)
        );
        assert!(Element::cast(d.syntax().clone()).is_none());
        assert!(Element::can_cast(OrgSyntaxKind::DIARY_SEXP));
        assert_eq!(source_slice(&doc, d.range()), "(diary-float t 4 2)");
        assert!(!source_slice(&doc, d.range()).contains('%'));
        assert!(!source_slice(&doc, d.range()).contains('<'));
        assert!(!source_slice(&doc, d.range()).contains('>'));
    }
}
