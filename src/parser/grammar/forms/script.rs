//! Subscript and superscript objects: `_SCRIPT` and `^SCRIPT`.
//!
//! One grammar, two kinds: `org-element-subscript-parser` and
//! `org-element-superscript-parser` differ only in the marker character and the
//! node they create. Both match `org-match-substring-regexp` (`org.el`) with
//! point moved back one character:
//!
//! ```text
//! \(\S-\)  \([_^]\)  \( {balanced} | (balanced) | * | [+-]?[[:alnum:].,\]*[[:alnum:]] \)
//!  group 1   group 2    group 3       (group 4 = inside {}, group 5 = inside ())
//! ```
//!
//! Evidence: Worg 5.14 Subscript and Superscript; `org-match-substring-regexp`,
//! `org-create-multibrace-regexp`, `org-match-sexp-depth`,
//! `org-element--object-regexp`, `org-element-recursive-objects`.
//!
//! **PRE.** The character before the marker must be non-whitespace and the
//! marker must not open the parsed region. Org spells that as `\S-` plus
//! `(unless (bolp) ...)`, where `bolp` is true at the start of a
//! `narrow-to-region`; here that is `at_line_start` plus `at_region_start`.
//! Superscript's guard is written differently (`(unless (bolp) (backward-char))`
//! without wrapping the match) but is equivalent: `org-element--object-regexp`
//! can only leave point at column 0 when the next character is *not* `_`/`^`,
//! which is exactly what the match then demands, so column 0 never yields a
//! superscript either.
//!
//! **Extent.** `:begin` is `(match-beginning 2)`: the node starts at the
//! marker, not at the preceding character.
//!
//! **Contents.** `use-brackets-p` comes from group 4 only, so the paren form is
//! *not* bracketed: its contents include the parentheses and its interpreter
//! round-trips `^%s`. Braced contents exclude the braces. All three forms parse
//! their contents recursively under the standard object set.
//!
//! **No config knob.** `org-use-sub-superscripts` is a display/export option;
//! `org-element.el` never reads it, so recognition here is unconditional.
//!
//! Emits:
//! ```text
//! SUBSCRIPT | SUPERSCRIPT
//!   UNDERSCORE | CARET
//!   [L_BRACE] contents... [R_BRACE]    // braced
//!   contents...                        // bare / paren (parens are contents)
//! ```
//!
//! [`probe`] is pure and measures the whole form; [`parse`] consumes
//! it with no rescan. A rejected probe leaves the marker to the dispatcher's
//! `eat_any`, so `x_` is an ordinary underscore token.

use crate::parser::grammar::drive::region;
use crate::parser::{Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

/// Maximum stacked delimiters in a braced or paren body (`org-match-sexp-depth`).
///
/// Org builds the body alternatives with `org-create-multibrace-regexp` at
/// depth 3, and a deeper nest matches *nothing*: the brace alternative fails
/// and no other alternative can begin on `{`.
const MAX_SCRIPT_DEPTH: usize = 3;

/// Measured sub/superscript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScriptShape {
    /// `SUBSCRIPT` (marker `_`) or `SUPERSCRIPT` (marker `^`).
    pub(crate) kind: OrgSyntaxKind,
    pub(crate) body: ScriptBody,
}

/// The three body alternatives of `org-match-substring-regexp` group 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptBody {
    /// `{...}`: braces are node children outside the contents region.
    /// `closer` is the matching `}`.
    Braced { closer: Pos },
    /// `(...)`: org's contents are group 3, so the parentheses are *inside*
    /// them. `end` is the exclusive position past the matching `)`.
    Paren { end: Pos },
    /// `*` or `[+-]?[[:alnum:].,\]*[[:alnum:]]`.
    ///
    /// The only body whose end can fall inside a lex tile (`x^a-b` measures
    /// `a` out of the tile `a-b`), so it carries both a token cap for the
    /// whole-tile prefix and the absolute byte end of the form.
    Bare {
        /// Exclusive token cap for the contents region (whole tiles only).
        contents_end: Pos,
        /// Absolute exclusive byte end of the object.
        byte_end: usize,
    },
}

/// Pure measure of a sub/superscript at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<ScriptShape> {
    let kind = match parser.current() {
        OrgSyntaxKind::UNDERSCORE => OrgSyntaxKind::SUBSCRIPT,
        OrgSyntaxKind::CARET => OrgSyntaxKind::SUPERSCRIPT,
        _ => return None,
    };
    if !pre_ok(parser) {
        return None;
    }
    let marker = parser.here()?;
    let body = match marker.kind_at(1) {
        OrgSyntaxKind::L_BRACE => {
            let open = marker.nth(1)?;
            let closer = balanced(open, OrgSyntaxKind::L_BRACE, OrgSyntaxKind::R_BRACE)?;
            ScriptBody::Braced {
                closer: closer.pos(),
            }
        }
        OrgSyntaxKind::L_PAREN => {
            let open = marker.nth(1)?;
            let closer = balanced(open, OrgSyntaxKind::L_PAREN, OrgSyntaxKind::R_PAREN)?;
            ScriptBody::Paren { end: closer.end() }
        }
        _ => measure_bare(parser, marker)?,
    };
    Some(ScriptShape { kind, body })
}

/// Emit one sub/superscript from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: ScriptShape) {
    debug_assert!(
        matches!(
            (parser.current(), shape.kind),
            (OrgSyntaxKind::UNDERSCORE, OrgSyntaxKind::SUBSCRIPT)
                | (OrgSyntaxKind::CARET, OrgSyntaxKind::SUPERSCRIPT)
        ),
        "script shape marker must match cursor"
    );
    let m = parser.start();
    parser.eat_any(); // `_` / `^`

    match shape.body {
        ScriptBody::Braced { closer } => {
            let opened = parser.eat(OrgSyntaxKind::L_BRACE);
            debug_assert!(opened, "braced opener guaranteed by probe");
            region::inline_in(parser, closer, shape.kind);
            let closed = parser.eat(OrgSyntaxKind::R_BRACE);
            debug_assert!(closed, "braced closer guaranteed by probe");
        }
        ScriptBody::Paren { end } => {
            // Parens are part of the contents (org group 3), so they fall
            // inside the inline region rather than being eaten around it.
            region::inline_in(parser, end, shape.kind);
        }
        ScriptBody::Bare {
            contents_end,
            byte_end,
        } => {
            region::inline_in(parser, contents_end, shape.kind);
            // A body that stops inside a tile leaves a raw remainder; objects
            // can only start on a token boundary, so nothing is skipped.
            if parser.source_start_here().is_some_and(|at| at < byte_end) {
                let ok = parser.eat_to(byte_end);
                debug_assert!(ok, "bare script remainder after measure");
            }
        }
    }

    m.complete(parser, shape.kind);
}

// ---------------------------------------------------------------------------
// PRE and body measurement
// ---------------------------------------------------------------------------

/// Org `\S-` before the marker, with `bolp` (buffer or narrowed region start)
/// excluded.
///
/// ASCII-only whitespace, like the markup PRE/POST byte sets: a non-ASCII lead
/// byte counts as non-whitespace, which is what org's syntax classes do for
/// ordinary letters.
#[inline]
fn pre_ok(parser: &Parser) -> bool {
    !parser.at_line_start()
        && !parser.at_region_start()
        && parser.prev_byte().is_some_and(|b| !b.is_ascii_whitespace())
}

/// Balanced `open` ... `close` from the token at `t` (which must be `open`).
///
/// Depth-capped at [`MAX_SCRIPT_DEPTH`]; newlines are ordinary interior
/// (`[^{}]*?` matches them) and the scan is bounded by the view's cap.
/// [`Parser::find_paired_at`] is deliberately not reused: it has no depth cap.
fn balanced<'a, 's>(
    t: TokenView<'a, 's>,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
) -> Option<TokenView<'a, 's>> {
    debug_assert_eq!(t.kind(), open, "balanced scan must start on its opener");
    let mut depth = 0usize;
    let mut k = 0usize;
    while let Some(tok) = t.nth(k) {
        let kind = tok.kind();
        if kind == open {
            depth += 1;
            if depth > MAX_SCRIPT_DEPTH {
                return None;
            }
        } else if kind == close {
            depth -= 1;
            if depth == 0 {
                return Some(tok);
            }
        }
        k += 1;
    }
    None
}

/// Measure the bare body (`*` or the alnum form) after the marker.
///
/// Returns the token cap for whole tiles plus the absolute byte end of the
/// whole object.
fn measure_bare(parser: &Parser, marker: TokenView) -> Option<ScriptBody> {
    let start = parser.source_start_here()?;
    let rest = parser.rest_source();
    let after = rest.get(1..)?;
    let body_len = measure_bare_body(after)?;

    // Whole tiles fully inside the body; the last one may be partial.
    let mut acc = 0usize;
    let mut whole = 0usize;
    while acc < body_len {
        let Some(tok) = marker.nth(1 + whole) else {
            break;
        };
        let len = tok.text().len();
        if acc + len > body_len {
            break;
        }
        acc += len;
        whole += 1;
    }
    let contents_end = marker
        .nth(1 + whole)
        .map_or_else(|| parser.limit(), |tok| tok.pos());

    Some(ScriptBody::Bare {
        contents_end,
        byte_end: start + 1 + body_len,
    })
}

/// Byte length of a bare body at the start of `after` (the text after the
/// marker), or `None` when no legal body is there.
///
/// `\*` is one alternative; otherwise an optional single sign, then a run of
/// alphanumerics / `.` / `,` / `\`, and the body must **end** on an
/// alphanumeric (org's trailing `[[:alnum:]]`, reached by backtracking the
/// greedy run). Alphanumeric is Unicode-aware, matching Emacs `[:alnum:]` and
/// the crate's other org character classes.
///
/// A leading `\` is rejected: `org-element--object-regexp` gates the character
/// after the marker on `[-{(*+.,[:alnum:]]`, so a bare body may *contain* a
/// backslash (`x_a\b`) but never start with one (`x_\alpha` is an entity).
fn measure_bare_body(after: &str) -> Option<usize> {
    if after.starts_with('*') {
        return Some(1);
    }
    if after.starts_with('\\') {
        return None;
    }
    let signed = after.starts_with(['+', '-']);
    let scan_from = usize::from(signed);
    let mut end = None;
    for (i, c) in after.get(scan_from..)?.char_indices() {
        if c.is_alphanumeric() {
            end = Some(scan_from + i + c.len_utf8());
        } else if c != '.' && c != ',' && c != '\\' {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree, run_inline, tree_text,
    };

    /// Probe + parse inside an inline region, the way the dispatcher runs it.
    fn run_script(src: &str) -> crate::tests::ParseResult {
        run_inline(src, |p| {
            while p.current() != OrgSyntaxKind::EOF {
                match probe(p) {
                    Some(shape) => parse(p, shape),
                    None => p.eat_any(),
                }
            }
        })
    }

    // --- body measurement --------------------------------------------------

    #[test]
    fn measure_bare_body_table() {
        for (after, want) in [
            ("2", Some(1)),
            ("2 rest", Some(1)),
            ("i", Some(1)),
            ("2O", Some(2)),        // greedy: H_2O subscripts "2O"
            ("-12", Some(3)),       // leading sign
            ("+12", Some(3)),       //
            ("*", Some(1)),         // the `\*` alternative
            ("a.b", Some(3)),       // interior dot
            ("a.b.", Some(3)),      // trailing dot is not a legal end
            ("a,b", Some(3)),       //
            ("a\\b", Some(3)),      // interior backslash is in the class
            ("12,", Some(2)),       //
            ("caf\u{e9}", Some(5)), // unicode alnum
        ] {
            assert_eq!(measure_bare_body(after), want, "body of {after:?}");
        }
        for after in [
            "", " ", "-", "+", ".", ",", "\\alpha", // leading `\` is prefiltered out
            "!x", "-.",
        ] {
            assert_eq!(measure_bare_body(after), None, "no body for {after:?}");
        }
    }

    // --- recognition -------------------------------------------------------

    #[test]
    fn script_shapes_accept() {
        for (src, kind, want) in [
            ("x^2", OrgSyntaxKind::SUPERSCRIPT, "^2"),
            ("x_i", OrgSyntaxKind::SUBSCRIPT, "_i"),
            ("H_2O", OrgSyntaxKind::SUBSCRIPT, "_2O"),
            ("10^-12", OrgSyntaxKind::SUPERSCRIPT, "^-12"),
            ("x^{n+1}", OrgSyntaxKind::SUPERSCRIPT, "^{n+1}"),
            ("x_{}", OrgSyntaxKind::SUBSCRIPT, "_{}"),
            ("x^(2 - i)", OrgSyntaxKind::SUPERSCRIPT, "^(2 - i)"),
            ("x^*", OrgSyntaxKind::SUPERSCRIPT, "^*"),
            ("x^a.b", OrgSyntaxKind::SUPERSCRIPT, "^a.b"),
            ("x^a.b.", OrgSyntaxKind::SUPERSCRIPT, "^a.b"),
            ("x_a\\b", OrgSyntaxKind::SUBSCRIPT, "_a\\b"),
            ("x^{a{b{c}}}", OrgSyntaxKind::SUPERSCRIPT, "^{a{b{c}}}"),
            // Shortest balanced match: the first `}` closes.
            ("x^{a}{b}", OrgSyntaxKind::SUPERSCRIPT, "^{a}"),
        ] {
            let (_, evs, _) = run_script(src);
            assert_eq!(
                node_text(&evs, src, kind).as_deref(),
                Some(want),
                "node text for {src:?}"
            );
        }
    }

    #[test]
    fn script_shapes_reject() {
        for src in [
            "x^-",            // sign with no alnum
            "x_",             // nothing after the marker
            "x^ 2",           // space after the marker
            "x_\\alpha",      // leading backslash is prefiltered out
            "x^{a{b{c{d}}}}", // depth 4 matches nothing at all
            "x^{unclosed",    // unbalanced braces
            "x^(unclosed",    // unbalanced parens
        ] {
            let p = make_parser(src);
            // Probe at the marker (index 1 for these fixtures).
            let mut q = make_parser(src);
            q.eat_any();
            assert!(
                probe(&q).is_none(),
                "must not form a script: {src:?} (tokens from {:?})",
                p.current()
            );
        }
    }

    #[test]
    fn depth_four_yields_no_object_at_all() {
        let src = "x^{a{b{c{d}}}}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
        );
    }

    // --- PRE ---------------------------------------------------------------

    #[test]
    fn pre_requires_a_non_whitespace_char_before_the_marker() {
        // Line start, region start, and whitespace all reject.
        for src in ["_x", "^x"] {
            let p = make_parser(src);
            assert!(probe(&p).is_none(), "line start must reject: {src:?}");
        }
        for src in ["a _x", "a ^x"] {
            let mut p = make_parser(src);
            p.eat_any(); // TEXT
            p.eat_any(); // WHITESPACE
            assert!(probe(&p).is_none(), "space PRE must reject: {src:?}");
        }
        // Region start rejects even when the outer byte is a letter.
        let mut p = make_parser("x_2");
        p.eat_any();
        assert!(probe(&p).is_some(), "letter PRE accepts");
        p.with_inline_region_start(Some(p.cursor()), |pp| {
            assert!(probe(pp).is_none(), "region start must reject");
        });
    }

    #[test]
    fn pre_accepts_after_a_closing_delimiter() {
        // `*bold*_2` and `x^{a}_2`: the previous byte is punctuation, not space.
        for (src, want) in [("*bold*_2\n", "_2"), ("x^{a}_2\n", "_2")] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src);
            let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
            assert_eq!(subs.len(), 1, "one subscript in {src:?}");
            assert_eq!(subs[0].text().to_string(), want);
        }
    }

    #[test]
    fn script_probe_pure_and_parse_uses_shape() {
        let mut p = make_parser("x^{a}");
        p.eat_any();
        let before = p.cursor();
        let shape = probe(&p).expect("superscript");
        assert_eq!(shape.kind, OrgSyntaxKind::SUPERSCRIPT);
        assert!(matches!(shape.body, ScriptBody::Braced { .. }));
        assert_eq!(p.cursor(), before, "probe must not advance");
    }

    // --- contents ----------------------------------------------------------

    #[test]
    fn braced_contents_exclude_the_braces() {
        let src = "x_{a *b* c}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].text().to_string(), "_{a *b* c}");
        assert_eq!(
            nodes_of_kind(&subs[0], OrgSyntaxKind::BOLD).len(),
            1,
            "contents parse recursively"
        );
        // The braces are children of the subscript, not of the contents.
        assert!(
            subs[0]
                .children_with_tokens()
                .any(|c| c.kind() == OrgSyntaxKind::L_BRACE),
            "opening brace is a direct child"
        );
    }

    #[test]
    fn paren_contents_include_the_parens() {
        let src = "x^(a /b/ c)\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let sups = nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT);
        assert_eq!(sups.len(), 1);
        assert_eq!(sups[0].text().to_string(), "^(a /b/ c)");
        assert_eq!(nodes_of_kind(&sups[0], OrgSyntaxKind::ITALIC).len(), 1);
    }

    #[test]
    fn scripts_nest() {
        let src = "x_{a_b}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
        assert_eq!(subs.len(), 2, "outer and inner subscript");
        assert_eq!(subs[0].text().to_string(), "_{a_b}");
        assert_eq!(subs[1].text().to_string(), "_b");
    }

    /// The bare body is the one inline region that can stop inside a lex tile.
    #[test]
    fn bare_body_may_end_mid_tile() {
        let src = "x^a-b\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "tiling holds across a mid-tile end");
        let sups = nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT);
        assert_eq!(sups.len(), 1);
        assert_eq!(sups[0].text().to_string(), "^a");
    }

    #[test]
    fn bare_body_parses_entities() {
        // Boundary-aligned body: the entity is a node inside the script.
        let src = "x^a\\alpha\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let sups = nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT);
        assert_eq!(sups.len(), 1);
        assert_eq!(sups[0].text().to_string(), "^a\\alpha");
        assert_eq!(nodes_of_kind(&sups[0], OrgSyntaxKind::ENTITY).len(), 1);
    }

    /// Braced and paren bodies are bounded by the container, not the line.
    #[test]
    fn braced_body_may_cross_lines_inside_one_paragraph() {
        let src = "x_{a\nb} tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let subs = nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].text().to_string(), "_{a\nb}");
    }

    /// The prefilter excludes `\` as the *first* body character, so the
    /// backslash is reached as an entity instead.
    #[test]
    fn leading_backslash_is_an_entity_not_a_script_body() {
        let src = "x_\\alpha\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
        );
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].text().to_string(), "\\alpha");
    }

    /// A `_` that forms neither underline nor subscript stays an ordinary
    /// token, and the document still round-trips.
    #[test]
    fn declined_marker_stays_plain_text() {
        for src in ["x_ y\n", "x^ y\n", "_x\n", "a _ b\n"] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "roundtrip {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT).is_empty(),
                "{:?}",
                nodes_of_kind(&root, OrgSyntaxKind::SUBSCRIPT)
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT).is_empty(),
                "{:?}",
                nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT)
            );
        }
    }

    #[test]
    fn scripts_in_minimal_scopes() {
        // Sub/superscript join org `minimal-set`, so radio targets, link
        // descriptions and table cells all take them.
        let src = "<<<r x_1>>>\n[[https://x.com][d y^2]]\n| c z_3 |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        for (scope, kind, want) in [
            (OrgSyntaxKind::RADIO_TARGET, OrgSyntaxKind::SUBSCRIPT, "_1"),
            (
                OrgSyntaxKind::LINK_DESCRIPTION,
                OrgSyntaxKind::SUPERSCRIPT,
                "^2",
            ),
            (OrgSyntaxKind::TABLE_CELL, OrgSyntaxKind::SUBSCRIPT, "_3"),
        ] {
            let scopes = nodes_of_kind(&root, scope);
            assert_eq!(scopes.len(), 1, "one {scope:?}");
            let found = nodes_of_kind(&scopes[0], kind);
            assert_eq!(found.len(), 1, "{kind:?} inside {scope:?}");
            assert_eq!(found[0].text().to_string(), want);
        }
    }
}
