//! Tests for the line and run consumers (`eat_line`, `eat_while`,
//! `eat_to_line_end`, `eat_run`, `eat_n`) and the public `parse` entry point.

use crate::parser::Event;
use crate::syntax::OrgSyntaxKind;
use crate::tests::{make_parser, pos_ahead};

// -----------------------------------------------------------------------
// eat_line
// -----------------------------------------------------------------------

#[test]
fn eat_line_consumes_at_line_start_and_stops_after_newline() {
    let src = "first line\nsecond\n";
    let mut p = make_parser(src);

    assert!(p.at_line_start());
    assert_eq!(p.eat_line(), 1);
    assert!(p.at_line_start());
    assert_eq!(p.text(), "second");
    assert_eq!(p.eat_line(), 1);
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_line_returns_zero_when_not_at_line_start() {
    let src = "foo bar\n";
    let mut p = make_parser(src);
    p.eat_any(); // "foo"
    p.eat_any(); // ws
    assert!(!p.at_line_start());
    assert_eq!(p.eat_line(), 0);
    assert_eq!(p.text(), "bar");
}

#[test]
fn eat_line_stops_at_eof_without_newline() {
    let src = "last line";
    let mut p = make_parser(src);
    assert!(p.at_line_start());
    assert_eq!(p.eat_line(), 1);
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_line_handles_line_with_only_newline() {
    let src = "\nnext\n";
    let mut p = make_parser(src);
    assert!(p.at_line_start());
    assert_eq!(p.eat_line(), 1);
    assert!(p.at_line_start());
    assert_eq!(p.text(), "next");
}

#[test]
fn eat_line_respects_limit() {
    let src = "line1\nline2\nline3\n";
    let mut base = make_parser(src);
    base.eat_line();
    let second_line_start = base.cursor();

    let mut p = make_parser(src);
    p.with_limit_at(second_line_start, |pp| {
        assert!(pp.at_line_start());
        assert_eq!(pp.eat_line(), 1);
        assert_eq!(pp.current(), OrgSyntaxKind::EOF);
    });
}

#[test]
fn eat_line_with_indent_consumes_indent_and_content() {
    let src = "  indented line\nnext\n";
    let mut p = make_parser(src);
    assert!(p.at_line_start());
    assert_eq!(p.current(), OrgSyntaxKind::INDENT);
    assert_eq!(p.eat_line(), 1);
    assert!(p.at_line_start());
    assert_eq!(p.text(), "next");
}

// -----------------------------------------------------------------------
// eat_while / eat_to_line_end
// -----------------------------------------------------------------------

#[test]
fn eat_while_drain_to_eof_returns_count() {
    let src = "abc def";
    let mut p = make_parser(src);
    let n = p.eat_while(|pp| pp.current() != OrgSyntaxKind::EOF);
    assert_eq!(n, 3); // TEXT, WHITESPACE, TEXT
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_to_line_end_stops_at_newline_without_consuming_it() {
    let src = "first line\nsecond";
    let mut p = make_parser(src);
    let n = p.eat_to_line_end();
    assert_eq!(n, 3); // TEXT "first", WHITESPACE, TEXT "line"
    assert_eq!(p.current(), OrgSyntaxKind::NEWLINE);
    assert_ne!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_to_line_end_drains_final_line_to_eof() {
    let src = "first line\nsecond";
    let mut p = make_parser(src);
    assert_eq!(p.eat_line(), 1);
    let n = p.eat_to_line_end();
    assert_eq!(n, 1); // "second" TEXT
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_to_line_end_zero_when_already_at_term() {
    let mut p = make_parser("x\n");
    p.eat_any(); // TEXT "x"
    assert!(p.at_line_term());
    assert_eq!(p.eat_to_line_end(), 0);
    assert_eq!(p.current(), OrgSyntaxKind::NEWLINE);

    let mut empty = make_parser("");
    assert!(empty.at_line_term()); // visible EOF
    assert_eq!(empty.eat_to_line_end(), 0);
    assert_eq!(empty.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_while_zero_count_when_pred_false_or_eof() {
    let src = "x";
    let mut p = make_parser(src);
    let n = p.eat_while(|pp| pp.current() == OrgSyntaxKind::NEWLINE);
    assert_eq!(n, 0);
    assert_ne!(p.current(), OrgSyntaxKind::EOF);

    p.eat_while(|pp| pp.current() != OrgSyntaxKind::EOF);
    let n2 = p.eat_while(|pp| pp.current() != OrgSyntaxKind::EOF);
    assert_eq!(n2, 0);
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
}

#[test]
fn eat_while_mixed_predicate_tags_style() {
    let src = ":a:b: rest";
    let mut p = make_parser(src);
    let n = p.eat_while(|pp| {
        pp.current() == OrgSyntaxKind::COLON
            || (pp.current() == OrgSyntaxKind::TEXT && pp.text().chars().all(char::is_alphanumeric))
    });
    assert_eq!(n, 5);
    assert_eq!(p.text(), " ");
}

#[test]
fn eat_while_respects_limit() {
    let src = "a b c d";
    let mut base = make_parser(src);
    base.eat_any(); // a
    base.eat_any(); // ws
    let b_abs = base.cursor();

    let mut p = make_parser(src);
    p.with_limit_at(b_abs, |pp| {
        let n = pp.eat_while(|ppp| ppp.current() != OrgSyntaxKind::EOF);
        assert_eq!(n, 2);
        assert_eq!(pp.current(), OrgSyntaxKind::EOF);
    });
    let p3 = make_parser(src);
    assert_eq!(p3.text(), "a");
}

#[test]
fn eat_while_nesting_with_limit_at() {
    let src = "w x y z";
    let mut p = make_parser(src);
    p.with_limit_at(pos_ahead(&p, 2), |pp| {
        pp.with_limit_at(pos_ahead(pp, 1), |ppp| {
            let n = ppp.eat_while(|q| q.current() != OrgSyntaxKind::EOF);
            assert_eq!(n, 1);
            assert_eq!(ppp.current(), OrgSyntaxKind::EOF);
        });
        let n2 = pp.eat_while(|q| q.current() != OrgSyntaxKind::EOF);
        assert_eq!(n2, 1);
        assert_eq!(pp.current(), OrgSyntaxKind::EOF);
    });
}

#[test]
fn eat_while_count_zero_on_empty_region() {
    let src = "";
    let mut p = make_parser(src);
    assert_eq!(p.eat_while(|pp| pp.current() != OrgSyntaxKind::EOF), 0);
    let mut p2 = make_parser("x");
    p2.with_limit_at(p2.cursor(), |pp| {
        assert_eq!(pp.eat_while(|_| true), 0);
        assert_eq!(pp.current(), OrgSyntaxKind::EOF);
    });
}

#[test]
fn eat_while_uses_eat_any_for_losslessness() {
    let src = "one two";
    let mut p = make_parser(src);
    let _ = p.eat_while(|pp| pp.current() != OrgSyntaxKind::EOF);
    let (toks, evs, _) = p.finish();
    assert!(evs.iter().all(|e| matches!(e, Event::Token { .. })));
    assert_eq!(evs.len(), toks.len());
}

#[test]
fn eat_run_counts_kind_run() {
    let src = "*** foo";
    let mut p = make_parser(src);
    let n = p.eat_run(OrgSyntaxKind::STAR);
    assert_eq!(n, 3);
    assert_eq!(p.text(), " ");
}

#[test]
fn eat_run_zero_when_not_matching() {
    let src = "foo";
    let mut p = make_parser(src);
    let n = p.eat_run(OrgSyntaxKind::STAR);
    assert_eq!(n, 0);
    assert_eq!(p.text(), "foo");
}

// -----------------------------------------------------------------------
// eat_n / leaf / public parse
// -----------------------------------------------------------------------

#[test]
fn eat_n_and_leaf_consume_and_bracket() {
    let mut p = make_parser("***");
    p.leaf(OrgSyntaxKind::STAR, 3);
    assert_eq!(p.current(), OrgSyntaxKind::EOF);
    let (_, evs, _) = p.finish();
    assert!(matches!(
        evs.first(),
        Some(Event::Start {
            kind: OrgSyntaxKind::STAR
        })
    ));
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, Event::Token { .. }))
            .count(),
        3
    );
    assert!(matches!(evs.last(), Some(Event::Finish)));

    let mut p2 = make_parser("x y");
    p2.eat_n(1);
    assert_ne!(p2.current(), OrgSyntaxKind::EOF);
    p2.eat_n(2); // remaining WS + TEXT
    assert_eq!(p2.current(), OrgSyntaxKind::EOF);
}

#[test]
fn cursor_and_limit_handles_match_token_window() {
    use crate::lexer::lex;
    let src = "a b c";
    let mut p = make_parser(src);
    assert_eq!(p.limit().index(), lex(src).tokens.len());
    assert_eq!(p.cursor().index(), 0);
    p.eat_any();
    assert_eq!(p.cursor().index(), 1);
    let cap = pos_ahead(&p, 1);
    p.with_limit_at(cap, |pp| {
        assert_eq!(pp.limit(), cap);
        assert_eq!(pp.current(), OrgSyntaxKind::WHITESPACE);
        pp.eat_any();
        assert_eq!(pp.current(), OrgSyntaxKind::EOF);
    });
    assert_eq!(p.limit().index(), lex(src).tokens.len());
}

#[test]
fn public_parse_api_returns_document_and_preserves_text() {
    let src = "* TODO [#B] Title :a:b:\nfirst para\n\nsecond\n";
    let res = crate::parse(src);
    assert_eq!(res.root().kind(), crate::OrgSyntaxKind::DOCUMENT);
    assert_eq!(
        res.root().text().to_string(),
        src,
        "public parse must be lossless"
    );

    let bad_src = "* [#a] bad\n";
    let bad = crate::parse(bad_src);
    assert!(!bad.diagnostics().is_empty(), "{:?}", bad.diagnostics());
    assert!(
        bad.diagnostics().iter().any(|d| {
            matches!(
                d.kind(),
                crate::DiagnosticKind::InvalidPriorityCookie { value } if value == "a"
            )
        }),
        "expected invalid-priority diagnostic, got {:?}",
        bad.diagnostics()
            .iter()
            .map(crate::Diagnostic::message)
            .collect::<Vec<_>>()
    );
    let d0 = &bad.diagnostics()[0];
    assert_ne!(
        d0.range().start(),
        d0.range().end(),
        "diagnostic should cover a source range (not empty)"
    );
    assert_eq!(format!("{d0}"), d0.message());
    assert!(d0.message().contains("invalid priority cookie value"));
    let start = usize::from(d0.range().start());
    let end = usize::from(d0.range().end());
    assert_eq!(&bad_src[start..end], "[#a]");
}
