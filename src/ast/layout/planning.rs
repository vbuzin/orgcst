//! Planning-line keyword/timestamp pairing.

use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode};

// ---------------------------------------------------------------------------
// Planning pairing (one sequence, one fold)
// ---------------------------------------------------------------------------

pub(crate) use crate::parser::grammar::forms::planning::{PlanningKeyword, planning_keyword};

/// One written `KEYWORD: [TIMESTAMP]` occurrence, in source order.
#[derive(Debug, Clone)]
pub(crate) struct PlanningOccurrence {
    pub kind: PlanningKeyword,
    /// Absolute span of the `TEXT` token plus the following `COLON`.
    pub keyword: TextRange,
    /// The `TIMESTAMP` child attributed to this keyword, if any.
    pub timestamp: Option<SyntaxNode>,
}

/// Direct-child walk of a `PLANNING` node: every keyword paired with the
/// `TIMESTAMP` that most recently follows it (or `None`).
///
/// Last-wins is a fold over this sequence, not a second scan.
#[must_use]
pub(crate) fn planning_occurrences(node: &SyntaxNode) -> Vec<PlanningOccurrence> {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < kids.len() {
        if let Some(tok) = kids[i].as_token()
            && tok.kind() == OrgSyntaxKind::TEXT
            && kids
                .get(i + 1)
                .is_some_and(|n| n.kind() == OrgSyntaxKind::COLON)
            && let Some(kind) = planning_keyword(tok.text())
        {
            let keyword = TextRange::new(tok.text_range().start(), kids[i + 1].text_range().end());
            out.push(PlanningOccurrence {
                kind,
                keyword,
                timestamp: None,
            });
            i += 2;
            continue;
        }
        if let Some(child) = kids[i].as_node()
            && child.kind() == OrgSyntaxKind::TIMESTAMP
            && let Some(occ) = out.last_mut()
            && occ.timestamp.is_none()
        {
            occ.timestamp = Some(child.clone());
        }
        i += 1;
    }
    out
}

/// Last written occurrence of each planning keyword, in the fixed order
/// (Deadline, Scheduled, Closed).
#[must_use]
pub(crate) fn planning_last_wins(node: &SyntaxNode) -> [Option<PlanningOccurrence>; 3] {
    let mut out = [None, None, None];
    for occ in planning_occurrences(node) {
        let i = occ.kind.slot();
        out[i] = Some(occ);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn planning_occurrences_pairs_whitespace_and_glued() {
        let spaced = first_kind(
            "* H\nDEADLINE:   <2020-01-01 Wed>\n",
            OrgSyntaxKind::PLANNING,
        );
        let occs = planning_occurrences(&spaced);
        assert_eq!(occs.len(), 1);
        assert_eq!(occs[0].kind, PlanningKeyword::Deadline);
        assert!(occs[0].timestamp.is_some());

        let glued = first_kind("* H\nSCHEDULED:<2020-01-01 Wed>\n", OrgSyntaxKind::PLANNING);
        let occs = planning_occurrences(&glued);
        assert_eq!(occs.len(), 1);
        assert_eq!(occs[0].kind, PlanningKeyword::Scheduled);
        assert!(occs[0].timestamp.is_some());
    }
    #[test]
    fn planning_last_wins_overwrites_repeated_scheduled() {
        let node = first_kind(
            "* H\nSCHEDULED: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu>\n",
            OrgSyntaxKind::PLANNING,
        );
        let wins = planning_last_wins(&node);
        assert!(wins[0].is_none());
        let sched = wins[1].as_ref().expect("scheduled");
        let doc = parse("* H\nSCHEDULED: <2020-01-01 Wed> SCHEDULED: <2020-01-02 Thu>\n");
        let ts = sched.timestamp.as_ref().expect("stamp");
        assert_eq!(&doc.source()[ts.text_range()], "<2020-01-02 Thu>");
        assert_eq!(planning_occurrences(&node).len(), 2);
    }
    #[test]
    fn planning_occurrences_closes_bare_keyword_before_later_stamp() {
        let node = first_kind(
            "* H\nSCHEDULED: DEADLINE: <2020-01-01 Wed>\n",
            OrgSyntaxKind::PLANNING,
        );
        let occs = planning_occurrences(&node);
        assert_eq!(occs.len(), 2);
        assert_eq!(occs[0].kind, PlanningKeyword::Scheduled);
        assert!(occs[0].timestamp.is_none());
        assert_eq!(occs[1].kind, PlanningKeyword::Deadline);
        assert!(occs[1].timestamp.is_some());
    }
}
