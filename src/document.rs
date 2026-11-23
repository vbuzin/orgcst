//! Owning document snapshot, the single function that builds one
//! (`materialize`), and the producer that resolves in-buffer settings over it.
//!
//! Only settings that change how the tree is carved reach the effective
//! `ParseConfig`; today that is the TODO vocabulary. Everything else a document
//! declares is read off the tree by the typed views in `ast`, and this module
//! does not import `ast`, so a snapshot never depends on its views.
//!
//! Text is canonical and the CST is derived from it. Every producer of a
//! [`Document`] (`parse`, `parse_with`, [`Document::apply`]) goes through
//! `materialize`, which takes an optional `prior` document as a reuse hint:
//! same text under a new vocabulary recarves only the affected star lines, and
//! a text change reuses the enclosing outline when its keyword name sets match.
//! Otherwise the parse is cold. `prior` never changes the result.
//!
//! Each returned document is a `materialize` of its own text under its
//! effective config, and its `prior` is the document it was derived from
//! (`None` on open, the pre-edit document on apply). To learn the effective
//! config a producer must first parse and read the settings off the tree; that
//! tree either is the result, when the guess was right, or becomes the `prior`
//! of the refresh that is. No parse is thrown away. Rebuilding from
//! `DocumentParts` (behind [`crate::PortableDocument`]) wraps an existing green
//! tree in O(1) and is not a producer.

use crate::assert_max_size;
use crate::edit::EditSet;
use crate::parser::{self, Diagnostic, ParseConfig};
use crate::settings;
use crate::syntax::SyntaxNode;
use rowan::{GreenNode, TextRange};
use std::sync::Arc;

/// Self-contained parse snapshot: source, config, tree, diagnostics.
///
/// Thread-affine: the red root is `!Send` / `!Sync`, as are typed views and
/// [`crate::EditContext`]. Move a snapshot with [`crate::PortableDocument`].
///
/// [`Document::apply`] returns a new value and leaves the original valid, so a
/// caller can keep earlier snapshots for undo.
///
/// The public config is the **effective** one the tree was carved under.
/// Internally the snapshot also keeps the caller's config it was resolved
/// over, so that an edit can re-resolve the in-buffer settings with no
/// ambient state. That base cannot be recovered from the effective config,
/// because a file's TODO vocabulary replaces the caller's. The two differ at
/// most in the vocabulary, and share one allocation when the file declares
/// none.
#[derive(Debug, Clone)]
pub struct Document {
    source: Arc<str>,
    root: SyntaxNode,
    config: Arc<ParseConfig>,
    caller: Arc<ParseConfig>,
    diagnostics: Vec<Diagnostic>,
}

/// The five `Send` parts of a snapshot. Not public: callers move a snapshot
/// with [`crate::PortableDocument`].
pub(crate) struct DocumentParts {
    pub source: Arc<str>,
    pub green: GreenNode,
    pub config: Arc<ParseConfig>,
    pub caller: Arc<ParseConfig>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Build a document from text and config.
///
/// `prior` is a reuse hint. When the delta does not touch a structural knob
/// (link types, user entities, inline-task level, alphabetical bullets,
/// nesting budget; none of them settable in-buffer), equal name sets on the
/// same bytes keep the prior green tree, a different name set on the same
/// bytes recarves the damaged star lines, and different bytes with equal name
/// sets replace one enclosing outline or the zeroth section.
///
/// That outline path still returns none, and the cold path runs, when no
/// single outline or zeroth section covers the edit; a heading was added,
/// removed, or re-levelled and no ancestor is strictly shallower (a new
/// level-1 heading is this case even though the name sets match); the
/// fragment runs past the mapped end and no parent outline remains; or a
/// zeroth-section slice begins with a file lead-in blank (`\n`, `\r\n`, or
/// a lone `\r`, optional indent). A structural-knob delta, or different
/// bytes together with a name-set change, is also cold.
///
/// Contract: for all `text`, `config`, `prior`,
/// `materialize(t, c, Some(p))` is byte-identical (tree and diagnostics) to
/// `materialize(t, c, None)`.
///
/// The result's caller layer is the config it was carved under; a producer
/// that resolved over a different one corrects it with [`Document::with_caller`].
/// Reuse does not count as a cold tree build and does not resolve settings.
pub(crate) fn materialize(
    text: impl Into<Arc<str>>,
    config: Arc<ParseConfig>,
    prior: Option<&Document>,
) -> Document {
    let source: Arc<str> = text.into();
    assert_max_size(source.as_ref());
    if let Some(prior) = prior
        && let Some(reused) = crate::damage::try_reuse(&source, &config, prior)
    {
        return reused;
    }
    #[cfg(test)]
    tree_builds::record();
    let mut p = parser::Parser::with_config(source.as_ref(), config.as_ref());
    parser::grammar::parse_document(&mut p);
    let (toks, evs, diagnostics) = p.finish();
    let green = parser::builder::build(source.as_ref(), &toks, evs);
    Document {
        source,
        root: SyntaxNode::new_root(green),
        caller: Arc::clone(&config),
        config,
        diagnostics,
    }
}

/// Counts cold [`materialize`] builds so tests can pin what a producer costs.
///
/// Same-bytes file-layer reuse does not tick this. A refresh that falls back
/// to a second cold parse shows up here. Per thread, because the test harness
/// runs test functions concurrently.
#[cfg(test)]
pub(crate) mod tree_builds {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record() {
        COUNT.with(|c| c.set(c.get() + 1));
    }

    /// Number of trees built while running `f`.
    pub(crate) fn while_running<T>(f: impl FnOnce() -> T) -> (T, usize) {
        COUNT.with(|c| c.set(0));
        let value = f();
        (value, COUNT.with(Cell::get))
    }
}

/// The one producer above [`materialize`]: guess the effective config, confirm
/// it against what the document says about itself, refresh if they disagree.
///
/// ```text
/// base     = prior's effective config, or the caller layer on a cold open
/// interim  = materialize(text, base, prior)
/// resolved = resolve(collect(interim) over caller)     the TODO vocabulary
/// resolved == base ?
///   yes -> interim IS the result: same text, same effective config, same
///          predecessor, so it is the value of the call the result must be
///   no  -> materialize(text, resolved, Some(&interim))       config damage
/// ```
///
/// `parse` and `parse_with` pass `prior = None`; `apply` passes the pre-edit
/// document. That is the only difference between open and apply.
///
/// **Guessing with `base` is what makes this cheap.** A cold open guesses the
/// caller layer, so a file that declares nothing (or only restates what the
/// caller already set) costs one cold parse. An apply guesses the
/// predecessor's effective config. A config-stable edit reuses the enclosing
/// outline when that hop's name sets match, and cold-parses when they do not
/// or the edit has no outline to replace; same bytes reuse the tree.
///
/// The second call is **config damage**: the text is unchanged, the
/// effective vocabulary moved, and `materialize` with the interim as `prior`
/// recarves only the damaged star lines instead of parsing the file again.
/// The text hop and the config hop are kept separate.
///
/// On an apply that moves the config, the chain is
/// `self -> interim (text) -> result (config)`: the result's predecessor is
/// the same-text interim, which is itself a successor of the pre-edit
/// document. Each hop is one damage class.
pub(crate) fn materialize_resolved(
    text: Arc<str>,
    caller: Arc<ParseConfig>,
    prior: Option<&Document>,
) -> Document {
    let base = prior.map_or_else(|| Arc::clone(&caller), |p| Arc::clone(&p.config));
    let interim = materialize(Arc::clone(&text), Arc::clone(&base), prior);
    let file = settings::collect_file_layer(interim.root());
    let resolved = settings::resolve(&caller, &file);

    let document = if Arc::ptr_eq(&resolved, &base) || *resolved == *base {
        interim
    } else {
        materialize(text, resolved, Some(&interim))
    };
    document.with_caller(caller)
}

impl Document {
    pub(crate) fn parts(&self) -> DocumentParts {
        DocumentParts {
            source: Arc::clone(&self.source),
            green: self.root.green().into_owned(),
            config: Arc::clone(&self.config),
            caller: Arc::clone(&self.caller),
            diagnostics: self.diagnostics.clone(),
        }
    }

    pub(crate) fn into_parts(self) -> DocumentParts {
        DocumentParts {
            source: self.source,
            green: self.root.green().into_owned(),
            config: self.config,
            caller: self.caller,
            diagnostics: self.diagnostics,
        }
    }

    /// Reconstruct without materialize. Restores both config layers;
    /// caller is restored independently of the effective layer.
    pub(crate) fn from_parts(parts: DocumentParts) -> Self {
        Document {
            source: parts.source,
            root: SyntaxNode::new_root(parts.green),
            config: parts.config,
            caller: parts.caller,
            diagnostics: parts.diagnostics,
        }
    }

    #[must_use]
    /// The source text of this snapshot.
    pub fn source(&self) -> &str {
        self.source.as_ref()
    }

    /// Byte-exact serialisation of this snapshot.
    ///
    /// Equal to [`Self::source`] for every document this crate produces:
    /// cold `parse` / `parse_with` and [`Self::apply`]. There is no
    /// pretty-printed form.
    #[inline]
    #[must_use]
    pub fn write(&self) -> &str {
        self.source()
    }

    /// Source bytes of `range`.
    ///
    /// Use it to copy a subtree verbatim (`Outline::subtree_range`,
    /// `AstNode::range`).
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds or not on a UTF-8 char boundary.
    /// Ranges taken from typed views never panic.
    #[must_use]
    pub fn slice(&self, range: TextRange) -> &str {
        let s = self.source();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        &s[start..end]
    }

    #[must_use]
    /// The root `DOCUMENT` node of the syntax tree.
    pub fn root(&self) -> &SyntaxNode {
        &self.root
    }

    #[must_use]
    /// Recoverable problems found while parsing. Parsing never fails, so
    /// these are the only report of malformed input.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Effective configuration: the one this document's tree was carved under.
    ///
    /// It holds only the inputs that shaped the CST. Most callers want a query
    /// about a node instead (`todo_kind(&heading)`, `todo_sequence(&heading)`,
    /// `property(&outline, key)`). Read this when you need the vocabulary in
    /// effect, for example to render a cycling menu. It can differ from what
    /// the document declares: a document that declares no vocabulary is carved
    /// under the caller's config, which is not in the text.
    #[must_use]
    pub fn config(&self) -> &ParseConfig {
        self.config.as_ref()
    }

    /// Replace the retained caller layer, leaving the tree and its effective
    /// config alone.
    ///
    /// The effective config describes the tree, so it may only change by
    /// re-carving.
    fn with_caller(mut self, caller: Arc<ParseConfig>) -> Document {
        self.caller = caller;
        self
    }

    /// Splice edits into source, re-resolve the effective config from the new
    /// text, and materialize against this document. Infallible once a valid
    /// [`EditSet`] exists for this document's source.
    ///
    /// Effective config is a function of the **new** text, so an edit that
    /// adds, removes, or rewrites a `#+TODO:` line takes effect in the same
    /// pass as the edit itself. It is resolved over the retained caller config
    /// rather than this document's effective config, so file settings do not
    /// accumulate across edits.
    ///
    /// Two hops, one result. Same bytes reuse the tree, or recarve the star
    /// lines whose keyword membership changed. A byte change reuses one
    /// outline or the zeroth section when the name sets on that hop match,
    /// and cold-parses only when that reuse does not apply. An edit that
    /// moves the vocabulary takes both hops: the text under the previous
    /// vocabulary, then the star-line recarve, since the new vocabulary is
    /// only known once there is a tree to read it from.
    ///
    /// [`EditSet::apply_to_text`] checks that the set belongs to this source.
    ///
    /// # Panics
    ///
    /// Panics if `edits` was not validated against text byte-identical to
    /// `self.source()` (out of bounds or mid-character ranges).
    ///
    /// Panics if the spliced text exceeds [`crate::MAX_SOURCE_LEN`].
    ///
    /// # Examples
    ///
    /// One replacement. [`Self::write`] is the spliced text. This example
    /// does not locate a node in the resulting snapshot.
    ///
    /// ```
    /// use orgcst::{Edit, EditSet, TextRange, TextSize, parse};
    ///
    /// let doc = parse("* Heading\n\nbody word\n");
    /// let at = u32::try_from(
    ///     doc.source().find("word").expect("fixture contains word"),
    /// )
    /// .expect("offset fits TextSize");
    /// let start = TextSize::from(at);
    /// let edits = EditSet::new(
    ///     doc.source(),
    ///     vec![Edit {
    ///         range: TextRange::new(start, start + TextSize::of("word")),
    ///         replacement: "text".to_string(),
    ///     }],
    /// )
    /// .expect("the range is inside the source");
    /// let applied = doc.apply(&edits);
    /// assert_eq!(applied.write(), edits.apply_to_text(doc.source()));
    /// assert_eq!(applied.write(), "* Heading\n\nbody text\n");
    /// ```
    #[must_use = "applied Document is unused"]
    pub fn apply(&self, edits: &EditSet) -> Document {
        let new_text: Arc<str> = edits.apply_to_text(self.source()).into();
        materialize_resolved(new_text, Arc::clone(&self.caller), Some(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Edit, EditSet};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind};
    use crate::{PortableDocument, TodoKind, parse, parse_with};
    use rowan::{TextRange, TextSize};

    /// Full snapshot equality, layers included.
    ///
    /// Two documents can share a tree and still disagree about the config it
    /// was carved under, or about the caller layer a later edit will re-resolve
    /// over. Both are bugs, and a tree-only oracle cannot see either.
    fn assert_same_document(a: &Document, b: &Document, ctx: &str) {
        assert_eq!(a.source(), b.source(), "source mismatch: {ctx}");
        assert_eq!(a.root().green(), b.root().green(), "tree mismatch: {ctx}");
        assert_eq!(
            a.diagnostics(),
            b.diagnostics(),
            "diagnostics mismatch: {ctx}"
        );
        assert_eq!(a.config(), b.config(), "effective config mismatch: {ctx}");
        assert_eq!(a.caller, b.caller, "caller layer mismatch: {ctx}");
    }

    fn open(text: &str, config: &ParseConfig) -> Document {
        materialize_resolved(Arc::from(text), Arc::new(config.clone()), None)
    }

    /// Text of every carved `TODO_KEYWORD`, in document order.
    fn carved_todo_keywords(doc: &Document) -> Vec<String> {
        nodes_of_kind(doc.root(), OrgSyntaxKind::TODO_KEYWORD)
            .iter()
            .map(|n| n.text().to_string())
            .collect()
    }

    #[test]
    fn round_trip_over_taxonomy() {
        for &(src, cfg_tag) in TAXONOMY {
            let cfg = cfg_tag.build();
            let doc = parse_with(src, &cfg);
            assert_eq!(
                doc.root().text().to_string(),
                doc.source(),
                "round-trip at Document interface"
            );
            assert_eq!(doc.source(), src);
        }
    }

    #[test]
    fn write_is_source_over_taxonomy() {
        for &(src, cfg_tag) in TAXONOMY {
            let doc = parse_with(src, &cfg_tag.build());
            assert_eq!(doc.write(), src);
            assert!(std::ptr::eq(doc.write(), doc.source()));
            assert_eq!(doc.root().text().to_string(), src);
        }
    }

    #[test]
    fn apply_then_write_equals_apply_to_text() {
        let doc = parse("* A\n* B\n");
        let set = EditSet::single(
            doc.source(),
            TextRange::new(TextSize::from(0), TextSize::from(5)),
            "* C\n",
        )
        .expect("first-line replace is in bounds");
        let next = doc.apply(&set);
        assert_eq!(next.write(), set.apply_to_text(doc.write()));
        assert!(std::ptr::eq(next.write(), next.source()));
    }

    #[test]
    fn slice_copies_a_view_minted_range() {
        let doc = parse("* Title\n");
        let heading = nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .expect("heading");
        assert_eq!(doc.slice(heading.text_range()), "* Title\n");
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn slice_panics_on_out_of_bounds_range() {
        let doc = parse("* A\n");
        let _ = doc.slice(TextRange::new(TextSize::from(0), TextSize::from(99)));
    }

    #[test]
    fn bom_same_line_as_stars_is_a_paragraph() {
        let src = "\u{feff}* H\n";
        let doc = parse(src);
        assert_eq!(doc.write(), src);
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING).is_empty(),
            "star_head wants a line-start STAR; a leading BOM is TEXT"
        );
    }

    #[test]
    fn bom_own_line_leaves_the_next_heading_intact() {
        let src = "\u{feff}\n* H\n";
        let doc = parse(src);
        assert_eq!(doc.write(), src);
        assert_eq!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING).len(),
            1,
            "BOM on its own line does not absorb the following star-head"
        );
        assert!(
            !nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE).is_empty(),
            "{:?}",
            nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE)
        );
    }

    #[test]
    fn determinism_green_equality() {
        // Same-process only: does not prove cross-process replica equality.
        for &(src, cfg_tag) in TAXONOMY {
            let cfg = cfg_tag.build();
            let a = parse_with(src, &cfg);
            let b = parse_with(src, &cfg);
            assert_same_document(&a, &b, "determinism");
        }
    }

    /// `prior` may never change the result, whether it is this document's own
    /// predecessor or an unrelated one. The whole corpus: fixtures are appended
    /// over time, so sampling them would leave each new construct uncovered.
    #[test]
    fn prior_invariance_same_and_wrong_prior() {
        let cfg = ParseConfig::default();
        for &(src, cfg_tag) in TAXONOMY {
            let c = cfg_tag.build();
            let cold = materialize(src, Arc::new(c.clone()), None);
            let warm = materialize(src, Arc::new(c.clone()), Some(&cold));
            assert_same_document(
                &cold,
                &warm,
                "prior-invariance: reuse path must match cold path (oracle)",
            );
            let older = materialize("unrelated prior\n", Arc::new(cfg.clone()), None);
            let with_wrong = materialize(src, Arc::new(c), Some(&older));
            assert_same_document(
                &cold,
                &with_wrong,
                "prior-invariance: wrong prior must not change result (cold path is definitionally right)",
            );
        }
    }

    #[test]
    fn prior_invariance_config_varying() {
        let src = "* TODO task\n";
        let default = ParseConfig::default();
        let with_next = ParseConfig::default().with_todo_keywords([
            ("TODO", TodoKind::Active),
            ("NEXT", TodoKind::Active),
            ("DONE", TodoKind::Done),
        ]);
        let a = materialize(src, Arc::new(default), None);
        let b = materialize(src, Arc::new(with_next.clone()), Some(&a));
        let cold_b = materialize(src, Arc::new(with_next), None);
        assert_same_document(
            &b,
            &cold_b,
            "prior-invariance under config change: cold path is the oracle",
        );
    }

    fn vocab_variants(base: &ParseConfig) -> [(&'static str, ParseConfig); 3] {
        [
            (
                "shrink",
                base.clone()
                    .with_todo_keywords([("TODO", TodoKind::Active)]),
            ),
            (
                "grow",
                base.clone().with_todo_keywords([
                    ("TODO", TodoKind::Active),
                    ("NEXT", TodoKind::Active),
                    ("DONE", TodoKind::Done),
                ]),
            ),
            (
                "disjoint",
                base.clone()
                    .with_todo_keywords([("BUG", TodoKind::Active), ("FIXED", TodoKind::Done)]),
            ),
        ]
    }

    fn assert_reuse_matches_cold(
        src: &str,
        prior_cfg: &ParseConfig,
        next_cfg: ParseConfig,
        ctx: &str,
    ) {
        let prior = materialize(src, Arc::new(prior_cfg.clone()), None);
        let warm = materialize(src, Arc::new(next_cfg.clone()), Some(&prior));
        let cold = materialize(src, Arc::new(next_cfg), None);
        assert_same_document(&warm, &cold, ctx);
    }

    /// Vocabulary changes, including ones that recarve inline tasks, stay
    /// byte-identical to a cold materialize.
    #[test]
    fn prior_invariance_over_taxonomy_vocabularies() {
        for &(src, cfg_tag) in TAXONOMY {
            let base = cfg_tag.build();
            for (name, cfg) in vocab_variants(&base) {
                assert_reuse_matches_cold(src, &base, cfg, &format!("{name}: {src:?}"));
            }
        }

        let inline = "\
*************** TODO outer\n\
kept\n\
*************** BUG inner\n\
inner\n\
*************** END\n\
*************** END\n\
- a\n\
*************** TODO inlist\n\
*************** END\n\
- b\n\
*** TODO lowered\n\
*** END\n\
* DONE gone\n\
* NEXT added\n";
        let level3 = ParseConfig::default().with_inline_task_min_level(3);
        for (name, cfg) in vocab_variants(&ParseConfig::default()) {
            assert_reuse_matches_cold(inline, &ParseConfig::default(), cfg, name);
        }
        for (name, cfg) in vocab_variants(&level3) {
            assert_reuse_matches_cold(inline, &level3, cfg, &format!("level3 {name}"));
        }
    }

    #[test]
    fn sequence_only_reuses_green_and_installs_the_new_config() {
        let src = "* TODO x\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let swapped = ParseConfig::default()
            .with_todo_keywords([("TODO", TodoKind::Done), ("DONE", TodoKind::Active)]);
        let (warm, trees) = tree_builds::while_running(|| {
            materialize(src, Arc::new(swapped.clone()), Some(&prior))
        });
        assert_eq!(trees, 0, "equal name sets must not cold-parse");
        assert!(std::ptr::eq(
            prior.root().green().as_ref(),
            warm.root().green().as_ref()
        ));
        let cold = materialize(src, Arc::new(swapped), None);
        assert_same_document(&warm, &cold, "kind swap");

        let typed = ParseConfig::default().with_todo_sequences([crate::TodoSequence::new(
            crate::TodoInterpretation::Type,
            ["TODO", "DONE"],
            1,
        )]);
        let (warm_typed, trees) =
            tree_builds::while_running(|| materialize(src, Arc::new(typed.clone()), Some(&prior)));
        assert_eq!(trees, 0);
        assert!(std::ptr::eq(
            prior.root().green().as_ref(),
            warm_typed.root().green().as_ref()
        ));
        assert_same_document(
            &warm_typed,
            &materialize(src, Arc::new(typed), None),
            "interpretation swap",
        );
    }

    #[test]
    fn two_damaged_headings_both_recarve() {
        let src = "* TODO a\n* DONE b\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let next_cfg = ParseConfig::default().with_todo_keywords([("BUG", TodoKind::Active)]);
        let (warm, trees) = tree_builds::while_running(|| {
            materialize(src, Arc::new(next_cfg.clone()), Some(&prior))
        });
        assert_eq!(trees, 0, "recarve is not a cold tree");
        assert_same_document(
            &warm,
            &materialize(src, Arc::new(next_cfg), None),
            "two damaged headings",
        );
        assert!(
            carved_todo_keywords(&warm).is_empty(),
            "{:?}",
            carved_todo_keywords(&warm)
        );
    }

    #[test]
    fn structural_delta_stays_cold_and_a_single_outline_edit_reuses() {
        let src = "* TODO a\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let structural = ParseConfig::default().with_max_nesting_depth(4);
        let (warm, trees) = tree_builds::while_running(|| {
            materialize(src, Arc::new(structural.clone()), Some(&prior))
        });
        assert_eq!(trees, 1, "structural-knob delta is cold");
        assert_same_document(
            &warm,
            &materialize(src, Arc::new(structural), None),
            "structural",
        );

        let (warm, trees) = tree_builds::while_running(|| {
            materialize("* other\n", Arc::new(ParseConfig::default()), Some(&prior))
        });
        assert_eq!(trees, 0, "one outline is reused, not cold-parsed");
        assert_eq!(warm.source(), "* other\n");
        assert_same_document(
            &warm,
            &materialize("* other\n", Arc::new(ParseConfig::default()), None),
            "outline text edit",
        );
    }

    #[test]
    fn recarved_star_line_diagnostics_match_cold() {
        let src = "* BUG [#zz] t\n#+begin_src rust\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let next_cfg = ParseConfig::default()
            .with_todo_keywords([("BUG", TodoKind::Active), ("FIXED", TodoKind::Done)]);
        let warm = materialize(src, Arc::new(next_cfg.clone()), Some(&prior));
        let cold = materialize(src, Arc::new(next_cfg), None);
        assert_same_document(&warm, &cold, "diagnostic merge");
        assert!(
            warm.diagnostics().iter().any(|d| {
                matches!(
                    d.kind(),
                    crate::DiagnosticKind::InvalidPriorityCookie { .. }
                )
            }),
            "invalid cookie on the recarved line"
        );
        let block = |d: &&crate::Diagnostic| {
            matches!(d.kind(), crate::DiagnosticKind::UnterminatedBlock { .. })
        };
        let prior_block = prior.diagnostics().iter().find(block).expect("block diag");
        let warm_block = warm
            .diagnostics()
            .iter()
            .find(block)
            .expect("kept block diag");
        assert_eq!(prior_block, warm_block, "off-line diagnostic is kept");
        assert!(
            prior.diagnostics().iter().all(|d| !matches!(
                d.kind(),
                crate::DiagnosticKind::InvalidPriorityCookie { .. }
            )),
            "fixture: the cookie diag appears only after BUG is a keyword"
        );
    }

    /// The cookie is already on the prior line and must leave with the keyword.
    #[test]
    fn recarve_drops_a_priority_cookie_diagnostic() {
        let src = "* TODO [#zz] t\n#+begin_src rust\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        assert!(
            prior.diagnostics().iter().any(|d| {
                matches!(
                    d.kind(),
                    crate::DiagnosticKind::InvalidPriorityCookie { .. }
                )
            }),
            "prior carves the invalid cookie after TODO"
        );
        let next = ParseConfig::default()
            .with_todo_keywords([("BUG", TodoKind::Active), ("FIXED", TodoKind::Done)]);
        let (warm, trees) =
            tree_builds::while_running(|| materialize(src, Arc::new(next.clone()), Some(&prior)));
        assert_eq!(
            trees, 0,
            "dropping the cookie is a recarve, not a cold parse"
        );
        let cold = materialize(src, Arc::new(next), None);
        assert_same_document(&warm, &cold, "cookie leaves, block diag stays");
        assert!(
            warm.diagnostics().iter().all(|d| !matches!(
                d.kind(),
                crate::DiagnosticKind::InvalidPriorityCookie { .. }
            )),
            "recarve must drop the cookie diagnostic"
        );
        let block = |d: &&crate::Diagnostic| {
            matches!(d.kind(), crate::DiagnosticKind::UnterminatedBlock { .. })
        };
        assert_eq!(
            prior.diagnostics().iter().find(block),
            warm.diagnostics().iter().find(block),
            "off-line block diagnostic is kept"
        );
    }

    /// Sticky clock diagnostics are not in start order. A recarve must not sort them.
    #[test]
    fn sticky_clock_diagnostics_keep_emit_order() {
        let src = "#+TODO: BUG | FIXED\n* TODO a\nCLOCK: [2020-01-01\n";
        let (parsed, trees) = tree_builds::while_running(|| parse(src));
        assert_eq!(trees, 1, "declaring open is one cold parse plus recarve");
        let cold = materialize(src, Arc::new(parsed.config().clone()), None);
        assert_eq!(parsed.diagnostics(), cold.diagnostics());
        assert!(
            matches!(
                parsed.diagnostics()[0].kind(),
                crate::DiagnosticKind::MalformedTimestamp
            ) && matches!(
                parsed.diagnostics()[1].kind(),
                crate::DiagnosticKind::MalformedClock
            ),
            "timestamp is emitted before the clock, though the clock range starts earlier: {:?}",
            parsed.diagnostics()
        );
    }

    /// A priority cookie glued to the following text is one `parse_todo` run.
    #[test]
    fn glued_priority_run_is_the_todo_candidate() {
        for keyword in ["[#A]GLUED", "[#10]X"] {
            let heading = format!("#+TODO: {keyword} | DONE\n* {keyword}\n");
            assert_glued_keyword_open(&heading, keyword);
            let stars = "*".repeat(15);
            let inline = format!("#+TODO: {keyword} | DONE\n{stars} {keyword}\n");
            assert_glued_keyword_open(&inline, keyword);
        }

        let spaced = "#+TODO: [#A]GLUED | DONE\n* [#A] GLUED\n";
        let doc = parse(spaced);
        let cold = materialize(spaced, Arc::new(doc.config().clone()), None);
        assert_eq!(doc.root().green(), cold.root().green());
        assert_eq!(doc.diagnostics(), cold.diagnostics());
        assert!(
            carved_todo_keywords(&doc).is_empty(),
            "space ends the run, so [#A] is not the keyword"
        );
        assert!(
            !nodes_of_kind(doc.root(), OrgSyntaxKind::PRIORITY).is_empty(),
            "spaced [#A] stays a priority cookie"
        );

        // `HEADING_COMMENT` is only carved when space or end-of-line follows,
        // so a glued `COMMENT` is the whole run, same as the cookie case.
        assert_glued_keyword_open(
            "#+TODO: COMMENTGLUED | DONE\n* COMMENTGLUED\n",
            "COMMENTGLUED",
        );
        let comment_spaced = "#+TODO: COMMENTGLUED | DONE\n* COMMENT stay\n";
        let commented = parse(comment_spaced);
        let cold = materialize(comment_spaced, Arc::new(commented.config().clone()), None);
        assert_eq!(commented.root().green(), cold.root().green());
        assert!(
            carved_todo_keywords(&commented).is_empty(),
            "{:?}",
            carved_todo_keywords(&commented)
        );
        assert!(
            !nodes_of_kind(commented.root(), OrgSyntaxKind::HEADING_COMMENT).is_empty(),
            "space after COMMENT keeps the comment marker"
        );
    }

    fn assert_glued_keyword_open(src: &str, keyword: &str) {
        let doc = parse(src);
        // Cold under the resolved vocabulary. `parse` keeps the caller layer,
        // so compare the tree and diagnostics, not the caller.
        let cold = materialize(src, Arc::new(doc.config().clone()), None);
        assert_eq!(doc.root().green(), cold.root().green(), "{src}");
        assert_eq!(doc.diagnostics(), cold.diagnostics(), "{src}");
        assert_eq!(
            carved_todo_keywords(&doc),
            vec![keyword.to_string()],
            "{src}"
        );
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PRIORITY).is_empty(),
            "glued cookie must not stay a PRIORITY: {src}"
        );
    }

    /// A name in both sets is not damaged, even when another name enters.
    #[test]
    fn a_word_in_both_name_sets_is_not_recarved() {
        let src = "* TODO stay\n* BUG move\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let next = ParseConfig::default().with_todo_keywords([
            ("TODO", TodoKind::Active),
            ("DONE", TodoKind::Done),
            ("BUG", TodoKind::Active),
        ]);
        let ((warm, carved), trees) = tree_builds::while_running(|| {
            crate::damage::recarves_while(|| materialize(src, Arc::new(next.clone()), Some(&prior)))
        });
        assert_eq!(trees, 0);
        assert_eq!(carved, 1, "BUG enters; TODO is in both sets");
        assert_same_document(
            &warm,
            &materialize(src, Arc::new(next), None),
            "shared name",
        );
        let prior_outlines = nodes_of_kind(prior.root(), OrgSyntaxKind::OUTLINE);
        let warm_outlines = nodes_of_kind(warm.root(), OrgSyntaxKind::OUTLINE);
        assert!(std::ptr::eq(
            prior_outlines[0].green().as_ref(),
            warm_outlines[0].green().as_ref()
        ));
        assert!(!std::ptr::eq(
            prior_outlines[1].green().as_ref(),
            warm_outlines[1].green().as_ref()
        ));
    }

    /// A structural knob plus a name-set change must not recarve under the old shape.
    #[test]
    fn structural_delta_with_a_name_set_change_stays_cold() {
        let src = "*** TODO x\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        assert_eq!(nodes_of_kind(prior.root(), OrgSyntaxKind::HEADING).len(), 1);
        assert!(
            nodes_of_kind(prior.root(), OrgSyntaxKind::INLINE_TASK).is_empty(),
            "{:?}",
            nodes_of_kind(prior.root(), OrgSyntaxKind::INLINE_TASK)
        );
        let next = ParseConfig::default()
            .with_inline_task_min_level(3)
            .with_todo_keywords([("BUG", TodoKind::Active), ("FIXED", TodoKind::Done)]);
        let (warm, trees) =
            tree_builds::while_running(|| materialize(src, Arc::new(next.clone()), Some(&prior)));
        assert_eq!(
            trees, 1,
            "structural delta is cold even when names also differ"
        );
        let cold = materialize(src, Arc::new(next), None);
        assert_same_document(&warm, &cold, "level and vocabulary");
        assert_eq!(
            nodes_of_kind(warm.root(), OrgSyntaxKind::INLINE_TASK).len(),
            1
        );
        assert!(
            nodes_of_kind(warm.root(), OrgSyntaxKind::HEADING).is_empty(),
            "{:?}",
            nodes_of_kind(warm.root(), OrgSyntaxKind::HEADING)
        );
    }

    #[test]
    fn names_outside_the_difference_reuse_the_whole_tree() {
        let src = "* PLAIN one\n";
        let prior = materialize(src, Arc::new(ParseConfig::default()), None);
        let grown = ParseConfig::default().with_todo_keywords([
            ("TODO", TodoKind::Active),
            ("NEXT", TodoKind::Active),
            ("DONE", TodoKind::Done),
        ]);
        let (warm, trees) =
            tree_builds::while_running(|| materialize(src, Arc::new(grown.clone()), Some(&prior)));
        assert_eq!(trees, 0);
        assert!(std::ptr::eq(
            prior.root().green().as_ref(),
            warm.root().green().as_ref()
        ));
        assert_same_document(
            &warm,
            &materialize(src, Arc::new(grown), None),
            "empty damaged set",
        );
    }

    /// The equation `apply == a fresh open of the spliced text`, over the whole
    /// corpus. It exists as the oracle the reuse paths are held to.
    ///
    /// The oracle is the open producer, not raw `materialize`: apply
    /// re-resolves in-buffer settings from the new text, so a raw materialize
    /// under the caller config would be a weaker check that still passes on
    /// every fixture without settings.
    ///
    /// Every fixture, so a construct added to `TAXONOMY` gets apply coverage
    /// without a second list. Insertion offsets are sampled per fixture: that
    /// is the axis to thin, because it takes fewer points from one document
    /// rather than dropping whole constructs.
    #[test]
    fn apply_equals_fresh_materialize() {
        /// Insertion points sampled per fixture, spread across the whole text.
        const SAMPLES: usize = 8;
        for &(src, cfg_tag) in TAXONOMY {
            let cfg = cfg_tag.build();
            let doc = parse_with(src, &cfg);
            let boundaries: Vec<usize> = (0..=src.len())
                .filter(|&o| src.is_char_boundary(o))
                .collect();
            let stride = boundaries.len().div_ceil(SAMPLES).max(1);
            // Strided samples plus the two structural extremes: prepend at 0
            // (always index 0) and append at len, which a stride can skip.
            let mut offsets: Vec<usize> = boundaries.iter().copied().step_by(stride).collect();
            if let Some(&last) = boundaries.last()
                && offsets.last() != Some(&last)
            {
                offsets.push(last);
            }
            for off in offsets {
                let set =
                    EditSet::single(src, TextRange::empty(TextSize::try_from(off).unwrap()), "x")
                        .expect("insert at char boundary is always valid");
                let want_text = set.apply_to_text(src);
                let want = open(&want_text, &cfg);
                let got = doc.apply(&set);
                assert_same_document(
                    &got,
                    &want,
                    "apply must equal cold materialize of spliced text",
                );
                assert_eq!(
                    got.root().text().to_string(),
                    got.source(),
                    "post-edit round-trip"
                );
            }
            if let Some(nl) = src.find('\n') {
                // Delete first line inclusive of its newline, then append a heading.
                // Both ranges are always in bounds when a newline exists.
                let end = TextSize::try_from(nl + 1).expect("newline offset fits TextSize");
                let set = EditSet::new(
                    src,
                    vec![
                        Edit {
                            range: TextRange::new(TextSize::from(0), end),
                            replacement: String::new(),
                        },
                        Edit {
                            range: TextRange::empty(
                                TextSize::try_from(src.len()).expect("len fits TextSize"),
                            ),
                            replacement: "\n* New\n".into(),
                        },
                    ],
                )
                .expect("first-line delete + append is non-overlapping and in bounds");
                let want_text = set.apply_to_text(src);
                let want = open(&want_text, &cfg);
                assert_same_document(&doc.apply(&set), &want, "multi-edit equation");
            }
        }
    }

    #[test]
    fn parse_returns_document() {
        let doc = parse("* a\n");
        assert_eq!(doc.source(), "* a\n");
        assert!(
            !nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING).is_empty(),
            "{:?}",
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
        );
    }

    #[test]
    #[should_panic(expected = "different text")]
    fn apply_panics_on_mismatched_edit_set() {
        // Valid against a longer string; OOB on the document.
        let doc = parse("ab");
        let set = EditSet::single(
            "abcdef",
            TextRange::new(TextSize::from(0), TextSize::from(4)),
            "x",
        )
        .unwrap();
        let _ = doc.apply(&set);
    }

    #[test]
    fn empty_edit_set_still_materializes() {
        let doc = parse("* a\n");
        let set = EditSet::new(doc.source(), vec![]).unwrap();
        // Same bytes, so the green tree is reused. Callers can skip apply
        // when the set is empty; this only pins the reuse.
        let (next, trees) = tree_builds::while_running(|| doc.apply(&set));
        assert_eq!(trees, 0, "empty apply must not build a second tree");
        assert!(std::ptr::eq(
            doc.root().green().as_ref(),
            next.root().green().as_ref()
        ));
        assert_same_document(&doc, &next, "empty apply equals re-materialize");
    }

    // -----------------------------------------------------------------------
    // In-buffer settings
    // -----------------------------------------------------------------------

    /// The correctness deliverable: a file that declares its own vocabulary is
    /// carved under it, and the default vocabulary is gone.
    #[test]
    fn file_vocabulary_carves_headlines() {
        let doc = parse("#+TODO: BUG | FIXED\n* BUG one\n* FIXED two\n* TODO three\n");
        assert_eq!(carved_todo_keywords(&doc), ["BUG", "FIXED"]);
        assert_eq!(doc.config().todo_kind("BUG"), Some(TodoKind::Active));
        assert_eq!(doc.config().todo_kind("TODO"), None);
        assert_eq!(doc.root().text().to_string(), doc.source(), "lossless");
    }

    /// Settings are document-level, not positional: a directive below the first
    /// headline still applies to the headlines above it.
    #[test]
    fn a_directive_after_the_first_headline_applies_document_wide() {
        let doc = parse("* BUG one\n#+TODO: BUG | FIXED\n* BUG two\n");
        assert_eq!(carved_todo_keywords(&doc), ["BUG", "BUG"]);
    }

    /// The caller layer stands in for what the file does not say, and is
    /// replaced wholesale for what it does.
    #[test]
    fn the_file_layer_replaces_the_caller_vocabulary() {
        let caller = ParseConfig::default()
            .with_todo_keywords([("NEXT", TodoKind::Active), ("CANCELLED", TodoKind::Done)]);

        let silent = parse_with("* NEXT one\n", &caller);
        assert_eq!(carved_todo_keywords(&silent), ["NEXT"], "caller stands");

        let declared = parse_with("#+TODO: BUG | FIXED\n* NEXT one\n* BUG two\n", &caller);
        assert_eq!(carved_todo_keywords(&declared), ["BUG"], "file replaces");
        assert_eq!(*declared.caller, caller, "provenance retained");
    }

    /// Setting nodes are in the tree because of losslessness, not because of
    /// any discovery mode: the directive that carved is still a
    /// `SETTING_KEYWORD` sibling, and the source round-trips.
    #[test]
    fn a_directive_that_carves_is_still_a_node_in_the_tree() {
        let src = "#+TODO: BUG | FIXED\n* BUG one\n* TODO two\n";
        let doc = parse(src);

        assert_eq!(carved_todo_keywords(&doc), ["BUG"], "file vocabulary");
        assert_eq!(doc.config().todo_kind("TODO"), None);
        assert_eq!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::SETTING_KEYWORD).len(),
            1,
            "the directive is still in the tree"
        );
        assert_eq!(doc.root().text().to_string(), src);
    }

    /// Nothing to resolve means nothing to re-materialize: the collection parse
    /// is the result, and both layers are the same allocation. Pointer identity
    /// is the observable proxy for "the second parse did not happen".
    #[test]
    fn an_open_that_resolves_to_its_caller_skips_the_refresh() {
        for src in [
            "* TODO plain\n",                                        // no settings at all
            "#+TODO: TODO | DONE\n#+PRIORITIES: A C B\n",            // restates the defaults
            "#+PRIORITIES: 1 9 5\n",                                 // projection, not a knob
            "#+TODO:\n#+TODO: |\n",                                  // contributes nothing
            "#+begin_example\n#+TODO: BUG | FIXED\n#+end_example\n", // inert
        ] {
            let doc = parse(src);
            assert!(
                Arc::ptr_eq(&doc.config, &doc.caller),
                "{src:?} must not trigger a config refresh"
            );
            assert_eq!(doc.config(), &ParseConfig::default());
        }

        let refreshed = parse("#+TODO: BUG | FIXED\n");
        assert!(!Arc::ptr_eq(&refreshed.config, &refreshed.caller));
        assert_ne!(*refreshed.config, *refreshed.caller);
    }

    /// Whatever path produced the snapshot, its tree and diagnostics are those
    /// of a cold parse under its effective config.
    #[test]
    fn every_producer_matches_a_cold_parse_under_its_effective_config() {
        const SOURCES: &[&str] = &[
            "#+TODO: BUG | FIXED\n* BUG one\n** TODO two\n",
            "* BUG one\n#+SEQ_TODO: BUG(b) | FIXED(f)\n",
            "#+TYP_TODO: Fred Sara | DONE\n* Fred task\n",
            "#+TODO: A | B\n#+TODO: C | D\n* C x\n",
            "#+PRIORITIES: 1 9 5\n* TODO [#5] p\n",
            "#+begin_quote\n#+TODO: BUG | FIXED\n#+end_quote\n* BUG q\n",
        ];
        for src in SOURCES {
            let doc = parse(src);
            let cold = materialize(*src, Arc::new(doc.config().clone()), None);
            assert_eq!(doc.root().green(), cold.root().green(), "tree: {src:?}");
            assert_eq!(doc.diagnostics(), cold.diagnostics(), "diags: {src:?}");

            // And again after an edit, whose result is a successor of `doc`.
            let set = EditSet::single(src, TextRange::empty(TextSize::from(0)), "\n")
                .expect("insert at 0");
            let edited = doc.apply(&set);
            let cold_edited = materialize(
                set.apply_to_text(src),
                Arc::new(edited.config().clone()),
                None,
            );
            assert_eq!(edited.root().green(), cold_edited.root().green(), "{src:?}");
            assert_eq!(edited.diagnostics(), cold_edited.diagnostics(), "{src:?}");
        }
    }

    /// Editing a directive re-resolves inside the same apply: stale config is a
    /// bug, in both directions.
    #[test]
    fn apply_re_resolves_the_vocabulary_from_the_new_text() {
        let doc = parse("* BUG one\n");
        assert!(
            carved_todo_keywords(&doc).is_empty(),
            "{:?}",
            carved_todo_keywords(&doc)
        );

        let directive = "#+TODO: BUG | FIXED\n";
        let add = EditSet::single(doc.source(), TextRange::empty(TextSize::from(0)), directive)
            .expect("prepend");
        let with_vocabulary = doc.apply(&add);
        assert_eq!(carved_todo_keywords(&with_vocabulary), ["BUG"]);
        assert_eq!(
            with_vocabulary.config().todo_kind("FIXED"),
            Some(TodoKind::Done)
        );

        let end = TextSize::try_from(directive.len()).expect("fits");
        let remove = EditSet::single(
            with_vocabulary.source(),
            TextRange::new(TextSize::from(0), end),
            "",
        )
        .expect("delete the directive line");
        let without = with_vocabulary.apply(&remove);
        assert!(carved_todo_keywords(&without).is_empty(), "back to caller");
        assert_eq!(without.config, without.caller);
    }

    /// Apply resolves over the retained caller layer, not over the previous
    /// effective config: file settings must not accumulate across edits.
    #[test]
    fn apply_resolves_over_the_caller_layer_not_the_previous_effective_one() {
        let caller = ParseConfig::default().with_todo_keywords([("NEXT", TodoKind::Active)]);
        let doc = parse_with("#+TODO: BUG | FIXED\n* NEXT one\n", &caller);
        assert!(carved_todo_keywords(&doc).is_empty(), "file replaced NEXT");

        // Drop the directive; the caller's NEXT must come back, and BUG go.
        let end = TextSize::try_from("#+TODO: BUG | FIXED\n".len()).expect("fits");
        let set = EditSet::single(doc.source(), TextRange::new(TextSize::from(0), end), "")
            .expect("delete the directive line");
        let next = doc.apply(&set);

        assert_eq!(carved_todo_keywords(&next), ["NEXT"]);
        assert_eq!(next.config(), &caller);
        assert_eq!(*next.caller, caller, "caller survives apply");
    }

    /// What each producer costs, in cold trees.
    ///
    /// Same-bytes file-layer reuse does not build a second tree. A vocabulary
    /// move is one cold parse of the new text plus a recarve. A same-bytes
    /// apply builds none.
    #[test]
    fn producers_build_one_cold_tree() {
        let vocabulary = "#+TODO: BUG | FIXED\n";
        let plain = "* TODO one\n* BUG two\n";
        let declared = format!("{vocabulary}{plain}");

        // One cold tree whether or not the file moves the vocabulary.
        for (src, want, why) in [
            (plain, 1, "declares nothing"),
            (
                "#+TODO: TODO | DONE\n",
                1,
                "restates the caller's vocabulary",
            ),
            ("#+TODO: |\n", 1, "declares nothing that contributes"),
            // A priority range is read off the tree, not a parse input, so
            // declaring one cannot move the effective config or cost a
            // refresh parse.
            (
                "#+PRIORITIES: 1 9 5\n* TODO [#5] p\n",
                1,
                "declares a priority range, which never carves",
            ),
            (
                declared.as_str(),
                1,
                "moves the config: collect, then recarve (not a second cold parse)",
            ),
        ] {
            let (_, trees) = tree_builds::while_running(|| parse(src));
            assert_eq!(trees, want, "open that {why}");
        }

        // Re-opening under the config a previous open resolved costs one tree:
        // the guess is already right, so there is nothing to refresh.
        let doc = parse(&declared);
        let (reopened, trees) = tree_builds::while_running(|| parse_with(&declared, doc.config()));
        assert_eq!(trees, 1, "re-open under the effective config");
        assert_eq!(reopened.config(), doc.config());

        // Appending a top-level headline has no ancestor outline to reuse, so
        // the text hop stays cold even though the effective config does not move.
        let body_edit = |doc: &Document| {
            let at = TextSize::try_from(doc.source().len()).expect("fits");
            EditSet::single(doc.source(), TextRange::empty(at), "* BUG three\n")
                .expect("append a headline")
        };
        for doc in [&doc, &parse(plain)] {
            let (next, trees) = tree_builds::while_running(|| doc.apply(&body_edit(doc)));
            assert_eq!(trees, 1, "apply that leaves the config alone");
            assert_eq!(next.config(), doc.config());
        }

        // Prepending the directive adds a zeroth section, a new root child, so
        // the text hop stays cold. The refresh hop recarves.
        let undeclared = parse(plain);
        let declaring = EditSet::single(plain, TextRange::empty(TextSize::from(0)), vocabulary)
            .expect("prepend the directive");
        let (declared_doc, trees) = tree_builds::while_running(|| undeclared.apply(&declaring));
        assert_eq!(
            trees, 1,
            "apply that declares a vocabulary: one cold parse, then recarve"
        );
        assert_eq!(carved_todo_keywords(&declared_doc), ["BUG"]);
    }

    /// Headings outside the symmetric difference are not recarved, and a
    /// declaring open recarves only the lines whose word changed membership.
    #[test]
    fn declaring_open_recarves_damaged_star_lines_only() {
        let untouched = "#+TODO: BUG | FIXED\n* plain one\n* also two\n";
        let ((), carved) = crate::damage::recarves_while(|| {
            let (_, trees) = tree_builds::while_running(|| parse(untouched));
            assert_eq!(trees, 1, "name-set change with nothing to recarve");
        });
        assert_eq!(
            carved, 0,
            "no heading word lies in the symmetric difference"
        );

        let mixed = "#+TODO: BUG | FIXED\n* TODO one\n* BUG two\n* plain three\n";
        let ((doc, carved), trees) =
            tree_builds::while_running(|| crate::damage::recarves_while(|| parse(mixed)));
        assert_eq!(trees, 1, "declaring open is one cold tree");
        assert_eq!(carved, 2, "TODO and BUG move; plain does not");
        assert_eq!(carved_todo_keywords(&doc), ["BUG"]);
    }

    #[test]
    fn from_parts_is_not_a_materialize() {
        let doc = parse("CLOCK: not-a-stamp\n* a\n");
        assert!(
            !doc.diagnostics().is_empty(),
            "fixture must carry a sticky diagnostic so reconstruct cannot drop the vec unnoticed"
        );
        let (copy, trees) = tree_builds::while_running(|| Document::from_parts(doc.parts()));
        assert_eq!(trees, 0);
        assert_same_document(&doc, &copy, "from_parts");
    }

    #[test]
    fn portable_hop_then_apply_resolves_over_the_restored_caller() {
        let caller = ParseConfig::default().with_todo_keywords([("NEXT", TodoKind::Active)]);
        let src = "#+TODO: BUG | FIXED\n* NEXT one\n";
        let original = parse_with(src, &caller);
        assert!(
            carved_todo_keywords(&original).is_empty(),
            "file replaced NEXT"
        );

        let portable: PortableDocument = original.to_portable();
        // Document is !Send, so rebuild and apply on the worker.
        std::thread::spawn(move || {
            let rebuilt = portable.into_document();
            let empty = EditSet::new(rebuilt.source(), vec![]).expect("empty set");
            let after_empty = rebuilt.apply(&empty);
            assert_eq!(after_empty.config(), rebuilt.config());

            let end = TextSize::try_from("#+TODO: BUG | FIXED\n".len()).expect("fits");
            let drop =
                EditSet::single(rebuilt.source(), TextRange::new(TextSize::from(0), end), "")
                    .expect("delete the directive line");
            let next = rebuilt.apply(&drop);
            assert_eq!(carved_todo_keywords(&next), ["NEXT"]);
            assert_eq!(next.config(), &caller);
            assert_eq!(*next.caller, caller, "caller survives hop then apply");
        })
        .join()
        .expect("thread");
    }
}
