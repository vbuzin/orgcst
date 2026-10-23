//! Inline babel calls: `call_NAME[HEADER](ARGS)[HEADER]`.
//!
//! The parts mirror [`crate::ast::BabelCallPart`], except that the end header
//! is bracketed instead of running to the end of the line.

use crate::ast::layout::delimited_interior;
use crate::ast::support::child_of_kind;
use crate::ast::{AstNode, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// An inline babel call object: `call_NAME[HEADER](ARGS)[HEADER]`.
    InlineBabelCall,
    OrgSyntaxKind::INLINE_BABEL_CALL
);

/// Parts of an inline babel call. Content spans exclude the delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineBabelCallPart {
    /// `NAME` after `call_`.
    Name,
    /// `[HEADER]` before the arguments; optional.
    InsideHeader,
    /// `(ARGS)`.
    Arguments,
    /// `[HEADER]` after the arguments; optional.
    EndHeader,
}

impl HasParts for InlineBabelCall {
    type Part = InlineBabelCallPart;
    const PARTS: &'static [InlineBabelCallPart] = &[
        InlineBabelCallPart::Name,
        InlineBabelCallPart::InsideHeader,
        InlineBabelCallPart::Arguments,
        InlineBabelCallPart::EndHeader,
    ];

    fn part_span(&self, part: InlineBabelCallPart) -> PartSpan {
        let syntax = self.syntax();
        let name_end = call_name_node(syntax).text_range().end();
        match part {
            InlineBabelCallPart::Name => {
                let node = call_name_node(syntax);
                PartSpan::Present(PresentPart::identical(node.text_range()))
            }
            InlineBabelCallPart::InsideHeader => delimited_part(
                syntax,
                OrgSyntaxKind::CALL_INSIDE_HEADER,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET,
                name_end,
            ),
            InlineBabelCallPart::Arguments => delimited_part(
                syntax,
                OrgSyntaxKind::CALL_ARGUMENTS,
                OrgSyntaxKind::L_PAREN,
                OrgSyntaxKind::R_PAREN,
                child_of_kind(syntax, OrgSyntaxKind::CALL_INSIDE_HEADER)
                    .map_or(name_end, |n| n.text_range().end()),
            ),
            InlineBabelCallPart::EndHeader => delimited_part(
                syntax,
                OrgSyntaxKind::CALL_END_HEADER,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET,
                child_of_kind(syntax, OrgSyntaxKind::CALL_ARGUMENTS)
                    .or_else(|| child_of_kind(syntax, OrgSyntaxKind::CALL_INSIDE_HEADER))
                    .map_or(name_end, |n| n.text_range().end()),
            ),
        }
    }
}

fn delimited_part(
    syntax: &SyntaxNode,
    kind: OrgSyntaxKind,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
    insert: crate::TextSize,
) -> PartSpan {
    match child_of_kind(syntax, kind) {
        Some(node) => {
            let carved = node.text_range();
            let content = delimited_interior(&node, open, close)
                .expect("grammar: CALL_* paired nodes include their delimiters (forms/call.rs)");
            PartSpan::Present(PresentPart::new(carved, content, carved))
        }
        None => PartSpan::Absent(InsertPoint::new(insert, Padding::None, Padding::None)),
    }
}

fn call_name_node(call: &SyntaxNode) -> SyntaxNode {
    child_of_kind(call, OrgSyntaxKind::CALL_NAME)
        .expect("grammar: INLINE_BABEL_CALL always has CALL_NAME (forms/call.rs::parse_inline)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::HasParts;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind};

    fn call(src: &str) -> InlineBabelCall {
        let doc = parse(src);
        InlineBabelCall::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_BABEL_CALL)
                .into_iter()
                .next()
                .expect("call"),
        )
        .expect("cast")
    }

    #[test]
    fn name_args_and_optional_headers() {
        let c = call("call_foo()\n");
        assert!(matches!(
            c.part_span(InlineBabelCallPart::Name),
            PartSpan::Present(_)
        ));
        assert!(
            c.part_insert_point(InlineBabelCallPart::InsideHeader)
                .is_some()
        );
        assert!(c.part_range(InlineBabelCallPart::Arguments).is_some());
        assert!(
            c.part_insert_point(InlineBabelCallPart::EndHeader)
                .is_some()
        );
        crate::ast::check_part_contract(&c);

        let c = call("call_foo[:x](a)[:y]\n");
        crate::ast::check_part_contract(&c);
        assert!(c.part_range(InlineBabelCallPart::InsideHeader).is_some());
        assert!(c.part_range(InlineBabelCallPart::EndHeader).is_some());
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_BABEL_CALL) {
                if let Some(c) = InlineBabelCall::cast(n) {
                    crate::ast::check_part_contract(&c);
                }
            }
        }
    }
}
