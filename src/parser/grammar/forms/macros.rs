//! Macro object: `{{{NAME}}}` / `{{{NAME(ARGUMENTS)}}}`.
//!
//! Named `macros` because `macro` is a keyword.
//!
//! Evidence:
//! - Worg 5.11 Macros
//! - `org-element-macro-parser` looking-at:
//!   `"{{{\\([a-zA-Z][-a-zA-Z0-9_]*\\)\\((\\(\\(?:.\\|\n\\)*?\\))\\)?}}}"`
//! - `org-element--object-lex` single arm on `?{` when `macro` is allowed
//!
//! Recognition is one atomic looking-at: three braces, an ASCII name, then
//! either `}}}` or `(args)}}}` where the closing `)` is the earliest `)` such
//! that `}}}` follows immediately (not balanced-paren scan). Incomplete forms
//! return `None` (silent miss) - unlike export snippet there is no committed
//! partial opener and no diagnostic.
//!
//! Name and arguments are opaque here. Splitting arguments on commas,
//! interpreting escapes and downcasing `:key` are left to the typed views,
//! not CST children. Emit is one flat [`OrgSyntaxKind::MACRO`] leaf via
//! [`Parser::leaf_len`], delimiters included, as for entities.
//!
//! [`probe`] is pure and returns the full byte length; [`parse`] emits the
//! leaf from it. On a miss the dispatcher consumes one `L_BRACE`.

use crate::parser::Parser;
use crate::syntax::OrgSyntaxKind;

/// Measured complete macro; incomplete forms return `None` from probe.
///
/// Unlike an export snippet, a macro never claims a partial opener.
///
/// `len` is the exclusive source-byte length from the cursor (first `{`)
/// through the closing `}}}`, consumed by `leaf_len` with no remeasure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MacroShape {
    /// Bytes from cursor through the closing `}}}`.
    pub(crate) len: usize,
}

/// Pure measure of a complete macro at the cursor.
///
/// No PRE: macros may open at BOL, after space, after `(`, after letters, etc.
/// Region-capped via [`Parser::rest_source`]. Cursor is never mutated.
pub(crate) fn probe(parser: &Parser) -> Option<MacroShape> {
    let t = parser.here()?;
    if !t.run_of(OrgSyntaxKind::L_BRACE, 3) {
        return None;
    }
    let rest = parser.rest_source();
    let b = rest.as_bytes();
    // Defensive check alongside run_of: rest_source could start mid-tile
    // under a region cap; with single-byte L_BRACE tiles the two agree.
    if b.len() < 3 || &b[..3] != b"{{{" {
        return None;
    }

    let name_len = measure_macro_name(&rest[3..])?;
    let i = 3 + name_len;
    let tail = rest.get(i..)?;

    // Case A: no args - `}}}` immediately after the name.
    if tail.starts_with("}}}") {
        return Some(MacroShape { len: i + 3 });
    }

    // Case B: args - `(` then first `)` such that the next three bytes are `}}}`.
    if !tail.starts_with('(') {
        return None;
    }
    // Scan for earliest j in rest with j >= i+1, j+4 <= rest.len(),
    // rest[j] == b')', rest[j+1..j+4] == b"}}}".
    let mut j = i + 1;
    while j + 4 <= b.len() {
        if b[j] == b')' && &b[j + 1..j + 4] == b"}}}" {
            return Some(MacroShape { len: j + 4 });
        }
        j += 1;
    }
    None
}

/// Emit one macro from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: MacroShape) {
    let ok = parser.leaf_len(OrgSyntaxKind::MACRO, shape.len);
    debug_assert!(ok, "macro leaf fits the length probe measured");
}

/// Exclusive byte length of a macro name at the start of `s`, or `None`.
///
/// Org: `[a-zA-Z][-a-zA-Z0-9_]*`. Strict ASCII - do not reuse
/// `atoms::is_name_atom` (Unicode alphanumeric) or `is_export_backend` (no `_`).
/// Underscore is its own lexer token, so name measure is source-byte oriented.
fn measure_macro_name(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_alphabetic() {
        return None;
    }
    let mut n = 1usize;
    while n < b.len() {
        let c = b[n];
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
            n += 1;
        } else {
            break;
        }
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree, run_object, tree_text,
    };

    #[test]
    fn measure_macro_name_charset() {
        assert_eq!(measure_macro_name("title"), Some(5));
        assert_eq!(measure_macro_name("a_b-c9"), Some(6));
        assert_eq!(measure_macro_name("Title"), Some(5));
        assert_eq!(measure_macro_name("a}}}"), Some(1));
        assert_eq!(measure_macro_name("foo(x)"), Some(3));
        assert_eq!(measure_macro_name("foo-bar.baz"), Some(7)); // stops before '.'
        assert!(measure_macro_name("").is_none());
        assert!(measure_macro_name("1bad").is_none());
        assert!(measure_macro_name("_bad").is_none());
        assert!(measure_macro_name("-x").is_none());
        // Strict ASCII: Unicode letters are not name chars (not is_name_atom).
        assert!(measure_macro_name("\u{03b1}").is_none()); // Greek alpha start
        assert!(measure_macro_name("\u{00e9}x").is_none()); // e-acute start
        // Interior non-ASCII stops the name walk (not accepted as continuation).
        assert_eq!(measure_macro_name("a\u{03b1}"), Some(1));
        assert_eq!(measure_macro_name("ab\u{00e9}c"), Some(2));
    }

    #[test]
    fn macro_shapes_accept() {
        for (src, want) in [
            ("{{{title}}}", "{{{title}}}"),
            ("{{{a}}}", "{{{a}}}"),
            ("{{{a_b-c9}}}", "{{{a_b-c9}}}"),
            ("{{{foo()}}}", "{{{foo()}}}"),
            ("{{{foo(x)}}}", "{{{foo(x)}}}"),
            ("{{{foo(x,y)}}}", "{{{foo(x,y)}}}"),
            ("{{{foo(a\\,b)}}}", "{{{foo(a\\,b)}}}"),
            // Elisp non-greedy: first `)` followed by `}}}` - args interior `a)b`.
            ("{{{foo(a)b)}}}", "{{{foo(a)b)}}}"),
            // Nested braces in args are opaque bytes (not balanced-brace).
            ("{{{foo(a{b})}}}", "{{{foo(a{b})}}}"),
            // `}}}` inside args when a later `)}}}` completes (elisp over Worg).
            ("{{{foo(a}}}b)}}}", "{{{foo(a}}}b)}}}"),
            ("{{{Title}}}", "{{{Title}}}"), // case preserved
        ] {
            let shape = probe(&make_parser(src)).unwrap_or_else(|| panic!("accept {src}"));
            assert_eq!(shape.len, want.len(), "len for {src:?}");
            let (_, evs, _) = run_object(src, probe, parse);
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::MACRO).as_deref(),
                Some(want),
                "node text for {src:?}"
            );
        }
    }

    #[test]
    fn macro_shapes_reject() {
        for src in [
            "{{{}}}",         // empty name
            "{{{1x}}}",       // digit-leading name
            "{{{_x}}}",       // underscore not a letter
            "{{{foo}",        // incomplete closer
            "{{{foo}}",       // two closers only
            "{{{foo(}}}",     // no `)` before `}}}`
            "{{{foo(a}}}",    // near-miss: no `)` before `}}}`
            "{{{foo(x)}y}}}", // `)` not immediately followed by `}}}`
            "{{{foo(unterminated",
            "{{foo}}}",          // only two opening braces
            "{foo}",             // single brace
            "{{{foo-bar.baz}}}", // name stops before `.`; not followed by `(`/`}}}`
            "plain",
            // Unicode start is not an ASCII letter.
            "{{{\u{03b1}}}}",
            "{{{\u{00e9}x}}}",
            // Name walk stops at non-ASCII interior; tail is not `(`/`}}}`.
            "{{{a\u{03b1}}}}",
        ] {
            assert!(probe(&make_parser(src)).is_none(), "not a macro: {src:?}");
        }
    }

    /// Reject forms tile as plain text under a full document (silent miss).
    #[test]
    fn rejected_macro_forms_round_trip_as_plain_text() {
        for src in [
            "{{{}}}\n",
            "{{{1x}}}\n",
            "{{{_x}}}\n",
            "{{{foo}}\n",
            "{{{foo(a}}}\n",
            "{{{foo(x)}y}}}\n",
            "{{{foo-bar.baz}}}\n",
            "{{{\u{03b1}}}}\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
                "no MACRO for reject {src:?}"
            );
        }
    }

    #[test]
    fn macro_probe_pure_and_parse_uses_shape() {
        for src in ["{{{title}}}", "{{{foo(x)}}}", "{{{foo", "{{{}}}", "{"] {
            let p = make_parser(src);
            let before = p.cursor();
            let _ = probe(&p);
            assert_eq!(p.cursor(), before, "probe must not advance for {src:?}");
        }

        let src = "{{{title}}}";
        let shape = probe(&make_parser(src)).expect("macro");
        assert_eq!(shape.len, src.len());
        let mut p = make_parser(src);
        parse(&mut p, shape);
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        let (_, evs, _) = p.finish();
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::MACRO).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn macro_in_paragraph_and_multi_line_args() {
        let src = "para {{{m}}} tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let macros = nodes_of_kind(&root, OrgSyntaxKind::MACRO);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].text().to_string(), "{{{m}}}");

        // Args may cross lines inside one paragraph (region cap is the bound).
        let src = "a {{{foo(a\nb)}}} c\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let macros = nodes_of_kind(&root, OrgSyntaxKind::MACRO);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].text().to_string(), "{{{foo(a\nb)}}}");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);

        // Two macros in one paragraph.
        let src = "{{{a}}} and {{{b}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 2);
    }

    /// No PRE: claim at BOL, after space, after letter, after `(`, after markup.
    #[test]
    fn macro_has_no_pre() {
        for src in [
            "{{{m}}}\n",
            "a {{{m}}}\n",
            "a{{{m}}}\n", // adjacent letter (no word-start / non-ws PRE)
            "({{{m}}})\n",
            "*bold*{{{m}}}\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "roundtrip {src:?}");
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(),
                1,
                "expected MACRO for {src:?}"
            );
        }
    }

    #[test]
    fn macro_args_stay_opaque() {
        let src = "{{{foo(*not bold*)}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::BOLD)
        );

        let src = "{{{foo([[a]])}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINK)
        );
    }

    /// Org link and table-cell rows both list macro; radio (MINIMAL) does not.
    #[test]
    fn macro_in_link_description_and_cell_not_radio() {
        let src = "[[https://x.com][d {{{m}}}]]\n| c {{{m}}} |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        for scope in [OrgSyntaxKind::LINK_DESCRIPTION, OrgSyntaxKind::TABLE_CELL] {
            let scopes = nodes_of_kind(&root, scope);
            assert_eq!(scopes.len(), 1, "one {scope:?}");
            assert_eq!(
                nodes_of_kind(&scopes[0], OrgSyntaxKind::MACRO).len(),
                1,
                "macro inside {scope:?}"
            );
        }

        // No leading/trailing separators inside radio (Worg interior rule).
        let src = "<<<{{{m}}}>>>\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
            "radio is MINIMAL; no MACRO"
        );
    }

    /// CAPTION value is `STANDARD_NO_FOOTNOTE` (still has `STANDARD` extras / macro).
    #[test]
    fn macro_in_caption_and_headline_title() {
        let src = "#+CAPTION: before {{{m}}} after\nA paragraph.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 1);

        let src = "* Title {{{m}}} rest\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let titles = nodes_of_kind(&root, OrgSyntaxKind::HEADING_TITLE);
        assert_eq!(titles.len(), 1);
        assert_eq!(
            nodes_of_kind(&titles[0], OrgSyntaxKind::MACRO).len(),
            1,
            "MACRO nested under HEADING_TITLE"
        );

        let src = "*bold {{{m}}}*\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let bolds = nodes_of_kind(&root, OrgSyntaxKind::BOLD);
        assert_eq!(bolds.len(), 1);
        assert_eq!(
            nodes_of_kind(&bolds[0], OrgSyntaxKind::MACRO).len(),
            1,
            "MACRO nested under BOLD"
        );
    }

    /// Closer past a `#+CAPTION:` value line must not form a macro (region cap).
    #[test]
    fn macro_respects_the_region_cap() {
        let src = "#+CAPTION: a {{{foo(v\nmore)}}}\nA paragraph.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
            "the closer is past the keyword value cap"
        );
    }

    /// Script coexistence: braced body cannot nest macro; paren body can.
    #[test]
    fn macro_script_coexistence() {
        // Bare macro is MACRO only.
        let src = "{{{name}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
        );

        // Ordinary scripts still form.
        let src = "x_2\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).len(), 1);

        let src = "x_{y}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).len(), 1);

        // Braced script body starts with `{`; interior is `{{name}}` (two braces)
        // so no nested MACRO under STANDARD either.
        let src = "x_{{{name}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
            "braced script interior has two leading braces"
        );

        // Paren script body re-enters parse_inline under STANDARD -> nested MACRO.
        let src = "x_({{{name}}})\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
        assert_eq!(subs.len(), 1);
        assert_eq!(
            nodes_of_kind(&subs[0], OrgSyntaxKind::MACRO).len(),
            1,
            "paren script body may nest MACRO"
        );

        // Five braces after `_` exceeds MAX_SCRIPT_DEPTH (3): no SUBSCRIPT.
        // After the `_` is declined and eaten, residual braces form a paragraph-
        // level MACRO on the inner three (`{{{name}}}`), leftover `}` bare.
        // This is not a macro nested under a script.
        let src = "x_{{{{{name}}}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
        );
        let macros = nodes_of_kind(&root, OrgSyntaxKind::MACRO);
        assert_eq!(
            macros.len(),
            1,
            "residual paragraph-level MACRO after script miss"
        );
        assert_eq!(macros[0].text().to_string(), "{{{name}}}");

        // Incomplete {{{ then later subscript still forms.
        let src = "{{{foo x_2\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::MACRO)
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).len(), 1);
    }

    /// Incomplete forms: silent miss, full tiling, no spin.
    #[test]
    fn incomplete_macro_is_silent_miss_with_progress() {
        for src in ["{{{foo\n", "{{{foo(bar\n", "{{{foo(a}}}\n"] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::MACRO).is_empty(),
                "no MACRO for {src:?}"
            );
        }
    }

    /// Four openers: first `{` miss -> `eat_any`; then `{{{title}}}` forms MACRO.
    #[test]
    fn four_braces_claim_inner_macro() {
        let src = "{{{{title}}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let macros = nodes_of_kind(&root, OrgSyntaxKind::MACRO);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].text().to_string(), "{{{title}}}");
    }

    /// Object extent ends at `}}}`; trailing spaces stay sibling under paragraph.
    #[test]
    fn macro_post_blank_is_outside_node() {
        let src = "{{{m}}}  tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let macros = nodes_of_kind(&root, OrgSyntaxKind::MACRO);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].text().to_string(), "{{{m}}}");
    }

    #[test]
    fn full_document_happy_macro_roundtrips() {
        let src = "{{{title}}} and {{{highlight(important text)}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::MACRO).len(), 2);
    }
}
