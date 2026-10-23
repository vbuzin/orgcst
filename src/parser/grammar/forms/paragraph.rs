//! Paragraph element grammar - the fallback container for inline content.
//!
//! Catch-all element: top-level material that is not a headline, comment,
//! fixed-width, block, latex environment, keyword, clock, horizontal rule, or
//! other recognised element. Content is a run of inline objects (markup,
//! links, timestamps, plain text, ...) up to a termination boundary.
//!
//! Emits:
//!   PARAGRAPH
//!     (inline objects and bare tokens)
//!     [post-blank tokens]
//!
//! The **content** region always includes the first line, then ends at the
//! earliest of:
//! - a blank line (content ends before it; blanks are post-blank);
//! - a line that interrupts a paragraph ([`decide::interrupts_paragraph`]).
//!   Every line [`decide::decide`] claims interrupts, and so does a heading,
//!   except a `#+(\S-+)[...]:` line whose name is not the written spelling
//!   `CAPTION` or `RESULTS`. That name stops at the rightmost `[` of the
//!   leading non-whitespace run, so `#+CAPTION[a b][c]:` ends the paragraph,
//!   `#+CAPTION[a][b]:` does not, and `RESULT` never does. A degraded opener
//!   interrupts only when decide sets `Element::Degraded.interrupts`: always
//!   for dynamic `#+begin:`, never for unclosed underscore blocks, drawers or
//!   latex environments. Closed ones are claims (`Element::Block`,
//!   `Element::Drawer`, `Element::LatexEnvironment`), not degraded openers;
//! - the end of the visible input (active limit or EOF).
//!
//! Checking whether a frame closes is amortised
//! ([`crate::parser::grammar::drive::frame`]).
//!
//! After the content, trailing blank lines are claimed under PARAGRAPH with
//! [`Parser::eat_blank_lines`]. The dispatcher never opens a paragraph at a
//! blank line.
//!
//! Contract:
//! - Precondition: caller has determined this region is paragraph material
//!   (no recognised element at the current line; not blank).
//! - Always emits one PARAGRAPH; inline runs under the measured end limit +
//!   PARAGRAPH restrictions; then post-blank under the same node.
//! - Cursor left just after content + post-blank (at next non-blank or EOF).
//! - No configuration; shape is purely structural.

use crate::parser::{Parser, Pos};
use crate::syntax::OrgSyntaxKind;

use crate::parser::grammar::drive::{decide, region};
use decide::interrupts_paragraph;

/// Parse a paragraph at the cursor.
///
/// Content region end from [`paragraph_end`] (stops before blank lines and
/// interrupting lines); inline under PARAGRAPH restrictions; trailing blanks
/// under this node. Precondition: not at a blank line.
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        !parser.at_blank(),
        "paragraph::parse must not open on a blank line (dispatcher contract)"
    );
    let end = paragraph_end(parser);
    let marker = parser.start();
    region::inline_in(parser, end, OrgSyntaxKind::PARAGRAPH);
    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::PARAGRAPH);
}

/// Exclusive position where the current paragraph's content ends.
///
/// Always includes the rest of the current line (also on mid-line entry);
/// then stops at the first following blank or interrupting line, else the
/// visible end.
fn paragraph_end(parser: &Parser) -> Pos {
    let Some(current) = parser.line_here() else {
        return parser.limit();
    };
    current
        .lines_after()
        .find(|l| l.is_blank() || interrupts_paragraph(*l))
        .map_or_else(|| parser.limit(), |l| l.start())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{ParseResult, make_parser, node_kinds, node_text, run, token_count};

    fn results_of(src: &str) -> ParseResult {
        run(src, parse)
    }

    // --- Basic structure ---------------------------------------------------

    #[test]
    fn emits_paragraph_node() {
        let src = "hello world\n";
        let (_tokens, events, _) = results_of(src);
        assert_eq!(node_kinds(&events), vec![OrgSyntaxKind::PARAGRAPH]);
        // The newline belongs to the paragraph region (first line consumed).
        assert!(token_count(&events, OrgSyntaxKind::NEWLINE) >= 1);
    }

    #[test]
    fn single_line_plain_text() {
        let src = "just text\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).as_deref(),
            Some("just text\n")
        );
    }

    // --- Multiline ---------------------------------------------------------

    #[test]
    fn multiline_continues_across_single_newline() {
        // Content lines separated by single newlines form one paragraph.
        // The paragraph owns the newline that ends its last included line.
        let src = "line one\nline two\nline three\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, src);
    }

    #[test]
    fn blank_line_is_post_blank_under_paragraph() {
        let src = "first paragraph\n\nsecond starts here\n";
        let (_toks, evs, _) = results_of(src);
        // Content ends before the blank; post-blank is claimed under PARAGRAPH.
        // Isolated parse only claims the first para + its post-blank.
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).as_deref(),
            Some("first paragraph\n\n")
        );
    }

    #[test]
    fn paragraph_without_trailing_newline_consumes_to_eof() {
        let src = "no trailing newline";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_kinds(&evs), vec![OrgSyntaxKind::PARAGRAPH]);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).as_deref(),
            Some(src)
        );
    }

    // --- Interrupting lines terminate --------------------------------------

    #[test]
    fn headline_terminates_paragraph() {
        let src = "some text\n* Headline\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "some text\n");
    }

    #[test]
    fn comment_terminates_paragraph() {
        let src = "body text\n# a comment\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "body text\n");
    }

    #[test]
    fn keyword_terminates_paragraph() {
        let src = "content\n#+TITLE: Hi\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "content\n");
    }

    #[test]
    fn indented_keyword_terminates() {
        let src = "text\n  #+NAME: foo\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "text\n");
    }

    #[test]
    fn closed_block_opener_terminates_paragraph() {
        let src = "before block\n#+begin_src\ncode\n#+end_src\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "before block\n");
        assert!(!text.contains("begin_src"));
    }

    #[test]
    fn unterminated_block_opener_does_not_terminate_paragraph() {
        // org-element paragraph rule: a `#+begin_NAME` line only separates
        // when the block is closed; an unterminated opener stays paragraph
        // text (org-element-paragraph-parser double-check).
        let src = "before\n#+begin_src\nstill the same paragraph\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, src);
    }

    #[test]
    fn empty_name_underscore_absorbed_dynamic_still_interrupts() {
        // Paired empty-name openers under a running paragraph: underscore
        // `#+begin_` degrades without interrupt; dynamic `#+begin:` always
        // interrupts (keyword-shaped), even with an empty name.
        let absorb = "before\n#+begin_\nstill same para\n";
        let (_toks, evs, _) = results_of(absorb);
        let text = node_text(&evs, absorb, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, absorb, "empty underscore opener stays in paragraph");

        let cut = "before\n#+begin:\nbody\n";
        let (_toks2, evs2, _) = results_of(cut);
        let text2 = node_text(&evs2, cut, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text2, "before\n", "empty dynamic opener interrupts");
    }

    #[test]
    fn unclosed_dynamic_opener_still_terminates_paragraph() {
        // The dynamic form is keyword-shaped: it separates regardless of a
        // matching `#+end:` (org paragraph-separate keyword alternative).
        let src = "before\n#+begin: clocktable\nbody\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).unwrap();
        assert_eq!(text, "before\n");
    }

    #[test]
    fn malformed_clock_still_interrupts_paragraph() {
        // Prefilter claim: any CLOCK: line ends the running paragraph even
        // when parse rewinds (body is not a legal clock); the line then falls
        // through to its own paragraph.
        let src = "before\nCLOCK: not-a-stamp\nafter\n";
        let root = crate::tests::parse_document_tree(src);
        let paras = crate::tests::nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        let texts: Vec<_> = paras.iter().map(crate::tests::tree_text).collect();
        assert!(
            texts.iter().any(|t| *t == "before\n"),
            "malformed CLOCK must interrupt; got {texts:?}"
        );
        assert!(
            !texts[0].contains("CLOCK:"),
            "first paragraph must end before CLOCK; got {:?}",
            texts[0]
        );
        assert!(
            texts.iter().any(|t| t.contains("CLOCK:")),
            "rewound CLOCK line falls through to a later paragraph; got {texts:?}"
        );
    }

    // --- Inline delegation -------------------------------------------------

    #[test]
    fn markup_objects_appear_inside_paragraph() {
        let src = "see *bold* and /italic/ here\n";
        let (_, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::PARAGRAPH));
        assert!(kinds.contains(&OrgSyntaxKind::BOLD));
        assert!(kinds.contains(&OrgSyntaxKind::ITALIC));
    }

    #[test]
    fn bracket_text_is_consumed_inside_paragraph() {
        // Lone brackets (not [[) are plain tokens; paragraph + inline must consume them.
        let src = "refs [1] [2]\n";
        let mut p = make_parser(src);
        let before = p.cursor().index();
        parse(&mut p);
        let after = p.cursor().index();
        let (_, evs, _) = p.finish();
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::PARAGRAPH));
        assert!(after > before);
    }

    #[test]
    fn timestamp_object_appears_inside_paragraph() {
        let src = "due <2026-06-24 Wed>\n";
        let (_, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::PARAGRAPH));
        // Timestamp scaffolding consumes the opening <; we just ensure the
        // object is accepted inside the paragraph without crashing.
    }

    // --- Cursor / region ---------------------------------------------------

    #[test]
    fn cursor_advances_past_paragraph_region() {
        // Paragraph followed by a comment (interrupting line) after continuation lines.
        let src = "one line\nnext thing\n# comment\n";
        let mut p = make_parser(src);
        let before = p.cursor().index();
        parse(&mut p);
        assert!(p.cursor().index() > before);
        // The comment line starts with HASH; paragraph should have stopped before it.
        assert_eq!(p.current(), OrgSyntaxKind::HASH);
    }

    #[test]
    fn paragraph_at_eof_after_first_line() {
        // No newline at all; paragraph takes everything.
        let src = "lonely";
        let mut p = make_parser(src);
        parse(&mut p);
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
    }

    // --- Outer limit -------------------------------------------------------

    #[test]
    fn respects_outer_limit() {
        // Simulate a caller that has already limited the view (e.g. block body).
        let src = "inside limit\noutside should be invisible\n";
        let mut p = make_parser(src);
        // Cap after the first line's newline.
        let nl = p
            .tokens_from_cursor()
            .find(|t| t.kind() == OrgSyntaxKind::NEWLINE)
            .expect("newline");
        let cap = nl.end(); // include the NL
        p.with_limit_at(cap, |p| {
            parse(p);
        });
        let (_, evs, _) = p.finish();
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::PARAGRAPH));
    }
}
