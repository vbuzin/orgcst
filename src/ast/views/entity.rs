//! Entity objects such as `\alpha` or `\alpha{}`.
//!
//! The parser checked the name against the entity table; the view reads it
//! back from the node without a second lookup.

use crate::ast::AstNode;
use crate::ast::layout::with_node_text;
use crate::entities::entity_form;
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// An entity object, such as `\alpha` or `\alpha{}`.
    Entity,
    OrgSyntaxKind::ENTITY
);

impl Entity {
    /// Name without the leading backslash.
    #[must_use]
    pub fn name(&self) -> String {
        self.form_bits().0
    }

    /// Whether the written form used a `{}` terminator.
    #[must_use]
    pub fn use_brackets(&self) -> bool {
        self.form_bits().1
    }

    fn form_bits(&self) -> (String, bool) {
        with_node_text(self.syntax(), |t| {
            let f = entity_form(t)
                .expect("grammar: ENTITY is \\name / \\name{} / \\_ (forms/entity.rs)");
            (f.name.to_string(), f.use_brackets)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;

    #[test]
    fn entity_name_and_brackets() {
        let doc = parse("\\alpha\n");
        let e = Entity::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ENTITY)
                .into_iter()
                .next()
                .expect("entity"),
        )
        .expect("cast");
        assert_eq!(e.name(), "alpha");
        assert!(!e.use_brackets());

        let doc = parse("\\alpha{}\n");
        let e = Entity::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ENTITY)
                .into_iter()
                .next()
                .expect("entity"),
        )
        .expect("cast");
        assert_eq!(e.name(), "alpha");
        assert!(e.use_brackets());
    }
}
