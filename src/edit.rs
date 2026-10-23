//! Raw text edits, the only way to change a document.
//!
//! An [`EditSet`] is an ordered, non-overlapping batch of replacements in
//! pre-edit absolute byte coordinates. Construction validates the batch;
//! applying it only re-checks that the text is one the batch fits.

// Offsets are `TextSize` (`u32`) or a source under `MAX_SOURCE_LEN`.
// `String` lengths fit in `i64`: an allocation cannot exceed `isize::MAX`.
#![allow(clippy::cast_possible_wrap)]

use rowan::{TextRange, TextSize};
use std::fmt;

/// Gravity at an insertion point when mapping an offset through an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bias {
    /// Stay before text inserted at this offset (fold/decoration start).
    Left,
    /// Move with text inserted at this offset (caret after typing).
    Right,
}

/// A pre-edit offset plus gravity through an [`EditSet`].
///
/// [`EditSet::map_anchor`] keeps `bias` and maps `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    /// Byte offset in the pre-edit text.
    pub offset: TextSize,
    /// Which side of text inserted exactly at `offset` the anchor ends up on.
    pub bias: Bias,
}

/// A single contiguous replacement in pre-edit absolute coordinates.
///
/// Empty `range` is a pure insertion at `range.start`. Empty `replacement` is
/// a pure deletion of `range`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Bytes to replace, in pre-edit coordinates.
    pub range: TextRange,
    /// Text written in their place.
    pub replacement: String,
}

/// Ordered, non-overlapping batch of edits. Only [`EditSet::new`] builds
/// one, so every value is valid for the text it was built against.
///
/// Coordinates are all pre-edit. Two empty insertions at the same offset are
/// legal; **vector order** decides which replacement appears first in the
/// output. The batch is never re-sorted after construction.
#[derive(Debug, Clone)]
pub struct EditSet {
    edits: Vec<Edit>,
}

/// Why [`EditSet::new`] rejected a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditSetError {
    /// The range ends past the end of the source.
    OutOfBounds(TextRange),
    /// The offset falls inside a UTF-8 character.
    NotCharBoundary(TextSize),
    /// `next` starts before `previous`.
    OutOfOrder {
        /// The range of the earlier edit in the batch.
        previous: TextRange,
        /// The range of the edit after it.
        next: TextRange,
    },
    /// The two ranges share bytes.
    Overlap {
        /// The range of the earlier edit in the batch.
        a: TextRange,
        /// The range of the edit after it, which starts inside `a`.
        b: TextRange,
    },
}

impl fmt::Display for EditSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds(r) => {
                write!(f, "edit range {r:?} is outside the source")
            }
            Self::NotCharBoundary(off) => {
                write!(f, "offset {off:?} is not a UTF-8 char boundary")
            }
            Self::OutOfOrder { previous, next } => {
                write!(
                    f,
                    "edits out of order: {previous:?} then {next:?} (must be sorted by start)"
                )
            }
            Self::Overlap { a, b } => {
                write!(f, "overlapping edit ranges {a:?} and {b:?}")
            }
        }
    }
}

impl std::error::Error for EditSetError {}

/// Bounds and UTF-8 char-boundary check for every edit range against `src`.
/// Shared by construction and the re-check on splice.
fn check_ranges(src: &str, edits: &[Edit]) -> Result<(), EditSetError> {
    let len = TextSize::of(src);
    for edit in edits {
        let r = edit.range;
        if r.end() > len {
            return Err(EditSetError::OutOfBounds(r));
        }
        let start = usize::from(r.start());
        let end = usize::from(r.end());
        if !src.is_char_boundary(start) {
            return Err(EditSetError::NotCharBoundary(r.start()));
        }
        if !src.is_char_boundary(end) {
            return Err(EditSetError::NotCharBoundary(r.end()));
        }
    }
    Ok(())
}

fn try_text_size_from_i64(n: i64) -> Option<TextSize> {
    u32::try_from(n).ok().map(TextSize::from)
}

fn text_size_from_i64(n: i64) -> TextSize {
    try_text_size_from_i64(n).expect("mapped offset must be non-negative and fit TextSize")
}

impl EditSet {
    /// Validate and build an edit set against `src`.
    ///
    /// Checks, in order (first failure wins):
    /// 1. each edit is in bounds (`0..=src.len()`);
    /// 2. each range endpoint is a UTF-8 char boundary of `src`;
    /// 3. edits are sorted by `range.start` (non-strict: equal starts allowed);
    /// 4. ranges do not overlap (`prev.end > next.start` is overlap; touching
    ///    endpoints are fine).
    ///
    /// # Precondition for later apply
    ///
    /// The set is only guaranteed valid against the `src` used here. Applying
    /// it to a document whose source is not byte-identical to that text is a
    /// programmer error and panics (see [`Self::apply_to_text`] /
    /// [`crate::Document::apply`]).
    ///
    /// # Errors
    ///
    /// [`EditSetError::OutOfBounds`] or [`EditSetError::NotCharBoundary`]
    /// from the range check, then [`EditSetError::OutOfOrder`] or
    /// [`EditSetError::Overlap`].
    #[must_use = "constructed EditSet is unused"]
    pub fn new(src: &str, edits: Vec<Edit>) -> Result<Self, EditSetError> {
        check_ranges(src, &edits)?;
        for pair in edits.windows(2) {
            let prev = pair[0].range;
            let next = pair[1].range;
            if prev.start() > next.start() {
                return Err(EditSetError::OutOfOrder {
                    previous: prev,
                    next,
                });
            }
            if prev.end() > next.start() {
                return Err(EditSetError::Overlap { a: prev, b: next });
            }
        }
        Ok(Self { edits })
    }

    /// Single-edit convenience; same validation as [`Self::new`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::new`].
    #[must_use = "constructed EditSet is unused"]
    pub fn single(
        src: &str,
        range: TextRange,
        replacement: impl Into<String>,
    ) -> Result<Self, EditSetError> {
        Self::new(
            src,
            vec![Edit {
                range,
                replacement: replacement.into(),
            }],
        )
    }

    /// Splice the edits into `src`, in O(text + replacements).
    ///
    /// This is where a set is checked against the text it is applied to, so
    /// a mismatched `src` panics here and in [`crate::Document::apply`].
    ///
    /// # Panics
    ///
    /// Panics if `src` is not a valid pairing for this set (out of bounds or
    /// mid-character ranges). Prefer constructing with [`Self::new`] against
    /// the same `src`.
    #[must_use = "spliced text is unused"]
    pub fn apply_to_text(&self, src: &str) -> String {
        if let Err(e) = check_ranges(src, &self.edits) {
            panic!("EditSet applied to a different text than it was validated against: {e}");
        }
        let new_len = (src.len() as i64 + self.delta())
            .try_into()
            .expect("edit delta cannot make length negative when invariants hold");
        let mut out = String::with_capacity(new_len);
        let mut cursor = 0usize;
        for edit in &self.edits {
            let start = usize::from(edit.range.start());
            let end = usize::from(edit.range.end());
            out.push_str(&src[cursor..start]);
            out.push_str(&edit.replacement);
            cursor = end;
        }
        out.push_str(&src[cursor..]);
        out
    }

    /// Net byte delta: sum of `replacement.len() - range.len()` over edits.
    #[must_use]
    pub fn delta(&self) -> i64 {
        self.edits.iter().fold(0i64, |acc, e| {
            acc + e.replacement.len() as i64 - usize::from(e.range.len()) as i64
        })
    }

    /// Map a pre-edit offset through this batch.
    ///
    /// Pure geometry over the edit list: `off` is **not** re-validated against
    /// a source string. Callers that hold carets should pass offsets in
    /// `0..=pre_edit_len`. Bias matters only when the offset lies inside a
    /// replaced span or at an insertion point (`s <= off <= e` for an edit
    /// `[s, e)`).
    ///
    /// **Several edits can contain one offset** - co-located insertions, or an
    /// insertion touching the start of a replacement (all legal, see
    /// [`Self::new`]). `Left` lands before *every* such edit's new material;
    /// `Right` lands after *all* of it. Stopping at the first match would put a
    /// caret in the middle of one logical insertion.
    #[must_use]
    pub fn map_offset(&self, off: TextSize, bias: Bias) -> TextSize {
        text_size_from_i64(self.mapped_offset_i64(off, bias))
    }

    fn mapped_offset_i64(&self, off: TextSize, bias: Bias) -> i64 {
        let mut delta: i64 = 0;
        let off_u = usize::from(off);
        // Right bias: updated by every containing edit, so the last one wins.
        let mut right_landing: Option<i64> = None;
        for edit in &self.edits {
            let s = usize::from(edit.range.start());
            let e = usize::from(edit.range.end());
            if off_u < s {
                break;
            }
            if off_u <= e {
                let s_new = s as i64 + delta;
                match bias {
                    // Edits are sorted by start, so the first container has the
                    // leftmost landing: nothing later can move it further left.
                    Bias::Left => return s_new,
                    Bias::Right => {
                        right_landing = Some(s_new + edit.replacement.len() as i64);
                    }
                }
            }
            delta += edit.replacement.len() as i64 - (e as i64 - s as i64);
        }
        match right_landing {
            Some(landing) => landing,
            None => off_u as i64 + delta,
        }
    }

    /// Map a pre-edit range by mapping each end with the given biases.
    ///
    /// If the mapped start would exceed the mapped end (both ends collapsed
    /// into a replacement with opposing biases), returns an empty range at the
    /// mapped start: the selected text no longer exists.
    #[must_use]
    pub fn map_range(&self, r: TextRange, start_bias: Bias, end_bias: Bias) -> TextRange {
        let start = self.map_offset(r.start(), start_bias);
        let end = self.map_offset(r.end(), end_bias);
        if start > end {
            TextRange::empty(start)
        } else {
            TextRange::new(start, end)
        }
    }

    /// Map a pre-edit [`Anchor`] through this batch. Bias is preserved.
    #[must_use]
    pub fn map_anchor(&self, a: Anchor) -> Anchor {
        Anchor {
            offset: self.map_offset(a.offset, a.bias),
            bias: a.bias,
        }
    }

    /// Like [`Self::map_anchor`], `None` when the landing does not fit `TextSize`.
    #[must_use]
    pub(crate) fn try_map_anchor(&self, a: Anchor) -> Option<Anchor> {
        Some(Anchor {
            offset: try_text_size_from_i64(self.mapped_offset_i64(a.offset, a.bias))?,
            bias: a.bias,
        })
    }

    #[must_use]
    /// The edits, in batch order.
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    #[must_use]
    /// Number of edits in the batch.
    pub fn len(&self) -> usize {
        self.edits.len()
    }

    #[must_use]
    /// Whether the batch has no edits; applying it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// The edits, in batch order.
    pub fn iter(&self) -> impl Iterator<Item = &Edit> {
        self.edits.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(n: u32) -> TextSize {
        TextSize::from(n)
    }

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(ts(start), ts(end))
    }

    fn edit(start: u32, end: u32, repl: &str) -> Edit {
        Edit {
            range: range(start, end),
            replacement: repl.to_string(),
        }
    }

    #[test]
    fn empty_set_is_identity() {
        let src = "hello";
        let set = EditSet::new(src, vec![]).unwrap();
        assert!(set.is_empty());
        assert_eq!(set.apply_to_text(src), src);
        assert_eq!(set.delta(), 0);
    }

    #[test]
    fn accepts_insert_at_zero_and_len() {
        let src = "ab";
        EditSet::single(src, TextRange::empty(ts(0)), "x").unwrap();
        EditSet::single(src, TextRange::empty(ts(2)), "y").unwrap();
    }

    #[test]
    fn accepts_touching_endpoints() {
        let src = "abcdef";
        let set = EditSet::new(src, vec![edit(0, 3, "X"), edit(3, 6, "Y")]).unwrap();
        assert_eq!(set.apply_to_text(src), "XY");
    }

    #[test]
    fn co_located_inserts_keep_vector_order() {
        let src = "ab";
        let set = EditSet::new(src, vec![edit(1, 1, "1"), edit(1, 1, "2")]).unwrap();
        assert_eq!(set.apply_to_text(src), "a12b");
    }

    #[test]
    fn empty_source_only_empty_range() {
        let src = "";
        EditSet::single(src, TextRange::empty(ts(0)), "x").unwrap();
        assert!(matches!(
            EditSet::single(src, range(0, 1), ""),
            Err(EditSetError::OutOfBounds(_))
        ));
    }

    #[test]
    fn rejects_out_of_order() {
        let src = "abcdef";
        let err = EditSet::new(src, vec![edit(3, 4, "x"), edit(1, 2, "y")]).unwrap_err();
        assert_eq!(
            err,
            EditSetError::OutOfOrder {
                previous: range(3, 4),
                next: range(1, 2),
            }
        );
    }

    #[test]
    fn rejects_overlap() {
        let src = "abcdef";
        let err = EditSet::new(src, vec![edit(0, 4, "x"), edit(2, 5, "y")]).unwrap_err();
        assert_eq!(
            err,
            EditSetError::Overlap {
                a: range(0, 4),
                b: range(2, 5),
            }
        );
    }

    #[test]
    fn rejects_mid_char_boundary() {
        // "cafe" with U+00E9: bytes c a f C3 A9
        let src = "caf\u{e9}";
        assert_eq!(src.len(), 5);
        let err = EditSet::single(src, range(0, 4), "x").unwrap_err();
        assert_eq!(err, EditSetError::NotCharBoundary(ts(4)));
    }

    #[test]
    fn rejects_out_of_bounds() {
        let src = "abc";
        let err = EditSet::single(src, range(0, 4), "").unwrap_err();
        assert_eq!(err, EditSetError::OutOfBounds(range(0, 4)));
    }

    #[test]
    fn validation_order_bounds_before_order() {
        // First edit OOB; second would also be out of order if checked first.
        let src = "ab";
        let err = EditSet::new(src, vec![edit(0, 5, "x"), edit(0, 1, "y")]).unwrap_err();
        assert!(matches!(err, EditSetError::OutOfBounds(_)));
    }

    #[test]
    fn splice_multi_edit_nonzero_delta() {
        let src = "abcdef";
        // replace [1,3) "bc" -> "XY", delete [4,5) "e" -> "aXYdf"
        let set = EditSet::new(src, vec![edit(1, 3, "XY"), edit(4, 5, "")]).unwrap();
        let out = set.apply_to_text(src);
        assert_eq!(out, "aXYdf");
        assert_eq!(set.delta(), out.len() as i64 - src.len() as i64);
    }

    #[test]
    fn splice_delete_all_and_prepend_append() {
        let src = "hi";
        let del = EditSet::single(src, range(0, 2), "").unwrap();
        assert_eq!(del.apply_to_text(src), "");
        let pre = EditSet::single(src, TextRange::empty(ts(0)), "X").unwrap();
        assert_eq!(pre.apply_to_text(src), "Xhi");
        let post = EditSet::single(src, TextRange::empty(ts(2)), "Y").unwrap();
        assert_eq!(post.apply_to_text(src), "hiY");
    }

    #[test]
    fn map_offset_matches_splice_landings() {
        let src = "abcdefgh";
        let set = EditSet::new(src, vec![edit(1, 3, "XYZ"), edit(5, 7, "")]).unwrap();
        let out = set.apply_to_text(src);
        let mut land = 0usize;
        let mut cursor = 0usize;
        for e in set.edits() {
            let s = usize::from(e.range.start());
            land += s - cursor;
            let left = usize::from(set.map_offset(e.range.start(), Bias::Left));
            let right = usize::from(set.map_offset(e.range.start(), Bias::Right));
            // Left at edit start -> start of replacement in spliced text.
            assert_eq!(left, land);
            // Right at edit start -> end of replacement.
            assert_eq!(right, land + e.replacement.len());
            assert_eq!(&out[left..right], e.replacement.as_str());
            land += e.replacement.len();
            cursor = usize::from(e.range.end());
        }
    }

    /// Several edits can share one offset. `Right` must clear all of their new
    /// material and `Left` land before all of it; stopping at the first
    /// containing edit would leave `Right` between two runs of one insertion.
    #[test]
    fn map_offset_spans_all_edits_containing_the_offset() {
        // Two co-located insertions: "a12b". Right at 1 lands after "12".
        let src = "ab";
        let set = EditSet::new(src, vec![edit(1, 1, "1"), edit(1, 1, "2")]).unwrap();
        assert_eq!(set.apply_to_text(src), "a12b");
        assert_eq!(set.map_offset(ts(1), Bias::Right), ts(3));
        assert_eq!(set.map_offset(ts(1), Bias::Left), ts(1));

        // Insertion touching the start of a replacement: "aXYd".
        let src2 = "abcd";
        let set2 = EditSet::new(src2, vec![edit(1, 1, "X"), edit(1, 3, "Y")]).unwrap();
        assert_eq!(set2.apply_to_text(src2), "aXYd");
        assert_eq!(set2.map_offset(ts(1), Bias::Right), ts(3));
        assert_eq!(set2.map_offset(ts(1), Bias::Left), ts(1));

        // Three at one point, to prove it is not an off-by-one for exactly two.
        let set3 =
            EditSet::new(src, vec![edit(1, 1, "1"), edit(1, 1, "2"), edit(1, 1, "3")]).unwrap();
        assert_eq!(set3.apply_to_text(src), "a123b");
        assert_eq!(set3.map_offset(ts(1), Bias::Right), ts(4));
    }

    /// Every offset in `0..=len`, both biases, must land where the spliced text
    /// says - and mapping must never move backwards as the offset grows.
    #[test]
    fn map_offset_is_monotonic_and_in_bounds() {
        let src = "abcdefgh";
        let sets = [
            EditSet::new(src, vec![edit(1, 3, "XYZ"), edit(5, 7, "")]).unwrap(),
            EditSet::new(src, vec![edit(0, 0, "P"), edit(0, 2, "Q")]).unwrap(),
            EditSet::new(src, vec![edit(4, 4, "1"), edit(4, 4, "2"), edit(8, 8, "!")]).unwrap(),
            EditSet::new(src, vec![edit(0, 8, "")]).unwrap(),
        ];
        for (i, set) in sets.iter().enumerate() {
            let out_len = set.apply_to_text(src).len();
            for bias in [Bias::Left, Bias::Right] {
                let mut prev = 0usize;
                for off in 0..=src.len() {
                    let m = usize::from(set.map_offset(ts(off as u32), bias));
                    assert!(
                        m <= out_len,
                        "set {i} bias {bias:?} off {off}: {m} > {out_len}"
                    );
                    assert!(m >= prev, "set {i} bias {bias:?} off {off}: {m} < {prev}");
                    prev = m;
                }
            }
        }
    }

    #[test]
    fn try_map_anchor_overflow_is_none() {
        let set = EditSet::single("a", TextRange::empty(ts(0)), "x").unwrap();
        let forged = Anchor {
            offset: TextSize::from(u32::MAX),
            bias: Bias::Right,
        };
        assert!(set.try_map_anchor(forged).is_none());
        assert_eq!(set.map_offset(ts(0), Bias::Right), ts(1));
    }

    #[test]
    fn map_anchor_keeps_bias_through_insert_and_delete() {
        let src = "hello";
        let insert = EditSet::single(src, TextRange::empty(ts(2)), "XX").unwrap();
        let at_insert = Anchor {
            offset: ts(2),
            bias: Bias::Right,
        };
        let mapped_right = insert.map_anchor(at_insert);
        assert_eq!(mapped_right.bias, Bias::Right);
        assert_eq!(mapped_right.offset, ts(4));
        assert_eq!(
            insert
                .map_anchor(Anchor {
                    offset: ts(2),
                    bias: Bias::Left,
                })
                .offset,
            ts(2)
        );
        assert_eq!(
            insert
                .map_anchor(Anchor {
                    offset: ts(4),
                    bias: Bias::Right,
                })
                .offset,
            ts(6)
        );

        // "hello" delete [1, 4) "ell" -> "ho". Offset 4 ('o') is the delete end.
        let delete = EditSet::single(src, range(1, 4), "").unwrap();
        assert_eq!(
            delete
                .map_anchor(Anchor {
                    offset: ts(0),
                    bias: Bias::Right,
                })
                .offset,
            ts(0)
        );
        assert_eq!(
            delete
                .map_anchor(Anchor {
                    offset: ts(4),
                    bias: Bias::Right,
                })
                .offset,
            ts(1)
        );
        assert_eq!(
            delete
                .map_anchor(Anchor {
                    offset: ts(4),
                    bias: Bias::Left,
                })
                .offset,
            ts(1)
        );
    }

    #[test]
    fn map_range_collapses_when_inverted() {
        let src = "0123456789";
        // Replace [2,9) with "XY". Range [3,7) both ends inside.
        let set = EditSet::single(src, range(2, 9), "XY").unwrap();
        let mapped = set.map_range(range(3, 7), Bias::Right, Bias::Left);
        // The start maps right to 4 (after "XY"), the end maps left to 2:
        // inverted, so the result is empty at 4.
        assert!(mapped.is_empty());
        assert_eq!(mapped.start(), set.map_offset(ts(3), Bias::Right));
    }

    #[test]
    fn error_is_std_error() {
        let e = EditSetError::OutOfBounds(range(0, 1));
        let _boxed: Box<dyn std::error::Error> = Box::new(e.clone());
        assert!(!e.to_string().is_empty(), "{:?}", e.to_string());
    }

    #[test]
    #[should_panic(expected = "different text")]
    fn apply_to_text_panics_on_mismatched_source() {
        let set = EditSet::single("abcdef", range(0, 3), "x").unwrap();
        let _ = set.apply_to_text("ab"); // shorter - OOB
    }
}
