//! Target and radio-target objects: `<<name>>` and `<<<name>>>`.
//!
//! Plain-text occurrences of a radio target are not parsed as `LINK` nodes;
//! finding them is a query over the text.

use crate::ast::layout::{angle_run_interior, text_in};
use crate::ast::{AstNode, HasObjects};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A target object: `<<name>>`.
    Target,
    OrgSyntaxKind::TARGET
);
crate::ast_node!(
    /// A radio target object: `<<<name>>>`.
    RadioTarget,
    OrgSyntaxKind::RADIO_TARGET
);

impl Target {
    /// Interior of `<<name>>`.
    #[must_use]
    pub fn name(&self) -> String {
        text_in(self.syntax(), interior(self.syntax(), 2))
    }
}

impl RadioTarget {
    /// Source text between `<<<` and `>>>`, markup included: the text that
    /// occurrences must match.
    #[must_use]
    pub fn key(&self) -> String {
        text_in(self.syntax(), interior(self.syntax(), 3))
    }
}

impl HasObjects for RadioTarget {}

fn interior(node: &crate::syntax::SyntaxNode, n: usize) -> crate::TextRange {
    angle_run_interior(node, n)
        .expect("grammar: TARGET / RADIO_TARGET is an angle run (forms/target.rs)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::HasObjects;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;

    #[test]
    fn target_name_and_radio_key() {
        let doc = parse("<<name>>\n");
        let t = Target::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::TARGET)
                .into_iter()
                .next()
                .expect("target"),
        )
        .expect("cast");
        assert_eq!(t.name(), "name");

        let doc = parse("<<<foo *bar*>>>\n");
        let r = RadioTarget::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::RADIO_TARGET)
                .into_iter()
                .next()
                .expect("radio"),
        )
        .expect("cast");
        assert_eq!(r.key(), "foo *bar*");
        let objs: Vec<_> = r.objects().collect();
        assert_eq!(objs.len(), 1);
    }
}
