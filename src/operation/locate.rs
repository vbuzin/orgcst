//! [`NodeRef`] re-resolution and insert-payload locating.

use crate::ast::{AstNode, Heading, InlineTask, InsertPoint, Outline, Task};
use crate::edit::{Anchor, Bias, EditSet};
use crate::writer::padding_str;
use crate::{Document, OperationError, OrgSyntaxKind, SyntaxNode, TextSize};

/// Kinds accepted by headline field ops and by task metadata ops.
///
/// `HEADING` resolves to its parent outline, which holds the metadata.
pub(super) const TASK_KINDS: &[OrgSyntaxKind] = &[
    OrgSyntaxKind::HEADING,
    OrgSyntaxKind::INLINE_TASK,
    OrgSyntaxKind::OUTLINE,
];

/// Kinds accepted by structural outline ops. `HEADING` resolves to its
/// outline; an inline task is rejected with `WrongKind`.
pub(super) const STRUCTURAL_KINDS: &[OrgSyntaxKind] =
    &[OrgSyntaxKind::OUTLINE, OrgSyntaxKind::HEADING];

/// Where to insert, move or clone an outline.
///
/// Each variant resolves to an insert point the outline views already
/// expose; compile does not compute offsets of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtreeDest {
    /// Before the outline, as a sibling at its level.
    Before(NodeRef),
    /// After the outline and its subtree, as a sibling at its level.
    After(NodeRef),
    /// As the first child of the outline.
    FirstChildOf(NodeRef),
    /// At the end of the outline's content, as its last child.
    ContentEndOf(NodeRef),
    /// Before the first outline of the document, at level 1.
    DocumentStart,
    /// After the last outline of the document, at level 1.
    DocumentEnd,
}

/// Resolved insert point, the level a subtree takes there, and the outline
/// the variant named (`DocumentStart` / `DocumentEnd` name none).
pub(super) struct SubtreeInsert {
    pub point: InsertPoint,
    pub level: usize,
    pub outline: Option<Outline>,
}

/// Resolve `target` to a [`Task`]. `HEADING` stands for its outline.
pub(super) fn resolve_task(doc: &Document, target: NodeRef) -> Result<Task, OperationError> {
    let node = target.resolve(doc).ok_or(OperationError::StaleTarget)?;
    match node.kind() {
        OrgSyntaxKind::HEADING => {
            let heading = Heading::cast(node).expect("grammar: HEADING kind always casts");
            Ok(Task::from(heading.outline()))
        }
        OrgSyntaxKind::OUTLINE => {
            let outline = Outline::cast(node).expect("grammar: OUTLINE kind always casts");
            Ok(Task::from(outline))
        }
        OrgSyntaxKind::INLINE_TASK => {
            let task = InlineTask::cast(node).expect("grammar: INLINE_TASK kind always casts");
            Ok(Task::from(task))
        }
        actual => Err(OperationError::WrongKind {
            expected: TASK_KINDS,
            actual,
        }),
    }
}

/// Resolve `target` to a view of kind `T`. No node of the exact kind is
/// [`OperationError::StaleTarget`]; one that does not cast to `T` is
/// [`OperationError::WrongKind`].
pub(super) fn resolve_view<T: AstNode>(
    doc: &Document,
    target: NodeRef,
    expected: &'static [OrgSyntaxKind],
) -> Result<T, OperationError> {
    let node = target.resolve(doc).ok_or(OperationError::StaleTarget)?;
    let actual = node.kind();
    T::cast(node).ok_or(OperationError::WrongKind { expected, actual })
}

/// Resolve `target` to an [`Outline`]. `HEADING` resolves to its parent outline.
pub(super) fn resolve_outline(doc: &Document, target: NodeRef) -> Result<Outline, OperationError> {
    let node = target.resolve(doc).ok_or(OperationError::StaleTarget)?;
    match node.kind() {
        OrgSyntaxKind::HEADING => {
            let heading = Heading::cast(node).expect("grammar: HEADING kind always casts");
            Ok(heading.outline())
        }
        OrgSyntaxKind::OUTLINE => {
            Ok(Outline::cast(node).expect("grammar: OUTLINE kind always casts"))
        }
        actual => Err(OperationError::WrongKind {
            expected: STRUCTURAL_KINDS,
            actual,
        }),
    }
}

/// Resolve a destination to an insert point and level. Outline variants need
/// a live outline; document start and end do not.
pub(super) fn resolve_subtree_dest(
    doc: &Document,
    dest: SubtreeDest,
) -> Result<SubtreeInsert, OperationError> {
    match dest {
        SubtreeDest::Before(target) => {
            let outline = resolve_outline(doc, target)?;
            Ok(SubtreeInsert {
                point: outline.insert_before(),
                level: outline.level(),
                outline: Some(outline),
            })
        }
        SubtreeDest::After(target) => {
            let outline = resolve_outline(doc, target)?;
            Ok(SubtreeInsert {
                point: outline.insert_after(),
                level: outline.level(),
                outline: Some(outline),
            })
        }
        SubtreeDest::FirstChildOf(target) => {
            let outline = resolve_outline(doc, target)?;
            Ok(SubtreeInsert {
                point: outline.first_child_insert_point(),
                level: outline.level() + 1,
                outline: Some(outline),
            })
        }
        SubtreeDest::ContentEndOf(target) => {
            let outline = resolve_outline(doc, target)?;
            Ok(SubtreeInsert {
                point: outline.content_end_insert_point(),
                level: outline.level() + 1,
                outline: Some(outline),
            })
        }
        SubtreeDest::DocumentStart => Ok(SubtreeInsert {
            point: doc.first_outline_insert_point(),
            level: 1,
            outline: None,
        }),
        SubtreeDest::DocumentEnd => Ok(SubtreeInsert {
            point: doc.last_outline_insert_point(),
            level: 1,
            outline: None,
        }),
    }
}

/// A handle that finds a node again: an [`Anchor`] plus the exact syntax kind.
///
/// A `NodeRef` is a position, not node identity. After an edit,
/// [`apply_locating`] maps the anchor and looks for a node of the same kind
/// there; if a pure deletion covered the anchor it returns `None` rather than
/// a neighbour of the same kind.
///
/// [`Self::of`] anchors at the construct start with [`Bias::Right`], so an
/// in-place non-empty rewrite of the construct maps the anchor to the end of
/// the replacement, which may miss or land on the next sibling. After
/// [`crate::Operation::InsertHeading`], [`crate::Operation::MoveSubtree`] or
/// [`crate::Operation::CloneSubtree`], find the new heading with
/// [`payload_start`]. After a clone the source handle still resolves to the
/// original, not the copy.
///
/// `anchor.bias` only controls mapping through an [`EditSet`];
/// [`Self::resolve`] ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeRef {
    /// Position of the node, with the bias used when mapping it through edits.
    pub anchor: Anchor,
    /// Exact syntax kind of the node.
    pub kind: OrgSyntaxKind,
}

impl NodeRef {
    /// A handle at the start of `node`'s construct range, biased right.
    ///
    /// Text inserted at that start keeps the handle on this node. A pure
    /// deletion covering the offset makes [`apply_locating`] return `None`.
    /// An in-place non-empty rewrite maps the handle to the end of the
    /// replacement.
    #[must_use]
    pub fn of(node: &impl AstNode) -> Self {
        Self {
            anchor: Anchor {
                offset: node.range().start(),
                bias: Bias::Right,
            },
            kind: node.kind(),
        }
    }

    /// A right-biased handle at `offset` for a node of exactly `kind`.
    #[must_use]
    pub fn at(offset: TextSize, kind: OrgSyntaxKind) -> Self {
        Self {
            anchor: Anchor {
                offset,
                bias: Bias::Right,
            },
            kind,
        }
    }

    /// Innermost node of exactly `self.kind` containing the anchor offset.
    ///
    /// The byte at the offset decides (the token to the right where two
    /// tokens meet), so [`Self::of`] round-trips for a construct that does not
    /// start at 0. At the end of the source the lookup is
    /// [`Document::node_at_kind`]. `None` when the offset is past the source,
    /// the document is empty, or no ancestor of that exact kind exists
    /// (`KEYWORD` does not match `SETTING_KEYWORD`).
    #[must_use]
    pub fn resolve(&self, doc: &Document) -> Option<SyntaxNode> {
        let src = doc.source();
        let len = TextSize::of(src);
        let off = self.anchor.offset;
        if off > len || src.is_empty() {
            return None;
        }
        if off == len {
            doc.node_at_kind(off, self.kind)
        } else {
            doc.node_owning_kind(off, self.kind)
        }
    }
}

/// Map `old` through `edits` and resolve it on `new`, the document they
/// produced.
///
/// A pure deletion that covers the anchor returns `None`, so a following
/// node of the same kind is not mistaken for the deleted one. Otherwise the
/// mapped handle is returned if it resolves on `new`. See [`NodeRef`] for
/// in-place rewrites and for finding an inserted, moved or cloned heading.
#[must_use]
pub fn apply_locating(old: &NodeRef, edits: &EditSet, new: &Document) -> Option<NodeRef> {
    let off = old.anchor.offset;
    for edit in edits.edits() {
        if edit.replacement.is_empty() && edit.range.start() <= off && off < edit.range.end() {
            return None;
        }
    }
    let mapped = NodeRef {
        anchor: edits.try_map_anchor(old.anchor)?,
        kind: old.kind,
    };
    mapped.resolve(new).map(|_| mapped)
}

/// Where the inserted construct starts in the new text, after an insert at
/// `dest`.
///
/// Finds the unique empty-range edit at `dest.offset()`, maps its start left
/// through the whole set, and skips the leading padding `dest` asks for when
/// the replacement actually begins with it. `None` if that edit is missing
/// or not unique.
#[must_use]
pub fn payload_start(edits: &EditSet, dest: InsertPoint) -> Option<TextSize> {
    let mut insert = None;
    for edit in edits.edits() {
        if edit.range.is_empty() && edit.range.start() == dest.offset() {
            if insert.is_some() {
                return None;
            }
            insert = Some(edit);
        }
    }
    let edit = insert?;
    // Left through the whole set, not the insert alone: a delete `[s, e)`
    // plus insert at `e` contains `e` (`off <= e`), and Left lands at the hole.
    let wrap = edits
        .try_map_anchor(Anchor {
            offset: edit.range.start(),
            bias: Bias::Left,
        })?
        .offset;
    let lead = padding_str(dest.lead());
    // `dest.lead()` describes the pre-edit text. Move and clone may pad
    // differently around surviving neighbours, so skip only what was written.
    let skip = if edit.replacement.starts_with(lead) {
        lead
    } else {
        ""
    };
    Some(wrap + TextSize::of(skip))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Heading, InsertPoint, Markup, Padding};
    use crate::edit::{Edit, EditSet};
    use crate::parse;
    use crate::writer::{padding_str, wrap_insert};
    use crate::{TextRange, TextSize};

    fn ts(n: u32) -> TextSize {
        TextSize::from(n)
    }

    fn first_heading(doc: &Document) -> Heading {
        doc.map::<Heading>().next().expect("heading")
    }

    fn headings(doc: &Document) -> Vec<Heading> {
        doc.map::<Heading>().collect()
    }

    #[test]
    fn node_ref_resolve_and_stale() {
        let doc = parse("* Alpha\n");
        let heading = first_heading(&doc);
        let live = NodeRef::of(&heading);
        assert_eq!(live.anchor.bias, Bias::Right);
        assert_eq!(live.kind, OrgSyntaxKind::HEADING);
        let resolved = live.resolve(&doc).expect("live heading");
        assert_eq!(resolved.text_range(), heading.range());

        assert!(
            NodeRef::at(heading.range().start(), OrgSyntaxKind::KEYWORD)
                .resolve(&doc)
                .is_none(),
            "exact kind: KEYWORD is not HEADING"
        );

        let end = TextSize::of(doc.source());
        assert!(
            NodeRef::at(end, OrgSyntaxKind::HEADING)
                .resolve(&doc)
                .is_some(),
            "EOF falls back to the last token"
        );
        assert!(
            NodeRef::at(end, OrgSyntaxKind::DOCUMENT)
                .resolve(&doc)
                .is_some()
        );
        assert!(
            NodeRef::at(end + ts(1), OrgSyntaxKind::HEADING)
                .resolve(&doc)
                .is_none()
        );

        let empty = parse("");
        assert!(
            NodeRef::at(ts(0), OrgSyntaxKind::DOCUMENT)
                .resolve(&empty)
                .is_none()
        );
    }

    #[test]
    fn node_ref_of_second_heading_is_beta_not_alpha() {
        let doc = parse("* Alpha\n* Beta\n");
        let hs = headings(&doc);
        assert_eq!(hs.len(), 2);
        let second = &hs[1];
        let r = NodeRef::of(second);
        assert_eq!(r.anchor.offset, second.range().start());
        assert_eq!(r.anchor.bias, Bias::Right);
        let resolved = r.resolve(&doc).expect("Beta");
        assert_eq!(resolved.text_range(), second.range());
        assert_eq!(resolved.text().to_string(), "* Beta\n");
        assert_ne!(resolved.text_range(), hs[0].range());
    }

    #[test]
    fn node_ref_of_round_trips_lead_in_heading_and_mid_paragraph_markup() {
        let doc = parse("lead-in\n* H\n");
        let heading = first_heading(&doc);
        assert!(heading.range().start() > ts(0));
        let resolved = NodeRef::of(&heading)
            .resolve(&doc)
            .expect("heading after zeroth section");
        assert_eq!(resolved.text_range(), heading.range());

        let doc = parse("see *bold* rest\n");
        let markup = doc.map::<Markup>().next().expect("bold");
        assert!(markup.range().start() > ts(0));
        let resolved = NodeRef::of(&markup).resolve(&doc).expect("markup");
        assert_eq!(resolved.text_range(), markup.range());
        assert_eq!(resolved.kind(), OrgSyntaxKind::BOLD);
    }

    #[test]
    fn node_ref_setting_keyword_is_not_keyword() {
        let doc = parse("#+TODO: A | B\n");
        let sk = doc
            .node_at_kind(ts(0), OrgSyntaxKind::SETTING_KEYWORD)
            .expect("setting");
        assert!(
            NodeRef::at(sk.text_range().start(), OrgSyntaxKind::KEYWORD)
                .resolve(&doc)
                .is_none()
        );
        assert!(
            NodeRef::at(sk.text_range().start(), OrgSyntaxKind::SETTING_KEYWORD)
                .resolve(&doc)
                .is_some()
        );
    }

    #[test]
    fn apply_locating_on_pure_delete_is_none_not_neighbour() {
        let doc = parse("* Alpha\n* Beta\n");
        let hs = headings(&doc);
        assert_eq!(hs.len(), 2, "two sibling headings");
        let first = &hs[0];
        let second = &hs[1];
        let set =
            EditSet::single(doc.source(), first.range(), "").expect("delete first heading range");
        let next = doc.apply(&set);
        assert_eq!(next.write(), "* Beta\n");
        let remaining = NodeRef::at(ts(0), OrgSyntaxKind::HEADING)
            .resolve(&next)
            .expect("following heading still exists");
        assert_eq!(remaining.text().to_string(), "* Beta\n");
        assert!(
            apply_locating(&NodeRef::of(first), &set, &next).is_none(),
            "deleted handle must not resolve to the neighbour"
        );

        let located = apply_locating(&NodeRef::of(second), &set, &next)
            .expect("second heading is not inside the delete");
        assert_eq!(located.kind, OrgSyntaxKind::HEADING);
        assert_eq!(located.anchor.bias, Bias::Right);
        assert_eq!(
            located
                .resolve(&next)
                .expect("Beta on the new snapshot")
                .text()
                .to_string(),
            "* Beta\n"
        );
    }

    #[test]
    fn apply_locating_delete_second_is_none_first_remains() {
        let doc = parse("* Alpha\n* Beta\n");
        let hs = headings(&doc);
        let set =
            EditSet::single(doc.source(), hs[1].range(), "").expect("delete second heading range");
        let next = doc.apply(&set);
        assert_eq!(next.write(), "* Alpha\n");
        assert!(apply_locating(&NodeRef::of(&hs[1]), &set, &next).is_none());
        let first = NodeRef::of(&hs[0])
            .resolve(&next)
            .expect("first heading remains");
        assert_eq!(first.text().to_string(), "* Alpha\n");
    }

    #[test]
    fn apply_locating_non_empty_rewrite_is_not_none() {
        let doc = parse("* Alpha\n* Beta\n");
        let hs = headings(&doc);
        let old = NodeRef::of(&hs[0]);
        let set = EditSet::single(doc.source(), hs[0].range(), "* Gamma\n")
            .expect("in-place heading rewrite");
        let next = doc.apply(&set);
        assert_eq!(next.write(), "* Gamma\n* Beta\n");
        let mapped =
            apply_locating(&old, &set, &next).expect("non-empty replacement is not a delete");
        assert_eq!(mapped.kind, OrgSyntaxKind::HEADING);
        assert_eq!(mapped.anchor.bias, Bias::Right);
        // Right at construct start lands at the replacement end, which is
        // where the following sibling starts.
        assert_eq!(mapped.anchor.offset, ts(8));
        assert_eq!(
            mapped
                .resolve(&next)
                .expect("owning byte at replacement end")
                .text()
                .to_string(),
            "* Beta\n"
        );
    }

    #[test]
    fn apply_locating_forged_max_offset_is_none() {
        let doc = parse("a");
        let set = EditSet::single(doc.source(), TextRange::empty(ts(0)), "x").expect("insert");
        let next = doc.apply(&set);
        let forged = NodeRef::at(TextSize::from(u32::MAX), OrgSyntaxKind::HEADING);
        assert!(forged.resolve(&doc).is_none());
        assert!(apply_locating(&forged, &set, &next).is_none());
    }

    #[test]
    fn payload_start_skips_newline_lead() {
        let doc = parse("* A");
        let outline = doc.root_outlines().next().expect("outline");
        let dest = outline.insert_after();
        assert_eq!(dest.lead(), Padding::Newline);
        let wrapped = wrap_insert(dest, "* New\n");
        let set = EditSet::single(doc.source(), TextRange::empty(dest.offset()), wrapped)
            .expect("insert at dest");
        let start = payload_start(&set, dest).expect("unique insert");
        let wrap = set.map_offset(dest.offset(), Bias::Left);
        assert_eq!(start, wrap + TextSize::of(padding_str(dest.lead())));
        let out = set.apply_to_text(doc.source());
        assert_eq!(out, "* A\n* New\n");
        let i = usize::from(start);
        assert_eq!(&out[i..], "* New\n");
        assert_eq!(&out[usize::from(wrap)..i], "\n");
        let next = doc.apply(&set);
        let minted = NodeRef::at(start, OrgSyntaxKind::HEADING)
            .resolve(&next)
            .expect("right-biased handle at payload_start");
        assert_eq!(minted.text().to_string(), "* New\n");
    }

    #[test]
    fn payload_start_left_through_delete_ending_at_dest() {
        let src = "abcdefgh";
        let s = ts(2);
        let e = ts(6);
        let dest = InsertPoint::new(e, Padding::Newline, Padding::None);
        let wrapped = wrap_insert(dest, "XY");
        let set = EditSet::new(
            src,
            vec![
                Edit {
                    range: TextRange::new(s, e),
                    replacement: String::new(),
                },
                Edit {
                    range: TextRange::empty(e),
                    replacement: wrapped,
                },
            ],
        )
        .expect("touching delete then insert");
        assert_eq!(
            set.map_offset(e, Bias::Left),
            s,
            "Left through the delete lands at the hole, not at dest"
        );
        let start = payload_start(&set, dest).expect("unique insert");
        assert_eq!(start, s + TextSize::of(padding_str(Padding::Newline)));
        let out = set.apply_to_text(src);
        assert_eq!(out, "ab\nXYgh");
        let i = usize::from(start);
        assert_eq!(&out[i..i + 2], "XY");
    }

    #[test]
    fn payload_start_none_when_insert_missing_or_not_unique() {
        let src = "ab";
        let dest = InsertPoint::new(ts(1), Padding::None, Padding::None);
        let empty = EditSet::new(src, Vec::new()).expect("empty set");
        assert!(payload_start(&empty, dest).is_none());

        let dup = EditSet::new(
            src,
            vec![
                Edit {
                    range: TextRange::empty(ts(1)),
                    replacement: "x".into(),
                },
                Edit {
                    range: TextRange::empty(ts(1)),
                    replacement: "y".into(),
                },
            ],
        )
        .expect("raw co-located inserts");
        assert!(payload_start(&dup, dest).is_none());
    }

    #[test]
    fn payload_start_skips_lead_only_when_written() {
        let src = "ab";
        let dest = InsertPoint::new(ts(2), Padding::Newline, Padding::None);
        let without = EditSet::single(src, TextRange::empty(ts(2)), "XY").expect("no lead");
        let start = payload_start(&without, dest).expect("unique insert");
        assert_eq!(start, ts(2));
        let out = without.apply_to_text(src);
        assert_eq!(&out[usize::from(start)..], "XY");

        let with_lead = EditSet::single(src, TextRange::empty(ts(2)), "\nXY").expect("lead");
        let start = payload_start(&with_lead, dest).expect("unique insert");
        assert_eq!(start, ts(3));
        let out = with_lead.apply_to_text(src);
        assert_eq!(&out[usize::from(start)..], "XY");
        assert_ne!(out.len(), src.len());
    }
}
