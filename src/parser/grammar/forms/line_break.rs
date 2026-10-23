//! Line break object: `\\` + optional spaces/tabs + end of line.
//!
//! Evidence:
//! - Worg 5.9 Line Breaks
//! - `org-element-line-break-parser`:
//!
//! ```elisp
//! (when (and (looking-at-p "\\\\\\\\[ \t]*$") (not (eq (char-before) ?\\)))
//!   ... :begin (point) :end (line-beginning-position 2) :post-blank 0)
//! ```
//!
//! **Extent includes the line terminator.** Org's `:end` is the start of the
//! next line with `:post-blank` 0, so the object covers `\\`, any trailing
//! spaces/tabs, and the newline. This is the one deliberate exception to the
//! rule that a content line's newline belongs to the enclosing element. Under
//! a region cap that hides the newline (a `#+CAPTION:` value, EOF) the node
//! stops at the cap, exactly as Emacs `$` matches at the end of a narrowed
//! region.
//!
//! **PRE is one byte.** The char before the first `\` must not be a backslash,
//! so in a run of backslashes ending a line only a run of exactly two is a
//! line break; three or more produce no object at all.
//!
//! **Backslash family** (`org-element--object-lex`): the dispatch branches on
//! the character after the backslash. `\\` reaches line-break and nothing
//! else - a miss yields no object rather than falling through to entity or
//! latex-fragment. See `forms/inline.rs` for the one-row backslash dispatch.
//!
//! Emits: a flat measured [`OrgSyntaxKind::LINE_BREAK`] leaf. No diagnostic:
//! org silently declines, and a stray `\\` mid-line is ordinary prose.
//!
//! [`probe`] is pure and returns the token count; [`parse`] emits the leaf
//! from it with [`Parser::leaf`].

use crate::parser::Parser;
use crate::syntax::OrgSyntaxKind;

/// Measured line break: tokens from the first `\` through the line terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineBreakShape {
    /// `\` `\` + optional WHITESPACE + optional NEWLINE.
    pub(crate) tokens: usize,
}

/// Pure measure of a line break at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<LineBreakShape> {
    let t = parser.here()?;
    if !t.run_of(OrgSyntaxKind::BACKSLASH, 2) {
        return None;
    }
    // PRE: `(not (eq (char-before) ?\\))`. Under a region cap the preceding
    // byte is the container's own delimiter, never a backslash, so reading the
    // absolute source here matches org's narrowed `char-before`.
    if parser.prev_byte() == Some(b'\\') {
        return None;
    }

    // `[ \t]*$`: a run of spaces/tabs mid-line is a single WHITESPACE tile.
    let mut i = 2usize;
    if t.kind_at(i) == OrgSyntaxKind::WHITESPACE {
        i += 1;
    }
    // `kind_at` answers EOF past the active cap, so a region end counts as `$`.
    match t.nth(i) {
        None => Some(LineBreakShape { tokens: i }),
        Some(next) if next.kind() == OrgSyntaxKind::NEWLINE => {
            // Org's `:end` is the next line's start: the newline is inside.
            Some(LineBreakShape { tokens: i + 1 })
        }
        Some(_) => None,
    }
}

/// Emit one line break from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: LineBreakShape) {
    parser.leaf(OrgSyntaxKind::LINE_BREAK, shape.tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::restrictions;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree, run_object, tree_text,
    };

    #[test]
    fn line_break_shapes_accept() {
        // (source, expected node text)
        let cases = [
            ("\\\\\n", "\\\\\n"),
            ("\\\\  \n", "\\\\  \n"),
            ("\\\\\t\n", "\\\\\t\n"),
            ("\\\\", "\\\\"),     // true EOF, no newline
            ("\\\\  ", "\\\\  "), // trailing spaces at EOF
        ];
        for (src, want) in cases {
            let shape =
                probe(&make_parser(src)).unwrap_or_else(|| panic!("expected line break: {src:?}"));
            let (_, evs, _) = run_object(src, probe, parse);
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::LINE_BREAK).as_deref(),
                Some(want),
                "node text for {src:?} (shape {shape:?})"
            );
        }
    }

    #[test]
    fn line_break_shapes_reject() {
        for src in [
            "\\\\x\n",  // material after the marker
            "\\\\ x\n", // material after the trailing space
            "\\x\n",    // single backslash
            "\\",       // single backslash at EOF
            "\\\\\\\n", // three backslashes: PRE fails on the pair at index 1
            "x\n",      // not a backslash at all
        ] {
            assert!(
                probe(&make_parser(src)).is_none(),
                "not a line break: {src:?}"
            );
        }
    }

    /// `(not (eq (char-before) ?\\))`: a pair preceded by a backslash is out,
    /// so any run of three or more backslashes at a line end yields nothing.
    #[test]
    fn backslash_run_pre_rejects_three_or_more() {
        for n in 3..=6usize {
            let src = format!("a{}\n", "\\".repeat(n));
            let root = parse_document_tree(&src);
            assert_eq!(tree_text(&root), src);
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
                "{n} backslashes must not produce a line break"
            );
        }
        // Exactly two still does.
        let src = "a\\\\\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).len(), 1);
    }

    #[test]
    fn line_break_probe_pure_and_parse_uses_shape() {
        let src = "\\\\ \n";
        let p = make_parser(src);
        let before = p.cursor();
        let shape = probe(&p).expect("line break");
        assert_eq!(shape.tokens, 4, "\\ \\ WHITESPACE NEWLINE");
        assert_eq!(p.cursor(), before, "probe must not advance");
    }

    #[test]
    fn line_break_in_paragraph_claims_the_newline() {
        let src = "one \\\\\ntwo\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let breaks = nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK);
        assert_eq!(breaks.len(), 1);
        assert_eq!(
            breaks[0].text().to_string(),
            "\\\\\n",
            "org `:end` is the next line start"
        );
        // Still one paragraph: `\\` does not separate.
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn line_break_alone_on_a_line() {
        // Worg prose says "otherwise non-blank line"; the parser has no such
        // check, so a line holding only `\\` is a line break.
        let src = "one\n\\\\\ntwo\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).len(), 1);
    }

    #[test]
    fn line_break_at_eof_without_newline() {
        let src = "one \\\\";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let breaks = nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK);
        assert_eq!(breaks.len(), 1);
        assert_eq!(breaks[0].text().to_string(), "\\\\");
    }

    /// `standard-set-no-line-break`: headline titles and item tags exclude it,
    /// as do table cells; paragraphs and CAPTION values keep it.
    #[test]
    fn line_break_scope_allowances() {
        assert!(!restrictions(OrgSyntaxKind::HEADING_TITLE).contains(OrgSyntaxKind::LINE_BREAK));

        let src = "* title \\\\\n- tag \\\\ :: body\n| cell \\\\ |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
            "no line break in titles, item tags, or cells"
        );

        // CAPTION values take the org `keyword` row, which keeps line-break.
        // The value region ends before the newline, so the node is just `\\`.
        let cap = "#+CAPTION: a \\\\\nA paragraph.\n";
        let root = parse_document_tree(cap);
        assert_eq!(tree_text(&root), cap);
        let breaks = nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK);
        assert_eq!(breaks.len(), 1);
        assert_eq!(breaks[0].text().to_string(), "\\\\");
    }

    /// The backslash row must not let a failed line break reach entity or latex.
    #[test]
    fn double_backslash_never_becomes_an_entity() {
        // `\\` in a heading title: line-break is not allowed there, and org
        // does not retry entity or latex-fragment on the `\\` branch.
        let src = "* t \\\\\n";
        let root = parse_document_tree(src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK)
        );
        assert_eq!(tree_text(&root), src);
    }
}
