//! Shared test support (compiled only under `cfg(test)`): the `TAXONOMY`
//! corpus and helpers for grammar tests.
//!
//! # Test styles
//!
//! Grammar unit tests fall into four styles. Pick the thinnest one that
//! still expresses the contract under test:
//!
//! 1. **Direct** - `run(src, |p| parse_X(p))` when every input under test is
//!    expected to be recognised. Prefer it for pure structure checks
//!    (heading, block, keyword, ...). Object forms are a `probe` plus a
//!    `parse(shape)`; a test may wrap the two as a single `parse_*` call.
//!
//! 2. **Shape-carrying** - [`run_element`] (line probe) and [`run_object`]
//!    (cursor probe). One probe, then emit with the measurement it returned,
//!    the same contract the dispatchers use, so a test cannot exercise a path
//!    that measures twice where production measures once.
//!    [`run_unit_element`] is the variant for forms whose probe returns no
//!    measurement.
//!
//! 3. **Inline region** - [`run_inline`] opens an inline region at the cursor
//!    the way `parse_inline` does, so object PRE sees `at_region_start`.
//!
//! 4. **Manual** - `make_parser` + direct mutation for cursor/limit/progress
//!    assertions. Keep rare.

#![cfg(test)]

use crate::DiagnosticKind;
use crate::lexer::LexedToken;
use crate::parser::{
    Diagnostic, Event, LineView, ParseConfig, Parser, Pos, TodoInterpretation, TodoKind,
};
use crate::syntax::OrgSyntaxKind;

/// The triple returned by `Parser::finish()`.
pub(crate) type ParseResult = (Vec<LexedToken>, Vec<Event>, Vec<Diagnostic>);

// ---------------------------------------------------------------------------
// Taxonomy corpus
// ---------------------------------------------------------------------------

/// ParseConfig variant for a [`TAXONOMY`] fixture.
///
/// [`ParseConfig`] is builder-constructed, not `const`, so fixtures name a variant
/// and build it at test time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cfg {
    /// `ParseConfig::default()` - the overwhelming majority of fixtures.
    Default,
    /// Lowered inline-task threshold. Exercises the config plumbing itself;
    /// the default-config inline task uses 15 stars instead.
    InlineTaskLevel3,
    /// Custom in-buffer TODO vocabulary.
    CustomTodo,
}

impl Cfg {
    pub(crate) fn build(self) -> ParseConfig {
        match self {
            Self::Default => ParseConfig::default(),
            Self::InlineTaskLevel3 => ParseConfig::default().with_inline_task_min_level(3),
            Self::CustomTodo => config_with_todo_keywords([
                ("NEXT", TodoKind::Active),
                ("CANCELLED", TodoKind::Done),
            ]),
        }
    }
}

/// Taxonomy corpus: one fixture per construct family, shared by many tests.
///
/// Among others it drives
/// `parser::builder::tests::every_construct_round_trips_through_the_public_parse`
/// (losslessness), [`every_node_kind_is_emitted_or_documented_as_never_emitted`]
/// (every node kind is reachable), `write_is_source_over_taxonomy` (byte-exact
/// write), the damage-reuse oracles, and the parity snapshot
/// `parity/corpus/taxonomy.json`. Keeping one list means a fixture added for
/// one check is covered by all of them.
///
/// An entry that exists only to reach a kind names that kind in a comment, so
/// nobody removes it as redundant.
pub(crate) const TAXONOMY: &[(&str, Cfg)] = &[
    // --- Degenerate inputs -------------------------------------------------
    ("", Cfg::Default),
    ("\n", Cfg::Default),
    ("\r\n", Cfg::Default),
    ("no trailing newline", Cfg::Default),
    ("crlf\r\nlines\r\n\r\nwith blanks\r\n", Cfg::Default),
    (
        "cafe\u{301} \u{2615} de\u{301}ja\u{300}-vu \u{2014} emoji \u{1F389} mixed\n",
        Cfg::Default,
    ),
    // BOM is TEXT (lexer non-ASCII dispatch). Same-line BOM + stars is not a
    // heading (`star_head` wants a line-start STAR). A BOM-only first line
    // leaves the next heading intact. Write preserves the bytes; synthesis
    // never emits a BOM.
    ("\u{feff}* H\n", Cfg::Default),
    ("\u{feff}\n* H\n", Cfg::Default),
    // --- Outline: TODO_KEYWORD, PRIORITY, TAGS, HEADING_TITLE, OUTLINE -----
    (
        "* TODO [#A] Title :work:home:\nbody\n\n** Child\n",
        Cfg::Default,
    ),
    // HEADING_COMMENT.
    ("* COMMENT not exported\nbody\n", Cfg::Default),
    // PLANNING, PROPERTY_DRAWER, NODE_PROPERTY(+KEY/+VALUE), CLOCK.
    (
        "Intro\n\n* H\nSCHEDULED: <2020-01-01 Wed>\n:PROPERTIES:\n:A: 1\n:END:\nCLOCK: [2020-01-01 Wed 10:00]\nbody\n",
        Cfg::Default,
    ),
    // INLINE_TASK at the default threshold (15 stars).
    (
        "*************** TASK inline\nbody\n*************** END\n",
        Cfg::Default,
    ),
    // INLINE_TASK via lowered config: proves fixtures can carry a ParseConfig.
    ("*** TASK lowered\nbody\n*** END\n", Cfg::InlineTaskLevel3),
    // Custom TODO vocabulary through ParseConfig.
    ("* NEXT do it\n* CANCELLED dropped\n", Cfg::CustomTodo),
    // --- Lists: BULLET, COUNTER, CHECKBOX, ITEM_TAG ------------------------
    ("- a\n- b\n\n1. x\n   deep\n2. y\n", Cfg::Default),
    (
        "1. [@3] counter\n- [ ] unchecked\n- [X] checked\n- term :: definition\n",
        Cfg::Default,
    ),
    // --- Tables: TABLE_ROW, TABLE_RULE_ROW, TABLE_CELL ---------------------
    (
        "| a | b |\n|---+---|\n| c | d |\n#+TBLFM: $1=1\n",
        Cfg::Default,
    ),
    // --- Blocks: every *_BLOCK kind ----------------------------------------
    (
        "#+TITLE: Doc\n#+begin_src rust\nlet x = 1;\n#+end_src\n",
        Cfg::Default,
    ),
    ("#+begin_quote\n- q\n#+end_quote\n", Cfg::Default),
    ("#+begin_center\ncentered\n#+end_center\n", Cfg::Default),
    ("#+begin_example\nverbatim\n#+end_example\n", Cfg::Default),
    ("#+begin_export html\n<p/>\n#+end_export\n", Cfg::Default),
    ("#+begin_comment\nhidden\n#+end_comment\n", Cfg::Default),
    ("#+begin_myspecial\nbody\n#+end_myspecial\n", Cfg::Default),
    (
        "#+begin: clocktable :scope file\ncontent\n#+end:\n",
        Cfg::Default,
    ),
    // VERSE_BLOCK with an object body (BodyMode::Objects).
    (
        "#+begin_verse\n*bold* line \\\\\nsecond <<vt>>\n#+end_verse\n",
        Cfg::Default,
    ),
    // Comma-quoted lines stay raw text in the CST: no unquoting, no escape nodes.
    (
        "#+begin_example\n,*not a heading\n,,*x\n#+end_example\n",
        Cfg::Default,
    ),
    // --- Drawers, footnotes, comments, fixed-width, rules ------------------
    (":DRAWER:\ntext\n:END:\nafter\n", Cfg::Default),
    (
        "[fn:1] a footnote\ntext[fn:1] with a reference\n",
        Cfg::Default,
    ),
    (": fixed width\n# a comment\n-----\n", Cfg::Default),
    // --- Keywords: SETTING_KEYWORD, KEYWORD(+KEY/DUAL/VALUE), BABEL_CALL ----
    (
        "#+TODO: TODO NEXT | DONE\n#+PRIORITIES: A C B\n",
        Cfg::Default,
    ),
    // In-buffer vocabulary *plus* headlines that only carve under it. The
    // directive-only fixture above cannot show the settings taking effect:
    // with no headline, every config produces the same tree. Keep the two
    // paired; this is the fixture that exercises config damage in the reuse
    // and apply oracles.
    (
        "#+TODO: BUG | FIXED\n* BUG one\n** FIXED two\n* TODO not a keyword\n",
        Cfg::Default,
    ),
    // AFFILIATED_KEYWORDS: the CAPTION line attaches to the table below it.
    // Keep the two lines adjacent - a blank between them orphans the keyword
    // and the run node is no longer reached.
    ("#+CAPTION[short]: long caption\n| a |\n", Cfg::Default),
    ("#+call: fname[:in 1](x=2)[:out 3]\n", Cfg::Default),
    // --- Objects: markup ---------------------------------------------------
    (
        "/italic/ *bold* =verb= ~code~ +strike+ _under_\n",
        Cfg::Default,
    ),
    // SUBSCRIPT / SUPERSCRIPT.
    ("x_a and y^{b} and z_{c}\n", Cfg::Default),
    // --- Objects: links, targets, cookies ----------------------------------
    (
        "[[https://example.com][desc]] plain https://x.com <https://y.com>\n",
        Cfg::Default,
    ),
    (
        "<<target>> and <<<radio>>> and [1/2] and [/]\n",
        Cfg::Default,
    ),
    // --- Objects: timestamps (DATE, TIME, TIME_RANGE, ROD) -----------------
    (
        "<2020-01-01 Wed 10:00-11:00 +1w -2d> and [2026-06-24] and <2020-01-01>--<2020-01-05>\n",
        Cfg::Default,
    ),
    // --- Objects: latex, entity, export snippet, macro, line break ---------
    (
        "\\alpha and $a$ and \\(b\\) and @@html:<i/>@@ and {{{title}}} and\\\\\nnext\n",
        Cfg::Default,
    ),
    (
        "\\begin{equation}\nE = mc^2\n\\end{equation}\n",
        Cfg::Default,
    ),
    // --- Objects: babel + inline src (CALL_*, SRC_*) -----------------------
    (
        "call_fn[:in 1](x)[:out 2] and src_python[:x 1]{print(1)} inline\n",
        Cfg::Default,
    ),
    // --- Objects: citation --------------------------------------------------
    ("[cite/t: pre; @key post; @key2]\n", Cfg::Default),
    // --- Diary sexp: element (column-0) and stamp body ---------------------
    ("%%(diary-float t 4 2)\n", Cfg::Default),
    ("<%%(diary-float t 4 2)>\n", Cfg::Default),
    // --- Degrading openers: the fallback path must stay lossless -----------
    ("#+begin_src\nunclosed\n", Cfg::Default),
    (":D:\nno end\n", Cfg::Default),
    ("#+begin_\nempty name\n", Cfg::Default),
    ("\\begin{eq}\nunclosed env\n", Cfg::Default),
];

/// Shared `ParseConfig::default()` with `'static` lifetime.
///
/// `Parser` borrows its config, so a test cannot hand it a temporary.
/// Tests that need the default vocabulary use this; tests that need a custom
/// one bind it locally and pass `&cfg`.
pub(crate) fn default_config() -> &'static ParseConfig {
    static DEFAULT: std::sync::OnceLock<ParseConfig> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(ParseConfig::default)
}

/// Construct a fresh parser over `src` with default params.
pub(crate) fn make_parser(src: &str) -> Parser<'_> {
    Parser::with_config(src, default_config())
}

/// Test-only helper to construct a ParseConfig with a custom set of TODO keywords.
/// Thin wrapper over the public [`ParseConfig::with_todo_keywords`] builder.
pub(crate) fn config_with_todo_keywords<I, S>(kws: I) -> ParseConfig
where
    I: IntoIterator<Item = (S, TodoKind)>,
    S: Into<String>,
{
    ParseConfig::default().with_todo_keywords(kws)
}

/// True if any diagnostic matches the given kind predicate.
pub(crate) fn diags_any(diags: &[Diagnostic], pred: impl Fn(&DiagnosticKind) -> bool) -> bool {
    diags.iter().any(|d| pred(d.kind()))
}

/// Run `f` against a parser for `src` and return the `(tokens, events, diagnostics)`
/// triple produced by `Parser::finish`.
pub(crate) fn run(src: &str, f: impl FnOnce(&mut Parser)) -> ParseResult {
    let mut p = make_parser(src);
    f(&mut p);
    p.finish()
}

/// Like [`run`], but construct the parser with a custom [`ParseConfig`].
pub(crate) fn run_with_config(
    src: &str,
    cfg: &ParseConfig,
    f: impl FnOnce(&mut Parser),
) -> ParseResult {
    // `cfg` outlives `p`; `finish` returns owned data, so the borrow ends here.
    let mut p = Parser::with_config(src, cfg);
    f(&mut p);
    p.finish()
}

/// Object harness: probe once at the cursor, then emit with the carried shape.
///
/// Mirrors the inline dispatcher: a rejected probe consumes one token (the
/// dispatcher's unclaimed-token fallthrough) so rejection fixtures still make
/// progress. The object twin of [`run_element`]; between them there is no test
/// path that probes twice.
pub(crate) fn run_object<S>(
    src: &str,
    probe: fn(&Parser) -> Option<S>,
    parse: fn(&mut Parser, S),
) -> ParseResult {
    run(src, |p| match probe(p) {
        Some(shape) => parse(p, shape),
        None if p.current() != OrgSyntaxKind::EOF => p.eat_any(),
        None => {}
    })
}

/// Probe the first line of `src` with a line-based element probe.
///
/// Returns the probe's measurement so tests can assert on shape fields
/// (heading level, keyword class) as well as recognition.
pub(crate) fn probe_line<T>(src: &str, probe: fn(LineView<'_, '_>) -> Option<T>) -> Option<T> {
    let p = make_parser(src);
    p.line().and_then(probe)
}

/// Element harness: probe the first line, then parse with the measurement.
///
/// Does nothing when the probe rejects (reject cases leave the cursor
/// unmoved, mirroring the dispatcher).
pub(crate) fn run_element<T>(
    src: &str,
    probe: fn(LineView<'_, '_>) -> Option<T>,
    parse: fn(&mut Parser, T),
) -> ParseResult {
    run(src, |p| {
        let shape = p.line().and_then(probe);
        if let Some(shape) = shape {
            parse(p, shape);
        }
    })
}

/// Unit-element harness: probe the first line, then parse with no measurement.
///
/// The [`run_element`] twin for unit shape-class forms (comment, fixed-width,
/// clock, planning) whose `probe` returns `Option<()>` and whose `parse` takes
/// only the parser. Guarding on the probe is what makes the harness honour the
/// dispatcher contract those `parse` bodies `debug_assert!` on; calling `parse`
/// unguarded would let a test drive a path production never reaches.
pub(crate) fn run_unit_element(
    src: &str,
    probe: fn(LineView<'_, '_>) -> Option<()>,
    parse: fn(&mut Parser),
) -> ParseResult {
    run(src, |p| {
        if p.line().and_then(probe).is_some() {
            parse(p);
        }
    })
}

/// Inline harness: open an inline region at the cursor, then run `f`.
///
/// Mirrors `parse_inline`: object PRE accepts an opener at the first token of
/// the active region, so tests exercise `at_region_start` without standing up
/// the full dispatcher.
pub(crate) fn run_inline(src: &str, f: impl FnOnce(&mut Parser)) -> ParseResult {
    run(src, |p| {
        let start = Pos::from_index(p.cursor().index());
        p.with_inline_region_start(Some(start), f);
    })
}

/// Position `rel` tokens ahead of `p`'s cursor (unsigned shorthand).
pub(crate) fn pos_ahead(p: &Parser<'_>, rel: usize) -> Pos {
    Pos::from_index(p.cursor().index() + rel)
}

/// Return the ordered list of node kinds for all `Start` events.
pub(crate) fn node_kinds(events: &[Event]) -> Vec<OrgSyntaxKind> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Start { kind } => Some(*kind),
            _ => None,
        })
        .collect()
}

/// Count `Token { kind, range }` events matching `kind`.
pub(crate) fn token_count(events: &[Event], kind: OrgSyntaxKind) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Token { kind: k, .. } if *k == kind))
        .count()
}

/// Count `Start { kind }` nodes matching `kind`.
pub(crate) fn node_count(events: &[Event], kind: OrgSyntaxKind) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Start { kind: k } if *k == kind))
        .count()
}

/// Reconstruct the source text belonging to the first `Start { kind }` node
/// encountered by walking the event stream and concatenating owned tokens.
/// Returns `None` if no such node is found. This is the builder-free way to
/// assert on the textual content of a produced node.
///
/// Mirrors builder accounting: each [`Event::Token`] contributes its absolute
/// `range` when inside the target node.
pub(crate) fn node_text(events: &[Event], src: &str, kind: OrgSyntaxKind) -> Option<String> {
    let mut depth = 0usize;
    let mut target_depth: Option<usize> = None;
    let mut buf = String::new();
    for ev in events {
        match ev {
            Event::Start { kind: k } => {
                depth += 1;
                if *k == kind && target_depth.is_none() {
                    target_depth = Some(depth);
                    buf.clear();
                }
            }
            Event::Token { range, .. } => {
                if target_depth.is_some() {
                    let s = usize::from(range.start());
                    let e = usize::from(range.end());
                    buf.push_str(&src[s..e]);
                }
            }
            Event::Finish => {
                if Some(depth) == target_depth {
                    return Some(std::mem::take(&mut buf));
                }
                depth -= 1;
            }
            Event::Tombstone => {}
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Full document + built tree helpers (for structure / fidelity / integration tests)
// -----------------------------------------------------------------------------

use rowan::SyntaxNode;

use crate::parser::builder;
use crate::parser::grammar::parse_document;
use crate::syntax::OrgLanguage;

/// Parse a complete document and return the root of the materialized rowan CST.
/// Preferred for tree fidelity, attribution, and structure tests.
pub(crate) fn parse_document_tree(src: &str) -> SyntaxNode<OrgLanguage> {
    // Diagnostics are discarded here. For diagnostics and tree together use
    // [`crate::parse`] (`Document`).
    crate::parse(src).root().clone()
}

/// Like [`parse_document_tree`], but construct the parser with a custom [`ParseConfig`].
pub(crate) fn parse_document_tree_with_config(
    src: &str,
    cfg: &ParseConfig,
) -> SyntaxNode<OrgLanguage> {
    let (tokens, events, _diags) = run_with_config(src, cfg, parse_document);
    let green = builder::build(src, &tokens, events);
    SyntaxNode::new_root(green)
}

// No `assert_tree_roundtrip` helper: `builder::build` enforces gap-free tiling
// of the whole source on every parse (always on, debug and release), so
// `tree_text(root) == src` cannot fail without the builder panicking first.
// The guarantee is stated once, over a multi-construct corpus, in
// `parser::builder::tests::every_construct_round_trips_through_the_public_parse`.
// Construct tests assert the *shape* the builder cannot see instead.

/// Kinds of the direct children of `node`.
pub(crate) fn children_kinds(node: &SyntaxNode<OrgLanguage>) -> Vec<OrgSyntaxKind> {
    node.children().map(|c| c.kind()).collect()
}

/// First direct child of the requested kind, if present.
pub(crate) fn first_child_of_kind(
    node: &SyntaxNode<OrgLanguage>,
    kind: OrgSyntaxKind,
) -> Option<SyntaxNode<OrgLanguage>> {
    node.children().find(|c| c.kind() == kind)
}

/// All descendants of `kind` (including `node` itself if it matches), in
/// pre-order.  This is the tree analogue of the event-based `node_count`.
pub(crate) fn nodes_of_kind(
    node: &SyntaxNode<OrgLanguage>,
    kind: OrgSyntaxKind,
) -> Vec<SyntaxNode<OrgLanguage>> {
    fn walk(n: &SyntaxNode<OrgLanguage>, k: OrgSyntaxKind, out: &mut Vec<SyntaxNode<OrgLanguage>>) {
        if n.kind() == k {
            out.push(n.clone());
        }
        for c in n.children() {
            walk(&c, k, out);
        }
    }
    let mut out = Vec::new();
    walk(node, kind, &mut out);
    out
}

/// Source bytes covered by `range`.
pub(crate) fn source_slice(doc: &crate::Document, range: crate::TextRange) -> &str {
    doc.slice(range)
}

/// Apply an insert point to `src` as a writer would (lead + text + trail).
pub(crate) fn written(src: &str, ip: crate::ast::InsertPoint, text: &str) -> String {
    let off = usize::from(ip.offset());
    format!(
        "{}{}{}",
        &src[..off],
        crate::writer::wrap_insert(ip, text),
        &src[off..]
    )
}

/// Text content of a syntax node (convenience to match the style of the
/// event-based `node_text` helper).
pub(crate) fn tree_text(node: &SyntaxNode<OrgLanguage>) -> String {
    node.text().to_string()
}

/// Compact indented dump of the tree, useful for diagnostics inside tests.
pub(crate) fn format_tree(node: &SyntaxNode<OrgLanguage>) -> String {
    fn rec(n: &SyntaxNode<OrgLanguage>, depth: usize, out: &mut String) {
        use std::fmt::Write;

        let indent = "  ".repeat(depth);
        let k = n.kind();
        let snippet: String = n
            .text()
            .to_string()
            .chars()
            .take(40)
            .collect::<String>()
            .replace('\n', "\\n");
        let _ = writeln!(out, "{indent}{k:?} [{snippet}]");
        for c in n.children() {
            rec(&c, depth + 1, out);
        }
    }
    let mut out = String::new();
    rec(node, 0, &mut out);
    out
}

#[cfg(test)]
mod public_parse_tests {
    use crate::syntax::OrgSyntaxKind;
    use crate::{ParseConfig, parse, parse_with};

    fn descendant_kinds(root: &crate::SyntaxNode) -> Vec<OrgSyntaxKind> {
        root.descendants().map(|n| n.kind()).collect()
    }

    #[test]
    fn parse_with_default_matches_parse() {
        // Multi-construct fixture: heading + paragraph + link + keyword.
        let src = "\
* Heading
Some text with https://example.com and *bold*.
#+TITLE: Doc
";
        let a = parse(src);
        let b = parse_with(src, &ParseConfig::default());
        assert_eq!(a.root().text().to_string(), src);
        assert_eq!(b.root().text().to_string(), src);
        assert_eq!(a.root().kind(), OrgSyntaxKind::DOCUMENT);
        assert_eq!(b.root().kind(), OrgSyntaxKind::DOCUMENT);
        assert_eq!(descendant_kinds(a.root()), descendant_kinds(b.root()));
        assert_eq!(a.diagnostics().len(), b.diagnostics().len());
        for (da, db) in a.diagnostics().iter().zip(b.diagnostics().iter()) {
            assert_eq!(da.kind(), db.kind());
            assert_eq!(da.range(), db.range());
        }
    }

    fn n_heading(root: &crate::SyntaxNode) -> usize {
        root.descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::HEADING)
            .count()
    }

    fn n_inline_task(root: &crate::SyntaxNode) -> usize {
        root.descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
            .count()
    }

    #[test]
    fn parse_with_inline_task_min_level_changes_classification() {
        // Neutral titles: classification depends only on star count + config.
        // Default min level 15: levels 1 and 2 are headlines.
        // with_inline_task_min_level(2): only `*` is a headline; `**` is INLINE_TASK.
        let src = "** two stars\n* one star\n";
        let default = parse(src);
        let with = parse_with(src, &ParseConfig::default().with_inline_task_min_level(2));
        assert_eq!(
            n_heading(default.root()),
            2,
            "default min_level=15: levels 1 and 2 are headlines"
        );
        assert_eq!(n_inline_task(default.root()), 0);
        assert_eq!(
            n_heading(with.root()),
            1,
            "min_level=2: only single-star line is a headline"
        );
        assert_eq!(
            n_inline_task(with.root()),
            1,
            "min_level=2: double-star line is INLINE_TASK"
        );
        // Lossless either way.
        assert_eq!(default.root().text().to_string(), src);
        assert_eq!(with.root().text().to_string(), src);
    }

    #[test]
    fn default_inline_task_min_level_boundary_is_15() {
        // Locks default 15: level 14 is HEADING; levels >= 15 are INLINE_TASK.
        let line = |stars: usize| format!("{} level {}\n", "*".repeat(stars), stars);
        let fourteen = parse(&line(14));
        let fifteen = parse(&line(15));
        let sixteen = parse(&line(16));
        assert_eq!(
            n_heading(fourteen.root()),
            1,
            "level 14 < default 15: HEADING"
        );
        assert_eq!(n_inline_task(fourteen.root()), 0);
        assert_eq!(
            n_heading(fifteen.root()),
            0,
            "level 15 >= default 15: not HEADING"
        );
        assert_eq!(
            n_inline_task(fifteen.root()),
            1,
            "level 15 >= default 15: INLINE_TASK"
        );
        assert_eq!(
            n_heading(sixteen.root()),
            0,
            "level 16 >= default 15: not HEADING"
        );
        assert_eq!(
            n_inline_task(sixteen.root()),
            1,
            "level 16 >= default 15: INLINE_TASK"
        );
        assert_eq!(fourteen.root().text().to_string(), line(14));
        assert_eq!(fifteen.root().text().to_string(), line(15));
        assert_eq!(sixteen.root().text().to_string(), line(16));
    }
}

#[cfg(test)]
mod node_text_tests {
    use super::node_text;
    use crate::parser::Event;
    use crate::syntax::OrgSyntaxKind;
    use rowan::{TextRange, TextSize};

    #[test]
    fn node_text_mid_tile_path_excludes_later_trail() {
        let src = "ab.";
        let events = vec![
            Event::Start {
                kind: OrgSyntaxKind::LINK,
            },
            Event::Start {
                kind: OrgSyntaxKind::LINK_PATH,
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: TextRange::new(TextSize::from(0u32), TextSize::from(2u32)),
            },
            Event::Finish,
            Event::Finish,
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: TextRange::new(TextSize::from(2u32), TextSize::from(3u32)),
            },
        ];
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("ab")
        );
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK).as_deref(),
            Some("ab")
        );
    }
}

// ---------------------------------------------------------------------------
// Every node kind is emitted
// ---------------------------------------------------------------------------

/// Every node kind the taxonomy corpus emits, across all fixtures.
///
/// Uses the public [`crate::parse_with`] path (same as the round-trip consumer).
fn observed_taxonomy_node_kinds() -> std::collections::BTreeSet<OrgSyntaxKind> {
    use std::collections::BTreeSet;
    let mut seen = BTreeSet::new();
    for &(src, cfg) in TAXONOMY {
        let root = crate::parse_with(src, &cfg.build()).root().clone();
        for node in root.descendants() {
            seen.insert(node.kind());
        }
    }
    seen
}

/// Every node kind is emitted by [`TAXONOMY`] or listed in
/// [`crate::syntax::NEVER_EMITTED`].
///
/// A failing kind is exactly one of: (a) grammar gap, (b) corpus gap, or
/// (c) undocumented deliberate exception. Fix the cause; do not weaken this
/// assertion.
#[test]
fn every_node_kind_is_emitted_or_documented_as_never_emitted() {
    use crate::syntax::NEVER_EMITTED;
    use std::collections::BTreeSet;

    let seen = observed_taxonomy_node_kinds();
    let excepted: BTreeSet<_> = NEVER_EMITTED.iter().map(|&(k, _)| k).collect();

    let missing: Vec<OrgSyntaxKind> = (0..OrgSyntaxKind::__LAST as u16)
        .map(|raw| <crate::OrgLanguage as rowan::Language>::kind_from_raw(rowan::SyntaxKind(raw)))
        .filter(|k| k.is_node() && !seen.contains(k) && !excepted.contains(k))
        .collect();

    assert!(
        missing.is_empty(),
        "node kinds neither emitted by TAXONOMY nor listed in NEVER_EMITTED: \
         {missing:?}\n\
         Each one is exactly one of:\n\
         (a) a real grammar gap - nothing emits it; fix the grammar;\n\
         (b) a corpus gap - the grammar emits it but no fixture reaches it; \
         add a fixture to TAXONOMY;\n\
         (c) a deliberate exception - add it to NEVER_EMITTED *with its \
         reason*.\n\
         Fix the cause. Do not weaken this assertion."
    );
}

/// Reverse direction: a kind listed as never-emitted must not appear in the
/// corpus (a stale entry would hide that kind from the check above).
#[test]
fn never_emitted_kinds_are_actually_never_emitted() {
    use crate::syntax::NEVER_EMITTED;

    let seen = observed_taxonomy_node_kinds();
    for &(kind, reason) in NEVER_EMITTED {
        assert!(
            !seen.contains(&kind),
            "{kind:?} is listed in NEVER_EMITTED ({reason:?}) but the \
             taxonomy corpus emits it; remove the stale entry"
        );
        assert!(
            kind.is_node(),
            "{kind:?} in NEVER_EMITTED is a token kind; this check only \
             covers node kinds"
        );
        assert!(
            !reason.trim().is_empty(),
            "{kind:?} in NEVER_EMITTED needs a reason"
        );
    }
}

// ---------------------------------------------------------------------------
// Nesting budget
// ---------------------------------------------------------------------------

/// Contract tests for `ParseConfig::max_nesting_depth`.
///
/// The budget exists so that parse stays infallible on hostile input: without
/// it, deep nesting aborts the process through an OS stack overflow, which is
/// not a `Diagnostic` and cannot be caught. Two hazards, both covered here:
///
/// 1. **Parse recursion** - object and element regions re-enter the grammar.
/// 2. **Green `Drop`** - rowan drops a tree recursively, so *tree depth*
///    matters even for the outline spine, which the parser walks iteratively.
///    That abort lands on the caller's stack after `parse` has returned.
///
/// The stack tests below run on an explicitly small thread stack. Without one
/// they would pass vacuously: a libtest thread has megabytes to spare, so
/// shapes that overflow a 1 MiB wasm stack would survive anyway.
#[cfg(test)]
mod nesting_budget {
    use super::{TAXONOMY, format_tree, nodes_of_kind};
    use crate::syntax::OrgSyntaxKind;
    use crate::{DEFAULT_MAX_NESTING_DEPTH, DiagnosticKind, Document, ParseConfig};

    /// The `wasm-ld` default stack, the smallest stack the crate targets.
    /// Deep fixtures run on a thread this size so the tests fail here rather
    /// than in a wasm runtime.
    const SMALL_STACK: usize = 1024 * 1024;

    /// `n` nested inline footnote refs - 5 source bytes per nesting level, the
    /// cheapest deep-nesting shape in Org. Without the budget it overflows a
    /// release build at about 4400 levels (22 KB of source) and a debug build
    /// at about 680 (3.4 KB).
    fn nested_footnote_refs(n: usize) -> String {
        let mut src = String::with_capacity(6 * n + 2);
        for _ in 0..n {
            src.push_str("[fn::");
        }
        src.push('x');
        for _ in 0..n {
            src.push(']');
        }
        src.push('\n');
        src
    }

    /// `n` nested list items (2-column indent step per level).
    fn nested_list_items(n: usize) -> String {
        let mut src = String::with_capacity(n * n + 4 * n);
        for i in 0..n {
            for _ in 0..(2 * i) {
                src.push(' ');
            }
            src.push_str("- i\n");
        }
        src
    }

    /// `n` nested greater blocks. Distinct names are required: same-name
    /// openers resolve to the *first* matching end, so `#+begin_quote` x n
    /// does not nest at all.
    fn nested_blocks(n: usize) -> String {
        use std::fmt::Write;

        let mut src = String::new();
        for i in 0..n {
            let _ = writeln!(src, "#+begin_q{i}");
        }
        src.push_str("x\n");
        for i in (0..n).rev() {
            let _ = writeln!(src, "#+end_q{i}");
        }
        src
    }

    /// `n`-deep outline. Costs no Rust stack (iterative spine) but is the
    /// cheapest way to build a deep *tree*, which is what green `Drop` walks.
    fn deep_outline(n: usize) -> String {
        let mut src = String::with_capacity(n * (n + 1) / 2 + n * 4);
        for depth in 1..=n {
            for _ in 0..depth {
                src.push('*');
            }
            src.push_str(" H\n");
        }
        src
    }

    fn outline_cfg(n: usize) -> ParseConfig {
        ParseConfig::default().with_inline_task_min_level(u16::try_from(n + 1).unwrap_or(u16::MAX))
    }

    /// Maximum node depth, computed iteratively so the measurement itself
    /// cannot overflow the stack it is measuring.
    fn max_node_depth(out: &Document) -> usize {
        let mut max = 0usize;
        let mut cur = 0usize;
        for event in out.root().preorder() {
            match event {
                rowan::WalkEvent::Enter(_) => {
                    cur += 1;
                    max = max.max(cur);
                }
                rowan::WalkEvent::Leave(_) => cur -= 1,
            }
        }
        max
    }

    /// Parse on a deliberately small stack, dropping the tree inside that
    /// thread so green `Drop` is covered too, and return what the assertions
    /// need (the tree itself must not escape - dropping it on the parent stack
    /// would move the hazard rather than test it).
    fn parse_on_small_stack(src: String, cfg: ParseConfig) -> (usize, usize, String) {
        std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(move || {
                let out = crate::parse_with(&src, &cfg);
                let text = out.root().text().to_string();
                assert_eq!(text, src, "round-trip on small stack");
                (max_node_depth(&out), out.diagnostics().len(), text)
            })
            .expect("spawn parse thread")
            .join()
            .expect("deep nesting must not abort or panic")
    }

    /// Shapes that overflow the stack without the budget return a `Document`,
    /// round-trip, and are depth-bounded, on a 1 MiB stack with the tree
    /// dropped inside that thread.
    #[test]
    fn stack_overflow_shapes_return_a_tree() {
        // (label, source, config, expects a diagnostic)
        let cases: Vec<(&str, String, ParseConfig, bool)> = vec![
            (
                "nested inline footnotes",
                nested_footnote_refs(5000),
                ParseConfig::default(),
                true,
            ),
            (
                "nested lists",
                nested_list_items(700),
                ParseConfig::default(),
                true,
            ),
            (
                "nested greater blocks",
                nested_blocks(3000),
                ParseConfig::default(),
                true,
            ),
            (
                // Parses iteratively, but without the budget rowan's recursive
                // green Drop overflows at about 2200 levels on a 1 MiB stack,
                // after `parse` has already returned.
                "deep outline",
                deep_outline(2500),
                outline_cfg(2500),
                false,
            ),
        ];

        let bound = usize::from(DEFAULT_MAX_NESTING_DEPTH) * 3 + 16;
        for (label, src, cfg, expect_diag) in cases {
            let (depth, diags, _) = parse_on_small_stack(src, cfg);
            assert!(
                depth <= bound,
                "{label}: depth {depth} over documented bound {bound}"
            );
            assert_eq!(
                diags > 0,
                expect_diag,
                "{label}: diagnostic expectation (got {diags})"
            );
        }
    }

    /// Depth is bounded by the budget, not by the input: the same fixture at
    /// two very different sizes must reach the same ceiling.
    #[test]
    fn green_depth_is_bounded_independently_of_input_size() {
        for (label, small, large, cfg) in [
            (
                "footnotes",
                nested_footnote_refs(300),
                nested_footnote_refs(900),
                ParseConfig::default(),
            ),
            (
                "lists",
                nested_list_items(300),
                nested_list_items(600),
                ParseConfig::default(),
            ),
            (
                "outline",
                deep_outline(300),
                deep_outline(600),
                outline_cfg(600),
            ),
        ] {
            let a = max_node_depth(&crate::parse_with(&small, &cfg));
            let b = max_node_depth(&crate::parse_with(&large, &cfg));
            assert_eq!(a, b, "{label}: depth must not grow with input size");
        }
    }

    /// Combined worst case: deep outline, deep lists and deep inline objects
    /// stacked in one document. Depth stays inside
    /// `outline_budget + 2 * region_budget + c` (2 green levels per list
    /// region: PLAIN_LIST + ITEM).
    #[test]
    fn combined_worst_case_stays_inside_the_documented_bound() {
        const N: usize = 300;
        let mut src = deep_outline(N);
        src.push_str(&nested_list_items(N));
        for _ in 0..(2 * N) {
            src.push(' ');
        }
        src.push_str(&nested_footnote_refs(N));

        let budget = usize::from(DEFAULT_MAX_NESTING_DEPTH);
        let (depth, _, _) = parse_on_small_stack(src, outline_cfg(N));
        assert!(
            depth <= budget * 3 + 16,
            "combined depth {depth} over bound {}",
            budget * 3 + 16
        );
        assert!(
            depth > budget,
            "fixture must actually exercise the budget (depth {depth})"
        );
    }

    /// Losslessness holds at *any* budget, including budgets so small that
    /// nothing may nest. Runs the whole taxonomy corpus, so this covers every
    /// construct the crate emits rather than a hand-picked few.
    #[test]
    fn taxonomy_round_trips_at_every_budget() {
        for budget in [0u16, 1, 2, 3, 8, DEFAULT_MAX_NESTING_DEPTH] {
            for &(src, cfg) in TAXONOMY {
                let config = cfg.build().with_max_nesting_depth(budget);
                let out = crate::parse_with(src, &config);
                assert_eq!(
                    out.root().text().to_string(),
                    src,
                    "round-trip broken at budget {budget} for {src:?}"
                );
            }
        }
    }

    /// Under the default budget the tree is byte-identical to an unbounded
    /// parse: no real document pays for the ceiling.
    #[test]
    fn default_budget_does_not_change_ordinary_documents() {
        for &(src, cfg) in TAXONOMY {
            let base = crate::parse_with(src, &cfg.build());
            let raised = crate::parse_with(src, &cfg.build().with_max_nesting_depth(u16::MAX));
            assert_eq!(
                format_tree(base.root()),
                format_tree(raised.root()),
                "default budget altered the tree for {src:?}"
            );
            assert!(
                !base
                    .diagnostics()
                    .iter()
                    .any(|d| d.kind() == &DiagnosticKind::NestingLimitExceeded),
                "no corpus fixture may hit the default budget: {src:?}"
            );
        }
    }

    /// The budget gates emission, never recognition, so a construct that fits
    /// is unaffected by how deep the *document* goes elsewhere.
    #[test]
    fn budget_does_not_change_recognition() {
        // Same paragraph, once alone and once after a document that exhausts
        // the outline budget. Region budget must be untouched by outline depth.
        let para = "*bold* text\n";
        let alone = crate::parse(para);
        let mut deep = deep_outline(200);
        deep.push_str(para);
        let after = crate::parse_with(&deep, &outline_cfg(200));

        assert_eq!(
            nodes_of_kind(alone.root(), OrgSyntaxKind::BOLD).len(),
            1,
            "control"
        );
        assert_eq!(
            nodes_of_kind(after.root(), OrgSyntaxKind::BOLD).len(),
            1,
            "outline depth must not starve the region budget"
        );
        assert!(
            !after
                .diagnostics()
                .iter()
                .any(|d| d.kind() == &DiagnosticKind::NestingLimitExceeded),
            "outline clamp must not emit region diagnostics: {:?}",
            after.diagnostics()
        );
    }
}

// ---------------------------------------------------------------------------
// Parity taxonomy snapshot
// ---------------------------------------------------------------------------

fn json_string(value: &str) -> String {
    use std::fmt::Write;

    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn taxonomy_snapshot_json() -> String {
    let defaults = ParseConfig::default();
    let mut out = String::from("[\n");
    for (index, (src, cfg)) in TAXONOMY.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        let built = cfg.build();
        out.push_str("  {\n");
        out.push_str("    \"src\": ");
        out.push_str(&json_string(src));
        out.push_str(",\n");
        if built.inline_task_min_level() == defaults.inline_task_min_level() {
            out.push_str("    \"inline_task_min_level\": null,\n");
        } else {
            out.push_str("    \"inline_task_min_level\": ");
            out.push_str(&built.inline_task_min_level().to_string());
            out.push_str(",\n");
        }
        if built.list_alphabetical() {
            out.push_str("    \"list_alphabetical\": true,\n");
        } else {
            out.push_str("    \"list_alphabetical\": null,\n");
        }
        if built.todo_sequences() == defaults.todo_sequences() {
            out.push_str("    \"todo\": null,\n");
        } else {
            out.push_str("    \"todo\": [\n");
            for (seq_index, seq) in built.todo_sequences().iter().enumerate() {
                if seq_index > 0 {
                    out.push_str(",\n");
                }
                let interpretation = match seq.interpretation() {
                    TodoInterpretation::Sequence => "sequence",
                    TodoInterpretation::Type => "type",
                };
                out.push_str("      {\n        \"interpretation\": ");
                out.push_str(&json_string(interpretation));
                out.push_str(",\n        \"active\": [");
                for (name_index, name) in seq.active().iter().enumerate() {
                    if name_index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&json_string(name));
                }
                out.push_str("],\n        \"done\": [");
                for (name_index, name) in seq.done().iter().enumerate() {
                    if name_index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&json_string(name));
                }
                out.push_str("]\n      }");
            }
            out.push_str("\n    ],\n");
        }
        out.push_str("    \"link_types\": null,\n");
        out.push_str("    \"user_entities\": null\n");
        out.push_str("  }");
    }
    out.push_str("\n]\n");
    out
}

fn taxonomy_snapshot_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("parity/corpus/taxonomy.json")
}

/// The parity corpus snapshot is the same strings and configs as [`TAXONOMY`].
/// It does not start Emacs or Docker. The parity corpus is not part of the
/// published package, so the check is skipped when it is absent.
#[test]
fn taxonomy_snapshot_matches_const() {
    let path = taxonomy_snapshot_path();
    let want = taxonomy_snapshot_json();
    if !path.parent().is_some_and(std::path::Path::exists) {
        return;
    }
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("taxonomy snapshot directory");
        }
        std::fs::write(&path, &want).expect("write taxonomy snapshot");
    }
    let got = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        got,
        want,
        "taxonomy snapshot drifted from TAXONOMY ({}); regenerate with UPDATE_SNAPSHOTS=1",
        path.display()
    );
}
