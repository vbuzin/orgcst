//! Walks from any node: its ancestors, the nearest enclosing view of a type,
//! and typed pre-order descendants. This module is not re-exported at the
//! crate root.
//!
//! Walks between views are methods on the views. [`crate::Outline`] has
//! `parent`, `children`, `prev_sibling`, `next_sibling`, `ancestors` and
//! `descendants`; [`crate::Item`] has `parent_item`, `prev_sibling`,
//! `next_sibling` and `sublists`; elements move with
//! [`crate::ElementView::next_element`], [`crate::ElementView::prev_element`]
//! and [`crate::ElementView::container`].
//!
//! ```
//! use orgcst::parse;
//!
//! let doc = parse("* Parent\n** Child\n");
//! let parent = doc
//!     .headings()
//!     .next()
//!     .expect("the fixture has a heading")
//!     .outline();
//! let child = parent
//!     .children()
//!     .next()
//!     .expect("one child outline");
//! assert_eq!(child.level(), 2);
//! assert_eq!(child.parent().map(|outline| outline.level()), Some(1));
//! ```

use crate::ast::{AstNode, Container, Element, Heading, InlineTask, Object, Outline};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use std::marker::PhantomData;

/// Walk parent links, excluding `node` itself.
pub fn ancestors(node: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + '_ {
    std::iter::successors(node.parent(), SyntaxNode::parent)
}

/// Nearest ancestor that casts to `T`.
#[must_use]
pub fn enclosing<T: AstNode>(node: &SyntaxNode) -> Option<T> {
    ancestors(node).find_map(T::cast)
}

/// Nearest enclosing outline.
#[must_use]
pub fn enclosing_outline(node: &SyntaxNode) -> Option<Outline> {
    enclosing(node)
}

/// Nearest enclosing element.
///
/// Inside an affiliated-keyword run this is the element the run attaches to,
/// not the run's container.
#[must_use]
pub fn enclosing_element(node: &SyntaxNode) -> Option<Element> {
    if node.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS {
        return node.next_sibling().and_then(Element::cast);
    }
    if let Some(run) = ancestors(node).find(|n| n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS) {
        return run.next_sibling().and_then(Element::cast);
    }
    enclosing::<Element>(node)
}

/// Where a covering walk starts: the element an affiliated run attaches to
/// when `start` is inside the run, otherwise `start` itself.
#[must_use]
pub fn affiliated_cover_seed(start: &SyntaxNode) -> SyntaxNode {
    let in_run = start.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS
        || ancestors(start).any(|n| n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS);
    if in_run && let Some(el) = enclosing_element(start) {
        return el.syntax().clone();
    }
    start.clone()
}

/// Nearest enclosing element container.
#[must_use]
pub fn enclosing_container(node: &SyntaxNode) -> Option<Container> {
    enclosing(node)
}

/// Outlines strictly above this node.
///
/// For an outline or heading, the parent outline and its ancestors; for an
/// inline task, the enclosing outline and its ancestors. Any other node
/// yields nothing. These are the outlines properties and tags are inherited
/// from.
pub fn inherited_outlines(syntax: &SyntaxNode) -> impl Iterator<Item = Outline> + '_ {
    std::iter::successors(first_inherited_outline(syntax), Outline::parent)
}

fn first_inherited_outline(syntax: &SyntaxNode) -> Option<Outline> {
    if let Some(outline) = Outline::cast(syntax.clone()) {
        return outline.parent();
    }
    if let Some(heading) = Heading::cast(syntax.clone()) {
        return heading.outline().parent();
    }
    InlineTask::cast(syntax.clone()).and_then(|_| enclosing_outline(syntax))
}

/// Nearest enclosing object.
#[must_use]
pub fn enclosing_object(node: &SyntaxNode) -> Option<Object> {
    enclosing(node)
}

/// Pre-order descendants, including `node` when it casts to `T`.
///
/// The walk does not descend into nodes whose kind is in `no_recurse`, but
/// still yields them when they cast. For example, only the outermost lists
/// are `descendants::<PlainList>(root, &[OrgSyntaxKind::PLAIN_LIST])`.
pub fn descendants<'a, T: AstNode + 'a>(
    node: &'a SyntaxNode,
    no_recurse: &'a [OrgSyntaxKind],
) -> impl Iterator<Item = T> + 'a {
    Descendants {
        stack: vec![node.clone()],
        no_recurse,
        _t: PhantomData,
    }
}

struct Descendants<'a, T> {
    stack: Vec<SyntaxNode>,
    no_recurse: &'a [OrgSyntaxKind],
    _t: PhantomData<fn() -> T>,
}

impl<T: AstNode> Iterator for Descendants<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        while let Some(node) = self.stack.pop() {
            if !self.no_recurse.contains(&node.kind()) {
                let mut children: Vec<_> = node.children().collect();
                children.reverse();
                self.stack.extend(children);
            }
            if let Some(t) = T::cast(node) {
                return Some(t);
            }
        }
        None
    }
}

/// First pre-order descendant of `T` (including `node`) matching `pred`.
#[must_use]
pub fn first<T: AstNode>(node: &SyntaxNode, mut pred: impl FnMut(&T) -> bool) -> Option<T> {
    descendants(node, &[]).find(|t| pred(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn nav_enclosing_outline() {
        let doc = parse("* A\n** B\n");
        let b = doc
            .root_outlines()
            .next()
            .unwrap()
            .children()
            .next()
            .unwrap();
        let heading = b.heading();
        let enc = enclosing_outline(heading.syntax()).unwrap();
        assert_eq!(enc.syntax(), b.syntax());
    }

    #[test]
    fn nav_inherited_outlines() {
        use crate::syntax::OrgSyntaxKind;
        use crate::tests::nodes_of_kind;

        let doc = parse("* A\n** B\n*************** T\n*************** END\n");
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();

        assert!(inherited_outlines(a.syntax()).next().is_none());
        assert!(inherited_outlines(a.heading().syntax()).next().is_none());

        let from_b: Vec<_> = inherited_outlines(b.syntax()).collect();
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].syntax(), a.syntax());
        let from_heading: Vec<_> = inherited_outlines(b.heading().syntax()).collect();
        assert_eq!(from_heading[0].syntax(), a.syntax());

        let task = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let from_task: Vec<_> = inherited_outlines(task.syntax()).collect();
        assert_eq!(from_task.len(), 2);
        assert_eq!(from_task[0].syntax(), b.syntax());
        assert_eq!(from_task[1].syntax(), a.syntax());
    }

    #[test]
    fn enclosing_element_from_paragraph_and_affiliated_run() {
        let doc = parse("#+CAPTION: c\n*bold*\n");
        let para = crate::ast::Paragraph::cast(
            crate::tests::nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let bold = crate::tests::nodes_of_kind(para.syntax(), OrgSyntaxKind::BOLD)
            .into_iter()
            .next()
            .expect("object inside paragraph");
        let enc = enclosing_element(&bold).unwrap();
        assert_eq!(enc.syntax(), para.syntax());

        let run = para.syntax().prev_sibling().unwrap();
        assert_eq!(run.kind(), OrgSyntaxKind::AFFILIATED_KEYWORDS);
        let from_run = enclosing_element(&run).unwrap();
        assert_eq!(from_run.syntax(), para.syntax());

        let caption = run.children().next().unwrap();
        assert_eq!(caption.kind(), OrgSyntaxKind::KEYWORD);
        let from_member = enclosing_element(&caption).unwrap();
        assert_eq!(from_member.syntax(), para.syntax());

        let container = enclosing_container(para.syntax()).unwrap();
        assert_eq!(container.kind(), OrgSyntaxKind::SECTION);
    }

    #[test]
    fn enclosing_element_from_attr_keyword() {
        let doc = parse("#+ATTR_HTML: :class x\npara\n");
        let para = crate::ast::Paragraph::cast(
            crate::tests::nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let run = para.syntax().prev_sibling().unwrap();
        let attr = run.children().next().unwrap();
        let enc = enclosing_element(&attr).unwrap();
        assert_eq!(enc.syntax(), para.syntax());
    }

    #[test]
    fn enclosing_object_from_nested_markup() {
        let doc = parse("*/inner/*\n");
        let italic = crate::tests::nodes_of_kind(doc.root(), OrgSyntaxKind::ITALIC)
            .into_iter()
            .next()
            .expect("italic");
        let enc = enclosing_object(&italic).unwrap();
        assert_eq!(enc.kind(), OrgSyntaxKind::BOLD);
        assert!(enclosing_object(&enc.syntax().clone()).is_none());
    }

    #[test]
    fn descendants_include_self_and_no_recurse_skips_nested_lists() {
        let src = "- a\n  - nested\n- b\n";
        let doc = parse(src);
        let lists: Vec<_> = descendants::<crate::ast::PlainList>(doc.root(), &[]).collect();
        assert_eq!(lists.len(), 2, "nested list is a descendant");
        let outer = lists[0].clone();
        let nested = lists[1].clone();
        let from_outer: Vec<_> =
            descendants::<crate::ast::PlainList>(outer.syntax(), &[]).collect();
        assert_eq!(from_outer[0].syntax(), outer.syntax());
        assert_eq!(from_outer.len(), 2);
        assert_eq!(from_outer[1].syntax(), nested.syntax());
        let no_nested: Vec<_> =
            descendants::<crate::ast::PlainList>(outer.syntax(), &[OrgSyntaxKind::PLAIN_LIST])
                .collect();
        assert_eq!(no_nested.len(), 1);
        assert_eq!(no_nested[0].syntax(), outer.syntax());
        assert_eq!(
            first::<crate::ast::PlainList>(doc.root(), |l| l.syntax() == nested.syntax())
                .as_ref()
                .map(AstNode::syntax),
            Some(nested.syntax())
        );
        let doc_first = doc.first::<crate::ast::PlainList>(|l| l.syntax() == nested.syntax());
        assert_eq!(
            doc_first.as_ref().map(AstNode::syntax),
            Some(nested.syntax())
        );
    }
}
