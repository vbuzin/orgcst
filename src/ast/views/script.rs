//! Subscript and superscript objects.
//!
//! The node starts at the `_` or `^` marker, not at the base character. A
//! parenthesised body is not braced: in `a_(x)` the body is `(x)`.

use crate::ast::layout::{ScriptLayout, script_layout};
use crate::ast::{AstNode, HasObjects, HasParts, PartSpan, PresentPart};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A subscript object: `_x`, `_{x}` or `_(x)`, starting at the `_`.
    Subscript,
    OrgSyntaxKind::SUBSCRIPT
);
crate::ast_node!(
    /// A superscript object: `^x`, `^{x}` or `^(x)`, starting at the `^`.
    Superscript,
    OrgSyntaxKind::SUPERSCRIPT
);

/// Parts of a subscript or superscript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptPart {
    /// The `_` or `^` marker.
    Marker,
    /// The script text, without braces when braced.
    Body,
}

impl Subscript {
    /// Whether the body used `{...}` braces.
    #[must_use]
    pub fn is_braced(&self) -> bool {
        layout(self.syntax()).braced
    }
}

impl Superscript {
    /// Whether the body used `{...}` braces.
    #[must_use]
    pub fn is_braced(&self) -> bool {
        layout(self.syntax()).braced
    }
}

impl HasParts for Subscript {
    type Part = ScriptPart;
    const PARTS: &'static [ScriptPart] = &[ScriptPart::Marker, ScriptPart::Body];

    fn part_span(&self, part: ScriptPart) -> PartSpan {
        script_part(layout(self.syntax()), part)
    }
}

impl HasParts for Superscript {
    type Part = ScriptPart;
    const PARTS: &'static [ScriptPart] = &[ScriptPart::Marker, ScriptPart::Body];

    fn part_span(&self, part: ScriptPart) -> PartSpan {
        script_part(layout(self.syntax()), part)
    }
}

impl HasObjects for Subscript {}
impl HasObjects for Superscript {}

fn layout(node: &crate::syntax::SyntaxNode) -> ScriptLayout {
    script_layout(node)
        .expect("grammar: SUBSCRIPT / SUPERSCRIPT starts with a marker (forms/script.rs)")
}

fn script_part(lay: ScriptLayout, part: ScriptPart) -> PartSpan {
    match part {
        ScriptPart::Marker => PartSpan::Present(PresentPart::identical(lay.marker)),
        ScriptPart::Body => PartSpan::Present(PresentPart::identical(lay.body)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{HasObjects, HasParts};
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    #[test]
    fn braced_vs_paren_vs_bare() {
        let doc = parse("a_{*b*}\n");
        let s = Subscript::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::SUBSCRIPT)
                .into_iter()
                .next()
                .expect("sub"),
        )
        .expect("cast");
        assert!(s.is_braced());
        assert_eq!(
            source_slice(&doc, s.part_content_range(ScriptPart::Body).unwrap()),
            "*b*"
        );
        assert_eq!(s.objects().count(), 1);
        crate::ast::check_part_contract(&s);

        let doc = parse("a_(x)\n");
        let s = Subscript::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::SUBSCRIPT)
                .into_iter()
                .next()
                .expect("sub"),
        )
        .expect("cast");
        assert!(!s.is_braced());
        assert_eq!(
            source_slice(&doc, s.part_content_range(ScriptPart::Body).unwrap()),
            "(x)"
        );

        let doc = parse("a^b\n");
        let s = Superscript::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::SUPERSCRIPT)
                .into_iter()
                .next()
                .expect("sup"),
        )
        .expect("cast");
        assert!(!s.is_braced());
        assert_eq!(
            source_slice(&doc, s.part_content_range(ScriptPart::Body).unwrap()),
            "b"
        );
        crate::ast::check_part_contract(&s);
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::SUBSCRIPT) {
                if let Some(s) = Subscript::cast(n) {
                    crate::ast::check_part_contract(&s);
                }
            }
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::SUPERSCRIPT) {
                if let Some(s) = Superscript::cast(n) {
                    crate::ast::check_part_contract(&s);
                }
            }
        }
    }
}
