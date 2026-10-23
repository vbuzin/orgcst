//! Planning line and per-slot fragments.

use super::SynthesisError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanningKeywordSpec {
    Deadline,
    Scheduled,
    Closed,
}

pub(crate) fn render_planning_slot(kind: PlanningKeywordSpec, ts: &str) -> String {
    let key = match kind {
        PlanningKeywordSpec::Deadline => "DEADLINE",
        PlanningKeywordSpec::Scheduled => "SCHEDULED",
        PlanningKeywordSpec::Closed => "CLOSED",
    };
    format!("{key}: {ts}")
}

/// A whole planning line. Slots are written as DEADLINE, SCHEDULED, CLOSED
/// whatever the input order; an empty or repeated slot is an error.
pub(crate) fn render_planning_line(
    slots: &[(PlanningKeywordSpec, String)],
) -> Result<String, SynthesisError> {
    if slots.is_empty() {
        return Err(SynthesisError::EmptyPlanning);
    }
    let mut deadline = None;
    let mut scheduled = None;
    let mut closed = None;
    for (kind, ts) in slots {
        let slot = match kind {
            PlanningKeywordSpec::Deadline => &mut deadline,
            PlanningKeywordSpec::Scheduled => &mut scheduled,
            PlanningKeywordSpec::Closed => &mut closed,
        };
        if slot.is_some() {
            return Err(SynthesisError::DuplicatePlanningSlot);
        }
        *slot = Some(ts.as_str());
    }
    let mut parts = Vec::new();
    if let Some(ts) = deadline {
        parts.push(render_planning_slot(PlanningKeywordSpec::Deadline, ts));
    }
    if let Some(ts) = scheduled {
        parts.push(render_planning_slot(PlanningKeywordSpec::Scheduled, ts));
    }
    if let Some(ts) = closed {
        parts.push(render_planning_slot(PlanningKeywordSpec::Closed, ts));
    }
    let mut s = parts.join(" ");
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::super::timestamp::{TimestampSpec, render_timestamp};
    use super::*;
    use crate::parse;
    use crate::{TimestampPoint, TimestampShape};

    fn stamp(src: &str) -> String {
        src.to_string()
    }

    fn point(year: u16, month: u8, day: u8) -> TimestampPoint {
        TimestampPoint {
            year,
            month,
            day,
            hour: None,
            minute: None,
        }
    }

    #[test]
    fn planning_slot_and_line_recipes() {
        let wed = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 1)),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        assert_eq!(wed, "<2020-01-01 Wed>");
        assert_eq!(
            render_planning_slot(PlanningKeywordSpec::Scheduled, &wed),
            "SCHEDULED: <2020-01-01 Wed>"
        );

        let thu = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 2)),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let closed = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Inactive,
            start: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 3,
                hour: Some(12),
                minute: Some(0),
            }),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        assert_eq!(closed, "[2020-01-03 Fri 12:00]");

        let line = render_planning_line(&[
            (PlanningKeywordSpec::Closed, closed.clone()),
            (PlanningKeywordSpec::Scheduled, wed.clone()),
            (PlanningKeywordSpec::Deadline, thu.clone()),
        ])
        .unwrap();
        assert_eq!(
            line,
            "DEADLINE: <2020-01-02 Thu> SCHEDULED: <2020-01-01 Wed> CLOSED: [2020-01-03 Fri 12:00]\n"
        );

        let one = render_planning_line(&[(PlanningKeywordSpec::Scheduled, wed.clone())]).unwrap();
        assert_eq!(one, "SCHEDULED: <2020-01-01 Wed>\n");

        let two = render_planning_line(&[
            (PlanningKeywordSpec::Scheduled, wed),
            (PlanningKeywordSpec::Deadline, thu),
        ])
        .unwrap();
        assert_eq!(
            two,
            "DEADLINE: <2020-01-02 Thu> SCHEDULED: <2020-01-01 Wed>\n"
        );
    }

    #[test]
    fn planning_line_rejects_empty_and_duplicate() {
        assert!(matches!(
            render_planning_line(&[]),
            Err(SynthesisError::EmptyPlanning)
        ));
        assert!(matches!(
            render_planning_line(&[
                (PlanningKeywordSpec::Scheduled, stamp("<2020-01-01 Wed>")),
                (PlanningKeywordSpec::Scheduled, stamp("<2020-01-02 Thu>")),
            ]),
            Err(SynthesisError::DuplicatePlanningSlot)
        ));
    }

    #[test]
    fn planning_parse_stability() {
        let ts = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 1)),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let line = render_planning_line(&[(PlanningKeywordSpec::Scheduled, ts)]).unwrap();
        let src = format!("* H\n{line}");
        let doc = parse(&src);
        super::super::equal::assert_planning_scheduled(&doc, "<2020-01-01 Wed>");

        let wed = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 1)),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let thu = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 2)),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let closed = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Inactive,
            start: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 3,
                hour: Some(12),
                minute: Some(0),
            }),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let line = render_planning_line(&[
            (PlanningKeywordSpec::Closed, closed.clone()),
            (PlanningKeywordSpec::Scheduled, wed.clone()),
            (PlanningKeywordSpec::Deadline, thu.clone()),
        ])
        .unwrap();
        let doc = parse(&format!("* H\n{line}"));
        super::super::equal::assert_planning_eq(
            &doc,
            Some("<2020-01-02 Thu>"),
            Some("<2020-01-01 Wed>"),
            Some("[2020-01-03 Fri 12:00]"),
        );
    }
}
