//! Markup objects: bold, italic, underline, strike-through, code and verbatim.
//!
//! [`Markup`] covers all six. Only the first four contain objects, so
//! [`HasObjects`] is implemented on [`Emphasis`], reached through
//! [`Markup::body`]; a code or verbatim body is raw text.

use crate::TextRange;
use crate::ast::layout::{delimited_interior, first_token_of_kind, last_token_of_kind};
use crate::ast::{AstNode, HasObjects, HasParts, PartSpan, PresentPart};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A markup object: bold, italic, underline, strike-through, code or
    /// verbatim.
    Markup,
    [
        OrgSyntaxKind::BOLD,
        OrgSyntaxKind::ITALIC,
        OrgSyntaxKind::UNDERLINE,
        OrgSyntaxKind::STRIKE_THROUGH,
        OrgSyntaxKind::CODE,
        OrgSyntaxKind::VERBATIM,
    ]
);

crate::ast_node!(
    /// A bold, italic, underline or strike-through object seen as the objects
    /// it holds; reached through [`Markup::body`].
    Emphasis,
    [
        OrgSyntaxKind::BOLD,
        OrgSyntaxKind::ITALIC,
        OrgSyntaxKind::UNDERLINE,
        OrgSyntaxKind::STRIKE_THROUGH,
    ]
);

/// Which of the six markup forms a [`Markup`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkupKind {
    /// `*bold*`.
    Bold,
    /// `/italic/`.
    Italic,
    /// `_underline_`.
    Underline,
    /// `+strike-through+`.
    StrikeThrough,
    /// `~code~`.
    Code,
    /// `=verbatim=`.
    Verbatim,
}

/// The body of a markup object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MarkupBody {
    /// Bold, italic, underline or strike-through: the body holds objects.
    Emphasis(Emphasis),
    /// Code or verbatim: the range of the body, which is plain text.
    Raw(TextRange),
}

/// Parts of a markup object.
///
/// At the exclusive end of the markup (`*bold*|next`), [`HasParts::part_at`]
/// is `None`, not [`Self::CloseMarker`]. [`crate::Document::context_at`] still
/// reports a [`crate::ContextFrame::Markup`] frame there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkupPart {
    /// The opening marker character.
    OpenMarker,
    /// Everything between the markers.
    Body,
    /// The closing marker character.
    CloseMarker,
}

impl Markup {
    /// Which markup form this is.
    #[must_use]
    pub fn kind(&self) -> MarkupKind {
        match self.syntax().kind() {
            OrgSyntaxKind::BOLD => MarkupKind::Bold,
            OrgSyntaxKind::ITALIC => MarkupKind::Italic,
            OrgSyntaxKind::UNDERLINE => MarkupKind::Underline,
            OrgSyntaxKind::STRIKE_THROUGH => MarkupKind::StrikeThrough,
            OrgSyntaxKind::CODE => MarkupKind::Code,
            OrgSyntaxKind::VERBATIM => MarkupKind::Verbatim,
            other => unreachable!("grammar: Markup only wraps the six markup kinds, got {other:?}"),
        }
    }

    /// Whether the body can contain objects: false for code and verbatim.
    #[must_use]
    pub fn is_recursive(&self) -> bool {
        !matches!(self.kind(), MarkupKind::Code | MarkupKind::Verbatim)
    }

    /// The body: objects for emphasis, a raw range for code and verbatim.
    #[must_use]
    pub fn body(&self) -> MarkupBody {
        match Emphasis::cast(self.syntax().clone()) {
            Some(em) => MarkupBody::Emphasis(em),
            None => MarkupBody::Raw(body_range(self.syntax(), marker_kind(self.kind()))),
        }
    }
}

impl HasParts for Markup {
    type Part = MarkupPart;
    const PARTS: &'static [MarkupPart] = &[
        MarkupPart::OpenMarker,
        MarkupPart::Body,
        MarkupPart::CloseMarker,
    ];

    fn part_span(&self, part: MarkupPart) -> PartSpan {
        let marker = marker_kind(self.kind());
        match part {
            MarkupPart::OpenMarker => {
                let tok = first_token_of_kind(self.syntax(), marker)
                    .expect("grammar: markup starts with its opener (forms/markup.rs::parse)");
                PartSpan::Present(PresentPart::identical(tok.text_range()))
            }
            MarkupPart::Body => {
                PartSpan::Present(PresentPart::identical(body_range(self.syntax(), marker)))
            }
            MarkupPart::CloseMarker => {
                let tok = last_token_of_kind(self.syntax(), marker)
                    .expect("grammar: markup ends with its closer (forms/markup.rs::parse)");
                PartSpan::Present(PresentPart::identical(tok.text_range()))
            }
        }
    }
}

impl HasObjects for Emphasis {}

fn marker_kind(kind: MarkupKind) -> OrgSyntaxKind {
    match kind {
        MarkupKind::Bold => OrgSyntaxKind::STAR,
        MarkupKind::Italic => OrgSyntaxKind::SLASH,
        MarkupKind::Underline => OrgSyntaxKind::UNDERSCORE,
        MarkupKind::StrikeThrough => OrgSyntaxKind::PLUS,
        MarkupKind::Code => OrgSyntaxKind::TILDE,
        MarkupKind::Verbatim => OrgSyntaxKind::EQUALS,
    }
}

fn body_range(syntax: &crate::syntax::SyntaxNode, marker: OrgSyntaxKind) -> TextRange {
    delimited_interior(syntax, marker, marker)
        .expect("grammar: markup has opener and closer tokens (forms/markup.rs::parse)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{HasObjects, HasParts, Object};
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn first_markup(src: &str, kind: OrgSyntaxKind) -> (crate::Document, Markup) {
        let doc = parse(src);
        let m = Markup::cast(
            nodes_of_kind(doc.root(), kind)
                .into_iter()
                .next()
                .expect("markup"),
        )
        .expect("cast");
        (doc, m)
    }

    #[test]
    fn nested_objects_inside_emphasis_none_inside_code() {
        let (doc, bold) = first_markup("*a /i/ b*\n", OrgSyntaxKind::BOLD);
        assert_eq!(bold.kind(), MarkupKind::Bold);
        assert!(bold.is_recursive());
        let MarkupBody::Emphasis(em) = bold.body() else {
            panic!("emphasis door");
        };
        let objs: Vec<_> = em.objects().collect();
        assert_eq!(objs.len(), 1);
        assert!(matches!(objs[0], Object::Markup(_)));
        assert_eq!(
            source_slice(&doc, bold.part_content_range(MarkupPart::Body).unwrap()),
            "a /i/ b"
        );
        crate::ast::check_part_contract(&bold);
        assert!(Emphasis::cast(bold.syntax().clone()).is_some());
        let closer = bold.part_range(MarkupPart::CloseMarker).unwrap();
        assert_eq!(source_slice(&doc, closer), "*");
        assert_eq!(
            bold.part_at(closer.start()),
            Some(MarkupPart::Body),
            "part_at on the closer start is Body (left-bias)"
        );

        let (doc, code) = first_markup("~*not bold*~\n", OrgSyntaxKind::CODE);
        assert!(!code.is_recursive());
        assert!(Emphasis::cast(code.syntax().clone()).is_none());
        let MarkupBody::Raw(raw) = code.body() else {
            panic!("raw door");
        };
        assert_eq!(source_slice(&doc, raw), "*not bold*");
        assert!(
            nodes_of_kind(code.syntax(), OrgSyntaxKind::BOLD).is_empty(),
            "code does not nest bold"
        );
        crate::ast::check_part_contract(&code);

        let (_, verb) = first_markup("=*not bold*=\n", OrgSyntaxKind::VERBATIM);
        assert!(matches!(verb.body(), MarkupBody::Raw(_)));
        assert!(
            nodes_of_kind(verb.syntax(), OrgSyntaxKind::BOLD).is_empty(),
            "verbatim does not nest bold"
        );
        crate::ast::check_part_contract(&verb);
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for kind in [
                OrgSyntaxKind::BOLD,
                OrgSyntaxKind::ITALIC,
                OrgSyntaxKind::UNDERLINE,
                OrgSyntaxKind::STRIKE_THROUGH,
                OrgSyntaxKind::CODE,
                OrgSyntaxKind::VERBATIM,
            ] {
                for n in nodes_of_kind(doc.root(), kind) {
                    if let Some(m) = Markup::cast(n) {
                        crate::ast::check_part_contract(&m);
                    }
                }
            }
        }
    }
}
