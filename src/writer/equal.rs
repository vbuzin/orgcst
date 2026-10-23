//! Test helpers that parse rendered text back and compare its typed fields,
//! through `with_node_text` / `key_eq` rather than spans or the allocating
//! `key()`.

use super::heading::HeadingSpec;
use super::property::NodePropertySpec;
use super::timestamp::TimestampSpec;
use crate::Document;
use crate::ast::{
    AstNode, CheckboxState, HasParts, Heading, HeadingPart, Headline, Item, Link, LinkShape,
    NodeProperty, Outline, PropertyDrawer, Timestamp, TimestampField, TimestampShape,
    unescape_link_path, with_node_text,
};
use crate::syntax::OrgSyntaxKind;
use crate::tests::nodes_of_kind;

pub(super) fn first_cast<T: AstNode>(doc: &Document, kind: OrgSyntaxKind) -> T {
    T::cast(
        nodes_of_kind(doc.root(), kind)
            .into_iter()
            .next()
            .expect("node"),
    )
    .expect("cast")
}

pub(super) fn assert_heading_eq(doc: &Document, spec: &HeadingSpec<'_>) {
    let h: Heading = first_cast(doc, OrgSyntaxKind::HEADING);
    assert_eq!(h.level(), spec.level);
    match (h.todo_keyword(), spec.todo) {
        (None, None) => {}
        (Some(kw), Some(want)) => with_node_text(kw.syntax(), |s| assert_eq!(s, want)),
        (got, want) => panic!("todo mismatch: present={} want={want:?}", got.is_some()),
    }
    assert_eq!(h.priority().map(|p| p.value()), spec.priority);
    assert_eq!(h.is_commented(), spec.commented);
    let title = h.part_content_range(HeadingPart::Title).expect("title");
    assert_eq!(doc.slice(title), spec.title);
    match (h.tags(), spec.tags) {
        (None, []) => {}
        (Some(tags), want) => {
            let got: Vec<_> = tags.iter().collect();
            assert_eq!(got.len(), want.len(), "tag count");
            for (g, w) in got.iter().zip(want.iter()) {
                assert_eq!(g.name(), *w);
            }
        }
        (got, want) => panic!("tags mismatch: present={} want={want:?}", got.is_some()),
    }
}

pub(super) fn assert_planning_scheduled(doc: &Document, ts: &str) {
    assert_planning_eq(doc, None, Some(ts), None);
}

pub(super) fn assert_planning_eq(
    doc: &Document,
    deadline: Option<&str>,
    scheduled: Option<&str>,
    closed: Option<&str>,
) {
    let o: Outline = first_cast(doc, OrgSyntaxKind::OUTLINE);
    let p = o.planning().expect("planning");
    match (p.deadline(), deadline) {
        (Some(ts), Some(want)) => with_node_text(ts.syntax(), |s| assert_eq!(s, want)),
        (None, None) => {}
        (g, w) => panic!("deadline mismatch present={} want={w:?}", g.is_some()),
    }
    match (p.scheduled(), scheduled) {
        (Some(ts), Some(want)) => with_node_text(ts.syntax(), |s| assert_eq!(s, want)),
        (None, None) => {}
        (g, w) => panic!("scheduled mismatch present={} want={w:?}", g.is_some()),
    }
    match (p.closed(), closed) {
        (Some(ts), Some(want)) => with_node_text(ts.syntax(), |s| assert_eq!(s, want)),
        (None, None) => {}
        (g, w) => panic!("closed mismatch present={} want={w:?}", g.is_some()),
    }
}

pub(super) fn assert_property_drawer_eq(doc: &Document, entries: &[NodePropertySpec<'_>]) {
    let drawer: PropertyDrawer = first_cast(doc, OrgSyntaxKind::PROPERTY_DRAWER);
    let props: Vec<NodeProperty> = drawer.properties().collect();
    assert_eq!(props.len(), entries.len());
    for (got, want) in props.iter().zip(entries.iter()) {
        assert!(got.key_eq(want.key), "key {}", want.key);
        assert_eq!(got.is_append(), want.append);
        match (got.value_node(), want.value) {
            (None, None) => {}
            (Some(n), Some(v)) => with_node_text(&n, |s| assert_eq!(s, v)),
            (g, w) => panic!("value mismatch present={} want={w:?}", g.is_some()),
        }
    }
}

// Diary is the odd shape; the other arm is every calendar stamp.
#[allow(clippy::single_match_else)]
pub(super) fn assert_timestamp_eq(doc: &Document, spec: &TimestampSpec<'_>) {
    let ts: Timestamp = first_cast(doc, OrgSyntaxKind::TIMESTAMP);
    assert_eq!(ts.shape(), spec.shape);
    match spec.shape {
        TimestampShape::Diary => {
            let sexp = ts.diary_sexp().expect("diary");
            let want = spec.diary_sexp.expect("spec diary");
            with_node_text(sexp.syntax(), |s| {
                assert_eq!(s, format!("({want})"));
            });
            assert!(ts.start().is_none());
            assert!(ts.end().is_none());
            assert!(ts.repeater().is_none());
            assert!(ts.delay().is_none());
            match spec.start {
                Some(start) => match (start.hour, start.minute) {
                    (Some(h), Some(m)) => {
                        let hr = ts.field_range(TimestampField::Hour).expect("diary hour");
                        let mn = ts
                            .field_range(TimestampField::Minute)
                            .expect("diary minute");
                        assert_eq!(doc.slice(hr).parse::<u8>().expect("hour digits"), h);
                        assert_eq!(doc.slice(mn).parse::<u8>().expect("minute digits"), m);
                    }
                    (None, None) => {
                        assert!(ts.field_range(TimestampField::Hour).is_none());
                        assert!(ts.field_range(TimestampField::Minute).is_none());
                    }
                    _ => panic!("diary clock must be paired"),
                },
                None => {
                    assert!(ts.field_range(TimestampField::Hour).is_none());
                    assert!(ts.field_range(TimestampField::Minute).is_none());
                }
            }
        }
        _ => {
            assert_eq!(ts.start(), spec.start);
            assert_eq!(ts.end(), spec.end);
            assert_eq!(ts.repeater(), spec.repeater);
            assert_eq!(ts.delay(), spec.delay);
        }
    }
}

pub(super) fn assert_link_eq(doc: &Document, shape: LinkShape, path: &str, desc: Option<&str>) {
    let link: Link = first_cast(doc, OrgSyntaxKind::LINK);
    assert_eq!(link.shape(), shape);
    with_node_text(&link.path_node(), |raw| {
        assert_eq!(unescape_link_path(raw).as_ref(), path);
    });
    match (link.description(), desc) {
        (None, None) => {}
        (Some(d), Some(want)) => with_node_text(d.syntax(), |s| assert_eq!(s, want)),
        (g, w) => panic!("desc mismatch present={} want={w:?}", g.is_some()),
    }
}

pub(super) fn assert_checkbox_eq(doc: &Document, state: CheckboxState) {
    let item: Item = first_cast(doc, OrgSyntaxKind::ITEM);
    assert_eq!(item.checkbox(), Some(state));
}

#[cfg(test)]
mod tests {
    use super::super::heading::render_heading;
    use super::super::planning::{PlanningKeywordSpec, render_planning_line};
    use super::super::property::render_property_drawer;
    use super::super::timestamp::render_timestamp;
    use super::{
        HeadingSpec, NodePropertySpec, TimestampSpec, assert_heading_eq, assert_planning_scheduled,
        assert_property_drawer_eq,
    };
    use crate::parse;
    use crate::parser::PriorityAtom;
    use crate::{ParseConfig, TimestampPoint, TimestampShape};

    #[test]
    fn kitchen_sink_heading_planning_drawer() {
        let cfg = ParseConfig::default();
        let heading = render_heading(
            HeadingSpec {
                level: 1,
                todo: Some("TODO"),
                priority: Some(PriorityAtom::Letter('A')),
                commented: false,
                title: "Title",
                tags: &["work"],
            },
            &cfg,
        )
        .unwrap();
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
        assert_eq!(ts, "<2020-01-01 Wed>");
        let planning =
            render_planning_line(&[(PlanningKeywordSpec::Scheduled, ts.clone())]).unwrap();
        let drawer = render_property_drawer(&[NodePropertySpec {
            key: "ID",
            value: Some("abc"),
            append: false,
        }])
        .unwrap();
        let src = format!("{heading}{planning}{drawer}");
        assert_eq!(
            src,
            "* TODO [#A] Title :work:\nSCHEDULED: <2020-01-01 Wed>\n:PROPERTIES:\n:ID: abc\n:END:\n"
        );
        let doc = parse(&src);
        assert_heading_eq(
            &doc,
            &HeadingSpec {
                level: 1,
                todo: Some("TODO"),
                priority: Some(PriorityAtom::Letter('A')),
                commented: false,
                title: "Title",
                tags: &["work"],
            },
        );
        assert_planning_scheduled(&doc, "<2020-01-01 Wed>");
        assert_property_drawer_eq(
            &doc,
            &[NodePropertySpec {
                key: "ID",
                value: Some("abc"),
                append: false,
            }],
        );
    }
}
