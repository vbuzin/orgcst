//! Planning line grammar: `SCHEDULED:` / `DEADLINE:` / `CLOSED:` in the
//! star-line metadata slot (after HEADING or a closed INLINE_TASK).
//!
//! Shape (org-syntax section 4.3.4 / `org-element-planning-line-re`):
//!
//! ```text
//! [INDENT] KEYWORD: [TIMESTAMP] ( KEYWORD: [TIMESTAMP] )*
//! ```
//!
//! Recognition is **context-gated** by the star-line family adjacency helper
//! ([`super::heading::parse_metadata`]): immediately after `HEADING`
//! (outline open) or the line of an `INLINE_TASK` that has an END line, with no
//! blank line. This module is pure shape: it does not know about headings,
//! sections, or adjacency. It is **not** in the element dispatcher
//! ([`crate::parser::grammar::drive::decide::decide`]), so decoy `SCHEDULED:`
//! lines elsewhere remain paragraph text.
//!
//! Evidence:
//! - org-syntax section 4.3.4
//! - `org-element-planning-parser` binds `case-fold-search` to nil, then
//!   `equal`s against `CLOSED:` / `DEADLINE:` / `SCHEDULED:` (case-sensitive).
//!   This parser matches those names ASCII-caseless, as the grammar and
//!   org-syntax 2.5.3. All keyword occurrences are kept; a missing timestamp
//!   is still planning.
//!
//! Emits:
//! ```text
//! PLANNING
//!   [INDENT]
//!   ( TEXT COLON [WHITESPACE*] [TIMESTAMP] [WHITESPACE*] | bare tokens )*
//!   [NEWLINE]
//! ```
//!
//! Contract:
//! - [`probe`] is the line prefilter (caseless keyword + colon at the content
//!   tile). Leading INDENT is outside the scan; only [`parse`] eats it.
//! - [`parse`] always emits `PLANNING` through end of line; missing/broken
//!   timestamps stay bare tokens.
//! - Diagnostics: [`DiagnosticKind::PlanningMissingTimestamp`] only when there
//!   is **no** timestamp opener after the keyword. A present opener that fails
//!   is owned by the timestamp grammar (`MalformedTimestamp` /
//!   `UnbalancedDiarySexp`); planning does not double-emit.
//! - Openers after a keyword that are not near misses (`SCHEDULED:
//!   <not-a-date>`, `[fn:1]`) stay silent: the timestamp grammar reports only
//!   near misses, and planning adds no `PlanningMissingTimestamp` when an
//!   opener was present.

use crate::DiagnosticKind;
use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

use super::timestamp::{self, try_parse_timestamp};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Cheap line prefilter: caseless planning keyword + `:` at line content.
///
/// Body quality is not checked.
pub(crate) fn probe(line: LineView) -> Option<()> {
    (line.kind(0) == OrgSyntaxKind::TEXT
        && is_planning_keyword(line.text(0))
        && line.kind(1) == OrgSyntaxKind::COLON)
        .then_some(())
}

/// Parse one planning line.
///
/// Always emits a `PLANNING` node through end of line (including optional
/// `NEWLINE`). Missing or broken timestamps remain bare tokens under PLANNING.
///
/// `PlanningMissingTimestamp` fires only when there is no timestamp opener
/// after the keyword; a failed stamp is diagnosed by the timestamp grammar.
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser.line().is_some_and(|l| probe(l).is_some()),
        "planning::parse called without probe (outline slot contract)"
    );

    let marker = parser.start();

    parser.eat(OrgSyntaxKind::INDENT);

    // Walk the line: planning keywords get optional timestamps; everything else
    // is bare (lossless). Keywords may appear anywhere on the line, not only
    // at the start, as org-element's re-search allows.
    while !parser.at_line_term() {
        if at_planning_keyword(parser) {
            let kw_start = parser.cursor();
            let kind = DiagnosticKind::planning_missing(parser.text());
            parser.eat(OrgSyntaxKind::TEXT);
            parser.eat(OrgSyntaxKind::COLON);
            // Look past optional whitespace for a stamp opener so the fail arm
            // can emit the keyword+colon span captured at entry.
            // Broad opener (any `<` / non-`[[` `[`) so non-date openers are
            // failed stamps (silent), not PlanningMissingTimestamp. Inline
            // uses the narrow at_timestamp so angle links stay free.
            if timestamp::at_timestamp_opener_after_ws(parser) {
                parser.eat_run(OrgSyntaxKind::WHITESPACE);
                // Owns MalformedTimestamp / UnbalancedDiarySexp and rewinds
                // fully on failure, so bare openers stay for this loop. Openers
                // that are not near misses stay silent, with no
                // PlanningMissingTimestamp.
                let _ = try_parse_timestamp(parser);
            } else {
                parser.error_span(kw_start, parser.cursor(), kind);
            }
            parser.eat_run(OrgSyntaxKind::WHITESPACE);
        } else {
            parser.eat_any();
        }
    }

    parser.eat(OrgSyntaxKind::NEWLINE);
    marker.complete(parser, OrgSyntaxKind::PLANNING);
}

/// `TEXT` + `COLON` at the cursor where TEXT is a planning keyword (caseless).
fn at_planning_keyword(parser: &Parser) -> bool {
    parser.here().is_some_and(|t| {
        t.seq(&[OrgSyntaxKind::TEXT, OrgSyntaxKind::COLON]) && is_planning_keyword(t.text())
    })
}

/// Which planning keyword `s` is (`DEADLINE` / `SCHEDULED` / `CLOSED`).
///
/// ASCII-caseless. The grammar probe and the typed views share this, so the
/// three names cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlanningKeyword {
    Deadline,
    Scheduled,
    Closed,
}

impl PlanningKeyword {
    /// Catalog-order slot: Deadline, Scheduled, Closed.
    pub(crate) fn slot(self) -> usize {
        match self {
            Self::Deadline => 0,
            Self::Scheduled => 1,
            Self::Closed => 2,
        }
    }
}

/// Classify a planning keyword. ASCII-caseless. `None` is not a keyword.
#[must_use]
pub(crate) fn planning_keyword(s: &str) -> Option<PlanningKeyword> {
    if s.eq_ignore_ascii_case("deadline") {
        Some(PlanningKeyword::Deadline)
    } else if s.eq_ignore_ascii_case("scheduled") {
        Some(PlanningKeyword::Scheduled)
    } else if s.eq_ignore_ascii_case("closed") {
        Some(PlanningKeyword::Closed)
    } else {
        None
    }
}

/// Whether `s` is a planning keyword.
#[must_use]
pub(crate) fn is_planning_keyword(s: &str) -> bool {
    planning_keyword(s).is_some()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{PlanningKeyword, at_planning_keyword, parse, planning_keyword, probe};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        ParseResult, children_kinds, diags_any, first_child_of_kind, make_parser, node_count,
        node_text, nodes_of_kind, parse_document_tree, probe_line, run_unit_element,
    };

    #[test]
    fn planning_keyword_is_exhaustive_caseless() {
        assert_eq!(
            planning_keyword("DEADLINE"),
            Some(PlanningKeyword::Deadline)
        );
        assert_eq!(
            planning_keyword("scheduled"),
            Some(PlanningKeyword::Scheduled)
        );
        assert_eq!(planning_keyword("Closed"), Some(PlanningKeyword::Closed));
        assert_eq!(planning_keyword("CLOCK"), None);
        assert_eq!(planning_keyword("deadlineX"), None);
    }

    #[test]
    fn at_planning_keyword_pure_positive_and_negative() {
        let p = make_parser("SCHEDULED: <d>\n");
        let before = p.cursor();
        assert!(at_planning_keyword(&p));
        assert_eq!(p.cursor(), before, "must not advance");

        assert!(at_planning_keyword(&make_parser("deadline:")));
        assert!(at_planning_keyword(&make_parser("CLOSED:")));
        assert!(!at_planning_keyword(&make_parser("CLOCK:")));
        assert!(!at_planning_keyword(&make_parser("SCHEDULED"))); // no colon
        assert!(!at_planning_keyword(&make_parser("foo:")));
        assert!(!at_planning_keyword(&make_parser("  SCHEDULED:"))); // INDENT first
    }

    fn results_of(src: &str) -> ParseResult {
        run_unit_element(src, probe, parse)
    }

    fn planning_count(src: &str) -> usize {
        node_count(&results_of(src).1, OrgSyntaxKind::PLANNING)
    }

    fn assert_full_planning(src: &str) {
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::PLANNING),
            1,
            "expected one PLANNING for {src:?}"
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PLANNING).as_deref(),
            Some(src),
            "PLANNING must cover full line for {src:?}"
        );
    }

    #[test]
    fn probe_line_open_and_caseless() {
        assert!(probe_line("SCHEDULED: <d>\n", probe).is_some());
        assert!(probe_line("DEADLINE: <d>\n", probe).is_some());
        assert!(probe_line("CLOSED: [d]\n", probe).is_some());
        assert!(probe_line("   \tscheduled: <d>\n", probe).is_some());
        assert!(probe_line("ClOsEd:\n", probe).is_some());
        assert!(probe_line("DEADLINE:\n", probe).is_some());
        // Mid-line "foo SCHEDULED" is content origin TEXT "foo" - not planning.
        assert!(probe_line("foo SCHEDULED: <d>\n", probe).is_none());
        assert!(probe_line("SCHED: <d>\n", probe).is_none());
        assert!(probe_line("not planning\n", probe).is_none());
        assert!(probe_line("CLOCK: [d]\n", probe).is_none());
    }

    #[test]
    fn parse_basic_and_multi_keyword() {
        let src = "SCHEDULED: <2020-01-01 Wed>\n";
        assert_full_planning(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 1);

        let multi = "SCHEDULED: <2020-01-01> DEADLINE: <2020-01-02> CLOSED: [2020-01-03]\n";
        assert_full_planning(multi);
        assert_eq!(
            node_count(&results_of(multi).1, OrgSyntaxKind::TIMESTAMP),
            3
        );
    }

    #[test]
    fn no_space_after_colon_and_missing_timestamp() {
        assert_full_planning("DEADLINE:<2020-01-01 Wed>\n");
        assert_eq!(
            node_count(
                &results_of("DEADLINE:<2020-01-01 Wed>\n").1,
                OrgSyntaxKind::TIMESTAMP
            ),
            1
        );

        // A keyword alone is still planning, as in org.
        // No opener after colon -> PlanningMissingTimestamp only.
        assert_full_planning("CLOSED:\n");
        let (_, evs, diags) = results_of("CLOSED:\n");
        assert_eq!(node_count(&evs, OrgSyntaxKind::TIMESTAMP), 0);
        assert_eq!(diags.len(), 1);
        assert!(diags_any(&diags, |k| {
            matches!(
                k,
                crate::DiagnosticKind::PlanningMissingTimestamp { keyword }
                    if keyword.eq_ignore_ascii_case("closed")
            )
        }));
        // Range policy: keyword + colon only (not the whole line).
        let r = diags[0].range();
        let s = usize::from(r.start());
        let e = usize::from(r.end());
        assert_eq!(&"CLOSED:\n"[s..e], "CLOSED:");
    }

    #[test]
    fn multi_keyword_missing_timestamp_diags_per_keyword() {
        let (_, _, diags) = results_of("SCHEDULED: DEADLINE: <2020-01-01>\n");
        // First keyword has no stamp; second has one.
        let missing: Vec<_> = diags
            .iter()
            .filter_map(|d| match d.kind() {
                crate::DiagnosticKind::PlanningMissingTimestamp { keyword } => {
                    Some(keyword.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(missing, vec!["SCHEDULED"]);
    }

    #[test]
    fn multi_keyword_both_missing_and_eof_without_newline() {
        let src = "SCHEDULED: DEADLINE:";
        assert_full_planning(src);
        let (_, _, diags) = results_of(src);
        let missing: Vec<_> = diags
            .iter()
            .filter_map(|d| match d.kind() {
                crate::DiagnosticKind::PlanningMissingTimestamp { keyword } => {
                    Some(keyword.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(missing, vec!["SCHEDULED", "DEADLINE"]);
    }

    #[test]
    fn date_like_unclosed_stamp_is_timestamp_diag_only() {
        // Opener present after keyword: timestamp owns the near-miss diag;
        // planning must not also emit PlanningMissingTimestamp.
        let src = "SCHEDULED: <2020-01-01\n";
        assert_full_planning(src);
        let (_, evs, diags) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::TIMESTAMP), 0);
        assert!(
            diags_any(&diags, |k| matches!(
                k,
                crate::DiagnosticKind::MalformedTimestamp
            )),
            "expected MalformedTimestamp: {diags:?}"
        );
        assert!(
            !diags_any(&diags, |k| {
                matches!(k, crate::DiagnosticKind::PlanningMissingTimestamp { .. })
            }),
            "must not double-emit PlanningMissingTimestamp: {diags:?}"
        );
    }

    #[test]
    fn unbalanced_diary_after_planning_is_stamp_diag_only() {
        let src = "SCHEDULED: <%%(foo\n";
        assert_full_planning(src);
        let (_, evs, diags) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::TIMESTAMP), 0);
        assert!(
            diags_any(&diags, |k| matches!(
                k,
                crate::DiagnosticKind::UnbalancedDiarySexp
            )),
            "expected UnbalancedDiarySexp: {diags:?}"
        );
        assert!(
            !diags_any(&diags, |k| {
                matches!(k, crate::DiagnosticKind::PlanningMissingTimestamp { .. })
            }),
            "must not double-emit PlanningMissingTimestamp: {diags:?}"
        );
    }

    #[test]
    fn non_near_miss_opener_after_planning_stays_silent() {
        // Near misses only: non-date openers are not MalformedTimestamp and
        // not PlanningMissingTimestamp (opener was present). Cover both the
        // active angle and inactive bracket branches of at_timestamp.
        for src in ["SCHEDULED: <not-a-date>\n", "SCHEDULED: [fn:1]\n"] {
            assert_full_planning(src);
            let (_, _, diags) = results_of(src);
            assert!(
                diags.is_empty(),
                "non-near-miss opener must stay silent for {src:?}: {diags:?}"
            );
            assert!(
                !diags_any(&diags, |k| {
                    matches!(
                        k,
                        crate::DiagnosticKind::PlanningMissingTimestamp { .. }
                            | crate::DiagnosticKind::MalformedTimestamp
                    )
                }),
                "must not invent planning/stamp diags for {src:?}: {diags:?}"
            );
        }
    }

    #[test]
    fn repeated_keyword_keeps_all_timestamps() {
        let src = "SCHEDULED: <2020-01-01> SCHEDULED: <2020-01-02>\n";
        assert_full_planning(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 2);
    }

    #[test]
    fn active_and_inactive_stamps() {
        let src = "SCHEDULED: <2020-01-01> CLOSED: [2020-01-02]\n";
        assert_full_planning(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 2);
    }

    #[test]
    fn leading_indent_and_eof_without_newline() {
        assert_eq!(planning_count("   deadline: <2020-01-01>\n"), 1);
        assert_full_planning("SCHEDULED: <2020-01-01>");
    }

    #[test]
    fn non_planning_line_fails_probe() {
        let src = "just text\n";
        assert!(probe_line(src, probe).is_none());
        assert_eq!(planning_count(src), 0);
    }

    // --- Full document: adjacency + OUTLINE placement ----------------------

    #[test]
    fn planning_after_heading_is_outline_metadata() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed>\n";
        let root = parse_document_tree(src);

        let outlines = nodes_of_kind(&root, OrgSyntaxKind::OUTLINE);
        assert_eq!(outlines.len(), 1);
        let kids = children_kinds(&outlines[0]);
        assert_eq!(
            kids,
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::PLANNING],
            "PLANNING is outline metadata (sibling of SECTION, not under it)"
        );

        let planning = first_child_of_kind(&outlines[0], OrgSyntaxKind::PLANNING).unwrap();
        assert_eq!(nodes_of_kind(&planning, OrgSyntaxKind::TIMESTAMP).len(), 1);
        // No SECTION when only metadata follows the heading.
        assert!(first_child_of_kind(&outlines[0], OrgSyntaxKind::SECTION).is_none());
    }

    #[test]
    fn planning_then_body_section() {
        let src = "* H\nSCHEDULED: <2020-01-01 Wed>\nbody text\n";
        let root = parse_document_tree(src);

        let outline = &nodes_of_kind(&root, OrgSyntaxKind::OUTLINE)[0];
        assert_eq!(
            children_kinds(outline),
            vec![
                OrgSyntaxKind::HEADING,
                OrgSyntaxKind::PLANNING,
                OrgSyntaxKind::SECTION,
            ]
        );
        let section = first_child_of_kind(outline, OrgSyntaxKind::SECTION).unwrap();
        assert!(
            nodes_of_kind(&section, OrgSyntaxKind::PLANNING).is_empty(),
            "SECTION must not contain PLANNING"
        );
        assert_eq!(nodes_of_kind(&section, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn blank_line_breaks_adjacency() {
        let src = "* H\n\nSCHEDULED: <2020-01-01 Wed>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PLANNING).len(), 0);
        let outline = &nodes_of_kind(&root, OrgSyntaxKind::OUTLINE)[0];
        let section = first_child_of_kind(outline, OrgSyntaxKind::SECTION).unwrap();
        assert_eq!(children_kinds(&section), vec![OrgSyntaxKind::PARAGRAPH]);
    }

    #[test]
    fn mid_section_scheduled_is_paragraph_text() {
        let src = "* H\nintro text\nSCHEDULED: <2020-01-01 Wed>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PLANNING).len(), 0);
        let outline = &nodes_of_kind(&root, OrgSyntaxKind::OUTLINE)[0];
        assert!(first_child_of_kind(outline, OrgSyntaxKind::PLANNING).is_none());
        let section = first_child_of_kind(outline, OrgSyntaxKind::SECTION).unwrap();
        // Not an element opener: stays one paragraph.
        assert_eq!(children_kinds(&section), vec![OrgSyntaxKind::PARAGRAPH]);
    }

    #[test]
    fn zeroth_section_scheduled_is_paragraph() {
        let src = "SCHEDULED: <2020-01-01 Wed>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PLANNING).len(), 0);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn caseless_planning_after_heading() {
        let src = "* H\nscheduled: <2020-01-01 Wed>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PLANNING).len(), 1);
    }

    #[test]
    fn multi_keyword_document_and_repeats() {
        let src = "* H\nSCHEDULED: <2020-01-01> DEADLINE: <2020-01-02> CLOSED: [2020-01-03]\n";
        let root = parse_document_tree(src);
        let planning = &nodes_of_kind(&root, OrgSyntaxKind::PLANNING)[0];
        assert_eq!(nodes_of_kind(planning, OrgSyntaxKind::TIMESTAMP).len(), 3);

        let src2 = "* H\nSCHEDULED: <2020-01-01> SCHEDULED: <2020-01-02>\n";
        let root2 = parse_document_tree(src2);
        let planning2 = &nodes_of_kind(&root2, OrgSyntaxKind::PLANNING)[0];
        assert_eq!(nodes_of_kind(planning2, OrgSyntaxKind::TIMESTAMP).len(), 2);
    }

    #[test]
    fn planning_not_registered_as_paragraph_opener() {
        // Without a heading, SCHEDULED: must not split anything as an element;
        // with body text on the previous line in a section, still one paragraph.
        let src = "* H\nline one\nSCHEDULED: <2020-01-01>\nline three\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PLANNING).len(), 0);
        let section = first_child_of_kind(
            &nodes_of_kind(&root, OrgSyntaxKind::OUTLINE)[0],
            OrgSyntaxKind::SECTION,
        )
        .unwrap();
        assert_eq!(children_kinds(&section), vec![OrgSyntaxKind::PARAGRAPH]);
    }
}
