//! Named drawers: `:NAME:` ... `:END:`.
//!
//! Names compare ASCII-caseless, and no name gets special treatment,
//! `LOGBOOK` included.

use crate::TextRange;
use crate::ast::layout::{drawer_body_range, first_line_end, with_node_text};
use crate::ast::support::{child_views, dest};
use crate::ast::{AstNode, ElementView, HasElements, HasParts, InsertPoint, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A drawer: `:NAME:` ... `:END:`. Property drawers have their own view,
    /// [`crate::ast::PropertyDrawer`].
    Drawer,
    OrgSyntaxKind::DRAWER
);

/// Parts of a named drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrawerPart {
    /// The name between the colons of the opener line.
    Name,
    /// Interior between the opener line and the `:END:` line.
    Body,
}

/// First direct `DRAWER` child whose name matches `name` (ASCII caseless).
#[must_use]
pub(crate) fn named_drawer(parent: &SyntaxNode, name: &str) -> Option<Drawer> {
    child_views(parent).find(|d: &Drawer| d.name_eq(name))
}

impl Drawer {
    /// The `DRAWER_NAME` child.
    ///
    /// # Panics
    ///
    /// Panics if `DRAWER_NAME` is missing, which the parser never produces.
    #[must_use]
    pub(crate) fn name_node(&self) -> SyntaxNode {
        name_node(self.syntax())
    }

    /// Name as written inside the `:NAME:` opener.
    ///
    /// Allocates; to compare a name, use [`Self::name_eq`].
    ///
    /// # Panics
    ///
    /// Panics if the node has no `DRAWER_NAME` child. The parser always
    /// produces one.
    #[must_use]
    pub fn name(&self) -> String {
        with_node_text(&self.name_node(), str::to_string)
    }

    /// Whether the name equals `want`, ignoring ASCII case.
    #[must_use]
    pub fn name_eq(&self, want: &str) -> bool {
        with_node_text(&self.name_node(), |s| s.eq_ignore_ascii_case(want))
    }

    /// Body between the opener line and the `:END:` line.
    #[must_use]
    pub fn body_range(&self) -> TextRange {
        drawer_body_range(self.syntax())
            .unwrap_or_else(|| TextRange::empty(first_line_end(self.syntax())))
    }

    /// Where to write a log entry when entries run newest first: the start of
    /// the body.
    #[must_use]
    pub fn log_prepend_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.body_range().start())
    }

    /// Where to append a line, such as a clock line: the end of the body,
    /// before `:END:`.
    #[must_use]
    pub fn clock_append_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.body_range().end())
    }
}

impl ElementView for Drawer {
    fn content_range(&self) -> Option<TextRange> {
        Some(self.body_range())
    }

    fn is_greater(&self) -> bool {
        true
    }
}

impl HasElements for Drawer {}

impl HasParts for Drawer {
    type Part = DrawerPart;
    const PARTS: &'static [DrawerPart] = &[DrawerPart::Name, DrawerPart::Body];

    fn part_span(&self, part: DrawerPart) -> PartSpan {
        match part {
            DrawerPart::Name => PartSpan::Present(PresentPart::identical(
                name_node(self.syntax()).text_range(),
            )),
            DrawerPart::Body => PartSpan::Present(PresentPart::identical(self.body_range())),
        }
    }
}

fn name_node(drawer: &SyntaxNode) -> SyntaxNode {
    drawer
        .children()
        .find(|c| c.kind() == OrgSyntaxKind::DRAWER_NAME)
        .expect("grammar: DRAWER always has DRAWER_NAME (forms/drawer.rs::parse_opener_line)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{HasElements, HasParts, InlineTask};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, source_slice};

    fn first_drawer(src: &str) -> (crate::Document, Drawer) {
        let doc = parse(src);
        let d = Drawer::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER)
                .into_iter()
                .next()
                .expect("drawer"),
        )
        .expect("cast");
        (doc, d)
    }

    #[test]
    fn name_is_caseless_and_logbook_is_not_special() {
        let (_, d) = first_drawer(":LOGBOOK:\n:END:\n");
        assert_eq!(d.name(), "LOGBOOK");
        assert!(d.name_eq("logbook"));
        assert!(d.name_eq("LOGBOOK"));
        assert!(!d.name_eq("notes"));
    }

    #[test]
    fn body_range_empty_and_excludes_end_and_post_blank() {
        let (_, d) = first_drawer(":NOTES:\n:END:\n");
        assert!(d.body_range().is_empty());
        assert_eq!(
            d.log_prepend_insert_point().offset(),
            d.body_range().start()
        );

        let src = ":NOTES:\nitem\n:END:\n\n";
        let (doc, d) = first_drawer(src);
        assert_eq!(source_slice(&doc, d.body_range()), "item\n");
        assert_eq!(d.clock_append_insert_point().offset(), d.body_range().end());
        assert!(
            d.body_range().end() < d.range().end(),
            "post-blank stays outside the body"
        );
    }

    #[test]
    fn section_drawer_first_match_never_property_drawer() {
        let src = "* H\n:PROPERTIES:\n:FOO: 1\n:END:\n:LOGBOOK:\none\n:END:\n:notes:\ntwo\n:END:\n";
        let doc = parse(src);
        let o = doc.root_outlines().next().unwrap();
        let section = o.section().unwrap();
        let log = section.drawer("logbook").unwrap();
        assert!(log.name_eq("LOGBOOK"));
        assert!(section.drawer("properties").is_none());
        assert_eq!(section.drawer("NOTES").unwrap().name(), "notes");
        assert!(o.property_drawer().is_some());

        let two = parse("* H\n:LOGBOOK:\nfirst\n:END:\n:logbook:\nsecond\n:END:\n");
        let section = two.root_outlines().next().unwrap().section().unwrap();
        assert_eq!(section.drawer("logbook").unwrap().name(), "LOGBOOK");
    }

    #[test]
    fn log_drawer_insert_point_with_and_without_property_drawer() {
        let no_pd = parse("* H\nbody\n");
        let o = no_pd.root_outlines().next().unwrap();
        assert_eq!(
            o.log_drawer_insert_point().offset(),
            o.property_drawer_insert_point().offset()
        );

        let with_pd = parse("* H\n:PROPERTIES:\n:FOO: 1\n:END:\nbody\n");
        let o = with_pd.root_outlines().next().unwrap();
        assert_eq!(
            o.log_drawer_insert_point().offset(),
            o.property_drawer().unwrap().range().end()
        );
        assert!(o.log_drawer_insert_point().offset() <= o.section().unwrap().range().start());
    }

    #[test]
    fn log_item_insert_point_with_and_without_section() {
        let headed = parse("* H\n");
        let o = headed.root_outlines().next().unwrap();
        assert_eq!(
            o.log_item_insert_point().offset(),
            o.first_child_insert_point().offset()
        );

        let with_section = parse("* H\nbody\n");
        let o = with_section.root_outlines().next().unwrap();
        assert_eq!(
            o.log_item_insert_point().offset(),
            o.section().unwrap().content_range().start()
        );
    }

    fn task(src: &str) -> InlineTask {
        InlineTask::cast(
            nodes_of_kind(parse(src).root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn has_elements_yields_body_skips_affiliated() {
        let src = ":NOTES:\n#+CAPTION: c\npara\n:END:\n";
        let (_, d) = first_drawer(src);
        let els: Vec<_> = d.elements().collect();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].kind(), OrgSyntaxKind::PARAGRAPH);
        assert!(
            !d.elements().any(|e| e.kind() == OrgSyntaxKind::KEYWORD),
            "affiliated run is not an element of the drawer"
        );
        crate::ast::check_part_contract(&d);
    }

    #[test]
    fn name_content_is_drawer_name_empty_body_present() {
        let (doc, d) = first_drawer(":NOTES:\n:END:\n");
        let name = d.part_content_range(DrawerPart::Name).expect("name");
        assert_eq!(source_slice(&doc, name), "NOTES");
        let body = d.part_content_range(DrawerPart::Body).expect("body");
        assert!(body.is_empty());
        assert_eq!(body, d.body_range());
        let c = crate::ast::Container::cast(d.syntax().clone()).expect("container");
        assert_eq!(c.first_element_insert_point().offset(), body.start());
        assert_eq!(c.last_element_insert_point().offset(), body.start());
        assert_eq!(d.elements().count(), 0);
        crate::ast::check_part_contract(&d);
    }

    #[test]
    fn has_parts_over_taxonomy() {
        use crate::parse_with;
        use crate::tests::TAXONOMY;
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER) {
                let d = Drawer::cast(node).expect("cast");
                crate::ast::check_part_contract(&d);
            }
        }
    }

    #[test]
    fn closed_inline_task_drawer_and_log_item_dests() {
        let para = parse("*************** T\npara\n*************** END\n");
        let t = InlineTask::cast(
            nodes_of_kind(para.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(t.drawer("logbook").is_none());
        let first = crate::ast::support::element_children(t.syntax())
            .next()
            .unwrap();
        assert_eq!(
            t.log_item_insert_point().offset(),
            crate::ast::support::affiliated_extent(&first).start()
        );

        let with_log =
            parse("*************** T\n:LOGBOOK:\nitem\n:END:\npara\n*************** END\n");
        let t = InlineTask::cast(
            nodes_of_kind(with_log.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let log = t.drawer("logbook").expect("logbook");
        assert_eq!(t.log_item_insert_point().offset(), log.range().start());

        let planned = task("*************** T\nSCHEDULED: <2020-01-01 Wed>\n*************** END\n");
        assert_eq!(
            planned.log_item_insert_point().offset(),
            planned.planning().unwrap().range().end()
        );
        assert_eq!(
            planned.log_drawer_insert_point().offset(),
            planned.property_drawer_insert_point().offset()
        );

        let props = task("*************** T\n:PROPERTIES:\n:ID: x\n:END:\n*************** END\n");
        assert_eq!(
            props.log_item_insert_point().offset(),
            props.property_drawer().unwrap().range().end()
        );
        assert_eq!(
            props.log_drawer_insert_point().offset(),
            props.property_drawer().unwrap().range().end()
        );

        let no_pd = task("*************** T\npara\n*************** END\n");
        assert_eq!(
            no_pd.log_drawer_insert_point().offset(),
            no_pd.property_drawer_insert_point().offset()
        );

        let deg = parse("*************** T\n\n");
        let t = InlineTask::cast(
            nodes_of_kind(deg.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(t.drawer("logbook").is_none());
        assert_eq!(
            t.log_item_insert_point().offset(),
            crate::TextSize::of("*************** T\n")
        );
    }
}
