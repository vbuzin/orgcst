//! LaTeX environments (`\begin{name}` ... `\end{name}`) and LaTeX fragment
//! objects.
//!
//! An environment's body is opaque text; it is never parsed into elements.

use crate::ast::layout::{
    LatexFragmentForm, latex_env_body_range, latex_env_name_range, latex_fragment_form,
    with_node_text,
};
use crate::ast::{AstNode, HasParts, PartSpan, PresentPart};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A LaTeX environment element: `\begin{name}` ... `\end{name}`.
    LatexEnvironment,
    OrgSyntaxKind::LATEX_ENVIRONMENT
);
crate::ast_node!(
    /// A LaTeX fragment object, such as `\(...\)`, `\[...\]`, `$...$` or a
    /// `\name` command with arguments.
    LatexFragment,
    OrgSyntaxKind::LATEX_FRAGMENT
);
crate::ast::impl_leaf_element_view!(LatexEnvironment);

/// How a LaTeX fragment is written, read from the node's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatexFragmentShape {
    /// `\(...\)`.
    InlineParen,
    /// `\[...\]`.
    DisplayBracket,
    /// `$$...$$`.
    DisplayDollar,
    /// `$...$`.
    InlineDollar,
    /// A command such as `\foo{bar}`.
    Command,
}

impl LatexFragment {
    /// How this fragment is written, read from its opening characters.
    ///
    /// # Panics
    ///
    /// Panics if the node text does not start with a fragment opener
    /// (`\(`, `\[`, `$$`, `$`, or a command). The parser only produces
    /// `LATEX_FRAGMENT` for those openers.
    #[must_use]
    pub fn shape(&self) -> LatexFragmentShape {
        with_node_text(self.syntax(), |t| {
            match latex_fragment_form(t).expect(
                "grammar: LATEX_FRAGMENT starts with \\(, \\[, $$, $, or \\NAME (forms/latex.rs)",
            ) {
                LatexFragmentForm::Parens => LatexFragmentShape::InlineParen,
                LatexFragmentForm::Brackets => LatexFragmentShape::DisplayBracket,
                LatexFragmentForm::DisplayDollar => LatexFragmentShape::DisplayDollar,
                LatexFragmentForm::InlineDollar => LatexFragmentShape::InlineDollar,
                LatexFragmentForm::Command => LatexFragmentShape::Command,
            }
        })
    }
}

/// Parts of a latex environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatexEnvironmentPart {
    /// The environment name between the braces of the `\begin` line.
    Name,
    /// Text after the `\begin` line and before the `\end` line.
    Body,
}

impl HasParts for LatexEnvironment {
    type Part = LatexEnvironmentPart;
    const PARTS: &'static [LatexEnvironmentPart] =
        &[LatexEnvironmentPart::Name, LatexEnvironmentPart::Body];

    fn part_span(&self, part: LatexEnvironmentPart) -> PartSpan {
        match part {
            LatexEnvironmentPart::Name => {
                PartSpan::Present(PresentPart::identical(latex_env_name_range(self.syntax())))
            }
            LatexEnvironmentPart::Body => {
                PartSpan::Present(PresentPart::identical(latex_env_body_range(self.syntax())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_line_end;
    use crate::ast::{AstNode, ElementView, HasParts};
    use crate::parse;
    use crate::tests::{nodes_of_kind, source_slice};

    #[test]
    fn fragment_shape_five_forms() {
        use crate::parse;
        use crate::tests::nodes_of_kind;
        let cases = [
            ("\\(x\\)\n", LatexFragmentShape::InlineParen),
            ("\\[x\\]\n", LatexFragmentShape::DisplayBracket),
            ("$$x$$\n", LatexFragmentShape::DisplayDollar),
            ("$x$\n", LatexFragmentShape::InlineDollar),
            ("\\foo{bar}\n", LatexFragmentShape::Command),
        ];
        for (src, want) in cases {
            let doc = parse(src);
            let f = LatexFragment::cast(
                nodes_of_kind(doc.root(), OrgSyntaxKind::LATEX_FRAGMENT)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("fragment in {src:?}")),
            )
            .expect("cast");
            assert_eq!(f.shape(), want, "{src:?}");
        }
    }

    fn first_env(src: &str) -> (crate::Document, LatexEnvironment) {
        let doc = parse(src);
        let env = LatexEnvironment::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LATEX_ENVIRONMENT)
                .into_iter()
                .next()
                .expect("env"),
        )
        .expect("cast");
        (doc, env)
    }

    #[test]
    fn latex_environment_is_opaque() {
        let (doc, env) = first_env("\\begin{equation}\nx\n\\end{equation}\n");
        assert!(env.content_range().is_none());
        assert!(!env.is_greater());
        assert!(
            env.syntax().children().next().is_none(),
            "opaque body has no child nodes"
        );
        let body = env
            .part_content_range(LatexEnvironmentPart::Body)
            .expect("body field");
        assert_eq!(source_slice(&doc, body), "x\n");
        crate::ast::check_part_contract(&env);
    }

    #[test]
    fn empty_body_is_present_zero_width() {
        let (_, env) = first_env("\\begin{eq}\n\\end{eq}\n");
        assert!(env.content_range().is_none());
        let body = env
            .part_content_range(LatexEnvironmentPart::Body)
            .expect("body field");
        assert!(body.is_empty());
        assert_eq!(body.start(), first_line_end(env.syntax()));
        crate::ast::check_part_contract(&env);
    }

    #[test]
    fn name_from_braces_under_awkward_whitespace() {
        let src = "  \\begin{align*} extra\n{x}\n\\end{align*}\n";
        let (doc, env) = first_env(src);
        let name = env
            .part_content_range(LatexEnvironmentPart::Name)
            .expect("name");
        assert_eq!(source_slice(&doc, name), "align*");
        let body = env
            .part_content_range(LatexEnvironmentPart::Body)
            .expect("body");
        assert_eq!(source_slice(&doc, body), "{x}\n");
        crate::ast::check_part_contract(&env);

        let src = "\\begin{eq}{x}\nbody\n\\end{eq}\n";
        let (doc, env) = first_env(src);
        let name = env
            .part_content_range(LatexEnvironmentPart::Name)
            .expect("name");
        assert_eq!(source_slice(&doc, name), "eq");
        crate::ast::check_part_contract(&env);
    }

    #[test]
    fn body_excludes_post_blank_and_end_line_prefix() {
        let src = "\\begin{equation}\nx\ntext\\end{equation}\n\n";
        let (doc, env) = first_env(src);
        let body = env
            .part_content_range(LatexEnvironmentPart::Body)
            .expect("body");
        assert_eq!(source_slice(&doc, body), "x\n");
        assert!(
            env.range().end() > body.end(),
            "post-blank stays on range()"
        );
        crate::ast::check_part_contract(&env);
    }

    #[test]
    fn has_parts_over_taxonomy() {
        use crate::parse_with;
        use crate::tests::TAXONOMY;
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::LATEX_ENVIRONMENT) {
                let env = LatexEnvironment::cast(node).expect("cast");
                crate::ast::check_part_contract(&env);
            }
        }
    }
}
