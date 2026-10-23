//! Footnote definition element and footnote-reference objects.

use crate::TextRange;
use crate::ast::layout::{
    FootnoteRefLayout, RemovalSide, first_line_end, footnote_label_spans, footnote_ref_layout,
    removal_extending,
};
use crate::ast::support::element_run_range;
use crate::ast::{AstNode, ElementView, HasElements, HasObjects, HasParts, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A footnote definition element: `[fn:label] contents` at the start of a
    /// line.
    FootnoteDefinition,
    OrgSyntaxKind::FOOTNOTE_DEFINITION
);
crate::ast_node!(
    /// A footnote reference object: `[fn:label]`, `[fn:label:definition]` or
    /// `[fn::definition]`.
    FootnoteReference,
    OrgSyntaxKind::FOOTNOTE_REFERENCE
);

/// How a footnote reference is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FootnoteShape {
    /// `[fn:label]`, pointing at a definition elsewhere.
    Named,
    /// `[fn:label:definition]`.
    Inline,
    /// `[fn::definition]`.
    Anonymous,
}

/// Parts of a footnote reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FootnoteReferencePart {
    /// The label; not applicable to an anonymous reference.
    Label,
    /// The inline definition; not applicable to a named reference.
    Definition,
}

/// The inline definition of `[fn:label:def]` or `[fn::def]`, as an object
/// region.
///
/// [`AstNode::cast`] returns `None` for a named reference (`[fn:label]`);
/// [`AstNode::can_cast`] tests the kind only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InlineFootnote(SyntaxNode);

impl AstNode for InlineFootnote {
    fn can_cast(kind: OrgSyntaxKind) -> bool {
        kind == OrgSyntaxKind::FOOTNOTE_REFERENCE
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        let fr = FootnoteReference::cast(node.clone())?;
        match fr.shape() {
            FootnoteShape::Named => None,
            FootnoteShape::Inline | FootnoteShape::Anonymous => Some(Self(node)),
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        &self.0
    }
}

impl crate::ast::Covering for InlineFootnote {}

impl FootnoteReference {
    /// How this reference is written.
    #[must_use]
    pub fn shape(&self) -> FootnoteShape {
        let lay = layout(self);
        match (lay.label, lay.definition) {
            (_, None) => FootnoteShape::Named,
            (Some(_), Some(_)) => FootnoteShape::Inline,
            (None, Some(_)) => FootnoteShape::Anonymous,
        }
    }

    /// Span of the label. `None` for an anonymous reference.
    #[must_use]
    pub fn label(&self) -> Option<TextRange> {
        layout(self).label
    }

    /// The inline definition. `None` for a named reference.
    #[must_use]
    pub fn definition(&self) -> Option<InlineFootnote> {
        InlineFootnote::cast(self.syntax().clone())
    }
}

impl HasParts for FootnoteReference {
    type Part = FootnoteReferencePart;
    const PARTS: &'static [FootnoteReferencePart] = &[
        FootnoteReferencePart::Label,
        FootnoteReferencePart::Definition,
    ];

    fn part_span(&self, part: FootnoteReferencePart) -> PartSpan {
        let lay = layout(self);
        match part {
            FootnoteReferencePart::Label => match lay.label {
                Some(r) => PartSpan::Present(PresentPart::identical(r)),
                None => PartSpan::NotApplicable,
            },
            FootnoteReferencePart::Definition => match lay.definition {
                Some(r) => PartSpan::Present(PresentPart::identical(r)),
                None => PartSpan::NotApplicable,
            },
        }
    }
}

impl HasObjects for InlineFootnote {}

fn layout(fr: &FootnoteReference) -> FootnoteRefLayout {
    footnote_ref_layout(fr.syntax())
        .expect("grammar: FOOTNOTE_REFERENCE is [fn:...] (forms/footnote.rs)")
}

/// Parts of a footnote definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FootnotePart {
    /// The `[fn:label]` that opens the definition at column zero; its content
    /// is the label.
    Label,
    /// Body after the header through the last body element.
    Body,
}

impl FootnoteDefinition {
    /// Body from after the `[fn:label]` header through the last body element;
    /// zero-width after the header line when empty.
    ///
    /// [`ElementView::content_range`] returns the same span, always `Some`.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        element_run_range(self.syntax())
            .unwrap_or_else(|| TextRange::empty(first_line_end(self.syntax())))
    }
}

impl ElementView for FootnoteDefinition {
    fn content_range(&self) -> Option<TextRange> {
        Some(FootnoteDefinition::content_range(self))
    }

    fn is_greater(&self) -> bool {
        true
    }
}

impl HasElements for FootnoteDefinition {}

impl HasParts for FootnoteDefinition {
    type Part = FootnotePart;
    const PARTS: &'static [FootnotePart] = &[FootnotePart::Label, FootnotePart::Body];

    fn part_span(&self, part: FootnotePart) -> PartSpan {
        match part {
            FootnotePart::Label => {
                let (carved, content) = footnote_label_spans(self.syntax());
                let removal = removal_extending(self.syntax(), carved, RemovalSide::After);
                PartSpan::Present(PresentPart::new(carved, content, removal))
            }
            FootnotePart::Body => PartSpan::Present(PresentPart::identical(self.content_range())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_line_end;
    use crate::ast::{Container, Element, HasElements, HasParts, PartSpan};
    use crate::parse;
    use crate::tests::{nodes_of_kind, source_slice};

    fn first_def(src: &str) -> (crate::Document, FootnoteDefinition) {
        let doc = parse(src);
        let f = FootnoteDefinition::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_DEFINITION)
                .into_iter()
                .next()
                .expect("fn"),
        )
        .expect("cast");
        (doc, f)
    }

    #[test]
    fn content_range_is_body_after_header() {
        let src = "[fn:1] body text\n";
        let (doc, f) = first_def(src);
        assert!(f.is_greater());
        let content = f.content_range();
        assert_eq!(ElementView::content_range(&f), Some(content));
        assert_eq!(Element::from(&f).content_range(), Some(content));
        assert_eq!(source_slice(&doc, content), "body text\n");
        assert!(!source_slice(&doc, content).contains("[fn:1]"));

        let src = "[fn:1]\nbody text\n";
        let (doc, f) = first_def(src);
        let content = f.content_range();
        assert_eq!(ElementView::content_range(&f), Some(content));
        assert_eq!(Element::from(&f).content_range(), Some(content));
        assert_eq!(source_slice(&doc, content), "body text\n");
        assert_eq!(content.start(), first_line_end(f.syntax()));
    }

    #[test]
    fn empty_definition_is_zero_width_after_header() {
        let (doc, f) = first_def("[fn:1]\n");
        let content = f.content_range();
        assert!(content.is_empty());
        assert_eq!(ElementView::content_range(&f), Some(content));
        assert_eq!(Element::from(&f).content_range(), Some(content));
        assert_eq!(content.start(), first_line_end(f.syntax()));
        let c = Container::cast(f.syntax().clone()).expect("container");
        assert_eq!(c.first_element_insert_point().offset(), content.start());
        assert_eq!(c.last_element_insert_point().offset(), content.start());
        assert_eq!(f.elements().count(), 0);
        assert!(matches!(
            f.part_span(FootnotePart::Body),
            PartSpan::Present(_)
        ));
        assert_eq!(f.part_content_range(FootnotePart::Body), Some(content));
        assert!(f.part_insert_point(FootnotePart::Body).is_none());
        let carved = f.part_range(FootnotePart::Label).expect("label");
        let removal = f.part_removal_range(FootnotePart::Label).expect("removal");
        assert_eq!(source_slice(&doc, carved), "[fn:1]");
        assert_eq!(removal, carved);
        crate::ast::check_part_contract(&f);
    }

    #[test]
    fn nested_body_elements_skip_affiliated() {
        let src = "[fn:1] lead\n#+CAPTION: c\n- item a\n- item b\n";
        let (doc, f) = first_def(src);
        let kinds: Vec<_> = f.elements().map(|e| e.kind()).collect();
        assert_eq!(
            kinds,
            vec![OrgSyntaxKind::PARAGRAPH, OrgSyntaxKind::PLAIN_LIST]
        );
        assert!(
            !f.elements().any(|e| e.kind() == OrgSyntaxKind::KEYWORD),
            "affiliated run is not an element of the definition"
        );
        assert!(
            source_slice(&doc, f.content_range()).contains("#+CAPTION:"),
            "affiliated run sits in Body even though it is not an element"
        );
        crate::ast::check_part_contract(&f);
    }

    #[test]
    fn label_is_token_layout_under_awkward_whitespace() {
        let src = "[fn:a-b_c]    same-line body\n";
        let (doc, f) = first_def(src);
        let carved = f.part_range(FootnotePart::Label).expect("label");
        let content = f.part_content_range(FootnotePart::Label).expect("content");
        let removal = f.part_removal_range(FootnotePart::Label).expect("removal");
        let body = f.part_content_range(FootnotePart::Body).expect("body");
        assert_eq!(source_slice(&doc, carved), "[fn:a-b_c]");
        assert_eq!(source_slice(&doc, content), "a-b_c");
        assert_eq!(source_slice(&doc, removal), "[fn:a-b_c]    ");
        assert!(
            !source_slice(&doc, removal).contains("same-line body"),
            "label removal must not swallow the same-line paragraph"
        );
        assert_eq!(body, f.content_range());
        assert_eq!(source_slice(&doc, body), "same-line body\n");
        crate::ast::check_part_contract(&f);

        let src = "[fn:1]body\n";
        let (doc, f) = first_def(src);
        let carved = f.part_range(FootnotePart::Label).expect("label");
        let removal = f.part_removal_range(FootnotePart::Label).expect("removal");
        assert_eq!(source_slice(&doc, carved), "[fn:1]");
        assert_eq!(removal, carved);
        assert_eq!(
            source_slice(
                &doc,
                f.part_content_range(FootnotePart::Body).expect("body")
            ),
            "body\n"
        );
        crate::ast::check_part_contract(&f);
    }

    #[test]
    fn has_parts_over_taxonomy() {
        use crate::parse_with;
        use crate::tests::TAXONOMY;
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_DEFINITION) {
                let f = FootnoteDefinition::cast(node).expect("cast");
                crate::ast::check_part_contract(&f);
            }
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE) {
                if let Some(r) = FootnoteReference::cast(node) {
                    crate::ast::check_part_contract(&r);
                }
            }
        }
    }

    #[test]
    fn reference_shapes_and_inline_cast() {
        let doc = parse("see [fn:lab]\n");
        let r = FootnoteReference::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE)
                .into_iter()
                .next()
                .expect("ref"),
        )
        .expect("cast");
        assert_eq!(r.shape(), FootnoteShape::Named);
        assert!(r.definition().is_none());
        assert!(InlineFootnote::cast(r.syntax().clone()).is_none());
        assert!(matches!(
            r.part_span(FootnoteReferencePart::Definition),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&r);

        let doc = parse("see [fn:lab:*b*]\n");
        let r = FootnoteReference::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE)
                .into_iter()
                .next()
                .expect("ref"),
        )
        .expect("cast");
        assert_eq!(r.shape(), FootnoteShape::Inline);
        let def = r.definition().expect("def");
        assert!(def.objects().count() >= 1);
        crate::ast::check_part_contract(&r);

        let doc = parse("see [fn::x]\n");
        let r = FootnoteReference::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE)
                .into_iter()
                .next()
                .expect("ref"),
        )
        .expect("cast");
        assert_eq!(r.shape(), FootnoteShape::Anonymous);
        assert!(r.label().is_none());
        assert!(r.definition().is_some());
    }

    #[test]
    fn footnote_in_caption_is_not_carved() {
        let doc = parse("#+CAPTION: see [fn:1]\npara\n");
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "STANDARD_NO_FOOTNOTE on KEYWORD_VALUE"
        );
    }
}
