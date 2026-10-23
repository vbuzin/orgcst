//! Inline tasks: headlines with many stars that sit among elements instead of
//! starting a subtree.
//!
//! An inline task is an element, not an `OUTLINE`. A task closed by an `END`
//! line holds its planning, property drawer and body elements directly. A
//! task with no `END` line is only its headline and the blank lines after it.

use super::drawer::{Drawer, named_drawer};
use super::heading::{HEADLINE_PARTS, HeadingPart, Headline, headline_part_span};
use super::planning::Planning;
use super::property::PropertyDrawer;
use crate::TextRange;
use crate::ast::support::{child_view, dest, dest_after, element_run_range};
use crate::ast::{AstNode, ElementView, HasElements, HasParts, InsertPoint, PartSpan};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// An inline task: a headline with at least the inline task level of
    /// stars, sitting among elements, optionally closed by an `END` line.
    InlineTask,
    OrgSyntaxKind::INLINE_TASK
);

impl HasParts for InlineTask {
    type Part = HeadingPart;
    const PARTS: &'static [HeadingPart] = HEADLINE_PARTS;

    fn part_span(&self, part: HeadingPart) -> PartSpan {
        headline_part_span(self.syntax(), part)
    }
}

impl Headline for InlineTask {}

impl HasElements for InlineTask {}

impl InlineTask {
    /// Planning line, when the task is closed and has one.
    #[must_use]
    pub fn planning(&self) -> Option<Planning> {
        child_view(self.syntax())
    }

    /// Property drawer, when the task is closed and has one.
    ///
    /// Lines inside the drawer are [`PropertyDrawer::properties`].
    #[must_use]
    pub fn property_drawer(&self) -> Option<PropertyDrawer> {
        child_view(self.syntax())
    }

    /// The task's whole extent: from the headline through the `END` line, or
    /// the headline and its blank lines when there is no `END` line.
    #[must_use]
    pub fn entry_range(&self) -> TextRange {
        self.range()
    }

    /// Where a planning line is written: after the headline.
    #[must_use]
    pub fn planning_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.star_line_range().end())
    }

    /// Where a property drawer is written: after the planning line, else
    /// after the headline.
    #[must_use]
    pub fn property_drawer_insert_point(&self) -> InsertPoint {
        dest_after(
            self.syntax(),
            self.planning().map(|p| p.range()),
            dest(self.syntax(), self.star_line_range().end()),
        )
    }

    /// The first drawer named `name` (ASCII caseless) among the direct
    /// children. The property drawer never matches.
    #[must_use]
    pub fn drawer(&self, name: &str) -> Option<Drawer> {
        named_drawer(self.syntax(), name)
    }

    /// Where a new drawer such as `LOGBOOK` is written: after the property
    /// drawer, else where a property drawer would go.
    #[must_use]
    pub fn log_drawer_insert_point(&self) -> InsertPoint {
        dest_after(
            self.syntax(),
            self.property_drawer().map(|p| p.range()),
            self.property_drawer_insert_point(),
        )
    }

    /// Where a log entry is written when entries run newest first: the start
    /// of the first body element, or just after the planning and property
    /// drawer when the body is empty.
    #[must_use]
    pub fn log_item_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.content_range().start())
    }

    /// Span of the body elements: after the headline, planning and property
    /// drawer, and before the `END` line. Zero-width when there are none.
    ///
    /// [`ElementView::content_range`] returns the same span, always `Some`.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        element_run_range(self.syntax()).unwrap_or_else(|| TextRange::empty(self.empty_body_slot()))
    }

    fn empty_body_slot(&self) -> crate::TextSize {
        self.property_drawer()
            .map(|p| p.range().end())
            .or_else(|| self.planning().map(|p| p.range().end()))
            .unwrap_or_else(|| self.star_line_range().end())
    }
}

impl ElementView for InlineTask {
    fn content_range(&self) -> Option<TextRange> {
        Some(InlineTask::content_range(self))
    }

    fn is_greater(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Element;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn task(src: &str) -> (crate::Document, InlineTask) {
        let doc = parse(src);
        let t = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .expect("inline task"),
        )
        .expect("cast");
        (doc, t)
    }

    #[test]
    fn closed_inline_task_has_metadata_degenerate_does_not() {
        let (_, t) = task(
            "*************** T\nSCHEDULED: <2024-01-01 Mon>\n:PROPERTIES:\n:ID: x\n:END:\nbody\n*************** END\n",
        );
        assert!(t.planning().is_some());
        assert!(t.property_drawer().is_some());
        crate::ast::check_part_contract(&t);

        let (_, t) = task("*************** T\n\n");
        assert!(t.planning().is_none());
        assert!(t.property_drawer().is_none());
        assert!(t.elements().next().is_none());
    }

    #[test]
    fn content_range_is_body_elements() {
        let src = "*************** T\nbody\n*************** END\n";
        let (doc, t) = task(src);
        let content = t.content_range();
        assert_eq!(ElementView::content_range(&t), Some(content));
        assert_eq!(source_slice(&doc, content).trim(), "body");

        let (_, empty) = task("*************** T\n*************** END\n");
        assert!(empty.content_range().is_empty());
        assert_eq!(empty.content_range().start(), empty.star_line_range().end());
        assert_eq!(
            ElementView::content_range(&empty),
            Some(empty.content_range())
        );
        assert_eq!(
            Element::from(&empty).content_range(),
            Some(empty.content_range())
        );
        assert_eq!(Element::from(&t).content_range(), Some(t.content_range()));

        let (_, props) =
            task("*************** T\n:PROPERTIES:\n:ID: x\n:END:\n*************** END\n");
        assert!(props.content_range().is_empty());
        assert_eq!(
            props.content_range().start(),
            props.property_drawer().unwrap().range().end()
        );
        assert!(props.content_range().start() > props.star_line_range().end());
        assert_eq!(
            ElementView::content_range(&props),
            Some(props.content_range())
        );
    }

    #[test]
    fn entry_range_is_the_task_node() {
        let src = "*************** T\nbody\n*************** END\n";
        let (_, t) = task(src);
        assert_eq!(t.entry_range(), t.range());

        let src = "*************** T\n\n";
        let (_, t) = task(src);
        assert_eq!(t.entry_range(), t.range());
        assert!(t.entry_range().end() > t.title().range().end());
    }

    #[test]
    fn closed_task_entry_range_covers_end_line() {
        let src = "*************** T\nSCHEDULED: <2024-01-01 Mon>\n*************** END\n";
        let (doc, t) = task(src);
        let slice = source_slice(&doc, t.entry_range());
        assert!(slice.contains("SCHEDULED:"));
        assert!(slice.contains("END"));
    }

    #[test]
    fn has_parts_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK) {
                let t = InlineTask::cast(node).unwrap();
                crate::ast::check_part_contract(&t);
            }
        }
    }
}
