//! Emphasis and verbatim markup: `*bold*`, `/italic/`, `_underline_`,
//! `+strike-through+`, `~code~`, `=verbatim=`.
//!
//! Rules from `org-emphasis-regexp-components` and
//! `org-element--parse-generic-emphasis`.
//!
//! Emits (on success):
//!   BOLD | ITALIC | UNDERLINE | STRIKE_THROUGH | CODE | VERBATIM
//!     (recursive kinds: nested inline under the child scope)
//!     (code/verbatim: opaque interior tokens)
//!
//! Recognition and emit:
//! - [`probe`] validates PRE, the opening border, the closer and its POST, and
//!   carries the closer position. It is pure.
//! - [`parse`] consumes the measured span with no rescan and no fail path.
//!
//! Recognition lives entirely in the probe because the `_` branch of
//! `org-element--object-lex` needs it: underline is tried first and, when it
//! does not form, subscript is tried at the same position (`forms/script.rs`).
//! A `parse` that consumed the opener before deciding would swallow the `_`.
//! A rejected opener becomes a bare token through the dispatcher's `eat_any`.
//!
//! PRE/POST are plain predicates over cursor bytes and token views; region
//! start is parser state set by `parse_inline` ([`Parser::at_region_start`]).

use crate::parser::grammar::drive::region;
use crate::parser::{Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

/// Max NEWLINE tokens inside a markup span (Org default via emphasis components).
const MAX_MARKUP_NEWLINES: usize = 1;

/// Org PRE byte set (`org-emphasis-regexp-components`), ASCII only.
/// Line start / region start are separate context checks, not bytes here.
const MARKUP_PRE_BYTES: &[u8] = b" \t-({'\"";

/// Org POST byte set (`org-emphasis-regexp-components`), ASCII only.
/// Line end / region end are the `None | \n | \r` arms of [`post_ok_at`].
/// The writer reads this set too, so the two cannot disagree on POST.
pub(crate) const MARKUP_POST_BYTES: &[u8] = b" \t-.,;:!?')}[\\\"";

// ---------------------------------------------------------------------------
// Public surface for the inlines dispatcher
// ---------------------------------------------------------------------------

/// Measured markup span: delimiter, node kind, and the validated closer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarkupShape {
    /// Opener delimiter kind (`STAR`, `SLASH`, ...).
    pub(crate) marker: OrgSyntaxKind,
    /// Node kind to emit (`BOLD`, `CODE`, ...).
    pub(crate) node_kind: OrgSyntaxKind,
    /// Recursive forms nest inline; code/verbatim interiors are opaque.
    pub(crate) is_recursive: bool,
    /// Position of the closing delimiter (PRE/border/POST already checked).
    pub(crate) closer: Pos,
}

/// Map a markup opener kind to its node kind and nesting behaviour.
///
/// Recursive forms nest inline under the child scope; code/verbatim are
/// opaque.
#[inline]
fn markup_kind(k: OrgSyntaxKind) -> Option<(OrgSyntaxKind, bool)> {
    match k {
        OrgSyntaxKind::STAR => Some((OrgSyntaxKind::BOLD, true)),
        OrgSyntaxKind::SLASH => Some((OrgSyntaxKind::ITALIC, true)),
        OrgSyntaxKind::UNDERSCORE => Some((OrgSyntaxKind::UNDERLINE, true)),
        OrgSyntaxKind::PLUS => Some((OrgSyntaxKind::STRIKE_THROUGH, true)),
        OrgSyntaxKind::TILDE => Some((OrgSyntaxKind::CODE, false)),
        OrgSyntaxKind::EQUALS => Some((OrgSyntaxKind::VERBATIM, false)),
        _ => None,
    }
}

/// Pure probe: opener kind, PRE, opening border, and a closer with legal POST.
#[inline]
pub(crate) fn probe(parser: &Parser) -> Option<MarkupShape> {
    let marker = parser.current();
    let (node_kind, is_recursive) = markup_kind(marker)?;
    if !pre_ok_here(parser) {
        return None;
    }
    // Opening border: must not open onto whitespace or a line/region end.
    // `kind_at` answers `EOF` past the cap, which `is_line_end_or_space` treats
    // as a terminator.
    let open = parser.here()?;
    if open.kind_at(1).is_line_end_or_space() {
        return None;
    }
    let closer = find_closer(open, marker)?;
    Some(MarkupShape {
        marker,
        node_kind,
        is_recursive,
        closer: closer.pos(),
    })
}

/// Emit one emphasis/verbatim form from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: MarkupShape) {
    // Shape marker must match the cursor (probe contract). Debug-only: release
    // still consumes the measured span (grammar-contract invariant the
    // dispatcher always establishes).
    debug_assert_eq!(
        parser.current(),
        shape.marker,
        "markup shape marker must match cursor"
    );

    let m = parser.start();
    parser.eat_any(); // opening marker

    if shape.is_recursive {
        // Content between the delimiters is itself inline content.
        region::inline_in(parser, shape.closer, shape.node_kind);
    } else {
        // Opaque (code/verbatim): eat the interior tokens as-is.
        parser.eat_until(shape.closer);
    }

    debug_assert_eq!(parser.current(), shape.marker);
    parser.eat_any(); // closing marker

    m.complete(parser, shape.node_kind);
}

/// Harness: probe then parse (unit tests / local walks).
#[cfg(test)]
pub(crate) fn parse_markup(parser: &mut Parser) {
    if let Some(shape) = probe(parser) {
        parse(parser, shape);
    }
}

// ---------------------------------------------------------------------------
// PRE / POST / closer (org-emphasis-regexp-components)
// ---------------------------------------------------------------------------

/// Markup opener PRE at the cursor: region/line start, or last preceding byte
/// in the Org PRE set.
///
/// The byte arm covers WHITESPACE / INDENT spaces, TEXT-ending PRE chars, and
/// single-byte delimiter tokens such as `(` / `{`. Region-start accepts
/// leading markup in a sub-view even when an outer delimiter precedes it.
#[inline]
fn pre_ok_here(parser: &Parser) -> bool {
    parser.at_line_start()
        || parser.at_region_start()
        || parser
            .prev_byte()
            .is_some_and(|b| MARKUP_PRE_BYTES.contains(&b))
}

/// Markup closer POST at a token: region/source end, line end, or an Org POST
/// byte.
///
/// `next_byte` is limit-safe: `None` at an active cap counts as region end
/// even when outer source bytes remain.
#[inline]
fn post_ok_at(closer: TokenView) -> bool {
    match closer.next_byte() {
        None => true,
        Some(b) => b == b'\n' || b == b'\r' || MARKUP_POST_BYTES.contains(&b),
    }
}

/// Search forward from the opener for a valid closer token.
///
/// `open` is the view at the opening delimiter, so the scan starts two tiles
/// later: the immediate next tile cannot close (empty interior is rejected).
/// A closer is the same marker kind, not preceded by narrow WHITESPACE, and
/// with valid POST context. Stops at the newline budget (one interior NEWLINE)
/// or the region end (`nth` is cap-bounded, so the walk cannot run away).
fn find_closer<'a, 's>(
    open: TokenView<'a, 's>,
    marker: OrgSyntaxKind,
) -> Option<TokenView<'a, 's>> {
    let mut newlines = 0usize;
    let mut k = 2usize;
    while let Some(t) = open.nth(k) {
        match t.kind() {
            OrgSyntaxKind::NEWLINE => {
                newlines += 1;
                if newlines > MAX_MARKUP_NEWLINES {
                    return None;
                }
            }
            found if found == marker && !t.after_space() => {
                // Closing border: marker not preceded by narrow WHITESPACE;
                // POST byte context decides acceptance.
                return post_ok_at(t).then_some(t);
            }
            _ => {}
        }
        k += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{ParseResult, make_parser, node_kinds, node_text, run_inline, token_count};

    /// Run the markup parser over the full token span of `src`.
    fn results_of(src: &str) -> ParseResult {
        run_inline(src, parse_markup)
    }

    /// The six emphasis forms: marker byte, node kind, and whether the interior
    /// nests inline objects (recursive) or stays opaque (code / verbatim).
    ///
    /// Border and PRE/POST rules are marker-independent, so the tables below
    /// assert them across every marker rather than only for `*`.
    const MARKUP_FORMS: &[(char, OrgSyntaxKind, bool)] = &[
        ('*', OrgSyntaxKind::BOLD, true),
        ('/', OrgSyntaxKind::ITALIC, true),
        ('_', OrgSyntaxKind::UNDERLINE, true),
        ('+', OrgSyntaxKind::STRIKE_THROUGH, true),
        ('~', OrgSyntaxKind::CODE, false),
        ('=', OrgSyntaxKind::VERBATIM, false),
    ];

    // --- PRE/POST byte sets and boundary presets ---------------------------

    #[test]
    fn pre_post_byte_membership() {
        // Set coverage against org-emphasis-regexp-components.
        for b in *b" \t-({'\"" {
            assert!(super::MARKUP_PRE_BYTES.contains(&b), "pre {b:?}");
        }
        assert!(!super::MARKUP_PRE_BYTES.contains(&b'x'));
        assert!(!super::MARKUP_PRE_BYTES.contains(&b'a'));
        assert!(!super::MARKUP_PRE_BYTES.contains(&b'\n'));

        for b in *b" \t-.,;:!?')}[\\\"" {
            assert!(super::MARKUP_POST_BYTES.contains(&b), "post {b:?}");
        }
        assert!(!super::MARKUP_POST_BYTES.contains(&b'x'));
        assert!(!super::MARKUP_POST_BYTES.contains(&b'a'));
        assert!(!super::MARKUP_POST_BYTES.contains(&b'\n'));
    }

    #[test]
    fn post_ok_at_accepts_region_end_under_limit() {
        // Closer POST must accept an active cap as region end even when outer
        // source bytes remain (limit-safe next_byte).
        let mut p = make_parser("*b*x");
        // Tokens: STAR TEXT STAR TEXT("x"); cap before trailing "x".
        let cap = p
            .token_view(p.cursor())
            .and_then(|t| t.nth(3))
            .expect("x")
            .pos();
        p.with_limit_at(cap, |pp| {
            let closer = pp
                .token_view(pp.cursor())
                .and_then(|t| t.nth(2))
                .expect("closer");
            assert_eq!(closer.kind(), OrgSyntaxKind::STAR);
            assert_eq!(closer.next_byte(), None, "cap hides the outer byte");
            assert!(super::post_ok_at(closer));
        });
        // Without the cap the outer letter is visible POST and rejects.
        let p_open = make_parser("*b*x");
        let closer = p_open
            .token_view(p_open.cursor())
            .and_then(|t| t.nth(2))
            .expect("closer");
        assert!(!super::post_ok_at(closer));
    }

    #[test]
    fn pre_and_post_predicates_via_parser() {
        // BOF / line start (no active region required).
        let p = make_parser("*b*");
        assert!(super::pre_ok_here(&p));
        // Closer at final STAR: EOF after it is a valid POST.
        let closer = p
            .token_view(p.cursor())
            .and_then(|t| t.nth(2))
            .expect("closer");
        assert!(super::post_ok_at(closer));

        // After L_PAREN (PRE byte).
        let mut p2 = make_parser("a (*b*)");
        p2.eat_any(); // TEXT "a"
        p2.eat_any(); // WHITESPACE
        p2.eat_any(); // L_PAREN
        assert_eq!(p2.current(), OrgSyntaxKind::STAR);
        assert!(super::pre_ok_here(&p2));

        // After L_BRACE (also in MARKUP_PRE_BYTES).
        let mut p_brace = make_parser("a {*b*}");
        p_brace.eat_any(); // TEXT
        p_brace.eat_any(); // WHITESPACE
        p_brace.eat_any(); // L_BRACE
        assert_eq!(p_brace.current(), OrgSyntaxKind::STAR);
        assert!(super::pre_ok_here(&p_brace));
        parse_markup(&mut p_brace);
        let (_, ev_brace, _) = p_brace.finish();
        assert!(
            node_kinds(&ev_brace).contains(&OrgSyntaxKind::BOLD),
            "L_BRACE is Org PRE"
        );

        // Letter before: PRE fails when not region start.
        let mut p3 = make_parser("x*b*");
        p3.eat_any();
        assert!(!super::pre_ok_here(&p3));
        p3.with_inline_region_start(Some(crate::parser::Pos::from_index(1)), |pp| {
            assert!(super::pre_ok_here(pp));
        });

        // POST rejects letter after closer.
        let p4 = make_parser("*b*x");
        let closer4 = p4
            .token_view(p4.cursor())
            .and_then(|t| t.nth(2))
            .expect("closer");
        assert!(!super::post_ok_at(closer4));
    }

    #[test]
    fn markup_pre_accepts_indent_via_pre_bytes() {
        // Leading INDENT ends with space/tab; Org PRE includes those bytes.
        let mut p = make_parser("  *b*");
        p.eat_any(); // INDENT
        assert_eq!(p.current(), OrgSyntaxKind::STAR);
        assert!(super::pre_ok_here(&p));
        parse_markup(&mut p);
        let (_, ev, _) = p.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::BOLD));
    }

    #[test]
    fn pre_accepts_non_ws_pre_chars() {
        // Leading PRE char before opener: '-', '\'', '"'. Cursor on '*';
        // region start stays 0 so the region-start bypass does not apply.
        for src in ["-*b*", "'*b*", "\"*b*\""] {
            let mut p = make_parser(src);
            p.with_inline_region_start(Some(crate::parser::Pos::from_index(0)), |pp| {
                pp.eat_any(); // skip PRE char TEXT
                parse_markup(pp);
            });
            let (_, ev, _) = p.finish();
            assert!(
                node_kinds(&ev).contains(&OrgSyntaxKind::BOLD),
                "expected bold in {src:?}"
            );
        }
    }

    #[test]
    fn pre_rejects_mid_stream_letter() {
        // Letter before opener is not PRE. Cursor on '*'; region start stays 0.
        let mut p = make_parser("x*b*");
        p.with_inline_region_start(Some(crate::parser::Pos::from_index(0)), |pp| {
            pp.eat_any(); // skip 'x'
            parse_markup(pp);
        });
        let (_, ev, _) = p.finish();
        assert!(!node_kinds(&ev).contains(&OrgSyntaxKind::BOLD));
    }

    // --- Recognition and boundaries across every marker ---------------------

    /// Body / POST shapes that must form the emphasis node, for every marker.
    ///
    /// `{m}` is the marker byte; the body avoids marker bytes so one template
    /// is valid for all six forms.
    const ACCEPTED: &[&str] = &[
        "{m}hello{m}",         // plain body
        "{m}b{m} ",            // POST space
        "{m}b{m}\nmore",       // POST line end
        "{m}b{m}.",            // POST punctuation
        "{m}b{m},",            //
        "{m}b{m})",            //
        "{m}first\nsecond{m}", // one interior newline is allowed
    ];

    /// Border / POST violations that must NOT form the node, for every marker.
    const REJECTED: &[&str] = &[
        "{m} hello{m}",  // space after opener
        "{m}hello {m}",  // space before closer
        "{m}hello\t{m}", // tab before closer (narrow whitespace)
        "{m}{m}",        // empty body
        "{m}b{m}x",      // POST letter after closer
        "{m}a\n\nb{m}",  // two interior newlines exceed the limit
        "{m}no close here",
    ];

    fn expand(template: &str, marker: char) -> String {
        template.replace("{m}", &marker.to_string())
    }

    #[test]
    fn every_marker_forms_its_node_on_accepted_shapes() {
        for &(marker, kind, _) in MARKUP_FORMS {
            for template in ACCEPTED {
                let src = expand(template, marker);
                let (_, ev, _) = results_of(&src);
                assert!(
                    node_kinds(&ev).contains(&kind),
                    "expected {kind:?} for {src:?}"
                );
            }
        }
    }

    #[test]
    fn every_marker_rejects_border_and_post_violations() {
        for &(marker, kind, _) in MARKUP_FORMS {
            for template in REJECTED {
                let src = expand(template, marker);
                let (_, ev, _) = results_of(&src);
                assert!(
                    !node_kinds(&ev).contains(&kind),
                    "expected no {kind:?} for {src:?}"
                );
            }
        }
    }

    /// A declined probe leaves plain text: every rejected template produces no
    /// node, keeps its bytes, and lets the dispatcher advance.
    #[test]
    fn rejected_templates_round_trip_as_plain_text() {
        for &(marker, kind, _) in MARKUP_FORMS {
            for template in REJECTED {
                let src = format!("{}\n", expand(template, marker));
                let root = crate::tests::parse_document_tree(&src);
                assert_eq!(crate::tests::tree_text(&root), src, "roundtrip {src:?}");
                assert!(
                    crate::tests::nodes_of_kind(&root, kind).is_empty(),
                    "expected no {kind:?} for {src:?}"
                );
            }
        }
    }

    #[test]
    fn accepted_span_is_the_whole_emphasis_including_delimiters() {
        // Ranges include delimiters: one node, both markers consumed.
        for &(marker, kind, _) in MARKUP_FORMS {
            let src = expand("{m}hello{m}", marker);
            let (_, ev, _) = results_of(&src);
            assert_eq!(
                node_kinds(&ev),
                vec![kind],
                "exactly one top node for {src:?}"
            );
            assert_eq!(
                node_text(&ev, &src, kind).as_deref(),
                Some(src.as_str()),
                "{kind:?} must span its delimiters"
            );
        }
    }

    #[test]
    fn opaque_markers_keep_inner_markup_literal() {
        // Code / verbatim interiors do not nest; recursive forms do (below).
        for &(marker, kind, is_recursive) in MARKUP_FORMS {
            if is_recursive {
                continue;
            }
            let src = expand("{m}*not-bold* /not-italic/{m}", marker);
            let (_, ev, _) = results_of(&src);
            let kinds = node_kinds(&ev);
            assert!(kinds.contains(&kind), "expected {kind:?} for {src:?}");
            assert!(
                !kinds.contains(&OrgSyntaxKind::BOLD),
                "{kind:?} interior must stay literal: {src:?}"
            );
            assert!(
                !kinds.contains(&OrgSyntaxKind::ITALIC),
                "{kind:?} interior must stay literal: {src:?}"
            );
        }
    }

    #[test]
    fn recursive_markers_nest_italic() {
        for &(marker, kind, is_recursive) in MARKUP_FORMS {
            if !is_recursive || marker == '/' {
                continue; // '/' would collide with the nested italic marker
            }
            let src = expand("{m}outer /inner/ outer{m}", marker);
            let (_, ev, _) = results_of(&src);
            let kinds = node_kinds(&ev);
            assert!(kinds.contains(&kind), "expected {kind:?} for {src:?}");
            assert!(
                kinds.contains(&OrgSyntaxKind::ITALIC),
                "{kind:?} must nest inline objects: {src:?}"
            );
        }
    }

    #[test]
    fn bold_opener_and_closer_are_both_star_tokens() {
        // Delimiter token accounting for the representative marker.
        let (_, ev, _) = results_of("*hello*");
        assert_eq!(token_count(&ev, OrgSyntaxKind::STAR), 2);
    }

    // --- Failure to form markup: the probe declines, the opener stays bare ---

    #[test]
    fn invalid_markup_probe_declines_without_consuming() {
        // PRE / border / closer / POST failures are decided in the probe,
        // which leaves the cursor untouched. The
        // dispatcher then emits the opener as a bare token via `eat_any`.
        let src = "* no close here";
        let mut p = make_parser(src);
        let start_pos = p.cursor().index();
        let before = p.checkpoint();
        p.with_inline_region_start(Some(crate::parser::Pos::from_index(start_pos)), |pp| {
            assert!(super::probe(pp).is_none(), "no closer: probe must decline");
            parse_markup(pp);
        });
        assert_eq!(p.cursor().index(), start_pos, "declined probe is pure");
        assert_eq!(p.checkpoint().events_len, before.events_len);
        assert!(!node_kinds(&p.finish().1).contains(&OrgSyntaxKind::BOLD));

        // Through the document path the delimiter survives as plain text.
        let doc = "a * no close here\n";
        let root = crate::tests::parse_document_tree(doc);
        assert_eq!(crate::tests::tree_text(&root), doc);
        assert!(
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::BOLD)
        );
    }

    #[test]
    fn probe_carries_the_closer_position() {
        // The closer travels on the shape, so emit never rescans. `*hello*`
        // is STAR TEXT STAR.
        let p = make_parser("*hello*");
        let shape = super::probe(&p).expect("bold");
        assert_eq!(shape.marker, OrgSyntaxKind::STAR);
        assert_eq!(shape.node_kind, OrgSyntaxKind::BOLD);
        assert!(shape.is_recursive);
        assert_eq!(shape.closer, crate::tests::pos_ahead(&p, 2));
        assert_eq!(
            p.cursor(),
            crate::parser::Pos::from_index(0),
            "probe is pure"
        );
    }

    // --- Region-start special case (synthetic) ---

    #[test]
    fn region_start_makes_pre_ok_even_for_non_pre_byte() {
        // Use a preceding TEXT whose last byte is not a PRE char (e.g. 'x').
        // Start a limited view at the '*' (not file line-start).
        // Without at_region_start at the region index, PRE would fail.
        let src = "x*b*";
        let mut p = make_parser(src);
        // Eat the leading "x" (TEXT) so we are at the region start token '*'.
        p.eat_any();
        let region_start = p.cursor();
        // Cap at natural end of the remaining token stream.
        let cap = p.limit();
        p.with_limit_at(cap, |pp| {
            pp.with_inline_region_start(Some(region_start), |ppp| {
                parse_markup(ppp);
            });
        });
        let (_, ev, _) = p.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::BOLD));
    }

    #[test]
    fn tight_nesting_at_region_start() {
        // "*/i/*" : outer BOLD region starts at first *, inner ITALIC
        // starts immediately after (region start for inner inline).
        // Requires region-start PRE for the inner '/'.
        let src = "*/i/*";
        let (_, ev, _) = results_of(src);
        let kinds = node_kinds(&ev);
        assert!(kinds.contains(&OrgSyntaxKind::BOLD));
        assert!(kinds.contains(&OrgSyntaxKind::ITALIC));
    }
}
