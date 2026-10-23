//! Typed views over the Org CST.
//!
//! A view is a zero-cost newtype over a rowan red node. It answers questions
//! about one construct in Org terms (its parts, its contents, where new text
//! goes) by reading the tree, never by re-parsing source text.
//!
//! This module holds what every view shares: the [`AstNode`] trait and the
//! `ast_node!` macro, the editing geometry ([`PartSpan`],
//! [`PresentPart`], [`InsertPoint`], [`SplitPoint`]), the capability traits
//! ([`HasParts`], [`ElementView`], [`HasElements`], [`HasObjects`]) and the
//! unions [`Element`], [`Object`] and [`Container`] that container walks
//! yield. A part is *carved* when the parser gave it its own node or token;
//! views locate parts through carved syntax only.
//!
//! Queries that need the whole snapshot (covering, [`EditContext`], TODO and
//! tag joins, discovery by ID or name, in-buffer settings) are methods on
//! [`crate::Document`]. [`PortableDocument`] moves a snapshot across threads.

mod context;
mod document;
mod layout;
pub mod nav;
mod portable;
mod support;
mod views;

#[cfg(test)]
mod container_matrix;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use layout::with_node_text;
use layout::{eof_lead, newlines_after_last_child, owns_trailing_blank_line};
use support::{
    affiliated_extent, dest, element_children, is_body_element, is_container_kind, is_element_kind,
    is_object_kind, next_element_sibling, prev_element_sibling, syntax_root,
};
pub(crate) use support::{token_at_caret, token_owning_byte};

pub use context::{ContextFrame, EditContext};
pub use document::{
    DeclaredTags, EntryFact, EntryFactValue, EntryFacts, Footnote, HeaderArgsOrigin,
    InternalAddress, LinkAbbrev, MacroDef, Property, PropertyOrigin, TagGroup, TagGroupKind,
};
pub use portable::PortableDocument;
pub use views::{
    AffiliatedKeywords, BabelCall, BabelCallPart, Block, BlockBody, BlockKind, BlockPart, Bullet,
    CellAddr, CellFormula, CellFormulaKind, CheckboxState, Citation, CitationPart,
    CitationReference, CitationReferencePart, ClassifiedLink, Clock, ClockPart, Comment,
    CookieData, CookieShape, CookieValue, Counter, CounterForm, Delay, DelayKind, DiarySexp,
    Drawer, DrawerPart, Emphasis, Entity, ExportSnippet, ExportSnippetPart, FixedWidth,
    FootnoteDefinition, FootnotePart, FootnoteReference, FootnoteReferencePart, FootnoteShape,
    Heading, HeadingPart, HeadingTitle, Headline, HorizontalRule, InlineBabelCall,
    InlineBabelCallPart, InlineFootnote, InlineSrcBlock, InlineSrcPart, InlineTask, Item, ItemPart,
    ItemTag, Keyword, KeywordDual, KeywordPart, KeywordRole, KeywordValue, LatexEnvironment,
    LatexEnvironmentPart, LatexFragment, LatexFragmentShape, LineBreak, Link, LinkDescription,
    LinkPart, LinkShape, LinkTarget, ListKind, Macro, Markup, MarkupBody, MarkupKind, MarkupPart,
    NodeProperty, Outline, Paragraph, PlainList, Planning, PlanningPart, Priority, PropertyDrawer,
    PropertyPart, RadioTarget, Repeater, RepeaterKind, RepeaterUnit, ScriptPart, Section,
    StatisticsCookie, Subscript, Superscript, Table, TableCell, TableKind, TableRow, Tag,
    TagOrigin, Tags, Target, Task, Timestamp, TimestampDelimiters, TimestampField, TimestampPart,
    TimestampPoint, TimestampShape, TodoKeyword, VerseBody, classify_link, escape_link_path,
    normalize_angle_link_ws, normalize_bracket_link_ws, unescape_link_path,
};

use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{TextRange, TextSize};
use std::fmt;
use std::hash::Hash;

// ---------------------------------------------------------------------------
// AstNode and its macro
// ---------------------------------------------------------------------------

/// Zero-cost typed projection over a red node.
///
/// Bespoke rather than `rowan::ast::AstNode`: there is one language, no
/// mutable-tree API, and [`Self::can_cast`] takes a plain [`OrgSyntaxKind`].
///
/// [`crate::Document`] is not an `AstNode`: it owns the source, the
/// configuration and the diagnostics, which a bare node cannot recover.
pub trait AstNode: Clone + Sized {
    /// Whether a node of `kind` may be viewed as `Self`.
    ///
    /// This tests the kind alone, which is what enum dispatch needs, so it is
    /// necessary but not sufficient. A view over a kind that plays more than
    /// one role can accept the kind here and still refuse the node in
    /// [`Self::cast`]: [`Element`] does so for the `PROPERTY_DRAWER` directly
    /// under an `OUTLINE`. Call `cast` and match on the result rather than
    /// writing `can_cast(k).then(|| cast(n).unwrap())`.
    fn can_cast(kind: OrgSyntaxKind) -> bool
    where
        Self: Sized;

    /// Build a view from `node`. The only public constructor.
    ///
    /// Returns `Some` when the kind is accepted **and** any context predicate
    /// the view carries holds. The guarantee generic code may rely on is the
    /// one direction: `cast(n).is_some()` implies `can_cast(n.kind())`. The
    /// converse does not hold; see [`Self::can_cast`].
    fn cast(node: SyntaxNode) -> Option<Self>
    where
        Self: Sized;

    /// Escape hatch to the untyped red node.
    fn syntax(&self) -> &SyntaxNode;

    /// Syntax kind of the wrapped node.
    #[inline]
    #[must_use]
    fn kind(&self) -> OrgSyntaxKind {
        self.syntax().kind()
    }

    /// Full syntactic extent, delimiters included.
    #[inline]
    #[must_use]
    fn range(&self) -> TextRange {
        self.syntax().text_range()
    }

    /// Source text of this node, byte for byte.
    #[inline]
    #[must_use]
    fn text(&self) -> rowan::SyntaxText {
        self.syntax().text()
    }
}

/// How [`crate::Document::covering`] finds and measures a view of this type.
///
/// By default the walk starts at the node it is given and tests
/// [`AstNode::range`]. [`Element`] starts past an affiliated-keyword run and
/// tests [`ElementView::full_range`], so a `#+CAPTION:` line counts as part of
/// the element it captions.
pub trait Covering: AstNode {
    /// Node the covering walk starts from. Default is `start` itself.
    #[must_use]
    fn covering_seed(start: &SyntaxNode) -> SyntaxNode {
        start.clone()
    }

    /// Span that must contain the query range. Default is [`AstNode::range`].
    fn covering_span(node: &Self) -> TextRange {
        node.range()
    }
}

/// Mint a zero-cost [`AstNode`] newtype over one or more syntax kinds.
///
/// `ast_node!(Name, KIND)` accepts one kind; `ast_node!(Name, [A, B])` accepts
/// any of a list. The list arm comes before the single-kind arm because
/// `[A, B]` is also a valid `expr`. Doc comments written before `Name` go on
/// the generated struct.
macro_rules! ast_node {
    (@impl $(#[$meta:meta])* $name:ident, |$k:ident| $pred:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name($crate::SyntaxNode);

        impl $crate::ast::AstNode for $name {
            #[inline]
            fn can_cast($k: $crate::OrgSyntaxKind) -> bool {
                $pred
            }

            #[inline]
            fn cast(node: $crate::SyntaxNode) -> Option<Self> {
                Self::can_cast(node.kind()).then(|| Self(node))
            }

            #[inline]
            fn syntax(&self) -> &$crate::SyntaxNode {
                &self.0
            }
        }

        impl $crate::ast::Covering for $name {}
    };
    // Multi-kind: comma list becomes a `matches!` alternation.
    ($(#[$meta:meta])* $name:ident, [ $($kind:path),+ $(,)? ]) => {
        $crate::ast_node!(@impl $(#[$meta])* $name, |k| matches!(k, $($kind)|+));
    };
    // Single kind.
    ($(#[$meta:meta])* $name:ident, $kind:expr) => {
        $crate::ast_node!(@impl $(#[$meta])* $name, |k| k == $kind);
    };
}
pub(crate) use ast_node;

/// Leaf [`ElementView`]: fills `content_range` (`None`) and `is_greater` (`false`).
macro_rules! impl_leaf_element_view {
    ($($name:ident),+ $(,)?) => {
        $(
            impl $crate::ast::ElementView for $name {
                fn content_range(&self) -> Option<$crate::TextRange> {
                    None
                }

                fn is_greater(&self) -> bool {
                    false
                }
            }
        )+
    };
}
pub(crate) use impl_leaf_element_view;

// ---------------------------------------------------------------------------
// Editing geometry
// ---------------------------------------------------------------------------

/// Separation a writer must emit **in addition to** bytes already present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Padding {
    /// No extra separator.
    None,
    /// A single ASCII space.
    Space,
    /// A line terminator.
    Newline,
    /// A blank line: the neighbour would otherwise absorb the new material.
    BlankLine,
}

/// Where text that is not in the document yet is written.
///
/// An insert point is a byte offset plus the padding a writer must emit on
/// each side so the new text parses as intended. Only this crate constructs
/// insert points, from the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InsertPoint {
    offset: TextSize,
    lead: Padding,
    trail: Padding,
}

impl InsertPoint {
    /// Construct an insert point.
    #[inline]
    #[must_use]
    pub(crate) const fn new(offset: TextSize, lead: Padding, trail: Padding) -> Self {
        Self {
            offset,
            lead,
            trail,
        }
    }

    /// Absolute byte offset where insertion begins.
    #[inline]
    #[must_use]
    pub const fn offset(self) -> TextSize {
        self.offset
    }

    /// Padding written before the new construct.
    #[inline]
    #[must_use]
    pub const fn lead(self) -> Padding {
        self.lead
    }

    /// Padding written after the new construct.
    #[inline]
    #[must_use]
    pub const fn trail(self) -> Padding {
        self.trail
    }
}

/// Cut through a present part: keep the left, move the right to a sibling.
///
/// Like [`InsertPoint`], only this crate constructs split points. The moved
/// half is named `moved` because `move` is a keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SplitPoint {
    keep: TextRange,
    moved: TextRange,
    dest: InsertPoint,
}

impl SplitPoint {
    /// Construct a split point.
    #[inline]
    #[must_use]
    pub(crate) const fn new(keep: TextRange, moved: TextRange, dest: InsertPoint) -> Self {
        Self { keep, moved, dest }
    }

    /// Span that stays in place.
    #[inline]
    #[must_use]
    pub const fn keep(self) -> TextRange {
        self.keep
    }

    /// Span copied into the new sibling and then deleted.
    #[inline]
    #[must_use]
    pub const fn moved(self) -> TextRange {
        self.moved
    }

    /// Where the sibling is written.
    #[inline]
    #[must_use]
    pub const fn dest(self) -> InsertPoint {
        self.dest
    }
}

/// Cut `content` at `offset`: keep `[start, offset)`, moved `[offset, end)`.
///
/// `None` when `offset` is outside the half-open span `[start, end)`. Offset
/// at `start` yields an empty keep.
#[must_use]
pub(crate) fn split_span(
    content: TextRange,
    offset: TextSize,
    dest: InsertPoint,
) -> Option<SplitPoint> {
    if offset < content.start() || offset >= content.end() {
        return None;
    }
    Some(SplitPoint::new(
        TextRange::new(content.start(), offset),
        TextRange::new(offset, content.end()),
        dest,
    ))
}

/// The three spans of a part that is present in the source.
///
/// [`Self::range`] is what the part occupies, [`Self::content`] is what a
/// replacement overwrites, and [`Self::removal`] is what a delete removes.
/// Like [`InsertPoint`], only this crate constructs present parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentPart {
    range: TextRange,
    content: TextRange,
    removal: TextRange,
}

impl PresentPart {
    /// Construct a present part.
    #[inline]
    #[must_use]
    pub(crate) const fn new(range: TextRange, content: TextRange, removal: TextRange) -> Self {
        Self {
            range,
            content,
            removal,
        }
    }

    /// Convenience when carved, content and removal coincide.
    #[inline]
    #[must_use]
    pub(crate) const fn identical(range: TextRange) -> Self {
        Self::new(range, range, range)
    }

    /// Span the part occupies, delimiters included.
    #[inline]
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }

    /// Span a replacement overwrites.
    #[inline]
    #[must_use]
    pub const fn content(self) -> TextRange {
        self.content
    }

    /// Span a delete removes, including the separator that exists only
    /// because this part is present.
    #[inline]
    #[must_use]
    pub const fn removal(self) -> TextRange {
        self.removal
    }
}

/// Where a part is, or where it would go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartSpan {
    /// The part is present; see [`PresentPart`] for its spans.
    Present(PresentPart),
    /// The part is absent; its text is written at this insert point.
    Absent(InsertPoint),
    /// The part cannot exist in this construct's current shape (for example a
    /// plain link has no description slot until its shape changes).
    NotApplicable,
}

// ---------------------------------------------------------------------------
// Capability traits
// ---------------------------------------------------------------------------

/// A construct with addressable parts.
///
/// Implementors provide [`Self::part_span`]; the other accessors derive from
/// it.
pub trait HasParts: AstNode {
    /// Closed part space for this construct.
    type Part: Copy + Eq + Hash + fmt::Debug + 'static;

    /// Every part this construct can have.
    ///
    /// The order need not be source order: [`crate::Planning`] lists its
    /// keywords in a fixed order whatever order the source writes them in.
    const PARTS: &'static [Self::Part];

    /// Which present part contains `offset`.
    ///
    /// Where one part ends and the next starts at `offset`, the part that ends
    /// wins. Otherwise a part's exclusive end is outside it, so an offset in a
    /// gap between parts, or just past the last part, yields `None`.
    #[inline]
    #[must_use]
    fn part_at(&self, offset: TextSize) -> Option<Self::Part> {
        left_biased_part_at(self, offset)
    }

    /// Where `part` is, or where it would be written.
    fn part_span(&self, part: Self::Part) -> PartSpan;

    /// Carved range when the part is present.
    #[inline]
    #[must_use]
    fn part_range(&self, part: Self::Part) -> Option<TextRange> {
        match self.part_span(part) {
            PartSpan::Present(p) => Some(p.range()),
            PartSpan::Absent(_) | PartSpan::NotApplicable => None,
        }
    }

    /// Content range when the part is present.
    #[inline]
    #[must_use]
    fn part_content_range(&self, part: Self::Part) -> Option<TextRange> {
        match self.part_span(part) {
            PartSpan::Present(p) => Some(p.content()),
            PartSpan::Absent(_) | PartSpan::NotApplicable => None,
        }
    }

    /// Removal range when the part is present.
    #[inline]
    #[must_use]
    fn part_removal_range(&self, part: Self::Part) -> Option<TextRange> {
        match self.part_span(part) {
            PartSpan::Present(p) => Some(p.removal()),
            PartSpan::Absent(_) | PartSpan::NotApplicable => None,
        }
    }

    /// Insert point when the part is absent.
    #[inline]
    #[must_use]
    fn part_insert_point(&self, part: Self::Part) -> Option<InsertPoint> {
        match self.part_span(part) {
            PartSpan::Absent(ip) => Some(ip),
            PartSpan::Present(_) | PartSpan::NotApplicable => None,
        }
    }

    /// Every part paired with its span, in [`Self::PARTS`] order.
    #[inline]
    fn part_spans(&self) -> impl Iterator<Item = (Self::Part, PartSpan)> + '_ {
        Self::PARTS
            .iter()
            .copied()
            .map(move |part| (part, self.part_span(part)))
    }
}

/// A view that can appear as an element child of a container.
///
/// Extents differ from org-element's. The parser makes a run of affiliated
/// keywords (`#+CAPTION:`, `#+NAME:`, ...) a preceding sibling of the element
/// rather than part of it, so [`AstNode::range`] covers the element proper:
/// `range().start()` is org-element's `:post-affiliated`, and
/// [`Self::full_range`] is `:begin`..`:end`.
pub trait ElementView: AstNode {
    /// The run of affiliated keywords attached to this element, if any.
    #[inline]
    #[must_use]
    fn affiliated(&self) -> Option<AffiliatedKeywords> {
        self.syntax()
            .prev_sibling()
            .and_then(AffiliatedKeywords::cast)
    }

    /// Org `:begin`..`:end`: the affiliated run plus the element.
    #[inline]
    #[must_use]
    fn full_range(&self) -> TextRange {
        affiliated_extent(self.syntax())
    }

    /// Org `:contents-begin`..`:contents-end`. `None` for a leaf or an opaque
    /// body.
    fn content_range(&self) -> Option<TextRange>;

    /// Whether this element holds other elements (org greater-element test).
    fn is_greater(&self) -> bool;

    /// Next sibling element in the same container (affiliated runs skipped).
    #[inline]
    #[must_use]
    fn next_element(&self) -> Option<Element> {
        next_element_sibling(self.syntax()).and_then(Element::cast)
    }

    /// Previous sibling element in the same container (affiliated runs skipped).
    #[inline]
    #[must_use]
    fn prev_element(&self) -> Option<Element> {
        prev_element_sibling(self.syntax()).and_then(Element::cast)
    }

    /// The container this element sits in, if any.
    #[inline]
    #[must_use]
    fn container(&self) -> Option<Container> {
        self.syntax().parent().and_then(|p| {
            if is_container_kind(p.kind()) {
                Container::cast(p)
            } else {
                None
            }
        })
    }
}

/// A view with element children.
pub trait HasElements: AstNode {
    /// Direct element children in source order.
    ///
    /// Affiliated-keyword runs are skipped: each belongs to the element after
    /// it.
    #[inline]
    fn elements(&self) -> impl Iterator<Item = Element> + '_ {
        element_children(self.syntax()).filter_map(Element::cast)
    }

    /// First direct element child.
    #[inline]
    #[must_use]
    fn first_element(&self) -> Option<Element> {
        self.elements().next()
    }

    /// Last direct element child.
    ///
    /// Defined through [`Self::elements`], so it agrees with an implementor
    /// that narrows that walk.
    #[inline]
    #[must_use]
    fn last_element(&self) -> Option<Element> {
        self.elements().last()
    }

    /// The direct child element containing `offset`, if any.
    ///
    /// Where one element ends and the next starts at `offset`, the one that
    /// ends wins. Unlike [`HasParts::part_at`], the start of the first element
    /// and the end of the last both still hit.
    #[inline]
    #[must_use]
    fn element_at(&self, offset: TextSize) -> Option<Element> {
        left_biased_child_at(self.elements(), offset)
    }
}

/// A view with an object region.
pub trait HasObjects: AstNode {
    /// Direct object children in source order. Never descendants.
    fn objects(&self) -> impl Iterator<Item = Object> + '_ {
        self.syntax().children().filter_map(Object::cast)
    }

    /// The direct object child containing `offset`, if any.
    ///
    /// Left-biased at a seam, same contract as [`HasElements::element_at`].
    #[inline]
    #[must_use]
    fn object_at(&self, offset: TextSize) -> Option<Object> {
        left_biased_child_at(self.objects(), offset)
    }
}

/// The default [`HasParts::part_at`]: left-biased ownership over the present
/// parts.
fn left_biased_part_at<T: HasParts>(node: &T, offset: TextSize) -> Option<T::Part> {
    let present: Vec<_> = T::PARTS
        .iter()
        .copied()
        .filter_map(|part| match node.part_span(part) {
            PartSpan::Present(p) => Some((part, p.range())),
            PartSpan::Absent(_) | PartSpan::NotApplicable => None,
        })
        .collect();
    left_biased_range_owner(&present, offset)
}

/// Left-biased owner of `offset` among present `(id, range)` pairs.
///
/// A range owns its half-open interior, and its exclusive end only when
/// another range starts at the same offset (a seam). A lone exclusive end is
/// a gap and yields `None`.
/// [`HasParts::part_at`] and [`Timestamp::field_at`] share this rule.
pub(crate) fn left_biased_range_owner<T: Copy>(
    present: &[(T, TextRange)],
    offset: TextSize,
) -> Option<T> {
    let mut ending = None;
    let mut starting = None;
    for &(id, r) in present {
        if r.is_empty() {
            if offset == r.start() {
                starting.get_or_insert(id);
            }
            continue;
        }
        if r.start() < offset && offset < r.end() {
            return Some(id);
        }
        if offset == r.end() {
            ending.get_or_insert(id);
        } else if offset == r.start() {
            starting.get_or_insert(id);
        }
    }
    match (ending, starting) {
        (Some(e), Some(_)) => Some(e),
        (None, Some(s)) => Some(s),
        (_, None) => None,
    }
}

/// Pick the child whose range owns `offset` under left-bias at seams.
///
/// Element and object children tile their region, so an offset at the
/// exclusive end of the last child still hits that child. At a shared
/// boundary the child that ends there wins over the one that starts there.
pub(crate) fn left_biased_child_at<T: AstNode>(
    children: impl Iterator<Item = T>,
    offset: TextSize,
) -> Option<T> {
    let mut ending = None;
    let mut starting = None;
    for child in children {
        let r = child.range();
        if r.is_empty() {
            if offset == r.start() {
                starting.get_or_insert(child);
            }
            continue;
        }
        if r.start() < offset && offset < r.end() {
            return Some(child);
        }
        if offset == r.end() {
            ending = Some(child);
        } else if offset == r.start() {
            starting.get_or_insert(child);
        }
    }
    ending.or(starting)
}

macro_rules! map_element {
    ($self:expr, $n:ident => $body:expr) => {
        match $self {
            Element::Paragraph($n) => $body,
            Element::PlainList($n) => $body,
            Element::Table($n) => $body,
            Element::Drawer($n) => $body,
            Element::PropertyDrawer($n) => $body,
            Element::Block($n) => $body,
            Element::Keyword($n) => $body,
            Element::Comment($n) => $body,
            Element::FixedWidth($n) => $body,
            Element::HorizontalRule($n) => $body,
            Element::Clock($n) => $body,
            Element::BabelCall($n) => $body,
            Element::InlineTask($n) => $body,
            Element::FootnoteDefinition($n) => $body,
            Element::LatexEnvironment($n) => $body,
            Element::DiarySexp($n) => $body,
        }
    };
}

macro_rules! impl_from_view {
    ($enum:ident, $($var:ident($ty:ty)),+ $(,)?) => {
        $(
            impl From<$ty> for $enum {
                fn from(value: $ty) -> Self {
                    Self::$var(value)
                }
            }
            impl From<&$ty> for $enum {
                fn from(value: &$ty) -> Self {
                    Self::$var(value.clone())
                }
            }
        )+
    };
}

/// A body element: a direct element child of a container.
///
/// Sixteen variants cover twenty-five node kinds: [`Block`] covers the nine
/// `*_BLOCK` kinds and [`Keyword`] covers `KEYWORD` and `SETTING_KEYWORD`.
///
/// [`Self::can_cast`] tests the kind only. [`Self::cast`] also looks at the
/// parent, because some kinds are a body element in one place and something
/// else in another, so `can_cast` can be true while `cast` returns `None`:
/// - `DIARY_SEXP` under `TIMESTAMP` (a diary timestamp);
/// - `PROPERTY_DRAWER` under `OUTLINE` or `INLINE_TASK` (the entry's
///   properties; under `SECTION` it is an element);
/// - `KEYWORD` under `AFFILIATED_KEYWORDS` (a run member) or under `TABLE`
///   (a `#+TBLFM:` line).
///
/// The element walks ([`HasElements::elements`], [`ElementView::next_element`],
/// [`ElementView::prev_element`]) apply the same test. For the same reason
/// the `Keyword`, `DiarySexp` and `PropertyDrawer` variants have no `From`
/// conversion; go through [`Self::cast`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Element {
    /// A paragraph.
    Paragraph(Paragraph),
    /// A plain list.
    PlainList(PlainList),
    /// A table, with any `#+TBLFM:` lines after it.
    Table(Table),
    /// A drawer other than a property drawer.
    Drawer(Drawer),
    /// A property drawer in a section, such as the one at the top of the file.
    /// The drawer of a heading or inline task is not an element.
    PropertyDrawer(PropertyDrawer),
    /// A block of any of the nine kinds, dynamic blocks included.
    Block(Block),
    /// A keyword line, setting keywords such as `#+TODO:` included.
    Keyword(Keyword),
    /// A comment.
    Comment(Comment),
    /// A fixed-width area.
    FixedWidth(FixedWidth),
    /// A horizontal rule.
    HorizontalRule(HorizontalRule),
    /// A clock line.
    Clock(Clock),
    /// A `#+CALL:` line.
    BabelCall(BabelCall),
    /// An inline task.
    InlineTask(InlineTask),
    /// A footnote definition.
    FootnoteDefinition(FootnoteDefinition),
    /// A LaTeX environment.
    LatexEnvironment(LatexEnvironment),
    /// A diary sexp line, `%%(...)`.
    DiarySexp(DiarySexp),
}

impl AstNode for Element {
    fn can_cast(kind: OrgSyntaxKind) -> bool {
        is_element_kind(kind)
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        if !is_body_element(&node) {
            return None;
        }
        match node.kind() {
            OrgSyntaxKind::PARAGRAPH => Paragraph::cast(node).map(Self::Paragraph),
            OrgSyntaxKind::PLAIN_LIST => PlainList::cast(node).map(Self::PlainList),
            OrgSyntaxKind::TABLE => Table::cast(node).map(Self::Table),
            OrgSyntaxKind::DRAWER => Drawer::cast(node).map(Self::Drawer),
            OrgSyntaxKind::PROPERTY_DRAWER => PropertyDrawer::cast(node).map(Self::PropertyDrawer),
            k if Block::can_cast(k) => Block::cast(node).map(Self::Block),
            OrgSyntaxKind::KEYWORD | OrgSyntaxKind::SETTING_KEYWORD => {
                Keyword::cast(node).map(Self::Keyword)
            }
            OrgSyntaxKind::COMMENT => Comment::cast(node).map(Self::Comment),
            OrgSyntaxKind::FIXED_WIDTH => FixedWidth::cast(node).map(Self::FixedWidth),
            OrgSyntaxKind::HORIZONTAL_RULE => HorizontalRule::cast(node).map(Self::HorizontalRule),
            OrgSyntaxKind::CLOCK => Clock::cast(node).map(Self::Clock),
            OrgSyntaxKind::BABEL_CALL => BabelCall::cast(node).map(Self::BabelCall),
            OrgSyntaxKind::INLINE_TASK => InlineTask::cast(node).map(Self::InlineTask),
            OrgSyntaxKind::FOOTNOTE_DEFINITION => {
                FootnoteDefinition::cast(node).map(Self::FootnoteDefinition)
            }
            OrgSyntaxKind::LATEX_ENVIRONMENT => {
                LatexEnvironment::cast(node).map(Self::LatexEnvironment)
            }
            OrgSyntaxKind::DIARY_SEXP => DiarySexp::cast(node).map(Self::DiarySexp),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        map_element!(self, n => n.syntax())
    }
}

impl Covering for Element {
    fn covering_seed(start: &SyntaxNode) -> SyntaxNode {
        nav::affiliated_cover_seed(start)
    }

    fn covering_span(node: &Self) -> TextRange {
        node.full_range()
    }
}

// `From` only where `cast` would always succeed for the kind. Keyword,
// DiarySexp and PropertyDrawer depend on the parent, so they go through `cast`.
impl_from_view!(
    Element,
    Paragraph(Paragraph),
    PlainList(PlainList),
    Table(Table),
    Drawer(Drawer),
    Block(Block),
    Comment(Comment),
    FixedWidth(FixedWidth),
    HorizontalRule(HorizontalRule),
    Clock(Clock),
    BabelCall(BabelCall),
    InlineTask(InlineTask),
    FootnoteDefinition(FootnoteDefinition),
    LatexEnvironment(LatexEnvironment),
);

impl ElementView for Element {
    fn content_range(&self) -> Option<TextRange> {
        // Qualified: FootnoteDefinition and InlineTask also have an inherent
        // `content_range` that returns a plain TextRange.
        map_element!(self, n => ElementView::content_range(n))
    }

    fn is_greater(&self) -> bool {
        map_element!(self, n => n.is_greater())
    }
}

impl Element {
    /// Where to write a new element before this one.
    ///
    /// The point is ahead of this element's affiliated keywords, so a caption
    /// stays with its target.
    #[must_use]
    pub fn insert_before(&self) -> InsertPoint {
        dest(self.syntax(), self.full_range().start())
    }

    /// Where to write a new element of `kind` after this one.
    ///
    /// The lead padding is the stronger of what end of file needs and
    /// [`separation_after`]. Two plain lists stay separate only with two blank
    /// lines between them; after a list whose last line ends in a newline and
    /// that owns no trailing blank, `BlankLine` (`\n\n`) supplies both. At end
    /// of file with no final newline, the first `\n` only ends the line, so a
    /// list written after a list there still merges with it.
    #[must_use]
    pub fn insert_after(&self, kind: OrgSyntaxKind) -> InsertPoint {
        let offset = self.range().end();
        InsertPoint::new(
            offset,
            merge_lead(
                eof_lead(&syntax_root(self.syntax()), offset),
                separation_after(self, kind),
            ),
            Padding::None,
        )
    }
}

macro_rules! map_object {
    ($self:expr, $n:ident => $body:expr) => {
        match $self {
            Object::Markup($n) => $body,
            Object::Subscript($n) => $body,
            Object::Superscript($n) => $body,
            Object::Entity($n) => $body,
            Object::LatexFragment($n) => $body,
            Object::ExportSnippet($n) => $body,
            Object::FootnoteReference($n) => $body,
            Object::InlineBabelCall($n) => $body,
            Object::InlineSrcBlock($n) => $body,
            Object::Link($n) => $body,
            Object::RadioTarget($n) => $body,
            Object::Target($n) => $body,
            Object::Macro($n) => $body,
            Object::StatisticsCookie($n) => $body,
            Object::Timestamp($n) => $body,
            Object::Citation($n) => $body,
            Object::CitationReference($n) => $body,
            Object::LineBreak($n) => $body,
        }
    };
}

/// A direct object child of an object region.
///
/// Eighteen variants cover twenty-three node kinds: [`Markup`] covers the six
/// emphasis and verbatim kinds. [`Self::can_cast`] and [`Self::cast`] agree,
/// since every object kind is an object wherever it appears. Parts of objects
/// (`LINK_PATH`, `DATE`, ...) are not variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Object {
    /// Bold, italic, underline, strike-through, code or verbatim markup.
    Markup(Markup),
    /// A subscript.
    Subscript(Subscript),
    /// A superscript.
    Superscript(Superscript),
    /// An entity, such as `\alpha`.
    Entity(Entity),
    /// A LaTeX fragment.
    LatexFragment(LatexFragment),
    /// An export snippet.
    ExportSnippet(ExportSnippet),
    /// A footnote reference.
    FootnoteReference(FootnoteReference),
    /// An inline babel call.
    InlineBabelCall(InlineBabelCall),
    /// An inline source block.
    InlineSrcBlock(InlineSrcBlock),
    /// A link.
    Link(Link),
    /// A radio target.
    RadioTarget(RadioTarget),
    /// A target.
    Target(Target),
    /// A macro.
    Macro(Macro),
    /// A statistics cookie.
    StatisticsCookie(StatisticsCookie),
    /// A timestamp.
    Timestamp(Timestamp),
    /// A citation.
    Citation(Citation),
    /// A citation reference.
    CitationReference(CitationReference),
    /// A line break.
    LineBreak(LineBreak),
}

impl AstNode for Object {
    fn can_cast(kind: OrgSyntaxKind) -> bool {
        is_object_kind(kind)
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            OrgSyntaxKind::BOLD
            | OrgSyntaxKind::ITALIC
            | OrgSyntaxKind::UNDERLINE
            | OrgSyntaxKind::STRIKE_THROUGH
            | OrgSyntaxKind::CODE
            | OrgSyntaxKind::VERBATIM => Markup::cast(node).map(Self::Markup),
            OrgSyntaxKind::SUBSCRIPT => Subscript::cast(node).map(Self::Subscript),
            OrgSyntaxKind::SUPERSCRIPT => Superscript::cast(node).map(Self::Superscript),
            OrgSyntaxKind::ENTITY => Entity::cast(node).map(Self::Entity),
            OrgSyntaxKind::LATEX_FRAGMENT => LatexFragment::cast(node).map(Self::LatexFragment),
            OrgSyntaxKind::EXPORT_SNIPPET => ExportSnippet::cast(node).map(Self::ExportSnippet),
            OrgSyntaxKind::FOOTNOTE_REFERENCE => {
                FootnoteReference::cast(node).map(Self::FootnoteReference)
            }
            OrgSyntaxKind::INLINE_BABEL_CALL => {
                InlineBabelCall::cast(node).map(Self::InlineBabelCall)
            }
            OrgSyntaxKind::INLINE_SRC_BLOCK => InlineSrcBlock::cast(node).map(Self::InlineSrcBlock),
            OrgSyntaxKind::LINK => Link::cast(node).map(Self::Link),
            OrgSyntaxKind::RADIO_TARGET => RadioTarget::cast(node).map(Self::RadioTarget),
            OrgSyntaxKind::TARGET => Target::cast(node).map(Self::Target),
            OrgSyntaxKind::MACRO => Macro::cast(node).map(Self::Macro),
            OrgSyntaxKind::STATISTICS_COOKIE => {
                StatisticsCookie::cast(node).map(Self::StatisticsCookie)
            }
            OrgSyntaxKind::TIMESTAMP => Timestamp::cast(node).map(Self::Timestamp),
            OrgSyntaxKind::CITATION => Citation::cast(node).map(Self::Citation),
            OrgSyntaxKind::CITATION_REFERENCE => {
                CitationReference::cast(node).map(Self::CitationReference)
            }
            OrgSyntaxKind::LINE_BREAK => LineBreak::cast(node).map(Self::LineBreak),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        map_object!(self, n => n.syntax())
    }
}

impl Covering for Object {}

impl_from_view!(
    Object,
    Markup(Markup),
    Subscript(Subscript),
    Superscript(Superscript),
    Entity(Entity),
    LatexFragment(LatexFragment),
    ExportSnippet(ExportSnippet),
    FootnoteReference(FootnoteReference),
    InlineBabelCall(InlineBabelCall),
    InlineSrcBlock(InlineSrcBlock),
    Link(Link),
    RadioTarget(RadioTarget),
    Target(Target),
    Macro(Macro),
    StatisticsCookie(StatisticsCookie),
    Timestamp(Timestamp),
    Citation(Citation),
    CitationReference(CitationReference),
    LineBreak(LineBreak),
);

/// A construct whose contents are elements.
///
/// `Block` here is only a center, quote, special or dynamic block; verbatim
/// and verse blocks are elements but not containers. There is no
/// `From<Block>`: reach a container block through [`Self::cast`] or
/// [`Block::body`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Container {
    /// A section.
    Section(Section),
    /// A drawer.
    Drawer(Drawer),
    /// A center, quote, special or dynamic block.
    Block(Block),
    /// A list item.
    Item(Item),
    /// A footnote definition.
    FootnoteDefinition(FootnoteDefinition),
    /// An inline task.
    InlineTask(InlineTask),
}

impl AstNode for Container {
    fn can_cast(kind: OrgSyntaxKind) -> bool {
        is_container_kind(kind)
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            OrgSyntaxKind::SECTION => Section::cast(node).map(Self::Section),
            OrgSyntaxKind::DRAWER => Drawer::cast(node).map(Self::Drawer),
            OrgSyntaxKind::CENTER_BLOCK
            | OrgSyntaxKind::QUOTE_BLOCK
            | OrgSyntaxKind::SPECIAL_BLOCK
            | OrgSyntaxKind::DYNAMIC_BLOCK => Block::cast(node).map(Self::Block),
            OrgSyntaxKind::ITEM => Item::cast(node).map(Self::Item),
            OrgSyntaxKind::FOOTNOTE_DEFINITION => {
                FootnoteDefinition::cast(node).map(Self::FootnoteDefinition)
            }
            OrgSyntaxKind::INLINE_TASK => InlineTask::cast(node).map(Self::InlineTask),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Section(n) => n.syntax(),
            Self::Drawer(n) => n.syntax(),
            Self::Block(n) => n.syntax(),
            Self::Item(n) => n.syntax(),
            Self::FootnoteDefinition(n) => n.syntax(),
            Self::InlineTask(n) => n.syntax(),
        }
    }
}

impl Covering for Container {}

impl_from_view!(
    Container,
    Section(Section),
    Drawer(Drawer),
    Item(Item),
    FootnoteDefinition(FootnoteDefinition),
    InlineTask(InlineTask)
);

impl HasElements for Container {}

impl Container {
    /// Content region; for an empty container, a zero-width span between its
    /// delimiters.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        match self {
            Self::Section(n) => n.content_range(),
            Self::Drawer(n) => n.body_range(),
            Self::Block(n) => n.container_body_range(),
            Self::Item(n) => n.content_range(),
            Self::FootnoteDefinition(n) => n.content_range(),
            Self::InlineTask(n) => n.content_range(),
        }
    }

    /// Where a new first element is written; in an empty container, the
    /// zero-width interior.
    #[must_use]
    pub fn first_element_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.content_range().start())
    }

    /// Where a new last element is written: after the last element, or as for
    /// the first element when the container is empty.
    #[must_use]
    pub fn last_element_insert_point(&self) -> InsertPoint {
        match self.last_element() {
            Some(e) => dest(self.syntax(), e.range().end()),
            None => self.first_element_insert_point(),
        }
    }
}

/// Separation Org requires between `previous` and a new construct of `kind`.
///
/// The answer is `BlankLine` when `previous` would otherwise absorb the new
/// construct: a paragraph after a paragraph, a comment after a comment, fixed
/// width after fixed width. A plain list after a plain list needs two blank
/// lines, since one blank line continues the same list: `BlankLine` when the
/// list owns no trailing blank, `Newline` when it owns one, `None` when it owns
/// two or more. See [`Element::insert_after`] for the end-of-file case.
#[must_use]
pub fn separation_after(previous: &Element, kind: OrgSyntaxKind) -> Padding {
    if let (Element::PlainList(list), OrgSyntaxKind::PLAIN_LIST) = (previous, kind) {
        return list_list_separation(list);
    }
    if owns_trailing_blank_line(previous.syntax()) {
        return Padding::None;
    }
    match (previous, kind) {
        (Element::Paragraph(_), OrgSyntaxKind::PARAGRAPH)
        | (Element::Comment(_), OrgSyntaxKind::COMMENT)
        | (Element::FixedWidth(_), OrgSyntaxKind::FIXED_WIDTH) => Padding::BlankLine,
        _ => Padding::None,
    }
}

fn list_list_separation(list: &PlainList) -> Padding {
    match newlines_after_last_child(list.syntax()) {
        0 => Padding::BlankLine,
        1 => Padding::Newline,
        _ => Padding::None,
    }
}

/// Stronger of two lead paddings: `BlankLine` > `Newline` > `Space` > `None`.
#[must_use]
pub(crate) fn merge_lead(a: Padding, b: Padding) -> Padding {
    fn rank(p: Padding) -> u8 {
        match p {
            Padding::BlankLine => 3,
            Padding::Newline => 2,
            Padding::Space => 1,
            Padding::None => 0,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}

// ---------------------------------------------------------------------------
// Test support: the part contract and view coverage
// ---------------------------------------------------------------------------

/// Assert the part contract for one `HasParts` node.
///
/// Checks: present spans nest inside the construct; content nests in the
/// part's range; removal contains the range and nests in the construct;
/// `part_at` at a part's start, middle and a seam agrees with left bias;
/// offsets in gaps between parts (and a lone exclusive end) return `None`; an
/// absent part's insert point lies inside the construct or at its end.
#[cfg(test)]
#[allow(clippy::too_many_lines)] // one pass over the whole contract
pub(crate) fn check_part_contract<T: HasParts>(node: &T) {
    let construct = node.range();
    let mut present: Vec<(T::Part, TextRange)> = Vec::new();

    for &part in T::PARTS {
        match node.part_span(part) {
            PartSpan::Present(p) => {
                let range = p.range();
                let content = p.content();
                let removal = p.removal();
                assert!(
                    range_contains_range(construct, range),
                    "{part:?}: carved {range:?} not inside construct {construct:?}"
                );
                assert!(
                    range_contains_range(range, content),
                    "{part:?}: content {content:?} not inside carved {range:?}"
                );
                // Removal is the range plus an optional separator.
                assert!(
                    range_contains_range(removal, range),
                    "{part:?}: removal {removal:?} must contain carved {range:?}"
                );
                assert!(
                    range_contains_range(construct, removal),
                    "{part:?}: removal {removal:?} not inside construct {construct:?}"
                );
                assert!(
                    range_contains_range(removal, content),
                    "{part:?}: content {content:?} not inside removal {removal:?}"
                );
                present.push((part, range));
            }
            PartSpan::Absent(ip) => {
                let off = ip.offset();
                assert!(
                    (construct.start() <= off && off <= construct.end()),
                    "{part:?}: insert offset {off:?} outside construct {construct:?}"
                );
            }
            PartSpan::NotApplicable => {}
        }
    }

    // PARTS need not be in source order (Planning's is fixed).
    present.sort_by_key(|(_, r)| r.start());

    for i in 0..present.len() {
        let (part, range) = present[i];
        // At a left-biased seam, range.start() belongs to the previous
        // source-order part.
        let seam_owned_by_prev = i > 0 && {
            let prev = present[i - 1].1;
            prev.end() == range.start() && (!prev.is_empty() || prev.start() == range.start())
        };
        if !seam_owned_by_prev {
            assert_eq!(
                node.part_at(range.start()),
                Some(part),
                "{part:?}: part_at(start) disagrees"
            );
        }
        if range.is_empty() {
            continue;
        }
        let half = u32::from(range.len()) / 2;
        let mid = range.start() + TextSize::from(half);
        if mid < range.end() {
            // A one-byte part's mid is its start. At a seam that start already
            // belongs to the previous part (left-bias); skip the mid check.
            if !(mid == range.start() && seam_owned_by_prev) {
                assert_eq!(
                    node.part_at(mid),
                    Some(part),
                    "{part:?}: part_at(mid) disagrees"
                );
            }
        }
        let next_starts_here = present
            .get(i + 1)
            .is_some_and(|(_, next)| next.start() == range.end());
        if next_starts_here {
            assert_eq!(
                node.part_at(range.end()),
                Some(part),
                "{part:?}: part_at(end) left-bias at seam disagrees"
            );
        }
    }

    // Gaps: the exclusive end of one present part, strictly before the next
    // start, is not a seam, so part_at must be None.
    for pair in present.windows(2) {
        let (left, a) = pair[0];
        let (_, b) = pair[1];
        if a.end() >= b.start() {
            continue;
        }
        // An empty part's exclusive end is the part itself, not a gap.
        if !a.is_empty() {
            assert_eq!(
                node.part_at(a.end()),
                None,
                "{left:?}: gap at exclusive end {:?} must be None",
                a.end()
            );
        }
        let gap = u32::from(b.start() - a.end());
        if gap > 1 {
            let mid = a.end() + TextSize::from(gap / 2);
            assert_eq!(
                node.part_at(mid),
                None,
                "{left:?}: gap mid {mid:?} must be None"
            );
        }
    }
    if let Some(&(part, last)) = present.last()
        && !last.is_empty()
        && last.end() < construct.end()
    {
        assert_eq!(
            node.part_at(last.end()),
            None,
            "{part:?}: trailing gap at exclusive end must be None"
        );
    }
}

#[cfg(test)]
#[inline]
fn range_contains_range(outer: TextRange, inner: TextRange) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

/// Whether `kind` has a view type of its own.
///
/// The unions (`Element`, `Object`, `Container`, `Task`) do not count. Views of
/// sub-parts that have their own newtype (title, tags, ...) do.
#[cfg(test)]
pub(crate) fn primary_view_covers(kind: OrgSyntaxKind) -> bool {
    AffiliatedKeywords::can_cast(kind)
        || TodoKeyword::can_cast(kind)
        || Priority::can_cast(kind)
        || HeadingTitle::can_cast(kind)
        || Tags::can_cast(kind)
        || Heading::can_cast(kind)
        || InlineTask::can_cast(kind)
        || Outline::can_cast(kind)
        || Section::can_cast(kind)
        || Planning::can_cast(kind)
        || PropertyDrawer::can_cast(kind)
        || NodeProperty::can_cast(kind)
        || Timestamp::can_cast(kind)
        || Clock::can_cast(kind)
        || Drawer::can_cast(kind)
        || Paragraph::can_cast(kind)
        || PlainList::can_cast(kind)
        || Item::can_cast(kind)
        || ItemTag::can_cast(kind)
        || Table::can_cast(kind)
        || TableRow::can_cast(kind)
        || TableCell::can_cast(kind)
        || Block::can_cast(kind)
        || VerseBody::can_cast(kind)
        || Keyword::can_cast(kind)
        || KeywordValue::can_cast(kind)
        || BabelCall::can_cast(kind)
        || Comment::can_cast(kind)
        || FixedWidth::can_cast(kind)
        || HorizontalRule::can_cast(kind)
        || FootnoteDefinition::can_cast(kind)
        || LatexEnvironment::can_cast(kind)
        || DiarySexp::can_cast(kind)
        || KeywordDual::can_cast(kind)
        || Link::can_cast(kind)
        || LinkDescription::can_cast(kind)
        || Markup::can_cast(kind)
        || Target::can_cast(kind)
        || RadioTarget::can_cast(kind)
        || StatisticsCookie::can_cast(kind)
        || Entity::can_cast(kind)
        || LatexFragment::can_cast(kind)
        || Macro::can_cast(kind)
        || ExportSnippet::can_cast(kind)
        || Subscript::can_cast(kind)
        || Superscript::can_cast(kind)
        || LineBreak::can_cast(kind)
        || Citation::can_cast(kind)
        || CitationReference::can_cast(kind)
        || InlineSrcBlock::can_cast(kind)
        || InlineBabelCall::can_cast(kind)
        || FootnoteReference::can_cast(kind)
}

/// Node kinds deliberately without a view of their own, with the reason.
///
/// A reason is either `reached as <Construct>Part::...` for a sub-part, or the
/// fixed sentence for `DOCUMENT`. `ERROR` is never emitted, so it is not
/// listed.
#[cfg(test)]
pub(crate) const NO_VIEW: &[(OrgSyntaxKind, &str)] = &[
    (
        OrgSyntaxKind::DOCUMENT,
        "owning snapshot root; Document is not an AstNode",
    ),
    (
        OrgSyntaxKind::HEADING_COMMENT,
        "reached as HeadingPart::CommentMarker",
    ),
    (
        OrgSyntaxKind::NODE_PROPERTY_KEY,
        "reached as PropertyPart::Key",
    ),
    (
        OrgSyntaxKind::NODE_PROPERTY_VALUE,
        "reached as PropertyPart::Value",
    ),
    (
        OrgSyntaxKind::DRAWER_NAME,
        "reached as DrawerPart::Name; Drawer::name reads the child",
    ),
    (OrgSyntaxKind::KEYWORD_KEY, "reached as KeywordPart::Key"),
    (
        OrgSyntaxKind::CALL_NAME,
        "reached as BabelCallPart::Name / InlineBabelCallPart::Name",
    ),
    (
        OrgSyntaxKind::CALL_INSIDE_HEADER,
        "reached as BabelCallPart::InsideHeader / InlineBabelCallPart::InsideHeader",
    ),
    (
        OrgSyntaxKind::CALL_ARGUMENTS,
        "reached as BabelCallPart::Arguments / InlineBabelCallPart::Arguments",
    ),
    (
        OrgSyntaxKind::CALL_END_HEADER,
        "reached as BabelCallPart::EndHeader / InlineBabelCallPart::EndHeader",
    ),
    (
        OrgSyntaxKind::BULLET,
        "reached as ItemPart::Bullet; Bullet is a typed value",
    ),
    (
        OrgSyntaxKind::COUNTER,
        "reached as ItemPart::Counter; Counter is a typed value",
    ),
    (
        OrgSyntaxKind::CHECKBOX,
        "reached as ItemPart::Checkbox; CheckboxState is a typed value",
    ),
    (OrgSyntaxKind::LINK_PATH, "reached as LinkPart::Path"),
    (
        OrgSyntaxKind::DATE,
        "reached as TimestampPart::Date / EndDate",
    ),
    (
        OrgSyntaxKind::TIME,
        "reached as TimestampPart::Time / EndTime",
    ),
    (
        OrgSyntaxKind::TIME_RANGE,
        "reached as TimestampPart::Time / EndTime halves",
    ),
    (
        OrgSyntaxKind::ROD,
        "reached as TimestampPart::Repeater / Delay",
    ),
    (
        OrgSyntaxKind::SRC_LANGUAGE,
        "reached as InlineSrcPart::Language",
    ),
    (
        OrgSyntaxKind::SRC_PARAMETERS,
        "reached as InlineSrcPart::Parameters",
    ),
    (OrgSyntaxKind::SRC_BODY, "reached as InlineSrcPart::Body"),
];

/// Node kinds seen under `roots` that have neither a view nor a [`NO_VIEW`]
/// entry. Tests require it to be empty over the taxonomy fixtures and the
/// container-matrix documents.
#[cfg(test)]
pub(crate) fn missing_view_kinds<'a>(
    roots: impl IntoIterator<Item = &'a SyntaxNode>,
) -> Vec<OrgSyntaxKind> {
    use std::collections::BTreeSet;

    let mut seen = BTreeSet::new();
    for root in roots {
        for node in root.descendants() {
            seen.insert(node.kind());
        }
    }
    let no_view: BTreeSet<_> = NO_VIEW.iter().map(|&(k, _)| k).collect();
    seen.into_iter()
        .filter(|k| k.is_node())
        .filter(|k| !primary_view_covers(*k) && !no_view.contains(k))
        .collect()
}
