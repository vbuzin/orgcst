//! Line facts at the cursor, read from the lexer's line table.
//!
//! Line starts come from the line table, not from looking at the previous
//! token. Per-line inspection lives on [`crate::parser::LineView`]; this
//! module holds the cursor-level facts it and `eat_line` build on.
//!
//! | Name | Meaning |
//! | ---- | ------- |
//! | `at_line_start()` | Cursor is that line's first token (not mid-tile) |
//! | `at_blank()` | Line-start of a blank line |
//! | `at_line_term()` | Cursor is on `NEWLINE` or visible `EOF` |
//! | `with_rest_of_line(f)` | Cap at rest-of-line content (no trailing NL), run `f` |
//!
//! Grammar rules read lines through [`Parser::line`] (a `LineView` when the
//! cursor is at a line's first token) and [`Parser::line_here`], never
//! through raw line indices. Heading probes read the line's first token,
//! element probes its first non-indent token.

use super::Parser;
use crate::lexer::line_at_token;
use crate::syntax::OrgSyntaxKind;

impl Parser<'_> {
    /// Line containing absolute token `abs`, if any.
    #[inline]
    pub(crate) fn line_at(&self, abs: usize) -> Option<usize> {
        line_at_token(&self.lines, abs)
    }

    /// Whether the cursor is on the first token of its line.
    ///
    /// Never true mid-tile or at a token hidden by the active limit.
    #[inline]
    pub(crate) fn at_line_start(&self) -> bool {
        if self.token_byte_off > 0 {
            return false;
        }
        if self.pos >= self.limit {
            return false;
        }
        self.line_at(self.pos)
            .is_some_and(|li| self.lines[li].first as usize == self.pos)
    }

    /// Exclusive absolute end of rest-of-line content under the active limit
    /// (exclude a visible trailing NEWLINE). Grammar rules use
    /// [`Self::with_rest_of_line`].
    #[inline]
    fn line_content_end(&self) -> usize {
        let Some(line) = self.line_here() else {
            return self.limit;
        };
        let end = line.end().index();
        if end > self.pos
            && self
                .tokens
                .get(end - 1)
                .is_some_and(|t| t.kind == OrgSyntaxKind::NEWLINE)
        {
            end - 1
        } else {
            end
        }
    }

    /// Cap at rest-of-line content (no trailing NEWLINE), run `f`, restore.
    ///
    /// Used for keyword and babel-call values.
    #[inline]
    pub(crate) fn with_rest_of_line<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let end = super::Pos::from_index(self.line_content_end());
        self.with_limit_at(end, f)
    }

    /// Whether the cursor is at the start of a blank line.
    #[inline]
    pub(crate) fn at_blank(&self) -> bool {
        self.at_line_start()
            && self
                .line_at(self.pos)
                .is_some_and(|li| self.lines[li].is_blank)
    }

    /// Whether the cursor is on a line terminator: `NEWLINE` or visible `EOF`.
    ///
    /// Compare [`crate::parser::LineView::end`], the line's exclusive end
    /// index, and [`crate::parser::TokenView::next_byte`] for the byte after
    /// a token.
    #[inline]
    pub(crate) fn at_line_term(&self) -> bool {
        self.current().is_line_end()
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::Parser;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn at_line_start_is_line_table_not_lookbehind() {
        let mut p = Parser::with_config("a\nb\n", crate::tests::default_config());
        assert!(p.at_line_start());
        assert_eq!(p.line().map(|l| l.id().index()), Some(0));
        p.eat_any();
        assert!(!p.at_line_start());
        assert_eq!(p.current(), OrgSyntaxKind::NEWLINE);
        p.eat_any();
        assert!(p.at_line_start());
        assert_eq!(p.line().map(|l| l.id().index()), Some(1));
        // Cap at cursor: invisible slot is not line-start.
        p.with_limit_at(p.cursor(), |pp| {
            assert!(!pp.at_line_start());
            assert!(pp.line().is_none());
        });
    }

    #[test]
    fn line_view_gate_and_content() {
        let mut p = Parser::with_config("# c\nx\n", crate::tests::default_config());
        {
            let line = p.line().expect("line 0");
            assert_eq!(line.id().index(), 0);
            assert_eq!(line.content().map(|t| t.pos()), Some(line.start()));
            assert_eq!(line.kind(0), OrgSyntaxKind::HASH);
        }
        p.eat_any();
        assert!(p.line().is_none(), "mid-line entry has no line view");
        assert!(!p.at_line_start());
        p.eat_any();
        p.eat_any();
        p.eat_any();
        assert_eq!(p.line().map(|l| l.id().index()), Some(1));

        let mut p2 = Parser::with_config("ab\n", crate::tests::default_config());
        assert!(p2.line().is_some());
        assert!(p2.eat_to(1));
        assert!(p2.progress().mid_offset() > 0);
        assert!(p2.line().is_none(), "mid-tile is not line start");

        let mut p3 = Parser::with_config("x\n", crate::tests::default_config());
        assert_eq!(p3.eat_line(), 1);
        assert_eq!(p3.current(), OrgSyntaxKind::EOF);
        assert!(p3.line_here().is_none());
        assert!(p3.line().is_none());
    }

    #[test]
    fn line_first_vs_content_on_indent() {
        let p = Parser::with_config("  # c\n", crate::tests::default_config());
        assert_eq!(p.current(), OrgSyntaxKind::INDENT);
        let line = p.line().expect("line 0");
        assert_eq!(line.indent_cols(), 2);
        assert_eq!(line.first().expect("first").kind(), OrgSyntaxKind::INDENT);
        assert_eq!(line.content().expect("content").kind(), OrgSyntaxKind::HASH);
        assert!(line.content().expect("content").pos() > line.start());
    }

    #[test]
    fn line_views_respect_limit() {
        let p = Parser::with_config("  # c\n#+K: v\n", crate::tests::default_config());
        {
            let line0 = p.line().expect("line 0");
            assert_eq!(line0.indent_cols(), 2);
            assert_eq!(line0.kind(0), OrgSyntaxKind::HASH);
            let line1 = line0.next().expect("line 1");
            assert_eq!(line1.kind(0), OrgSyntaxKind::HASH);
        }

        let mut p2 = Parser::with_config("  # c\n#+K: v\n", crate::tests::default_config());
        let cap = p2.line().expect("line 0").end();
        p2.with_limit_at(cap, |pp| {
            let line0 = pp.line().expect("line 0 visible");
            assert!(line0.next().is_none(), "line 1 hidden by the limit");
        });
    }

    #[test]
    fn at_line_term_is_newline_or_eof_not_line_end_index() {
        let mut p = Parser::with_config("ab\n", crate::tests::default_config());
        assert!(!p.at_line_term());
        p.eat_any();
        assert!(p.at_line_term());
        p.eat_any();
        assert!(p.at_line_term()); // true EOF
        // LineView::end is an absolute bound, not "on terminator"
        let p2 = Parser::with_config("ab\n", crate::tests::default_config());
        // tokens: TEXT, NEWLINE; exclusive end index is 2
        assert_eq!(p2.line().expect("line").end().index(), 2);
        assert!(!p2.at_line_term());
    }

    #[test]
    fn lines_after_iterates_and_respects_limit() {
        let empty = Parser::with_config("", crate::tests::default_config());
        assert!(empty.lines.is_empty(), "{:?}", empty.lines);
        assert!(empty.line().is_none());

        let p = Parser::with_config("a\nb\nc\n", crate::tests::default_config());
        let line0 = p.line().expect("line 0");
        let ids: Vec<usize> = line0.lines_after().map(|l| l.id().index()).collect();
        assert_eq!(ids, vec![1, 2]);
        let last = line0.lines_after().last().expect("line 2");
        assert!(last.lines_after().next().is_none());

        let mut limited = Parser::with_config("a\nb\nc\n", crate::tests::default_config());
        let l1_first = limited.lines[1].first as usize;
        limited.with_limit_at(crate::parser::Pos::from_index(l1_first), |pp| {
            let line0 = pp.line().expect("line 0");
            assert!(
                line0.lines_after().next().is_none(),
                "no lines visible past the cap"
            );
        });
        let mid = (limited.lines[1].content as usize) + 1;
        limited.with_limit_at(crate::parser::Pos::from_index(mid), |pp| {
            let line0 = pp.line().expect("line 0");
            let ids: Vec<usize> = line0.lines_after().map(|l| l.id().index()).collect();
            assert_eq!(ids, vec![1], "line 2 hidden by the limit");
        });
    }

    #[test]
    fn with_rest_of_line_excludes_trailing_newline() {
        // tokens: HASH PLUS TEXT COLON WHITESPACE TEXT NEWLINE
        let mut p = Parser::with_config("#+K: v\n", crate::tests::default_config());
        let content_end = p.line().expect("line").end().index() - 1; // exclude NL
        assert_eq!(p.line_content_end(), content_end);
        p.with_rest_of_line(|pp| {
            assert_eq!(pp.limit().index(), content_end);
            // Last content token visible; past it is EOF under the cap.
            while pp.current() != OrgSyntaxKind::EOF {
                pp.eat_any();
            }
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });
        assert_eq!(p.limit().index(), p.token_count());

        let mut p2 = Parser::with_config("#+K: value\n", crate::tests::default_config());
        let nl_abs = p2.lines[0].end as usize - 1;
        assert_eq!(p2.token_kind(nl_abs), OrgSyntaxKind::NEWLINE);
        // Outer abs cap before NL: content end clamps to that window.
        p2.with_limit_at(crate::parser::Pos::from_index(nl_abs), |pp| {
            assert_eq!(pp.line_content_end(), nl_abs);
            assert_eq!(pp.token_kind(nl_abs - 1), OrgSyntaxKind::TEXT);
            pp.with_rest_of_line(|ppp| {
                assert_eq!(ppp.limit().index(), nl_abs);
            });
        });
        p2.with_limit_at(crate::parser::Pos::from_index(nl_abs + 1), |pp| {
            assert_eq!(pp.line_content_end(), nl_abs);
            pp.with_rest_of_line(|ppp| {
                assert_eq!(ppp.limit().index(), nl_abs);
                assert_eq!(ppp.current(), OrgSyntaxKind::HASH);
                // Cap equals content end: drain reaches EOF under cap.
                ppp.eat_to_limit();
                assert_eq!(ppp.current(), OrgSyntaxKind::EOF);
            });
        });
    }

    #[test]
    fn with_rest_of_line_mid_line_and_edges() {
        // Mid-line after `:` and whitespace (keyword value start).
        // tokens: HASH PLUS TEXT COLON WHITESPACE TEXT NEWLINE
        let mut mid = Parser::with_config("#+K: v\n", crate::tests::default_config());
        for _ in 0..5 {
            mid.eat_any();
        }
        assert_eq!(mid.current(), OrgSyntaxKind::TEXT);
        assert_eq!(mid.text(), "v");
        mid.with_rest_of_line(|pp| {
            assert_eq!(pp.limit().index(), pp.cursor().index() + 1);
            assert_eq!(pp.current(), OrgSyntaxKind::TEXT);
            pp.eat_any();
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });

        // No trailing NEWLINE: content runs to visible end.
        let mut no_nl = Parser::with_config("#+K: v", crate::tests::default_config());
        assert_eq!(no_nl.line_content_end(), no_nl.token_count());
        no_nl.with_rest_of_line(|pp| {
            assert_eq!(pp.limit().index(), pp.token_count());
            pp.eat_to_limit();
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
        });

        // Cursor on NEWLINE: exclusive content end equals pos -> empty window.
        let mut on_nl = Parser::with_config("#+K: v\n", crate::tests::default_config());
        for _ in 0..6 {
            on_nl.eat_any();
        }
        assert_eq!(on_nl.current(), OrgSyntaxKind::NEWLINE);
        assert_eq!(on_nl.line_content_end(), on_nl.cursor().index());
        on_nl.with_rest_of_line(|pp| {
            assert_eq!(pp.current(), OrgSyntaxKind::EOF);
            assert_eq!(pp.limit().index(), pp.cursor().index());
        });
        assert_eq!(on_nl.current(), OrgSyntaxKind::NEWLINE);
    }

    #[test]
    fn eat_line_requires_line_start() {
        let mut p = Parser::with_config("first\nsecond\n", crate::tests::default_config());
        assert_eq!(p.line().map(|l| l.id().index()), Some(0));
        assert_eq!(p.eat_line(), 1);
        assert_eq!(p.line().map(|l| l.id().index()), Some(1));
        assert_eq!(p.text(), "second");

        let mut mid = Parser::with_config("foo bar\n", crate::tests::default_config());
        mid.eat_any();
        assert!(mid.line().is_none());
        assert_eq!(mid.current(), OrgSyntaxKind::WHITESPACE);
        let key = mid.progress();
        assert_eq!(mid.eat_line(), 0);
        assert_eq!(mid.progress(), key);
    }

    #[test]
    fn blank_line_header_matches_at_blank() {
        let p = Parser::with_config("a\n\n  \nb\n", crate::tests::default_config());
        assert_eq!(p.lines.len(), 4);
        assert!(!p.lines[0].is_blank);
        assert!(p.lines[1].is_blank);
        assert!(p.lines[2].is_blank);
        assert_eq!(p.lines[2].indent_cols, 2);
        assert!(!p.lines[3].is_blank);

        let mut blank = Parser::with_config("\nnext\n", crate::tests::default_config());
        assert!(blank.at_blank());
        assert!(blank.line().is_some_and(|l| l.is_blank()));
        assert_eq!(blank.eat_line(), 1);
        assert!(!blank.at_blank());
        assert_eq!(blank.text(), "next");

        // Empty source has no line at all, so it is not "at a blank line".
        let empty = Parser::with_config("", crate::tests::default_config());
        assert!(
            !empty.at_blank(),
            "empty source has no line table entry to be blank"
        );
    }
}
