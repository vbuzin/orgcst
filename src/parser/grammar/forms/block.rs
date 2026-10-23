//! Block grammar: `#+begin_...` / `#+begin:` ... `#+end...` frames.
//!
//! Blocks walk their bodies in one of three ways ([`frame::BodyMode`]); being
//! a lesser block does not make a body opaque:
//!
//! - lesser non-verse (src/example/export/comment) keep body tokens verbatim;
//! - **verse** is lesser (no nested elements) but its body is an **object
//!   region** under the standard set - markup, links, targets, line breaks;
//! - greater blocks (quote/center), dynamic, and special parse nested elements.
//!
//! Element and object bodies run under a limit at the measured end line
//! (positions are absolute, so the bound survives consuming the begin line).
//!
//! Emits (on success):
//!   SRC_BLOCK | EXAMPLE_BLOCK | EXPORT_BLOCK | COMMENT_BLOCK | VERSE_BLOCK
//!   | QUOTE_BLOCK | CENTER_BLOCK | DYNAMIC_BLOCK | SPECIAL_BLOCK
//!     (begin line + body/contents + end line + optional post-blank tokens)
//!
//! Recognition is split so that a block which fails to close degrades cheaply:
//! - [`opener`] measures the opener line's shape without allocating.
//! - [`resolve`] validates the name, finds the matching end line through the
//!   amortised frame-end index ([`crate::parser::grammar::drive::frame`]), and
//!   yields a parse-ready [`BlockShape`] or a [`decide::DegradeReason`]. The
//!   diagnostic message is built only when the block degrades.
//! - A `#+begin_NAME` opener interrupts a paragraph only when `resolve`
//!   succeeds; the dynamic `#+begin:` form ([`BlockOpener::dynamic`]) always
//!   interrupts, even when degraded. [`decide::decide`] sets that flag and
//!   [`decide::interrupts_paragraph`] reads it.
//! - [`parse`] runs [`frame::parse_frame`] with the whole begin line as the
//!   opener and cannot fail.
//!
//! End lines must be whole lines: `#+end_NAME` / `#+end:` followed by
//! whitespace only (org `^[ \t]*#\+END%s[ \t]*$`).
//!
//! Only CST structure is built here; switches and body text are read by the
//! typed views.

use crate::parser::grammar::drive::{atoms, decide, frame};
use crate::parser::{LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Public surface for the element dispatcher
// ---------------------------------------------------------------------------

/// Opener-line measurement: `#+begin_` or `#+begin:` at line content.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockOpener {
    /// True for the keyword-shaped dynamic form `#+begin:`. Dynamic openers
    /// always interrupt paragraphs, even when [`resolve`] fails.
    pub(crate) dynamic: bool,
}

/// Parse-ready block measurement (kind, body end, body walk).
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockShape {
    /// Shared closed frame (`*_BLOCK`, end line, [`frame::BodyMode`]).
    frame: frame::FrameShape,
}

impl BlockShape {
    /// Exclusive body end: the start of the end line. List parsing uses it to
    /// skip the whole frame.
    #[inline]
    pub(crate) fn body_end(self) -> Pos {
        self.frame.body_end()
    }
}

/// Body walk for a closed block kind.
///
/// One arm per `*_BLOCK` kind [`block_kind`] can return. rustc cannot check
/// that: `OrgSyntaxKind` is the whole parser vocabulary, so the `_` arm below
/// is unavoidable. The real gate is the `block_body_modes_cover_every_block_kind`
/// table test, which pins name -> kind **and** kind -> mode, so a new block
/// kind cannot silently inherit another kind's body walk.
///
/// Never default unknown blocks to `Elements`: a missed [`OrgSyntaxKind::VERSE_BLOCK`]
/// arm would split stanzas into `PARAGRAPH`s and claim nested blocks / drawers
/// / lists inside a poem. Opaque fails closed - it invents no structure.
///
/// [`crate::ast::Block::body`] reads this function, so the grammar and the
/// view share one table.
#[inline]
pub(crate) fn block_body_mode(kind: OrgSyntaxKind) -> frame::BodyMode {
    match kind {
        // Lesser non-verse: opaque raw tiles (org `:value`).
        OrgSyntaxKind::SRC_BLOCK
        | OrgSyntaxKind::EXAMPLE_BLOCK
        | OrgSyntaxKind::EXPORT_BLOCK
        | OrgSyntaxKind::COMMENT_BLOCK => frame::BodyMode::Opaque,
        // Lesser with objects: org `(verse-block ,@standard-set)`. Objects
        // only, never nested elements.
        OrgSyntaxKind::VERSE_BLOCK => frame::BodyMode::Objects,
        // Greater / dynamic / special: nested elements.
        OrgSyntaxKind::QUOTE_BLOCK
        | OrgSyntaxKind::CENTER_BLOCK
        | OrgSyntaxKind::DYNAMIC_BLOCK
        | OrgSyntaxKind::SPECIAL_BLOCK => frame::BodyMode::Elements,
        // Not a block kind: `resolve` always passes `block_kind`'s output.
        _ => {
            debug_assert!(false, "block_body_mode: non-block kind {kind:?}");
            // Fail closed: opaque tiles never invent nested elements.
            frame::BodyMode::Opaque
        }
    }
}

/// Cheap opener-line shape: `#+` `begin` (`_` | `:`), case-insensitive.
///
/// Shape only - name and closedness are [`resolve`]'s job. `#+end...` lines
/// are not openers.
pub(crate) fn opener(line: LineView) -> Option<BlockOpener> {
    let (is_begin, dynamic) = marker_shape(line)?;
    is_begin.then_some(BlockOpener { dynamic })
}

/// Validate an opener into a parse-ready shape, or a Copy degrade reason.
///
/// `Err` is [`decide::DegradeReason::EmptyBlockName`] for bare `#+begin_` /
/// `#+begin:`, or [`decide::DegradeReason::UnterminatedBlock`] when no matching
/// end line exists under the visible window.
///
/// The end lookup is amortised ([`crate::parser::grammar::drive::frame`]) and
/// does not allocate. The diagnostic message is built later from the reason,
/// re-reading the name from the opener line.
pub(crate) fn resolve(
    line: LineView,
    op: BlockOpener,
) -> Result<BlockShape, decide::DegradeReason> {
    let name = name_range(line, op.dynamic);
    let name_text = line.text_range(name.0, name.1);
    if name_text.is_empty() {
        return Err(decide::DegradeReason::EmptyBlockName {
            dynamic: op.dynamic,
        });
    }

    let body_end = if op.dynamic {
        frame::dynamic_block_end_after(line.parser(), line)
    } else {
        frame::underscore_block_end_after(line.parser(), line, name_text)
    };
    let Some(body_end) = body_end else {
        return Err(decide::DegradeReason::UnterminatedBlock {
            dynamic: op.dynamic,
        });
    };

    let kind = block_kind(name_text, op.dynamic);
    Ok(BlockShape {
        frame: frame::FrameShape {
            kind,
            body_end,
            body: block_body_mode(kind),
            end_line: true,
        },
    })
}

/// Combined recognition for tests: opener plus successful [`resolve`].
#[cfg(test)]
pub(crate) fn probe(line: LineView) -> Option<BlockShape> {
    resolve(line, opener(line)?).ok()
}

/// Parse a complete block from its measured shape (no fail path).
///
/// Emits the specific `*_BLOCK` node and advances past the end line and
/// post-blank. Body / end line / blanks via [`frame::parse_frame`];
/// opener is a whole begin line (`eat_line`).
pub(crate) fn parse(parser: &mut Parser, shape: BlockShape) {
    frame::parse_frame(parser, shape.frame, |p| {
        p.eat_line();
    });
}

// ---------------------------------------------------------------------------
// Marker recognition
// ---------------------------------------------------------------------------

/// Map a block name (plus dynamic flag) to its concrete syntax kind.
///
/// Caseless comparison in place (no lowered allocation).
fn block_kind(name: &str, dynamic: bool) -> OrgSyntaxKind {
    if dynamic {
        return OrgSyntaxKind::DYNAMIC_BLOCK;
    }

    match name {
        n if n.eq_ignore_ascii_case("src") => OrgSyntaxKind::SRC_BLOCK,
        n if n.eq_ignore_ascii_case("example") => OrgSyntaxKind::EXAMPLE_BLOCK,
        n if n.eq_ignore_ascii_case("export") => OrgSyntaxKind::EXPORT_BLOCK,
        n if n.eq_ignore_ascii_case("comment") => OrgSyntaxKind::COMMENT_BLOCK,
        n if n.eq_ignore_ascii_case("verse") => OrgSyntaxKind::VERSE_BLOCK,
        n if n.eq_ignore_ascii_case("quote") => OrgSyntaxKind::QUOTE_BLOCK,
        n if n.eq_ignore_ascii_case("center") => OrgSyntaxKind::CENTER_BLOCK,
        _ => OrgSyntaxKind::SPECIAL_BLOCK,
    }
}

/// `#+` (begin|end) (`_`|`:`) at line content: `(is_begin, dynamic)`.
fn marker_shape(line: LineView) -> Option<(bool, bool)> {
    let key = atoms::line_keyword_prefix(line)?;
    let is_begin = line.text_eq_caseless(key, "begin");
    let is_end = line.text_eq_caseless(key, "end");
    if !is_begin && !is_end {
        return None;
    }

    let dynamic = match line.kind(key + 1) {
        OrgSyntaxKind::COLON => true,
        OrgSyntaxKind::UNDERSCORE => false,
        _ => return None,
    };
    Some((is_begin, dynamic))
}

/// Content-relative token range `[from, to)` of the marker's name.
///
/// Underscore form: everything glued after `begin_` / `end_` (UNDERSCORE
/// tokens kept, e.g. `foo_bar`). Dynamic form: optional whitespace after the
/// colon, then the glued run. Empty range for bare `#+begin_` / `#+begin:`.
///
/// Precondition: [`marker_shape`] accepted the line (`#+` + begin/end word).
pub(crate) fn name_text<'s>(line: LineView<'_, 's>, dynamic: bool) -> &'s str {
    let (from, to) = name_range(line, dynamic);
    line.text_range(from, to)
}

fn name_range(line: LineView, dynamic: bool) -> (usize, usize) {
    // After `#+` key (`begin`/`end`) and the `_` or `:` marker: key + 2.
    let key = atoms::line_keyword_prefix(line)
        .expect("name_range requires a line that marker_shape accepted");
    let mut k = key + 2;
    if dynamic {
        k += line.run_while(k, |kind, _| kind == OrgSyntaxKind::WHITESPACE);
    }
    let from = k;
    k += line.run_while(k, |kind, _| !kind.is_line_end_or_space());
    (from, k)
}

/// Whether only whitespace follows content token `k` on this line.
fn rest_is_blank(line: LineView, k: usize) -> bool {
    let ws = line.run_while(k, |kind, _| kind == OrgSyntaxKind::WHITESPACE);
    line.kind(k + ws).is_line_end()
}

/// Valid whole-line dynamic end: `#+end:` with empty name and trailing ws only.
pub(crate) fn is_dynamic_end_line(line: LineView) -> bool {
    let Some((is_begin, dynamic)) = marker_shape(line) else {
        return false;
    };
    if is_begin || !dynamic {
        return false;
    }
    let (from, to) = name_range(line, true);
    line.text_range(from, to).is_empty() && rest_is_blank(line, to)
}

/// Valid whole-line underscore end: `#+end_NAME` with non-empty name, rest blank.
pub(crate) fn is_underscore_end_line(line: LineView) -> bool {
    let Some((is_begin, dynamic)) = marker_shape(line) else {
        return false;
    };
    if is_begin || dynamic {
        return false;
    }
    let (from, to) = name_range(line, false);
    !line.text_range(from, to).is_empty() && rest_is_blank(line, to)
}

/// Whether a pre-validated underscore end line's name matches `name` (caseless).
pub(crate) fn underscore_end_name_matches(line: LineView, name: &str) -> bool {
    let (from, to) = name_range(line, false);
    line.text_range(from, to).eq_ignore_ascii_case(name)
}

#[cfg(test)]
mod tests {
    use crate::parser::grammar::drive::frame;
    use crate::parser::grammar::forms::block::{
        block_body_mode, block_kind, opener, parse, probe, resolve,
    };
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        ParseResult, diags_any, make_parser, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, probe_line, run_element, tree_text,
    };

    fn results_of(src: &str) -> ParseResult {
        run_element(src, probe, parse)
    }

    /// Resolve the first line's opener, or panic when it is not begin-shaped.
    fn resolve_first(
        src: &str,
    ) -> Result<(), crate::parser::grammar::drive::decide::DegradeReason> {
        let p = make_parser(src);
        let line = p.line().expect("line start");
        let op = opener(line).expect("begin-shaped opener");
        resolve(line, op).map(|_| ())
    }

    // --- opener / probe (shape) --------------------------------------------

    #[test]
    fn detects_variants_and_indents() {
        assert!(probe_line("#+begin_src\n", opener).is_some());
        assert!(probe_line("  #+BEGIN_EXAMPLE\n", opener).is_some());
        assert!(probe_line("\t#+begin: clocktable\n", opener).is_some());
        assert!(probe_line("#+begin_foo\n", opener).is_some());
        assert!(probe_line("#+begin:\n", opener).is_some()); // shape even if empty name
    }

    #[test]
    fn rejects_non_openers() {
        assert!(probe_line("#+title: x\n", opener).is_none());
        assert!(probe_line("#+begin\n", opener).is_none());
        assert!(probe_line("foo #+begin_src\n", opener).is_none());
        assert!(probe_line("#+beginning\n", opener).is_none());
        // End markers are not openers (begin-only contract).
        assert!(probe_line("#+end_src\n", opener).is_none());
        assert!(probe_line("#+end:\n", opener).is_none());
    }

    #[test]
    fn resolve_validates_name_and_end() {
        use crate::parser::grammar::drive::decide::DegradeReason;
        assert!(resolve_first("#+begin_src\nx\n#+end_src\n").is_ok());
        assert!(matches!(
            resolve_first("#+begin_src\nx\n"),
            Err(DegradeReason::UnterminatedBlock { dynamic: false, .. })
        ));
        assert!(matches!(
            resolve_first("#+begin_\nx\n"),
            Err(DegradeReason::EmptyBlockName { dynamic: false })
        ));
        assert!(matches!(
            resolve_first("#+begin:\nx\n#+end:\n"),
            Err(DegradeReason::EmptyBlockName { dynamic: true })
        ));
        // End lines are whole lines: trailing junk does not close.
        assert!(matches!(
            resolve_first("#+begin_src\nx\n#+end_src extra\n"),
            Err(DegradeReason::UnterminatedBlock { .. })
        ));
        // Dynamic end is bare `#+end:`; a named end line does not close.
        assert!(matches!(
            resolve_first("#+begin: d\nx\n#+end: d\n"),
            Err(DegradeReason::UnterminatedBlock { dynamic: true, .. })
        ));
        assert!(resolve_first("#+begin: d\nx\n#+end:  \n").is_ok());
    }

    // --- Successful blocks (node kinds) -------------------------------------

    #[test]
    fn src_block_kind_and_body() {
        let src = "#+begin_src rust\nfn x() {}\n#+end_src\n";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::SRC_BLOCK));
        let text = node_text(&evs, src, OrgSyntaxKind::SRC_BLOCK).unwrap();
        assert!(text.contains("begin_src"));
        assert!(text.contains("fn x() {}"));
        assert!(text.contains("end_src"));
    }

    #[test]
    fn example_block_kind() {
        let src = "#+begin_example\nraw\n#+end_example\n";
        let (_, evs, _) = results_of(src);
        assert_eq!(node_kinds(&evs), vec![OrgSyntaxKind::EXAMPLE_BLOCK]);
    }

    #[test]
    fn quote_block_has_child_paragraph() {
        let src = "#+begin_quote\nHello *world*.\n#+end_quote\n";
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::QUOTE_BLOCK));
        assert!(kinds.contains(&OrgSyntaxKind::PARAGRAPH));
        let block_text = node_text(&evs, src, OrgSyntaxKind::QUOTE_BLOCK).unwrap();
        assert!(block_text.contains("Hello"));
    }

    #[test]
    fn dynamic_block_kind() {
        let src = "#+begin: mydyn :param 1\nbody\n#+end:\n";
        let (_, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::DYNAMIC_BLOCK));
    }

    #[test]
    fn special_block_for_unknown_name() {
        let src = "#+begin_foo_bar\nx\n#+end_foo_bar\n";
        let (_, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::SPECIAL_BLOCK));
    }

    // --- Unterminated falls back (no block node) ----------------------------

    #[test]
    fn degraded_openers_via_dispatcher_emit_diag_and_paragraph() {
        // (source, expected diag predicate)
        type DiagPred = fn(&crate::DiagnosticKind) -> bool;
        let cases: &[(&str, DiagPred)] = &[
            ("#+begin_src\nx = 1\n", |k| {
                matches!(
                    k,
                    crate::DiagnosticKind::UnterminatedBlock { name, dynamic: false }
                        if name.eq_ignore_ascii_case("src")
                )
            }),
            ("#+begin_\nx\n", |k| {
                matches!(k, crate::DiagnosticKind::EmptyBlockName { dynamic: false })
            }),
            ("#+begin:\nx\n", |k| {
                matches!(k, crate::DiagnosticKind::EmptyBlockName { dynamic: true })
            }),
            ("#+begin: clocktable\nbody\n", |k| {
                matches!(
                    k,
                    crate::DiagnosticKind::UnterminatedBlock { name, dynamic: true }
                        if name.eq_ignore_ascii_case("clocktable")
                )
            }),
        ];
        for &(src, pred) in cases {
            let mut p = make_parser(src);
            crate::parser::grammar::drive::region::parse_one_element(&mut p);
            let (_, evs, diags) = p.finish();
            assert!(
                !node_kinds(&evs).iter().any(|k| matches!(
                    k,
                    OrgSyntaxKind::SRC_BLOCK
                        | OrgSyntaxKind::SPECIAL_BLOCK
                        | OrgSyntaxKind::DYNAMIC_BLOCK
                )),
                "no block node for {src:?}"
            );
            assert!(
                node_kinds(&evs).contains(&OrgSyntaxKind::PARAGRAPH),
                "degraded opener becomes paragraph text for {src:?}"
            );
            assert!(
                diags_any(&diags, pred),
                "expected diag for {src:?}: {diags:?}"
            );
        }
    }

    #[test]
    fn unterminated_block_full_document_fallthrough() {
        // Dispatcher recovery: no block node, paragraph fallthrough, one diag
        // covering the opener line (newline excluded).
        let src = "#+begin_src\nx = 1\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::SRC_BLOCK).len(),
            0,
            "unterminated opener is not a block node"
        );
        assert!(
            !nodes_of_kind(out.root(), OrgSyntaxKind::PARAGRAPH).is_empty(),
            "expected paragraph fallthrough"
        );
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(
                k,
                crate::DiagnosticKind::UnterminatedBlock { name, dynamic: false }
                    if name.eq_ignore_ascii_case("src")
            )
        }));
        let d = out
            .diagnostics()
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::UnterminatedBlock { .. }))
            .expect("unterminated block diag");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "#+begin_src");
    }

    #[test]
    fn empty_block_name_full_document_fallthrough() {
        let src = "#+begin_\nx\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::SRC_BLOCK).len()
                + nodes_of_kind(out.root(), OrgSyntaxKind::SPECIAL_BLOCK).len()
                + nodes_of_kind(out.root(), OrgSyntaxKind::DYNAMIC_BLOCK).len(),
            0,
            "empty name is not a block node"
        );
        assert!(
            !nodes_of_kind(out.root(), OrgSyntaxKind::PARAGRAPH).is_empty(),
            "expected paragraph fallthrough"
        );
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(k, crate::DiagnosticKind::EmptyBlockName { dynamic: false })
        }));
        let d = out
            .diagnostics()
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::EmptyBlockName { .. }))
            .expect("empty block name diag");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "#+begin_");
    }

    // --- Lesser non-verse body is opaque (no inner heading) -----------------

    #[test]
    fn non_verse_lesser_body_does_not_parse_inner_headings() {
        let src = "#+begin_example\n* not a heading\n#+end_example\n";
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::EXAMPLE_BLOCK));
        // The * must be plain TEXT inside the block, not turned into HEADING
        assert!(!kinds.contains(&OrgSyntaxKind::HEADING));
        let text = node_text(&evs, src, OrgSyntaxKind::EXAMPLE_BLOCK).unwrap();
        assert!(text.contains("* not a heading"));
    }

    // --- Greater recurses, including nested blocks --------------------------

    #[test]
    fn greater_recurses_to_inner_block() {
        let src = "#+begin_quote\n#+begin_example\ninner\n#+end_example\n#+end_quote\n";
        let (_, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::QUOTE_BLOCK));
        assert!(kinds.contains(&OrgSyntaxKind::EXAMPLE_BLOCK));
    }

    // --- Header material is present as tokens -------------------------------

    #[test]
    fn src_header_tokens_preserved() {
        let src = "#+begin_src rust -n 10 :results output\ncode\n#+end_src\n";
        let (_toks, evs, _) = results_of(src);
        let text = node_text(&evs, src, OrgSyntaxKind::SRC_BLOCK).unwrap();
        assert!(text.contains("rust"));
        assert!(text.contains("-n"));
    }

    // --- Body mode table ----------------------------------------------------

    /// The gate rustc cannot give us: `block_body_mode` must have an arm for
    /// every kind [`block_kind`] can return.
    ///
    /// Pinned in **both** directions - name -> kind and kind -> mode - so a new
    /// block kind cannot silently inherit another kind's body walk.
    #[test]
    fn block_body_modes_cover_every_block_kind() {
        use frame::BodyMode::{Elements, Objects, Opaque};

        // (name, dynamic, kind, mode) for every kind `block_kind` yields.
        let table: &[(&str, bool, OrgSyntaxKind, frame::BodyMode)] = &[
            ("src", false, OrgSyntaxKind::SRC_BLOCK, Opaque),
            ("example", false, OrgSyntaxKind::EXAMPLE_BLOCK, Opaque),
            ("export", false, OrgSyntaxKind::EXPORT_BLOCK, Opaque),
            ("comment", false, OrgSyntaxKind::COMMENT_BLOCK, Opaque),
            // Lesser, but objects - the whole point of BodyMode.
            ("verse", false, OrgSyntaxKind::VERSE_BLOCK, Objects),
            ("quote", false, OrgSyntaxKind::QUOTE_BLOCK, Elements),
            ("center", false, OrgSyntaxKind::CENTER_BLOCK, Elements),
            ("whatever", false, OrgSyntaxKind::SPECIAL_BLOCK, Elements),
            ("", true, OrgSyntaxKind::DYNAMIC_BLOCK, Elements),
        ];

        for &(name, dynamic, kind, mode) in table {
            assert_eq!(block_kind(name, dynamic), kind, "block_kind({name:?})");
            assert_eq!(block_body_mode(kind), mode, "block_body_mode({kind:?})");
        }

        // Closure: every kind `block_kind` can produce is in the table. Names
        // are caseless and unknown names fall to SPECIAL_BLOCK, so this corpus
        // covers the whole codomain.
        let covered: Vec<_> = table.iter().map(|&(_, _, k, _)| k).collect();
        for name in [
            "src",
            "SRC",
            "Example",
            "export",
            "comment",
            "verse",
            "VERSE",
            "quote",
            "center",
            "unknown",
            "",
            "src_extra",
        ] {
            for dynamic in [false, true] {
                let kind = block_kind(name, dynamic);
                assert!(
                    covered.contains(&kind),
                    "block_kind({name:?}, dynamic={dynamic}) -> {kind:?} is missing from the table"
                );
            }
        }
    }

    // --- Verse: objects body ------------------------------------------------

    /// Positive check for the verse tests below: without
    /// `restrictions(VERSE_BLOCK) = STANDARD` the body would tile as bare
    /// tokens and every "no element child" assertion would still pass.
    #[test]
    fn verse_body_parses_standard_objects() {
        let src = "#+begin_verse\n*bold* and /it/ <<t>> <<<radio>>> [[a]] \\alpha\n#+end_verse\n";
        let root = parse_document_tree(src);
        let verse = only_verse(&root);
        for kind in [
            OrgSyntaxKind::BOLD,
            OrgSyntaxKind::ITALIC,
            OrgSyntaxKind::TARGET,
            OrgSyntaxKind::RADIO_TARGET,
            OrgSyntaxKind::LINK,
            OrgSyntaxKind::ENTITY,
        ] {
            assert!(
                !nodes_of_kind(&verse, kind).is_empty(),
                "{kind:?} must parse inside a verse body"
            );
        }
        assert_eq!(tree_text(&root), src);
    }

    /// Verse is where `\\` line breaks matter most (poetry).
    #[test]
    fn verse_body_parses_line_break() {
        let src = "#+begin_verse\nline one \\\\\nline two\n#+end_verse\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&only_verse(&root), OrgSyntaxKind::LINE_BREAK).len(),
            1
        );
        assert_eq!(tree_text(&root), src);
    }

    /// Lesser: the body holds objects but never *elements*.
    #[test]
    fn verse_body_has_no_element_children() {
        let src = "#+begin_verse\n* not a heading\n- not a list\n| not | a table |\n#+end_verse\n";
        let root = parse_document_tree(src);
        let verse = only_verse(&root);
        for kind in [
            OrgSyntaxKind::HEADING,
            OrgSyntaxKind::PARAGRAPH,
            OrgSyntaxKind::PLAIN_LIST,
            OrgSyntaxKind::TABLE,
            OrgSyntaxKind::SRC_BLOCK,
            OrgSyntaxKind::DRAWER,
        ] {
            assert!(
                nodes_of_kind(&verse, kind).is_empty(),
                "{kind:?} must not appear under a verse body"
            );
        }
        assert_eq!(tree_text(&root), src);
    }

    /// A star line is neither a headline nor bold (org opening border rejects
    /// whitespace after the marker); it stays bare tokens.
    #[test]
    fn verse_star_line_is_not_heading_or_bold() {
        let src = "#+begin_verse\n* not a heading\n#+end_verse\n";
        let root = parse_document_tree(src);
        let verse = only_verse(&root);
        assert!(
            nodes_of_kind(&verse, OrgSyntaxKind::HEADING).is_empty(),
            "{:?}",
            nodes_of_kind(&verse, OrgSyntaxKind::HEADING)
        );
        assert!(
            nodes_of_kind(&verse, OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            nodes_of_kind(&verse, OrgSyntaxKind::BOLD)
        );
        assert!(
            verse
                .children_with_tokens()
                .any(|c| c.kind() == OrgSyntaxKind::STAR),
            "bare STAR token must survive under the verse body"
        );
        assert_eq!(tree_text(&root), src);
    }

    /// No element loop runs inside the body, so `#+` is token material.
    #[test]
    fn verse_hash_plus_line_is_not_a_keyword() {
        let src = "#+begin_verse\n#+foo: bar\n#+end_verse\n";
        let root = parse_document_tree(src);
        let verse = only_verse(&root);
        assert!(
            nodes_of_kind(&verse, OrgSyntaxKind::KEYWORD).is_empty(),
            "{:?}",
            nodes_of_kind(&verse, OrgSyntaxKind::KEYWORD)
        );
        assert!(tree_text(&verse).contains("#+foo: bar"));
        assert_eq!(tree_text(&root), src);
    }

    /// Blanks stay interior: no body split, no inner paragraph post-blank.
    #[test]
    fn verse_empty_blank_only_and_interior_blank_bodies() {
        for src in [
            "#+begin_verse\n#+end_verse\n",
            "#+begin_verse\n\n\n#+end_verse\n",
            "#+begin_verse\nstanza one\n\nstanza two\n#+end_verse\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::VERSE_BLOCK).len(),
                1,
                "exactly one VERSE_BLOCK for {src:?}"
            );
            assert!(
                nodes_of_kind(&only_verse(&root), OrgSyntaxKind::PARAGRAPH).is_empty(),
                "interior blanks must not create paragraphs for {src:?}"
            );
            assert_eq!(tree_text(&root), src, "tiling for {src:?}");
        }
    }

    /// A verse body can offer markup a span with two or more newlines, which
    /// paragraph content never does (it stops at a blank line). Org's emphasis
    /// newline budget still applies, so emphasis cannot bind across stanzas.
    #[test]
    fn verse_emphasis_respects_newline_budget() {
        let cases: &[(&str, bool)] = &[
            ("*a*", true),
            ("*a\nb*", true),     // one interior newline: within budget
            ("*a\nb\nc*", false), // two newlines: over budget
            ("*a\n\nb*", false),  // blank line is two newlines
        ];
        for &(body, want_bold) in cases {
            let src = format!("#+begin_verse\n{body}\n#+end_verse\n");
            let root = parse_document_tree(&src);
            let has_bold = !nodes_of_kind(&only_verse(&root), OrgSyntaxKind::BOLD).is_empty();
            assert_eq!(has_bold, want_bold, "BOLD for verse body {body:?}");
            assert_eq!(tree_text(&root), src, "tiling for {body:?}");
        }
    }

    /// The first matching end line closes the block; the objects body walk
    /// does not change how the end is found.
    #[test]
    fn nested_verse_first_end_wins() {
        let src = "#+begin_verse\n#+begin_verse\n*x*\n#+end_verse\nleftover\n#+end_verse\n";
        let root = parse_document_tree(src);
        let verses = nodes_of_kind(&root, OrgSyntaxKind::VERSE_BLOCK);
        assert_eq!(verses.len(), 1, "outer only; first #+end_verse closes");
        assert!(
            !tree_text(&verses[0]).contains("leftover"),
            "material after the first end is outside the frame"
        );
        assert!(
            !nodes_of_kind(&verses[0], OrgSyntaxKind::BOLD).is_empty(),
            "{:?}",
            nodes_of_kind(&verses[0], OrgSyntaxKind::BOLD)
        );
        assert_eq!(tree_text(&root), src);
    }

    /// Quote body is Elements, verse body is Objects - both in one tree.
    #[test]
    fn verse_inside_quote_and_list_item() {
        for src in [
            "#+begin_quote\n#+begin_verse\n*x*\n#+end_verse\n#+end_quote\n",
            "- item\n  #+begin_verse\n  *x*\n  #+end_verse\n",
        ] {
            let root = parse_document_tree(src);
            let verses = nodes_of_kind(&root, OrgSyntaxKind::VERSE_BLOCK);
            assert_eq!(verses.len(), 1, "verse claimed in {src:?}");
            assert!(
                !nodes_of_kind(&verses[0], OrgSyntaxKind::BOLD).is_empty(),
                "objects inside nested verse for {src:?}"
            );
            assert_eq!(tree_text(&root), src, "tiling for {src:?}");
        }
    }

    // --- Regression: lesser NON-verse stays opaque --------------------------

    /// Twin of `non_verse_lesser_body_does_not_parse_inner_headings`: the four
    /// opaque bodies get no object children, unlike verse.
    #[test]
    fn non_verse_lesser_body_is_opaque() {
        for name in ["src", "example", "export", "comment"] {
            let src = format!("#+begin_{name}\n*bold* and /it/\n#+end_{name}\n");
            let root = parse_document_tree(&src);
            for kind in [OrgSyntaxKind::BOLD, OrgSyntaxKind::ITALIC] {
                assert!(
                    nodes_of_kind(&root, kind).is_empty(),
                    "{name} body must stay opaque, found {kind:?}"
                );
            }
            assert_eq!(tree_text(&root), src);
        }
    }

    /// Comma-quoted lines stay raw in the CST: nothing is unquoted and no
    /// escape nodes are added. Callers unquote with `unquote_lesser_body`.
    #[test]
    fn comma_quoting_stays_raw_in_block_bodies() {
        for src in [
            // Opaque body: org would unquote these at interpret time.
            "#+begin_example\n,*not a heading\n#+end_example\n",
            "#+begin_example\n,,*x\n#+end_example\n",
            "#+begin_example\n,#+end_example\n#+end_example\n",
            // Verse never supports comma-quoting (org explicit).
            "#+begin_verse\n,*x\n#+end_verse\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "byte-exact tiling for {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::HEADING).is_empty(),
                "quoted star must not become a heading in {src:?}"
            );
        }
    }

    /// Single `VERSE_BLOCK` in the tree, for per-node assertions.
    fn only_verse(root: &crate::syntax::SyntaxNode) -> crate::syntax::SyntaxNode {
        let mut found = nodes_of_kind(root, OrgSyntaxKind::VERSE_BLOCK);
        assert_eq!(found.len(), 1, "expected exactly one VERSE_BLOCK");
        found.remove(0)
    }
}
