//! Comment element grammar: `# ...` lines.
//!
//! One or more consecutive comment lines form a single COMMENT node. Leading
//! INDENT is line trivia. After `#`, the next byte must be space, line end,
//! or region/source end (tab or glued content disqualifies - not a comment).
//!
//! Emits:
//!   COMMENT
//!     (optional INDENT, HASH, body tokens, NEWLINE)+
//!     [post-blank tokens]
//!
//! Contract (element probe/parse):
//! - [`probe`] recognises one comment line.
//! - [`parse`] is called at that line's first token; consumes every
//!   consecutive comment line, then post-blank.

use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

use crate::parser::grammar::drive::atoms;
use atoms::{marker_line_probe, parse_marker_run};

/// One comment line: content is `#` followed by space / line end.
pub(crate) fn probe(line: LineView) -> Option<()> {
    marker_line_probe(line, OrgSyntaxKind::HASH)
}

/// Parse one COMMENT element: one or more consecutive comment lines.
pub(crate) fn parse(parser: &mut Parser) {
    parse_marker_run(parser, OrgSyntaxKind::HASH, OrgSyntaxKind::COMMENT);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::parser::grammar::forms::comment::probe;
    use crate::tests::probe_line;

    // Marker-line behaviour shared with fixed-width (space / line-end suffix,
    // indent tolerance, folding consecutive lines) is asserted once over both
    // forms in `drive/atoms.rs`. Only `#`-specific recognition lives here.

    #[test]
    fn glued_hash_belongs_to_a_neighbouring_construct() {
        // `#+` is the keyword / block prefix, not a comment.
        assert!(probe_line("#+TITLE: keyword, not a comment\n", probe).is_none());
        assert!(probe_line("#foo\n", probe).is_none());
        assert!(probe_line("#!shebang-ish\n", probe).is_none());
    }
}
