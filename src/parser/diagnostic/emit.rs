//! Recording diagnostics: `Parser` helpers and the range builders they use.
//!
//! - **Diagnostics survive backtracking.** [`Parser::attempt`] rewinds the
//!   cursor, events, limit and inline region, but not diagnostics, so a
//!   failed alternative keeps whatever it reported.
//! - **Grammar rules always report** a near miss, without checking whether
//!   it was already reported.
//! - **Duplicates are dropped here.** [`Parser::error_range`] is the only
//!   place a diagnostic is pushed; it skips one whose kind (string fields
//!   included) and range equal an existing one. This matters when a failed
//!   construct is re-parsed as something else (a bad clock line becomes a
//!   paragraph) and hits the same near miss twice.
//!
//! The duplicate check scans the list linearly; diagnostic counts stay
//! small.

use rowan::{TextRange, TextSize};

use super::super::Parser;
use super::kind::{Diagnostic, DiagnosticKind};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Parser emission + range helpers
// ---------------------------------------------------------------------------

impl Parser<'_> {
    /// Whether a new diagnostic with this exact kind and range should be pushed.
    ///
    /// Duplicates have an equal [`DiagnosticKind`] (string fields included)
    /// and an equal range.
    #[inline]
    fn should_emit(&self, kind: &DiagnosticKind, range: TextRange) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.kind() == kind && d.range() == range)
    }

    /// Record a diagnostic at an explicit source range (low-level sink).
    ///
    /// Every other helper calls this. Skips a duplicate (see module docs).
    pub(crate) fn error_range(&mut self, range: TextRange, kind: DiagnosticKind) {
        if !self.should_emit(&kind, range) {
            return;
        }
        self.diagnostics.push(Diagnostic::new(kind, range));
    }

    /// Diagnostic from construct entry through the current cursor.
    ///
    /// When the cursor has advanced past `start`, covers tokens
    /// `[start, cursor)` (multiline-safe). When still at entry, falls back to
    /// the line span ([`Self::error_line_at`]). Prefer this for constructs
    /// that may span newlines, such as links.
    pub(crate) fn error_span_to_cursor(&mut self, start: super::super::Pos, kind: DiagnosticKind) {
        if self.pos > start.index() {
            let range = self.abs_token_span(start.index(), self.pos);
            self.error_range(range, kind);
        } else {
            self.error_line_at(start, kind);
        }
    }

    /// Record a diagnostic covering the next `count` tokens from the cursor.
    ///
    /// For multi-token constructs recognised but rejected before any eats
    /// (priority cookie `[#VALUE]`).
    pub(crate) fn error_ahead(&mut self, count: usize, kind: DiagnosticKind) {
        let range = self.abs_token_span(self.pos, self.pos + count);
        self.error_range(range, kind);
    }

    /// Record a diagnostic covering the line from position `at` through the
    /// last non-NEWLINE token (NEWLINE excluded).
    ///
    /// `at` usually comes from [`Parser::cursor`] captured at construct entry.
    pub(crate) fn error_line_at(&mut self, at: super::super::Pos, kind: DiagnosticKind) {
        let range = self.abs_line_span(at.index());
        self.error_range(range, kind);
    }

    /// Record a diagnostic covering tokens `[a, b)` (typed positions).
    pub(crate) fn error_span(
        &mut self,
        a: super::super::Pos,
        b: super::super::Pos,
        kind: DiagnosticKind,
    ) {
        let range = self.abs_token_span(a.index(), b.index());
        self.error_range(range, kind);
    }

    /// Contiguous source range covering absolute token indices `[start, end)`.
    fn abs_token_span(&self, start: usize, end: usize) -> TextRange {
        if start >= self.tokens.len() || start >= end {
            return self.range_at_token_or_eof(start);
        }
        let end = end.min(self.tokens.len());
        let from = self.tokens[start].range.start();
        let to = self.tokens[end - 1].range.end();
        TextRange::new(from, to)
    }

    /// Line from absolute token `start` through the last non-NEWLINE token
    /// (NEWLINE excluded). Respects the active visible window.
    fn abs_line_span(&self, start: usize) -> TextRange {
        if start >= self.tokens.len() {
            return self.eof_empty_range();
        }
        let from = self.tokens[start].range.start();
        let mut i = start;
        let limit = self.limit.min(self.tokens.len());
        while i < limit && self.tokens[i].kind != OrgSyntaxKind::NEWLINE {
            i += 1;
        }
        if i == start {
            // Only a NEWLINE (or empty): point at that token.
            return self.tokens[start].range;
        }
        let to = self.tokens[i - 1].range.end();
        TextRange::new(from, to)
    }

    fn range_at_token_or_eof(&self, abs: usize) -> TextRange {
        if let Some(t) = self.tokens.get(abs) {
            t.range
        } else {
            self.eof_empty_range()
        }
    }

    fn eof_empty_range(&self) -> TextRange {
        // Empty range at the end of source, not at file start (0..0 would
        // mis-point diagnostics). The cast cannot truncate: the parser rejects
        // sources longer than `u32::MAX` bytes on construction.
        let end = TextSize::from(self.source.len() as u32);
        TextRange::new(end, end)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::diagnostic::Severity;
    use rowan::TextRange;

    fn range_text(src: &str, r: TextRange) -> &str {
        let s = usize::from(r.start());
        let e = usize::from(r.end());
        &src[s..e]
    }

    #[test]
    fn abs_line_span_excludes_newline() {
        let src = "foo bar\nnext\n";
        let p = Parser::with_config(src, crate::tests::default_config());
        assert_eq!(range_text(src, p.abs_line_span(0)), "foo bar");
    }

    #[test]
    fn abs_line_span_without_trailing_newline() {
        let src = "foo bar";
        let p = Parser::with_config(src, crate::tests::default_config());
        assert_eq!(range_text(src, p.abs_line_span(0)), "foo bar");
    }

    #[test]
    fn error_line_at_cursor_covers_line_and_records_kind() {
        let src = "CLOCK: text\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        p.error_line_at(p.cursor(), DiagnosticKind::MalformedClock);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert!(matches!(diags[0].kind(), DiagnosticKind::MalformedClock));
        assert_eq!(diags[0].severity(), Severity::Error);
        assert_eq!(range_text(src, diags[0].range()), "CLOCK: text");
        assert_eq!(format!("{}", diags[0]), diags[0].message().as_ref());
    }

    #[test]
    fn error_range_dedupes_same_kind_and_range() {
        // Direct sink API: double error_range with equal kind + range pushes once.
        let src = "foo bar";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let range = p.abs_line_span(0);
        p.error_range(range, DiagnosticKind::MalformedTimestamp);
        p.error_range(range, DiagnosticKind::MalformedTimestamp);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1, "same kind+range must dedupe: {diags:?}");
        assert!(matches!(
            diags[0].kind(),
            DiagnosticKind::MalformedTimestamp
        ));
        assert_eq!(diags[0].range(), range);
    }

    #[test]
    fn error_range_keeps_different_kind_or_range() {
        // Different kind, same range: both kept.
        let src = "foo bar\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let range = p.abs_line_span(0);
        p.error_range(range, DiagnosticKind::MalformedTimestamp);
        p.error_range(range, DiagnosticKind::MalformedClock);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 2, "different kinds must both push: {diags:?}");
        assert!(matches!(
            diags[0].kind(),
            DiagnosticKind::MalformedTimestamp
        ));
        assert!(matches!(diags[1].kind(), DiagnosticKind::MalformedClock));

        // Same unit kind, different ranges: both kept (isolates the range half
        // of the dedupe key; payload-bearing kinds would also differ by field).
        let src2 = "alpha beta\n";
        let mut p2 = Parser::with_config(src2, crate::tests::default_config());
        // "alpha" then after advance "beta"
        p2.error_ahead(1, DiagnosticKind::MalformedTimestamp);
        p2.eat_any(); // TEXT "alpha"
        p2.eat_any(); // WHITESPACE
        p2.error_ahead(1, DiagnosticKind::MalformedTimestamp);
        let diags2 = p2.finish().2;
        assert_eq!(
            diags2.len(),
            2,
            "same kind, different ranges must both push: {diags2:?}"
        );
        let r0 = diags2[0].range();
        let r1 = diags2[1].range();
        assert_ne!(r0, r1);
        assert_eq!(range_text(src2, r0), "alpha");
        assert_eq!(range_text(src2, r1), "beta");
    }

    #[test]
    fn error_range_dedupe_uses_exact_kind_including_string_fields() {
        // Same range, same variant shape, different string payloads: both kept.
        let src = "foo bar\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let range = p.abs_line_span(0);
        p.error_range(range, DiagnosticKind::planning_missing("SCHEDULED"));
        p.error_range(range, DiagnosticKind::planning_missing("DEADLINE"));
        let diags = p.finish().2;
        assert_eq!(
            diags.len(),
            2,
            "string fields participate in dedupe key: {diags:?}"
        );

        // Same string payload + same range: collapse to one.
        let mut p2 = Parser::with_config(src, crate::tests::default_config());
        let range2 = p2.abs_line_span(0);
        p2.error_range(range2, DiagnosticKind::planning_missing("SCHEDULED"));
        p2.error_range(range2, DiagnosticKind::planning_missing("SCHEDULED"));
        let diags2 = p2.finish().2;
        assert_eq!(
            diags2.len(),
            1,
            "identical kind+string+range must dedupe: {diags2:?}"
        );
        assert!(matches!(
            diags2[0].kind(),
            DiagnosticKind::PlanningMissingTimestamp { keyword } if keyword == "SCHEDULED"
        ));
    }

    #[test]
    fn error_line_at_covers_line_after_cursor_moves() {
        let src = "CLOCK: text\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let start = p.cursor();
        // Advance past CLOCK so the fail site is not the current cursor.
        p.eat_any(); // TEXT "CLOCK"
        p.error_line_at(start, DiagnosticKind::MalformedClock);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert!(matches!(diags[0].kind(), DiagnosticKind::MalformedClock));
        assert_eq!(range_text(src, diags[0].range()), "CLOCK: text");
    }

    #[test]
    fn error_line_at_eof_uses_empty_end_range() {
        let mut p = Parser::with_config("", crate::tests::default_config());
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        p.error_line_at(p.cursor(), DiagnosticKind::UnterminatedLink);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert!(matches!(diags[0].kind(), DiagnosticKind::UnterminatedLink));
        assert_eq!(diags[0].range().start(), diags[0].range().end());
        assert_eq!(u32::from(diags[0].range().start()), 0);
    }

    #[test]
    fn error_line_at_without_trailing_newline() {
        let src = "foo bar";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        p.error_line_at(p.cursor(), DiagnosticKind::MalformedTimestamp);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert!(matches!(
            diags[0].kind(),
            DiagnosticKind::MalformedTimestamp
        ));
        assert_eq!(range_text(src, diags[0].range()), "foo bar");
    }

    #[test]
    fn error_ahead_covers_token_span() {
        // Tokens: `[` `#` `a` `]` NEWLINE -> invalid priority cookie shape range.
        let src = "[#a]\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        p.error_ahead(4, DiagnosticKind::invalid_priority("a"));
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert!(matches!(
            diags[0].kind(),
            DiagnosticKind::InvalidPriorityCookie { value } if value == "a"
        ));
        assert_eq!(range_text(src, diags[0].range()), "[#a]");
    }

    #[test]
    fn error_span_covers_entry_to_cursor_after_advance() {
        // Capture the entry position, eat `[` `#`, then span [entry, cursor).
        let src = "[#a]\n";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let entry = p.cursor();
        p.eat_any(); // `[`
        p.eat_any(); // `#`
        p.error_span(entry, p.cursor(), DiagnosticKind::invalid_priority("a"));
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert_eq!(range_text(src, diags[0].range()), "[#");
    }

    #[test]
    fn error_span_to_cursor_span_and_line_fallback() {
        // Cursor advanced: span from entry through cursor (multiline-safe).
        let src = "[[t\nx";
        let mut p = Parser::with_config(src, crate::tests::default_config());
        let entry = p.cursor();
        p.eat_any(); // `[`
        p.eat_any(); // `[`
        p.eat_any(); // TEXT "t"
        p.error_span_to_cursor(entry, DiagnosticKind::UnterminatedLink);
        let diags = p.finish().2;
        assert_eq!(diags.len(), 1);
        assert_eq!(range_text(src, diags[0].range()), "[[t");

        // Cursor still at entry: line-span fallback (newline excluded).
        let src2 = "CLOCK: text\n";
        let mut p2 = Parser::with_config(src2, crate::tests::default_config());
        p2.error_span_to_cursor(p2.cursor(), DiagnosticKind::MalformedClock);
        let diags2 = p2.finish().2;
        assert_eq!(diags2.len(), 1);
        assert_eq!(range_text(src2, diags2[0].range()), "CLOCK: text");
    }
}
