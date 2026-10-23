//! Envelope checks for every operation, applying settings edits, and
//! locating a target across sequential applies.
//!
//! - Only pre-edit `Edit.range` values are checked: headline field edits
//!   stay on the star line, not anywhere in `Task::range()`, and
//!   `SetTodoState` never replaces the whole line.
//! - Empty-range inserts land only at the insert point the operation names.
//! - Over `TAXONOMY`, no-op compiles must still apply to identical text and
//!   pass the envelope. Link operations that depend on shape may only fail
//!   with `PartNotApplicable`.

use super::{assert_compile_apply, assert_envelope, compile};
use crate::ast::{
    AstNode, CheckboxState, HasParts, Heading, HeadingPart, Headline, InlineTask, Item, Keyword,
    KeywordPart, Link, LinkShape, Outline, Task,
};
use crate::tests::{Cfg, TAXONOMY};
use crate::{
    Document, HeadingInput, NodeRef, Operation, OperationError, OrgSyntaxKind, PlanningPatch,
    PriorityAtom, PropertyEdit, SlotPatch, SubtreeDest, TextRange, TextSize, TimestampInput,
    TimestampPoint, TimestampShape, TodoKind, apply_locating, parse, parse_with,
};

fn first_heading(doc: &Document) -> Heading {
    doc.map::<Heading>().next().expect("heading")
}

fn first_item(doc: &Document) -> Item {
    doc.map::<Item>().next().expect("item")
}

fn first_keyword(doc: &Document) -> Keyword {
    doc.map::<Keyword>().next().expect("keyword")
}

fn first_link(doc: &Document) -> Link {
    doc.map::<Link>().next().expect("link")
}

fn heading_ref(doc: &Document) -> NodeRef {
    NodeRef::of(&first_heading(doc))
}

fn ymd(year: u16, month: u8, day: u8) -> TimestampInput {
    TimestampInput {
        shape: TimestampShape::Active,
        start: Some(TimestampPoint {
            year,
            month,
            day,
            hour: None,
            minute: None,
        }),
        end: None,
        repeater: None,
        delay: None,
        diary_sexp: None,
    }
}

fn patch(
    deadline: SlotPatch<TimestampInput>,
    scheduled: SlotPatch<TimestampInput>,
    closed: SlotPatch<TimestampInput>,
) -> PlanningPatch {
    PlanningPatch {
        deadline,
        scheduled,
        closed,
    }
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

fn set_prop(target: NodeRef, key: &str, edit: PropertyEdit) -> Operation {
    Operation::SetProperty {
        target,
        key: key.to_string(),
        edit,
    }
}

fn set_plan(target: NodeRef, patch: PlanningPatch) -> Operation {
    Operation::SetPlanning { target, patch }
}

fn set_keyword(target: NodeRef, value: &str) -> Operation {
    Operation::SetKeyword {
        target,
        value: value.to_string(),
    }
}

fn toggle(target: NodeRef) -> Operation {
    Operation::ToggleCheckbox { target }
}

fn set_path(target: NodeRef, path: &str) -> Operation {
    Operation::SetLinkPath {
        target,
        path: path.to_string(),
    }
}

fn set_desc(target: NodeRef, description: Option<&str>) -> Operation {
    Operation::SetLinkDescription {
        target,
        description: description.map(str::to_string),
    }
}

fn convert(target: NodeRef, to: LinkShape) -> Operation {
    Operation::ConvertLinkShape { target, to }
}

fn replace(start: u32, end: u32, repl: &str) -> Operation {
    Operation::ReplaceText {
        range: TextRange::new(TextSize::from(start), TextSize::from(end)),
        replacement: repl.to_string(),
    }
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

fn structure_ops(doc: &Document, outline: &Outline) -> Vec<Operation> {
    let target = NodeRef::of(outline);
    let min = doc.config().inline_task_min_level() as usize;
    let mut ops = vec![
        Operation::Promote {
            target,
            with_subtree: true,
            by: 0,
        },
        Operation::Demote {
            target,
            with_subtree: false,
            by: 0,
        },
        Operation::CloneSubtree {
            target,
            to: SubtreeDest::After(target),
        },
        Operation::MoveSubtree {
            target,
            to: SubtreeDest::After(target),
        },
        Operation::InsertHeading {
            at: SubtreeDest::After(target),
            heading: heading_input(outline.level(), "New"),
        },
        Operation::DeleteSubtree { target },
    ];
    if outline.level() > 1 {
        ops.push(Operation::Promote {
            target,
            with_subtree: false,
            by: 1,
        });
        ops.push(Operation::Promote {
            target,
            with_subtree: true,
            by: 1,
        });
    }
    if outline.level().checked_add(1).is_some_and(|n| n < min) {
        ops.push(Operation::Demote {
            target,
            with_subtree: false,
            by: 1,
        });
    }
    ops
}

fn apply_op(doc: &Document, op: &Operation, live: &mut [&mut NodeRef]) -> Document {
    let set = compile(doc, op).unwrap_or_else(|e| panic!("compile on {:?}: {e}", doc.source()));
    assert_envelope(doc, op, &set);
    let next = doc.apply(&set);
    for r in live {
        **r = apply_locating(r, &set, &next).expect("ref survives");
    }
    next
}

fn check(src: &str, make: impl FnOnce(&Document) -> Operation) {
    let doc = parse(src);
    check_compiled(&doc, &make(&doc));
}

fn check_compiled(doc: &Document, op: &Operation) {
    let set = compile(doc, op).unwrap_or_else(|e| panic!("compile on {:?}: {e}", doc.source()));
    assert_compile_apply(doc, &set);
    assert_envelope(doc, op, &set);
}

fn try_part_applicable(doc: &Document, op: &Operation) {
    match compile(doc, op) {
        Ok(set) => {
            assert_compile_apply(doc, &set);
            assert_envelope(doc, op, &set);
        }
        Err(OperationError::PartNotApplicable) => {}
        Err(e) => panic!("compile on {:?}: {e}", doc.source()),
    }
}

fn heading_ops(doc: &Document, task: &Task) -> Vec<Operation> {
    let target = NodeRef::of(task);
    let current = task.todo_keyword().map(|k| k.text().to_string());
    let names: Vec<&str> = doc
        .todo_sequences()
        .iter()
        .flat_map(crate::TodoSequence::keywords)
        .map(String::as_str)
        .collect();
    let mut ops = Vec::new();
    if let Some(state) = names
        .iter()
        .copied()
        .find(|&n| Some(n) != current.as_deref())
        .or_else(|| names.first().copied())
    {
        ops.push(set_todo(target, Some(state)));
    }
    ops.push(set_todo(target, None));
    ops.push(set_priority(target, Some(PriorityAtom::Letter('A'))));
    ops.push(set_priority(target, None));
    ops.push(set_tags(target, &["t"]));
    ops.push(set_tags(target, &[]));
    ops.push(set_title(target, "Renamed"));
    ops.push(set_commented(target, true));
    ops.push(set_commented(target, false));
    ops.push(set_prop(
        target,
        "KEY",
        PropertyEdit::Set {
            value: Some("v".into()),
        },
    ));
    ops.push(set_plan(
        target,
        patch(
            SlotPatch::Set(ymd(2020, 1, 2)),
            SlotPatch::Keep,
            SlotPatch::Keep,
        ),
    ));
    if let Some(title) = task.part_content_range(HeadingPart::Title) {
        ops.push(Operation::ReplaceText {
            range: title,
            replacement: doc.slice(title).to_string(),
        });
    }
    ops
}

#[test]
fn only_compiled_ranges_change_headline() {
    check("* Title\n", |doc| set_todo(heading_ref(doc), Some("TODO")));
    check("* TODO Title\n", |doc| {
        set_todo(heading_ref(doc), Some("DONE"))
    });
    check("* TODO Title\n", |doc| set_todo(heading_ref(doc), None));
    check("* TODO Title\n", |doc| {
        set_todo(heading_ref(doc), Some("TODO"))
    });
    check("* Title\n", |doc| {
        set_priority(heading_ref(doc), Some(PriorityAtom::Letter('A')))
    });
    check("* Title\n", |doc| set_tags(heading_ref(doc), &["a"]));
    check("* Hello\n", |doc| set_title(heading_ref(doc), "World"));
    check("* \n", |doc| set_title(heading_ref(doc), "Hello"));
    check("* :a:\n", |doc| set_title(heading_ref(doc), "Hello"));
    check("* Title\n", |doc| set_commented(heading_ref(doc), true));
    check("* H", |doc| set_tags(heading_ref(doc), &["a"]));
    check("#+TODO: WAIT | DONE\n* Title\n", |doc| {
        set_todo(heading_ref(doc), Some("WAIT"))
    });
    check("#+PRIORITIES: A C B\n* Title\n", |doc| {
        set_priority(heading_ref(doc), Some(PriorityAtom::Letter('A')))
    });
    check("* Parent\n** Child\n", |doc| {
        let child = doc.map::<Heading>().nth(1).expect("child");
        set_todo(NodeRef::of(&child), Some("TODO"))
    });
    check("*************** Title\n*************** END\n", |doc| {
        let task = doc.map::<InlineTask>().next().expect("inline task");
        set_todo(NodeRef::of(&task), Some("TODO"))
    });
}

#[test]
fn only_compiled_ranges_change_metadata() {
    check("* H\n", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 1)),
                SlotPatch::Keep,
            ),
        )
    });
    check("* H\nSCHEDULED: <2020-01-01 Wed>\n", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 2)),
                SlotPatch::Keep,
            ),
        )
    });
    check(
        "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n",
        |doc| {
            set_plan(
                heading_ref(doc),
                patch(SlotPatch::Clear, SlotPatch::Keep, SlotPatch::Keep),
            )
        },
    );
    check("* H\nDEADLINE: <2020-01-01 Wed> SCHEDULED:\n", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 2)),
                SlotPatch::Keep,
            ),
        )
    });
    check("* H\nSCHEDULED: <2020-01-01 Wed>\n", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Set(ymd(2020, 1, 2)),
                SlotPatch::Set(ymd(2020, 1, 3)),
                SlotPatch::Keep,
            ),
        )
    });
    check("* H", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 1)),
                SlotPatch::Keep,
            ),
        )
    });
    check("* H\nDEADLINE:\n", |doc| {
        set_plan(
            heading_ref(doc),
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 1)),
                SlotPatch::Keep,
            ),
        )
    });
    check("* H\n", |doc| {
        set_prop(heading_ref(doc), "KEY", PropertyEdit::Set { value: None })
    });
    check("* H\n:PROPERTIES:\n:FOO: v\n:END:\n", |doc| {
        set_prop(
            heading_ref(doc),
            "FOO",
            PropertyEdit::Set {
                value: Some("w".into()),
            },
        )
    });
    check("* H\n:PROPERTIES:\n:KEY:\n:END:\n", |doc| {
        set_prop(heading_ref(doc), "KEY", PropertyEdit::Delete)
    });
    check("* H\n:PROPERTIES:\n:FOO: v\n:END:\n", |doc| {
        set_prop(
            heading_ref(doc),
            "BAR",
            PropertyEdit::Set {
                value: Some("1".into()),
            },
        )
    });
}

#[test]
fn only_compiled_ranges_change_structure() {
    check("* A\n", |_| Operation::InsertHeading {
        at: SubtreeDest::DocumentEnd,
        heading: heading_input(1, "B"),
    });
    check("* A\n* B\n", |doc| Operation::DeleteSubtree {
        target: NodeRef::of(&doc.root_outlines().next().expect("A")),
    });
    check("* A\n* B\n", |doc| {
        let roots: Vec<_> = doc.root_outlines().collect();
        Operation::MoveSubtree {
            target: NodeRef::of(&roots[0]),
            to: SubtreeDest::After(NodeRef::of(&roots[1])),
        }
    });
    check("* H\n", |doc| {
        let target = NodeRef::of(&doc.root_outlines().next().expect("H"));
        Operation::CloneSubtree {
            target,
            to: SubtreeDest::After(target),
        }
    });
    check("* H\n", |doc| {
        let target = NodeRef::of(&doc.root_outlines().next().expect("H"));
        Operation::CloneSubtree {
            target,
            to: SubtreeDest::FirstChildOf(target),
        }
    });
    check("* A\n** B\n", |doc| Operation::Promote {
        target: NodeRef::of(
            &doc.root_outlines()
                .next()
                .expect("A")
                .children()
                .next()
                .expect("B"),
        ),
        with_subtree: false,
        by: 1,
    });
    check("* A\n", |doc| Operation::Demote {
        target: NodeRef::of(&doc.root_outlines().next().expect("A")),
        with_subtree: false,
        by: 1,
    });
    check("* A\n** B\n*** C\n", |doc| Operation::Promote {
        target: NodeRef::of(
            &doc.root_outlines()
                .next()
                .expect("A")
                .children()
                .next()
                .expect("B"),
        ),
        with_subtree: true,
        by: 1,
    });
    check("* A\n** B\n*** C\n", |doc| Operation::Demote {
        target: NodeRef::of(&doc.root_outlines().next().expect("A")),
        with_subtree: true,
        by: 1,
    });
}

#[test]
fn only_compiled_ranges_change_objects_and_text() {
    check("- item\n", |doc| toggle(NodeRef::of(&first_item(doc))));
    check("- [ ] item\n", |doc| toggle(NodeRef::of(&first_item(doc))));
    check("#+TITLE: hello\n", |doc| {
        set_keyword(NodeRef::of(&first_keyword(doc)), "world")
    });
    check("#+TITLE:\n", |doc| {
        set_keyword(NodeRef::of(&first_keyword(doc)), "hello")
    });
    check("#+TITLE:", |doc| {
        set_keyword(NodeRef::of(&first_keyword(doc)), "hello")
    });
    check("#+TODO: WAIT | DONE\n", |doc| {
        set_keyword(NodeRef::of(&first_keyword(doc)), "BUG | FIXED")
    });
    check("[[https://x.com]]\n", |doc| {
        set_path(NodeRef::of(&first_link(doc)), "https://y.com")
    });
    check("[[path]]\n", |doc| {
        set_desc(NodeRef::of(&first_link(doc)), Some("site"))
    });
    check("[[https://x.com][site]]\n", |doc| {
        set_desc(NodeRef::of(&first_link(doc)), Some("other"))
    });
    check("[[https://x.com][site]]\n", |doc| {
        convert(NodeRef::of(&first_link(doc)), LinkShape::Angle)
    });
    check("<https://x.com>\n", |doc| {
        set_path(NodeRef::of(&first_link(doc)), "https://y.com")
    });
    check("[[https://x.com]]\n", |doc| {
        convert(NodeRef::of(&first_link(doc)), LinkShape::Angle)
    });
    check("hello", |_| replace(1, 4, "ipp"));
    check("hello", |_| replace(0, 5, "hello"));
    check("ab", |_| replace(1, 1, "X"));
}

#[test]
fn only_compiled_ranges_change_taxonomy() {
    for &(src, cfg) in TAXONOMY {
        let doc = parse_with(src, &cfg.build());
        match cfg {
            Cfg::CustomTodo => {
                let heading = heading_ref(&doc);
                check_compiled(&doc, &set_todo(heading, Some("NEXT")));
                let err = compile(&doc, &set_todo(heading, Some("TODO"))).unwrap_err();
                assert_eq!(err, OperationError::UnknownTodoKeyword("TODO".into()));
            }
            Cfg::InlineTaskLevel3 => {
                assert!(
                    doc.map::<InlineTask>().next().is_some(),
                    "level-3 fixture must carve INLINE_TASK: {src:?}"
                );
            }
            Cfg::Default => {}
        }
        for task in doc.map::<Task>() {
            for op in heading_ops(&doc, &task) {
                check_compiled(&doc, &op);
            }
        }
        for outline in doc.map::<Outline>() {
            for op in structure_ops(&doc, &outline) {
                check_compiled(&doc, &op);
            }
        }
        check_compiled(
            &doc,
            &Operation::InsertHeading {
                at: SubtreeDest::DocumentStart,
                heading: heading_input(1, "Head"),
            },
        );
        check_compiled(
            &doc,
            &Operation::InsertHeading {
                at: SubtreeDest::DocumentEnd,
                heading: heading_input(1, "Tail"),
            },
        );
        for item in doc.map::<Item>() {
            check_compiled(&doc, &toggle(NodeRef::of(&item)));
        }
        for keyword in doc.map::<Keyword>() {
            check_compiled(&doc, &set_keyword(NodeRef::of(&keyword), "x"));
        }
        for link in doc.map::<Link>() {
            let target = NodeRef::of(&link);
            check_compiled(&doc, &set_path(target, "https://z.com"));
            try_part_applicable(&doc, &set_desc(target, Some("d")));
            try_part_applicable(&doc, &convert(target, LinkShape::Bracket));
        }
        let len = u32::try_from(src.len()).expect("fixture fits TextSize");
        check_compiled(&doc, &replace(0, len, src));
    }
}

#[test]
fn replace_text_insert_todo_setting_re_resolves() {
    let doc = parse("* Title\n");
    let heading = heading_ref(&doc);
    check_compiled(&doc, &set_todo(heading, Some("TODO")));
    let err = compile(&doc, &set_todo(heading, Some("WAIT"))).unwrap_err();
    assert_eq!(err, OperationError::UnknownTodoKeyword("WAIT".into()));

    let insert = replace(0, 0, "#+TODO: WAIT | DONE\n");
    let set = compile(&doc, &insert).expect("insert setting");
    assert_envelope(&doc, &insert, &set);
    let next = doc.apply(&set);
    assert_eq!(next.config().todo_kind("WAIT"), Some(TodoKind::Active));
    assert_eq!(next.config().todo_kind("TODO"), None);
    let heading = apply_locating(&heading, &set, &next).expect("heading shifts with a lead-in");
    check_compiled(&next, &set_todo(heading, Some("WAIT")));
    let err = compile(&next, &set_todo(heading, Some("TODO"))).unwrap_err();
    assert_eq!(err, OperationError::UnknownTodoKeyword("TODO".into()));
    let applied = next.apply(&compile(&next, &set_todo(heading, Some("WAIT"))).expect("WAIT"));
    let h = first_heading(&applied);
    assert_eq!(h.todo_keyword().unwrap().text().to_string(), "WAIT");
    assert_eq!(applied.todo_kind(&h), Some(TodoKind::Active));
}

#[test]
fn priorities_projection_follows_keyword_and_replace_text() {
    let src = "#+PRIORITIES: A C B\n* Title\n";
    let d = PriorityAtom::Letter('D');
    let a = PriorityAtom::Letter('A');

    for via_keyword in [true, false] {
        let doc = parse(src);
        let heading = heading_ref(&doc);
        check_compiled(&doc, &set_priority(heading, Some(a)));
        let err = compile(&doc, &set_priority(heading, Some(d))).unwrap_err();
        assert_eq!(err, OperationError::PriorityOutOfRange(d));

        let kw = doc
            .map::<Keyword>()
            .find(|k| k.key_eq("PRIORITIES"))
            .expect("PRIORITIES line");
        let widen = if via_keyword {
            set_keyword(NodeRef::of(&kw), "A E C")
        } else {
            let content = kw
                .part_content_range(KeywordPart::Value)
                .expect("PRIORITIES value");
            Operation::ReplaceText {
                range: content,
                replacement: "A E C".into(),
            }
        };
        let set = compile(&doc, &widen).expect("widen range");
        assert_envelope(&doc, &widen, &set);
        let next = doc.apply(&set);
        assert!(next.priority_range().expect("live range").contains(d));
        let heading = apply_locating(&heading, &set, &next).expect("heading remains");
        check_compiled(&next, &set_priority(heading, Some(d)));

        let kw = next
            .map::<Keyword>()
            .find(|k| k.key_eq("PRIORITIES"))
            .expect("PRIORITIES line");
        let shrink = if via_keyword {
            set_keyword(NodeRef::of(&kw), "A C B")
        } else {
            let content = kw
                .part_content_range(KeywordPart::Value)
                .expect("PRIORITIES value");
            Operation::ReplaceText {
                range: content,
                replacement: "A C B".into(),
            }
        };
        let set = compile(&next, &shrink).expect("restore range");
        let restored = next.apply(&set);
        let heading = apply_locating(&heading, &set, &restored).expect("heading remains");
        let err = compile(&restored, &set_priority(heading, Some(d))).unwrap_err();
        assert_eq!(err, OperationError::PriorityOutOfRange(d));
        check_compiled(&restored, &set_priority(heading, Some(a)));
    }
}

#[test]
fn sequential_clone_then_todo_on_source_via_apply_locating() {
    let doc = parse("* Title\n");
    let mut target = heading_ref(&doc);
    let doc = apply_op(
        &doc,
        &Operation::CloneSubtree {
            target,
            to: SubtreeDest::After(target),
        },
        &mut [&mut target],
    );
    let doc = apply_op(&doc, &set_todo(target, Some("TODO")), &mut [&mut target]);
    assert_eq!(doc.write(), "* TODO Title\n* Title\n");
}

#[test]
fn sequential_todo_then_property_via_apply_locating() {
    let doc = parse("* Title\n");
    let mut target = heading_ref(&doc);
    let doc = apply_op(&doc, &set_todo(target, Some("TODO")), &mut [&mut target]);
    let doc = apply_op(
        &doc,
        &set_prop(
            target,
            "KEY",
            PropertyEdit::Set {
                value: Some("v".into()),
            },
        ),
        &mut [&mut target],
    );
    assert_eq!(doc.write(), "* TODO Title\n:PROPERTIES:\n:KEY: v\n:END:\n");
    let h = first_heading(&doc);
    assert_eq!(h.todo_keyword().unwrap().text().to_string(), "TODO");
    assert_eq!(doc.property(h.outline(), "KEY").unwrap().value(), "v");
}

#[test]
fn sequential_planning_then_tags_via_apply_locating() {
    let doc = parse("* Title\n");
    let mut target = heading_ref(&doc);
    let doc = apply_op(
        &doc,
        &set_plan(
            target,
            patch(
                SlotPatch::Keep,
                SlotPatch::Set(ymd(2020, 1, 1)),
                SlotPatch::Keep,
            ),
        ),
        &mut [&mut target],
    );
    let doc = apply_op(&doc, &set_tags(target, &["a"]), &mut [&mut target]);
    assert_eq!(doc.write(), "* Title :a:\nSCHEDULED: <2020-01-01 Wed>\n");
    let h = first_heading(&doc);
    let tags: Vec<_> = h
        .tags()
        .unwrap()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    assert_eq!(tags, ["a"]);
    assert!(h.outline().planning().is_some());
}

#[test]
fn sequential_independent_item_then_heading_via_apply_locating() {
    let doc = parse("* Title\n- item\n");
    let mut heading = heading_ref(&doc);
    let mut item = NodeRef::of(&first_item(&doc));
    let doc = apply_op(&doc, &toggle(item), &mut [&mut heading, &mut item]);
    let doc = apply_op(
        &doc,
        &set_todo(heading, Some("TODO")),
        &mut [&mut heading, &mut item],
    );
    assert_eq!(doc.write(), "* TODO Title\n- [X] item\n");
    let item = Item::cast(item.resolve(&doc).expect("item on new snapshot")).expect("ITEM");
    assert_eq!(item.checkbox(), Some(CheckboxState::Checked));
    assert_eq!(
        first_heading(&doc)
            .todo_keyword()
            .unwrap()
            .text()
            .to_string(),
        "TODO"
    );
}

#[test]
fn sequential_independent_keyword_then_heading_via_apply_locating() {
    let doc = parse("#+TITLE: hello\n* Title\n");
    let mut heading = heading_ref(&doc);
    let mut kw = NodeRef::of(&first_keyword(&doc));
    let doc = apply_op(
        &doc,
        &set_keyword(kw, "hello world"),
        &mut [&mut heading, &mut kw],
    );
    let doc = apply_op(
        &doc,
        &set_todo(heading, Some("TODO")),
        &mut [&mut heading, &mut kw],
    );
    assert_eq!(doc.write(), "#+TITLE: hello world\n* TODO Title\n");
    assert!(
        Keyword::cast(kw.resolve(&doc).expect("keyword on new snapshot"))
            .expect("KEYWORD")
            .key_eq("TITLE")
    );
}

#[test]
fn sequential_interior_heading_and_outline_refs_via_apply_locating() {
    let doc = parse("* Hello Title\nbody\n");
    let heading = first_heading(&doc);
    let title = heading
        .part_content_range(HeadingPart::Title)
        .expect("title");
    let mid = title.start() + TextSize::from(1);
    assert!(mid < title.end(), "mid-title caret");
    let body = heading.range().end();
    assert!(body < heading.outline().range().end(), "body in outline");

    let mut of = NodeRef::of(&heading);
    let mut mid_heading = NodeRef::at(mid, OrgSyntaxKind::HEADING);
    let mut body_outline = NodeRef::at(body, OrgSyntaxKind::OUTLINE);
    assert!(mid_heading.resolve(&doc).is_some());
    assert!(body_outline.resolve(&doc).is_some());

    let doc = apply_op(
        &doc,
        &set_todo(of, Some("TODO")),
        &mut [&mut of, &mut mid_heading, &mut body_outline],
    );
    let doc = apply_op(
        &doc,
        &set_tags(of, &["a"]),
        &mut [&mut of, &mut mid_heading, &mut body_outline],
    );
    assert_eq!(doc.write(), "* TODO Hello Title :a:\nbody\n");
}
