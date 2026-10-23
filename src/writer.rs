//! Canonical text for Org constructs that do not exist in the source yet.
//!
//! A snapshot serialises verbatim with [`crate::Document::write`], and
//! existing nodes are copied with [`crate::Document::slice`]; this module
//! only writes new text. Its `render_*` functions are called by operation
//! compile.

mod escape;
mod heading;
mod object;
mod planning;
mod property;
mod timestamp;

#[cfg(test)]
mod equal;

pub use escape::{is_valid_tag_name, quote_lesser_body, unquote_lesser_body};
pub use heading::{TitleProblem, title_is_writable, title_problem};
pub use property::is_valid_property_key;

pub(crate) use heading::{
    HeadingSpec, OccupiedHeadlinePrefix, render_comment, render_heading, render_priority,
    render_stars, render_tags, render_todo, title_problem_given,
};
pub(crate) use object::{
    invalid_link_description, render_checkbox, render_link, render_link_description_insert,
};
pub(crate) use planning::{PlanningKeywordSpec, render_planning_line, render_planning_slot};
pub(crate) use property::{NodePropertySpec, render_node_property, render_property_drawer};
pub(crate) use timestamp::{TimestampSpec, render_timestamp};

use crate::ast::{InsertPoint, Padding};

/// Failure of a fallible `render_*`, with one arm per way a renderer can
/// reject its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SynthesisError {
    InvalidLevel,
    InvalidTag(String),
    InvalidTitle(TitleProblem),
    InvalidPropertyKey(String),
    InvalidPropertyValue(String),
    InvalidPriority,
    InvalidLinkPath,
    InvalidLinkShape,
    InvalidTimestamp(TimestampProblem),
    EmptyPlanning,
    DuplicatePlanningSlot,
    InvalidTodo,
}

/// Why a timestamp spec cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimestampProblem {
    /// A point or range shape has no start.
    MissingStart,
    /// A range shape has no end.
    MissingEnd,
    /// A point shape's end falls on another day; use a range shape.
    EndDateMismatch,
    /// A point shape has an end with no time of day.
    EndWithoutTime,
    /// An hour without a minute or the reverse, `hour >= 24`, `minute >= 60`,
    /// or a point timestamp with an end time but no start time (a time range
    /// inside one stamp).
    InvalidTime,
    /// A repeater or habit interval of zero.
    ZeroRepeater,
    /// A delay of zero.
    ZeroDelay,
    /// A diary timestamp cannot carry a repeater or delay.
    DiaryHasRepeaterOrDelay,
    /// The diary sexp contains `>` or a line break.
    InvalidDiarySexp,
    /// The diary shape has no sexp.
    MissingDiarySexp,
    /// A sexp was given for a shape that is not a diary timestamp.
    UnexpectedDiary,
}

pub(crate) fn padding_str(p: Padding) -> &'static str {
    match p {
        Padding::None => "",
        Padding::Space => " ",
        Padding::Newline => "\n",
        Padding::BlankLine => "\n\n",
    }
}

pub(crate) fn wrap_insert(point: InsertPoint, body: &str) -> String {
    let mut s = String::with_capacity(
        padding_str(point.lead()).len() + body.len() + padding_str(point.trail()).len(),
    );
    s.push_str(padding_str(point.lead()));
    s.push_str(body);
    s.push_str(padding_str(point.trail()));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, HasParts, Heading, HeadingPart, Outline, Padding};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind};
    use crate::{TimestampPoint, TimestampShape};

    fn first_heading(src: &str) -> (crate::Document, Heading) {
        let doc = parse(src);
        let h = Heading::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
                .into_iter()
                .next()
                .expect("heading"),
        )
        .expect("cast");
        (doc, h)
    }

    fn first_outline(src: &str) -> (crate::Document, Outline) {
        let doc = parse(src);
        let o = Outline::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE)
                .into_iter()
                .next()
                .expect("outline"),
        )
        .expect("cast");
        (doc, o)
    }

    #[test]
    fn write_is_source_over_taxonomy() {
        for &(src, cfg_tag) in TAXONOMY {
            let doc = crate::parse_with(src, &cfg_tag.build());
            assert_eq!(doc.write(), src);
            assert!(std::ptr::eq(doc.write(), doc.source()));
        }
    }

    #[test]
    fn padding_str_maps_each_arm() {
        assert_eq!(padding_str(Padding::None), "");
        assert_eq!(padding_str(Padding::Space), " ");
        assert_eq!(padding_str(Padding::Newline), "\n");
        assert_eq!(padding_str(Padding::BlankLine), "\n\n");
    }

    #[test]
    fn wrap_insert_uses_eof_lead_on_a_file_without_final_newline() {
        let (doc, outline) = first_outline("* A");
        let ip = outline.insert_after();
        assert_eq!(ip.lead(), Padding::Newline);
        assert_eq!(ip.trail(), Padding::None);
        assert_eq!(wrap_insert(ip, "* New\n"), "\n* New\n");
        let spliced = format!("{}{}", doc.source(), wrap_insert(ip, "* New\n"));
        assert_eq!(spliced, "* A\n* New\n");
        assert!(
            !spliced.contains("* A**"),
            "eof_lead must insert a newline so stars do not glue"
        );
    }

    #[test]
    fn wrap_insert_tag_dest_is_space_when_the_title_has_content() {
        let (_, h) = first_heading("* Title\n");
        let ip = h.part_insert_point(HeadingPart::Tags).expect("absent tags");
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(wrap_insert(ip, ":a:"), " :a:");
    }

    #[test]
    fn wrap_insert_planning_dest_adds_no_blank_on_a_heading_with_body() {
        let (_, outline) = first_outline("* H\nbody\n");
        let ip = outline.planning_insert_point();
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(ip.trail(), Padding::None);
        let ts = render_timestamp(&TimestampSpec {
            shape: TimestampShape::Active,
            start: Some(TimestampPoint {
                year: 2020,
                month: 1,
                day: 1,
                hour: None,
                minute: None,
            }),
            end: None,
            repeater: None,
            delay: None,
            diary_sexp: None,
        })
        .unwrap();
        let line = render_planning_line(&[(PlanningKeywordSpec::Scheduled, ts)]).unwrap();
        assert_eq!(wrap_insert(ip, &line), line);
        assert!(!wrap_insert(ip, &line).starts_with('\n'));
    }

    #[test]
    fn wrap_insert_planning_dest_at_eof_uses_eof_lead() {
        let (_, outline) = first_outline("* H");
        let ip = outline.planning_insert_point();
        assert_eq!(ip.lead(), Padding::Newline);
        let line = "SCHEDULED: <2020-01-01 Wed>\n";
        assert_eq!(wrap_insert(ip, line), format!("\n{line}"));
    }

    #[test]
    fn wrap_insert_tag_dest_is_none_when_the_title_is_empty() {
        let (_, h) = first_heading("* \n");
        let ip = h.part_insert_point(HeadingPart::Tags).expect("absent tags");
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(wrap_insert(ip, ":a:"), ":a:");
    }
}
