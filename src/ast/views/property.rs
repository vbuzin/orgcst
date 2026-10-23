//! Property drawers and their `:KEY: value` lines.
//!
//! These views read single lines as written. [`crate::Document::property`]
//! merges them, `KEY+` lines included, into a [`crate::Property`]. To delete a
//! whole property, remove the line's [`AstNode::range`], which includes its
//! newline.

use crate::TextRange;
use crate::ast::layout::{
    RemovalSide, children_after_last, drawer_body_range, first_line_end, removal_extending,
    with_node_text,
};
use crate::ast::support::{child_views, dest, field_dest};
use crate::ast::{AstNode, ElementView, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A property drawer: `:PROPERTIES:` ... `:END:`.
    PropertyDrawer,
    OrgSyntaxKind::PROPERTY_DRAWER
);
crate::ast_node!(
    /// One `:KEY: value` line of a property drawer.
    NodeProperty,
    OrgSyntaxKind::NODE_PROPERTY
);

/// Parts of a property line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyPart {
    /// The key between the colons, with any trailing `+`.
    Key,
    /// The value after the key; absent when the line has none.
    Value,
}

impl PropertyDrawer {
    /// The property lines, in source order.
    pub fn properties(&self) -> impl Iterator<Item = NodeProperty> + '_ {
        child_views(self.syntax())
    }

    /// The last line whose key matches `key`, ignoring ASCII case and a
    /// trailing `+` on the line (but not on `key`).
    ///
    /// This is the line an edit of the property would rewrite. For the value
    /// Org computes, with `+` lines merged, use [`crate::Document::property`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<NodeProperty> {
        self.properties().filter(|np| np.key_eq(key)).last()
    }

    /// Where a new property line is written: after the last property line,
    /// or between `:PROPERTIES:` and `:END:` when the drawer is empty.
    #[must_use]
    pub fn new_property_insert_point(&self) -> InsertPoint {
        if let Some(last) = self.properties().last() {
            dest(self.syntax(), last.range().end())
        } else {
            dest(
                self.syntax(),
                drawer_body_range(self.syntax())
                    .map_or_else(|| self.range().end(), rowan::TextRange::start),
            )
        }
    }
}

impl ElementView for PropertyDrawer {
    fn content_range(&self) -> Option<TextRange> {
        Some(
            drawer_body_range(self.syntax())
                .unwrap_or_else(|| TextRange::empty(first_line_end(self.syntax()))),
        )
    }

    fn is_greater(&self) -> bool {
        true
    }
}

impl NodeProperty {
    /// The `NODE_PROPERTY_KEY` child.
    ///
    /// # Panics
    ///
    /// Panics if the node has no `NODE_PROPERTY_KEY` child. The parser always
    /// produces one.
    #[must_use]
    pub(crate) fn key_node(&self) -> SyntaxNode {
        key_syntax(self.syntax())
    }

    /// The `NODE_PROPERTY_VALUE` child, if any.
    #[must_use]
    pub(crate) fn value_node(&self) -> Option<SyntaxNode> {
        value_node(self.syntax())
    }

    /// The key as written, including the trailing `+` of an appending line.
    ///
    /// Allocates; to compare a key, use [`Self::key_eq`].
    ///
    /// # Panics
    ///
    /// Panics if the node has no `NODE_PROPERTY_KEY` child. The parser always
    /// produces one.
    #[must_use]
    pub fn key(&self) -> String {
        with_node_text(&self.key_node(), str::to_string)
    }

    /// Whether the key equals `want`, ignoring ASCII case and a trailing `+`
    /// on the line. A `+` in `want` is not stripped.
    #[must_use]
    pub fn key_eq(&self, want: &str) -> bool {
        with_node_text(&self.key_node(), |s| {
            crate::settings::property_key_eq(s, want)
        })
    }

    /// Range of the key.
    ///
    /// # Panics
    ///
    /// Panics if the node has no `NODE_PROPERTY_KEY` child. The parser always
    /// produces one.
    #[must_use]
    pub fn key_range(&self) -> TextRange {
        self.key_node().text_range()
    }

    /// Whether the key ends with `+`, which makes the line add to the
    /// property's value instead of replacing it.
    #[must_use]
    pub fn is_append(&self) -> bool {
        with_node_text(&self.key_node(), |s| s.ends_with('+'))
    }

    /// The key as written, without a trailing `+`.
    #[must_use]
    pub(crate) fn written_base_key(&self) -> String {
        with_node_text(&self.key_node(), |s| {
            s.strip_suffix('+').unwrap_or(s).to_string()
        })
    }

    /// The value as written, if the line has one.
    #[must_use]
    pub fn value(&self) -> Option<String> {
        self.value_node()
            .map(|n| with_node_text(&n, str::to_string))
    }

    /// Range of the value, if the line has one.
    #[must_use]
    pub fn value_range(&self) -> Option<TextRange> {
        self.value_node().map(|n| n.text_range())
    }
}

impl HasParts for NodeProperty {
    type Part = PropertyPart;
    const PARTS: &'static [PropertyPart] = &[PropertyPart::Key, PropertyPart::Value];

    fn part_span(&self, part: PropertyPart) -> PartSpan {
        match part {
            PropertyPart::Key => {
                let carved = self.key_range();
                PartSpan::Present(PresentPart::identical(carved))
            }
            PropertyPart::Value => {
                if let Some(carved) = self.value_range() {
                    let removal = removal_extending(self.syntax(), carved, RemovalSide::Before);
                    PartSpan::Present(PresentPart::new(carved, carved, removal))
                } else {
                    let offset =
                        closer_end(self.syntax()).unwrap_or_else(|| self.key_range().end());
                    PartSpan::Absent(field_dest(self.syntax(), offset, Padding::None))
                }
            }
        }
    }
}

fn key_syntax(prop: &SyntaxNode) -> SyntaxNode {
    key_node(prop).expect(
        "grammar: NODE_PROPERTY always has NODE_PROPERTY_KEY (forms/drawer.rs::parse_node_property)",
    )
}

fn closer_end(prop: &SyntaxNode) -> Option<crate::TextSize> {
    children_after_last(prop, OrgSyntaxKind::NODE_PROPERTY_KEY)?
        .next()
        .map(|e| e.text_range().end())
}

fn key_node(prop: &SyntaxNode) -> Option<SyntaxNode> {
    prop.children()
        .find(|c| c.kind() == OrgSyntaxKind::NODE_PROPERTY_KEY)
}

fn value_node(prop: &SyntaxNode) -> Option<SyntaxNode> {
    prop.children()
        .find(|c| c.kind() == OrgSyntaxKind::NODE_PROPERTY_VALUE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Padding;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice, written};

    fn drawer(src: &str) -> (crate::Document, PropertyDrawer) {
        let doc = parse(src);
        let d = PropertyDrawer::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PROPERTY_DRAWER)
                .into_iter()
                .next()
                .expect("property drawer"),
        )
        .expect("cast");
        (doc, d)
    }

    fn first_prop(src: &str) -> (crate::Document, NodeProperty) {
        let (doc, d) = drawer(src);
        let np = d.properties().next().expect("property");
        (doc, np)
    }

    #[test]
    fn key_value_additive_valueless_and_trailing_ws() {
        let (doc, np) = first_prop("* H\n:PROPERTIES:\n:FOO: bar\n:END:\n");
        assert_eq!(np.key(), "FOO");
        assert_eq!(np.written_base_key(), "FOO");
        assert_eq!(np.value().as_deref(), Some("bar"));
        assert!(!np.is_append());
        crate::ast::check_part_contract(&np);
        let value_rm = np.part_removal_range(PropertyPart::Value).unwrap();
        assert!(source_slice(&doc, value_rm).starts_with(' '));
        assert!(source_slice(&doc, value_rm).contains("bar"));
        assert!(!source_slice(&doc, value_rm).contains('\n'));

        let (doc, np) = first_prop("* H\n:PROPERTIES:\n:FOO+: a\n:END:\n");
        assert_eq!(np.key(), "FOO+");
        assert_eq!(np.written_base_key(), "FOO");
        assert!(np.key_eq("FOO"));
        assert!(!np.key_eq("FOO+"));
        assert!(np.is_append());
        assert_eq!(source_slice(&doc, np.key_range()), "FOO+");

        let (_, np) = first_prop("* H\n:PROPERTIES:\n:FOO:\n:END:\n");
        assert_eq!(np.key(), "FOO");
        assert!(np.value().is_none());
        assert!(np.part_insert_point(PropertyPart::Value).is_some());
        crate::ast::check_part_contract(&np);

        let (_, spaced) = first_prop("* H\n:PROPERTIES:\n:FOO:   \n:END:\n");
        crate::ast::check_part_contract(&spaced);

        let (doc, np) = first_prop("* H\n:PROPERTIES:\n:FOO: bar   \n:END:\n");
        assert_eq!(np.value().as_deref(), Some("bar"));
        assert_eq!(source_slice(&doc, np.value_range().unwrap()), "bar");
    }

    #[test]
    fn get_is_caseless_last_match_and_does_not_strip_query() {
        let src = "* H\n:PROPERTIES:\n:FOO: a\n:foo: b\n:END:\n";
        let (_, d) = drawer(src);
        let hit = d.get("FOO").expect("foo");
        assert_eq!(hit.value().as_deref(), Some("b"));
        assert_eq!(d.get("foo").unwrap().value().as_deref(), Some("b"));
        assert!(d.get("FOO+").is_none());

        let (_, d) = drawer("* H\n:PROPERTIES:\n:FOO+: a\n:END:\n");
        assert_eq!(d.get("FOO").unwrap().value().as_deref(), Some("a"));
        assert!(d.get("FOO+").is_none());
    }

    #[test]
    fn get_and_document_property_share_case_and_differ_on_merge() {
        let src = "* H\n:PROPERTIES:\n:FOO: a\n:foo+: b\n:END:\n";
        let (doc, d) = drawer(src);
        let line = d.get("FOO").expect("line");
        assert_eq!(line.key(), "foo+");
        assert_eq!(line.value().as_deref(), Some("b"));
        let o = doc.root_outlines().next().unwrap();
        assert_eq!(
            doc.property(&o, "FOO").map(|p| p.value().to_string()),
            Some("a b".into())
        );
        assert!(doc.property(&o, "FOO+").is_none());
    }

    #[test]
    fn absent_value_insert_keeps_existing_spaces() {
        let src = "* H\n:PROPERTIES:\n:FOO:   \n:END:\n";
        let (doc, np) = first_prop(src);
        let ip = np.part_insert_point(PropertyPart::Value).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        let after = written(doc.source(), ip, "bar");
        assert!(
            after.contains(":FOO: bar   \n"),
            "spaces stay after the value: {after:?}"
        );
        assert!(
            !after.contains(":FOO:bar"),
            "must not glue the value: {after:?}"
        );

        let src = "* H\n:PROPERTIES:\n:FOO:\n:END:\n";
        let (doc, np) = first_prop(src);
        let ip = np.part_insert_point(PropertyPart::Value).unwrap();
        let after = written(doc.source(), ip, "bar");
        assert!(after.contains(":FOO: bar\n"), "{after:?}");
    }

    #[test]
    fn inheritance_provenance_against_node_property_surface() {
        let src = "* A\n:PROPERTIES:\n:FOO: a\n:END:\n** B\n:PROPERTIES:\n:BAR: b\n:END:\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();
        let local = b.property_drawer().unwrap().get("BAR").unwrap();
        assert_eq!(local.key(), "BAR");
        assert!(!local.is_append());
        assert_eq!(source_slice(&doc, local.key_range()), "BAR");
        assert_eq!(
            doc.property_inherited(&b, "FOO")
                .map(|(p, _)| p.value().to_string()),
            Some("a".into())
        );
    }

    #[test]
    fn new_property_insert_point_empty_and_non_empty() {
        let empty = "* H\n:PROPERTIES:\n:END:\n";
        let (_, d) = drawer(empty);
        let ip = d.new_property_insert_point();
        let body = drawer_body_range(d.syntax()).unwrap();
        assert_eq!(ip.offset(), body.start());

        let src = "* H\n:PROPERTIES:\n:FOO: 1\n:END:\n";
        let (_, d) = drawer(src);
        let last = d.properties().last().unwrap();
        assert_eq!(d.new_property_insert_point().offset(), last.range().end());
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::NODE_PROPERTY) {
                let np = NodeProperty::cast(node).unwrap();
                crate::ast::check_part_contract(&np);
            }
        }
    }

    #[test]
    fn malformed_properties_with_blank_interior_is_generic_drawer() {
        let doc = parse("* H\n:PROPERTIES:\n\n:END:\n");
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PROPERTY_DRAWER).is_empty(),
            "{:?}",
            nodes_of_kind(doc.root(), OrgSyntaxKind::PROPERTY_DRAWER)
        );
        assert!(
            !nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER).is_empty(),
            "{:?}",
            nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER)
        );
    }
}
