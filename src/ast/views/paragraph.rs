//! Paragraph element: inline objects plus owned post-blank.

use crate::TextRange;
use crate::TextSize;
use crate::ast::layout::without_trailing_blank_lines;
use crate::ast::{AstNode, Element, ElementView, HasObjects, SplitPoint, split_span};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A paragraph: a run of objects, with the blank lines after it.
    Paragraph,
    OrgSyntaxKind::PARAGRAPH
);

impl HasObjects for Paragraph {}

impl Paragraph {
    /// Cut the text region at `offset`.
    ///
    /// `None` when `offset` is not in [`ElementView::content_range`]'s
    /// half-open `[start, end)`. Offset at `start` yields an empty keep.
    #[must_use]
    pub fn split_at(&self, offset: TextSize) -> Option<SplitPoint> {
        split_span(
            self.content_range()?,
            offset,
            Element::from(self).insert_after(OrgSyntaxKind::PARAGRAPH),
        )
    }
}

impl ElementView for Paragraph {
    fn content_range(&self) -> Option<TextRange> {
        Some(without_trailing_blank_lines(self.syntax()))
    }

    fn is_greater(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, ElementView, Padding};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, source_slice};

    fn paragraph(src: &str) -> (crate::Document, Paragraph) {
        let doc = parse(src);
        let p = Paragraph::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .expect("paragraph"),
        )
        .expect("cast");
        (doc, p)
    }

    #[test]
    fn content_range_excludes_owned_trailing_blank() {
        let (doc, p) = paragraph("hello\n\n");
        assert_eq!(source_slice(&doc, p.content_range().unwrap()), "hello\n");
        assert!(p.range().end() > p.content_range().unwrap().end());
        assert!(!p.is_greater());
    }

    #[test]
    fn split_at_inside_content_yields_keep_and_moved() {
        let (doc, p) = paragraph("hello world\n");
        let content = p.content_range().unwrap();
        let offset = content.start() + TextSize::from(6); // at 'w'
        let sp = p.split_at(offset).expect("inside");
        assert_eq!(source_slice(&doc, sp.keep()), "hello ");
        assert_eq!(source_slice(&doc, sp.moved()), "world\n");
        assert_eq!(sp.dest().offset(), p.range().end());
        assert_eq!(sp.dest().lead(), Padding::BlankLine);

        let sp = p.split_at(content.start()).expect("start");
        assert!(sp.keep().is_empty());
        assert_eq!(source_slice(&doc, sp.moved()), "hello world\n");
    }

    #[test]
    fn nested_objects_are_not_flattened() {
        use crate::ast::{HasObjects, Object};
        let (_, p) = paragraph("*a /i/ b*\n");
        let objs: Vec<_> = p.objects().collect();
        assert_eq!(objs.len(), 1, "nested italic is not a paragraph object");
        let Object::Markup(m) = &objs[0] else {
            panic!("bold");
        };
        let crate::ast::MarkupBody::Emphasis(em) = m.body() else {
            panic!("emphasis");
        };
        assert_eq!(em.objects().count(), 1);
    }

    #[test]
    fn split_at_outside_content_is_none() {
        let src = "#+CAPTION: c\nhello\n\n";
        let (_, p) = paragraph(src);
        let content = p.content_range().unwrap();
        assert!(p.split_at(content.end()).is_none());
        assert!(p.split_at(p.range().end()).is_none());
        assert!(p.full_range().start() < content.start());
        assert!(p.split_at(p.full_range().start()).is_none());
    }
}
