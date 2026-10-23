//! Links and checkboxes.

use super::SynthesisError;
use crate::ast::{CheckboxState, LinkShape};
use crate::escape_link_path;

pub(crate) fn render_link(
    shape: LinkShape,
    path: &str,
    desc: Option<&str>,
) -> Result<String, SynthesisError> {
    if shape == LinkShape::Radio {
        return Err(SynthesisError::InvalidLinkShape);
    }
    if path.is_empty() || path.contains(['\n', '\r']) {
        return Err(SynthesisError::InvalidLinkPath);
    }
    if desc.is_some_and(invalid_link_description) {
        return Err(SynthesisError::InvalidLinkPath);
    }
    match shape {
        LinkShape::Bracket => Ok(match desc {
            Some(d) => format!("[[{}][{d}]]", escape_link_path(path)),
            None => format!("[[{}]]", escape_link_path(path)),
        }),
        LinkShape::Angle => {
            if desc.is_some() || path.contains('>') {
                return Err(SynthesisError::InvalidLinkPath);
            }
            Ok(format!("<{path}>"))
        }
        LinkShape::Plain => {
            if desc.is_some() || plain_path_reparses(path) {
                return Err(SynthesisError::InvalidLinkPath);
            }
            Ok(path.to_string())
        }
        LinkShape::Radio => unreachable!("checked above"),
    }
}

pub(crate) fn render_checkbox(state: CheckboxState) -> String {
    match state {
        CheckboxState::Unchecked => "[ ]".to_string(),
        CheckboxState::Checked => "[X]".to_string(),
        CheckboxState::Partial => "[-]".to_string(),
    }
}

/// True when a bracket description plus the existing closer `]]` would close
/// early: newline, a literal `]]` in the body, or a trailing `]` that joins
/// the first closer byte.
pub(crate) fn invalid_link_description(d: &str) -> bool {
    d.contains(['\n', '\r']) || d.contains("]]") || d.ends_with(']')
}

/// Separator plus interior for inserting a description on a bracket link
/// that has none (`[[path]]` -> `[[path][d]]`).
pub(crate) fn render_link_description_insert(d: &str) -> Result<String, SynthesisError> {
    if invalid_link_description(d) {
        return Err(SynthesisError::InvalidLinkPath);
    }
    Ok(format!("][{d}"))
}

/// True when a plain-link path would re-parse as other syntax: `*` or `#+`
/// after leading blanks, or `[[` or `<` anywhere.
fn plain_path_reparses(path: &str) -> bool {
    let rest = path.trim_start_matches([' ', '\t']);
    rest.starts_with('*') || rest.starts_with("#+") || path.contains("[[") || path.contains('<')
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::parse;

    #[test]
    #[allow(clippy::too_many_lines)] // one link recipe table
    fn link_recipes() {
        assert_eq!(
            render_link(LinkShape::Bracket, "https://example.com", None).unwrap(),
            "[[https://example.com]]"
        );
        assert_eq!(
            render_link(LinkShape::Bracket, "https://example.com", Some("desc")).unwrap(),
            "[[https://example.com][desc]]"
        );
        assert_eq!(
            render_link(LinkShape::Bracket, "a[b]", None).unwrap(),
            "[[a\\[b\\]]]"
        );
        assert_eq!(
            render_link(LinkShape::Angle, "https://y.com", None).unwrap(),
            "<https://y.com>"
        );
        assert_eq!(
            render_link(LinkShape::Plain, "https://x.com", None).unwrap(),
            "https://x.com"
        );
        assert!(matches!(
            render_link(LinkShape::Radio, "phrase", None),
            Err(SynthesisError::InvalidLinkShape)
        ));
        assert!(matches!(
            render_link(LinkShape::Bracket, "a\nb", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Angle, "a>b", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Bracket, "t", Some("see ]] here")),
            Err(SynthesisError::InvalidLinkPath)
        ));
        for desc in ["See [1]", "foo]", "]"] {
            assert!(
                matches!(
                    render_link(LinkShape::Bracket, "t", Some(desc)),
                    Err(SynthesisError::InvalidLinkPath)
                ),
                "{desc:?} joins the closer"
            );
        }
        assert_eq!(
            render_link(LinkShape::Bracket, "t", Some("a]b")).unwrap(),
            "[[t][a]b]]"
        );
        assert_eq!(render_link_description_insert("site").unwrap(), "][site");
        assert_eq!(render_link_description_insert("").unwrap(), "][");
        assert_eq!(render_link_description_insert("a]b").unwrap(), "][a]b");
        assert!(matches!(
            render_link_description_insert("foo]"),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "https://x.com", Some("d")),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Angle, "https://y.com", Some("d")),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Bracket, "", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Angle, "", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(
                LinkShape::Bracket,
                "https://example.com",
                Some("foo\n* Injected\nbar")
            ),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(
                LinkShape::Bracket,
                "https://example.com",
                Some("foo\r#+TITLE: x")
            ),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert_eq!(
            render_link(LinkShape::Angle, "a[b]", None).unwrap(),
            "<a[b]>"
        );
        assert_eq!(render_link(LinkShape::Plain, "a[b]", None).unwrap(), "a[b]");
        assert!(matches!(
            render_link(LinkShape::Plain, "* Injected", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "#+TITLE: x", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "[[https://evil.com][phish]]", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "<https://x.com>", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, " [[https://evil.com][phish]]", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, " <https://x.com>", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(
                LinkShape::Plain,
                "https://x.com [[https://evil][click]]",
                None
            ),
            Err(SynthesisError::InvalidLinkPath)
        ));
        assert!(matches!(
            render_link(LinkShape::Plain, "https://x.com <https://evil>", None),
            Err(SynthesisError::InvalidLinkPath)
        ));
    }

    #[test]
    fn link_parse_stability() {
        let doc = parse(&format!(
            "{}\n",
            render_link(LinkShape::Bracket, "https://example.com", Some("desc")).unwrap()
        ));
        super::super::equal::assert_link_eq(
            &doc,
            LinkShape::Bracket,
            "https://example.com",
            Some("desc"),
        );
        let doc = parse(&format!(
            "{}\n",
            render_link(LinkShape::Angle, "https://y.com", None).unwrap()
        ));
        super::super::equal::assert_link_eq(&doc, LinkShape::Angle, "https://y.com", None);
        let doc = parse(&format!(
            "{}\n",
            render_link(LinkShape::Plain, "https://x.com", None).unwrap()
        ));
        super::super::equal::assert_link_eq(&doc, LinkShape::Plain, "https://x.com", None);
        let doc = parse(&format!(
            "{}\n",
            render_link(LinkShape::Bracket, "a[b]", None).unwrap()
        ));
        super::super::equal::assert_link_eq(&doc, LinkShape::Bracket, "a[b]", None);
        let doc = parse(&format!(
            "{}\n",
            render_link(LinkShape::Angle, "https://y.com/a[b]", None).unwrap()
        ));
        super::super::equal::assert_link_eq(&doc, LinkShape::Angle, "https://y.com/a[b]", None);
    }

    #[test]
    fn checkbox_recipes_and_parse_stability() {
        for (state, text) in [
            (CheckboxState::Unchecked, "[ ]"),
            (CheckboxState::Checked, "[X]"),
            (CheckboxState::Partial, "[-]"),
        ] {
            assert_eq!(render_checkbox(state), text);
            let doc = parse(&format!("- {} item\n", render_checkbox(state)));
            super::super::equal::assert_checkbox_eq(&doc, state);
        }
    }
}
