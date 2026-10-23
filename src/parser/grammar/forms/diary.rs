//! Diary s-expression family: column-0 element and active-stamp body.
//!
//! Both tree positions emit [`OrgSyntaxKind::DIARY_SEXP`]. They are not the
//! same CST shape; consumers branch on parent (`TIMESTAMP` vs section body).
//!
//! ## Element (lesser)
//!
//! Unindented line beginning `%%(` (org-element diary-sexp). Recognition
//! takes the whole line: only the column-0 lead is checked, parentheses need
//! not balance, and no diagnostic is recorded. Extent is the full line plus
//! post-blank.
//!
//! Evidence: org-syntax 4.3.3; `org-element-diary-sexp-parser` looking-at
//! `%%(.*` through end-of-line. Where Worg says the sexp is balanced, the
//! parser's behaviour is followed.
//!
//! ## Stamp body (child under TIMESTAMP)
//!
//! Active stamp only: after `<` is eaten, lead `%%(` then balanced interior
//! against the outer same-line `>`. Child extent is `(...)` only; the two
//! `PERCENT` tiles sit under `TIMESTAMP` outside the child. Success returns
//! `Some(outer_end)` (exclusive end of outer `>`); failure returns `None`.
//! The timestamp shell owns `UnbalancedDiarySexp` and optional wall time
//! after a successful body.
//!
//! Emits (element):
//! ```text
//! DIARY_SEXP
//!   PERCENT PERCENT L_PAREN ... [WHITESPACE] [NEWLINE]
//!   [post-blank tokens]
//! ```
//!
//! Emits (stamp child, under TIMESTAMP):
//! ```text
//! DIARY_SEXP   // tokens spanning (...) only
//! ```
//!
//! Entry points:
//! - [`probe`] / [`parse`]: the element, from the element dispatcher.
//! - [`lead_at`] / [`try_parse_stamp_body`]: the stamp body, from the
//!   timestamp grammar. The object dispatcher has no diary entry of its own.

use crate::parser::{LineView, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Shared lead
// ---------------------------------------------------------------------------

/// Pure diary lead `%%(` starting at `t` (first `%`).
///
/// Used by the element probe (column-0 line content) and the timestamp
/// active-stamp branch / probe gate.
#[inline]
pub(crate) fn lead_at(t: Option<TokenView<'_, '_>>) -> bool {
    t.is_some_and(|p1| {
        p1.seq(&[
            OrgSyntaxKind::PERCENT,
            OrgSyntaxKind::PERCENT,
            OrgSyntaxKind::L_PAREN,
        ])
    })
}

// ---------------------------------------------------------------------------
// Element (column-0 line)
// ---------------------------------------------------------------------------

/// Column-0 diary sexp line: content begins with `%%(`.
///
/// org-element-diary-sexp-parser looking-at takes the whole line after `%%(`;
/// balanced parentheses are not required for recognition. Indented lines are
/// ordinary paragraph text.
pub(crate) fn probe(line: LineView) -> Option<()> {
    if line.indent_cols() != 0 {
        return None;
    }
    lead_at(line.nth(0)).then_some(())
}

/// Parse one DIARY_SEXP element (exactly one line + post-blank).
///
/// Flat leaf: interior sexp atoms are not structured. Consecutive diary lines
/// are separate nodes (do not fold like comment / fixed-width marker runs).
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser.line().is_some_and(|l| probe(l).is_some()),
        "diary::parse called without probe (dispatcher contract)"
    );

    let marker = parser.start();
    parser.eat_line();
    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::DIARY_SEXP);
}

// ---------------------------------------------------------------------------
// Stamp body (active TIMESTAMP child)
// ---------------------------------------------------------------------------

/// Diary stamp body after the active opener was eaten; cursor on first `%` of
/// `%%(`.
///
/// On success: emits a `DIARY_SEXP` child spanning `(...)` only, leaves the
/// cursor after the closing `)`, and returns the exclusive end of the outer
/// same-line `>` so the caller can carve optional time through that closer.
/// On failure: may leave partial tokens and returns `None`; the caller records
/// the diagnostic and recovers. Both bounds are measured before emitting, so
/// the caller never searches for the closer again.
pub(crate) fn try_parse_stamp_body(parser: &mut Parser) -> Option<Pos> {
    // Cursor is on the first `%` of `%%(`.
    parser.eat_any(); // %
    parser.eat_any(); // %

    let sexp_marker = parser.start();
    parser.eat_any(); // (

    // Outer closer first (first `>` before newline); sexp ends at last `)`
    // before it. Both ends are absolute so `eat_until(sexp_end)` is safe;
    // outer end is returned for the timestamp shell (optional time + drain).
    let Some((outer_end, sexp_end)) = diary_bounds(parser) else {
        sexp_marker.abandon(parser);
        return None;
    };

    // Consume `(...)` including the closing `)`.
    parser.eat_until(sexp_end);
    sexp_marker.complete(parser, OrgSyntaxKind::DIARY_SEXP);
    Some(outer_end)
}

/// Exclusive ends after the outer `>` and after the sexp-closing `)` (the
/// last `)` before the outer closer). `None` when either bound is missing.
fn diary_bounds(parser: &Parser) -> Option<(Pos, Pos)> {
    let outer = parser.find_same_line_kind(OrgSyntaxKind::R_ANGLE)?;
    let mut last_paren = None;
    for t in parser.tokens_from_cursor() {
        if t.pos() >= outer.pos() {
            break;
        }
        if t.kind() == OrgSyntaxKind::R_PAREN {
            last_paren = Some(t);
        }
    }
    Some((outer.end(), last_paren?.end()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{lead_at, parse, probe, try_parse_stamp_body};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        ParseResult, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, probe_line, run, tree_text,
    };

    fn results_of(src: &str) -> ParseResult {
        run(src, parse)
    }

    // --- Element probe (pure) ----------------------------------------------

    #[test]
    fn probe_column0_lead_accepts() {
        assert!(probe_line("%%(org-calendar-holiday)\n", probe).is_some());
        assert!(probe_line("%%(foo\n", probe).is_some()); // unbalanced still claims
        assert!(probe_line("%%((nested) x)\n", probe).is_some());
        assert!(probe_line("%%(\n", probe).is_some()); // minimal after lead
        assert!(probe_line("%%(foo)", probe).is_some()); // EOF, no NL
    }

    #[test]
    fn probe_indent_and_non_lead_reject() {
        assert!(probe_line("  %%(foo)\n", probe).is_none());
        assert!(probe_line("\t%%(foo)\n", probe).is_none());
        assert!(probe_line("%% foo\n", probe).is_none());
        assert!(probe_line("% (foo)\n", probe).is_none());
        // Four PERCENT then L_PAREN: third tile is %, not `(`.
        assert!(probe_line("%%%%(foo)\n", probe).is_none());
        assert!(probe_line("See <%%(foo)>\n", probe).is_none());
        assert!(probe_line("not diary\n", probe).is_none());
    }

    #[test]
    fn lead_at_three_token_shape() {
        let p = make_parser("%%(x)");
        assert!(lead_at(p.here()));
        let p = make_parser("%(x)");
        assert!(!lead_at(p.here()));
        let p = make_parser("%%%%(x)");
        assert!(!lead_at(p.here()));
        let p = make_parser("%% x)");
        assert!(!lead_at(p.here()));
    }

    // --- Element parse -----------------------------------------------------

    #[test]
    fn parse_consumes_exactly_one_line() {
        let src = "%%(a)\n%%(b)\n";
        let mut p = make_parser(src);
        parse(&mut p);
        assert!(p.line().is_some_and(|l| probe(l).is_some()));
        // Lead is three single-char tokens; cursor sits on the first `%`.
        assert_eq!(p.current(), OrgSyntaxKind::PERCENT);
        assert!(lead_at(p.here()));
    }

    #[test]
    fn emits_diary_sexp_covering_full_line() {
        let src = "%%(org-calendar-holiday)\n";
        let (_tokens, events, _) = results_of(src);
        assert_eq!(node_count(&events, OrgSyntaxKind::DIARY_SEXP), 1);
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::DIARY_SEXP).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn line_take_includes_trailing_junk() {
        // org-element looking-at claims the whole line after `%%(`; no rest
        // shape gate. Trailing same-line text stays under the element.
        let src = "%%(foo) trailing text\n";
        let (_tokens, events, _) = results_of(src);
        assert_eq!(node_count(&events, OrgSyntaxKind::DIARY_SEXP), 1);
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::DIARY_SEXP).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn post_blank_owned_by_element() {
        let src = "%%(foo)\n\n\n";
        let (_tokens, events, _) = results_of(src);
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::DIARY_SEXP).as_deref(),
            Some(src)
        );
    }

    // --- Document tree (element) -------------------------------------------

    #[test]
    fn diary_element_appears_in_document_tree() {
        let src = "before\n%%(org-calendar-holiday)\nafter\n";
        let root = parse_document_tree(src);
        let diaries = nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&diaries[0]), "%%(org-calendar-holiday)\n");
        // Unlike the stamp child, the element's parent is the zeroth SECTION.
        assert_eq!(
            diaries[0].parent().map(|p| p.kind()),
            Some(OrgSyntaxKind::SECTION)
        );
    }

    #[test]
    fn consecutive_diary_lines_are_two_nodes() {
        let src = "%%(a)\n%%(b)\n";
        let root = parse_document_tree(src);
        let diaries = nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 2, "must not fold into one marker run");
        assert_eq!(tree_text(&diaries[0]), "%%(a)\n");
        assert_eq!(tree_text(&diaries[1]), "%%(b)\n");
    }

    #[test]
    fn indented_diary_stays_paragraph() {
        let src = "  %%(foo)\n";
        let root = parse_document_tree(src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP).is_empty(),
            "indented %%( is not a diary element"
        );
        assert!(
            !nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).is_empty(),
            "indented line is paragraph text"
        );
    }

    #[test]
    fn interrupts_paragraph() {
        let src = "para\n%%(foo)\nmore\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        let diaries = nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(paras.len(), 2, "split around diary: {paras:?}");
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&paras[0]), "para\n");
        assert_eq!(tree_text(&diaries[0]), "%%(foo)\n");
        assert_eq!(tree_text(&paras[1]), "more\n");
    }

    #[test]
    fn unbalanced_line_is_element_without_stamp_diag() {
        let src = "%%(foo\n";
        let out = crate::parse(src);
        let diaries = nodes_of_kind(out.root(), OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&diaries[0]), src);
        assert!(
            !out.diagnostics()
                .iter()
                .any(|d| { matches!(d.kind(), crate::DiagnosticKind::UnbalancedDiarySexp) }),
            "element path must not emit UnbalancedDiarySexp: {:?}",
            out.diagnostics()
        );
    }

    #[test]
    fn does_not_steal_stamp_diary_in_paragraph() {
        let src = "See <%%(foo)>\n";
        let root = parse_document_tree(src);
        let diaries = nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        // Sole DIARY_SEXP is the stamp child under TIMESTAMP.
        assert_eq!(
            diaries[0].parent().map(|p| p.kind()),
            Some(OrgSyntaxKind::TIMESTAMP)
        );
        assert_eq!(tree_text(&diaries[0]), "(foo)");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).len(),
            1,
            "active stamp still forms"
        );
    }

    #[test]
    fn list_sibling_after_item_not_list_content() {
        let src = "- item\n%%(hol)\n";
        let root = parse_document_tree(src);
        let lists = nodes_of_kind(&root, OrgSyntaxKind::PLAIN_LIST);
        let diaries = nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(lists.len(), 1);
        assert_eq!(diaries.len(), 1);
        // Diary is not under the list.
        assert!(
            nodes_of_kind(&lists[0], OrgSyntaxKind::DIARY_SEXP).is_empty(),
            "diary must not nest under PLAIN_LIST"
        );
        // Same parent as the list (section / document body).
        assert_eq!(
            lists[0].parent().map(|p| p.kind()),
            diaries[0].parent().map(|p| p.kind()),
            "list and diary are siblings"
        );
    }

    #[test]
    fn indented_under_item_stays_paragraph() {
        let src = "- item\n  %%(foo)\n";
        let root = parse_document_tree(src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP).is_empty(),
            "indented under item is not diary element"
        );
        let lists = nodes_of_kind(&root, OrgSyntaxKind::PLAIN_LIST);
        assert_eq!(lists.len(), 1);
        assert!(
            !nodes_of_kind(&lists[0], OrgSyntaxKind::PARAGRAPH).is_empty(),
            "indented %%( remains paragraph text in the item"
        );
    }

    #[test]
    fn footnote_body_child_not_terminator() {
        let src = "[fn:1] x\n%%(hol)\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        let diaries = nodes_of_kind(&defs[0], OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(
            diaries.len(),
            1,
            "diary is a child of FOOTNOTE_DEFINITION, not a body terminator"
        );
        assert_eq!(tree_text(&diaries[0]), "%%(hol)\n");
    }

    // --- Stamp body ---------------------------------------------------------

    #[test]
    fn stamp_body_basic_emits_child_extent() {
        // Full stamp shell is timestamp's job; body helper alone after `<`.
        let src = "%%(diary-float t 4 2)>";
        let mut p = make_parser(src);
        let outer_end = try_parse_stamp_body(&mut p).expect("stamp body ok");
        // Cursor after `)`; outer `>` remains for the timestamp shell.
        assert_eq!(p.current(), OrgSyntaxKind::R_ANGLE);
        // Returned outer end is exclusive end of that closer (no re-find).
        assert_eq!(outer_end, p.here().expect("on closer").end());
        let (_toks, evs, diags) = p.finish();
        assert!(diags.is_empty(), "stamp body success is silent: {diags:?}");
        assert_eq!(node_count(&evs, OrgSyntaxKind::DIARY_SEXP), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::DIARY_SEXP).as_deref(),
            Some("(diary-float t 4 2)")
        );
    }

    #[test]
    fn stamp_body_nested_parens() {
        let src = "%%(memq (calendar-day-of-week date) '(1 3 5))>";
        let mut p = make_parser(src);
        assert!(try_parse_stamp_body(&mut p).is_some());
        let (_toks, evs, _) = p.finish();
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::DIARY_SEXP).as_deref(),
            Some("(memq (calendar-day-of-week date) '(1 3 5))")
        );
    }

    #[test]
    fn stamp_body_incomplete_fails() {
        let mut p = make_parser("%%(foo");
        assert!(try_parse_stamp_body(&mut p).is_none());
        let (_toks, evs, diags) = p.finish();
        assert!(
            !node_kinds(&evs).contains(&OrgSyntaxKind::DIARY_SEXP),
            "failed body must not emit DIARY_SEXP"
        );
        // Diag is owned by the timestamp shell, not this helper.
        assert!(
            diags.is_empty(),
            "body helper does not emit diags: {diags:?}"
        );
    }

    #[test]
    fn stamp_body_no_outer_closer_fails() {
        let mut p = make_parser("%%(foo) 12:00");
        assert!(try_parse_stamp_body(&mut p).is_none());
        let (_toks, evs, diags) = p.finish();
        assert!(
            !node_kinds(&evs).contains(&OrgSyntaxKind::DIARY_SEXP),
            "failed body must not emit DIARY_SEXP"
        );
        assert!(
            diags.is_empty(),
            "body helper does not emit diags: {diags:?}"
        );
    }

    #[test]
    fn stamp_body_rejects_angle_inside_sexp() {
        for src in ["%%(a>b)>", "%%(memq (x > y) z)>"] {
            let mut p = make_parser(src);
            assert!(
                try_parse_stamp_body(&mut p).is_none(),
                "angle-inside must fail for {src:?}"
            );
            let (_toks, evs, diags) = p.finish();
            assert!(
                !node_kinds(&evs).contains(&OrgSyntaxKind::DIARY_SEXP),
                "failed body must not emit DIARY_SEXP for {src:?}"
            );
            assert!(
                diags.is_empty(),
                "body helper does not emit diags for {src:?}: {diags:?}"
            );
        }
    }

    #[test]
    fn stamp_diary_document_tree_child_extent() {
        // The child is `(...)` only; `%%` sits under TIMESTAMP.
        let src = "See <%%(diary-float t 4 2)>\n";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(tree_text(&stamps[0]), "<%%(diary-float t 4 2)>");
        let diaries = nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&diaries[0]), "(diary-float t 4 2)");
    }

    #[test]
    fn stamp_diary_with_time_still_under_timestamp() {
        let src = "<%%(diary-float t 4 2) 10:00-12:00>";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(tree_text(&stamps[0]), src);
        let diaries = nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&diaries[0]), "(diary-float t 4 2)");
        let ranges = nodes_of_kind(&stamps[0], OrgSyntaxKind::TIME_RANGE);
        assert_eq!(ranges.len(), 1, "optional time stays timestamp-owned");
        assert_eq!(tree_text(&ranges[0]), "10:00-12:00");
    }

    // --- Full timestamp, via document parse ---------------------------------

    #[test]
    fn stamp_document_basic() {
        let src = "<%%(diary-float t 4 2)>";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(tree_text(&stamps[0]), src);
        let diaries = nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(tree_text(&diaries[0]), "(diary-float t 4 2)");
    }

    #[test]
    fn stamp_document_with_point_time() {
        let src = "<%%(diary-float t 4 2) 09:05>";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(tree_text(&stamps[0]), src);
        assert_eq!(
            nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP).len(),
            1
        );
        let times = nodes_of_kind(&stamps[0], OrgSyntaxKind::TIME);
        assert_eq!(times.len(), 1);
        assert_eq!(tree_text(&times[0]), "09:05");
        assert!(
            nodes_of_kind(&stamps[0], OrgSyntaxKind::TIME_RANGE).is_empty(),
            "point wall time is TIME, not TIME_RANGE"
        );
    }

    #[test]
    fn stamp_document_nested_parens() {
        let src = "<%%(memq (calendar-day-of-week date) '(1 3 5))>";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(tree_text(&stamps[0]), src);
        let diaries = nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP);
        assert_eq!(diaries.len(), 1);
        assert_eq!(
            tree_text(&diaries[0]),
            "(memq (calendar-day-of-week date) '(1 3 5))"
        );
    }

    #[test]
    fn stamp_document_rejects_angle_inside_sexp() {
        // Inner `>` terminates the outer closer search before any later `)`.
        for src in ["See <%%(a>b)>\n", "See <%%(memq (x > y) z)>\n"] {
            let root = parse_document_tree(src);
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).is_empty(),
                "not a TIMESTAMP: {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::DIARY_SEXP).is_empty(),
                "angle-inside sexp must not emit DIARY_SEXP for {src:?}"
            );
        }
    }

    #[test]
    fn stamp_document_seconds_time_is_point_not_range() {
        let src = "<%%(diary-float t 4 2) 10:00:30>";
        let root = parse_document_tree(src);
        let stamps = nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(stamps.len(), 1);
        assert_eq!(
            nodes_of_kind(&stamps[0], OrgSyntaxKind::DIARY_SEXP).len(),
            1
        );
        let times = nodes_of_kind(&stamps[0], OrgSyntaxKind::TIME);
        assert_eq!(times.len(), 1);
        assert_eq!(tree_text(&times[0]), "10:00");
        assert!(
            nodes_of_kind(&stamps[0], OrgSyntaxKind::TIME_RANGE).is_empty(),
            "seconds wall time stays point TIME"
        );
    }
}
