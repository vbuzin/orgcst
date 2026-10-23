//! Timestamp text: a point, a time range inside one stamp, a `--` range, a
//! diary sexp.

use super::{SynthesisError, TimestampProblem};
use crate::{
    Delay, DelayKind, Repeater, RepeaterKind, RepeaterUnit, TimestampPoint, TimestampShape,
};
use std::fmt::Write;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TimestampSpec<'a> {
    pub shape: TimestampShape,
    pub start: Option<TimestampPoint>,
    pub end: Option<TimestampPoint>,
    pub repeater: Option<Repeater>,
    pub delay: Option<Delay>,
    pub diary_sexp: Option<&'a str>,
}

pub(crate) fn render_timestamp(spec: &TimestampSpec<'_>) -> Result<String, SynthesisError> {
    render_timestamp_inner(spec).map_err(SynthesisError::InvalidTimestamp)
}

fn render_timestamp_inner(spec: &TimestampSpec<'_>) -> Result<String, TimestampProblem> {
    match spec.shape {
        TimestampShape::Active | TimestampShape::Inactive => render_point_shape(spec),
        TimestampShape::ActiveRange | TimestampShape::InactiveRange => render_range_shape(spec),
        TimestampShape::Diary => render_diary(spec),
    }
}

fn render_point_shape(spec: &TimestampSpec<'_>) -> Result<String, TimestampProblem> {
    if spec.diary_sexp.is_some() {
        return Err(TimestampProblem::UnexpectedDiary);
    }
    let Some(start) = spec.start else {
        return Err(TimestampProblem::MissingStart);
    };
    check_repeater_and_delay(spec.repeater, spec.delay)?;
    let (open, close) = brackets(spec.shape);
    match spec.end {
        None => render_point(open, close, start, None, spec.repeater, spec.delay),
        Some(end) => {
            if !same_ymd(start, end) {
                return Err(TimestampProblem::EndDateMismatch);
            }
            let Some(end_clock) = clock_of(end)? else {
                return Err(TimestampProblem::EndWithoutTime);
            };
            render_point(
                open,
                close,
                start,
                Some(end_clock),
                spec.repeater,
                spec.delay,
            )
        }
    }
}

fn render_range_shape(spec: &TimestampSpec<'_>) -> Result<String, TimestampProblem> {
    if spec.diary_sexp.is_some() {
        return Err(TimestampProblem::UnexpectedDiary);
    }
    let Some(start) = spec.start else {
        return Err(TimestampProblem::MissingStart);
    };
    let Some(end) = spec.end else {
        return Err(TimestampProblem::MissingEnd);
    };
    check_repeater_and_delay(spec.repeater, spec.delay)?;
    clock_of(start)?;
    clock_of(end)?;
    let (open, close) = brackets(spec.shape);
    let a = render_point(open, close, start, None, spec.repeater, spec.delay)?;
    let b = render_point(open, close, end, None, None, None)?;
    Ok(format!("{a}--{b}"))
}

fn render_diary(spec: &TimestampSpec<'_>) -> Result<String, TimestampProblem> {
    let Some(sexp) = spec.diary_sexp else {
        return Err(TimestampProblem::MissingDiarySexp);
    };
    if spec.repeater.is_some() || spec.delay.is_some() {
        return Err(TimestampProblem::DiaryHasRepeaterOrDelay);
    }
    if sexp.contains(['>', '\n', '\r']) {
        return Err(TimestampProblem::InvalidDiarySexp);
    }
    let mut s = String::from("<%%(");
    s.push_str(sexp);
    s.push(')');
    if let Some(start) = spec.start
        && let Some((h, m)) = clock_of(start)?
    {
        s.push(' ');
        let _ = write!(s, "{h:02}:{m:02}");
    }
    s.push('>');
    Ok(s)
}

fn brackets(shape: TimestampShape) -> (char, char) {
    match shape {
        TimestampShape::Active | TimestampShape::ActiveRange | TimestampShape::Diary => ('<', '>'),
        TimestampShape::Inactive | TimestampShape::InactiveRange => ('[', ']'),
    }
}

fn render_point(
    open: char,
    close: char,
    p: TimestampPoint,
    end_clock: Option<(u8, u8)>,
    repeater: Option<Repeater>,
    delay: Option<Delay>,
) -> Result<String, TimestampProblem> {
    let start_clock = clock_of(p)?;
    if end_clock.is_some() && start_clock.is_none() {
        return Err(TimestampProblem::InvalidTime);
    }
    let mut s = String::new();
    s.push(open);
    let _ = write!(s, "{:04}-{:02}-{:02}", p.year, p.month, p.day);
    if let Some(wd) = weekday_abbrev(p.year, p.month, p.day) {
        s.push(' ');
        s.push_str(wd);
    }
    if let Some((h, m)) = start_clock {
        s.push(' ');
        let _ = write!(s, "{h:02}:{m:02}");
        if let Some((h2, m2)) = end_clock {
            s.push('-');
            let _ = write!(s, "{h2:02}:{m2:02}");
        }
    }
    if let Some(r) = repeater {
        s.push(' ');
        s.push_str(&render_repeater(r));
    }
    if let Some(d) = delay {
        s.push(' ');
        s.push_str(&render_delay(d));
    }
    s.push(close);
    Ok(s)
}

fn clock_of(p: TimestampPoint) -> Result<Option<(u8, u8)>, TimestampProblem> {
    match (p.hour, p.minute) {
        (None, None) => Ok(None),
        (Some(h), Some(m)) if h < 24 && m < 60 => Ok(Some((h, m))),
        _ => Err(TimestampProblem::InvalidTime),
    }
}

fn same_ymd(a: TimestampPoint, b: TimestampPoint) -> bool {
    a.year == b.year && a.month == b.month && a.day == b.day
}

fn check_repeater_and_delay(
    repeater: Option<Repeater>,
    delay: Option<Delay>,
) -> Result<(), TimestampProblem> {
    if let Some(r) = repeater
        && (r.value == 0 || matches!(r.habit, Some((0, _))))
    {
        return Err(TimestampProblem::ZeroRepeater);
    }
    if let Some(d) = delay
        && d.value == 0
    {
        return Err(TimestampProblem::ZeroDelay);
    }
    Ok(())
}

fn render_repeater(r: Repeater) -> String {
    let sign = match r.kind {
        RepeaterKind::Cumulate => "+",
        RepeaterKind::CatchUp => "++",
        RepeaterKind::Restart => ".+",
    };
    let mut s = format!("{}{}{}", sign, r.value, unit_char(r.unit));
    if let Some((v, u)) = r.habit {
        s.push('/');
        let _ = write!(s, "{v}{}", unit_char(u));
    }
    s
}

fn render_delay(d: Delay) -> String {
    let sign = match d.kind {
        DelayKind::All => "-",
        DelayKind::First => "--",
    };
    format!("{}{}{}", sign, d.value, unit_char(d.unit))
}

fn unit_char(u: RepeaterUnit) -> char {
    match u {
        RepeaterUnit::Hour => 'h',
        RepeaterUnit::Day => 'd',
        RepeaterUnit::Week => 'w',
        RepeaterUnit::Month => 'm',
        RepeaterUnit::Year => 'y',
    }
}

fn weekday_abbrev(year: u16, month: u8, day: u8) -> Option<&'static str> {
    if !is_valid_gregorian(year, month, day) {
        return None;
    }
    let days = days_from_civil(i32::from(year), u32::from(month), u32::from(day));
    // Unix day 0 is Thursday; +4 maps 0 onto NAMES[4].
    Some(NAMES[(days + 4).rem_euclid(7) as usize])
}

const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn is_valid_gregorian(year: u16, month: u8, day: u8) -> bool {
    days_in_month(year, month).is_some_and(|dim| (1..=dim).contains(&day))
}

fn days_in_month(year: u16, month: u8) -> Option<u8> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap(year) { 29 } else { 28 }),
        _ => None,
    }
}

fn is_leap(year: u16) -> bool {
    let y = u32::from(year);
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days since 1970-01-01 (Unix epoch), after Howard Hinnant's `days_from_civil`.
///
/// Callers pass a `u16` year. The era split keeps year-of-era in `0..400`
/// and day-of-era inside `i32`.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn days_from_civil(mut y: i32, m: u32, d: u32) -> i32 {
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::{
        Delay, DelayKind, Repeater, RepeaterKind, RepeaterUnit, TimestampPoint, TimestampShape,
    };

    fn point(year: u16, month: u8, day: u8) -> TimestampPoint {
        TimestampPoint {
            year,
            month,
            day,
            hour: None,
            minute: None,
        }
    }

    fn spec_active(p: TimestampPoint) -> TimestampSpec<'static> {
        TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(p),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        }
    }

    #[test]
    fn weekday_pins() {
        assert_eq!(weekday_abbrev(2020, 1, 1), Some("Wed"));
        assert_eq!(weekday_abbrev(1970, 1, 1), Some("Thu"));
        assert_eq!(weekday_abbrev(2020, 2, 29), Some("Sat"));
        assert_eq!(weekday_abbrev(2020, 2, 30), None);
        assert_eq!(weekday_abbrev(2020, 0, 1), None);
        assert_eq!(weekday_abbrev(2020, 13, 1), None);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one timestamp recipe table
    fn point_and_range_recipes() {
        assert_eq!(
            render_timestamp(&spec_active(point(2020, 1, 1))).unwrap(),
            "<2020-01-01 Wed>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Inactive,
                start: Some(point(2026, 6, 24)),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "[2026-06-24 Wed]"
        );
        let in_stamp = TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 1,
                hour: Some(10),
                minute: Some(0),
            }),
            end: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 1,
                hour: Some(11),
                minute: Some(0),
            }),
            repeater: Some(Repeater {
                kind: RepeaterKind::Cumulate,
                value: 1,
                unit: RepeaterUnit::Week,
                habit: None,
            }),
            delay: Some(Delay {
                kind: DelayKind::All,
                value: 2,
                unit: RepeaterUnit::Day,
            }),
            diary_sexp: None,
        };
        assert_eq!(
            render_timestamp(&in_stamp).unwrap(),
            "<2020-01-01 Wed 10:00-11:00 +1w -2d>"
        );
        let range = TimestampSpec {
            shape: TimestampShape::ActiveRange,
            start: Some(point(2020, 1, 1)),
            end: Some(point(2020, 1, 5)),
            repeater: None,
            delay: None,
            diary_sexp: None,
        };
        assert_eq!(
            render_timestamp(&range).unwrap(),
            "<2020-01-01 Wed>--<2020-01-05 Sun>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 2, 30)),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-02-30>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 0, 1)),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-00-01>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("diary-float t 4 2"),
            })
            .unwrap(),
            "<%%(diary-float t 4 2)>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::InactiveRange,
                start: Some(point(2020, 1, 1)),
                end: Some(point(2020, 1, 5)),
                repeater: None,
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "[2020-01-01 Wed]--[2020-01-05 Sun]"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::CatchUp,
                    value: 1,
                    unit: RepeaterUnit::Day,
                    habit: None,
                }),
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-01-01 Wed ++1d>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::Restart,
                    value: 1,
                    unit: RepeaterUnit::Week,
                    habit: None,
                }),
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-01-01 Wed .+1w>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: None,
                delay: Some(Delay {
                    kind: DelayKind::First,
                    value: 2,
                    unit: RepeaterUnit::Day,
                }),
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-01-01 Wed --2d>"
        );
        assert_eq!(
            render_timestamp(&TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::CatchUp,
                    value: 1,
                    unit: RepeaterUnit::Day,
                    habit: Some((3, RepeaterUnit::Day)),
                }),
                delay: None,
                diary_sexp: None,
            })
            .unwrap(),
            "<2020-01-01 Wed ++1d/3d>"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // parse-stability cases for one family
    fn timestamp_parse_stability() {
        let in_stamp = TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 1,
                hour: Some(10),
                minute: Some(0),
            }),
            end: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 1,
                hour: Some(11),
                minute: Some(0),
            }),
            repeater: Some(Repeater {
                kind: RepeaterKind::Cumulate,
                value: 1,
                unit: RepeaterUnit::Week,
                habit: None,
            }),
            delay: Some(Delay {
                kind: DelayKind::All,
                value: 2,
                unit: RepeaterUnit::Day,
            }),
            diary_sexp: None,
        };
        let bytes = render_timestamp(&in_stamp).unwrap();
        assert_eq!(bytes, "<2020-01-01 Wed 10:00-11:00 +1w -2d>");
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &in_stamp);

        let range = TimestampSpec {
            shape: TimestampShape::ActiveRange,
            start: Some(point(2020, 1, 1)),
            end: Some(point(2020, 1, 5)),
            repeater: None,
            delay: None,
            diary_sexp: None,
        };
        let bytes = render_timestamp(&range).unwrap();
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &range);

        let mixed = TimestampSpec {
            shape: TimestampShape::ActiveRange,
            start: Some(point(2020, 1, 1)),
            end: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 5,
                hour: Some(12),
                minute: Some(0),
            }),
            repeater: None,
            delay: None,
            diary_sexp: None,
        };
        let bytes = render_timestamp(&mixed).unwrap();
        assert_eq!(bytes, "<2020-01-01 Wed>--<2020-01-05 Sun 12:00>");
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &mixed);

        let diary = TimestampSpec {
            shape: TimestampShape::Diary,
            start: Some(TimestampPoint {
                year: 0,
                month: 0,
                day: 0,
                hour: Some(9),
                minute: Some(0),
            }),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: Some("diary-float t 4 2"),
        };
        let bytes = render_timestamp(&diary).unwrap();
        assert_eq!(bytes, "<%%(diary-float t 4 2) 09:00>");
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &diary);

        let inactive_range = TimestampSpec {
            shape: TimestampShape::InactiveRange,
            start: Some(point(2020, 1, 1)),
            end: Some(point(2020, 1, 5)),
            repeater: None,
            delay: None,
            diary_sexp: None,
        };
        let bytes = render_timestamp(&inactive_range).unwrap();
        assert_eq!(bytes, "[2020-01-01 Wed]--[2020-01-05 Sun]");
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &inactive_range);

        let catch_up = TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 1)),
            end: None,
            repeater: Some(Repeater {
                kind: RepeaterKind::CatchUp,
                value: 1,
                unit: RepeaterUnit::Day,
                habit: Some((3, RepeaterUnit::Day)),
            }),
            delay: Some(Delay {
                kind: DelayKind::First,
                value: 2,
                unit: RepeaterUnit::Day,
            }),
            diary_sexp: None,
        };
        let bytes = render_timestamp(&catch_up).unwrap();
        assert_eq!(bytes, "<2020-01-01 Wed ++1d/3d --2d>");
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &catch_up);

        let restart = TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(point(2020, 1, 1)),
            end: None,
            repeater: Some(Repeater {
                kind: RepeaterKind::Restart,
                value: 1,
                unit: RepeaterUnit::Week,
                habit: None,
            }),
            delay: None,
            diary_sexp: None,
        };
        let bytes = render_timestamp(&restart).unwrap();
        super::super::equal::assert_timestamp_eq(&parse(&format!("{bytes}\n")), &restart);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one rejection case table
    fn timestamp_rejects() {
        use TimestampProblem::*;
        let err = |spec: TimestampSpec<'_>| match render_timestamp(&spec) {
            Err(SynthesisError::InvalidTimestamp(p)) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            MissingStart
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: Some(point(2020, 1, 2)),
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            EndDateMismatch
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: None,
                    minute: None,
                }),
                end: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: Some(11),
                    minute: Some(0),
                }),
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            InvalidTime
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: Some(10),
                    minute: None,
                }),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            InvalidTime
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: Some(24),
                    minute: Some(0),
                }),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            InvalidTime
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::ActiveRange,
                start: Some(point(2020, 1, 1)),
                end: Some(point(2020, 1, 5)),
                repeater: None,
                delay: None,
                diary_sexp: Some("x"),
            }),
            UnexpectedDiary
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::CatchUp,
                    value: 1,
                    unit: RepeaterUnit::Day,
                    habit: Some((0, RepeaterUnit::Day)),
                }),
                delay: None,
                diary_sexp: None,
            }),
            ZeroRepeater
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::Cumulate,
                    value: 1,
                    unit: RepeaterUnit::Day,
                    habit: None,
                }),
                delay: None,
                diary_sexp: Some("x"),
            }),
            DiaryHasRepeaterOrDelay
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            MissingDiarySexp
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("foo)>bar"),
            }),
            InvalidDiarySexp
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: Some(Repeater {
                    kind: RepeaterKind::Cumulate,
                    value: 0,
                    unit: RepeaterUnit::Day,
                    habit: None,
                }),
                delay: None,
                diary_sexp: None,
            }),
            ZeroRepeater
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::ActiveRange,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            MissingEnd
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: Some(10),
                    minute: Some(0),
                }),
                end: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: None,
                    minute: None,
                }),
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            EndWithoutTime
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(TimestampPoint {
                    year: 2020,
                    month: 1,
                    day: 1,
                    hour: Some(10),
                    minute: Some(60),
                }),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            InvalidTime
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: None,
                delay: Some(Delay {
                    kind: DelayKind::All,
                    value: 0,
                    unit: RepeaterUnit::Day,
                }),
                diary_sexp: None,
            }),
            ZeroDelay
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Active,
                start: Some(point(2020, 1, 1)),
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("x"),
            }),
            UnexpectedDiary
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: Some(Delay {
                    kind: DelayKind::All,
                    value: 1,
                    unit: RepeaterUnit::Day,
                }),
                diary_sexp: Some("x"),
            }),
            DiaryHasRepeaterOrDelay
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::ActiveRange,
                start: None,
                end: Some(point(2020, 1, 5)),
                repeater: None,
                delay: None,
                diary_sexp: None,
            }),
            MissingStart
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("a>b"),
            }),
            InvalidDiarySexp
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("foo) extra >\n* Injected heading"),
            }),
            InvalidDiarySexp
        );
        assert_eq!(
            err(TimestampSpec {
                shape: TimestampShape::Diary,
                start: None,
                end: None,
                repeater: None,
                delay: None,
                diary_sexp: Some("x\ny"),
            }),
            InvalidDiarySexp
        );
    }
}
