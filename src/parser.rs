//! Event-emitting recursive descent parser for the Org concrete syntax tree.
//!
//! ## Pipeline
//!
//! ```text
//! source
//!   -> lex() -> Lexed { tokens, lines }   (tiles + line headers, one walk)
//!   -> grammar rules (document, heading, comment, ...)
//!        drive Parser, which records Events
//!   -> builder::build(tokens, events) -> rowan::GreenNode
//!   -> SyntaxNode root (red tree views live in ast::*)
//! ```
//!
//! ## How grammar rules are written
//!
//! Most constructs have a *probe* that inspects a [`LineView`] or
//! [`TokenView`] without consuming and returns a *shape* (what it measured),
//! and a *parse* that consumes that shape. A rule either consumes input or
//! claims nothing; callers check [`Parser::progress`] and fall back to a
//! paragraph or plain text. A body is parsed with the visible limit lowered
//! to its end ([`Parser::with_limit_at`]), so it cannot read past its
//! container. A construct that cannot complete is kept as text, with a
//! [`Diagnostic`] when it was a near miss; parsing never fails.
//!
//! ## Events
//!
//! The parser never constructs the tree. It records a flat sequence of
//! structural decisions ([`Event`]), which is what makes backtracking cheap: a
//! failed alternative truncates the events `Vec`. `Start`/`Finish` bracket
//! nodes; `Token { kind, range }` records a green leaf over an absolute source
//! span; `Tombstone` is both the placeholder written by `start()` and the
//! residue of `abandon()` (the builder skips it).
//!
//! ## Cursor state
//!
//! The cursor is `(token_index, byte_offset_in_token)`. Measured leaves may end
//! inside a lex tile (`eat_to` / `leaf_len`); the cursor stays at that
//! exclusive end and the parent loop emits the remainder as ordinary text, so
//! lex tiling stays fixed and context-free. Progress checks compare
//! [`Parser::progress`], which includes both. Mid-tile, the position
//! predicates (`at_line_start`, `at_blank`, `at_region_start`) are false;
//! `prev_byte` and `prev_char` account for the offset.
//!
//! ## Fields with invariants
//!
//! - `limit` - exclusive visible token bound; all consumption reads it.
//!   Lowered only by [`Parser::with_limit_at`] /
//!   [`Parser::with_rest_of_line`] (nesting can only shrink).
//! - `inline_region_start` - set by object walks so the PRE condition can
//!   accept an opener at the first token of the active region.
//! - `nesting`, `outline_nesting` - open nesting levels against
//!   `ParseConfig::max_nesting_depth`. They bound Rust stack during the parse and
//!   green tree depth (whose `Drop` recurses) after it; past the budget a
//!   region is kept as plain tiles rather than recursed into.
//! - `frame_ends` - cached index of drawer, block and latex END lines; see
//!   the field docs below and `grammar::drive::frame`.
//! - `math_closer_miss` - known-empty absolute byte ranges for latex math
//!   closers (`\)` / `\]`); see field docs and `forms/latex`.
//!
//! `finish(self)` consumes the parser and hands the builder the three pieces it
//! needs; the `Vec<LexedToken>` is dropped once the green tree exists.

use crate::lexer::{Lexed, LexedToken, Line, lex};
use crate::syntax::OrgSyntaxKind;
use rowan::TextRange;
use std::cell::Cell;
use std::cell::RefCell;

mod config;
mod diagnostic;
mod eat;
mod event;
mod line;
mod peek;
mod pos;
mod source;
mod view;

pub(crate) mod builder;
pub(crate) mod grammar;

#[cfg(test)]
mod tests;

pub(crate) use event::Event;
pub(crate) use peek::PairNewlines;
pub(crate) use pos::{LineId, Pos, Progress};
pub(crate) use view::{LineView, TokenView};

pub(crate) use config::delta::{ParseConfigDelta, TodoKeywordDelta, parse_config_delta};
pub use config::{
    DEFAULT_INLINE_TASK_MIN_LEVEL, DEFAULT_LINK_TYPES, DEFAULT_MAX_NESTING_DEPTH,
    DEFAULT_TODO_KEYWORDS, ParseConfig, PriorityAtom, PriorityRange, TodoInterpretation, TodoKind,
    TodoSequence,
};
pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};

/// Event-emitting recursive-descent driver over a pre-lexed token stream.
#[derive(Debug)]
pub(crate) struct Parser<'s> {
    source: &'s str,
    /// Absolute index of the current lex token (or `tokens.len()` at true EOF).
    pos: usize,
    /// Byte offset into `tokens[pos]` when the cursor sits mid-tile.
    ///
    /// Zero means the cursor is at the start of the current token (or idle at
    /// EOF). After a measured leaf ends inside a tile, this holds the exclusive
    /// end offset relative to that token's start so the parent loop can emit
    /// the remainder as ordinary text. Feeds [`Self::progress`] (token index +
    /// mid-tile offset); mid-tile, the line-start and PRE predicates are
    /// false.
    token_byte_off: usize,
    tokens: Vec<LexedToken>,
    /// Line headers fused in lex (structure only).
    lines: Vec<Line>,
    events: Vec<Event>,
    diagnostics: Vec<Diagnostic>,

    /// Parser configuration (todo keywords, inline-task threshold, link types).
    ///
    /// Borrowed for one materialize: the owning `Document` holds the sole
    /// `Arc<ParseConfig>`, so reparsing never clones the keyword maps.
    pub(crate) config: &'s ParseConfig,

    /// Exclusive upper bound on the visible token range.
    ///
    /// Peeking, eating and paired scans never see tokens at or after it.
    /// Defaults to `tokens.len()`; lowered temporarily by
    /// [`Self::with_limit_at`] / [`Self::with_rest_of_line`]. Grammar code
    /// reads it through the typed [`Self::limit`] handle.
    limit: usize,

    /// Absolute token index of the current inline region start, if any.
    ///
    /// Set and restored by the object walks in `grammar::drive::region`
    /// through [`Self::with_inline_region_start`]. Feeds
    /// [`Self::at_region_start`], so the PRE condition accepts openers at the
    /// first token of the active region even when other tokens precede it.
    /// Independent of the visible limit.
    inline_region_start: Option<usize>,

    /// Open *region* nesting levels (object and element bodies).
    ///
    /// Incremented by [`Self::with_nesting`], whose only callers are the
    /// three region functions in `grammar::drive::region`. This tracks Rust
    /// recursion depth: one level is one live object walk or
    /// `parse_one_element` call chain, so the counter is what keeps
    /// pathological input from overflowing the stack.
    nesting: u16,

    /// Open OUTLINE frames.
    ///
    /// Costs no Rust stack (the spine is iterative), but each level is a green
    /// tree level, and rowan's green `Drop` recurses. Kept separate from
    /// `nesting` so a deeply outlined document does not starve the budget its
    /// sections need for their own bodies.
    outline_nesting: u16,

    /// Cached next-END tables for drawers, blocks and latex environments,
    /// built once per parse.
    ///
    /// See [`grammar::drive::frame`]: N unclosed openers share one O(lines)
    /// build instead of each scanning for its END line. The build reads whole
    /// lines via [`Self::full_line_at`]; look-ups drop ends past the region
    /// limit.
    pub(crate) frame_ends: RefCell<Option<grammar::drive::frame::FrameEndCache>>,

    /// Known-empty absolute byte ranges for latex math closers (`\)` / `\]`).
    ///
    /// Type and policy live in [`grammar::forms::latex`]: on a miss, the search
    /// records that `source[haystack_abs..window_end)` has no closer of that
    /// kind. Later unclosed openers whose window starts at or after
    /// `haystack_abs` and ends at or before `window_end` fail in O(1) instead
    /// of rescanning. Successful finds leave the memo unchanged.
    pub(crate) math_closer_miss: Cell<grammar::forms::latex::MathCloserMissMemo>,

    /// Test-only work counter for frame-end amortisation assertions.
    #[cfg(test)]
    pub(crate) frame_end_work: Cell<usize>,

    /// Test-only: bytes examined by math-closer scans during real parses.
    ///
    /// Charged once per actual closer search (not on memo hits); checked by
    /// `math_closer_work_is_linear_for_unclosed`.
    #[cfg(test)]
    pub(crate) math_closer_work: Cell<usize>,
}

impl<'s> Parser<'s> {
    /// Construct a parser with an explicit configuration.
    ///
    /// # Panics
    ///
    /// Panics if `source.len() > crate::MAX_SOURCE_LEN` (rowan `TextSize` is
    /// `u32`, and offsets must not be silently truncated).
    pub(crate) fn with_config(source: &'s str, config: &'s ParseConfig) -> Self {
        crate::assert_max_size(source);
        let Lexed { tokens, lines } = lex(source);
        let n = tokens.len();

        Self {
            source,
            pos: 0,
            token_byte_off: 0,
            tokens,
            lines,
            events: Vec::new(),
            diagnostics: Vec::new(),
            config,
            limit: n,
            inline_region_start: None,
            nesting: 0,
            outline_nesting: 0,
            frame_ends: RefCell::new(None),
            math_closer_miss: Cell::new(grammar::forms::latex::MathCloserMissMemo::default()),
            #[cfg(test)]
            frame_end_work: Cell::new(0),
            #[cfg(test)]
            math_closer_work: Cell::new(0),
        }
    }

    /// Consume the parser and return tokens, events, and diagnostics.
    ///
    /// The tokens are handed over by value; they are dropped once the green
    /// tree is built.
    pub(crate) fn finish(self) -> (Vec<LexedToken>, Vec<Event>, Vec<Diagnostic>) {
        (self.tokens, self.events, self.diagnostics)
    }

    // -----------------------------------------------------------------------
    // Nesting budget
    // -----------------------------------------------------------------------

    /// Run `f` one region-nesting level deeper, or return `None` without
    /// running it when the budget is exhausted.
    ///
    /// Scoped like [`Self::with_limit_at`] and
    /// [`Self::with_inline_region_start`], so the counter is restored on every
    /// normal return path. `Checkpoint` also stores `nesting`, so an
    /// `attempt` rewind cannot leave the counter wrong if it crosses a region
    /// boundary.
    ///
    /// `None` is not an error: the caller consumes the region as plain tiles
    /// (`grammar::drive::region`). The budget affects only what is emitted,
    /// never recognition: nesting depth cannot change what a line is.
    pub(crate) fn with_nesting<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if self.nesting >= self.config.max_nesting_depth() {
            return None;
        }
        self.nesting += 1;
        let result = f(self);
        self.nesting -= 1;
        Some(result)
    }

    /// Region nesting already charged around a star-line fragment.
    ///
    /// A fresh parser starts at 0. Inline-task titles are parsed inside
    /// `elements_in` regions that have already spent budget, so reparsing
    /// such a title from 0 would nest it more deeply than the full parse
    /// did.
    pub(crate) fn set_region_nesting(&mut self, depth: u16) {
        self.nesting = depth;
    }

    /// Region nesting currently charged.
    #[inline]
    pub(crate) fn region_nesting(&self) -> u16 {
        self.nesting
    }

    /// Ancestor outlines already open above a fragment.
    ///
    /// A fresh parser starts at 0. The full parse opened this outline under
    /// that many ancestors; starting a reparse at 0 would let a tight budget
    /// nest a child that the full parse attached higher up.
    pub(crate) fn set_outline_nesting(&mut self, depth: u16) {
        self.outline_nesting = depth;
    }

    /// Outline nesting currently charged.
    #[inline]
    pub(crate) fn outline_nesting(&self) -> u16 {
        self.outline_nesting
    }

    /// Open one OUTLINE nesting level.
    ///
    /// Not scoped, because an OUTLINE stays open across iterations of the
    /// document loop. `grammar::drive::document::open_outline` and
    /// `close_outline` are the only push/pop pair. Plain arithmetic, not
    /// saturating: an imbalance is a bug, caught by
    /// [`Self::nesting_balanced`] at the end of the document.
    pub(crate) fn push_outline(&mut self) {
        self.outline_nesting += 1;
    }

    /// Close one OUTLINE nesting level (pairs with [`Self::push_outline`]).
    ///
    /// # Panics
    ///
    /// Panics if no outline is open: every pop must match a push.
    pub(crate) fn pop_outline(&mut self) {
        self.outline_nesting = self
            .outline_nesting
            .checked_sub(1)
            .expect("pop_outline without a matching push");
    }

    /// Whether another OUTLINE level fits under the budget.
    #[inline]
    pub(crate) fn outline_available(&self) -> bool {
        self.outline_nesting < self.config.max_nesting_depth()
    }

    /// Both nesting counters are back to zero (end-of-parse balance check).
    #[inline]
    pub(crate) fn nesting_balanced(&self) -> bool {
        self.nesting == 0 && self.outline_nesting == 0
    }

    // -----------------------------------------------------------------------
    // Raw accessors for the parser's own views; grammar rules use the views
    // -----------------------------------------------------------------------

    /// Full source text.
    #[inline]
    pub(crate) fn source(&self) -> &'s str {
        self.source
    }

    /// Total lex token count.
    #[inline]
    pub(crate) fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Kind of the token at absolute index `i` (must be in range).
    #[inline]
    pub(crate) fn token_kind(&self, i: usize) -> OrgSyntaxKind {
        self.tokens[i].kind
    }

    /// Absolute byte range of the token at index `i` (must be in range).
    #[inline]
    pub(crate) fn token_range(&self, i: usize) -> TextRange {
        self.tokens[i].range
    }

    /// Copy of the line record at index `i`, if any.
    #[inline]
    pub(crate) fn line_rec(&self, i: usize) -> Option<Line> {
        self.lines.get(i).copied()
    }

    /// Number of lines in the lex line table.
    #[inline]
    pub(crate) fn line_table_len(&self) -> usize {
        self.lines.len()
    }
}

/// Signed relative offset -> absolute token index under the visible limit.
///
/// Backs `peek_rel`, so out-of-range lookups are handled in one place.
/// Grammar rules use views instead.
#[inline]
fn abs_index(p: &Parser<'_>, rel: isize) -> Option<usize> {
    let abs = if rel >= 0 {
        // `rel >= 0`, so the conversion cannot lose a sign.
        p.pos.checked_add(usize::try_from(rel).ok()?)?
    } else {
        p.pos.checked_sub(rel.unsigned_abs())?
    };
    (abs < p.limit).then_some(abs)
}
