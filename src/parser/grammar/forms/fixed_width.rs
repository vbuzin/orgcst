//! Fixed-width area element grammar: `: ...` lines.
//!
//! One or more consecutive fixed-width lines form a single FIXED_WIDTH node.
//! Leading INDENT is allowed. After `:`, the next byte must be space, line
//! end, or region/source end (tab or glued content disqualifies).
//!
//! Evidence: org-syntax section 4.3.6; `org-element-fixed-width-parser` and
//! the `org-element--current-element` recogniser `[ \t]*:\\( \\|$\\)`.
//!
//! Emits:
//!   FIXED_WIDTH
//!     (optional INDENT, COLON, body tokens, NEWLINE)+
//!     [post-blank tokens]
//!
//! Contract (element probe/parse):
//! - [`probe`] recognises one fixed-width line.
//! - [`parse`] is called at that line's first token; consumes every
//!   consecutive fixed-width line, then post-blank.

use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

use crate::parser::grammar::drive::atoms;
use atoms::{marker_line_probe, parse_marker_run};

/// One fixed-width line: content is `:` followed by space / line end.
pub(crate) fn probe(line: LineView) -> Option<()> {
    marker_line_probe(line, OrgSyntaxKind::COLON)
}

/// Parse one FIXED_WIDTH element: one or more consecutive fixed-width lines.
pub(crate) fn parse(parser: &mut Parser) {
    parse_marker_run(parser, OrgSyntaxKind::COLON, OrgSyntaxKind::FIXED_WIDTH);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::parser::grammar::forms::fixed_width::probe;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, parse_document_tree, probe_line, tree_text};

    // Marker-line behaviour shared with comment (space / line-end suffix,
    // indent tolerance, folding consecutive lines) is asserted once over both
    // forms in `drive/atoms.rs`. Only `:`-specific recognition lives here.

    #[test]
    fn glued_colon_belongs_to_a_neighbouring_construct() {
        // `:NAME:` is the drawer opener, not a fixed-width line.
        assert!(probe_line(":content\n", probe).is_none());
        assert!(probe_line(":DRAWER:\n", probe).is_none());
        assert!(probe_line(":PROPERTIES:\n", probe).is_none());
    }

    #[test]
    fn fixed_width_run_is_bounded_in_the_document_tree() {
        // The dispatcher path: the run claims both `:` lines and stops before
        // the paragraph, rather than swallowing the rest of the section.
        let src = ": a\n: b\npara\n";
        let root = parse_document_tree(src);
        let nodes = nodes_of_kind(&root, OrgSyntaxKind::FIXED_WIDTH);
        assert_eq!(nodes.len(), 1);
        assert_eq!(tree_text(&nodes[0]), ": a\n: b\n");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(),
            1,
            "the trailing line stays a paragraph"
        );
    }
}
