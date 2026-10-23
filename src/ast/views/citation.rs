//! Citations (`[cite/style:prefix; @key; suffix]`) and their references.
//!
//! [`Citation::objects`] yields the references only; the objects of the
//! global prefix and suffix have their own iterators.

use crate::TextRange;
use crate::ast::layout::{
    CitationAffixSpan, citation_cite_end, citation_contents_start, citation_key_range,
    citation_prefix_span, citation_ref_trailing_semi, citation_style_spans, citation_suffix_span,
    text_in,
};
use crate::ast::support::child_views;
use crate::ast::{
    AstNode, HasObjects, HasParts, InsertPoint, Object, Padding, PartSpan, PresentPart,
};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A citation object: `[cite/style:prefix; @key; suffix]`.
    Citation,
    OrgSyntaxKind::CITATION
);
crate::ast_node!(
    /// One reference inside a citation: `@key` with its own prefix and suffix.
    CitationReference,
    OrgSyntaxKind::CITATION_REFERENCE
);

/// Parts of a citation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CitationPart {
    /// `/style` after `cite`; its content excludes the slash.
    Style,
    /// Text before the first reference, up to its `;`.
    GlobalPrefix,
    /// Text after the last reference's `;`.
    GlobalSuffix,
}

/// Parts of a citation reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CitationReferencePart {
    /// Text before `@key`.
    Prefix,
    /// The key after `@`.
    Key,
    /// Text after the key; its removal includes the `;` that follows.
    Suffix,
}

impl Citation {
    /// Style name after `/`, without the slash, when present.
    #[must_use]
    pub fn style(&self) -> Option<TextRange> {
        citation_style_spans(self.syntax()).map(|s| s.content)
    }

    /// The references, in source order.
    pub fn references(&self) -> impl Iterator<Item = CitationReference> + '_ {
        child_views(self.syntax())
    }

    /// Objects in the global prefix, before the first reference.
    ///
    /// [`HasObjects::objects`] does not yield these.
    pub fn global_prefix_objects(&self) -> impl Iterator<Item = Object> + '_ {
        affix_objects(self.syntax(), citation_prefix_span(self.syntax()))
    }

    /// Objects in the global suffix, after the last reference's `;`.
    pub fn global_suffix_objects(&self) -> impl Iterator<Item = Object> + '_ {
        affix_objects(self.syntax(), citation_suffix_span(self.syntax()))
    }
}

impl HasObjects for Citation {
    fn objects(&self) -> impl Iterator<Item = Object> + '_ {
        self.references().map(Object::from)
    }
}

impl HasParts for Citation {
    type Part = CitationPart;
    const PARTS: &'static [CitationPart] = &[
        CitationPart::Style,
        CitationPart::GlobalPrefix,
        CitationPart::GlobalSuffix,
    ];

    fn part_span(&self, part: CitationPart) -> PartSpan {
        match part {
            CitationPart::Style => {
                if let Some(s) = citation_style_spans(self.syntax()) {
                    PartSpan::Present(PresentPart::new(s.carved, s.content, s.carved))
                } else {
                    let end = citation_cite_end(self.syntax()).expect(
                        "grammar: CITATION opener includes cite (forms/citation.rs::probe)",
                    );
                    PartSpan::Absent(InsertPoint::new(end, Padding::None, Padding::None))
                }
            }
            CitationPart::GlobalPrefix => {
                affix_part(self.syntax(), citation_prefix_span(self.syntax()))
            }
            CitationPart::GlobalSuffix => {
                affix_part(self.syntax(), citation_suffix_span(self.syntax()))
            }
        }
    }
}

impl CitationReference {
    /// The key after `@`.
    ///
    /// # Panics
    ///
    /// Panics if the node has no `@key`. The parser always produces one.
    #[must_use]
    pub fn key(&self) -> String {
        let r = citation_key_range(self.syntax()).expect(
            "grammar: CITATION_REFERENCE has an @key (forms/citation.rs::parse_one_citation_reference)",
        );
        text_in(self.syntax(), r)
    }
}

impl HasObjects for CitationReference {}

impl HasParts for CitationReference {
    type Part = CitationReferencePart;
    const PARTS: &'static [CitationReferencePart] = &[
        CitationReferencePart::Prefix,
        CitationReferencePart::Key,
        CitationReferencePart::Suffix,
    ];

    fn part_span(&self, part: CitationReferencePart) -> PartSpan {
        let key = citation_key_range(self.syntax()).expect(
            "grammar: CITATION_REFERENCE has an @key (forms/citation.rs::parse_one_citation_reference)",
        );
        let at = crate::ast::layout::first_token_of_kind(self.syntax(), OrgSyntaxKind::AT)
            .expect("grammar: citation key is preceded by @ (forms/citation.rs)");
        match part {
            CitationReferencePart::Key => PartSpan::Present(PresentPart::identical(key)),
            CitationReferencePart::Prefix => {
                let start = self.range().start();
                let end = at.text_range().start();
                if start < end {
                    let carved = TextRange::new(start, end);
                    PartSpan::Present(PresentPart::identical(carved))
                } else {
                    PartSpan::Absent(InsertPoint::new(start, Padding::None, Padding::None))
                }
            }
            CitationReferencePart::Suffix => {
                let after_key = key.end();
                let end = self.range().end();
                let semi = citation_ref_trailing_semi(self.syntax());
                if after_key >= end && semi.is_none() {
                    return PartSpan::Absent(InsertPoint::new(end, Padding::None, Padding::None));
                }
                let carved_end = semi.map_or(end, rowan::TextRange::start);
                let carved = if after_key < carved_end {
                    TextRange::new(after_key, carved_end)
                } else {
                    TextRange::empty(after_key)
                };
                let removal = match semi {
                    Some(s) => TextRange::new(carved.start(), s.end()),
                    None => carved,
                };
                PartSpan::Present(PresentPart::new(carved, carved, removal))
            }
        }
    }
}

fn affix_part(syntax: &crate::syntax::SyntaxNode, span: Option<CitationAffixSpan>) -> PartSpan {
    match span {
        Some(CitationAffixSpan::Present { carved, removal }) => {
            PartSpan::Present(PresentPart::new(carved, carved, removal))
        }
        Some(CitationAffixSpan::Absent { insert }) => {
            PartSpan::Absent(InsertPoint::new(insert, Padding::None, Padding::None))
        }
        None => {
            let insert =
                citation_contents_start(syntax).unwrap_or_else(|| syntax.text_range().end());
            PartSpan::Absent(InsertPoint::new(insert, Padding::None, Padding::None))
        }
    }
}

fn affix_objects(
    cite: &crate::syntax::SyntaxNode,
    span: Option<CitationAffixSpan>,
) -> impl Iterator<Item = Object> + '_ {
    let carved = match span {
        Some(CitationAffixSpan::Present { carved, .. }) if !carved.is_empty() => Some(carved),
        _ => None,
    };
    cite.children().filter_map(move |n| {
        let r = n.text_range();
        if carved.is_some_and(|c| r.start() >= c.start() && r.end() <= c.end()) {
            Object::cast(n)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{HasObjects, HasParts};
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn cite(src: &str) -> (crate::Document, Citation) {
        let doc = parse(src);
        let c = Citation::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::CITATION)
                .into_iter()
                .next()
                .expect("cite"),
        )
        .expect("cast");
        (doc, c)
    }

    #[test]
    fn objects_are_references_only() {
        let (doc, c) = cite("[cite: *a*; @k1; @k2; *b*]\n");
        let objs: Vec<_> = c.objects().collect();
        assert_eq!(objs.len(), 2);
        assert!(
            objs.iter()
                .all(|o| matches!(o, Object::CitationReference(_)))
        );
        assert_eq!(c.global_prefix_objects().count(), 1);
        assert_eq!(c.global_suffix_objects().count(), 1);
        crate::ast::check_part_contract(&c);
        let refs: Vec<_> = c.references().collect();
        crate::ast::check_part_contract(&refs[0]);
        crate::ast::check_part_contract(&refs[1]);
        let s0 = refs[0]
            .part_removal_range(CitationReferencePart::Suffix)
            .unwrap();
        assert!(source_slice(&doc, s0).contains(';'));
        match refs[1].part_span(CitationReferencePart::Suffix) {
            PartSpan::Absent(_) => {}
            PartSpan::Present(p) => {
                assert!(
                    !source_slice(&doc, p.removal()).contains(';'),
                    "last ref does not own the global-suffix semi"
                );
            }
            PartSpan::NotApplicable => {
                panic!("last-ref Suffix Present or Absent, got NotApplicable")
            }
        }
    }

    #[test]
    fn style_removal_includes_slash() {
        let (doc, c) = cite("[cite/n:@k]\n");
        let rm = c.part_removal_range(CitationPart::Style).unwrap();
        assert_eq!(source_slice(&doc, rm), "/n");
        let start = usize::from(rm.start());
        let end = usize::from(rm.end());
        assert_eq!(
            format!("{}{}", &doc.source()[..start], &doc.source()[end..]),
            "[cite:@k]\n"
        );
        crate::ast::check_part_contract(&c);
    }

    #[test]
    fn reference_prefix_objects() {
        let (_, c) = cite("[cite:*b* @k]\n");
        let refs: Vec<_> = c.references().collect();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].objects().count(), 1);
        crate::ast::check_part_contract(&refs[0]);
    }

    #[test]
    fn empty_prefix_present_missing_semi_absent() {
        let (_, c) = cite("[cite:;@k]\n");
        match c.part_span(CitationPart::GlobalPrefix) {
            PartSpan::Present(p) => assert!(p.content().is_empty()),
            other => panic!("expected present empty, got {other:?}"),
        }
        crate::ast::check_part_contract(&c);

        let (_, c) = cite("[cite:@k]\n");
        assert!(matches!(
            c.part_span(CitationPart::GlobalPrefix),
            PartSpan::Absent(_)
        ));
        crate::ast::check_part_contract(&c);

        let (_, c) = cite("[cite:@k;]\n");
        match c.part_span(CitationPart::GlobalSuffix) {
            PartSpan::Present(p) => assert!(p.content().is_empty()),
            other => panic!("expected present empty suffix, got {other:?}"),
        }
    }

    #[test]
    fn style_absent_after_cite() {
        let (doc, c) = cite("[cite/n:@k]\n");
        assert_eq!(source_slice(&doc, c.style().unwrap()), "n");
        let (_, c) = cite("[cite:@k]\n");
        let ip = c.part_insert_point(CitationPart::Style).unwrap();
        assert_eq!(ip.lead(), Padding::None);
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::CITATION) {
                if let Some(c) = Citation::cast(n) {
                    crate::ast::check_part_contract(&c);
                }
            }
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::CITATION_REFERENCE) {
                if let Some(r) = CitationReference::cast(n) {
                    crate::ast::check_part_contract(&r);
                }
            }
        }
    }
}
