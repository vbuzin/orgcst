//! Citation opener and reference token walks.

use super::{first_token_of_kind, last_token_of_kind, text_in};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{TextRange, TextSize};

/// Style part: carved includes the `/`; content is the name after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CitationStyleSpans {
    pub carved: TextRange,
    pub content: TextRange,
}

/// `/STYLE` on a citation opener. `None` when there is no slash.
///
/// For `[cite/:]` the style is present but empty: the range is the slash and
/// the content is zero-width after it. The parser rejects that form, but the
/// helper still answers it.
#[must_use]
pub(crate) fn citation_style_spans(cite: &SyntaxNode) -> Option<CitationStyleSpans> {
    if cite.kind() != OrgSyntaxKind::CITATION {
        return None;
    }
    let mut after_cite = false;
    let mut slash = None;
    let mut end = None;
    for e in cite.children_with_tokens() {
        if !after_cite {
            if e.kind() == OrgSyntaxKind::TEXT && e.as_token().is_some_and(|t| t.text() == "cite") {
                after_cite = true;
            }
            continue;
        }
        if e.kind() == OrgSyntaxKind::COLON {
            break;
        }
        if slash.is_none() {
            if e.kind() != OrgSyntaxKind::SLASH {
                return None;
            }
            slash = Some(e.text_range());
            end = Some(e.text_range().end());
            continue;
        }
        end = Some(e.text_range().end());
    }
    let slash = slash?;
    let end = end?;
    let content = TextRange::new(slash.end(), end);
    Some(CitationStyleSpans {
        carved: TextRange::new(slash.start(), end),
        content,
    })
}

/// Exclusive end of the `cite` token (Absent Style insert).
#[must_use]
pub(crate) fn citation_cite_end(cite: &SyntaxNode) -> Option<TextSize> {
    if cite.kind() != OrgSyntaxKind::CITATION {
        return None;
    }
    cite.children_with_tokens().find_map(|e| {
        let t = e.as_token()?;
        (t.kind() == OrgSyntaxKind::TEXT && t.text() == "cite").then_some(t.text_range().end())
    })
}

/// Present/absent global prefix or suffix over unwrapped citation children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CitationAffixSpan {
    Present {
        carved: TextRange,
        removal: TextRange,
    },
    Absent {
        insert: TextSize,
    },
}

/// Contents start after `[cite[/STYLE]:` and opener blanks.
#[must_use]
pub(crate) fn citation_contents_start(cite: &SyntaxNode) -> Option<TextSize> {
    if cite.kind() != OrgSyntaxKind::CITATION {
        return None;
    }
    let mut after_cite = false;
    let mut after_colon = false;
    let mut last = None;
    for e in cite.children_with_tokens() {
        if !after_cite {
            if e.kind() == OrgSyntaxKind::TEXT && e.as_token().is_some_and(|t| t.text() == "cite") {
                after_cite = true;
            }
            continue;
        }
        if !after_colon {
            if e.kind() == OrgSyntaxKind::COLON {
                after_colon = true;
                last = Some(e.text_range().end());
            }
            continue;
        }
        if matches!(
            e.kind(),
            OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::NEWLINE | OrgSyntaxKind::INDENT
        ) {
            last = Some(e.text_range().end());
            continue;
        }
        return Some(e.text_range().start());
    }
    last
}

fn last_semi_in(node: &SyntaxNode, from: TextSize, to: TextSize) -> Option<TextRange> {
    semi_in(node, from, to, true)
}

fn first_semi_in(node: &SyntaxNode, from: TextSize, to: TextSize) -> Option<TextRange> {
    semi_in(node, from, to, false)
}

fn semi_in(node: &SyntaxNode, from: TextSize, to: TextSize, last: bool) -> Option<TextRange> {
    let base = node.text_range().start();
    if from < base || to > node.text_range().end() || from >= to {
        return None;
    }
    let rel_from = u32::from(from - base) as usize;
    let rel_to = u32::from(to - base) as usize;
    let s = node.text().to_string();
    let slice = s.get(rel_from..rel_to)?;
    let rel = if last {
        slice.as_bytes().iter().rposition(|&c| c == b';')?
    } else {
        slice.as_bytes().iter().position(|&c| c == b';')?
    };
    Some(TextRange::at(
        from + TextSize::from(rel as u32),
        TextSize::from(1),
    ))
}

fn trim_end_ws_range(node: &SyntaxNode, range: TextRange) -> TextRange {
    let s = text_in(node, range);
    let keep = s.trim_end_matches([' ', '\t', '\n', '\r']).len() as u32;
    if keep == 0 {
        TextRange::empty(range.start())
    } else {
        TextRange::new(range.start(), range.start() + TextSize::from(keep))
    }
}

/// Global prefix: the children before the first reference, present (maybe
/// empty) when a `;` ends it, absent otherwise.
#[must_use]
pub(crate) fn citation_prefix_span(cite: &SyntaxNode) -> Option<CitationAffixSpan> {
    let contents = citation_contents_start(cite)?;
    let first_ref = cite
        .children()
        .find(|c| c.kind() == OrgSyntaxKind::CITATION_REFERENCE)?;
    match last_semi_in(cite, contents, first_ref.text_range().start()) {
        None => Some(CitationAffixSpan::Absent { insert: contents }),
        Some(semi) => {
            let carved = if contents < semi.start() {
                trim_end_ws_range(cite, TextRange::new(contents, semi.start()))
            } else {
                TextRange::empty(semi.start())
            };
            let removal = if carved.is_empty() {
                semi
            } else {
                TextRange::new(carved.start(), semi.end())
            };
            Some(CitationAffixSpan::Present { carved, removal })
        }
    }
}

/// Global suffix: the children after the last reference's `;`, present
/// (maybe empty) when that `;` exists, absent otherwise.
#[must_use]
pub(crate) fn citation_suffix_span(cite: &SyntaxNode) -> Option<CitationAffixSpan> {
    let last_ref = cite
        .children()
        .filter(|c| c.kind() == OrgSyntaxKind::CITATION_REFERENCE)
        .last()?;
    let closer_start = last_token_of_kind(cite, OrgSyntaxKind::R_BRACKET)?
        .text_range()
        .start();
    match first_semi_in(cite, last_ref.text_range().end(), closer_start) {
        None => Some(CitationAffixSpan::Absent {
            insert: last_ref.text_range().end(),
        }),
        Some(semi) => {
            let after = semi.end();
            let carved = if after < closer_start {
                let raw = TextRange::new(after, closer_start);
                let s = text_in(cite, raw);
                let lead = s
                    .bytes()
                    .take_while(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
                    .count() as u32;
                let rest = TextRange::new(after + TextSize::from(lead), closer_start);
                trim_end_ws_range(cite, rest)
            } else {
                TextRange::empty(after)
            };
            let removal = if carved.is_empty() {
                semi
            } else {
                TextRange::new(semi.start(), carved.end())
            };
            Some(CitationAffixSpan::Present { carved, removal })
        }
    }
}

/// Key body after `@` on a citation reference (excludes `@`).
#[must_use]
pub(crate) fn citation_key_range(reference: &SyntaxNode) -> Option<TextRange> {
    if reference.kind() != OrgSyntaxKind::CITATION_REFERENCE {
        return None;
    }
    let at = first_token_of_kind(reference, OrgSyntaxKind::AT)?;
    let node_start = reference.text_range().start();
    let rel = u32::from(at.text_range().start() - node_start) as usize;
    let text = reference.text().to_string();
    let (ks, ke) = crate::parser::grammar::forms::citation::measure_citation_key_at(&text, rel)?;
    Some(TextRange::new(
        node_start + TextSize::from(ks as u32 + 1),
        node_start + TextSize::from(ke as u32),
    ))
}

/// Trailing `;` owned by a citation reference (between-ref separator).
#[must_use]
pub(crate) fn citation_ref_trailing_semi(reference: &SyntaxNode) -> Option<TextRange> {
    if reference.kind() != OrgSyntaxKind::CITATION_REFERENCE {
        return None;
    }
    last_semi_in(
        reference,
        reference.text_range().start(),
        reference.text_range().end(),
    )
    .filter(|semi| semi.end() == reference.text_range().end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn citation_style_and_affix_bounds() {
        let src = "[cite/n:@k]\n";
        let c = first_kind(src, OrgSyntaxKind::CITATION);
        let doc = parse(src);
        let style = citation_style_spans(&c).expect("style");
        assert_eq!(&doc.source()[style.content], "n");
        assert_eq!(&doc.source()[style.carved], "/n");

        let src = "[cite:@k]\n";
        let c = first_kind(src, OrgSyntaxKind::CITATION);
        assert!(citation_style_spans(&c).is_none());
        assert_eq!(
            usize::from(citation_cite_end(&c).expect("cite token")),
            5,
            "Absent Style inserts after `cite`"
        );
        assert!(matches!(
            citation_prefix_span(&c),
            Some(CitationAffixSpan::Absent { .. })
        ));
        assert!(matches!(
            citation_suffix_span(&c),
            Some(CitationAffixSpan::Absent { .. })
        ));

        let src = "[cite:;@k]\n";
        let c = first_kind(src, OrgSyntaxKind::CITATION);
        let CitationAffixSpan::Present { carved, removal } =
            citation_prefix_span(&c).expect("empty prefix")
        else {
            panic!("present empty");
        };
        assert!(carved.is_empty());
        let doc = parse(src);
        assert_eq!(&doc.source()[removal], ";");

        let src = "[cite: a; @k1; @k2; b]\n";
        let c = first_kind(src, OrgSyntaxKind::CITATION);
        let doc = parse(src);
        let CitationAffixSpan::Present { carved, removal } =
            citation_prefix_span(&c).expect("prefix")
        else {
            panic!("prefix");
        };
        assert_eq!(doc.source()[carved].trim(), "a");
        assert!(doc.source()[removal].contains(';'));
        let CitationAffixSpan::Present { carved, removal } =
            citation_suffix_span(&c).expect("suffix")
        else {
            panic!("suffix");
        };
        assert_eq!(doc.source()[carved].trim(), "b");
        assert!(doc.source()[removal].starts_with(';'));

        let refs: Vec<_> = c
            .children()
            .filter(|n| n.kind() == OrgSyntaxKind::CITATION_REFERENCE)
            .collect();
        assert_eq!(refs.len(), 2);
        let k1 = citation_key_range(&refs[0]).expect("k1");
        let k2 = citation_key_range(&refs[1]).expect("k2");
        assert_eq!(&doc.source()[k1], "k1");
        assert_eq!(&doc.source()[k2], "k2");
        assert!(citation_ref_trailing_semi(&refs[0]).is_some());
        assert!(
            citation_ref_trailing_semi(&refs[1]).is_none(),
            "global-suffix semi is not on the last ref"
        );
    }
}
