//! Timestamp objects: active `<2024-01-01 Mon 10:00 +1w>`, inactive
//! `[...]`, `--` ranges and diary sexps.
//!
//! The view reads the shape, the parts, single fields such as the year or the
//! repeater unit, and typed values ([`TimestampPoint`], [`Repeater`],
//! [`Delay`]). Repeaters and delays are both `ROD` (repeater or delay) nodes.

use super::diary::DiarySexp;
use crate::TextRange;
use crate::TextSize;
use crate::ast::layout::{
    RemovalSide, RodKind, RodSign, TimeFields, TimeRangeHalves, TimestampDelimiterSpans,
    date_fields, field_dest, hm_fields, removal_extending, rod_fields, rod_habit_fields, rod_kind,
    rod_sign, text_in, time_fields, time_range_halves, timestamp_date_nodes, timestamp_delimiters,
    timestamp_end_weekday, timestamp_stamp_times, timestamp_weekday,
};
use crate::ast::support::child_view;
use crate::ast::{AstNode, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A timestamp object: active `<...>`, inactive `[...]`, a `--` range or
    /// a diary `<%%(...)>`.
    Timestamp,
    OrgSyntaxKind::TIMESTAMP
);

/// How a timestamp is written.
///
/// A time range inside one stamp, as in `<2024-01-01 Mon 10:00-11:00>`, is
/// still `Active` or `Inactive`; only `--` between two stamps makes a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampShape {
    /// `<...>`.
    Active,
    /// `[...]`.
    Inactive,
    /// `<...>--<...>`.
    ActiveRange,
    /// `[...]--[...]`.
    InactiveRange,
    /// `<%%(sexp)>`.
    Diary,
}

/// Parts of a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampPart {
    /// The start date; not applicable to a diary timestamp.
    Date,
    /// The start time.
    Time,
    /// The date of a range's second stamp; only for ranges.
    EndDate,
    /// The end time: of a range's second stamp, or of a time range such as
    /// `10:00-11:00`.
    EndTime,
    /// The repeater, such as `+1w`, with any habit interval.
    Repeater,
    /// The warning or delay period, such as `-2d`.
    Delay,
}

/// Single fields of a timestamp that can be read or replaced by range.
///
/// The `End` date fields belong to a range's second stamp. `EndHour` and
/// `EndMinute` are also the end of a time range such as `10:00-11:00`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampField {
    /// Year of the start date.
    Year,
    /// Month of the start date.
    Month,
    /// Day of the month of the start date.
    Day,
    /// Day name of the start date, such as `Mon`.
    Weekday,
    /// Hour of the start time.
    Hour,
    /// Minute of the start time.
    Minute,
    /// Year of a range's second date.
    EndYear,
    /// Month of a range's second date.
    EndMonth,
    /// Day of the month of a range's second date.
    EndDay,
    /// Day name of a range's second date.
    EndWeekday,
    /// Hour of the end time.
    EndHour,
    /// Minute of the end time.
    EndMinute,
    /// Number of units in the repeater, as `1` in `+1w`.
    RepeatValue,
    /// Unit of the repeater, as `w` in `+1w`.
    RepeatUnit,
    /// Number of units in the delay, as `2` in `-2d`.
    DelayValue,
    /// Unit of the delay, as `d` in `-2d`.
    DelayUnit,
    /// Number of units in the habit's maximum interval, as `3` in `.+1d/3d`.
    HabitValue,
    /// Unit of the habit's maximum interval, as the last `d` in `.+1d/3d`.
    HabitUnit,
}

/// Ranges of a timestamp's `<`/`>` or `[`/`]` tokens, which switching between
/// active and inactive rewrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimestampDelimiters {
    /// Opener of the first stamp.
    pub start_open: TextRange,
    /// Closer of the first stamp.
    pub start_close: TextRange,
    /// Opener of a range's second stamp.
    pub end_open: Option<TextRange>,
    /// Closer of a range's second stamp.
    pub end_close: Option<TextRange>,
}

/// Date and optional time of one end of a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimestampPoint {
    /// Year.
    pub year: u16,
    /// Month of the year.
    pub month: u8,
    /// Day of the month.
    pub day: u8,
    /// Hour, when a time is written.
    pub hour: Option<u8>,
    /// Minute, when a time is written.
    pub minute: Option<u8>,
}

/// How a repeater moves the date when the entry is marked done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepeaterKind {
    /// `+`: by one interval.
    Cumulate,
    /// `++`: by whole intervals until the date is in the future.
    CatchUp,
    /// `.+`: to one interval after today.
    Restart,
}

/// Unit of a repeater, delay or habit interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepeaterUnit {
    /// `h`.
    Hour,
    /// `d`.
    Day,
    /// `w`.
    Week,
    /// `m`.
    Month,
    /// `y`.
    Year,
}

/// A timestamp's repeater, such as `+1w` or `.+1d/3d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Repeater {
    /// How the repeater moves the date.
    pub kind: RepeaterKind,
    /// Number of units in the interval.
    pub value: u32,
    /// Unit of the interval.
    pub unit: RepeaterUnit,
    /// The habit's maximum interval, as in `.+1d/3d`.
    pub habit: Option<(u32, RepeaterUnit)>,
}

/// Which occurrences of a repeating timestamp a delay applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelayKind {
    /// `-`: every occurrence.
    All,
    /// `--`: only the first occurrence.
    First,
}

/// A timestamp's warning or delay period, such as `-2d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Delay {
    /// Which occurrences the delay applies to.
    pub kind: DelayKind,
    /// Number of units.
    pub value: u32,
    /// Unit of the period.
    pub unit: RepeaterUnit,
}

const TIMESTAMP_FIELDS: &[TimestampField] = &[
    TimestampField::Year,
    TimestampField::Month,
    TimestampField::Day,
    TimestampField::Weekday,
    TimestampField::Hour,
    TimestampField::Minute,
    TimestampField::EndYear,
    TimestampField::EndMonth,
    TimestampField::EndDay,
    TimestampField::EndWeekday,
    TimestampField::EndHour,
    TimestampField::EndMinute,
    TimestampField::RepeatValue,
    TimestampField::RepeatUnit,
    TimestampField::DelayValue,
    TimestampField::DelayUnit,
    TimestampField::HabitValue,
    TimestampField::HabitUnit,
];

impl Timestamp {
    /// How this timestamp is written.
    #[must_use]
    pub fn shape(&self) -> TimestampShape {
        if self.diary_sexp().is_some() {
            return TimestampShape::Diary;
        }
        let d = delimiters(self);
        match (d.active, d.end_open.is_some()) {
            (true, true) => TimestampShape::ActiveRange,
            (false, true) => TimestampShape::InactiveRange,
            (true, false) => TimestampShape::Active,
            (false, false) => TimestampShape::Inactive,
        }
    }

    /// Whether the timestamp is active (`<...>`). A diary timestamp always is.
    #[must_use]
    pub fn is_active(&self) -> bool {
        delimiters(self).active
    }

    /// Interior from after the first opener to before the last closer.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        let d = delimiters(self);
        let close = d.end_close.unwrap_or(d.start_close);
        TextRange::new(d.start_open.end(), close.start())
    }

    /// The bracket tokens: two for a single stamp, four for a `--` range.
    #[must_use]
    pub fn delimiters(&self) -> TimestampDelimiters {
        let d = delimiters(self);
        TimestampDelimiters {
            start_open: d.start_open,
            start_close: d.start_close,
            end_open: d.end_open,
            end_close: d.end_close,
        }
    }

    /// The start date and time. `None` for a diary timestamp.
    #[must_use]
    pub fn start(&self) -> Option<TimestampPoint> {
        let date = dates(self).into_iter().next()?;
        let fields = date_fields(&date)?;
        let (hour, minute) = start_clock(self);
        Some(TimestampPoint {
            year: parse_num(self.syntax(), fields.year)?,
            month: parse_num(self.syntax(), fields.month)?,
            day: parse_num(self.syntax(), fields.day)?,
            hour,
            minute,
        })
    }

    /// The end date and time, if the timestamp has an end.
    ///
    /// For a `--` range this is the second stamp. For a single stamp with a
    /// time range such as `10:00-11:00`, it is the start date with the end
    /// time.
    #[must_use]
    pub fn end(&self) -> Option<TimestampPoint> {
        match self.shape() {
            TimestampShape::ActiveRange | TimestampShape::InactiveRange => {
                let date = dates(self).into_iter().nth(1)?;
                let fields = date_fields(&date)?;
                let (hour, minute) = end_clock_on_range(self);
                Some(TimestampPoint {
                    year: parse_num(self.syntax(), fields.year)?,
                    month: parse_num(self.syntax(), fields.month)?,
                    day: parse_num(self.syntax(), fields.day)?,
                    hour,
                    minute,
                })
            }
            TimestampShape::Active | TimestampShape::Inactive | TimestampShape::Diary => {
                let start = self.start()?;
                let (hour, minute) = point_end_clock(self)?;
                Some(TimestampPoint {
                    year: start.year,
                    month: start.month,
                    day: start.day,
                    hour: Some(hour),
                    minute: Some(minute),
                })
            }
        }
    }

    /// The repeater, if any; the last one when several are written. Always
    /// `None` for a diary timestamp.
    #[must_use]
    pub fn repeater(&self) -> Option<Repeater> {
        let rod = last_rod(self, RodKind::Repeater)?;
        let sign = rod_sign(&rod)?;
        let kind = match sign {
            RodSign::Cumulate => RepeaterKind::Cumulate,
            RodSign::CatchUp => RepeaterKind::CatchUp,
            RodSign::Restart => RepeaterKind::Restart,
            RodSign::DelayAll | RodSign::DelayFirst => return None,
        };
        let f = rod_fields(&rod)?;
        let unit = unit_of(self.syntax(), f.unit)?;
        let value = parse_num(self.syntax(), f.value)?;
        let habit = rod_habit_fields(&rod).and_then(|h| {
            Some((
                parse_num(self.syntax(), h.value)?,
                unit_of(self.syntax(), h.unit)?,
            ))
        });
        Some(Repeater {
            kind,
            value,
            unit,
            habit,
        })
    }

    /// The delay, if any; the last one when several are written. Always
    /// `None` for a diary timestamp.
    #[must_use]
    pub fn delay(&self) -> Option<Delay> {
        let rod = last_rod(self, RodKind::Delay)?;
        let sign = rod_sign(&rod)?;
        let kind = match sign {
            RodSign::DelayAll => DelayKind::All,
            RodSign::DelayFirst => DelayKind::First,
            _ => return None,
        };
        let f = rod_fields(&rod)?;
        Some(Delay {
            kind,
            value: parse_num(self.syntax(), f.value)?,
            unit: unit_of(self.syntax(), f.unit)?,
        })
    }

    /// Diary sexp child; `%%` is not included.
    #[must_use]
    pub fn diary_sexp(&self) -> Option<DiarySexp> {
        child_view(self.syntax())
    }

    /// The field containing `offset`.
    ///
    /// Where two fields meet, the left one wins; an offset between fields
    /// yields `None`.
    #[must_use]
    pub fn field_at(&self, offset: TextSize) -> Option<TimestampField> {
        let present: Vec<_> = TIMESTAMP_FIELDS
            .iter()
            .copied()
            .filter_map(|f| self.field_range(f).map(|r| (f, r)))
            .collect();
        crate::ast::left_biased_range_owner(&present, offset)
    }

    /// Span of `field`, when this timestamp has it.
    #[must_use]
    pub fn field_range(&self, field: TimestampField) -> Option<TextRange> {
        match field {
            TimestampField::Year => date_fields(dates(self).first()?).map(|f| f.year),
            TimestampField::Month => date_fields(dates(self).first()?).map(|f| f.month),
            TimestampField::Day => date_fields(dates(self).first()?).map(|f| f.day),
            TimestampField::Weekday => timestamp_weekday(self.syntax()),
            TimestampField::Hour => start_hm(self).map(|f| f.hour),
            TimestampField::Minute => start_hm(self).map(|f| f.minute),
            TimestampField::EndYear => date_fields(dates(self).get(1)?).map(|f| f.year),
            TimestampField::EndMonth => date_fields(dates(self).get(1)?).map(|f| f.month),
            TimestampField::EndDay => date_fields(dates(self).get(1)?).map(|f| f.day),
            TimestampField::EndWeekday => timestamp_end_weekday(self.syntax()),
            TimestampField::EndHour => end_hm(self).map(|f| f.hour),
            TimestampField::EndMinute => end_hm(self).map(|f| f.minute),
            TimestampField::RepeatValue => {
                rod_fields(&last_rod(self, RodKind::Repeater)?).map(|f| f.value)
            }
            TimestampField::RepeatUnit => {
                rod_fields(&last_rod(self, RodKind::Repeater)?).map(|f| f.unit)
            }
            TimestampField::DelayValue => {
                rod_fields(&last_rod(self, RodKind::Delay)?).map(|f| f.value)
            }
            TimestampField::DelayUnit => {
                rod_fields(&last_rod(self, RodKind::Delay)?).map(|f| f.unit)
            }
            TimestampField::HabitValue => {
                rod_habit_fields(&last_rod(self, RodKind::Repeater)?).map(|f| f.value)
            }
            TimestampField::HabitUnit => {
                rod_habit_fields(&last_rod(self, RodKind::Repeater)?).map(|f| f.unit)
            }
        }
    }
}

impl HasParts for Timestamp {
    type Part = TimestampPart;
    const PARTS: &'static [TimestampPart] = &[
        TimestampPart::Date,
        TimestampPart::Time,
        TimestampPart::EndDate,
        TimestampPart::EndTime,
        TimestampPart::Repeater,
        TimestampPart::Delay,
    ];

    fn part_span(&self, part: TimestampPart) -> PartSpan {
        match part {
            TimestampPart::Date => date_part(self),
            TimestampPart::Time => time_part(self),
            TimestampPart::EndDate => end_date_part(self),
            TimestampPart::EndTime => end_time_part(self),
            TimestampPart::Repeater => rod_part(self, RodKind::Repeater),
            TimestampPart::Delay => rod_part(self, RodKind::Delay),
        }
    }
}

fn delimiters(ts: &Timestamp) -> TimestampDelimiterSpans {
    timestamp_delimiters(ts.syntax()).expect("grammar: TIMESTAMP is bracketed (forms/timestamp.rs)")
}

fn dates(ts: &Timestamp) -> Vec<SyntaxNode> {
    timestamp_date_nodes(ts.syntax())
}

fn is_range(ts: &Timestamp) -> bool {
    matches!(
        ts.shape(),
        TimestampShape::ActiveRange | TimestampShape::InactiveRange
    )
}

fn is_diary(ts: &Timestamp) -> bool {
    ts.shape() == TimestampShape::Diary
}

fn time_nodes(ts: &Timestamp) -> (Option<SyntaxNode>, Option<SyntaxNode>) {
    timestamp_stamp_times(ts.syntax())
}

/// One TIME node, or a TIME_RANGE with grammar-guaranteed halves.
struct ClockNode {
    node: SyntaxNode,
    halves: Option<TimeRangeHalves>,
}

fn clock_node(n: SyntaxNode) -> Option<ClockNode> {
    match n.kind() {
        OrgSyntaxKind::TIME => Some(ClockNode {
            node: n,
            halves: None,
        }),
        OrgSyntaxKind::TIME_RANGE => Some(ClockNode {
            halves: Some(
                time_range_halves(&n)
                    .expect("grammar: TIME_RANGE splits on interior - (forms/timestamp.rs)"),
            ),
            node: n,
        }),
        _ => None,
    }
}

impl ClockNode {
    fn start_hm(&self) -> Option<TimeFields> {
        match self.halves {
            None => time_fields(&self.node),
            Some(h) => hm_fields(&self.node, h.start),
        }
    }

    fn end_hm(&self) -> Option<TimeFields> {
        hm_fields(&self.node, self.halves?.end)
    }

    fn start_half(&self) -> TextRange {
        self.halves
            .map_or_else(|| self.node.text_range(), |h| h.start)
    }

    fn end_half(&self) -> Option<TextRange> {
        self.halves.map(|h| h.end)
    }

    fn range(&self) -> TextRange {
        self.node.text_range()
    }

    fn is_glued_range(&self) -> bool {
        self.halves.is_some()
    }
}

fn first_clock(ts: &Timestamp) -> Option<ClockNode> {
    clock_node(time_nodes(ts).0?)
}

fn second_clock(ts: &Timestamp) -> Option<ClockNode> {
    clock_node(time_nodes(ts).1?)
}

fn start_hm(ts: &Timestamp) -> Option<TimeFields> {
    first_clock(ts)?.start_hm()
}

fn end_hm(ts: &Timestamp) -> Option<TimeFields> {
    if is_range(ts) {
        second_clock(ts)?.start_hm()
    } else {
        first_clock(ts)?.end_hm()
    }
}

fn hm_pair(syntax: &SyntaxNode, fields: Option<TimeFields>) -> (Option<u8>, Option<u8>) {
    match fields {
        Some(f) => (parse_num(syntax, f.hour), parse_num(syntax, f.minute)),
        None => (None, None),
    }
}

fn start_clock(ts: &Timestamp) -> (Option<u8>, Option<u8>) {
    hm_pair(ts.syntax(), start_hm(ts))
}

fn point_end_clock(ts: &Timestamp) -> Option<(u8, u8)> {
    let (h, m) = hm_pair(ts.syntax(), end_hm(ts));
    Some((h?, m?))
}

fn end_clock_on_range(ts: &Timestamp) -> (Option<u8>, Option<u8>) {
    hm_pair(ts.syntax(), end_hm(ts))
}

fn last_rod(ts: &Timestamp, kind: RodKind) -> Option<SyntaxNode> {
    if is_diary(ts) {
        return None;
    }
    ts.syntax()
        .children()
        .filter(|c| c.kind() == OrgSyntaxKind::ROD)
        .filter(|c| rod_kind(c) == Some(kind))
        .last()
}

fn date_part(ts: &Timestamp) -> PartSpan {
    if is_diary(ts) {
        return PartSpan::NotApplicable;
    }
    let date = dates(ts)
        .into_iter()
        .next()
        .expect("grammar: regular TIMESTAMP has a DATE (forms/timestamp.rs)");
    PartSpan::Present(PresentPart::identical(date.text_range()))
}

fn end_date_part(ts: &Timestamp) -> PartSpan {
    if !is_range(ts) {
        return PartSpan::NotApplicable;
    }
    let date = dates(ts)
        .into_iter()
        .nth(1)
        .expect("grammar: -- range has two DATE children (forms/timestamp.rs)");
    PartSpan::Present(PresentPart::identical(date.text_range()))
}

fn time_part(ts: &Timestamp) -> PartSpan {
    match first_clock(ts) {
        Some(c) if c.is_glued_range() && is_range(ts) => {
            extend_before(ts, c.range(), c.start_half())
        }
        Some(c) if c.is_glued_range() => PartSpan::Present(PresentPart::identical(c.start_half())),
        Some(c) => extend_before(ts, c.range(), c.range()),
        None => PartSpan::Absent(field_dest(
            ts.syntax(),
            time_insert_offset(ts),
            Padding::None,
        )),
    }
}

fn end_time_part(ts: &Timestamp) -> PartSpan {
    if is_range(ts) {
        match second_clock(ts) {
            Some(c) if c.is_glued_range() => {
                let start = c.start_half();
                extend_before(ts, start, start)
            }
            Some(c) => extend_before(ts, c.range(), c.range()),
            None => PartSpan::Absent(field_dest(
                ts.syntax(),
                after_calendar(ts, StampEnd::End).unwrap_or_else(|| ts.content_range().end()),
                Padding::None,
            )),
        }
    } else {
        match first_clock(ts) {
            Some(c) if let Some(end) = c.end_half() => {
                let start = c.start_half();
                let removal = TextRange::new(start.end(), end.end());
                PartSpan::Present(PresentPart::new(end, end, removal))
            }
            Some(c) => PartSpan::Absent(InsertPoint::new(
                c.range().end(),
                Padding::None,
                Padding::None,
            )),
            None => PartSpan::NotApplicable,
        }
    }
}

fn rod_part(ts: &Timestamp, kind: RodKind) -> PartSpan {
    if is_diary(ts) {
        return PartSpan::NotApplicable;
    }
    if let Some(rod) = last_rod(ts, kind) {
        return extend_before(ts, rod.text_range(), rod.text_range());
    }
    let insert = rod_insert_offset(ts, kind);
    // Repeater sits before Delay (`+1w -2d`). The offset is Delay.start(),
    // which already has a space token ending there, so lead is None. Trail
    // Space stops the writer gluing `+1w-2d`.
    let trail = match kind {
        RodKind::Repeater if last_rod(ts, RodKind::Delay).is_some() => Padding::Space,
        _ => Padding::None,
    };
    PartSpan::Absent(field_dest(ts.syntax(), insert, trail))
}

/// Which stamp of a range.
#[derive(Clone, Copy)]
enum StampEnd {
    Start,
    End,
}

fn extend_before(ts: &Timestamp, carved: TextRange, content: TextRange) -> PartSpan {
    let removal = removal_extending(ts.syntax(), carved, RemovalSide::Before);
    PartSpan::Present(PresentPart::new(carved, content, removal))
}

fn after_calendar(ts: &Timestamp, which: StampEnd) -> Option<TextSize> {
    match which {
        StampEnd::Start => timestamp_weekday(ts.syntax())
            .map(rowan::TextRange::end)
            .or_else(|| dates(ts).into_iter().next().map(|d| d.text_range().end())),
        StampEnd::End => timestamp_end_weekday(ts.syntax())
            .map(rowan::TextRange::end)
            .or_else(|| dates(ts).into_iter().nth(1).map(|d| d.text_range().end())),
    }
}

fn time_insert_offset(ts: &Timestamp) -> TextSize {
    if let Some(at) = after_calendar(ts, StampEnd::Start) {
        return at;
    }
    if let Some(sx) = ts.diary_sexp() {
        return sx.range().end();
    }
    ts.content_range().start()
}

fn payload_end_before_rods(ts: &Timestamp) -> TextSize {
    if let PartSpan::Present(p) = end_time_part(ts) {
        return p.range().end();
    }
    if let PartSpan::Present(p) = time_part(ts) {
        return p.range().end();
    }
    if is_range(ts)
        && let Some(at) = after_calendar(ts, StampEnd::End)
    {
        return at;
    }
    after_calendar(ts, StampEnd::Start).unwrap_or_else(|| delimiters(ts).start_close.start())
}

/// Where a missing repeater or delay goes.
///
/// Inside a stamp the order is date, time, repeater, delay. A repeater goes
/// before an existing delay and a delay after an existing repeater; otherwise
/// either goes after the last time or date.
fn rod_insert_offset(ts: &Timestamp, kind: RodKind) -> TextSize {
    match kind {
        RodKind::Repeater => last_rod(ts, RodKind::Delay)
            .map_or_else(|| payload_end_before_rods(ts), |d| d.text_range().start()),
        RodKind::Delay => last_rod(ts, RodKind::Repeater)
            .map_or_else(|| payload_end_before_rods(ts), |r| r.text_range().end()),
    }
}

fn parse_num<T: std::str::FromStr>(node: &SyntaxNode, range: TextRange) -> Option<T> {
    text_in(node, range).parse().ok()
}

fn unit_of(node: &SyntaxNode, range: TextRange) -> Option<RepeaterUnit> {
    match text_in(node, range).as_str() {
        "h" => Some(RepeaterUnit::Hour),
        "d" => Some(RepeaterUnit::Day),
        "w" => Some(RepeaterUnit::Week),
        "m" => Some(RepeaterUnit::Month),
        "y" => Some(RepeaterUnit::Year),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::HasParts;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn ts(src: &str) -> (crate::Document, Timestamp) {
        let doc = parse(src);
        let t = Timestamp::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::TIMESTAMP)
                .into_iter()
                .next()
                .expect("timestamp"),
        )
        .expect("cast");
        (doc, t)
    }

    #[test]
    fn two_date_range_vs_clock_time_range() {
        let (_, range) = ts("<2026-01-01>--<2026-01-02>\n");
        assert_eq!(range.shape(), TimestampShape::ActiveRange);
        assert_eq!(dates(&range).len(), 2);
        assert!(time_nodes(&range).0.is_none());

        let (_, clock) = ts("[2026-01-01 Thu 09:00-10:00]\n");
        assert_eq!(clock.shape(), TimestampShape::Inactive);
        assert_eq!(dates(&clock).len(), 1);
        assert_eq!(
            time_nodes(&clock).0.as_ref().map(crate::SyntaxNode::kind),
            Some(OrgSyntaxKind::TIME_RANGE)
        );
        let start = clock.start().expect("start");
        let end = clock.end().expect("end");
        assert_eq!(
            start,
            TimestampPoint {
                year: 2026,
                month: 1,
                day: 1,
                hour: Some(9),
                minute: Some(0),
            }
        );
        assert_eq!(
            end,
            TimestampPoint {
                year: 2026,
                month: 1,
                day: 1,
                hour: Some(10),
                minute: Some(0),
            }
        );
        assert!(clock.field_range(TimestampField::EndYear).is_none());
        let (doc, clock) = ts("[2026-01-01 Thu 09:00-10:00]\n");
        assert_eq!(
            source_slice(&doc, clock.part_content_range(TimestampPart::Time).unwrap()),
            "09:00"
        );
        assert_eq!(
            source_slice(
                &doc,
                clock.part_content_range(TimestampPart::EndTime).unwrap()
            ),
            "10:00"
        );
        let end_rm = clock.part_removal_range(TimestampPart::EndTime).unwrap();
        assert!(source_slice(&doc, end_rm).contains('-'));
        let tr = nodes_of_kind(clock.syntax(), OrgSyntaxKind::TIME_RANGE)
            .into_iter()
            .next()
            .expect("time range");
        let halves = time_range_halves(&tr).unwrap();
        assert!(clock.part_at(halves.start.end()).is_none());
        assert_eq!(
            source_slice(&doc, clock.field_range(TimestampField::EndHour).unwrap()),
            "10"
        );
        assert_eq!(
            source_slice(&doc, clock.field_range(TimestampField::EndMinute).unwrap()),
            "00"
        );
        crate::ast::check_part_contract(&clock);
        crate::ast::check_part_contract(&range);
    }

    #[test]
    fn every_field_on_point_stamp() {
        let src = "<2026-01-01 Thu 09:00-10:00 +1w -2d>\n";
        let (doc, t) = ts(src);
        let present = [
            (TimestampField::Year, "2026"),
            (TimestampField::Month, "01"),
            (TimestampField::Day, "01"),
            (TimestampField::Weekday, "Thu"),
            (TimestampField::Hour, "09"),
            (TimestampField::Minute, "00"),
            (TimestampField::EndHour, "10"),
            (TimestampField::EndMinute, "00"),
            (TimestampField::RepeatValue, "1"),
            (TimestampField::RepeatUnit, "w"),
            (TimestampField::DelayValue, "2"),
            (TimestampField::DelayUnit, "d"),
        ];
        for (field, want) in present {
            assert_eq!(
                source_slice(&doc, t.field_range(field).unwrap()),
                want,
                "{field:?}"
            );
        }
        for field in [
            TimestampField::EndYear,
            TimestampField::EndMonth,
            TimestampField::EndDay,
            TimestampField::EndWeekday,
            TimestampField::HabitValue,
            TimestampField::HabitUnit,
        ] {
            assert!(t.field_range(field).is_none(), "{field:?}");
        }
        crate::ast::check_part_contract(&t);
    }

    #[test]
    fn end_year_on_range_not_year() {
        let src = "<2026-01-01 Thu>--<2026-02-03 Fri>\n";
        let (doc, t) = ts(src);
        let y2 = t.field_range(TimestampField::EndYear).unwrap();
        assert_eq!(source_slice(&doc, y2), "2026");
        assert_eq!(t.field_at(y2.start()), Some(TimestampField::EndYear));
        let wd = t.field_range(TimestampField::EndWeekday).unwrap();
        assert_eq!(t.field_at(wd.start()), Some(TimestampField::EndWeekday));
        assert_eq!(
            t.field_at(t.field_range(TimestampField::Year).unwrap().start()),
            Some(TimestampField::Year)
        );
    }

    #[test]
    fn mixed_end_time_on_range() {
        let src = "<2026-01-01 09:00-10:00>--<2026-01-02 11:00>\n";
        let (doc, t) = ts(src);
        assert_eq!(
            source_slice(&doc, t.field_range(TimestampField::EndHour).unwrap()),
            "11"
        );
        assert_eq!(
            source_slice(&doc, t.part_content_range(TimestampPart::EndTime).unwrap()),
            "11:00"
        );
        let tr = nodes_of_kind(t.syntax(), OrgSyntaxKind::TIME_RANGE)
            .into_iter()
            .next()
            .expect("first-stamp time range");
        let halves = time_range_halves(&tr).unwrap();
        assert!(t.field_at(halves.end.start()).is_none());
        assert_eq!(
            t.part_range(TimestampPart::Time).unwrap(),
            tr.text_range(),
            "Time on a -- first-stamp TIME_RANGE is the whole node"
        );
        crate::ast::check_part_contract(&t);
    }

    #[test]
    fn repeater_delay_either_order_and_habit() {
        let (_, t) = ts("<2026-01-01 +1w -2d>\n");
        assert_eq!(t.repeater().unwrap().kind, RepeaterKind::Cumulate);
        assert_eq!(t.repeater().unwrap().value, 1);
        assert_eq!(t.delay().unwrap().kind, DelayKind::All);
        assert_eq!(t.delay().unwrap().value, 2);
        let (_, t) = ts("<2026-01-01 -2d +1w>\n");
        assert_eq!(t.repeater().unwrap().value, 1);
        assert_eq!(t.delay().unwrap().value, 2);
        let (_, t) = ts("<2026-01-01 .+2d/4d>\n");
        let r = t.repeater().unwrap();
        assert_eq!(r.kind, RepeaterKind::Restart);
        assert_eq!(r.habit, Some((4, RepeaterUnit::Day)));
        crate::ast::check_part_contract(&t);
        let (_, t) = ts("<2026-01-01 ++1d>\n");
        assert_eq!(t.repeater().unwrap().kind, RepeaterKind::CatchUp);
        let (_, t) = ts("<2026-01-01 --2d>\n");
        assert_eq!(t.delay().unwrap().kind, DelayKind::First);
    }

    #[test]
    fn diary_percent_outside_sexp_and_wallclock_none() {
        let src = "<%%(diary-float t 0 1) 09:00-10:00>\n";
        let (doc, t) = ts(src);
        assert_eq!(t.shape(), TimestampShape::Diary);
        let sx = t.diary_sexp().expect("sexp");
        assert!(!source_slice(&doc, sx.range()).contains("%%"));
        assert!(t.start().is_none());
        assert!(t.end().is_none());
        assert!(t.field_range(TimestampField::Hour).is_some());
        assert!(t.field_range(TimestampField::EndHour).is_some());
        assert!(matches!(
            t.part_span(TimestampPart::Repeater),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&t);

        let (_, t) = ts("<%%(diary-float t 0 1)>\n");
        let ip = t.part_insert_point(TimestampPart::Time).unwrap();
        assert_eq!(ip.offset(), t.diary_sexp().unwrap().range().end());
    }

    #[test]
    fn delimiters_point_two_range_four() {
        let (_, t) = ts("<2026-01-01>\n");
        let d = t.delimiters();
        assert!(d.end_open.is_none());
        let (_, t) = ts("<2026-01-01>--<2026-01-02>\n");
        let d = t.delimiters();
        assert!(d.end_open.is_some() && d.end_close.is_some());
        assert!(t.content_range().contains(d.start_close.start()));
    }

    #[test]
    fn planning_slot_is_same_timestamp_type() {
        let doc = parse("* H\nDEADLINE: <2026-01-01 Thu>\n");
        let p = crate::ast::Planning::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PLANNING)
                .into_iter()
                .next()
                .expect("planning"),
        )
        .expect("cast");
        let t = p.deadline().expect("deadline");
        assert_eq!(
            t.field_at(t.field_range(TimestampField::Year).unwrap().start()),
            Some(TimestampField::Year)
        );
        crate::ast::check_part_contract(&t);
    }

    #[test]
    fn absent_time_and_end_time_inserts() {
        let (doc, t) = ts("<2026-01-01 Thu>\n");
        let ip = t.part_insert_point(TimestampPart::Time).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        assert!(matches!(
            t.part_span(TimestampPart::EndTime),
            PartSpan::NotApplicable
        ));
        let wd = t.field_range(TimestampField::Weekday).unwrap();
        let rip = t.part_insert_point(TimestampPart::Repeater).unwrap();
        assert_eq!(rip.offset(), wd.end());
        assert_eq!(
            crate::tests::written(doc.source(), rip, "+1w"),
            "<2026-01-01 Thu +1w>\n"
        );
        let dip = t.part_insert_point(TimestampPart::Delay).unwrap();
        assert_eq!(dip.offset(), wd.end());
        assert_eq!(
            crate::tests::written(doc.source(), dip, "-2d"),
            "<2026-01-01 Thu -2d>\n"
        );

        let (doc, t) = ts("<2026-01-01 09:00>\n");
        let ip = t.part_insert_point(TimestampPart::EndTime).unwrap();
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(
            crate::tests::written(doc.source(), ip, "-10:00"),
            "<2026-01-01 09:00-10:00>\n"
        );
        crate::ast::check_part_contract(&t);

        let src = "<2026-01-01 Thu>--<2026-01-02 Fri>\n";
        let (doc, t) = ts(src);
        let wd2 = t.field_range(TimestampField::EndWeekday).unwrap();
        let rip = t.part_insert_point(TimestampPart::Repeater).unwrap();
        assert_eq!(rip.offset(), wd2.end());
        assert_eq!(
            crate::tests::written(doc.source(), rip, "+1w"),
            "<2026-01-01 Thu>--<2026-01-02 Fri +1w>\n"
        );
        let dip = t.part_insert_point(TimestampPart::Delay).unwrap();
        assert_eq!(dip.offset(), wd2.end());

        let (doc, t) = ts("<2026-01-01 -2d>\n");
        let rip = t.part_insert_point(TimestampPart::Repeater).unwrap();
        assert_eq!(rip.lead(), Padding::None);
        assert_eq!(rip.trail(), Padding::Space);
        assert_eq!(
            crate::tests::written(doc.source(), rip, "+1w"),
            "<2026-01-01 +1w -2d>\n"
        );
        let (doc, t) = ts("<2026-01-01 +1w>\n");
        let dip = t.part_insert_point(TimestampPart::Delay).unwrap();
        assert_eq!(
            crate::tests::written(doc.source(), dip, "-2d"),
            "<2026-01-01 +1w -2d>\n"
        );
    }

    #[test]
    fn whitespace_perturbed_fields_and_start() {
        let (doc, t) = ts("<2026-01-01  Thu   09:00>\n");
        assert_eq!(
            source_slice(&doc, t.field_range(TimestampField::Weekday).unwrap()),
            "Thu"
        );
        assert_eq!(
            source_slice(&doc, t.field_range(TimestampField::Hour).unwrap()),
            "09"
        );
        let start = t.start().expect("start");
        assert_eq!(start.year, 2026);
        assert_eq!(start.month, 1);
        assert_eq!(start.day, 1);
        assert_eq!(start.hour, Some(9));
        assert_eq!(start.minute, Some(0));
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::TIMESTAMP) {
                if let Some(t) = Timestamp::cast(n) {
                    crate::ast::check_part_contract(&t);
                }
            }
        }
    }
}
