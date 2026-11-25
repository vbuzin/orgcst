//! Queries on [`Document`] that need the whole snapshot rather than one node:
//!
//! - `queries`: outlines, typed walks, covering and enclosing ranges;
//! - `joins`: TODO state, priorities, tags and properties;
//! - `declarations`: what a file declares for its headlines to inherit, read
//!   once for a caller that asks about many;
//! - `discovery`: iterators per construct and lookups by ID, name or label;
//! - `facts`: `#+MACRO:`, `#+LINK:`, header arguments and special properties.
//!
//! A *live* keyword line is one the parser made a keyword node. The same text
//! inside a src or example block is body text and does not count.

mod declarations;
mod discovery;
mod facts;
mod joins;
mod queries;

pub use declarations::Declarations;
pub use discovery::{Footnote, RadioLink};
pub use facts::{EntryFacts, LinkAbbrev, MacroDef};

#[cfg(test)]
use crate::TextSize;
use crate::ast::{AstNode, NodeProperty, Outline, PropertyDrawer, Tag};
use crate::settings::{RawProperty, RawTag, merge_properties};
use crate::syntax::OrgSyntaxKind;
use crate::{Document, TextRange};

// ---------------------------------------------------------------------------
// Query items shared by joins and facts
// ---------------------------------------------------------------------------

/// One property after merging `KEY+` lines: from a drawer or a `#+PROPERTY:`
/// line, never a computed special property such as `ALLTAGS`.
///
/// The key is as written, `+` included; the value is as written; the range
/// is the value's, or the key's when there is no value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Property {
    key: String,
    value: String,
    range: TextRange,
}

impl Property {
    pub(crate) fn new(key: impl Into<String>, value: impl Into<String>, range: TextRange) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            range,
        }
    }

    /// Key as written, including a trailing `+` when that is how it was carved.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Value as written. Empty when the line has no value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Range of the value, or of the key when there is no value.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// The key without a trailing `+`.
    #[must_use]
    pub fn base_key(&self) -> &str {
        self.key.strip_suffix('+').unwrap_or(&self.key)
    }
}

/// A property as Org reads it from a heading, inheriting: the levels it is
/// written at, nearest first, ending at the first that sets it outright.
/// Every level but that last one only adds to it (`KEY+`). From
/// [`Document::property_inherited`] and [`Document::file_property_inherited`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InheritedProperty {
    parts: Vec<(Property, PropertyOrigin)>,
}

impl InheritedProperty {
    pub(crate) fn from_parts(parts: Vec<(Property, PropertyOrigin)>) -> Self {
        Self { parts }
    }

    /// The levels that give the value, nearest first, each its own value
    /// after merging that level's `KEY+` lines.
    #[must_use]
    pub fn parts(&self) -> &[(Property, PropertyOrigin)] {
        &self.parts
    }

    /// The nearest level that writes the property.
    ///
    /// # Panics
    ///
    /// Never: an inherited property has at least one part.
    #[must_use]
    pub fn nearest(&self) -> &(Property, PropertyOrigin) {
        self.parts
            .first()
            .expect("an inherited property is built from at least one part")
    }

    /// The value Org reads: each level's value from the outermost in,
    /// joined by a space, as `KEY+` adds to the value above it.
    #[must_use]
    pub fn value(&self) -> String {
        let mut out = String::new();
        for (p, _) in self.parts.iter().rev() {
            if p.value().is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(p.value());
        }
        out
    }
}

/// Where a property in [`InheritedProperty`] / [`Document::file_properties`]
/// came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropertyOrigin {
    /// This task's own drawer.
    Local,
    /// An ancestor outline's drawer (the enclosing outline, for an inline
    /// task).
    Inherited {
        /// The outline whose drawer holds the property.
        from: Outline,
    },
    /// The property drawer at the top of the file.
    FileDrawer,
    /// A live `#+PROPERTY:` line.
    FileKeyword,
}

/// Kind of group in a `#+TAGS:` declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagGroupKind {
    /// `{ a b }`: mutually exclusive tags.
    Exclusive,
    /// `[ name : children ]`: a group tag and its members.
    GroupTag,
}

/// One group from `#+TAGS:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagGroup {
    kind: TagGroupKind,
    name: Option<Tag>,
    members: Vec<Tag>,
}

impl TagGroup {
    pub(crate) fn new(kind: TagGroupKind, name: Option<Tag>, members: Vec<Tag>) -> Self {
        Self {
            kind,
            name,
            members,
        }
    }

    /// Which kind of group this is.
    #[must_use]
    pub fn kind(&self) -> TagGroupKind {
        self.kind
    }

    /// Group tag for `[name : children]`. `None` for `{ a b }`.
    #[must_use]
    pub fn name(&self) -> Option<&Tag> {
        self.name.as_ref()
    }

    /// The tags in the group.
    #[must_use]
    pub fn members(&self) -> &[Tag] {
        &self.members
    }
}

/// The tags a file declares with `#+TAGS:`: groups and free tags. See
/// [`Document::declared_tags`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredTags {
    groups: Vec<TagGroup>,
    free: Vec<Tag>,
}

impl DeclaredTags {
    pub(crate) fn new(groups: Vec<TagGroup>, free: Vec<Tag>) -> Self {
        Self { groups, free }
    }

    /// Tag groups, in declaration order.
    #[must_use]
    pub fn groups(&self) -> &[TagGroup] {
        &self.groups
    }

    /// Tags outside any group.
    #[must_use]
    pub fn free(&self) -> &[Tag] {
        &self.free
    }
}

pub(super) fn source_slice(source: &str, range: TextRange) -> &str {
    let start: usize = range.start().into();
    let end: usize = range.end().into();
    &source[start..end]
}

pub(super) fn region_hits<'a, T: AstNode + 'a>(
    range: TextRange,
    iter: impl Iterator<Item = T> + 'a,
) -> impl Iterator<Item = T> + 'a {
    iter.filter(move |n| {
        !range.is_empty() && n.range().intersect(range).is_some_and(|i| !i.is_empty())
    })
}

pub(super) fn lead_in_file_drawer(doc: &Document) -> Option<PropertyDrawer> {
    let section = doc.zeroth_section()?;
    for child in section.syntax().children() {
        if child.kind() == OrgSyntaxKind::COMMENT {
            continue;
        }
        return PropertyDrawer::cast(child);
    }
    None
}

pub(super) fn property_from_raw(raw: RawProperty) -> Property {
    Property::new(raw.key, raw.value, raw.range)
}

pub(super) fn tag_from_raw(raw: RawTag) -> Tag {
    Tag::new(raw.name, raw.range, raw.fast_key)
}

pub(super) fn raw_from_node(np: &NodeProperty) -> RawProperty {
    RawProperty {
        key: np.key(),
        value: np.value().unwrap_or_default(),
        range: np.value_range().unwrap_or_else(|| np.key_range()),
    }
}

pub(crate) fn merged_drawer_properties(drawer: &PropertyDrawer) -> Vec<Property> {
    merge_properties(drawer.properties().map(|np| raw_from_node(&np)))
        .into_iter()
        .map(property_from_raw)
        .collect()
}

pub(super) fn property_from_drawer(drawer: Option<PropertyDrawer>, key: &str) -> Option<Property> {
    use crate::settings::property_key_eq;
    merged_drawer_properties(&drawer?)
        .into_iter()
        .find(|p| property_key_eq(p.key(), key))
}

#[cfg(test)]
pub(super) fn at(src: &str, pat: &str) -> TextSize {
    TextSize::from(src.find(pat).expect(pat) as u32)
}

#[cfg(test)]
pub(super) fn span(src: &str, pat: &str) -> TextRange {
    let start = src.find(pat).expect(pat) as u32;
    TextRange::new(
        TextSize::from(start),
        TextSize::from(start + pat.len() as u32),
    )
}
