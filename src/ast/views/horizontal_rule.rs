//! Horizontal rule. Consecutive lines are never merged.

use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A horizontal rule: a line of five or more hyphens.
    HorizontalRule,
    OrgSyntaxKind::HORIZONTAL_RULE
);
crate::ast::impl_leaf_element_view!(HorizontalRule);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, ElementView};
    use crate::parse;
    use crate::tests::nodes_of_kind;

    #[test]
    fn horizontal_rule_is_a_leaf() {
        let doc = parse("-----\n");
        let h = HorizontalRule::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HORIZONTAL_RULE)
                .into_iter()
                .next()
                .expect("rule"),
        )
        .expect("cast");
        assert!(h.content_range().is_none());
        assert!(!h.is_greater());
    }

    #[test]
    fn consecutive_rules_are_separate_nodes() {
        let doc = parse("-----\n-----\n");
        let rules = nodes_of_kind(doc.root(), OrgSyntaxKind::HORIZONTAL_RULE);
        assert_eq!(rules.len(), 2, "consecutive rules are never merged");
        for node in rules {
            let h = HorizontalRule::cast(node).expect("cast");
            assert!(h.content_range().is_none());
            assert!(!h.is_greater());
        }
    }
}
