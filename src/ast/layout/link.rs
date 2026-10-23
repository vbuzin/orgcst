//! Link and angle-run token walks.

use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

/// Delimiter class of a `LINK` node (direct child tokens only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkOpeners {
    Bracket,
    Angle,
    Plain,
}

/// Bracket: first two child tokens are `[[`. Angle: first child token is `<`.
/// Plain: neither (equivalently `LINK.range() == LINK_PATH.range()`).
#[must_use]
pub(crate) fn link_openers(link: &SyntaxNode) -> LinkOpeners {
    let mut toks = link
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token);
    match (toks.next(), toks.next()) {
        (Some(a), Some(b))
            if a.kind() == OrgSyntaxKind::L_BRACKET && b.kind() == OrgSyntaxKind::L_BRACKET =>
        {
            LinkOpeners::Bracket
        }
        (Some(a), _) if a.kind() == OrgSyntaxKind::L_ANGLE => LinkOpeners::Angle,
        _ => LinkOpeners::Plain,
    }
}

/// Removal of a bracket description: the `][` pair plus `LINK_DESCRIPTION`.
///
/// Delete yields `[[path]]`. `None` when the description child is missing.
#[must_use]
pub(crate) fn link_description_removal(link: &SyntaxNode) -> Option<TextRange> {
    let kids: Vec<_> = link.children_with_tokens().collect();
    let path_idx = kids
        .iter()
        .position(|e| e.kind() == OrgSyntaxKind::LINK_PATH)?;
    let desc = kids
        .iter()
        .find(|e| e.kind() == OrgSyntaxKind::LINK_DESCRIPTION)?;
    let start = kids[path_idx + 1..]
        .iter()
        .find(|e| e.kind() == OrgSyntaxKind::R_BRACKET)?
        .text_range()
        .start();
    Some(TextRange::new(start, desc.text_range().end()))
}

/// Interior after `n` leading `L_ANGLE` tokens and before `n` trailing
/// `R_ANGLE` tokens (`<<name>>` is n=2, `<<<contents>>>` is n=3).
#[must_use]
pub(crate) fn angle_run_interior(node: &SyntaxNode, n: usize) -> Option<TextRange> {
    if n == 0 {
        return None;
    }
    let kids: Vec<_> = node.children_with_tokens().collect();
    if kids.len() < 2 * n {
        return None;
    }
    if !kids[..n].iter().all(|e| e.kind() == OrgSyntaxKind::L_ANGLE) {
        return None;
    }
    if !kids[kids.len() - n..]
        .iter()
        .all(|e| e.kind() == OrgSyntaxKind::R_ANGLE)
    {
        return None;
    }
    let start = kids[n - 1].text_range().end();
    let end = kids[kids.len() - n].text_range().start();
    (start <= end).then(|| TextRange::new(start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn link_openers_bracket_angle_plain() {
        let br = first_kind("[[path][site]]\n", OrgSyntaxKind::LINK);
        assert_eq!(link_openers(&br), LinkOpeners::Bracket);
        let ang = first_kind("<https://ex.com>\n", OrgSyntaxKind::LINK);
        assert_eq!(link_openers(&ang), LinkOpeners::Angle);
        let plain = first_kind("https://ex.com\n", OrgSyntaxKind::LINK);
        assert_eq!(link_openers(&plain), LinkOpeners::Plain);
    }
    #[test]
    fn link_description_removal_includes_separator() {
        let src = "[[path][site]]\n";
        let link = first_kind(src, OrgSyntaxKind::LINK);
        let doc = parse(src);
        let rm = link_description_removal(&link).expect("desc");
        assert_eq!(&doc.source()[rm], "][site");
        let empty = first_kind("[[t][]]\n", OrgSyntaxKind::LINK);
        let doc = parse("[[t][]]\n");
        assert_eq!(
            &doc.source()[link_description_removal(&empty).expect("empty")],
            "]["
        );
        let none = first_kind("[[path]]\n", OrgSyntaxKind::LINK);
        assert!(link_description_removal(&none).is_none());
    }
    #[test]
    fn angle_run_interior_target_and_radio() {
        let t = first_kind("<<name>>\n", OrgSyntaxKind::TARGET);
        let doc = parse("<<name>>\n");
        assert_eq!(
            &doc.source()[angle_run_interior(&t, 2).expect("name")],
            "name"
        );
        let r = first_kind("<<<foo bar>>>\n", OrgSyntaxKind::RADIO_TARGET);
        let doc = parse("<<<foo bar>>>\n");
        assert_eq!(
            &doc.source()[angle_run_interior(&r, 3).expect("key")],
            "foo bar"
        );
        assert!(angle_run_interior(&t, 3).is_none());
        assert!(angle_run_interior(&t, 0).is_none());
    }
}
