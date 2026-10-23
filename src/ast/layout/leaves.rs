//! Flat-object token walks: scripts, cookies, snippets, macros, footnote
//! labels and references.

use super::{between, first_token_of_kind, span_of};
use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

/// Marker and body of a sub/superscript. `braced` is the `{...}` form only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScriptLayout {
    pub marker: TextRange,
    pub body: TextRange,
    pub braced: bool,
}

#[must_use]
pub(crate) fn script_layout(node: &SyntaxNode) -> Option<ScriptLayout> {
    let kids: Vec<_> = node.children_with_tokens().collect();
    let first = kids.first()?;
    if !matches!(
        first.kind(),
        OrgSyntaxKind::UNDERSCORE | OrgSyntaxKind::CARET
    ) {
        return None;
    }
    let marker = first.text_range();
    let braced = kids
        .get(1)
        .is_some_and(|e| e.kind() == OrgSyntaxKind::L_BRACE)
        && kids
            .last()
            .is_some_and(|e| e.kind() == OrgSyntaxKind::R_BRACE);
    let body = if braced {
        let start = kids.get(1)?.text_range().end();
        let end = kids.last()?.text_range().start();
        TextRange::new(start, end)
    } else {
        TextRange::new(marker.end(), node.text_range().end())
    };
    Some(ScriptLayout {
        marker,
        body,
        braced,
    })
}

/// Cookie interior: fraction vs percent, with optional digit spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CookieLayout {
    Fraction {
        done: Option<TextRange>,
        total: Option<TextRange>,
    },
    Percent {
        value: Option<TextRange>,
    },
}

#[must_use]
pub(crate) fn cookie_layout(node: &SyntaxNode) -> Option<CookieLayout> {
    if node.kind() != OrgSyntaxKind::STATISTICS_COOKIE {
        return None;
    }
    let kids: Vec<_> = node.children_with_tokens().collect();
    if kids.len() < 3
        || kids.first()?.kind() != OrgSyntaxKind::L_BRACKET
        || kids.last()?.kind() != OrgSyntaxKind::R_BRACKET
    {
        return None;
    }
    let inner = &kids[1..kids.len() - 1];
    match inner {
        [slash] if slash.kind() == OrgSyntaxKind::SLASH => Some(CookieLayout::Fraction {
            done: None,
            total: None,
        }),
        [pct] if pct.kind() == OrgSyntaxKind::PERCENT => {
            Some(CookieLayout::Percent { value: None })
        }
        [done, slash]
            if slash.kind() == OrgSyntaxKind::SLASH && done.kind() == OrgSyntaxKind::TEXT =>
        {
            Some(CookieLayout::Fraction {
                done: Some(done.text_range()),
                total: None,
            })
        }
        [slash, total]
            if slash.kind() == OrgSyntaxKind::SLASH && total.kind() == OrgSyntaxKind::TEXT =>
        {
            Some(CookieLayout::Fraction {
                done: None,
                total: Some(total.text_range()),
            })
        }
        [done, slash, total]
            if slash.kind() == OrgSyntaxKind::SLASH
                && done.kind() == OrgSyntaxKind::TEXT
                && total.kind() == OrgSyntaxKind::TEXT =>
        {
            Some(CookieLayout::Fraction {
                done: Some(done.text_range()),
                total: Some(total.text_range()),
            })
        }
        [value, pct]
            if pct.kind() == OrgSyntaxKind::PERCENT && value.kind() == OrgSyntaxKind::TEXT =>
        {
            Some(CookieLayout::Percent {
                value: Some(value.text_range()),
            })
        }
        _ => None,
    }
}

/// `@@backend:value@@` backend and value (value excludes the colon and `@@`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExportSnippetSpans {
    pub backend: TextRange,
    pub value: TextRange,
}

#[must_use]
pub(crate) fn export_snippet_spans(node: &SyntaxNode) -> Option<ExportSnippetSpans> {
    let kids: Vec<_> = node.children_with_tokens().collect();
    if kids.len() < 6 {
        return None;
    }
    if kids[0].kind() != OrgSyntaxKind::AT
        || kids[1].kind() != OrgSyntaxKind::AT
        || kids[2].kind() != OrgSyntaxKind::TEXT
        || kids[3].kind() != OrgSyntaxKind::COLON
        || kids[kids.len() - 2].kind() != OrgSyntaxKind::AT
        || kids[kids.len() - 1].kind() != OrgSyntaxKind::AT
    {
        return None;
    }
    Some(ExportSnippetSpans {
        backend: kids[2].text_range(),
        value: TextRange::new(
            kids[3].text_range().end(),
            kids[kids.len() - 2].text_range().start(),
        ),
    })
}

/// `{{{name}}}` / `{{{name(args)}}}` name and optional argument interior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MacroSpans {
    pub name: TextRange,
    pub args: Option<TextRange>,
}

#[must_use]
pub(crate) fn macro_spans(node: &SyntaxNode) -> Option<MacroSpans> {
    let kids: Vec<_> = node.children_with_tokens().collect();
    if kids.len() < 7 {
        return None;
    }
    if !kids[..3].iter().all(|e| e.kind() == OrgSyntaxKind::L_BRACE) {
        return None;
    }
    let n = kids.len();
    if !kids[n - 3..]
        .iter()
        .all(|e| e.kind() == OrgSyntaxKind::R_BRACE)
    {
        return None;
    }
    let inner = &kids[3..n - 3];
    let paren = inner
        .iter()
        .position(|e| e.kind() == OrgSyntaxKind::L_PAREN);
    match paren {
        None => Some(MacroSpans {
            name: span_of(inner.iter().cloned())?,
            args: None,
        }),
        Some(i) => {
            if i == 0 || inner.last()?.kind() != OrgSyntaxKind::R_PAREN {
                return None;
            }
            let args_elems = &inner[i + 1..inner.len() - 1];
            let args = Some(match span_of(args_elems.iter().cloned()) {
                Some(r) => r,
                None => TextRange::empty(inner[i].text_range().end()),
            });
            Some(MacroSpans {
                name: span_of(inner[..i].iter().cloned())?,
                args,
            })
        }
    }
}

/// Footnote-reference label and optional definition interior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FootnoteRefLayout {
    pub label: Option<TextRange>,
    pub definition: Option<TextRange>,
}

#[must_use]
pub(crate) fn footnote_ref_layout(node: &SyntaxNode) -> Option<FootnoteRefLayout> {
    if node.kind() != OrgSyntaxKind::FOOTNOTE_REFERENCE {
        return None;
    }
    let kids: Vec<_> = node.children_with_tokens().collect();
    let colon1 = kids.iter().position(|e| e.kind() == OrgSyntaxKind::COLON)?;
    let after = colon1 + 1;
    let closer = kids
        .iter()
        .rposition(|e| e.kind() == OrgSyntaxKind::R_BRACKET)?;
    if after >= closer {
        return Some(FootnoteRefLayout {
            label: None,
            definition: None,
        });
    }
    if kids[after].kind() == OrgSyntaxKind::COLON {
        let def_start = kids[after].text_range().end();
        let def_end = kids[closer].text_range().start();
        return Some(FootnoteRefLayout {
            label: None,
            definition: Some(TextRange::new(def_start, def_end)),
        });
    }
    let colon2 = kids[after..closer]
        .iter()
        .position(|e| e.kind() == OrgSyntaxKind::COLON)
        .map(|rel| after + rel);
    match colon2 {
        None => {
            let start = kids[after].text_range().start();
            let end = kids[closer - 1].text_range().end();
            Some(FootnoteRefLayout {
                label: Some(TextRange::new(start, end)),
                definition: None,
            })
        }
        Some(c2) => {
            let start = kids[after].text_range().start();
            let end = kids[c2 - 1].text_range().end();
            let def_start = kids[c2].text_range().end();
            let def_end = kids[closer].text_range().start();
            Some(FootnoteRefLayout {
                label: Some(TextRange::new(start, end)),
                definition: Some(TextRange::new(def_start, def_end)),
            })
        }
    }
}

/// Carved `[fn:label]` header and the label-atom span between `:` and `]`.
///
/// # Panics
///
/// Panics if the header tokens are missing (`forms/footnote.rs::opener`).
#[must_use]
pub(crate) fn footnote_label_spans(node: &SyntaxNode) -> (TextRange, TextRange) {
    let open = first_token_of_kind(node, OrgSyntaxKind::L_BRACKET)
        .expect("grammar: FOOTNOTE_DEFINITION starts with [fn: (forms/footnote.rs::opener)");
    let close = first_token_of_kind(node, OrgSyntaxKind::R_BRACKET)
        .expect("grammar: FOOTNOTE_DEFINITION header closes with ] (forms/footnote.rs::opener)");
    let content = between(node, OrgSyntaxKind::COLON, OrgSyntaxKind::R_BRACKET).expect(
        "grammar: FOOTNOTE_DEFINITION label sits between : and ] (forms/footnote.rs::opener)",
    );
    (
        TextRange::new(open.text_range().start(), close.text_range().end()),
        content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn footnote_label_spans_header_and_multi_atom_content() {
        let src = "[fn:a-b_c]    body\n";
        let node = first_kind(src, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        let (carved, content) = footnote_label_spans(&node);
        assert_eq!(&src[carved], "[fn:a-b_c]");
        assert_eq!(&src[content], "a-b_c");
        assert!(carved.start() < content.start());
        assert!(content.end() < carved.end());

        // `]` in the same-line body lives under PARAGRAPH, not a direct token.
        let src = "[fn:1] see ] here\n";
        let node = first_kind(src, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        let (carved, content) = footnote_label_spans(&node);
        assert_eq!(&src[carved], "[fn:1]");
        assert_eq!(&src[content], "1");
    }
    #[test]
    fn script_layout_braced_vs_bare() {
        let src = "a_{b}\n";
        let s = first_kind(src, OrgSyntaxKind::SUBSCRIPT);
        let lay = script_layout(&s).expect("braced");
        let doc = parse(src);
        assert!(lay.braced);
        assert_eq!(&doc.source()[lay.marker], "_");
        assert_eq!(&doc.source()[lay.body], "b");
        let src = "a_b\n";
        let s = first_kind(src, OrgSyntaxKind::SUBSCRIPT);
        let lay = script_layout(&s).expect("bare");
        let doc = parse(src);
        assert!(!lay.braced);
        assert_eq!(&doc.source()[lay.body], "b");
        let src = "a_(x)\n";
        let s = first_kind(src, OrgSyntaxKind::SUBSCRIPT);
        let lay = script_layout(&s).expect("paren");
        let doc = parse(src);
        assert!(!lay.braced);
        assert_eq!(&doc.source()[lay.body], "(x)");
    }
    #[test]
    #[allow(clippy::many_single_char_names)] // fixture bindings in one cookie table
    fn cookie_layout_shapes() {
        let n = first_kind("[/]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        assert!(matches!(
            cookie_layout(&n),
            Some(CookieLayout::Fraction {
                done: None,
                total: None
            })
        ));
        let n = first_kind("[1/3]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        let CookieLayout::Fraction {
            done: Some(d),
            total: Some(t),
        } = cookie_layout(&n).expect("frac")
        else {
            panic!("fraction");
        };
        let doc = parse("[1/3]\n");
        assert_eq!(&doc.source()[d], "1");
        assert_eq!(&doc.source()[t], "3");
        let n = first_kind("[50%]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        let CookieLayout::Percent { value: Some(v) } = cookie_layout(&n).expect("pct") else {
            panic!("percent");
        };
        let doc = parse("[50%]\n");
        assert_eq!(&doc.source()[v], "50");
        let n = first_kind("[%]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        assert!(matches!(
            cookie_layout(&n),
            Some(CookieLayout::Percent { value: None })
        ));
        let n = first_kind("[2/]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        assert!(matches!(
            cookie_layout(&n),
            Some(CookieLayout::Fraction {
                done: Some(_),
                total: None
            })
        ));
        let n = first_kind("[/7]\n", OrgSyntaxKind::STATISTICS_COOKIE);
        assert!(matches!(
            cookie_layout(&n),
            Some(CookieLayout::Fraction {
                done: None,
                total: Some(_)
            })
        ));
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(cookie_layout(&p).is_none());
    }
    #[test]
    fn export_snippet_and_macro_spans() {
        let src = "@@html:<b>@@\n";
        let n = first_kind(src, OrgSyntaxKind::EXPORT_SNIPPET);
        let sp = export_snippet_spans(&n).expect("snippet");
        let doc = parse(src);
        assert_eq!(&doc.source()[sp.backend], "html");
        assert_eq!(&doc.source()[sp.value], "<b>");

        let src = "{{{title}}}\n";
        let n = first_kind(src, OrgSyntaxKind::MACRO);
        let sp = macro_spans(&n).expect("macro");
        let doc = parse(src);
        assert_eq!(&doc.source()[sp.name], "title");
        assert!(sp.args.is_none());

        let src = "{{{kw(a, b)}}}\n";
        let n = first_kind(src, OrgSyntaxKind::MACRO);
        let sp = macro_spans(&n).expect("args");
        let doc = parse(src);
        assert_eq!(&doc.source()[sp.name], "kw");
        assert_eq!(&doc.source()[sp.args.expect("args")], "a, b");
    }
    #[test]
    fn footnote_ref_layout_named_inline_anonymous() {
        // Column 0 `[fn:lab]` is a definition; refs need a non-column-0 home.
        let n = first_kind("x [fn:lab]\n", OrgSyntaxKind::FOOTNOTE_REFERENCE);
        let lay = footnote_ref_layout(&n).expect("named");
        let doc = parse("x [fn:lab]\n");
        assert_eq!(&doc.source()[lay.label.expect("label")], "lab");
        assert!(lay.definition.is_none());

        let n = first_kind("x [fn:lab:def]\n", OrgSyntaxKind::FOOTNOTE_REFERENCE);
        let lay = footnote_ref_layout(&n).expect("inline");
        let doc = parse("x [fn:lab:def]\n");
        assert_eq!(&doc.source()[lay.label.expect("label")], "lab");
        assert_eq!(&doc.source()[lay.definition.expect("def")], "def");

        let n = first_kind("x [fn::def]\n", OrgSyntaxKind::FOOTNOTE_REFERENCE);
        let lay = footnote_ref_layout(&n).expect("anon");
        let doc = parse("x [fn::def]\n");
        assert!(lay.label.is_none());
        assert_eq!(&doc.source()[lay.definition.expect("def")], "def");
    }
}
