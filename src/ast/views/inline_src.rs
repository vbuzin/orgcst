//! Inline source-block objects: `src_LANG{BODY}` / `src_LANG[HEADERS]{BODY}`.

use crate::ast::layout::{delimited_interior, with_node_text};
use crate::ast::support::child_of_kind;
use crate::ast::{AstNode, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// An inline source block object: `src_LANG{BODY}` or
    /// `src_LANG[HEADERS]{BODY}`.
    InlineSrcBlock,
    OrgSyntaxKind::INLINE_SRC_BLOCK
);

/// Parts of an inline source block. Content spans exclude the delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineSrcPart {
    /// `LANG` after `src_`.
    Language,
    /// `[HEADERS]`; optional.
    Parameters,
    /// `{BODY}`.
    Body,
}

impl HasParts for InlineSrcBlock {
    type Part = InlineSrcPart;
    const PARTS: &'static [InlineSrcPart] = &[
        InlineSrcPart::Language,
        InlineSrcPart::Parameters,
        InlineSrcPart::Body,
    ];

    fn part_span(&self, part: InlineSrcPart) -> PartSpan {
        match part {
            InlineSrcPart::Language => {
                let node = lang_node(self.syntax());
                PartSpan::Present(PresentPart::identical(node.text_range()))
            }
            InlineSrcPart::Parameters => {
                match child_of_kind(self.syntax(), OrgSyntaxKind::SRC_PARAMETERS) {
                    Some(node) => {
                        let carved = node.text_range();
                        let content = delimited_interior(
                            &node,
                            OrgSyntaxKind::L_BRACKET,
                            OrgSyntaxKind::R_BRACKET,
                        )
                        .expect("grammar: SRC_PARAMETERS includes brackets (forms/inline_src.rs)");
                        PartSpan::Present(PresentPart::new(carved, content, carved))
                    }
                    None => PartSpan::Absent(InsertPoint::new(
                        lang_node(self.syntax()).text_range().end(),
                        Padding::None,
                        Padding::None,
                    )),
                }
            }
            InlineSrcPart::Body => {
                let node = child_of_kind(self.syntax(), OrgSyntaxKind::SRC_BODY)
                    .expect("grammar: INLINE_SRC_BLOCK always has SRC_BODY (forms/inline_src.rs)");
                let carved = node.text_range();
                let content =
                    delimited_interior(&node, OrgSyntaxKind::L_BRACE, OrgSyntaxKind::R_BRACE)
                        .expect("grammar: SRC_BODY includes braces (forms/inline_src.rs)");
                PartSpan::Present(PresentPart::new(carved, content, carved))
            }
        }
    }
}

impl InlineSrcBlock {
    /// Language name as written.
    #[must_use]
    pub fn language(&self) -> String {
        with_node_text(&lang_node(self.syntax()), str::to_string)
    }
}

fn lang_node(syntax: &SyntaxNode) -> SyntaxNode {
    child_of_kind(syntax, OrgSyntaxKind::SRC_LANGUAGE)
        .expect("grammar: INLINE_SRC_BLOCK always has SRC_LANGUAGE (forms/inline_src.rs)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::HasParts;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn src_block(src: &str) -> (crate::Document, InlineSrcBlock) {
        let doc = parse(src);
        let b = InlineSrcBlock::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_SRC_BLOCK)
                .into_iter()
                .next()
                .expect("inline src"),
        )
        .expect("cast");
        (doc, b)
    }

    #[test]
    fn language_params_body() {
        let (doc, b) = src_block("src_elisp{(+ 1 2)}\n");
        assert_eq!(b.language(), "elisp");
        assert!(b.part_insert_point(InlineSrcPart::Parameters).is_some());
        assert_eq!(
            source_slice(&doc, b.part_content_range(InlineSrcPart::Body).unwrap()),
            "(+ 1 2)"
        );
        crate::ast::check_part_contract(&b);

        let (_, b) = src_block("src_elisp[]{ }\n");
        assert!(
            b.part_content_range(InlineSrcPart::Parameters)
                .unwrap()
                .is_empty()
        );
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_SRC_BLOCK) {
                if let Some(b) = InlineSrcBlock::cast(n) {
                    crate::ast::check_part_contract(&b);
                }
            }
        }
    }
}
