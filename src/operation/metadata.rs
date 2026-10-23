//! Property-drawer and planning-line ops.

use super::empty_edit_set;
use super::locate::resolve_task;
use super::{PlanningPatch, PropertyEdit, SlotPatch, TimestampInput, map_synthesis};
use crate::ast::{AstNode, HasParts, Planning, PlanningPart, Task, Timestamp};
use crate::edit::EditSet;
use crate::writer::{
    NodePropertySpec, PlanningKeywordSpec, TimestampSpec, is_valid_property_key,
    render_node_property, render_planning_line, render_planning_slot, render_property_drawer,
    render_timestamp, wrap_insert,
};
use crate::{Document, NodeRef, OperationError, TextRange};

pub(super) fn compile_set_property(
    doc: &Document,
    target: NodeRef,
    key: &str,
    edit: &PropertyEdit,
) -> Result<EditSet, OperationError> {
    if !is_valid_property_key(key) {
        return Err(OperationError::InvalidPropertyKey(key.to_string()));
    }
    let task = resolve_task(doc, target)?;
    match edit {
        PropertyEdit::Delete => compile_property_delete(doc, &task, key),
        PropertyEdit::Set { value } => compile_property_set(doc, &task, key, value.as_deref()),
    }
}

fn compile_property_delete(
    doc: &Document,
    task: &Task,
    key: &str,
) -> Result<EditSet, OperationError> {
    let Some(drawer) = task.property_drawer() else {
        return Ok(empty_edit_set(doc.source()));
    };
    let Some(np) = drawer.get(key) else {
        return Ok(empty_edit_set(doc.source()));
    };
    let remaining = drawer.properties().count();
    let range = if remaining <= 1 {
        drawer.range()
    } else {
        np.range()
    };
    Ok(EditSet::single(doc.source(), range, "")?)
}

fn compile_property_set(
    doc: &Document,
    task: &Task,
    key: &str,
    value: Option<&str>,
) -> Result<EditSet, OperationError> {
    if matches!(value, Some(v) if v.is_empty() || v.contains(['\n', '\r'])) {
        return Err(OperationError::InvalidPropertyValue(key.to_string()));
    }
    if let Some(drawer) = task.property_drawer() {
        if let Some(np) = drawer.get(key) {
            if !np.is_append() && np.value().as_deref() == value {
                return Ok(empty_edit_set(doc.source()));
            }
            let base = np.written_base_key();
            let line = render_node_property(NodePropertySpec {
                key: &base,
                value,
                append: false,
            })
            .map_err(map_synthesis)?;
            return Ok(EditSet::single(doc.source(), np.range(), line)?);
        }
        let line = render_node_property(NodePropertySpec {
            key,
            value,
            append: false,
        })
        .map_err(map_synthesis)?;
        let dest = drawer.new_property_insert_point();
        return Ok(EditSet::single(
            doc.source(),
            TextRange::empty(dest.offset()),
            wrap_insert(dest, &line),
        )?);
    }
    let drawer = render_property_drawer(&[NodePropertySpec {
        key,
        value,
        append: false,
    }])
    .map_err(map_synthesis)?;
    let dest = task.property_drawer_insert_point();
    Ok(EditSet::single(
        doc.source(),
        TextRange::empty(dest.offset()),
        wrap_insert(dest, &drawer),
    )?)
}

pub(super) fn compile_set_planning(
    doc: &Document,
    target: NodeRef,
    patch: &PlanningPatch,
) -> Result<EditSet, OperationError> {
    let task = resolve_task(doc, target)?;
    if matches!(
        (&patch.deadline, &patch.scheduled, &patch.closed),
        (SlotPatch::Keep, SlotPatch::Keep, SlotPatch::Keep)
    ) {
        return Ok(empty_edit_set(doc.source()));
    }

    let planning = task.planning();
    let effective = effective_slots(doc, planning.as_ref(), patch)?;
    if effective.is_empty() {
        return match planning.as_ref() {
            Some(p) => Ok(EditSet::single(doc.source(), p.range(), "")?),
            None => Ok(empty_edit_set(doc.source())),
        };
    }

    let Some(p) = planning.as_ref() else {
        let line = render_planning_line(&effective).map_err(map_synthesis)?;
        let dest = task.planning_insert_point();
        return Ok(EditSet::single(
            doc.source(),
            TextRange::empty(dest.offset()),
            wrap_insert(dest, &line),
        )?);
    };

    let timestamped = usize::from(p.deadline().is_some())
        + usize::from(p.scheduled().is_some())
        + usize::from(p.closed().is_some());
    let mut only: Option<(PlanningPart, &SlotPatch<TimestampInput>)> = None;
    let mut rebuild = timestamped == 0;
    for (part, slot) in patch_slots(patch) {
        if matches!(slot, SlotPatch::Keep) {
            continue;
        }
        if only.is_some() {
            rebuild = true;
            break;
        }
        only = Some((part, slot));
        if matches!(slot, SlotPatch::Clear) && p.slot_is_bare(part) {
            rebuild = true;
        }
    }
    if rebuild {
        let line = render_planning_line(&effective).map_err(map_synthesis)?;
        return Ok(EditSet::single(doc.source(), p.range(), line)?);
    }
    match only {
        Some((part, SlotPatch::Clear)) => surgical_clear(doc, p, part),
        Some((part, SlotPatch::Set(_))) => {
            let Some((_, ts)) = effective.iter().find(|(k, _)| *k == spec_kind(part)) else {
                unreachable!("Set slot is in effective");
            };
            surgical_set(doc, p, part, ts)
        }
        Some((_, SlotPatch::Keep)) | None => {
            unreachable!("all-Keep returned empty; rebuild handled above")
        }
    }
}

fn surgical_clear(
    doc: &Document,
    planning: &Planning,
    part: PlanningPart,
) -> Result<EditSet, OperationError> {
    let Some(range) = planning.part_removal_range(part) else {
        unreachable!("surgical Clear of a timestamped slot has a removal range");
    };
    Ok(EditSet::single(doc.source(), range, "")?)
}

fn surgical_set(
    doc: &Document,
    planning: &Planning,
    part: PlanningPart,
    ts: &str,
) -> Result<EditSet, OperationError> {
    if let Some(existing) = timestamp_of(planning, part) {
        if doc.slice(existing.range()) == ts {
            return Ok(empty_edit_set(doc.source()));
        }
        return Ok(EditSet::single(
            doc.source(),
            existing.range(),
            ts.to_string(),
        )?);
    }
    let dest = planning
        .part_insert_point(part)
        .expect("absent planning slot has an insert dest");
    let body = if planning.slot_is_bare(part) {
        ts.to_string()
    } else {
        render_planning_slot(spec_kind(part), ts)
    };
    Ok(EditSet::single(
        doc.source(),
        TextRange::empty(dest.offset()),
        wrap_insert(dest, &body),
    )?)
}

fn patch_slots(patch: &PlanningPatch) -> [(PlanningPart, &SlotPatch<TimestampInput>); 3] {
    [
        (PlanningPart::Deadline, &patch.deadline),
        (PlanningPart::Scheduled, &patch.scheduled),
        (PlanningPart::Closed, &patch.closed),
    ]
}

fn effective_slots(
    doc: &Document,
    planning: Option<&Planning>,
    patch: &PlanningPatch,
) -> Result<Vec<(PlanningKeywordSpec, String)>, OperationError> {
    let mut out = Vec::new();
    for (part, slot) in patch_slots(patch) {
        match slot {
            SlotPatch::Keep => {
                if let Some(p) = planning
                    && let Some(ts) = timestamp_of(p, part)
                {
                    out.push((spec_kind(part), doc.slice(ts.range()).to_string()));
                }
            }
            SlotPatch::Clear => {}
            SlotPatch::Set(input) => {
                out.push((spec_kind(part), render_input(input)?));
            }
        }
    }
    Ok(out)
}

fn render_input(input: &TimestampInput) -> Result<String, OperationError> {
    render_timestamp(&TimestampSpec {
        shape: input.shape,
        start: input.start,
        end: input.end,
        repeater: input.repeater,
        delay: input.delay,
        diary_sexp: input.diary_sexp.as_deref(),
    })
    .map_err(map_synthesis)
}

fn timestamp_of(planning: &Planning, part: PlanningPart) -> Option<Timestamp> {
    match part {
        PlanningPart::Deadline => planning.deadline(),
        PlanningPart::Scheduled => planning.scheduled(),
        PlanningPart::Closed => planning.closed(),
    }
}

fn spec_kind(part: PlanningPart) -> PlanningKeywordSpec {
    match part {
        PlanningPart::Deadline => PlanningKeywordSpec::Deadline,
        PlanningPart::Scheduled => PlanningKeywordSpec::Scheduled,
        PlanningPart::Closed => PlanningKeywordSpec::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, HasParts, Heading, Outline, PlanningPart};
    use crate::operation::compile_write;
    use crate::{
        NodeRef, Operation, OrgSyntaxKind, TimestampPoint, TimestampProblem, TimestampShape,
        compile, compile_all, parse,
    };

    fn first_heading(doc: &Document) -> Heading {
        doc.map::<Heading>().next().expect("heading")
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

    fn inactive_ymd(year: u16, month: u8, day: u8) -> TimestampInput {
        TimestampInput {
            shape: TimestampShape::Inactive,
            start: Some(TimestampPoint {
                year,
                month,
                day,
                hour: Some(12),
                minute: Some(0),
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

    #[test]
    fn set_property_insert_drawer_valueless() {
        let doc = parse("* H\n");
        let (set, text) = compile_write(
            &doc,
            &set_prop(heading_ref(&doc), "KEY", PropertyEdit::Set { value: None }),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(
            set.edits()[0].range.start(),
            first_heading(&doc).range().end()
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:KEY:\n:END:\n");
    }

    #[test]
    fn set_property_valued_valueless_roundtrip_and_last_delete() {
        let doc = parse("* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
        let target = heading_ref(&doc);
        let (_, text) = compile_write(
            &doc,
            &set_prop(target, "FOO", PropertyEdit::Set { value: None }),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:FOO:\n:END:\n");

        let doc = parse(&text);
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                heading_ref(&doc),
                "FOO",
                PropertyEdit::Set {
                    value: Some("v".into()),
                },
            ),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:FOO: v\n:END:\n");

        let doc = parse("* H\n:PROPERTIES:\n:KEY:\n:END:\n");
        let (set, text) = compile_write(
            &doc,
            &set_prop(heading_ref(&doc), "KEY", PropertyEdit::Delete),
        );
        assert_eq!(
            set.edits()[0].range,
            doc.root_outlines()
                .next()
                .unwrap()
                .property_drawer()
                .unwrap()
                .range()
        );
        assert_eq!(text, "* H\n");
    }

    #[test]
    fn set_property_drops_plus_on_update() {
        let doc = parse("* H\n:PROPERTIES:\n:FOO+: v\n:END:\n");
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                heading_ref(&doc),
                "FOO",
                PropertyEdit::Set {
                    value: Some("v".into()),
                },
            ),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
    }

    #[test]
    fn set_property_preserves_written_case_and_inserts_caller_spelling() {
        let doc = parse("* H\n:PROPERTIES:\n:foo: v\n:END:\n");
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                heading_ref(&doc),
                "FOO",
                PropertyEdit::Set {
                    value: Some("w".into()),
                },
            ),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:foo: w\n:END:\n");

        let doc = parse("* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                heading_ref(&doc),
                "BAR",
                PropertyEdit::Set {
                    value: Some("x".into()),
                },
            ),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:FOO: v\n:BAR: x\n:END:\n");
    }

    #[test]
    fn set_property_delete_one_of_two_keeps_drawer() {
        let doc = parse("* H\n:PROPERTIES:\n:A: 1\n:B: 2\n:END:\n");
        let (_, text) = compile_write(
            &doc,
            &set_prop(heading_ref(&doc), "A", PropertyEdit::Delete),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:B: 2\n:END:\n");
    }

    #[test]
    fn set_property_noop_and_invalid() {
        let doc = parse("* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
        let target = heading_ref(&doc);
        let set = compile(
            &doc,
            &set_prop(
                target,
                "FOO",
                PropertyEdit::Set {
                    value: Some("v".into()),
                },
            ),
        )
        .expect("no-op");
        assert!(set.is_empty());

        let set = compile(&doc, &set_prop(target, "MISSING", PropertyEdit::Delete))
            .expect("missing delete");
        assert!(set.is_empty());

        let err = compile(
            &doc,
            &set_prop(
                target,
                "FOO",
                PropertyEdit::Set {
                    value: Some(String::new()),
                },
            ),
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::InvalidPropertyValue(s) if s == "FOO"));

        let err = compile(
            &doc,
            &set_prop(
                target,
                "FOO",
                PropertyEdit::Set {
                    value: Some("a\nb".into()),
                },
            ),
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::InvalidPropertyValue(_)));

        let err = compile(&doc, &set_prop(target, "FOO+", PropertyEdit::Delete)).unwrap_err();
        assert!(matches!(err, OperationError::InvalidPropertyKey(s) if s == "FOO+"));

        for key in ["END", "end", ""] {
            let err = compile(&doc, &set_prop(target, key, PropertyEdit::Delete)).unwrap_err();
            assert!(
                matches!(err, OperationError::InvalidPropertyKey(s) if s == key),
                "{key:?}"
            );
        }
    }

    #[test]
    fn set_planning_canonical_set_and_keep_locale() {
        let doc = parse("* H\n");
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");

        let doc = parse("* H\nSCHEDULED: <2020-01-01 Mi>\n");
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Set(ymd(2020, 1, 2)),
                    SlotPatch::Keep,
                    SlotPatch::Set(inactive_ymd(2020, 1, 3)),
                ),
            ),
        );
        assert_eq!(
            text,
            "* H\nDEADLINE: <2020-01-02 Thu> SCHEDULED: <2020-01-01 Mi> CLOSED: [2020-01-03 Fri 12:00]\n"
        );
        assert!(text.contains("<2020-01-01 Mi>"));
    }

    #[test]
    fn set_planning_bare_rebuild_and_all_clear() {
        let doc = parse("* H\nDEADLINE:\n");
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");

        let doc = parse("* H\nSCHEDULED:\n");
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Clear, SlotPatch::Clear, SlotPatch::Clear),
            ),
        );
        assert_eq!(
            set.edits()[0].range,
            doc.root_outlines()
                .next()
                .unwrap()
                .planning()
                .unwrap()
                .range()
        );
        assert_eq!(text, "* H\n");
    }

    #[test]
    fn set_planning_fill_in_and_surgical_timestamp() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE:\n");
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Set(ymd(2020, 1, 2)),
                    SlotPatch::Keep,
                    SlotPatch::Keep,
                ),
            ),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(
            text,
            "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n"
        );
        assert_eq!(text.matches("DEADLINE:").count(), 1);

        let doc = parse("* H\nscheduled: <2020-01-01 Wed>\n");
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 2)),
                    SlotPatch::Keep,
                ),
            ),
        );
        let p = doc.root_outlines().next().unwrap().planning().unwrap();
        assert_eq!(set.edits()[0].range, p.scheduled().unwrap().range());
        assert_eq!(text, "* H\nscheduled: <2020-01-02 Thu>\n");

        let doc = parse("* H\nDEADLINE: <2020-01-01 Wed> SCHEDULED:\n");
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 2)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(
            text,
            "* H\nDEADLINE: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu>\n"
        );
        assert_eq!(text.matches("SCHEDULED:").count(), 1);
    }

    #[test]
    fn set_planning_clear_one_of_two_and_last() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n");
        let p = doc.root_outlines().next().unwrap().planning().unwrap();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Clear, SlotPatch::Keep, SlotPatch::Keep),
            ),
        );
        assert_eq!(
            set.edits()[0].range,
            p.part_removal_range(PlanningPart::Deadline).unwrap()
        );
        assert!(!text.contains("DEADLINE"));
        assert!(text.contains("SCHEDULED: <2020-01-01 Wed>"));

        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Keep, SlotPatch::Clear, SlotPatch::Keep),
            ),
        );
        assert_eq!(text, "* H\n");
    }

    #[test]
    fn set_planning_eof_lead_and_keep_noop() {
        let doc = parse("* H");
        let dest = doc.root_outlines().next().unwrap().planning_insert_point();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");

        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let set = compile(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Keep, SlotPatch::Keep, SlotPatch::Keep),
            ),
        )
        .expect("all keep");
        assert!(set.is_empty());

        let doc = parse("* H\nDEADLINE:\n");
        let set = compile(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Keep, SlotPatch::Keep, SlotPatch::Keep),
            ),
        )
        .expect("bare all-keep");
        assert!(set.is_empty());

        let doc = parse("* H\n");
        let set = compile(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Clear, SlotPatch::Clear, SlotPatch::Clear),
            ),
        )
        .expect("clear absent");
        assert!(set.is_empty());
    }

    #[test]
    fn set_planning_invalid_timestamp() {
        let doc = parse("* H\n");
        let bad = TimestampInput {
            shape: TimestampShape::Active,
            start: None,
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        };
        let err = compile(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Keep, SlotPatch::Set(bad), SlotPatch::Keep),
            ),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::InvalidTimestamp(TimestampProblem::MissingStart)
        );
    }

    #[test]
    fn compile_all_property_and_planning_both_absent_conflict() {
        let doc = parse("* H\n");
        let target = heading_ref(&doc);
        let err = compile_all(
            &doc,
            &[
                set_prop(target, "KEY", PropertyEdit::Set { value: None }),
                set_plan(
                    target,
                    patch(
                        SlotPatch::Keep,
                        SlotPatch::Set(ymd(2020, 1, 1)),
                        SlotPatch::Keep,
                    ),
                ),
            ],
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: first_heading(&doc).range().end()
            }
        );

        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let target = heading_ref(&doc);
        let set = compile_all(
            &doc,
            &[
                set_prop(target, "KEY", PropertyEdit::Set { value: None }),
                set_plan(
                    target,
                    patch(
                        SlotPatch::Set(ymd(2020, 1, 2)),
                        SlotPatch::Keep,
                        SlotPatch::Keep,
                    ),
                ),
            ],
        )
        .expect("planning present: dests differ");
        assert_eq!(
            set.apply_to_text(doc.source()),
            "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n:PROPERTIES:\n:KEY:\n:END:\n"
        );
    }

    #[test]
    fn heading_sugar_and_inline_task_metadata() {
        let doc = parse("* H\n");
        let outline: Outline = doc.root_outlines().next().unwrap();
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                NodeRef::of(&outline.heading()),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");

        let src = "*************** T\n*************** END\n";
        let doc = parse(src);
        let it = doc.map::<crate::ast::InlineTask>().next().unwrap();
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                NodeRef::of(&it),
                "ID",
                PropertyEdit::Set {
                    value: Some("x".into()),
                },
            ),
        );
        assert_eq!(
            text,
            "*************** T\n:PROPERTIES:\n:ID: x\n:END:\n*************** END\n"
        );
    }

    #[test]
    fn set_planning_missing_kind_appends_slot() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let (_, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Set(ymd(2020, 1, 2)),
                    SlotPatch::Keep,
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(
            text,
            "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n"
        );
    }

    #[test]
    fn set_planning_set_same_stamp_is_noop() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let set = compile(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        )
        .expect("canonical same bytes");
        assert!(set.is_empty());
    }

    #[test]
    fn set_property_on_outline_kind() {
        let doc = parse("* H\n");
        let o = doc.root_outlines().next().unwrap();
        let (_, text) = compile_write(
            &doc,
            &set_prop(
                NodeRef::of(&o),
                "ID",
                PropertyEdit::Set {
                    value: Some("abc".into()),
                },
            ),
        );
        assert_eq!(text, "* H\n:PROPERTIES:\n:ID: abc\n:END:\n");
    }

    #[test]
    fn metadata_stale_and_wrong_kind() {
        let doc = parse("* H\nbody\n");
        let err = compile(
            &doc,
            &set_prop(
                NodeRef::at(crate::TextSize::from(99), OrgSyntaxKind::OUTLINE),
                "K",
                PropertyEdit::Delete,
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);

        let body_off = crate::TextSize::from(
            u32::try_from(doc.source().find("body").expect("body")).expect("offset"),
        );
        let err = compile(
            &doc,
            &set_plan(
                NodeRef::at(body_off, OrgSyntaxKind::PARAGRAPH),
                patch(SlotPatch::Keep, SlotPatch::Keep, SlotPatch::Clear),
            ),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            OperationError::WrongKind { actual, .. } if actual == OrgSyntaxKind::PARAGRAPH
        ));
    }

    #[test]
    fn set_planning_clear_bare_while_stamp_remains_rebuilds() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE:\n");
        let p = doc.root_outlines().next().unwrap().planning().unwrap();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(SlotPatch::Clear, SlotPatch::Keep, SlotPatch::Keep),
            ),
        );
        assert_eq!(set.edits()[0].range, p.range());
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");
    }

    #[test]
    fn set_property_delete_last_valued_removes_drawer() {
        let doc = parse("* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
        let drawer = doc
            .root_outlines()
            .next()
            .unwrap()
            .property_drawer()
            .unwrap();
        let (set, text) = compile_write(
            &doc,
            &set_prop(heading_ref(&doc), "FOO", PropertyEdit::Delete),
        );
        assert_eq!(set.edits()[0].range, drawer.range());
        assert_eq!(text, "* H\n");
    }

    #[test]
    fn set_planning_clear_plus_set_rebuilds_line() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n");
        let p = doc.root_outlines().next().unwrap().planning().unwrap();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Set(ymd(2020, 1, 3)),
                    SlotPatch::Clear,
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set.edits()[0].range, p.range());
        assert_eq!(text, "* H\nDEADLINE: <2020-01-03 Fri>\n");
        assert!(!text.contains("SCHEDULED"));
    }

    #[test]
    fn set_planning_set_corrects_stale_weekday() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Mi>\n");
        let p = doc.root_outlines().next().unwrap().planning().unwrap();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert_eq!(set.edits()[0].range, p.scheduled().unwrap().range());
        assert_eq!(text, "* H\nSCHEDULED: <2020-01-01 Wed>\n");
    }

    #[test]
    fn property_and_planning_insert_dests_with_the_other_present() {
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        let dest = doc
            .root_outlines()
            .next()
            .unwrap()
            .property_drawer_insert_point();
        let (set, text) = compile_write(
            &doc,
            &set_prop(heading_ref(&doc), "KEY", PropertyEdit::Set { value: None }),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert_eq!(
            text,
            "* H\nSCHEDULED: <2020-01-01 Wed>\n:PROPERTIES:\n:KEY:\n:END:\n"
        );

        let doc = parse("* H\n:PROPERTIES:\n:FOO: v\n:END:\n");
        let dest = doc.root_outlines().next().unwrap().planning_insert_point();
        let (set, text) = compile_write(
            &doc,
            &set_plan(
                heading_ref(&doc),
                patch(
                    SlotPatch::Keep,
                    SlotPatch::Set(ymd(2020, 1, 1)),
                    SlotPatch::Keep,
                ),
            ),
        );
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert_eq!(
            text,
            "* H\nSCHEDULED: <2020-01-01 Wed>\n:PROPERTIES:\n:FOO: v\n:END:\n"
        );
    }
}
