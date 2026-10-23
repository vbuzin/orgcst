//! Shared compile steps for one part of a construct: rewrite or remove a
//! present part, insert an absent one at its insert point.

use super::empty_edit_set;
use crate::ast::{InsertPoint, PartSpan, PresentPart};
use crate::edit::EditSet;
use crate::writer::wrap_insert;
use crate::{Document, OperationError, TextRange};

/// Result of [`optional_part_edit`]. `None` is an idempotent no-op.
#[derive(Debug)]
pub(super) enum FieldAction {
    None,
    Replace {
        range: TextRange,
        replacement: String,
    },
}

impl FieldAction {
    pub(super) fn into_edit_set(self, doc: &Document) -> Result<EditSet, OperationError> {
        match self {
            Self::None => Ok(empty_edit_set(doc.source())),
            Self::Replace { range, replacement } => {
                Ok(EditSet::single(doc.source(), range, replacement)?)
            }
        }
    }
}

/// Edit for an optional part. `new_fragment` is the canonical text without
/// surrounding padding (`"TODO"`, `"[#A]"`, `"COMMENT"`, `":a:b:"`); `None`
/// clears the part through its removal range.
pub(super) fn optional_part_edit(
    doc: &Document,
    span: PartSpan,
    new_fragment: Option<&str>,
) -> Result<FieldAction, OperationError> {
    match (span, new_fragment) {
        (PartSpan::Present(part), None) => Ok(FieldAction::Replace {
            range: part.removal(),
            replacement: String::new(),
        }),
        (PartSpan::Present(part), Some(frag)) if doc.slice(part.range()) == frag => {
            Ok(FieldAction::None)
        }
        (PartSpan::Present(part), Some(frag)) => Ok(FieldAction::Replace {
            range: part.range(),
            replacement: frag.to_string(),
        }),
        (PartSpan::Absent(_), None) => Ok(FieldAction::None),
        (PartSpan::Absent(point), Some(frag)) => Ok(FieldAction::Replace {
            range: TextRange::empty(point.offset()),
            replacement: wrap_insert(point, frag),
        }),
        (PartSpan::NotApplicable, _) => Err(OperationError::PartNotApplicable),
    }
}

/// Edit for a part the grammar always emits, such as a title or keyword
/// value.
///
/// No-op when `new` matches. Empty content is filled by replacing the whole
/// part with `new` padded for `fill_dest`; otherwise only the content is
/// replaced. An empty `new` on non-empty content empties it (a title);
/// callers that clear through the removal range (a keyword) do that
/// themselves.
pub(super) fn replace_content_or_fill(
    doc: &Document,
    part: PresentPart,
    fill_dest: InsertPoint,
    new: &str,
) -> Result<EditSet, OperationError> {
    let content = part.content();
    if doc.slice(content) == new {
        return Ok(empty_edit_set(doc.source()));
    }
    if content.is_empty() {
        return Ok(EditSet::single(
            doc.source(),
            part.range(),
            wrap_insert(fill_dest, new),
        )?);
    }
    Ok(EditSet::single(doc.source(), content, new.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{HasParts, Heading, HeadingPart, Headline};
    use crate::parse;
    use crate::writer::wrap_insert;

    fn first_heading(doc: &crate::Document) -> Heading {
        doc.map::<Heading>().next().expect("heading")
    }

    #[test]
    fn present_clear_uses_removal() {
        let doc = parse("* TODO Title\n");
        let h = first_heading(&doc);
        let span = h.part_span(HeadingPart::Todo);
        let action = optional_part_edit(&doc, span, None).expect("clear");
        let FieldAction::Replace { range, replacement } = action else {
            panic!("expected replace");
        };
        assert!(replacement.is_empty(), "{replacement:?}");
        assert_eq!(doc.slice(range), "TODO ");
    }

    #[test]
    fn present_same_fragment_is_none() {
        let doc = parse("* TODO Title\n");
        let h = first_heading(&doc);
        let action =
            optional_part_edit(&doc, h.part_span(HeadingPart::Todo), Some("TODO")).expect("no-op");
        assert!(matches!(action, FieldAction::None));
    }

    #[test]
    fn present_rewrite_uses_construct() {
        let doc = parse("* TODO Title\n");
        let h = first_heading(&doc);
        let action = optional_part_edit(&doc, h.part_span(HeadingPart::Todo), Some("DONE"))
            .expect("rewrite");
        let FieldAction::Replace { range, replacement } = action else {
            panic!("expected replace");
        };
        assert_eq!(doc.slice(range), "TODO");
        assert_eq!(replacement, "DONE");
    }

    #[test]
    fn absent_clear_is_none() {
        let doc = parse("* Title\n");
        let h = first_heading(&doc);
        let action =
            optional_part_edit(&doc, h.part_span(HeadingPart::Todo), None).expect("absent clear");
        assert!(matches!(action, FieldAction::None));
    }

    #[test]
    fn absent_insert_wraps_at_dest() {
        let doc = parse("* Title\n");
        let h = first_heading(&doc);
        let span = h.part_span(HeadingPart::Todo);
        let dest = h.part_insert_point(HeadingPart::Todo).expect("absent");
        let action = optional_part_edit(&doc, span, Some("TODO")).expect("insert");
        let FieldAction::Replace { range, replacement } = action else {
            panic!("expected replace");
        };
        assert!(range.is_empty());
        assert_eq!(range.start(), dest.offset());
        assert_eq!(replacement, wrap_insert(dest, "TODO"));
        assert_eq!(replacement, "TODO ");
    }

    #[test]
    fn not_applicable_is_error() {
        let doc = parse("* H\n");
        let err = optional_part_edit(&doc, PartSpan::NotApplicable, Some("x")).unwrap_err();
        assert_eq!(err, OperationError::PartNotApplicable);
    }

    #[test]
    fn replace_content_or_fill_noop_fill_and_replace() {
        let doc = parse("* Hello\n");
        let h = first_heading(&doc);
        let PartSpan::Present(part) = h.part_span(HeadingPart::Title) else {
            panic!("title");
        };
        let dest = h.title_fill_insert_point();
        let set = replace_content_or_fill(&doc, part, dest, "Hello").expect("no-op");
        assert!(set.is_empty());
        let set = replace_content_or_fill(&doc, part, dest, "World").expect("replace");
        assert_eq!(doc.slice(set.edits()[0].range), "Hello");
        assert_eq!(set.edits()[0].replacement, "World");
        let set = replace_content_or_fill(&doc, part, dest, "").expect("empty content");
        assert_eq!(set.edits()[0].replacement, "");

        let doc = parse("* TODO\n");
        let h = first_heading(&doc);
        let PartSpan::Present(part) = h.part_span(HeadingPart::Title) else {
            panic!("title");
        };
        assert!(part.content().is_empty());
        let dest = h.title_fill_insert_point();
        let set = replace_content_or_fill(&doc, part, dest, "").expect("empty no-op");
        assert!(set.is_empty());
        let set = replace_content_or_fill(&doc, part, dest, "World").expect("fill");
        assert_eq!(set.edits()[0].range, part.range());
        assert_eq!(set.edits()[0].replacement, wrap_insert(dest, "World"));
    }
}
