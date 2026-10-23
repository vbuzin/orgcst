//! Consumption, backtracking, and region limiting.
//!
//! `eat_*` advances the cursor and emits token events under the open node.
//! `leaf` / `leaf_len` / `leaf_to` open, fill and complete a leaf node in one
//! call. Byte-measured ends may land inside a lex tile (`eat_to` / `leaf_len`);
//! the parent loop then emits the remainder of that tile. Token-bounded ends
//! use `eat_until`. [`Parser::attempt`] runs an alternative and rewinds
//! everything but diagnostics if it fails. A rejected `eat_to` / `leaf_len`
//! changes neither the cursor nor the events.

use super::Parser;
use crate::syntax::OrgSyntaxKind;
use rowan::{TextRange, TextSize};

/// Parser state saved before an alternative, so a failed one can be rewound.
///
/// The lex tokens never change during a parse, so they are not saved. Region
/// nesting is saved so an `attempt` that crosses a nested region cannot leave
/// the counter wrong. Outline nesting is not: outlines are only opened and
/// closed by the document loop, never inside an `attempt`.
#[derive(Clone, Copy)]
pub(crate) struct Checkpoint {
    pub(super) pos: usize,
    /// Visible to tests in descendant modules that assert progress.
    pub(crate) events_len: usize,
    pub(super) limit: usize,
    /// Copy of [`Parser::inline_region_start`] at capture.
    pub(super) inline_region_start: Option<usize>,
    /// Copy of [`Parser::nesting`] at capture (region budget counter).
    pub(super) nesting: u16,
    /// Mid-token byte offset into the current lex tile at capture.
    pub(super) token_byte_off: usize,
}

impl Parser<'_> {
    /// Absolute start of the unconsumed remainder of the tile at `pos`.
    #[inline]
    fn remainder_start(&self, tok_start: usize) -> usize {
        tok_start + self.token_byte_off
    }

    /// Opaque key that changes whenever source is consumed.
    ///
    /// A rule that claims to have parsed something must change it. The token
    /// index alone is not enough when a measured leaf ends mid-tile without
    /// advancing `pos`.
    #[inline]
    pub(crate) fn progress(&self) -> super::Progress {
        super::Progress::new(self.pos, self.token_byte_off)
    }

    /// Consume source from the cursor through exclusive absolute **byte**
    /// `end_byte` (may end mid-tile).
    ///
    /// Do not confuse with [`Self::eat_until`]: that takes a token [`Pos`] and
    /// always stops on a token boundary. This method is for measured byte ends
    /// (plain-link paths, entities) and can leave the cursor mid-tile.
    ///
    /// Emits one or more [`super::Event::Token`] pieces covering that span.
    /// When `end_byte` falls inside a lex tile, the cursor stays mid-tile at
    /// that end (parent loop emits the remainder later).
    ///
    /// Returns `false` if `end_byte` is at/before the cursor, past the visible
    /// source end, not a UTF-8 char boundary, or no token intersects the span.
    /// On `false`, cursor and events are unchanged.
    #[must_use]
    pub(crate) fn eat_to(&mut self, end_byte: usize) -> bool {
        let Some(start) = self.source_start_here() else {
            return false;
        };
        let vis_end = self.visible_source_end();
        if end_byte <= start || end_byte > vis_end {
            return false;
        }
        if !self.source.is_char_boundary(end_byte) {
            return false;
        }

        let mut progressed = false;
        while self.current() != OrgSyntaxKind::EOF {
            let i = self.pos;
            let r = self.tokens[i].range;
            let tok_start = usize::from(r.start());
            let tok_end = usize::from(r.end());
            let cur_start = self.remainder_start(tok_start);
            debug_assert!(cur_start <= tok_end);
            debug_assert!(cur_start < tok_end || self.token_byte_off == 0);

            if tok_end <= end_byte {
                // Remainder of this tile falls entirely inside the span.
                if cur_start < tok_end {
                    self.push_token_range(self.tokens[i].kind, cur_start, tok_end);
                    progressed = true;
                }
                self.pos += 1;
                self.token_byte_off = 0;
                continue;
            }
            if cur_start < end_byte {
                // Mid-tile exclusive end: emit left piece, leave cursor there.
                debug_assert!(end_byte < tok_end);
                self.push_token_range(self.tokens[i].kind, cur_start, end_byte);
                self.token_byte_off = end_byte - tok_start;
                progressed = true;
            }
            break;
        }
        progressed
    }

    /// Open and complete a leaf of `kind` covering the next `len` source bytes
    /// from the cursor.
    ///
    /// For ends that may fall mid-tile (plain-link paths, entities). Prefer
    /// [`Self::leaf`] for whole-token counts. Rejects on absolute-end overflow
    /// or when [`Self::eat_to`] would reject; on reject the leaf is abandoned and
    /// cursor/events are unchanged.
    #[must_use]
    pub(crate) fn leaf_len(&mut self, kind: OrgSyntaxKind, len: usize) -> bool {
        let Some(start) = self.source_start_here() else {
            return false;
        };
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        let m = self.start();
        if !self.eat_to(end) {
            m.abandon(self);
            return false;
        }
        m.complete(self, kind);
        true
    }

    /// Cap the visible window at exclusive absolute token index `end`, run `f`,
    /// restore. Backs [`Self::with_limit_at`].
    ///
    /// `end` is clamped to the current limit, so the window can only shrink.
    /// Mid-token ends never insert tokens, so restoring is a plain write-back.
    fn with_limit_abs<R>(&mut self, end: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let previous = self.limit;
        self.limit = end.min(previous);
        let result = f(self);
        self.limit = previous;
        result
    }

    /// Cap the visible window at exclusive position `end`, run `f`, restore.
    ///
    /// Positions are absolute, so a bound measured before consumption stays
    /// valid after `eat_line` / mid-region eats. Nesting only ever shrinks the
    /// window (clamped to the current limit at entry).
    #[inline]
    pub(crate) fn with_limit_at<R>(
        &mut self,
        end: super::Pos,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.with_limit_abs(end.index(), f)
    }

    /// Consume every visible token from the cursor up to exclusive token `end`.
    ///
    /// Do not confuse with [`Self::eat_to`]: that takes an absolute **byte**
    /// end and may leave the cursor mid-tile. This method is token-granular
    /// (`Pos`), always stops on a token boundary, and never rejects (no-op
    /// when the cursor is already at or past `end`, or past the active cap).
    ///
    /// Same effect as `with_limit_at(end, |p| p.eat_to_limit())` without
    /// changing the limit, and clamped to it. Use [`Self::eat_to_limit`] only
    /// when the visible window is already capped (e.g. under
    /// `with_rest_of_line` or a parent limit).
    #[inline]
    pub(crate) fn eat_until(&mut self, end: super::Pos) {
        let stop = end.index().min(self.limit);
        while self.pos < stop {
            self.eat_any();
        }
    }

    /// Open and complete a leaf of `kind` covering tokens from the cursor up
    /// to exclusive token `end`.
    ///
    /// Empty regions still emit the (empty) node (e.g. element CALL_NAME with
    /// no name tokens). Prefer [`Self::leaf`] for whole-token counts and
    /// [`Self::leaf_len`] for measured **byte** ends that may land mid-tile.
    #[inline]
    pub(crate) fn leaf_to(&mut self, kind: OrgSyntaxKind, end: super::Pos) {
        let m = self.start();
        self.eat_until(end);
        m.complete(self, kind);
    }

    /// Run `f`; on `None`, rewind cursor, events, limit, and inline region to
    /// the entry state (diagnostics stay sticky).
    ///
    /// Use for optional parts and alternatives that must leave no trace on
    /// failure. `f` must resolve any [`Marker`] it opened (complete or
    /// abandon) before returning `None`.
    ///
    /// [`Marker`]: super::event::Marker
    #[inline]
    pub(crate) fn attempt<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let cp = self.save();
        let result = f(self);
        if result.is_none() {
            self.restore(cp);
        }
        result
    }

    /// Consume every visible token up to the active limit.
    ///
    /// Use when the visible window is already capped (under `with_limit_at` /
    /// `with_rest_of_line`, or a parent region). To consume up to a measured
    /// `Pos`, prefer [`Self::eat_until`].
    #[inline]
    pub(crate) fn eat_to_limit(&mut self) {
        while self.current() != OrgSyntaxKind::EOF {
            self.eat_any();
        }
    }

    /// Consume zero or more blank lines under the currently open marker.
    ///
    /// Body elements call this before `Marker::complete` so trailing blanks
    /// sit under that element. Mid-token safe: `at_blank` is false mid-tile.
    pub(crate) fn eat_blank_lines(&mut self) {
        while self.at_blank() {
            if self.eat_line() == 0 {
                break;
            }
        }
    }

    /// Snapshot cursor, event length, active limit, inline region start,
    /// region nesting, and mid-token offset for a later
    /// [`restore`](Self::restore).
    fn save(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            events_len: self.events.len(),
            limit: self.limit,
            inline_region_start: self.inline_region_start,
            nesting: self.nesting,
            token_byte_off: self.token_byte_off,
        }
    }

    /// Restore a prior [`save`](Self::save).
    ///
    /// Restores cursor (including mid-token offset), events, limit, inline
    /// region start, and region nesting. Diagnostics are not rewound; see
    /// `parser::diagnostic::emit`.
    fn restore(&mut self, cp: Checkpoint) {
        self.pos = cp.pos;
        self.events.truncate(cp.events_len);
        self.limit = cp.limit;
        self.inline_region_start = cp.inline_region_start;
        self.nesting = cp.nesting;
        self.token_byte_off = cp.token_byte_off;
    }

    /// Test access to [`Self::save`]; parser code uses [`Self::attempt`].
    #[inline]
    #[cfg(test)]
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.save()
    }

    /// Test access to [`Self::restore`]; parser code uses [`Self::attempt`].
    #[inline]
    #[cfg(test)]
    pub(crate) fn rewind(&mut self, cp: Checkpoint) {
        self.restore(cp);
    }

    /// Run `f` with the inline region start temporarily set to `start`.
    /// Restores the previous value afterwards (not unwind-safe).
    ///
    /// Object walks in [`super::grammar::drive::region`] set it to
    /// [`Self::cursor`] so [`Self::at_region_start`] accepts openers at the
    /// region's first token.
    pub(crate) fn with_inline_region_start<R>(
        &mut self,
        start: Option<super::Pos>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = self.inline_region_start;
        self.inline_region_start = start.map(super::pos::Pos::index);
        let result = f(self);
        self.inline_region_start = previous;
        result
    }

    /// Emit a `Token` event for the remainder of the token at the cursor and
    /// advance to the next tile.
    ///
    /// When the cursor sits mid-tile, only the unconsumed suffix is emitted.
    ///
    /// # Panics
    ///
    /// Panics when the cursor is at or past the active limit. Grammar rules
    /// must check `current()` before calling.
    pub(crate) fn eat_any(&mut self) {
        assert!(
            self.pos < self.limit,
            "eat_any past the visible limit: grammar rules must check before consuming"
        );
        // The limit never exceeds `tokens.len()`.
        let token = self.tokens[self.pos];
        let tok_start = usize::from(token.range.start());
        let tok_end = usize::from(token.range.end());
        let cur_start = self.remainder_start(tok_start);
        // An empty remainder is a grammar bug.
        assert!(
            cur_start < tok_end,
            "eat_any with empty remainder (token_byte_off past tile end)"
        );
        self.push_token_range(token.kind, cur_start, tok_end);
        self.pos += 1;
        self.token_byte_off = 0;
    }

    /// Push one range-bearing token event (no cursor change).
    #[inline]
    fn push_token_range(&mut self, kind: OrgSyntaxKind, start: usize, end: usize) {
        debug_assert!(start < end);
        debug_assert!(self.source.is_char_boundary(start));
        debug_assert!(self.source.is_char_boundary(end));
        self.events.push(super::Event::Token {
            kind,
            range: TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32)),
        });
    }

    /// Consume exactly `n` token remainders via [`eat_any`](Self::eat_any).
    ///
    /// Caller must ensure `n` tiles (from the cursor, counting a mid-tile
    /// remainder as one) are visible under the active limit.
    pub(crate) fn eat_n(&mut self, n: usize) {
        for _ in 0..n {
            self.eat_any();
        }
    }

    /// Open and complete a leaf of `kind` covering the next `n` whole-tile remainders.
    ///
    /// Prefer this when the leaf ends on a token boundary (including a mid-tile
    /// remainder as one). For measured source-byte ends, use [`Self::leaf_len`].
    pub(crate) fn leaf(&mut self, kind: OrgSyntaxKind, n: usize) {
        let m = self.start();
        self.eat_n(n);
        m.complete(self, kind);
    }

    /// Consume a run of tokens while `current() == kind`. Returns count consumed.
    pub(crate) fn eat_run(&mut self, kind: OrgSyntaxKind) -> usize {
        self.eat_while(|p| p.current() == kind)
    }

    /// Consume tokens until a line terminator (`NEWLINE` or visible `EOF`).
    ///
    /// Does not consume the terminator. Returns the count of tokens eaten (0
    /// when already at the line end under the active limit).
    #[inline]
    pub(crate) fn eat_to_line_end(&mut self) -> usize {
        self.eat_while(|p| !p.at_line_term())
    }

    /// Consume tokens while `pred` holds, stopping at the active limit.
    ///
    /// Returns the count of tokens consumed. `pred` receives `&self`, so it
    /// can only observe.
    pub(crate) fn eat_while(&mut self, mut pred: impl FnMut(&Self) -> bool) -> usize {
        let mut cnt = 0usize;
        while self.current() != OrgSyntaxKind::EOF && pred(self) {
            self.eat_any();
            cnt += 1;
        }
        cnt
    }

    /// Consume the current token if it has `kind`. Returns whether it did.
    pub(crate) fn eat(&mut self, kind: OrgSyntaxKind) -> bool {
        if self.current() == kind {
            self.eat_any();
            true
        } else {
            false
        }
    }

    /// Consume one logical line if the cursor is at that line's first token.
    ///
    /// If not at line start (including mid-tile), returns 0 and does nothing.
    /// Otherwise eats every token through the line end (including NEWLINE when
    /// present), clamped by the active limit. Returns the number of lines
    /// consumed (0 or 1).
    pub(crate) fn eat_line(&mut self) -> usize {
        let Some(line) = self.line() else {
            return 0;
        };
        let end = line.end();
        if self.pos >= end.index() {
            return 0;
        }
        self.eat_until(end);
        1
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use crate::lexer::lex;
    use crate::parser::{Event, Parser};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{make_parser, node_text, pos_ahead};

    fn token_ranges(events: &[Event]) -> Vec<(u32, u32)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Token { range, .. } => {
                    Some((u32::from(range.start()), u32::from(range.end())))
                }
                _ => None,
            })
            .collect()
    }

    fn token_texts<'a>(events: &'a [Event], src: &'a str) -> Vec<&'a str> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Token { range, .. } => {
                    let s = usize::from(range.start());
                    let e = usize::from(range.end());
                    Some(&src[s..e])
                }
                _ => None,
            })
            .collect()
    }

    /// Test helper: length form of [`Parser::eat_to`] from the cursor.
    fn eat_to_len(p: &mut Parser<'_>, len: usize) -> bool {
        let Some(start) = p.source_start_here() else {
            return false;
        };
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        p.eat_to(end)
    }

    /// A plain link: `leaf_len` ends inside a tile, leaving the cursor on the
    /// trailing `.`.
    #[test]
    fn leaf_len_mid_text_leaves_cursor_on_trail() {
        let src = "https://x.com.";
        let mut p = make_parser(src);
        let path_len = "https://x.com".len();
        let m = p.start();
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, path_len));
        m.complete(&mut p, OrgSyntaxKind::LINK);
        assert_eq!(p.text(), ".");
        let host_tile_start = src.find("x.com.").expect("host tile");
        assert_eq!(p.progress().mid_offset(), path_len - host_tile_start);
        let (toks, events, _) = p.finish();
        assert_eq!(toks.len(), lex(src).tokens.len());
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("https://x.com")
        );
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK).as_deref(),
            Some("https://x.com")
        );
    }

    #[test]
    fn leaf_len_under_document_roundtrips_with_parent_trail() {
        let src = "https://x.com. more";
        let mut p = make_parser(src);
        let doc = p.start();
        let path_len = "https://x.com".len();
        let m = p.start();
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, path_len));
        m.complete(&mut p, OrgSyntaxKind::LINK);
        // Parent loop: remainder trail + following tokens as bare text.
        while p.current() != OrgSyntaxKind::EOF {
            p.eat_any();
        }
        doc.complete(&mut p, OrgSyntaxKind::DOCUMENT);
        let (toks, events, _) = p.finish();
        let green = crate::parser::builder::build(src, &toks, events);
        let root = crate::SyntaxNode::new_root(green);
        assert_eq!(root.text().to_string(), src);
        let path = root
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
            .expect("LINK_PATH");
        assert_eq!(path.text().to_string(), "https://x.com");
        let link = root
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK)
            .expect("LINK");
        assert_eq!(link.text().to_string(), "https://x.com");
        // Trail is bare DOCUMENT child immediately after LINK.
        let mut after_link = root
            .children_with_tokens()
            .skip_while(|c| c.as_node().is_none_or(|n| n.kind() != OrgSyntaxKind::LINK));
        let _ = after_link.next().expect("LINK under DOCUMENT");
        assert_eq!(
            after_link.next().expect("trail after LINK").to_string(),
            ".",
            "mid-tile remainder must be bare DOCUMENT child exactly \".\""
        );
    }

    #[test]
    fn sequential_plain_paths_each_leave_own_trail() {
        let src = "https://a.com. https://b.com.";
        let mut p = make_parser(src);
        let doc = p.start();

        let m1 = p.start();
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, "https://a.com".len()));
        m1.complete(&mut p, OrgSyntaxKind::LINK);
        assert_eq!(p.text(), ".");
        p.eat_any(); // trail
        assert!(
            p.eat(OrgSyntaxKind::WHITESPACE),
            "whitespace between plain links"
        );

        let m2 = p.start();
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, "https://b.com".len()));
        m2.complete(&mut p, OrgSyntaxKind::LINK);
        assert_eq!(p.text(), ".");
        p.eat_any();

        doc.complete(&mut p, OrgSyntaxKind::DOCUMENT);
        let (toks, events, _) = p.finish();
        let green = crate::parser::builder::build(src, &toks, events);
        let root = crate::SyntaxNode::new_root(green);
        assert_eq!(root.text().to_string(), src);
        let links: Vec<_> = root
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::LINK)
            .collect();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].text().to_string(), "https://a.com");
        assert_eq!(links[1].text().to_string(), "https://b.com");
    }

    #[test]
    fn eat_to_len_on_token_boundary() {
        let src = "hello";
        let mut p = make_parser(src);
        let n_toks_before = lex(src).tokens.len();
        assert!(eat_to_len(&mut p, src.len()));
        assert_eq!(p.progress().mid_offset(), 0);
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        let (toks, events, _) = p.finish();
        assert_eq!(toks.len(), n_toks_before);
        assert!(!events.iter().any(|e| matches!(e, Event::Start { .. })));
        assert_eq!(token_texts(&events, src), ["hello"]);
    }

    #[test]
    fn eat_to_len_empty_does_not_advance() {
        let src = "abc";
        let mut p = make_parser(src);
        let key = p.progress();
        let events_before = p.checkpoint().events_len;
        assert!(!eat_to_len(&mut p, 0));
        assert_eq!(p.progress(), key, "reject is atomic on progress");
        assert_eq!(p.checkpoint().events_len, events_before);
    }

    #[test]
    fn eat_to_rejects_past_visible_and_under_limit() {
        let src = "aa bb";
        let mut p = make_parser(src);
        let cap = p
            .token_view(p.cursor())
            .and_then(|t| t.nth(1))
            .expect("ws")
            .pos();
        p.with_limit_at(cap, |pp| {
            assert!(eat_to_len(pp, 2)); // "aa"
            let key = pp.progress();
            let elen = pp.checkpoint().events_len;
            assert!(
                !eat_to_len(pp, 1),
                "past visible window under with_limit_at"
            );
            assert_eq!(pp.progress(), key);
            assert_eq!(pp.checkpoint().events_len, elen);
        });
        let mut p2 = make_parser(src);
        let key2 = p2.progress();
        let elen2 = p2.checkpoint().events_len;
        assert!(
            !eat_to_len(&mut p2, src.len() + 10),
            "past visible_source_end"
        );
        assert_eq!(p2.progress(), key2);
        assert_eq!(p2.checkpoint().events_len, elen2);
    }

    #[test]
    fn eat_to_rejects_mid_utf8_char_boundary() {
        let src = "caf\u{e9}.";
        let mut p = make_parser(src);
        let acute_start = src.find('\u{e9}').expect("acute");
        let mid = acute_start + 1;
        assert!(!src.is_char_boundary(mid));
        let key = p.progress();
        let elen = p.checkpoint().events_len;
        assert!(!eat_to_len(&mut p, mid));
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
    }

    #[test]
    fn eat_to_mid_utf8_success_then_parent_trail() {
        let src = "caf\u{e9}.";
        let mut p = make_parser(src);
        let doc = p.start();
        let end = src.len() - 1;
        assert!(src.is_char_boundary(end));
        assert!(p.eat_to(end));
        assert_eq!(p.text(), ".");
        p.eat_any();
        doc.complete(&mut p, OrgSyntaxKind::DOCUMENT);
        let (toks, events, _) = p.finish();
        assert_eq!(token_texts(&events, src).concat(), src);
        let green = crate::parser::builder::build(src, &toks, events);
        let root = crate::SyntaxNode::new_root(green);
        assert_eq!(root.text().to_string(), src);
    }

    #[test]
    fn leaf_len_reject_leaves_no_start_event() {
        let src = "abc";
        let mut p = make_parser(src);
        let events_before = p.checkpoint().events_len;
        let key = p.progress();
        assert!(!p.leaf_len(OrgSyntaxKind::LINK_PATH, 0));
        assert_eq!(p.progress(), key, "leaf_len reject keeps progress");
        let (_, events, _) = p.finish();
        assert_eq!(events.len(), events_before);
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::Start {
                kind: OrgSyntaxKind::LINK_PATH
            }
        )));
    }

    /// Locks length-from-cursor: after mid-tile `eat_to`, `leaf_len(1)` covers
    /// only the remainder (not absolute byte `1`).
    #[test]
    fn leaf_len_from_mid_tile_uses_length_not_absolute() {
        let src = "ab.";
        let mut p = make_parser(src);
        assert!(p.eat_to(2)); // remainder "."
        assert_eq!(p.text(), ".");
        assert_eq!(p.source_start_here(), Some(2));
        assert_eq!(p.progress().mid_offset(), 2);
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, 1));
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        assert_eq!(
            p.progress().mid_offset(),
            0,
            "mid-tile offset cleared after full remainder"
        );
        let (_, events, _) = p.finish();
        // Prefix "ab" is bare (from eat_to); LINK_PATH owns only ".".
        // If leaf_len wrongly used absolute end=len (byte 1), it would reject
        // (end <= start) or cover the wrong span.
        assert_eq!(token_texts(&events, src), ["ab", "."]);
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("."),
            "leaf_len must cover length-from-cursor remainder only"
        );
    }

    #[test]
    fn abandon_after_rejected_leaf_is_safe() {
        let src = "abc";
        let mut p = make_parser(src);
        let m = p.start();
        assert!(!p.leaf_len(OrgSyntaxKind::LINK_PATH, 0));
        m.abandon(&mut p);
        assert_eq!(p.checkpoint().events_len, 0);
        assert_ne!(p.current(), OrgSyntaxKind::EOF);
        p.eat_any();
        while p.current() != OrgSyntaxKind::EOF {
            p.eat_any();
        }
        let _ = p.finish();
    }

    #[test]
    fn eat_any_after_mid_token_emits_remainder_only() {
        let src = "ab.";
        let mut p = make_parser(src);
        assert!(p.eat_to(2)); // "ab"
        assert_eq!(p.progress().mid_offset(), 2);
        assert_eq!(p.text(), ".");
        p.eat_any();
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        assert_eq!(p.progress().mid_offset(), 0);
        let (_, events, _) = p.finish();
        assert_eq!(token_texts(&events, src), ["ab", "."]);
        assert_eq!(
            token_ranges(&events),
            [(0, 2), (2, 3)],
            "adjacent pieces tile the single lex tile"
        );
    }

    #[test]
    fn checkpoint_rewind_restores_cursor_and_events() {
        let src = "hello world";
        let mut p = make_parser(src);
        let n_toks = lex(src).tokens.len();
        let limit_before = p.limit().index();
        let cp = p.checkpoint();
        let pos_before = p.cursor().index();
        let events_before = cp.events_len;
        p.eat_any();
        // Open+close a node past the checkpoint; rewind truncates those events.
        // (Bare start() without complete/abandon trips the Marker drop bomb.)
        let m = p.start();
        m.complete(&mut p, OrgSyntaxKind::PARAGRAPH);
        assert!(p.cursor().index() > pos_before);
        p.rewind(cp);
        assert_eq!(p.cursor().index(), pos_before);
        assert_eq!(p.progress().mid_offset(), 0);
        assert_eq!(p.checkpoint().events_len, events_before);
        assert_eq!(p.limit().index(), limit_before, "limit restored");
        let (toks, _, _) = p.finish();
        assert_eq!(
            toks.len(),
            n_toks,
            "token vector length stable across rewind"
        );
    }

    #[test]
    fn checkpoint_rewind_restores_inline_region_start() {
        let mut p = make_parser("*b*");

        assert_eq!(p.inline_region_start, None);
        assert!(!p.at_region_start());
        let cp_none = p.checkpoint();
        p.inline_region_start = Some(0);
        assert!(p.at_region_start());
        p.rewind(cp_none);
        assert_eq!(p.inline_region_start, None);
        assert!(
            !p.at_region_start(),
            "rewind restores inactive region (None)"
        );

        p.inline_region_start = Some(0);
        assert!(p.at_region_start());
        let cp_some = p.checkpoint();
        p.inline_region_start = Some(1);
        assert!(!p.at_region_start());
        p.rewind(cp_some);
        assert_eq!(p.inline_region_start, Some(0));
        assert!(
            p.at_region_start(),
            "inline_region_start restored by rewind"
        );

        p.with_inline_region_start(None, |pp| {
            assert!(!pp.at_region_start());
        });
        assert!(p.at_region_start());
    }

    #[test]
    fn checkpoint_rewind_restores_region_nesting() {
        let mut p = make_parser("x");
        assert!(p.nesting_balanced());
        let cp = p.checkpoint();
        // Simulate a budgeted region that was entered without the scoped
        // helper (attempt can cross that boundary only via checkpoint).
        p.nesting = 3;
        assert!(!p.nesting_balanced());
        p.rewind(cp);
        assert!(
            p.nesting_balanced(),
            "region nesting counter restored by rewind"
        );
    }

    #[test]
    fn rewind_after_mid_token_eat_restores_clean_state() {
        let src = "https://x.com.";
        let mut p = make_parser(src);
        let n_toks = lex(src).tokens.len();
        let cp = p.checkpoint();
        assert!(eat_to_len(&mut p, "https://x.com".len()));
        assert!(p.progress().mid_offset() > 0);
        p.rewind(cp);
        assert_eq!(p.cursor().index(), 0);
        assert_eq!(
            p.progress().mid_offset(),
            0,
            "mid-token offset cleared on rewind"
        );
        assert_eq!(p.checkpoint().events_len, cp.events_len);

        // Parse it as a plain link; the parent consumes the trailing `.`.
        let doc = p.start();
        let m = p.start();
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, "https://x.com".len()));
        m.complete(&mut p, OrgSyntaxKind::LINK);
        while p.current() != OrgSyntaxKind::EOF {
            p.eat_any();
        }
        doc.complete(&mut p, OrgSyntaxKind::DOCUMENT);
        let (toks, events, _) = p.finish();
        assert_eq!(toks.len(), n_toks);
        let green = crate::parser::builder::build(src, &toks, events);
        let root = crate::SyntaxNode::new_root(green);
        assert_eq!(root.text().to_string(), src);
    }

    #[test]
    fn checkpoint_after_mid_token_restores_offset() {
        let src = "https://x.com.";
        let mut p = make_parser(src);
        let n_toks = lex(src).tokens.len();
        assert!(eat_to_len(&mut p, "https://x.com".len()));
        assert_eq!(p.limit().index(), n_toks, "no token inserts");
        let off = p.progress().mid_offset();
        assert!(off > 0);
        let cp = p.checkpoint();
        let pos = p.cursor().index();
        p.eat_any(); // drain trail
        // Emit structure past the checkpoint; rewind must restore mid-token off.
        let m = p.start();
        m.complete(&mut p, OrgSyntaxKind::PARAGRAPH);
        p.rewind(cp);
        assert_eq!(p.cursor().index(), pos);
        assert_eq!(p.progress().mid_offset(), off, "mid-token offset restored");
        assert_eq!(p.text(), ".");
        p.eat_any();
        let (toks, events, _) = p.finish();
        assert_eq!(toks.len(), n_toks);
        assert_eq!(token_texts(&events, src).last().copied(), Some("."));
    }

    #[test]
    fn plain_link_public_trail_is_sibling_of_link() {
        let src = "Visit https://x.com. next\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        let link = out
            .root()
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK)
            .expect("LINK");
        assert_eq!(link.text().to_string(), "https://x.com");
        let parent = link.parent().expect("parent");
        let mut after_link = parent
            .children_with_tokens()
            .skip_while(|c| c.as_node().is_none_or(|n| n.kind() != OrgSyntaxKind::LINK));
        let _link_elem = after_link.next().expect("LINK element");
        let trail = after_link.next().expect("token after LINK");
        assert!(
            trail.as_token().is_some(),
            "trail must be a bare token sibling, not a node"
        );
        assert_eq!(
            trail.to_string(),
            ".",
            "the trailing `.` is a sibling of the LINK"
        );
    }

    #[test]
    fn parse_document_plain_link_roundtrips() {
        let src = "https://x.com.\n";
        let mut p = make_parser(src);
        crate::parser::grammar::parse_document(&mut p);
        assert_eq!(p.progress().mid_offset(), 0, "document fully drained");
        let (toks, events, _) = p.finish();
        let green = crate::parser::builder::build(src, &toks, events);
        let root = crate::SyntaxNode::new_root(green);
        assert_eq!(root.kind(), OrgSyntaxKind::DOCUMENT);
        assert_eq!(root.text().to_string(), src);
        let link = root
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK)
            .expect("LINK");
        assert_eq!(link.text().to_string(), "https://x.com");
    }

    #[test]
    fn source_start_and_progress_mid_tile() {
        let src = "ab.";
        let mut p = make_parser(src);
        assert_eq!(p.source_start_here(), Some(0));
        assert_eq!(p.progress().mid_offset(), 0);
        assert!(p.eat_to(2));
        assert_eq!(p.source_start_here(), Some(2));
        assert_eq!(p.progress().mid_offset(), 2);
        // Absolute end of remainder equals source len.
        assert_eq!(
            p.source_start_here().expect("cur") + p.text().len(),
            src.len()
        );
    }

    #[test]
    fn eat_to_len_partial_then_eat_to_rest_of_tile() {
        // Two measured eats inside one tile: "hello." -> "hel" then "lo".
        let src = "hello.";
        let mut p = make_parser(src);
        assert!(eat_to_len(&mut p, 3));
        assert_eq!(p.text(), "lo.");
        assert!(eat_to_len(&mut p, 2));
        assert_eq!(p.text(), ".");
        assert_eq!(p.progress().mid_offset(), 5);
        p.eat_any();
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        let (_, events, _) = p.finish();
        assert_eq!(token_texts(&events, src), ["hel", "lo", "."]);
    }

    #[test]
    fn leaf_len_rejects_match_eat_to_paths() {
        // Table: empty span, mid-UTF-8, past visible end -- reject atomicity.
        let cases: &[(&str, usize)] = &[
            ("abc", 0),
            ("caf\u{e9}.", {
                let acute = "caf\u{e9}.".find('\u{e9}').expect("acute");
                acute + 1 // mid code unit
            }),
            ("hi", 99),
        ];
        for &(src, end) in cases {
            let mut p = make_parser(src);
            let key = p.progress();
            let elen = p.checkpoint().events_len;
            assert!(!p.eat_to(end), "eat_to reject for {src:?} end={end}");
            assert_eq!(p.progress(), key, "eat_to reject: progress");
            assert_eq!(p.checkpoint().events_len, elen, "eat_to reject: events");

            // Length form from cursor 0: absolute end equals length when start is 0.
            let mut p2 = make_parser(src);
            let key2 = p2.progress();
            assert!(
                !p2.leaf_len(OrgSyntaxKind::LINK_PATH, end),
                "leaf_len reject for {src:?} len={end}"
            );
            assert_eq!(p2.progress(), key2, "leaf_len reject: progress");
            assert_eq!(
                p2.checkpoint().events_len,
                0,
                "leaf_len reject: abandoned leaf"
            );
        }
    }

    #[test]
    fn leaf_len_success_and_eof_reject() {
        let src = "hello";
        let mut p = make_parser(src);
        assert!(p.leaf_len(OrgSyntaxKind::LINK_PATH, src.len()));
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        assert_eq!(p.source_start_here(), None);
        let events_at_eof = p.checkpoint().events_len;
        // At EOF: length form rejects without inventing events.
        assert!(!p.leaf_len(OrgSyntaxKind::LINK_PATH, 1));
        assert_eq!(
            p.checkpoint().events_len,
            events_at_eof,
            "EOF reject must not append Start/Token/Finish"
        );
        let (_, events, _) = p.finish();
        // Exactly one LINK_PATH leaf: Start + Token(s) + Finish for the success only.
        let starts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Start { kind } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(starts, [OrgSyntaxKind::LINK_PATH]);
        assert_eq!(
            events.iter().filter(|e| matches!(e, Event::Finish)).count(),
            1
        );
        assert_eq!(token_texts(&events, src), ["hello"]);
    }

    #[test]
    fn leaf_len_rejects_past_with_limit_at() {
        // Cap after first TEXT; leaf_len past the visible window must reject.
        let src = "aa bb";
        let mut p = make_parser(src);
        let cap = p
            .token_view(p.cursor())
            .and_then(|t| t.nth(1))
            .expect("ws after aa")
            .pos();
        p.with_limit_at(cap, |pp| {
            let key = pp.progress();
            let elen = pp.checkpoint().events_len;
            assert!(
                !pp.leaf_len(OrgSyntaxKind::LINK_PATH, 3),
                "len past visible window under with_limit_at"
            );
            assert_eq!(pp.progress(), key);
            assert_eq!(pp.checkpoint().events_len, elen);
            // Success within the cap still works.
            assert!(pp.leaf_len(OrgSyntaxKind::LINK_PATH, 2));
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
    }

    #[test]
    fn with_limit_at_caps_and_restores() {
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let end = pos_ahead(&p, 1);
        p.with_limit_at(end, |pp| {
            assert_eq!(pp.limit(), end);
            assert_eq!(pp.current(), OrgSyntaxKind::TEXT);
            // Cap exclusive: next token slot is EOF under the window.
            assert!(
                pp.token_view(pp.cursor())
                    .is_none_or(|t| t.kind_at(1) == OrgSyntaxKind::EOF)
            );
        });
        // Limit restored; full input visible again.
        assert_eq!(p.limit().index(), p.tokens.len());
        assert!(
            p.token_view(p.cursor())
                .is_some_and(|t| t.kind_at(1) != OrgSyntaxKind::EOF)
        );

        // Empty cap at the cursor: no live tokens.
        let here = p.cursor();
        p.with_limit_at(here, |pp| {
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
            assert_eq!(pp.limit(), pp.cursor());
        });
        assert_ne!(p.current(), OrgSyntaxKind::EOF);
    }

    #[test]
    fn eat_until_empty_region_is_noop() {
        let src = "aa bb";
        let mut p = make_parser(src);
        let here = p.cursor();
        let key = p.progress();
        let elen = p.checkpoint().events_len;
        p.eat_until(here);
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
        assert_eq!(p.cursor(), here);
    }

    #[test]
    fn eat_until_past_end_is_noop() {
        // Cursor already at or past the exclusive end: no drain, no events.
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let mid = pos_ahead(&p, 2);
        p.eat_until(mid);
        assert_eq!(p.cursor(), mid);
        let key = p.progress();
        let elen = p.checkpoint().events_len;
        // Already at end: no-op.
        p.eat_until(mid);
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
        // End strictly before cursor: still no-op.
        let earlier = crate::parser::Pos::from_index(mid.index().saturating_sub(2));
        p.eat_until(earlier);
        assert_eq!(p.progress(), key);
        assert_eq!(p.checkpoint().events_len, elen);
    }

    #[test]
    fn eat_until_drains_to_end_and_respects_outer_limit() {
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let mid = pos_ahead(&p, 2); // exclusive after "aa" and " "
        let far = pos_ahead(&p, 4);
        p.eat_until(mid);
        assert_eq!(p.cursor(), mid);
        assert_eq!(p.current(), OrgSyntaxKind::TEXT); // "bb"
        let (_, events, _) = {
            // Re-run drain from start to assert token texts.
            let mut p0 = make_parser(src);
            p0.eat_until(mid);
            p0.finish()
        };
        assert_eq!(token_texts(&events, src), ["aa", " "]);

        // Nested active limit: eat_until past the cap stops at the cap.
        let mut p2 = make_parser(src);
        let outer = pos_ahead(&p2, 2);
        p2.with_limit_at(outer, |pp| {
            pp.eat_until(far);
            assert_eq!(pp.cursor(), outer);
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
        assert_eq!(p2.cursor(), outer);
        assert_eq!(p2.limit().index(), p2.tokens.len());
    }

    #[test]
    fn eat_until_matches_with_limit_at_eat_to_limit() {
        // Documented equivalence: eat_until(end) == with_limit_at(end, eat_to_limit).
        let src = "aa bb cc dd";
        let end = {
            let p = make_parser(src);
            pos_ahead(&p, 3)
        };
        let outer = {
            let p = make_parser(src);
            pos_ahead(&p, 2)
        };

        let run = |f: &dyn Fn(&mut Parser<'_>)| {
            let mut p = make_parser(src);
            f(&mut p);
            let cur = p.cursor();
            let lim = p.limit().index();
            let (_, events, _) = p.finish();
            let texts: Vec<String> = token_texts(&events, src)
                .into_iter()
                .map(str::to_owned)
                .collect();
            (cur, texts, lim)
        };

        let a = run(&|p| p.eat_until(end));
        let b = run(&|p| p.with_limit_at(end, super::super::Parser::eat_to_limit));
        assert_eq!(a, b);
        assert_eq!(a.2, make_parser(src).tokens.len());

        // Under an outer cap: both stop at the outer limit.
        let a = run(&|p| {
            p.with_limit_at(outer, |pp| {
                pp.eat_until(end);
            });
        });
        let b = run(&|p| {
            p.with_limit_at(outer, |pp| {
                pp.with_limit_at(end, super::super::Parser::eat_to_limit);
            });
        });
        assert_eq!(a, b);
        assert_eq!(a.0, outer);
        assert_eq!(a.2, make_parser(src).tokens.len());
    }

    #[test]
    fn leaf_to_empty_emits_empty_node() {
        let src = "aa bb";
        let mut p = make_parser(src);
        let here = p.cursor();
        let before = p.checkpoint().events_len;
        p.leaf_to(OrgSyntaxKind::CALL_NAME, here);
        // start + finish, no tokens between.
        assert_eq!(p.checkpoint().events_len, before + 2);
        assert_eq!(p.cursor(), here);
        assert_eq!(p.current(), OrgSyntaxKind::TEXT);
        let (_, events, _) = p.finish();
        assert_eq!(
            crate::tests::node_kinds(&events),
            vec![OrgSyntaxKind::CALL_NAME]
        );
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::CALL_NAME).as_deref(),
            Some("")
        );
        assert!(
            token_texts(&events, src).is_empty(),
            "{:?}",
            token_texts(&events, src)
        );
    }

    #[test]
    fn leaf_to_covers_region() {
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let end = pos_ahead(&p, 2);
        p.leaf_to(OrgSyntaxKind::LINK_PATH, end);
        assert_eq!(p.cursor(), end);
        assert_eq!(p.current(), OrgSyntaxKind::TEXT); // "bb"
        let (_, events, _) = p.finish();
        assert_eq!(
            crate::tests::node_kinds(&events),
            vec![OrgSyntaxKind::LINK_PATH]
        );
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("aa ")
        );
        assert_eq!(token_texts(&events, src), ["aa", " "]);
    }

    #[test]
    fn leaf_to_clamps_to_outer_limit() {
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let outer = pos_ahead(&p, 2);
        let far = pos_ahead(&p, 4);
        p.with_limit_at(outer, |pp| {
            pp.leaf_to(OrgSyntaxKind::LINK_PATH, far);
            assert_eq!(pp.cursor(), outer);
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
        assert_eq!(p.cursor(), outer);
        assert_eq!(p.limit().index(), p.tokens.len());
        let (_, events, _) = p.finish();
        assert_eq!(
            crate::tests::node_kinds(&events),
            vec![OrgSyntaxKind::LINK_PATH]
        );
        assert_eq!(
            node_text(&events, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("aa ")
        );
        assert_eq!(token_texts(&events, src), ["aa", " "]);
    }

    #[test]
    fn with_limit_at_nests_and_restores() {
        let src = "a b c";
        let mut p = make_parser(src);
        let outer_end = pos_ahead(&p, 3);
        p.with_limit_at(outer_end, |pp| {
            assert_eq!(pp.limit(), outer_end);
            assert!(
                pp.token_view(pp.cursor())
                    .is_some_and(|t| t.kind_at(2) != OrgSyntaxKind::EOF)
            );
            let inner_end = pos_ahead(pp, 1);
            pp.with_limit_at(inner_end, |ppp| {
                assert_eq!(ppp.limit(), inner_end);
                assert!(
                    ppp.token_view(ppp.cursor())
                        .is_none_or(|t| t.kind_at(1) == OrgSyntaxKind::EOF)
                );
                assert_eq!(ppp.current(), OrgSyntaxKind::TEXT);
            });
            // Outer window restored.
            assert_eq!(pp.limit(), outer_end);
            assert!(
                pp.token_view(pp.cursor())
                    .is_some_and(|t| t.kind_at(2) != OrgSyntaxKind::EOF)
            );
        });
        assert_eq!(p.limit().index(), p.tokens.len());
    }

    #[test]
    fn with_limit_at_clamps_to_outer_and_survives_cursor_moves() {
        let src = "aa bb cc";
        let mut p = make_parser(src);
        let outer_end = pos_ahead(&p, 2); // exclusive end after "aa" and " "
        p.with_limit_at(outer_end, |pp| {
            // A wider request clamps to the outer window.
            let wide = pos_ahead(pp, 4);
            pp.with_limit_at(wide, |ppp| {
                assert_eq!(ppp.limit(), outer_end);
                assert!(
                    ppp.token_view(ppp.cursor())
                        .is_none_or(|t| t.kind_at(2) == OrgSyntaxKind::EOF)
                );
            });
            assert_eq!(pp.limit(), outer_end);
        });

        // A bound measured before consumption stays valid after eats
        // (positions are absolute).
        let mut p2 = make_parser(src);
        let end = pos_ahead(&p2, 2);
        p2.eat_any(); // past "aa"
        assert_eq!(p2.current(), OrgSyntaxKind::WHITESPACE);
        p2.with_limit_at(end, |pp| {
            assert_eq!(pp.limit(), end);
            assert_eq!(pp.current(), OrgSyntaxKind::WHITESPACE);
            assert!(
                pp.token_view(pp.cursor())
                    .is_none_or(|t| t.kind_at(1) == OrgSyntaxKind::EOF)
            );
        });
        assert_eq!(p2.limit().index(), p2.tokens.len());
        assert_eq!(p2.current(), OrgSyntaxKind::WHITESPACE);
    }

    #[test]
    fn leaf_len_checked_add_overflow_rejects() {
        // Mid-tile + len that overflows start + len: reject, cursor unchanged.
        let src = "ab.";
        let mut p = make_parser(src);
        assert!(p.eat_to(2));
        let key = p.progress();
        let events_len = p.checkpoint().events_len;
        assert!(
            !p.leaf_len(OrgSyntaxKind::LINK_PATH, usize::MAX),
            "overflow must reject"
        );
        assert_eq!(p.progress(), key, "cursor unchanged on overflow reject");
        assert_eq!(p.checkpoint().events_len, events_len, "no events on reject");
        // The test helper rejects the overflow the same way.
        assert!(!eat_to_len(&mut p, usize::MAX));
        assert_eq!(p.progress(), key);
    }

    #[test]
    fn progress_advances_on_mid_tile_eat() {
        let src = "ab.";
        let mut p = make_parser(src);
        let before = p.progress();
        let pos_before = p.cursor().index();
        assert!(p.eat_to(2));
        let after = p.progress();
        assert_ne!(before, after, "mid-tile eat must advance progress");
        assert_eq!(p.cursor().index(), pos_before, "pos may stay on same tile");
        assert!(after.mid_offset() > before.mid_offset());
    }
}
