//! Timestamp object grammar.
//!
//! Implements recognition and parsing for all timestamp forms per org-syntax
//! section 5.16 and org-element-timestamp-parser, adapted to the token-stream /
//! event-emitting CST model.
//!
//! Emits:
//!   TIMESTAMP (the full bracketed form, brackets included)
//!     DATE
//!     TIME (optional point time)
//!     TIME_RANGE (for ranges like 10:00-11:30)
//!     ROD (repeater or delay clause; emitted per recognised clause)
//!   or for diary forms (body emit in [`super::diary`]; optional time here):
//!   TIMESTAMP
//!     DIARY_SEXP
//!     [TIME | TIME_RANGE] (optional; carved when TIME shape follows the sexp)
//!
//! Entry points:
//! - Inline: the pure [`probe`] returns a [`TimestampShape`] that records only
//!   the opener (`active`); the interior is measured at emit. When [`parse`]
//!   fails it leaves the opener as a bare token instead of rewinding.
//! - `try_parse_timestamp` (element contexts such as planning) rewinds cursor
//!   and events fully on failure and reports a [`TimestampForm`] on success.
//! - `try_parse_inactive_timestamp` has the same contract but accepts only
//!   inactive `[...]` (the clock body). `[[` is excluded by the broad opener
//!   check, not by the inline date probe.
//!
//! ## Measure model
//!
//! [`TimestampShape`] records only the opener polarity. The full layout is
//! too large for a Copy probe, so pure observers over [`TokenView`] measure
//! the parts (date text, weekday, wall time or range, ROD, diary lead,
//! same-line closer) while emitting. The wrappers choose how to fail: leave a
//! bare opener, or rewind to a checkpoint.
//!
//! A near miss or a diary failure always records a diagnostic. Diagnostics
//! are *sticky*: they survive a rewind, and `parser::diagnostic::emit`
//! deduplicates repeats.

use crate::DiagnosticKind;
use crate::parser::{Parser, TokenView};
use crate::syntax::OrgSyntaxKind;

use super::diary;
use crate::parser::grammar::drive::atoms;
use atoms::is_digits;

// ---------------------------------------------------------------------------
// Shape constants
// ---------------------------------------------------------------------------

const DATE_LEN: usize = 10; // YYYY-MM-DD
const MAX_HOUR_DIGITS: usize = 2;
const MAX_ROD_VALUE_DIGITS: usize = 16;

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Point stamp vs `--` range stamp (mirrors org-element value shape).
///
/// Internal wall-clock ranges (`HH:MM-HH:MM` inside one pair of brackets) are
/// still [`TimestampForm::Point`]; only a second stamp after `--` is Range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimestampForm {
    Point,
    Range,
}

/// Shape for the inline timestamp row.
///
/// Carries only the opener polarity (`active`). The interior (diary or
/// regular, date/time/ROD) is resolved at emit, as it is too large for a
/// cheap Copy probe payload (see the module's measure model).
///
/// Probe gate (must not steal angle links): `L_ANGLE` when diary lead or
/// date-shaped after optional WS; `L_BRACKET` when date-shaped and not `[[`.
/// Element contexts that need any stamp-shaped opener use
/// [`at_timestamp_opener`] / [`at_timestamp_opener_after_ws`] then
/// [`try_parse_timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimestampShape {
    /// `true` when the opener is `L_ANGLE` (active / diary).
    pub(crate) active: bool,
}

/// Pure probe: date-shaped interior or diary lead at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<TimestampShape> {
    let t = parser.here()?;
    match t.kind() {
        OrgSyntaxKind::L_ANGLE => {
            if diary::lead_at(t.nth(1)) || token_after_ws(t.nth(1)).is_some_and(looks_like_date_tok)
            {
                Some(TimestampShape { active: true })
            } else {
                None
            }
        }
        // Date-shaped only (same interior gate as the active arm). A broad
        // non-`[[` claim would steal cookies / footnotes and force table order.
        OrgSyntaxKind::L_BRACKET => (t.kind_at(1) != OrgSyntaxKind::L_BRACKET
            && token_after_ws(t.nth(1)).is_some_and(looks_like_date_tok))
        .then_some(TimestampShape { active: false }),
        _ => None,
    }
}

/// Date-shaped or diary lead at the cursor (inline object gate).
#[cfg(test)]
#[inline]
pub(crate) fn at_timestamp(parser: &Parser) -> bool {
    probe(parser).is_some()
}

/// Broad stamp opener at the cursor: any `<` or non-`[[` `[`.
pub(crate) fn at_timestamp_opener(parser: &Parser) -> bool {
    parser.here().is_some_and(is_timestamp_opener_tok)
}

/// Broad stamp opener after optional leading `WHITESPACE` at the cursor.
///
/// Used by planning after `KEYWORD:` so optional space before the stamp is
/// not treated as "missing timestamp".
pub(crate) fn at_timestamp_opener_after_ws(parser: &Parser) -> bool {
    token_after_ws(parser.here()).is_some_and(is_timestamp_opener_tok)
}

/// Emit a timestamp from its probe shape (inline path).
///
/// Uses `shape.active` for opener polarity; interior is measured at emit.
/// On success, emits a TIMESTAMP node (and sub-nodes) and advances past it.
/// On failure, the opener is left as a bare token and the cursor advances past
/// at least the opener. Prefer [`try_parse_timestamp`] when the caller must
/// fully rewind on failure (element contexts such as planning).
///
/// Near-miss failures always record diagnostics (sticky; see module docs).
pub(crate) fn parse(parser: &mut Parser, shape: TimestampShape) {
    let _ = parse_timestamp_inner(parser, shape.active);
}

/// Harness: probe then parse (unit tests).
#[cfg(test)]
pub(crate) fn parse_timestamp(parser: &mut Parser) {
    if let Some(shape) = probe(parser) {
        parse(parser, shape);
    }
}

/// Try to parse a timestamp at the cursor for element contexts.
///
/// Success: TIMESTAMP emitted; returns point vs `--` range. Failure: full
/// rewind of cursor and events; diagnostics survive the rewind. Accepts
/// active, inactive, and diary openers. Prefer
/// [`try_parse_inactive_timestamp`] when only `[...]` is legal (clock body).
pub(crate) fn try_parse_timestamp(parser: &mut Parser) -> Option<TimestampForm> {
    // Broad opener: planning treats `<not-a-date>` as a failed stamp attempt
    // (silent, as it is not a near miss), not as "missing timestamp".
    if !at_timestamp_opener(parser) {
        return None;
    }
    let active = parser.current() == OrgSyntaxKind::L_ANGLE;
    parser.attempt(|p| parse_timestamp_inner(p, active))
}

/// Try to parse an inactive (`[...]`) timestamp at the cursor.
///
/// Shape-gated: returns `None` without consuming when the cursor is not on
/// `L_BRACKET`. `[[` is excluded by [`at_timestamp_opener`] inside
/// [`try_parse_timestamp`] (not the inline date probe). Same contract as
/// [`try_parse_timestamp`]: full rewind of cursor/events on failure; near-miss
/// diagnostics still emit and survive rewind.
///
/// Used by clock, which accepts only inactive stamps. Active `<...>` is not a
/// clock stamp body; callers fall through without a nested stamp diag from this
/// gate (active openers never enter the inner body).
pub(crate) fn try_parse_inactive_timestamp(parser: &mut Parser) -> Option<TimestampForm> {
    if parser.current() != OrgSyntaxKind::L_BRACKET {
        return None;
    }
    try_parse_timestamp(parser)
}

/// Shared body for [`parse`] and the try APIs.
///
/// On failure may leave a bare opener (and partial tokens after it); the try
/// API rewinds via checkpoint, the inline path keeps bare-opener semantics.
/// Near-miss / diary failures always emit.
fn parse_timestamp_inner(parser: &mut Parser, active: bool) -> Option<TimestampForm> {
    let start = parser.cursor();
    debug_assert_eq!(
        parser.current() == OrgSyntaxKind::L_ANGLE,
        active,
        "timestamp shape active must match cursor opener"
    );
    let marker = parser.start();
    parser.eat_any(); // opener

    // Diary is active-only: `<%%(...)>`, never `[%%(...)`.
    // Fail ranges cover the line from the opener (stamps do not span newlines).
    // Stamp body lives in `forms/diary`; optional time and drain stay here.
    let form = if active && diary::lead_at(parser.here()) {
        if let Some(outer_end) = diary::try_parse_stamp_body(parser) {
            parser.with_limit_at(outer_end, |p| {
                p.eat_run(OrgSyntaxKind::WHITESPACE);
                opt_time_then_ws(p);
                p.eat_to_limit();
            });
            Some(TimestampForm::Point)
        } else {
            parser.error_line_at(start, DiagnosticKind::UnbalancedDiarySexp);
            None
        }
    } else {
        // Near-miss only: silence bare `a < b` and non-date `[fn:...]` forms.
        // Optional WS so the gate matches interior (`eat_run(WHITESPACE)` then
        // date shape).
        let near_miss = token_after_ws(parser.here()).is_some_and(looks_like_date_tok);
        let form = try_parse_regular(parser, active);
        if form.is_none() && near_miss {
            parser.error_line_at(start, DiagnosticKind::MalformedTimestamp);
        }
        form
    };

    if let Some(f) = form {
        marker.complete(parser, OrgSyntaxKind::TIMESTAMP);
        Some(f)
    } else {
        marker.abandon(parser);
        None
    }
}

// ---------------------------------------------------------------------------
// Regular (and `--` ranges)
// ---------------------------------------------------------------------------

/// Regular stamp body after the opener was eaten.
///
/// Returns the form from the parse path: Range only when `--` + second stamp
/// is fully consumed; otherwise Point (including after a failed range tail that
/// rewinds to just past the first stamp).
fn try_parse_regular(parser: &mut Parser, active: bool) -> Option<TimestampForm> {
    let (open, close) = stamp_brackets(active);
    if !try_parse_one_stamp(parser, close) {
        return None;
    }

    // Optional `--` second stamp; failed attempt rewinds to just after the first.
    if parser
        .attempt(|p| try_parse_range_tail(p, open, close).then_some(()))
        .is_some()
    {
        return Some(TimestampForm::Range);
    }
    Some(TimestampForm::Point)
}

/// Attempt `--<second>` / `--[second]` after the first stamp's closer.
/// Returns false without requiring the caller to rewind (use under `attempt`).
fn try_parse_range_tail(parser: &mut Parser, open: OrgSyntaxKind, close: OrgSyntaxKind) -> bool {
    parser.eat_run(OrgSyntaxKind::WHITESPACE);
    if parser.current() != OrgSyntaxKind::TEXT || parser.text() != "--" {
        return false;
    }
    parser.eat_any(); // --
    if parser.current() != open {
        return false;
    }
    parser.eat_any(); // second opener
    try_parse_one_stamp(parser, close)
}

/// Interior + matching closer for one regular stamp. Leaves the cursor past the closer.
fn try_parse_one_stamp(parser: &mut Parser, close: OrgSyntaxKind) -> bool {
    let Some(closer) = parser.find_same_line_kind(close).map(|t| t.pos()) else {
        return false;
    };
    if !parser.with_limit_at(closer, try_parse_regular_stamp_interior) {
        return false;
    }
    debug_assert_eq!(parser.current(), close);
    parser.eat_any(); // closer
    true
}

fn stamp_brackets(active: bool) -> (OrgSyntaxKind, OrgSyntaxKind) {
    if active {
        (OrgSyntaxKind::L_ANGLE, OrgSyntaxKind::R_ANGLE)
    } else {
        (OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET)
    }
}

// ---------------------------------------------------------------------------
// Shared interior (one regular stamp between opener and closer)
// ---------------------------------------------------------------------------

/// Parse date / weekday / time / ROD under a limit at the closer.
/// Returns true when a DATE was recognised (required for a valid regular stamp).
/// Trailing junk under the limit is eaten for losslessness.
fn try_parse_regular_stamp_interior(parser: &mut Parser) -> bool {
    parser.eat_run(OrgSyntaxKind::WHITESPACE);

    if !looks_like_date_here(parser) {
        parser.eat_to_limit();
        return false;
    }
    parser.leaf(OrgSyntaxKind::DATE, 1);
    parser.eat_run(OrgSyntaxKind::WHITESPACE);

    // Optional weekday: bare token, never a subnode.
    if is_weekday_here(parser) {
        parser.eat_any();
        parser.eat_run(OrgSyntaxKind::WHITESPACE);
    }

    opt_time_then_ws(parser);
    parse_rods(parser);
    parser.eat_to_limit();
    true
}

/// At most one repeater and one delay, in either order.
fn parse_rods(parser: &mut Parser) {
    let mut repeater_seen = false;
    let mut delay_seen = false;
    loop {
        parser.eat_run(OrgSyntaxKind::WHITESPACE);
        if parser.current() == OrgSyntaxKind::EOF {
            break;
        }
        if !repeater_seen && let Some(len) = parser.here().and_then(probe_repeater) {
            parser.leaf(OrgSyntaxKind::ROD, len);
            repeater_seen = true;
            continue;
        }
        if !delay_seen && let Some(len) = parser.here().and_then(probe_delay) {
            parser.leaf(OrgSyntaxKind::ROD, len);
            delay_seen = true;
            continue;
        }
        break;
    }
}

// ---------------------------------------------------------------------------
// Time sequence (lexer impedance: `:` is COLON, `-` stays inside TEXT)
// ---------------------------------------------------------------------------

/// Optional wall time / range, then whitespace if matched.
fn opt_time_then_ws(parser: &mut Parser) {
    if try_parse_time_sequence(parser) {
        parser.eat_run(OrgSyntaxKind::WHITESPACE);
    }
}

/// Shape-level wall time / shorthand range at the cursor.
///
/// Token layout:
/// - point:  `TEXT(hour) COLON TEXT(min...)`
/// - range:  `TEXT(hour) COLON TEXT("MM-H...") COLON TEXT(min2)`
///
/// `"10:00:30"` (seconds) is TIME for the first three tokens only; the rest is
/// left bare (tolerant). Range vs point is decided before consuming.
fn try_parse_time_sequence(parser: &mut Parser) -> bool {
    let Some(t) = parser.here() else {
        return false;
    };
    if t.kind() != OrgSyntaxKind::TEXT || !is_hour_text(t.text()) {
        return false;
    }
    if t.kind_at(1) != OrgSyntaxKind::COLON {
        return false;
    }
    if t.kind_at(2) != OrgSyntaxKind::TEXT {
        return false;
    }
    let mid = t.text_at(2);
    if !starts_with_digit(mid) {
        return false;
    }

    let as_range = is_glued_range_middle(mid)
        && t.kind_at(3) == OrgSyntaxKind::COLON
        && t.kind_at(4) == OrgSyntaxKind::TEXT
        && starts_with_digit(t.text_at(4));

    let kind = if as_range {
        OrgSyntaxKind::TIME_RANGE
    } else {
        OrgSyntaxKind::TIME
    };
    let n = if as_range { 5 } else { 3 };
    parser.leaf(kind, n);
    true
}

fn is_hour_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_HOUR_DIGITS && is_digits(s)
}

/// Middle TEXT of a shorthand range: minutes, `-`, hour digits (`"00-11"`).
fn is_glued_range_middle(s: &str) -> bool {
    let Some((mins, hour)) = s.split_once('-') else {
        return false;
    };
    is_digits(mins) && is_digits(hour)
}

// ---------------------------------------------------------------------------
// Date / weekday / opener shape (TokenView)
// ---------------------------------------------------------------------------

fn is_timestamp_opener_tok(t: TokenView<'_, '_>) -> bool {
    match t.kind() {
        OrgSyntaxKind::L_ANGLE => true,
        OrgSyntaxKind::L_BRACKET => t.kind_at(1) != OrgSyntaxKind::L_BRACKET,
        _ => false,
    }
}

/// First non-`WHITESPACE` token at or after `start`.
fn token_after_ws<'a, 's>(start: Option<TokenView<'a, 's>>) -> Option<TokenView<'a, 's>> {
    let t = start?;
    let n = t.run_while(0, |kind, _| kind == OrgSyntaxKind::WHITESPACE);
    t.nth(n)
}

fn looks_like_date_here(parser: &Parser) -> bool {
    parser.here().is_some_and(looks_like_date_tok)
}

fn looks_like_date_tok(t: TokenView<'_, '_>) -> bool {
    if t.kind() != OrgSyntaxKind::TEXT {
        return false;
    }
    looks_like_date_text(t.text())
}

fn looks_like_date_text(s: &str) -> bool {
    let b = s.as_bytes();
    // Exact length: overlong glued TEXT (`2026-06-21foo`) is not a date.
    if b.len() != DATE_LEN {
        return false;
    }
    b[4] == b'-'
        && b[7] == b'-'
        && is_digit_slice(&b[..4])
        && is_digit_slice(&b[5..7])
        && is_digit_slice(&b[8..10])
}

fn is_weekday_here(parser: &Parser) -> bool {
    parser.here().is_some_and(is_weekday_tok)
}

fn is_weekday_tok(t: TokenView<'_, '_>) -> bool {
    t.kind() == OrgSyntaxKind::TEXT && is_weekday_text(t.text())
}

/// Whether `s` is a weekday atom inside a timestamp (not a ROD lead or hour).
///
/// The typed views call this too instead of re-deriving the predicate.
#[must_use]
pub(crate) fn is_weekday_text(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Restart lead (`.+`), delay lead (`-Nunit`), pure digit hour fragment.
    if s == "." || s.starts_with('-') || is_digits(s) {
        return false;
    }
    if s.bytes()
        .any(|c| matches!(c, b'<' | b'>' | b'[' | b']' | b'\n'))
    {
        return false;
    }
    // A weekday is a name, not a pure number.
    s.bytes().any(|c| c.is_ascii_alphabetic())
}

// ---------------------------------------------------------------------------
// ROD (repeater / delay) - pure measure from a TokenView
// ---------------------------------------------------------------------------

/// Exact `VALUE UNIT` with value > 0 (no trailing junk).
/// Units: h d w m y. Caps digit run length for hostile input.
fn is_exact_rod_value(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_ROD_VALUE_DIGITS {
        return false;
    }
    let b = s.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    // Need at least one digit and exactly one unit char with nothing after.
    if digits == 0 || digits + 1 != b.len() {
        return false;
    }
    if !matches!(b[digits], b'h' | b'd' | b'w' | b'm' | b'y') {
        return false;
    }
    s[..digits].parse::<u32>().is_ok_and(|v| v > 0)
}

/// Token length of a repeater at `t`, including optional habit deadline.
///
/// Lexer layout:
/// - `+Nunit`  -> PLUS TEXT
/// - `++Nunit` -> PLUS PLUS TEXT
/// - `.+Nunit` -> TEXT(".") PLUS TEXT
/// - habit:    after value, SLASH TEXT(`Nunit`)  e.g. `.+2d/4d`
fn probe_repeater(t: TokenView<'_, '_>) -> Option<usize> {
    use OrgSyntaxKind::{PLUS, SLASH, TEXT};
    let (value_idx, base_len) = if t.seq(&[PLUS, PLUS, TEXT]) {
        // ++VALUE
        (2usize, 3usize)
    } else if t.seq(&[PLUS, TEXT]) {
        // +VALUE
        (1usize, 2usize)
    } else if t.text() == "." && t.seq(&[TEXT, PLUS, TEXT]) {
        // .+VALUE
        (2usize, 3usize)
    } else {
        return None;
    };
    if !is_exact_rod_value(t.text_at(value_idx)) {
        return None;
    }
    // Habit deadline: after the value token, optional `/VALUE`.
    let habit = if t.kind_at(base_len) == SLASH
        && t.kind_at(base_len + 1) == TEXT
        && is_exact_rod_value(t.text_at(base_len + 1))
    {
        2
    } else {
        0
    };
    Some(base_len + habit)
}

/// Delay is one TEXT token: `-` is not a lexer delimiter.
fn probe_delay(t: TokenView<'_, '_>) -> Option<usize> {
    if t.kind() != OrgSyntaxKind::TEXT {
        return None;
    }
    let rest = t
        .text()
        .strip_prefix("--")
        .or_else(|| t.text().strip_prefix("-"))?;
    is_exact_rod_value(rest).then_some(1)
}

// ---------------------------------------------------------------------------
// Digit helpers
// ---------------------------------------------------------------------------

fn is_digit_slice(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(u8::is_ascii_digit)
}

fn starts_with_digit(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        ParseResult, diags_any, make_parser, node_count, node_kinds, node_text, run, run_object,
    };

    /// Guarded object collector: parse when `at_timestamp`, else eat one token.
    fn results_of(src: &str) -> ParseResult {
        run_object(src, probe, parse)
    }

    /// Domain convenience: true if a single guarded parse emitted TIMESTAMP.
    fn has_ts(src: &str) -> bool {
        node_kinds(&results_of(src).1).contains(&OrgSyntaxKind::TIMESTAMP)
    }

    /// Domain convenience: text of the first ROD under a single guarded parse.
    fn rod_text(src: &str) -> Option<String> {
        let (_toks, evs, _) = results_of(src);
        node_text(&evs, src, OrgSyntaxKind::ROD)
    }

    /// Assert one TIMESTAMP node whose text equals the whole `src` (single parse).
    /// Returns the parse result so callers can layer ROD / kind checks without
    /// re-parsing or re-asserting the full-stamp contract.
    fn assert_full_ts(src: &str) -> ParseResult {
        let result = results_of(src);
        let (ref _toks, ref evs, _) = result;
        assert!(
            node_kinds(evs).contains(&OrgSyntaxKind::TIMESTAMP),
            "expected TIMESTAMP for {src:?}"
        );
        assert_eq!(
            node_text(evs, src, OrgSyntaxKind::TIMESTAMP).as_deref(),
            Some(src)
        );
        result
    }

    // --- Basic forms -------------------------------------------------------

    #[test]
    fn basic_active_date() {
        assert_full_ts("<2026-06-21>");
    }

    #[test]
    fn basic_inactive_date() {
        assert_full_ts("[2026-06-21]");
    }

    #[test]
    fn with_weekday() {
        assert_full_ts("<2026-06-21 Sun>");
    }

    #[test]
    fn with_time() {
        let src = "<2026-06-21 14:30>";
        let kinds = node_kinds(&results_of(src).1);
        assert!(kinds.contains(&OrgSyntaxKind::TIMESTAMP));
        assert!(kinds.contains(&OrgSyntaxKind::DATE));
        assert!(kinds.contains(&OrgSyntaxKind::TIME));
    }

    #[test]
    fn with_repeater() {
        let src = "<2026-06-21 +1w>";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIMESTAMP));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::ROD).as_deref(),
            Some("+1w")
        );
    }

    #[test]
    fn with_delay() {
        let src = "<2026-06-21 -2d>";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIMESTAMP));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::ROD).as_deref(),
            Some("-2d")
        );
    }

    #[test]
    fn combined_repeater_delay() {
        let src = "<2030-10-05 Sat +1m -3d>";
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::ROD), 2);
    }

    #[test]
    fn range_via_dash_dash() {
        let src = "<2026-06-10>--<2026-06-11>";
        assert_eq!(node_count(&results_of(src).1, OrgSyntaxKind::DATE), 2);
    }

    #[test]
    fn time_range_shorthand() {
        let src = "<2026-06-21 10:00-11:30>";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIMESTAMP));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIME_RANGE).as_deref(),
            Some("10:00-11:30")
        );
    }

    // --- Diary shell (full trees are tested in diary.rs) --------------------

    #[test]
    fn diary_incomplete_sexp_no_timestamp() {
        // Shell owns UnbalancedDiarySexp; body helper is silent (see diary).
        assert!(!has_ts("<%%(foo"));
        let (_, evs, diags) = results_of("<%%(foo");
        assert!(
            !node_kinds(&evs).contains(&OrgSyntaxKind::DIARY_SEXP),
            "failed diary stamp must not emit DIARY_SEXP: {evs:?}"
        );
        assert!(diags_any(&diags, |k| {
            matches!(k, crate::DiagnosticKind::UnbalancedDiarySexp)
        }));
    }

    #[test]
    fn diary_no_outer_closer_no_timestamp() {
        assert!(!has_ts("<%%(foo) 12:00"));
        let (_, evs, diags) = results_of("<%%(foo) 12:00");
        assert!(
            !node_kinds(&evs).contains(&OrgSyntaxKind::DIARY_SEXP),
            "failed diary stamp must not emit DIARY_SEXP: {evs:?}"
        );
        assert!(diags_any(&diags, |k| {
            matches!(k, crate::DiagnosticKind::UnbalancedDiarySexp)
        }));
    }

    #[test]
    fn inactive_bracket_is_never_diary() {
        // Diary lead is active-stamp only; inactive openers never take it.
        assert!(!has_ts("[%%(foo)>"));
        assert!(!has_ts("[%%(diary-float t 4 2)]"));
    }

    // --- Time edge cases ---------------------------------------------------

    #[test]
    fn time_with_seconds_is_point_not_range() {
        let src = "<2026-06-21 10:00:30>";
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::TIME));
        assert!(!kinds.contains(&OrgSyntaxKind::TIME_RANGE));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIME).as_deref(),
            Some("10:00")
        );
    }

    // --- Rejection / tolerance ---------------------------------------------

    #[test]
    fn not_a_timestamp_rejected() {
        assert!(!has_ts("<not-a-date>"));
        assert!(!has_ts("<2026-06-21foo>"));
        // Not date-like (including glued overlong TEXT): no MalformedTimestamp.
        for src in ["<not-a-date>", "<2026-06-21foo>"] {
            let (_, _, diags) = results_of(src);
            assert!(
                !diags_any(&diags, |k| matches!(
                    k,
                    crate::DiagnosticKind::MalformedTimestamp
                )),
                "non-date opener must not emit MalformedTimestamp for {src:?}: {diags:?}"
            );
        }
    }

    #[test]
    fn date_like_unclosed_emits_malformed_timestamp() {
        let src = "<2020-01-01";
        assert!(!has_ts(src));
        let (_, _, diags) = results_of(src);
        assert!(diags_any(&diags, |k| {
            matches!(k, crate::DiagnosticKind::MalformedTimestamp)
        }));
    }

    #[test]
    fn date_like_unclosed_with_space_after_opener_emits() {
        // Near-miss gate skips optional whitespace (aligned with interior).
        for src in ["< 2020-01-01", "[ 2020-01-01"] {
            assert!(
                !has_ts(src),
                "unclosed spaced form is not a TIMESTAMP: {src:?}"
            );
            let (_, _, diags) = results_of(src);
            assert!(
                diags_any(&diags, |k| {
                    matches!(k, crate::DiagnosticKind::MalformedTimestamp)
                }),
                "spaced date-like unclosed must emit MalformedTimestamp for {src:?}: {diags:?}"
            );
        }
        // Successful stamp with space after opener still parses.
        assert_full_ts("< 2020-01-01>");
        assert_full_ts("[ 2020-01-01]");
    }

    #[test]
    fn comparison_angle_is_silent() {
        // `a < b` must not diagnose (PRE/POST-style bare opener).
        let (_, _, diags) = run("a < b\n", |p| {
            while p.current() != OrgSyntaxKind::EOF {
                if at_timestamp(p) {
                    parse_timestamp(p);
                } else {
                    p.eat_any();
                }
            }
        });
        assert!(diags.is_empty(), "comparison must not diagnose: {diags:?}");
    }

    #[test]
    fn trailing_text_after_modifier_keeps_stamp() {
        assert_full_ts("<2026-06-21 +1w x>");
    }

    #[test]
    fn dangling_dash_dash_keeps_first() {
        let src = "<2026-06-10>--";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIMESTAMP));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIMESTAMP).as_deref(),
            Some("<2026-06-10>")
        );
    }

    #[test]
    fn mismatched_range_brackets_keep_first() {
        let src = "<2026-06-10>--[2026-06-11]";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIMESTAMP));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIMESTAMP).as_deref(),
            Some("<2026-06-10>")
        );
    }

    // --- ROD fidelity ------------------------------------------------------

    #[test]
    fn large_repeater_value() {
        assert_eq!(rod_text("<2026-06-21 +365d>").as_deref(), Some("+365d"));
    }

    #[test]
    fn zero_repeater_value_not_carved_as_rod() {
        // 0-value is not a valid ROD; tolerant stamp still emitted.
        let src = "<2026-06-21 +0d>";
        let kinds = node_kinds(&results_of(src).1);
        assert!(kinds.contains(&OrgSyntaxKind::TIMESTAMP));
        assert!(!kinds.contains(&OrgSyntaxKind::ROD));
    }

    #[test]
    fn restart_repeater() {
        assert_eq!(rod_text("<2026-06-21 .+2d>").as_deref(), Some(".+2d"));
    }

    #[test]
    fn catch_up_repeater() {
        assert_eq!(rod_text("<2026-06-21 ++1w>").as_deref(), Some("++1w"));
    }

    #[test]
    fn habit_deadline_included_in_rod() {
        // Full repeater clause including `/4d` is one ROD (core parity).
        let src = "<2026-06-21 .+2d/4d>";
        let (_toks, evs, _) = assert_full_ts(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::ROD).as_deref(),
            Some(".+2d/4d")
        );
    }

    #[test]
    fn cumulative_habit_deadline_included_in_rod() {
        assert_eq!(rod_text("<2026-06-21 +1w/3d>").as_deref(), Some("+1w/3d"));
    }

    #[test]
    fn garbage_suffix_on_value_not_carved_as_rod() {
        // Exact value+unit only; trailing junk fails the ROD shape.
        let src = "<2026-06-21 +2dfoo>";
        let (_toks, evs, _) = assert_full_ts(src);
        let kinds = node_kinds(&evs);
        assert!(!kinds.contains(&OrgSyntaxKind::ROD));
    }

    // --- Isolated helper contracts (timestamp-owned only) ------------------

    #[test]
    fn is_weekday_text_accepts_names_rejects_rods_and_hours() {
        assert!(is_weekday_text("Thu"));
        assert!(is_weekday_text("Wednesday"));
        assert!(!is_weekday_text(""));
        assert!(!is_weekday_text("."));
        assert!(!is_weekday_text("-2d"));
        assert!(!is_weekday_text("09"));
        assert!(!is_weekday_text("2026"));
        assert!(!is_weekday_text("<Thu>"));
    }

    #[test]
    fn try_parse_time_sequence_point() {
        let src = "14:30";
        let (_toks, evs, _) = run(src, |p| {
            assert!(try_parse_time_sequence(p));
        });
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIME).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn try_parse_time_sequence_range() {
        let src = "10:00-11:30";
        let (_toks, evs, _) = run(src, |p| {
            assert!(try_parse_time_sequence(p));
        });
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIME_RANGE).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn try_parse_time_sequence_seconds_is_point() {
        let src = "10:00:30";
        let (_toks, evs, _) = run(src, |p| {
            assert!(try_parse_time_sequence(p));
        });
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TIME));
        assert!(!node_kinds(&evs).contains(&OrgSyntaxKind::TIME_RANGE));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIME).as_deref(),
            Some("10:00")
        );
    }

    #[test]
    fn try_parse_time_sequence_non_match_leaves_cursor() {
        let mut p = make_parser("not-a-time");
        let before = p.cursor().index();
        assert!(!try_parse_time_sequence(&mut p));
        assert_eq!(p.cursor().index(), before);
    }

    #[test]
    fn is_exact_rod_value_shape() {
        assert!(is_exact_rod_value("1d"));
        assert!(is_exact_rod_value("365y"));
        assert!(!is_exact_rod_value("0h"));
        assert!(!is_exact_rod_value("abc"));
        assert!(!is_exact_rod_value(""));
        assert!(!is_exact_rod_value("1x"));
        assert!(!is_exact_rod_value("2dfoo"));
        assert!(!is_exact_rod_value("2d/4d")); // slash is a separate token
    }

    // --- try_parse_timestamp (element-context API) -------------------------

    /// Dense form matrix for the try API. Stamp shapes themselves are covered
    /// by `assert_full_ts` / `results_of` above; this pins Point vs Range
    /// reporting (including internal time-range still Point, diary Point).
    #[test]
    fn try_parse_timestamp_forms() {
        let cases: &[(&str, TimestampForm, Option<OrgSyntaxKind>)] = &[
            ("[2020-01-01 Wed 10:00]", TimestampForm::Point, None),
            ("<2020-01-01>", TimestampForm::Point, None),
            (
                "[2020-01-01 10:00]--[2020-01-01 11:00]",
                TimestampForm::Range,
                None,
            ),
            ("<2020-01-01>--<2020-01-02>", TimestampForm::Range, None),
            (
                "[2020-01-01 Wed 10:00-11:00]",
                TimestampForm::Point,
                Some(OrgSyntaxKind::TIME_RANGE),
            ),
            (
                "<%%(diary-float t 4 2)>",
                TimestampForm::Point,
                Some(OrgSyntaxKind::DIARY_SEXP),
            ),
        ];
        for &(src, form, extra) in cases {
            let (_toks, evs, _) = run(src, |p| {
                assert_eq!(try_parse_timestamp(p), Some(form), "form for {src:?}");
            });
            assert_eq!(
                node_count(&evs, OrgSyntaxKind::TIMESTAMP),
                1,
                "TIMESTAMP for {src:?}"
            );
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::TIMESTAMP).as_deref(),
                Some(src),
                "full span for {src:?}"
            );
            if form == TimestampForm::Range {
                assert_eq!(
                    node_count(&evs, OrgSyntaxKind::DATE),
                    2,
                    "range dates {src:?}"
                );
            }
            if let Some(kind) = extra {
                assert!(
                    node_kinds(&evs).contains(&kind),
                    "expected {kind:?} under {src:?}"
                );
            }
        }
    }

    #[test]
    fn parse_timestamp_vs_try_dual_failure_semantics() {
        // Non-date body: try uses broad opener (planning), rewinds, stays silent.
        // Inline `at_timestamp` is date-gated, so the dispatcher never claims.
        let src = "[garbage";

        assert!(
            !at_timestamp(&make_parser(src)),
            "inline at_timestamp is date-shaped only"
        );
        assert!(
            at_timestamp_opener(&make_parser(src)),
            "element try still sees a bracket opener"
        );

        let mut try_p = make_parser(src);
        let try_pos = try_p.cursor().index();
        let try_events = try_p.checkpoint().events_len;
        assert_eq!(try_parse_timestamp(&mut try_p), None);
        assert_eq!(
            try_p.cursor().index(),
            try_pos,
            "try: cursor restored on failure"
        );
        assert_eq!(
            try_p.checkpoint().events_len,
            try_events,
            "try: events restored on failure"
        );
        assert_eq!(try_p.current(), OrgSyntaxKind::L_BRACKET);
        let try_diags = try_p.finish().2;
        assert!(
            try_diags.is_empty(),
            "non-date try fail stays silent: {try_diags:?}"
        );

        // Date-shaped unclosed: both paths fail and report (see the
        // near-miss emit test below).
        let (toks, evs, inline_diags) = run("[2020-01-01", |p| {
            assert!(at_timestamp(p));
            let before = p.cursor().index();
            parse_timestamp(p);
            assert!(
                p.cursor().index() > before,
                "inline parse_timestamp advances past bare opener"
            );
        });
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::TIMESTAMP),
            0,
            "failed inline parse emits no TIMESTAMP"
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, crate::parser::Event::Token { kind, .. } if *kind == OrgSyntaxKind::L_BRACKET)),
            "bare opener token present; toks={toks:?}"
        );
        // Date near-miss emits (sticky); silence is only for non-date openers.
        assert!(
            !inline_diags.is_empty(),
            "date-shaped unclosed is a near-miss: {inline_diags:?}"
        );
    }

    #[test]
    fn parse_timestamp_vs_try_dual_near_miss_emit() {
        // On a date-like unclosed stamp both paths emit exactly one sticky
        // MalformedTimestamp with the same range; try rewinds, inline advances.
        // Covers inactive and active openers.
        for src in ["[2020-01-01", "<2020-01-01"] {
            let mut try_p = make_parser(src);
            let try_pos = try_p.cursor().index();
            let try_events = try_p.checkpoint().events_len;
            assert_eq!(
                try_parse_timestamp(&mut try_p),
                None,
                "try fail for {src:?}"
            );
            assert_eq!(
                try_p.cursor().index(),
                try_pos,
                "try: cursor restored for {src:?}"
            );
            assert_eq!(
                try_p.checkpoint().events_len,
                try_events,
                "try: events restored for {src:?}"
            );
            let try_diags = try_p.finish().2;
            assert_eq!(
                try_diags.len(),
                1,
                "try: one sticky stamp diag for {src:?}: {try_diags:?}"
            );
            assert!(matches!(
                try_diags[0].kind(),
                crate::DiagnosticKind::MalformedTimestamp
            ));
            let try_range = try_diags[0].range();

            let mut inline_p = make_parser(src);
            let before = inline_p.cursor().index();
            assert!(at_timestamp(&inline_p));
            parse_timestamp(&mut inline_p);
            assert!(
                inline_p.cursor().index() > before,
                "inline advances past bare opener on near-miss for {src:?}"
            );
            let inline_diags = inline_p.finish().2;
            assert_eq!(
                inline_diags.len(),
                1,
                "inline: one stamp diag for {src:?}: {inline_diags:?}"
            );
            assert!(matches!(
                inline_diags[0].kind(),
                crate::DiagnosticKind::MalformedTimestamp
            ));
            assert_eq!(
                inline_diags[0].range(),
                try_range,
                "both paths cover the same fail range for {src:?}"
            );
        }
    }

    #[test]
    fn try_parse_timestamp_rejects_non_opener() {
        let mut p = make_parser("not a stamp");
        assert_eq!(try_parse_timestamp(&mut p), None);
        assert_eq!(p.cursor().index(), 0);
    }

    #[test]
    fn try_parse_inactive_timestamp_rejects_active_and_non_opener() {
        // Shape gate: only L_BRACKET (and not `[[`) enters the try body.
        for src in ["<2020-01-01>", "not a stamp", "[[link]]"] {
            let mut p = make_parser(src);
            assert_eq!(
                try_parse_inactive_timestamp(&mut p),
                None,
                "inactive reject for {src:?}"
            );
            assert_eq!(
                p.cursor().index(),
                0,
                "cursor unmoved on reject for {src:?}"
            );
        }
        let src = "[2020-01-01]";
        let (_toks, evs, diags) = run(src, |p| {
            assert_eq!(try_parse_inactive_timestamp(p), Some(TimestampForm::Point));
        });
        assert_eq!(node_count(&evs, OrgSyntaxKind::TIMESTAMP), 1);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn try_parse_timestamp_near_miss_emits_and_rewinds() {
        // Try path: full rewind of cursor/events, but sticky MalformedTimestamp.
        let src = "[2020-01-01";
        let mut p = make_parser(src);
        let pos = p.cursor().index();
        let events_len = p.checkpoint().events_len;
        assert_eq!(try_parse_timestamp(&mut p), None);
        assert_eq!(p.cursor().index(), pos, "try rewinds cursor");
        assert_eq!(p.checkpoint().events_len, events_len, "try rewinds events");
        let diags = p.finish().2;
        assert!(
            diags_any(&diags, |k| matches!(
                k,
                crate::DiagnosticKind::MalformedTimestamp
            )),
            "try near-miss must emit sticky MalformedTimestamp: {diags:?}"
        );
    }

    #[test]
    fn try_parse_inactive_timestamp_near_miss_emits() {
        // Inactive try: same emit and full rewind as try_parse_timestamp.
        let src = "[2020-01-01";
        let mut p = make_parser(src);
        let pos = p.cursor().index();
        let events_len = p.checkpoint().events_len;
        assert_eq!(try_parse_inactive_timestamp(&mut p), None);
        assert_eq!(p.cursor().index(), pos, "inactive try rewinds cursor");
        assert_eq!(
            p.checkpoint().events_len,
            events_len,
            "inactive try rewinds events"
        );
        let diags = p.finish().2;
        assert!(
            diags_any(&diags, |k| matches!(
                k,
                crate::DiagnosticKind::MalformedTimestamp
            )),
            "inactive try near-miss must emit: {diags:?}"
        );
    }

    #[test]
    fn try_parse_timestamp_broken_range_tail_is_point() {
        // First stamp succeeds; failed `--` tail rewinds to point (same as inline).
        let src = "[2020-01-01]--";
        let (_toks, evs, _) = run(src, |p| {
            assert_eq!(try_parse_timestamp(p), Some(TimestampForm::Point));
            assert_eq!(p.current(), OrgSyntaxKind::TEXT);
            assert_eq!(p.text(), "--");
        });
        assert_eq!(node_count(&evs, OrgSyntaxKind::TIMESTAMP), 1);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TIMESTAMP).as_deref(),
            Some("[2020-01-01]")
        );
    }

    // --- Pure observers (TokenView measure helpers) ------------------------

    #[test]
    fn pure_observers_date_rod_and_at_timestamp() {
        // Date shape table + purity.
        let date = |src: &str| {
            let p = make_parser(src);
            let before = p.cursor();
            let ok = p.token_view(p.cursor()).is_some_and(looks_like_date_tok);
            assert_eq!(p.cursor(), before, "date pure for {src:?}");
            ok
        };
        assert!(date("2026-06-21"));
        assert!(!date("2026-06-21foo"));
        assert!(!date("not-a-date"));

        // Repeater lengths (+ / ++ / .+ / habit).
        let cases: &[(&str, Option<usize>)] = &[
            ("+1w", Some(2)),
            ("++2d", Some(3)),
            (".+2d", Some(3)),
            (".+2d/4d", Some(5)),
            ("+0d", None), // zero value not a ROD
            ("1w", None),  // missing lead
        ];
        for &(src, expect) in cases {
            let p = make_parser(src);
            let before = p.cursor();
            let got = p.token_view(p.cursor()).and_then(probe_repeater);
            assert_eq!(got, expect, "repeater for {src:?}");
            assert_eq!(p.cursor(), before, "probe_repeater pure for {src:?}");
        }

        // Delay is one TEXT token.
        let delay = |src: &str| {
            let p = make_parser(src);
            let before = p.cursor();
            let got = p.token_view(p.cursor()).and_then(probe_delay);
            assert_eq!(p.cursor(), before, "delay pure for {src:?}");
            got
        };
        assert_eq!(delay("-1d"), Some(1));
        assert_eq!(delay("--2w"), Some(1));
        assert_eq!(delay("+1d"), None);

        // at_timestamp purity + cheap shapes.
        let at = make_parser("<2026-06-21>");
        let before_at = at.cursor();
        assert!(at_timestamp(&at));
        assert_eq!(at.cursor(), before_at);
        assert!(at_timestamp(&make_parser("[2026-06-21]")));
        assert!(!at_timestamp(&make_parser("[[link]]")));
        assert!(!at_timestamp(&make_parser("<not-a-date>")));
        assert!(at_timestamp(&make_parser("<%%(diary)>")));
    }

    #[test]
    fn parse_timestamp_vs_try_dual_unbalanced_diary() {
        // Diary fail: both paths emit UnbalancedDiarySexp; try rewinds, inline advances.
        let src = "<%%(foo";
        let mut try_p = make_parser(src);
        let try_pos = try_p.cursor().index();
        let try_events = try_p.checkpoint().events_len;
        assert_eq!(try_parse_timestamp(&mut try_p), None);
        assert_eq!(try_p.cursor().index(), try_pos, "try: cursor restored");
        assert_eq!(
            try_p.checkpoint().events_len,
            try_events,
            "try: events restored"
        );
        let try_diags = try_p.finish().2;
        assert_eq!(try_diags.len(), 1, "try: one diary diag: {try_diags:?}");
        assert!(matches!(
            try_diags[0].kind(),
            crate::DiagnosticKind::UnbalancedDiarySexp
        ));
        let try_range = try_diags[0].range();

        let mut inline_p = make_parser(src);
        let before = inline_p.cursor().index();
        assert!(at_timestamp(&inline_p));
        parse_timestamp(&mut inline_p);
        assert!(
            inline_p.cursor().index() > before,
            "inline advances past bare opener on diary fail"
        );
        let inline_diags = inline_p.finish().2;
        assert_eq!(
            inline_diags.len(),
            1,
            "inline: one diary diag: {inline_diags:?}"
        );
        assert!(matches!(
            inline_diags[0].kind(),
            crate::DiagnosticKind::UnbalancedDiarySexp
        ));
        assert_eq!(
            inline_diags[0].range(),
            try_range,
            "both paths cover the same fail range"
        );
        assert_eq!(
            node_count(
                &run(src, |p| {
                    if at_timestamp(p) {
                        parse_timestamp(p);
                    }
                })
                .1,
                OrgSyntaxKind::TIMESTAMP
            ),
            0
        );
    }

    #[test]
    fn parse_timestamp_vs_try_dual_active_non_date_silent() {
        // Active non-date opener: try fails silent with full rewind; inline never enters.
        let src = "<not-a-date>";
        let mut try_p = make_parser(src);
        let try_pos = try_p.cursor().index();
        let try_events = try_p.checkpoint().events_len;
        assert_eq!(try_parse_timestamp(&mut try_p), None);
        assert_eq!(try_p.cursor().index(), try_pos, "try: cursor restored");
        assert_eq!(
            try_p.checkpoint().events_len,
            try_events,
            "try: events restored"
        );
        let try_diags = try_p.finish().2;
        assert!(
            try_diags.is_empty(),
            "non-date try stays silent: {try_diags:?}"
        );

        // Inline dispatcher never enters: narrow at_timestamp is false, so
        // parse_timestamp is not called (contrast with try's broad opener).
        let inline_p = make_parser(src);
        assert!(!at_timestamp(&inline_p));
        assert_eq!(inline_p.cursor().index(), 0);

        // Broad opener path (planning): silent fail + rewind.
        let mut plan_p = make_parser(src);
        assert!(at_timestamp_opener(&plan_p));
        let pos = plan_p.cursor().index();
        let events = plan_p.checkpoint().events_len;
        assert_eq!(try_parse_timestamp(&mut plan_p), None);
        assert_eq!(plan_p.cursor().index(), pos);
        assert_eq!(plan_p.checkpoint().events_len, events);
        let diagnostics = plan_p.finish().2;
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
