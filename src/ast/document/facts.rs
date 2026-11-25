//! Values a document declares (`#+MACRO:`, `#+LINK:`, `#+CATEGORY:`,
//! `#+ARCHIVE:`) and typed accessors for one entry.

use super::{Property, merged_drawer_properties, source_slice};
use crate::ast::{
    AstNode, Clock, HeadingTitle, Headline, Keyword, Planning, Tag, TagOrigin, Task, Timestamp,
    TodoKeyword,
};
use crate::settings::{parse_link_abbrev_value, parse_macro_value};
use crate::{Document, TextRange};

/// One live `#+MACRO: name(args) body` definition. This crate does not
/// expand macros.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MacroDef {
    name: String,
    args: Vec<String>,
    body: String,
    range: TextRange,
}

impl MacroDef {
    /// The macro name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Argument names from an optional `(a, b)` after the name.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The template text after the name and arguments.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Range of the whole `#+MACRO:` line.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }
}

/// One live `#+LINK: key template` abbreviation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkAbbrev {
    key: String,
    template: String,
    range: TextRange,
}

impl LinkAbbrev {
    /// The abbreviation, used as `[[key:tag]]`.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// What the abbreviation expands to.
    #[must_use]
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Range of the whole `#+LINK:` line.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }
}

/// Typed accessors for one entry: headline fields, planning and properties.
/// From [`Document::entry_facts`].
pub struct EntryFacts<'a> {
    doc: &'a Document,
    task: &'a Task,
}

impl<'a> EntryFacts<'a> {
    /// The headline title.
    #[must_use]
    pub fn title(&self) -> HeadingTitle {
        self.task.title()
    }

    /// The TODO keyword, if any.
    #[must_use]
    pub fn todo_keyword(&self) -> Option<TodoKeyword> {
        self.task.todo_keyword()
    }

    /// The priority cookie, if any.
    #[must_use]
    pub fn priority(&self) -> Option<crate::ast::Priority> {
        self.task.priority()
    }

    /// The tags written on the headline, if any.
    #[must_use]
    pub fn tags_written(&self) -> Option<crate::ast::Tags> {
        self.task.tags()
    }

    /// Own, inherited and file tags with their origins; see
    /// [`Document::tags`].
    pub fn tags_chain(&self) -> impl Iterator<Item = (Tag, TagOrigin)> + 'a {
        self.doc.tags(self.task)
    }

    /// The planning line, if any.
    #[must_use]
    pub fn planning(&self) -> Option<Planning> {
        self.task.planning()
    }

    /// The `DEADLINE:` timestamp, if any.
    #[must_use]
    pub fn deadline(&self) -> Option<Timestamp> {
        self.task.planning().and_then(|p| p.deadline())
    }

    /// The `SCHEDULED:` timestamp, if any.
    #[must_use]
    pub fn scheduled(&self) -> Option<Timestamp> {
        self.task.planning().and_then(|p| p.scheduled())
    }

    /// The `CLOSED:` timestamp, if any.
    #[must_use]
    pub fn closed(&self) -> Option<Timestamp> {
        self.task.planning().and_then(|p| p.closed())
    }

    /// The entry's own properties, with `KEY+` lines merged.
    pub fn properties_local(&self) -> impl Iterator<Item = Property> + 'a {
        local_properties(self.task).into_iter()
    }

    /// Clock lines in the subtree of an outline, or inside an inline task.
    pub fn clocks_in_subtree(&self) -> impl Iterator<Item = Clock> + 'a {
        clocks_in_task(self.doc, self.task).into_iter()
    }
}

impl Document {
    /// Every live `#+MACRO:` definition, in document order.
    pub fn macros(&self) -> impl Iterator<Item = MacroDef> + '_ {
        self.keywords().filter_map(|kw| {
            if !kw.key_eq("macro") {
                return None;
            }
            let (name, args, body) = parse_macro_value(keyword_value_text(self, &kw))?;
            Some(MacroDef {
                name,
                args,
                body,
                range: kw.range(),
            })
        })
    }

    /// Every live `#+LINK:` abbreviation, in document order.
    pub fn link_abbrevs(&self) -> impl Iterator<Item = LinkAbbrev> + '_ {
        self.keywords().filter_map(|kw| {
            if !kw.key_eq("link") {
                return None;
            }
            let (key, template) = parse_link_abbrev_value(keyword_value_text(self, &kw))?;
            Some(LinkAbbrev {
                key,
                template,
                range: kw.range(),
            })
        })
    }

    /// The value of the last live `#+ARCHIVE:` line, wherever it is.
    #[must_use]
    pub fn archive_location(&self) -> Option<&str> {
        last_keyword_content(self, "archive")
    }

    /// The value of the last live `#+CATEGORY:` line, wherever it is.
    #[must_use]
    pub fn category(&self) -> Option<&str> {
        last_keyword_content(self, "category")
    }

    /// Typed accessors for `task`'s fields and properties.
    #[must_use]
    pub fn entry_facts<'a>(&'a self, task: &'a Task) -> EntryFacts<'a> {
        EntryFacts { doc: self, task }
    }
}

fn keyword_value_text<'a>(doc: &'a Document, kw: &Keyword) -> &'a str {
    source_slice(doc.source(), kw.value().content_range())
}

fn last_keyword_content<'a>(doc: &'a Document, key: &str) -> Option<&'a str> {
    let mut last = None;
    for kw in doc.keywords() {
        if kw.key_eq(key) {
            last = Some(kw);
        }
    }
    last.map(|kw| source_slice(doc.source(), kw.value().content_range()))
}

fn local_properties(task: &Task) -> Vec<Property> {
    task.property_drawer()
        .map_or_default(|d| merged_drawer_properties(&d))
}

fn clocks_in_task(doc: &Document, task: &Task) -> Vec<Clock> {
    let span = match task {
        Task::Outline(o) => o.subtree_range(),
        Task::InlineTask(_) => task.entry_range(),
    };
    doc.clocks()
        .filter(|c| span.contains_range(c.range()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn macros_link_abbrevs_archive_and_category() {
        let src = concat!(
            "#+MACRO: greet(name) Hello $1\n",
            "#+MACRO: bad(unclosed\n",
            "#+LINK: gh https://github.com/%s\n",
            "#+ARCHIVE: archive.org\n",
            "#+CATEGORY: first\n",
            "#+CATEGORY: filecat\n",
            "* H\n",
        );
        let doc = parse(src);
        let macros: Vec<_> = doc.macros().collect();
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name(), "greet");
        assert_eq!(macros[0].args(), &["name".to_string()]);
        let abbrevs: Vec<_> = doc.link_abbrevs().collect();
        assert_eq!(abbrevs.len(), 1);
        assert_eq!(abbrevs[0].key(), "gh");
        assert_eq!(doc.archive_location(), Some("archive.org"));
        assert_eq!(doc.category(), Some("filecat"));
    }

    #[test]
    fn entry_facts_read_the_entry() {
        let doc = parse(concat!(
            "* TODO [#A] H :a:\n",
            "DEADLINE: <2020-01-01>\n",
            ":PROPERTIES:\n:K: v\n:K+: w\n:END:\n",
            "CLOCK: [2020-01-02 Thu 10:00]--[2020-01-02 Thu 11:00] =>  1:00\n",
        ));
        let task = Task::Outline(doc.outlines().next().expect("one outline"));
        let facts = doc.entry_facts(&task);
        assert_eq!(doc.slice(facts.title().range()).trim_end(), "H");
        assert!(facts.todo_keyword().is_some());
        assert!(facts.priority().is_some());
        assert!(facts.deadline().is_some());
        assert!(facts.scheduled().is_none());
        let props: Vec<_> = facts.properties_local().collect();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].value(), "v w");
        assert_eq!(facts.clocks_in_subtree().count(), 1);
    }
}
