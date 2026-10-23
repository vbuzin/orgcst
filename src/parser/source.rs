//! Source text at and after the cursor.

use super::Parser;

impl Parser<'_> {
    /// Source text of the token at the cursor; mid-tile, only the unconsumed
    /// remainder.
    #[inline]
    pub(crate) fn text(&self) -> &str {
        if self.pos >= self.limit {
            return "";
        }
        let Some(token) = self.tokens.get(self.pos) else {
            return "";
        };
        let range = token.range;
        let start = usize::from(range.start()) + self.token_byte_off;
        let end = usize::from(range.end());
        if start >= end {
            return "";
        }
        // Invariant: token ranges from lex() lie on UTF-8 char boundaries
        // (lexer splits only on ASCII structural bytes or inside TEXT);
        // mid-token offsets are always set at char boundaries by eat_to.
        &self.source[start..end]
    }

    /// Visible source from the cursor through the active limit (mid-tile aware).
    ///
    /// Used by plain/angle link protocol matching over the remaining window.
    #[inline]
    pub(crate) fn rest_source(&self) -> &str {
        let Some(start) = self.source_start_here() else {
            return "";
        };
        let end = self.visible_source_end();
        if start <= end {
            &self.source[start..end]
        } else {
            ""
        }
    }

    /// Contiguous source covering the next `n` tokens from the cursor.
    ///
    /// Mid-tile: open start is the unconsumed remainder of the current tile.
    #[inline]
    pub(crate) fn source_of_n(&self, n: usize) -> &str {
        if n == 0 {
            return "";
        }
        let Some(start) = self.source_start_here() else {
            return "";
        };
        // Exclusive end: start of the token n steps ahead, or visible end.
        let end = match self.pos.checked_add(n) {
            Some(abs) if abs < self.limit => self.tokens.get(abs).map_or_else(
                || self.visible_source_end(),
                |t| usize::from(t.range.start()),
            ),
            _ => self.visible_source_end(),
        };
        if start <= end {
            &self.source[start..end]
        } else {
            ""
        }
    }

    /// Absolute source-byte start of the unconsumed remainder at the cursor,
    /// or `None` at the visible end.
    #[inline]
    pub(crate) fn source_start_here(&self) -> Option<usize> {
        if self.pos >= self.limit {
            return None;
        }
        let token = self.tokens.get(self.pos)?;
        Some(usize::from(token.range.start()) + self.token_byte_off)
    }

    /// Exclusive byte end of the visible token window.
    ///
    /// `source.len()` when the whole input is visible; otherwise the start of
    /// the first hidden token, which is also the end of the last visible one.
    pub(crate) fn visible_source_end(&self) -> usize {
        if self.limit < self.tokens.len() {
            usize::from(self.tokens[self.limit].range.start())
        } else {
            self.source.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::make_parser;

    #[test]
    fn rest_source_and_source_of_n_cursor_forms() {
        let p = make_parser("aa bb");
        assert_eq!(p.rest_source(), "aa bb");
        assert_eq!(p.source_of_n(0), "");
        assert_eq!(p.source_of_n(1), "aa");
        assert_eq!(p.source_of_n(2), "aa ");
        assert_eq!(p.source_of_n(3), "aa bb");
        assert_eq!(p.source_of_n(99), "aa bb");

        let mut mid = make_parser("ab. ");
        assert!(mid.leaf_len(OrgSyntaxKind::TEXT, 2));
        assert_eq!(mid.rest_source(), ". ");
        assert_eq!(mid.source_of_n(1), ".");
        assert_eq!(mid.source_of_n(2), ". ");
        assert_eq!(mid.text(), ".");
        assert_eq!(mid.source_start_here(), Some(2));
        assert_eq!(mid.prev_byte(), Some(b'b'));
    }

    #[test]
    fn rest_source_respects_with_limit_at() {
        // Tokens: TEXT "aa", WHITESPACE " ", TEXT "bb". Cap after first TEXT.
        let src = "aa bb";
        let mut p = make_parser(src);
        assert_eq!(p.current(), OrgSyntaxKind::TEXT);
        let after_first = p
            .token_view(p.cursor())
            .and_then(|t| t.nth(1))
            .expect("ws")
            .pos();
        p.with_limit_at(after_first, |pp| {
            assert_eq!(pp.rest_source(), "aa");
            assert!(!pp.rest_source().contains('b'));
            assert_eq!(pp.source_of_n(1), "aa");
            assert_eq!(pp.source_of_n(99), "aa");
        });
        assert!(p.rest_source().contains("bb"));
    }

    #[test]
    fn source_start_here_none_at_eof() {
        let mut p = make_parser("x");
        p.eat_any();
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        assert_eq!(p.source_start_here(), None);
        assert_eq!(p.text(), "");
        assert_eq!(p.rest_source(), "");
    }

    #[test]
    fn multibyte_text_via_token_view() {
        // "cafe:tag" with multi-byte e-acute: TEXT(...) COLON TEXT("tag")
        let source = "caf\u{e9}:tag";
        let parser = make_parser(source);
        let colon = parser
            .tokens_from_cursor()
            .find(|t| t.kind() == OrgSyntaxKind::COLON)
            .expect("COLON not found");
        let colon_start = usize::from(parser.token_range(colon.pos().index()).start());
        assert_eq!(parser.source().as_bytes()[colon_start - 1], 0xA9);
        assert_eq!(colon.text(), ":");
        assert_eq!(colon.next().expect("tag").text(), "tag");
    }
}
