//! Export snippet object: `@@BACKEND:VALUE@@`.
//!
//! Evidence:
//! - Worg 5.3 Export Snippets
//! - `org-element-export-snippet-parser`: `@@\([-A-Za-z0-9]+\):` then
//!   `(re-search-forward "@@" nil t)` for the closer.
//!
//! The closer search is unbounded inside the element's narrowed contents, so a
//! value may cross line boundaries; here the active region cap plays the part
//! of the narrowing. The value is opaque (export snippet is not in
//! `org-element-recursive-objects`) and may hold single `@` bytes - only the
//! first `@@` pair after the colon closes.
//!
//! Emits: a flat measured [`OrgSyntaxKind::EXPORT_SNIPPET`] leaf over the fixed
//! token layout `AT AT TEXT COLON ... AT AT`. No decomposition kinds: the typed
//! view derives back-end and value ranges from that layout, the way
//! `FOOTNOTE_REFERENCE` derives its label.
//!
//! [`probe`] is pure. A complete snippet carries its exclusive end. An opener
//! with no closer still claims (`end: None`), so emit records
//! [`DiagnosticKind::UnterminatedExportSnippet`] and leaves the two `@` tokens
//! bare, as an inline babel call does when its layout is `None`. Org itself is
//! silent here; the diagnostic reports the near miss the way
//! `UnterminatedLink` does.

use crate::DiagnosticKind;
use crate::parser::grammar::drive::atoms;
use crate::parser::{Parser, Pos};
use crate::syntax::OrgSyntaxKind;

/// Tokens of the committed opener: `@` `@` BACKEND `:`.
const OPEN_TOKENS: usize = 4;

/// Measured export snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExportSnippetShape {
    /// Exclusive end past the closing `@@`; `None` when unterminated.
    pub(crate) end: Option<Pos>,
}

/// Pure measure of an export snippet opener at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<ExportSnippetShape> {
    let t = parser.here()?;
    if !t.run_of(OrgSyntaxKind::AT, 2) {
        return None;
    }
    if t.kind_at(2) != OrgSyntaxKind::TEXT || !atoms::is_export_backend(t.text_at(2)) {
        return None;
    }
    if t.kind_at(3) != OrgSyntaxKind::COLON {
        return None;
    }

    // First `@@` after the colon closes; NEWLINE is ordinary interior. `nth`
    // is bounded by the active cap, so the scan cannot run past the element.
    let mut i = OPEN_TOKENS;
    let end = loop {
        let Some(tok) = t.nth(i) else { break None };
        if tok.kind() == OrgSyntaxKind::AT && t.kind_at(i + 1) == OrgSyntaxKind::AT {
            break t.nth(i + 1).map(|second| second.end());
        }
        i += 1;
    };
    Some(ExportSnippetShape { end })
}

/// Emit one export snippet from a measured shape (no remeasure).
///
/// Unterminated openers record a diagnostic and consume the bare `@@` so the
/// inline loop advances; the rest of the line stays ordinary text.
pub(crate) fn parse(parser: &mut Parser, shape: ExportSnippetShape) {
    if let Some(end) = shape.end {
        parser.leaf_to(OrgSyntaxKind::EXPORT_SNIPPET, end);
    } else {
        parser.error_ahead(OPEN_TOKENS, DiagnosticKind::UnterminatedExportSnippet);
        parser.eat_n(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        diags_any, make_parser, node_text, nodes_of_kind, parse_document_tree, run_object,
        tree_text,
    };

    #[test]
    fn export_snippet_shapes_accept() {
        for (src, want) in [
            ("@@html:<b>x</b>@@", "@@html:<b>x</b>@@"),
            ("@@html:@@", "@@html:@@"),
            ("@@my-backend:v@@", "@@my-backend:v@@"),
            ("@@a1:v@@", "@@a1:v@@"),
            ("@@a:b@c@@", "@@a:b@c@@"),
            // Only the first pair closes; the trailing `@@` is outside.
            ("@@a:v@@ tail @@", "@@a:v@@"),
            // Three `@` at the close: the first pair wins.
            ("@@a:v@@@", "@@a:v@@"),
        ] {
            let (_, evs, _) = run_object(src, probe, parse);
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::EXPORT_SNIPPET).as_deref(),
                Some(want),
                "node text for {src:?}"
            );
        }
    }

    #[test]
    fn export_snippet_shapes_reject() {
        for src in [
            "@@:v@@",    // empty back-end
            "@@a.b:v@@", // `.` is not in [-A-Za-z0-9]
            "@@a_b:v@@", // `_` breaks the back-end tile
            "@@a v:x@@", // space in the back-end
            "@@a@@",     // no colon
            "@a:v@@",    // single opening `@`
            "plain",
        ] {
            assert!(
                probe(&make_parser(src)).is_none(),
                "not an export snippet: {src:?}"
            );
        }
    }

    #[test]
    fn export_snippet_probe_pure_and_parse_uses_shape() {
        let src = "@@html:v@@";
        let p = make_parser(src);
        let before = p.cursor();
        let shape = probe(&p).expect("snippet");
        assert!(shape.end.is_some());
        assert_eq!(p.cursor(), before, "probe must not advance");
    }

    /// The closer search is bounded only by the container, so a value may span
    /// lines inside one paragraph.
    #[test]
    fn export_snippet_value_may_cross_lines() {
        let src = "a @@html:one\ntwo@@ b\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let snippets = nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].text().to_string(), "@@html:one\ntwo@@");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn unterminated_export_snippet_diagnoses_and_progresses() {
        let src = "a @@html:no closer\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET)
        );

        // The committed opener is the diagnostic range; `@@a:` at exact EOF is
        // the tight case (no token after the colon).
        for src in ["@@html:no closer", "@@a:"] {
            let (_, evs, diags) = run_object(src, probe, parse);
            assert!(
                diags_any(&diags, |k| matches!(
                    k,
                    DiagnosticKind::UnterminatedExportSnippet
                )),
                "expected an unterminated-snippet diagnostic for {src:?}"
            );
            assert!(node_text(&evs, src, OrgSyntaxKind::EXPORT_SNIPPET).is_none());
        }
    }

    /// A capped region hides a closer that lives outside it: a `#+CAPTION:`
    /// value stops at the line end, so a cross-line snippet cannot form there.
    #[test]
    fn export_snippet_respects_the_region_cap() {
        let src = "#+CAPTION: a @@html:v\nmore@@\nA paragraph.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET).is_empty(),
            "the closer is past the keyword value cap"
        );
    }

    /// Org's `link` and `table-cell` rows both list export-snippet.
    #[test]
    fn export_snippet_in_link_description_and_cell() {
        let src = "[[https://x.com][d @@html:<b>@@]]\n| c @@html:v@@ |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        for scope in [OrgSyntaxKind::LINK_DESCRIPTION, OrgSyntaxKind::TABLE_CELL] {
            let scopes = nodes_of_kind(&root, scope);
            assert_eq!(scopes.len(), 1, "one {scope:?}");
            assert_eq!(
                nodes_of_kind(&scopes[0], OrgSyntaxKind::EXPORT_SNIPPET).len(),
                1,
                "export snippet inside {scope:?}"
            );
        }
    }

    /// A rejected opener leaves ordinary text, and the `_` in it is still an
    /// ordinary subscript - the same as org, which finds that candidate on the
    /// next pass of its object search.
    #[test]
    fn rejected_openers_leave_plain_text() {
        for src in ["@@a.b:v@@\n", "@@:v@@\n", "@@a@@\n"] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "roundtrip {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET).is_empty(),
                "{:?}",
                nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET)
            );
        }

        let src = "@@a_b:v@@\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET)
        );
        let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
        assert_eq!(subs.len(), 1, "the `_` is still a subscript candidate");
        assert_eq!(subs[0].text().to_string(), "_b");
    }

    #[test]
    fn export_snippet_value_stays_opaque() {
        let src = "@@html:*not bold* [[a]]@@\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::EXPORT_SNIPPET).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::BOLD)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINK)
        );
    }
}
