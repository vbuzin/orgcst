//! Planning lines: `SCHEDULED:`, `DEADLINE:` and `CLOSED:` with their
//! timestamps.
//!
//! One `PLANNING` node holds every keyword on the line. When a keyword occurs
//! more than once, the last occurrence wins, as in org-element. A keyword with
//! no timestamp leaves its slot `None`; the line is still a planning line.

use super::timestamp::Timestamp;
use crate::TextRange;
use crate::ast::layout::{
    PlanningKeyword, PlanningOccurrence, RemovalSide, planning_last_wins, planning_occurrences,
    removal_extending,
};
use crate::ast::support::field_dest;
use crate::ast::{AstNode, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A planning line: `SCHEDULED:`, `DEADLINE:` and `CLOSED:` with their
    /// timestamps.
    Planning,
    OrgSyntaxKind::PLANNING
);

/// The three planning keywords, each with its timestamp.
///
/// [`HasParts::PARTS`] lists them in this fixed order, whatever the order on
/// the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanningPart {
    /// `DEADLINE: <timestamp>`.
    Deadline,
    /// `SCHEDULED: <timestamp>`.
    Scheduled,
    /// `CLOSED: [timestamp]`.
    Closed,
}

impl From<PlanningPart> for PlanningKeyword {
    fn from(part: PlanningPart) -> Self {
        match part {
            PlanningPart::Deadline => Self::Deadline,
            PlanningPart::Scheduled => Self::Scheduled,
            PlanningPart::Closed => Self::Closed,
        }
    }
}

/// Spans of one keyword occurrence on the line.
///
/// The removal takes the keyword, its timestamp and the spaces after them,
/// never the whole line. To clear a keyword that occurs more than once,
/// delete each occurrence's removal, or the whole [`Planning::range`] when no
/// other keyword with a timestamp remains.
#[derive(Debug, Clone, Copy)]
struct PlanningOccurrenceSpan {
    carved: TextRange,
    content: TextRange,
    removal: TextRange,
}

impl Planning {
    /// Last `DEADLINE:` timestamp on this line, if that keyword has one.
    #[must_use]
    pub fn deadline(&self) -> Option<Timestamp> {
        slot_timestamp(self.syntax(), PlanningKeyword::Deadline)
    }

    /// Last `SCHEDULED:` timestamp on this line, if that keyword has one.
    #[must_use]
    pub fn scheduled(&self) -> Option<Timestamp> {
        slot_timestamp(self.syntax(), PlanningKeyword::Scheduled)
    }

    /// Last `CLOSED:` timestamp on this line, if that keyword has one.
    #[must_use]
    pub fn closed(&self) -> Option<Timestamp> {
        slot_timestamp(self.syntax(), PlanningKeyword::Closed)
    }

    /// Whether the winning occurrence of `part` is a keyword with no timestamp.
    #[must_use]
    pub(crate) fn slot_is_bare(&self, part: PlanningPart) -> bool {
        matches!(
            &planning_last_wins(self.syntax())[PlanningKeyword::from(part).slot()],
            Some(occ) if occ.timestamp.is_none()
        )
    }

    /// Every occurrence of `part`, in source order.
    #[cfg(test)]
    fn occurrences(&self, part: PlanningPart) -> impl Iterator<Item = PlanningOccurrenceSpan> + '_ {
        let kind = PlanningKeyword::from(part);
        let syntax = self.syntax();
        planning_occurrences(syntax)
            .into_iter()
            .filter(move |o| o.kind == kind)
            .map(move |o| occurrence_span(syntax, &o))
    }
}

impl HasParts for Planning {
    type Part = PlanningPart;
    const PARTS: &'static [PlanningPart] = &[
        PlanningPart::Deadline,
        PlanningPart::Scheduled,
        PlanningPart::Closed,
    ];

    fn part_span(&self, part: PlanningPart) -> PartSpan {
        let wins = planning_last_wins(self.syntax());
        let slot = &wins[PlanningKeyword::from(part).slot()];
        let Some(occ) = slot.as_ref() else {
            return PartSpan::Absent(absent_slot_insert(self.syntax(), part));
        };
        if occ.timestamp.is_none() {
            return PartSpan::Absent(absent_slot_insert(self.syntax(), part));
        }
        let occ_span = occurrence_span(self.syntax(), occ);
        let present_slots = wins
            .iter()
            .filter(|o| o.as_ref().is_some_and(|o| o.timestamp.is_some()))
            .count();
        // Last remaining timestamped slot: leave no bare PLANNING line.
        let removal = if present_slots == 1 {
            self.range()
        } else {
            occ_span.removal
        };
        PartSpan::Present(PresentPart::new(occ_span.carved, occ_span.content, removal))
    }
}

fn slot_timestamp(node: &SyntaxNode, kind: PlanningKeyword) -> Option<Timestamp> {
    planning_last_wins(node)[kind.slot()]
        .as_ref()?
        .timestamp
        .clone()
        .and_then(Timestamp::cast)
}

fn occurrence_end(occ: &PlanningOccurrence) -> crate::TextSize {
    occ.timestamp
        .as_ref()
        .map_or(occ.keyword.end(), |t| t.text_range().end())
}

fn occurrence_span(syntax: &SyntaxNode, occ: &PlanningOccurrence) -> PlanningOccurrenceSpan {
    let carved = match &occ.timestamp {
        Some(ts) => TextRange::new(occ.keyword.start(), ts.text_range().end()),
        None => occ.keyword,
    };
    let removal = removal_extending(syntax, carved, RemovalSide::After);
    PlanningOccurrenceSpan {
        carved,
        content: carved,
        removal,
    }
}

/// Where a missing slot is written.
///
/// After the last bare keyword of this kind, if there is one, so the writer
/// emits only the timestamp; otherwise after the line's last occurrence, where
/// the writer emits `KEYWORD: TIMESTAMP`.
fn absent_slot_insert(syntax: &SyntaxNode, part: PlanningPart) -> InsertPoint {
    let kind = PlanningKeyword::from(part);
    let occs = planning_occurrences(syntax);
    // Fill the last bare KEYWORD: of this kind so we never emit KEYWORD: KEYWORD:.
    if let Some(bare) = occs
        .iter()
        .rev()
        .find(|o| o.kind == kind && o.timestamp.is_none())
    {
        return field_dest(syntax, bare.keyword.end(), Padding::None);
    }
    let offset = occs
        .last()
        .map_or_else(|| syntax.text_range().end(), occurrence_end);
    field_dest(syntax, offset, Padding::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Padding;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice, written};

    fn planning(src: &str) -> (crate::Document, Planning) {
        let doc = parse(src);
        let p = Planning::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PLANNING)
                .into_iter()
                .next()
                .expect("planning"),
        )
        .expect("cast");
        (doc, p)
    }

    #[test]
    fn slots_scheduled_deadline_and_all_three() {
        let (doc, p) = planning("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        assert_eq!(
            source_slice(&doc, p.scheduled().unwrap().range()),
            "<2020-01-01 Wed>"
        );
        assert!(p.deadline().is_none());
        assert!(p.closed().is_none());

        let (doc, p) = planning("* H\nDEADLINE: <2020-01-02 Thu>\n");
        assert_eq!(
            source_slice(&doc, p.deadline().unwrap().range()),
            "<2020-01-02 Thu>"
        );
        assert!(p.scheduled().is_none());

        let (doc, p) = planning(
            "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu> CLOSED: [2020-01-03 Fri]\n",
        );
        assert_eq!(
            source_slice(&doc, p.scheduled().unwrap().range()),
            "<2020-01-01 Wed>"
        );
        assert_eq!(
            source_slice(&doc, p.deadline().unwrap().range()),
            "<2020-01-02 Thu>"
        );
        assert_eq!(
            source_slice(&doc, p.closed().unwrap().range()),
            "[2020-01-03 Fri]"
        );
        crate::ast::check_part_contract(&p);
    }

    #[test]
    fn repeated_scheduled_last_wins_and_two_occurrences() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu>\n";
        let (doc, p) = planning(src);
        assert_eq!(
            source_slice(&doc, p.scheduled().unwrap().range()),
            "<2020-01-02 Thu>"
        );
        let occs: Vec<_> = p.occurrences(PlanningPart::Scheduled).collect();
        assert_eq!(occs.len(), 2);
        assert!(source_slice(&doc, occs[0].carved).contains("<2020-01-01 Wed>"));
        assert!(source_slice(&doc, occs[1].carved).contains("<2020-01-02 Thu>"));
        assert_eq!(occs[0].content, occs[0].carved);
        assert_eq!(
            p.part_removal_range(PlanningPart::Scheduled).unwrap(),
            p.range()
        );
        for occ in &occs {
            assert!(
                !source_slice(&doc, occ.removal).contains('\n'),
                "per-occurrence removal must not take the line"
            );
            assert_ne!(occ.removal, p.range());
        }
        let first_kw = src.find("SCHEDULED:").expect("first");
        assert!(p.part_at(crate::TextSize::from(first_kw as u32)).is_none());
    }

    #[test]
    fn last_wins_bare_repeat_fills_the_trailing_keyword() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed> SCHEDULED:\n";
        let (doc, p) = planning(src);
        assert!(p.scheduled().is_none());
        let occs: Vec<_> = p.occurrences(PlanningPart::Scheduled).collect();
        assert_eq!(occs.len(), 2);
        let ip = p.part_insert_point(PlanningPart::Scheduled).unwrap();
        let after = written(doc.source(), ip, "<2020-01-02 Thu>");
        assert!(
            after.ends_with("SCHEDULED: <2020-01-02 Thu>\n")
                || after.contains("SCHEDULED: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu>"),
            "{after:?}"
        );
        assert_eq!(
            after.matches("SCHEDULED:").count(),
            2,
            "must fill the last bare keyword: {after:?}"
        );
    }

    #[test]
    fn bare_scheduled_then_deadline_fills_in_place() {
        let src = "* H\nSCHEDULED: DEADLINE: <2020-01-01 Wed>\n";
        let (doc, p) = planning(src);
        assert!(p.scheduled().is_none());
        assert!(p.deadline().is_some());
        let ip = p.part_insert_point(PlanningPart::Scheduled).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        let after = written(doc.source(), ip, "<2020-01-02 Thu>");
        assert!(
            after.contains("SCHEDULED: <2020-01-02 Thu> DEADLINE:"),
            "{after:?}"
        );
        assert!(
            !after.contains("SCHEDULED: SCHEDULED:"),
            "must fill the bare keyword: {after:?}"
        );
    }

    #[test]
    fn deadline_alone_is_still_planning() {
        let (_, p) = planning("* H\nDEADLINE:\n");
        assert!(p.deadline().is_none());
        assert!(p.scheduled().is_none());
        assert!(p.slot_is_bare(PlanningPart::Deadline));
        assert!(!p.slot_is_bare(PlanningPart::Scheduled));
        assert!(p.part_insert_point(PlanningPart::Deadline).is_some());
        crate::ast::check_part_contract(&p);
    }

    #[test]
    fn caseless_scheduled_fills_scheduled_not_closed() {
        let (doc, p) = planning("* H\nscheduled: <2020-01-01 Wed>\n");
        assert_eq!(
            source_slice(&doc, p.scheduled().unwrap().range()),
            "<2020-01-01 Wed>"
        );
        assert!(p.closed().is_none());
        assert!(p.deadline().is_none());
    }

    #[test]
    fn pairing_accepts_spaces_and_glued_colon() {
        let (doc, p) = planning("* H\nDEADLINE:   <2020-01-01 Wed>\n");
        assert_eq!(
            source_slice(&doc, p.deadline().unwrap().range()),
            "<2020-01-01 Wed>"
        );

        let (doc, p) = planning("* H\nSCHEDULED:<2020-01-01 Wed>\n");
        assert_eq!(
            source_slice(&doc, p.scheduled().unwrap().range()),
            "<2020-01-01 Wed>"
        );
    }

    #[test]
    fn last_remaining_slot_removal_is_the_planning_node() {
        let (_, p) = planning("* H\nSCHEDULED: <2020-01-01 Wed>\n");
        assert_eq!(
            p.part_removal_range(PlanningPart::Scheduled).unwrap(),
            p.range()
        );
    }

    #[test]
    fn two_slots_removal_does_not_swallow_newline() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n";
        let (doc, p) = planning(src);
        for part in [PlanningPart::Scheduled, PlanningPart::Deadline] {
            let removal = p.part_removal_range(part).unwrap();
            assert!(
                !source_slice(&doc, removal).contains('\n'),
                "{part:?} swallowed the terminator"
            );
            let carved = p.part_range(part).unwrap();
            assert_ne!(removal, p.range());
            assert!(removal.end() >= carved.end());
        }
    }

    #[test]
    fn repeated_scheduled_with_deadline_after_extends_last_only() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu> DEADLINE: <2020-01-03 Fri>\n";
        let (doc, p) = planning(src);
        let removal = p.part_removal_range(PlanningPart::Scheduled).unwrap();
        assert_ne!(removal, p.range());
        assert!(!source_slice(&doc, removal).contains('\n'));
        assert!(source_slice(&doc, removal).contains("<2020-01-02 Thu>"));
        assert!(!source_slice(&doc, removal).contains("<2020-01-01 Wed>"));
        let occs: Vec<_> = p.occurrences(PlanningPart::Scheduled).collect();
        assert_eq!(occs.len(), 2);
        assert_eq!(occs[1].removal, removal);
    }

    #[test]
    fn last_timestamped_slot_with_bare_other_kind_takes_the_line() {
        let (_, p) = planning("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE:\n");
        assert!(p.deadline().is_none());
        assert_eq!(
            p.part_removal_range(PlanningPart::Scheduled).unwrap(),
            p.range()
        );
    }

    #[test]
    fn absent_slot_appends_after_last_occurrence() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed>\n";
        let (doc, p) = planning(src);
        let ip = p.part_insert_point(PlanningPart::Deadline).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, "DEADLINE: <2020-01-02 Thu>"),
            "* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n"
        );
    }

    #[test]
    fn fill_bare_deadline_keeps_existing_spaces() {
        let src = "* H\nDEADLINE:   \n";
        let (doc, p) = planning(src);
        let ip = p.part_insert_point(PlanningPart::Deadline).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, "<2020-01-01 Wed>"),
            "* H\nDEADLINE: <2020-01-01 Wed>   \n"
        );
    }

    #[test]
    fn check_part_contract_on_fixed_vs_source_order() {
        let (_, gapped) = planning("* H\nSCHEDULED: <2020-01-01 Wed> DEADLINE: <2020-01-02 Thu>\n");
        crate::ast::check_part_contract(&gapped);
        let (_, glued) = planning("* H\nSCHEDULED:<2020-01-01 Wed>DEADLINE:<2020-01-02 Thu>\n");
        crate::ast::check_part_contract(&glued);
        assert_eq!(
            glued.part_at(glued.part_range(PlanningPart::Scheduled).unwrap().end()),
            Some(PlanningPart::Scheduled)
        );
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::PLANNING) {
                let p = Planning::cast(node).unwrap();
                crate::ast::check_part_contract(&p);
            }
        }
    }

    #[test]
    fn absent_line_still_uses_outline_planning_insert_point() {
        let doc = parse("* H\n");
        let o = doc.root_outlines().next().unwrap();
        assert!(o.planning().is_none());
        assert_eq!(
            o.planning_insert_point().offset(),
            o.heading().range().end()
        );
    }
}
