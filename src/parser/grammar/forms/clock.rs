//! Clock element grammar: `CLOCK:` lines with inactive timestamps / duration.
//!
//! Three legal shapes (org-syntax section 4.3.2 / `org-element-clock-line-re`):
//!
//! ```text
//! CLOCK: [inactive]                              (running; single point)
//! CLOCK: [inactive]--[inactive] => H:MM          (closed; range + duration)
//! CLOCK: => H:MM                                 (closed; duration only)
//! ```
//!
//! Evidence:
//! - `org-element-clock-line-re-no-group` / `org-element-clock-parser`
//!   (same three shapes; caseless marker)
//!
//! The timestamp reuses [`try_parse_inactive_timestamp`], which accepts only
//! `[...]` and records diagnostics for near misses; it also reports the form
//! (point or `--` range). The duration `=> H:MM` stays bare tokens under CLOCK,
//! with no dedicated kind, as org-element keeps it as a string.
//!
//! Emits:
//! ```text
//! CLOCK
//!   [INDENT] TEXT("CLOCK") COLON WHITESPACE+
//!   [ TIMESTAMP | (EQUALS R_ANGLE WHITESPACE+ hours COLON mins) ]
//!   [ WHITESPACE+ duration when range ]
//!   [NEWLINE]
//!   [post-blank tokens]
//! ```
//!
//! Recognition is a cheap prefilter followed by a parse that may fail:
//! - [`probe`] matches only the caseless `CLOCK:` marker at line content.
//!   Body validation stays in [`parse`]: a pure body probe would either repeat
//!   the parse or record near-miss timestamp diagnostics twice.
//! - Every `CLOCK:` line is claimed by
//!   [`crate::parser::grammar::drive::decide::decide`], so it interrupts a
//!   running paragraph even when [`parse`] later rewinds. The marker, not the
//!   body, decides paragraph termination.
//! - When the body is not a legal clock, [`parse`] rewinds through
//!   [`Parser::attempt`] and the dispatcher falls through to a paragraph,
//!   matching org-element, where a malformed `CLOCK:` line is paragraph text.
//!   A line-level [`DiagnosticKind::MalformedClock`] is recorded.
//! - Timestamp near misses in the body always record their own diagnostic;
//!   `parser::diagnostic::emit` deduplicates repeats.

use crate::DiagnosticKind;
use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

use super::timestamp;
use crate::parser::grammar::drive::atoms;
use atoms::is_digits;
use timestamp::{TimestampForm, try_parse_inactive_timestamp};

// ---------------------------------------------------------------------------
// Public surface for the element dispatcher
// ---------------------------------------------------------------------------

/// Cheap marker prefilter: caseless `CLOCK:` at line content.
///
/// Body validity is decided by [`parse`]. Any `CLOCK:`-prefixed line reaches
/// the grammar so non-clocks fall through to paragraph.
pub(crate) fn probe(line: LineView) -> Option<()> {
    (line.kind(0) == OrgSyntaxKind::TEXT
        && line.text(0).eq_ignore_ascii_case("clock")
        && line.kind(1) == OrgSyntaxKind::COLON)
        .then_some(())
}

/// Parse one CLOCK element when the body matches a legal clock shape.
///
/// On structural failure the cursor and events are restored (no partial CLOCK
/// node) and a diagnostic is recorded.
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser.line().is_some_and(|l| probe(l).is_some()),
        "clock::parse called without probe (dispatcher contract)"
    );

    let entry = parser.cursor();
    if parser.attempt(try_parse_clock_body).is_none() {
        parser.error_line_at(entry, DiagnosticKind::MalformedClock);
    }
}

// ---------------------------------------------------------------------------
// Body parse (None => attempt rewinds)
// ---------------------------------------------------------------------------

fn try_parse_clock_body(parser: &mut Parser) -> Option<()> {
    let marker = parser.start();

    parser.eat(OrgSyntaxKind::INDENT);
    parser.eat(OrgSyntaxKind::TEXT); // CLOCK (shape guard)
    parser.eat(OrgSyntaxKind::COLON);

    // Both shapes require at least one space/tab after `CLOCK:`.
    if parser.eat_run(OrgSyntaxKind::WHITESPACE) == 0 {
        marker.abandon(parser);
        return None;
    }

    if try_parse_value_then_line(parser) {
        parser.eat_blank_lines();
        marker.complete(parser, OrgSyntaxKind::CLOCK);
        Some(())
    } else {
        marker.abandon(parser);
        None
    }
}

/// Timestamp arms or duration-only, then line end.
fn try_parse_value_then_line(parser: &mut Parser) -> bool {
    if parser.current() == OrgSyntaxKind::L_BRACKET {
        try_parse_timestamp_clock(parser)
    } else {
        try_parse_duration(parser) && try_finish_line(parser)
    }
}

/// Running point or closed range+duration (cursor on `L_BRACKET`).
fn try_parse_timestamp_clock(parser: &mut Parser) -> bool {
    // Records near-miss diagnostics; `parser::diagnostic::emit` deduplicates.
    let Some(form) = try_parse_inactive_timestamp(parser) else {
        return false;
    };
    match form {
        TimestampForm::Range => {
            // Closed: required whitespace then `=> H:MM` to end of line.
            parser.eat_run(OrgSyntaxKind::WHITESPACE) > 0
                && try_parse_duration(parser)
                && try_finish_line(parser)
        }
        TimestampForm::Point => {
            // Running: only trailing whitespace may follow.
            parser.eat_run(OrgSyntaxKind::WHITESPACE);
            try_finish_line(parser)
        }
    }
}

// ---------------------------------------------------------------------------
// Duration: => H:MM
// ---------------------------------------------------------------------------

/// Pure shape length of `=> H:MM` at the cursor (hours 1+ digits, minutes
/// exactly two). Does not include trailing whitespace after the minutes.
///
/// Token layout: EQUALS R_ANGLE WHITESPACE TEXT COLON TEXT.
fn duration_token_len(parser: &Parser) -> Option<usize> {
    use OrgSyntaxKind::{COLON, EQUALS, R_ANGLE, TEXT, WHITESPACE};

    let t = parser.here()?;
    // org duration: (1+ ws) after `=>` (lexer merges runs into one WHITESPACE).
    if !t.seq(&[EQUALS, R_ANGLE, WHITESPACE, TEXT, COLON, TEXT]) {
        return None;
    }
    if !is_digits(t.text_at(3)) {
        return None;
    }
    let mins = t.text_at(5);
    if mins.len() != 2 || !is_digits(mins) {
        return None;
    }
    Some(6)
}

/// Parse `=> H:MM` at the cursor. Classify fully before consume.
fn try_parse_duration(parser: &mut Parser) -> bool {
    let Some(n) = duration_token_len(parser) else {
        return false;
    };
    parser.eat_n(n);
    parser.eat_run(OrgSyntaxKind::WHITESPACE); // trailing after H:MM
    true
}

/// Accept only EOL after the body (optional NEWLINE).
fn try_finish_line(parser: &mut Parser) -> bool {
    if !parser.at_line_term() {
        return false;
    }
    parser.eat(OrgSyntaxKind::NEWLINE);
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{duration_token_len, parse, probe};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        ParseResult, diags_any, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, probe_line, run_unit_element, tree_text,
    };

    /// Guarded: only call parse when the probe holds (malformed bodies must
    /// not panic).
    fn results_of(src: &str) -> ParseResult {
        run_unit_element(src, probe, parse)
    }

    fn clock_count(src: &str) -> usize {
        node_count(&results_of(src).1, OrgSyntaxKind::CLOCK)
    }

    /// No CLOCK / TIMESTAMP nodes (outer checkpoint drops nested stamp work).
    fn assert_not_a_clock_clean(src: &str) {
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::CLOCK),
            0,
            "CLOCK must not be emitted for {src:?}"
        );
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::TIMESTAMP),
            0,
            "outer rewind must drop nested TIMESTAMP for {src:?}"
        );
    }

    /// Rejected clock: clean tree, cursor and events restored, and a
    /// `MalformedClock` diagnostic.
    fn assert_clock_rejected(src: &str) {
        assert_not_a_clock_clean(src);
        let mut p = make_parser(src);
        assert!(
            p.line().is_some_and(|l| probe(l).is_some()),
            "expected clock probe for {src:?}"
        );
        let pos_before = p.cursor().index();
        let events_before = p.checkpoint().events_len;
        parse(&mut p);
        assert_eq!(
            p.cursor().index(),
            pos_before,
            "cursor restored for {src:?}"
        );
        assert_eq!(
            p.checkpoint().events_len,
            events_before,
            "events restored for {src:?}"
        );
        let (_, _, diags) = p.finish();
        assert!(
            diags_any(&diags, |k| matches!(
                k,
                crate::DiagnosticKind::MalformedClock
            )),
            "prefilter-hit reject must emit MalformedClock for {src:?}: {diags:?}"
        );
    }

    fn assert_full_clock(src: &str) {
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::CLOCK), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CLOCK).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn probe_is_caseless_marker_prefilter() {
        assert!(probe_line("CLOCK: [d]\n", probe).is_some());
        assert!(probe_line("   \tclock: [d]\n", probe).is_some());
        assert!(probe_line("ClOcK: [d]\n", probe).is_some());
        assert!(probe_line("CLOCK: => 1:00\n", probe).is_some());
        assert!(probe_line("CLOCK:\n", probe).is_some());
        assert!(probe_line("CLOCK: text\n", probe).is_some());
        assert!(probe_line("CLOCK: <2020-01-01>\n", probe).is_some());
        assert!(probe_line("foo CLOCK: [d]\n", probe).is_none());
        assert!(probe_line("CLOCKX: [d]\n", probe).is_none());
        assert!(probe_line("not a clock\n", probe).is_none());
    }

    #[test]
    fn running_clock_single_inactive_point() {
        let src = "CLOCK: [2020-01-01 Wed 10:00]\n";
        assert_full_clock(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 1);
    }

    #[test]
    fn running_clock_with_internal_time_range() {
        // Internal HH:MM-HH:MM is still a point stamp (no `--`, no duration).
        let src = "CLOCK: [2020-01-01 Wed 10:00-11:00]\n";
        assert_eq!(clock_count(src), 1);
        assert!(node_kinds(&results_of(src).1).contains(&OrgSyntaxKind::TIME_RANGE));
    }

    #[test]
    fn closed_clock_range_plus_duration() {
        let src = "CLOCK: [2020-01-01 Wed 10:00]--[2020-01-01 Wed 11:00] => 1:00\n";
        assert_full_clock(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 1);
    }

    #[test]
    fn duration_only_clock() {
        let src = "CLOCK: => 2:15\n";
        assert_full_clock(src);
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::TIMESTAMP), 0);
    }

    #[test]
    fn caseless_and_leading_indent() {
        assert_eq!(clock_count("   clock: [2020-01-01]\n"), 1);
    }

    #[test]
    fn rejects_active_and_garbage_bodies() {
        // All prefilter-hit rejects: clean tree + MalformedClock (assert_clock_rejected).
        for src in [
            "CLOCK: <2020-01-01 Wed 10:00>\n",
            "CLOCK:\n",
            "CLOCK: \n",
            "CLOCK: text\n",
            "CLOCK: [garbage\n",
        ] {
            assert_clock_rejected(src);
        }
    }

    #[test]
    fn rejects_point_with_duration_and_range_without() {
        assert_clock_rejected("CLOCK: [2020-01-01 10:00] => 1:00\n");
        assert_clock_rejected("CLOCK: [2020-01-01 10:00]--[2020-01-01 11:00]\n");
    }

    #[test]
    fn duration_must_be_well_formed_h_mm() {
        let range = "CLOCK: [2020-01-01 10:00]--[2020-01-01 11:00]";
        assert_eq!(clock_count(&format!("{range} => 1:00\n")), 1);
        assert_eq!(clock_count(&format!("{range} => 12:05\n")), 1);
        // Rejects after a successful Range stamp: no orphan TIMESTAMP.
        assert_not_a_clock_clean(&format!("{range} => 1:5\n")); // one minute digit
        assert_not_a_clock_clean(&format!("{range} =>1:00\n")); // no ws after `=>`
        assert_not_a_clock_clean(&format!("{range}=> 1:00\n")); // no ws before `=>`
        assert_not_a_clock_clean(&format!("{range} => x:00\n")); // non-digit hour
        assert_not_a_clock_clean(&format!("{range} => 1:00 x\n")); // trailing junk
    }

    #[test]
    fn duration_token_len_shape() {
        let p = make_parser("=> 1:00\n");
        let before = p.cursor();
        assert_eq!(duration_token_len(&p), Some(6));
        assert_eq!(p.cursor(), before, "measure must not advance");
        // Multi-digit hour (H:MM with 2+ hour digits).
        assert_eq!(duration_token_len(&make_parser("=> 12:05\n")), Some(6));
        assert_eq!(duration_token_len(&make_parser("=>1:00\n")), None);
        assert_eq!(duration_token_len(&make_parser("=> 1:5\n")), None);
        assert_eq!(duration_token_len(&make_parser("=> x:00\n")), None);
        assert_eq!(duration_token_len(&make_parser("1:00\n")), None);
    }

    #[test]
    fn duration_only_at_eof_without_newline() {
        assert_full_clock("CLOCK: => 2:15");
        assert_eq!(
            node_count(&results_of("CLOCK: => 2:15").1, OrgSyntaxKind::TIMESTAMP),
            0
        );
    }

    #[test]
    fn clock_appears_in_document_tree() {
        let src = "CLOCK: [2020-01-01 Wed 10:00]\npara\n";
        let root = parse_document_tree(src);
        let clocks = nodes_of_kind(&root, OrgSyntaxKind::CLOCK);
        assert_eq!(clocks.len(), 1);
        assert!(tree_text(&clocks[0]).contains("CLOCK:"));
    }

    #[test]
    fn clock_interrupts_a_paragraph() {
        let src = "some text\nCLOCK: [2020-01-01 Wed 10:00]\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CLOCK).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn malformed_clock_falls_through_to_paragraph() {
        // A malformed CLOCK: line is paragraph text, not a partial CLOCK or an
        // orphaned TIMESTAMP. Full source is preserved.
        let src = "CLOCK: text\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CLOCK).len(), 0);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).len(), 0);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);

        // Fallthrough then a valid clock: paragraph, then CLOCK (dispatcher
        // recovers and continues).
        let src2 = "CLOCK: text\nCLOCK: [2020-01-01 Wed 10:00]\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::CLOCK).len(), 1);
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::PARAGRAPH).len(), 1);
        assert_eq!(tree_text(&root2), src2);
    }

    #[test]
    fn malformed_clock_emits_diagnostic() {
        let src = "CLOCK: text\n";
        let out = crate::parse(src);
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(k, crate::DiagnosticKind::MalformedClock)
        }));
        assert_eq!(out.root().text().to_string(), src);
        // Range covers the opener line, newline excluded (error_line_at).
        let d = out
            .diagnostics()
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::MalformedClock))
            .expect("MalformedClock diag");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "CLOCK: text");

        // Well-formed clocks emit no diagnostics.
        let ok = crate::parse("CLOCK: [2020-01-01 Wed 10:00]\n");
        assert!(
            !diags_any(ok.diagnostics(), |k| matches!(
                k,
                crate::DiagnosticKind::MalformedClock
            )),
            "valid clock must not emit MalformedClock: {:?}",
            ok.diagnostics()
        );
        assert!(
            ok.diagnostics().is_empty(),
            "valid clock must not emit any diag: {:?}",
            ok.diagnostics()
        );
    }

    #[test]
    fn date_like_malformed_clock_full_document_single_fire() {
        // Full pipeline: the timestamp near miss, MalformedClock, then the
        // paragraph re-parsing the same stamp. Only final counts are checked;
        // deduplication itself is tested by `diagnostic::emit`'s
        // `error_range_*` tests.
        let src = "CLOCK: [2020-01-01\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::CLOCK).len(),
            0,
            "malformed body is not a CLOCK node"
        );
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::PARAGRAPH).len(),
            1,
            "fallthrough to paragraph"
        );

        let clock = out
            .diagnostics()
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::MalformedClock))
            .expect("MalformedClock");
        let stamp = out
            .diagnostics()
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::MalformedTimestamp))
            .expect("MalformedTimestamp");
        assert_eq!(
            out.diagnostics().len(),
            2,
            "exactly one clock + one stamp (deduped): {:?}",
            out.diagnostics()
        );

        // Distinct kinds and ranges: clock covers full line, stamp from `[`.
        assert_ne!(
            clock.range(),
            stamp.range(),
            "distinct kinds keep distinct ranges; dedupe is not collapsing them"
        );
        let cs = usize::from(clock.range().start());
        let ce = usize::from(clock.range().end());
        let ss = usize::from(stamp.range().start());
        let se = usize::from(stamp.range().end());
        assert_eq!(&src[cs..ce], "CLOCK: [2020-01-01");
        assert_eq!(&src[ss..se], "[2020-01-01");
    }
}
