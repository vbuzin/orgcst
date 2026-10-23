//! Drawer body range (end-name line excluded).

use super::{line_start_backing_indent, token_text_eq_caseless};
use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode};

// ---------------------------------------------------------------------------
// Drawer body (closer-anchored)
// ---------------------------------------------------------------------------

/// Body span of a `DRAWER` or `PROPERTY_DRAWER`: after the opener line,
/// before the `:END:` line. Empty drawer: zero-width at end of first line.
///
/// Anchors on the closer (`COLON` + `TEXT` caseless `end`), not on
/// `line_partition`'s last line, so drawer-owned post-blank is excluded.
/// The closer is `eat_line` tokens; the grammar does not carve
/// `DRAWER_NAME` on `:END:`. First closer after the opener line wins
/// (same first-`:END:`-wins as recognition).
#[must_use]
pub(crate) fn drawer_body_range(node: &SyntaxNode) -> Option<TextRange> {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let first_nl = kids
        .iter()
        .position(|e| e.kind() == OrgSyntaxKind::NEWLINE)?;
    let body_start = kids[first_nl].text_range().end();
    // Skip the opener line so a drawer named END cannot steal the closer.
    for i in (first_nl + 1)..kids.len() {
        if kids[i].kind() != OrgSyntaxKind::COLON {
            continue;
        }
        let Some(name) = kids.get(i + 1) else {
            continue;
        };
        if !is_drawer_end_name(name) {
            continue;
        }
        let body_end = line_start_backing_indent(&kids[i]);
        if body_end < body_start {
            return None;
        }
        return Some(TextRange::new(body_start, body_end));
    }
    None
}

fn is_drawer_end_name(elem: &SyntaxElement) -> bool {
    token_text_eq_caseless(elem, "end")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn drawer_body_range_excludes_owned_post_blank() {
        let src = ":NOTES:\nitem\n:END:\n\n";
        let drawer = first_kind(src, OrgSyntaxKind::DRAWER);
        let body = drawer_body_range(&drawer).expect("body");
        let doc = parse(src);
        assert_eq!(&doc.source()[body], "item\n");
        assert!(
            !&doc.source()[body].contains(":END:"),
            "closer must stay outside the body"
        );

        let empty = first_kind(":NOTES:\n:END:\n\n", OrgSyntaxKind::DRAWER);
        let body = drawer_body_range(&empty).expect("empty body");
        assert!(body.is_empty());

        let props = first_kind(
            "* H\n:PROPERTIES:\n:FOO: 1\n:END:\n",
            OrgSyntaxKind::PROPERTY_DRAWER,
        );
        let body = drawer_body_range(&props).expect("property body");
        let doc = parse("* H\n:PROPERTIES:\n:FOO: 1\n:END:\n");
        assert_eq!(&doc.source()[body], ":FOO: 1\n");

        // Opener named END must not steal the closer (search starts after
        // the first line).
        let named = first_kind(":END:\nitem\n:END:\n", OrgSyntaxKind::DRAWER);
        let body = drawer_body_range(&named).expect("named-end body");
        let doc = parse(":END:\nitem\n:END:\n");
        assert_eq!(&doc.source()[body], "item\n");

        let indented = first_kind(":NOTES:\nitem\n  :END:\n", OrgSyntaxKind::DRAWER);
        let body = drawer_body_range(&indented).expect("indented closer");
        let doc = parse(":NOTES:\nitem\n  :END:\n");
        assert_eq!(&doc.source()[body], "item\n");
        assert!(
            !&doc.source()[body].ends_with("  "),
            "indent before :END: is not body"
        );
    }
}
