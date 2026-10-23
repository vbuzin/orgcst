//! Shared pure atoms and marker-line helpers used by element/object grammars.

use crate::parser::{LineView, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

/// Shared marker-suffix check: the token's next visible byte is a literal
/// space, a line end, or the region/source end.
///
/// Org's ` \|$` after element markers (`#`, fixed-width `:`, headline
/// `COMMENT`). Tab does not qualify.
#[inline]
pub(crate) fn space_or_line_end(t: TokenView) -> bool {
    matches!(t.next_byte(), None | Some(b' ' | b'\n' | b'\r'))
}

/// Word-start context at the cursor for inline objects, approximating Emacs
/// `\<`: line start, inline region start, or a separator token (WHITESPACE /
/// INDENT / NEWLINE) before the cursor.
///
/// Not full syntax-table word chars: punctuation such as `(` / `:` before
/// `call_` is a known false negative.
#[inline]
pub(crate) fn at_word_start(parser: &Parser) -> bool {
    parser.at_line_start() || parser.at_region_start() || parser.after_separator()
}

/// Fixed opener: TEXT(literal) + UNDERSCORE at the cursor.
///
/// Returns the absolute position of the first name token (third token), or
/// the visible limit when no third token exists. Case-sensitive equality on
/// the TEXT tile. Inline-object use only (`call_` / `src_`); not used by the
/// `#+call:` element path. Line-bounded pairs use
/// [`Parser::measure_paired_at`](crate::parser::Parser::measure_paired_at).
#[inline]
pub(crate) fn fixed_underscore_prefix(parser: &Parser, literal: &str) -> Option<Pos> {
    let t0 = parser.here()?;
    if t0.kind() != OrgSyntaxKind::TEXT || t0.text() != literal {
        return None;
    }
    if t0.kind_at(1) != OrgSyntaxKind::UNDERSCORE {
        return None;
    }
    Some(t0.nth(2).map_or_else(|| parser.limit(), |t| t.pos()))
}

/// Walk name tokens from `start` until `is_stop` returns true.
///
/// Pure: `TokenView` / absolute `Pos` only; does not move the parser cursor.
/// Returns `(name_start, name_end, first_token_after_name)` where
/// `name_end` is exclusive (the stop token's position, or the limit).
pub(crate) fn measure_name<'a, 's>(
    parser: &'a Parser<'s>,
    start: Pos,
    is_stop: impl Fn(OrgSyntaxKind) -> bool,
) -> (Pos, Pos, Option<TokenView<'a, 's>>) {
    let mut cur = parser.token_view(start);
    while let Some(t) = cur {
        if is_stop(t.kind()) {
            break;
        }
        cur = t.next();
    }
    let name_end = cur.map_or_else(|| parser.limit(), |t| t.pos());
    (start, name_end, cur)
}

/// True when `s` is non-empty and every byte is an ASCII digit.
///
/// Shared by statistics cookies, ordered list bullets/counters, clock
/// duration, and timestamp wall-clock pieces.
#[inline]
pub(crate) fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// True when `s` is a legal export-snippet back-end name (`[-A-Za-z0-9]+`).
///
/// Org's `org-element-export-snippet-parser` matches `@@\([-A-Za-z0-9]+\):`.
/// Every byte of that class is TEXT-class, so the back-end is exactly one
/// `TEXT` tile and the whole tile must pass: `@@a.b:` is not a snippet in org
/// either (the `+` can shrink, but `:` must follow immediately).
#[inline]
pub(crate) fn is_export_backend(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Drawer / footnote label atom: unicode alphanumerics and `-` in TEXT, or `_`.
///
/// Shared character class for names that are word constituents plus hyphen
/// and underscore (drawer names, footnote labels). Tag atoms use a wider
/// punct set and stay in heading.
#[inline]
pub(crate) fn is_name_atom(kind: OrgSyntaxKind, text: &str) -> bool {
    match kind {
        OrgSyntaxKind::UNDERSCORE => true,
        OrgSyntaxKind::TEXT => {
            !text.is_empty() && text.chars().all(|c| c.is_alphanumeric() || c == '-')
        }
        _ => false,
    }
}

/// Content index of the first token after `#+` when the line starts with
/// `HASH` `PLUS`.
///
/// Shared prefix for keyword / block / TBLFM probes. Returns `Some(2)`
/// (content-relative key start) on match, else `None`.
///
/// Does **not** require the key start to be `TEXT`: keyword keys may begin
/// with any non-terminator tile (`UNDERSCORE`, `PLUS`, `STAR`, ...). Callers
/// that need a word key (`begin` / `end` / `tblfm`) check the start tile
/// themselves (`text_eq_caseless` fails on non-TEXT).
#[inline]
pub(crate) fn line_keyword_prefix(line: LineView<'_, '_>) -> Option<usize> {
    use OrgSyntaxKind::{HASH, PLUS};
    line.seq(&[HASH, PLUS]).then_some(2)
}

/// One marker-line form: content starts with `marker` and the next visible
/// byte is space / line end / region end ([`space_or_line_end`]).
///
/// Used by comment (`#`) and fixed-width (`:`) probes.
#[inline]
pub(crate) fn marker_line_probe(line: LineView, marker: OrgSyntaxKind) -> Option<()> {
    let t = line.content()?;
    (t.kind() == marker && space_or_line_end(t)).then_some(())
}

/// Consume one or more consecutive marker lines into `kind`, then post-blank.
///
/// Precondition: the cursor is at a line where [`marker_line_probe`] succeeds
/// for `marker` (dispatcher / element contract).
pub(crate) fn parse_marker_run(parser: &mut Parser, marker: OrgSyntaxKind, kind: OrgSyntaxKind) {
    debug_assert!(
        parser
            .line()
            .is_some_and(|l| marker_line_probe(l, marker).is_some()),
        "parse_marker_run called without probe (dispatcher contract)"
    );
    let m = parser.start();
    while parser
        .line()
        .is_some_and(|l| marker_line_probe(l, marker).is_some())
    {
        parser.eat_line();
    }
    parser.eat_blank_lines();
    m.complete(parser, kind);
}

#[cfg(test)]
mod tests {
    use crate::parser::grammar::forms;
    use crate::parser::{LineView, Parser};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{make_parser, node_count, node_text, probe_line, run_unit_element};

    /// One marker-line form built on [`marker_line_probe`] /
    /// [`parse_marker_run`]: marker byte, node kind, and the form's probe /
    /// parse entry points (each a two-line delegation to the atoms above).
    type MarkerForm = (
        char,
        OrgSyntaxKind,
        fn(LineView<'_, '_>) -> Option<()>,
        fn(&mut Parser),
    );

    /// The two marker-line forms in the grammar.
    ///
    /// Behaviour they share is tested here once; the comment and fixed-width
    /// modules test only what is specific to their marker.
    const MARKER_FORMS: &[MarkerForm] = &[
        (
            '#',
            OrgSyntaxKind::COMMENT,
            forms::comment::probe,
            forms::comment::parse,
        ),
        (
            ':',
            OrgSyntaxKind::FIXED_WIDTH,
            forms::fixed_width::probe,
            forms::fixed_width::parse,
        ),
    ];

    #[test]
    fn marker_line_probe_accepts_space_line_end_and_indent() {
        for &(c, kind, probe, _) in MARKER_FORMS {
            for src in [
                format!("{c} content\n"),  // marker then space
                format!("{c}\n"),          // marker then line end
                format!("{c}"),            // marker at true EOF
                format!("{c}   spaced\n"), // a run of spaces is still ` \|$`
                format!("   {c} indented\n"),
                format!("\t{c} tab indented\n"), // tab in *indent* is fine
                format!("  {c}\n"),
            ] {
                assert!(
                    probe_line(&src, probe).is_some(),
                    "{kind:?} must claim {src:?}"
                );
            }
        }
    }

    #[test]
    fn marker_line_probe_rejects_tab_suffix_and_non_marker_lines() {
        for &(c, kind, probe, _) in MARKER_FORMS {
            for src in [
                format!("{c}\tnot a marker line\n"), // tab after marker is not ` \|$`
                format!("{c}glued\n"),
                "plain text\n".to_string(),
                "* headline\n".to_string(),
            ] {
                assert!(
                    probe_line(&src, probe).is_none(),
                    "{kind:?} must reject {src:?}"
                );
            }
        }
    }

    #[test]
    fn parse_marker_run_folds_consecutive_lines_and_stops() {
        for &(c, kind, probe, parse) in MARKER_FORMS {
            // Consecutive lines fold into exactly one node spanning both.
            let src = format!("{c} first\n{c} second\n");
            let (_toks, evs, _) = run_unit_element(&src, probe, parse);
            assert_eq!(
                node_count(&evs, kind),
                1,
                "{kind:?} folds consecutive lines"
            );
            assert_eq!(
                node_text(&evs, &src, kind).as_deref(),
                Some(src.as_str()),
                "{kind:?} spans every folded line"
            );

            // The run stops at the first non-marker line, cursor past its newline.
            let trailing = format!("{c} a\n{c} b\nx\n");
            let mut p = make_parser(&trailing);
            assert!(p.line().and_then(probe).is_some());
            parse(&mut p);
            assert_eq!(
                p.current(),
                OrgSyntaxKind::TEXT,
                "{kind:?} run stops on the non-marker line"
            );
        }
    }

    #[test]
    fn is_digits_table() {
        for s in ["0", "1", "42", "007", "999"] {
            assert!(super::is_digits(s), "expected digits: {s:?}");
        }
        for s in ["", " ", "1a", "a1", "1.0", "-1", "1 2", "\u{0661}"] {
            assert!(!super::is_digits(s), "expected non-digits: {s:?}");
        }
    }

    #[test]
    fn line_keyword_prefix_happy_and_reject() {
        use super::line_keyword_prefix;

        // HASH PLUS only - key start need not be TEXT.
        for src in ["#+TITLE: x\n", "#+_FOO: x\n", "#++BAR: x\n", "#+*STAR: x\n"] {
            let p = make_parser(src);
            let line = p.line().expect(src);
            assert_eq!(line_keyword_prefix(line), Some(2), "{src}");
        }
        // Reject non-prefix lines.
        for src in ["#TITLE: x\n", "+TITLE: x\n", "TITLE: x\n", "# +TITLE: x\n"] {
            let p = make_parser(src);
            let line = p.line().expect(src);
            assert!(line_keyword_prefix(line).is_none(), "{src}");
        }
        // Indent before #+ still works (content-relative).
        let p = make_parser("  #+NAME: x\n");
        assert_eq!(line_keyword_prefix(p.line().expect("line")), Some(2));
    }

    // --- TEXT-prefix opener + name walk (call_ / src_) ---------------------

    #[inline]
    fn is_src_language_stop(kind: OrgSyntaxKind) -> bool {
        matches!(
            kind,
            OrgSyntaxKind::WHITESPACE
                | OrgSyntaxKind::INDENT
                | OrgSyntaxKind::NEWLINE
                | OrgSyntaxKind::L_BRACKET
                | OrgSyntaxKind::L_BRACE
                | OrgSyntaxKind::EOF
        )
    }

    #[inline]
    fn is_inline_call_name_stop(kind: OrgSyntaxKind) -> bool {
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

    #[test]
    fn fixed_underscore_prefix_accepts_and_rejects() {
        use super::fixed_underscore_prefix;

        let p = make_parser("src_py{x}");
        let before = p.cursor();
        let name = fixed_underscore_prefix(&p, "src").expect("src_");
        assert_eq!(p.cursor(), before, "prefix measure must not advance");
        // Third token is TEXT("py").
        assert_eq!(p.token_view(name).map(|t| t.text()), Some("py"));

        assert!(fixed_underscore_prefix(&make_parser("call_n()"), "call").is_some());
        assert!(fixed_underscore_prefix(&make_parser("SRC_py{x}"), "src").is_none());
        assert!(fixed_underscore_prefix(&make_parser("Src_py{x}"), "src").is_none());
        assert!(fixed_underscore_prefix(&make_parser("xsrc_py{x}"), "src").is_none());
        assert!(fixed_underscore_prefix(&make_parser("src py{x}"), "src").is_none());
        assert!(fixed_underscore_prefix(&make_parser("src"), "src").is_none());
    }

    #[test]
    fn fixed_underscore_prefix_at_limit_without_name_token() {
        use super::fixed_underscore_prefix;

        // TEXT + UNDERSCORE only: name start is the visible limit.
        let p = make_parser("src_");
        let name = fixed_underscore_prefix(&p, "src").expect("opener");
        assert_eq!(name, p.limit());
        assert!(p.token_view(name).is_none());
    }

    #[test]
    fn measure_name_stops_on_predicate() {
        use super::{fixed_underscore_prefix, measure_name};

        let p = make_parser("src_foo-bar{x}");
        let name_start = fixed_underscore_prefix(&p, "src").expect("prefix");
        let before = p.cursor();
        let (ns, ne, after) = measure_name(&p, name_start, is_src_language_stop);
        assert_eq!(p.cursor(), before);
        assert_eq!(ns, name_start);
        // Reconstruct name via exclusive end positions.
        let from = usize::from(p.token_range(ns.index()).start());
        let to = if ne < p.limit() {
            usize::from(p.token_range(ne.index()).start())
        } else {
            p.source().len()
        };
        assert_eq!(&p.source()[from..to], "foo-bar");
        assert_eq!(after.map(|t| t.kind()), Some(OrgSyntaxKind::L_BRACE));
        // Relative offset of the after-name token from name_start.
        assert_eq!(ne.index().saturating_sub(ns.index()), 1);

        // Empty name: start already on stop kind.
        let p2 = make_parser("src_{x}");
        let start = fixed_underscore_prefix(&p2, "src").expect("prefix");
        let (ns2, ne2, after2) = measure_name(&p2, start, is_src_language_stop);
        assert_eq!(ns2, ne2);
        assert_eq!(after2.map(|t| t.kind()), Some(OrgSyntaxKind::L_BRACE));
    }

    #[test]
    fn measure_name_call_stops_on_paren_not_brace() {
        use super::{fixed_underscore_prefix, measure_name};

        let p = make_parser("call_n(a)");
        let start = fixed_underscore_prefix(&p, "call").expect("prefix");
        let (_ns, _ne, after) = measure_name(&p, start, is_inline_call_name_stop);
        assert_eq!(after.map(|t| t.kind()), Some(OrgSyntaxKind::L_PAREN));

        // Brace is not an inline-call name stop; name consumes through `{`.
        let p2 = make_parser("call_n{a}");
        let start2 = fixed_underscore_prefix(&p2, "call").expect("prefix");
        let (ns, ne, after2) = measure_name(&p2, start2, is_inline_call_name_stop);
        assert!(
            after2.is_none(),
            "walk runs to limit when brace is not a stop"
        );
        let from = usize::from(p2.token_range(ns.index()).start());
        let to = if ne < p2.limit() {
            usize::from(p2.token_range(ne.index()).start())
        } else {
            p2.source().len()
        };
        assert_eq!(&p2.source()[from..to], "n{a}");
    }

    /// Closed-set stops for `measure_name` (whitespace, bracket, multi-token).
    #[test]
    fn measure_name_stop_set_table() {
        use super::{fixed_underscore_prefix, measure_name};

        // (src after fixed prefix setup via full src form, expected name, stop kind)
        let cases: &[(&str, &str, OrgSyntaxKind)] = &[
            ("src_ab cd{x}", "ab", OrgSyntaxKind::WHITESPACE),
            ("src_ab[hdr]{x}", "ab", OrgSyntaxKind::L_BRACKET),
            ("src_ab{x}", "ab", OrgSyntaxKind::L_BRACE),
            ("src_a_b_c{x}", "a_b_c", OrgSyntaxKind::L_BRACE),
        ];
        for &(src, want_name, stop) in cases {
            let p = make_parser(src);
            let start = fixed_underscore_prefix(&p, "src").expect("prefix");
            let (ns, ne, after) = measure_name(&p, start, is_src_language_stop);
            let from = usize::from(p.token_range(ns.index()).start());
            let to = usize::from(p.token_range(ne.index()).start());
            assert_eq!(&p.source()[from..to], want_name, "name for {src:?}");
            assert_eq!(after.map(|t| t.kind()), Some(stop), "stop for {src:?}");
            // Multi-token language span: underscore interiors keep name_end > name_start + 1.
            if want_name.contains('_') {
                assert!(
                    ne.index() - ns.index() > 1,
                    "multi-token language for {src:?}"
                );
            }
        }
    }
}
