//! Outlines (a heading with everything under it) and sections.
//!
//! Tree shape:
//!
//! ```text
//!   OUTLINE
//!     HEADING
//!     [PLANNING]
//!     [PROPERTY_DRAWER]
//!     [SECTION]
//!     OUTLINE*
//! ```
//!
//! Blank lines right after the heading belong to `OUTLINE`, outside
//! `SECTION`. The zeroth section, before the first heading, is a direct child
//! of `DOCUMENT`.

use super::drawer::{Drawer, named_drawer};
use super::heading::{Heading, Headline};
use super::list::Item;
use super::planning::Planning;
use super::property::PropertyDrawer;
use crate::ast::support::{
    child_view, child_views, dest, dest_after, element_run_range, next_typed, prev_typed,
};
use crate::ast::{AstNode, Element, HasElements, InsertPoint};
use crate::syntax::OrgSyntaxKind;
use crate::{TextRange, TextSize};

crate::ast_node!(
    /// An outline: a heading with its planning, properties, section and child
    /// outlines.
    Outline,
    OrgSyntaxKind::OUTLINE
);
crate::ast_node!(
    /// A section: the elements under a heading, or before the first heading.
    Section,
    OrgSyntaxKind::SECTION
);

impl HasElements for Section {}

impl Section {
    /// The first drawer named `name` (ASCII caseless) among the section's
    /// direct children. The property drawer never matches.
    #[must_use]
    pub fn drawer(&self, name: &str) -> Option<Drawer> {
        named_drawer(self.syntax(), name)
    }

    /// Extent of the element children, affiliated runs included.
    ///
    /// The parser never produces a section of blank lines only; the empty
    /// range at the section start covers a tree that has one anyway.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        element_run_range(self.syntax()).unwrap_or_else(|| TextRange::empty(self.range().start()))
    }
}

impl Outline {
    /// The heading line.
    ///
    /// # Panics
    ///
    /// Panics if the outline has no `HEADING` child. The parser always opens
    /// an outline with one.
    #[must_use]
    pub fn heading(&self) -> Heading {
        child_view(self.syntax()).expect("grammar: OUTLINE always has a HEADING child")
    }

    /// Planning line immediately under this outline, if any.
    #[must_use]
    pub fn planning(&self) -> Option<Planning> {
        child_view(self.syntax())
    }

    /// The property drawer right after the heading and planning, if any.
    ///
    /// Lines inside the drawer are [`PropertyDrawer::properties`].
    #[must_use]
    pub fn property_drawer(&self) -> Option<PropertyDrawer> {
        child_view(self.syntax())
    }

    /// The section, if this outline has body elements.
    #[must_use]
    pub fn section(&self) -> Option<Section> {
        child_view(self.syntax())
    }

    /// Checkbox items of the plain lists directly in this outline's section,
    /// sublists included.
    ///
    /// Lists inside other elements (drawers, blocks) and in child outlines are
    /// not visited.
    pub fn direct_checkbox_items(&self) -> impl Iterator<Item = Item> + '_ {
        self.section()
            .map(|s| checkbox_items_in_section(&s))
            .into_iter()
            .flatten()
    }

    /// [`Self::direct_checkbox_items`] of this outline and of every
    /// descendant outline.
    pub fn descendant_checkbox_items(&self) -> impl Iterator<Item = Item> + '_ {
        let mut items = Vec::new();
        for o in std::iter::once(self.clone()).chain(self.descendants()) {
            items.extend(o.direct_checkbox_items());
        }
        items.into_iter()
    }

    /// Star count of the heading, which can differ from tree depth.
    #[must_use]
    pub fn level(&self) -> usize {
        self.heading().level()
    }

    /// Enclosing outline, if this is not a root outline.
    #[must_use]
    pub fn parent(&self) -> Option<Outline> {
        self.syntax().parent().and_then(Outline::cast)
    }

    /// Direct child outlines, in source order.
    pub fn children(&self) -> impl Iterator<Item = Outline> + '_ {
        child_views(self.syntax())
    }

    /// Previous sibling outline under the same parent.
    #[must_use]
    pub fn prev_sibling(&self) -> Option<Outline> {
        prev_typed(self.syntax())
    }

    /// Next sibling outline under the same parent.
    #[must_use]
    pub fn next_sibling(&self) -> Option<Outline> {
        next_typed(self.syntax())
    }

    /// Ancestor outlines, innermost first.
    pub fn ancestors(&self) -> impl Iterator<Item = Outline> + '_ {
        std::iter::successors(self.parent(), Outline::parent)
    }

    /// Descendant outlines, pre-order, excluding self.
    pub fn descendants(&self) -> impl Iterator<Item = Outline> + '_ {
        let kids: Vec<Outline> = self.children().collect();
        let mut stack: Vec<Outline> = kids.into_iter().rev().collect();
        std::iter::from_fn(move || {
            let next = stack.pop()?;
            let kids: Vec<_> = next.children().collect();
            stack.extend(kids.into_iter().rev());
            Some(next)
        })
    }

    /// Extent of the whole subtree: heading, metadata, section, descendants
    /// and trailing blank lines.
    #[must_use]
    pub fn subtree_range(&self) -> TextRange {
        self.range()
    }

    /// The entry's own text: from the heading through the section, or through
    /// the last of property drawer, planning and heading when there is none.
    ///
    /// Child outlines and trailing blank lines are excluded, so for an outline
    /// with only a heading this is the heading's range.
    #[must_use]
    pub fn entry_range(&self) -> TextRange {
        TextRange::new(self.heading().range().start(), local_body_end(self))
    }

    /// This outline's heading, then every descendant outline's, in document
    /// order. Inline tasks are not included.
    pub fn subtree_headings(&self) -> impl Iterator<Item = Heading> + '_ {
        std::iter::once(self.heading()).chain(self.descendants().map(|o| o.heading()))
    }

    /// Where a new sibling outline above this one is written.
    #[must_use]
    pub fn insert_before(&self) -> InsertPoint {
        dest(self.syntax(), self.subtree_range().start())
    }

    /// Where a new sibling outline below this one is written: after the
    /// subtree, trailing blank lines included.
    #[must_use]
    pub fn insert_after(&self) -> InsertPoint {
        dest(self.syntax(), self.subtree_range().end())
    }

    /// Where a new last child outline is written: after the last child, or
    /// before the trailing blank lines when there is no child.
    #[must_use]
    pub fn content_end_insert_point(&self) -> InsertPoint {
        match self.children().last() {
            Some(child) => child.insert_after(),
            None => self.first_child_insert_point(),
        }
    }

    /// Where a new first child outline is written: before the first child, or
    /// after the entry's own text, before trailing blank lines, when there is
    /// no child.
    #[must_use]
    pub fn first_child_insert_point(&self) -> InsertPoint {
        if let Some(child) = self.children().next() {
            return child.insert_before();
        }
        dest(self.syntax(), local_body_end(self))
    }

    /// Where a planning line is written: right after the heading, the only
    /// place Org recognises one.
    #[must_use]
    pub fn planning_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.heading().range().end())
    }

    /// Where a property drawer is written: after the planning line, else after
    /// the heading.
    #[must_use]
    pub fn property_drawer_insert_point(&self) -> InsertPoint {
        dest_after(
            self.syntax(),
            self.planning().map(|p| p.range()),
            dest(self.syntax(), self.heading().range().end()),
        )
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
    /// of the section, or [`Self::first_child_insert_point`] when there is no
    /// section.
    #[must_use]
    pub fn log_item_insert_point(&self) -> InsertPoint {
        match self.section() {
            Some(s) => dest(self.syntax(), s.content_range().start()),
            None => self.first_child_insert_point(),
        }
    }
}

impl Heading {
    /// The outline this heading opens.
    ///
    /// # Panics
    ///
    /// Panics if the heading's parent is not `OUTLINE`. The parser always
    /// wraps a heading in an outline.
    #[must_use]
    pub fn outline(&self) -> Outline {
        self.syntax()
            .parent()
            .and_then(Outline::cast)
            .expect("grammar: HEADING parent is always OUTLINE (drive/document.rs)")
    }
}

/// End of the entry's own text: the last of section, property drawer,
/// planning and heading, before child outlines and trailing blank lines.
fn local_body_end(outline: &Outline) -> TextSize {
    outline
        .section()
        .map(|s| s.range().end())
        .or_else(|| outline.property_drawer().map(|p| p.range().end()))
        .or_else(|| outline.planning().map(|p| p.range().end()))
        .unwrap_or_else(|| outline.heading().range().end())
}

fn checkbox_items_in_section(section: &Section) -> Vec<Item> {
    let mut items = Vec::new();
    for e in section.elements() {
        if let Element::PlainList(list) = e {
            items.extend(list.descendant_items().filter(|i| i.checkbox().is_some()));
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Padding;
    use crate::parse;
    use crate::parser::ParseConfig;
    use crate::tests::source_slice;
    use crate::{TextSize, parse_with};

    fn first_outline(src: &str) -> (crate::Document, Outline) {
        let doc = parse(src);
        let o = doc.root_outlines().next().expect("outline");
        (doc, o)
    }

    #[test]
    fn multi_level_outline_navigation() {
        let src = "* A\n\n** B\nbody\n* C\n";
        let doc = parse(src);
        let roots: Vec<_> = doc.root_outlines().collect();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].heading().title().text().to_string().trim(), "A");
        assert_eq!(roots[1].heading().title().text().to_string().trim(), "C");

        let a = &roots[0];
        let b = a.children().next().expect("child");
        assert_eq!(b.heading().title().text().to_string().trim(), "B");
        assert_eq!(b.parent().as_ref(), Some(a));
        assert_eq!(a.next_sibling().as_ref(), Some(&roots[1]));
        assert_eq!(roots[1].prev_sibling().as_ref(), Some(a));
        assert_eq!(b.ancestors().count(), 1);
        assert!(a.subtree_range().end() <= roots[1].subtree_range().start());
        assert!(
            !source_slice(&doc, a.subtree_range()).contains("* C"),
            "outline A must not swallow C's stars"
        );
    }

    #[test]
    fn child_outline_prev_sibling_does_not_skip_section() {
        let src = "* Parent\nsection body\n** B\n** C\n";
        let doc = parse(src);
        let parent = doc.root_outlines().next().unwrap();
        let kids: Vec<_> = parent.children().collect();
        assert_eq!(kids.len(), 2);
        let b = &kids[0];
        let c = &kids[1];
        assert_eq!(
            b.syntax().prev_sibling().map(|n| n.kind()),
            Some(OrgSyntaxKind::SECTION)
        );
        assert!(b.prev_sibling().is_none());
        assert_eq!(
            b.next_sibling().as_ref().map(Outline::syntax),
            Some(c.syntax())
        );
        assert_eq!(
            c.prev_sibling().as_ref().map(Outline::syntax),
            Some(b.syntax())
        );
        assert!(c.next_sibling().is_none());

        let src = "intro\n* A\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        assert!(a.prev_sibling().is_none());
        assert_eq!(
            a.syntax().prev_sibling().map(|n| n.kind()),
            Some(OrgSyntaxKind::SECTION)
        );
    }

    #[test]
    fn subtree_headings_skip_inline_tasks() {
        let src = "* A\n*************** TASK\n*************** END\n** B\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let titles: Vec<_> = a
            .subtree_headings()
            .map(|h| h.title().text().to_string())
            .collect();
        assert_eq!(
            titles.len(),
            2,
            "A and B only, not the inline task: {titles:?}"
        );
        assert!(
            titles
                .iter()
                .all(|t| !t.to_ascii_uppercase().contains("TASK"))
        );
    }

    #[test]
    fn nesting_ceiling_is_not_level_plus_one() {
        let cfg = ParseConfig::default().with_max_nesting_depth(1);
        let doc = parse_with("* A\n** B\n", &cfg);
        let a = doc.root_outlines().next().unwrap();
        let b = a
            .next_sibling()
            .expect("B attaches as sibling at the ceiling");
        assert_eq!(a.level(), 1);
        assert_eq!(b.level(), 2);
        assert!(a.parent().is_none());
        assert!(b.parent().is_none());
    }

    #[test]
    fn insert_after_versus_content_end_on_heading_only_blanks() {
        let src = "* H\n\n\n";
        let (_, o) = first_outline(src);
        let after = o.insert_after();
        let end = o.content_end_insert_point();
        let first = o.first_child_insert_point();
        assert_eq!(after.offset(), TextSize::of(src));
        assert_eq!(end.offset(), first.offset());
        assert!(end.offset() < after.offset());
    }

    #[test]
    fn multi_level_insert_after_matches_parent_content_end() {
        let src = "* A\n** B\n\n* C\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();
        let c = a.next_sibling().unwrap();
        let b_after = b.insert_after();
        assert_eq!(b_after, a.content_end_insert_point());
        assert!(b_after.offset() <= c.range().start());
        let a_after = a.insert_after();
        assert!(a_after.offset() >= b.range().end());
        assert!(
            a_after.offset() <= c.range().start(),
            "A.insert_after must not land inside C"
        );
    }

    #[test]
    fn first_child_table() {
        let heading_only = first_outline("* H\n").1;
        assert_eq!(
            heading_only.first_child_insert_point().offset(),
            heading_only.heading().range().end()
        );

        let with_blanks = first_outline("* H\n\n").1;
        assert_eq!(
            with_blanks.first_child_insert_point().offset(),
            with_blanks.heading().range().end()
        );

        let with_plan = first_outline("* H\nSCHEDULED: <2024-01-01 Mon>\n").1;
        assert_eq!(
            with_plan.first_child_insert_point().offset(),
            with_plan.planning().unwrap().range().end()
        );

        let with_props = first_outline("* H\n:PROPERTIES:\n:FOO: 1\n:END:\n").1;
        assert_eq!(
            with_props.first_child_insert_point().offset(),
            with_props.property_drawer().unwrap().range().end()
        );

        let with_section = first_outline("* H\nbody\n").1;
        assert_eq!(
            with_section.first_child_insert_point().offset(),
            with_section.section().unwrap().range().end()
        );

        let with_child = first_outline("* H\n** C\n").1;
        assert_eq!(
            with_child.first_child_insert_point().offset(),
            with_child.children().next().unwrap().range().start()
        );
    }

    #[test]
    fn no_trailing_newline_insert_after_leads_newline() {
        let src = "* H";
        let (_, o) = first_outline(src);
        let after = o.insert_after();
        assert_eq!(after.offset(), TextSize::of(src));
        assert_eq!(after.lead(), Padding::Newline);
        let first = o.first_child_insert_point();
        assert_eq!(first.lead(), Padding::Newline);
    }

    #[test]
    fn decoy_scheduled_after_blank_is_not_planning() {
        let src = "* H\n\nSCHEDULED: <2024-01-01 Mon>\n";
        let (_, o) = first_outline(src);
        assert!(o.planning().is_none());
        assert!(o.section().is_some());
        assert_eq!(
            o.planning_insert_point().offset(),
            o.heading().range().end()
        );
    }

    #[test]
    fn heading_outline_round_trip() {
        let src = "* A\n** B\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();
        assert_eq!(a.heading().outline().syntax(), a.syntax());
        assert_eq!(b.heading().outline().syntax(), b.syntax());
        assert_eq!(
            b.heading().outline().parent().as_ref().map(Outline::syntax),
            Some(a.syntax())
        );
    }

    #[test]
    fn entry_range_heading_only_planning_and_child() {
        let src = "* H\n";
        let (_, o) = first_outline(src);
        assert_eq!(o.entry_range(), o.heading().range());

        let src = "* H\n\n\n";
        let (_, o) = first_outline(src);
        assert_eq!(o.entry_range(), o.heading().range());
        assert!(o.entry_range().end() < o.subtree_range().end());

        let src = "* H\nSCHEDULED: <2024-01-01 Mon>\n";
        let (_, o) = first_outline(src);
        assert_eq!(o.entry_range().start(), o.heading().range().start());
        assert_eq!(o.entry_range().end(), o.planning().unwrap().range().end());

        let src = "* H\n:PROPERTIES:\n:FOO: 1\n:END:\n";
        let (_, o) = first_outline(src);
        assert_eq!(o.entry_range().start(), o.heading().range().start());
        assert_eq!(
            o.entry_range().end(),
            o.property_drawer().unwrap().range().end()
        );

        let src = "* Parent\nsection body\n** Child\n";
        let (doc, o) = first_outline(src);
        let child = o.children().next().unwrap();
        let slice = source_slice(&doc, o.entry_range());
        assert!(slice.contains("section body"));
        assert!(
            !slice.contains("** Child"),
            "entry_range must not include child outlines: {slice:?}"
        );
        assert!(o.entry_range().end() <= child.range().start());
        assert!(o.subtree_range().end() >= child.range().end());
    }
}
