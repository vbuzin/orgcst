//! Queries that combine a headline with its document: TODO state,
//! priorities, tags and properties.
//!
//! Tag and property results carry their origin (written here, inherited,
//! file-level). Applying Emacs user options such as
//! `org-use-tag-inheritance` or `org-use-property-inheritance` is left to the
//! caller.

use super::{
    DeclaredTags, Property, PropertyOrigin, TagGroup, TagGroupKind, lead_in_file_drawer,
    merged_drawer_properties, property_from_drawer, property_from_raw, tag_from_raw,
};
use crate::ast::nav;
use crate::ast::{AstNode, Heading, Headline, Tag, TagOrigin, Task};
use crate::settings::{
    declared_priority_range, file_keyword_properties, parse_filetags_from_settings,
    parse_tags_from_settings, property_key_eq,
};
use crate::{Document, PriorityRange, TodoKind, TodoSequence};

// ---------------------------------------------------------------------------
// Contextual queries
// ---------------------------------------------------------------------------

impl Document {
    /// Whether the headline's TODO keyword is a not-done or a done state.
    /// `None` when the headline has no TODO keyword.
    #[must_use]
    pub fn todo_kind(&self, heading: &impl Headline) -> Option<TodoKind> {
        let kw = heading.todo_keyword()?;
        crate::ast::layout::with_node_text(kw.syntax(), |s| self.config().todo_kind(s))
    }

    /// The TODO sequence the headline's keyword belongs to; the last one
    /// declared when several contain it.
    #[must_use]
    pub fn todo_sequence(&self, heading: &impl Headline) -> Option<&TodoSequence> {
        let kw = heading.todo_keyword()?;
        crate::ast::layout::with_node_text(kw.syntax(), |s| {
            self.config()
                .todo_sequences()
                .iter()
                .rfind(|seq| seq.keywords().iter().any(|k| k == s))
        })
    }

    /// The TODO sequences this document was parsed with: the caller's
    /// configuration with the file's `#+TODO:` lines applied.
    #[must_use]
    pub fn todo_sequences(&self) -> &[TodoSequence] {
        self.config().todo_sequences()
    }

    /// Whether a written priority lies in the declared `#+PRIORITIES:` range.
    ///
    /// Without a `#+PRIORITIES:` range every priority is valid; this crate
    /// does not assume Org's default of `A` to `C`.
    #[must_use]
    pub fn priority_is_valid(&self, priority: &crate::ast::Priority) -> bool {
        match self.priority_range() {
            None => true,
            Some(range) => range.contains(priority.value()),
        }
    }

    /// Declared `#+PRIORITIES:` range, if the file has a well-formed line.
    ///
    /// The first live line wins, including when it does not parse. A second
    /// line does not override it. A file with no `#+PRIORITIES:` line is
    /// `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use orgcst::{PriorityAtom, parse};
    ///
    /// let doc = parse("#+PRIORITIES: 1 9 5\n#+PRIORITIES: A C B\n");
    /// let range = doc.priority_range().expect("the first line parses");
    /// assert_eq!(range.highest, PriorityAtom::Number(1));
    /// assert_eq!(range.lowest, PriorityAtom::Number(9));
    /// assert_eq!(range.default, PriorityAtom::Number(5));
    ///
    /// assert!(parse("* heading\n").priority_range().is_none());
    /// ```
    #[must_use]
    pub fn priority_range(&self) -> Option<PriorityRange> {
        declared_priority_range(self.root())
    }

    /// `#+TAGS:` groups and free tags. `None` when no live line exists.
    #[must_use]
    pub fn declared_tags(&self) -> Option<DeclaredTags> {
        let (groups, free) = parse_tags_from_settings(self.root())?;
        Some(DeclaredTags::new(
            groups
                .into_iter()
                .map(|g| {
                    TagGroup::new(
                        if g.exclusive {
                            TagGroupKind::Exclusive
                        } else {
                            TagGroupKind::GroupTag
                        },
                        g.name.map(tag_from_raw),
                        g.members.into_iter().map(tag_from_raw).collect(),
                    )
                })
                .collect(),
            free.into_iter().map(tag_from_raw).collect(),
        ))
    }

    /// Tags from every live `#+FILETAGS:` line, in document order.
    pub fn file_tags(&self) -> impl Iterator<Item = Tag> + '_ {
        parse_filetags_from_settings(self.root())
            .into_iter()
            .map(tag_from_raw)
    }

    /// File-level properties: the property drawer at the top of the file,
    /// then the merged `#+PROPERTY:` lines.
    pub fn file_properties(&self) -> impl Iterator<Item = (Property, PropertyOrigin)> + '_ {
        file_properties_vec(self).into_iter()
    }

    /// The headline's own tags, then each ancestor outline's, then
    /// `#+FILETAGS:`, each with its origin.
    pub fn tags<'a>(
        &'a self,
        heading: &'a impl Headline,
    ) -> impl Iterator<Item = (Tag, TagOrigin)> + 'a {
        tag_chain(self, heading).into_iter()
    }

    /// The property `key` from the task's own drawer, without inheritance.
    ///
    /// `task` is a [`crate::ast::Outline`], a [`crate::ast::InlineTask`] or a
    /// [`Task`].
    #[must_use]
    pub fn property(&self, task: impl Into<Task>, key: &str) -> Option<Property> {
        property_from_drawer(task.into().property_drawer(), key)
    }

    /// The nearest value of `key` walking outwards, with where it came from.
    ///
    /// The order is the task's own drawer, then the ancestor outlines (for an
    /// inline task, its enclosing outline first), then the file's property
    /// drawer, then `#+PROPERTY:` lines. Values are inherited only from
    /// outlines, never from another inline task.
    #[must_use]
    pub fn property_inherited(
        &self,
        task: impl Into<Task>,
        key: &str,
    ) -> Option<(Property, PropertyOrigin)> {
        let task = task.into();
        if let Some(v) = property_from_drawer(task.property_drawer(), key) {
            return Some((v, PropertyOrigin::Local));
        }
        for outline in nav::inherited_outlines(task.syntax()) {
            if let Some(v) = property_from_drawer(outline.property_drawer(), key) {
                return Some((v, PropertyOrigin::Inherited { from: outline }));
            }
        }
        if let Some(drawer) = lead_in_file_drawer(self)
            && let Some(v) = property_from_drawer(Some(drawer), key)
        {
            return Some((v, PropertyOrigin::FileDrawer));
        }
        file_keyword_properties(self.root())
            .into_iter()
            .map(property_from_raw)
            .find(|p| property_key_eq(p.key(), key))
            .map(|v| (v, PropertyOrigin::FileKeyword))
    }
}

fn tag_chain(doc: &Document, heading: &impl Headline) -> Vec<(Tag, TagOrigin)> {
    let mut out = Vec::new();
    if let Some(tags) = heading.tags() {
        out.extend(tags.iter().map(|t| (t, TagOrigin::Written)));
    }
    for outline in nav::inherited_outlines(heading.syntax()) {
        push_inherited_tags(&mut out, &outline.heading());
    }
    out.extend(doc.file_tags().map(|t| (t, TagOrigin::FileTags)));
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

fn file_properties_vec(doc: &Document) -> Vec<(Property, PropertyOrigin)> {
    let mut out = Vec::new();
    if let Some(drawer) = lead_in_file_drawer(doc) {
        out.extend(
            merged_drawer_properties(&drawer)
                .into_iter()
                .map(|p| (p, PropertyOrigin::FileDrawer)),
        );
    }
    out.extend(
        file_keyword_properties(doc.root())
            .into_iter()
            .map(|p| (property_from_raw(p), PropertyOrigin::FileKeyword)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::*;

    #[test]
    fn todo_kind_agrees_with_tree() {
        let src = "#+TODO: NEXT | DONE\n* NEXT Hello\n* Plain\n";
        let doc = parse(src);
        let mut outlines = doc.root_outlines();
        let next = outlines.next().unwrap().heading();
        let plain = outlines.next().unwrap().heading();
        assert_eq!(doc.todo_kind(&next), Some(TodoKind::Active));
        assert!(doc.todo_kind(&plain).is_none());
        assert!(next.todo_keyword().is_some());
        assert!(plain.todo_keyword().is_none());
    }

    #[test]
    fn todo_sequence_takes_the_last_sequence_that_names_the_keyword() {
        let src = "#+TODO: X | B\n#+TODO: A | X\n* X Hello\n";
        let doc = parse(src);
        let h = doc.root_outlines().next().unwrap().heading();
        assert_eq!(doc.todo_kind(&h), Some(TodoKind::Done));
        let seq = doc.todo_sequence(&h).expect("sequence");
        assert_eq!(
            (
                seq.active().iter().map(String::as_str).collect::<Vec<_>>(),
                seq.done().iter().map(String::as_str).collect::<Vec<_>>(),
            ),
            (vec!["A"], vec!["X"])
        );

        let flipped = parse("#+TODO: A | X\n#+TODO: X | B\n* X Hello\n");
        let h = flipped.root_outlines().next().unwrap().heading();
        assert_eq!(flipped.todo_kind(&h), Some(TodoKind::Active));
        let seq = flipped.todo_sequence(&h).expect("sequence");
        assert_eq!(
            (
                seq.active().iter().map(String::as_str).collect::<Vec<_>>(),
                seq.done().iter().map(String::as_str).collect::<Vec<_>>(),
            ),
            (vec!["X"], vec!["B"])
        );
    }

    #[test]
    fn priority_validity_file_only() {
        let none = parse("* [#Z] H\n");
        let p = none
            .root_outlines()
            .next()
            .unwrap()
            .heading()
            .priority()
            .unwrap();
        assert!(none.priority_is_valid(&p));
        assert!(none.priority_range().is_none());

        let declared = parse("#+PRIORITIES: A C B\n* [#Z] H\n");
        let p = declared
            .root_outlines()
            .next()
            .unwrap()
            .heading()
            .priority()
            .unwrap();
        assert!(!declared.priority_is_valid(&p));
    }

    #[test]
    fn tags_provenance_and_filetags() {
        let src = "#+FILETAGS: :file:\n* Parent :p:\n** Child :c:\n";
        let doc = parse(src);
        let parent = doc.root_outlines().next().unwrap();
        let child = parent.children().next().unwrap().heading();
        let chain: Vec<_> = doc.tags(&child).collect();
        assert_eq!(chain[0].0.name(), "c");
        assert!(matches!(chain[0].1, TagOrigin::Written));
        assert_eq!(chain[1].0.name(), "p");
        assert!(matches!(chain[1].1, TagOrigin::Inherited { .. }));
        assert_eq!(chain[2].0.name(), "file");
        assert!(matches!(chain[2].1, TagOrigin::FileTags));
    }

    #[test]
    fn tags_on_task_outline_match_heading() {
        let src = "* Parent :p:\n** Child :c:\n";
        let doc = parse(src);
        let parent = doc.root_outlines().next().unwrap();
        let child = parent.children().next().unwrap();
        let via_task: Vec<_> = doc.tags(&Task::from(&child)).collect();
        let via_heading: Vec<_> = doc.tags(&child.heading()).collect();
        assert_eq!(via_task, via_heading);
        assert_eq!(via_task[0].0.name(), "c");
        assert!(matches!(via_task[0].1, TagOrigin::Written));
        assert_eq!(via_task[1].0.name(), "p");
        assert!(matches!(via_task[1].1, TagOrigin::Inherited { .. }));
    }

    #[test]
    fn inline_task_tag_walk_uses_enclosing_outline() {
        let src = "* Parent :p:\n*************** T :t:\n*************** END\n";
        let doc = parse(src);
        let task = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let names: Vec<_> = doc.tags(&task).map(|(t, _)| t.name().to_string()).collect();
        assert_eq!(names, ["t", "p"]);
        let via_task: Vec<_> = doc.tags(&Task::from(&task)).collect();
        let via_inline: Vec<_> = doc.tags(&task).collect();
        assert_eq!(via_task, via_inline);
    }

    #[test]
    fn property_local_versus_inherited() {
        let src = "* A\n:PROPERTIES:\n:FOO: a\n:END:\n** B\n:PROPERTIES:\n:BAR: b\n:END:\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();
        assert_eq!(doc.property(&b, "FOO").map(|p| p.value().to_string()), None);
        assert_eq!(
            doc.property_inherited(&b, "FOO")
                .map(|(p, _)| p.value().to_string()),
            Some("a".into())
        );
        assert_eq!(
            doc.property(&b, "BAR").map(|p| p.value().to_string()),
            Some("b".into())
        );
        let inherited = doc.property_inherited(&b, "FOO").unwrap();
        assert!(
            matches!(inherited.1, PropertyOrigin::Inherited { from } if from.syntax() == a.syntax())
        );
    }

    #[test]
    fn property_on_inline_task_local_and_inherited() {
        let src = "* Parent\n:PROPERTIES:\n:FOO: from-parent\n:END:\n*************** T\n:PROPERTIES:\n:BAR: from-task\n:END:\n*************** END\n";
        let doc = parse(src);
        let parent = doc.root_outlines().next().unwrap();
        let task = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            doc.property(&task, "BAR").map(|p| p.value().to_string()),
            Some("from-task".into())
        );
        assert_eq!(
            doc.property(&task, "FOO").map(|p| p.value().to_string()),
            None
        );
        let inherited = doc.property_inherited(&task, "FOO").unwrap();
        assert_eq!(inherited.0.value(), "from-parent");
        assert!(matches!(
            inherited.1,
            PropertyOrigin::Inherited { from } if from.syntax() == parent.syntax()
        ));
        let local = doc.property_inherited(&task, "BAR").unwrap();
        assert_eq!(local.0.value(), "from-task");
        assert!(matches!(local.1, PropertyOrigin::Local));

        let no_drawer = parse(
            "* Parent\n:PROPERTIES:\n:FOO: from-parent\n:END:\n*************** T\n*************** END\n",
        );
        let parent = no_drawer.root_outlines().next().unwrap();
        let task = InlineTask::cast(
            nodes_of_kind(no_drawer.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(no_drawer.property(&task, "FOO").is_none());
        let inherited = no_drawer.property_inherited(&task, "FOO").unwrap();
        assert_eq!(inherited.0.value(), "from-parent");
        assert!(matches!(
            inherited.1,
            PropertyOrigin::Inherited { from } if from.syntax() == parent.syntax()
        ));
    }

    #[test]
    fn property_inherited_walks_past_the_immediate_outline() {
        let src = "* A\n:PROPERTIES:\n:FOO: from-a\n:END:\n** B\n*** C\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let b = a.children().next().unwrap();
        let c = b.children().next().unwrap();
        assert!(doc.property(&c, "FOO").is_none());
        let inherited = doc.property_inherited(&c, "FOO").unwrap();
        assert_eq!(inherited.0.value(), "from-a");
        assert!(matches!(
            inherited.1,
            PropertyOrigin::Inherited { from } if from.syntax() == a.syntax()
        ));

        let src = "* A\n:PROPERTIES:\n:FOO: from-a\n:END:\n** B\n*************** T\n*************** END\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        let task = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(doc.property(&task, "FOO").is_none());
        let inherited = doc.property_inherited(&task, "FOO").unwrap();
        assert_eq!(inherited.0.value(), "from-a");
        assert!(matches!(
            inherited.1,
            PropertyOrigin::Inherited { from } if from.syntax() == a.syntax()
        ));
    }

    #[test]
    fn filetags_both_spellings_and_accumulate() {
        let doc = parse("#+FILETAGS: :a:b:\n#+FILETAGS: c d\n");
        let names: Vec<_> = doc.file_tags().map(|t| t.name().to_string()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
    }

    #[test]
    fn declared_tags_exclusive_and_group_tag() {
        let doc = parse("#+TAGS: { a(x) b }\n#+TAGS: [work : @office @home]\n");
        let tags = doc.declared_tags().expect("tags");
        assert_eq!(tags.groups().len(), 2);
        assert_eq!(tags.groups()[0].kind(), TagGroupKind::Exclusive);
        assert_eq!(tags.groups()[0].members()[0].name(), "a");
        assert_eq!(tags.groups()[0].members()[0].fast_key(), Some('x'));
        assert_eq!(tags.groups()[1].kind(), TagGroupKind::GroupTag);
        assert_eq!(tags.groups()[1].name().unwrap().name(), "work");
        assert_eq!(
            tags.groups()[1]
                .members()
                .iter()
                .map(crate::Tag::name)
                .collect::<Vec<_>>(),
            ["@office", "@home"]
        );
    }

    #[test]
    fn property_keyword_merge_caseless_append() {
        let doc = parse("#+PROPERTY: FOO bar\n#+PROPERTY: foo+ baz\n");
        let props: Vec<_> = doc.file_properties().collect();
        assert_eq!(props.len(), 1);
        assert!(props[0].0.base_key().eq_ignore_ascii_case("FOO"));
        assert_eq!(props[0].0.value(), "bar baz");
        assert!(matches!(props[0].1, PropertyOrigin::FileKeyword));
    }

    #[test]
    fn drawer_property_merge_uses_the_same_rule() {
        let src = "* A\n:PROPERTIES:\n:FOO: bar\n:foo+: baz\n:END:\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        assert_eq!(
            doc.property(&a, "FOO").map(|p| p.value().to_string()),
            Some("bar baz".into())
        );
    }

    #[test]
    fn file_properties_lead_in_drawer_not_mid_section() {
        let lead = parse(":PROPERTIES:\n:ID: file\n:END:\npara\n");
        let got: Vec<_> = lead.file_properties().collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.base_key(), "ID");
        assert!(matches!(got[0].1, PropertyOrigin::FileDrawer));

        let mid = parse("para\n:PROPERTIES:\n:ID: no\n:END:\n");
        assert!(mid.file_properties().next().is_none());
    }
}
