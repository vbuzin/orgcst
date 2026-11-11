//! Outline ops that write text of their own: split a heading, insert Org
//! text as subtrees, reorder an outline's children, turn a heading into
//! text.

use super::SubtreeDest;
use super::empty_edit_set;
use super::locate::{resolve_outline, resolve_subtree_dest};
use super::map_synthesis;
use super::structure::{pad_from_dest, shifted_level};
use crate::ast::{
    AstNode, HasParts, Heading, HeadingPart, Headline, InsertPoint, Outline, token_owning_byte,
};
use crate::edit::{Edit, EditSet};
use crate::syntax::OrgSyntaxKind;
use crate::writer::{
    HeadingSpec, OccupiedHeadlinePrefix, render_heading, render_stars, title_problem_given,
    wrap_insert,
};
use crate::{Document, NodeRef, OperationError, ParseConfig, TextRange, TextSize, parse_with};

/// The title's text without the space around it.
fn title_content(heading: &Heading) -> TextRange {
    heading
        .part_content_range(HeadingPart::Title)
        .unwrap_or_else(|| TextRange::empty(heading.title().range().start()))
}

// ---------------------------------------------------------------------------
// Split
// ---------------------------------------------------------------------------

pub(super) fn compile_split_heading(
    doc: &Document,
    target: NodeRef,
    at: TextSize,
) -> Result<EditSet, OperationError> {
    let outline = resolve_outline(doc, target)?;
    let heading = outline.heading();
    let content = title_content(&heading);
    if at < content.start() || at > content.end() {
        return Err(OperationError::InvalidOffset);
    }
    let keep = doc
        .slice(TextRange::new(content.start(), at))
        .trim_end_matches([' ', '\t']);
    let moved = doc
        .slice(TextRange::new(at, content.end()))
        .trim_start_matches([' ', '\t']);
    let mut edits = Vec::with_capacity(2);
    let cut = TextRange::new(content.start() + TextSize::of(keep), content.end());
    if !cut.is_empty() {
        let occupied = OccupiedHeadlinePrefix {
            todo: heading.todo_keyword().is_some(),
            priority: heading.priority().is_some(),
            commented: heading.is_commented(),
        };
        if !keep.is_empty()
            && let Some(problem) = title_problem_given(keep, doc.config(), occupied)
        {
            return Err(OperationError::InvalidTitle(problem));
        }
        edits.push(Edit {
            range: cut,
            replacement: String::new(),
        });
    }
    let rendered = render_heading(
        HeadingSpec {
            level: outline.level(),
            todo: None,
            priority: None,
            commented: false,
            title: moved,
            tags: &[],
        },
        doc.config(),
    )
    .map_err(map_synthesis)?;
    let point = outline.insert_after();
    edits.push(Edit {
        range: TextRange::empty(point.offset()),
        replacement: wrap_insert(point, &rendered),
    });
    Ok(EditSet::new(doc.source(), edits)?)
}

// ---------------------------------------------------------------------------
// Insert Org text
// ---------------------------------------------------------------------------

/// `text` with every heading shifted by the same number of levels, so that
/// the shallowest one is at `level`. Text before the first heading, and text
/// with no heading, is returned as it is.
///
/// The text is read under `config`, the configuration of the document it is
/// meant for, so its TODO keywords and inline-task threshold are that
/// document's.
///
/// # Errors
///
/// [`OperationError::InvalidLevel`] when `level` is 0;
/// [`OperationError::WouldBreakInlineTask`] when a shifted heading would
/// read as an inline task.
pub fn relevel_text(
    text: &str,
    config: &ParseConfig,
    level: usize,
) -> Result<String, OperationError> {
    if level == 0 {
        return Err(OperationError::InvalidLevel);
    }
    let parsed = parse_with(text, config);
    relevel_parsed(&parsed, level)
}

fn relevel_parsed(parsed: &Document, level: usize) -> Result<String, OperationError> {
    let text = parsed.source();
    let Some(shallowest) = parsed.root_outlines().map(|o| o.level()).min() else {
        return Ok(text.to_owned());
    };
    if shallowest == level {
        return Ok(text.to_owned());
    }
    let by = shallowest.abs_diff(level);
    let promote = level < shallowest;
    let inline_min = parsed.config().inline_task_min_level() as usize;
    let mut edits = Vec::new();
    for heading in parsed.map::<Heading>() {
        let new_level = shifted_level(heading.level(), by, promote, inline_min)?;
        edits.push(Edit {
            range: heading.stars_range(),
            replacement: render_stars(new_level),
        });
    }
    Ok(EditSet::new(text, edits)?.apply_to_text(text))
}

pub(super) fn compile_insert_subtree(
    doc: &Document,
    at: SubtreeDest,
    text: &str,
) -> Result<EditSet, OperationError> {
    let dest = resolve_subtree_dest(doc, at)?;
    let parsed = parse_with(text, doc.config());
    let first = parsed
        .root_outlines()
        .next()
        .ok_or(OperationError::InvalidSubtree)?;
    let start = first.range().start();
    if !parsed
        .slice(TextRange::new(TextSize::from(0), start))
        .trim()
        .is_empty()
    {
        return Err(OperationError::InvalidSubtree);
    }
    let subtrees = parse_with(&text[usize::from(start)..], doc.config());
    let mut payload = relevel_parsed(&subtrees, dest.level)?;
    if !payload.ends_with('\n') {
        payload.push('\n');
    }
    let (lead, trail) = pad_from_dest(doc.root(), dest.point, &payload);
    let point = InsertPoint::new(dest.point.offset(), lead, trail);
    Ok(EditSet::single(
        doc.source(),
        TextRange::empty(point.offset()),
        wrap_insert(point, &payload),
    )?)
}

// ---------------------------------------------------------------------------
// Reorder children
// ---------------------------------------------------------------------------

pub(super) fn compile_reorder_children(
    doc: &Document,
    target: NodeRef,
    order: &[usize],
) -> Result<EditSet, OperationError> {
    let outline = resolve_outline(doc, target)?;
    let children: Vec<Outline> = outline.children().collect();
    let n = children.len();
    let mut seen = vec![false; n];
    if order.len() != n {
        return Err(OperationError::InvalidOrder);
    }
    for &i in order {
        if i >= n || std::mem::replace(&mut seen[i], true) {
            return Err(OperationError::InvalidOrder);
        }
    }
    if order.iter().enumerate().all(|(at, &i)| at == i) {
        return Ok(empty_edit_set(doc.source()));
    }
    // A child's piece runs to the next child's start, so nothing between
    // two children is lost or left behind.
    let span = TextRange::new(children[0].range().start(), children[n - 1].range().end());
    let pieces: Vec<&str> = (0..n)
        .map(|i| {
            let end = children
                .get(i + 1)
                .map_or(span.end(), |next| next.range().start());
            doc.slice(TextRange::new(children[i].range().start(), end))
        })
        .collect();
    let mut joined = String::with_capacity(usize::from(span.len()) + 1);
    for &i in order {
        joined.push_str(pieces[i]);
        if !joined.ends_with('\n') {
            joined.push('\n');
        }
    }
    if !doc.slice(span).ends_with('\n') {
        joined.pop();
    }
    Ok(EditSet::single(doc.source(), span, joined)?)
}

// ---------------------------------------------------------------------------
// Convert to text
// ---------------------------------------------------------------------------

/// The end of the heading's line and of the metadata Org reads only under
/// it: the planning line and the property drawer.
fn metadata_end(outline: &Outline) -> TextSize {
    outline
        .property_drawer()
        .map(|p| p.range().end())
        .or_else(|| outline.planning().map(|p| p.range().end()))
        .unwrap_or_else(|| outline.heading().range().end())
}

/// Whether the line before `start` is text a paragraph written at `start`
/// would join: not blank, and not a headline or its metadata.
fn joins_text_before(doc: &Document, start: TextSize) -> bool {
    let Some(newline) = start.checked_sub(TextSize::from(1)) else {
        return false;
    };
    let line_start = doc.source()[..usize::from(newline)]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    if doc.source()[line_start..usize::from(newline)]
        .trim()
        .is_empty()
    {
        return false;
    }
    !token_owning_byte(doc.root(), newline).is_some_and(|token| {
        token.parent_ancestors().any(|n| {
            matches!(
                n.kind(),
                OrgSyntaxKind::HEADING | OrgSyntaxKind::PLANNING | OrgSyntaxKind::PROPERTY_DRAWER
            )
        })
    })
}

pub(super) fn compile_convert_to_text(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let outline = resolve_outline(doc, target)?;
    let heading = outline.heading();
    let start = heading.range().start();
    let meta = TextRange::new(start, metadata_end(&outline));
    let title = doc.slice(title_content(&heading));
    let before = joins_text_before(doc, start);
    let after = outline
        .section()
        .is_some_and(|s| s.content_range().start() == meta.end());
    let newline = if doc.slice(meta).ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let mut lead = 0u32;
    let replacement = if title.is_empty() {
        if before && after {
            "\n".to_owned()
        } else {
            String::new()
        }
    } else {
        let mut text = String::new();
        if before {
            text.push('\n');
            lead = 1;
        }
        text.push_str(title);
        text.push_str(newline);
        if after {
            text.push('\n');
        }
        text
    };
    let mut edits = vec![Edit {
        range: meta,
        replacement,
    }];
    for child in outline.descendants() {
        let heading = child.heading();
        edits.push(Edit {
            range: heading.stars_range(),
            replacement: render_stars(heading.level() - 1),
        });
    }
    let set = EditSet::new(doc.source(), edits)?;
    if !title.is_empty() {
        let written = set.apply_to_text(doc.source());
        let reread = parse_with(&written, doc.config());
        let at = start + TextSize::from(lead);
        if !reads_as_paragraph(&reread, at, title) {
            return Err(OperationError::InvalidText);
        }
    }
    Ok(set)
}

/// Whether `doc` has a paragraph starting at `at` whose text is `title`.
fn reads_as_paragraph(doc: &Document, at: TextSize, title: &str) -> bool {
    token_owning_byte(doc.root(), at)
        .and_then(|token| {
            token
                .parent_ancestors()
                .find(|n| n.kind() == OrgSyntaxKind::PARAGRAPH)
        })
        .is_some_and(|p| {
            p.text_range().start() == at && doc.slice(p.text_range()).trim_end() == title
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::compile_write;
    use crate::{Operation, compile, parse, parse_with};

    fn outline_named(doc: &Document, title: &str) -> Outline {
        doc.map::<Outline>()
            .find(|o| doc.slice(title_content(&o.heading())) == title)
            .unwrap_or_else(|| panic!("no heading {title:?}"))
    }

    fn at(doc: &Document, title: &str) -> NodeRef {
        NodeRef::of(&outline_named(doc, title))
    }

    fn write(src: &str, op: impl FnOnce(&Document) -> Operation) -> String {
        let doc = parse(src);
        let op = op(&doc);
        let (_, text) = compile_write(&doc, &op);
        text
    }

    fn fails(src: &str, op: impl FnOnce(&Document) -> Operation) -> OperationError {
        let doc = parse(src);
        compile(&doc, &op(&doc)).expect_err("refused")
    }

    fn offset_of(src: &str, needle: &str) -> TextSize {
        TextSize::try_from(src.find(needle).expect("needle")).expect("fits")
    }

    // -- split ---------------------------------------------------------------

    #[test]
    fn split_moves_the_rest_after_the_subtree() {
        let src = "* Buy paint and brushes :home:\nText.\n** Child\n* Next\n";
        let out = write(src, |doc| Operation::SplitHeading {
            target: at(doc, "Buy paint and brushes"),
            at: offset_of(src, "brushes"),
        });
        assert_eq!(
            out,
            "* Buy paint and :home:\nText.\n** Child\n* brushes\n* Next\n"
        );
    }

    #[test]
    fn split_trims_the_space_at_the_cut() {
        let src = "** One two\n";
        let out = write(src, |doc| Operation::SplitHeading {
            target: at(doc, "One two"),
            at: offset_of(src, " two"),
        });
        assert_eq!(out, "** One\n** two\n");
    }

    #[test]
    fn split_at_the_end_adds_an_empty_heading() {
        let src = "* TODO Title\nBody";
        let out = write(src, |doc| Operation::SplitHeading {
            target: at(doc, "Title"),
            at: offset_of(src, "\nBody"),
        });
        assert_eq!(out, "* TODO Title\nBody\n* \n");
    }

    #[test]
    fn split_outside_the_title_is_refused() {
        let src = "* TODO Title\n";
        let err = fails(src, |doc| Operation::SplitHeading {
            target: at(doc, "Title"),
            at: TextSize::from(2),
        });
        assert_eq!(err, OperationError::InvalidOffset);
    }

    #[test]
    fn split_refuses_a_rest_that_reads_as_a_keyword() {
        let src = "* Call TODO list\n";
        let err = fails(src, |doc| Operation::SplitHeading {
            target: at(doc, "Call TODO list"),
            at: offset_of(src, "TODO"),
        });
        assert!(matches!(err, OperationError::InvalidTitle(_)));
    }

    #[test]
    fn split_refuses_a_kept_part_that_reads_as_tags() {
        let src = "* a :b:c d\n";
        let err = fails(src, |doc| Operation::SplitHeading {
            target: at(doc, "a :b:c d"),
            at: offset_of(src, "c d"),
        });
        assert!(matches!(err, OperationError::InvalidTitle(_)));
    }

    // -- insert subtree ------------------------------------------------------

    #[test]
    fn insert_subtree_relevels_to_the_destination() {
        let src = "* A\n** B\n*** C\n";
        let out = write(src, |doc| Operation::InsertSubtree {
            at: SubtreeDest::After(at(doc, "C")),
            text: "* X\nx text\n** Y\n".to_owned(),
        });
        assert_eq!(out, "* A\n** B\n*** C\n*** X\nx text\n**** Y\n");
    }

    #[test]
    fn insert_subtree_promotes_deep_text() {
        let src = "* A\n";
        let out = write(src, |doc| Operation::InsertSubtree {
            at: SubtreeDest::After(at(doc, "A")),
            text: "\n*** X\n**** Y\n*** Z".to_owned(),
        });
        assert_eq!(out, "* A\n* X\n** Y\n* Z\n");
    }

    #[test]
    fn insert_subtree_between_headings_ends_its_line() {
        let src = "* A\n* B\n";
        let out = write(src, |doc| Operation::InsertSubtree {
            at: SubtreeDest::Before(at(doc, "B")),
            text: "** X".to_owned(),
        });
        assert_eq!(out, "* A\n* X\n* B\n");
    }

    #[test]
    fn insert_subtree_refuses_text_before_the_first_heading() {
        let src = "* A\n";
        for text in ["prose\n* X\n", "no heading\n", ""] {
            let err = fails(src, |doc| Operation::InsertSubtree {
                at: SubtreeDest::After(at(doc, "A")),
                text: text.to_owned(),
            });
            assert_eq!(err, OperationError::InvalidSubtree, "{text:?}");
        }
    }

    #[test]
    fn relevel_text_keeps_leading_text_and_shifts_every_heading() {
        let config = ParseConfig::default();
        assert_eq!(
            relevel_text("intro\n** A\n*** B\n** C\n", &config, 1).expect("relevel"),
            "intro\n* A\n** B\n* C\n"
        );
        assert_eq!(
            relevel_text("no headings\n", &config, 3).expect("relevel"),
            "no headings\n"
        );
        assert_eq!(
            relevel_text("* A\n", &config, 0),
            Err(OperationError::InvalidLevel)
        );
    }

    #[test]
    fn relevel_text_reads_the_documents_keywords() {
        let config = ParseConfig::default().with_inline_task_min_level(4);
        assert_eq!(
            relevel_text("* A\n** B\n", &config, 3),
            Err(OperationError::WouldBreakInlineTask)
        );
        let doc = parse_with("#+TODO: OPEN | SHUT\n* x\n", &config);
        assert_eq!(
            relevel_text("** OPEN A\n", doc.config(), 1).expect("relevel"),
            "* OPEN A\n"
        );
    }

    // -- reorder -------------------------------------------------------------

    #[test]
    fn reorder_moves_whole_children_with_their_blank_lines() {
        let src = "* P\ntext\n** B\nb\n\n** A\n*** A1\n** C";
        let out = write(src, |doc| Operation::ReorderChildren {
            target: at(doc, "P"),
            order: vec![1, 0, 2],
        });
        assert_eq!(out, "* P\ntext\n** A\n*** A1\n** B\nb\n\n** C");
        let out = write(src, |doc| Operation::ReorderChildren {
            target: at(doc, "P"),
            order: vec![2, 1, 0],
        });
        assert_eq!(out, "* P\ntext\n** C\n** A\n*** A1\n** B\nb\n");
    }

    #[test]
    fn reorder_identity_is_empty_and_bad_orders_are_refused() {
        let src = "* P\n** A\n** B\n";
        let doc = parse(src);
        let target = at(&doc, "P");
        let set = compile(
            &doc,
            &Operation::ReorderChildren {
                target,
                order: vec![0, 1],
            },
        )
        .expect("identity");
        assert!(set.is_empty());
        for order in [vec![0], vec![0, 0], vec![0, 2], vec![1, 0, 2]] {
            let err = compile(&doc, &Operation::ReorderChildren { target, order }).unwrap_err();
            assert_eq!(err, OperationError::InvalidOrder);
        }
    }

    // -- convert to text -----------------------------------------------------

    #[test]
    fn convert_first_child_joins_the_parents_text() {
        let src = "* P\nIntro.\n** TODO [#A] Buy paint :home:\nSCHEDULED: <2026-11-12 Thu>\n:PROPERTIES:\n:K: v\n:END:\nNotes.\n*** Brushes\n**** Fine\n** Next\n";
        let out = write(src, |doc| Operation::ConvertToText {
            target: at(doc, "Buy paint"),
        });
        assert_eq!(
            out,
            "* P\nIntro.\n\nBuy paint\n\nNotes.\n** Brushes\n*** Fine\n** Next\n"
        );
    }

    #[test]
    fn convert_after_a_headline_needs_no_blank_line() {
        let src = "* P\n** H\n";
        let out = write(src, |doc| Operation::ConvertToText {
            target: at(doc, "H"),
        });
        assert_eq!(out, "* P\nH\n");
        let src = "* Only";
        let out = write(src, |doc| Operation::ConvertToText {
            target: at(doc, "Only"),
        });
        assert_eq!(out, "Only");
    }

    #[test]
    fn convert_level_one_promotes_children_to_the_top() {
        let src = "* A\n** B\n*** C\n";
        let out = write(src, |doc| Operation::ConvertToText {
            target: at(doc, "A"),
        });
        assert_eq!(out, "A\n* B\n** C\n");
    }

    #[test]
    fn convert_an_empty_title_keeps_paragraphs_apart() {
        let src = "* P\nOne.\n** \nTwo.\n";
        let doc = parse(src);
        let target = NodeRef::of(&doc.map::<Outline>().nth(1).expect("empty"));
        let (_, out) = compile_write(&doc, &Operation::ConvertToText { target });
        assert_eq!(out, "* P\nOne.\n\nTwo.\n");
    }

    #[test]
    fn convert_refuses_a_title_that_would_not_read_as_a_paragraph() {
        for title in ["- groceries", "| a |", "#+TITLE: x", ": fixed", "1. first"] {
            let src = format!("* P\n** {title}\n");
            let err = fails(&src, |doc| Operation::ConvertToText {
                target: NodeRef::of(&doc.map::<Outline>().nth(1).expect("child")),
            });
            assert_eq!(err, OperationError::InvalidText, "{title:?}");
        }
    }
}
