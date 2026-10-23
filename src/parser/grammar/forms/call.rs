//! Babel call value grammar: element (`#+call:`) and inline (`call_...`).
//!
//! Structure only, matching the org-element properties: name, optional inside
//! header, arguments, end header. Interiors stay opaque tokens; babel
//! `:key val` headers and argument lists are not split. Empty `()` is kept in
//! the CST for losslessness; org-element's `org-string-nw-p` collapse of empty
//! values to nil is not applied here.
//!
//! One walker serves both forms, with mode-specific acceptance:
//!
//! | Rule        | Element                         | Inline                          |
//! | ----------- | ------------------------------- | ------------------------------- |
//! | Prefix      | `#+call:` (keyword shell)       | literal `call_` (case-sensitive)|
//! | Name stops  | `[]()\n` (spaces allowed)       | whitespace / `[` / `(`          |
//! | Empty name  | allowed (empty CALL_NAME)       | hard fail                       |
//! | Args        | optional                        | required open `(` for accept    |
//! | End header  | rest of line                    | paired `[]` only                |
//!
//! Evidence: org-element-babel-call-parser, org-element-inline-babel-call-parser,
//! org-syntax sections 4.3.8.1 and 5.7. Element end-header is remainder of line
//! after name/inside/args, not a second paired-bracket parse.
//!
//! Emits under BABEL_CALL (via keyword.rs) or INLINE_BABEL_CALL:
//! ```text
//! CALL_NAME
//! [CALL_INSIDE_HEADER]   // includes [ ]
//! [CALL_ARGUMENTS]       // includes ( ); empty interior kept for ()
//! [CALL_END_HEADER]
//! ```
//!
//! Failure handling:
//! - Element: always completes once the key is `call`; an empty or malformed
//!   value still tiles.
//! - Inline: [`probe_inline`] walks the name and arguments once, purely, and
//!   [`parse_inline`] emits from that shape. When the prefix matches (org's
//!   `looking-at`) but the layout is incomplete, the shape is
//!   `InlineCallShape { layout: None }` and emit consumes only the bare
//!   `call_` tokens, leaving no orphan `CALL_*` nodes.
//!
//! Word boundary before `call_` ([`atoms::at_word_start`]): region or line
//! start, or WHITESPACE / INDENT / NEWLINE. This approximates Emacs `\<` with
//! separator tokens rather than syntax-table word characters. Glued prefixes
//! such as `xcall_` fail because the TEXT run is not literally `call`.

use crate::parser::grammar::drive::atoms;
use crate::parser::{PairNewlines, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

/// Value-side acceptance mode for the shared call walker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallMode {
    /// After `#+call:` / `#+CALL:` (keyword shell owns prefix).
    Element,
    /// After literal `call_` (object owns prefix).
    Inline,
}

// ---------------------------------------------------------------------------
// Measured layout (typed positions; emit is a second pass)
// ---------------------------------------------------------------------------

/// Position layout of a call value (name through optional end).
///
/// Every region is a half-open `[start, end)` position pair; paired regions
/// include their delimiters (`[`/`]`, `(`/`)`). Positions are absolute, so
/// the layout survives the consuming pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CallLayout {
    /// First name token (value start).
    name_start: Pos,
    /// One past the last name token (empty name => equal).
    name_end: Pos,
    /// `[` through `]` (exclusive end) for inside header.
    inside: Option<(Pos, Pos)>,
    /// `(` through `)` (exclusive end) for arguments.
    args: Option<(Pos, Pos)>,
    /// End-header region.
    end: Option<(Pos, Pos)>,
}

/// Inline babel call: an opener claim plus an optional full layout.
///
/// `layout` is `None` when the opener matches but the full measure fails
/// (missing or unclosed arguments); parse then leaves `call_` as bare tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineCallShape {
    /// Full name/args/end layout, or `None` when only the opener matched.
    pub(crate) layout: Option<CallLayout>,
}

// ---------------------------------------------------------------------------
// Public surface: element value (keyword path)
// ---------------------------------------------------------------------------

/// Parse the value of a `#+call:` line after `:` and optional whitespace.
///
/// Bound is the end of the line (NEWLINE or visible end). Always succeeds and
/// tiles the value region with `CALL_*` children, without a `KEYWORD_VALUE`
/// wrapper.
pub(crate) fn parse_babel_call_value(parser: &mut Parser) {
    parser.with_rest_of_line(|p| {
        let layout = measure_element_call_value(p);
        consume_call_value(p, &layout);
        // The element measure always tiles to the line bound: name, optional
        // parts, then the rest of the line as end header.
        debug_assert_eq!(
            p.current(),
            OrgSyntaxKind::EOF,
            "babel call value must tile the full line-bounded region"
        );
    });
}

// ---------------------------------------------------------------------------
// Public surface: inline object
// ---------------------------------------------------------------------------

/// Pure probe: word start, `call_`, then one name and arguments walk.
///
/// Returns `None` when the opener does not match (no word start, bad prefix,
/// empty name, or name not followed by `[`/`(`). Returns
/// `Some(InlineCallShape { layout: None })` when the opener matches but the
/// arguments are missing or unclosed, so emit consumes only `call_`. Returns
/// `Some` with a full layout when the complete inline form measures.
pub(crate) fn probe_inline(parser: &Parser) -> Option<InlineCallShape> {
    if !atoms::at_word_start(parser) {
        return None;
    }
    // Fixed prefix: TEXT("call") UNDERSCORE (shared TEXT-prefix atom).
    let name_start = atoms::fixed_underscore_prefix(parser, "call")?;

    // Single walk from the first name token (after `call_`).
    let walked = walk_call_value(parser, parser.token_view(name_start), CallMode::Inline);
    if walked.name_end == walked.name_start {
        return None; // empty name: the opener does not match
    }
    // Looking-at requires `[` or `(` immediately after the name run.
    let after_name_kind = parser
        .token_view(walked.name_end)
        .map_or(OrgSyntaxKind::EOF, |t| t.kind());
    if !matches!(
        after_name_kind,
        OrgSyntaxKind::L_BRACKET | OrgSyntaxKind::L_PAREN
    ) {
        return None;
    }

    // A full layout needs closed arguments; otherwise claim only the opener.
    if walked.args.is_none() {
        return Some(InlineCallShape { layout: None });
    }
    Some(InlineCallShape {
        layout: Some(CallLayout {
            name_start: walked.name_start,
            name_end: walked.name_end,
            inside: walked.inside,
            args: walked.args,
            end: measure_inline_end_header(parser, walked.after),
        }),
    })
}

/// Test harness: true when [`probe_inline`] claims, including the
/// `layout: None` cases.
#[cfg(test)]
#[inline]
pub(crate) fn at_inline_babel_call(parser: &Parser) -> bool {
    probe_inline(parser).is_some()
}

/// Emit an inline babel call from a measured shape.
///
/// Always consumes the two-token `call_` opener. On `layout: None` (opener
/// matched, full measure failed) it abandons the node after that opener.
pub(crate) fn parse_inline(parser: &mut Parser, shape: InlineCallShape) {
    let marker = parser.start();
    parser.eat(OrgSyntaxKind::TEXT);
    parser.eat(OrgSyntaxKind::UNDERSCORE);
    match shape.layout {
        Some(layout) => {
            consume_call_value(parser, &layout);
            marker.complete(parser, OrgSyntaxKind::INLINE_BABEL_CALL);
        }
        None => {
            // Leave the opener as bare tokens, as link and timestamp do.
            marker.abandon(parser);
        }
    }
}

/// Harness: probe then parse (unit tests / local walks).
#[cfg(test)]
pub(crate) fn parse_inline_babel_call(parser: &mut Parser) {
    if let Some(shape) = probe_inline(parser) {
        parse_inline(parser, shape);
    }
}

// ---------------------------------------------------------------------------
// Shared measure / emit
// ---------------------------------------------------------------------------

/// Element measure: always a layout (empty name allowed, arguments optional,
/// end header is the rest of the line).
fn measure_element_call_value(parser: &Parser) -> CallLayout {
    let walked = walk_call_value(
        parser,
        parser.tokens_from_cursor().next(),
        CallMode::Element,
    );
    CallLayout {
        name_start: walked.name_start,
        name_end: walked.name_end,
        inside: walked.inside,
        args: walked.args,
        end: measure_element_end_header(parser, walked.after),
    }
}

/// Inline measure starting after `call_` (two tokens ahead of the cursor).
///
/// Used by unit tests of the value walk; the parser itself uses
/// [`probe_inline`]. `None` when the name is empty or the arguments are
/// missing or unclosed.
#[cfg(test)]
fn measure_inline_after_prefix(parser: &Parser) -> Option<CallLayout> {
    // `call_` is two tokens (TEXT + UNDERSCORE); value walk starts after them.
    let walked = walk_call_value(parser, parser.tokens_from_cursor().nth(2), CallMode::Inline);
    if walked.name_end == walked.name_start || walked.args.is_none() {
        return None;
    }
    Some(CallLayout {
        name_start: walked.name_start,
        name_end: walked.name_end,
        inside: walked.inside,
        args: walked.args,
        end: measure_inline_end_header(parser, walked.after),
    })
}

/// Intermediate walk result: name + optional pairs; end-header measured separately.
struct WalkedCall {
    name_start: Pos,
    name_end: Pos,
    inside: Option<(Pos, Pos)>,
    args: Option<(Pos, Pos)>,
    /// Position of the first token after name/inside/args (region end when
    /// the walk ran out of visible tokens).
    after: Pos,
}

/// Shared walk over the name and optional `[...]` / `(...)`, without
/// acceptance checks. `cur` is the first candidate name token (`None` at the
/// visible end).
fn walk_call_value<'a, 's>(
    parser: &'a Parser<'s>,
    cur: Option<TokenView<'a, 's>>,
    mode: CallMode,
) -> WalkedCall {
    // The name uses the shared measure with this module's mode-specific stops.
    let name_start = cur.map_or_else(|| parser.limit(), |t| t.pos());
    let (name_start, name_end, mut cur) =
        atoms::measure_name(parser, name_start, |k| is_name_stop(k, mode));

    // Unclosed `[` leaves the walk at the opener for args / end handling.
    // Inside-header / args are line-bounded (Worg 4.3.8.1 / 5.7).
    let mut inside = None;
    if let Some(open) = cur.filter(|t| t.kind() == OrgSyntaxKind::L_BRACKET)
        && let Some((p_start, p_end)) = parser.measure_paired_at(
            open.pos(),
            OrgSyntaxKind::L_BRACKET,
            OrgSyntaxKind::R_BRACKET,
            PairNewlines::Stop,
        )
    {
        inside = Some((p_start, p_end));
        cur = parser.token_view(p_end);
    }

    let mut args = None;
    if let Some(open) = cur.filter(|t| t.kind() == OrgSyntaxKind::L_PAREN)
        && let Some((p_start, p_end)) = parser.measure_paired_at(
            open.pos(),
            OrgSyntaxKind::L_PAREN,
            OrgSyntaxKind::R_PAREN,
            PairNewlines::Stop,
        )
    {
        args = Some((p_start, p_end));
        cur = parser.token_view(p_end);
    }

    WalkedCall {
        name_start,
        name_end,
        inside,
        args,
        after: cur.map_or_else(|| parser.limit(), |t| t.pos()),
    }
}

/// Element end header: the rest of the line under the caller's line region.
fn measure_element_end_header(parser: &Parser, after: Pos) -> Option<(Pos, Pos)> {
    // NEWLINE is outside the visible window by design; remaining tokens run
    // to the region end.
    (after < parser.limit()).then(|| (after, parser.limit()))
}

/// Inline end-header: paired `[]` only; unclosed or absent is omitted.
fn measure_inline_end_header(parser: &Parser, after: Pos) -> Option<(Pos, Pos)> {
    parser.measure_paired_at(
        after,
        OrgSyntaxKind::L_BRACKET,
        OrgSyntaxKind::R_BRACKET,
        PairNewlines::Stop,
    )
}

/// Consume CALL_* nodes according to a measured layout.
///
/// Precondition: cursor is at `layout.name_start`.
fn consume_call_value(parser: &mut Parser, layout: &CallLayout) {
    debug_assert_eq!(parser.cursor(), layout.name_start);

    // CALL_NAME (empty node allowed for element tile).
    parser.leaf_to(OrgSyntaxKind::CALL_NAME, layout.name_end);

    if let Some((_, end)) = layout.inside {
        parser.leaf_to(OrgSyntaxKind::CALL_INSIDE_HEADER, end);
    }
    if let Some((_, end)) = layout.args {
        parser.leaf_to(OrgSyntaxKind::CALL_ARGUMENTS, end);
    }
    if let Some((_, end)) = layout.end {
        parser.leaf_to(OrgSyntaxKind::CALL_END_HEADER, end);
    }
}

// ---------------------------------------------------------------------------
// Name stop predicates
// ---------------------------------------------------------------------------

#[inline]
fn is_name_stop(kind: OrgSyntaxKind, mode: CallMode) -> bool {
    match mode {
        CallMode::Element => is_element_name_stop(kind),
        CallMode::Inline => is_inline_name_stop(kind),
    }
}

/// Element name: elisp `skip-chars-forward "^[]()\n"`.
///
/// `EOF` is in the set so kind probes past the run stop too; the walks
/// themselves are bounded by `Option`, not by `EOF`.
#[inline]
fn is_element_name_stop(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::L_BRACKET
            | OrgSyntaxKind::R_BRACKET
            | OrgSyntaxKind::L_PAREN
            | OrgSyntaxKind::R_PAREN
            | OrgSyntaxKind::NEWLINE
            | OrgSyntaxKind::EOF
    )
}

/// Inline name: elisp `[^ \t\n[(]+`.
///
/// `EOF` is in the set for kind probes past the run, as for the element form.
#[inline]
fn is_inline_name_stop(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::WHITESPACE
            | OrgSyntaxKind::INDENT
            | OrgSyntaxKind::NEWLINE
            | OrgSyntaxKind::L_BRACKET
            | OrgSyntaxKind::L_PAREN
            | OrgSyntaxKind::EOF
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        ParseResult, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, run, run_inline, tree_text,
    };

    // Guarded object harness (inline can reject). Region start = cursor at entry.
    fn results_of_inline(src: &str) -> ParseResult {
        run_inline(src, |p| {
            if at_inline_babel_call(p) {
                parse_inline_babel_call(p);
            } else if p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
        })
    }

    /// `at_*` at absolute region start 0 (standalone snippets and BOF cases).
    fn at_call(src: &str) -> bool {
        at_inline_babel_call(&make_parser(src))
    }

    /// `at_*` with the cursor advanced `eats` tokens first (PRE context cases).
    fn at_call_after(src: &str, eats: usize) -> bool {
        let mut p = make_parser(src);
        for _ in 0..eats {
            p.eat_any();
        }
        at_inline_babel_call(&p)
    }

    fn results_of_element_value(src: &str) -> ParseResult {
        // Value-only: src is the value region (no #+call: shell).
        run(src, parse_babel_call_value)
    }

    fn parses_inline(src: &str) -> bool {
        node_kinds(&results_of_inline(src).1).contains(&OrgSyntaxKind::INLINE_BABEL_CALL)
    }

    /// Absolute index of the first token after literal `call_` (two tokens).
    fn after_call_prefix(parser: &Parser) -> crate::parser::Pos {
        crate::parser::Pos::from_index(parser.cursor().index() + 2)
    }

    /// Row for data-driven pattern tables: (src, name, inside, args, end).
    type PatternRow<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );

    /// Inline variant where args are always present (required for recognition).
    type InlinePatternRow<'a> = (&'a str, &'a str, Option<&'a str>, &'a str, Option<&'a str>);

    // --- Pure observers ----------------------------------------------------

    #[test]
    fn cross_line_arg_closer_does_not_form_inline_call() {
        // Worg 5.7: ARGUMENTS are non-newline; `)` on the next line is not the closer.
        let src = "call_foo(bar\n)\n";
        assert!(!parses_inline(src));
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            !out.root()
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::INLINE_BABEL_CALL)
        );
    }

    #[test]
    fn at_inline_babel_call_is_pure_and_case_sensitive() {
        let p = make_parser("call_foo(bar)");
        let before = p.cursor().index();
        assert!(at_inline_babel_call(&p));
        assert_eq!(p.cursor().index(), before);

        assert!(!at_call("CALL_foo(bar)"));
        assert!(!at_call("Call_foo(bar)"));
        assert!(!at_call("call_(bar)"));
        assert!(!at_call("call_foo"));
        // Leading TEXT is "xcall", not "call".
        assert!(!at_call("xcall_foo(bar)"));
        assert!(!at_call("not_a_call(x)"));
    }

    #[test]
    fn at_inline_babel_call_accepts_inside_open_or_paren() {
        assert!(at_call("call_n[i](a)"));
        assert!(at_call("call_n(a)"));
        assert!(at_call("call_n[i]")); // opener only
        assert!(!at_call("call_n x(a)"));
    }

    #[test]
    fn at_inline_babel_call_after_leading_text() {
        // Cursor at line start; call sits after "pre " (TEXT WHITESPACE).
        let p = make_parser("pre call_foo(bar)");
        // tokens: TEXT("pre") WHITESPACE TEXT("call") UNDERSCORE ...
        assert!(!at_inline_babel_call(&p));
        assert_eq!(p.cursor().index(), 0, "at_ must not advance");
        assert!(at_call_after("pre call_foo(bar)", 2));
    }

    #[test]
    fn at_inline_babel_call_word_boundary_pre() {
        // After WHITESPACE (space/tab).
        assert!(at_call_after("pre call_foo(bar)", 2));
        assert!(at_call_after("pre\tcall_foo(bar)", 2));

        // After INDENT (indented continuation / leading indent).
        let p_indent = make_parser("  call_foo(bar)");
        assert_eq!(p_indent.current(), OrgSyntaxKind::INDENT);
        assert!(at_call_after("  call_foo(bar)", 1));

        // After NEWLINE (multiline paragraph; call at line-start of next line).
        assert!(at_call_after("line\ncall_foo(bar)", 2));

        // At content/region start (BOF).
        assert!(at_call("call_foo(bar)"));

        // After R_PAREN without separator: PRE fails unless region start is the call.
        let mut p_glue = make_parser(")call_foo(bar)");
        p_glue.eat_any(); // R_PAREN; cursor on "call"
        assert!(!at_inline_babel_call(&p_glue));
        p_glue.with_inline_region_start(Some(crate::parser::Pos::from_index(1)), |p| {
            assert!(
                at_inline_babel_call(p),
                "at_region_start must accept call at synthetic region start"
            );
        });

        // Glued alphanum still rejected (TEXT is "xcall", not "call").
        assert!(!at_call("xcall_foo(bar)"));
    }

    /// The word-start context is deliberately narrower than Emacs `\<`.
    ///
    /// These cases would match under elisp word-boundary rules after
    /// punctuation, but fail here because only region or line start and
    /// WHITESPACE / INDENT / NEWLINE count as PRE. Widening the set changes
    /// which prose parses as a call, so these pin the current behaviour.
    #[test]
    fn at_inline_babel_call_word_start_known_emacs_false_negatives() {
        // After `(`: Emacs `\<` may treat this as a word start; this parser does not.
        let p_paren = make_parser("(call_x()");
        // L_PAREN TEXT("call") ...
        assert_eq!(p_paren.current(), OrgSyntaxKind::L_PAREN);
        assert!(
            !at_call_after("(call_x()", 1),
            "no PRE after L_PAREN (known gap vs Emacs \\<)"
        );

        // After `:` (e.g. property-ish glue).
        let p_colon = make_parser(":call_x()");
        assert_eq!(p_colon.current(), OrgSyntaxKind::COLON);
        assert!(
            !at_call_after(":call_x()", 1),
            "no PRE after COLON (known gap vs Emacs \\<)"
        );

        // After `]` (closing bracket then call).
        let p_br = make_parser("]call_x()");
        assert_eq!(p_br.current(), OrgSyntaxKind::R_BRACKET);
        assert!(
            !at_call_after("]call_x()", 1),
            "no PRE after R_BRACKET (known gap vs Emacs \\<)"
        );

        // Contrast: a space before the call is a separator and accepts.
        assert!(at_call_after("x call_x()", 2));
    }

    #[test]
    fn name_stop_helpers_direct() {
        assert!(is_element_name_stop(OrgSyntaxKind::L_BRACKET));
        assert!(is_element_name_stop(OrgSyntaxKind::L_PAREN));
        assert!(!is_element_name_stop(OrgSyntaxKind::WHITESPACE));
        assert!(!is_element_name_stop(OrgSyntaxKind::TEXT));

        assert!(is_inline_name_stop(OrgSyntaxKind::WHITESPACE));
        assert!(is_inline_name_stop(OrgSyntaxKind::L_PAREN));
        assert!(!is_inline_name_stop(OrgSyntaxKind::TEXT));
        assert!(!is_inline_name_stop(OrgSyntaxKind::UNDERSCORE));

        // EOF remains a post-run terminator; name scans use run_while / next().
        assert!(
            is_element_name_stop(OrgSyntaxKind::EOF),
            "EOF must act as element name stop for post-run probes"
        );
        assert!(
            is_inline_name_stop(OrgSyntaxKind::EOF),
            "EOF must act as inline name stop for post-run probes"
        );
    }

    // --- Direct measure helpers --------------------------------------------

    #[test]
    fn measure_element_direct_layouts() {
        // empty name + args
        let p = make_parser("()");
        let lay = measure_element_call_value(&p);
        assert_eq!(lay.name_start, lay.name_end);
        assert!(lay.args.is_some());
        assert!(lay.inside.is_none());
        assert!(lay.end.is_none());

        // Rest-of-line end header, not only paired brackets.
        let p2 = make_parser("name() leftover");
        let lay2 = measure_element_call_value(&p2);
        assert!(lay2.args.is_some());
        let (es, ee) = lay2.end.expect("end");
        // The end header starts exactly at the arguments' exclusive end; its
        // text is checked through the emit path in later tests.
        let args_end = lay2.args.unwrap().1;
        assert_eq!(es, args_end);
        assert!(ee >= es);

        // missing args: space before paren folds into end-header
        let p3 = make_parser("name[in] (a)");
        let lay3 = measure_element_call_value(&p3);
        assert!(lay3.inside.is_some());
        assert!(lay3.args.is_none());
        assert!(lay3.end.is_some());

        // empty name with headers
        let p4 = make_parser("[in](a)");
        let lay4 = measure_element_call_value(&p4);
        assert_eq!(lay4.name_start, lay4.name_end);
        assert!(lay4.inside.is_some());
        assert!(lay4.args.is_some());
    }

    #[test]
    fn measure_inline_direct_accept_and_reject() {
        let p = make_parser("call_foo(bar)");
        let lay = measure_inline_after_prefix(&p).expect("accept");
        assert_eq!(lay.name_start, after_call_prefix(&p));
        assert!(lay.args.is_some());
        assert!(lay.inside.is_none());

        // opener matches but arguments are missing
        let p2 = make_parser("call_foo[inside]");
        assert!(at_inline_babel_call(&p2));
        assert!(measure_inline_after_prefix(&p2).is_none());

        // empty name
        let p3 = make_parser("call_(bar)");
        assert!(measure_inline_after_prefix(&p3).is_none());

        // full form with paired end only
        let p4 = make_parser("call_n[i](a)[e]");
        let lay4 = measure_inline_after_prefix(&p4).expect("full");
        assert!(lay4.inside.is_some());
        assert!(lay4.args.is_some());
        assert!(lay4.end.is_some());

        // unclosed end header is omitted (paired-only); still accepted
        let p5 = make_parser("call_n(a)[unclosed");
        let lay5 = measure_inline_after_prefix(&p5).expect("args enough");
        assert!(lay5.end.is_none());
    }

    #[test]
    fn measure_end_header_element_respects_line_cap() {
        let src = "n() leftover\nnext";
        let mut p = make_parser(src);
        let before = p.cursor();
        let cap = p
            .tokens_from_cursor()
            .find(|t| t.kind() == OrgSyntaxKind::NEWLINE)
            .expect("newline")
            .pos()
            .index();
        p.with_limit_at(crate::parser::Pos::from_index(cap), |pp| {
            // After measuring full value under the line cap, end must stop at cap.
            let lay = measure_element_call_value(pp);
            let (es, ee) = lay.end.expect("end under cap");
            let cap_pos = crate::parser::Pos::from_index(cap);
            assert_eq!(ee, cap_pos, "exclusive end stops at the cap");
            // And must not include tokens past the newline.
            assert!(es < cap_pos);
        });
        assert_eq!(
            p.cursor(),
            before,
            "measure under cap must not advance outer cursor"
        );
    }

    #[test]
    fn measure_helpers_are_pure() {
        let p = make_parser("name[in](a) leftover");
        let before = p.cursor();
        let lay = measure_element_call_value(&p);
        assert_eq!(p.cursor(), before, "element measure must not advance");
        assert!(lay.args.is_some());
        assert!(lay.end.is_some());

        let p2 = make_parser("call_foo(bar)[e]");
        let before2 = p2.cursor();
        let lay2 = measure_inline_after_prefix(&p2).expect("inline layout");
        assert_eq!(p2.cursor(), before2, "inline measure must not advance");
        assert!(lay2.args.is_some());
        assert!(lay2.end.is_some());
    }

    // --- Inline happy paths (Worg / org-syntax 5.7 patterns) ---------------

    #[test]
    fn inline_five_patterns_data_driven() {
        // (src, name, inside, args, end)
        let cases: &[InlinePatternRow<'_>] = &[
            ("call_foo(bar,baz)", "foo", None, "(bar,baz)", None),
            (
                "call_foo[inside](bar)",
                "foo",
                Some("[inside]"),
                "(bar)",
                None,
            ),
            ("call_foo(bar)[end]", "foo", None, "(bar)", Some("[end]")),
            (
                "call_foo[inside](a1, a2)[end hdr]",
                "foo",
                Some("[inside]"),
                "(a1, a2)",
                Some("[end hdr]"),
            ),
            ("call_foo()", "foo", None, "()", None),
        ];
        for &(src, name, inside, args, end) in cases {
            let (_toks, evs, _) = results_of_inline(src);
            assert!(
                node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_BABEL_CALL),
                "expected INLINE for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
                Some(name),
                "name for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
                inside,
                "inside for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
                Some(args),
                "args for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
                end,
                "end for {src:?}"
            );
        }
    }

    #[test]
    fn inline_name_allows_hyphen_underscore_dot() {
        let src = "call_foo-bar_baz.1(quux)";
        let (_toks, evs, _) = results_of_inline(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("foo-bar_baz.1")
        );
    }

    // --- Inline rejection and fallback --------------------------------------

    #[test]
    fn inline_missing_parens_not_object() {
        // The opener matches on `[`, but arguments are required, so no
        // INLINE_BABEL_CALL.
        for src in ["call_foo[inside]text", "call_foo", "call_(bar)"] {
            assert!(!parses_inline(src), "no INLINE for {src:?}");
            let out = crate::parse(src);
            assert_eq!(out.root().text().to_string(), src);
            assert!(
                !out.root()
                    .descendants()
                    .any(|n| n.kind() == OrgSyntaxKind::INLINE_BABEL_CALL),
                "full document must not invent INLINE for {src:?}"
            );
        }
    }

    #[test]
    fn inline_fail_path_exact_progress_and_no_orphans() {
        // Opener hits that fail to parse advance exactly past the two-token
        // `call_` opener; non-hits below do not advance at all.
        let cases: &[&str] = &[
            "call_foo[no paren",
            "call_foo[inside]text",
            "call_foo(unclosed",
        ];
        for src in cases {
            let mut p = make_parser(src);
            assert!(at_inline_babel_call(&p), "opener should match for {src:?}");
            let before = p.cursor().index();
            parse_inline_babel_call(&mut p);
            assert_eq!(
                p.cursor().index(),
                before + 2,
                "fail must advance exactly past call_ for {src:?}"
            );
            let (_toks, evs, _) = p.finish();
            assert!(
                !node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_BABEL_CALL),
                "no object for {src:?}"
            );
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_NAME), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_ARGUMENTS), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_INSIDE_HEADER), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_END_HEADER), 0);
        }

        // Non-hit: at_ false; dispatcher must not invent INLINE / CALL_* nodes.
        for src in ["call_foo", "not_a_call(x)", "xcall_foo(bar)"] {
            assert!(
                !at_inline_babel_call(&make_parser(src)),
                "at_ must miss for {src:?}"
            );
            let before = make_parser(src).cursor().index();
            let mut p = make_parser(src);
            // Only parse when at_ holds - non-hit leaves cursor and emits nothing.
            if at_inline_babel_call(&p) {
                parse_inline_babel_call(&mut p);
            }
            assert_eq!(
                p.cursor().index(),
                before,
                "non-hit must not advance for {src:?}"
            );
            let (_toks, evs, _) = p.finish();
            assert!(
                !node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_BABEL_CALL),
                "no INLINE for non-hit {src:?}"
            );
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_NAME), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_ARGUMENTS), 0);
            // Residual source still round-trips under a full document parse.
            let out = crate::parse(src);
            assert_eq!(out.root().text().to_string(), src);
        }
    }

    #[test]
    fn inline_uppercase_prefix_not_recognised() {
        assert!(!parses_inline("CALL_foo(bar)"));
        assert!(!parses_inline("Call_foo(bar)"));
    }

    // --- Element value (direct; keyword shell tested in keyword.rs) --------

    #[test]
    fn element_bare_name() {
        let src = "bare";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("bare")
        );
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_ARGUMENTS), 0);
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_END_HEADER), 0);
    }

    #[test]
    fn element_empty_value_emits_empty_name() {
        let src = "";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_NAME), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("")
        );
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_ARGUMENTS), 0);
    }

    #[test]
    fn element_empty_parens() {
        let src = "()";
        let (_toks, evs, _) = results_of_element_value(src);
        // Empty name + empty args (lossless CST keeps ()).
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("()")
        );
    }

    #[test]
    fn element_name_allows_spaces() {
        // elisp skip-chars-forward "^[]()\n" keeps spaces in the name.
        let src = "foo bar(baz)";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("foo bar")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(baz)")
        );
    }

    #[test]
    fn element_end_header_is_rest_of_line_not_paired_only() {
        // Cases that pass with a rest-of-line end header but would fail a
        // paired-only parse.
        let (_toks, evs, _) = results_of_element_value("name() leftover");
        assert_eq!(
            node_text(&evs, "name() leftover", OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some(" leftover")
        );

        let src2 = "name()[end] more";
        let (_toks2, evs2, _) = results_of_element_value(src2);
        assert_eq!(
            node_text(&evs2, src2, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("[end] more")
        );

        // A well-formed bracket remainder is still rest of line, brackets included.
        let src3 = "n[inside](a1,a2)[end hdr]";
        let (_toks3, evs3, _) = results_of_element_value(src3);
        assert_eq!(
            node_text(&evs3, src3, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("[end hdr]")
        );
    }

    #[test]
    fn element_rest_of_line_after_name_without_args() {
        let src = "name leftover text";
        let (_toks, evs, _) = results_of_element_value(src);
        // No []() => whole remainder is the name (spaces allowed).
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("name leftover text")
        );
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_END_HEADER), 0);
    }

    #[test]
    fn element_end_header_after_inside_without_args() {
        let src = "name[in] leftover";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[in]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some(" leftover")
        );
    }

    #[test]
    fn element_unclosed_pairs_fold_into_end_header() {
        let src = "name[unclosed";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("name")
        );
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_INSIDE_HEADER), 0);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("[unclosed")
        );

        let src2 = "name(unclosed";
        let (_toks2, evs2, _) = results_of_element_value(src2);
        assert_eq!(node_count(&evs2, OrgSyntaxKind::CALL_ARGUMENTS), 0);
        assert_eq!(
            node_text(&evs2, src2, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("(unclosed")
        );
    }

    #[test]
    fn element_nested_same_delimiter_pairs() {
        let src = "name[[x]](a(b))";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[[x]]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(a(b))")
        );
    }

    #[test]
    fn element_space_between_parts_is_end_header_not_args() {
        // After the inside header, a space then `(` is not contiguous
        // arguments; the end header takes the rest.
        let src = "name[in] (a)";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[in]")
        );
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_ARGUMENTS), 0);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some(" (a)")
        );
    }

    #[test]
    fn element_empty_name_with_headers() {
        let src = "[in](a)";
        let (_toks, evs, _) = results_of_element_value(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[in]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(a)")
        );
    }

    #[test]
    fn element_five_worg_patterns() {
        // (value, name, inside, args, end)
        let cases: &[PatternRow<'_>] = &[
            ("name()", "name", None, Some("()"), None),
            ("name[inside]()", "name", Some("[inside]"), Some("()"), None),
            ("name(args)", "name", None, Some("(args)"), None),
            (
                "name[inside](args)",
                "name",
                Some("[inside]"),
                Some("(args)"),
                None,
            ),
            (
                "name[inside](args)[end]",
                "name",
                Some("[inside]"),
                Some("(args)"),
                Some("[end]"),
            ),
        ];
        for &(src, name, inside, args, end) in cases {
            let (_toks, evs, _) = results_of_element_value(src);
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
                Some(name),
                "name for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
                inside,
                "inside for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
                args,
                "args for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
                end,
                "end for {src:?}"
            );
        }
    }

    // --- Full document smoke / fallthrough ---------------------------------

    #[test]
    fn element_babel_call_in_document_tree() {
        let src = "#+CALL: foo(bar)\n#+call: n[i](a)[e]\npara call_x(y) here\n";
        let root = parse_document_tree(src);
        let babels = nodes_of_kind(&root, OrgSyntaxKind::BABEL_CALL);
        assert_eq!(babels.len(), 2);
        let inlines = nodes_of_kind(&root, OrgSyntaxKind::INLINE_BABEL_CALL);
        assert_eq!(inlines.len(), 1);
    }

    #[test]
    fn element_caseless_key_in_document() {
        let src = "#+CaLl: myfunc()\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::BABEL_CALL).len(), 1);
    }

    #[test]
    fn inline_document_fail_soft_and_happy_contrast() {
        // Incomplete opener hits must tile losslessly with no INLINE / CALL_* nodes.
        let incomplete = "before call_foo[x] after\n";
        let root = parse_document_tree(incomplete);
        assert_eq!(tree_text(&root), incomplete);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_BABEL_CALL).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_BABEL_CALL)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::CALL_NAME).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::CALL_NAME)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::CALL_ARGUMENTS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::CALL_ARGUMENTS)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::CALL_INSIDE_HEADER).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::CALL_INSIDE_HEADER)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::CALL_END_HEADER).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::CALL_END_HEADER)
        );

        // Happy path with trailing prose after a successful call.
        let happy = "para call_x(y) z\n";
        let root2 = parse_document_tree(happy);
        assert_eq!(tree_text(&root2), happy);
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::INLINE_BABEL_CALL).len(),
            1
        );

        // An opener hit via `(` with unclosed arguments also falls back in a document.
        let unclosed = "x call_n(y z\n";
        let root3 = parse_document_tree(unclosed);
        assert_eq!(tree_text(&root3), unclosed);
        assert!(
            nodes_of_kind(&root3, OrgSyntaxKind::INLINE_BABEL_CALL).is_empty(),
            "{:?}",
            nodes_of_kind(&root3, OrgSyntaxKind::INLINE_BABEL_CALL)
        );
        assert!(
            nodes_of_kind(&root3, OrgSyntaxKind::CALL_NAME).is_empty(),
            "{:?}",
            nodes_of_kind(&root3, OrgSyntaxKind::CALL_NAME)
        );
    }
}
