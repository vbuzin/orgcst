//! List item cookies, counters, tags, and marker-stripped values.

use super::{child_lines, is_horizontal_ws, trim_trailing_blank_lines};
use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode, SyntaxToken};

// ---------------------------------------------------------------------------
// Marker-stripped value (comment / fixed-width)
// ---------------------------------------------------------------------------

/// Marker-stripped interiors of a folded comment / fixed-width run.
///
/// Per line skips INDENT, `marker`, and one conventional space (a following
/// WHITESPACE token that begins with ` `). Interiors join with `\n`; the
/// node's trailing blanks are omitted. Token-kind skip, not a re-parse.
#[must_use]
pub(crate) fn marker_stripped_value(node: &SyntaxNode, marker: OrgSyntaxKind) -> String {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut lines = child_lines(&kids);
    trim_trailing_blank_lines(&mut lines);
    let mut out = String::new();
    for (n, line) in lines.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        append_marker_stripped_line(&mut out, line, marker);
    }
    out
}

fn append_marker_stripped_line(out: &mut String, line: &[SyntaxElement], marker: OrgSyntaxKind) {
    let mut rest = line;
    if rest
        .first()
        .is_some_and(|e| e.kind() == OrgSyntaxKind::INDENT)
    {
        rest = &rest[1..];
    }
    if rest.first().is_some_and(|e| e.kind() == marker) {
        rest = &rest[1..];
        if let Some(tok) = rest.first().and_then(SyntaxElement::as_token)
            && tok.kind() == OrgSyntaxKind::WHITESPACE
            && let Some(ws) = tok.text().strip_prefix(' ')
        {
            out.push_str(ws);
            rest = &rest[1..];
        }
    }
    for e in rest {
        if e.kind() == OrgSyntaxKind::NEWLINE {
            break;
        }
        if let Some(tok) = e.as_token() {
            out.push_str(tok.text());
        }
    }
}

/// Interior token of a `CHECKBOX` node (` `, `X`, or `-`).
#[must_use]
pub(crate) fn checkbox_interior_token(checkbox: &SyntaxNode) -> Option<SyntaxToken> {
    let mut toks = checkbox
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token);
    let open = toks.next()?;
    if open.kind() != OrgSyntaxKind::L_BRACKET {
        return None;
    }
    let interior = toks.next()?;
    let close = toks.next()?;
    (close.kind() == OrgSyntaxKind::R_BRACKET).then_some(interior)
}

/// Value token of a `COUNTER` cookie: `N` in `[@N]` / `[@c]`, or `N` in
/// `[@start:N]`. `None` on a pathological tile.
#[must_use]
pub(crate) fn counter_value_token(counter: &SyntaxNode) -> Option<SyntaxToken> {
    let toks: Vec<SyntaxToken> = counter
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .collect();
    match toks.as_slice() {
        [lb, at, _start, col, value, rb]
            if lb.kind() == OrgSyntaxKind::L_BRACKET
                && at.kind() == OrgSyntaxKind::AT
                && col.kind() == OrgSyntaxKind::COLON
                && rb.kind() == OrgSyntaxKind::R_BRACKET =>
        {
            Some(value.clone())
        }
        [lb, at, value, rb]
            if lb.kind() == OrgSyntaxKind::L_BRACKET
                && at.kind() == OrgSyntaxKind::AT
                && rb.kind() == OrgSyntaxKind::R_BRACKET =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

/// Removal span of an `ITEM_TAG`: the tag plus the preceding separator
/// whitespace and the following ` :: ` tokens.
#[must_use]
pub(crate) fn item_tag_removal_range(item: &SyntaxNode, tag: TextRange) -> TextRange {
    let kids: Vec<SyntaxElement> = item.children_with_tokens().collect();
    let Some(idx) = kids.iter().position(|e| e.text_range() == tag) else {
        return tag;
    };
    let mut start = tag.start();
    let mut end = tag.end();
    if idx > 0 && is_horizontal_ws(&kids[idx - 1]) {
        start = kids[idx - 1].text_range().start();
    }
    let mut i = idx + 1;
    if kids.get(i).is_some_and(is_horizontal_ws) {
        end = kids[i].text_range().end();
        i += 1;
    }
    if kids
        .get(i)
        .is_some_and(|e| e.kind() == OrgSyntaxKind::COLON)
        && kids
            .get(i + 1)
            .is_some_and(|e| e.kind() == OrgSyntaxKind::COLON)
    {
        end = kids[i + 1].text_range().end();
    }
    TextRange::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::{first_kind, synth};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    #[allow(clippy::too_many_lines)] // one marker-strip case table
    fn marker_stripped_value_hash_and_colon_branches() {
        use OrgSyntaxKind::{COLON, COMMENT, FIXED_WIDTH, HASH, NEWLINE, PARAGRAPH, TEXT};

        assert_eq!(
            marker_stripped_value(&first_kind("# hello\n", COMMENT), HASH),
            "hello"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": hello\n", FIXED_WIDTH), COLON),
            "hello"
        );

        // Grammar will not emit COMMENT/FIXED_WIDTH for a glued marker; the
        // helper still must not eat the first body character.
        assert_eq!(
            marker_stripped_value(&first_kind("#hello\n", PARAGRAPH), HASH),
            "hello"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(":hello\n", PARAGRAPH), COLON),
            "hello"
        );
        let glued = synth(COMMENT, &[(HASH, "#"), (TEXT, "hello"), (NEWLINE, "\n")]);
        assert_eq!(marker_stripped_value(&glued, HASH), "hello");
        let glued = synth(
            FIXED_WIDTH,
            &[(COLON, ":"), (TEXT, "hello"), (NEWLINE, "\n")],
        );
        assert_eq!(marker_stripped_value(&glued, COLON), "hello");

        assert_eq!(
            marker_stripped_value(&first_kind("#  two spaces\n", COMMENT), HASH),
            " two spaces"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(":  two spaces\n", FIXED_WIDTH), COLON),
            " two spaces"
        );

        assert_eq!(
            marker_stripped_value(&first_kind("# a\n# b\n", COMMENT), HASH),
            "a\nb"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": a\n: b\n", FIXED_WIDTH), COLON),
            "a\nb"
        );
        assert_eq!(
            marker_stripped_value(&first_kind("# a\r\n# b\r\n", COMMENT), HASH),
            "a\nb"
        );

        assert_eq!(
            marker_stripped_value(&first_kind("# a\n\n", COMMENT), HASH),
            "a"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": a\n\n", FIXED_WIDTH), COLON),
            "a"
        );

        assert_eq!(marker_stripped_value(&first_kind("#\n", COMMENT), HASH), "");
        assert_eq!(
            marker_stripped_value(&first_kind("# \n", COMMENT), HASH),
            ""
        );
        assert_eq!(
            marker_stripped_value(&first_kind(":\n", FIXED_WIDTH), COLON),
            ""
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": \n", FIXED_WIDTH), COLON),
            ""
        );
        assert_eq!(
            marker_stripped_value(&first_kind("  # indented\n", COMMENT), HASH),
            "indented"
        );

        // Unterminated last line (true EOF). A forgotten remainder push
        // would yield "" for a single line and drop the last folded line.
        assert_eq!(
            marker_stripped_value(&first_kind("# hello", COMMENT), HASH),
            "hello"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": hello", FIXED_WIDTH), COLON),
            "hello"
        );
        assert_eq!(marker_stripped_value(&first_kind("#", COMMENT), HASH), "");
        assert_eq!(
            marker_stripped_value(&first_kind(":", FIXED_WIDTH), COLON),
            ""
        );
        assert_eq!(
            marker_stripped_value(&first_kind("# a\n# b", COMMENT), HASH),
            "a\nb"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": a\n: b", FIXED_WIDTH), COLON),
            "a\nb"
        );

        // Tab after the marker is WHITESPACE that is not space-prefixed;
        // grammar rejects it as COMMENT/FIXED_WIDTH. Keep the tab in the body.
        assert_eq!(
            marker_stripped_value(&first_kind("#\thello\n", PARAGRAPH), HASH),
            "\thello"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(":\thello\n", PARAGRAPH), COLON),
            "\thello"
        );

        assert_eq!(
            marker_stripped_value(&first_kind("# see #tag\n", COMMENT), HASH),
            "see #tag"
        );
        assert_eq!(
            marker_stripped_value(&first_kind(": key: value\n", FIXED_WIDTH), COLON),
            "key: value"
        );
    }
    #[test]
    fn counter_value_token_short_long_and_overflow_tile() {
        let node = first_kind("- [@20] x\n", OrgSyntaxKind::COUNTER);
        assert_eq!(counter_value_token(&node).unwrap().text(), "20");

        let node = first_kind("- [@start:20] x\n", OrgSyntaxKind::COUNTER);
        assert_eq!(counter_value_token(&node).unwrap().text(), "20");

        let node = first_kind("- [@4294967296] x\n", OrgSyntaxKind::COUNTER);
        assert_eq!(counter_value_token(&node).unwrap().text(), "4294967296");
    }
    #[test]
    fn checkbox_interior_token_three_states() {
        let node = first_kind("- [ ] a\n", OrgSyntaxKind::CHECKBOX);
        assert_eq!(checkbox_interior_token(&node).unwrap().text(), " ");

        let node = first_kind("- [X] a\n", OrgSyntaxKind::CHECKBOX);
        assert_eq!(checkbox_interior_token(&node).unwrap().text(), "X");

        let node = first_kind("- [-] a\n", OrgSyntaxKind::CHECKBOX);
        assert_eq!(checkbox_interior_token(&node).unwrap().text(), "-");
    }
    #[test]
    fn item_tag_removal_covers_separator() {
        let src = "- tag :: def\n";
        let item = first_kind(src, OrgSyntaxKind::ITEM);
        let tag = item
            .children()
            .find(|c| c.kind() == OrgSyntaxKind::ITEM_TAG)
            .expect("tag");
        let removal = item_tag_removal_range(&item, tag.text_range());
        let doc = parse(src);
        assert_eq!(&doc.source()[removal], " tag ::");

        let src = "- term  ::  def\n";
        let item = first_kind(src, OrgSyntaxKind::ITEM);
        let tag = item
            .children()
            .find(|c| c.kind() == OrgSyntaxKind::ITEM_TAG)
            .expect("tag");
        let removal = item_tag_removal_range(&item, tag.text_range());
        let doc = parse(src);
        assert!(doc.source()[removal].contains("::"));
        assert!(!doc.source()[removal].contains("def"));
    }
    #[test]
    fn counter_value_token_pathological_tile_is_none() {
        let node = synth(OrgSyntaxKind::COUNTER, &[(OrgSyntaxKind::TEXT, "[@")]);
        assert!(counter_value_token(&node).is_none());
    }
}
