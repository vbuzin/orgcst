//! Tests of the shared view machinery: `ast_node!`, casting, the part
//! contract, element and object walks.

use super::container_matrix::{for_each_corpus, parse_with_cfg};
use super::layout::{content_of, line_partition};
use super::support::{
    child_view, element_children, element_run_range, is_container_kind, next_element_sibling,
    prev_element_sibling,
};
use super::*;
use crate::parse;
use crate::syntax::OrgSyntaxKind;
use crate::tests::nodes_of_kind;
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

#[test]
fn split_span_keep_moved_and_rejects_outside() {
    let content = TextRange::new(TextSize::from(10), TextSize::from(20));
    let dest = InsertPoint::new(TextSize::from(20), Padding::None, Padding::None);

    let at_start = split_span(content, content.start(), dest).expect("start");
    assert!(at_start.keep().is_empty());
    assert_eq!(at_start.moved(), content);
    assert_eq!(at_start.dest(), dest);

    let inside = split_span(content, TextSize::from(15), dest).expect("inside");
    assert_eq!(
        inside.keep(),
        TextRange::new(TextSize::from(10), TextSize::from(15))
    );
    assert_eq!(
        inside.moved(),
        TextRange::new(TextSize::from(15), TextSize::from(20))
    );

    assert!(split_span(content, content.end(), dest).is_none());
    assert!(split_span(content, TextSize::from(9), dest).is_none());
    assert!(split_span(content, TextSize::from(21), dest).is_none());
    assert!(split_span(TextRange::empty(TextSize::from(5)), TextSize::from(5), dest).is_none());
}

// ---------------------------------------------------------------------------
// Macro + AstNode cast
// ---------------------------------------------------------------------------

crate::ast_node!(MacroSingle, OrgSyntaxKind::PARAGRAPH);
crate::ast_node!(
    MacroMulti,
    [OrgSyntaxKind::KEYWORD, OrgSyntaxKind::SETTING_KEYWORD]
);

#[test]
fn ast_node_single_kind_cast() {
    let doc = parse("hello\n");
    let para = nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
        .into_iter()
        .next()
        .expect("paragraph");
    let v = MacroSingle::cast(para.clone()).expect("cast");
    assert_eq!(v.kind(), OrgSyntaxKind::PARAGRAPH);
    assert_eq!(v.range(), para.text_range());
    assert_eq!(v.text().to_string(), para.text().to_string());
    assert!(MacroSingle::can_cast(OrgSyntaxKind::PARAGRAPH));
    assert!(!MacroSingle::can_cast(OrgSyntaxKind::HEADING));
}

#[test]
fn ast_node_multi_kind_cast() {
    let doc = parse("#+TITLE: x\n");
    let kw = nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
        .into_iter()
        .next()
        .expect("keyword");
    assert!(MacroMulti::cast(kw).is_some());

    let doc = parse("#+TODO: A | B\n");
    let sk = nodes_of_kind(doc.root(), OrgSyntaxKind::SETTING_KEYWORD)
        .into_iter()
        .next()
        .expect("setting");
    assert!(MacroMulti::cast(sk).is_some());

    let doc = parse("para\n");
    let para = nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
        .into_iter()
        .next()
        .expect("para");
    assert!(MacroMulti::cast(para).is_none());
}

#[test]
fn document_is_not_an_ast_node() {
    // Greppable gate: there is no ast_node!(Document, ...) and no cast arm.
    let doc = parse("* H\n");
    let root = doc.root().clone();
    assert_eq!(root.kind(), OrgSyntaxKind::DOCUMENT);
    assert!(!Element::can_cast(OrgSyntaxKind::DOCUMENT));
    assert!(!Object::can_cast(OrgSyntaxKind::DOCUMENT));
    assert!(!Container::can_cast(OrgSyntaxKind::DOCUMENT));
    assert!(Element::cast(root).is_none());
}

#[test]
fn thin_views_cast() {
    let doc = parse("* TODO [#A] Title :t:\n");
    let root = doc.root();
    assert!(
        TodoKeyword::cast(
            nodes_of_kind(root, OrgSyntaxKind::TODO_KEYWORD)
                .into_iter()
                .next()
                .unwrap()
        )
        .is_some()
    );
    assert!(
        Priority::cast(
            nodes_of_kind(root, OrgSyntaxKind::PRIORITY)
                .into_iter()
                .next()
                .unwrap()
        )
        .is_some()
    );
    assert!(
        HeadingTitle::cast(
            nodes_of_kind(root, OrgSyntaxKind::HEADING_TITLE)
                .into_iter()
                .next()
                .unwrap()
        )
        .is_some()
    );
    assert!(
        Tags::cast(
            nodes_of_kind(root, OrgSyntaxKind::TAGS)
                .into_iter()
                .next()
                .unwrap()
        )
        .is_some()
    );
}

#[test]
fn object_cast_positive_and_negative() {
    let doc = parse("*bold* and [[a][b]]\n");
    let bold = nodes_of_kind(doc.root(), OrgSyntaxKind::BOLD)
        .into_iter()
        .next()
        .expect("bold");
    assert!(matches!(Object::cast(bold), Some(Object::Markup(_))));
    let link = nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
        .into_iter()
        .next()
        .expect("link");
    assert!(matches!(Object::cast(link), Some(Object::Link(_))));
    let para = nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
        .into_iter()
        .next()
        .expect("para");
    assert!(Object::cast(para).is_none());
    assert!(!Object::can_cast(OrgSyntaxKind::PARAGRAPH));
}

#[test]
fn object_cast_agrees_with_object_kinds() {
    use crate::ast::support::object_kinds;
    let src = concat!(
        "*bold* /i/ _u_ +s+ =v= ~c~ \\alpha \\(x\\) @@html:x@@ ",
        "src_elisp{x} call_n() [[p]] <<t>> <<<r>>> {{{m}}} [1/2] ",
        "<2026-01-01> [cite:@k] a_{b} a^{c} hello\\\\\n",
        "see [fn:1]\n",
    );
    let doc = parse(src);
    for &kind in object_kinds() {
        assert!(Object::can_cast(kind), "{kind:?}");
        let node = nodes_of_kind(doc.root(), kind).into_iter().next();
        let Some(node) = node else {
            panic!("TAXONOMY-style fixture missed object kind {kind:?}");
        };
        assert!(
            Object::cast(node).is_some(),
            "Object::cast must succeed for {kind:?}"
        );
    }
    for kind in [
        OrgSyntaxKind::LINK_PATH,
        OrgSyntaxKind::LINK_DESCRIPTION,
        OrgSyntaxKind::DATE,
        OrgSyntaxKind::TIME,
        OrgSyntaxKind::TIME_RANGE,
        OrgSyntaxKind::ROD,
    ] {
        assert!(
            !Object::can_cast(kind),
            "part kind {kind:?} is not an Object arm"
        );
    }
}

#[test]
fn object_cast_diary_sexp_and_planning_timestamp() {
    let elem_src = "%%(diary-float t 4 2)\n";
    let elem_doc = parse(elem_src);
    let diary_el = nodes_of_kind(elem_doc.root(), OrgSyntaxKind::DIARY_SEXP)
        .into_iter()
        .next()
        .expect("diary element");
    assert!(DiarySexp::cast(diary_el.clone()).is_some());
    assert!(Element::cast(diary_el.clone()).is_some());
    assert!(Object::cast(diary_el).is_none());

    let stamp_src = "<%%(diary-float t 4 2)>\n";
    let stamp_doc = parse(stamp_src);
    let diary_stamp = nodes_of_kind(stamp_doc.root(), OrgSyntaxKind::DIARY_SEXP)
        .into_iter()
        .next()
        .expect("diary under timestamp");
    assert!(DiarySexp::cast(diary_stamp.clone()).is_some());
    assert!(Element::cast(diary_stamp.clone()).is_none());
    assert!(Object::cast(diary_stamp).is_none());

    let para = parse("<2026-01-01>\n");
    let ts = nodes_of_kind(para.root(), OrgSyntaxKind::TIMESTAMP)
        .into_iter()
        .next()
        .expect("ts");
    assert!(matches!(Object::cast(ts), Some(Object::Timestamp(_))));

    let plan = parse("* H\nDEADLINE: <2026-01-01>\n");
    let ts = nodes_of_kind(plan.root(), OrgSyntaxKind::TIMESTAMP)
        .into_iter()
        .next()
        .expect("planning ts");
    assert!(
        matches!(Object::cast(ts), Some(Object::Timestamp(_))),
        "planning timestamps are still timestamp objects"
    );
}

#[test]
fn diary_sexp_element_cast_dual_use() {
    // Element form: bare line diary is an element.
    let elem_src = "%%(diary-float t 4 2)\n";
    let elem_doc = parse(elem_src);
    let diary_el = nodes_of_kind(elem_doc.root(), OrgSyntaxKind::DIARY_SEXP)
        .into_iter()
        .next()
        .expect("diary element");
    assert!(Element::can_cast(OrgSyntaxKind::DIARY_SEXP));
    assert!(
        Element::cast(diary_el).is_some(),
        "bare diary line must cast as Element"
    );

    // Stamp child: under TIMESTAMP, same kind is not an element.
    let stamp_src = "<%%(diary-float t 4 2)>\n";
    let stamp_doc = parse(stamp_src);
    let diary_stamp = nodes_of_kind(stamp_doc.root(), OrgSyntaxKind::DIARY_SEXP)
        .into_iter()
        .next()
        .expect("diary under timestamp");
    assert!(
        diary_stamp
            .parent()
            .is_some_and(|p| p.kind() == OrgSyntaxKind::TIMESTAMP)
    );
    assert!(
        Element::cast(diary_stamp).is_none(),
        "diary under TIMESTAMP must not cast as Element"
    );
}

// ---------------------------------------------------------------------------
// Part harness self-test
// ---------------------------------------------------------------------------

#[test]
fn part_harness_on_heading_with_todo() {
    let doc = parse("* TODO Title :tag:\n");
    let h = Heading::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    check_part_contract(&h);

    assert!(matches!(
        h.part_span(HeadingPart::Todo),
        PartSpan::Present(_)
    ));
    let title_content = h.part_content_range(HeadingPart::Title).unwrap();
    assert_eq!(&doc.source()[title_content], "Title");

    let title_r = h.part_range(HeadingPart::Title).unwrap();
    let tags_r = h.part_range(HeadingPart::Tags).unwrap();
    assert_eq!(title_r.end(), tags_r.start());
    assert_eq!(h.part_at(title_r.end()), Some(HeadingPart::Title));
}

#[test]
fn part_harness_on_heading_absent_todo() {
    let doc = parse("* Title\n");
    let h = Heading::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    check_part_contract(&h);

    let ip = h.part_insert_point(HeadingPart::Todo).expect("absent");
    let title = child_view::<HeadingTitle>(h.syntax()).expect("title");
    assert_eq!(ip.offset(), title.range().start());
    assert_eq!(ip.lead(), Padding::None);
    assert_eq!(ip.trail(), Padding::Space);

    let tags_ip = h.part_insert_point(HeadingPart::Tags).expect("no tags");
    assert_eq!(
        tags_ip.offset(),
        h.part_content_range(HeadingPart::Title).unwrap().end()
    );
    assert_eq!(tags_ip.lead(), Padding::Space);
}

#[test]
fn part_harness_tags_insert_skips_lead_on_empty_title() {
    // Empty title: insert sits at the end of the post-stars space, so the
    // lead is None. Line-trailing space after a non-empty title is *after*
    // the title node (eat_to_line_end), so that case keeps lead Space.
    let doc = parse("* \n");
    let h = Heading::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    check_part_contract(&h);

    let title = child_view::<HeadingTitle>(h.syntax()).expect("title");
    assert!(content_of(title.syntax()).is_empty());
    let tags_ip = h.part_insert_point(HeadingPart::Tags).expect("no tags");
    assert_eq!(tags_ip.offset(), title.range().end());
    assert_eq!(tags_ip.lead(), Padding::None);

    let todo_ip = h.part_insert_point(HeadingPart::Todo).expect("absent todo");
    assert_eq!(todo_ip.offset(), title.range().start());
    assert_eq!(todo_ip.trail(), Padding::None);
}

#[test]
fn has_parts_provided_methods_agree() {
    let doc = parse("* TODO X\n");
    let h = Heading::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let spans: Vec<_> = h.part_spans().collect();
    assert_eq!(spans.len(), Heading::PARTS.len());
    for (part, span) in spans {
        match span {
            PartSpan::Present(p) => {
                assert_eq!(h.part_range(part), Some(p.range()));
                assert_eq!(h.part_content_range(part), Some(p.content()));
                assert_eq!(h.part_removal_range(part), Some(p.removal()));
                assert!(h.part_insert_point(part).is_none());
            }
            PartSpan::Absent(ip) => {
                assert!(h.part_range(part).is_none());
                assert_eq!(h.part_insert_point(part), Some(ip));
            }
            PartSpan::NotApplicable => {
                assert!(h.part_range(part).is_none());
                assert!(h.part_insert_point(part).is_none());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Element, container and object walks through the unions
// ---------------------------------------------------------------------------

#[test]
fn container_elements_skip_affiliated() {
    let doc = parse("#+CAPTION: c\nbody\n");
    let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
        .into_iter()
        .next()
        .unwrap();
    let c = Container::cast(section).expect("section is container");
    let els: Vec<_> = c.elements().collect();
    assert_eq!(els.len(), 1);
    assert_eq!(els[0].kind(), OrgSyntaxKind::PARAGRAPH);
}

#[test]
fn container_cast_excludes_verbatim_and_verse_blocks() {
    // A block's body mode is fixed by its kind. Only the four
    // container-mode kinds hold element children, so only they are containers:
    // otherwise `elements() == []` would mean both "empty" and "impossible".
    for (src, kind) in [
        (
            "#+begin_src elisp\n(+ 1)\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        ),
        (
            "#+begin_example\nx\n#+end_example\n",
            OrgSyntaxKind::EXAMPLE_BLOCK,
        ),
        (
            "#+begin_export html\n<p/>\n#+end_export\n",
            OrgSyntaxKind::EXPORT_BLOCK,
        ),
        (
            "#+begin_comment\nx\n#+end_comment\n",
            OrgSyntaxKind::COMMENT_BLOCK,
        ),
        (
            "#+begin_verse\nline\n#+end_verse\n",
            OrgSyntaxKind::VERSE_BLOCK,
        ),
    ] {
        let doc = parse(src);
        let node = nodes_of_kind(doc.root(), kind)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("missing {kind:?}"));
        assert!(
            !Container::can_cast(kind),
            "{kind:?} is not a container: its body is verbatim text or objects"
        );
        assert!(Container::cast(node.clone()).is_none(), "{kind:?}");
        // Still an element, still walkable as one - only the container role
        // is refused.
        assert!(
            Element::cast(node.clone()).is_some(),
            "{kind:?} is an element"
        );
        let els: Vec<_> = element_children(&node).collect();
        assert!(els.is_empty(), "{kind:?} has no element children: {els:?}");
    }
}

#[test]
fn container_cast_includes_container_mode_blocks() {
    for (src, kind) in [
        (
            "#+begin_quote\nq\n#+end_quote\n",
            OrgSyntaxKind::QUOTE_BLOCK,
        ),
        (
            "#+begin_center\nc\n#+end_center\n",
            OrgSyntaxKind::CENTER_BLOCK,
        ),
        ("#+begin_foo\nb\n#+end_foo\n", OrgSyntaxKind::SPECIAL_BLOCK),
        (
            "#+BEGIN: clocktable\n#+END:\n",
            OrgSyntaxKind::DYNAMIC_BLOCK,
        ),
    ] {
        let doc = parse(src);
        let node = nodes_of_kind(doc.root(), kind)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("missing {kind:?}"));
        assert!(
            Container::cast(node).is_some(),
            "{kind:?} is a container-mode block"
        );
    }
}

#[test]
fn no_element_has_a_non_container_parent() {
    // Every body element's parent is a container kind (or OUTLINE /
    // DOCUMENT), so the Container union covers every parent.
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            let Some(el) = Element::cast(node) else {
                continue;
            };
            let Some(parent) = el.syntax().parent() else {
                continue;
            };
            // OUTLINE and DOCUMENT hold structure, not container bodies.
            if matches!(
                parent.kind(),
                OrgSyntaxKind::OUTLINE | OrgSyntaxKind::DOCUMENT
            ) {
                continue;
            }
            assert!(
                is_container_kind(parent.kind()),
                "{:?} in {src:?} has parent {:?}, which is not a container",
                el.kind(),
                parent.kind()
            );
        }
    });
}

#[test]
fn property_drawer_under_inline_task_is_not_body_element() {
    // Closed inline task with properties + body. Default min level is 15.
    let src = "*************** TASK\n:PROPERTIES:\n:ID: x\n:END:\nbody\n*************** END\n";
    let doc = parse(src);
    let task = nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
        .into_iter()
        .next()
        .expect("inline task");
    let drawer = task
        .children()
        .find(|n| n.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
        .expect("fixture must carve PROPERTY_DRAWER under the task");

    // Walk, cast, and siblings share is_body_element.
    let kinds: Vec<_> = element_children(&task).map(|n| n.kind()).collect();
    assert!(
        !kinds.contains(&OrgSyntaxKind::PROPERTY_DRAWER),
        "metadata drawer must not be a body element: {kinds:?}"
    );
    assert!(
        kinds.contains(&OrgSyntaxKind::PARAGRAPH),
        "body paragraph still an element: {kinds:?}"
    );
    assert!(
        Element::cast(drawer.clone()).is_none(),
        "Element::cast must reject drawer under INLINE_TASK"
    );
    assert!(
        Element::can_cast(OrgSyntaxKind::PROPERTY_DRAWER),
        "can_cast stays kind-only"
    );

    let body = element_children(&task)
        .find(|n| n.kind() == OrgSyntaxKind::PARAGRAPH)
        .expect("body");
    assert!(
        prev_element_sibling(&body).is_none_or(|n| n.kind() != OrgSyntaxKind::PROPERTY_DRAWER),
        "prev_element from body must not surface the metadata drawer"
    );
    // Stronger: first body element's prev is None (drawer is only prior sibling
    // that could have been an element).
    assert_eq!(
        prev_element_sibling(&body).map(|n| n.kind()),
        None,
        "prev from first body element"
    );
    assert_eq!(
        next_element_sibling(&drawer).map(|n| n.kind()),
        Some(OrgSyntaxKind::PARAGRAPH),
        "next from drawer still reaches body (drawer is not itself an element)"
    );
}

#[test]
fn property_drawer_under_section_is_element() {
    let doc = parse(":PROPERTIES:\n:ID: x\n:END:\n");
    let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
        .into_iter()
        .next()
        .expect("section");
    let drawer = section
        .children()
        .find(|n| n.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
        .expect("drawer");
    let kinds: Vec<_> = element_children(&section).map(|n| n.kind()).collect();
    assert!(kinds.contains(&OrgSyntaxKind::PROPERTY_DRAWER));
    let el = Element::cast(drawer.clone()).expect("file-level drawer under SECTION");
    assert!(el.is_greater());
    assert!(
        Container::cast(drawer).is_none(),
        "PropertyDrawer is greater, not a Container"
    );
}

#[test]
fn property_drawer_under_outline_is_not_body_element() {
    let src = "* H\n:PROPERTIES:\n:ID: x\n:END:\nbody\n";
    let doc = parse(src);
    let outline = nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE)
        .into_iter()
        .next()
        .expect("outline");
    let drawer = outline
        .children()
        .find(|n| n.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
        .expect("metadata drawer under outline");
    assert!(Element::can_cast(OrgSyntaxKind::PROPERTY_DRAWER));
    assert!(
        Element::cast(drawer).is_none(),
        "drawer under OUTLINE is metadata, not Element"
    );
}

#[test]
fn element_view_defaults_on_paragraph() {
    let doc = parse("#+NAME: n\npara\n");
    let para = nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
        .into_iter()
        .next()
        .unwrap();
    let p = Paragraph::cast(para).unwrap();
    let run = p.affiliated().expect("affiliated run");
    // range() is the element proper (org `:post-affiliated`); full_range() is
    // org `:begin`..`:end`. The affiliated run being a separate sibling is
    // what makes the two differ.
    assert!(p.full_range().start() < p.range().start());
    assert_eq!(
        p.full_range(),
        TextRange::new(run.range().start(), p.range().end())
    );
    assert!(p.container().is_some());
    assert!(!Element::can_cast(OrgSyntaxKind::AFFILIATED_KEYWORDS));
}

#[test]
fn element_at_left_bias_at_seam_and_gaps() {
    // Keywords are adjacent elements with no inter-element blank ownership gap.
    let doc = parse("#+A: 1\n#+B: 2\n");
    let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
        .into_iter()
        .next()
        .unwrap();
    let c = Container::cast(section).unwrap();
    let els: Vec<_> = c.elements().collect();
    assert!(els.len() >= 2, "expected two keywords, got {els:?}");
    let a_r = els[0].range();
    let b_r = els[1].range();

    // Interior of first.
    let mid_a = a_r.start() + TextSize::from(1);
    assert_eq!(
        c.element_at(mid_a).map(|e| e.range()),
        Some(a_r),
        "interior of first"
    );

    // Exclusive end of last still hits last (tiled elements).
    assert_eq!(
        c.element_at(b_r.end()).map(|e| e.range()),
        Some(b_r),
        "end of last"
    );

    // Start of first.
    assert_eq!(
        c.element_at(a_r.start()).map(|e| e.range()),
        Some(a_r),
        "start of first"
    );

    // Hard pin: adjacent seam prefers the earlier element.
    assert_eq!(
        a_r.end(),
        b_r.start(),
        "keyword fixture must be range-adjacent: a={a_r:?} b={b_r:?}"
    );
    assert_eq!(
        c.element_at(a_r.end()).map(|e| e.range()),
        Some(a_r),
        "adjacent seam left-bias"
    );

    assert_eq!(c.first_element().map(|e| e.range()), Some(a_r));
    assert_eq!(c.last_element().map(|e| e.range()), Some(b_r));
}

#[test]
fn element_at_gap_between_paragraphs_hits_prior_post_blank() {
    // Blank-separated paragraphs: first range typically owns the blank as
    // post-blank, so a.end() may equal b.start() or leave a gap owned by a.
    let doc = parse("one\n\ntwo\n");
    let section = nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
        .into_iter()
        .next()
        .unwrap();
    let c = Container::cast(section).unwrap();
    let els: Vec<_> = c.elements().collect();
    assert_eq!(els.len(), 2);
    let a_r = els[0].range();
    let b_r = els[1].range();
    if a_r.end() == b_r.start() {
        assert_eq!(
            c.element_at(a_r.end()).map(|e| e.range()),
            Some(a_r),
            "seam left-bias"
        );
    } else {
        // Gap exclusive end of a still hits a under tiled left-bias (post-blank).
        assert_eq!(
            c.element_at(a_r.end()).map(|e| e.range()),
            Some(a_r),
            "post-blank end of first paragraph"
        );
        // Offset strictly between ranges (if any) is not in either half-open
        // interior; left-bias at a's end already covered. At b.start hits b.
        assert_eq!(
            c.element_at(b_r.start()).map(|e| e.range()),
            Some(b_r),
            "start of second"
        );
    }
}

#[test]
fn object_at_left_bias() {
    let doc = parse("* *bold* plain\n");
    let title = nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING_TITLE)
        .into_iter()
        .next()
        .unwrap();
    let title_view = HeadingTitle::cast(title).unwrap();
    let objs: Vec<_> = title_view.objects().collect();
    assert_eq!(objs.len(), 1, "only bold is an object");
    let bold = &objs[0];
    let br = bold.range();
    assert_eq!(
        title_view.object_at(br.start()).map(|o| o.range()),
        Some(br)
    );
    assert_eq!(
        title_view
            .object_at(br.start() + TextSize::from(1))
            .map(|o| o.range()),
        Some(br)
    );
    // End of bold: tiled left-bias still hits bold (no following object).
    assert_eq!(title_view.object_at(br.end()).map(|o| o.range()), Some(br));
    // Plain text after bold is not an object - offset strictly after exclusive
    // end with no following object is None under left-bias (only ending hit
    // bold at br.end(); br.end()+1 is neither start nor end of any object).
    let after = br.end() + TextSize::from(1);
    if after < title_view.range().end() {
        assert_eq!(
            title_view.object_at(after).map(|o| o.range()),
            None,
            "plain text after bold is not an object"
        );
    }
}

// ---------------------------------------------------------------------------
// Corpus properties over the mechanism
// ---------------------------------------------------------------------------

#[test]
fn line_partition_reassembles_every_node_in_the_corpus() {
    // The partition is a split, never a filter: first + body + last is the
    // node's children in order. A mis-anchored last line still satisfies
    // "non-empty".
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            let p = line_partition(&node);
            let joined: String = p
                .first_line
                .iter()
                .chain(&p.body)
                .chain(&p.last_line)
                .map(std::string::ToString::to_string)
                .collect();
            assert_eq!(
                joined,
                node.text().to_string(),
                "{:?} in {src:?} does not reassemble",
                node.kind()
            );
            // A single-line node has no distinct last line; a multi-line one
            // never ends its last line before the node ends.
            if !p.last_line.is_empty() {
                let last_end = p.last_line.last().expect("non-empty").text_range().end();
                assert_eq!(
                    last_end,
                    node.text_range().end(),
                    "{:?} in {src:?}: last_line must reach the node end",
                    node.kind()
                );
            }
        }
    });
}

#[test]
fn cast_success_implies_can_cast_across_the_corpus() {
    // The one direction generic code may rely on (see `AstNode::cast`). The
    // converse deliberately does not hold for dual-use kinds.
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            let kind = node.kind();
            if Element::cast(node.clone()).is_some() {
                assert!(Element::can_cast(kind), "Element {kind:?} in {src:?}");
            }
            if Object::cast(node.clone()).is_some() {
                assert!(Object::can_cast(kind), "Object {kind:?} in {src:?}");
            }
            if Container::cast(node.clone()).is_some() {
                assert!(Container::can_cast(kind), "Container {kind:?} in {src:?}");
            }
        }
    });
}

#[test]
fn last_element_agrees_with_the_elements_walk() {
    // `last_element` must go through `elements()`, so an implementor that
    // narrows the walk cannot get a last element its own iterator never yields.
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            let Some(c) = Container::cast(node) else {
                continue;
            };
            assert_eq!(
                c.last_element().map(|e| e.range()),
                c.elements().last().map(|e| e.range()),
                "container {:?} in {src:?}",
                c.kind()
            );
        }
    });
}

#[test]
fn geometry_constructors_are_crate_internal_usable() {
    let ip = InsertPoint::new(TextSize::from(0), Padding::Newline, Padding::None);
    assert_eq!(ip.offset(), TextSize::from(0));
    assert_eq!(ip.lead(), Padding::Newline);
    let p = PresentPart::identical(TextRange::new(TextSize::from(1), TextSize::from(3)));
    assert_eq!(p.range(), p.content());
    assert_eq!(p.removal(), p.range());
}

#[test]
fn nested_containers_walk_children_not_descendants() {
    let src = "#+begin_quote\n:NOTES:\ninside\n:END:\n#+end_quote\n";
    let doc = parse(src);
    let section = Section::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::SECTION)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let section_els: Vec<_> = section.elements().collect();
    assert_eq!(section_els.len(), 1);
    assert!(matches!(section_els[0], Element::Block(_)));

    let quote = match &section_els[0] {
        Element::Block(b) => b.clone(),
        _ => panic!("quote"),
    };
    let c = Container::cast(quote.syntax().clone()).unwrap();
    let kids: Vec<_> = c.elements().collect();
    assert_eq!(kids.len(), 1);
    assert!(matches!(kids[0], Element::Drawer(_)));
    assert!(
        !c.elements().any(|e| e.kind() == OrgSyntaxKind::PARAGRAPH),
        "inner paragraph is a child of the drawer, not the quote"
    );
    assert!(
        doc.elements().any(|e| e.kind() == OrgSyntaxKind::PARAGRAPH),
        "document descendants walk includes the inner paragraph"
    );
    assert!(
        !section
            .elements()
            .any(|e| e.kind() == OrgSyntaxKind::PARAGRAPH),
        "section walk is direct children only"
    );
}

#[test]
fn affiliated_run_is_not_an_element_insert_before_includes_caption() {
    let src = "#+CAPTION: c\npara\n";
    let doc = parse(src);
    let els: Vec<_> = doc.elements().collect();
    assert_eq!(els.len(), 1);
    assert!(matches!(els[0], Element::Paragraph(_)));
    let run = els[0].affiliated().expect("run");
    let member = run.syntax().children().next().expect("caption keyword");
    assert!(Element::can_cast(member.kind()));
    assert!(Element::cast(member).is_none());
    assert!(els[0].affiliated().is_some());
    assert!(els[0].full_range().start() < els[0].range().start());
    assert_eq!(els[0].insert_before().offset(), els[0].full_range().start());
    assert_eq!(
        usize::from(els[0].insert_before().offset()),
        0,
        "insert before lands before the caption"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // one absorb-pair case table
fn separation_after_absorb_pairs() {
    let para = Paragraph::cast(
        nodes_of_kind(parse("hello\n").root(), OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let el = Element::from(&para);
    assert_eq!(
        separation_after(&el, OrgSyntaxKind::PARAGRAPH),
        Padding::BlankLine
    );

    for src in ["*bold*\n", "hello *bold*\n"] {
        let p = Paragraph::cast(
            nodes_of_kind(parse(src).root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            separation_after(&Element::from(&p), OrgSyntaxKind::PARAGRAPH),
            Padding::BlankLine,
            "object then line terminator is not a trailing blank: {src:?}"
        );
    }
    for src in ["*bold*\n\n", "hello *bold*\n\n"] {
        let p = Paragraph::cast(
            nodes_of_kind(parse(src).root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            separation_after(&Element::from(&p), OrgSyntaxKind::PARAGRAPH),
            Padding::None,
            "object then owned blank: {src:?}"
        );
    }
    assert_eq!(separation_after(&el, OrgSyntaxKind::HEADING), Padding::None);
    assert_eq!(
        separation_after(&el, OrgSyntaxKind::PLAIN_LIST),
        Padding::None
    );

    let para_blank = Paragraph::cast(
        nodes_of_kind(parse("hello\n\n").root(), OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&para_blank), OrgSyntaxKind::PARAGRAPH),
        Padding::None
    );

    let comment = Comment::cast(
        nodes_of_kind(parse("# a\n").root(), OrgSyntaxKind::COMMENT)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&comment), OrgSyntaxKind::COMMENT),
        Padding::BlankLine
    );
    let comment_blank = Comment::cast(
        nodes_of_kind(parse("# a\n\n").root(), OrgSyntaxKind::COMMENT)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&comment_blank), OrgSyntaxKind::COMMENT),
        Padding::None
    );

    let fw = FixedWidth::cast(
        nodes_of_kind(parse(": x\n").root(), OrgSyntaxKind::FIXED_WIDTH)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&fw), OrgSyntaxKind::FIXED_WIDTH),
        Padding::BlankLine
    );
    let fw_blank = FixedWidth::cast(
        nodes_of_kind(parse(": x\n\n").root(), OrgSyntaxKind::FIXED_WIDTH)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&fw_blank), OrgSyntaxKind::FIXED_WIDTH),
        Padding::None
    );

    let hr = HorizontalRule::cast(
        nodes_of_kind(parse("-----\n").root(), OrgSyntaxKind::HORIZONTAL_RULE)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&hr), OrgSyntaxKind::HORIZONTAL_RULE),
        Padding::None
    );

    let list = PlainList::cast(
        nodes_of_kind(parse("- a\n").root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&list), OrgSyntaxKind::PLAIN_LIST),
        Padding::BlankLine
    );

    let list_blank = PlainList::cast(
        nodes_of_kind(parse("- a\n\n").root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&list_blank), OrgSyntaxKind::PLAIN_LIST),
        Padding::Newline,
        "one list-final blank still needs a second newline"
    );

    let list_two = PlainList::cast(
        nodes_of_kind(parse("- a\n\n\n- b\n").root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        separation_after(&Element::from(&list_two), OrgSyntaxKind::PLAIN_LIST),
        Padding::None
    );
}

#[test]
fn list_after_list_round_trips_as_two_lists() {
    use crate::tests::written;

    let src = "- a\n";
    let doc = parse(src);
    let list = PlainList::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let ip = Element::from(&list).insert_after(OrgSyntaxKind::PLAIN_LIST);
    assert_eq!(ip.lead(), Padding::BlankLine);
    let out = written(src, ip, "- b\n");
    assert_eq!(out, "- a\n\n\n- b\n");
    let again = parse(&out);
    assert_eq!(
        again
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::PLAIN_LIST)
            .count(),
        2,
        "BlankLine after a terminated list is two consecutive blanks"
    );

    let src = "- a\n\n";
    let doc = parse(src);
    let list = PlainList::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let ip = Element::from(&list).insert_after(OrgSyntaxKind::PLAIN_LIST);
    assert_eq!(ip.lead(), Padding::Newline);
    let out = written(src, ip, "- b\n");
    let again = parse(&out);
    assert_eq!(
        again
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::PLAIN_LIST)
            .count(),
        2,
        "one owned list-final blank plus Newline splits"
    );

    let src = "- a";
    let doc = parse(src);
    let list = PlainList::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::PLAIN_LIST)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let ip = Element::from(&list).insert_after(OrgSyntaxKind::PLAIN_LIST);
    assert_eq!(ip.lead(), Padding::BlankLine);
    let out = written(src, ip, "- b\n");
    assert_eq!(out, "- a\n\n- b\n");
    let again = parse(&out);
    assert_eq!(
        again
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::PLAIN_LIST)
            .count(),
        1,
        "unterminated EOF list + BlankLine is still one list"
    );
}

#[test]
fn empty_elements_in_and_items_in_yield_nothing() {
    let src = "#+CAPTION: c\npara\n- item\n";
    let doc = parse(src);
    let para = doc
        .elements()
        .find(|e| matches!(e, Element::Paragraph(_)))
        .unwrap();
    let caption = para.affiliated().unwrap();
    let caret = TextRange::empty(caption.range().start());
    assert_eq!(doc.elements_in(caret).count(), 0);
    assert_eq!(doc.items_in(caret).count(), 0);

    let caption_only: Vec<_> = doc.elements_in(caption.range()).collect();
    assert!(
        caption_only.iter().any(|e| e.syntax() == para.syntax()),
        "non-empty range that is only the affiliated run still hits the target"
    );
    let caption_kw = caption.syntax().children().next().unwrap();
    assert!(Element::cast(caption_kw).is_none());

    let item = doc.items_in(doc.root().text_range()).next().expect("item");
    let interior = TextRange::empty(item.range().start() + TextSize::from(1));
    assert_eq!(doc.items_in(interior).count(), 0);
    assert_eq!(doc.items_in(item.range()).count(), 1);
}

#[test]
fn positional_elements_hides_tblfm_run_member_and_metadata_drawer() {
    let doc = parse("| a |\n#+TBLFM: $1=$1\n#+TITLE: t\n");
    let kinds: Vec<_> = doc.elements().map(|e| e.kind()).collect();
    assert!(kinds.contains(&OrgSyntaxKind::TABLE));
    assert!(kinds.contains(&OrgSyntaxKind::KEYWORD));
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == OrgSyntaxKind::KEYWORD)
            .count(),
        1,
        "TBLFM is not an element; TITLE is"
    );

    let doc = parse("#+CAPTION: c\npara\n");
    assert_eq!(doc.elements().count(), 1);

    let doc = parse(":PROPERTIES:\n:ID: x\n:END:\n");
    assert!(
        doc.elements()
            .any(|e| e.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
    );

    let doc = parse("* H\n:PROPERTIES:\n:ID: x\n:END:\nbody\n");
    assert!(
        !doc.elements()
            .any(|e| e.kind() == OrgSyntaxKind::PROPERTY_DRAWER)
    );

    let doc = parse("%%(diary-float t 4 2)\n");
    assert!(
        doc.elements()
            .any(|e| e.kind() == OrgSyntaxKind::DIARY_SEXP)
    );
    let doc = parse("<%%(diary-float t 4 2)>\n");
    assert!(
        !doc.elements()
            .any(|e| e.kind() == OrgSyntaxKind::DIARY_SEXP)
    );
}

#[test]
fn empty_container_dests_collapse() {
    use crate::ast::layout::first_line_end;

    let src = "#+begin_quote\n#+end_quote\n";
    let doc = parse(src);
    let quote = Container::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::QUOTE_BLOCK)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(quote.content_range().is_empty());
    let dest = quote.first_element_insert_point().offset();
    assert_eq!(dest, quote.last_element_insert_point().offset());
    assert_eq!(dest, quote.content_range().start());
    assert_eq!(dest, first_line_end(quote.syntax()));
    assert_eq!(&doc.source()[..usize::from(dest)], "#+begin_quote\n");
    assert!(quote.first_element().is_none());

    let src = ":NOTES:\n:END:\n";
    let doc = parse(src);
    let drawer = Container::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(drawer.content_range().is_empty());
    let dest = drawer.first_element_insert_point().offset();
    assert_eq!(dest, drawer.last_element_insert_point().offset());
    assert_eq!(dest, drawer.content_range().start());
    assert_eq!(dest, first_line_end(drawer.syntax()));
    assert_eq!(&doc.source()[..usize::from(dest)], ":NOTES:\n");

    let src = "[fn:1]\n";
    let doc = parse(src);
    let fn_def = Container::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::FOOTNOTE_DEFINITION)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(fn_def.content_range().is_empty());
    let dest = fn_def.first_element_insert_point().offset();
    assert_eq!(dest, fn_def.last_element_insert_point().offset());
    assert_eq!(dest, fn_def.content_range().start());
    assert_eq!(dest, first_line_end(fn_def.syntax()));
}

#[test]
fn container_content_range_matches_arm_interior() {
    let src = ":NOTES:\nitem\n:END:\n";
    let doc = parse(src);
    let node = nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER)
        .into_iter()
        .next()
        .unwrap();
    let drawer = Drawer::cast(node.clone()).unwrap();
    let c = Container::cast(node).unwrap();
    assert!(!c.content_range().is_empty());
    assert_eq!(c.content_range(), drawer.body_range());

    let src = "#+begin_quote\ninner\n#+end_quote\n";
    let doc = parse(src);
    let node = nodes_of_kind(doc.root(), OrgSyntaxKind::QUOTE_BLOCK)
        .into_iter()
        .next()
        .unwrap();
    let quote = Block::cast(node.clone()).unwrap();
    let c = Container::cast(node).unwrap();
    let run = element_run_range(quote.syntax()).expect("inner paragraph");
    assert!(!c.content_range().is_empty());
    assert_eq!(c.content_range(), run);
    assert_eq!(c.content_range(), quote.container_body_range());
    assert_eq!(ElementView::content_range(&quote), Some(c.content_range()));
}

#[test]
fn merge_lead_prefers_blank_line_over_eof_newline() {
    assert_eq!(
        merge_lead(Padding::Newline, Padding::BlankLine),
        Padding::BlankLine
    );
    assert_eq!(merge_lead(Padding::None, Padding::Space), Padding::Space);
    assert_eq!(merge_lead(Padding::Space, Padding::None), Padding::Space);

    let doc = parse("hello");
    let para = Paragraph::cast(
        nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .next()
            .unwrap(),
    )
    .unwrap();
    let ip = Element::from(&para).insert_after(OrgSyntaxKind::PARAGRAPH);
    assert_eq!(ip.lead(), Padding::BlankLine);

    let heading = Element::from(&para).insert_after(OrgSyntaxKind::HEADING);
    assert_eq!(
        heading.lead(),
        Padding::Newline,
        "eof_lead Newline is visible when absorption does not apply"
    );
}

#[test]
fn keyword_under_table_can_cast_true_cast_none() {
    let doc = parse("| a |\n#+TBLFM: $1=$1\n");
    let kw = nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
        .into_iter()
        .next()
        .unwrap();
    assert!(Element::can_cast(kw.kind()));
    assert!(Element::cast(kw).is_none());
}
