//! Region forms: `parse_one_element`, `elements_in`, `inline_in`, `opaque_frame_end`.
//!
//! # Nesting budget
//!
//! The three region functions below ([`inline_in`], [`inline_to_limit`],
//! [`elements_in`]) are the only places the grammar re-enters itself, so they
//! are the only places that charge `ParseConfig::max_nesting_depth`. The other
//! `parse_one_element` callers (`drive::document`) loop at one level.
//!
//! The object loop lives here rather than in `forms::inline` so that no form
//! module can open an object region without charging the budget; the forms
//! own only the candidate table and [`inline::try_object`]. Past the budget a
//! region is flattened, not abandoned: its tokens become plain tiles under the
//! already-open node, the cursor lands where the recursive walk would have,
//! and callers close their constructs unchanged.

use crate::parser::grammar::forms::{inline, paragraph};
use crate::parser::{DiagnosticKind, LineView, Parser, Pos};
use crate::syntax::{OrgSyntaxKind, restrictions};

/// Exclusive body end (the start of the END line) of a closed block, drawer,
/// or latex environment opened on `line`; `None` when the line opens none.
///
/// List structure walks skip such interiors without measuring their
/// indentation. "Opaque" refers to that skip, not to
/// [`super::frame::BodyMode::Opaque`]: a closed verse block is skipped the
/// same way although its body holds objects. Closedness comes from
/// [`super::decide::decide`].
#[inline]
pub(crate) fn opaque_frame_end(line: LineView) -> Option<Pos> {
    match super::decide::decide(line) {
        Some(super::decide::Element::Block(shape)) => Some(shape.body_end()),
        Some(super::decide::Element::Drawer(shape)) => Some(shape.body_end()),
        Some(super::decide::Element::LatexEnvironment(shape)) => Some(shape.body_end()),
        _ => None,
    }
}

/// Parse inline objects from the cursor up to exclusive `end` under `scope`.
///
/// Charges one nesting level, pushes the active limit, walks objects under
/// `restrictions(scope)`, then restores. Over budget, [`flatten_region`] keeps
/// the region as plain tiles.
#[inline]
pub(crate) fn inline_in(parser: &mut Parser, end: Pos, scope: OrgSyntaxKind) {
    enter_region(parser, end, |p| {
        p.with_limit_at(end, |p| walk_inline(p, scope));
    });
}

/// Parse inline objects under `scope` up to the **already active** limit.
///
/// For regions whose cap the caller has pushed by other means (keyword values
/// under `with_rest_of_line`). Same budget contract as [`inline_in`].
#[inline]
pub(crate) fn inline_to_limit(parser: &mut Parser, scope: OrgSyntaxKind) {
    let end = parser.limit();
    enter_region(parser, end, |p| walk_inline(p, scope));
}

/// Walk objects at the cursor under `scope` until the active limit / EOF.
///
/// Sets the inline region start so PRE accepts openers at the first token of
/// this region. Called only through [`inline_in`] / [`inline_to_limit`].
fn walk_inline(parser: &mut Parser, scope: OrgSyntaxKind) {
    let allowed = restrictions(scope);
    let region_start = parser.cursor();
    parser.with_inline_region_start(Some(region_start), |parser| {
        while parser.current() != OrgSyntaxKind::EOF {
            let before = parser.progress();
            match inline::try_object(parser, allowed) {
                // Stop inside a TEXT tile where a TEXT row could open there,
                // else take the whole tile.
                None => match inline::next_text_opener(parser, allowed) {
                    Some(at) if parser.eat_to(at) => {}
                    _ => parser.eat_any(),
                },
                Some(name) if parser.progress() == before => {
                    // Claimed without progress: a grammar bug. In release,
                    // treat it as unclaimed so the loop cannot spin; `name`
                    // says which object stalled.
                    debug_assert!(
                        false,
                        "inline object claim made no source-cursor progress ({name})"
                    );
                    parser.eat_any();
                }
                Some(_) => {}
            }
        }
    });
}

/// Run `body` one nesting level deeper, or flatten `[cursor, end)` when the
/// budget is spent. All three region functions go through here.
fn enter_region(parser: &mut Parser, end: Pos, body: impl FnOnce(&mut Parser)) {
    if parser.with_nesting(body).is_none() {
        flatten_region(parser, end);
    }
}

/// Over budget: one diagnostic, then the region as plain tiles.
///
/// Still lossless: every byte stays in the tree, just without inner nodes.
/// The cursor lands where the recursive walk would have left it
/// (`min(end, limit)`), so the caller closes its construct unchanged.
fn flatten_region(parser: &mut Parser, end: Pos) {
    parser.error_span(parser.cursor(), end, DiagnosticKind::NestingLimitExceeded);
    parser.eat_until(end);
}

/// Parse successive elements from the cursor up to exclusive `end`.
///
/// The body of a greater element: one nesting level, then
/// [`parse_one_element`] until the limit.
///
/// Used by drawers, element-bodied blocks (`BodyMode::Elements`), footnote
/// definitions, list items, and closed inline tasks. Verse blocks take
/// [`inline_in`] instead. Over budget, [`flatten_region`] keeps the body as
/// plain tiles.
#[inline]
pub(crate) fn elements_in(parser: &mut Parser, end: Pos) {
    enter_region(parser, end, |parser| {
        parser.with_limit_at(end, |p| {
            while p.current() != OrgSyntaxKind::EOF {
                parse_one_element(p);
            }
        });
    });
}

/// Parse exactly one top-level element at the cursor.
///
/// 1. On a blank line, put the blank lines under the open container
///    (element rules normally take their own trailing blanks).
/// 2. At a line's first token, [`decide`] probes once and
///    [`super::decide::Element::parse`] parses. Mid-line or mid-tile, no probe
///    runs.
/// 3. If nothing was consumed (no probe matched, a degraded opener only
///    recorded its diagnostic, or a rule rewound, as clock lines can), a
///    paragraph takes the line, which guarantees progress.
///
/// Every element rule must either consume input or claim nothing, checked
/// with [`Parser::progress`]; a measured leaf ending mid-tile counts as
/// progress.
///
/// [`decide`]: super::decide::decide
pub(crate) fn parse_one_element(parser: &mut Parser) {
    // Never open PARAGRAPH on blank. Orphan blanks (e.g. lead-in inside a
    // greater block body) become token children of the open container.
    if parser.at_blank() {
        let before = parser.progress();
        parser.eat_blank_lines();
        debug_assert!(
            parser.progress() != before,
            "eat_blank_lines must advance when at_blank was true (spin guard)"
        );
        return;
    }

    let before = parser.progress();

    let element = parser.line().and_then(super::decide::decide);
    if let Some(element) = element {
        element.parse(parser);
    }

    if parser.progress() == before {
        paragraph::parse(parser);
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{DiagnosticKind, ParseConfig, Severity};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        make_parser, node_count, node_kinds, node_text, nodes_of_kind, parse_document_tree,
        tree_text,
    };

    /// `n` nested inline footnote references: the cheapest deep-nesting shape
    /// in the language at 5 source bytes per level (`[fn::` ... `]`).
    fn nested_footnote_refs(n: usize) -> String {
        let mut src = String::with_capacity(6 * n + 2);
        for _ in 0..n {
            src.push_str("[fn::");
        }
        src.push('x');
        for _ in 0..n {
            src.push(']');
        }
        src.push('\n');
        src
    }

    /// `n` nested list items, one indent step (2 columns) per level.
    fn nested_list_items(n: usize) -> String {
        let mut src = String::new();
        for i in 0..n {
            for _ in 0..(2 * i) {
                src.push(' ');
            }
            src.push_str("- i\n");
        }
        src
    }

    fn parse_at_budget(src: &str, budget: u16) -> crate::Document {
        crate::parse_with(src, &ParseConfig::default().with_max_nesting_depth(budget))
    }

    /// Object regions stop nesting at the budget and keep the rest as tiles.
    #[test]
    fn inline_region_over_budget_flattens_to_tiles() {
        let src = nested_footnote_refs(5);
        let out = parse_at_budget(&src, 2);

        assert_eq!(out.root().text().to_string(), src, "flatten stays lossless");
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE).len(),
            2,
            "exactly `budget` levels are structured; tree=\n{}",
            crate::tests::format_tree(out.root())
        );
        assert_eq!(
            out.diagnostics().len(),
            1,
            "one diagnostic per over-budget region (nothing inside recurses)"
        );
        assert_eq!(
            out.diagnostics()[0].kind(),
            &DiagnosticKind::NestingLimitExceeded
        );
        assert_eq!(out.diagnostics()[0].severity(), Severity::Error);
    }

    /// Element regions do the same, and the enclosing construct still closes:
    /// the flattened body leaves the cursor where the recursive walk would
    /// have, so `ITEM` / `PLAIN_LIST` complete unchanged.
    ///
    /// The count is `budget + 1` because the budget charges regions, and a
    /// SECTION body is not one: `parse_elements_until_heading` loops at one
    /// level. So the outermost list is free and each `elements_in` under an
    /// item costs one. Inline content has no free level: a paragraph opens a
    /// region for its own objects (see the footnote test above, where
    /// `budget = 2` yields exactly 2 levels).
    #[test]
    fn element_region_over_budget_flattens_and_parent_still_completes() {
        let src = nested_list_items(5);
        let out = parse_at_budget(&src, 2);

        assert_eq!(out.root().text().to_string(), src, "flatten stays lossless");
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::PLAIN_LIST).len(),
            3,
            "budget + 1: the top-level list is not inside a region; tree=\n{}",
            crate::tests::format_tree(out.root())
        );
        // Deepest structured item still owns its whole extent as text.
        let items = nodes_of_kind(out.root(), OrgSyntaxKind::ITEM);
        let deepest = items.last().expect("at least one ITEM");
        assert!(
            tree_text(deepest).contains("- i"),
            "flattened body text stays under the item"
        );
        assert!(
            out.diagnostics()
                .iter()
                .any(|d| d.kind() == &DiagnosticKind::NestingLimitExceeded)
        );
    }

    /// The diagnostic points at the span that lost its structure.
    #[test]
    fn flatten_diagnostic_covers_the_flattened_region() {
        // Budget 1: BOLD forms, its interior region flattens.
        let src = "*a [fn::x] b*\n";
        let out = parse_at_budget(src, 1);

        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(out.diagnostics().len(), 1);
        let d = &out.diagnostics()[0];
        assert_eq!(d.kind(), &DiagnosticKind::NestingLimitExceeded);
        let start = usize::from(d.range().start());
        let end = usize::from(d.range().end());
        assert_eq!(
            &src[start..end],
            "a [fn::x] b",
            "range is the bold interior, not the whole line"
        );
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "interior objects are tiles, not nodes"
        );
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::BOLD).len(),
            1,
            "the enclosing object still completes"
        );
    }

    /// Budget zero is the degenerate case, and it must still parse: paragraphs
    /// exist, their object regions are flat, every byte is in the tree.
    #[test]
    fn budget_zero_parses_flat_and_lossless() {
        let src = "*bold* and /italic/ with [[https://x.com][link]].\n";
        let out = parse_at_budget(src, 0);

        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(nodes_of_kind(out.root(), OrgSyntaxKind::PARAGRAPH).len(), 1);
        for kind in [
            OrgSyntaxKind::BOLD,
            OrgSyntaxKind::ITALIC,
            OrgSyntaxKind::LINK,
        ] {
            assert!(
                nodes_of_kind(out.root(), kind).is_empty(),
                "no {kind:?} at budget 0"
            );
        }
    }

    /// The keyword CAPTION value is an object region and charges the budget
    /// like any other (via `inline_to_limit`).
    #[test]
    fn caption_value_charges_the_budget() {
        let src = "#+CAPTION: a *b* c\npara\n";
        let full = crate::parse(src);
        assert_eq!(
            nodes_of_kind(full.root(), OrgSyntaxKind::BOLD).len(),
            1,
            "caption value is object-parsed by default"
        );

        let out = parse_at_budget(src, 0);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::BOLD).is_empty(),
            "caption objects flatten under the budget"
        );
        assert!(
            out.diagnostics()
                .iter()
                .any(|d| d.kind() == &DiagnosticKind::NestingLimitExceeded),
            "caption flatten is reported"
        );
    }

    #[test]
    fn opaque_frame_end_closed_block_drawer_and_latex_env() {
        // body_end is the first token of the matching END line.
        let p = make_parser("#+begin_src\nbody\n#+end_src\n");
        let line = p.line().expect("line");
        let end = super::opaque_frame_end(line).expect("closed src block");
        let end_line = p.line_of(end).expect("end line");
        assert_eq!(end_line.kind(0), OrgSyntaxKind::HASH);
        assert!(end_line.text(2).eq_ignore_ascii_case("end"));

        let p = make_parser(":LOG:\nx\n:END:\n");
        let line = p.line().expect("line");
        let end = super::opaque_frame_end(line).expect("closed drawer");
        let end_line = p.line_of(end).expect("end line");
        assert_eq!(end_line.kind(0), OrgSyntaxKind::COLON);
        assert!(end_line.text(1).eq_ignore_ascii_case("end"));

        let p = make_parser("\\begin{eq}\nbody\n\\end{eq}\n");
        let line = p.line().expect("line");
        let end = super::opaque_frame_end(line).expect("closed latex env");
        let end_line = p.line_of(end).expect("end line");
        assert_eq!(end_line.kind(0), OrgSyntaxKind::BACKSLASH);
        assert!(end_line.text_eq_caseless(1, "end"));
    }

    #[test]
    fn opaque_frame_end_none_for_non_frame_and_unclosed() {
        assert!(
            make_parser("- item\n")
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "list item is not a frame"
        );
        assert!(
            make_parser("paragraph\n")
                .line()
                .and_then(super::opaque_frame_end)
                .is_none()
        );
        assert!(
            make_parser("#+begin_src\nbody\n")
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "unterminated block"
        );
        assert!(
            make_parser(":LOG:\nx\n")
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "unterminated drawer"
        );
        assert!(
            make_parser("\\begin{eq}\nbody\n")
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "unterminated latex env"
        );
        // High-star tasks are never opaque frames (list uses a separate early arm).
        let high = "*".repeat(15);
        assert!(
            make_parser(&format!("{high} task\n"))
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "degenerate high-star task is not opaque_frame_end"
        );
        assert!(
            make_parser(&format!("{high} task\nbody\n{high} END\n"))
                .line()
                .and_then(super::opaque_frame_end)
                .is_none(),
            "closed high-star task is not opaque_frame_end"
        );
    }

    #[test]
    fn elements_in_empty_region_is_noop() {
        use super::elements_in;

        let mut p = make_parser("a\nb\n");
        let here = p.cursor();
        let key = p.progress();
        let elen = p.checkpoint().events_len;
        elements_in(&mut p, here);
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
        assert_eq!(p.cursor(), here);
        assert_eq!(p.limit().index(), p.tokens.len());
    }

    #[test]
    fn elements_in_drains_to_end_and_restores_limit() {
        use super::elements_in;

        // Two paragraphs; body ends at start of second line.
        let src = "first\nsecond\n";
        let mut p = make_parser(src);
        let second = p.line().expect("line 0").next().expect("line 1").start();
        elements_in(&mut p, second);
        assert_eq!(p.cursor(), second);
        assert_eq!(p.limit().index(), p.tokens.len());
        let (_, evs, _) = p.finish();
        assert_eq!(node_count(&evs, OrgSyntaxKind::PARAGRAPH), 1);
    }

    #[test]
    fn elements_in_multi_element_body() {
        use super::elements_in;

        // Blank between paragraphs yields two PARAGRAPH children before the cap.
        let src = "one\n\ntwo\nthree\n";
        let mut p = make_parser(src);
        let three = p
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| l.text(0) == "three")
            .expect("three line")
            .start();
        elements_in(&mut p, three);
        assert_eq!(p.cursor(), three);
        assert_eq!(p.limit().index(), p.tokens.len());
        let (_, evs, _) = p.finish();
        assert_eq!(node_count(&evs, OrgSyntaxKind::PARAGRAPH), 2);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::PARAGRAPH));
    }

    #[test]
    fn elements_in_clamps_to_outer_limit() {
        use super::elements_in;

        let src = "first\nsecond\nthird\n";
        let mut p = make_parser(src);
        let second = p.line().expect("l0").next().expect("l1").start();
        let third = p
            .line()
            .expect("l0")
            .next()
            .expect("l1")
            .next()
            .expect("l2")
            .start();
        // Outer cap at second; request body through third -- must stop at second.
        p.with_limit_at(second, |pp| {
            elements_in(pp, third);
            assert_eq!(pp.cursor(), second);
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
        assert_eq!(p.cursor(), second);
        assert_eq!(p.limit().index(), p.tokens.len());
        let (_, evs, _) = p.finish();
        assert_eq!(node_count(&evs, OrgSyntaxKind::PARAGRAPH), 1);
    }

    #[test]
    fn inline_in_empty_region_is_noop() {
        use super::inline_in;

        let mut p = make_parser("*bold* rest");
        let here = p.cursor();
        let key = p.progress();
        let elen = p.checkpoint().events_len;
        inline_in(&mut p, here, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
        assert_eq!(p.cursor(), here);
        assert_eq!(p.limit().index(), p.tokens.len());
    }

    #[test]
    fn inline_in_drains_and_restores_limit() {
        use super::inline_in;

        let src = "*bold* rest";
        let mut p = make_parser(src);
        // Cap after the closing * of bold (token index of WHITESPACE before "rest").
        let after_bold = p
            .tokens_from_cursor()
            .find(|t| t.kind() == OrgSyntaxKind::WHITESPACE)
            .expect("space after bold")
            .pos();
        inline_in(&mut p, after_bold, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(p.cursor(), after_bold);
        assert_eq!(p.limit().index(), p.tokens.len());
        assert_eq!(p.current(), OrgSyntaxKind::WHITESPACE);
        let (_, evs, _) = p.finish();
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::BOLD));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::BOLD).as_deref(),
            Some("*bold*")
        );
    }

    #[test]
    fn inline_in_clamps_to_outer_limit() {
        use super::inline_in;

        let src = "*bold* rest";
        let mut p = make_parser(src);
        // Outer cap mid-bold (after opener * and first TEXT tile of "bold").
        // Path TEXT is one tile; stop after first * so no closer is visible.
        let outer = crate::parser::Pos::from_index(p.cursor().index() + 1);
        let far = p.limit();
        p.with_limit_at(outer, |pp| {
            inline_in(pp, far, OrgSyntaxKind::PARAGRAPH);
            assert_eq!(pp.cursor(), outer);
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
        assert_eq!(p.cursor(), outer);
        assert_eq!(p.limit().index(), p.tokens.len());
        let (_, evs, _) = p.finish();
        // No complete bold under a cap that hides the closer.
        assert!(!node_kinds(&evs).contains(&OrgSyntaxKind::BOLD));
    }

    #[test]
    fn dispatcher_skips_decide_when_not_at_line_start() {
        // Line-shape still matches via `LineView` (paragraph termination), but
        // the dispatcher only probes at line-first (`Parser::line`). Mid-line
        // entry falls through to paragraph.
        use super::parse_one_element;

        let cases: &[(&str, OrgSyntaxKind)] = &[
            ("# c\n", OrgSyntaxKind::COMMENT),
            (": c\n", OrgSyntaxKind::FIXED_WIDTH),
            ("-----\n", OrgSyntaxKind::HORIZONTAL_RULE),
        ];
        for &(src, kind) in cases {
            let mut p = make_parser(src);
            assert!(
                p.line()
                    .and_then(crate::parser::grammar::drive::decide::decide)
                    .is_some(),
                "line-shape still matches for {src:?}"
            );
            p.eat_any(); // leave line.first
            assert!(p.line().is_none(), "fixture {src:?} must leave line-first");
            let before = p.progress();
            parse_one_element(&mut p);
            assert_ne!(
                p.progress(),
                before,
                "paragraph fallback must advance mid-line for {src:?}"
            );
            let (_, evs, _) = p.finish();
            assert_eq!(
                node_count(&evs, kind),
                0,
                "decide must not fire off line-first for {src:?}"
            );
            assert_eq!(
                node_count(&evs, OrgSyntaxKind::PARAGRAPH),
                1,
                "expected paragraph for {src:?}"
            );
            // Remainder of the line (after the first token) is paragraph text.
            let para = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH)
                .expect("paragraph text for mid-line fallthrough");
            assert!(
                !para.is_empty(),
                "paragraph must claim residual mid-line bytes for {src:?}"
            );
            assert_eq!(
                tree_text(&parse_document_tree(src)),
                src,
                "full-document roundtrip for {src:?}"
            );
        }
    }

    #[test]
    fn dispatcher_skips_decide_when_mid_tile() {
        // Inside the first tile the cursor is not at line start: no probe.
        use super::parse_one_element;

        let src = "-----\n";
        let mut p = make_parser(src);
        assert!(
            p.line()
                .and_then(crate::parser::grammar::drive::decide::decide)
                .is_some()
        );
        assert!(p.eat_to(1), "enter mid-tile on hyphen TEXT");
        assert!(p.progress().mid_offset() > 0);
        assert!(p.line().is_none());
        parse_one_element(&mut p);
        let (_, evs, _) = p.finish();
        assert_eq!(node_count(&evs, OrgSyntaxKind::HORIZONTAL_RULE), 0);
        assert_eq!(node_count(&evs, OrgSyntaxKind::PARAGRAPH), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).as_deref(),
            Some("----\n"), // exclusive residual after mid-tile start byte
        );
    }
}
