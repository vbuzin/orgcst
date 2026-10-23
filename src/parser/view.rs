//! Line and token views: element rules read lines, object rules read tokens,
//! and both yield typed positions for region caps.
//!
//! A [`LineView`] / [`TokenView`] is a `Copy` handle (`&Parser` + index)
//! rooted at one line or token. Views only observe; extracting a [`Pos`] /
//! [`LineId`] ends the borrow so consumption can resume. Nothing at or past
//! the active limit is visible, and byte questions answer `None` there (a
//! region end is not a byte).
//!
//! A [`TokenView`] always reports the whole lex tile at its index. The
//! unconsumed remainder of a tile the cursor sits inside is only visible
//! through [`Parser::text`] / [`Parser::rest_source`] / [`Parser::prev_byte`].
//! Element probes take a `LineView`, so the same probe serves the dispatcher
//! (current line) and lookahead scans (paragraph termination, drawer and list
//! end walks).
//!
//! Object rules read consecutive tokens from a [`TokenView`] with `nth` /
//! `kind_at` / `text_at` / `seq` (view-relative offsets; `EOF` / `""` past
//! the end or the visible limit), as [`LineView`] does with content-relative
//! `nth` / `kind` / `text`.

use super::Parser;
use super::pos::{LineId, Pos};
use crate::lexer::Line;
use crate::syntax::OrgSyntaxKind;

/// One source line as an observation handle.
///
/// `cap` is the exclusive token bound this view observes under. Views made by
/// the normal constructors carry the active region limit; [`Parser::full_line_at`]
/// carries the whole token stream. Derived views (`next`, `nth`,
/// `lines_after`, ...) inherit it, so a walk cannot change its own visibility
/// mid-way.
#[derive(Clone, Copy)]
pub(crate) struct LineView<'a, 's> {
    p: &'a Parser<'s>,
    idx: u32,
    cap: u32,
}

/// One lex token as an observation handle. `cap` as on [`LineView`].
#[derive(Clone, Copy)]
pub(crate) struct TokenView<'a, 's> {
    p: &'a Parser<'s>,
    idx: u32,
    cap: u32,
}

impl<'s> Parser<'s> {
    /// View of the line whose first token is the cursor.
    ///
    /// `None` anywhere else, including mid-tile. Element and heading
    /// recognition start here.
    pub(crate) fn line(&self) -> Option<LineView<'_, 's>> {
        if !self.at_line_start() {
            return None;
        }
        let idx = self.line_at(self.pos)?;
        Some(LineView {
            p: self,
            idx: idx as u32,
            cap: self.limit as u32,
        })
    }

    /// View of the line containing the cursor, at any position on it.
    ///
    /// Unlike [`Self::line`] this does not require line-start: paragraph end
    /// scans use it after mid-line entry. `None` at EOF.
    pub(crate) fn line_here(&self) -> Option<LineView<'_, 's>> {
        let idx = self.line_at(self.pos)?;
        Some(LineView {
            p: self,
            idx: idx as u32,
            cap: self.limit as u32,
        })
    }

    /// Tokens from the cursor's token onward, full tiles, limit-clamped.
    ///
    /// Mid-tile: the first item is still the *full* lex tile at the cursor
    /// index. Unconsumed remainder bytes are only on [`Parser::text`] /
    /// [`Parser::rest_source`] / [`Parser::prev_byte`] - not on this view.
    pub(crate) fn tokens_from_cursor(&self) -> impl Iterator<Item = TokenView<'_, 's>> {
        let end = self.limit.min(self.token_count());
        (self.pos..end).map(move |i| TokenView {
            p: self,
            idx: i as u32,
            cap: self.limit as u32,
        })
    }

    /// View of the line containing position `pos`, when visible.
    pub(crate) fn line_of(&self, pos: Pos) -> Option<LineView<'_, 's>> {
        let idx = self.line_at(pos.index())?;
        ((pos.index()) < self.limit).then_some(LineView {
            p: self,
            idx: idx as u32,
            cap: self.limit as u32,
        })
    }

    /// View of the token at `pos`, when it is under the limit.
    pub(crate) fn token_view(&self, pos: Pos) -> Option<TokenView<'_, 's>> {
        (pos.index() < self.limit.min(self.token_count())).then_some(TokenView {
            p: self,
            idx: pos.index() as u32,
            cap: self.limit as u32,
        })
    }

    /// View of line table index `idx` observing the **whole** token stream.
    ///
    /// The one unrestricted constructor: the cached block and drawer END
    /// index must be built from full lines even when first needed inside a
    /// nested region, and its look-ups re-apply the active limit themselves.
    /// Grammar rules use [`Self::line`] / [`Self::line_of`], which carry the
    /// region cap.
    #[inline]
    pub(crate) fn full_line_at(&self, idx: usize) -> Option<LineView<'_, 's>> {
        (idx < self.line_table_len()).then_some(LineView {
            p: self,
            idx: idx as u32,
            cap: self.token_count() as u32,
        })
    }

    /// Token view at the cursor, when the cursor is under the limit.
    ///
    /// Shorthand for `token_view(cursor())`. Mid-tile it still views the
    /// whole tile.
    #[inline]
    pub(crate) fn here(&self) -> Option<TokenView<'_, 's>> {
        self.token_view(self.cursor())
    }

    /// Current cursor as a typed position (token-granular).
    #[inline]
    pub(crate) fn cursor(&self) -> Pos {
        Pos::from_index(self.pos)
    }

    /// Exclusive end of the visible window as a typed position.
    #[inline]
    pub(crate) fn limit(&self) -> Pos {
        Pos::from_index(self.limit)
    }
}

impl<'a, 's> LineView<'a, 's> {
    #[inline]
    fn rec(&self) -> Line {
        self.p
            .line_rec(self.idx as usize)
            .expect("LineView index is valid by construction")
    }

    /// Opaque id of this line.
    #[inline]
    pub(crate) fn id(&self) -> LineId {
        LineId::from_index(self.idx as usize)
    }

    /// Position of the line's first token.
    #[inline]
    pub(crate) fn start(&self) -> Pos {
        Pos::from_index(self.rec().first as usize)
    }

    /// Exclusive end position of the line, clamped to the observation window.
    #[inline]
    pub(crate) fn end(&self) -> Pos {
        Pos::from_index((self.rec().end as usize).min(self.cap as usize))
    }

    /// Leading indent in columns (space = 1, tab to next multiple of 8).
    #[inline]
    pub(crate) fn indent_cols(&self) -> u32 {
        self.rec().indent_cols
    }

    /// Whether the line is blank (optional INDENT + NEWLINE only).
    #[inline]
    pub(crate) fn is_blank(&self) -> bool {
        self.rec().is_blank
    }

    /// View of the line's first token (INDENT when indented).
    pub(crate) fn first(&self) -> Option<TokenView<'a, 's>> {
        self.token_at_abs(self.rec().first as usize)
    }

    /// View of the first content token (skipping leading INDENT).
    pub(crate) fn content(&self) -> Option<TokenView<'a, 's>> {
        self.token_at_abs(self.rec().content as usize)
    }

    /// View of the `k`-th token counted from line content.
    pub(crate) fn nth(&self, k: usize) -> Option<TokenView<'a, 's>> {
        let abs = (self.rec().content as usize).checked_add(k)?;
        self.token_at_abs(abs)
    }

    /// Kind of the `k`-th content token; `EOF` outside the line or window.
    ///
    /// Prefer [`Self::run_while`] for consecutive-token runs: it is bounded by
    /// [`Self::nth`] and cannot hang. `EOF` here remains useful for single-shot
    /// probes (`kind(k) == COLON`, terminator checks after a measured run).
    pub(crate) fn kind(&self, k: usize) -> OrgSyntaxKind {
        self.nth(k).map_or(OrgSyntaxKind::EOF, |t| t.kind())
    }

    /// Text of the `k`-th content token; empty outside the line or window.
    pub(crate) fn text(&self, k: usize) -> &'s str {
        self.nth(k).map_or("", |t| t.text())
    }

    /// Count consecutive content tokens from `k` satisfying `pred`.
    ///
    /// Bounded by [`Self::nth`] (line end, token stream, active limit). Cannot
    /// hang even if `pred` would accept a sentinel kind: past-end tokens are
    /// never offered to `pred`.
    pub(crate) fn run_while(
        &self,
        k: usize,
        mut pred: impl FnMut(OrgSyntaxKind, &str) -> bool,
    ) -> usize {
        let mut n = 0usize;
        while let Some(t) = self.nth(k + n) {
            if !pred(t.kind(), t.text()) {
                break;
            }
            n += 1;
        }
        n
    }

    /// Whether content tokens from index 0 are a **prefix match** of `kinds`.
    ///
    /// Empty `kinds` is always true. A longer `kinds` than the visible line
    /// fails (past end / past the limit answers `EOF`, not a match). Same
    /// contract as [`TokenView::seq`].
    #[inline]
    pub(crate) fn seq(&self, kinds: &[OrgSyntaxKind]) -> bool {
        kinds.iter().enumerate().all(|(i, &k)| self.kind(i) == k)
    }

    /// Caseless ASCII equality of content token `k`'s text with `s`.
    ///
    /// Out-of-range `k` uses empty text (`""`), so `s == ""` is true and any
    /// non-empty `s` is false. Callers that need a live token should check
    /// `nth(k).is_some()` first when empty-string equality would be wrong.
    #[inline]
    pub(crate) fn text_eq_caseless(&self, k: usize, s: &str) -> bool {
        self.text(k).eq_ignore_ascii_case(s)
    }

    /// Parser configuration, for probes that depend on it (heading level,
    /// list bullets).
    #[inline]
    pub(crate) fn config(&self) -> &'a crate::parser::ParseConfig {
        self.p.config
    }

    /// Underlying parser (for the cached END index).
    #[inline]
    pub(crate) fn parser(&self) -> &'a Parser<'s> {
        self.p
    }

    /// Contiguous source text of content tokens `[from_k, to_k)` (both
    /// content-relative). Empty when the range is empty or out of the line.
    pub(crate) fn text_range(&self, from_k: usize, to_k: usize) -> &'s str {
        let (Some(a), Some(b)) = (
            self.nth(from_k),
            to_k.checked_sub(1).and_then(|k| self.nth(k)),
        ) else {
            return "";
        };
        let from = usize::from(self.p.token_range(a.pos().index()).start());
        let to = usize::from(self.p.token_range(b.pos().index()).end());
        &self.p.source()[from..to]
    }

    /// Balanced same-delimiter pair scan from content token `from_k`.
    ///
    /// Line-bounded ([`super::PairNewlines::Stop`]). Returns the closer's
    /// content-relative index; `None` when `from_k` is not `open` or the pair
    /// does not close on this line.
    pub(crate) fn find_paired(
        &self,
        from_k: usize,
        open: OrgSyntaxKind,
        close: OrgSyntaxKind,
    ) -> Option<usize> {
        let from = self.nth(from_k)?.pos();
        let closer = self
            .p
            .find_paired_at(from, open, close, super::PairNewlines::Stop)?;
        Some(from_k + (closer.index() - from.index()))
    }

    /// The next line, when it starts under the observation limit.
    pub(crate) fn next(&self) -> Option<LineView<'a, 's>> {
        let next = (self.idx as usize).checked_add(1)?;
        let rec = self.p.line_rec(next)?;
        ((rec.first as usize) < self.cap as usize).then_some(LineView {
            p: self.p,
            idx: next as u32,
            cap: self.cap,
        })
    }

    /// Lines strictly after this one, stopping at the limit.
    pub(crate) fn lines_after(&self) -> impl Iterator<Item = LineView<'a, 's>> + use<'a, 's> {
        let mut cur = *self;
        std::iter::from_fn(move || {
            let n = cur.next()?;
            cur = n;
            Some(n)
        })
    }

    fn token_at_abs(&self, abs: usize) -> Option<TokenView<'a, 's>> {
        let end = (self.rec().end as usize).min(self.cap as usize);
        ((self.rec().first as usize..end).contains(&abs)).then_some(TokenView {
            p: self.p,
            idx: abs as u32,
            cap: self.cap,
        })
    }
}

impl<'a, 's> TokenView<'a, 's> {
    /// Typed position of this token.
    #[inline]
    pub(crate) fn pos(&self) -> Pos {
        Pos::from_index(self.idx as usize)
    }

    /// Token kind.
    #[inline]
    pub(crate) fn kind(&self) -> OrgSyntaxKind {
        self.p.token_kind(self.idx as usize)
    }

    /// Full-tile source text (always the whole lex tile at this index).
    ///
    /// Not mid-tile-aware: when the cursor sits mid-tile on this token,
    /// [`Parser::text`] returns only the unconsumed remainder. Prefer that for
    /// cursor-local object PRE/POST text questions.
    pub(crate) fn text(&self) -> &'s str {
        let r = self.p.token_range(self.idx as usize);
        &self.p.source()[r]
    }

    /// View of the `k`-th token counted from this one (`0` = self).
    ///
    /// `None` past the observation limit or the token stream.
    pub(crate) fn nth(&self, k: usize) -> Option<TokenView<'a, 's>> {
        let abs = (self.idx as usize).checked_add(k)?;
        (abs < (self.cap as usize).min(self.p.token_count())).then_some(TokenView {
            p: self.p,
            idx: abs as u32,
            cap: self.cap,
        })
    }

    /// Kind of the `k`-th token from this view; `EOF` past end or past the
    /// visible limit.
    ///
    /// Prefer [`Self::run_while`] for consecutive-token runs: it is bounded by
    /// [`Self::nth`] and cannot hang. `EOF` here remains useful for single-shot
    /// probes after a measured run.
    #[inline]
    pub(crate) fn kind_at(&self, k: usize) -> OrgSyntaxKind {
        self.nth(k).map_or(OrgSyntaxKind::EOF, |t| t.kind())
    }

    /// Text of the `k`-th token from this view; empty past end or past the
    /// visible limit.
    #[inline]
    pub(crate) fn text_at(&self, k: usize) -> &'s str {
        self.nth(k).map_or("", |t| t.text())
    }

    /// Count consecutive tokens from relative offset `k` satisfying `pred`.
    ///
    /// Bounded by [`Self::nth`] (token stream / active limit). Cannot hang:
    /// past-end tokens are never offered to `pred`.
    pub(crate) fn run_while(
        &self,
        k: usize,
        mut pred: impl FnMut(OrgSyntaxKind, &str) -> bool,
    ) -> usize {
        let mut n = 0usize;
        while let Some(t) = self.nth(k + n) {
            if !pred(t.kind(), t.text()) {
                break;
            }
            n += 1;
        }
        n
    }

    /// Whether consecutive tokens from this view are a **prefix match** of
    /// `kinds`. Empty `kinds` is always true. A longer `kinds` than the
    /// visible window fails (past end answers `EOF`).
    #[inline]
    pub(crate) fn seq(&self, kinds: &[OrgSyntaxKind]) -> bool {
        kinds.iter().enumerate().all(|(i, &k)| self.kind_at(i) == k)
    }

    /// Whether the next `n` tokens from this view are all `kind` (`n > 0`).
    ///
    /// Shared by run-of-N closers (`>>` / `>>>` targets, bracket `]]`). False
    /// when `n == 0` or any tile is past the observation limit.
    #[inline]
    pub(crate) fn run_of(&self, kind: OrgSyntaxKind, n: usize) -> bool {
        n > 0 && (0..n).all(|i| self.kind_at(i) == kind)
    }

    /// The first *visible* source byte after this token.
    ///
    /// `None` when the next token is at/past the observation limit or the
    /// source ends: a region end is not a byte. POST-style checks accept
    /// `None` as end-of-region.
    pub(crate) fn next_byte(&self) -> Option<u8> {
        let next = (self.idx as usize) + 1;
        if next >= self.cap as usize {
            return None;
        }
        let end = usize::from(self.p.token_range(self.idx as usize).end());
        (end < self.p.source().len()).then(|| self.p.source().as_bytes()[end])
    }

    /// Whether the previous token is `WHITESPACE` (narrow: not INDENT/NEWLINE).
    pub(crate) fn after_space(&self) -> bool {
        self.prev_kind() == Some(OrgSyntaxKind::WHITESPACE)
    }

    /// The next token under the limit (`nth(1)`).
    pub(crate) fn next(&self) -> Option<TokenView<'a, 's>> {
        self.nth(1)
    }

    /// Exclusive end of this token as a region bound (the position after it).
    ///
    /// Use when a scan's result token should be *inside* the capped region:
    /// `p.with_limit_at(closer.end(), ...)` includes the closer.
    ///
    /// Unlike [`LineView::end`] this is not clamped to the active limit, so
    /// it depends only on the token. The view is already under the limit, and
    /// [`Parser::with_limit_at`] clamps anyway.
    #[inline]
    pub(crate) fn end(&self) -> Pos {
        Pos::from_index(self.idx as usize + 1)
    }

    /// The previous token, if any (escape-run lookbehind).
    pub(crate) fn prev(&self) -> Option<TokenView<'a, 's>> {
        let prev = (self.idx as usize).checked_sub(1)?;
        Some(TokenView {
            p: self.p,
            idx: prev as u32,
            cap: self.cap,
        })
    }

    fn prev_kind(&self) -> Option<OrgSyntaxKind> {
        let prev = (self.idx as usize).checked_sub(1)?;
        Some(self.p.token_kind(prev))
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::make_parser;

    #[test]
    fn here_matches_token_view_at_cursor() {
        let p = make_parser("[[path]]");
        let via_here = p.here().expect("here");
        let via_tv = p.token_view(p.cursor()).expect("token_view");
        assert_eq!(via_here.pos(), via_tv.pos());
        assert_eq!(via_here.kind(), via_tv.kind());
        assert_eq!(via_here.text(), via_tv.text());

        // Empty cap at the cursor: no live token under the limit.
        let mut p2 = make_parser("x");
        let here = p2.cursor();
        p2.with_limit_at(here, |pp| {
            assert!(pp.here().is_none());
            assert!(pp.token_view(pp.cursor()).is_none());
        });
    }

    #[test]
    fn token_view_nth_kind_at_text_at() {
        let p = make_parser("[[path]]");
        let t = p.token_view(p.cursor()).expect("cursor token");
        assert_eq!(t.kind_at(0), t.kind());
        assert_eq!(t.text_at(0), t.text());
        assert_eq!(t.nth(0).map(|v| v.kind()), Some(OrgSyntaxKind::L_BRACKET));
        assert_eq!(t.nth(1).map(|v| v.kind()), Some(OrgSyntaxKind::L_BRACKET));
        assert_eq!(t.kind_at(0), OrgSyntaxKind::L_BRACKET);
        assert_eq!(t.kind_at(1), OrgSyntaxKind::L_BRACKET);
        assert_eq!(t.kind_at(2), OrgSyntaxKind::TEXT);
        assert_eq!(t.text_at(2), "path");
        assert_eq!(t.kind_at(4), OrgSyntaxKind::R_BRACKET);
        // Exact end of stream: five tokens 0..4, index 5 is past end.
        assert_eq!(t.kind_at(5), OrgSyntaxKind::EOF);
        assert_eq!(t.text_at(5), "");
        assert!(t.nth(5).is_none());
        // Far past the stream.
        assert_eq!(t.kind_at(100), OrgSyntaxKind::EOF);
        assert_eq!(t.text_at(100), "");
        assert!(t.nth(100).is_none());
        // next() is nth(1); re-root offsets from nth(k) align with kind_at(k).
        assert_eq!(t.next().map(|v| v.pos()), t.nth(1).map(|v| v.pos()));
        let path = t.nth(2).expect("path TEXT");
        assert_eq!(path.kind_at(0), t.kind_at(2));
        assert_eq!(path.text_at(0), t.text_at(2));
        assert_eq!(path.kind(), OrgSyntaxKind::TEXT);
        assert_eq!(t.nth(1).expect("second [").kind_at(0), t.kind_at(1));
    }

    #[test]
    fn run_while_token_and_line_views() {
        use OrgSyntaxKind::{HASH, TEXT, WHITESPACE};
        // --- TokenView ---
        let p = make_parser("aa bb]");
        let t = p.here().expect("cursor");
        let n = t.run_while(0, |k, _| k == TEXT || k == WHITESPACE);
        assert_eq!(n, 3); // aa, space, bb
        assert_eq!(t.kind_at(n), OrgSyntaxKind::R_BRACKET);
        // Empty run when predicate fails immediately.
        assert_eq!(t.run_while(0, |k, _| k == HASH), 0);
        // run_of: first tile is TEXT; next is WHITESPACE so run of 2 TEXT fails.
        assert!(t.run_of(TEXT, 1));
        assert!(!t.run_of(TEXT, 2));
        assert!(!t.run_of(HASH, 1));
        assert!(!t.run_of(TEXT, 0), "n==0 is never a run");
        assert!(t.nth(3).expect("]").run_of(OrgSyntaxKind::R_BRACKET, 1));
        // Mid-offset run.
        assert_eq!(t.run_while(1, |k, _| k == WHITESPACE), 1);
        // Past end: never offers tokens.
        assert_eq!(t.run_while(100, |_, _| true), 0);
        // Text-using predicate (length of TEXT tiles only).
        assert_eq!(t.run_while(0, |k, s| k == TEXT && s == "aa"), 1);

        // Under a limit: TokenView run stops at the cap.
        let mut p_lim = make_parser("aa bb cc");
        let cap = p_lim
            .here()
            .and_then(|t| t.nth(2))
            .expect("third token")
            .pos();
        p_lim.with_limit_at(cap, |pp| {
            let t = pp.here().expect("cursor under cap");
            let n = t.run_while(0, |_, _| true);
            assert_eq!(n, 2); // only first two tiles visible
            assert_eq!(t.kind_at(n), OrgSyntaxKind::EOF);
            // run_of past the cap fails (second TEXT is hidden).
            assert!(t.run_of(TEXT, 1));
            assert!(!t.run_of(TEXT, 2));
        });

        // run_of n=2 success and limit bisect on identical-kind run.
        let mut p_run = make_parser("]]x");
        let t_run = p_run.here().expect("]]");
        assert!(t_run.run_of(OrgSyntaxKind::R_BRACKET, 2));
        assert!(!t_run.run_of(OrgSyntaxKind::R_BRACKET, 3));
        let after_one = t_run.nth(1).expect("second ]").pos();
        p_run.with_limit_at(after_one, |pp| {
            let t = pp.here().expect("cursor");
            assert!(t.run_of(OrgSyntaxKind::R_BRACKET, 1));
            assert!(
                !t.run_of(OrgSyntaxKind::R_BRACKET, 2),
                "cap hiding second ] must fail run_of(2)"
            );
        });

        // --- LineView (symmetric cases) ---
        let p2 = make_parser(":LOGBOOK:\n");
        let line = p2.line().expect("line");
        assert_eq!(line.kind(0), OrgSyntaxKind::COLON);
        // Full name atom run.
        let name = line.run_while(1, crate::parser::grammar::drive::atoms::is_name_atom);
        assert_eq!(name, 1);
        assert_eq!(line.text(1), "LOGBOOK");
        assert_eq!(line.kind(1 + name), OrgSyntaxKind::COLON);
        // Empty at a stop token.
        assert_eq!(line.run_while(0, |k, _| k == TEXT), 0);
        // Mid-offset: from the name TEXT only.
        assert_eq!(
            line.run_while(1, |k, s| k == TEXT && s.starts_with("LOG")),
            1
        );
        // Past end.
        assert_eq!(line.run_while(100, |_, _| true), 0);
        // Under a mid-line limit: only tiles before the cap are visible.
        // "aa bb\n" content = TEXT WHITESPACE TEXT NEWLINE; cap exclusive at "bb".
        let mut p3 = make_parser("aa bb\ncc\n");
        let line0 = p3.line().expect("l0");
        assert_eq!(line0.run_while(0, |_, _| true), 4, "uncapped full line");
        let mid = line0.nth(2).expect("TEXT bb").pos();
        p3.with_limit_at(mid, |pp| {
            let line = pp.line().expect("line under mid-line cap");
            let n = line.run_while(0, |_, _| true);
            assert_eq!(n, 2); // only "aa" and space
            assert_eq!(line.kind(n), OrgSyntaxKind::EOF);
            assert_eq!(line.kind(0), TEXT);
            assert_eq!(line.kind(1), WHITESPACE);
        });
    }

    #[test]
    fn line_view_seq_and_text_eq_caseless() {
        use OrgSyntaxKind::{COLON, HASH, PLUS, TEXT, WHITESPACE};
        let p = make_parser("#+TBLFM: x\n");
        let line = p.line().expect("line");
        // Prefix match (not full-line equality).
        assert!(line.seq(&[HASH, PLUS, TEXT]));
        assert!(line.seq(&[HASH, PLUS, TEXT, COLON, WHITESPACE]));
        assert!(line.seq(&[]));
        // Mismatch and over-long.
        assert!(!line.seq(&[HASH, PLUS, TEXT, TEXT]));
        assert!(!line.seq(&[HASH, TEXT]));
        // Caseless matrix.
        assert!(line.text_eq_caseless(2, "tblfm"));
        assert!(line.text_eq_caseless(2, "TBLFM"));
        assert!(line.text_eq_caseless(2, "TblFm"));
        assert!(!line.text_eq_caseless(2, "title"));
        // OOB: empty text; empty s matches, non-empty does not.
        assert!(line.text_eq_caseless(100, ""));
        assert!(!line.text_eq_caseless(100, "x"));

        // Limit: seq fails when the required tail is capped away.
        let mut p2 = make_parser("#+TITLE: hi\n");
        let after_hash = p2.line().and_then(|l| l.nth(1)).expect("PLUS").pos();
        p2.with_limit_at(after_hash, |pp| {
            let line = pp.line().expect("line");
            assert!(line.seq(&[HASH]));
            assert!(!line.seq(&[HASH, PLUS]));
        });
    }

    #[test]
    fn token_view_seq_and_limit() {
        use OrgSyntaxKind::{L_BRACKET, R_BRACKET, TEXT};
        let p = make_parser("[[x]]");
        let t = p.token_view(p.cursor()).expect("cursor token");
        assert!(t.seq(&[L_BRACKET, L_BRACKET, TEXT, R_BRACKET, R_BRACKET]));
        assert!(t.seq(&[]));
        assert!(!t.seq(&[L_BRACKET, TEXT]));
        assert!(!t.seq(&[L_BRACKET, L_BRACKET, TEXT, R_BRACKET, R_BRACKET, TEXT]));

        // Under a cap that hides the path, kind_at/text_at/seq stop at the limit.
        let mut p2 = make_parser("[[x]]");
        // Cap before the second `[` (exclusive end = index of second bracket).
        let second = p2
            .token_view(p2.cursor())
            .and_then(|v| v.nth(1))
            .expect("second bracket")
            .pos();
        p2.with_limit_at(second, |p| {
            let t = p.token_view(p.cursor()).expect("cursor under limit");
            assert_eq!(t.kind_at(0), t.kind());
            assert_eq!(t.kind_at(0), L_BRACKET);
            assert_eq!(t.kind_at(1), OrgSyntaxKind::EOF);
            assert_eq!(t.text_at(1), "");
            assert!(t.seq(&[L_BRACKET]));
            assert!(!t.seq(&[L_BRACKET, L_BRACKET]));
            assert!(t.nth(1).is_none());
        });

        // `seq` is literal: it never auto-skips leading INDENT. Grammar probes
        // that want the content tile go through `LineView::content`.
        let p3 = make_parser("   #+FOO: bar\n");
        let indent = p3.token_view(p3.cursor()).expect("indent");
        assert_eq!(indent.kind(), OrgSyntaxKind::INDENT);
        assert!(!indent.seq(&[OrgSyntaxKind::HASH, OrgSyntaxKind::PLUS]));
        assert!(
            indent
                .nth(1)
                .expect("hash")
                .seq(&[OrgSyntaxKind::HASH, OrgSyntaxKind::PLUS])
        );
    }

    #[test]
    fn tokens_from_cursor_stops_at_the_limit() {
        let src = "* a * b *";
        let p = make_parser(src);
        assert_eq!(p.current(), OrgSyntaxKind::STAR);
        let second = p
            .tokens_from_cursor()
            .skip(1)
            .find(|t| t.kind() == OrgSyntaxKind::STAR)
            .expect("second star");

        let mut p2 = make_parser(src);
        p2.with_limit_at(second.pos(), |p| {
            p.eat_any(); // first *
            p.eat_any(); // ws
            assert!(
                !p.tokens_from_cursor()
                    .any(|t| t.kind() == OrgSyntaxKind::STAR),
                "views inside limit must not see the second star"
            );
        });

        // Cap exactly at the cursor: the iterator is empty.
        let mut p3 = make_parser(src);
        let here = p3.cursor();
        p3.with_limit_at(here, |p| {
            assert!(
                p.tokens_from_cursor().next().is_none(),
                "no views past an empty cap"
            );
        });
    }

    #[test]
    fn token_view_accessors_are_pure() {
        use OrgSyntaxKind::{COLON, EQUALS, R_ANGLE, TEXT, WHITESPACE};
        let p = make_parser("=> 1:00");
        let before = p.cursor();
        let t = p.token_view(p.cursor()).expect("cursor token");

        // Return values (not discarded) and identity at offset 0.
        assert_eq!(t.kind_at(0), t.kind());
        assert_eq!(t.kind(), EQUALS);
        assert_eq!(t.text_at(0), t.text());
        assert_eq!(t.text_at(3), "1");
        assert_eq!(t.kind_at(5), TEXT);
        assert_eq!(t.text_at(5), "00");
        // Exact end pin: six tokens 0..5, index 6 is EOF.
        assert_eq!(t.kind_at(6), OrgSyntaxKind::EOF);
        assert!(t.nth(6).is_none());
        assert!(t.seq(&[EQUALS, R_ANGLE, WHITESPACE, TEXT, COLON, TEXT]));

        // nth re-root: offsets from nth(k) match original kind_at(k + j).
        let hour = t.nth(3).expect("hour TEXT");
        assert_eq!(hour.kind_at(0), t.kind_at(3));
        assert_eq!(hour.kind(), TEXT);
        assert_eq!(hour.kind_at(1), COLON);
        assert_eq!(hour.kind_at(2), TEXT);
        assert_eq!(t.nth(1).expect("R_ANGLE").kind_at(0), t.kind_at(1));

        assert_eq!(
            p.cursor(),
            before,
            "TokenView accessors must not move cursor"
        );
    }

    #[test]
    fn token_view_next_byte_some_none_and_limit() {
        // Positive: next source byte after STAR is space.
        let p = make_parser("* a");
        let star = p.token_view(p.cursor()).expect("STAR");
        assert_eq!(star.kind(), OrgSyntaxKind::STAR);
        assert_eq!(star.next_byte(), Some(b' '));
        // Mid-line: after TEXT "a" at end of source -> None.
        let a = star.nth(2).expect("TEXT a");
        assert_eq!(a.kind(), OrgSyntaxKind::TEXT);
        assert_eq!(a.next_byte(), None, "true EOF after last tile");

        // Cap before trailing TEXT: next_byte is None even though outer source
        // still has the letter (region end is not a byte).
        let mut p2 = make_parser("*x");
        // Tokens: STAR TEXT("x"); cap at TEXT.
        let x = p2
            .token_view(p2.cursor())
            .and_then(|t| t.nth(1))
            .expect("x")
            .pos();
        assert_eq!(
            p2.token_view(p2.cursor()).expect("STAR").next_byte(),
            Some(b'x'),
            "unbounded POST sees following tile"
        );
        p2.with_limit_at(x, |pp| {
            let star = pp.token_view(pp.cursor()).expect("STAR under cap");
            assert_eq!(star.next_byte(), None, "cap hides the outer byte");
        });
    }

    #[test]
    fn token_view_after_space_is_whitespace_only() {
        // After WHITESPACE: true.
        let mut p = make_parser("a :");
        p.eat_any(); // TEXT
        p.eat_any(); // WHITESPACE
        assert_eq!(p.current(), OrgSyntaxKind::COLON);
        assert!(p.token_view(p.cursor()).expect("colon").after_space());

        // After INDENT: false (narrow).
        let mut ind = make_parser("  x");
        ind.eat_any();
        assert!(!ind.token_view(ind.cursor()).expect("x").after_space());

        // After NEWLINE: false (narrow).
        let mut nl = make_parser("a\nb");
        nl.eat_any();
        nl.eat_any();
        assert!(!nl.token_view(nl.cursor()).expect("b").after_space());

        // First token: no previous -> false.
        let p0 = make_parser("x");
        assert!(!p0.token_view(p0.cursor()).expect("x").after_space());
    }
}
