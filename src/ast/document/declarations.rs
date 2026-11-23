//! What a file declares for its headlines to inherit, read once and held by
//! the caller: the tags of its `#+FILETAGS:` lines, its lead-in property
//! drawer and its `#+PROPERTY:` lines.
//!
//! Org reads a buffer's in-buffer settings once, when the buffer is set up,
//! and every headline's inherited tags and properties end in them. A
//! document's declarations are read from its tree on demand (no snapshot
//! keeps them), so a caller that asks for many headlines at once takes a
//! [`Declarations`] and asks it: the shared part is read once, and the
//! borrow ties the answers to the snapshot they were read from.

use std::marker::PhantomData;

use super::{
    Property, PropertyOrigin, lead_in_file_drawer, property_from_drawer, property_from_raw,
    tag_from_raw,
};
use crate::Document;
use crate::ast::nav;
use crate::ast::{AstNode, Heading, Headline, PropertyDrawer, Tag, TagOrigin, Task};
use crate::settings::{file_keyword_properties, parse_filetags_from_settings, property_key_eq};

/// A document's declarations that its headlines inherit, read once. See
/// [`Document::declarations`].
#[derive(Debug, Clone)]
pub struct Declarations<'a> {
    file_tags: Vec<Tag>,
    file_drawer: Option<PropertyDrawer>,
    keyword_properties: Vec<Property>,
    doc: PhantomData<&'a Document>,
}

impl Document {
    /// What this document declares for its headlines to inherit, read in one
    /// walk of the tree: ask it, not the document, when asking about many
    /// headlines.
    ///
    /// # Examples
    ///
    /// ```
    /// use orgcst::parse;
    ///
    /// let doc = parse("#+FILETAGS: :home:\n* Plan :work:\n** Step\n");
    /// let declared = doc.declarations();
    /// for heading in doc.headings() {
    ///     let tags: Vec<String> = declared.tags(&heading).map(|(t, _)| t.name().to_owned()).collect();
    ///     assert_eq!(tags.last().map(String::as_str), Some("home"));
    /// }
    /// ```
    #[must_use]
    pub fn declarations(&self) -> Declarations<'_> {
        Declarations {
            file_tags: parse_filetags_from_settings(self.root())
                .into_iter()
                .map(tag_from_raw)
                .collect(),
            file_drawer: lead_in_file_drawer(self),
            keyword_properties: file_keyword_properties(self.root())
                .into_iter()
                .map(property_from_raw)
                .collect(),
            doc: PhantomData,
        }
    }
}

impl Declarations<'_> {
    /// The tags of every live `#+FILETAGS:` line, in document order.
    #[must_use]
    pub fn file_tags(&self) -> &[Tag] {
        &self.file_tags
    }

    /// The headline's own tags, then each ancestor outline's, then
    /// `#+FILETAGS:`, each with its origin.
    pub fn tags(&self, heading: &impl Headline) -> impl Iterator<Item = (Tag, TagOrigin)> {
        tag_chain(heading, self.file_tags.iter().cloned()).into_iter()
    }

    /// The nearest value of `key` walking outwards, with where it came from:
    /// the task's own drawer, the ancestor outlines (for an inline task, its
    /// enclosing outline first), the file's property drawer, then
    /// `#+PROPERTY:` lines. Values are inherited only from outlines, never
    /// from another inline task.
    #[must_use]
    pub fn property_inherited(
        &self,
        task: impl Into<Task>,
        key: &str,
    ) -> Option<(Property, PropertyOrigin)> {
        property_chain(&task.into(), key).or_else(|| {
            file_property(
                self.file_drawer.clone(),
                self.keyword_properties.iter().cloned(),
                key,
            )
        })
    }
}

/// The headline's own tags, its ancestors' and then `file`: the one rule
/// [`Document::tags`] and [`Declarations::tags`] answer by.
pub(super) fn tag_chain(
    heading: &impl Headline,
    file: impl Iterator<Item = Tag>,
) -> Vec<(Tag, TagOrigin)> {
    let mut out = Vec::new();
    if let Some(tags) = heading.tags() {
        out.extend(tags.iter().map(|t| (t, TagOrigin::Written)));
    }
    for outline in nav::inherited_outlines(heading.syntax()) {
        push_inherited_tags(&mut out, &outline.heading());
    }
    out.extend(file.map(|t| (t, TagOrigin::FileTags)));
    out
}

fn push_inherited_tags(out: &mut Vec<(Tag, TagOrigin)>, heading: &Heading) {
    if let Some(tags) = heading.tags() {
        for t in tags.iter() {
            out.push((
                t,
                TagOrigin::Inherited {
                    from: heading.clone(),
                },
            ));
        }
    }
}

/// `key` in the task's own drawer, else the nearest ancestor outline's.
pub(super) fn property_chain(task: &Task, key: &str) -> Option<(Property, PropertyOrigin)> {
    if let Some(v) = property_from_drawer(task.property_drawer(), key) {
        return Some((v, PropertyOrigin::Local));
    }
    for outline in nav::inherited_outlines(task.syntax()) {
        if let Some(v) = property_from_drawer(outline.property_drawer(), key) {
            return Some((v, PropertyOrigin::Inherited { from: outline }));
        }
    }
    None
}

/// `key` at the file level: its lead-in property drawer, then its
/// `#+PROPERTY:` lines.
pub(super) fn file_property(
    drawer: Option<PropertyDrawer>,
    mut keyword_lines: impl Iterator<Item = Property>,
    key: &str,
) -> Option<(Property, PropertyOrigin)> {
    if let Some(v) = property_from_drawer(drawer, key) {
        return Some((v, PropertyOrigin::FileDrawer));
    }
    keyword_lines
        .find(|p| property_key_eq(p.key(), key))
        .map(|v| (v, PropertyOrigin::FileKeyword))
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::parse;

    const SRC: &str = "\
:PROPERTIES:
:AREA: home
:END:
#+FILETAGS: :file:
#+PROPERTY: OWNER sam
* Parent :p:
:PROPERTIES:
:ROOM: hall
:END:
** Child :c:
*** Grandchild
";

    #[test]
    fn the_view_answers_as_the_document_does() {
        let doc = parse(SRC);
        let declared = doc.declarations();
        let names = |chain: Vec<(Tag, TagOrigin)>| -> Vec<String> {
            chain
                .into_iter()
                .map(|(t, _)| t.name().to_owned())
                .collect()
        };
        for heading in doc.headings() {
            assert_eq!(
                names(declared.tags(&heading).collect()),
                names(doc.tags(&heading).collect())
            );
        }
        for outline in doc.outlines() {
            for key in ["ROOM", "AREA", "OWNER", "NONE"] {
                let a = declared.property_inherited(outline.clone(), key);
                let b = doc.property_inherited(outline.clone(), key);
                assert_eq!(
                    a.map(|(p, o)| (p.value().to_owned(), std::mem::discriminant(&o))),
                    b.map(|(p, o)| (p.value().to_owned(), std::mem::discriminant(&o))),
                    "{key}"
                );
            }
        }
        assert_eq!(declared.file_tags()[0].name(), "file");
    }

    #[test]
    fn a_grandchild_inherits_through_every_level() {
        let doc = parse(SRC);
        let declared = doc.declarations();
        let grandchild = doc.headings().last().expect("three headings");
        let chain: Vec<(String, bool)> = declared
            .tags(&grandchild)
            .map(|(t, o)| (t.name().to_owned(), matches!(o, TagOrigin::FileTags)))
            .collect();
        assert_eq!(
            chain,
            vec![
                ("c".to_owned(), false),
                ("p".to_owned(), false),
                ("file".to_owned(), true)
            ]
        );
        let task = Task::from(doc.outlines().last().expect("three outlines"));
        let (area, origin) = declared
            .property_inherited(task.clone(), "AREA")
            .expect("file drawer");
        assert_eq!(area.value(), "home");
        assert!(matches!(origin, PropertyOrigin::FileDrawer));
        let (room, origin) = declared
            .property_inherited(task, "ROOM")
            .expect("inherited");
        assert_eq!(room.value(), "hall");
        assert!(matches!(origin, PropertyOrigin::Inherited { .. }));
    }
}
