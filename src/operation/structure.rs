//! Structural outline ops: insert, delete, move, clone, promote, demote.

use super::empty_edit_set;
use super::locate::{SubtreeInsert, resolve_outline, resolve_subtree_dest};
use super::map_synthesis;
use super::{HeadingInput, SubtreeDest, reject_priority, reject_todo};
use crate::ast::{Headline, InsertPoint, Outline, Padding, token_at_caret, token_owning_byte};
use crate::edit::{Edit, EditSet};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::writer::{HeadingSpec, render_heading, render_stars, wrap_insert};
use crate::{Document, NodeRef, OperationError, TextRange};

pub(super) fn compile_insert_heading(
    doc: &Document,
    at: SubtreeDest,
    heading: &HeadingInput,
) -> Result<EditSet, OperationError> {
    if heading.level == 0 {
        return Err(OperationError::InvalidLevel);
    }
    if heading.level >= doc.config().inline_task_min_level() as usize {
        return Err(OperationError::WouldBreakInlineTask);
    }
    if let Some(todo) = heading.todo.as_deref() {
        reject_todo(doc, todo)?;
    }
    if let Some(atom) = heading.priority {
        reject_priority(doc, atom)?;
    }
    let dest = resolve_subtree_dest(doc, at)?;
    let tags: Vec<&str> = heading.tags.iter().map(String::as_str).collect();
    let rendered = render_heading(
        HeadingSpec {
            level: heading.level,
            todo: heading.todo.as_deref(),
            priority: heading.priority,
            commented: heading.commented,
            title: &heading.title,
            tags: &tags,
        },
        doc.config(),
    )
    .map_err(map_synthesis)?;
    let replacement = wrap_insert(dest.point, &rendered);
    Ok(EditSet::single(
        doc.source(),
        TextRange::empty(dest.point.offset()),
        replacement,
    )?)
}

pub(super) fn compile_delete_subtree(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let outline = resolve_outline(doc, target)?;
    Ok(EditSet::single(doc.source(), outline.subtree_range(), "")?)
}

pub(super) fn compile_move_subtree(
    doc: &Document,
    target: NodeRef,
    to: SubtreeDest,
) -> Result<EditSet, OperationError> {
    let src = resolve_outline(doc, target)?;
    let dest = resolve_subtree_dest(doc, to)?;
    let range = src.subtree_range();
    if dest.level == src.level()
        && (dest.point.offset() == range.start() || dest.point.offset() == range.end())
    {
        return Ok(empty_edit_set(doc.source()));
    }
    if dest
        .outline
        .as_ref()
        .is_some_and(|d| d == &src || d.ancestors().any(|a| a == src))
    {
        return Err(OperationError::WouldContainSelf);
    }
    splice_subtree(doc, &src, &dest, true)
}

pub(super) fn compile_clone_subtree(
    doc: &Document,
    target: NodeRef,
    to: SubtreeDest,
) -> Result<EditSet, OperationError> {
    let src = resolve_outline(doc, target)?;
    let dest = resolve_subtree_dest(doc, to)?;
    splice_subtree(doc, &src, &dest, false)
}

pub(super) fn compile_relevel(
    doc: &Document,
    target: NodeRef,
    with_subtree: bool,
    by: usize,
    promote: bool,
) -> Result<EditSet, OperationError> {
    let outline = resolve_outline(doc, target)?;
    if by == 0 {
        return Ok(empty_edit_set(doc.source()));
    }
    let inline_min = doc.config().inline_task_min_level() as usize;
    let headings: Vec<_> = if with_subtree {
        outline.subtree_headings().collect()
    } else {
        vec![outline.heading()]
    };
    let mut edits = Vec::with_capacity(headings.len());
    for heading in headings {
        let new_level = shifted_level(heading.level(), by, promote, inline_min)?;
        edits.push(Edit {
            range: heading.stars_range(),
            replacement: render_stars(new_level),
        });
    }
    Ok(EditSet::new(doc.source(), edits)?)
}

fn splice_subtree(
    doc: &Document,
    src: &Outline,
    dest: &SubtreeInsert,
    delete_source: bool,
) -> Result<EditSet, OperationError> {
    let payload = restar_payload(doc, src, dest.level)?;
    let dest_off = dest.point.offset();
    let root = doc.root();
    let (lead, trail) = match delete_source.then(|| src.subtree_range()) {
        Some(range) if dest_off >= range.start() && dest_off <= range.end() => {
            pad_from_hole(root, range, &payload)
        }
        _ => pad_from_dest(root, dest.point, &payload),
    };
    let insert = Edit {
        range: TextRange::empty(dest_off),
        replacement: wrap_insert(InsertPoint::new(dest_off, lead, trail), &payload),
    };
    if !delete_source {
        return Ok(EditSet::new(doc.source(), vec![insert])?);
    }
    let mut edits = vec![
        insert,
        Edit {
            range: src.subtree_range(),
            replacement: String::new(),
        },
    ];
    // Empty insert before a delete at the same start; the reverse overlaps.
    edits.sort_by_key(|e| (e.range.start(), !e.range.is_empty()));
    Ok(EditSet::new(doc.source(), edits)?)
}

fn pad_from_dest(root: &SyntaxNode, dest: InsertPoint, payload: &str) -> (Padding, Padding) {
    let mut lead = dest.lead();
    let mut trail = dest.trail();
    let prev =
        token_at_caret(root, dest.offset()).filter(|t| t.text_range().end() == dest.offset());
    if !matches!(lead, Padding::Newline | Padding::BlankLine)
        && prev.is_some_and(|t| t.kind() != OrgSyntaxKind::NEWLINE)
    {
        lead = Padding::Newline;
    }
    if !payload.ends_with('\n')
        && !matches!(trail, Padding::Newline | Padding::BlankLine)
        && token_owning_byte(root, dest.offset())
            .is_some_and(|t| t.kind() != OrgSyntaxKind::NEWLINE)
    {
        trail = Padding::Newline;
    }
    (lead, trail)
}

fn pad_from_hole(root: &SyntaxNode, range: TextRange, payload: &str) -> (Padding, Padding) {
    let prev =
        token_at_caret(root, range.start()).filter(|t| t.text_range().end() == range.start());
    let lead = if prev.is_some_and(|t| t.kind() != OrgSyntaxKind::NEWLINE) {
        Padding::Newline
    } else {
        Padding::None
    };
    let trail = if !payload.ends_with('\n')
        && token_owning_byte(root, range.end()).is_some_and(|t| t.kind() != OrgSyntaxKind::NEWLINE)
    {
        Padding::Newline
    } else {
        Padding::None
    };
    (lead, trail)
}

fn restar_payload(
    doc: &Document,
    src: &Outline,
    dest_level: usize,
) -> Result<String, OperationError> {
    let range = src.subtree_range();
    let raw = doc.slice(range);
    let src_level = src.level();
    if dest_level == src_level {
        return Ok(raw.to_string());
    }
    let by = dest_level.abs_diff(src_level);
    let promote = dest_level < src_level;
    let inline_min = doc.config().inline_task_min_level() as usize;
    let src_start = range.start();
    let mut star_edits = Vec::new();
    for heading in src.subtree_headings() {
        let new_level = shifted_level(heading.level(), by, promote, inline_min)?;
        let rel = heading.stars_range();
        star_edits.push(Edit {
            range: TextRange::new(rel.start() - src_start, rel.end() - src_start),
            replacement: render_stars(new_level),
        });
    }
    Ok(EditSet::new(raw, star_edits)?.apply_to_text(raw))
}

fn shifted_level(
    old: usize,
    by: usize,
    promote: bool,
    inline_min: usize,
) -> Result<usize, OperationError> {
    let new = if promote {
        old.checked_sub(by)
            .filter(|&n| n >= 1)
            .ok_or(OperationError::CannotPromote)?
    } else {
        old.checked_add(by)
            .ok_or(OperationError::WouldBreakInlineTask)?
    };
    if new >= inline_min {
        return Err(OperationError::WouldBreakInlineTask);
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Heading, Headline, InlineTask, InsertPoint};
    use crate::edit::EditSet;
    use crate::operation::compile_write;
    use crate::writer::TitleProblem;
    use crate::{
        DEFAULT_INLINE_TASK_MIN_LEVEL, Operation, OrgSyntaxKind, ParseConfig, PriorityAtom,
        apply_locating, compile, compile_all, parse, parse_with, payload_start,
    };

    fn first_outline(doc: &Document) -> Outline {
        doc.root_outlines().next().expect("outline")
    }

    fn outlines(doc: &Document) -> Vec<Outline> {
        doc.root_outlines().collect()
    }

    fn outline_ref(doc: &Document) -> NodeRef {
        NodeRef::of(&first_outline(doc))
    }

    fn heading_input(level: usize, title: &str) -> HeadingInput {
        HeadingInput {
            level,
            title: title.to_string(),
            todo: None,
            priority: None,
            commented: false,
            tags: Vec::new(),
        }
    }

    fn insert_at(at: SubtreeDest, heading: HeadingInput) -> Operation {
        Operation::InsertHeading { at, heading }
    }

    fn apply_op(doc: &Document, op: &Operation) -> (EditSet, Document) {
        let set = compile(doc, op).expect("compile");
        super::super::assert_compile_apply(doc, &set);
        super::super::assert_envelope(doc, op, &set);
        let next = doc.apply(&set);
        (set, next)
    }

    fn payload_at(set: &EditSet, dest: InsertPoint, next: &Document) -> Outline {
        let start = payload_start(set, dest).expect("unique insert");
        let node = NodeRef::at(start, OrgSyntaxKind::OUTLINE)
            .resolve(next)
            .expect("payload outline");
        Outline::cast(node).expect("OUTLINE")
    }

    fn title_of(outline: &Outline) -> String {
        outline.heading().title().syntax().text().to_string()
    }

    #[test]
    fn insert_heading_keeps_the_given_level() {
        let doc = parse("* A\n** B\n");
        let parent = outline_ref(&doc);
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::FirstChildOf(parent), heading_input(1, "New")),
        );
        assert_eq!(text, "* A\n* New\n** B\n");
        let split = parse(&text);
        let roots = outlines(&split);
        assert_eq!(roots.len(), 2);
        assert_eq!(title_of(&roots[0]).trim(), "A");
        assert_eq!(title_of(&roots[1]).trim(), "New");
        assert_eq!(
            title_of(&roots[1].children().next().expect("B under New")).trim(),
            "B"
        );

        let doc = parse("* A\n");
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::DocumentStart, heading_input(3, "Deep")),
        );
        assert_eq!(text, "*** Deep\n* A\n");
        assert_eq!(first_outline(&parse(&text)).level(), 3);
    }

    #[test]
    fn insert_heading_document_start_leaves_zeroth_section() {
        let doc = parse("paragraph\n");
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::DocumentStart, heading_input(1, "New")),
        );
        assert_eq!(text, "paragraph\n* New\n");
    }

    #[test]
    fn insert_heading_empty_and_eof_lead() {
        let doc = parse("");
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::DocumentEnd, heading_input(1, "New")),
        );
        assert_eq!(text, "* New\n");

        let doc = parse("* A");
        let dest = doc.last_outline_insert_point();
        let op = insert_at(SubtreeDest::DocumentEnd, heading_input(1, "New"));
        let (set, text) = compile_write(&doc, &op);
        assert_eq!(text, "* A\n* New\n");
        assert!(!text.contains("* A**"), "eof_lead must separate stars");
        let next = doc.apply(&set);
        assert_ne!(next.write().len(), doc.source().len());
        let minted = payload_at(&set, dest, &next);
        assert_eq!(title_of(&minted).trim(), "New");
        assert_ne!(title_of(&minted).trim(), "A");
    }

    #[test]
    fn insert_heading_occupancy_flags_are_independent() {
        let doc = parse("");
        let at = SubtreeDest::DocumentStart;
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    todo: Some("TODO".into()),
                    ..heading_input(1, "DONE")
                },
            ),
        );
        assert_eq!(text, "* TODO DONE\n");
        let err = compile(&doc, &insert_at(at, heading_input(1, "DONE"))).unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::TodoKeyword));

        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    priority: Some(PriorityAtom::Letter('A')),
                    ..heading_input(1, "[#B] rest")
                },
            ),
        );
        assert_eq!(text, "* [#A] [#B] rest\n");
        let err = compile(&doc, &insert_at(at, heading_input(1, "[#B] rest"))).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::PriorityCookie)
        );

        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    commented: true,
                    ..heading_input(1, "COMMENT")
                },
            ),
        );
        assert_eq!(text, "* COMMENT COMMENT\n");
        let err = compile(&doc, &insert_at(at, heading_input(1, "COMMENT"))).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::CommentMarker)
        );

        let err = compile(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    tags: vec!["a".into()],
                    ..heading_input(1, "foo :a:")
                },
            ),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::TrailingTagGroup)
        );
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    tags: vec!["a".into()],
                    ..heading_input(1, "foo")
                },
            ),
        );
        assert_eq!(text, "* foo :a:\n");
    }

    #[test]
    #[allow(clippy::too_many_lines)] // legal counterparts sit next to each error
    fn insert_heading_level_and_field_errors_have_legal_counterparts() {
        let doc = parse("* A\n");
        let at = SubtreeDest::DocumentEnd;
        let err = compile(&doc, &insert_at(at, heading_input(0, "X"))).unwrap_err();
        assert_eq!(err, OperationError::InvalidLevel);
        let (_, text) = compile_write(&doc, &insert_at(at, heading_input(1, "X")));
        assert_eq!(text, "* A\n* X\n");

        let err = compile(
            &doc,
            &insert_at(
                at,
                heading_input(DEFAULT_INLINE_TASK_MIN_LEVEL as usize, "X"),
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                heading_input(DEFAULT_INLINE_TASK_MIN_LEVEL as usize - 1, "X"),
            ),
        );
        assert!(text.starts_with("* A\n"));
        assert!(text.contains('X'));

        let err = compile(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    todo: Some("WAIT".into()),
                    ..heading_input(1, "X")
                },
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword("WAIT".into()));
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    todo: Some("TODO".into()),
                    ..heading_input(1, "X")
                },
            ),
        );
        assert_eq!(text, "* A\n* TODO X\n");

        let err = compile(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    tags: vec!["a b".into()],
                    ..heading_input(1, "X")
                },
            ),
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::InvalidTag(s) if s == "a b"));
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    tags: vec!["ok".into()],
                    ..heading_input(1, "X")
                },
            ),
        );
        assert_eq!(text, "* A\n* X :ok:\n");

        let err = compile(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    priority: Some(PriorityAtom::Letter('a')),
                    ..heading_input(1, "X")
                },
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidPriority);
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                at,
                HeadingInput {
                    priority: Some(PriorityAtom::Letter('A')),
                    ..heading_input(1, "X")
                },
            ),
        );
        assert_eq!(text, "* A\n* [#A] X\n");

        let doc = parse("#+PRIORITIES: A C B\n* A\n");
        let err = compile(
            &doc,
            &insert_at(
                SubtreeDest::DocumentEnd,
                HeadingInput {
                    priority: Some(PriorityAtom::Letter('D')),
                    ..heading_input(1, "X")
                },
            ),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::PriorityOutOfRange(PriorityAtom::Letter('D'))
        );
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                SubtreeDest::DocumentEnd,
                HeadingInput {
                    priority: Some(PriorityAtom::Letter('A')),
                    ..heading_input(1, "X")
                },
            ),
        );
        assert_eq!(text, "#+PRIORITIES: A C B\n* A\n* [#A] X\n");
    }

    #[test]
    fn delete_subtree_locating_is_none_not_neighbour() {
        let doc = parse("* Alpha\n* Beta\n");
        let roots = outlines(&doc);
        let first = NodeRef::of(&roots[0]);
        let second = NodeRef::of(&roots[1]);
        let op = Operation::DeleteSubtree { target: first };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* Beta\n");
        assert!(
            apply_locating(&first, &set, &next).is_none(),
            "deleted handle must not resolve to the neighbour"
        );
        assert!(apply_locating(&NodeRef::of(&roots[0].heading()), &set, &next).is_none());
        let remaining = apply_locating(&second, &set, &next).expect("Beta remains");
        assert_eq!(
            remaining.resolve(&next).expect("Beta").text().to_string(),
            "* Beta\n"
        );

        let (_, text) = compile_write(&doc, &Operation::DeleteSubtree { target: second });
        assert_eq!(text, "* Alpha\n");
    }

    #[test]
    fn move_noops_and_dest_coincidence() {
        let doc = parse("* A\n* B\n");
        let roots = outlines(&doc);
        let a = NodeRef::of(&roots[0]);
        let b = NodeRef::of(&roots[1]);
        for to in [SubtreeDest::Before(a), SubtreeDest::After(a)] {
            let (set, text) = compile_write(&doc, &Operation::MoveSubtree { target: a, to });
            assert!(set.is_empty());
            assert_eq!(text, "* A\n* B\n");
        }
        let (set, _) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: b,
                to: SubtreeDest::After(a),
            },
        );
        assert!(set.is_empty(), "adjacent same-level After(prev) is a no-op");
        let (set, _) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: a,
                to: SubtreeDest::Before(b),
            },
        );
        assert!(
            set.is_empty(),
            "adjacent same-level Before(next) is a no-op"
        );
        let (set, _) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: b,
                to: SubtreeDest::DocumentEnd,
            },
        );
        assert!(
            set.is_empty(),
            "last-root level-1 to DocumentEnd is a no-op"
        );
        let (set, _) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: a,
                to: SubtreeDest::DocumentStart,
            },
        );
        assert!(
            set.is_empty(),
            "first-root level-1 to DocumentStart is a no-op"
        );

        let (set, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: a,
                to: SubtreeDest::After(b),
            },
        );
        assert!(!set.is_empty());
        assert_eq!(text, "* B\n* A\n");

        let doc = parse("* A\n* B");
        let roots = outlines(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: NodeRef::of(&roots[1]),
                to: SubtreeDest::Before(NodeRef::of(&roots[0])),
            },
        );
        assert_eq!(text, "* B\n* A\n");
    }

    #[test]
    fn move_equal_start_insert_before_delete() {
        let doc = parse("*** H\n");
        let target = outline_ref(&doc);
        let dest = doc.first_outline_insert_point();
        let op = Operation::MoveSubtree {
            target,
            to: SubtreeDest::DocumentStart,
        };
        let (set, text) = compile_write(&doc, &op);
        assert_eq!(text, "* H\n");
        assert_eq!(set.len(), 2);
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert!(
            set.edits()[1].replacement.is_empty(),
            "{:?}",
            set.edits()[1].replacement
        );

        let doc = parse("*** A\n* B\n");
        let roots = outlines(&doc);
        assert_eq!(
            roots[0].subtree_range().end(),
            roots[1].subtree_range().start()
        );
        let op = Operation::MoveSubtree {
            target: NodeRef::of(&roots[1]),
            to: SubtreeDest::After(NodeRef::of(&roots[0])),
        };
        let (set, text) = compile_write(&doc, &op);
        assert!(!set.is_empty());
        assert_eq!(text, "*** A\n*** B\n");
    }

    #[test]
    fn move_last_root_deep_to_document_end_restars() {
        let doc = parse("*** H\n");
        let target = outline_ref(&doc);
        let dest = doc.last_outline_insert_point();
        let op = Operation::MoveSubtree {
            target,
            to: SubtreeDest::DocumentEnd,
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* H\n");
        assert_ne!(next.write().len(), doc.source().len());
        let minted = payload_at(&set, dest, &next);
        assert_eq!(minted.level(), 1);
        assert_eq!(title_of(&minted).trim(), "H");
        assert!(apply_locating(&target, &set, &next).is_none());

        let doc = parse("*** H");
        let dest = doc.last_outline_insert_point();
        let op = Operation::MoveSubtree {
            target: outline_ref(&doc),
            to: SubtreeDest::DocumentEnd,
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* H");
        assert_ne!(next.write().len(), doc.source().len());
        let minted = payload_at(&set, dest, &next);
        assert_eq!(minted.level(), 1);
        assert_eq!(title_of(&minted).trim(), "H");

        let doc = parse("* A\n*** H");
        let deep = doc.map::<Outline>().find(|o| o.level() == 3).expect("***");
        let dest = doc.last_outline_insert_point();
        let op = Operation::MoveSubtree {
            target: NodeRef::of(&deep),
            to: SubtreeDest::DocumentEnd,
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* A\n* H");
        let minted = payload_at(&set, dest, &next);
        assert_eq!(minted.level(), 1);
        assert_eq!(title_of(&minted).trim(), "H");

        let doc = parse("* A\n*** H\n");
        let deep = doc.map::<Outline>().find(|o| o.level() == 3).expect("***");
        let dest = doc.last_outline_insert_point();
        let op = Operation::MoveSubtree {
            target: NodeRef::of(&deep),
            to: SubtreeDest::DocumentEnd,
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* A\n* H\n");
        assert_ne!(next.write().len(), doc.source().len());
        let minted = payload_at(&set, dest, &next);
        assert_eq!(minted.level(), 1);
        assert_eq!(title_of(&minted).trim(), "H");
    }

    #[test]
    fn move_would_contain_self_and_legal_refile() {
        let doc = parse("* P\n** C\n");
        let p = first_outline(&doc);
        let c = p.children().next().expect("child");
        let target = NodeRef::of(&p);
        for to in [
            SubtreeDest::FirstChildOf(target),
            SubtreeDest::ContentEndOf(target),
            SubtreeDest::After(NodeRef::of(&c)),
            SubtreeDest::Before(NodeRef::of(&c)),
        ] {
            let err = compile(&doc, &Operation::MoveSubtree { target, to }).unwrap_err();
            assert_eq!(err, OperationError::WouldContainSelf, "{to:?}");
        }
        assert_eq!(c.subtree_range().end(), p.subtree_range().end());

        let doc = parse("* Q\n* P\n** C\n");
        let roots = outlines(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: NodeRef::of(&roots[1]),
                to: SubtreeDest::FirstChildOf(NodeRef::of(&roots[0])),
            },
        );
        assert_eq!(text, "* Q\n** P\n*** C\n");
    }

    #[test]
    fn move_nested_star_width_and_inline_task_stars() {
        let doc = parse("* P\n** A\n*** B\n**** C\n");
        let p = first_outline(&doc);
        let a = p.children().next().expect("A");
        let (_, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: NodeRef::of(&a),
                to: SubtreeDest::After(NodeRef::of(&p)),
            },
        );
        assert_eq!(text, "* P\n* A\n** B\n*** C\n");

        let src = "* Z\n* A\n*************** TASK\n*************** END\n** B\n";
        let doc = parse(src);
        let roots = outlines(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: NodeRef::of(&roots[1]),
                to: SubtreeDest::FirstChildOf(NodeRef::of(&roots[0])),
            },
        );
        assert_eq!(
            text,
            "* Z\n** A\n*************** TASK\n*************** END\n*** B\n"
        );
        let next = parse(&text);
        let task = next.map::<InlineTask>().next().expect("inline task");
        assert_eq!(task.level(), 15);
    }

    #[test]
    fn clone_classifies_by_dest_not_offset() {
        let doc = parse("* H\n");
        let src = outline_ref(&doc);
        let after_off = first_outline(&doc).insert_after().offset();
        let child_off = first_outline(&doc).first_child_insert_point().offset();
        assert_eq!(after_off, child_off);

        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::After(src),
            },
        );
        assert_eq!(text, "* H\n* H\n");

        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::FirstChildOf(src),
            },
        );
        assert_eq!(text, "* H\n** H\n");

        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::ContentEndOf(src),
            },
        );
        assert_eq!(text, "* H\n** H\n");
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::ContentEndOf(src), heading_input(2, "H")),
        );
        assert_eq!(text, "* H\n** H\n");

        let doc = parse("* P\n*** C\n");
        let p = outline_ref(&doc);
        let c = NodeRef::of(&first_outline(&doc).children().next().expect("C"));
        let content_end = first_outline(&doc).content_end_insert_point().offset();
        let after_c = first_outline(&doc)
            .children()
            .next()
            .expect("C")
            .insert_after()
            .offset();
        assert_eq!(content_end, after_c);
        let first_child = first_outline(&doc).first_child_insert_point().offset();
        assert_ne!(first_child, content_end);
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: p,
                to: SubtreeDest::ContentEndOf(p),
            },
        );
        assert_eq!(text, "* P\n*** C\n** P\n**** C\n");
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: p,
                to: SubtreeDest::After(c),
            },
        );
        assert_eq!(text, "* P\n*** C\n*** P\n***** C\n");
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: p,
                to: SubtreeDest::FirstChildOf(p),
            },
        );
        assert_eq!(text, "* P\n** P\n**** C\n*** C\n");

        let doc = parse("* H\n\n");
        let src = outline_ref(&doc);
        let o = first_outline(&doc);
        assert!(o.first_child_insert_point().offset() < o.insert_after().offset());
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::FirstChildOf(src),
            },
        );
        // Subtree copy includes owned trailing blanks; dest sits before them.
        assert_eq!(text, "* H\n** H\n\n\n");
    }

    #[test]
    fn clone_before_after_self_duplicate_not_noop() {
        let doc = parse("* H\n");
        let src = outline_ref(&doc);
        let (set, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::Before(src),
            },
        );
        assert!(!set.is_empty());
        assert_eq!(text, "* H\n* H\n");
        let (set, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::After(src),
            },
        );
        assert!(!set.is_empty());
        assert_eq!(text, "* H\n* H\n");

        let doc = parse("* A");
        let src = outline_ref(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::Before(src),
            },
        );
        assert_eq!(text, "* A\n* A");
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: src,
                to: SubtreeDest::DocumentStart,
            },
        );
        assert_eq!(text, "* A\n* A");
    }

    #[test]
    fn clone_after_last_child_is_not_would_contain_self() {
        let doc = parse("* P\n** C\n");
        let p = first_outline(&doc);
        let c = p.children().next().expect("child");
        assert_eq!(c.subtree_range().end(), p.subtree_range().end());
        let err = compile(
            &doc,
            &Operation::MoveSubtree {
                target: NodeRef::of(&p),
                to: SubtreeDest::After(NodeRef::of(&c)),
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldContainSelf);
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: NodeRef::of(&p),
                to: SubtreeDest::After(NodeRef::of(&c)),
            },
        );
        assert_eq!(text, "* P\n** C\n** P\n*** C\n");
    }

    #[test]
    fn clone_locating_keeps_source_payload_is_copy() {
        let doc = parse("* A");
        let src = outline_ref(&doc);
        let dest = doc.last_outline_insert_point();
        let op = Operation::CloneSubtree {
            target: src,
            to: SubtreeDest::DocumentEnd,
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* A\n* A");
        assert_ne!(next.write().len(), doc.source().len());
        let located = apply_locating(&src, &set, &next).expect("source remains");
        let source = Outline::cast(located.resolve(&next).expect("source")).expect("OUTLINE");
        assert_eq!(source.level(), 1);
        let copy = payload_at(&set, dest, &next);
        assert_eq!(title_of(&copy).trim(), "A");
        assert!(copy.range().start() > source.range().start());

        let doc = parse("* H\n");
        let src = outline_ref(&doc);
        let dest = first_outline(&doc).first_child_insert_point();
        let op = Operation::CloneSubtree {
            target: src,
            to: SubtreeDest::FirstChildOf(src),
        };
        let (set, next) = apply_op(&doc, &op);
        assert_eq!(next.write(), "* H\n** H\n");
        assert!(apply_locating(&src, &set, &next).is_some());
        let copy = payload_at(&set, dest, &next);
        assert_eq!(copy.level(), 2);
    }

    #[test]
    fn clone_nested_star_width_and_inline_task_stars() {
        let doc = parse("* P\n** A\n*** B\n**** C\n");
        let p = first_outline(&doc);
        let a = p.children().next().expect("A");
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: NodeRef::of(&a),
                to: SubtreeDest::After(NodeRef::of(&p)),
            },
        );
        assert_eq!(text, "* P\n** A\n*** B\n**** C\n* A\n** B\n*** C\n");

        let src = "* Z\n* A\n*************** TASK\n*************** END\n** B\n";
        let doc = parse(src);
        let roots = outlines(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: NodeRef::of(&roots[1]),
                to: SubtreeDest::FirstChildOf(NodeRef::of(&roots[0])),
            },
        );
        assert_eq!(
            text,
            "* Z\n** A\n*************** TASK\n*************** END\n*** B\n* A\n*************** TASK\n*************** END\n** B\n"
        );
        let next = parse(&text);
        for task in next.map::<InlineTask>() {
            assert_eq!(task.level(), 15);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // floor, threshold, and locating are one sequence
    fn promote_demote_floor_threshold_and_locating() {
        let doc = parse("* A\n** B\n");
        let a = outline_ref(&doc);
        let b = NodeRef::of(&first_outline(&doc).children().next().expect("B"));
        let (set, text) = compile_write(
            &doc,
            &Operation::Promote {
                target: a,
                with_subtree: true,
                by: 0,
            },
        );
        assert!(set.is_empty());
        assert_eq!(text, "* A\n** B\n");

        let err = compile(
            &doc,
            &Operation::Promote {
                target: a,
                with_subtree: true,
                by: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::CannotPromote);
        let (set, next) = apply_op(
            &doc,
            &Operation::Promote {
                target: b,
                with_subtree: false,
                by: 1,
            },
        );
        assert_eq!(next.write(), "* A\n* B\n");
        assert_ne!(next.write().len(), doc.source().len());
        let heading = first_outline(&doc).children().next().expect("B").heading();
        let located = apply_locating(&NodeRef::of(&heading), &set, &next).expect("stars survive");
        assert_eq!(located.kind, OrgSyntaxKind::HEADING);
        assert_eq!(
            located
                .resolve(&next)
                .expect("heading")
                .text()
                .to_string()
                .trim(),
            "* B"
        );

        let (set, next) = apply_op(
            &doc,
            &Operation::Demote {
                target: b,
                with_subtree: false,
                by: 1,
            },
        );
        assert_eq!(next.write(), "* A\n*** B\n");
        assert_ne!(next.write().len(), doc.source().len());
        let heading = first_outline(&doc).children().next().expect("B").heading();
        let located = apply_locating(&NodeRef::of(&heading), &set, &next).expect("stars survive");
        assert_eq!(located.kind, OrgSyntaxKind::HEADING);
        assert_eq!(
            located
                .resolve(&next)
                .expect("heading")
                .text()
                .to_string()
                .trim(),
            "*** B"
        );
        assert_ne!(
            located
                .resolve(&next)
                .expect("heading")
                .text()
                .to_string()
                .trim(),
            "* A"
        );

        let doc = parse("* A\n** B\n*** C\n");
        let a = outline_ref(&doc);
        let b = NodeRef::of(&first_outline(&doc).children().next().expect("B"));
        let (_, text) = compile_write(
            &doc,
            &Operation::Promote {
                target: b,
                with_subtree: true,
                by: 1,
            },
        );
        assert_eq!(text, "* A\n* B\n** C\n");
        let (_, text) = compile_write(
            &doc,
            &Operation::Demote {
                target: a,
                with_subtree: true,
                by: 1,
            },
        );
        assert_eq!(text, "** A\n*** B\n**** C\n");
        let src = "* A\n*************** TASK\n*************** END\n** B\n";
        let doc = parse(src);
        let (_, text) = compile_write(
            &doc,
            &Operation::Demote {
                target: outline_ref(&doc),
                with_subtree: true,
                by: 1,
            },
        );
        assert_eq!(
            text,
            "** A\n*************** TASK\n*************** END\n*** B\n"
        );
        let task = parse(&text)
            .map::<InlineTask>()
            .next()
            .expect("inline task");
        assert_eq!(task.level(), 15);

        let doc = parse("************** H\n");
        let target = outline_ref(&doc);
        assert_eq!(first_outline(&doc).level(), 14);
        let err = compile(
            &doc,
            &Operation::Demote {
                target,
                with_subtree: false,
                by: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let doc = parse("************* H\n************** C\n");
        let target = outline_ref(&doc);
        let err = compile(
            &doc,
            &Operation::Demote {
                target,
                with_subtree: true,
                by: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let (_, text) = compile_write(
            &doc,
            &Operation::Demote {
                target,
                with_subtree: false,
                by: 1,
            },
        );
        assert_eq!(text, "************** H\n************** C\n");
    }

    #[test]
    fn structural_wrong_kind_and_stale_have_legal_counterparts() {
        let doc = parse("* A\n*************** TASK\n*************** END\n");
        let task = NodeRef::of(&doc.map::<InlineTask>().next().expect("inline task"));
        let ops = [
            Operation::DeleteSubtree { target: task },
            Operation::MoveSubtree {
                target: task,
                to: SubtreeDest::DocumentEnd,
            },
            Operation::CloneSubtree {
                target: task,
                to: SubtreeDest::DocumentEnd,
            },
            Operation::Promote {
                target: task,
                with_subtree: false,
                by: 1,
            },
            Operation::Demote {
                target: task,
                with_subtree: false,
                by: 1,
            },
            insert_at(SubtreeDest::FirstChildOf(task), heading_input(1, "X")),
            insert_at(SubtreeDest::Before(task), heading_input(1, "X")),
        ];
        for op in &ops {
            let err = compile(&doc, op).unwrap_err();
            assert!(
                matches!(
                    err,
                    OperationError::WrongKind {
                        actual: OrgSyntaxKind::INLINE_TASK,
                        ..
                    }
                ),
                "{op:?} -> {err:?}"
            );
        }
        let (_, text) = compile_write(
            &doc,
            &Operation::DeleteSubtree {
                target: outline_ref(&doc),
            },
        );
        assert_eq!(text, "");

        let doc = parse("* Hello Title\nbody\n");
        let heading = doc.map::<Heading>().next().expect("heading");
        let body = heading.range().end();
        assert!(body < heading.outline().range().end());
        let err = compile(
            &doc,
            &Operation::DeleteSubtree {
                target: NodeRef::at(body, OrgSyntaxKind::HEADING),
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);
        let (_, text) = compile_write(
            &doc,
            &Operation::DeleteSubtree {
                target: NodeRef::at(body, OrgSyntaxKind::OUTLINE),
            },
        );
        assert_eq!(text, "");

        let (_, text) = compile_write(
            &parse("* Hello Title\nbody\n"),
            &Operation::DeleteSubtree {
                target: NodeRef::of(&heading),
            },
        );
        assert_eq!(text, "");
    }

    #[test]
    fn compile_all_clone_conflicts_and_overlap() {
        let doc = parse("* A\n** B\n* C\n");
        let a = first_outline(&doc);
        let c = a.next_sibling().expect("C");
        let a_ref = NodeRef::of(&a);
        let c_ref = NodeRef::of(&c);
        let clone_end = Operation::CloneSubtree {
            target: a_ref,
            to: SubtreeDest::DocumentEnd,
        };
        let clone_end_c = Operation::CloneSubtree {
            target: c_ref,
            to: SubtreeDest::DocumentEnd,
        };
        let err = compile_all(&doc, &[clone_end.clone(), clone_end_c.clone()]).unwrap_err();
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: doc.last_outline_insert_point().offset()
            }
        );
        let set = compile_all(
            &doc,
            &[
                Operation::CloneSubtree {
                    target: a_ref,
                    to: SubtreeDest::After(c_ref),
                },
                Operation::CloneSubtree {
                    target: c_ref,
                    to: SubtreeDest::DocumentStart,
                },
            ],
        )
        .expect("distinct dest offsets");
        super::super::assert_compile_apply(&doc, &set);

        let delete_a = Operation::DeleteSubtree { target: a_ref };
        let clone_into_a = Operation::CloneSubtree {
            target: c_ref,
            to: SubtreeDest::FirstChildOf(a_ref),
        };
        let err = compile_all(&doc, &[delete_a, clone_into_a]).unwrap_err();
        assert!(matches!(err, OperationError::EditSet(_)));

        let err = compile_all(
            &doc,
            &[
                Operation::CloneSubtree {
                    target: c_ref,
                    to: SubtreeDest::FirstChildOf(a_ref),
                },
                Operation::MoveSubtree {
                    target: a_ref,
                    to: SubtreeDest::After(c_ref),
                },
            ],
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::EditSet(_)));

        let set = compile_all(
            &doc,
            &[
                Operation::CloneSubtree {
                    target: a_ref,
                    to: SubtreeDest::Before(a_ref),
                },
                Operation::MoveSubtree {
                    target: a_ref,
                    to: SubtreeDest::After(c_ref),
                },
            ],
        )
        .expect("clone before src plus move is touching");
        super::super::assert_compile_apply(&doc, &set);
    }

    #[test]
    fn insert_heading_dest_wrong_kind() {
        let doc = parse("* A\n*************** TASK\n*************** END\n");
        let task = doc.map::<InlineTask>().next().expect("inline task");
        let err = compile(
            &doc,
            &insert_at(
                SubtreeDest::After(NodeRef::of(&task)),
                heading_input(1, "X"),
            ),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            OperationError::WrongKind {
                actual: OrgSyntaxKind::INLINE_TASK,
                ..
            }
        ));
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::After(outline_ref(&doc)), heading_input(1, "X")),
        );
        assert!(text.contains("* X\n"));
    }

    #[test]
    fn custom_inline_task_min_rejects_insert_and_demote() {
        let cfg = ParseConfig::default().with_inline_task_min_level(3);
        let doc = parse_with("* A\n** B\n", &cfg);
        let err = compile(
            &doc,
            &insert_at(SubtreeDest::DocumentEnd, heading_input(3, "X")),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let (_, text) = compile_write(
            &doc,
            &insert_at(SubtreeDest::DocumentEnd, heading_input(2, "X")),
        );
        assert_eq!(text, "* A\n** B\n** X\n");

        let err = compile(
            &doc,
            &Operation::Demote {
                target: NodeRef::of(&first_outline(&doc).children().next().expect("B")),
                with_subtree: false,
                by: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let (_, text) = compile_write(
            &doc,
            &Operation::Demote {
                target: outline_ref(&doc),
                with_subtree: false,
                by: 1,
            },
        );
        assert_eq!(text, "** A\n** B\n");
    }

    #[test]
    fn move_clone_restar_hits_inline_task_threshold() {
        let src = "* Z\n* A\n************** Deep\n";
        let doc = parse(src);
        let roots = outlines(&doc);
        let z = NodeRef::of(&roots[0]);
        let a = NodeRef::of(&roots[1]);
        let err = compile(
            &doc,
            &Operation::MoveSubtree {
                target: a,
                to: SubtreeDest::FirstChildOf(z),
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);
        let err = compile(
            &doc,
            &Operation::CloneSubtree {
                target: a,
                to: SubtreeDest::FirstChildOf(z),
            },
        )
        .unwrap_err();
        assert_eq!(err, OperationError::WouldBreakInlineTask);

        let src = "* Z\n* A\n************* Deep\n";
        let doc = parse(src);
        let roots = outlines(&doc);
        let z = NodeRef::of(&roots[0]);
        let a = NodeRef::of(&roots[1]);
        let (_, text) = compile_write(
            &doc,
            &Operation::MoveSubtree {
                target: a,
                to: SubtreeDest::FirstChildOf(z),
            },
        );
        assert_eq!(text, "* Z\n** A\n************** Deep\n");
        let doc = parse(src);
        let roots = outlines(&doc);
        let (_, text) = compile_write(
            &doc,
            &Operation::CloneSubtree {
                target: NodeRef::of(&roots[1]),
                to: SubtreeDest::FirstChildOf(NodeRef::of(&roots[0])),
            },
        );
        assert_eq!(
            text,
            "* Z\n** A\n************** Deep\n* A\n************* Deep\n"
        );
    }

    #[test]
    fn heading_ref_hops_to_outline_for_structural_ops() {
        let doc = parse("* A\nbody\n");
        let heading = doc.map::<Heading>().next().expect("heading");
        let (_, text) = compile_write(
            &doc,
            &Operation::Promote {
                target: NodeRef::of(&heading),
                with_subtree: false,
                by: 0,
            },
        );
        assert_eq!(text, "* A\nbody\n");
        let (_, text) = compile_write(
            &doc,
            &insert_at(
                SubtreeDest::After(NodeRef::of(&heading)),
                heading_input(1, "B"),
            ),
        );
        assert_eq!(text, "* A\nbody\n* B\n");
    }
}
