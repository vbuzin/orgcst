//! Statistics cookie object: `[/]`, `[1/3]`, `[%]`, `[50%]`.
//!
//! Evidence: `org-element-statistics-cookie-parser` matches
//! `\[[0-9]*\(%\|/[0-9]*\)\]` - digits are optional on both sides, so `[%]`,
//! `[/]`, `[2/]`, and `[/7]` are all cookies. Allowed in the standard object
//! set (paragraphs, emphasis, headline titles, item tags) and in link
//! descriptions; excluded from table cells.
//!
//! Emits: STATISTICS_COOKIE covering the full bracketed span.
//!
//! [`probe`] is pure and returns the token count; [`parse`] emits the leaf
//! from that count and cannot fail after a successful probe.

use crate::parser::Parser;
use crate::syntax::OrgSyntaxKind;

use crate::parser::grammar::drive::atoms;
use atoms::is_digits;

/// Measured statistics cookie: full bracket span token count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CookieShape {
    /// Tokens from `[` through `]` inclusive.
    pub(crate) tokens: usize,
}

/// Pure measure of an exact cookie at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<CookieShape> {
    let t = parser.here()?;
    if t.kind() != OrgSyntaxKind::L_BRACKET {
        return None;
    }
    let mut i = 1usize;

    // Optional leading digits.
    if t.kind_at(i) == OrgSyntaxKind::TEXT && is_digits(t.text_at(i)) {
        i += 1;
    }

    match t.kind_at(i) {
        OrgSyntaxKind::PERCENT => {
            i += 1;
        }
        OrgSyntaxKind::SLASH => {
            i += 1;
            // Optional trailing digits after `/`.
            if t.kind_at(i) == OrgSyntaxKind::TEXT && is_digits(t.text_at(i)) {
                i += 1;
            }
        }
        _ => return None,
    }

    (t.kind_at(i) == OrgSyntaxKind::R_BRACKET).then_some(CookieShape { tokens: i + 1 })
}

/// Emit one statistics cookie from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: CookieShape) {
    parser.leaf(OrgSyntaxKind::STATISTICS_COOKIE, shape.tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{make_parser, node_text, nodes_of_kind, parse_document_tree, tree_text};

    #[test]
    fn cookie_shapes() {
        for src in ["[/]", "[%]", "[1/3]", "[50%]", "[2/]", "[/7]", "[10/20]"] {
            assert!(
                probe(&make_parser(src)).is_some(),
                "expected cookie for {src:?}"
            );
        }
        for src in [
            "[a/b]",
            "[1.5%]",
            "[1 / 3]",
            "[]",
            "[1]",
            "(1/3)",
            "[2026-06-24]",
        ] {
            assert!(probe(&make_parser(src)).is_none(), "not a cookie: {src:?}");
        }
    }

    #[test]
    fn cookie_probe_is_pure_and_parse_uses_shape() {
        let p = make_parser("[2/7]");
        let before = p.cursor();
        let shape = probe(&p).expect("cookie");
        assert_eq!(shape.tokens, 5);
        assert_eq!(p.cursor(), before, "probe must not advance");

        let mut p2 = make_parser("[2/7]");
        let shape = probe(&p2).expect("cookie");
        parse(&mut p2, shape);
        let (_, evs, _) = p2.finish();
        assert_eq!(
            node_text(&evs, "[2/7]", OrgSyntaxKind::STATISTICS_COOKIE).as_deref(),
            Some("[2/7]")
        );
    }

    #[test]
    fn document_roundtrip_cookies() {
        let src = "done [/] and [50%]\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            2
        );
    }

    #[test]
    fn cookie_in_paragraph_and_headline_title() {
        let src = "tasks [2/7] done\n";
        let root = parse_document_tree(src);
        let cookies = nodes_of_kind(&root, OrgSyntaxKind::STATISTICS_COOKIE);
        assert_eq!(cookies.len(), 1);
        assert_eq!(tree_text(&cookies[0]), "[2/7]");

        let src2 = "* Todos [50%]\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let cookies2 = nodes_of_kind(&root2, OrgSyntaxKind::STATISTICS_COOKIE);
        assert_eq!(cookies2.len(), 1, "cookie inside HEADING_TITLE");
    }

    #[test]
    fn near_miss_brackets_stay_text() {
        let src = "see [2026-06-24] and [1] refs\n";
        let (_toks, evs, _) = crate::tests::run(src, |p| {
            crate::parser::grammar::forms::paragraph::parse(p);
        });
        assert_eq!(node_text(&evs, src, OrgSyntaxKind::STATISTICS_COOKIE), None);
    }
}
