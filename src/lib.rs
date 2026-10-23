//! A lossless parser for Org mode documents.
//!
//! [`parse`] turns Org text into a [`Document`]: an immutable snapshot that
//! owns the source, a [rowan] concrete syntax tree in which every byte sits in
//! exactly one token, the configuration the tree was built under, and any
//! [`Diagnostic`]s. Parsing never fails.
//!
//! ```
//! use orgcst::{AstNode, Headline, TodoKind, parse};
//!
//! let doc = parse("#+TODO: NEXT | DONE\n* NEXT Write the docs :work:\n");
//! assert_eq!(doc.root().text().to_string(), doc.source());
//!
//! let heading = doc.headings().next().unwrap();
//! assert_eq!(doc.todo_kind(&heading), Some(TodoKind::Active));
//! assert_eq!(doc.slice(heading.todo_keyword().unwrap().range()), "NEXT");
//! ```
//!
//! # Layers
//!
//! - **Syntax tree.** [`Document::root`] is the untyped red tree, with
//!   [`OrgSyntaxKind`] naming every node and token. Use it when you need every
//!   byte, for example to draw syntax highlighting.
//! - **Typed views.** [`ast`] has zero-cost wrappers over tree nodes:
//!   [`Heading`], [`Planning`], [`Timestamp`], [`Link`], [`PlainList`],
//!   [`Table`] and so on, and the enums [`Element`], [`Object`] and
//!   [`Container`] over them. Every part of a construct reports its range, and
//!   an absent part reports where it would be inserted ([`InsertPoint`]).
//! - **Document queries.** Methods on [`Document`] answer questions that need
//!   the whole file: TODO state under the file's own `#+TODO:` lines, tags and
//!   properties with inheritance, macros, footnotes, IDs.
//! - **Edits.** An [`EditSet`] is a batch of byte-range replacements, and
//!   [`Document::apply`] returns the next snapshot, reusing unchanged outlines.
//!   [`Operation`]s such as setting a TODO state or moving a subtree
//!   [`compile`] to an `EditSet`, so every change is plain text in the end.
//!
//! # Coordinates
//!
//! Ranges are absolute byte offsets ([`TextRange`], [`TextSize`]) into
//! [`Document::source`]. A construct's range covers its full syntax,
//! delimiters included. Sources longer than [`MAX_SOURCE_LEN`] (4 GiB) are
//! rejected with a panic, because offsets are `u32`.
//!
//! # Threads
//!
//! Red tree nodes are reference-counted and not `Send`, so neither are a
//! [`Document`] or its views. Convert a snapshot into a [`PortableDocument`]
//! to move it to another thread; rebuilding the tree there is cheap.
//!
//! # Configuration
//!
//! [`ParseConfig`] holds only what changes the shape of the tree: the TODO
//! vocabulary, link types, user entities, alphabetical list bullets, the
//! inline task level and the nesting limit. A
//! document's own `#+TODO:` lines take precedence over the caller's
//! vocabulary. Everything else a document declares, such as `#+PRIORITIES:`
//! or `#+FILETAGS:`, is read from the tree on demand.
//!
//! # Stability
//!
//! Before 1.0, minor releases may break the API. [`DiagnosticKind`] is
//! `non_exhaustive`. [`Element`], [`Object`], [`Container`] and [`Task`] are
//! exhaustive so that matches over them stay complete; a new variant is a
//! breaking change.
//!
//! [rowan]: https://docs.rs/rowan
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_docs)]
// Docs name CST kinds such as INLINE_TASK as plain words.
#![allow(clippy::doc_markdown)]
// Byte offsets (`TextSize`), token indexes (`Pos`), and line indexes
// (`LineId`) are `u32` and stay inside `MAX_SOURCE_LEN`.
#![allow(clippy::cast_possible_truncation)]
pub mod ast;
pub(crate) use ast::ast_node;
mod damage;
mod document;
mod edit;
pub mod entities;
mod lexer;
mod operation;
mod parser;
mod settings;
mod writer;

pub mod syntax;

#[cfg(test)]
mod tests;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use ast::{
    AffiliatedKeywords, AstNode, BabelCall, BabelCallPart, Block, BlockBody, BlockKind, BlockPart,
    Bullet, CellAddr, CellFormula, CellFormulaKind, CheckboxState, Citation, CitationPart,
    CitationReference, CitationReferencePart, ClassifiedLink, Clock, ClockPart, Comment, Container,
    ContextFrame, CookieData, CookieShape, CookieValue, Counter, CounterForm, Covering,
    DeclaredTags, Delay, DelayKind, DiarySexp, Drawer, DrawerPart, EditContext, Element,
    ElementView, Emphasis, Entity, EntryFact, EntryFactValue, EntryFacts, ExportSnippet,
    ExportSnippetPart, FixedWidth, Footnote, FootnoteDefinition, FootnotePart, FootnoteReference,
    FootnoteReferencePart, FootnoteShape, HasElements, HasObjects, HasParts, HeaderArgsOrigin,
    Heading, HeadingPart, HeadingTitle, Headline, HorizontalRule, InlineBabelCall,
    InlineBabelCallPart, InlineFootnote, InlineSrcBlock, InlineSrcPart, InlineTask, InsertPoint,
    InternalAddress, Item, ItemPart, ItemTag, Keyword, KeywordDual, KeywordPart, KeywordRole,
    KeywordValue, LatexEnvironment, LatexEnvironmentPart, LatexFragment, LatexFragmentShape,
    LineBreak, Link, LinkAbbrev, LinkDescription, LinkPart, LinkShape, LinkTarget, ListKind, Macro,
    MacroDef, Markup, MarkupBody, MarkupKind, MarkupPart, NodeProperty, Object, Outline, Padding,
    Paragraph, PartSpan, PlainList, Planning, PlanningPart, PortableDocument, PresentPart,
    Priority, Property, PropertyDrawer, PropertyOrigin, PropertyPart, RadioTarget, Repeater,
    RepeaterKind, RepeaterUnit, ScriptPart, Section, SplitPoint, StatisticsCookie, Subscript,
    Superscript, Table, TableCell, TableKind, TableRow, Tag, TagGroup, TagGroupKind, TagOrigin,
    Tags, Target, Task, Timestamp, TimestampDelimiters, TimestampField, TimestampPart,
    TimestampPoint, TimestampShape, TodoKeyword, VerseBody, classify_link, escape_link_path,
    normalize_angle_link_ws, normalize_bracket_link_ws, separation_after, unescape_link_path,
};
pub use document::Document;
pub use edit::{Anchor, Bias, Edit, EditSet, EditSetError};
pub use entities::{EntityDef, EntityForm, entity_form};
pub use operation::{
    HeadingInput, NodeRef, Operation, OperationError, PlanningPatch, PropertyEdit, SlotPatch,
    SubtreeDest, TimestampInput, apply_locating, compile, compile_all, payload_start,
};
pub use parser::{
    DEFAULT_INLINE_TASK_MIN_LEVEL, DEFAULT_LINK_TYPES, DEFAULT_MAX_NESTING_DEPTH,
    DEFAULT_TODO_KEYWORDS, Diagnostic, DiagnosticKind, ParseConfig, PriorityAtom, PriorityRange,
    Severity, TodoInterpretation, TodoKind, TodoSequence,
};
pub use rowan::{TextRange, TextSize};
pub use syntax::{OrgLanguage, OrgSyntaxKind, SyntaxElement, SyntaxNode, SyntaxToken};
pub use writer::{
    TimestampProblem, TitleProblem, is_valid_property_key, is_valid_tag_name, quote_lesser_body,
    title_is_writable, title_problem, unquote_lesser_body,
};

use std::sync::Arc;

/// Maximum source length accepted by [`parse`] / [`parse_with`] / apply.
///
/// Rowan `TextSize` is `u32` byte offsets, so inputs above this ceiling cannot
/// be represented without truncating ranges. The public API rejects them with
/// a panic rather than silently corrupting the tree.
pub const MAX_SOURCE_LEN: usize = u32::MAX as usize;

/// Assert that `source` fits in rowan `TextSize` (`u32` byte offsets).
///
/// # Panics
///
/// Panics if `source.len() > MAX_SOURCE_LEN` (~4 GiB). Parsing remains
/// infallible for all supported sizes; oversized inputs are out of scope.
#[inline]
pub(crate) fn assert_max_size(source: &str) {
    assert!(
        source.len() <= MAX_SOURCE_LEN,
        "source length {} exceeds TextSize u32 range (MAX_SOURCE_LEN = {}); \
         sources larger than ~4 GiB are out of scope",
        source.len(),
        MAX_SOURCE_LEN
    );
}

/// Parse `source` into a self-contained [`Document`] snapshot.
///
/// Equivalent to [`parse_with`] using [`ParseConfig::default`].
///
/// The returned document always has a `DOCUMENT` root. The full original text
/// is preserved (`root().text() == source()`). All bytes are accounted for in
/// tokens.
///
/// Single structural pass: written syntax only (bracket / angle / plain links,
/// `TARGET` / `RADIO_TARGET` objects, etc.).
///
/// Coordinate model: token and diagnostic ranges use rowan `TextSize` (`u32`
/// byte offsets). Sources larger than [`MAX_SOURCE_LEN`] (~4 GiB) panic at the
/// public boundary rather than truncating ranges.
///
/// # Panics
///
/// Panics if `source.len() > MAX_SOURCE_LEN`.
///
/// # Examples
///
/// ```
/// use orgcst::{AstNode, Headline, parse};
///
/// let doc = parse("* TODO Ship it\n");
/// let heading = doc
///     .headings()
///     .next()
///     .expect("the fixture has one heading");
/// let keyword = heading
///     .todo_keyword()
///     .expect("TODO is in the default vocabulary");
/// assert_eq!(doc.slice(keyword.range()), "TODO");
/// ```
#[must_use]
pub fn parse(source: &str) -> Document {
    parse_with(source, &ParseConfig::default())
}

/// Parse with explicit configuration. Equivalent to [`parse`] when `config` is
/// [`ParseConfig::default`].
///
/// `config` supplies the defaults for a document that declares none. The
/// document's own `#+TODO:`, `#+SEQ_TODO:` and `#+TYP_TODO:` lines always
/// replace the caller's TODO vocabulary, as they do in Emacs, and the tree is
/// built under the result, which [`Document::config`] returns. The document
/// keeps the caller's layer so that [`Document::apply`] resolves the edited
/// text over the same defaults.
///
/// Settings that do not change the tree are not resolved here:
/// `#+PRIORITIES:`, for example, stays a keyword in the tree and is read by
/// [`Document::priority_range`] when asked.
///
/// # Panics
///
/// Panics if `source.len() > MAX_SOURCE_LEN`.
///
/// # Examples
///
/// A caller vocabulary carves its own keywords. `TODO` is not carved when
/// it is absent from that vocabulary.
///
/// ```
/// use orgcst::{AstNode, Headline, ParseConfig, TodoKind, parse_with};
///
/// let config = ParseConfig::default().with_todo_keywords([
///     ("NEXT", TodoKind::Active),
///     ("DONE", TodoKind::Done),
/// ]);
/// let doc = parse_with("* NEXT carved\n* TODO plain\n", &config);
/// let mut headings = doc.headings();
/// let next = headings.next().expect("first heading");
/// let todo = headings.next().expect("second heading");
/// assert_eq!(
///     doc.slice(
///         next.todo_keyword()
///             .expect("NEXT is carved")
///             .range()
///     ),
///     "NEXT"
/// );
/// assert!(todo.todo_keyword().is_none());
/// ```
#[must_use]
pub fn parse_with(source: &str, config: &ParseConfig) -> Document {
    document::materialize_resolved(Arc::from(source), Arc::new(config.clone()), None)
}
