//! Footnote references (objects) and footnote definitions (greater elements).
//!
//! Evidence:
//! - Worg 5.4 / `org-element-footnote-reference-parser` / `org-footnote-re`
//! - Worg 4.2.4 / `org-element-footnote-definition-parser` /
//!   `org-footnote-definition-re` / `org-element--footnote-separator`
//!
//! Reference forms: `[fn:LABEL]`, `[fn:LABEL:DEFINITION]`, `[fn::DEFINITION]`.
//! Label = word constituents + `-` + `_` (unicode alphanumerics; `_` is its
//! own token). Inline DEFINITION uses the standard object set with balanced
//! square brackets (multi-line; [`Parser::find_paired_at`] with
//! [`crate::parser::PairNewlines::Allow`]).
//! Standard form at column 0 is a definition element, not a reference.
//!
//! Definition: unindented `[fn:LABEL]` greater element. Contents end at the
//! next definition, next star-head line (`^\*+ `, any level), two consecutive
//! blank lines, or EOF. Single blanks are allowed inside. Affiliated keyword
//! lines immediately above the next definition are excluded from this
//! definition's body so the run can attach to the next def instead.
//!
//! Emits:
//! ```text
//! FOOTNOTE_REFERENCE          // flat leaf (standard) or open marker +
//!                             //   inline objects (inline/anonymous) + ]
//! FOOTNOTE_DEFINITION
//!   [fn:LABEL] tokens
//!   ... element contents ...  // recursive parse_one_element
//!   [post-blank]
//! ```

use crate::parser::{LineView, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

use super::{affiliated, heading};
use crate::parser::grammar::drive::{atoms, frame, region};

// ---------------------------------------------------------------------------
// Label class (shared by ref + def)
// ---------------------------------------------------------------------------

/// `[` + `fn` + `:` at absolute view offset 0.
fn at_fn_prefix(t: TokenView) -> bool {
    t.kind() == OrgSyntaxKind::L_BRACKET
        && t.kind_at(1) == OrgSyntaxKind::TEXT
        && t.text_at(1) == "fn"
        && t.kind_at(2) == OrgSyntaxKind::COLON
}

// ---------------------------------------------------------------------------
// Footnote reference (object)
// ---------------------------------------------------------------------------

/// Measured closed footnote reference, as returned by [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FootnoteRefShape {
    /// Flat `[fn:LABEL]`; token count includes both brackets.
    Standard { tokens: usize },
    /// `[fn:LABEL:DEF]` / `[fn::DEF]`; open tokens through the second `:`,
    /// closer is the matching outer `]`.
    Inline { open_tokens: usize, closer: Pos },
}

/// Whether the cursor starts an unindented line, where a standard
/// `[fn:LABEL]` is a definition instead of a reference.
///
/// [`Parser::line`] is `None` off line start.
fn at_column0_line_start(parser: &Parser) -> bool {
    parser.line().is_some_and(|l| l.indent_cols() == 0)
}

/// Pure measure of a closed footnote reference at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<FootnoteRefShape> {
    let t = parser.here()?;
    if !at_fn_prefix(t) {
        return None;
    }

    // After `[fn:`
    let label_len = t.run_while(3, atoms::is_name_atom);
    let i = 3 + label_len;

    match t.kind_at(i) {
        OrgSyntaxKind::R_BRACKET if label_len > 0 => {
            // Standard `[fn:LABEL]`. Column-0 unindented is a definition.
            if at_column0_line_start(parser) {
                return None;
            }
            Some(FootnoteRefShape::Standard { tokens: i + 1 })
        }
        OrgSyntaxKind::COLON => {
            // Inline / anonymous: second colon opens the definition body.
            // Balanced multi-line `]` from the opening `[` (PairNewlines::Allow).
            let open_tokens = i + 1;
            let closer = parser.find_paired_at(
                parser.cursor(),
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET,
                crate::parser::PairNewlines::Allow,
            )?;
            // Closer must be strictly after the second colon's token.
            let after_open = t.nth(open_tokens).map(|n| n.pos())?;
            if closer.index() < after_open.index() {
                return None;
            }
            Some(FootnoteRefShape::Inline {
                open_tokens,
                closer,
            })
        }
        _ => None,
    }
}

/// True when a closed footnote reference starts at the cursor.
#[cfg(test)]
#[inline]
pub(crate) fn at_footnote_reference(parser: &Parser) -> bool {
    probe(parser).is_some()
}

/// Emit one footnote reference from a measured layout (no remeasure).
pub(crate) fn parse_reference(parser: &mut Parser, layout: FootnoteRefShape) {
    match layout {
        FootnoteRefShape::Standard { tokens } => {
            parser.leaf(OrgSyntaxKind::FOOTNOTE_REFERENCE, tokens);
        }
        FootnoteRefShape::Inline {
            open_tokens,
            closer,
        } => {
            let marker = parser.start();
            parser.eat_n(open_tokens);
            region::inline_in(parser, closer, OrgSyntaxKind::FOOTNOTE_REFERENCE);
            let ok = parser.eat(OrgSyntaxKind::R_BRACKET);
            debug_assert!(ok, "closer guaranteed by probe");
            marker.complete(parser, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        }
    }
}

/// Harness: probe then parse reference (unit tests / local walks).
#[cfg(test)]
pub(crate) fn parse_footnote_reference(parser: &mut Parser) {
    if let Some(layout) = probe(parser) {
        parse_reference(parser, layout);
    }
}

// ---------------------------------------------------------------------------
// Footnote definition (element)
// ---------------------------------------------------------------------------

/// Cheap header check: unindented `[fn:LABEL]`, without walking the body.
///
/// The dispatcher and `interrupts_paragraph` run only this check; [`resolve`]
/// measures the body once, at parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FootnoteOpener {
    /// Content-relative token count of `[fn:LABEL]` (delimiters included).
    header_tokens: usize,
}

/// Parse-ready definition: opener plus shared frame (no end line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FootnoteDefShape {
    opener: FootnoteOpener,
    frame: frame::FrameShape,
}

/// Unindented `[fn:LABEL]` header at line content (no body measure).
pub(crate) fn opener(line: LineView) -> Option<FootnoteOpener> {
    if line.indent_cols() != 0 {
        return None;
    }
    if line.kind(0) != OrgSyntaxKind::L_BRACKET {
        return None;
    }
    if line.kind(1) != OrgSyntaxKind::TEXT || line.text(1) != "fn" {
        return None;
    }
    if line.kind(2) != OrgSyntaxKind::COLON {
        return None;
    }
    let label_len = line.run_while(3, atoms::is_name_atom);
    if label_len == 0 {
        return None;
    }
    let k = 3 + label_len;
    if line.kind(k) != OrgSyntaxKind::R_BRACKET {
        return None;
    }
    Some(FootnoteOpener {
        header_tokens: k + 1,
    })
}

/// Exclusive contents end for a definition opened on `def_line`.
///
/// One forward pass over the following lines. Org ends the previous definition
/// before affiliated keywords immediately above the next def
/// (`org-element-footnote-definition-parser`).
fn measure_body_end(def_line: LineView) -> Pos {
    let mut consecutive_blanks = 0usize;
    let mut first_blank_start: Option<Pos> = None;
    // Start of the current contiguous run of affiliated-keyword lines.
    let mut affiliated_run_start: Option<Pos> = None;
    let mut last_end = def_line.end();

    for line in def_line.lines_after() {
        last_end = line.end();
        if line.is_blank() {
            if consecutive_blanks == 0 {
                first_blank_start = Some(line.start());
            }
            consecutive_blanks += 1;
            if consecutive_blanks >= 2 {
                return first_blank_start.unwrap_or(line.start());
            }
            // A blank breaks an affiliated run; org's backward walk stops there.
            affiliated_run_start = None;
            continue;
        }
        consecutive_blanks = 0;
        first_blank_start = None;

        // Org footnote separator includes any headline-re (`^\*+ `), so stop
        // on every star-head (true headline or high-star task), not only
        // outline-level probe.
        if heading::star_head(line).is_some() {
            return line.start();
        }
        if opener(line).is_some() {
            // Next definition: exclude trailing affiliated keywords so the
            // run above it stays outside this body and can attach there.
            return affiliated_run_start.unwrap_or(line.start());
        }
        // Line shape only: org applies no attach test on this walk either.
        affiliated_run_start = if affiliated::is_affiliated_line(line) {
            affiliated_run_start.or(Some(line.start()))
        } else {
            None
        };
    }
    last_end
}

/// Resolve opener into a parse-ready shape (body extent measured once).
fn resolve(line: LineView, op: FootnoteOpener) -> FootnoteDefShape {
    FootnoteDefShape {
        opener: op,
        frame: frame::FrameShape {
            kind: OrgSyntaxKind::FOOTNOTE_DEFINITION,
            body_end: measure_body_end(line),
            body: frame::BodyMode::Elements,
            end_line: false,
        },
    }
}

/// Parse a footnote definition from its dispatcher prefilter.
///
/// Precondition: cursor at the definition line's first token; `op` from
/// [`opener`] on that line. Body extent is measured here (once), not in
/// `decide` / `interrupts_paragraph`. Body / blanks via
/// [`frame::parse_frame`] (no end line).
pub(crate) fn parse(parser: &mut Parser, op: FootnoteOpener) {
    // `op` can only come from `opener`, so the shape needs no re-measure here;
    // line-start is the one precondition the argument does not carry.
    let Some(line) = parser.line() else {
        debug_assert!(false, "footnote::parse requires line-start");
        return;
    };
    let shape = resolve(line, op);
    let header_tokens = shape.opener.header_tokens;

    frame::parse_frame(parser, shape.frame, |p| {
        p.eat_n(header_tokens);
        // Optional horizontal space before same-line first paragraph.
        if p.current() == OrgSyntaxKind::WHITESPACE {
            p.eat_any();
        }
        // Empty lead (`[fn:1]` alone on the line): consume the newline so the
        // contents loop starts at a line start, as list items do.
        if p.at_line_term() {
            p.eat(OrgSyntaxKind::NEWLINE);
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        make_parser, node_kinds, node_text, nodes_of_kind, parse_document_tree, probe_line,
        run_element, run_object, tree_text,
    };

    // --- Reference measure -------------------------------------------------

    /// Advance to the first `L_BRACKET` so column-0 definition rejection does
    /// not apply (mid-line / post-indent object entry).
    fn parser_at_bracket(src: &str) -> crate::parser::Parser<'_> {
        let mut p = make_parser(src);
        while p.current() != OrgSyntaxKind::EOF && p.current() != OrgSyntaxKind::L_BRACKET {
            p.eat_any();
        }
        p
    }

    #[test]
    fn reference_standard_shapes() {
        // Mid-line: prefix keeps the form out of column-0 definition territory.
        for body in ["[fn:1]", "[fn:label]", "[fn:a-b_c]", "[fn:α1]"] {
            let src = format!("x{body}");
            assert!(
                at_footnote_reference(&parser_at_bracket(&src)),
                "expected ref for {src:?}"
            );
        }
        for src in ["x[fn:]", "x[fn", "fn:1]", "x[[fn:1]]", "x[fn:1:unclosed"] {
            assert!(
                !at_footnote_reference(&parser_at_bracket(src)),
                "not a ref: {src:?}"
            );
        }
    }

    #[test]
    fn reference_inline_and_anonymous() {
        // Inline/anonymous forms are never definitions; column-0 is fine.
        assert!(at_footnote_reference(&make_parser("[fn:lbl:def]")));
        assert!(at_footnote_reference(&make_parser("[fn::anon]")));
        assert!(at_footnote_reference(&make_parser("[fn:1:see [nested]]")));
        assert!(at_footnote_reference(&make_parser("[fn:1:*bold*]")));
        // Unclosed inline
        assert!(!at_footnote_reference(&make_parser("[fn:1:no close")));
        assert!(!at_footnote_reference(&make_parser("[fn::")));
    }

    #[test]
    fn reference_measure_is_pure() {
        let p = parser_at_bracket("x[fn:1]");
        let before = p.cursor();
        assert!(matches!(
            probe(&p),
            Some(FootnoteRefShape::Standard { tokens: 5 })
        ));
        assert_eq!(p.cursor(), before);
    }

    #[test]
    fn column0_standard_is_not_a_reference() {
        // Unindented line-first: definition territory.
        let p = make_parser("[fn:1] rest\n");
        assert!(p.at_line_start());
        assert!(!at_footnote_reference(&p));
        // Indented: advance past INDENT onto `[`.
        let mut indented = make_parser("  [fn:1]\n");
        assert_eq!(indented.current(), OrgSyntaxKind::INDENT);
        indented.eat_any();
        assert!(at_footnote_reference(&indented));
        // Mid-line after text.
        let mut p2 = make_parser("x[fn:1]");
        p2.eat_any(); // TEXT "x"
        assert!(at_footnote_reference(&p2));
    }

    #[test]
    fn parse_standard_leaf_in_paragraph() {
        let src = "see [fn:1] now\n";
        let root = parse_document_tree(src);
        let refs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(tree_text(&refs[0]), "[fn:1]");
    }

    #[test]
    fn parse_inline_with_markup() {
        let src = "note [fn:lbl:*bold* text] end\n";
        let root = parse_document_tree(src);
        let refs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(tree_text(&refs[0]), "[fn:lbl:*bold* text]");
        let bold = nodes_of_kind(&refs[0], OrgSyntaxKind::BOLD);
        assert_eq!(bold.len(), 1, "inline def parses markup");
    }

    #[test]
    fn parse_anonymous() {
        let src = "a [fn::anonymous note] b\n";
        let root = parse_document_tree(src);
        let refs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(tree_text(&refs[0]), "[fn::anonymous note]");
    }

    #[test]
    fn nested_brackets_in_inline_def() {
        let src = "x [fn:1:see [a] and [b]] y\n";
        let root = parse_document_tree(src);
        let refs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(tree_text(&refs[0]), "[fn:1:see [a] and [b]]");
    }

    /// A multi-line inline body exercises `PairNewlines::Allow` in a full parse.
    #[test]
    fn multi_line_inline_footnote_reference() {
        let src = "note [fn::line1\nline2] after\n";
        let root = parse_document_tree(src);
        let refs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs.len(), 1, "one multi-line FOOTNOTE_REFERENCE");
        assert_eq!(
            tree_text(&refs[0]),
            "[fn::line1\nline2]",
            "reference span includes both lines"
        );
        assert!(
            tree_text(&refs[0]).contains('\n'),
            "body must span the newline (Allow closer)"
        );

        // Labeled multi-line form too.
        let labeled = "x [fn:lab:first\nsecond] y\n";
        let root_l = parse_document_tree(labeled);
        assert_eq!(tree_text(&root_l), labeled);
        let refs_l = nodes_of_kind(&root_l, OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(refs_l.len(), 1);
        assert_eq!(tree_text(&refs_l[0]), "[fn:lab:first\nsecond]");
    }

    #[test]
    fn unclosed_inline_stays_text() {
        let (_toks, evs, _) = run_object("[fn:1:no close\n", probe, parse_reference);
        assert!(!node_kinds(&evs).contains(&OrgSyntaxKind::FOOTNOTE_REFERENCE));
    }

    #[test]
    fn cookie_timestamp_link_coexist() {
        let src = "tasks [2/7] date [2026-06-24] link [[https://orgmode.org]] fn [fn:1]\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            1
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE).len(),
            1
        );
    }

    // --- Definition probe / parse ------------------------------------------

    #[test]
    fn definition_opener_header() {
        assert!(probe_line("[fn:1] text\n", opener).is_some());
        assert!(probe_line("[fn:label]\n", opener).is_some());
        assert!(probe_line("[fn:a-b_c] body\n", opener).is_some());
        // Indent rejected.
        assert!(probe_line("  [fn:1] text\n", opener).is_none());
        // Inline form is not a definition.
        assert!(probe_line("[fn:1:inline]\n", opener).is_none());
        assert!(probe_line("[fn::anon]\n", opener).is_none());
        // Empty label.
        assert!(probe_line("[fn:] text\n", opener).is_none());
    }

    #[test]
    fn definition_one_liner() {
        let src = "[fn:1] A short footnote.\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        assert_eq!(tree_text(&defs[0]), src);
        let paras = nodes_of_kind(&defs[0], OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1);
        assert!(tree_text(&paras[0]).contains("A short footnote"));
    }

    #[test]
    fn definition_terminated_by_next_def() {
        let src = "[fn:1] first\n[fn:2] second\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 2);
        assert_eq!(tree_text(&defs[0]), "[fn:1] first\n");
        assert_eq!(tree_text(&defs[1]), "[fn:2] second\n");
    }

    #[test]
    fn definition_terminated_by_heading() {
        let src = "[fn:1] body\n* Heading\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        assert_eq!(tree_text(&defs[0]), "[fn:1] body\n");
    }

    #[test]
    fn definition_terminated_by_high_star_task() {
        // Org footnote separator is any headline-re (`^\*+ `), so high-star
        // lines end the definition (not swallowed into the body).
        let high = "*".repeat(15);
        let src = format!("[fn:1] body\n{high} Task\n");
        let root = parse_document_tree(&src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        assert_eq!(tree_text(&defs[0]), "[fn:1] body\n");
        assert!(
            !tree_text(&defs[0]).contains("Task"),
            "high-star task must not be inside footnote body"
        );
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK).len(),
            1,
            "high-star line is a sibling INLINE_TASK"
        );
    }

    #[test]
    fn definition_single_blank_continues_double_terminates() {
        let src = "[fn:2] longer.\n\nStill inside.\n\n\nAfter.\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        let def_text = tree_text(&defs[0]);
        assert!(
            def_text.contains("Still inside"),
            "single blank continues: {def_text:?}"
        );
        assert!(
            def_text.ends_with("\n\n\n") || def_text.contains("Still inside.\n\n\n"),
            "double blank under definition: {def_text:?}"
        );
        assert!(
            !def_text.contains("After"),
            "content after double blank is outside: {def_text:?}"
        );
    }

    #[test]
    fn definition_mid_line_first_paragraph() {
        let src = "[fn:1] same line para\nnext line of para\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        let paras = nodes_of_kind(&defs[0], OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1);
        assert!(tree_text(&paras[0]).contains("same line para"));
        assert!(tree_text(&paras[0]).contains("next line of para"));
    }

    #[test]
    fn definition_with_nested_list() {
        let src = "[fn:1] lead\n- item a\n- item b\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 1);
        assert!(
            !nodes_of_kind(&defs[0], OrgSyntaxKind::PLAIN_LIST).is_empty(),
            "list nests inside definition"
        );
    }

    #[test]
    fn affiliated_before_next_def_stays_outside() {
        let src = "[fn:1] first\n#+CAPTION: for next\n[fn:2] second\n";
        let root = parse_document_tree(src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 2);
        assert_eq!(
            tree_text(&defs[0]),
            "[fn:1] first\n",
            "caption must not live under first def"
        );
        // CAPTION is a sibling keyword before the second def (zeroth section).
        let captions = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert!(
            captions.iter().any(|k| tree_text(k).contains("CAPTION")),
            "caption keyword present as sibling"
        );
    }

    #[test]
    fn definition_interrupts_paragraph() {
        let src = "para text\n[fn:1] note\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION).len(),
            1
        );
        // Paragraph does not swallow the definition line.
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert!(
            paras.iter().all(|p| !tree_text(p).contains("[fn:1]")),
            "definition not inside paragraph"
        );
    }

    #[test]
    fn column0_standard_is_definition_not_ref() {
        let src = "[fn:1] def only\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION).len(),
            1
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "column-0 standard is not a reference object"
        );
    }

    #[test]
    fn reference_inside_table_cell() {
        let src = "| a [fn:1] | b |\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE).len(),
            1
        );
    }

    #[test]
    fn reference_excluded_from_caption() {
        let src = "#+CAPTION: see [fn:1]\npara\n";
        let root = parse_document_tree(src);
        // CAPTION value must not form a footnote reference.
        let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert_eq!(kws.len(), 1);
        assert!(
            nodes_of_kind(&kws[0], OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "CAPTION excludes footnote references"
        );
    }

    #[test]
    fn reference_excluded_from_link_description() {
        let src = "[[https://orgmode.org][see [fn:1]]]\n";
        let root = parse_document_tree(src);
        let links = nodes_of_kind(&root, OrgSyntaxKind::LINK);
        assert_eq!(links.len(), 1);
        assert!(
            nodes_of_kind(&links[0], OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "link description excludes footnote references"
        );
    }

    #[test]
    fn run_element_empty_definition() {
        let src = "[fn:1]\n";
        let (_toks, evs, _) = run_element(src, opener, parse);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::FOOTNOTE_DEFINITION));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::FOOTNOTE_DEFINITION).as_deref(),
            Some("[fn:1]\n")
        );
    }

    #[test]
    fn definition_position_isolation_same_structure() {
        use std::fmt::Write;

        // Body measure must not depend on how much source *precedes* the
        // defs. A double blank ends the last def so trailing filler is not
        // absorbed (EOF would claim it).
        let defs = "[fn:1] a\n[fn:2] b\n\n\n";
        let mut filler = String::new();
        for i in 0..200 {
            let _ = writeln!(filler, "filler line {i}");
        }
        let top = format!("{defs}{filler}");
        let bottom = format!("{filler}{defs}");
        let root_top = parse_document_tree(&top);
        let root_bot = parse_document_tree(&bottom);
        let d_top = nodes_of_kind(&root_top, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        let d_bot = nodes_of_kind(&root_bot, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(d_top.len(), 2);
        assert_eq!(d_bot.len(), 2);
        assert_eq!(tree_text(&d_top[0]), tree_text(&d_bot[0]));
        assert_eq!(tree_text(&d_top[1]), tree_text(&d_bot[1]));
        assert_eq!(tree_text(&d_top[0]), "[fn:1] a\n");
        assert!(
            tree_text(&d_top[1]).starts_with("[fn:2] b\n"),
            "last def owns terminating blanks only, not filler"
        );
        assert!(!tree_text(&d_top[1]).contains("filler line"));
    }

    #[test]
    fn guarded_ref_parse() {
        // Mid-line so column-0 definition rejection does not apply.
        let (_toks, evs, _) = crate::tests::run("x[fn:xyz]", |p| {
            p.eat_any();
            if at_footnote_reference(p) {
                parse_footnote_reference(p);
            }
        });
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::FOOTNOTE_REFERENCE));
    }

    #[test]
    fn inline_footnote_on_column0_is_reference_in_paragraph() {
        // Inline form is never a definition; falls through to paragraph.
        let src = "[fn:1:inline def]\n";
        let root = parse_document_tree(src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION)
        );
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_REFERENCE).len(),
            1
        );
    }
}
