//! Target and radio-target object grammar (`<<name>>`, `<<<contents>>>`).
//!
//! Worg 5.12 / org-element-target-parser / org-element-radio-target-parser.
//!
//! [`probe_radio`] / [`probe_target`] confirm only the opener run; parse eats
//! the openers, then validates the interior. On failure it abandons with the
//! cursor past the opening `<<` / `<<<`, so the inline dispatcher always
//! advances. Both probes produce an `ObjectShape::Target`.

use crate::parser::Parser;
use crate::parser::grammar::drive::region;
use crate::syntax::OrgSyntaxKind;

/// Angle-delimited target opener.
///
/// Probe confirms only the `<<` / `<<<` run; the interior and closer are
/// validated at emit, and a failure leaves the openers bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetShape {
    pub(crate) kind: OrgSyntaxKind,
    pub(crate) open_n: usize,
    pub(crate) close_n: usize,
}

/// `<<<` at the cursor.
pub(crate) fn probe_radio(parser: &Parser) -> Option<TargetShape> {
    parser.here().and_then(|a| {
        a.seq(&[
            OrgSyntaxKind::L_ANGLE,
            OrgSyntaxKind::L_ANGLE,
            OrgSyntaxKind::L_ANGLE,
        ])
        .then_some(TargetShape {
            kind: OrgSyntaxKind::RADIO_TARGET,
            open_n: 3,
            close_n: 3,
        })
    })
}

/// `<<` but not `<<<` at the cursor.
pub(crate) fn probe_target(parser: &Parser) -> Option<TargetShape> {
    parser.here().and_then(|a| {
        (a.seq(&[OrgSyntaxKind::L_ANGLE, OrgSyntaxKind::L_ANGLE])
            && a.kind_at(2) != OrgSyntaxKind::L_ANGLE)
            .then_some(TargetShape {
                kind: OrgSyntaxKind::TARGET,
                open_n: 2,
                close_n: 2,
            })
    })
}

/// `<<<` at the cursor.
#[cfg(test)]
#[inline]
pub(crate) fn at_radio_target(parser: &Parser) -> bool {
    probe_radio(parser).is_some()
}

/// `<<` but not `<<<` at the cursor.
#[cfg(test)]
#[inline]
pub(crate) fn at_target(parser: &Parser) -> bool {
    probe_target(parser).is_some()
}

/// Emit a target or radio target from its opener shape.
pub(crate) fn parse(parser: &mut Parser, shape: TargetShape) {
    parse_angle_delimited_target(parser, shape.kind, shape.open_n, shape.close_n);
}

/// Harness: probe then parse radio target.
#[cfg(test)]
pub(crate) fn parse_radio_target(parser: &mut Parser) {
    if let Some(shape) = probe_radio(parser) {
        parse(parser, shape);
    }
}

/// Harness: probe then parse plain target.
#[cfg(test)]
pub(crate) fn parse_target(parser: &mut Parser) {
    if let Some(shape) = probe_target(parser) {
        parse(parser, shape);
    }
}

/// Shared skeleton for `<<...>>` / `<<<...>>>`.
///
/// Closer scan is at-least-N consecutive `>` (eat exactly `close_n`); extra
/// trailing `>` stay bare so `<<a>>>` yields TARGET `<<a>>` + bare `>`.
/// Interior always goes through `inline_in(kind)`:
/// radio uses markup restrictions; plain TARGET has an empty restriction set
/// so the walk is opaque token eat.
fn parse_angle_delimited_target(
    parser: &mut Parser,
    kind: OrgSyntaxKind,
    open_n: usize,
    close_n: usize,
) {
    let marker = parser.start();
    parser.eat_n(open_n);

    let Some(close) = find_rangle_closer(parser, close_n) else {
        marker.abandon(parser);
        return;
    };
    // Interior: no empty, no leading/trailing whitespace (Worg).
    if close == parser.cursor() {
        marker.abandon(parser);
        return;
    }
    // Reject if the interior has a newline or a lone `<` / `>`, or
    // whitespace at either edge of the interior.
    let (forbidden, last_kind) = {
        let mut forbidden = false;
        let mut last = OrgSyntaxKind::EOF;
        for t in parser.tokens_from_cursor() {
            if t.pos() >= close {
                break;
            }
            let k = t.kind();
            if matches!(
                k,
                OrgSyntaxKind::NEWLINE | OrgSyntaxKind::L_ANGLE | OrgSyntaxKind::R_ANGLE
            ) {
                forbidden = true;
                break;
            }
            last = k;
        }
        (forbidden, last)
    };
    if forbidden || parser.current().is_separator() || last_kind.is_separator() {
        marker.abandon(parser);
        return;
    }

    region::inline_in(parser, close, kind);

    // Closer guaranteed by find. Eat is outside debug_assert so release
    // builds still consume (assert-only side effects are stripped).
    for _ in 0..close_n {
        let ok = parser.eat(OrgSyntaxKind::R_ANGLE);
        debug_assert!(ok, "closer guaranteed by find_rangle_closer");
    }
    marker.complete(parser, kind);
}

/// Position of the first run of at least `n` consecutive `>` from the cursor.
/// The caller eats exactly `n` closers; any extra `>` remain bare.
fn find_rangle_closer(parser: &Parser, n: usize) -> Option<crate::parser::Pos> {
    debug_assert!(n > 0);
    parser
        .tokens_from_cursor()
        .find(|t| t.run_of(OrgSyntaxKind::R_ANGLE, n))
        .map(|t| t.pos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{node_kinds, node_text, nodes_of_kind, run, tree_text};

    fn try_target(src: &str) -> bool {
        let (_toks, evs, _) = run(src, |p| {
            if at_target(p) {
                parse_target(p);
            } else if p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
        });
        node_kinds(&evs).contains(&OrgSyntaxKind::TARGET)
    }

    fn try_radio(src: &str) -> bool {
        let (_toks, evs, _) = run(src, |p| {
            if at_radio_target(p) {
                parse_radio_target(p);
            } else if p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
        });
        node_kinds(&evs).contains(&OrgSyntaxKind::RADIO_TARGET)
    }

    #[test]
    fn parses_plain_target() {
        let src = "<<dest>>";
        let (_toks, evs, _) = run(src, |p| {
            if at_target(p) {
                parse_target(p);
            }
        });
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TARGET));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TARGET).as_deref(),
            Some("<<dest>>")
        );
    }

    #[test]
    fn parses_radio_target() {
        let src = "<<<important>>>";
        let (_toks, evs, _) = run(src, |p| {
            if at_radio_target(p) {
                parse_radio_target(p);
            }
        });
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::RADIO_TARGET));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::RADIO_TARGET).as_deref(),
            Some("<<<important>>>")
        );
    }

    /// Org fidelity: at-least-N closer; extra `>` stay bare after the object.
    #[test]
    fn plain_target_trailing_extra_rangle_stays_bare() {
        let src = "<<a>>>";
        let root = parse(src).root().clone();
        let targets = nodes_of_kind(&root, OrgSyntaxKind::TARGET);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].text().to_string(), "<<a>>");
        // Trailing bare `>` is source text, not part of the TARGET.
        assert!(tree_text(&root).ends_with('>'));
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET).len(), 0);
    }

    #[test]
    fn radio_target_trailing_extra_rangle_stays_bare() {
        let src = "<<<a>>>>\n";
        let root = parse(src).root().clone();
        let radios = nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET);
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].text().to_string(), "<<<a>>>");
    }

    #[test]
    fn empty_and_ws_targets_rejected() {
        assert!(!try_target("<<>>"));
        assert!(!try_target("<< >>"));
        assert!(!try_target("<<x >>"));
        assert!(!try_target("<< x>>"));
        assert!(!try_radio("<<<>>>"));
        assert!(!try_radio("<<< >>>"));
        assert!(!try_radio("<<<x >>>"));
        assert!(!try_radio("<<< x>>>"));
    }

    #[test]
    fn unclosed_and_newline_targets_rejected() {
        assert!(!try_target("<<dest"));
        assert!(!try_radio("<<<dest"));
        assert!(!try_target("<<de\nst>>"));
        assert!(!try_radio("<<<de\nst>>>"));
    }

    #[test]
    fn interior_angle_brackets_rejected() {
        assert!(!try_target("<<a<b>>"));
        assert!(!try_target("<<a>b>>"));
        assert!(!try_radio("<<<a<b>>>"));
        assert!(!try_radio("<<<a>b>>>"));
    }

    #[test]
    fn opener_discrimination_triple_vs_double() {
        // `<<<` is radio, not plain target.
        assert!(at_radio_target(&crate::tests::make_parser("<<<x>>>")));
        assert!(!at_target(&crate::tests::make_parser("<<<x>>>")));
        // `<<` (not triple) is plain target.
        assert!(at_target(&crate::tests::make_parser("<<x>>")));
        assert!(!at_radio_target(&crate::tests::make_parser("<<x>>")));
    }

    #[test]
    fn radio_target_allows_markup_not_links() {
        let src = "<<<*bold*>>>\n";
        let root = parse(src).root().clone();
        let radios = nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET);
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].text().to_string(), "<<<*bold*>>>");
        assert!(
            radios[0]
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::BOLD),
            "RADIO_TARGET interior should parse markup"
        );
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::LINK).len(),
            0,
            "markup-only restrictions: no LINK inside radio target"
        );
    }

    #[test]
    fn abandon_still_advances_past_opener() {
        // Plain empty, radio empty, and unclosed forms: progress past opener,
        // no completed node.
        let cases: &[(&str, bool, bool)] = &[
            // (src, is_radio, expect_radio_at)
            ("<<>>", false, false),
            ("<<<>>>", true, true),
            ("<<dest", false, false),
            ("<<<dest", true, true),
        ];
        for &(src, want_radio, at_radio) in cases {
            let mut p = crate::tests::make_parser(src);
            let before = p.cursor().index();
            if at_radio {
                assert!(at_radio_target(&p), "at_radio for {src:?}");
                parse_radio_target(&mut p);
            } else {
                assert!(at_target(&p), "at_target for {src:?}");
                parse_target(&mut p);
            }
            assert!(
                p.cursor().index() > before,
                "must not stall after abandon for {src:?}"
            );
            let events = p.finish().1;
            let kinds = node_kinds(&events);
            if want_radio {
                assert!(
                    !kinds.contains(&OrgSyntaxKind::RADIO_TARGET),
                    "no RADIO_TARGET for {src:?}"
                );
            } else {
                assert!(
                    !kinds.contains(&OrgSyntaxKind::TARGET),
                    "no TARGET for {src:?}"
                );
            }
        }
    }

    #[test]
    fn find_rangle_closer_at_least_n() {
        fn at(src: &str, eats: usize, n: usize) -> Option<usize> {
            let mut p = crate::tests::make_parser(src);
            for _ in 0..eats {
                p.eat_any();
            }
            find_rangle_closer(&p, n).map(crate::parser::pos::Pos::index)
        }
        assert_eq!(at(">>", 0, 2), Some(0));
        assert_eq!(at(">>", 0, 3), None);

        // At-least-2 matches at index 0 (first two of the triple).
        assert_eq!(at(">>>", 0, 2), Some(0));
        assert_eq!(at(">>>", 0, 3), Some(0));
        // Advanced cursor: skip first `>`, still finds a pair.
        assert_eq!(at(">>>", 1, 2), Some(1));
        // Past the run -> None.
        assert_eq!(at(">>>", 2, 2), None);
        assert_eq!(at(">>>", 3, 1), None);

        assert_eq!(at("abc", 0, 1), None);
        assert_eq!(at("abc", 0, 2), None);

        // Single `>` is enough for n=1, not for n=2.
        assert_eq!(at(">", 0, 1), Some(0));
        assert_eq!(at(">", 0, 2), None);
    }
}
