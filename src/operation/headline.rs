//! Headline field ops: TODO, priority, COMMENT, title, tags.

use super::empty_edit_set;
use super::field::{optional_part_edit, replace_content_or_fill};
use super::locate::resolve_task;
use super::{reject_priority, reject_todo};
use crate::ast::{HasParts, HeadingPart, Headline, PartSpan, Task};
use crate::edit::EditSet;
use crate::writer::{
    OccupiedHeadlinePrefix, render_comment, render_tags, render_todo, title_problem_given,
};
use crate::{Document, NodeRef, OperationError, PriorityAtom};

pub(super) fn compile_set_todo(
    doc: &Document,
    target: NodeRef,
    state: Option<&str>,
) -> Result<EditSet, OperationError> {
    let heading = resolve_task(doc, target)?;
    match state {
        None => {
            if heading.todo_keyword().is_some() {
                reject_title_if_unwritable(doc, &heading, |occ| occ.todo = false)?;
            }
            optional_part_edit(doc, heading.part_span(HeadingPart::Todo), None)?.into_edit_set(doc)
        }
        Some(s) => {
            reject_todo(doc, s)?;
            let frag = render_todo(s);
            optional_part_edit(doc, heading.part_span(HeadingPart::Todo), Some(&frag))?
                .into_edit_set(doc)
        }
    }
}

pub(super) fn compile_set_priority(
    doc: &Document,
    target: NodeRef,
    priority: Option<PriorityAtom>,
) -> Result<EditSet, OperationError> {
    let heading = resolve_task(doc, target)?;
    match priority {
        None => {
            if heading.priority().is_some() {
                reject_title_if_unwritable(doc, &heading, |occ| occ.priority = false)?;
            }
            optional_part_edit(doc, heading.part_span(HeadingPart::Priority), None)?
                .into_edit_set(doc)
        }
        Some(atom) => {
            let frag = reject_priority(doc, atom)?;
            optional_part_edit(doc, heading.part_span(HeadingPart::Priority), Some(&frag))?
                .into_edit_set(doc)
        }
    }
}

pub(super) fn compile_set_tags(
    doc: &Document,
    target: NodeRef,
    tags: &[String],
) -> Result<EditSet, OperationError> {
    let heading = resolve_task(doc, target)?;
    let span = heading.part_span(HeadingPart::Tags);
    if tags.is_empty() {
        return optional_part_edit(doc, span, None)?.into_edit_set(doc);
    }
    let current: Option<Vec<String>> = heading
        .tags()
        .map(|t| t.iter().map(|g| g.name().to_string()).collect());
    if current.as_deref() == Some(tags) {
        return Ok(empty_edit_set(doc.source()));
    }
    let names: Vec<&str> = tags.iter().map(String::as_str).collect();
    let frag = match render_tags(&names) {
        Ok(s) => s,
        Err(e) => return Err(super::map_synthesis(e)),
    };
    optional_part_edit(doc, span, Some(&frag))?.into_edit_set(doc)
}

pub(super) fn compile_set_title(
    doc: &Document,
    target: NodeRef,
    title: &str,
) -> Result<EditSet, OperationError> {
    let heading = resolve_task(doc, target)?;
    let PartSpan::Present(part) = heading.part_span(HeadingPart::Title) else {
        unreachable!("grammar: HEADING_TITLE is always carved");
    };
    if let Some(problem) = title_problem_given(title, doc.config(), occupancy(&heading)) {
        return Err(OperationError::InvalidTitle(problem));
    }
    replace_content_or_fill(doc, part, heading.title_fill_insert_point(), title)
}

pub(super) fn compile_set_commented(
    doc: &Document,
    target: NodeRef,
    commented: bool,
) -> Result<EditSet, OperationError> {
    let heading = resolve_task(doc, target)?;
    if commented {
        let frag = render_comment();
        optional_part_edit(
            doc,
            heading.part_span(HeadingPart::CommentMarker),
            Some(&frag),
        )?
        .into_edit_set(doc)
    } else {
        if heading.is_commented() {
            reject_title_if_unwritable(doc, &heading, |occ| occ.commented = false)?;
        }
        optional_part_edit(doc, heading.part_span(HeadingPart::CommentMarker), None)?
            .into_edit_set(doc)
    }
}

fn occupancy(heading: &impl Headline) -> OccupiedHeadlinePrefix {
    OccupiedHeadlinePrefix {
        todo: heading.todo_keyword().is_some(),
        priority: heading.priority().is_some(),
        commented: heading.is_commented(),
    }
}

fn reject_title_if_unwritable(
    doc: &Document,
    heading: &Task,
    clear: impl FnOnce(&mut OccupiedHeadlinePrefix),
) -> Result<(), OperationError> {
    let range = heading
        .part_content_range(HeadingPart::Title)
        .expect("grammar: HEADING_TITLE is always carved");
    let mut occ = occupancy(heading);
    clear(&mut occ);
    if let Some(problem) = title_problem_given(doc.slice(range), doc.config(), occ) {
        return Err(OperationError::InvalidTitle(problem));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, HasParts, Heading, HeadingPart, Headline, InlineTask};
    use crate::operation::compile_write;
    use crate::parse_with;
    use crate::writer::TitleProblem;
    use crate::{
        NodeRef, Operation, OperationError, OrgSyntaxKind, ParseConfig, PriorityAtom, TodoKind,
        apply_locating, compile, compile_all, parse,
    };
    use std::ptr;

    fn first_heading(doc: &Document) -> Heading {
        doc.map::<Heading>().next().expect("heading")
    }

    fn first_inline_task(doc: &Document) -> InlineTask {
        doc.map::<InlineTask>().next().expect("inline task")
    }

    fn heading_ref(doc: &Document) -> NodeRef {
        NodeRef::of(&first_heading(doc))
    }

    fn set_todo(target: NodeRef, state: Option<&str>) -> Operation {
        Operation::SetTodoState {
            target,
            state: state.map(str::to_string),
        }
    }

    fn set_priority(target: NodeRef, priority: Option<PriorityAtom>) -> Operation {
        Operation::SetPriority { target, priority }
    }

    fn set_tags(target: NodeRef, tags: &[&str]) -> Operation {
        Operation::SetTags {
            target,
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn set_title(target: NodeRef, title: &str) -> Operation {
        Operation::SetTitle {
            target,
            title: title.to_string(),
        }
    }

    fn set_commented(target: NodeRef, commented: bool) -> Operation {
        Operation::SetCommented { target, commented }
    }

    #[test]
    fn set_todo_insert_rewrite_clear_and_noop() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_todo(target, Some("TODO")));
        assert_eq!(set.len(), 1);
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].replacement, "TODO ");
        assert_eq!(text, "* TODO Title\n");

        let doc = parse("* TODO Title\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_todo(target, Some("DONE")));
        assert_eq!(doc.slice(set.edits()[0].range), "TODO");
        assert_eq!(set.edits()[0].replacement, "DONE");
        assert_eq!(text, "* DONE Title\n");

        let doc = parse("* TODO Title\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_todo(target, None));
        assert_eq!(doc.slice(set.edits()[0].range), "TODO ");
        assert!(
            set.edits()[0].replacement.is_empty(),
            "{:?}",
            set.edits()[0].replacement
        );
        assert_eq!(text, "* Title\n");

        let doc = parse("* TODO Title\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_todo(target, Some("TODO")));
        assert!(set.is_empty());
        assert_eq!(text, "* TODO Title\n");

        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let set = compile(&doc, &set_todo(target, None)).expect("absent clear");
        assert!(set.is_empty());
    }

    #[test]
    fn set_todo_clear_keeps_star_space_before_tags() {
        let doc = parse("* TODO :a:\n");
        let (_, text) = compile_write(&doc, &set_todo(heading_ref(&doc), None));
        assert_eq!(text, "* :a:\n");
    }

    #[test]
    fn occupied_title_pins() {
        let doc = parse("* TODO Hello\n");
        let target = heading_ref(&doc);
        let (_, text) = compile_write(&doc, &set_title(target, "DONE"));
        assert_eq!(text, "* TODO DONE\n");
        let next = doc.apply(&compile(&doc, &set_title(target, "DONE")).unwrap());
        let h = first_heading(&next);
        assert_eq!(h.todo_keyword().unwrap().text().to_string(), "TODO");
        assert_eq!(
            next.slice(h.part_content_range(HeadingPart::Title).unwrap()),
            "DONE"
        );

        let doc = parse("* Hello\n");
        let err = compile(&doc, &set_title(heading_ref(&doc), "DONE")).unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::TodoKeyword));

        let doc = parse("* TODO Hello\n");
        let err = compile(&doc, &set_title(heading_ref(&doc), "foo :a:")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::TrailingTagGroup)
        );

        let doc = parse("* TODO Hello :a:\n");
        let err = compile(&doc, &set_title(heading_ref(&doc), "foo :a:")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::TrailingTagGroup)
        );
    }

    #[test]
    fn prefix_clear_rechecks_title() {
        let doc = parse("* TODO DONE\n");
        let err = compile(&doc, &set_todo(heading_ref(&doc), None)).unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::TodoKeyword));

        let doc = parse("* TODO Hello\n");
        let (_, text) = compile_write(&doc, &set_todo(heading_ref(&doc), None));
        assert_eq!(text, "* Hello\n");

        let doc = parse("* [#A] [#B] rest\n");
        let err = compile(&doc, &set_priority(heading_ref(&doc), None)).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::PriorityCookie)
        );

        let doc = parse("* COMMENT COMMENT\n");
        let err = compile(&doc, &set_commented(heading_ref(&doc), false)).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::CommentMarker)
        );
    }

    #[test]
    fn compile_all_clear_todo_and_set_title_done_recarves() {
        let doc = parse("* TODO Hello\n");
        let target = heading_ref(&doc);
        let ops = [set_todo(target, None), set_title(target, "DONE")];
        for op in &ops {
            super::super::assert_envelope(&doc, op, &compile(&doc, op).expect("compile"));
        }
        let set = compile_all(&doc, &ops).expect("same-snapshot batch");
        super::super::assert_compile_apply(&doc, &set);
        assert_eq!(set.apply_to_text(doc.source()), "* DONE\n");
        let next = doc.apply(&set);
        let h = first_heading(&next);
        assert_eq!(h.todo_keyword().unwrap().text().to_string(), "DONE");
        assert_eq!(
            next.slice(h.part_content_range(HeadingPart::Title).unwrap()),
            ""
        );
    }

    #[test]
    fn sequential_clear_then_set_title_done_rejects() {
        let doc = parse("* TODO Hello\n");
        let target = heading_ref(&doc);
        let set = compile(&doc, &set_todo(target, None)).expect("clear");
        let next = doc.apply(&set);
        let target = apply_locating(&target, &set, &next).expect("heading remains");
        let err = compile(&next, &set_title(target, "DONE")).unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::TodoKeyword));
    }

    #[test]
    fn compile_all_two_present_rewrites() {
        let doc = parse("* TODO Title :a:\n");
        let target = heading_ref(&doc);
        let ops = [set_todo(target, Some("DONE")), set_tags(target, &["b"])];
        for op in &ops {
            super::super::assert_envelope(&doc, op, &compile(&doc, op).expect("compile"));
        }
        let set = compile_all(&doc, &ops).expect("disjoint present fields");
        assert_eq!(set.len(), 2);
        assert_eq!(set.apply_to_text(doc.source()), "* DONE Title :b:\n");
    }

    #[test]
    fn compile_all_two_absent_prefix_fields_conflict() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let err = compile_all(
            &doc,
            &[
                set_priority(target, Some(PriorityAtom::Letter('A'))),
                set_todo(target, Some("TODO")),
            ],
        )
        .unwrap_err();
        let dest = first_heading(&doc)
            .part_insert_point(HeadingPart::Todo)
            .expect("absent todo");
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: dest.offset()
            }
        );
    }

    #[test]
    fn compile_all_set_title_plus_absent_todo_either_order() {
        let doc = parse("* Hello\n");
        let target = heading_ref(&doc);
        let title_then_todo = [set_title(target, "World"), set_todo(target, Some("TODO"))];
        let todo_then_title = [set_todo(target, Some("TODO")), set_title(target, "World")];
        for ops in [&title_then_todo[..], &todo_then_title[..]] {
            for op in ops {
                super::super::assert_envelope(&doc, op, &compile(&doc, op).expect("compile"));
            }
            let set = compile_all(&doc, ops).expect("insert then title replace");
            assert_eq!(set.apply_to_text(doc.source()), "* TODO World\n");
        }

        let err = compile_all(
            &doc,
            &[set_title(target, "DONE"), set_todo(target, Some("TODO"))],
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::TodoKeyword));
    }

    #[test]
    fn sequential_todo_then_tags_via_apply_locating() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let set = compile(&doc, &set_todo(target, Some("TODO"))).expect("todo");
        let next = doc.apply(&set);
        let target = apply_locating(&target, &set, &next).expect("heading remains");
        let set = compile(&next, &set_tags(target, &["a"])).expect("tags");
        let next = next.apply(&set);
        assert_eq!(next.write(), "* TODO Title :a:\n");
        assert!(ptr::eq(next.write(), next.source()));
    }

    #[test]
    fn set_priority_cookie_and_range() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let (_, text) = compile_write(&doc, &set_priority(target, Some(PriorityAtom::Letter('A'))));
        assert_eq!(text, "* [#A] Title\n");

        let doc = parse("* [#A] Title\n");
        let target = heading_ref(&doc);
        let (set, _) = compile_write(&doc, &set_priority(target, Some(PriorityAtom::Letter('A'))));
        assert!(set.is_empty());

        let doc = parse("* [#A] Title\n");
        let (_, text) = compile_write(&doc, &set_priority(heading_ref(&doc), None));
        assert_eq!(text, "* Title\n");

        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let err =
            compile(&doc, &set_priority(target, Some(PriorityAtom::Letter('a')))).unwrap_err();
        assert_eq!(err, OperationError::InvalidPriority);

        let err = compile(&doc, &set_priority(target, Some(PriorityAtom::Number(65)))).unwrap_err();
        assert_eq!(err, OperationError::InvalidPriority);

        let doc = parse("#+PRIORITIES: A C B\n* Title\n");
        let err = compile(
            &doc,
            &set_priority(heading_ref(&doc), Some(PriorityAtom::Letter('D'))),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::PriorityOutOfRange(PriorityAtom::Letter('D'))
        );

        let (_, text) = compile_write(
            &doc,
            &set_priority(heading_ref(&doc), Some(PriorityAtom::Letter('A'))),
        );
        assert_eq!(text, "#+PRIORITIES: A C B\n* [#A] Title\n");
    }

    #[test]
    fn set_tags_insert_clear_charset_and_noop() {
        let doc = parse("* Title\n");
        let (_, text) = compile_write(&doc, &set_tags(heading_ref(&doc), &["a", "b"]));
        assert_eq!(text, "* Title :a:b:\n");

        let doc = parse("* \n");
        let (_, text) = compile_write(&doc, &set_tags(heading_ref(&doc), &["a"]));
        assert_eq!(text, "* :a:\n");

        let doc = parse("* Title :a:b:\n");
        let (set, _) = compile_write(&doc, &set_tags(heading_ref(&doc), &["a", "b"]));
        assert!(set.is_empty());

        let doc = parse("* Title :a:\n");
        let (_, text) = compile_write(&doc, &set_tags(heading_ref(&doc), &[]));
        assert_eq!(text, "* Title \n");
        let next = parse(&text);
        assert!(first_heading(&next).tags().is_none());
        assert_eq!(
            next.slice(
                first_heading(&next)
                    .part_content_range(HeadingPart::Title)
                    .unwrap()
            ),
            "Title"
        );

        let doc = parse("* Title\n");
        let err = compile(&doc, &set_tags(heading_ref(&doc), &["a b"])).unwrap_err();
        assert!(matches!(err, OperationError::InvalidTag(s) if s == "a b"));

        let (_, text) = compile_write(&doc, &set_tags(heading_ref(&doc), &["ARCHIVE"]));
        assert_eq!(text, "* Title :ARCHIVE:\n");

        let doc = parse("* Title :a::b:\n");
        let (set, _) = compile_write(&doc, &set_tags(heading_ref(&doc), &["a", "b"]));
        assert!(set.is_empty(), "no-op compares names, not the construct");
    }

    #[test]
    fn set_title_empty_and_noop_and_newline() {
        let doc = parse("* Hello\n");
        let target = heading_ref(&doc);
        let (set, _) = compile_write(&doc, &set_title(target, "Hello"));
        assert!(set.is_empty());

        let (_, text) = compile_write(&doc, &set_title(target, ""));
        assert_eq!(text, "* \n");

        let err = compile(&doc, &set_title(target, "a\nb")).unwrap_err();
        assert_eq!(err, OperationError::InvalidTitle(TitleProblem::Newline));
        let err = compile(&doc, &set_title(target, " Hello")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::AbsorbedWhitespace)
        );
    }

    #[test]
    fn empty_title_pads_against_prefix_and_tags() {
        let doc = parse("* TODO\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_title(target, "World"));
        assert_eq!(text, "* TODO World\n");
        assert_eq!(
            set.edits()[0].range,
            first_heading(&doc)
                .part_content_range(HeadingPart::Title)
                .unwrap()
        );

        let doc = parse("* COMMENT\n");
        let (_, text) = compile_write(&doc, &set_title(heading_ref(&doc), "World"));
        assert_eq!(text, "* COMMENT World\n");

        let doc = parse("* :a:\n");
        let (_, text) = compile_write(&doc, &set_title(heading_ref(&doc), "Hello"));
        assert_eq!(text, "* Hello :a:\n");

        let doc = parse("* TODO :a:\n");
        let (_, text) = compile_write(&doc, &set_title(heading_ref(&doc), "World"));
        assert_eq!(text, "* TODO World :a:\n");

        let doc = parse("* \n");
        let target = heading_ref(&doc);
        let set = compile(&doc, &set_todo(target, Some("TODO"))).expect("todo");
        let next = doc.apply(&set);
        let target = apply_locating(&target, &set, &next).expect("heading remains");
        let set = compile(&next, &set_title(target, "World")).expect("title");
        assert_eq!(next.apply(&set).write(), "* TODO World\n");

        let doc = parse("* :a:\n");
        let (_, text) = compile_write(&doc, &set_todo(heading_ref(&doc), Some("TODO")));
        assert_eq!(text, "* TODO :a:\n");
    }

    #[test]
    fn compile_all_empty_title_and_absent_todo_conflicts() {
        let doc = parse("* \n");
        let target = heading_ref(&doc);
        let err = compile_all(
            &doc,
            &[set_title(target, "New"), set_todo(target, Some("TODO"))],
        )
        .unwrap_err();
        let dest = first_heading(&doc)
            .part_insert_point(HeadingPart::Todo)
            .expect("absent todo");
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: dest.offset()
            }
        );
    }

    #[test]
    fn set_title_keeps_existing_tags() {
        let doc = parse("* TODO Hello :a:\n");
        let target = heading_ref(&doc);
        let (set, text) = compile_write(&doc, &set_title(target, "World"));
        assert_eq!(text, "* TODO World :a:\n");
        assert_eq!(
            set.edits()[0].range,
            first_heading(&doc)
                .part_content_range(HeadingPart::Title)
                .unwrap()
        );
        let next = doc.apply(&set);
        let tags: Vec<_> = first_heading(&next)
            .tags()
            .unwrap()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        assert_eq!(tags, ["a"]);
    }

    #[test]
    fn set_title_occupied_prefix_matrix() {
        let doc = parse("* Hello\n");
        let err = compile(&doc, &set_title(heading_ref(&doc), "[#A] rest")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::PriorityCookie)
        );
        let err = compile(&doc, &set_title(heading_ref(&doc), "COMMENT foo")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::CommentMarker)
        );

        let doc = parse("* [#A] Hello\n");
        let (_, text) = compile_write(&doc, &set_title(heading_ref(&doc), "[#B] rest"));
        assert_eq!(text, "* [#A] [#B] rest\n");

        let doc = parse("* COMMENT Hello\n");
        let (_, text) = compile_write(&doc, &set_title(heading_ref(&doc), "COMMENT foo"));
        assert_eq!(text, "* COMMENT COMMENT foo\n");

        let doc = parse("* TODO Hello\n");
        let err = compile(&doc, &set_title(heading_ref(&doc), "[#A] rest")).unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTitle(TitleProblem::PriorityCookie)
        );
    }

    #[test]
    fn set_todo_does_not_recheck_keyword_shaped_title() {
        let doc = parse("* [#A] DONE\n");
        let (_, text) = compile_write(&doc, &set_todo(heading_ref(&doc), Some("TODO")));
        // Dest is title start (after the present cookie), not Org field order.
        assert_eq!(text, "* [#A] TODO DONE\n");
    }

    #[test]
    fn sequential_absent_prefix_fields() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let set = compile(&doc, &set_priority(target, Some(PriorityAtom::Letter('A'))))
            .expect("priority");
        let next = doc.apply(&set);
        let target = apply_locating(&target, &set, &next).expect("heading remains");
        let set = compile(&next, &set_todo(target, Some("TODO"))).expect("todo");
        // Dest is title start, after the cookie now present.
        assert_eq!(next.apply(&set).write(), "* [#A] TODO Title\n");
    }

    #[test]
    fn set_commented_insert_clear_noop() {
        let doc = parse("* Title\n");
        let (_, text) = compile_write(&doc, &set_commented(heading_ref(&doc), true));
        assert_eq!(text, "* COMMENT Title\n");

        let doc = parse("* COMMENT Title\n");
        let (set, _) = compile_write(&doc, &set_commented(heading_ref(&doc), true));
        assert!(set.is_empty());

        let (_, text) = compile_write(&doc, &set_commented(heading_ref(&doc), false));
        assert_eq!(text, "* Title\n");

        let doc = parse("* Title\n");
        let set = compile(&doc, &set_commented(heading_ref(&doc), false)).expect("already false");
        assert!(set.is_empty());
    }

    #[test]
    fn unknown_todo_and_file_vocab() {
        let doc = parse("* Title\n");
        let target = heading_ref(&doc);
        let err = compile(&doc, &set_todo(target, Some("NEXT"))).unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword("NEXT".into()));
        let err = compile(&doc, &set_todo(target, Some(""))).unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword(String::new()));
        let err = compile(&doc, &set_todo(target, Some("TO DO"))).unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword("TO DO".into()));

        let doc = parse("#+TODO: WAIT | DONE\n* Title\n");
        let target = heading_ref(&doc);
        let (_, text) = compile_write(&doc, &set_todo(target, Some("WAIT")));
        assert_eq!(text, "#+TODO: WAIT | DONE\n* WAIT Title\n");
        let err = compile(&doc, &set_todo(target, Some("TODO"))).unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword("TODO".into()));

        let cfg = ParseConfig::default()
            .with_todo_keywords([("NEXT", TodoKind::Active), ("CANCELLED", TodoKind::Done)]);
        let doc = parse_with("* Title\n", &cfg);
        let (_, text) = compile_write(&doc, &set_todo(heading_ref(&doc), Some("NEXT")));
        assert_eq!(text, "* NEXT Title\n");
    }

    #[test]
    fn outline_and_inline_task_targets() {
        let doc = parse("* Title\n");
        let outline = doc.root_outlines().next().expect("outline");
        let (_, text) = compile_write(&doc, &set_todo(NodeRef::of(&outline), Some("TODO")));
        assert_eq!(text, "* TODO Title\n");

        let src = "*************** Title\n*************** END\n";
        let doc = parse(src);
        let it = first_inline_task(&doc);
        let (_, text) = compile_write(&doc, &set_todo(NodeRef::of(&it), Some("TODO")));
        assert_eq!(text, "*************** TODO Title\n*************** END\n");

        let body = parse("* H\nbody\n");
        let heading = first_heading(&body);
        let body_off = heading.range().end();
        let err = compile(
            &body,
            &set_todo(NodeRef::at(body_off, OrgSyntaxKind::HEADING), Some("TODO")),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);

        let err = compile(
            &body,
            &set_todo(NodeRef::at(body_off, OrgSyntaxKind::DOCUMENT), Some("TODO")),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            OperationError::WrongKind { actual, .. } if actual == OrgSyntaxKind::DOCUMENT
        ));
    }

    #[test]
    fn stale_target() {
        let doc = parse("* Title\n");
        let err = compile(
            &doc,
            &set_todo(
                NodeRef::at(crate::TextSize::from(99), OrgSyntaxKind::HEADING),
                Some("TODO"),
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);
    }

    #[test]
    fn numeric_priority_fragment() {
        let doc = parse("* Title\n");
        let (_, text) = compile_write(
            &doc,
            &set_priority(heading_ref(&doc), Some(PriorityAtom::Number(12))),
        );
        assert_eq!(text, "* [#12] Title\n");
    }

    #[test]
    fn compile_all_present_rewrite_sort_order() {
        let doc = parse("* TODO Title :a:\n");
        let target = heading_ref(&doc);
        let fwd = [set_todo(target, Some("DONE")), set_tags(target, &["b"])];
        let rev = [set_tags(target, &["b"]), set_todo(target, Some("DONE"))];
        for ops in [&fwd[..], &rev[..]] {
            let set = compile_all(&doc, ops).expect("order independent");
            assert_eq!(set.apply_to_text(doc.source()), "* DONE Title :b:\n");
        }
    }

    #[test]
    fn overlapping_title_and_present_todo_is_touching_not_overlap() {
        let doc = parse("* TODO Hello\n");
        let target = heading_ref(&doc);
        let set = compile_all(
            &doc,
            &[set_todo(target, Some("DONE")), set_title(target, "World")],
        )
        .expect("construct then title content");
        assert_eq!(set.apply_to_text(doc.source()), "* DONE World\n");
    }
}
