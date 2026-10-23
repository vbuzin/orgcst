//! Latex environment begin/end lines and fragment form.

use super::{between_in, first_line, first_line_end, token_text_eq_caseless};
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode};
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
// Latex environment (begin-line name, closer-anchored body)
// ---------------------------------------------------------------------------

/// Token run between the braces of the begin line (`{NAME}` interior).
///
/// Walks [`first_line`] so a missing `{NAME}` panics rather than matching a
/// later pair, and so a second brace group on the begin line is not the name.
///
/// # Panics
///
/// Panics if the begin line has no `{NAME}` (`forms/latex.rs::opener`).
#[must_use]
pub(crate) fn latex_env_name_range(node: &SyntaxNode) -> TextRange {
    between_in(
        first_line(node),
        OrgSyntaxKind::L_BRACE,
        OrgSyntaxKind::R_BRACE,
    )
    .expect("grammar: LATEX_ENVIRONMENT begin line has {NAME} (forms/latex.rs::opener)")
}

/// After the begin line, before the closer line (post-blank excluded).
/// Empty: zero-width at end of the begin line.
///
/// Anchors on the last line-final `\end{NAME}` run, not on
/// [`partition_body_range`]'s last line (post-blank would swallow the closer).
///
/// # Panics
///
/// Panics if no closer exists (`forms/latex.rs::resolve`).
#[must_use]
pub(crate) fn latex_env_body_range(node: &SyntaxNode) -> TextRange {
    let start = first_line_end(node);
    let closer_start = latex_env_closer_line_start(node);
    if closer_start < start {
        TextRange::empty(start)
    } else {
        TextRange::new(start, closer_start)
    }
}

fn latex_env_closer_line_start(node: &SyntaxNode) -> TextSize {
    let mut cur = node.last_child_or_token();
    while let Some(e) = cur {
        if e.kind() == OrgSyntaxKind::BACKSLASH
            && let Some(line_start) = latex_end_line_start_from_backslash(&e)
        {
            return line_start;
        }
        cur = e.prev_sibling_or_token();
    }
    panic!("grammar: LATEX_ENVIRONMENT has an end line (forms/latex.rs::resolve)");
}

/// Line start of a line-final `\end{NAME}[ \t]*` run beginning at `bs`.
fn latex_end_line_start_from_backslash(bs: &SyntaxElement) -> Option<TextSize> {
    let word = bs.next_sibling_or_token()?;
    if !token_text_eq_caseless(&word, "end") {
        return None;
    }
    let lbrace = word.next_sibling_or_token()?;
    if lbrace.kind() != OrgSyntaxKind::L_BRACE {
        return None;
    }
    let mut n = lbrace.next_sibling_or_token()?;
    loop {
        if n.kind() == OrgSyntaxKind::R_BRACE {
            break;
        }
        if n.kind() == OrgSyntaxKind::NEWLINE {
            return None;
        }
        n = n.next_sibling_or_token()?;
    }
    let mut rest = n.next_sibling_or_token();
    while let Some(e) = rest {
        match e.kind() {
            OrgSyntaxKind::WHITESPACE => rest = e.next_sibling_or_token(),
            OrgSyntaxKind::NEWLINE => break,
            _ => return None,
        }
    }
    Some(super::line_start_of(bs))
}

/// Five latex-fragment forms as a prefix test over already-extracted node text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatexFragmentForm {
    Parens,
    Brackets,
    DisplayDollar,
    InlineDollar,
    Command,
}

#[must_use]
pub(crate) fn latex_fragment_form(text: &str) -> Option<LatexFragmentForm> {
    if text.starts_with("\\(") {
        Some(LatexFragmentForm::Parens)
    } else if text.starts_with("\\[") {
        Some(LatexFragmentForm::Brackets)
    } else if text.starts_with("$$") {
        Some(LatexFragmentForm::DisplayDollar)
    } else if text.starts_with('$') {
        Some(LatexFragmentForm::InlineDollar)
    } else if text.starts_with('\\') {
        Some(LatexFragmentForm::Command)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::{first_kind, line_partition, partition_body_range, synth};
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn latex_env_name_range_is_first_line_braces() {
        let src = "  \\begin{align*} extra\n{x}\n\\end{align*}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let name = latex_env_name_range(&node);
        assert_eq!(&src[name], "align*");
        assert_ne!(&src[name], "x");

        let src = "\\BEGIN{Equation}\nx\n\\end{equation}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        assert_eq!(&src[latex_env_name_range(&node)], "Equation");

        // Trailing first-line braces: first pair is the name, not `{x}`.
        let src = "\\begin{eq}{x}\nbody\n\\end{eq}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        assert_eq!(&src[latex_env_name_range(&node)], "eq");
    }
    #[test]
    fn latex_env_body_range_is_closer_anchored() {
        let src = "\\begin{eq}\nx\n\\end{eq}\n\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");
        let swallowed = partition_body_range(&line_partition(&node)).expect("partition body");
        assert!(
            src[swallowed].contains("\\end{eq}"),
            "partition_body_range swallows the closer into last_line's predecessor"
        );

        let empty = first_kind("\\begin{eq}\n\\end{eq}\n", OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&empty);
        assert!(body.is_empty());
        assert_eq!(body.start(), first_line_end(&empty));

        let src = "\\begin{equation}\nx\ntext\\end{equation}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");
        assert!(!src[body].contains("text"), "end-line prefix is not body");

        let src = "\\begin{eq}\n\\end{other}\nx\n\\end{eq}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "\\end{other}\nx\n");

        let src = "\\begin{eq}\nx\n\\end{eq}  \n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");

        let src = "\\begin{eq}\nx\n\\end{eq}\t\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");

        let src = "\\begin{eq}\nx\n  \\end{eq}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");
        assert!(
            !src[body].ends_with("  "),
            "indent before the closer is not body"
        );

        let src = "\\begin{eq}\nx\n\\END{eq}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");

        let src = "\\begin{eq}\nx\n\\end{eq}";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "x\n");

        let src = "\\begin{eq}\n\\end{eq} junk\nx\n\\end{eq}\n";
        let node = first_kind(src, OrgSyntaxKind::LATEX_ENVIRONMENT);
        let body = latex_env_body_range(&node);
        assert_eq!(&src[body], "\\end{eq} junk\nx\n");

        // Last backslash is `\\end{eq}` that is not line-final; skip it.
        let node = synth(
            OrgSyntaxKind::LATEX_ENVIRONMENT,
            &[
                (OrgSyntaxKind::BACKSLASH, "\\"),
                (OrgSyntaxKind::TEXT, "begin"),
                (OrgSyntaxKind::L_BRACE, "{"),
                (OrgSyntaxKind::TEXT, "eq"),
                (OrgSyntaxKind::R_BRACE, "}"),
                (OrgSyntaxKind::NEWLINE, "\n"),
                (OrgSyntaxKind::BACKSLASH, "\\"),
                (OrgSyntaxKind::TEXT, "end"),
                (OrgSyntaxKind::L_BRACE, "{"),
                (OrgSyntaxKind::TEXT, "eq"),
                (OrgSyntaxKind::R_BRACE, "}"),
                (OrgSyntaxKind::NEWLINE, "\n"),
                (OrgSyntaxKind::BACKSLASH, "\\"),
                (OrgSyntaxKind::TEXT, "end"),
                (OrgSyntaxKind::L_BRACE, "{"),
                (OrgSyntaxKind::TEXT, "eq"),
                (OrgSyntaxKind::R_BRACE, "}"),
                (OrgSyntaxKind::TEXT, "x"),
            ],
        );
        let body = latex_env_body_range(&node);
        assert!(body.is_empty());
        assert_eq!(body.start(), first_line_end(&node));
    }
    #[test]
    fn latex_fragment_form_prefix_order() {
        assert_eq!(
            latex_fragment_form("\\(x\\)"),
            Some(LatexFragmentForm::Parens)
        );
        assert_eq!(
            latex_fragment_form("\\[x\\]"),
            Some(LatexFragmentForm::Brackets)
        );
        assert_eq!(
            latex_fragment_form("$$x$$"),
            Some(LatexFragmentForm::DisplayDollar)
        );
        assert_eq!(
            latex_fragment_form("$x$"),
            Some(LatexFragmentForm::InlineDollar)
        );
        assert_eq!(
            latex_fragment_form("\\alpha"),
            Some(LatexFragmentForm::Command)
        );
        assert!(latex_fragment_form("alpha").is_none());
    }
}
