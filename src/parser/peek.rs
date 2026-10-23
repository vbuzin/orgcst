//! Peeking, position predicates, and balanced-pair scans.
//!
//! Grammar rules address positions only through typed handles
//! ([`Parser::cursor`], [`Parser::limit`], [`Parser::here`],
//! [`super::LineView`], [`super::TokenView`], [`super::Pos`],
//! [`super::LineId`]) and bound regions in one of two ways:
//!
//! - A structured body is parsed under a lowered limit: `with_limit_at` plus
//!   a walk (`inline_in` / `elements_in`), or the rest of an already capped
//!   window (`eat_to_limit` / `with_rest_of_line`).
//! - An opaque body is consumed up to a measured `Pos` with
//!   [`Parser::eat_until`] / [`Parser::leaf_to`], without changing the limit.
//!
//! Raw relative peeks (`peek_rel`) stay inside the `parser` module. Object
//! rules walk [`super::TokenView`]s; element rules walk lines.

use super::Parser;
use crate::syntax::OrgSyntaxKind;

impl Parser<'_> {
    // -------------------------------------------------------------------------
    // Position predicates (cursor-relative; line geometry uses these)
    // -------------------------------------------------------------------------

    /// Whether the cursor is inside a lex tile.
    #[inline]
    fn mid_tile_here(&self) -> bool {
        self.token_byte_off > 0
    }

    // -------------------------------------------------------------------------
    // Peeking (non-advancing)
    // -------------------------------------------------------------------------

    /// The kind of the token `n` steps from the cursor (0 = current).
    ///
    /// Grammar rules use [`Self::current`] and [`super::TokenView`] instead.
    ///
    /// Positive = ahead, negative = behind. Forward out of range yields `EOF`;
    /// backward before the first token yields `BOF`. Neither sentinel is
    /// lexed or stored in the tree.
    #[inline]
    pub(super) fn peek_rel(&self, n: isize) -> OrgSyntaxKind {
        match super::abs_index(self, n) {
            Some(i) => self.tokens.get(i).map_or(OrgSyntaxKind::EOF, |t| t.kind),
            None if n < 0 => OrgSyntaxKind::BOF,
            None => OrgSyntaxKind::EOF,
        }
    }

    /// The kind at the cursor, or `EOF` at end of input.
    #[inline]
    pub(crate) fn current(&self) -> OrgSyntaxKind {
        self.peek_rel(0)
    }

    // -------------------------------------------------------------------------
    // Cursor-context predicates (mid-tile aware)
    // -------------------------------------------------------------------------

    /// The source byte immediately before the cursor (mid-tile aware).
    ///
    /// Mid-tile this is the last consumed byte of the current tile - the PRE
    /// lookbehind for the unconsumed remainder.
    #[inline]
    pub(crate) fn prev_byte(&self) -> Option<u8> {
        self.source_start_here()
            .and_then(|start| (start > 0).then(|| self.source.as_bytes()[start - 1]))
    }

    /// Whether the token before the cursor is `WHITESPACE`, `INDENT`, or
    /// `NEWLINE`. False mid-tile.
    #[inline]
    pub(crate) fn after_separator(&self) -> bool {
        !self.mid_tile_here() && self.peek_rel(-1).is_separator()
    }

    /// Whether the cursor sits at the active inline region start.
    ///
    /// The region start is set by the object walks in `drive::region`. False
    /// mid-tile, when no region is active, or when the cursor is not visible.
    #[inline]
    pub(crate) fn at_region_start(&self) -> bool {
        !self.mid_tile_here() && self.pos < self.limit && self.inline_region_start == Some(self.pos)
    }

    /// Scan forward from the token at absolute `from` for a balanced
    /// `open` ... `close` pair (same-delimiter nesting).
    ///
    /// Returns the closer's position, or `None` when `from` is not `open`, the
    /// pair is unclosed under the limit, or (when `newlines` is
    /// [`PairNewlines::Stop`]) a NEWLINE intervenes. [`PairNewlines::Allow`]
    /// is for inline footnote bodies, which may span lines; keyword duals and
    /// babel headers use [`PairNewlines::Stop`].
    ///
    /// For a delimiter-inclusive half-open span, use [`Self::measure_paired_at`].
    /// Markup has its own closer search (`markup::find_closer`), and run-of-N
    /// closers use [`super::TokenView::run_of`].
    pub(crate) fn find_paired_at(
        &self,
        from: super::Pos,
        open: OrgSyntaxKind,
        close: OrgSyntaxKind,
        newlines: PairNewlines,
    ) -> Option<super::Pos> {
        let cap = self.limit.min(self.tokens.len());
        let start = from.index();
        if start >= cap || self.tokens[start].kind != open {
            return None;
        }
        let mut depth = 1isize;
        let mut i = start + 1;
        while i < cap {
            match self.tokens[i].kind {
                OrgSyntaxKind::NEWLINE if newlines == PairNewlines::Stop => return None,
                k if k == open => depth += 1,
                k if k == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(super::Pos::from_index(i));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Balanced pair as a half-open `[open, end)` span including delimiters.
    ///
    /// Wraps [`Self::find_paired_at`]: on success returns the open position and
    /// the exclusive end after the closer token. `None` under the same
    /// conditions as `find_paired_at`. Does not move the cursor.
    pub(crate) fn measure_paired_at(
        &self,
        open_pos: super::Pos,
        open: OrgSyntaxKind,
        close: OrgSyntaxKind,
        newlines: PairNewlines,
    ) -> Option<(super::Pos, super::Pos)> {
        let closer = self.find_paired_at(open_pos, open, close, newlines)?;
        // Closer is always a token under the limit when find_paired_at succeeds.
        let end = self
            .token_view(closer)
            .map_or_else(|| self.limit(), |t| t.end());
        Some((open_pos, end))
    }

    /// First token of `kind` on the current line from the cursor, or `None` if a
    /// NEWLINE (or the region end) is reached first.
    ///
    /// Not a balanced pair search (see [`Self::find_paired_at`]). Used to find
    /// timestamp closers.
    pub(crate) fn find_same_line_kind(
        &self,
        kind: OrgSyntaxKind,
    ) -> Option<super::TokenView<'_, '_>> {
        for t in self.tokens_from_cursor() {
            if t.kind() == kind {
                return Some(t);
            }
            if t.kind() == OrgSyntaxKind::NEWLINE {
                return None;
            }
        }
        None
    }
}

/// Whether a balanced pair scan may cross NEWLINE tokens.
///
/// For [`Parser::find_paired_at`] / [`Parser::measure_paired_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairNewlines {
    /// Abort on NEWLINE (line-bounded balanced pair).
    Stop,
    /// NEWLINE is ordinary interior (nested brackets may span lines).
    Allow,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::make_parser;

    #[test]
    fn current_and_prev_byte_at_bof_and_mid_line() {
        let mut p = make_parser("* A :tag:\nB");
        assert_eq!(p.current(), OrgSyntaxKind::STAR);
        assert_eq!(p.prev_byte(), None, "BOF: no byte before first token");
        assert!(!p.after_separator());
        assert!(p.at_line_start());

        p.eat_any(); // STAR
        p.eat_any(); // WS
        p.eat_any(); // TEXT "A"
        p.eat_any(); // WS
        assert_eq!(p.current(), OrgSyntaxKind::COLON);
        assert_eq!(p.prev_byte(), Some(b' '));
        assert!(p.after_separator());
        assert!(!p.at_line_start());

        while p.current() != OrgSyntaxKind::EOF
            && !(p.current() == OrgSyntaxKind::TEXT && p.text() == "B")
        {
            p.eat_any();
        }
        assert_eq!(p.current(), OrgSyntaxKind::TEXT);
        assert_eq!(p.text(), "B");
        assert!(p.after_separator()); // after NEWLINE
        assert!(p.at_line_start());
    }

    #[test]
    fn after_separator_false_mid_tile() {
        let mut p = make_parser("ab. ");
        assert!(p.leaf_len(OrgSyntaxKind::TEXT, 2));
        assert!(p.progress().mid_offset() > 0);
        assert!(!p.after_separator(), "mid-tile PRE is not separator-based");
        assert_eq!(p.prev_byte(), Some(b'b'));
    }

    /// `Parser::after_separator` is wide (WHITESPACE | INDENT | NEWLINE);
    /// `TokenView::after_space` is narrow (WHITESPACE only). Grammar rules
    /// choose between them, so they must stay different.
    #[test]
    fn after_separator_is_wider_than_after_space() {
        // WHITESPACE: both agree.
        let mut ws = make_parser("a b");
        ws.eat_any(); // TEXT "a"
        ws.eat_any(); // WHITESPACE
        assert!(ws.after_separator());
        assert!(ws.token_view(ws.cursor()).expect("b").after_space());

        // INDENT and NEWLINE: wide only.
        for (src, skip, label) in [("  x", 1usize, "INDENT"), ("a\nb", 2, "NEWLINE")] {
            let mut p = make_parser(src);
            for _ in 0..skip {
                p.eat_any();
            }
            assert_eq!(p.current(), OrgSyntaxKind::TEXT, "{label}");
            assert!(p.after_separator(), "{label} is a separator");
            assert!(
                !p.token_view(p.cursor()).expect("text").after_space(),
                "after_space is WHITESPACE-only, not {label}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // find_paired_at / measure_paired_at - balanced pairs
    // -----------------------------------------------------------------------

    #[test]
    fn find_paired_at_nested_empty_and_parens() {
        use crate::parser::PairNewlines;
        use crate::tests::pos_ahead;
        use OrgSyntaxKind::{L_BRACKET, L_PAREN, R_BRACKET, R_PAREN};

        let p = make_parser("[a[b]c]");
        assert_eq!(
            p.find_paired_at(p.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(pos_ahead(&p, 6))
        );
        assert_eq!(
            p.find_paired_at(pos_ahead(&p, 2), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(pos_ahead(&p, 4))
        );

        let p2 = make_parser("[[x]]");
        assert_eq!(
            p2.find_paired_at(p2.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(pos_ahead(&p2, 4))
        );
        assert_eq!(
            p2.find_paired_at(pos_ahead(&p2, 1), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(pos_ahead(&p2, 3))
        );

        let p3 = make_parser("[]");
        assert_eq!(
            p3.find_paired_at(p3.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(pos_ahead(&p3, 1))
        );

        let p4 = make_parser("call(args)");
        let open = p4
            .tokens_from_cursor()
            .find(|t| t.kind() == L_PAREN)
            .expect("L_PAREN");
        let closer = p4
            .find_paired_at(open.pos(), L_PAREN, R_PAREN, PairNewlines::Stop)
            .expect("closer");
        assert_eq!(p4.token_view(closer).expect("closer view").kind(), R_PAREN);
    }

    #[test]
    fn find_paired_at_rejects_unclosed_wrong_start_cross_line_and_limit() {
        use crate::parser::PairNewlines;
        use crate::tests::pos_ahead;
        use OrgSyntaxKind::{L_BRACKET, R_BRACKET};

        let p = make_parser("[no close");
        assert!(
            p.find_paired_at(p.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                .is_none()
        );

        let p2 = make_parser("foo[bar]");
        assert!(
            p2.find_paired_at(p2.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                .is_none()
        );

        // Stop policy: newline aborts even if a closer follows.
        let p3 = make_parser("[foo\nbar]");
        assert!(
            p3.find_paired_at(p3.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                .is_none()
        );
        // Allow policy: same source closes across the newline (footnote form).
        assert_eq!(
            p3.find_paired_at(p3.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Allow),
            Some(pos_ahead(&p3, 4)),
            "Allow must close across NEWLINE"
        );

        // Cap before the closer: pair is unclosed under the visible window.
        // Tokens: L_BRACKET TEXT R_BRACKET
        let mut p4 = make_parser("[x]");
        let closer = pos_ahead(&p4, 2);
        assert_eq!(
            p4.find_paired_at(p4.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop),
            Some(closer),
            "unbounded: pair completes"
        );
        p4.with_limit_at(closer, |pp| {
            assert!(
                pp.find_paired_at(pp.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                    .is_none(),
                "cap hiding R_BRACKET must not complete the pair"
            );
        });
    }

    /// Exclusive-end span wrapper over `find_paired_at` (Stop and Allow).
    #[test]
    fn measure_paired_at_exclusive_end_closed_empty_and_nested() {
        use crate::parser::PairNewlines;
        use crate::tests::pos_ahead;
        use OrgSyntaxKind::{L_BRACE, L_BRACKET, L_PAREN, R_BRACE, R_BRACKET, R_PAREN};

        let p = make_parser("{x}");
        let before = p.cursor();
        let (s, e) = p
            .measure_paired_at(p.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
            .expect("closed");
        assert_eq!(p.cursor(), before, "measure must not advance");
        assert_eq!(s, p.cursor());
        // `{` TEXT `}`: exclusive end is one past the closer.
        assert_eq!(e.index(), s.index() + 3);
        assert_eq!(
            p.find_paired_at(p.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
                .map(|c| c.index() + 1),
            Some(e.index()),
            "end is exclusive after find_paired_at closer"
        );

        let p_empty = make_parser("{}");
        let (s, e) = p_empty
            .measure_paired_at(p_empty.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
            .expect("empty");
        assert_eq!(e.index(), s.index() + 2);

        // Nested braces: exclusive end after the outer closer.
        // Tokens: L_BRACE TEXT L_BRACE TEXT R_BRACE TEXT R_BRACE
        let p_nest = make_parser("{a{b}c}");
        let (s, e) = p_nest
            .measure_paired_at(p_nest.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
            .expect("nested");
        assert_eq!(e.index(), s.index() + 7);

        let p_br = make_parser("[a[b]c]");
        let (s, e) = p_br
            .measure_paired_at(p_br.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
            .expect("brackets");
        assert_eq!(e.index(), s.index() + 7);

        // Empty paren pair (babel args form).
        let p_par = make_parser("()");
        let (s, e) = p_par
            .measure_paired_at(p_par.cursor(), L_PAREN, R_PAREN, PairNewlines::Stop)
            .expect("parens");
        assert_eq!(e.index(), s.index() + 2);

        // Non-cursor open_pos: nested inner pair from pos_ahead.
        let p_inner = make_parser("[a[b]c]");
        let (s, e) = p_inner
            .measure_paired_at(
                pos_ahead(&p_inner, 2),
                L_BRACKET,
                R_BRACKET,
                PairNewlines::Stop,
            )
            .expect("inner open");
        assert_eq!(s, pos_ahead(&p_inner, 2));
        assert_eq!(e.index(), s.index() + 3); // `[` TEXT `]`

        // Tail after closer: exclusive end lands on the following token.
        let p_tail = make_parser("{x}tail");
        let (s, e) = p_tail
            .measure_paired_at(p_tail.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
            .expect("with tail");
        assert_eq!(e.index(), s.index() + 3);
        assert_eq!(p_tail.token_view(e).map(|t| t.text()), Some("tail"));
    }

    #[test]
    fn measure_paired_at_rejects_like_find_and_allow_cross_line() {
        use crate::parser::PairNewlines;
        use crate::tests::pos_ahead;
        use OrgSyntaxKind::{L_BRACE, L_BRACKET, R_BRACE, R_BRACKET};

        let p_u = make_parser("{unclosed");
        assert!(
            p_u.measure_paired_at(p_u.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
                .is_none()
        );

        // Stop: newline aborts before the closer.
        let p = make_parser("{a\nb}");
        assert!(
            p.measure_paired_at(p.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
                .is_none()
        );

        // Wrong opener kind at open_pos.
        let p2 = make_parser("x{y}");
        assert!(
            p2.measure_paired_at(p2.cursor(), L_BRACE, R_BRACE, PairNewlines::Stop)
                .is_none()
        );

        // Cap before the closer: pair is unclosed under the visible window.
        // Tokens: L_BRACKET TEXT R_BRACKET
        let mut p_cap = make_parser("[x]");
        let closer = pos_ahead(&p_cap, 2);
        assert!(
            p_cap
                .measure_paired_at(p_cap.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                .is_some(),
            "unbounded: pair completes"
        );
        p_cap.with_limit_at(closer, |pp| {
            assert!(
                pp.measure_paired_at(pp.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                    .is_none(),
                "cap hiding R_BRACKET must not complete the pair"
            );
        });

        // Allow: exclusive end after cross-line closer (footnote form).
        // Tokens: L_BRACKET TEXT NEWLINE TEXT R_BRACKET
        let p3 = make_parser("[foo\nbar]");
        let (s, e) = p3
            .measure_paired_at(p3.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Allow)
            .expect("Allow closes across NEWLINE");
        assert_eq!(s, p3.cursor());
        assert_eq!(e, pos_ahead(&p3, 5));
        assert!(
            p3.measure_paired_at(p3.cursor(), L_BRACKET, R_BRACKET, PairNewlines::Stop)
                .is_none(),
            "Stop must still reject the same source"
        );
    }

    // -----------------------------------------------------------------------
    // find_same_line_kind - first kind before NEWLINE / region end
    // -----------------------------------------------------------------------

    #[test]
    fn find_same_line_kind_hit_miss_newline_and_limit() {
        use OrgSyntaxKind::{R_ANGLE, R_PAREN};

        // Hit: first matching kind on the line; pure (cursor unmoved).
        let p = make_parser("a) b>");
        let before = p.cursor();
        let hit = p.find_same_line_kind(R_PAREN).expect("R_PAREN");
        assert_eq!(hit.kind(), R_PAREN);
        assert_eq!(
            p.find_same_line_kind(R_ANGLE).map(|t| t.kind()),
            Some(R_ANGLE)
        );
        assert_eq!(p.cursor(), before, "find_same_line_kind must not advance");

        // Cursor token itself matches.
        let p_cur = make_parser(">");
        assert_eq!(
            p_cur.find_same_line_kind(R_ANGLE).map(|t| t.kind()),
            Some(R_ANGLE)
        );

        // Miss: kind absent before end of tokens.
        let p_miss = make_parser("plain text");
        assert!(p_miss.find_same_line_kind(R_ANGLE).is_none());

        // NEWLINE stops: closer after the line break is not visible.
        let p_nl = make_parser("foo\n>");
        assert!(
            p_nl.find_same_line_kind(R_ANGLE).is_none(),
            "must stop at NEWLINE before a later match"
        );
        // Same-line hit still wins when present before the newline.
        let p_before_nl = make_parser("x> y\n>");
        assert_eq!(
            p_before_nl.find_same_line_kind(R_ANGLE).map(|t| t.kind()),
            Some(R_ANGLE)
        );

        // Cap before the target: miss under the visible window.
        let mut p_cap = make_parser("a >");
        let closer = p_cap
            .find_same_line_kind(R_ANGLE)
            .expect("unbounded hit")
            .pos();
        p_cap.with_limit_at(closer, |pp| {
            assert!(
                pp.find_same_line_kind(R_ANGLE).is_none(),
                "cap hiding target must miss"
            );
        });
    }
}
