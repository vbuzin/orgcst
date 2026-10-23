//! Timestamp field slicers and delimiter walks.

use super::{is_weekday_text, range_contains_range, text_in};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
/// Year / month / day absolute ranges inside a `DATE` node (`YYYY-MM-DD`).
///
/// The grammar emits `DATE` only for a 10-byte digit-hyphen shape (timestamp
/// form). Degenerate or wrong-kind nodes yield `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DateFields {
    pub year: TextRange,
    pub month: TextRange,
    pub day: TextRange,
}

/// Slice `YYYY-MM-DD` inside a `DATE` node.
#[must_use]
pub(crate) fn date_fields(date: &SyntaxNode) -> Option<DateFields> {
    if date.kind() != OrgSyntaxKind::DATE {
        return None;
    }
    // DATE is a leaf over one TEXT token of length 10; to_string is 10 bytes.
    let s = date.text().to_string();
    if s.len() != 10 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b[..4].iter().all(u8::is_ascii_digit)
        || !b[5..7].iter().all(u8::is_ascii_digit)
        || !b[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let start = date.text_range().start();
    Some(DateFields {
        year: TextRange::at(start, TextSize::from(4)),
        month: TextRange::at(start + TextSize::from(5), TextSize::from(2)),
        day: TextRange::at(start + TextSize::from(8), TextSize::from(2)),
    })
}

/// Value digits and unit char of the **primary** repeater/delay inside a `ROD`.
///
/// Habit forms (`.+2d/4d`, `++1d/3d`) keep a `/Nunit` deadline tail on the same
/// `ROD` node; this slicer returns the first `Nunit` pair after the sign, not
/// the habit tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RodFields {
    pub value: TextRange,
    pub unit: TextRange,
}

/// Slice primary value and unit from a `ROD` node.
///
/// Scans left-to-right for the first unit letter (`h|d|w|m|y`) that is
/// immediately preceded by a non-empty digit run. Leading sign bytes (`+`,
/// `++`, `.+`, `-`) are skipped by that digit requirement.
#[must_use]
pub(crate) fn rod_fields(rod: &SyntaxNode) -> Option<RodFields> {
    if rod.kind() != OrgSyntaxKind::ROD {
        return None;
    }
    let s = rod.text().to_string();
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if matches!(b[i], b'h' | b'd' | b'w' | b'm' | b'y') {
            // Digit run immediately before this unit.
            let mut dig_start = i;
            while dig_start > 0 && b[dig_start - 1].is_ascii_digit() {
                dig_start -= 1;
            }
            if dig_start < i {
                let base = rod.text_range().start();
                return Some(RodFields {
                    value: TextRange::new(
                        base + TextSize::from(dig_start as u32),
                        base + TextSize::from(i as u32),
                    ),
                    unit: TextRange::at(base + TextSize::from(i as u32), TextSize::from(1)),
                });
            }
        }
        i += 1;
    }
    None
}

/// Start and end halves of a `TIME_RANGE` (`HH:MM-HH:MM` or similar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeRangeHalves {
    pub start: TextRange,
    pub end: TextRange,
}

/// Split a `TIME_RANGE` node into start and end absolute ranges.
///
/// Splits on the interior `-`, which the lexer leaves inside a `TEXT` token.
/// Degenerate shapes yield `None`.
#[must_use]
pub(crate) fn time_range_halves(time_range: &SyntaxNode) -> Option<TimeRangeHalves> {
    if time_range.kind() != OrgSyntaxKind::TIME_RANGE {
        return None;
    }
    let s = time_range.text().to_string();
    let b = s.as_bytes();
    let hy = b
        .iter()
        .enumerate()
        .find(|(i, c)| **c == b'-' && *i > 0 && *i + 1 < b.len())
        .map(|(i, _)| i)?;
    let base = time_range.text_range().start();
    Some(TimeRangeHalves {
        start: TextRange::new(base, base + TextSize::from(hy as u32)),
        end: TextRange::new(
            base + TextSize::from((hy + 1) as u32),
            time_range.text_range().end(),
        ),
    })
}

/// Hour / minute absolute ranges inside a `TIME` node (`H:MM` / `HH:MM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeFields {
    pub hour: TextRange,
    pub minute: TextRange,
}

/// Slice `TEXT COLON TEXT` inside a `TIME` node.
#[must_use]
pub(crate) fn time_fields(time: &SyntaxNode) -> Option<TimeFields> {
    if time.kind() != OrgSyntaxKind::TIME {
        return None;
    }
    let toks: Vec<_> = time.children_with_tokens().collect();
    if toks.len() != 3 {
        return None;
    }
    if toks[0].kind() != OrgSyntaxKind::TEXT
        || toks[1].kind() != OrgSyntaxKind::COLON
        || toks[2].kind() != OrgSyntaxKind::TEXT
    {
        return None;
    }
    Some(TimeFields {
        hour: toks[0].text_range(),
        minute: toks[2].text_range(),
    })
}

/// Slice `HH:MM` / `H:MM` inside `span`, a sub-range of `node`.
///
/// Rejects a span outside `node.range()` or a slice with no colon.
#[must_use]
pub(crate) fn hm_fields(node: &SyntaxNode, span: TextRange) -> Option<TimeFields> {
    if !range_contains_range(node.text_range(), span) {
        return None;
    }
    let s = text_in(node, span);
    let colon = s.find(':')?;
    if colon == 0 || colon + 1 >= s.len() {
        return None;
    }
    let hour_end = span.start() + TextSize::from(colon as u32);
    let minute_start = hour_end + TextSize::from(1);
    Some(TimeFields {
        hour: TextRange::new(span.start(), hour_end),
        minute: TextRange::new(minute_start, span.end()),
    })
}

/// Prefix of a `ROD` region: repeater vs delay, including `+` / `++` / `.+` /
/// `-` / `--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RodSign {
    Cumulate,
    CatchUp,
    Restart,
    DelayAll,
    DelayFirst,
}

/// Read the sign at the start of a `ROD`.
#[must_use]
pub(crate) fn rod_sign(rod: &SyntaxNode) -> Option<RodSign> {
    if rod.kind() != OrgSyntaxKind::ROD {
        return None;
    }
    let s = rod.text().to_string();
    if s.starts_with(".+") {
        Some(RodSign::Restart)
    } else if s.starts_with("++") {
        Some(RodSign::CatchUp)
    } else if s.starts_with('+') {
        Some(RodSign::Cumulate)
    } else if s.starts_with("--") {
        Some(RodSign::DelayFirst)
    } else if s.starts_with('-') {
        Some(RodSign::DelayAll)
    } else {
        None
    }
}

/// Repeater vs delay, derived from [`rod_sign`] (not a second scan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RodKind {
    Repeater,
    Delay,
}

#[must_use]
pub(crate) fn rod_kind(rod: &SyntaxNode) -> Option<RodKind> {
    match rod_sign(rod)? {
        RodSign::Cumulate | RodSign::CatchUp | RodSign::Restart => Some(RodKind::Repeater),
        RodSign::DelayAll | RodSign::DelayFirst => Some(RodKind::Delay),
    }
}

/// Habit `/Nunit` pair after the primary unit. `None` if there is no slash tail.
#[must_use]
pub(crate) fn rod_habit_fields(rod: &SyntaxNode) -> Option<RodFields> {
    let primary = rod_fields(rod)?;
    let s = rod.text().to_string();
    let base = rod.text_range().start();
    let unit_end = u32::from(primary.unit.end() - base) as usize;
    let rest = s.get(unit_end..)?;
    let after = rest.strip_prefix('/')?;
    let b = after.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= b.len() {
        return None;
    }
    if !matches!(b[i], b'h' | b'd' | b'w' | b'm' | b'y') {
        return None;
    }
    let dig_start = unit_end + 1;
    Some(RodFields {
        value: TextRange::new(
            base + TextSize::from(dig_start as u32),
            base + TextSize::from((dig_start + i) as u32),
        ),
        unit: TextRange::at(
            base + TextSize::from((dig_start + i) as u32),
            TextSize::from(1),
        ),
    })
}

/// Bare weekday TEXT after the first `DATE`.
#[must_use]
pub(crate) fn timestamp_weekday(ts: &SyntaxNode) -> Option<TextRange> {
    weekday_after_date(ts, 0)
}

/// Bare weekday TEXT after the second `DATE` (`--` range only).
#[must_use]
pub(crate) fn timestamp_end_weekday(ts: &SyntaxNode) -> Option<TextRange> {
    weekday_after_date(ts, 1)
}

fn weekday_after_date(ts: &SyntaxNode, date_index: usize) -> Option<TextRange> {
    if ts.kind() != OrgSyntaxKind::TIMESTAMP {
        return None;
    }
    let mut seen = 0usize;
    let mut after = false;
    for e in ts.children_with_tokens() {
        if e.kind() == OrgSyntaxKind::DATE {
            after = seen == date_index;
            seen += 1;
            continue;
        }
        if !after {
            continue;
        }
        if e.kind() == OrgSyntaxKind::WHITESPACE {
            continue;
        }
        if e.kind() == OrgSyntaxKind::TEXT {
            let tok = e.as_token()?;
            if is_weekday_text(tok.text()) {
                return Some(tok.text_range());
            }
        }
        return None;
    }
    None
}

/// Outer (and, on a `--` range, inner) delimiter tokens of a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimestampDelimiterSpans {
    pub start_open: TextRange,
    pub start_close: TextRange,
    pub end_open: Option<TextRange>,
    pub end_close: Option<TextRange>,
    /// `true` when the first opener is `<`.
    pub active: bool,
}

/// Four delimiter tokens on a `--` range; two on a point or diary stamp.
#[must_use]
pub(crate) fn timestamp_delimiters(ts: &SyntaxNode) -> Option<TimestampDelimiterSpans> {
    if ts.kind() != OrgSyntaxKind::TIMESTAMP {
        return None;
    }
    let kids: Vec<_> = ts.children_with_tokens().collect();
    let first = kids.first()?.as_token()?;
    if !matches!(
        first.kind(),
        OrgSyntaxKind::L_ANGLE | OrgSyntaxKind::L_BRACKET
    ) {
        return None;
    }
    let last = kids.last()?.as_token()?;
    if !matches!(
        last.kind(),
        OrgSyntaxKind::R_ANGLE | OrgSyntaxKind::R_BRACKET
    ) {
        return None;
    }
    let dash = kids.iter().position(|e| {
        e.kind() == OrgSyntaxKind::TEXT && e.as_token().is_some_and(|t| t.text() == "--")
    });
    let active = first.kind() == OrgSyntaxKind::L_ANGLE;
    match dash {
        None => Some(TimestampDelimiterSpans {
            start_open: first.text_range(),
            start_close: last.text_range(),
            end_open: None,
            end_close: None,
            active,
        }),
        Some(i) => {
            let start_close = kids[..i]
                .iter()
                .rev()
                .find(|e| matches!(e.kind(), OrgSyntaxKind::R_ANGLE | OrgSyntaxKind::R_BRACKET))?;
            let end_open = kids[i + 1..]
                .iter()
                .find(|e| matches!(e.kind(), OrgSyntaxKind::L_ANGLE | OrgSyntaxKind::L_BRACKET))?;
            Some(TimestampDelimiterSpans {
                start_open: first.text_range(),
                start_close: start_close.text_range(),
                end_open: Some(end_open.text_range()),
                end_close: Some(last.text_range()),
                active,
            })
        }
    }
}

/// `DATE` children of a `TIMESTAMP`, source order.
#[must_use]
pub(crate) fn timestamp_date_nodes(ts: &SyntaxNode) -> Vec<SyntaxNode> {
    if ts.kind() != OrgSyntaxKind::TIMESTAMP {
        return Vec::new();
    }
    ts.children()
        .filter(|c| c.kind() == OrgSyntaxKind::DATE)
        .collect()
}

/// First-stamp and second-stamp `TIME` / `TIME_RANGE` children.
///
/// Diary wall-clock sits on the first slot (`DIARY_SEXP` counts as the first
/// stamp). A `--` range's second slot is after the second `DATE`.
#[must_use]
pub(crate) fn timestamp_stamp_times(ts: &SyntaxNode) -> (Option<SyntaxNode>, Option<SyntaxNode>) {
    if ts.kind() != OrgSyntaxKind::TIMESTAMP {
        return (None, None);
    }
    let mut n_stamps = 0usize;
    let mut first = None;
    let mut second = None;
    for child in ts.children() {
        match child.kind() {
            OrgSyntaxKind::DATE | OrgSyntaxKind::DIARY_SEXP => n_stamps += 1,
            OrgSyntaxKind::TIME | OrgSyntaxKind::TIME_RANGE => {
                if n_stamps <= 1 {
                    if first.is_none() {
                        first = Some(child);
                    }
                } else if second.is_none() {
                    second = Some(child);
                }
            }
            _ => {}
        }
    }
    (first, second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::{first_kind, synth};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::{TextRange, TextSize};

    #[test]
    fn date_fields_happy() {
        let date = first_kind("<2024-06-15>\n", OrgSyntaxKind::DATE);
        let f = date_fields(&date).expect("fields");
        let doc = parse("<2024-06-15>\n");
        assert_eq!(&doc.source()[f.year], "2024");
        assert_eq!(&doc.source()[f.month], "06");
        assert_eq!(&doc.source()[f.day], "15");

        let src = "<2026-01-01>--<2026-02-03>\n";
        let dates = nodes_of_kind(parse(src).root(), OrgSyntaxKind::DATE);
        assert_eq!(dates.len(), 2);
        let f = date_fields(&dates[1]).expect("second DATE");
        let doc = parse(src);
        assert_eq!(&doc.source()[f.year], "2026");
        assert_eq!(&doc.source()[f.month], "02");
        assert_eq!(&doc.source()[f.day], "03");
    }
    #[test]
    fn date_fields_rejects_wrong_kind() {
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(date_fields(&p).is_none());
    }
    #[test]
    fn rod_fields_repeater() {
        let rod = first_kind("<2024-06-15 +1w>\n", OrgSyntaxKind::ROD);
        let f = rod_fields(&rod).expect("rod");
        let doc = parse("<2024-06-15 +1w>\n");
        assert_eq!(&doc.source()[f.value], "1");
        assert_eq!(&doc.source()[f.unit], "w");
    }
    #[test]
    fn rod_fields_delay() {
        let rod = first_kind("<2024-06-15 -2d>\n", OrgSyntaxKind::ROD);
        let f = rod_fields(&rod).expect("rod");
        let doc = parse("<2024-06-15 -2d>\n");
        assert_eq!(&doc.source()[f.value], "2");
        assert_eq!(&doc.source()[f.unit], "d");
    }
    #[test]
    fn rod_fields_habit_primary_not_deadline() {
        let src = "<2024-06-15 .+2d/4d>\n";
        let rod = first_kind(src, OrgSyntaxKind::ROD);
        let f = rod_fields(&rod).expect("rod");
        let doc = parse(src);
        assert_eq!(&doc.source()[f.value], "2");
        assert_eq!(&doc.source()[f.unit], "d");
        // Full rod text still includes the habit tail.
        assert_eq!(rod.text().to_string(), ".+2d/4d");
    }
    #[test]
    fn rod_fields_cumulative_habit_primary() {
        let src = "<2024-06-15 ++1d/3d>\n";
        let rod = first_kind(src, OrgSyntaxKind::ROD);
        let f = rod_fields(&rod).expect("rod");
        let doc = parse(src);
        assert_eq!(&doc.source()[f.value], "1");
        assert_eq!(&doc.source()[f.unit], "d");
    }
    #[test]
    fn rod_fields_rejects_wrong_kind() {
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(rod_fields(&p).is_none());
    }
    #[test]
    fn time_range_halves_split() {
        let src = "<2024-06-15 09:00-10:30>\n";
        let tr = first_kind(src, OrgSyntaxKind::TIME_RANGE);
        let h = time_range_halves(&tr).expect("halves");
        let doc = parse(src);
        assert_eq!(&doc.source()[h.start], "09:00");
        assert_eq!(&doc.source()[h.end], "10:30");
    }
    #[test]
    fn time_range_halves_rejects_wrong_kind() {
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(time_range_halves(&p).is_none());
    }
    #[test]
    fn time_range_halves_wrong_kind_time_node() {
        // A plain TIME is not TIME_RANGE (wrong-kind sibling under TIMESTAMP).
        let src = "<2024-06-15 09:00>\n";
        let doc = parse(src);
        let times = nodes_of_kind(doc.root(), OrgSyntaxKind::TIME);
        assert!(!times.is_empty(), "{times:?}");
        assert!(time_range_halves(&times[0]).is_none());
    }

    // -----------------------------------------------------------------------
    // Degenerate shapes: every slicer has a fixture, malformed ones included
    //
    // The kind guard is only the first gate; these reach the shape guards
    // behind it. The grammar cannot emit any of these shapes, which is why
    // slicing text is safe, so the fixtures are synthesised. If a grammar
    // change ever starts emitting one, the slicer returns None here instead of
    // a wrong absolute range, and that stays pinned.
    // -----------------------------------------------------------------------
    #[test]
    fn date_fields_rejects_degenerate_shapes() {
        let text = OrgSyntaxKind::TEXT;
        // Wrong length: single-digit month/day.
        let short = synth(OrgSyntaxKind::DATE, &[(text, "2024-6-15")]);
        assert!(date_fields(&short).is_none(), "9-byte date");
        let long = synth(OrgSyntaxKind::DATE, &[(text, "2024-06-155")]);
        assert!(date_fields(&long).is_none(), "11-byte date");
        // Right length, wrong separators.
        let slashes = synth(OrgSyntaxKind::DATE, &[(text, "2024/06/15")]);
        assert!(date_fields(&slashes).is_none(), "slash separators");
        // Right length and separators, non-digit fields.
        let alpha = synth(OrgSyntaxKind::DATE, &[(text, "abcd-ef-gh")]);
        assert!(date_fields(&alpha).is_none(), "non-digit fields");
        // Multi-byte characters must not slice mid-codepoint.
        let wide = synth(OrgSyntaxKind::DATE, &[(text, "20\u{00e9}4-06-15")]);
        assert!(wide.text().to_string().len() != 10 || date_fields(&wide).is_none());
    }
    #[test]
    fn rod_fields_rejects_degenerate_shapes() {
        let text = OrgSyntaxKind::TEXT;
        // Empty rod.
        let empty = synth(OrgSyntaxKind::ROD, &[(text, "")]);
        assert!(rod_fields(&empty).is_none(), "empty rod");
        // Unit with no digit run before it.
        let no_digits = synth(OrgSyntaxKind::ROD, &[(text, "+d")]);
        assert!(rod_fields(&no_digits).is_none(), "unit without value");
        // Digits with no unit letter.
        let no_unit = synth(OrgSyntaxKind::ROD, &[(text, "+12")]);
        assert!(rod_fields(&no_unit).is_none(), "value without unit");
    }
    #[test]
    fn time_range_halves_rejects_degenerate_shapes() {
        let text = OrgSyntaxKind::TEXT;
        // No interior hyphen at all.
        let no_hyphen = synth(OrgSyntaxKind::TIME_RANGE, &[(text, "09:00")]);
        assert!(time_range_halves(&no_hyphen).is_none(), "no hyphen");
        // Leading hyphen: no start half before it.
        let leading = synth(OrgSyntaxKind::TIME_RANGE, &[(text, "-10:30")]);
        assert!(time_range_halves(&leading).is_none(), "leading hyphen");
        // Trailing hyphen: no end half after it.
        let trailing = synth(OrgSyntaxKind::TIME_RANGE, &[(text, "09:00-")]);
        assert!(time_range_halves(&trailing).is_none(), "trailing hyphen");
    }
    #[test]
    fn time_fields_happy_and_rejects_wrong_kind() {
        let src = "<2024-06-15 09:00>\n";
        let time = first_kind(src, OrgSyntaxKind::TIME);
        let f = time_fields(&time).expect("fields");
        let doc = parse(src);
        assert_eq!(&doc.source()[f.hour], "09");
        assert_eq!(&doc.source()[f.minute], "00");
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(time_fields(&p).is_none());
    }
    #[test]
    fn hm_fields_on_time_range_halves_and_rejects_outside() {
        let src = "<2024-06-15 09:00-10:30>\n";
        let tr = first_kind(src, OrgSyntaxKind::TIME_RANGE);
        let h = time_range_halves(&tr).expect("halves");
        let start = hm_fields(&tr, h.start).expect("start");
        let end = hm_fields(&tr, h.end).expect("end");
        let doc = parse(src);
        assert_eq!(&doc.source()[start.hour], "09");
        assert_eq!(&doc.source()[start.minute], "00");
        assert_eq!(&doc.source()[end.hour], "10");
        assert_eq!(&doc.source()[end.minute], "30");
        let outside = TextRange::new(TextSize::from(0), TextSize::from(1));
        assert!(hm_fields(&tr, outside).is_none());
        assert!(hm_fields(&tr, tr.text_range()).is_some());

        let src = "<2026-01-01 9:05>\n";
        let time = first_kind(src, OrgSyntaxKind::TIME);
        let f = time_fields(&time).expect("single-digit hour");
        let doc = parse(src);
        assert_eq!(&doc.source()[f.hour], "9");
        assert_eq!(&doc.source()[f.minute], "05");
        let hm = hm_fields(&time, time.text_range()).expect("hm 9:05");
        assert_eq!(&doc.source()[hm.hour], "9");
        assert_eq!(&doc.source()[hm.minute], "05");
    }
    #[test]
    fn time_and_hm_fields_reject_degenerate_shapes() {
        let text = OrgSyntaxKind::TEXT;
        let no_colon = synth(OrgSyntaxKind::TIME, &[(text, "0900")]);
        assert!(time_fields(&no_colon).is_none(), "TIME without COLON token");
        let colon_start = synth(
            OrgSyntaxKind::TIME,
            &[(OrgSyntaxKind::COLON, ":"), (text, "05")],
        );
        assert!(
            time_fields(&colon_start).is_none(),
            "TIME not TEXT COLON TEXT"
        );
        let empty = synth(OrgSyntaxKind::TIME, &[]);
        assert!(time_fields(&empty).is_none(), "empty TIME");

        let no_colon = synth(OrgSyntaxKind::TIME_RANGE, &[(text, "0900")]);
        assert!(
            hm_fields(&no_colon, no_colon.text_range()).is_none(),
            "no colon"
        );
        let colon_at_start = synth(OrgSyntaxKind::TIME_RANGE, &[(text, ":05")]);
        assert!(
            hm_fields(&colon_at_start, colon_at_start.text_range()).is_none(),
            "colon at start"
        );
        let colon_at_end = synth(OrgSyntaxKind::TIME_RANGE, &[(text, "9:")]);
        assert!(
            hm_fields(&colon_at_end, colon_at_end.text_range()).is_none(),
            "colon at end"
        );
    }
    #[test]
    fn rod_sign_five_way_and_kind() {
        let plus = first_kind("<2024-06-15 +1w>\n", OrgSyntaxKind::ROD);
        assert_eq!(rod_sign(&plus), Some(RodSign::Cumulate));
        assert_eq!(rod_kind(&plus), Some(RodKind::Repeater));
        let catch = first_kind("<2024-06-15 ++1d>\n", OrgSyntaxKind::ROD);
        assert_eq!(rod_sign(&catch), Some(RodSign::CatchUp));
        let restart = first_kind("<2024-06-15 .+2d>\n", OrgSyntaxKind::ROD);
        assert_eq!(rod_sign(&restart), Some(RodSign::Restart));
        let all = first_kind("<2024-06-15 -2d>\n", OrgSyntaxKind::ROD);
        assert_eq!(rod_sign(&all), Some(RodSign::DelayAll));
        assert_eq!(rod_kind(&all), Some(RodKind::Delay));
        let first = first_kind("<2024-06-15 --2d>\n", OrgSyntaxKind::ROD);
        assert_eq!(rod_sign(&first), Some(RodSign::DelayFirst));
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(rod_sign(&p).is_none());
        assert!(rod_kind(&p).is_none());
    }
    #[test]
    fn rod_habit_fields_primary_vs_tail() {
        let src = "<2024-06-15 .+2d/4d>\n";
        let rod = first_kind(src, OrgSyntaxKind::ROD);
        let primary = rod_fields(&rod).expect("primary");
        let habit = rod_habit_fields(&rod).expect("habit");
        let doc = parse(src);
        assert_eq!(&doc.source()[primary.value], "2");
        assert_eq!(&doc.source()[primary.unit], "d");
        assert_eq!(&doc.source()[habit.value], "4");
        assert_eq!(&doc.source()[habit.unit], "d");
        let no_habit = first_kind("<2024-06-15 +1w>\n", OrgSyntaxKind::ROD);
        assert!(rod_habit_fields(&no_habit).is_none());

        let slash_only = synth(OrgSyntaxKind::ROD, &[(OrgSyntaxKind::TEXT, "+1d/")]);
        assert!(
            rod_habit_fields(&slash_only).is_none(),
            "slash without tail"
        );
        let digits_no_unit = synth(OrgSyntaxKind::ROD, &[(OrgSyntaxKind::TEXT, "+1d/4")]);
        assert!(
            rod_habit_fields(&digits_no_unit).is_none(),
            "habit digits without unit"
        );
        let unit_no_digits = synth(OrgSyntaxKind::ROD, &[(OrgSyntaxKind::TEXT, "+1d/d")]);
        assert!(
            rod_habit_fields(&unit_no_digits).is_none(),
            "habit unit without digits"
        );
    }
    #[test]
    fn timestamp_weekday_start_and_end() {
        let src = "<2026-01-01 Thu>--<2026-01-02 Fri>\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        let doc = parse(src);
        assert_eq!(
            &doc.source()[timestamp_weekday(&ts).expect("weekday")],
            "Thu"
        );
        assert_eq!(
            &doc.source()[timestamp_end_weekday(&ts).expect("end weekday")],
            "Fri"
        );
        let point = first_kind("<2026-01-01 Thu>\n", OrgSyntaxKind::TIMESTAMP);
        assert!(timestamp_end_weekday(&point).is_none());
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(timestamp_weekday(&p).is_none());
    }
    #[test]
    fn timestamp_delimiters_point_vs_range() {
        let point = first_kind("<2026-01-01>\n", OrgSyntaxKind::TIMESTAMP);
        let d = timestamp_delimiters(&point).expect("point");
        let doc = parse("<2026-01-01>\n");
        assert_eq!(&doc.source()[d.start_open], "<");
        assert_eq!(&doc.source()[d.start_close], ">");
        assert!(d.end_open.is_none());
        assert!(d.end_close.is_none());

        let src = "<2026-01-01>--<2026-01-02>\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        let d = timestamp_delimiters(&ts).expect("range");
        let doc = parse(src);
        assert_eq!(&doc.source()[d.start_open], "<");
        assert_eq!(&doc.source()[d.start_close], ">");
        assert_eq!(&doc.source()[d.end_open.expect("end open")], "<");
        assert_eq!(&doc.source()[d.end_close.expect("end close")], ">");
        assert!(d.active);

        let src = "[2026-01-01]--[2026-01-02]\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        let d = timestamp_delimiters(&ts).expect("inactive range");
        let doc = parse(src);
        assert_eq!(&doc.source()[d.start_open], "[");
        assert_eq!(&doc.source()[d.end_open.expect("end open")], "[");
        assert!(!d.active);

        let src = "<%%(diary-float t 0 1)>\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        let d = timestamp_delimiters(&ts).expect("diary");
        let doc = parse(src);
        assert_eq!(&doc.source()[d.start_open], "<");
        assert_eq!(&doc.source()[d.start_close], ">");
        assert!(d.end_open.is_none());
        assert!(d.active);

        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(timestamp_delimiters(&p).is_none());
    }
    #[test]
    fn timestamp_date_nodes_and_stamp_times() {
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(
            timestamp_date_nodes(&p).is_empty(),
            "{:?}",
            timestamp_date_nodes(&p)
        );
        assert!(matches!(timestamp_stamp_times(&p), (None, None)));

        let src = "<2026-01-01>--<2026-01-02>\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(timestamp_date_nodes(&ts).len(), 2);
        assert!(matches!(timestamp_stamp_times(&ts), (None, None)));

        let src = "[2026-01-01 Thu 09:00-10:00]\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        assert_eq!(timestamp_date_nodes(&ts).len(), 1);
        let (first, second) = timestamp_stamp_times(&ts);
        assert_eq!(
            first.as_ref().map(crate::SyntaxNode::kind),
            Some(OrgSyntaxKind::TIME_RANGE)
        );
        assert!(second.is_none());

        let src = "<%%(diary-float t 0 1) 09:00-10:00>\n";
        let ts = first_kind(src, OrgSyntaxKind::TIMESTAMP);
        assert!(
            timestamp_date_nodes(&ts).is_empty(),
            "{:?}",
            timestamp_date_nodes(&ts)
        );
        let (first, second) = timestamp_stamp_times(&ts);
        assert_eq!(
            first.as_ref().map(crate::SyntaxNode::kind),
            Some(OrgSyntaxKind::TIME_RANGE)
        );
        assert!(second.is_none());
    }
}
