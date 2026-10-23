//! Child helpers, kind tables and the container walk shared by the views.
//!
//! The container walk yields bare [`SyntaxNode`]s; [`crate::ast::Element`]
//! types them. Affiliated-keyword runs are never yielded: a run is a sibling
//! placed before the element it belongs to.

use crate::ast::layout::eof_lead;
#[cfg(test)]
use crate::ast::layout::first_token_of_kind;
use crate::ast::{AstNode, InsertPoint, Padding};
use crate::syntax::{OrgSyntaxKind, SyntaxNode, SyntaxToken};
use crate::{TextRange, TextSize};
use rowan::TokenAtOffset;

pub(crate) use crate::ast::layout::field_dest;

/// The token a caret at `offset` belongs to: the left one where two meet.
///
/// `None` for an empty tree. Offsets past the end are clamped to the end,
/// where the last token is the answer.
#[must_use]
pub(crate) fn token_at_caret(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let range = root.text_range();
    if range.is_empty() {
        return None;
    }
    let o = offset.min(range.end());
    match root.token_at_offset(o) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(left, _) => Some(left),
    }
}

/// The token that owns byte `offset` (right token at a seam).
///
/// `None` when that byte does not exist (empty tree or `offset >= end`).
#[must_use]
pub(crate) fn token_owning_byte(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let range = root.text_range();
    if range.is_empty() || offset >= range.end() {
        return None;
    }
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(_, right) => Some(right),
    }
}

/// Walk to the syntax-tree root (the `DOCUMENT` node).
#[must_use]
pub(crate) fn syntax_root(node: &SyntaxNode) -> SyntaxNode {
    let mut n = node.clone();
    while let Some(parent) = n.parent() {
        n = parent;
    }
    n
}

/// Insert point at `offset` for a new construct: the lead padding end of
/// file needs, no trail.
#[must_use]
pub(crate) fn dest(syntax: &SyntaxNode, offset: TextSize) -> InsertPoint {
    InsertPoint::new(
        offset,
        eof_lead(&syntax_root(syntax), offset),
        Padding::None,
    )
}

/// After a present construct, else `fallback`.
#[must_use]
pub(crate) fn dest_after(
    syntax: &SyntaxNode,
    present: Option<TextRange>,
    fallback: InsertPoint,
) -> InsertPoint {
    match present {
        Some(r) => dest(syntax, r.end()),
        None => fallback,
    }
}

/// Org `:begin`..`:end`: the affiliated run (previous sibling) plus `node`.
#[must_use]
pub(crate) fn affiliated_extent(node: &SyntaxNode) -> TextRange {
    match node.prev_sibling() {
        Some(prev) if prev.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS => {
            TextRange::new(prev.text_range().start(), node.text_range().end())
        }
        _ => node.text_range(),
    }
}

/// First direct child that casts to `T`.
#[must_use]
pub(crate) fn child_view<T: AstNode>(parent: &SyntaxNode) -> Option<T> {
    parent.children().find_map(T::cast)
}

/// All direct children that cast to `T`, in source order.
pub(crate) fn child_views<'a, T: AstNode + 'a>(
    parent: &'a SyntaxNode,
) -> impl Iterator<Item = T> + 'a {
    parent.children().filter_map(T::cast)
}

/// Immediate previous node sibling, if it casts to `T`.
///
/// Does not skip mixed kinds: `PLAIN_LIST` children are consecutive `ITEM`
/// nodes, so a parent rescan is unnecessary. Parents with mixed children
/// (an `OUTLINE` under `DOCUMENT` may follow `SECTION`) get `None` when the
/// neighbour is the other kind, which is the sibling relation they want.
#[must_use]
pub(crate) fn prev_typed<T: AstNode>(node: &SyntaxNode) -> Option<T> {
    node.prev_sibling().and_then(T::cast)
}

/// Immediate next node sibling, if it casts to `T`. Same rule as [`prev_typed`].
#[must_use]
pub(crate) fn next_typed<T: AstNode>(node: &SyntaxNode) -> Option<T> {
    node.next_sibling().and_then(T::cast)
}

/// First direct child token of `kind`.
///
/// Same as [`crate::ast::layout::first_token_of_kind`].
#[cfg(test)]
#[must_use]
pub(crate) fn child_token(parent: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxToken> {
    first_token_of_kind(parent, kind)
}

/// First direct child **node** of `kind`.
#[must_use]
pub(crate) fn child_of_kind(parent: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxNode> {
    parent.children().find(|c| c.kind() == kind)
}

// ---------------------------------------------------------------------------
// Element, object and container kind tables
// ---------------------------------------------------------------------------

/// Whether `kind` is an element kind.
///
/// `AFFILIATED_KEYWORDS` is not: the walk skips the run.
/// `PLANNING` is outline/inline-task metadata, not an element.
/// `SECTION` / `OUTLINE` / `HEADING` are structure, not elements.
#[inline]
#[must_use]
pub(crate) fn is_element_kind(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::PARAGRAPH
            | OrgSyntaxKind::PLAIN_LIST
            | OrgSyntaxKind::TABLE
            | OrgSyntaxKind::DRAWER
            | OrgSyntaxKind::PROPERTY_DRAWER
            | OrgSyntaxKind::CENTER_BLOCK
            | OrgSyntaxKind::QUOTE_BLOCK
            | OrgSyntaxKind::SPECIAL_BLOCK
            | OrgSyntaxKind::DYNAMIC_BLOCK
            | OrgSyntaxKind::COMMENT_BLOCK
            | OrgSyntaxKind::EXAMPLE_BLOCK
            | OrgSyntaxKind::EXPORT_BLOCK
            | OrgSyntaxKind::SRC_BLOCK
            | OrgSyntaxKind::VERSE_BLOCK
            | OrgSyntaxKind::KEYWORD
            | OrgSyntaxKind::SETTING_KEYWORD
            | OrgSyntaxKind::COMMENT
            | OrgSyntaxKind::FIXED_WIDTH
            | OrgSyntaxKind::HORIZONTAL_RULE
            | OrgSyntaxKind::CLOCK
            | OrgSyntaxKind::BABEL_CALL
            | OrgSyntaxKind::INLINE_TASK
            | OrgSyntaxKind::FOOTNOTE_DEFINITION
            | OrgSyntaxKind::LATEX_ENVIRONMENT
            | OrgSyntaxKind::DIARY_SEXP
    )
}

/// Every element kind, for the container walk and the coverage tests.
#[cfg(test)]
#[must_use]
pub(crate) fn element_kinds() -> &'static [OrgSyntaxKind] {
    &[
        OrgSyntaxKind::PARAGRAPH,
        OrgSyntaxKind::PLAIN_LIST,
        OrgSyntaxKind::TABLE,
        OrgSyntaxKind::DRAWER,
        OrgSyntaxKind::PROPERTY_DRAWER,
        OrgSyntaxKind::CENTER_BLOCK,
        OrgSyntaxKind::QUOTE_BLOCK,
        OrgSyntaxKind::SPECIAL_BLOCK,
        OrgSyntaxKind::DYNAMIC_BLOCK,
        OrgSyntaxKind::COMMENT_BLOCK,
        OrgSyntaxKind::EXAMPLE_BLOCK,
        OrgSyntaxKind::EXPORT_BLOCK,
        OrgSyntaxKind::SRC_BLOCK,
        OrgSyntaxKind::VERSE_BLOCK,
        OrgSyntaxKind::KEYWORD,
        OrgSyntaxKind::SETTING_KEYWORD,
        OrgSyntaxKind::COMMENT,
        OrgSyntaxKind::FIXED_WIDTH,
        OrgSyntaxKind::HORIZONTAL_RULE,
        OrgSyntaxKind::CLOCK,
        OrgSyntaxKind::BABEL_CALL,
        OrgSyntaxKind::INLINE_TASK,
        OrgSyntaxKind::FOOTNOTE_DEFINITION,
        OrgSyntaxKind::LATEX_ENVIRONMENT,
        OrgSyntaxKind::DIARY_SEXP,
    ]
}

/// Whether `kind` is an object (inline) node.
///
/// [`object_kinds`] is the source of truth; this is membership in that table.
#[inline]
#[must_use]
pub(crate) fn is_object_kind(kind: OrgSyntaxKind) -> bool {
    object_kinds().contains(&kind)
}

/// Every object kind.
#[must_use]
pub(crate) fn object_kinds() -> &'static [OrgSyntaxKind] {
    &[
        OrgSyntaxKind::BOLD,
        OrgSyntaxKind::ITALIC,
        OrgSyntaxKind::UNDERLINE,
        OrgSyntaxKind::VERBATIM,
        OrgSyntaxKind::CODE,
        OrgSyntaxKind::STRIKE_THROUGH,
        OrgSyntaxKind::SUBSCRIPT,
        OrgSyntaxKind::SUPERSCRIPT,
        OrgSyntaxKind::ENTITY,
        OrgSyntaxKind::LATEX_FRAGMENT,
        OrgSyntaxKind::EXPORT_SNIPPET,
        OrgSyntaxKind::FOOTNOTE_REFERENCE,
        OrgSyntaxKind::INLINE_BABEL_CALL,
        OrgSyntaxKind::INLINE_SRC_BLOCK,
        OrgSyntaxKind::LINK,
        OrgSyntaxKind::RADIO_TARGET,
        OrgSyntaxKind::TARGET,
        OrgSyntaxKind::MACRO,
        OrgSyntaxKind::STATISTICS_COOKIE,
        OrgSyntaxKind::TIMESTAMP,
        OrgSyntaxKind::CITATION,
        OrgSyntaxKind::CITATION_REFERENCE,
        OrgSyntaxKind::LINE_BREAK,
    ]
}

/// Whether `kind` is an element container (a [`crate::ast::Container`]).
///
/// Of the blocks, only center, quote, special and dynamic blocks qualify: a
/// block's kind fixes its body, and verse, src, example, export and comment
/// bodies never hold elements. Admitting them would make `elements()` answer
/// `[]` for a block that cannot have elements, which a caller could not tell
/// from an empty quote block, and there would be nowhere to insert one.
///
/// Every kind here is also an Org greater element. The converse fails:
/// `PLAIN_LIST`, `TABLE` and `PROPERTY_DRAWER` are greater elements whose
/// children are `ITEM` / `TABLE_ROW` / `NODE_PROPERTY`, not elements.
#[inline]
#[must_use]
pub(crate) fn is_container_kind(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::SECTION
            | OrgSyntaxKind::DRAWER
            | OrgSyntaxKind::CENTER_BLOCK
            | OrgSyntaxKind::QUOTE_BLOCK
            | OrgSyntaxKind::SPECIAL_BLOCK
            | OrgSyntaxKind::DYNAMIC_BLOCK
            | OrgSyntaxKind::ITEM
            | OrgSyntaxKind::FOOTNOTE_DEFINITION
            | OrgSyntaxKind::INLINE_TASK
    )
}

/// Every container kind; [`is_container_kind`] tests membership.
#[cfg(test)]
#[must_use]
pub(crate) fn container_kinds() -> &'static [OrgSyntaxKind] {
    &[
        OrgSyntaxKind::SECTION,
        OrgSyntaxKind::DRAWER,
        OrgSyntaxKind::CENTER_BLOCK,
        OrgSyntaxKind::QUOTE_BLOCK,
        OrgSyntaxKind::SPECIAL_BLOCK,
        OrgSyntaxKind::DYNAMIC_BLOCK,
        OrgSyntaxKind::ITEM,
        OrgSyntaxKind::FOOTNOTE_DEFINITION,
        OrgSyntaxKind::INLINE_TASK,
    ]
}

// ---------------------------------------------------------------------------
// Body-element predicate, shared by the walk and Element::cast
// ---------------------------------------------------------------------------

/// Whether `node` is a **body element** of its parent.
///
/// The container walk, the sibling helpers and [`crate::ast::Element::cast`]
/// all use this, so they cannot disagree.
///
/// Exclusions by parent (the kind still counts in [`is_element_kind`]):
/// - `DIARY_SEXP` under `TIMESTAMP`: part of a diary timestamp.
/// - `PROPERTY_DRAWER` under `OUTLINE` or `INLINE_TASK`: the entry's
///   properties, not a body element. Under `SECTION` the file-level property
///   drawer is a body element.
/// - `KEYWORD` under `AFFILIATED_KEYWORDS`: a run member belongs to the run,
///   which belongs to the element after it. Reach it through
///   `AffiliatedKeywords::keywords()`.
/// - `KEYWORD` under `TABLE`: a `#+TBLFM:` line is part of the table.
///
/// The list names only shapes the parser produces;
/// `no_element_has_a_non_container_parent` in `ast/tests.rs` fails if a new
/// one appears.
#[inline]
#[must_use]
pub(crate) fn is_body_element(node: &SyntaxNode) -> bool {
    let kind = node.kind();
    if !is_element_kind(kind) {
        return false;
    }
    match kind {
        OrgSyntaxKind::DIARY_SEXP => node
            .parent()
            .is_none_or(|p| p.kind() != OrgSyntaxKind::TIMESTAMP),
        OrgSyntaxKind::PROPERTY_DRAWER => node.parent().is_none_or(|p| {
            !matches!(
                p.kind(),
                OrgSyntaxKind::OUTLINE | OrgSyntaxKind::INLINE_TASK
            )
        }),
        OrgSyntaxKind::KEYWORD => node.parent().is_none_or(|p| {
            !matches!(
                p.kind(),
                OrgSyntaxKind::AFFILIATED_KEYWORDS | OrgSyntaxKind::TABLE
            )
        }),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Container walk
// ---------------------------------------------------------------------------

/// Direct body-element children of `container`, in source order.
///
/// Filters with [`is_body_element`] and does not descend into nested
/// containers. [`crate::ast::HasElements::elements`] is this walk, typed.
pub(crate) fn element_children(container: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + '_ {
    container.children().filter(is_body_element)
}

/// Extent of direct body-element children, affiliated run included at start.
///
/// Affiliation only moves the start, so the end is the last element's end.
/// `None` when there are no body elements; each caller decides where its
/// empty body sits.
#[must_use]
pub(crate) fn element_run_range(parent: &SyntaxNode) -> Option<TextRange> {
    let mut start = None;
    let mut end = None;
    for el in element_children(parent) {
        if start.is_none() {
            start = Some(affiliated_extent(&el).start());
        }
        end = Some(el.text_range().end());
    }
    Some(TextRange::new(start?, end?))
}

/// Next body-element sibling after `node`, skipping affiliated runs and
/// metadata siblings (same predicate as [`element_children`]).
#[must_use]
pub(crate) fn next_element_sibling(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut sib = node.next_sibling();
    while let Some(n) = sib {
        if n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS {
            sib = n.next_sibling();
            continue;
        }
        if is_body_element(&n) {
            return Some(n);
        }
        // Keep scanning: planning / metadata property drawers under an
        // inline task must not hide a later body element.
        sib = n.next_sibling();
    }
    None
}

/// Previous body-element sibling before `node`, skipping affiliated runs and
/// metadata siblings (same predicate as [`element_children`]).
#[must_use]
pub(crate) fn prev_element_sibling(node: &SyntaxNode) -> Option<SyntaxNode> {
    // Walk left. An affiliated run immediately before an element belongs to
    // *that* element (the next one), so when we land on a run we skip it and
    // keep looking for the previous body-element target.
    let mut sib = node.prev_sibling();
    while let Some(n) = sib {
        if n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS {
            sib = n.prev_sibling();
            continue;
        }
        if is_body_element(&n) {
            return Some(n);
        }
        sib = n.prev_sibling();
    }
    None
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::parse;
    use crate::tests::nodes_of_kind;

    #[test]
    fn element_run_range_empty_parent_is_none() {
        let doc = parse("-\n");
        let item = nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
            .into_iter()
            .next()
            .expect("item");
        assert_eq!(element_run_range(&item), None);

        let doc = parse("[fn:1]\n");
        let def = nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_DEFINITION)
            .into_iter()
            .next()
            .expect("fn");
        assert_eq!(element_run_range(&def), None);
    }

    #[test]
    fn element_run_range_start_includes_affiliated_run() {
        let src = "#+CAPTION: c\nparagraph\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let run = section
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS)
            .expect("run");
        let para = element_children(&section).next().expect("paragraph");
        let r = element_run_range(&section).expect("run");
        assert_eq!(r.start(), run.text_range().start());
        assert!(r.start() < para.text_range().start());
        assert_eq!(r.end(), para.text_range().end());
    }

    #[test]
    fn element_run_range_start_is_first_even_when_last_is_affiliated() {
        // Caption on the last child: start must stay the first element's,
        // not the last affiliated run.
        let src = "one\n\n#+CAPTION: c\ntwo\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let kids: Vec<_> = element_children(&section).collect();
        assert_eq!(kids.len(), 2);
        let r = element_run_range(&section).expect("run");
        assert_eq!(r.start(), affiliated_extent(&kids[0]).start());
        assert_eq!(r.start(), kids[0].text_range().start());
        let last_run = section
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS)
            .expect("caption on last");
        assert!(r.start() < last_run.text_range().start());
        assert_eq!(r.end(), kids[1].text_range().end());
        assert_eq!(r.end(), affiliated_extent(&kids[1]).end());
    }

    #[test]
    fn prev_next_typed_are_immediate_siblings() {
        use crate::ast::Item;
        let src = "- a\n- b\n- c\n";
        let doc = parse(src);
        let items = nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM);
        assert_eq!(items.len(), 3);
        assert!(prev_typed::<Item>(&items[0]).is_none());
        assert_eq!(
            prev_typed::<Item>(&items[1]).as_ref().map(AstNode::syntax),
            Some(&items[0])
        );
        assert_eq!(
            next_typed::<Item>(&items[1]).as_ref().map(AstNode::syntax),
            Some(&items[2])
        );
        assert!(next_typed::<Item>(&items[2]).is_none());
    }

    #[test]
    fn prev_next_typed_do_not_skip_mixed_kinds() {
        use crate::ast::{Item, Paragraph};
        let src = "before\n\n- item\n\nafter\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let paras: Vec<_> = section
            .children()
            .filter(|n| n.kind() == OrgSyntaxKind::PARAGRAPH)
            .collect();
        assert_eq!(paras.len(), 2);
        assert_eq!(
            paras[0].next_sibling().map(|n| n.kind()),
            Some(OrgSyntaxKind::PLAIN_LIST)
        );
        assert!(next_typed::<Paragraph>(&paras[0]).is_none());
        assert!(prev_typed::<Paragraph>(&paras[1]).is_none());
        let item = nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
            .into_iter()
            .next()
            .expect("item");
        assert!(prev_typed::<Item>(&item).is_none());
        assert!(next_typed::<Item>(&item).is_none());
    }

    #[test]
    fn element_children_skips_affiliated_run() {
        let src = "#+CAPTION: c\nparagraph\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let kids: Vec<_> = element_children(&section).collect();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].kind(), OrgSyntaxKind::PARAGRAPH);
        assert!(
            section
                .children()
                .any(|n| n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS),
            "fixture must emit an affiliated run"
        );
    }

    #[test]
    fn is_body_element_property_drawer_parent_aware() {
        let task_src =
            "*************** TASK\n:PROPERTIES:\n:ID: x\n:END:\nbody\n*************** END\n";
        let task_doc = parse(task_src);
        let task = nodes_of_kind(task_doc.root(), OrgSyntaxKind::INLINE_TASK)
            .into_iter()
            .next()
            .unwrap();
        let drawer = task
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
            .unwrap();
        assert!(!is_body_element(&drawer));
        let body = task
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::PARAGRAPH)
            .unwrap();
        assert!(is_body_element(&body));

        let sec_doc = parse(":PROPERTIES:\n:ID: y\n:END:\n");
        let section = nodes_of_kind(sec_doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .unwrap();
        let sec_drawer = section
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
            .unwrap();
        assert!(is_body_element(&sec_drawer));
    }

    #[test]
    fn element_children_is_direct_only() {
        let src = "#+begin_quote\ninner\n#+end_quote\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let kids: Vec<_> = element_children(&section).collect();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].kind(), OrgSyntaxKind::QUOTE_BLOCK);
        // Inner paragraph is not a direct section child.
        assert!(!kids.iter().any(|n| n.kind() == OrgSyntaxKind::PARAGRAPH));
    }

    #[test]
    fn next_prev_element_skip_affiliated() {
        let src = "one\n\n#+CAPTION: c\ntwo\n";
        let doc = parse(src);
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let kids: Vec<_> = element_children(&section).collect();
        assert_eq!(kids.len(), 2);
        let a = &kids[0];
        let b = &kids[1];
        assert_eq!(
            next_element_sibling(a).map(|n| n.text_range()),
            Some(b.text_range())
        );
        assert_eq!(
            prev_element_sibling(b).map(|n| n.text_range()),
            Some(a.text_range())
        );
    }

    #[test]
    fn child_token_finds_star() {
        let doc = parse("* T\n");
        let heading = nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .expect("heading");
        let star = child_token(&heading, OrgSyntaxKind::STAR).expect("star");
        assert_eq!(star.text(), "*");
    }

    #[test]
    fn child_of_kind_finds_keyword_key() {
        let doc = parse("#+TITLE: x\n");
        let kw = nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
            .into_iter()
            .next()
            .expect("keyword");
        let key = child_of_kind(&kw, OrgSyntaxKind::KEYWORD_KEY).expect("key");
        assert_eq!(key.text().to_string(), "TITLE");
        assert!(child_of_kind(&kw, OrgSyntaxKind::KEYWORD_DUAL).is_none());
    }

    #[test]
    fn kind_tables_are_bidirectional() {
        // Element / container: table is a subset of the predicate, and every
        // node kind that matches the predicate is in the table. Object:
        // `object_kinds` *is* the predicate (`is_object_kind` is membership);
        // the restriction-union test pins the table to the grammar.
        for &k in element_kinds() {
            assert!(is_element_kind(k), "{k:?}");
        }
        for &k in object_kinds() {
            assert!(is_object_kind(k), "{k:?}");
        }
        for &k in container_kinds() {
            assert!(is_container_kind(k), "{k:?}");
        }
        for raw in 0..OrgSyntaxKind::__LAST as u16 {
            let k = <crate::OrgLanguage as rowan::Language>::kind_from_raw(rowan::SyntaxKind(raw));
            if !k.is_node() {
                continue;
            }
            if is_element_kind(k) {
                assert!(
                    element_kinds().contains(&k),
                    "{k:?} is_element_kind but not in element_kinds()"
                );
            }
            if is_object_kind(k) {
                assert!(
                    object_kinds().contains(&k),
                    "{k:?} is_object_kind but not in object_kinds()"
                );
            }
            if is_container_kind(k) {
                assert!(
                    container_kinds().contains(&k),
                    "{k:?} is_container_kind but not in container_kinds()"
                );
            }
        }
        assert!(!is_element_kind(OrgSyntaxKind::AFFILIATED_KEYWORDS));
        assert!(!is_element_kind(OrgSyntaxKind::PLANNING));
        assert!(!is_element_kind(OrgSyntaxKind::HEADING));
        // Container mode is kind-determined: only the four container-mode
        // block kinds are containers.
        assert!(is_container_kind(OrgSyntaxKind::QUOTE_BLOCK));
        assert!(is_container_kind(OrgSyntaxKind::CENTER_BLOCK));
        assert!(is_container_kind(OrgSyntaxKind::SPECIAL_BLOCK));
        assert!(is_container_kind(OrgSyntaxKind::DYNAMIC_BLOCK));
        assert!(!is_container_kind(OrgSyntaxKind::SRC_BLOCK));
        assert!(!is_container_kind(OrgSyntaxKind::EXAMPLE_BLOCK));
        assert!(!is_container_kind(OrgSyntaxKind::EXPORT_BLOCK));
        assert!(!is_container_kind(OrgSyntaxKind::COMMENT_BLOCK));
        assert!(!is_container_kind(OrgSyntaxKind::VERSE_BLOCK));
        // Every container kind is an Org greater element; the converse fails
        // for kinds whose children are not elements.
        assert!(!is_container_kind(OrgSyntaxKind::PLAIN_LIST));
        assert!(!is_container_kind(OrgSyntaxKind::TABLE));
        assert!(!is_container_kind(OrgSyntaxKind::PROPERTY_DRAWER));
    }

    #[test]
    fn keyword_dual_use_is_parent_aware() {
        // A run member and a `#+TBLFM:` line are keywords that belong to their
        // parent construct, not body elements of a container.
        let doc = parse("#+CAPTION: c\npara\n");
        let run = nodes_of_kind(doc.root(), OrgSyntaxKind::AFFILIATED_KEYWORDS)
            .into_iter()
            .next()
            .expect("run");
        let member = run
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::KEYWORD)
            .expect("run member");
        assert!(!is_body_element(&member), "run member is not an element");

        let doc = parse("| a |\n#+TBLFM: $1=1\n");
        let table = nodes_of_kind(doc.root(), OrgSyntaxKind::TABLE)
            .into_iter()
            .next()
            .expect("table");
        let tblfm = table
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::KEYWORD)
            .expect("tblfm");
        assert!(!is_body_element(&tblfm), "#+TBLFM: belongs to the table");

        // A free-standing keyword in a section is still an element.
        let doc = parse("#+TITLE: t\n");
        let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .expect("section");
        let kw = section
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::KEYWORD)
            .expect("keyword");
        assert!(is_body_element(&kw), "section-level keyword is an element");
    }

    #[test]
    fn child_view_and_views_find_heading_title() {
        use crate::ast::HeadingTitle;
        let doc = parse("* Title\n");
        let heading = nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .expect("heading");
        let title = child_view::<HeadingTitle>(&heading).expect("title");
        assert_eq!(title.kind(), OrgSyntaxKind::HEADING_TITLE);
        let titles: Vec<_> = child_views::<HeadingTitle>(&heading).collect();
        assert_eq!(titles.len(), 1);
    }

    #[test]
    fn token_at_caret_empty_tree_is_none() {
        let doc = parse("");
        assert!(token_at_caret(doc.root(), TextSize::from(0)).is_none());
        assert!(token_owning_byte(doc.root(), TextSize::from(0)).is_none());
    }

    #[test]
    fn token_at_caret_eof_is_last_token_not_end_minus_one() {
        // POST punctuation so the markup actually carves; last token is one byte.
        let src = "*bold*!";
        let doc = parse(src);
        let end = TextSize::of(src);
        let tok = token_at_caret(doc.root(), end).expect("last token");
        assert_eq!(tok.text(), "!");
        assert_eq!(
            tok.parent().map(|n| n.kind()),
            Some(OrgSyntaxKind::PARAGRAPH)
        );
        assert!(token_owning_byte(doc.root(), end).is_none());
    }

    #[test]
    fn token_at_caret_between_takes_left() {
        let src = "*bold*!";
        let doc = parse(src);
        let bang = src.find('!').expect("!") as u32;
        let tok = token_at_caret(doc.root(), TextSize::from(bang)).expect("left");
        assert_eq!(tok.text(), "*");
        assert_eq!(tok.parent().map(|n| n.kind()), Some(OrgSyntaxKind::BOLD));
        let owned = token_owning_byte(doc.root(), TextSize::from(bang)).expect("right");
        assert_eq!(owned.text(), "!");
        let end = TextSize::of(src);
        assert_eq!(
            token_at_caret(doc.root(), end + TextSize::from(1))
                .as_ref()
                .map(rowan::SyntaxToken::text),
            token_at_caret(doc.root(), end)
                .as_ref()
                .map(rowan::SyntaxToken::text)
        );
        let para = parse("para");
        let mid = TextSize::from(2);
        let tok = token_owning_byte(para.root(), mid).expect("mid");
        assert_eq!(
            tok.parent().map(|n| n.kind()),
            Some(OrgSyntaxKind::PARAGRAPH)
        );
        assert_eq!(tok.kind(), OrgSyntaxKind::TEXT);
    }

    #[test]
    fn token_at_caret_eof_word_after_markup_is_paragraph() {
        // `x` is not a legal markup POST, so this is paragraph text throughout.
        let src = "*bold*x";
        let doc = parse(src);
        let end = TextSize::of(src);
        let tok = token_at_caret(doc.root(), end).expect("last");
        assert_eq!(
            tok.parent().map(|n| n.kind()),
            Some(OrgSyntaxKind::PARAGRAPH)
        );
    }
}
