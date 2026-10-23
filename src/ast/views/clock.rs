//! Clock lines in three shapes: running (`CLOCK: [ts]`), closed
//! (`CLOCK: [ts]--[ts] => H:MM`) and duration only (`CLOCK: => H:MM`).
//!
//! The duration has no node of its own; the view finds it among the tokens
//! of the line. A clock is closed when [`Clock::duration`] is `Some`.

use super::timestamp::Timestamp;
use crate::TextRange;
use crate::ast::layout::{
    RemovalSide, clock_duration_range, clock_duration_value_range, removal_extending,
};
use crate::ast::support::{child_view, field_dest};
use crate::ast::{AstNode, HasParts, Padding, PartSpan, PresentPart};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A clock line: `CLOCK: [ts]`, `CLOCK: [ts]--[ts] => H:MM` or
    /// `CLOCK: => H:MM`.
    Clock,
    OrgSyntaxKind::CLOCK
);
crate::ast::impl_leaf_element_view!(Clock);

/// Parts of a clock line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockPart {
    /// The timestamp or timestamp range after `CLOCK:`.
    Timestamp,
    /// `=> H:MM`; its content is the `H:MM` value.
    Duration,
}

impl Clock {
    /// Inactive timestamp child, when the shape has one.
    #[must_use]
    pub fn timestamp(&self) -> Option<Timestamp> {
        child_view(self.syntax())
    }

    /// Absolute span of `=> H:MM`, when the shape has a duration.
    #[must_use]
    pub fn duration(&self) -> Option<TextRange> {
        clock_duration_range(self.syntax())
    }
}

impl HasParts for Clock {
    type Part = ClockPart;
    const PARTS: &'static [ClockPart] = &[ClockPart::Timestamp, ClockPart::Duration];

    fn part_span(&self, part: ClockPart) -> PartSpan {
        match part {
            ClockPart::Timestamp => timestamp_part(self),
            ClockPart::Duration => duration_part(self),
        }
    }
}

fn timestamp_part(clock: &Clock) -> PartSpan {
    if let Some(ts) = clock.timestamp() {
        let carved = ts.range();
        let removal = if clock.duration().is_some() {
            removal_extending(clock.syntax(), carved, RemovalSide::After)
        } else {
            // Deleting the last payload must not leave a bare CLOCK:.
            clock.range()
        };
        return PartSpan::Present(PresentPart::new(carved, carved, removal));
    }
    let Some(d) = clock.duration() else {
        return PartSpan::NotApplicable;
    };
    PartSpan::Absent(field_dest(clock.syntax(), d.start(), Padding::Space))
}

fn duration_part(clock: &Clock) -> PartSpan {
    if let Some(carved) = clock.duration() {
        let content = clock_duration_value_range(clock.syntax()).unwrap_or(carved);
        let removal = if clock.timestamp().is_some() {
            removal_extending(clock.syntax(), carved, RemovalSide::Before)
        } else {
            clock.range()
        };
        return PartSpan::Present(PresentPart::new(carved, content, removal));
    }
    let Some(ts) = clock.timestamp() else {
        return PartSpan::NotApplicable;
    };
    PartSpan::Absent(field_dest(clock.syntax(), ts.range().end(), Padding::None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiagnosticKind;
    use crate::ast::Element;
    use crate::ast::ElementView;
    use crate::ast::Padding;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice, written};

    fn clock(src: &str) -> (crate::Document, Clock) {
        let doc = parse(src);
        let c = Clock::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK)
                .into_iter()
                .next()
                .expect("clock"),
        )
        .expect("cast");
        (doc, c)
    }

    #[test]
    fn clock_is_a_leaf_element() {
        let src = "#+CAPTION: c\nCLOCK: [2020-01-01 Wed 10:00]\npara\n";
        let doc = parse(src);
        let c = Clock::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK)
                .into_iter()
                .next()
                .expect("clock"),
        )
        .expect("cast");
        assert!(c.content_range().is_none());
        assert!(!c.is_greater());
        assert!(c.affiliated().is_none());
        assert_eq!(c.full_range(), c.range());
        let clock_el = Element::cast(c.syntax().clone()).unwrap();
        let caption = clock_el.prev_element().expect("caption keyword");
        assert_eq!(caption.kind(), OrgSyntaxKind::KEYWORD);
        assert!(
            matches!(caption, Element::Keyword(_)),
            "clocks reject affiliation; CAPTION stays a keyword element"
        );
        let para = clock_el.next_element().expect("paragraph sibling");
        assert_eq!(para.kind(), OrgSyntaxKind::PARAGRAPH);
        assert!(matches!(para, Element::Paragraph(_)));
        assert!(matches!(Element::from(&c), Element::Clock(_)));
        assert_eq!(clock_el.range(), c.range());
        assert_eq!(clock_el.full_range(), c.full_range());
        assert!(clock_el.content_range().is_none());
        assert!(!clock_el.is_greater());
    }

    #[test]
    fn element_clock_agrees_with_view_on_three_shapes() {
        for src in [
            "CLOCK: [2020-01-01 Wed 10:00]\n",
            "CLOCK: [2020-01-01 Wed 10:00]--[2020-01-01 Wed 11:00] => 1:00\n",
            "CLOCK: => 2:15\n",
        ] {
            let (_, c) = clock(src);
            let el = Element::from(&c);
            assert!(matches!(el, Element::Clock(_)), "{src:?}");
            assert_eq!(el.range(), c.range(), "{src:?}");
            assert_eq!(el.full_range(), c.full_range(), "{src:?}");
            assert!(el.content_range().is_none(), "{src:?}");
            assert!(!el.is_greater(), "{src:?}");
            let Element::Clock(view) = el else {
                panic!("Element::from(Clock) is Element::Clock");
            };
            assert_eq!(
                view.timestamp().map(|t| t.range()),
                c.timestamp().map(|t| t.range()),
                "{src:?}"
            );
            assert_eq!(view.duration(), c.duration(), "{src:?}");
        }
    }

    #[test]
    fn three_shapes() {
        let (doc, c) = clock("CLOCK: [2020-01-01 Wed 10:00]\n");
        assert!(c.timestamp().is_some());
        assert!(c.duration().is_none());
        assert_eq!(
            source_slice(&doc, c.timestamp().unwrap().range()),
            "[2020-01-01 Wed 10:00]"
        );
        crate::ast::check_part_contract(&c);

        let src = "CLOCK: [2020-01-01 Wed 10:00]--[2020-01-01 Wed 11:00] => 1:00\n";
        let (doc, c) = clock(src);
        assert!(c.timestamp().is_some());
        let d = c.duration().unwrap();
        assert_eq!(source_slice(&doc, d), "=> 1:00");
        assert_eq!(
            source_slice(&doc, c.part_content_range(ClockPart::Duration).unwrap()),
            "1:00"
        );
        crate::ast::check_part_contract(&c);
        let ts_rm = c.part_removal_range(ClockPart::Timestamp).unwrap();
        assert!(source_slice(&doc, ts_rm).ends_with(' '));
        assert!(!source_slice(&doc, ts_rm).contains("=>"));
        assert!(!source_slice(&doc, ts_rm).contains('\n'));
        let dur_rm = c.part_removal_range(ClockPart::Duration).unwrap();
        assert!(source_slice(&doc, dur_rm).contains("=>"));
        assert!(!source_slice(&doc, dur_rm).contains("[2020-01-01"));
        assert!(!source_slice(&doc, dur_rm).contains('\n'));

        let (doc, c) = clock("CLOCK: => 2:15\n");
        assert!(c.timestamp().is_none());
        assert_eq!(source_slice(&doc, c.duration().unwrap()), "=> 2:15");
        assert_eq!(
            source_slice(&doc, c.part_content_range(ClockPart::Duration).unwrap()),
            "2:15"
        );
        crate::ast::check_part_contract(&c);
    }

    #[test]
    fn internal_time_range_is_still_a_point_stamp() {
        let (_, c) = clock("CLOCK: [2020-01-01 Wed 10:00-11:00]\n");
        assert!(c.timestamp().is_some());
        assert!(c.duration().is_none());
    }

    #[test]
    fn malformed_clock_is_paragraph() {
        for src in [
            "CLOCK: <2020-01-01 Wed>\n",
            "CLOCK: text\n",
            "CLOCK:[2020-01-01 Wed]\n",
        ] {
            let doc = parse(src);
            assert!(
                nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK).is_empty(),
                "{src:?}"
            );
            assert!(
                !nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH).is_empty(),
                "{src:?}"
            );
            assert!(
                doc.diagnostics()
                    .iter()
                    .any(|d| matches!(d.kind(), DiagnosticKind::MalformedClock)),
                "{src:?}"
            );
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH) {
                assert!(
                    Clock::cast(node.clone()).is_none(),
                    "Clock::cast must refuse {src:?}"
                );
                let el = Element::cast(node).expect("paragraph element");
                assert!(
                    matches!(el, Element::Paragraph(_)),
                    "Element::Clock must not succeed on {src:?}"
                );
            }
        }
    }

    #[test]
    fn whitespace_indent_and_merged_duration_space() {
        let (_, c) = clock("   clock: [2020-01-01 Wed]\n");
        assert!(c.timestamp().is_some());

        let (doc, c) = clock("CLOCK: [2020-01-01 Wed]--[2020-01-01 Wed] =>  12:05\n");
        assert_eq!(source_slice(&doc, c.duration().unwrap()), "=>  12:05");
        assert_eq!(
            source_slice(&doc, c.part_content_range(ClockPart::Duration).unwrap()),
            "12:05"
        );
    }

    #[test]
    fn duration_only_timestamp_insert() {
        let (doc, c) = clock("CLOCK: => 2:15\n");
        let ip = c.part_insert_point(ClockPart::Timestamp).unwrap();
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(ip.trail(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, "[2020-01-01]"),
            "CLOCK: [2020-01-01] => 2:15\n"
        );
    }

    #[test]
    fn running_duration_insert_keeps_trailing_spaces() {
        let (doc, c) = clock("CLOCK: [2020-01-01 Wed]  \n");
        let ip = c.part_insert_point(ClockPart::Duration).unwrap();
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(ip.trail(), Padding::None);
        assert_eq!(
            written(doc.source(), ip, "=> H:MM"),
            "CLOCK: [2020-01-01 Wed] => H:MM  \n"
        );
    }

    #[test]
    fn singleton_payload_removal_is_the_clock_node() {
        let (_, running) = clock("CLOCK: [2020-01-01 Wed]\n");
        assert_eq!(
            running.part_removal_range(ClockPart::Timestamp).unwrap(),
            running.range()
        );
        let (_, only) = clock("CLOCK: => 2:15\n");
        assert_eq!(
            only.part_removal_range(ClockPart::Duration).unwrap(),
            only.range()
        );
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK) {
                let c = Clock::cast(node).unwrap();
                crate::ast::check_part_contract(&c);
            }
        }
    }
}
