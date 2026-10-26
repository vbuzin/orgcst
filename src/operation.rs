//! Typed operations, compiled to an [`EditSet`].
//!
//! A caller names a change as an [`Operation`]. [`compile`] validates it
//! against one snapshot and is the only fallible step; [`Document::apply`]
//! then applies the set.
//!
//! The tests check every compiled set against an "envelope": the ranges and
//! insert points of the construct the operation targets, which its edits may
//! not leave.

mod field;
mod headline;
mod locate;
mod metadata;
mod object;
mod structure;
mod text;

#[cfg(test)]
mod envelope;

pub use locate::{NodeRef, SubtreeDest, apply_locating, payload_start};

use crate::ast::{Delay, LinkShape, Repeater, TimestampPoint, TimestampShape};
use crate::edit::{EditSet, EditSetError};
use crate::writer::{SynthesisError, TimestampProblem, TitleProblem, render_priority};
use crate::{Document, OrgSyntaxKind, PriorityAtom, TextRange, TextSize};
use std::fmt;

// ---------------------------------------------------------------------------
// Supporting input types
// ---------------------------------------------------------------------------

/// Three-state property-line edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyEdit {
    /// Remove the matching line; the last line removes the drawer.
    Delete,
    /// `None` writes `:KEY:` plus a newline; `Some(v)` writes `:KEY: v`
    /// plus a newline. `Some("")` is invalid.
    Set {
        /// The value to write; `None` writes the key alone.
        value: Option<String>,
    },
}

/// Per-slot patch on a planning line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotPatch<T> {
    /// Leave this slot's last-wins timestamp (or omit a bare keyword).
    Keep,
    /// Drop this slot.
    Clear,
    /// Write this value.
    Set(T),
}

/// Deadline / scheduled / closed patch. Every slot must be filled; there is
/// no default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningPatch {
    /// Patch for the `DEADLINE:` slot.
    pub deadline: SlotPatch<TimestampInput>,
    /// Patch for the `SCHEDULED:` slot.
    pub scheduled: SlotPatch<TimestampInput>,
    /// Patch for the `CLOSED:` slot.
    pub closed: SlotPatch<TimestampInput>,
}

/// Timestamp to write: shape, civil points, optional repeater / delay / diary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampInput {
    /// How the timestamp is written.
    pub shape: TimestampShape,
    /// Date and optional time of the start. Required for every shape except
    /// [`TimestampShape::Diary`], where only its time is written.
    pub start: Option<TimestampPoint>,
    /// For a range shape, the second stamp; for a single stamp, the end of a
    /// time range on the start date, which must carry a time.
    pub end: Option<TimestampPoint>,
    /// Repeater, such as `+1w`. Not allowed on a diary timestamp.
    pub repeater: Option<Repeater>,
    /// Warning or delay period, such as `-2d`. Not allowed on a diary
    /// timestamp.
    pub delay: Option<Delay>,
    /// Sexp of a diary timestamp, written inside `<%%(` and `)>`; only for
    /// [`TimestampShape::Diary`].
    pub diary_sexp: Option<String>,
}

/// Spec for a newly inserted heading. The level is written as given: compile
/// does not adjust the stars to the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingInput {
    /// Star count. `0` is [`OperationError::InvalidLevel`]; `>=` the
    /// snapshot's inline-task minimum is [`OperationError::WouldBreakInlineTask`].
    pub level: usize,
    /// Title text.
    pub title: String,
    /// TODO keyword; it must be in the snapshot's vocabulary.
    pub todo: Option<String>,
    /// Priority cookie value.
    pub priority: Option<PriorityAtom>,
    /// Whether to write the `COMMENT` marker.
    pub commented: bool,
    /// Tags, in order; empty writes no tag group.
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Operation / OperationError
// ---------------------------------------------------------------------------

/// A typed edit request. Compile is the only fallible step; apply of the
/// resulting [`EditSet`] is not.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operation {
    /// Set or clear the TODO keyword. `None` clears. This does not cycle: the
    /// caller picks the next keyword.
    SetTodoState {
        /// The heading, inline task or outline.
        target: NodeRef,
        /// The new keyword; `None` clears it.
        state: Option<String>,
    },
    /// Set or clear the priority cookie. Cookie grammar is
    /// [`OperationError::InvalidPriority`]; a live `#+PRIORITIES:` range is
    /// [`OperationError::PriorityOutOfRange`].
    SetPriority {
        /// The heading, inline task or outline.
        target: NodeRef,
        /// The new priority; `None` removes the cookie.
        priority: Option<PriorityAtom>,
    },
    /// Replace the tag group. Empty `tags` clears it.
    SetTags {
        /// The heading, inline task or outline.
        target: NodeRef,
        /// The new tags, in order.
        tags: Vec<String>,
    },
    /// Replace the title content.
    ///
    /// A title that would re-parse as headline structure is
    /// [`OperationError::InvalidTitle`]. A leading word that looks like a TODO
    /// keyword, priority cookie or `COMMENT` is accepted when that slot is
    /// already filled, since it cannot be read as one.
    ///
    /// Filling an empty title is a zero-width insert, so batching it in
    /// [`compile_all`] with an insert of a missing TODO, priority or
    /// `COMMENT` on the same heading is [`OperationError::ConflictingInserts`].
    /// Apply them one after the other instead.
    SetTitle {
        /// The heading, inline task or outline.
        target: NodeRef,
        /// The new title text.
        title: String,
    },
    /// Set or clear the `COMMENT` marker.
    SetCommented {
        /// The heading, inline task or outline.
        target: NodeRef,
        /// Whether the marker is present afterwards.
        commented: bool,
    },

    /// Insert, update, or delete one property line (last line removes the
    /// drawer).
    SetProperty {
        /// The heading, inline task or outline whose drawer is edited.
        target: NodeRef,
        /// Property key, without the colons.
        key: String,
        /// What to write or delete.
        edit: PropertyEdit,
    },
    /// Patch planning slots. Timestamps under [`SlotPatch::Keep`] are copied
    /// from the source; a rebuilt line uses the canonical keyword spellings.
    SetPlanning {
        /// The heading, inline task or outline whose planning line is patched.
        target: NodeRef,
        /// The change for each slot.
        patch: PlanningPatch,
    },

    /// Insert a heading at `at`, at the level given in `heading`. Find the
    /// new heading with [`payload_start`]; mapping the destination offset
    /// does not land on it.
    InsertHeading {
        /// Where to insert the heading.
        at: SubtreeDest,
        /// The heading to write.
        heading: HeadingInput,
    },
    /// Delete an outline's subtree (its full CST extent). Afterwards
    /// [`apply_locating`] on the target returns `None`.
    DeleteSubtree {
        /// The outline, or its heading.
        target: NodeRef,
    },
    /// Move a subtree: re-level its stars for the destination, insert the
    /// copy, delete the source. [`apply_locating`] on the target returns
    /// `None`; find the moved heading with [`payload_start`].
    MoveSubtree {
        /// The outline to move, or its heading.
        target: NodeRef,
        /// Where to move it.
        to: SubtreeDest,
    },
    /// Clone a subtree: re-level like a move, but only insert. The target
    /// still resolves to the original; find the copy with [`payload_start`].
    CloneSubtree {
        /// The outline to copy, or its heading.
        target: NodeRef,
        /// Where to insert the copy.
        to: SubtreeDest,
    },
    /// Remove `by` stars from this heading, or the whole subtree. `by == 0`
    /// is an empty set.
    Promote {
        /// The outline, or its heading.
        target: NodeRef,
        /// Whether the descendants are promoted too.
        with_subtree: bool,
        /// Number of stars to remove.
        by: usize,
    },
    /// Add `by` stars to this heading, or the whole subtree. `by == 0` is
    /// an empty set.
    Demote {
        /// The outline, or its heading.
        target: NodeRef,
        /// Whether the descendants are demoted too.
        with_subtree: bool,
        /// Number of stars to add.
        by: usize,
    },

    /// Cycle an item checkbox. An unchecked box becomes checked and back; a
    /// partial box becomes checked; an item without a box gains a checked
    /// one. Parent checkboxes and statistics cookies are not updated.
    ToggleCheckbox {
        /// The list item.
        target: NodeRef,
    },
    /// Replace the keyword value. An empty `value` removes it, leaving
    /// `#+KEY:\n`. Newlines are [`OperationError::InvalidKeyword`].
    ///
    /// Setting keywords such as `#+TODO:` are edited like any other keyword;
    /// [`Document::apply`] re-resolves the settings from the new text.
    SetKeyword {
        /// The keyword line.
        target: NodeRef,
        /// The new value.
        value: String,
    },
    /// Replace the link path. Pass the unescaped path; a bracket link escapes
    /// it on write.
    ///
    /// [`OperationError::InvalidLink`] on an empty path, a newline, `>` in an
    /// angle link, a plain-link path that would re-parse as other syntax
    /// (`*` or `#+` near the start of a line, `[[` or `<` anywhere) or would
    /// not read back as the whole plain link (no registered `TYPE:`, or a
    /// tail such as trailing punctuation that Org leaves outside the link),
    /// or [`LinkShape::Radio`].
    SetLinkPath {
        /// The link.
        target: NodeRef,
        /// The new path, unescaped.
        path: String,
    },
    /// Set or clear a bracket link description. Angle, plain, and radio are
    /// [`OperationError::PartNotApplicable`]. Newlines, `]]`, or a trailing
    /// `]` in `description` are [`OperationError::InvalidLink`].
    SetLinkDescription {
        /// The bracket link.
        target: NodeRef,
        /// The new description; `None` removes it.
        description: Option<String>,
    },
    /// Rebuild the whole link construct as `to`. [`LinkShape::Radio`] is
    /// [`OperationError::InvalidLink`], and so is [`LinkShape::Plain`] when
    /// the path would not read back as the whole plain link. Same shape is a
    /// no-op.
    ///
    /// The writer rejects an angle or plain path that would not survive a
    /// re-parse (empty path, newline, `>` in an angle link, other syntax in a
    /// plain one) with [`OperationError::InvalidLink`].
    ///
    /// This replaces the whole construct in place. [`NodeRef::of`] is
    /// [`crate::Bias::Right`] at construct start, which maps to the end of the
    /// new bytes; to find the rewritten `LINK`, map [`crate::Bias::Left`] at
    /// construct start instead.
    ConvertLinkShape {
        /// The link.
        target: NodeRef,
        /// The shape to write.
        to: LinkShape,
    },

    /// Replace `range` with `replacement`, for typing, pasting or any change
    /// the caller computes itself. Identical bytes compile to an empty set.
    ReplaceText {
        /// Bytes to replace, in snapshot coordinates.
        range: TextRange,
        /// Text written in their place.
        replacement: String,
    },
}

/// Why [`compile`] / [`compile_all`] rejected an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationError {
    /// `target` does not name a live node of the requested kind.
    StaleTarget,
    /// The node is live but this operation does not accept it.
    WrongKind {
        /// The kinds this operation accepts.
        expected: &'static [OrgSyntaxKind],
        /// The kind of the node found.
        actual: OrgSyntaxKind,
    },
    /// Not in the snapshot's TODO vocabulary, or empty / whitespace.
    UnknownTodoKeyword(String),
    /// Cookie grammar is valid but outside the declared `#+PRIORITIES:` range.
    PriorityOutOfRange(PriorityAtom),
    /// Cookie grammar rejected the atom (letter not A-Z, number above 64).
    InvalidPriority,
    /// Tag name failed [`crate::is_valid_tag_name`].
    InvalidTag(String),
    /// Title would re-parse as headline structure.
    InvalidTitle(TitleProblem),
    /// Heading level was 0.
    InvalidLevel,
    /// Promote would drop a heading below level 1.
    CannotPromote,
    /// New level would parse as an inline task.
    WouldBreakInlineTask,
    /// Move destination is the source outline or a descendant.
    WouldContainSelf,
    /// Two empty-range inserts at the same offset in one [`compile_all`] batch.
    ConflictingInserts {
        /// Where both inserts land.
        offset: TextSize,
    },
    /// The property key is empty, contains whitespace, ends in `+`, or is
    /// `END`.
    InvalidPropertyKey(String),
    /// The value for this property key is empty or spans more than one line.
    InvalidPropertyValue(String),
    /// Planning line could not be synthesised (empty or duplicate slot).
    InvalidPlanning,
    /// Timestamp spec failed synthesis.
    InvalidTimestamp(TimestampProblem),
    /// Keyword value is not writable.
    InvalidKeyword,
    /// Link path, description, or shape is not writable.
    InvalidLink,
    /// This part cannot exist on the construct's current shape.
    PartNotApplicable,
    /// Coordinate validation failed ([`EditSet::new`]).
    EditSet(EditSetError),
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleTarget => {
                write!(
                    f,
                    "target no longer names a live node of the requested kind"
                )
            }
            Self::WrongKind { expected, actual } => {
                write!(
                    f,
                    "operation does not accept {actual:?}; expected one of {expected:?}"
                )
            }
            Self::UnknownTodoKeyword(s) => write!(f, "unknown TODO keyword {s:?}"),
            Self::PriorityOutOfRange(atom) => {
                write!(
                    f,
                    "priority {atom:?} is outside the declared #+PRIORITIES: range"
                )
            }
            Self::InvalidPriority => write!(f, "priority cookie is not valid grammar"),
            Self::InvalidTag(s) => write!(f, "invalid tag {s:?}"),
            Self::InvalidTitle(p) => write!(f, "title is not writable: {p:?}"),
            Self::InvalidLevel => write!(f, "heading level must be at least 1"),
            Self::CannotPromote => write!(f, "cannot promote a level-1 heading"),
            Self::WouldBreakInlineTask => {
                write!(f, "new level would parse as an inline task")
            }
            Self::WouldContainSelf => {
                write!(f, "move destination is the source outline or a descendant")
            }
            Self::ConflictingInserts { offset } => {
                write!(f, "conflicting inserts at offset {offset:?}")
            }
            Self::InvalidPropertyKey(s) => write!(f, "invalid property key {s:?}"),
            Self::InvalidPropertyValue(s) => write!(f, "invalid value for property {s:?}"),
            Self::InvalidPlanning => write!(f, "planning line cannot be synthesised"),
            Self::InvalidTimestamp(p) => write!(f, "timestamp is not writable: {p:?}"),
            Self::InvalidKeyword => write!(f, "keyword value is not writable"),
            Self::InvalidLink => write!(f, "link cannot be rewritten"),
            Self::PartNotApplicable => {
                write!(f, "this part is not applicable to the construct")
            }
            Self::EditSet(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EditSet(e) => Some(e),
            _ => None,
        }
    }
}

impl From<EditSetError> for OperationError {
    fn from(err: EditSetError) -> Self {
        Self::EditSet(err)
    }
}

pub(super) fn reject_todo(doc: &Document, s: &str) -> Result<(), OperationError> {
    if s.is_empty() || s.contains([' ', '\t', '\n', '\r']) || doc.config().todo_kind(s).is_none() {
        return Err(OperationError::UnknownTodoKeyword(s.to_string()));
    }
    Ok(())
}

pub(super) fn reject_priority(
    doc: &Document,
    atom: PriorityAtom,
) -> Result<String, OperationError> {
    let frag = render_priority(atom).map_err(map_synthesis)?;
    if let Some(range) = doc.priority_range()
        && !range.contains(atom)
    {
        return Err(OperationError::PriorityOutOfRange(atom));
    }
    Ok(frag)
}

pub(super) fn empty_edit_set(src: &str) -> EditSet {
    match EditSet::new(src, Vec::new()) {
        Ok(set) => set,
        Err(_) => unreachable!("empty edit list is always valid"),
    }
}

pub(super) fn map_synthesis(err: SynthesisError) -> OperationError {
    match err {
        SynthesisError::InvalidTag(s) => OperationError::InvalidTag(s),
        SynthesisError::InvalidTitle(p) => OperationError::InvalidTitle(p),
        SynthesisError::InvalidLevel => OperationError::InvalidLevel,
        SynthesisError::InvalidPropertyKey(s) => OperationError::InvalidPropertyKey(s),
        SynthesisError::InvalidPropertyValue(s) => OperationError::InvalidPropertyValue(s),
        SynthesisError::InvalidPriority => OperationError::InvalidPriority,
        SynthesisError::InvalidTimestamp(p) => OperationError::InvalidTimestamp(p),
        SynthesisError::EmptyPlanning | SynthesisError::DuplicatePlanningSlot => {
            OperationError::InvalidPlanning
        }
        SynthesisError::InvalidLinkPath | SynthesisError::InvalidLinkShape => {
            OperationError::InvalidLink
        }
        SynthesisError::InvalidTodo => {
            unreachable!("compile rejects TODO before render_heading")
        }
    }
}

// ---------------------------------------------------------------------------
// compile / compile_all
// ---------------------------------------------------------------------------

/// Compile one operation against `doc`.
///
/// Validation happens here. Apply the resulting set with
/// [`Document::apply`]; an empty set means there is nothing to do.
///
/// # Errors
///
/// Any [`OperationError`] for this snapshot. A coordinate failure is
/// [`OperationError::EditSet`].
///
/// # Examples
///
/// Set a TODO keyword on an existing headline, then apply the compiled
/// set. Compile does not apply, and this example does not re-resolve the
/// heading afterwards.
///
/// ```
/// use orgcst::{NodeRef, Operation, compile, parse};
///
/// let doc = parse("* TODO Ship it\n");
/// let heading = doc.headings().next().expect("one heading");
/// let op = Operation::SetTodoState {
///     target: NodeRef::of(&heading),
///     state: Some("DONE".to_string()),
/// };
/// let edits = compile(&doc, &op).expect("DONE is in the default vocabulary");
/// assert_eq!(doc.apply(&edits).write(), "* DONE Ship it\n");
/// ```
pub fn compile(doc: &Document, op: &Operation) -> Result<EditSet, OperationError> {
    match op {
        Operation::SetTodoState { target, state } => {
            headline::compile_set_todo(doc, *target, state.as_deref())
        }
        Operation::SetPriority { target, priority } => {
            headline::compile_set_priority(doc, *target, *priority)
        }
        Operation::SetTags { target, tags } => headline::compile_set_tags(doc, *target, tags),
        Operation::SetTitle { target, title } => headline::compile_set_title(doc, *target, title),
        Operation::SetCommented { target, commented } => {
            headline::compile_set_commented(doc, *target, *commented)
        }
        Operation::SetProperty { target, key, edit } => {
            metadata::compile_set_property(doc, *target, key, edit)
        }
        Operation::SetPlanning { target, patch } => {
            metadata::compile_set_planning(doc, *target, patch)
        }
        Operation::InsertHeading { at, heading } => {
            structure::compile_insert_heading(doc, *at, heading)
        }
        Operation::DeleteSubtree { target } => structure::compile_delete_subtree(doc, *target),
        Operation::MoveSubtree { target, to } => structure::compile_move_subtree(doc, *target, *to),
        Operation::CloneSubtree { target, to } => {
            structure::compile_clone_subtree(doc, *target, *to)
        }
        Operation::Promote {
            target,
            with_subtree,
            by,
        } => structure::compile_relevel(doc, *target, *with_subtree, *by, true),
        Operation::Demote {
            target,
            with_subtree,
            by,
        } => structure::compile_relevel(doc, *target, *with_subtree, *by, false),
        Operation::ToggleCheckbox { target } => object::compile_toggle_checkbox(doc, *target),
        Operation::SetKeyword { target, value } => object::compile_set_keyword(doc, *target, value),
        Operation::SetLinkPath { target, path } => {
            object::compile_set_link_path(doc, *target, path)
        }
        Operation::SetLinkDescription {
            target,
            description,
        } => object::compile_set_link_description(doc, *target, description.as_deref()),
        Operation::ConvertLinkShape { target, to } => {
            object::compile_convert_link_shape(doc, *target, *to)
        }
        Operation::ReplaceText { range, replacement } => {
            text::compile_replace_text(doc, *range, replacement)
        }
    }
}

/// Compile independent operations against **this** snapshot and merge them
/// into one [`EditSet`].
///
/// Each op is validated against `doc`, not against a hypothetical post-edit
/// tree. Empty results are dropped. Two empty-range inserts at the same
/// offset are [`OperationError::ConflictingInserts`]. Remaining coordinate
/// failures are [`OperationError::EditSet`]. Several inserts at one point,
/// whose order matters, belong in a single operation rather than this batch.
///
/// # Errors
///
/// Any error from [`compile`], plus [`OperationError::ConflictingInserts`]
/// when two empty-range inserts share an offset, and
/// [`OperationError::EditSet`] when the merged edits fail [`EditSet::new`].
pub fn compile_all(doc: &Document, ops: &[Operation]) -> Result<EditSet, OperationError> {
    let mut edits = Vec::new();
    for op in ops {
        let set = compile(doc, op)?;
        if !set.is_empty() {
            edits.extend_from_slice(set.edits());
        }
    }

    // At equal starts, put an empty-range insert before a replacement so the
    // pair touches rather than overlaps. Co-located empty inserts are then
    // adjacent and rejected: batch order says nothing about the order the
    // syntax needs.
    edits.sort_by_key(|e| (e.range.start(), !e.range.is_empty()));
    if let Some(pair) = edits.windows(2).find(|w| {
        w[0].range.is_empty() && w[1].range.is_empty() && w[0].range.start() == w[1].range.start()
    }) {
        return Err(OperationError::ConflictingInserts {
            offset: pair[0].range.start(),
        });
    }
    Ok(EditSet::new(doc.source(), edits)?)
}

impl Document {
    /// Compile `op` against this snapshot. Same as [`compile`].
    ///
    /// # Errors
    ///
    /// Same as [`compile`].
    pub fn compile(&self, op: &Operation) -> Result<EditSet, OperationError> {
        compile(self, op)
    }

    /// Compile independent ops against this snapshot. Same as [`compile_all`].
    ///
    /// # Errors
    ///
    /// Same as [`compile_all`].
    pub fn compile_all(&self, ops: &[Operation]) -> Result<EditSet, OperationError> {
        compile_all(self, ops)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn assert_compile_apply(doc: &Document, set: &EditSet) {
    let next = doc.apply(set);
    assert_eq!(next.write(), set.apply_to_text(doc.write()));
    assert!(std::ptr::eq(next.write(), next.source()));
}

/// Compile `op`, check the set against apply and the envelope, and return
/// it with the spliced text.
#[cfg(test)]
pub(crate) fn compile_write(doc: &Document, op: &Operation) -> (EditSet, String) {
    let set = compile(doc, op).expect("compile");
    assert_compile_apply(doc, &set);
    assert_envelope(doc, op, &set);
    let text = set.apply_to_text(doc.source());
    (set, text)
}

#[cfg(test)]
fn dests_of(part: impl IntoIterator<Item = crate::ast::InsertPoint>) -> Vec<TextSize> {
    part.into_iter().map(crate::InsertPoint::offset).collect()
}

#[cfg(test)]
fn assert_edits_within(set: &EditSet, outer: TextRange) {
    for e in set.edits() {
        assert!(
            e.range.start() >= outer.start() && e.range.end() <= outer.end(),
            "edit {:?} is outside envelope {:?}",
            e.range,
            outer
        );
    }
}

#[cfg(test)]
fn assert_edits_allowed(set: &EditSet, allowed: &[TextRange]) {
    for e in set.edits() {
        assert!(
            allowed.contains(&e.range),
            "edit {:?} is outside envelope {allowed:?}",
            e.range
        );
    }
}

#[cfg(test)]
fn assert_empty_inserts_at(set: &EditSet, dests: &[TextSize]) {
    for e in set.edits() {
        if e.range.is_empty() && !e.replacement.is_empty() {
            assert!(
                dests.contains(&e.range.start()),
                "empty-range insert {:?} is not a named dest {dests:?}",
                e.range
            );
        }
    }
}

#[cfg(test)]
fn assert_headline_envelope(
    task: &crate::ast::Task,
    set: &EditSet,
    dests: &[TextSize],
    todo: bool,
) {
    use crate::ast::Headline;
    let line = task.star_line_range();
    assert_edits_within(set, line);
    assert_empty_inserts_at(set, dests);
    if todo {
        for e in set.edits() {
            assert!(
                e.range != line || e.range.is_empty(),
                "SetTodoState must not replace the whole heading line"
            );
        }
    }
}

#[cfg(test)]
fn assert_construct_envelope(set: &EditSet, outer: TextRange, dests: &[TextSize]) {
    assert_edits_within(set, outer);
    assert_empty_inserts_at(set, dests);
}

#[cfg(test)]
fn property_envelope_ranges(doc: &Document, target: NodeRef) -> Vec<TextRange> {
    use crate::ast::AstNode;
    let task = locate::resolve_task(doc, target).expect("live metadata target");
    let mut allowed = vec![TextRange::empty(
        task.property_drawer_insert_point().offset(),
    )];
    if let Some(d) = task.property_drawer() {
        allowed.push(d.range());
        allowed.push(TextRange::empty(d.new_property_insert_point().offset()));
        for np in d.properties() {
            allowed.push(np.range());
        }
    }
    allowed
}

#[cfg(test)]
fn planning_envelope_ranges(doc: &Document, target: NodeRef) -> Vec<TextRange> {
    use crate::ast::{AstNode, HasParts, PlanningPart};
    let task = locate::resolve_task(doc, target).expect("live metadata target");
    let mut allowed = vec![TextRange::empty(task.planning_insert_point().offset())];
    if let Some(p) = task.planning() {
        allowed.push(p.range());
        for part in [
            PlanningPart::Deadline,
            PlanningPart::Scheduled,
            PlanningPart::Closed,
        ] {
            if let Some(r) = p.part_removal_range(part) {
                allowed.push(r);
            }
            if let Some(ip) = p.part_insert_point(part) {
                allowed.push(TextRange::empty(ip.offset()));
            }
        }
        if let Some(ts) = p.deadline() {
            allowed.push(ts.range());
        }
        if let Some(ts) = p.scheduled() {
            allowed.push(ts.range());
        }
        if let Some(ts) = p.closed() {
            allowed.push(ts.range());
        }
    }
    allowed
}

/// Every edit must stay inside the operation's envelope, and empty-range
/// inserts may only sit at the insert points the target's view names.
#[cfg(test)]
#[allow(clippy::too_many_lines)] // one arm of the envelope per operation
pub(crate) fn assert_envelope(doc: &Document, op: &Operation, set: &EditSet) {
    use crate::ast::{
        AstNode, HasParts, HeadingPart, Headline, Item, ItemPart, Keyword, KeywordPart, Link,
        LinkPart,
    };
    match op {
        Operation::SetTodoState { target, .. } => {
            let task = locate::resolve_task(doc, *target).expect("live headline target");
            let dests = dests_of(task.part_insert_point(HeadingPart::Todo));
            assert_headline_envelope(&task, set, &dests, true);
        }
        Operation::SetPriority { target, .. } => {
            let task = locate::resolve_task(doc, *target).expect("live headline target");
            let dests = dests_of(task.part_insert_point(HeadingPart::Priority));
            assert_headline_envelope(&task, set, &dests, false);
        }
        Operation::SetTags { target, .. } => {
            let task = locate::resolve_task(doc, *target).expect("live headline target");
            let dests = dests_of(task.part_insert_point(HeadingPart::Tags));
            assert_headline_envelope(&task, set, &dests, false);
        }
        Operation::SetCommented { target, .. } => {
            let task = locate::resolve_task(doc, *target).expect("live headline target");
            let dests = dests_of(task.part_insert_point(HeadingPart::CommentMarker));
            assert_headline_envelope(&task, set, &dests, false);
        }
        Operation::SetTitle { target, .. } => {
            let task = locate::resolve_task(doc, *target).expect("live headline target");
            let mut dests = vec![task.title_fill_insert_point().offset()];
            if let Some(c) = task.part_content_range(HeadingPart::Title)
                && c.is_empty()
            {
                dests.push(c.start());
            }
            assert_headline_envelope(&task, set, &dests, false);
        }
        Operation::SetProperty { target, .. } => {
            assert_edits_allowed(set, &property_envelope_ranges(doc, *target));
        }
        Operation::SetPlanning { target, .. } => {
            assert_edits_allowed(set, &planning_envelope_ranges(doc, *target));
        }
        Operation::InsertHeading { at, .. } => {
            let dest = locate::resolve_subtree_dest(doc, *at).expect("live dest");
            assert_edits_allowed(set, &[TextRange::empty(dest.point.offset())]);
        }
        Operation::DeleteSubtree { target } => {
            let outline = locate::resolve_outline(doc, *target).expect("live outline");
            assert_edits_allowed(set, &[outline.subtree_range()]);
        }
        Operation::MoveSubtree { target, to } => {
            let outline = locate::resolve_outline(doc, *target).expect("live outline");
            let dest = locate::resolve_subtree_dest(doc, *to).expect("live dest");
            assert_edits_allowed(
                set,
                &[
                    outline.subtree_range(),
                    TextRange::empty(dest.point.offset()),
                ],
            );
        }
        Operation::CloneSubtree { to, .. } => {
            let dest = locate::resolve_subtree_dest(doc, *to).expect("live dest");
            assert_edits_allowed(set, &[TextRange::empty(dest.point.offset())]);
        }
        Operation::Promote {
            target,
            with_subtree,
            ..
        }
        | Operation::Demote {
            target,
            with_subtree,
            ..
        } => {
            let outline = locate::resolve_outline(doc, *target).expect("live outline");
            let allowed: Vec<TextRange> = if *with_subtree {
                outline
                    .subtree_headings()
                    .map(|h| h.stars_range())
                    .collect()
            } else {
                vec![outline.heading().stars_range()]
            };
            assert_edits_allowed(set, &allowed);
        }
        Operation::ToggleCheckbox { target } => {
            let item = Item::cast(target.resolve(doc).expect("live item")).expect("ITEM");
            let dests = dests_of(item.part_insert_point(ItemPart::Checkbox));
            assert_construct_envelope(set, item.range(), &dests);
        }
        Operation::SetKeyword { target, .. } => {
            let keyword =
                Keyword::cast(target.resolve(doc).expect("live keyword")).expect("KEYWORD");
            let mut dests = vec![keyword.value_fill_insert_point().offset()];
            if let Some(c) = keyword.part_content_range(KeywordPart::Value)
                && c.is_empty()
            {
                dests.push(c.start());
            }
            assert_construct_envelope(set, keyword.range(), &dests);
        }
        Operation::SetLinkDescription { target, .. } => {
            let link = Link::cast(target.resolve(doc).expect("live link")).expect("LINK");
            let mut dests = dests_of(link.part_insert_point(LinkPart::Description));
            if let Some(c) = link.part_content_range(LinkPart::Description)
                && c.is_empty()
            {
                dests.push(c.start());
            }
            assert_construct_envelope(set, link.range(), &dests);
        }
        Operation::SetLinkPath { target, .. } | Operation::ConvertLinkShape { target, .. } => {
            let link = Link::cast(target.resolve(doc).expect("live link")).expect("LINK");
            assert_construct_envelope(set, link.range(), &[]);
        }
        Operation::ReplaceText { range, .. } => {
            assert_edits_within(set, *range);
            for e in set.edits() {
                assert_eq!(e.range, *range);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Edit, EditSet, EditSetError};
    use crate::parse;
    use rowan::{TextRange, TextSize};

    fn ts(n: u32) -> TextSize {
        TextSize::from(n)
    }

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(ts(start), ts(end))
    }

    fn replace(start: u32, end: u32, repl: &str) -> Operation {
        Operation::ReplaceText {
            range: range(start, end),
            replacement: repl.to_string(),
        }
    }

    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_copy<T: Copy>() {}

    #[test]
    fn public_handles_are_send_sync() {
        assert_send_sync::<Operation>();
        assert_send_sync::<OperationError>();
        assert_send_sync::<NodeRef>();
        assert_send_sync::<crate::Anchor>();
        assert_send_sync::<PropertyEdit>();
        assert_send_sync::<PlanningPatch>();
        assert_send_sync::<TimestampInput>();
        assert_send_sync::<HeadingInput>();
        assert_send_sync::<SubtreeDest>();
        assert_copy::<NodeRef>();
        assert_copy::<SubtreeDest>();
        assert_copy::<SlotPatch<()>>();
    }

    #[test]
    fn compile_all_conflicting_inserts_at_one_offset() {
        let doc = parse("ab");
        let err = compile_all(&doc, &[replace(1, 1, "1"), replace(1, 1, "2")]).unwrap_err();
        assert_eq!(err, OperationError::ConflictingInserts { offset: ts(1) });
        assert!(err.to_string().contains("conflicting inserts"));
    }

    #[test]
    fn compile_all_two_non_overlapping_replace_text() {
        let doc = parse("abcdef");
        let fwd = [replace(0, 2, "XY"), replace(4, 6, "Z")];
        let rev = [replace(4, 6, "Z"), replace(0, 2, "XY")];
        for ops in [&fwd[..], &rev[..]] {
            let set = compile_all(&doc, ops).expect("disjoint replacements");
            assert_eq!(set.apply_to_text(doc.source()), "XYcdZ");
            let via_method = doc.compile_all(ops).expect("Document::compile_all");
            assert_eq!(via_method.apply_to_text(doc.source()), "XYcdZ");
        }
        let next = doc.apply(&compile_all(&doc, &fwd).expect("fwd"));
        assert_eq!(next.write(), "XYcdZ");
    }

    #[test]
    fn compile_all_insert_then_replace_at_equal_start_either_order() {
        let doc = parse("abcdef");
        let insert_then_replace = [replace(2, 2, "XY"), replace(2, 4, "Z")];
        let replace_then_insert = [replace(2, 4, "Z"), replace(2, 2, "XY")];
        for ops in [&insert_then_replace[..], &replace_then_insert[..]] {
            let set = compile_all(&doc, ops).expect("insert before replacement at equal start");
            assert_eq!(set.apply_to_text(doc.source()), "abXYZef");
        }
    }

    #[test]
    fn compile_all_empty_inserts_at_different_offsets() {
        let doc = parse("ab");
        let set = compile_all(&doc, &[replace(2, 2, "2"), replace(1, 1, "1")])
            .expect("distinct insert offsets");
        assert_eq!(set.apply_to_text(doc.source()), "a1b2");
    }

    #[test]
    fn compile_all_overlapping_replace_text_is_editset() {
        let doc = parse("abcdef");
        let err = compile_all(&doc, &[replace(0, 3, "X"), replace(2, 5, "Y")]).unwrap_err();
        assert!(matches!(
            err,
            OperationError::EditSet(EditSetError::Overlap { .. })
        ));
    }

    #[test]
    fn raw_edit_set_still_allows_co_located_inserts() {
        let src = "ab";
        let set = EditSet::new(
            src,
            vec![
                Edit {
                    range: TextRange::empty(ts(1)),
                    replacement: "1".into(),
                },
                Edit {
                    range: TextRange::empty(ts(1)),
                    replacement: "2".into(),
                },
            ],
        )
        .expect("vector order is the tie-break for raw construction");
        assert_eq!(set.apply_to_text(src), "a12b");
        let doc = parse(src);
        assert!(matches!(
            compile_all(&doc, &[replace(1, 1, "1"), replace(1, 1, "2")]).unwrap_err(),
            OperationError::ConflictingInserts { offset } if offset == ts(1)
        ));
    }

    #[test]
    fn compile_all_empty_ops_is_empty_set() {
        let doc = parse("hello");
        let set = compile_all(&doc, &[]).expect("empty batch");
        assert!(set.is_empty());
        assert_eq!(set.apply_to_text(doc.source()), doc.source());
    }

    #[test]
    fn document_compile_matches_free_function() {
        let doc = parse("hello");
        let op = replace(1, 4, "ipp");
        let via_free = compile(&doc, &op).expect("compile");
        let via_method = doc.compile(&op).expect("Document::compile");
        assert_eq!(via_free.apply_to_text(doc.source()), "hippo");
        assert_eq!(
            via_free.apply_to_text(doc.source()),
            via_method.apply_to_text(doc.source())
        );
    }

    #[test]
    fn error_is_std_error() {
        let e = OperationError::ConflictingInserts { offset: ts(0) };
        let _boxed: Box<dyn std::error::Error> = Box::new(e.clone());
        assert!(!e.to_string().is_empty(), "{:?}", e.to_string());
        let wrapped = OperationError::EditSet(EditSetError::OutOfBounds(range(0, 1)));
        assert!(std::error::Error::source(&wrapped).is_some());
    }

    #[test]
    fn error_display_covers_every_arm() {
        let arms = [
            OperationError::StaleTarget,
            OperationError::WrongKind {
                expected: super::locate::TASK_KINDS,
                actual: OrgSyntaxKind::PARAGRAPH,
            },
            OperationError::UnknownTodoKeyword("NEXT".into()),
            OperationError::PriorityOutOfRange(PriorityAtom::Letter('D')),
            OperationError::InvalidPriority,
            OperationError::InvalidTag("a b".into()),
            OperationError::InvalidTitle(TitleProblem::TodoKeyword),
            OperationError::InvalidLevel,
            OperationError::CannotPromote,
            OperationError::WouldBreakInlineTask,
            OperationError::WouldContainSelf,
            OperationError::ConflictingInserts { offset: ts(0) },
            OperationError::InvalidPropertyKey("END".into()),
            OperationError::InvalidPropertyValue("KEY".into()),
            OperationError::InvalidPlanning,
            OperationError::InvalidTimestamp(TimestampProblem::MissingStart),
            OperationError::InvalidKeyword,
            OperationError::InvalidLink,
            OperationError::PartNotApplicable,
        ];
        for e in arms {
            assert!(!e.to_string().is_empty(), "{e:?}");
        }
    }
}
