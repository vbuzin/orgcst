//! Export snippet objects: `@@backend:value@@`.

use crate::ast::layout::{ExportSnippetSpans, export_snippet_spans};
use crate::ast::{AstNode, HasParts, PartSpan, PresentPart};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// An export snippet object: `@@backend:value@@`.
    ExportSnippet,
    OrgSyntaxKind::EXPORT_SNIPPET
);

/// Parts of an export snippet. Each span excludes the `@@` and the colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportSnippetPart {
    /// `backend` in `@@backend:value@@`.
    Backend,
    /// `value` in `@@backend:value@@`; may be empty.
    Value,
}

impl HasParts for ExportSnippet {
    type Part = ExportSnippetPart;
    const PARTS: &'static [ExportSnippetPart] =
        &[ExportSnippetPart::Backend, ExportSnippetPart::Value];

    fn part_span(&self, part: ExportSnippetPart) -> PartSpan {
        let sp = spans(self);
        match part {
            ExportSnippetPart::Backend => PartSpan::Present(PresentPart::identical(sp.backend)),
            ExportSnippetPart::Value => PartSpan::Present(PresentPart::identical(sp.value)),
        }
    }
}

fn spans(snip: &ExportSnippet) -> ExportSnippetSpans {
    export_snippet_spans(snip.syntax())
        .expect("grammar: EXPORT_SNIPPET is @@backend:value@@ (forms/export_snippet.rs)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::HasParts;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn snip(src: &str) -> (crate::Document, ExportSnippet) {
        let doc = parse(src);
        let s = ExportSnippet::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::EXPORT_SNIPPET)
                .into_iter()
                .next()
                .expect("snippet"),
        )
        .expect("cast");
        (doc, s)
    }

    #[test]
    fn backend_and_value_exclude_delimiters() {
        let (doc, s) = snip("@@html:<b>@@\n");
        assert_eq!(
            source_slice(
                &doc,
                s.part_content_range(ExportSnippetPart::Backend).unwrap()
            ),
            "html"
        );
        assert_eq!(
            source_slice(
                &doc,
                s.part_content_range(ExportSnippetPart::Value).unwrap()
            ),
            "<b>"
        );
        crate::ast::check_part_contract(&s);

        let (_, empty) = snip("@@html:@@\n");
        assert!(
            empty
                .part_content_range(ExportSnippetPart::Value)
                .unwrap()
                .is_empty()
        );
        crate::ast::check_part_contract(&empty);
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::EXPORT_SNIPPET) {
                if let Some(s) = ExportSnippet::cast(n) {
                    crate::ast::check_part_contract(&s);
                }
            }
        }
    }
}
