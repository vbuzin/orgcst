//! Keyword grammar: one uniform recogniser for every `#+KEY: value` line.
//!
//! Covers plain keywords (`#+TITLE:`, ...), setting directives (`#+TODO:`,
//! `#+PRIORITIES:`), and `#+CALL:` (emitted as BABEL_CALL with structured
//! call-value children; see [`super::call`]).
//!
//! Emits one of:
//! ```text
//! KEYWORD | SETTING_KEYWORD | BABEL_CALL
//!   [INDENT] # + KEYWORD_KEY [KEYWORD_DUAL] :
//!   [WHITESPACE]
//!   KEYWORD_VALUE          // non-babel (caption may hold inlines)
//!     | CALL_* children    // babel only; no KEYWORD_VALUE wrapper
//!   [NEWLINE]
//!   [post-blank tokens]
//! ```
//!
//! Shape notes:
//! - Leading INDENT allowed.
//! - [`probe`] reads one line. The key runs to the last `:` of the
//!   non-whitespace run (org `#+\(\S-*\):`). Caseless `call` keeps that key
//!   when the first colon is the one right after `call`
//!   (`#+call:name[inside]: extra`). A bracket is key material, never a
//!   rejection: `#+FOO[bar]baz:` and `#+FOO[bar:` are keywords whose key
//!   covers the whole run.
//! - An orphan keyword (one whose affiliated run does not attach) keeps that
//!   shape. An attaching run narrows each
//!   member to the `org-element--affiliated-re` key at emit
//!   ([`KeywordShape::with_key_extent`]), so `#+NAME:fig:1` before a
//!   paragraph is key `NAME` and the same line before a blank is `NAME:fig`.
//! - The `[dual]` slot is carved out of the probed run when it ends `]` right
//!   before the `:` (greedy to the last `]`, as org's `\[.*\]`). It is carved
//!   for **every** name; whether the slot carries dual *semantics* is a
//!   name-table question the view answers, not a grammar one.
//! - Dual content parsed as inlines only for caption.
//! - Dual on CALL is key-side KEYWORD_DUAL, not value-side CALL_INSIDE_HEADER.
//!   Elisp `CALL:` group does not allow dual; the CST accepts it so every
//!   keyword line has the same lossless shape (a deliberate divergence).
//! - `#+begin:...` has keyword shape but blocks are tried first in the dispatcher.
//!
//! Contract (element probe/parse):
//! - [`probe`] is a function of one line. It carries the orphan key, the dual
//!   extent, the classification of that key, and the paragraph-interrupt bit.
//!   It does not read later lines. [`parse`] consumes those counts.
//! - Babel values always tile as BABEL_CALL once the key is `call`.

use crate::{
    parser::{LineView, Parser, Pos},
    syntax::OrgSyntaxKind,
};

use super::{affiliated, call};
use crate::parser::grammar::drive::{atoms, region};

// ---------------------------------------------------------------------------
// Public surface for the element dispatcher
// ---------------------------------------------------------------------------

/// Probe measurement for one keyword line: `#+KEY:` or `#+KEY[dual]:`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KeywordShape {
    /// Token index of the first key-run token (after `#+`).
    key_from: usize,
    /// Number of tokens in the base key run (supports multi-token keys such
    /// as those containing `_` which the lexer splits). A run that opens with
    /// `[` keeps the whole run as its key (no dual carve), so this is never
    /// zero for a successful probe.
    key_tokens: usize,
    /// Tokens in the trailing `[dual]` slot including both brackets; `0` when
    /// the run carries no bracket slot.
    dual_tokens: usize,
    /// Caseless classification of the key (top kind + inline treatment).
    class: KeywordClass,
    /// Whether this line ends a paragraph. False when `#+(\S-+)[...]:`
    /// matches and that written spelling is not `CAPTION` or `RESULTS`
    /// (caseless). `RESULT` is an affiliation alias and does not end a
    /// paragraph. The name may contain colons (`#+FOO:bar[baz]:`).
    pub(crate) interrupts: bool,
}

impl KeywordShape {
    /// Whether the probed run carved a `[dual]` slot.
    #[cfg(test)]
    #[inline]
    fn has_dual(self) -> bool {
        self.dual_tokens > 0
    }

    /// Replace the orphan key with the affiliated-regexp key and reclassify.
    ///
    /// `key_tokens` / `dual_tokens` come from [`affiliated_extent`] on this
    /// same line. Reclassification is the point: `#+CAPTION:a: b` probes as
    /// key `CAPTION:a` (plain) and emits as `CAPTION` (inlines) only after
    /// the run has attached. Orphans keep the probed key and never call this.
    pub(crate) fn with_key_extent(
        self,
        line: LineView,
        key_tokens: usize,
        dual_tokens: usize,
    ) -> Self {
        debug_assert!(key_tokens > 0, "affiliated key is non-empty");
        Self {
            key_tokens,
            dual_tokens,
            class: KeywordClass::classify(
                line.text_range(self.key_from, self.key_from + key_tokens),
            ),
            ..self
        }
    }
}

/// Recognise a keyword line and classify its key.
///
/// Shape at line content: `#+` RUN `:` where RUN is measured by
/// [`measure_key_run`]; [`measure_dual`] then splits a trailing `[...]` slot
/// out of it.
pub(crate) fn probe(line: LineView) -> Option<KeywordShape> {
    let key_from = atoms::line_keyword_prefix(line)?;

    let (run, first_colon) = measure_key_run(line, key_from)?;
    let before_first = line.text_range(key_from, key_from + first_colon);
    // `CALL:` is the colon immediately after caseless `call`. A later `[...]`
    // is value (`#+call:name[inside]: extra`), not a reason to drop the key.
    // `#+call[inside]:` is not this group: the text before the first colon is
    // longer than `call`.
    let (key_tokens, dual_tokens) = if before_first.eq_ignore_ascii_case("call") {
        (first_colon, 0)
    } else {
        let dual_tokens = measure_dual(line, key_from, run);
        (run - dual_tokens, dual_tokens)
    };
    debug_assert!(
        key_tokens > 0,
        "keyword probe always leaves a non-empty base key"
    );
    Some(KeywordShape {
        key_from,
        key_tokens,
        dual_tokens,
        class: KeywordClass::classify(line.text_range(key_from, key_from + key_tokens)),
        interrupts: bracketed_keyword_interrupts(line, key_from),
    })
}

/// Whether this keyword line ends a paragraph.
///
/// `org-element-paragraph-parser` keeps walking when
/// `#\+(\S-+)\[.*\]:` matches and the captured name is not the written
/// spelling `CAPTION` or `RESULTS`. A nil `member` does not fall through
/// to the unambiguous arm, so `RESULT` stays inside the paragraph. The
/// translation alist is affiliation, not this check.
///
/// The name is the text before the rightmost `[` in the leading
/// non-whitespace run. `\S-+` cannot cross a space, and `\[.*\]:` is
/// greedy, so `#+CAPTION[a b][c]:` is name `CAPTION` and
/// `#+CAPTION[a][b]:` is name `CAPTION[a]`. A colon before the bracket
/// is part of that name (`#+FOO:bar[baz]:`). No paired-bracket scan:
/// `find_paired` closes `[a b]` before the later `]:`.
fn bracketed_keyword_interrupts(line: LineView, key_from: usize) -> bool {
    let mut k = key_from;
    let mut bracket_at = None;
    loop {
        let kind = line.kind(k);
        if is_keyword_key_terminator(kind) {
            break;
        }
        if kind == OrgSyntaxKind::L_BRACKET && k > key_from {
            bracket_at = Some(k);
        }
        k += 1;
    }
    let Some(at) = bracket_at else {
        return true;
    };
    let mut j = at + 1;
    loop {
        match line.kind(j) {
            OrgSyntaxKind::NEWLINE | OrgSyntaxKind::EOF => return true,
            OrgSyntaxKind::R_BRACKET if line.kind(j + 1) == OrgSyntaxKind::COLON => break,
            _ => j += 1,
        }
    }
    let name = line.text_range(key_from, at);
    name.eq_ignore_ascii_case("CAPTION") || name.eq_ignore_ascii_case("RESULTS")
}

/// Measure the key run from `key_from` to the last `:`.
///
/// Returns `(tokens before the last colon, tokens before the first colon)`.
/// The run is org's `\S-*`: non-whitespace tokens of any kind, so no bracket
/// shape can reject the line (`#+FOO[bar]baz:` and `#+FOO[bar:` are keywords
/// whose key is the whole run). Colons inside that run are key material; the
/// terminator is the last one (`#+A:B:C: v` has key `A:B:C`). One dialect
/// superset: a `[...]` pair that closes on this line is skipped whole, so its
/// interior may hold whitespace (`#+CAPTION[short cap]: v`). Org's element
/// regexp stops at the space and only its *affiliated* regexp accepts the
/// spaced OPTVAL; the CST keeps one keyword shell for both, which is what
/// lets such a line affiliate. An orphaned spaced line stays a keyword by
/// design, where Org reads a paragraph: an orphaned caption is a plain
/// keyword either way (`#+CAPTION: v` is one in Org too), and caption meaning
/// comes only from attachment.
///
/// `None` when the run is empty or does not end at a `:` on this line.
fn measure_key_run(line: LineView, key_from: usize) -> Option<(usize, usize)> {
    let mut k = key_from;
    let mut first_colon = None;
    let mut last_colon = None;
    loop {
        match line.kind(k) {
            OrgSyntaxKind::COLON => {
                if first_colon.is_none() {
                    first_colon = Some(k);
                }
                last_colon = Some(k);
                k += 1;
            }
            // Unclosed `[` is ordinary key material, not a rejection.
            OrgSyntaxKind::L_BRACKET => {
                k = match line.find_paired(k, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET) {
                    Some(closer) => closer + 1,
                    None => k + 1,
                }
            }
            kind if is_keyword_key_terminator(kind) => break,
            _ => k += 1,
        }
    }
    let last = last_colon?;
    let first = first_colon?;
    (last > key_from).then_some((last - key_from, first - key_from))
}

/// Split the `[dual]` slot out of a measured key run.
///
/// The slot exists when the run ends `]` immediately before the `:` and a `[`
/// opens after at least one key token; it then spans the **first** such `[` to
/// that final `]`, matching org's greedy `\[.*\]` (so `[a][b]` is one slot,
/// interior `a][b`). A run that opens with `[` keeps the whole run as its key,
/// which is org's flattened `:key` for that shape. Returns the slot's token
/// count, `0` when there is none.
fn measure_dual(line: LineView, key_from: usize, run: usize) -> usize {
    if line.kind(key_from + run - 1) != OrgSyntaxKind::R_BRACKET {
        return 0;
    }
    (1..run - 1)
        .find(|&i| line.kind(key_from + i) == OrgSyntaxKind::L_BRACKET)
        .map_or(0, |i| run - i)
}

/// `(key tokens, dual tokens)` for the colon `org-element--affiliated-re`
/// matches: after the name, or after a dual `[...]`. `None` when that name
/// is not an affiliated key. Attachment is [`affiliated::try_run`], not this
/// extent: an orphan still matches the regexp and emits [`probe`]'s key.
fn affiliated_extent_at(
    line: LineView,
    key_from: usize,
    first_colon: usize,
) -> Option<(usize, usize)> {
    let prefix_dual = measure_dual(line, key_from, first_colon);
    let name = line.text_range(key_from, key_from + first_colon - prefix_dual);
    if affiliated::is_affiliated_key(name) && (prefix_dual == 0 || affiliated::is_dual_key(name)) {
        Some((first_colon - prefix_dual, prefix_dual))
    } else {
        None
    }
}

/// [`affiliated_extent_at`] for a whole line. `None` when the line is not an
/// affiliated-keyword pattern, including an orphan whose emitted key is longer.
pub(crate) fn affiliated_extent(line: LineView) -> Option<(usize, usize)> {
    let key_from = atoms::line_keyword_prefix(line)?;
    let (_, first_colon) = measure_key_run(line, key_from)?;
    affiliated_extent_at(line, key_from, first_colon)
}

/// Source text of the base key (the run minus any `[dual]` slot).
#[cfg(test)]
fn shape_key<'s>(line: LineView<'_, 's>, shape: KeywordShape) -> &'s str {
    line.text_range(shape.key_from, shape.key_from + shape.key_tokens)
}

/// Parse one keyword line with its probe measurement.
///
/// Emits KEYWORD, SETTING_KEYWORD, or BABEL_CALL. Non-babel lines decompose to
/// KEY / optional DUAL / VALUE. Babel lines decompose to KEY / optional DUAL /
/// CALL_* value children (no KEYWORD_VALUE). Advances past the line and its
/// post-blank.
pub(crate) fn parse(parser: &mut Parser, shape: KeywordShape) {
    let top = parser.start();
    parse_interior(parser, shape);
    // Trailing blanks belong to the keyword element - except for `#+TBLFM:`
    // lines inside a TABLE ([`parse_line`]), whose caller collects
    // directly-consecutive lines only (a blank stops the formula run).
    parser.eat_blank_lines();
    top.complete(parser, shape.class.syntax_kind());
}

/// Parse one keyword line without claiming trailing blanks.
///
/// The TABLE grammar attaches `#+TBLFM:` lines with this so a blank line
/// stops the formula run (org `#+TBLFM:` collection is line-consecutive).
pub(crate) fn parse_line(parser: &mut Parser, shape: KeywordShape) {
    let top = parser.start();
    parse_interior(parser, shape);
    top.complete(parser, shape.class.syntax_kind());
}

/// Shared line interior: prefix, key, dual, colon, value, terminating NL.
fn parse_interior(parser: &mut Parser, shape: KeywordShape) {
    // Optional leading INDENT (significant for structure).
    parser.eat(OrgSyntaxKind::INDENT);

    // #+ prefix (HASH PLUS).
    parser.eat(OrgSyntaxKind::HASH);
    parser.eat(OrgSyntaxKind::PLUS);

    // KEYWORD_KEY: consume exactly the tokens measured by the probe.
    parser.leaf(OrgSyntaxKind::KEYWORD_KEY, shape.key_tokens);

    // Optional dual slot (key-side). CAPTION gets inlines; others stay flat.
    parse_optional_dual(parser, shape.dual_tokens, shape.class.parses_inlines());

    // Colon (required by shape).
    parser.eat(OrgSyntaxKind::COLON);
    // Optional separator whitespace after `:`.
    parser.eat(OrgSyntaxKind::WHITESPACE);

    if shape.class == KeywordClass::Babel {
        // Structured call value: CALL_NAME / headers / args (no KEYWORD_VALUE).
        call::parse_babel_call_value(parser);
    } else {
        parse_keyword_value(parser, shape.class.parses_inlines());
    }

    // Terminating NEWLINE (if visible).
    parser.eat(OrgSyntaxKind::NEWLINE);
}

// ---------------------------------------------------------------------------
// Value (non-babel)
// ---------------------------------------------------------------------------

/// Emit KEYWORD_VALUE covering the rest of the line (flat or caption inlines).
fn parse_keyword_value(parser: &mut Parser, inline: bool) {
    let val_m = parser.start();
    // Only CAPTION values are parsed as inline objects (`inline == true`).
    parser.with_rest_of_line(|p| consume_region(p, inline));
    val_m.complete(parser, OrgSyntaxKind::KEYWORD_VALUE);
}

/// Token predicate: ends the non-whitespace scan of a keyword key.
///
/// `COLON` is not a terminator. The key includes every colon but the last
/// ([`measure_key_run`]). `EOF` is included so a probe past the line stops.
/// `L_BRACKET` is not a terminator either: org's key pattern is `\S-*`, so a
/// bracket is key material and the optional dual slot is split out afterwards
/// ([`measure_dual`]).
#[inline]
fn is_keyword_key_terminator(k: OrgSyntaxKind) -> bool {
    matches!(
        k,
        OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::NEWLINE | OrgSyntaxKind::EOF
    )
}

/// Consume the visible region as flat tokens or inline objects.
///
/// `inline == true` only for CAPTION (the sole keyword whose *value* is
/// parsed to objects per org-syntax).
fn consume_region(parser: &mut Parser, inline: bool) {
    if inline {
        region::inline_to_limit(parser, OrgSyntaxKind::KEYWORD_VALUE);
    } else {
        parser.eat_to_limit();
    }
}

/// Emit the measured `[...]` dual slot, if the probe carved one.
///
/// `dual_tokens` covers both brackets, so the closer sits at
/// `cursor + dual_tokens - 1`. The extent comes from the probe, never from a
/// second balanced scan: org's `\[.*\]` is greedy, so `[a][b]` is one slot
/// while a balanced scan would stop at the first `]`. `inline` expands the
/// interior as objects (only true for CAPTION).
fn parse_optional_dual(parser: &mut Parser, dual_tokens: usize, inline: bool) {
    if dual_tokens == 0 {
        return;
    }
    debug_assert!(
        parser.current() == OrgSyntaxKind::L_BRACKET,
        "measured dual slot must start at `[`"
    );
    let closer = Pos::from_index(parser.cursor().index() + dual_tokens - 1);
    let m = parser.start();
    parser.eat(OrgSyntaxKind::L_BRACKET);
    // After eating `[`, the dual interior is a pure measured region.
    if inline {
        region::inline_in(parser, closer, OrgSyntaxKind::KEYWORD_DUAL);
    } else {
        parser.eat_until(closer);
    }
    parser.eat(OrgSyntaxKind::R_BRACKET);
    m.complete(parser, OrgSyntaxKind::KEYWORD_DUAL);
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Keys that carve `SETTING_KEYWORD`: the in-buffer settings this crate reads
/// back from the document (TODO keyword sets and priorities).
///
/// Other settings such as `#+STARTUP:` and `#+COLUMNS:` stay ordinary
/// `KEYWORD` nodes.
const CARVING_KEYS: &[&str] = &["todo", "seq_todo", "typ_todo", "priorities"];

/// Whether `key` is one of the four names that carve `SETTING_KEYWORD`.
///
/// ASCII caseless.
#[inline]
pub(crate) fn is_carving_key(key: &str) -> bool {
    CARVING_KEYS
        .iter()
        .any(|name| key.eq_ignore_ascii_case(name))
}

/// Single caseless classification of a keyword base name (source case preserved).
///
/// Folds top-kind selection and CAPTION inline treatment into one pass so parse
/// does not re-scan the key. Caption still emits top-kind [`OrgSyntaxKind::KEYWORD`]
/// (no separate CAPTION syntax kind). Affiliated-candidate names uniformly become
/// Plain / `KEYWORD` (no name-based `AFFILIATED_KEYWORD` emission).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordClass {
    Babel,
    Setting,
    Caption,
    Plain,
}

impl KeywordClass {
    /// Caseless; the carving names live in [`CARVING_KEYS`].
    #[inline]
    fn classify(key: &str) -> Self {
        match key {
            k if k.eq_ignore_ascii_case("call") => Self::Babel,
            k if is_carving_key(k) => Self::Setting,
            // Inline treatment is org's parsed-keywords table (CAPTION only),
            // read from the one affiliated name table.
            k if affiliated::is_parsed_key(k) => Self::Caption,
            _ => Self::Plain,
        }
    }

    /// Top CST node kind. Caption maps to KEYWORD (not a distinct syntax kind).
    #[inline]
    fn syntax_kind(self) -> OrgSyntaxKind {
        match self {
            Self::Babel => OrgSyntaxKind::BABEL_CALL,
            Self::Setting => OrgSyntaxKind::SETTING_KEYWORD,
            Self::Caption | Self::Plain => OrgSyntaxKind::KEYWORD,
        }
    }

    /// Whether dual and value interiors are expanded as inline objects.
    ///
    /// True only for Caption: both the optional `[dual]` and the value after `:`
    /// parse as inline objects. All other classes keep dual/value flat.
    #[inline]
    fn parses_inlines(self) -> bool {
        matches!(self, Self::Caption)
    }
}

#[cfg(test)]
mod tests {
    use super::KeywordClass;
    use crate::parser::grammar::forms::keyword::{
        is_carving_key, is_keyword_key_terminator, parse, probe, shape_key,
    };
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        ParseResult, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, probe_line, run_element, tree_text,
    };

    // ---------------------------------------------------------------------
    // probe contract test (recognition + measurement in one pass).
    // ---------------------------------------------------------------------

    /// Key terminators for post-run probes; key scan itself uses `run_while`.
    #[test]
    fn keyword_key_terminator_includes_eof() {
        assert!(
            is_keyword_key_terminator(OrgSyntaxKind::EOF),
            "EOF must act as a post-run key terminator"
        );
        for k in [OrgSyntaxKind::WHITESPACE, OrgSyntaxKind::NEWLINE] {
            assert!(is_keyword_key_terminator(k), "{k:?} terminates the key run");
        }
        for k in [
            OrgSyntaxKind::TEXT,
            OrgSyntaxKind::UNDERSCORE,
            // A colon is key material until the last one in the run.
            OrgSyntaxKind::COLON,
            // A bracket is key material: the dual slot is split out of the
            // measured run, never a reason to reject the line.
            OrgSyntaxKind::L_BRACKET,
            OrgSyntaxKind::R_BRACKET,
        ] {
            assert!(!is_keyword_key_terminator(k), "{k:?} is key material");
        }
        // Shape fail (no colon): probe returns None without hanging.
        let p = make_parser("#+NOCOLON");
        assert!(p.line().is_some_and(|l| probe(l).is_none()));
    }

    #[test]
    fn probe_accepts_non_text_key_start() {
        // Any non-terminator may start the key after `#+`.
        for src in ["#+_FOO: v\n", "#++BAR: v\n", "#+*STAR: v\n"] {
            let s = probe_line(src, probe).expect(src);
            assert!(s.key_tokens >= 1, "{src}");
            let p = make_parser(src);
            let line = p.line().expect("line");
            assert!(!shape_key(line, s).is_empty(), "{src}");
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(),
                1,
                "non-TEXT key start must be KEYWORD for {src}"
            );
        }
    }

    #[test]
    fn probe_does_not_advance_position() {
        let src = "   #+TITLE: hi\nnext\n";
        let parser = make_parser(src);
        let before = parser.cursor().index();
        assert!(parser.line().is_some_and(|l| probe(l).is_some()));
        assert_eq!(parser.cursor().index(), before);
        assert_eq!(parser.current(), OrgSyntaxKind::INDENT);

        let p2 = make_parser("* headline\n");
        let b2 = p2.cursor().index();
        assert!(p2.line().is_some_and(|l| probe(l).is_none()));
        assert_eq!(p2.cursor().index(), b2);
    }

    // ---------------------------------------------------------------------
    // Direct coverage for the canonical line probe (recognition + shape).
    // ---------------------------------------------------------------------

    #[test]
    fn probe_measures_key_and_class() {
        let s = probe_line("#+TITLE: hi\n", probe).expect("shape");
        assert_eq!(s.key_tokens, 1);
        assert_eq!(s.class, KeywordClass::Plain);

        // Probe works on any line view, not only the first line.
        let p2 = make_parser("para\n  #+FOO: bar\n");
        let line1 = p2.line().expect("line 0").next().expect("line 1");
        assert!(probe(line1).is_some());

        // Key text is measured in place (case preserved).
        let p3 = make_parser("#+ATTR_LATEX: x\n");
        let line = p3.line().expect("line");
        let shape = probe(line).expect("attr");
        assert!(shape.key_tokens > 1, "expected split key tokens");
        assert_eq!(shape_key(line, shape), "ATTR_LATEX");

        let s4 = probe_line("#+attr_: v\n", probe).expect("shape");
        assert!(s4.key_tokens > 1);
    }

    #[test]
    fn probe_respects_active_limit() {
        let src = "#+FIRST: a\n#+SECOND: b\n";
        let mut p = make_parser(src);
        let cap = p
            .tokens_from_cursor()
            .find(|t| t.kind() == OrgSyntaxKind::NEWLINE)
            .expect("newline")
            .end();
        p.with_limit_at(cap, |pp| {
            assert!(pp.line().and_then(probe).is_some(), "first line visible");
            assert!(
                pp.line().expect("line 0").next().is_none(),
                "second line hidden by the limit"
            );
        });

        // Cap inside the key: colon is invisible, probe rejects.
        let mut p2 = make_parser("#+TRUNC: value\n");
        p2.with_limit_at(crate::parser::Pos::from_index(3), |pp| {
            assert!(pp.line().and_then(probe).is_none());
        });
    }

    #[test]
    fn probe_malformed_duals_and_edges() {
        assert!(probe_line("#+CAPTION[unclosed\n", probe).is_none());
        // Closer on the next line must not complete a dual (line-bounded OPTVAL).
        let cross_line = "#+CAPTION[foo\nbar]: value\n";
        assert!(probe_line(cross_line, probe).is_none());
        let out = crate::parse(cross_line);
        assert_eq!(out.root().text().to_string(), cross_line);
        assert!(
            !out.root()
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::KEYWORD),
            "cross-line dual must not parse as KEYWORD"
        );

        // `]` not immediately before `:`: no dual slot, but still a keyword
        // (the whole run is the key, as in org's `\S-*`).
        let x = probe_line("#+CAPTION[short]X: v\n", probe).expect("keyword");
        assert!(!x.has_dual());
        // Whitespace before the colon: not a keyword line at all.
        assert!(probe_line("#+CAPTION[short] : v\n", probe).is_none());
        assert!(probe_line("#+: foo\n", probe).is_none());
        assert!(probe_line("#+:\n", probe).is_none());
        assert!(probe_line("#+KEY value\n", probe).is_none());
        assert!(probe_line("# a comment\n", probe).is_none());
        assert!(probe_line("TITLE: x\n", probe).is_none());
    }

    #[test]
    fn long_key_shape_and_plain_kind() {
        let long = "#+A12345678901234567890XYZ: v";
        let s = probe_line(long, probe).expect("long key shape");
        assert!(s.key_tokens > 0);
        assert_eq!(s.class, KeywordClass::Plain);
        assert_eq!(s.class.syntax_kind(), OrgSyntaxKind::KEYWORD);
        assert!(!s.class.parses_inlines());

        let (kinds, _) = parse_kw(long);
        assert_eq!(*kinds.first().unwrap(), OrgSyntaxKind::KEYWORD);
    }

    #[test]
    fn keyword_class_direct_coverage() {
        use super::KeywordClass;

        assert_eq!(KeywordClass::classify("call"), KeywordClass::Babel);
        assert_eq!(KeywordClass::classify("CALL"), KeywordClass::Babel);
        assert_eq!(
            KeywordClass::classify("call").syntax_kind(),
            OrgSyntaxKind::BABEL_CALL
        );
        assert!(!KeywordClass::classify("call").parses_inlines());

        assert_eq!(KeywordClass::classify("todo"), KeywordClass::Setting);
        assert_eq!(KeywordClass::classify("ToDo"), KeywordClass::Setting);
        assert_eq!(KeywordClass::classify("seq_todo"), KeywordClass::Setting);
        assert_eq!(KeywordClass::classify("TYP_TODO"), KeywordClass::Setting);
        assert_eq!(KeywordClass::classify("priorities"), KeywordClass::Setting);
        assert_eq!(
            KeywordClass::classify("todo").syntax_kind(),
            OrgSyntaxKind::SETTING_KEYWORD
        );
        assert!(!KeywordClass::classify("todo").parses_inlines());

        // Carving table, not every declared setting.
        assert_eq!(KeywordClass::classify("startup"), KeywordClass::Plain);
        assert_eq!(KeywordClass::classify("columns"), KeywordClass::Plain);
        assert!(!is_carving_key("startup"));
        assert!(!is_carving_key("columns"));

        // Caption: distinct class for inline dual/value, still KEYWORD top kind.
        assert_eq!(KeywordClass::classify("caption"), KeywordClass::Caption);
        assert_eq!(KeywordClass::classify("CAPTION"), KeywordClass::Caption);
        assert_eq!(
            KeywordClass::classify("caption").syntax_kind(),
            OrgSyntaxKind::KEYWORD
        );
        assert!(KeywordClass::classify("caption").parses_inlines());

        assert_eq!(KeywordClass::classify("name"), KeywordClass::Plain);
        assert_eq!(KeywordClass::classify("attr_foo"), KeywordClass::Plain);
        assert_eq!(KeywordClass::classify("title"), KeywordClass::Plain);
        assert_eq!(
            KeywordClass::classify("title").syntax_kind(),
            OrgSyntaxKind::KEYWORD
        );
        assert!(!KeywordClass::classify("title").parses_inlines());

        // Near-misses after plain caseless match (not length-gated prefixes).
        for near in [
            "callx",
            "calls",
            "captions",
            "captionx",
            "todoo",
            "todos",
            "prioritie",
        ] {
            assert_eq!(
                KeywordClass::classify(near),
                KeywordClass::Plain,
                "near-miss {near:?} must be Plain"
            );
            assert_eq!(
                KeywordClass::classify(near).syntax_kind(),
                OrgSyntaxKind::KEYWORD
            );
            assert!(!KeywordClass::classify(near).parses_inlines());
        }
    }

    #[test]
    fn carving_key_table_is_the_four_setting_names() {
        for k in [
            "todo",
            "TODO",
            "seq_todo",
            "SEQ_TODO",
            "typ_todo",
            "TYP_TODO",
            "priorities",
            "PRIORITIES",
        ] {
            assert!(is_carving_key(k), "{k}");
            assert_eq!(KeywordClass::classify(k), KeywordClass::Setting, "{k}");
        }
        for k in [
            "startup",
            "STARTUP",
            "columns",
            "title",
            "caption",
            "call",
            "attr_latex",
            "ATTR_LATEX",
        ] {
            assert!(!is_carving_key(k), "{k}");
            assert_ne!(KeywordClass::classify(k), KeywordClass::Setting, "{k}");
        }
        assert_eq!(
            super::CARVING_KEYS,
            &["todo", "seq_todo", "typ_todo", "priorities"]
        );
    }

    // ---------------------------------------------------------------------
    // parse_keyword: emitted kinds and coverage
    // ---------------------------------------------------------------------

    fn results_of(src: &str) -> ParseResult {
        run_element(src, probe, parse)
    }

    fn parse_kw(src: &str) -> (Vec<OrgSyntaxKind>, Option<String>) {
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        let kind = *kinds
            .first()
            .expect("parse_kw produces keyword-kind top node");
        let text = node_text(&evs, src, kind);
        (kinds, text)
    }

    #[test]
    fn parses_emits_expected_kinds() {
        let cases: &[(&str, OrgSyntaxKind)] = &[
            ("#+TITLE: Hello World\n", OrgSyntaxKind::KEYWORD),
            ("#+NAME: fig1\n", OrgSyntaxKind::KEYWORD),
            ("#+CAPTION[short]: main\n", OrgSyntaxKind::KEYWORD),
            ("#+ATTR_LATEX: :w 5\n", OrgSyntaxKind::KEYWORD),
            ("#+TODO: A | B\n", OrgSyntaxKind::SETTING_KEYWORD),
            // Multi-token setting keys (lexer splits on `_`).
            ("#+SEQ_TODO: A | B\n", OrgSyntaxKind::SETTING_KEYWORD),
            ("#+TYP_TODO: A | B\n", OrgSyntaxKind::SETTING_KEYWORD),
            ("#+PRIORITIES: A B C\n", OrgSyntaxKind::SETTING_KEYWORD),
            ("#+seq_todo: X | Y\n", OrgSyntaxKind::SETTING_KEYWORD),
            ("#+CALL: foo()\n", OrgSyntaxKind::BABEL_CALL),
        ];
        for (src, expected) in cases {
            let (kinds, _) = parse_kw(src);
            assert_eq!(
                *kinds.first().unwrap(),
                *expected,
                "top kind failed for {src:?}"
            );
        }

        let (kinds, text) = parse_kw("#+CALL: mycall(arg1)\n");
        assert_eq!(*kinds.first().unwrap(), OrgSyntaxKind::BABEL_CALL);
        assert!(text.as_deref().unwrap().contains("CALL"));

        let (kinds, text) = parse_kw("  #+HEADER: :results output\n");
        assert_eq!(*kinds.first().unwrap(), OrgSyntaxKind::KEYWORD);
        assert_eq!(text.as_deref(), Some("  #+HEADER: :results output\n"));
    }

    #[test]
    fn babel_call_value_is_structured_not_keyword_value() {
        let src = "#+CALL: foo[in](a,b)[end]\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_KEY), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 0);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("foo")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[in]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(a,b)")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("[end]")
        );
    }

    #[test]
    fn babel_call_caseless_key_and_empty_value() {
        let (_toks, evs, _) = results_of("#+CaLl:\n");
        assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_NAME), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 0);

        let (_toks2, evs2, _) = results_of("#+call: bare\n");
        assert_eq!(
            node_text(&evs2, "#+call: bare\n", OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("bare")
        );
        assert_eq!(node_count(&evs2, OrgSyntaxKind::CALL_ARGUMENTS), 0);
    }

    #[test]
    fn keyword_key_runs_to_the_last_colon() {
        let cases: &[(&str, &str, &str, OrgSyntaxKind)] = &[
            ("#+A:B:C: v\n", "A:B:C", "v", OrgSyntaxKind::KEYWORD),
            (
                "#+TODO:FOO: bar\n",
                "TODO:FOO",
                "bar",
                OrgSyntaxKind::KEYWORD,
            ),
            (
                "#+TODO: FOO: bar\n",
                "TODO",
                "FOO: bar",
                OrgSyntaxKind::SETTING_KEYWORD,
            ),
            (
                "#+LINK:foo:bar\n",
                "LINK:foo",
                "bar",
                OrgSyntaxKind::KEYWORD,
            ),
            (
                "#+calling:name:extra\n",
                "calling:name",
                "extra",
                OrgSyntaxKind::KEYWORD,
            ),
        ];
        for &(src, key, value, kind) in cases {
            let (_toks, evs, _) = results_of(src);
            assert_eq!(node_kinds(&evs).first().copied(), Some(kind), "{src}");
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
                Some(key),
                "{src}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::KEYWORD_VALUE).as_deref(),
                Some(value),
                "{src}"
            );
            assert_eq!(
                parse_document_tree(src).text().to_string(),
                src,
                "lossless {src}"
            );
        }
    }

    #[test]
    fn call_without_a_dual_keeps_the_call_key() {
        for src in ["#+call:name:extra\n", "#+CALL:name:extra\n"] {
            let (_toks, evs, _) = results_of(src);
            assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1, "{src}");
            assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 0, "{src}");
            assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 0, "{src}");
            let want_key = if src.contains("CALL") { "CALL" } else { "call" };
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
                Some(want_key),
                "{src}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
                Some("name:extra"),
                "{src}"
            );
        }

        // Dual on the key is not Org's `CALL:` group. The superset stays.
        let src = "#+call[inside]: value\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
            Some("call")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_DUAL).as_deref(),
            Some("[inside]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("value")
        );
    }

    #[test]
    fn call_value_brackets_do_not_steal_the_key() {
        let src = "#+call:name[inside]: extra\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 0);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
            Some("call")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("name")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[inside]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some(": extra")
        );

        let src = "#+call:name(arg)[end]:results\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::BABEL_CALL), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 0);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
            Some("call")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("name")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(arg)")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_END_HEADER).as_deref(),
            Some("[end]:results")
        );
    }

    #[test]
    fn affiliated_key_stops_before_a_value_colon() {
        let cases: &[(&str, &str, Option<&str>, &str)] = &[
            ("#+NAME:fig:1\npara\n", "NAME", None, "fig:1"),
            ("#+CAPTION:a: b\npara\n", "CAPTION", None, "a: b"),
            ("#+CAPTION[s]:a: b\npara\n", "CAPTION", Some("[s]"), "a: b"),
            ("#+ATTR_LATEX::w 1\npara\n", "ATTR_LATEX", None, ":w 1"),
        ];
        for &(src, key, dual, value) in cases {
            let root = parse_document_tree(src);
            assert_eq!(root.text().to_string(), src, "lossless {src}");
            let runs = nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS);
            assert_eq!(runs.len(), 1, "affiliates {src}");
            let kw = &nodes_of_kind(&runs[0], OrgSyntaxKind::KEYWORD)[0];
            assert_eq!(
                nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_KEY)[0]
                    .text()
                    .to_string(),
                key,
                "{src}"
            );
            assert_eq!(
                nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_VALUE)[0]
                    .text()
                    .to_string(),
                value,
                "{src}"
            );
            let dual_text = nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_DUAL)
                .first()
                .map(|n| n.text().to_string());
            assert_eq!(dual_text.as_deref(), dual, "{src}");
        }
    }

    #[test]
    fn orphan_affiliated_name_keeps_the_last_colon() {
        let cases: &[(&str, &str, &str)] = &[
            ("#+NAME:fig:1\n\npara\n", "NAME:fig", "1"),
            ("#+NAME:fig:1\n* H\n", "NAME:fig", "1"),
            ("#+NAME:fig:1", "NAME:fig", "1"),
            ("#+CAPTION:See: this\n", "CAPTION:See", "this"),
            ("#+ATTR_LATEX::w 1\n", "ATTR_LATEX:", "w 1"),
        ];
        for &(src, key, value) in cases {
            let root = parse_document_tree(src);
            assert_eq!(root.text().to_string(), src, "lossless {src}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
                "orphan does not attach {src}"
            );
            let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
            assert_eq!(kws.len(), 1, "{src}");
            assert_eq!(
                nodes_of_kind(&kws[0], OrgSyntaxKind::KEYWORD_KEY)[0]
                    .text()
                    .to_string(),
                key,
                "{src}"
            );
            assert_eq!(
                nodes_of_kind(&kws[0], OrgSyntaxKind::KEYWORD_VALUE)[0]
                    .text()
                    .to_string(),
                value,
                "{src}"
            );
        }
    }

    #[test]
    fn bracketed_non_dual_keyword_stays_in_a_paragraph() {
        let src = "prose\n#+FOO[bar]: baz\nmore\n";
        let later = "prose\n#+FOO[bar]:baz: more\n";
        let after_colon = "prose\n#+FOO:bar[baz]: qux\nmore\n";
        let shorter = "prose\n#+A:B[c]: d\nmore\n";
        let call = "prose\n#+call:name[inside]: extra\nmore\n";
        for src in [src, later, after_colon, shorter, call] {
            let root = parse_document_tree(src);
            assert_eq!(root.text().to_string(), src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(),
                1,
                "{src}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).is_empty(),
                "{src}"
            );
        }

        let alias = "prose\n#+RESULT[h]: v\nmore\n";
        let root = parse_document_tree(alias);
        assert_eq!(root.text().to_string(), alias);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );

        let results = "prose\n#+RESULTS[h]: v\nmore\n";
        let root = parse_document_tree(results);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text().to_string(), "prose\n");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).len(),
            1
        );

        let at_start = "#+RESULT[h]: v\npara\n";
        let root = parse_document_tree(at_start);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).len(),
            1,
            "RESULT still affiliates when it is not inside a paragraph"
        );

        let name = "prose\n#+NAME[s]: v\nmore\n";
        let root = parse_document_tree(name);
        assert_eq!(root.text().to_string(), name);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD)
        );

        let alone = "#+FOO[bar]: baz\n";
        let root = parse_document_tree(alone);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH)
        );
        assert_eq!(root.text().to_string(), alone);

        let split = "prose\n#+CAPTION[short]: long\n";
        let root = parse_document_tree(split);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].text().to_string(), "prose\n");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(), 1);

        let plain = "prose\n#+NAME: v\nmore\n";
        let root = parse_document_tree(plain);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 2);

        // Rightmost `[` in the leading non-whitespace run. A space in an
        // earlier bracket keeps the name `CAPTION`, so the line ends the
        // paragraph. No space, and the name is `CAPTION[a]`, so it does not.
        let spaced = "prose\n#+CAPTION[a b][c]: v\nmore\n";
        let root = parse_document_tree(spaced);
        assert_eq!(root.text().to_string(), spaced);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2, "{spaced}");
        assert_eq!(paras[0].text().to_string(), "prose\n");
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).len(),
            1,
            "{spaced}"
        );

        let glued = "prose\n#+CAPTION[a][b]: v\nmore\n";
        let root = parse_document_tree(glued);
        assert_eq!(root.text().to_string(), glued);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );
    }

    /// The orphan key is what [`probe`] measured. Narrowing and
    /// reclassification happen only after the run attaches, so a value
    /// colon does not make an orphan caption parse inlines, and the same
    /// line above a paragraph does.
    #[test]
    fn attached_caption_reclassifies_after_the_key_shrinks() {
        let orphan = "#+CAPTION:See: *this*\n";
        let root = parse_document_tree(orphan);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD_KEY)[0]
                .text()
                .to_string(),
            "CAPTION:See"
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::BOLD).is_empty(),
            "orphan key is not CAPTION, so the value stays flat"
        );

        let attached = "#+CAPTION:See: *this*\npara\n";
        let root = parse_document_tree(attached);
        assert_eq!(root.text().to_string(), attached);
        let runs = nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(runs.len(), 1);
        let kw = &nodes_of_kind(&runs[0], OrgSyntaxKind::KEYWORD)[0];
        assert_eq!(
            nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_KEY)[0]
                .text()
                .to_string(),
            "CAPTION"
        );
        assert_eq!(
            nodes_of_kind(kw, OrgSyntaxKind::KEYWORD_VALUE)[0]
                .text()
                .to_string(),
            "See: *this*"
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::BOLD).len(), 1);
    }

    #[test]
    fn babel_call_dual_is_key_side_not_inside_header() {
        // Dual on the key stays KEYWORD_DUAL (uniform keyword shell). Elisp CALL:
        // group does not allow dual; CST keeps it for losslessness. Value-side
        // CALL_INSIDE_HEADER is only after `:`.
        let src = "#+CALL[dual]: name()\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_DUAL).as_deref(),
            Some("[dual]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("name")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("()")
        );
        // Dual must not be mistaken for CALL_INSIDE_HEADER.
        assert_eq!(node_count(&evs, OrgSyntaxKind::CALL_INSIDE_HEADER), 0);
    }

    #[test]
    fn babel_call_dual_and_value_inside_coexist() {
        // Key-side dual and value-side CALL_INSIDE_HEADER are independent.
        let src = "#+CALL[dual]: name[in]()\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_DUAL).as_deref(),
            Some("[dual]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_INSIDE_HEADER).as_deref(),
            Some("[in]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("()")
        );
    }

    #[test]
    fn babel_call_space_in_name_via_keyword_shell() {
        let src = "#+call: foo bar(baz)\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("foo bar")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CALL_ARGUMENTS).as_deref(),
            Some("(baz)")
        );
    }

    #[test]
    fn keyword_line_ends_at_newline_not_beyond() {
        let src = "#+TITLE: one\nnext line\n";
        let mut p = make_parser(src);
        let shape = p.line().and_then(probe).expect("keyword probe");
        parse(&mut p, shape);
        assert_eq!(p.text(), "next");
    }

    #[test]
    fn empty_dual_emits_keyword_dual_brackets() {
        // Empty dual interior: region between brackets is empty after opener eat.
        let src = "#+CAPTION[]: body\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_DUAL).as_deref(),
            Some("[]")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_VALUE).as_deref(),
            Some("body")
        );
        assert_eq!(tree_text(&parse_document_tree(src)), src);
    }

    #[test]
    fn empty_keyword_value_after_colon() {
        // Non-babel empty value: with_rest_of_line empty window after colon.
        let src = "#+TITLE:\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_VALUE).as_deref(),
            Some("")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
            Some("TITLE")
        );
    }

    #[test]
    fn empty_keyword_value_without_trailing_newline() {
        // Empty value at EOF (no trailing NEWLINE): still empty KEYWORD_VALUE.
        let src = "#+TITLE:";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_VALUE).as_deref(),
            Some("")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::KEYWORD_KEY).as_deref(),
            Some("TITLE")
        );
        assert_eq!(tree_text(&parse_document_tree(src)), src);
    }

    #[test]
    fn keyword_no_final_newline_and_long_key() {
        let src = "#+TITLE: no nl at end";
        let (_toks, evs, _) = run_element(src, probe, parse);
        let kinds = node_kinds(&evs);
        assert_eq!(*kinds.first().unwrap(), OrgSyntaxKind::KEYWORD);
        let mut p = make_parser(src);
        let shape = p.line().and_then(probe).expect("keyword probe");
        parse(&mut p, shape);
        assert_eq!(p.current(), OrgSyntaxKind::EOF);

        let long = "#+A12345678901234567890: v";
        let (_toks2, evs2, _) = run_element(long, probe, parse);
        let kinds2 = node_kinds(&evs2);
        assert_eq!(*kinds2.first().unwrap(), OrgSyntaxKind::KEYWORD);
    }

    // ---------------------------------------------------------------------
    // Full pipeline: keywords appear correctly in a parsed document
    // ---------------------------------------------------------------------

    #[test]
    fn keywords_appear_in_document_tree() {
        use crate::tests::{nodes_of_kind, parse_document_tree};

        let src = "#+TITLE: My Doc\n#+NAME: fig1\nSome text.\n* H\nbody\n";
        let root = parse_document_tree(src);

        let keywords = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert!(
            !keywords.is_empty(),
            "expected KEYWORD nodes (affiliated candidates are KEYWORD uniformly)"
        );
    }

    #[test]
    fn babel_call_appears_in_document_tree() {
        use crate::tests::{nodes_of_kind, parse_document_tree};

        let src = "#+CALL: foo()\n#+call: n[i](a)[e]\nbody\n";
        let root = parse_document_tree(src);
        let babels = nodes_of_kind(&root, OrgSyntaxKind::BABEL_CALL);
        assert_eq!(babels.len(), 2);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD_VALUE).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD_VALUE)
        );
    }

    /// The keyword line itself is the same node whether or not it affiliates;
    /// only its parent differs. Attachment cases live in `forms::affiliated`.
    #[test]
    fn aff_name_keywords_keep_kind_keyword_either_way() {
        use crate::tests::{nodes_of_kind, parse_document_tree};

        let orphaned: &[(&str, &str)] = &[
            ("#+NAME: orphan\n* H\nbody\n", "before heading"),
            (
                "#+CAPTION: gap case\n\nA paragraph after gap.\n",
                "blank gap",
            ),
            (
                "#+ATTR_HTML: :a b\n# this is comment\npara\n",
                "before comment",
            ),
            (
                "#+NAME: c\nCLOCK: [2026-07-07 Wed 10:00]\nmore\n",
                "before clock",
            ),
        ];
        for (src, desc) in orphaned {
            let root = parse_document_tree(src);
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).is_empty(),
                "orphaned candidate must not be wrapped in {desc}"
            );
            assert!(
                !nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).is_empty(),
                "must emit KEYWORD for aff-cand in {desc}"
            );
        }

        let attached: &[(&str, &str)] = &[
            ("#+NAME: fig\nA paragraph here.\n", "attachable para"),
            (
                "#+NAME: fig\n*not-headline text here\nmore para.\n",
                "*not heading (no space after the star)",
            ),
            ("#+RESNAME: x\r\nNext line\r\n", "CRLF after aff-cand"),
        ];
        for (src, desc) in attached {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::AFFILIATED_KEYWORDS).len(),
                1,
                "attaching candidate must be wrapped in {desc}"
            );
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(),
                1,
                "run members keep kind KEYWORD in {desc}"
            );
        }

        let src_star = "#+NAME: fig\n*not-headline text here\nmore para.\n";
        let root_star = parse_document_tree(src_star);
        assert!(
            !nodes_of_kind(&root_star, OrgSyntaxKind::PARAGRAPH).is_empty(),
            "{:?}",
            nodes_of_kind(&root_star, OrgSyntaxKind::PARAGRAPH)
        );
    }

    #[test]
    fn block_still_wins_over_keyword_shape() {
        use crate::tests::{nodes_of_kind, parse_document_tree};

        let src = "#+begin_example\ninside\n#+end_example\n";
        let root = parse_document_tree(src);
        let blocks = nodes_of_kind(&root, OrgSyntaxKind::EXAMPLE_BLOCK);
        let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert!(!blocks.is_empty(), "{blocks:?}");
        assert!(kws.is_empty(), "{kws:?}");
    }

    #[test]
    fn caption_value_parses_inlines_during_grammar() {
        use crate::tests::{nodes_of_kind, parse_document_tree};

        let src = "#+CAPTION: text with *bold* and /italic/\nA paragraph.\n";
        let root = parse_document_tree(src);

        let kws = nodes_of_kind(&root, OrgSyntaxKind::KEYWORD);
        assert!(!kws.is_empty(), "{kws:?}");

        let bolds = nodes_of_kind(&root, OrgSyntaxKind::BOLD);
        let italics = nodes_of_kind(&root, OrgSyntaxKind::ITALIC);
        assert!(!bolds.is_empty(), "expected BOLD from caption value");
        assert!(!italics.is_empty(), "expected ITALIC from caption value");
    }

    #[test]
    fn caption_caseless_parse_keyword_emits_keyword_and_inlines() {
        // Lowercase / mixed-case CAPTION through parse (not only classify).
        let src = "#+caption[short *s*]: main *body*\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 1);
        assert!(
            node_count(&evs, OrgSyntaxKind::BOLD) >= 2,
            "caseless caption dual+value should parse BOLD"
        );

        let src2 = "#+CaPtIoN: *only*\n";
        let (_t2, evs2, _) = results_of(src2);
        assert_eq!(*node_kinds(&evs2).first().unwrap(), OrgSyntaxKind::KEYWORD);
        assert_eq!(node_count(&evs2, OrgSyntaxKind::BOLD), 1);
    }

    #[test]
    fn non_caption_dual_stays_flat_with_markup() {
        // Non-caption dual must not expand markup (contrast with CAPTION).
        let src = "#+RESULTS[*not* bold]: out\n";
        let (_toks, evs, _) = results_of(src);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD), 1);
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_DUAL), 1);
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::BOLD),
            0,
            "non-caption dual must stay flat (no BOLD)"
        );
        // Value side also flat for non-caption.
        assert_eq!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE), 1);
    }

    #[test]
    fn keyword_decomposition_and_caption_scope() {
        let (_toks, evs, _) = results_of("#+TITLE: Hello *world*\n");
        assert!(node_count(&evs, OrgSyntaxKind::KEYWORD_KEY) > 0);
        assert!(node_count(&evs, OrgSyntaxKind::KEYWORD_VALUE) > 0);
        assert_eq!(node_count(&evs, OrgSyntaxKind::BOLD), 0);

        let (_toks2, evs2, _) = results_of("#+CAPTION[short *s*]: main *body*\n");
        assert!(node_count(&evs2, OrgSyntaxKind::KEYWORD_KEY) > 0);
        assert!(node_count(&evs2, OrgSyntaxKind::KEYWORD_DUAL) > 0);
        assert!(node_count(&evs2, OrgSyntaxKind::KEYWORD_VALUE) > 0);
        assert!(node_count(&evs2, OrgSyntaxKind::BOLD) >= 2);

        let (_t, evs3, _) = results_of("#+RESULTS[abc123]: output\n");
        assert!(node_count(&evs3, OrgSyntaxKind::KEYWORD_DUAL) > 0);

        let (_toks_m, evs_m, _) = results_of("#+MY_KEY-2: val\n");
        assert!(node_count(&evs_m, OrgSyntaxKind::KEYWORD_KEY) > 0);
        let kt = node_text(&evs_m, "#+MY_KEY-2: val\n", OrgSyntaxKind::KEYWORD_KEY);
        assert_eq!(kt.as_deref(), Some("MY_KEY-2"));

        let (_t4, evs4, _) = results_of("#+CAPTION[[n]ested]: v\n");
        assert!(node_count(&evs4, OrgSyntaxKind::KEYWORD_DUAL) > 0);
    }

    /// Only CAPTION opens an object region in its dual and value; every other
    /// key keeps flat text.
    ///
    /// Pinned end-to-end, not through `KeywordClass` alone: classification
    /// says which class a key is, and a wiring change that consulted the
    /// dual-key table instead of the parsed-key table would keep every class
    /// answer identical while parsing RESULTS markup into objects. Only the
    /// emitted tree can tell the two apart.
    #[test]
    fn parsed_keyword_scope_is_caption_only() {
        let flat = parse_document_tree("#+RESULTS[*not* bold]: out *x*\n");
        assert_eq!(nodes_of_kind(&flat, OrgSyntaxKind::KEYWORD_DUAL).len(), 1);
        assert!(
            nodes_of_kind(&flat, OrgSyntaxKind::BOLD).is_empty(),
            "a RESULTS dual and value stay flat text"
        );

        let parsed = parse_document_tree("#+CAPTION[*s*]: *body*\n");
        assert_eq!(
            nodes_of_kind(&parsed, OrgSyntaxKind::BOLD).len(),
            2,
            "a CAPTION dual and value are both object regions"
        );
    }

    /// A keyword line inside a **lesser** block body is body text, never a
    /// node; inside a **greater** block it is a real keyword.
    ///
    /// Settings collection reads directives from exactly the nodes the grammar
    /// emitted. If a lesser body ever produced keyword nodes, an inert
    /// `#+TODO:` inside an example block would silently start carving, and no
    /// other test would notice.
    #[test]
    fn keyword_lines_in_a_lesser_block_body_are_not_nodes() {
        let root = parse_document_tree(concat!(
            "#+NAME: top\n",
            "#+begin_quote\n",
            "#+NAME: in-greater\n",
            "quoted\n",
            "#+end_quote\n",
            "#+begin_example\n",
            "#+NAME: not-a-node\n",
            "#+end_example\n",
        ));
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::KEYWORD).len(),
            2,
            "top level and inside the greater block; not inside the example"
        );
    }
}
