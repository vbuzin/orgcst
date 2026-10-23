//! Shared parse path for frames: drawers, blocks, footnote definitions and
//! latex environments.
//!
//! A frame opens on a line and owns a body, an optional end line and its
//! trailing blank lines. Greater elements, lesser blocks (opaque bodies) and
//! verse (an object body) all fit; [`BodyMode`] says which. Each construct's
//! module keeps its own recognition (`opener` / `resolve`) and opener line;
//! this module parses the body, the end line and the blank lines.
//!
//! It also holds the per-parse cache of END lines, so that many unclosed
//! openers do not each rescan the rest of the document.
//!
//! List structure skips closed drawer/block/latex-env interiors via
//! [`super::region::opaque_frame_end`].

use crate::parser::grammar::forms::{block, drawer, heading, latex};
use crate::parser::{LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

/// How a closed frame walks its body region.
///
/// Three cases; "lesser" does not mean "opaque":
///
/// - lesser non-verse (src / example / export / comment) and latex
///   environments hold raw text -> [`BodyMode::Opaque`];
/// - greater containers (quote / center / dynamic / special, drawers,
///   footnote definitions) hold **elements** -> [`BodyMode::Elements`];
/// - verse is *lesser* (no nested elements) yet its contents are
///   **objects** -> [`BodyMode::Objects`].
///
/// Paragraph occupies the same objects cell as verse but is not a frame (no
/// delimiters; its end comes from termination rules), so it reaches
/// [`super::region::inline_in`] by its own path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyMode {
    /// Verbatim token tiles until `body_end` (`eat_until`).
    Opaque,
    /// Nested elements under a limit at `body_end`
    /// ([`super::region::elements_in`]).
    Elements,
    /// Inline objects under a limit at `body_end`
    /// ([`super::region::inline_in`]) with the frame's completed kind as the
    /// restriction scope. Only verse blocks use it.
    Objects,
}

/// Parse-ready closed frame: kind, exclusive body end, body walk, end line.
///
/// Produced by each construct's `resolve` and parsed by [`parse_frame`].
/// `Copy`, so it can be part of a measured element shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameShape {
    /// Node kind completed after the body (and optional end line). Doubles as
    /// the restriction scope under [`BodyMode::Objects`].
    pub(crate) kind: OrgSyntaxKind,
    /// Exclusive end of the body (start of end line for drawer / block / latex
    /// environment; contents terminator for footnote definitions).
    pub(crate) body_end: Pos,
    /// Body walk: opaque tiles, nested elements, or inline objects.
    pub(crate) body: BodyMode,
    /// `true`: consume one whole end line after the body (drawer / block /
    /// latex environment); `false`: no end line (footnote definition).
    pub(crate) end_line: bool,
}

impl FrameShape {
    /// Exclusive body end (list structure / opaque skip).
    #[inline]
    pub(crate) fn body_end(self) -> Pos {
        self.body_end
    }
}

/// Parse a measured frame: opener callback, body, optional end line, blanks.
///
/// ```text
/// start
///   open(parser)                            // family-specific opener emit
///   eat_until | elements_in | inline_in     // body under body_end (BodyMode)
///   [eat_line]                              // end line when frame.end_line
///   eat_blank_lines
/// complete(kind)
/// ```
///
/// Precondition: cursor at the opener line (or header start for footnotes).
/// `open` must advance past the opener so the body region starts cleanly.
#[inline]
pub(crate) fn parse_frame(parser: &mut Parser, frame: FrameShape, open: impl FnOnce(&mut Parser)) {
    let marker = parser.start();
    open(parser);
    match frame.body {
        BodyMode::Opaque => parser.eat_until(frame.body_end),
        BodyMode::Elements => super::region::elements_in(parser, frame.body_end),
        // The scope is the frame's own kind, as a paragraph's is PARAGRAPH.
        BodyMode::Objects => super::region::inline_in(parser, frame.body_end, frame.kind),
    }
    if frame.end_line {
        parser.eat_line();
    }
    parser.eat_blank_lines();
    marker.complete(parser, frame.kind);
}

// ---------------------------------------------------------------------------
// Cached END line index
// ---------------------------------------------------------------------------

/// Next-END tables for the whole document, built lazily once per parse.
#[derive(Debug)]
pub(crate) struct FrameEndCache {
    /// First drawer `:END:` strictly after line `i`, with no heading between.
    drawer_next: Vec<Option<Pos>>,
    /// First dynamic `#+end:` strictly after line `i`.
    dynamic_next: Vec<Option<Pos>>,
    /// Valid whole-line underscore `#+end_NAME` markers as
    /// `(caseless name key, line index)`, sorted by both.
    ///
    /// Keyed by name so N unclosed openers of one name do not rescan M end
    /// markers of another (the look-up binary-searches its own bucket).
    underscore_ends: Vec<(u64, u32)>,
    /// End lines matching org `\end{NAME}[ \t]*$` (prefix allowed):
    /// `(caseless name key, line index)`, sorted by both.
    latex_ends: Vec<(u64, u32)>,
    /// First heading line index strictly after line `i`, or `n` (sentinel) when
    /// none remain. A latex environment must end inside its section:
    /// `opener < end < next_heading_after(opener)`.
    next_heading_after: Vec<u32>,
}

impl FrameEndCache {
    /// One O(lines) reverse + collect pass over the **full** line table.
    ///
    /// Uses [`Parser::full_line_at`], so building it inside a nested region
    /// cannot leave a cached index that misses lines past that region.
    fn build(parser: &Parser<'_>) -> Self {
        let n = parser.line_table_len();
        #[cfg(test)]
        {
            // Count every line touched while building.
            parser
                .frame_end_work
                .set(parser.frame_end_work.get().saturating_add(n));
        }

        let mut drawer_next = vec![None; n];
        let mut dynamic_next = vec![None; n];
        let mut underscore_ends = Vec::new();
        let mut latex_ends = Vec::new();
        let mut next_heading_after = vec![0u32; n];

        // Reverse: propagate "next end after this line"; headings clear drawer.
        // Also propagate next-heading-after for latex section bounds.
        let mut next_drawer: Option<Pos> = None;
        let mut next_dynamic: Option<Pos> = None;
        let mut next_heading = n as u32;
        for i in (0..n).rev() {
            let line = parser
                .full_line_at(i)
                .expect("line index in 0..line_table_len");

            next_heading_after[i] = next_heading;

            if heading::probe(line)
                .and_then(heading::StarLine::as_heading)
                .is_some()
            {
                next_drawer = None;
                next_heading = i as u32;
            }

            drawer_next[i] = next_drawer;
            dynamic_next[i] = next_dynamic;

            if drawer::is_end_line(line) {
                next_drawer = Some(line.start());
            }
            if block::is_dynamic_end_line(line) {
                next_dynamic = Some(line.start());
            }
            if block::is_underscore_end_line(line) {
                underscore_ends.push((caseless_name_key(block::name_text(line, false)), i as u32));
            }
            if let Some(name) = latex::end_name(line) {
                latex_ends.push((caseless_name_key(name), i as u32));
            }
        }
        // Collected in reverse line order; sort by (name key, line) so a
        // look-up can binary-search its bucket and walk it in line order.
        underscore_ends.sort_unstable();
        latex_ends.sort_unstable();

        Self {
            drawer_next,
            dynamic_next,
            underscore_ends,
            latex_ends,
            next_heading_after,
        }
    }
}

/// Build the cache if needed, then run `f` against it.
///
/// The only place `frame_ends` is borrowed; no look-up may hold the borrow
/// across a call that could re-enter [`ensure`].
fn with_cache<R>(parser: &Parser<'_>, f: impl FnOnce(&FrameEndCache) -> R) -> R {
    ensure(parser);
    let cache = parser.frame_ends.borrow();
    f(cache.as_ref().expect("frame_ends built by ensure"))
}

/// Drawer `:END:` start after `line`, if visible under the active limit.
pub(crate) fn drawer_end_after(parser: &Parser<'_>, line: LineView<'_, '_>) -> Option<Pos> {
    let idx = line.id().index();
    let end = with_cache(parser, |c| c.drawer_next.get(idx).copied().flatten())?;
    visible(parser, end)
}

/// Dynamic `#+end:` start after `line`, if visible under the active limit.
pub(crate) fn dynamic_block_end_after(parser: &Parser<'_>, line: LineView<'_, '_>) -> Option<Pos> {
    let idx = line.id().index();
    let end = with_cache(parser, |c| c.dynamic_next.get(idx).copied().flatten())?;
    visible(parser, end)
}

/// Matching underscore `#+end_NAME` start after `line`, if visible.
pub(crate) fn underscore_block_end_after(
    parser: &Parser<'_>,
    line: LineView<'_, '_>,
    name: &str,
) -> Option<Pos> {
    let start_id = line.id().index() as u32;
    let limit = parser.limit().index();
    // The loop body calls only pure line probes, so holding the cache borrow
    // across it cannot re-enter `ensure`.
    let key = caseless_name_key(name);
    with_cache(parser, |cache| {
        // No section half-open bound for underscore blocks (u32::MAX).
        keyed_end_after(
            parser,
            &cache.underscore_ends,
            start_id,
            key,
            u32::MAX,
            limit,
            |end_line| block::underscore_end_name_matches(end_line, name),
        )
    })
}

/// First matching `\end{NAME}` strictly after `line` and **strictly before**
/// the next headline after `line` (or EOF), visible under the active limit.
///
/// Org requires the end inside the opener's section:
/// `opener < end < next_heading_after(opener)`. That one inequality rejects
/// both a heading in between and an end on a headline line (Org allows text
/// before `\end{NAME}`). The active region limit applies on top.
pub(crate) fn latex_env_end_after(
    parser: &Parser<'_>,
    line: LineView<'_, '_>,
    name: &str,
) -> Option<Pos> {
    let start_id = line.id().index() as u32;
    let limit = parser.limit().index();
    let key = caseless_name_key(name);
    with_cache(parser, |cache| {
        let bound = *cache.next_heading_after.get(start_id as usize)?;
        keyed_end_after(
            parser,
            &cache.latex_ends,
            start_id,
            key,
            bound,
            limit,
            |end_line| latex::end_name_matches(end_line, name),
        )
    })
}

/// Shared name-keyed end look-up for underscore blocks and latex environments.
///
/// `ends` is a sorted `(caseless name key, line index)` table. Walks the name
/// bucket after `start_id`, stops past `bound` (the section end for latex;
/// `u32::MAX` when unbounded) or the active `limit`, and confirms each hit
/// with `confirm` on the full end line (a region cap may hide its tokens).
fn keyed_end_after(
    parser: &Parser<'_>,
    ends: &[(u64, u32)],
    start_id: u32,
    key: u64,
    bound: u32,
    limit: usize,
    confirm: impl Fn(LineView<'_, '_>) -> bool,
) -> Option<Pos> {
    // First end with this name key strictly after the opener line.
    let i = ends.partition_point(|&(h, l)| h < key || (h == key && l <= start_id));
    for &(h, end_idx) in &ends[i..] {
        if h != key {
            // Past this name's bucket: no matching end exists.
            return None;
        }
        // Ends are sorted by line within a name bucket; once at/after the
        // bound, further candidates are also out of range.
        if end_idx >= bound {
            return None;
        }
        #[cfg(test)]
        {
            parser
                .frame_end_work
                .set(parser.frame_end_work.get().saturating_add(1));
        }
        let end = Pos::from_index(parser.line_rec(end_idx as usize)?.first as usize);
        // Ends are in line order; once past the limit, further ends are too.
        if end.index() >= limit {
            return None;
        }
        // Name compare needs full end-line tokens (region cap may hide them).
        let end_line = parser.full_line_at(end_idx as usize)?;
        if confirm(end_line) {
            return Some(end);
        }
    }
    None
}

/// FNV-1a over the ASCII-lowercased name: bucket key for end-index tables.
///
/// Only selects a bucket: every hit is still confirmed with a real caseless
/// compare ([`block::underscore_end_name_matches`] /
/// [`latex::end_name_matches`]), so a collision costs one extra probe and
/// cannot change the parse.
fn caseless_name_key(name: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in name.bytes() {
        h ^= u64::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn visible(parser: &Parser<'_>, end: Pos) -> Option<Pos> {
    (end.index() < parser.limit().index()).then_some(end)
}

fn ensure(parser: &Parser<'_>) {
    if parser.frame_ends.borrow().is_some() {
        return;
    }
    let built = FrameEndCache::build(parser);
    *parser.frame_ends.borrow_mut() = Some(built);
}

/// Test-only: cumulative line visits charged to frame-end work (build +
/// underscore and latex-env look-up probes).
#[cfg(test)]
pub(crate) fn frame_end_work(parser: &Parser<'_>) -> usize {
    parser.frame_end_work.get()
}

/// Test-only: reset the work counter (does not drop the cache).
#[cfg(test)]
pub(crate) fn reset_frame_end_work(parser: &Parser<'_>) {
    parser.frame_end_work.set(0);
}

/// Test-only: whether the END cache has been built.
#[cfg(test)]
pub(crate) fn frame_ends_ready(parser: &Parser<'_>) -> bool {
    parser.frame_ends.borrow().is_some()
}

#[cfg(test)]
mod tests {
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::make_parser;

    /// N unclosed `#+begin_a` openers must not rescan M mismatched `#+end_b`
    /// markers.
    ///
    /// The name-keyed bucket makes each look-up a binary search plus its own
    /// bucket; a flat scan of every end line per opener would charge ~N*M
    /// probes and blow the bound below.
    #[test]
    // work/line ratio; the counters stay under the f64 mantissa.
    #[allow(clippy::cast_precision_loss)]
    fn mismatched_underscore_ends_do_not_scale_with_opener_count() {
        fn doc(n_open: usize, n_end: usize) -> String {
            let mut s = String::from("lead\n");
            for _ in 0..n_open {
                s.push_str("#+begin_a\n");
            }
            for _ in 0..n_end {
                s.push_str("#+end_b\n");
            }
            s
        }

        // Fixed end count, growing opener count: work must stay ~O(lines).
        let mut prev_ratio = 0.0f64;
        for n in [64usize, 256] {
            let src = doc(n, 64);
            let mut p = make_parser(&src);
            super::reset_frame_end_work(&p);
            crate::parser::grammar::drive::document::parse_document(&mut p);
            let work = super::frame_end_work(&p);
            let lines = p.line_table_len();
            assert!(super::frame_ends_ready(&p), "n={n}: cache must be built");
            assert!(
                work <= lines.saturating_mul(4),
                "n={n}: frame_end_work={work} lines={lines}; a per-opener scan of \
                 all {} end markers would be ~{}",
                64,
                n * 64
            );
            let ratio = work as f64 / lines.max(1) as f64;
            if prev_ratio > 0.0 {
                assert!(
                    ratio <= prev_ratio * 1.5 + 0.5,
                    "work/line grew from {prev_ratio} to {ratio} (n={n})"
                );
            }
            prev_ratio = ratio;
            let (_t, _e, diags) = p.finish();
            assert!(diags.is_empty(), "n={n}: unexpected diags {diags:?}");
        }

        // Correctness under the bucketing: the matching end still closes the
        // block even with mismatched ends in between, caselessly.
        let root = crate::tests::parse_document_tree("#+begin_A\nx\n#+end_b\n#+end_a\n");
        assert_eq!(
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::SPECIAL_BLOCK).len(),
            1,
            "matching end after a mismatched one still closes the block"
        );
    }

    #[test]
    // work/line ratio; the counters stay under the f64 mantissa.
    #[allow(clippy::cast_precision_loss)]
    fn unclosed_frame_openers_frame_end_work_is_linear() {
        use crate::tests::parse_document_tree;

        fn drawer_run(n: usize) -> String {
            use std::fmt::Write;

            let mut s = String::from("lead\n");
            for i in 0..n {
                let _ = writeln!(s, ":D{i}:");
            }
            s.push_str("tail\n");
            s
        }
        fn block_run(n: usize) -> String {
            // Same name every line: without amortisation each resolve re-walked
            // the full tail looking for `#+end_src`.
            let mut s = String::from("lead\n");
            for _ in 0..n {
                s.push_str("#+begin_src\n");
            }
            s.push_str("tail\n");
            s
        }

        for (label, build) in [
            ("drawer", drawer_run as fn(usize) -> String),
            ("block", block_run),
        ] {
            let mut prev_work_per_line = 0.0f64;
            for n in [64usize, 256] {
                let src = build(n);
                let mut p = make_parser(&src);
                super::reset_frame_end_work(&p);
                crate::parser::grammar::drive::document::parse_document(&mut p);
                let work = super::frame_end_work(&p);
                let lines = p.line_table_len();
                assert!(
                    super::frame_ends_ready(&p),
                    "{label} n={n}: frame_ends must be built (resolve path engaged)"
                );
                // The build charges one visit per line, so this proves the
                // cache was used (the upper bound alone would pass at zero).
                assert!(
                    work >= lines,
                    "{label} n={n}: frame_end_work={work} < lines={lines} (build must run)"
                );
                let (_toks, _evs, diags) = p.finish();
                // The openers continue the lead paragraph, so nothing degrades.
                assert!(
                    diags.is_empty(),
                    "{label} n={n}: unexpected diags {diags:?}"
                );

                let root = parse_document_tree(&src);
                assert_eq!(crate::tests::tree_text(&root), src, "{label} n={n}");
                assert!(
                    crate::tests::nodes_of_kind(&root, OrgSyntaxKind::DRAWER).is_empty(),
                    "{label} n={n}: no DRAWER nodes"
                );
                assert!(
                    crate::tests::nodes_of_kind(&root, OrgSyntaxKind::SRC_BLOCK).is_empty(),
                    "{label} n={n}: no SRC_BLOCK nodes"
                );

                // One build pass is O(lines). Quadratic rescans would be ~n^2/2.
                assert!(
                    work <= lines.saturating_mul(4),
                    "{label} n={n}: frame_end_work={work} lines={lines} (expected O(lines))"
                );
                let ratio = work as f64 / lines.max(1) as f64;
                if prev_work_per_line > 0.0 {
                    assert!(
                        ratio <= prev_work_per_line * 1.5 + 0.5,
                        "{label}: work/line grew from {prev_work_per_line} to {ratio} (n={n})"
                    );
                }
                prev_work_per_line = ratio;
            }
        }
    }

    #[test]
    fn nested_limit_first_resolve_does_not_poison_outer_closed_frames() {
        use crate::DiagnosticKind;
        use crate::tests::{diags_any, nodes_of_kind, parse_document_tree};

        // Footnote body (nested limit) holds a closed frame; then a heading and
        // another closed frame at document/section scope.
        let cases: &[(&str, OrgSyntaxKind, usize)] = &[
            (
                "[fn:1] note\n:INNER:\nx\n:END:\n\n* H\n:OUTER:\ny\n:END:\n",
                OrgSyntaxKind::DRAWER,
                2,
            ),
            (
                "[fn:1] note\n#+begin_src\ncode\n#+end_src\n\n* H\n#+begin_src\ny\n#+end_src\n",
                OrgSyntaxKind::SRC_BLOCK,
                2,
            ),
            (
                "[fn:1] note\n#+begin: d\nbody\n#+end:\n\n* H\n#+begin: d\ny\n#+end:\n",
                OrgSyntaxKind::DYNAMIC_BLOCK,
                2,
            ),
            (
                "[fn:1] note\n\\begin{eq}\nx\n\\end{eq}\n\n* H\n\\begin{eq}\ny\n\\end{eq}\n",
                OrgSyntaxKind::LATEX_ENVIRONMENT,
                2,
            ),
        ];

        for &(src, kind, want_count) in cases {
            let root = parse_document_tree(src);
            let frames = nodes_of_kind(&root, kind);
            assert_eq!(
                frames.len(),
                want_count,
                "expected {want_count} {kind:?} nodes (inner+outer) for {src:?}, got {}",
                frames.len()
            );

            let mut p = make_parser(src);
            crate::parser::grammar::drive::document::parse_document(&mut p);
            let (_toks, _evs, diags) = p.finish();
            assert!(
                !diags_any(&diags, |k| matches!(
                    k,
                    DiagnosticKind::UnterminatedDrawer { .. }
                        | DiagnosticKind::UnterminatedBlock { .. }
                        | DiagnosticKind::UnterminatedLatexEnvironment { .. }
                )),
                "outer closed frame must not degrade after nested-limit build; diags={diags:?} src={src:?}"
            );
        }
    }

    /// N unclosed `\begin{a}` openers must not rescan M `\end{b}` markers.
    #[test]
    // work/line ratio; the counters stay under the f64 mantissa.
    #[allow(clippy::cast_precision_loss)]
    fn mismatched_latex_env_ends_do_not_scale_with_opener_count() {
        fn doc(n_open: usize, n_end: usize) -> String {
            let mut s = String::from("lead\n");
            for _ in 0..n_open {
                s.push_str("\\begin{a}\n");
            }
            for _ in 0..n_end {
                s.push_str("\\end{b}\n");
            }
            s
        }

        let mut prev_ratio = 0.0f64;
        for n in [64usize, 256] {
            let src = doc(n, 64);
            let mut p = make_parser(&src);
            super::reset_frame_end_work(&p);
            crate::parser::grammar::drive::document::parse_document(&mut p);
            let work = super::frame_end_work(&p);
            let lines = p.line_table_len();
            assert!(super::frame_ends_ready(&p), "n={n}: cache must be built");
            assert!(
                work <= lines.saturating_mul(4),
                "n={n}: frame_end_work={work} lines={lines}; a per-opener scan of \
                 all {} end markers would be ~{}",
                64,
                n * 64
            );
            let ratio = work as f64 / lines.max(1) as f64;
            if prev_ratio > 0.0 {
                assert!(
                    ratio <= prev_ratio * 1.5 + 0.5,
                    "work/line grew from {prev_ratio} to {ratio} (n={n})"
                );
            }
            prev_ratio = ratio;
            let (_t, _e, diags) = p.finish();
            // The unclosed openers continue the lead paragraph (they do not
            // interrupt it), so no degradation diagnostic is recorded.
            assert!(
                diags.is_empty(),
                "n={n}: unclosed begins after prose must not emit diags, got {diags:?}"
            );
        }

        // Correctness: matching end still closes after a mismatched name, caselessly.
        let root = crate::tests::parse_document_tree("\\begin{A}\nx\n\\end{b}\n\\end{a}\n");
        assert_eq!(
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT).len(),
            1,
            "matching end after a mismatched one still closes the env"
        );
    }

    /// Underscore blocks may end past the next heading; latex environments may
    /// not.
    #[test]
    fn underscore_block_end_after_no_heading_bound() {
        // Heading between begin and end: underscore still finds the end line.
        let p = make_parser("#+begin_src\nx\n* H\n#+end_src\n");
        let line = p.line().expect("begin");
        assert!(
            super::underscore_block_end_after(&p, line, "src").is_some(),
            "underscore keyed_end_after has no heading bound (u32::MAX)"
        );

        // Latex half-open section bound rejects the same shape.
        let p = make_parser("\\begin{eq}\nx\n* H\n\\end{eq}\n");
        let line = p.line().expect("begin");
        assert!(
            super::latex_env_end_after(&p, line, "eq").is_none(),
            "latex heading-between remains rejected"
        );
    }

    #[test]
    fn latex_env_end_after_half_open_bound_and_limit() {
        use crate::parser::grammar::forms::latex;

        // Happy: end strictly after opener and before next heading.
        let p = make_parser("\\begin{eq}\nbody\n\\end{eq}\n* H\n");
        let line = p.line().expect("begin");
        let end = super::latex_env_end_after(&p, line, "eq").expect("closed");
        let end_line = p.line_of(end).expect("end line");
        assert_eq!(end_line.kind(0), OrgSyntaxKind::BACKSLASH);
        assert!(end_line.text_eq_caseless(1, "end"));

        // Heading strictly between begin and end: reject.
        let p = make_parser("\\begin{eq}\nx\n* H\n\\end{eq}\n");
        let line = p.line().expect("begin");
        assert!(
            super::latex_env_end_after(&p, line, "eq").is_none(),
            "heading between must reject"
        );

        // End form on a heading line: reject (half-open excludes end == heading).
        let p = make_parser("\\begin{eq}\nx\n* Results \\end{eq}\n");
        let line = p.line().expect("begin");
        assert!(
            super::latex_env_end_after(&p, line, "eq").is_none(),
            "end-on-heading must reject"
        );

        // Active limit hides the end line: reject.
        let mut p = make_parser("\\begin{eq}\nbody\n\\end{eq}\n");
        let end_line = p
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| latex::end_name(*l).is_some())
            .expect("end line");
        let cap = end_line.start();
        p.with_limit_at(cap, |pp| {
            let line = pp.line().expect("begin under cap");
            assert!(
                super::latex_env_end_after(pp, line, "eq").is_none(),
                "limit hiding end must reject"
            );
        });
        // Outer limit restored: same opener finds the end again.
        let line = p.line().expect("begin restored");
        assert!(
            super::latex_env_end_after(&p, line, "eq").is_some(),
            "outer limit must see closed end"
        );
    }

    #[test]
    fn caseless_name_key_buckets_ascii_fold() {
        assert_eq!(
            super::caseless_name_key("Eq"),
            super::caseless_name_key("eq")
        );
        assert_eq!(
            super::caseless_name_key("align*"),
            super::caseless_name_key("ALIGN*")
        );
        assert_ne!(super::caseless_name_key("a"), super::caseless_name_key("b"));
    }
}
