//! Token-level primitives the views share.
//!
//! Views read child tokens only through this module: line partition, token
//! and span scans, content, removal and insert geometry, trailing blanks.
//! Per-construct walks live in `layout/<family>.rs` beside their tests.
//!
//! Rules:
//! 1. Read token kinds. Read token text only in a *slicer*: a function that
//!    splits text whose shape the parser already checked, such as the year of
//!    a date. Never re-lex; no parser combinators or regular expressions.
//! 2. Never leave the node passed in: no `parent()`, no sibling walk off the
//!    node, no indexing into the document source.
//! 3. Span searches (`between`, `delimited_interior`, field slicers): missing
//!    marker yields `None`. A present empty interior is a zero-width span at
//!    the interior, never a fake span at offset 0.
//!
//! Test-only helpers (`first_kind`, `synth`, `run_text`) are `cfg(test)`.

use crate::ast::{InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode, SyntaxToken};
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
// Line partition
// ---------------------------------------------------------------------------

/// Three-way split of a node's direct children into whole source lines.
///
/// A `NEWLINE` **terminates** the line it ends rather than opening the next
/// one, so every line here carries its own terminator, exactly as the CST
/// stores it (a `HEADING` node includes its `NEWLINE`). A node
/// whose last child is not a `NEWLINE` has one unterminated final line.
///
/// - `first_line` - the first line, terminator included. The whole node when
///   the node is a single line.
/// - `body` - the whole lines strictly between the first and the last, each
///   with its own terminator. Empty for a node of fewer than three lines.
/// - `last_line` - the final line, terminator included. **Empty when the node
///   is a single line**: a degenerate one-line block has no end line, and
///   reporting the begin line as the end line would carve the wrong part.
///
/// The three concatenate back to the node's children in order.
#[derive(Debug, Clone)]
#[cfg(test)]
pub(crate) struct LinePartition {
    pub first_line: Vec<SyntaxElement>,
    pub body: Vec<SyntaxElement>,
    pub last_line: Vec<SyntaxElement>,
}

/// Partition `node`'s direct children into first line, body lines, last line.
#[cfg(test)]
#[must_use]
pub(crate) fn line_partition(node: &SyntaxNode) -> LinePartition {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    // Cut one past each NEWLINE, so a cut is a line *end*. A trailing run with
    // no terminator is the final (unterminated) line.
    let mut cuts: Vec<usize> = kids
        .iter()
        .enumerate()
        .filter_map(|(i, e)| (e.kind() == OrgSyntaxKind::NEWLINE).then_some(i + 1))
        .collect();
    if !kids.is_empty() && cuts.last().copied() != Some(kids.len()) {
        cuts.push(kids.len());
    }

    // Fewer than two lines: the whole node is the first line and there is no
    // distinct last line.
    if cuts.len() < 2 {
        return LinePartition {
            first_line: kids,
            body: Vec::new(),
            last_line: Vec::new(),
        };
    }

    let first_end = cuts[0];
    let last_start = cuts[cuts.len() - 2];
    LinePartition {
        first_line: kids[..first_end].to_vec(),
        body: kids[first_end..last_start].to_vec(),
        last_line: kids[last_start..].to_vec(),
    }
}

/// Body span of a [`LinePartition`]: the middle lines, or a zero-width span
/// at the end of `first_line` when the node has a distinct last line and an
/// empty body (empty quote, empty drawer).
///
/// `None` for a single-line node - there is no interior between a begin line
/// and an end line.
#[cfg(test)]
#[must_use]
pub(crate) fn partition_body_range(p: &LinePartition) -> Option<TextRange> {
    if let Some(r) = span_of(p.body.iter().cloned()) {
        return Some(r);
    }
    if p.last_line.is_empty() {
        return None;
    }
    Some(TextRange::empty(p.first_line.last()?.text_range().end()))
}

/// Direct children of `node` up to and including the first `NEWLINE`, or all
/// children when the node is single-line.
pub(crate) fn first_line(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> + '_ {
    let mut seen_nl = false;
    node.children_with_tokens().take_while(move |e| {
        if seen_nl {
            return false;
        }
        if e.kind() == OrgSyntaxKind::NEWLINE {
            seen_nl = true;
        }
        true
    })
}

/// End of the first source line under `node` (the line terminator, or the
/// node end when there is none).
#[must_use]
pub(crate) fn first_line_end(node: &SyntaxNode) -> TextSize {
    first_line(node)
        .last()
        .map_or_else(|| node.text_range().end(), |e| e.text_range().end())
}

// ---------------------------------------------------------------------------
// Token runs
// ---------------------------------------------------------------------------

/// Absolute range of the direct-child token run strictly between the first
/// `open` and the first subsequent `close`.
///
/// Returns `None` when either marker is missing. Adjacent markers with
/// nothing between are a present empty interior: a zero-width span at the
/// closer. This never invents a span at offset 0 for a failed search.
#[must_use]
pub(crate) fn between(
    node: &SyntaxNode,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
) -> Option<TextRange> {
    between_in(node.children_with_tokens(), open, close)
}

/// [`between`] over an arbitrary element run. Missing marker yields `None`.
fn between_in(
    elems: impl IntoIterator<Item = SyntaxElement>,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
) -> Option<TextRange> {
    let mut elems = elems.into_iter();
    loop {
        let e = elems.next()?;
        if e.kind() == open {
            break;
        }
    }
    let mut interior = Vec::new();
    for e in elems {
        if e.kind() == close {
            return Some(match span_of(interior) {
                Some(r) => r,
                None => TextRange::empty(e.text_range().start()),
            });
        }
        interior.push(e);
    }
    None
}

/// Interior of `node` after stripping a matching opener/closer pair of
/// **first and last** direct-child tokens.
///
/// `between` stops at the first closer, which is wrong for a greedy dual
/// (`[a][b]` is one `KEYWORD_DUAL`). Empty interior is a zero-width span at
/// the closer. `None` when the first child is not `open` or the last is not
/// `close`.
#[must_use]
pub(crate) fn delimited_interior(
    node: &SyntaxNode,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
) -> Option<TextRange> {
    let first = node.first_child_or_token()?;
    let last = node.last_child_or_token()?;
    if first.kind() != open || last.kind() != close {
        return None;
    }
    let start = first.text_range().end();
    let end = last.text_range().start();
    (start <= end).then(|| TextRange::new(start, end))
}

/// Absolute range spanned by a contiguous run of elements, or `None` if empty.
#[must_use]
pub(crate) fn span_of(elements: impl IntoIterator<Item = SyntaxElement>) -> Option<TextRange> {
    let mut start = None;
    let mut end = None;
    for e in elements {
        let r = e.text_range();
        if start.is_none() {
            start = Some(r.start());
        }
        end = Some(r.end());
    }
    match (start, end) {
        (Some(s), Some(e)) if s <= e => Some(TextRange::new(s, e)),
        _ => None,
    }
}

/// Direct child tokens of `kind`, in source order.
pub(crate) fn tokens(
    node: &SyntaxNode,
    kind: OrgSyntaxKind,
) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.children_with_tokens().filter_map(move |e| {
        let t = e.into_token()?;
        (t.kind() == kind).then_some(t)
    })
}

/// First direct child token of `kind`.
#[must_use]
pub(crate) fn first_token_of_kind(node: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxToken> {
    tokens(node, kind).next()
}

/// Last direct child token of `kind`.
#[must_use]
pub(crate) fn last_token_of_kind(node: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxToken> {
    tokens(node, kind).last()
}

/// Run `f` on the node's text. Single-token nodes borrow; multi-token nodes
/// concatenate.
pub(crate) fn with_node_text<R>(node: &SyntaxNode, f: impl FnOnce(&str) -> R) -> R {
    match single_child_token(node) {
        Some(tok) => f(tok.text()),
        None => f(&node.text().to_string()),
    }
}

/// Absolute `range` as text inside `node`.
///
/// Slices [`SyntaxNode::text`] so a first-line field does not stringify the
/// body. `range` is a token-aligned sub-span of the node.
pub(crate) fn text_in(node: &SyntaxNode, range: TextRange) -> String {
    let node_range = node.text_range();
    debug_assert!(node_range.start() <= range.start() && range.end() <= node_range.end());
    let rel = TextRange::new(
        range.start() - node_range.start(),
        range.end() - node_range.start(),
    );
    node.text().slice(rel).to_string()
}

fn single_child_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut kids = node.children_with_tokens();
    let t = kids.next()?.into_token()?;
    kids.next().is_none().then_some(t)
}

/// First whitespace-delimited word inside `node`, after leading horizontal
/// whitespace. `None` when the node is empty or whitespace-only.
#[must_use]
pub(crate) fn first_word_range(node: &SyntaxNode) -> Option<TextRange> {
    word_run(node.children_with_tokens())
}

pub(crate) fn word_run(elements: impl IntoIterator<Item = SyntaxElement>) -> Option<TextRange> {
    let mut start = None;
    let mut end = None;
    for el in elements {
        if is_word_break(&el) {
            if start.is_some() {
                break;
            }
            continue;
        }
        let r = el.text_range();
        if start.is_none() {
            start = Some(r.start());
        }
        end = Some(r.end());
    }
    Some(TextRange::new(start?, end?))
}

pub(crate) fn is_word_break(el: &SyntaxElement) -> bool {
    is_horizontal_ws(el) || el.kind() == OrgSyntaxKind::NEWLINE
}

/// Direct children after the last direct child of kind `kind`.
///
/// The anchor may be a **node or a token** kind: the star run of a heading is
/// a run of `STAR` tokens, not a carved node. Both the anchor and the yielded
/// children
/// span nodes and tokens alike (e.g. `TAGS` after `HEADING_TITLE`).
///
/// `None` when `node` has no direct child of `kind` - a missing anchor is
/// never an empty iterator standing in for one (module rule 3). "Anchor
/// present, nothing after it" is `Some(<empty>)`.
///
/// Anchoring is by child index, not by offset, so a zero-length anchor cannot
/// match itself.
#[must_use]
pub(crate) fn children_after_last(
    node: &SyntaxNode,
    kind: OrgSyntaxKind,
) -> Option<impl Iterator<Item = SyntaxElement> + '_> {
    let last_idx = node
        .children_with_tokens()
        .enumerate()
        .filter(|(_, e)| e.kind() == kind)
        .map(|(i, _)| i)
        .last()?;
    Some(node.children_with_tokens().skip(last_idx + 1))
}

// ---------------------------------------------------------------------------
// Content trim and removal
// ---------------------------------------------------------------------------

/// Content span of `node`: full range with leading and trailing horizontal
/// whitespace tokens among direct children stripped.
///
/// An all-whitespace node yields an empty range at `range.start()` so a
/// replacement can clear the field without inventing a `None` case for a
/// present node.
#[must_use]
pub(crate) fn content_of(node: &SyntaxNode) -> TextRange {
    content_of_range(node, node.text_range())
}

/// Content span of `carved` interpreted as a sub-range of `node`.
///
/// Only direct-child elements whose ranges sit inside `carved` participate.
/// Leading/trailing children that are horizontal whitespace (`WHITESPACE` or
/// `INDENT`) are dropped. `NEWLINE` is not trimmed: line ends are structural.
///
/// When `carved` is exactly one child **node**, the trim descends into that
/// child: its own separator whitespace is a child of *it*, not of `node`, so
/// trimming at this level alone would return the carve untouched and a
/// replacement would overwrite the separator, as when replacing a heading's
/// title.
#[must_use]
pub(crate) fn content_of_range(node: &SyntaxNode, carved: TextRange) -> TextRange {
    let mut elems: Vec<SyntaxElement> = node
        .children_with_tokens()
        .filter(|e| range_contains_range(carved, e.text_range()))
        .collect();

    if let [only] = elems.as_slice()
        && let Some(child) = only.as_node()
        && child.text_range() == carved
    {
        return content_of(child);
    }

    while elems.first().is_some_and(is_horizontal_ws) {
        elems.remove(0);
    }
    while elems.last().is_some_and(is_horizontal_ws) {
        elems.pop();
    }

    match span_of(elems) {
        Some(r) => r,
        None => TextRange::empty(carved.start()),
    }
}

/// Which side of a carved span absorbs the separator that exists only because
/// the part is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemovalSide {
    /// Separator immediately before the carved span.
    Before,
    /// Separator immediately after the carved span.
    After,
}

/// Removal span: `carved` extended by one adjacent horizontal-whitespace token
/// on `side` when that token is a direct child of `node`.
///
/// Only `WHITESPACE` / `INDENT` count - never `NEWLINE` (the line terminator
/// belongs to the construct, not the field). When no such separator exists,
/// returns `carved` unchanged.
#[must_use]
pub(crate) fn removal_extending(
    node: &SyntaxNode,
    carved: TextRange,
    side: RemovalSide,
) -> TextRange {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    match side {
        RemovalSide::Before => {
            let Some(idx) = kids
                .iter()
                .position(|e| e.text_range().start() == carved.start())
            else {
                return carved;
            };
            if idx == 0 {
                return carved;
            }
            let prev = &kids[idx - 1];
            if is_horizontal_ws(prev) {
                TextRange::new(prev.text_range().start(), carved.end())
            } else {
                carved
            }
        }
        RemovalSide::After => {
            let Some(idx) = kids
                .iter()
                .rposition(|e| e.text_range().end() == carved.end())
            else {
                return carved;
            };
            let Some(next) = kids.get(idx + 1) else {
                return carved;
            };
            if is_horizontal_ws(next) {
                TextRange::new(carved.start(), next.text_range().end())
            } else {
                carved
            }
        }
    }
}

/// Present part whose removal After-extends following horizontal whitespace.
#[must_use]
pub(crate) fn present_with_removal_after(node: &SyntaxNode, carved: TextRange) -> PartSpan {
    let removal = removal_extending(node, carved, RemovalSide::After);
    PartSpan::Present(PresentPart::new(carved, carved, removal))
}

/// Field-slot dest: Space lead unless a horizontal-ws token of `syntax`
/// ends at `offset`.
#[must_use]
pub(crate) fn field_dest(syntax: &SyntaxNode, offset: TextSize, trail: Padding) -> InsertPoint {
    let lead = if whitespace_ends_at(syntax, offset) {
        Padding::None
    } else {
        Padding::Space
    };
    InsertPoint::new(offset, lead, trail)
}

/// Present with After-removal, or Absent at `insert` (Space lead unless a
/// horizontal-ws token already ends there).
pub(crate) fn optional_after(
    syntax: &SyntaxNode,
    carved: Option<TextRange>,
    insert: TextSize,
) -> PartSpan {
    match carved {
        Some(r) => present_with_removal_after(syntax, r),
        None => PartSpan::Absent(field_dest(syntax, insert, Padding::None)),
    }
}

pub(crate) fn is_horizontal_ws(e: &SyntaxElement) -> bool {
    matches!(e.kind(), OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::INDENT)
}

/// Whether a direct-child horizontal-whitespace token of `node` ends at `offset`.
#[must_use]
pub(crate) fn whitespace_ends_at(node: &SyntaxNode, offset: TextSize) -> bool {
    node.children_with_tokens()
        .any(|e| is_horizontal_ws(&e) && e.text_range().end() == offset)
}

/// Padding a writer must emit when inserting at `offset` if that offset is
/// EOF and the file has no terminating newline.
///
/// Empty source: `None` (there is no last byte). Non-empty source whose last
/// token is not `NEWLINE`: `Newline`. Any other offset: `None`.
#[must_use]
pub(crate) fn eof_lead(root: &SyntaxNode, offset: TextSize) -> crate::ast::Padding {
    use crate::ast::Padding;
    if offset != root.text_range().end() || root.text_range().is_empty() {
        return Padding::None;
    }
    match root.last_token() {
        Some(t) if t.kind() == OrgSyntaxKind::NEWLINE => Padding::None,
        Some(_) => Padding::Newline,
        None => Padding::None,
    }
}

#[inline]
pub(crate) fn range_contains_range(outer: TextRange, inner: TextRange) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

pub(crate) fn token_text_eq_caseless(e: &SyntaxElement, want: &str) -> bool {
    e.as_token()
        .is_some_and(|t| t.kind() == OrgSyntaxKind::TEXT && t.text().eq_ignore_ascii_case(want))
}

/// Start of `elem`, or of an immediately preceding `INDENT` on the same
/// line. Does not walk through child nodes (a paragraph before `:END:`
/// must not pull the closer back onto the body line).
pub(crate) fn line_start_backing_indent(elem: &SyntaxElement) -> TextSize {
    match elem.prev_sibling_or_token() {
        Some(prev) if prev.kind() == OrgSyntaxKind::INDENT => prev.text_range().start(),
        _ => elem.text_range().start(),
    }
}

/// Start of the source line that contains `elem`, walking prev to a
/// `NEWLINE`. For token-only runs (latex `\end{...}`); not for a closer
/// that follows an element child.
pub(crate) fn line_start_of(elem: &SyntaxElement) -> TextSize {
    let mut cur = elem.clone();
    loop {
        match cur.prev_sibling_or_token() {
            Some(prev) if prev.kind() == OrgSyntaxKind::NEWLINE => {
                return cur.text_range().start();
            }
            Some(prev) => cur = prev,
            None => return cur.text_range().start(),
        }
    }
}

pub(crate) use crate::parser::grammar::forms::timestamp::is_weekday_text;

mod block;
mod citation;
mod clock;
mod drawer;
mod heading;
mod latex;
mod leaves;
mod link;
mod list;
mod planning;
mod table;
mod timestamp;

pub(crate) use block::*;
pub(crate) use citation::*;
pub(crate) use clock::*;
pub(crate) use drawer::*;
pub(crate) use heading::*;
pub(crate) use latex::*;
pub(crate) use leaves::*;
pub(crate) use link::*;
pub(crate) use list::*;
pub(crate) use planning::*;
pub(crate) use table::*;
pub(crate) use timestamp::*;

// ---------------------------------------------------------------------------
// Trailing blank trim
// ---------------------------------------------------------------------------

/// Direct children of `kids` split into source lines.
///
/// Each slice includes its terminating `NEWLINE` when present. A trailing run
/// with no terminator is the final unterminated line.
pub(crate) fn child_lines(kids: &[SyntaxElement]) -> Vec<&[SyntaxElement]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, e) in kids.iter().enumerate() {
        if e.kind() == OrgSyntaxKind::NEWLINE {
            lines.push(&kids[start..=i]);
            start = i + 1;
        }
    }
    if start < kids.len() {
        lines.push(&kids[start..]);
    }
    lines
}

pub(crate) fn trim_trailing_blank_lines(lines: &mut Vec<&[SyntaxElement]>) {
    while lines.len() > 1 && line_is_blank(lines[lines.len() - 1]) {
        lines.pop();
    }
}

fn line_is_blank(line: &[SyntaxElement]) -> bool {
    line.iter().all(|e| {
        matches!(
            e.kind(),
            OrgSyntaxKind::NEWLINE | OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::INDENT
        )
    })
}

/// Range of `node` with trailing blank lines stripped (post-blank).
///
/// The last non-blank line keeps its terminator. A node with no trailing
/// blank is unchanged.
#[must_use]
pub(crate) fn without_trailing_blank_lines(node: &SyntaxNode) -> TextRange {
    let kids: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut lines = child_lines(&kids);
    trim_trailing_blank_lines(&mut lines);
    span_of(lines.into_iter().flatten().cloned()).unwrap_or_else(|| node.text_range())
}

/// Whether `node` owns at least one trailing blank line after its last
/// non-blank line.
///
/// Line reconstruction over mixed children. A paragraph whose last object
/// is followed by the line terminator (`*bold*\n`) is not a trailing blank:
/// that `NEWLINE` closes the content line. List-final blanks are counted by
/// [`newlines_after_last_child`], not this helper.
#[must_use]
pub(crate) fn owns_trailing_blank_line(node: &SyntaxNode) -> bool {
    without_trailing_blank_lines(node).end() < node.text_range().end()
}

/// Direct-child `NEWLINE` tokens after the last child node.
///
/// List-final blanks on `PLAIN_LIST` live here (the last `ITEM` already owns
/// its line terminator).
#[must_use]
pub(crate) fn newlines_after_last_child(node: &SyntaxNode) -> usize {
    let Some(last) = node.children().last() else {
        return 0;
    };
    let after = last.text_range().end();
    node.children_with_tokens()
        .filter(|e| {
            e.as_token().is_some()
                && e.kind() == OrgSyntaxKind::NEWLINE
                && e.text_range().start() >= after
        })
        .count()
}

#[cfg(test)]
pub(crate) fn first_kind(src: &str, kind: OrgSyntaxKind) -> SyntaxNode {
    let doc = crate::parse(src);
    crate::tests::nodes_of_kind(doc.root(), kind)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no {kind:?} in {src:?}"))
}

/// Source text of an element run. Layout tests assert reconstructed text,
/// never emptiness: a run primitive that returns the wrong children is
/// still non-empty.
#[cfg(test)]
pub(crate) fn run_text(run: &[SyntaxElement]) -> String {
    run.iter().map(std::string::ToString::to_string).collect()
}

/// Build a node the grammar cannot emit, to reach a slicer's shape guard.
///
/// Slicers guard shapes the parser already checks, so a
/// degenerate fixture cannot come from `parse`. Synthesising the green node
/// is the only way to execute the guard, and it stays confined to these
/// tests.
#[cfg(test)]
pub(crate) fn synth(kind: OrgSyntaxKind, tokens: &[(OrgSyntaxKind, &str)]) -> SyntaxNode {
    use rowan::Language;
    let mut builder = rowan::GreenNodeBuilder::new();
    builder.start_node(crate::OrgLanguage::kind_to_raw(kind));
    for &(k, text) in tokens {
        builder.token(crate::OrgLanguage::kind_to_raw(k), text);
    }
    builder.finish_node();
    SyntaxNode::new_root(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    use crate::{TextRange, TextSize};

    #[test]
    fn between_missing_marker_is_none() {
        let node = first_kind("*bold*\n", OrgSyntaxKind::BOLD);
        let body = between(&node, OrgSyntaxKind::STAR, OrgSyntaxKind::STAR).expect("body");
        let doc = parse("*bold*\n");
        assert_eq!(&doc.source()[body], "bold");
        assert_eq!(
            between(&node, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET),
            None
        );
    }
    #[test]
    fn between_never_walks_outside_node() {
        // Markers present later in the document but not inside this paragraph.
        let node = first_kind("plain\n\n*bold*\n", OrgSyntaxKind::PARAGRAPH);
        assert_eq!(
            between(&node, OrgSyntaxKind::STAR, OrgSyntaxKind::STAR),
            None
        );
    }
    #[test]
    fn between_empty_interior_is_zero_width_at_closer() {
        // Grammar does not emit empty `<<>>` / `****`; synthesise adjacent markers.
        let empty = synth(
            OrgSyntaxKind::BOLD,
            &[(OrgSyntaxKind::STAR, "*"), (OrgSyntaxKind::STAR, "*")],
        );
        let body = between(&empty, OrgSyntaxKind::STAR, OrgSyntaxKind::STAR).expect("empty body");
        assert!(body.is_empty());
        assert_eq!(body.start(), TextSize::from(1));
        assert_eq!(span_of(std::iter::empty::<SyntaxElement>()), None);
    }
    #[test]
    fn between_missing_markers_is_none() {
        let node = first_kind("plain\n", OrgSyntaxKind::PARAGRAPH);
        assert_eq!(
            between(&node, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET),
            None
        );
    }
    #[test]
    fn delimited_interior_uses_last_closer_unlike_between() {
        let src = "#+CAPTION[a][b]: v\n";
        let dual = first_kind(src, OrgSyntaxKind::KEYWORD_DUAL);
        let last = delimited_interior(&dual, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET)
            .expect("pair");
        let first = between(&dual, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET)
            .expect("first closer");
        assert_eq!(&src[last], "a][b");
        assert_eq!(&src[first], "a");
        assert!(first.end() < last.end());
    }
    #[test]
    fn delimited_interior_empty_is_zero_width_at_closer() {
        let empty = synth(
            OrgSyntaxKind::KEYWORD_DUAL,
            &[
                (OrgSyntaxKind::L_BRACKET, "["),
                (OrgSyntaxKind::R_BRACKET, "]"),
            ],
        );
        let inner = delimited_interior(&empty, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET)
            .expect("empty pair");
        assert!(inner.is_empty());
        assert_eq!(inner.start(), TextSize::from(1));
    }
    #[test]
    fn delimited_interior_parens_and_missing_markers() {
        let src = "#+CALL: n(a,b)\n";
        let args = first_kind(src, OrgSyntaxKind::CALL_ARGUMENTS);
        let inner = delimited_interior(&args, OrgSyntaxKind::L_PAREN, OrgSyntaxKind::R_PAREN)
            .expect("args");
        assert_eq!(&src[inner], "a,b");

        let node = first_kind("plain\n", OrgSyntaxKind::PARAGRAPH);
        assert_eq!(
            delimited_interior(&node, OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::R_BRACKET),
            None
        );
        let open_only = synth(
            OrgSyntaxKind::KEYWORD_DUAL,
            &[(OrgSyntaxKind::L_BRACKET, "[")],
        );
        assert_eq!(
            delimited_interior(
                &open_only,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET
            ),
            None
        );
    }
    #[test]
    fn line_partition_single_line_heading() {
        let node = first_kind("* Title\n", OrgSyntaxKind::HEADING);
        let p = line_partition(&node);
        assert_eq!(run_text(&p.first_line), "* Title\n");
        assert_eq!(run_text(&p.body), "");
        // A single-line node has no distinct last line.
        assert_eq!(run_text(&p.last_line), "");
    }
    #[test]
    fn line_partition_block_three_way() {
        let node = first_kind(
            "#+begin_src rust\nlet x = 1;\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        let p = line_partition(&node);
        // The end line is the last line - not the trailing newline alone.
        assert_eq!(run_text(&p.first_line), "#+begin_src rust\n");
        assert_eq!(run_text(&p.body), "let x = 1;\n");
        assert_eq!(run_text(&p.last_line), "#+end_src\n");
    }
    #[test]
    fn line_partition_two_lines_has_no_body() {
        let node = first_kind("a\nb\n", OrgSyntaxKind::PARAGRAPH);
        let p = line_partition(&node);
        assert_eq!(run_text(&p.first_line), "a\n");
        assert_eq!(run_text(&p.body), "");
        assert_eq!(run_text(&p.last_line), "b\n");
    }
    #[test]
    fn line_partition_unterminated_last_line() {
        let node = first_kind("a\nb", OrgSyntaxKind::PARAGRAPH);
        let p = line_partition(&node);
        assert_eq!(run_text(&p.first_line), "a\n");
        assert_eq!(run_text(&p.body), "");
        assert_eq!(run_text(&p.last_line), "b");
    }
    #[test]
    fn line_partition_no_newline() {
        let node = first_kind("no trailing newline", OrgSyntaxKind::PARAGRAPH);
        let p = line_partition(&node);
        assert_eq!(run_text(&p.first_line), "no trailing newline");
        assert_eq!(run_text(&p.body), "");
        assert_eq!(run_text(&p.last_line), "");
    }
    #[test]
    fn partition_body_range_empty_quote_is_zero_width() {
        let node = first_kind("#+begin_quote\n#+end_quote\n", OrgSyntaxKind::QUOTE_BLOCK);
        let p = line_partition(&node);
        assert_eq!(run_text(&p.body), "");
        let body = partition_body_range(&p).expect("empty body slot");
        assert!(body.is_empty());
        assert_eq!(
            body.start(),
            p.first_line.last().unwrap().text_range().end()
        );
    }
    #[test]
    fn partition_body_range_single_line_is_none() {
        let node = first_kind("* Title\n", OrgSyntaxKind::HEADING);
        assert!(partition_body_range(&line_partition(&node)).is_none());
    }
    #[test]
    fn line_partition_verse_block_end_line() {
        let node = first_kind(
            "#+begin_verse\na\nb\n#+end_verse\n",
            OrgSyntaxKind::VERSE_BLOCK,
        );
        let p = line_partition(&node);
        assert_eq!(run_text(&p.first_line), "#+begin_verse\n");
        assert_eq!(run_text(&p.body), "a\nb\n");
        assert_eq!(run_text(&p.last_line), "#+end_verse\n");
    }
    #[test]
    fn content_of_strips_trailing_whitespace_in_title() {
        let title = first_kind("* Title :tag:\n", OrgSyntaxKind::HEADING_TITLE);
        let full = title.text_range();
        let content = content_of(&title);
        let src = parse("* Title :tag:\n");
        let full_text = &src.source()[full];
        let content_text = &src.source()[content];
        assert!(full_text.ends_with(' '), "fixture shape: {full_text:?}");
        assert_eq!(content_text, "Title");
    }
    #[test]
    fn content_of_title_without_tags_is_full() {
        let title = first_kind("* just a title\n", OrgSyntaxKind::HEADING_TITLE);
        assert_eq!(content_of(&title), title.text_range());
    }
    #[test]
    fn content_of_all_whitespace_is_empty_at_start() {
        let title = first_kind("* TODO \n", OrgSyntaxKind::HEADING_TITLE);
        let c = content_of(&title);
        assert!(c.is_empty());
        assert_eq!(c.start(), title.text_range().start());
    }
    #[test]
    fn removal_extends_after_todo() {
        let heading = first_kind("* TODO Title\n", OrgSyntaxKind::HEADING);
        let todo = heading
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::TODO_KEYWORD)
            .expect("TODO");
        let carved = todo.text_range();
        let removal = removal_extending(&heading, carved, RemovalSide::After);
        assert!(removal.end() > carved.end());
        assert_eq!(removal.start(), carved.start());
    }
    #[test]
    fn removal_does_not_swallow_newline() {
        let heading = first_kind("* T :a:\n", OrgSyntaxKind::HEADING);
        let tags = heading
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::TAGS)
            .expect("TAGS");
        let carved = tags.text_range();
        let removal = removal_extending(&heading, carved, RemovalSide::After);
        assert_eq!(removal, carved, "NEWLINE must not join a field removal");
    }
    #[test]
    fn synthesised_nodes_carry_absolute_ranges_from_zero() {
        // Guards the fixtures themselves: a synthesised root starts at 0, so a
        // slicer that returned offsets relative to the wrong base would still
        // look right here. Pin the happy path through a synthesised node
        // against the parsed one.
        let parsed = first_kind("<2024-06-15>\n", OrgSyntaxKind::DATE);
        let parsed_fields = date_fields(&parsed).expect("parsed date");
        assert_eq!(parsed.text_range().start(), TextSize::from(1));
        assert_eq!(parsed_fields.year.start(), TextSize::from(1));

        let made = synth(OrgSyntaxKind::DATE, &[(OrgSyntaxKind::TEXT, "2024-06-15")]);
        let made_fields = date_fields(&made).expect("synth date");
        assert_eq!(made.text_range().start(), TextSize::from(0));
        assert_eq!(made_fields.year.start(), TextSize::from(0));
        assert_eq!(u32::from(made_fields.day.start()), 8);
    }
    #[test]
    fn span_of_empty_is_none() {
        assert_eq!(span_of(std::iter::empty()), None);
    }
    #[test]
    fn first_token_of_kind_finds_star() {
        let h = first_kind("* T\n", OrgSyntaxKind::HEADING);
        let t = first_token_of_kind(&h, OrgSyntaxKind::STAR).expect("star");
        assert_eq!(t.text(), "*");
    }
    #[test]
    fn first_token_of_kind_missing_is_none() {
        let h = first_kind("* T\n", OrgSyntaxKind::HEADING);
        assert!(first_token_of_kind(&h, OrgSyntaxKind::PIPE).is_none());
    }
    #[test]
    fn first_word_range_skips_leading_ws() {
        let title = first_kind("*   WAITING Title\n", OrgSyntaxKind::HEADING_TITLE);
        let r = first_word_range(&title).expect("word");
        let doc = parse("*   WAITING Title\n");
        assert_eq!(&doc.source()[r], "WAITING");
    }
    #[test]
    fn first_word_range_empty_title_is_none() {
        let title = first_kind("* \n", OrgSyntaxKind::HEADING_TITLE);
        assert!(first_word_range(&title).is_none());
    }
    #[test]
    fn with_node_text_borrows_a_single_token() {
        let todo = first_kind("* TODO Title\n", OrgSyntaxKind::TODO_KEYWORD);
        with_node_text(&todo, |s| assert_eq!(s, "TODO"));
        assert!(
            single_child_token(&todo).is_some(),
            "TODO_KEYWORD is one token so the lookup must not allocate"
        );
    }
    #[test]
    fn children_after_last_node_anchor() {
        let h = first_kind("* Title :a:\n", OrgSyntaxKind::HEADING);
        let after: Vec<_> = children_after_last(&h, OrgSyntaxKind::HEADING_TITLE)
            .expect("title anchor")
            .collect();
        assert_eq!(run_text(&after), ":a:\n");
    }
    #[test]
    fn children_after_last_token_anchor() {
        // The star run is a token run, not a carved node. A node-only lookup
        // would silently return nothing here.
        let h = first_kind("** TODO Title\n", OrgSyntaxKind::HEADING);
        let after: Vec<_> = children_after_last(&h, OrgSyntaxKind::STAR)
            .expect("star anchor")
            .collect();
        assert_eq!(run_text(&after), " TODO Title\n");
    }
    #[test]
    fn children_after_last_missing_anchor_is_none() {
        // Missing anchor is None, never an empty iterator standing in for one.
        let h = first_kind("* T\n", OrgSyntaxKind::HEADING);
        assert!(children_after_last(&h, OrgSyntaxKind::PLANNING).is_none());
        assert!(children_after_last(&h, OrgSyntaxKind::PIPE).is_none());
    }
    #[test]
    fn children_after_last_present_anchor_with_nothing_after_is_empty_some() {
        // Anchor present, nothing after it: Some(empty), not None.
        let h = first_kind("* T\n", OrgSyntaxKind::HEADING);
        let after: Vec<_> = children_after_last(&h, OrgSyntaxKind::NEWLINE)
            .expect("trailing newline is present")
            .collect();
        assert!(
            after.is_empty(),
            "nothing follows the terminator: {after:?}"
        );
    }
    #[test]
    fn content_of_range_matches_content_of_on_full() {
        let title = first_kind("* Title :tag:\n", OrgSyntaxKind::HEADING_TITLE);
        assert_eq!(
            content_of_range(&title, title.text_range()),
            content_of(&title)
        );
    }
    #[test]
    fn content_of_range_on_own_node() {
        let title = first_kind("* Title :tag:\n", OrgSyntaxKind::HEADING_TITLE);
        let full = title.text_range();
        let doc = parse("* Title :tag:\n");
        let word = content_of(&title);
        assert_eq!(&doc.source()[word], "Title");
        // Carved = full title (includes trailing space); content trims it.
        assert_eq!(content_of_range(&title, full), word);
        // Carved already content-trimmed: no further trim.
        assert_eq!(content_of_range(&title, word), word);
    }
    #[test]
    fn content_of_range_from_parent_descends_into_the_child() {
        // The title's separator before TAGS is a child of the *title*, so a
        // trim that only looks at heading-direct children returns the carve
        // untouched and a title replacement overwrites the separator.
        let src = "* T Title :a:\n";
        let doc = parse(src);
        let heading = first_kind(src, OrgSyntaxKind::HEADING);
        let title = heading
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::HEADING_TITLE)
            .expect("title");
        let carved = title.text_range();
        assert_eq!(&doc.source()[carved], "T Title ", "fixture shape");

        let from_parent = content_of_range(&heading, carved);
        assert_eq!(&doc.source()[from_parent], "T Title");
        assert_eq!(
            from_parent,
            content_of(&title),
            "parent carve must agree with the child's own content span"
        );
    }
    #[test]
    fn first_line_stops_at_newline() {
        let node = first_kind(
            "#+begin_src rust\nlet x = 1;\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        let line: Vec<_> = first_line(&node).collect();
        assert!(line.iter().any(|e| e.kind() == OrgSyntaxKind::NEWLINE));
        // Body token "let" must not appear in first_line.
        let texts: Vec<_> = line
            .iter()
            .filter_map(|e| e.as_token().map(|t| t.text().to_string()))
            .collect();
        assert!(
            !texts.iter().any(|t| t == "let"),
            "first_line leaked body: {texts:?}"
        );
    }
    #[test]
    fn removal_extends_before() {
        let heading = first_kind("* TODO Title\n", OrgSyntaxKind::HEADING);
        let todo = heading
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::TODO_KEYWORD)
            .expect("TODO");
        let carved = todo.text_range();
        let removal = removal_extending(&heading, carved, RemovalSide::Before);
        // Space after stars precedes TODO.
        assert!(removal.start() < carved.start());
        assert_eq!(removal.end(), carved.end());
    }
    #[test]
    fn removal_unknown_carved_returns_carved() {
        let heading = first_kind("* TODO Title\n", OrgSyntaxKind::HEADING);
        let bogus = TextRange::new(TextSize::from(999), TextSize::from(1000));
        assert_eq!(
            removal_extending(&heading, bogus, RemovalSide::After),
            bogus
        );
    }
    #[test]
    fn tokens_of_kind_lists_stars() {
        let h = first_kind("** T\n", OrgSyntaxKind::HEADING);
        let stars: Vec<_> = tokens(&h, OrgSyntaxKind::STAR).collect();
        assert_eq!(stars.len(), 2);
    }
    #[test]
    fn without_trailing_blank_lines_keeps_content_terminator() {
        let node = first_kind("hello\n\n", OrgSyntaxKind::PARAGRAPH);
        let doc = parse("hello\n\n");
        assert_eq!(
            &doc.source()[without_trailing_blank_lines(&node)],
            "hello\n"
        );
        assert!(owns_trailing_blank_line(&node));

        let node = first_kind("hello\n", OrgSyntaxKind::PARAGRAPH);
        assert!(!owns_trailing_blank_line(&node));
        assert_eq!(without_trailing_blank_lines(&node), node.text_range());

        let node = first_kind("*bold*\n", OrgSyntaxKind::PARAGRAPH);
        assert!(!owns_trailing_blank_line(&node));
        let node = first_kind("hello *bold*\n", OrgSyntaxKind::PARAGRAPH);
        assert!(!owns_trailing_blank_line(&node));
        let node = first_kind("*bold*\n\n", OrgSyntaxKind::PARAGRAPH);
        assert!(owns_trailing_blank_line(&node));
    }
    #[test]
    fn newlines_after_last_child_counts_list_final_blanks() {
        let list = first_kind("- a\n", OrgSyntaxKind::PLAIN_LIST);
        assert_eq!(newlines_after_last_child(&list), 0);

        let list = first_kind("- a\n\n", OrgSyntaxKind::PLAIN_LIST);
        assert_eq!(newlines_after_last_child(&list), 1);
    }
    #[test]
    fn last_token_of_kind_stars() {
        let h = first_kind("** T\n", OrgSyntaxKind::HEADING);
        let last = last_token_of_kind(&h, OrgSyntaxKind::STAR).expect("star");
        let first = first_token_of_kind(&h, OrgSyntaxKind::STAR).expect("star");
        assert!(last.text_range().start() > first.text_range().start());
        assert_eq!(last.text(), "*");
    }
}
