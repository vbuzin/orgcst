//! Inline source block object: `src_LANG{BODY}` / `src_LANG[HEADERS]{BODY}`.
//!
//! Evidence:
//! - org-syntax 5.8 (Inline Source Blocks)
//! - `org-element-inline-src-block-parser`: case-sensitive looking-at
//!   `\<src_\([^ \t\n[{]+\)[{[]`, optional line-bounded `[]` parameters,
//!   required line-bounded `{}` body.
//!
//! Structural twin is inline babel call ([`super::call`]), not element
//! `SRC_BLOCK`. Shared opener/name atoms (`fixed_underscore_prefix`,
//! `measure_name`) live in [`atoms`]; line-bounded pairs use
//! [`Parser::measure_paired_at`].
//!
//! Emits under `INLINE_SRC_BLOCK`:
//! ```text
//! TEXT("src") UNDERSCORE
//! SRC_LANGUAGE
//! [SRC_PARAMETERS]   // includes [ ]; empty [] retained
//! SRC_BODY           // includes { }; empty {} retained
//! ```
//!
//! [`probe`] is pure and [`parse`] emits from its shape. When the opener
//! matches (org's `looking-at` test) but the full form does not, the shape is
//! `InlineSrcShape { layout: None }` and emit falls back ("fail-soft") to
//! consuming only the bare `src_` tokens, leaving no orphan `SRC_*` nodes.
//!
//! The word boundary before `src_` is [`atoms::at_word_start`], the same rule
//! as inline babel calls.

use crate::parser::grammar::drive::atoms;
use crate::parser::{PairNewlines, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Measured layout
// ---------------------------------------------------------------------------

/// Position layout of a complete inline src object value (after `src_`).
///
/// Every region is half-open `[start, end)` with absolute positions.
/// Paired regions include their delimiters (`[`/`]`, `{`/`}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineSrcLayout {
    language_start: Pos,
    language_end: Pos,
    /// Optional `[...]` parameters (delimiters included).
    parameters: Option<(Pos, Pos)>,
    /// Required `{...}` body (delimiters included).
    body: (Pos, Pos),
}

/// Opener claim plus optional full layout.
///
/// `layout` is `None` when the opener matches but the full measure fails
/// (missing or unclosed body, or unclosed parameters that prevent a body);
/// parse then consumes only the bare `src_`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineSrcShape {
    /// Full language/params/body layout, or `None` when only the opener matched.
    pub(crate) layout: Option<InlineSrcLayout>,
}

// ---------------------------------------------------------------------------
// Name stop
// ---------------------------------------------------------------------------

/// Language name stop: elisp `[^ \t\n[{]+`.
#[inline]
fn is_src_language_stop(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::WHITESPACE
            | OrgSyntaxKind::INDENT
            | OrgSyntaxKind::NEWLINE
            | OrgSyntaxKind::L_BRACKET
            | OrgSyntaxKind::L_BRACE
            | OrgSyntaxKind::EOF
    )
}

// ---------------------------------------------------------------------------
// Probe / parse
// ---------------------------------------------------------------------------

/// Pure probe: word-start + `src_` + language + optional params + body.
///
/// Returns `None` when the opener does not match (no word start, bad prefix,
/// empty language, or language not followed by `[`/`{`). Returns
/// `Some(InlineSrcShape { layout: None })` when the opener matches but the
/// full form fails, and `Some` with a full layout when complete.
pub(crate) fn probe(parser: &Parser) -> Option<InlineSrcShape> {
    if !atoms::at_word_start(parser) {
        return None;
    }
    let language_start = atoms::fixed_underscore_prefix(parser, "src")?;
    let (language_start, language_end, after_name) =
        atoms::measure_name(parser, language_start, is_src_language_stop);
    if language_end == language_start {
        return None; // empty language: the opener does not match
    }

    // The opener requires `[` or `{` immediately after the language run.
    let after = match after_name {
        Some(t) if matches!(t.kind(), OrgSyntaxKind::L_BRACKET | OrgSyntaxKind::L_BRACE) => t,
        _ => return None,
    };

    // From here the opener matched; the full layout may still fail.
    let mut parameters = None;
    let body_open_pos = if after.kind() == OrgSyntaxKind::L_BRACKET {
        let open_pos = after.pos();
        match parser.measure_paired_at(
            open_pos,
            OrgSyntaxKind::L_BRACKET,
            OrgSyntaxKind::R_BRACKET,
            PairNewlines::Stop,
        ) {
            None => {
                // Unclosed parameters: the opener matched, the full form fails.
                return Some(InlineSrcShape { layout: None });
            }
            Some((_, p_end)) => {
                parameters = Some((open_pos, p_end));
                // Exclusive end of parameters is the candidate body open.
                p_end
            }
        }
    } else {
        // L_BRACE (gated by the match above).
        after.pos()
    };

    let body_tok = parser.token_view(body_open_pos);
    if body_tok.map(|t| t.kind()) != Some(OrgSyntaxKind::L_BRACE) {
        // Closed params with no immediate body, or space before `{`.
        return Some(InlineSrcShape { layout: None });
    }
    let Some(body) = parser.measure_paired_at(
        body_open_pos,
        OrgSyntaxKind::L_BRACE,
        OrgSyntaxKind::R_BRACE,
        PairNewlines::Stop,
    ) else {
        return Some(InlineSrcShape { layout: None });
    };

    Some(InlineSrcShape {
        layout: Some(InlineSrcLayout {
            language_start,
            language_end,
            parameters,
            body,
        }),
    })
}

/// Test harness: true when [`probe`] claims, including `layout: None`.
#[cfg(test)]
#[inline]
pub(crate) fn at_inline_src(parser: &Parser) -> bool {
    probe(parser).is_some()
}

/// Emit an inline src block from a measured shape.
///
/// Always consumes the two-token `src_` opener. On `layout: None` (opener
/// matched, full measure failed) it abandons the node after that opener.
pub(crate) fn parse(parser: &mut Parser, shape: InlineSrcShape) {
    let marker = parser.start();
    parser.eat(OrgSyntaxKind::TEXT);
    parser.eat(OrgSyntaxKind::UNDERSCORE);
    match shape.layout {
        Some(layout) => {
            debug_assert_eq!(parser.cursor(), layout.language_start);
            parser.leaf_to(OrgSyntaxKind::SRC_LANGUAGE, layout.language_end);
            if let Some((_, end)) = layout.parameters {
                // Includes empty [] when parameters span is present in layout.
                parser.leaf_to(OrgSyntaxKind::SRC_PARAMETERS, end);
            }
            parser.leaf_to(OrgSyntaxKind::SRC_BODY, layout.body.1);
            marker.complete(parser, OrgSyntaxKind::INLINE_SRC_BLOCK);
        }
        None => {
            // Leave the opener as bare tokens, as inline babel calls do.
            marker.abandon(parser);
        }
    }
}

/// Harness: probe then parse (unit tests / local walks).
#[cfg(test)]
pub(crate) fn parse_inline_src(parser: &mut Parser) {
    if let Some(shape) = probe(parser) {
        parse(parser, shape);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        ParseResult, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, run_inline, tree_text,
    };

    fn results_of_inline(src: &str) -> ParseResult {
        run_inline(src, |p| {
            if at_inline_src(p) {
                parse_inline_src(p);
            } else if p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
        })
    }

    fn at_src(src: &str) -> bool {
        at_inline_src(&make_parser(src))
    }

    fn at_src_after(src: &str, eats: usize) -> bool {
        let mut p = make_parser(src);
        for _ in 0..eats {
            p.eat_any();
        }
        at_inline_src(&p)
    }

    fn parses(src: &str) -> bool {
        node_kinds(&results_of_inline(src).1).contains(&OrgSyntaxKind::INLINE_SRC_BLOCK)
    }

    // --- Pure observers ----------------------------------------------------

    #[test]
    fn probe_is_pure_and_case_sensitive() {
        let p = make_parser("src_py{x}");
        let before = p.cursor().index();
        assert!(at_inline_src(&p));
        assert_eq!(p.cursor().index(), before);

        assert!(!at_src("SRC_py{x}"));
        assert!(!at_src("Src_py{x}"));
        assert!(!at_src("src_{x}"));
        assert!(!at_src("src_py"));
        assert!(!at_src("xsrc_py{x}"));
        assert!(!at_src("not_src{x}"));
    }

    #[test]
    fn probe_looking_at_accepts_bracket_or_brace() {
        assert!(at_src("src_py{x}"));
        assert!(at_src("src_py[hdr]{x}"));
        assert!(at_src("src_py[hdr]")); // looking-at only
        assert!(at_src("src_py{unclosed")); // looking-at; layout None
        assert!(!at_src("src_py {x}")); // space after name
        // `(` is interior to LANG, not a looking-at gate; without `[`/`{` after
        // the name run this is not looking-at.
        assert!(!at_src("src_py(x)"));
    }

    #[test]
    fn probe_word_boundary_pre() {
        assert!(at_src_after("pre src_py{x}", 2));
        assert!(at_src_after("pre\tsrc_py{x}", 2));
        assert!(at_src_after("  src_py{x}", 1));
        assert!(at_src_after("line\nsrc_py{x}", 2));
        assert!(at_src("src_py{x}"));

        // `$` is its own token and a word character in Org's syntax table.
        let mut p_glue = make_parser("$src_py{x}");
        p_glue.eat_any();
        assert!(!at_inline_src(&p_glue));
        p_glue.with_inline_region_start(Some(crate::parser::Pos::from_index(1)), |p| {
            assert!(
                at_inline_src(p),
                "at_region_start must accept src at synthetic region start"
            );
        });
        assert!(!at_src("xsrc_py{x}"));
    }

    /// Emacs `\<`, as for inline babel calls: punctuation before `src_` is a
    /// word start, as its own token or inside the TEXT tile.
    #[test]
    fn probe_word_start_follows_emacs() {
        for src in ["(src_py{x}", ":src_py{x}", "]src_py{x}"] {
            assert!(
                at_src_after(src, 1),
                "word start after punctuation in {src:?}"
            );
        }
        for src in ["\"src_sh{x}\n", "-src_sh{x}\n"] {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
                1,
                "{src:?}"
            );
        }
        for src in ["xsrc_sh{x}\n", "'src_sh{x}\n"] {
            let root = parse_document_tree(src);
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
                "{src:?}"
            );
        }
    }

    #[test]
    fn language_stop_helper_direct() {
        assert!(is_src_language_stop(OrgSyntaxKind::WHITESPACE));
        assert!(is_src_language_stop(OrgSyntaxKind::L_BRACKET));
        assert!(is_src_language_stop(OrgSyntaxKind::L_BRACE));
        assert!(is_src_language_stop(OrgSyntaxKind::EOF));
        assert!(!is_src_language_stop(OrgSyntaxKind::TEXT));
        assert!(!is_src_language_stop(OrgSyntaxKind::L_PAREN));
        assert!(!is_src_language_stop(OrgSyntaxKind::UNDERSCORE));
    }

    // --- Happy path --------------------------------------------------------

    #[test]
    fn happy_path_data_driven() {
        // (src, language, parameters, body)
        let cases: &[(&str, &str, Option<&str>, &str)] = &[
            ("src_python{print(1)}", "python", None, "{print(1)}"),
            (
                "src_python[:exports code]{print(1)}",
                "python",
                Some("[:exports code]"),
                "{print(1)}",
            ),
            ("src_c{x}", "c", None, "{x}"),
            ("src_foo-bar.1{x}", "foo-bar.1", None, "{x}"),
            // LANG may include `(` (org `[^ \t\n[{]+`); multi-token `_` interior.
            ("src_py(x){y}", "py(x)", None, "{y}"),
            ("src_foo_bar{x}", "foo_bar", None, "{x}"),
            ("src_c_mode{x}", "c_mode", None, "{x}"),
            ("src_py{}", "py", None, "{}"),
            ("src_py[]{x}", "py", Some("[]"), "{x}"),
            ("src_py[]{}", "py", Some("[]"), "{}"),
            ("src_py[hdr]{}", "py", Some("[hdr]"), "{}"),
            ("src_py{a{b}c}", "py", None, "{a{b}c}"),
            ("src_py[a[b]c]{x}", "py", Some("[a[b]c]"), "{x}"),
        ];
        for &(src, lang, params, body) in cases {
            let (_toks, evs, _) = results_of_inline(src);
            assert!(
                node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_SRC_BLOCK),
                "expected INLINE_SRC for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::SRC_LANGUAGE).as_deref(),
                Some(lang),
                "language for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::SRC_PARAMETERS).as_deref(),
                params,
                "parameters for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::SRC_BODY).as_deref(),
                Some(body),
                "body for {src:?}"
            );
        }
    }

    #[test]
    fn para_with_src_and_trailing_prose() {
        let src = "para src_py{x} tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let objs = nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK);
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].text().to_string(), "src_py{x}");
    }

    #[test]
    fn post_blank_spaces_outside_object() {
        let src = "src_py{x}  tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let objs = nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK);
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].text().to_string(), "src_py{x}");
    }

    #[test]
    fn complete_form_is_not_subscript() {
        let src = "src_py{x}\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
        );
    }

    /// Child shape order for a full form with parameters (tokens + nodes).
    #[test]
    fn child_shape_order_with_parameters() {
        let src = "src_py[hdr]{x}\n";
        let root = parse_document_tree(src);
        let objs = nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK);
        assert_eq!(objs.len(), 1);
        let kids: Vec<_> = objs[0].children_with_tokens().map(|e| e.kind()).collect();
        assert_eq!(
            kids,
            vec![
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::UNDERSCORE,
                OrgSyntaxKind::SRC_LANGUAGE,
                OrgSyntaxKind::SRC_PARAMETERS,
                OrgSyntaxKind::SRC_BODY,
            ]
        );
    }

    /// Parameters/body interiors are opaque (no nested object reparse).
    #[test]
    fn parameters_and_body_interiors_are_opaque() {
        let src = "src_py[*b*]{*not bold* [[a]]}\n";
        let root = parse_document_tree(src);
        let objs = nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK);
        assert_eq!(objs.len(), 1);
        let params = objs[0]
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::SRC_PARAMETERS)
            .expect("SRC_PARAMETERS");
        let body = objs[0]
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::SRC_BODY)
            .expect("SRC_BODY");
        assert_eq!(params.text().to_string(), "[*b*]");
        assert_eq!(body.text().to_string(), "{*not bold* [[a]]}");
        for (node, label) in [(&params, "SRC_PARAMETERS"), (&body, "SRC_BODY")] {
            assert!(
                !node
                    .descendants()
                    .any(|n| matches!(n.kind(), OrgSyntaxKind::BOLD | OrgSyntaxKind::LINK)),
                "no BOLD/LINK under {label}"
            );
        }
    }

    /// Incomplete non-looking-at residual vs looking-at fail-soft contrast.
    #[test]
    fn incomplete_src_subscript_residual_vs_looking_at_fail_soft() {
        // Non-looking-at: after TEXT("src") is eaten, `_` may form SUBSCRIPT.
        let root = parse_document_tree("src_py\n");
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK)
        );
        assert!(
            !nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "incomplete src_py may form SUBSCRIPT on `_`"
        );

        // Looking-at fail-soft: bare `src_` progress; `_` is not left for subscript.
        let root2 = parse_document_tree("src_py[hdr]\n");
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::INLINE_SRC_BLOCK)
        );
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "looking-at fail-soft must not form SUBSCRIPT"
        );
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_LANGUAGE).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_LANGUAGE)
        );
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_PARAMETERS).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_PARAMETERS)
        );
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_BODY).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::SRC_BODY)
        );
    }

    // --- Fail-soft / reject ------------------------------------------------

    #[test]
    fn reject_not_looking_at() {
        for src in [
            "src_py",
            "src_{x}",
            "SRC_py{x}",
            "Src_py{x}",
            "xsrc_py{x}",
            "src_py {x}",
        ] {
            assert!(!parses(src), "no INLINE_SRC for {src:?}");
            assert!(!at_src(src), "at_ must miss for {src:?}");
            let out = crate::parse(src);
            assert_eq!(out.root().text().to_string(), src);
            assert!(
                !out.root()
                    .descendants()
                    .any(|n| n.kind() == OrgSyntaxKind::INLINE_SRC_BLOCK)
            );
        }
    }

    #[test]
    fn looking_at_layout_none_fail_soft() {
        // Dual table: looking-at hit + parse miss vs non-hit.
        let cases: &[&str] = &[
            "src_py[hdr]",
            "src_py[hdr]x",
            "src_py[h] {x}",
            "src_py{unclosed",
            "src_py[unclosed",
        ];
        for src in cases {
            let mut p = make_parser(src);
            let shape = probe(&p).expect("looking-at claim");
            assert!(shape.layout.is_none(), "layout None for {src:?}");
            let before = p.cursor().index();
            parse(&mut p, shape);
            assert_eq!(
                p.cursor().index(),
                before + 2,
                "fail must advance exactly past src_ for {src:?}"
            );
            let (_toks, evs, _) = p.finish();
            assert!(!node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_SRC_BLOCK));
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_LANGUAGE), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_PARAMETERS), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_BODY), 0);
        }

        for src in ["src_py", "xsrc_py{x}", "not_src{x}"] {
            assert!(!at_src(src), "at_ must miss for {src:?}");
            let before = make_parser(src).cursor().index();
            let mut p = make_parser(src);
            if at_inline_src(&p) {
                parse_inline_src(&mut p);
            }
            assert_eq!(p.cursor().index(), before, "non-hit must not advance");
            let (_toks, evs, _) = p.finish();
            assert!(!node_kinds(&evs).contains(&OrgSyntaxKind::INLINE_SRC_BLOCK));
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_LANGUAGE), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_PARAMETERS), 0);
            assert_eq!(node_count(&evs, OrgSyntaxKind::SRC_BODY), 0);
        }
    }

    /// Full-document looking-at fail-soft: no `INLINE_SRC_BLOCK` or `SRC_*` nodes.
    #[test]
    fn looking_at_fail_soft_full_document() {
        let cases: &[&str] = &[
            "src_py[hdr]\n",
            "src_py[hdr]x\n",
            "src_py[h] {x}\n",
            "src_py{unclosed\n",
            "src_py[unclosed\n",
        ];
        for src in cases {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), *src, "tiling for {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
                "no INLINE_SRC for {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::SRC_LANGUAGE).is_empty(),
                "no SRC_LANGUAGE orphan for {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::SRC_PARAMETERS).is_empty(),
                "no SRC_PARAMETERS orphan for {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::SRC_BODY).is_empty(),
                "no SRC_BODY orphan for {src:?}"
            );
        }
    }

    /// Fail-soft claim mid-paragraph then a later complete src still forms.
    #[test]
    fn fail_soft_then_later_complete_src() {
        let src = "src_bad[hdr] src_ok{x}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let objs = nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK);
        assert_eq!(objs.len(), 1, "only the complete form claims");
        assert_eq!(objs[0].text().to_string(), "src_ok{x}");
        // No orphan SRC_* outside the single completed object.
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SRC_LANGUAGE).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SRC_BODY).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SRC_PARAMETERS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SRC_PARAMETERS)
        );
    }

    #[test]
    fn cross_line_body_and_params_fail_soft() {
        // PairNewlines::Stop: body closer on next line is not accepted.
        let src = "src_py{a\nb}\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            !out.root()
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::INLINE_SRC_BLOCK)
        );

        // Unclosed params on line (newline before `]`): looking-at then fail-soft.
        let src2 = "src_py[a\nb]{x}\n";
        let out2 = crate::parse(src2);
        assert_eq!(out2.root().text().to_string(), src2);
        assert!(
            !out2
                .root()
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::INLINE_SRC_BLOCK)
        );
        // Probe on the first line's tokens alone still claims looking-at.
        assert!(at_src("src_py[a"));
        match probe(&make_parser("src_py[a")).expect("looking-at") {
            InlineSrcShape { layout: None } => {}
            other => panic!("expected layout None, got {other:?}"),
        }
    }

    // --- Restrictions / scopes (document-level) ----------------------------

    #[test]
    fn scopes_paragraph_link_cell_radio() {
        // Paragraph: yes
        let root = parse_document_tree("src_py{x}\n");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );

        // Headline title: yes (standard-set-no-line-break)
        let root_h = parse_document_tree("* src_py{x}\n");
        assert_eq!(
            nodes_of_kind(&root_h, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );

        // Link description: yes
        let root_d = parse_document_tree("[[t][src_py{x}]]\n");
        assert_eq!(
            nodes_of_kind(&root_d, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );

        // Table cell: no (remq with babel)
        let root_c = parse_document_tree("| src_py{x} |\n");
        assert!(
            nodes_of_kind(&root_c, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(&root_c, OrgSyntaxKind::INLINE_SRC_BLOCK)
        );
        assert_eq!(tree_text(&root_c), "| src_py{x} |\n");

        // Radio target: no (MINIMAL). Interior must not have edge whitespace
        // (radio PRE rejects leading/trailing separators).
        let root_r = parse_document_tree("<<<src_py{x}>>>\n");
        assert_eq!(nodes_of_kind(&root_r, OrgSyntaxKind::RADIO_TARGET).len(), 1);
        assert!(
            nodes_of_kind(&root_r, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(&root_r, OrgSyntaxKind::INLINE_SRC_BLOCK)
        );

        // Emphasis interior: yes (STANDARD)
        let root_e = parse_document_tree("*src_py{x}*\n");
        assert_eq!(
            nodes_of_kind(&root_e, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );
    }

    // --- Precedence --------------------------------------------------------

    #[test]
    fn precedence_with_call_and_plain_link() {
        let src = "call_n() src_py{x} https://x.com\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_BABEL_CALL).len(),
            1
        );
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::INLINE_SRC_BLOCK).len(),
            1
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);

        // Incomplete src does not block a later plain link.
        let src2 = "src_py https://x.com\n";
        let root2 = parse_document_tree(src2);
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::INLINE_SRC_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::INLINE_SRC_BLOCK)
        );
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::LINK).len(), 1);
    }

    #[test]
    fn full_layout_probe_shape() {
        match probe(&make_parser("src_py[hdr]{x}")).expect("full") {
            InlineSrcShape { layout: Some(lay) } => {
                assert!(lay.parameters.is_some());
                assert_eq!(
                    lay.body.1.index() - lay.body.0.index(),
                    3 // `{` `x` `}`
                );
            }
            other => panic!("expected Some(layout), got {other:?}"),
        }
    }
}
