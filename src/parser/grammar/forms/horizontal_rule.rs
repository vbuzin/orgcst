//! Horizontal rule element grammar: five or more hyphens on their own line.
//!
//! Optional leading INDENT and trailing WHITESPACE are allowed; any other
//! non-whitespace content on the line disqualifies the rule. Each qualifying
//! line is its own HORIZONTAL_RULE (rules are never merged).
//!
//! Evidence: org-syntax section 4.3.7; `org-element-horizontal-rule-parser`
//! and the `org-element--current-element` recogniser `[ \t]*-----+[ \t]*$`.
//!
//! Emits:
//! ```text
//! HORIZONTAL_RULE
//!   [INDENT] TEXT("---...") [WHITESPACE] [NEWLINE]
//!   [post-blank tokens]
//! ```
//!
//! Contract (element probe/parse):
//! - [`probe`] is the exact one-line shape.
//! - [`parse`] consumes exactly that line, then post-blank.

use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

/// Minimum consecutive hyphens for a horizontal rule (org-syntax section 4.3.7).
const MIN_HYPHENS: usize = 5;

/// One horizontal-rule line: TEXT of only hyphens (>= 5), optional trailing
/// WHITESPACE, then the line end.
pub(crate) fn probe(line: LineView) -> Option<()> {
    let t = line.content()?;
    if t.kind() != OrgSyntaxKind::TEXT {
        return None;
    }
    let text = t.text();
    if text.len() < MIN_HYPHENS || !text.bytes().all(|b| b == b'-') {
        return None;
    }
    let mut k = 1usize;
    if line.kind(k) == OrgSyntaxKind::WHITESPACE {
        k += 1;
    }
    line.kind(k).is_line_end().then_some(())
}

/// Parse one HORIZONTAL_RULE element (exactly one line).
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser.line().is_some_and(|l| probe(l).is_some()),
        "horizontal_rule::parse called without probe (dispatcher contract)"
    );

    let marker = parser.start();
    parser.eat_line();
    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::HORIZONTAL_RULE);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::parser::grammar::forms::horizontal_rule::{parse, probe};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{ParseResult, make_parser, node_count, node_text, probe_line, run};

    fn results_of(src: &str) -> ParseResult {
        run(src, parse)
    }

    #[test]
    fn five_or_more_hyphens_recognised() {
        assert!(probe_line("-----\n", probe).is_some());
        assert!(probe_line("----------\n", probe).is_some());
        assert!(probe_line("-----", probe).is_some()); // EOF, no NL
    }

    #[test]
    fn surrounding_whitespace_allowed() {
        assert!(probe_line("  -----\n", probe).is_some());
        assert!(probe_line("-----\t  \n", probe).is_some());
        assert!(probe_line("  ------  \n", probe).is_some());
    }

    #[test]
    fn four_hyphens_rejected() {
        assert!(probe_line("----\n", probe).is_none());
    }

    #[test]
    fn non_hyphen_or_gap_disqualifies() {
        assert!(probe_line("-----x\n", probe).is_none());
        assert!(probe_line("--- --\n", probe).is_none());
        assert!(probe_line("-----*\n", probe).is_none());
        assert!(probe_line("not a rule\n", probe).is_none());
    }

    #[test]
    fn parser_consumes_exactly_one_line() {
        let src = "-----\n-----\n";
        let mut p = make_parser(src);
        parse(&mut p);
        assert!(p.line().is_some_and(|l| probe(l).is_some()));
        assert_eq!(p.text(), "-----");
    }

    #[test]
    fn emits_horizontal_rule_covering_full_line() {
        let src = "  ------  \n";
        let (_tokens, events, _) = results_of(src);
        assert_eq!(node_count(&events, OrgSyntaxKind::HORIZONTAL_RULE), 1);
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::HORIZONTAL_RULE).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn horizontal_rule_appears_in_document_tree() {
        use crate::tests::{nodes_of_kind, parse_document_tree, tree_text};

        let src = "before\n-----\nafter\n";
        let root = parse_document_tree(src);
        let rules = nodes_of_kind(&root, OrgSyntaxKind::HORIZONTAL_RULE);
        assert_eq!(rules.len(), 1);
        assert_eq!(tree_text(&rules[0]), "-----\n");
    }
}
