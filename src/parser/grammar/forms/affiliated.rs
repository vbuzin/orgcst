//! Affiliated keywords: name dialect + the `AFFILIATED_KEYWORDS` run node.
//!
//! ## Name dialect
//!
//! One closed set plus the `ATTR_[-_A-Za-z0-9]+` rule, in the same
//! translate-then-test order as `org-element--collect-affiliated-keywords`:
//! look up the spelling, then read dual / multiple / parsed from the
//! **canonical** name (`HEADERS` is multiple because `HEADER` is; `RESULT` is
//! dual because `RESULTS` is). The tables hold no parse state, so the grammar
//! and the typed views read the same definition.
//!
//! ## Run node
//!
//! Org keeps affiliated keywords as properties of the element below them. The
//! CST keeps them as `KEYWORD` element nodes wrapped in one run node, emitted
//! **only when the run actually attaches**:
//!
//! ```text
//! SECTION
//!   AFFILIATED_KEYWORDS          <- emitted only when the run affiliates
//!     KEYWORD   #+NAME: fig1
//!     KEYWORD   #+CAPTION: A *cap*
//!   PARAGRAPH                    <- the target; range unchanged by affiliation
//! ```
//!
//! Degraded (blank line, comment, clock, star line, EOF, or a container limit
//! right after the run) there is no run node at all: each line stays a plain
//! `KEYWORD` element owning its post-blank, byte-identical to a non-affiliated
//! keyword.
//!
//! Grammar contract (asserted by tests):
//!
//! | # | Invariant |
//! | - | --------- |
//! | I1 | `AFFILIATED_KEYWORDS` has one or more children, all `KEYWORD`, each a complete line including its `NEWLINE` |
//! | I2 | It contains no blank line and claims no post-blank |
//! | I3 | Its next sibling always exists and its kind satisfies [`accepts_affiliated`] |
//! | I4 | It is never the last child of its parent |
//! | I5 | A keyword line that does not affiliate is a plain `KEYWORD` element, post-blank ownership unchanged |
//! | I6 | Token tiling unchanged - the node adds structure only |
//!
//! Org facts implemented here (`org-element.el`):
//! - Name dialect -> [`is_affiliated_key`] / [`canonical_key`] / dual /
//!   multiple / parsed / [`attr_backend`].
//! - `org-element--affiliated-re` -> [`is_affiliated_line`].
//! - The orphan peek (blank / comment / clock / `^\*+ `) and the container
//!   limit -> [`try_run`].
//! - `org-element-elements-no-affiliated` -> [`accepts_affiliated`] (kept as
//!   the documented org table and the I3 test oracle; the parse path never
//!   needs it because the peek is line-shaped).

use crate::parser::grammar::forms::{clock, comment, heading, keyword};
use crate::parser::{LineView, Parser};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Name dialect (pure tables; no parse state)
// ---------------------------------------------------------------------------

/// What one lookup in the name table answers.
///
/// `canonical` is `org-element-keyword-translation-alist` (`None` means the key
/// is its own canonical name - the `ATTR_*` case, whose spelling is not in the
/// table). The three flags are membership of the canonical name.
///
/// `parsed` gates CAPTION object regions in `forms::keyword`. The `Keyword`
/// view reads all four fields from here instead of keeping its own copy.
struct Row {
    canonical: Option<&'static str>,
    dual: bool,
    multiple: bool,
    parsed: bool,
}

/// Closed affiliated-keyword set as
/// `(name, canonical, dual, multiple, parsed)`.
///
/// `ATTR_*` is a rule, not a row ([`attr_backend`]): always multiple, never
/// dual or parsed, and its own canonical name.
const NAMES: &[(&str, Option<&str>, bool, bool, bool)] = &[
    ("CAPTION", Some("CAPTION"), true, true, true),
    ("DATA", Some("NAME"), false, false, false),
    ("HEADER", Some("HEADER"), false, true, false),
    ("HEADERS", Some("HEADER"), false, true, false),
    ("LABEL", Some("NAME"), false, false, false),
    ("NAME", Some("NAME"), false, false, false),
    ("PLOT", Some("PLOT"), false, false, false),
    ("RESNAME", Some("NAME"), false, false, false),
    ("RESULT", Some("RESULTS"), true, false, false),
    ("RESULTS", Some("RESULTS"), true, false, false),
    ("SOURCE", Some("NAME"), false, false, false),
    ("SRCNAME", Some("NAME"), false, false, false),
    ("TBLNAME", Some("NAME"), false, false, false),
];

/// Single caseless lookup: the closed set, then the `ATTR_` rule.
fn row(key: &str) -> Option<Row> {
    if let Some(&(_, canonical, dual, multiple, parsed)) = NAMES
        .iter()
        .find(|(name, ..)| name.eq_ignore_ascii_case(key))
    {
        return Some(Row {
            canonical,
            dual,
            multiple,
            parsed,
        });
    }
    attr_backend(key).map(|_| Row {
        canonical: None,
        dual: false,
        multiple: true,
        parsed: false,
    })
}

/// Backend suffix of an `ATTR_*` key, if the name matches org's
/// `ATTR_[-_A-Za-z0-9]+` (ASCII caseless prefix; non-empty backend).
///
/// Uses `str::get` so multi-byte keys never panic on a mid-char slice.
pub(crate) fn attr_backend(key: &str) -> Option<&str> {
    let prefix = key.get(..5)?;
    if !prefix.eq_ignore_ascii_case("ATTR_") {
        return None;
    }
    let rest = key.get(5..)?;
    if rest.is_empty() {
        return None;
    }
    if !rest
        .bytes()
        .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return None;
    }
    Some(rest)
}

/// Whether `key` names an affiliated keyword (`org-element-affiliated-keywords`
/// plus the `ATTR_` rule). ASCII caseless.
pub(crate) fn is_affiliated_key(key: &str) -> bool {
    row(key).is_some()
}

/// Whether `key`'s canonical name is in `org-element-dual-keywords`
/// (CAPTION, RESULTS - so RESULT too).
pub(crate) fn is_dual_key(key: &str) -> bool {
    row(key).is_some_and(|r| r.dual)
}

/// Whether `key`'s canonical name is in `org-element-multiple-keywords`
/// (CAPTION, HEADER - so HEADERS too - and every `ATTR_*`).
pub(crate) fn is_multiple_key(key: &str) -> bool {
    row(key).is_some_and(|r| r.multiple)
}

/// Whether `key`'s canonical name is in `org-element-parsed-keywords`
/// (CAPTION only): its dual and value hold inline objects.
pub(crate) fn is_parsed_key(key: &str) -> bool {
    row(key).is_some_and(|r| r.parsed)
}

/// `org-element-keyword-translation-alist` applied to `key`.
///
/// Non-affiliated names and `ATTR_*` are their own canonical name (returned as
/// the same borrow, source case preserved). Table spellings fold to the
/// uppercase canonical form (`tblname` -> `NAME`).
pub(crate) fn canonical_key(key: &str) -> &str {
    match row(key) {
        Some(Row {
            canonical: Some(c), ..
        }) => c,
        _ => key,
    }
}

// ---------------------------------------------------------------------------
// Line predicate and run measurement
// ---------------------------------------------------------------------------

/// Whether `line` matches `org-element--affiliated-re`.
///
/// This tests the regexp, not the key [`keyword::probe`] emits: `#+NAME:fig:1`
/// matches here, yet as an orphan its key still runs to the last colon.
/// A carved `[dual]` matches only for a dual name, so `#+CAPTION[s]: v`
/// matches and `#+NAME[s]: v` does not.
pub(crate) fn is_affiliated_line(line: LineView) -> bool {
    keyword::affiliated_extent(line).is_some()
}

/// A measured run of affiliated keyword lines that attaches to the element
/// below it. `Copy` so the dispatcher `Element` row can store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AffiliatedRun {
    lines: u32,
    /// First line's paragraph interrupt. `#+RESULT[h]:` can attach and still
    /// not end an open paragraph; `CAPTION` and `RESULTS` do.
    interrupts: bool,
}

impl AffiliatedRun {
    /// Number of keyword lines in the run (always >= 1).
    pub(crate) fn lines(self) -> u32 {
        self.lines
    }

    /// Whether the first line ends an open paragraph.
    pub(crate) fn interrupts(self) -> bool {
        self.interrupts
    }
}

/// Measure the run whose first line is already a measured keyword shape.
///
/// `Some` only when the run attaches. The peek mirrors
/// `org-element--collect-affiliated-keywords`: a blank line, a comment, a
/// clock line or a star line (`^\*+ `, config-free, so headline and inline
/// task alike) orphans the whole run. So does the absence of a next line,
/// which covers both EOF and a container limit - the caller's window already
/// bounds `lines_after`.
///
/// Call after [`keyword::probe`]. The shape holds the orphan key; this walk
/// is the forward peek. Returning `None` leaves the caller with
/// that shape as a plain `KEYWORD` (no rewind). A multi-line run that
/// degrades emits its first line, returns, and the next `parse_one_element`
/// re-measures the shorter run, so the whole run degrades as one unit like
/// org's rewind-to-origin.
pub(crate) fn try_run(line: LineView, first: keyword::KeywordShape) -> Option<AffiliatedRun> {
    if !is_affiliated_line(line) {
        return None;
    }

    let mut lines = 1u32;
    let mut after = None;
    for next in line.lines_after() {
        if is_affiliated_line(next) {
            lines += 1;
        } else {
            after = Some(next);
            break;
        }
    }
    // No line after the run: EOF or the container limit. Org degrades both.
    let after = after?;
    if after.is_blank()
        || comment::probe(after).is_some()
        || clock::probe(after).is_some()
        || heading::star_head(after).is_some()
    {
        return None;
    }
    Some(AffiliatedRun {
        lines,
        interrupts: first.interrupts,
    })
}

/// Emit `AFFILIATED_KEYWORDS` over the measured run.
///
/// Each member is parsed with the no-post-blank keyword entry, so the run
/// holds complete lines and nothing else (I1 / I2). The target element is
/// parsed by the *next* `parse_one_element` call; the two are never coupled.
pub(crate) fn parse(parser: &mut Parser, run: AffiliatedRun) {
    let top = parser.start();
    let mut emitted = 0;
    while emitted < run.lines() {
        let Some(line) = parser.line() else {
            break;
        };
        let Some(shape) = keyword::probe(line) else {
            break;
        };
        // The run already attached. Emit the regexp key, not the orphan key
        // `probe` carries, so `CAPTION:a` narrows to `CAPTION`.
        let extent = keyword::affiliated_extent(line);
        debug_assert!(
            extent.is_some(),
            "affiliated run member matches the affiliated regexp"
        );
        let shape = match extent {
            Some((key_tokens, dual_tokens)) => shape.with_key_extent(line, key_tokens, dual_tokens),
            None => shape,
        };
        keyword::parse_line(parser, shape);
        emitted += 1;
    }
    debug_assert_eq!(
        emitted,
        run.lines(),
        "affiliated::parse ran past the measured run"
    );
    if emitted == 0 {
        // Unreachable: `decide` measured a keyword line at this very cursor.
        // Stay infallible anyway - an empty run node would violate I1.
        top.abandon(parser);
        return;
    }
    top.complete(parser, OrgSyntaxKind::AFFILIATED_KEYWORDS);
}

/// Whether an element kind accepts an affiliated keyword prefix.
///
/// Inverse of `org-element-elements-no-affiliated` for kinds in our
/// vocabulary (`org-data` is `DOCUMENT`). Not on the parse path - org's own
/// peek is line-shaped, not kind-shaped - but it is the oracle for invariant
/// I3.
#[cfg(test)]
pub(crate) fn accepts_affiliated(kind: OrgSyntaxKind) -> bool {
    !matches!(
        kind,
        OrgSyntaxKind::DOCUMENT
            | OrgSyntaxKind::COMMENT
            | OrgSyntaxKind::CLOCK
            | OrgSyntaxKind::HEADING
            | OrgSyntaxKind::INLINE_TASK
            | OrgSyntaxKind::ITEM
            | OrgSyntaxKind::NODE_PROPERTY
            | OrgSyntaxKind::PLANNING
            | OrgSyntaxKind::PROPERTY_DRAWER
            | OrgSyntaxKind::SECTION
            | OrgSyntaxKind::TABLE_ROW
    )
}

/// Probe composition for tests: keyword shape then [`try_run`].
#[cfg(test)]
pub(crate) fn probe(line: LineView) -> Option<AffiliatedRun> {
    keyword::probe(line).and_then(|shape| try_run(line, shape))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{make_parser, nodes_of_kind, parse_document_tree, probe_line, tree_text};

    // -- name dialect --------------------------------------------------------

    #[test]
    fn affiliated_key_closed_set_and_attr_rule() {
        for key in [
            "CAPTION", "DATA", "HEADER", "HEADERS", "LABEL", "NAME", "PLOT", "RESNAME", "RESULT",
            "RESULTS", "SOURCE", "SRCNAME", "TBLNAME",
        ] {
            assert!(is_affiliated_key(key), "expected affiliated: {key}");
            assert!(
                is_affiliated_key(&key.to_ascii_lowercase()),
                "caseless: {key}"
            );
        }
        assert!(is_affiliated_key("ATTR_HTML"));
        assert!(is_affiliated_key("attr_latex"));
        assert!(is_affiliated_key("ATTR_ORG-X"));
        assert!(is_affiliated_key("ATTR_A1"));
        // Bare ATTR_ / ATTR and out-of-charset backends are not affiliated.
        assert!(!is_affiliated_key("ATTR_"));
        assert!(!is_affiliated_key("ATTR"));
        assert!(!is_affiliated_key("ATTR_foo.bar"));
        assert!(!is_affiliated_key("ATTR_@"));
        assert!(!is_affiliated_key("ATTR_foo bar"));
        assert!(!is_affiliated_key("TITLE"));
        assert!(!is_affiliated_key("TODO"));
        assert!(!is_affiliated_key("CALL"));
        assert!(!is_affiliated_key(""));
        // Multi-byte keys must not panic (UTF-8 boundary-safe prefix).
        assert!(!is_affiliated_key("\u{65e5}\u{672c}\u{8a9e}"));
        assert!(!is_affiliated_key("ATTR_\u{65e5}\u{672c}"));
        assert_eq!(attr_backend("ATTR_HTML"), Some("HTML"));
        assert_eq!(attr_backend("NAME"), None);
    }

    #[test]
    fn translate_then_test_matches_org_order() {
        assert_eq!(canonical_key("TBLNAME"), "NAME");
        assert_eq!(canonical_key("tblname"), "NAME");
        assert_eq!(canonical_key("DATA"), "NAME");
        assert_eq!(canonical_key("LABEL"), "NAME");
        assert_eq!(canonical_key("RESNAME"), "NAME");
        assert_eq!(canonical_key("SOURCE"), "NAME");
        assert_eq!(canonical_key("SRCNAME"), "NAME");
        assert_eq!(canonical_key("RESULT"), "RESULTS");
        assert_eq!(canonical_key("HEADERS"), "HEADER");
        assert_eq!(canonical_key("NAME"), "NAME");
        assert_eq!(canonical_key("CAPTION"), "CAPTION");
        // ATTR_* and unknown names are their own canonical name (case kept).
        assert_eq!(canonical_key("ATTR_HTML"), "ATTR_HTML");
        assert_eq!(canonical_key("attr_html"), "attr_html");
        assert_eq!(canonical_key("TITLE"), "TITLE");

        // Flags are the canonical name's membership, not the spelling's.
        assert!(is_dual_key("CAPTION"));
        assert!(is_dual_key("RESULTS"));
        assert!(is_dual_key("RESULT"), "RESULT translates to RESULTS");
        assert!(!is_dual_key("NAME"));
        assert!(!is_dual_key("HEADER"));
        assert!(!is_dual_key("TITLE"));

        assert!(is_multiple_key("CAPTION"));
        assert!(is_multiple_key("HEADER"));
        assert!(is_multiple_key("HEADERS"), "HEADERS translates to HEADER");
        assert!(is_multiple_key("ATTR_HTML"));
        assert!(!is_multiple_key("ATTR_"));
        assert!(!is_multiple_key("NAME"));
        assert!(!is_multiple_key("RESULTS"));

        assert!(is_parsed_key("CAPTION"));
        assert!(is_parsed_key("caption"));
        assert!(!is_parsed_key("NAME"));
        assert!(!is_parsed_key("RESULTS"));
    }

    // -- acceptance table ----------------------------------------------------

    #[test]
    fn accepts_affiliated_exclusion_table() {
        for kind in [
            OrgSyntaxKind::DOCUMENT,
            OrgSyntaxKind::COMMENT,
            OrgSyntaxKind::CLOCK,
            OrgSyntaxKind::HEADING,
            OrgSyntaxKind::INLINE_TASK,
            OrgSyntaxKind::ITEM,
            OrgSyntaxKind::NODE_PROPERTY,
            OrgSyntaxKind::PLANNING,
            OrgSyntaxKind::PROPERTY_DRAWER,
            OrgSyntaxKind::SECTION,
            OrgSyntaxKind::TABLE_ROW,
        ] {
            assert!(!accepts_affiliated(kind), "{kind:?} rejects affiliation");
        }
        for kind in [
            OrgSyntaxKind::PARAGRAPH,
            OrgSyntaxKind::SRC_BLOCK,
            OrgSyntaxKind::QUOTE_BLOCK,
            OrgSyntaxKind::FIXED_WIDTH,
            OrgSyntaxKind::HORIZONTAL_RULE,
            OrgSyntaxKind::DIARY_SEXP,
            OrgSyntaxKind::KEYWORD,
            OrgSyntaxKind::PLAIN_LIST,
            OrgSyntaxKind::TABLE,
            OrgSyntaxKind::BABEL_CALL,
            OrgSyntaxKind::SETTING_KEYWORD,
            OrgSyntaxKind::FOOTNOTE_DEFINITION,
            OrgSyntaxKind::LATEX_ENVIRONMENT,
        ] {
            assert!(accepts_affiliated(kind), "{kind:?} accepts affiliation");
        }
    }

    // -- line predicate ------------------------------------------------------

    #[test]
    fn is_affiliated_line_mirrors_the_regexp() {
        let yes = [
            "#+NAME: fig\n",
            "#+name: fig\n",
            "  #+NAME: fig\n",
            "\t#+CAPTION: c\n",
            "#+ATTR_HTML: :a b\n",
            "#+CAPTION[short]: v\n",
            "#+RESULTS[hash]: v\n",
            "#+RESULT[h]: v\n",
        ];
        for src in yes {
            let p = make_parser(src);
            assert!(
                is_affiliated_line(p.line().expect(src)),
                "expected affiliated line: {src:?}"
            );
        }
        let no = [
            "#+TITLE: t\n",
            "#+TODO: A | B\n",
            "#+CALL: f()\n",
            // Dual bracket after a non-dual name: keyword yes, affiliated no.
            "#+NAME[s]: v\n",
            "#+HEADER[x]: v\n",
            "#+ATTR_HTML[x]: v\n",
            "#+ATTR_: v\n",
            "para\n",
            "* H\n",
            "\n",
        ];
        for src in no {
            let p = make_parser(src);
            let line = p.line().expect(src);
            assert!(
                !is_affiliated_line(line),
                "expected non-affiliated line: {src:?}"
            );
        }
    }

    // -- run measurement -----------------------------------------------------

    #[test]
    fn probe_measures_run_length() {
        let run = probe_line("#+NAME: a\n#+CAPTION: b\npara\n", probe).expect("run");
        assert_eq!(run.lines(), 2);
        let one = probe_line("#+NAME: a\npara\n", probe).expect("run");
        assert_eq!(one.lines(), 1);
        // A non-affiliated keyword ends the run and is the target.
        let ends = probe_line("#+NAME: a\n#+TITLE: t\npara\n", probe).expect("run");
        assert_eq!(ends.lines(), 1);
    }

    #[test]
    fn probe_degrades_on_the_org_peek_set() {
        // Blank (LF / CRLF / CR / spaces / tabs), comment, clock, star lines.
        let degrade = [
            "#+NAME: a\n\npara\n",
            "#+NAME: a\r\n\r\npara\r\n",
            "#+NAME: a\r\rpara\r",
            "#+NAME: a\n  \npara\n",
            "#+NAME: a\n\t\npara\n",
            "#+NAME: a\n# comment\npara\n",
            "#+NAME: a\nCLOCK: [2026-07-07 Wed 10:00]\npara\n",
            "#+NAME: a\n* Headline\npara\n",
            // Does not read ParseConfig: an inline-task opener orphans too.
            "#+NAME: a\n*************** Task\npara\n",
            // EOF right after the run.
            "#+NAME: a\n",
            "#+NAME: a",
        ];
        for src in degrade {
            assert!(
                probe_line(src, probe).is_none(),
                "expected degradation for {src:?}"
            );
        }
    }

    #[test]
    fn probe_degrades_at_a_container_limit() {
        // The same text attaches at section level and degrades when the run is
        // the last thing under a narrower window (org's `(>= (point) limit)`).
        let src = "#+NAME: a\npara\n";
        assert!(probe_line(src, probe).is_some(), "attaches unbounded");

        let mut p = make_parser(src);
        let cap = p.line().expect("line 0").end();
        p.with_limit_at(cap, |pp| {
            assert!(
                pp.line().and_then(probe).is_none(),
                "run at the window end must degrade"
            );
        });
    }

    // -- emitted shape -------------------------------------------------------

    #[test]
    fn attached_run_wraps_members_and_leaves_target_alone() {
        let src = "#+NAME: fig1\n#+CAPTION: A *cap*\nParagraph body.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);

        let runs = nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(tree_text(run), "#+NAME: fig1\n#+CAPTION: A *cap*\n");
        assert_eq!(run.children().count(), 2);
        assert!(
            run.children().all(|c| c.kind() == OrgSyntaxKind::KEYWORD),
            "members keep kind KEYWORD"
        );
        let target = run.next_sibling().expect("target exists (I3/I4)");
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);
        assert_eq!(tree_text(&target), "Paragraph body.\n");
    }

    #[test]
    fn degraded_run_is_plain_keywords_with_post_blank() {
        let src = "#+NAME: a\n#+CAPTION: b\n\npara\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
            "all members degrade, none attach"
        );
        let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert_eq!(kws.len(), 2);
        assert_eq!(tree_text(&kws[0]), "#+NAME: a\n");
        // The last member owns the blank line, exactly as a lone keyword does.
        assert_eq!(tree_text(&kws[1]), "#+CAPTION: b\n\n");
    }

    /// The one run node in `src`, with the sibling it attaches to.
    fn run_and_target(src: &str) -> (crate::syntax::SyntaxNode, crate::syntax::SyntaxNode) {
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "lossless tiling for {src:?}");
        let runs = nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(runs.len(), 1, "expected exactly one run in {src:?}");
        let target = runs[0]
            .next_sibling()
            .unwrap_or_else(|| panic!("I3/I4: run has a next sibling in {src:?}"));
        (runs[0].clone(), target)
    }

    #[test]
    fn a_run_attaches_to_every_accepting_element_kind() {
        let cases: &[(&str, OrgSyntaxKind)] = &[
            ("Paragraph body.\n", OrgSyntaxKind::PARAGRAPH),
            (
                "#+begin_quote\nQ\n#+end_quote\n",
                OrgSyntaxKind::QUOTE_BLOCK,
            ),
            (
                "#+begin_src rust\nlet x = 1;\n#+end_src\n",
                OrgSyntaxKind::SRC_BLOCK,
            ),
            ("| a | b |\n", OrgSyntaxKind::TABLE),
            ("- item\n- second\n", OrgSyntaxKind::PLAIN_LIST),
            (":LOGBOOK:\nx\n:END:\n", OrgSyntaxKind::DRAWER),
            (": fixed width\n", OrgSyntaxKind::FIXED_WIDTH),
            ("-----\n", OrgSyntaxKind::HORIZONTAL_RULE),
            ("[fn:1] a note\n", OrgSyntaxKind::FOOTNOTE_DEFINITION),
            ("#+TITLE: t\n", OrgSyntaxKind::KEYWORD),
            ("#+TODO: T | D\n", OrgSyntaxKind::SETTING_KEYWORD),
            ("#+CALL: f()\n", OrgSyntaxKind::BABEL_CALL),
            ("%%(org-calendar-holiday)\n", OrgSyntaxKind::DIARY_SEXP),
            (
                "\\begin{equation}\nx\n\\end{equation}\n",
                OrgSyntaxKind::LATEX_ENVIRONMENT,
            ),
        ];
        for (tail, kind) in cases {
            let src = format!("#+NAME: fig1\n#+CAPTION: cap\n{tail}");
            let (run, target) = run_and_target(&src);
            assert_eq!(run.children().count(), 2, "two members for {kind:?}");
            assert_eq!(target.kind(), *kind, "target kind for {src:?}");
            assert!(
                accepts_affiliated(target.kind()),
                "I3 oracle: {kind:?} accepts affiliation"
            );
            assert_eq!(
                tree_text(&target),
                *tail,
                "the target's own range is unchanged by affiliation"
            );
        }
    }

    #[test]
    fn indented_run_attaches_to_an_indented_target() {
        let (run, target) = run_and_target("  #+NAME: n\n  #+CAPTION: c\n  indented para\n");
        assert_eq!(tree_text(&run), "  #+NAME: n\n  #+CAPTION: c\n");
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);
    }

    #[test]
    fn greater_block_affiliation_is_local() {
        let src = concat!(
            "#+NAME: outer\n",
            "#+begin_quote\n",
            "#+NAME: inner\n",
            "quoted body\n",
            "#+end_quote\n",
        );
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let runs = nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(runs.len(), 2, "one run outside, one inside the block");
        assert_eq!(tree_text(&runs[0]), "#+NAME: outer\n");
        assert_eq!(
            runs[0].next_sibling().map(|n| n.kind()),
            Some(OrgSyntaxKind::QUOTE_BLOCK)
        );
        assert_eq!(tree_text(&runs[1]), "#+NAME: inner\n");
        assert_eq!(
            runs[1].next_sibling().map(|n| n.kind()),
            Some(OrgSyntaxKind::PARAGRAPH)
        );
        assert_eq!(
            runs[1].parent().map(|n| n.kind()),
            Some(OrgSyntaxKind::QUOTE_BLOCK),
            "the inner run lives under the block, not beside it"
        );
    }

    #[test]
    fn run_members_cover_aliases_repeats_and_attrs() {
        let src = concat!(
            "#+SRCNAME: s\n",
            "#+DATA: d\n",
            "#+TBLNAME: t\n",
            "#+CAPTION: first\n",
            "#+CAPTION: second\n",
            "#+HEADER: h1\n",
            "#+HEADERS: h2\n",
            "#+RESULT: r\n",
            "#+ATTR_HTML: :class x\n",
            "#+ATTR_HTML: :id y\n",
            "#+ATTR_LATEX: :width 5\n",
            "para\n",
        );
        let (run, target) = run_and_target(src);
        assert_eq!(run.children().count(), 11);
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);

        // Canonical grouping is a name-table question over the members.
        let keys: Vec<String> = run
            .children()
            .filter_map(|c| {
                c.children()
                    .find(|n| n.kind() == OrgSyntaxKind::KEYWORD_KEY)
                    .map(|n| n.text().to_string())
            })
            .collect();
        let canon: Vec<&str> = keys.iter().map(|k| canonical_key(k)).collect();
        assert_eq!(canon.iter().filter(|c| **c == "NAME").count(), 3);
        assert_eq!(canon.iter().filter(|c| **c == "CAPTION").count(), 2);
        assert_eq!(canon.iter().filter(|c| **c == "HEADER").count(), 2);
        assert_eq!(canon.iter().filter(|c| **c == "RESULTS").count(), 1);
        assert_eq!(run.children().count() - 8, 3, "three ATTR_* members");
    }

    #[test]
    fn a_non_affiliated_key_ends_the_run() {
        // NAME attaches to the TITLE keyword element; the paragraph gets none.
        let src = "#+NAME: n\n#+TITLE: t\npara\n";
        let (run, target) = run_and_target(src);
        assert_eq!(tree_text(&run), "#+NAME: n\n");
        assert_eq!(target.kind(), OrgSyntaxKind::KEYWORD);
        assert_eq!(tree_text(&target), "#+TITLE: t\n");
        let paras = nodes_of_kind(&parse_document_tree(src), OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1);
        assert!(
            paras[0].prev_sibling().map(|n| n.kind()) != Some(OrgSyntaxKind::AFFILIATED_KEYWORDS),
            "the paragraph inherits nothing"
        );
    }

    #[test]
    fn attr_charset_gates_run_membership() {
        for key in ["ATTR_HTML", "ATTR_ORG-X", "ATTR_A1"] {
            let src = format!("#+{key}: v\npara\n");
            let (run, _) = run_and_target(&src);
            assert_eq!(run.children().count(), 1, "{key} is a run member");
        }
        for key in ["ATTR_", "ATTR", "ATTR_foo.bar", "ATTR_@", "ATTR_\u{65e5}"] {
            let src = format!("#+{key}: v\npara\n");
            let root = parse_document_tree(&src);
            assert_eq!(tree_text(&root), src);
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
                "{key} must not affiliate"
            );
        }
    }

    #[test]
    fn blank_between_keywords_degrades_only_the_first() {
        let src = "#+NAME: orphan\n\n#+CAPTION: keep\nBody.\n";
        let (run, target) = run_and_target(src);
        assert_eq!(tree_text(&run), "#+CAPTION: keep\n");
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);
        // The orphan is a plain sibling before the run, owning its blank.
        let orphan = run.prev_sibling().expect("orphaned keyword");
        assert_eq!(orphan.kind(), OrgSyntaxKind::KEYWORD);
        assert_eq!(tree_text(&orphan), "#+NAME: orphan\n\n");
    }

    #[test]
    fn a_run_at_a_body_or_section_end_degrades() {
        // Greater-block body end, footnote-definition body end, section end
        // before the next heading, and EOF: no next line under the window.
        let cases: &[(&str, &str)] = &[
            (
                "#+begin_quote\nbody\n#+NAME: x\n#+end_quote\n",
                "greater-block body end",
            ),
            ("[fn:1] note\n#+NAME: x\n\n\npara\n", "footnote body end"),
            (
                "* H\npara\n#+NAME: x\n* H2\n",
                "section end before a heading",
            ),
            ("para\n#+NAME: x\n", "EOF"),
        ];
        for (src, desc) in cases {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), *src, "lossless for {desc}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
                "must degrade at {desc}"
            );
            assert!(
                !nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).is_empty(),
                "the line stays a plain KEYWORD at {desc}"
            );
        }

        // Same text one level out: at section level the run attaches.
        let (_, target) = run_and_target("#+NAME: x\npara\n");
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);
    }

    // -- dual bracket --------------------------------------------------------

    #[test]
    fn dual_membership_follows_the_name_table() {
        // Dual names keep their bracket and stay run members.
        for src in [
            "#+CAPTION[s]: v\npara\n",
            "#+RESULTS[hash]: v\npara\n",
            "#+RESULT[h]: v\npara\n",
        ] {
            let (run, _) = run_and_target(src);
            assert_eq!(run.children().count(), 1, "{src:?}");
            assert_eq!(
                nodes_of_kind(&run, OrgSyntaxKind::KEYWORD_DUAL).len(),
                1,
                "dual carved for {src:?}"
            );
        }

        // A bracket on a non-dual name is still carved, but ends the run: the
        // line is a keyword element, not an affiliated one.
        let src = "#+CAPTION: c\n#+NAME[s]: v\npara\n";
        let (run, target) = run_and_target(src);
        assert_eq!(tree_text(&run), "#+CAPTION: c\n");
        assert_eq!(target.kind(), OrgSyntaxKind::KEYWORD);
        assert_eq!(tree_text(&target), "#+NAME[s]: v\n");
        assert_eq!(
            nodes_of_kind(&target, OrgSyntaxKind::KEYWORD_DUAL).len(),
            1,
            "the dual slot is carved regardless of the name"
        );
    }

    #[test]
    fn a_bracket_never_costs_keyword_hood() {
        // Every one of these is a KEYWORD element in org; none is a paragraph.
        // `(source, key text, dual text)` - org's flattened `:key` is the two
        // concatenated.
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("#+FOO[bar]: v\n", "FOO", Some("[bar]")),
            ("#+FOO[bar]baz: v\n", "FOO[bar]baz", None),
            ("#+FOO[bar: v\n", "FOO[bar", None),
            ("#+CAPTION[a][b]: v\n", "CAPTION", Some("[a][b]")),
            ("#+FOO[]: v\n", "FOO", Some("[]")),
        ];
        for (src, key, dual) in cases {
            // Orphaned by EOF so the line stands alone as an element.
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), *src);
            let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
            assert_eq!(kws.len(), 1, "{src:?} must be a KEYWORD, not a paragraph");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).is_empty(),
                "{src:?} must not degrade to a paragraph"
            );
            let key_text = nodes_of_kind(&kws[0], OrgSyntaxKind::KEYWORD_KEY)
                .first()
                .map(tree_text)
                .expect("KEYWORD_KEY");
            assert_eq!(key_text, *key, "key extent for {src:?}");
            let dual_text = nodes_of_kind(&kws[0], OrgSyntaxKind::KEYWORD_DUAL)
                .first()
                .map(tree_text);
            assert_eq!(dual_text.as_deref(), *dual, "dual extent for {src:?}");
        }
    }

    #[test]
    fn orphaned_dual_keyword_keeps_orgs_flattened_key_recoverable() {
        // Org restructures this line depending on a blank line above the
        // target; the CST keeps one shape and lets the view flatten.
        let src = "#+CAPTION[short]: v\n\npara\n";
        let root = parse_document_tree(src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );
        let kw = &nodes_of_kind(&root, OrgSyntaxKind::KEYWORD)[0];
        let key = tree_text(&nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_KEY)[0]);
        let dual = tree_text(&nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_DUAL)[0]);
        assert_eq!(format!("{key}{dual}"), "CAPTION[short]", "org `:key`");
    }

    #[test]
    fn caption_inlines_expand_in_the_dual_only_for_caption() {
        let (run, _) = run_and_target("#+CAPTION[short *s*]: main *body*\npara\n");
        assert!(
            nodes_of_kind(&run, OrgSyntaxKind::BOLD).len() >= 2,
            "caption dual and value both hold objects"
        );
        let (flat, _) = run_and_target("#+RESULTS[*not* bold]: out\npara\n");
        assert!(
            nodes_of_kind(&flat, OrgSyntaxKind::BOLD).is_empty(),
            "a non-parsed dual stays flat"
        );
    }

    // -- invariants over the shared corpus -----------------------------------

    #[test]
    fn run_invariants_hold_across_the_taxonomy_corpus() {
        use crate::tests::{TAXONOMY, parse_document_tree_with_config};

        let mut seen = 0usize;
        for (src, cfg) in TAXONOMY {
            let root = parse_document_tree_with_config(src, &cfg.build());
            for run in nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS) {
                seen += 1;
                // I1: one or more KEYWORD children, each a complete line.
                let members: Vec<_> = run.children().collect();
                assert!(!members.is_empty(), "I1: non-empty run in {src:?}");
                for m in &members {
                    assert_eq!(m.kind(), OrgSyntaxKind::KEYWORD, "I1: member kind");
                    let text = tree_text(m);
                    assert!(
                        text.ends_with('\n') || text.ends_with('\r'),
                        "I1: member is a complete line, got {text:?}"
                    );
                }
                // I1: the run holds nothing but its members (no stray tokens).
                assert_eq!(
                    run.children_with_tokens().count(),
                    members.len(),
                    "I1: run holds only KEYWORD children in {src:?}"
                );
                // I2: no blank line inside, no post-blank claimed.
                let text = tree_text(&run);
                assert!(
                    !text.lines().any(|l| l.trim().is_empty()),
                    "I2: no blank line in {text:?}"
                );
                // I3 / I4: the next sibling exists and accepts affiliation.
                let target = run
                    .next_sibling()
                    .unwrap_or_else(|| panic!("I3/I4: run has a next sibling in {src:?}"));
                assert!(
                    accepts_affiliated(target.kind()),
                    "I3: {:?} accepts affiliation in {src:?}",
                    target.kind()
                );
                assert_eq!(
                    run.next_sibling_or_token()
                        .and_then(rowan::NodeOrToken::into_node),
                    Some(target),
                    "I4: nothing but the target follows the run"
                );
            }
        }
        assert!(seen > 0, "the corpus must exercise AFFILIATED_KEYWORDS");
    }

    // -- non-regression ------------------------------------------------------

    /// Wrapping a run does not change what its members *are*: they stay
    /// `KEYWORD` nodes, reachable by any descendants walk.
    ///
    /// Settings collection and typed-view discovery both find keywords by a
    /// descendants walk, so a run that hid its members would silently drop
    /// them from both.
    #[test]
    fn run_members_stay_ordinary_keyword_nodes() {
        let src = "#+NAME: fig\n#+CAPTION: cap\npara\n";
        let root = crate::tests::parse_document_tree(src);
        assert_eq!(
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).len(),
            1,
            "the run is wrapped"
        );
        assert_eq!(
            crate::tests::nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(),
            2,
            "and both members are still KEYWORD descendants"
        );
    }

    #[test]
    fn tblfm_and_footnote_boundaries_are_unchanged() {
        // `#+TBLFM:` is not affiliated: it stays a suffix child of TABLE.
        let src = "| a |\n#+TBLFM: $1=1\nafter\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );
        let table = &nodes_of_kind(&root, OrgSyntaxKind::TABLE)[0];
        assert_eq!(nodes_of_kind(table, OrgSyntaxKind::KEYWORD).len(), 1);

        // A run above the next definition stays outside the previous body and
        // attaches to the definition below it.
        let src = "[fn:1] first\n#+NAME: n\n[fn:2] second\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let defs = nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION);
        assert_eq!(defs.len(), 2);
        assert_eq!(tree_text(&defs[0]), "[fn:1] first\n");
        let run = defs[1].prev_sibling().expect("run above the second def");
        assert_eq!(run.kind(), OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(tree_text(&run), "#+NAME: n\n");
    }
}
