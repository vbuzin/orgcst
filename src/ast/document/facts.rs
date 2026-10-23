//! Values a document declares (`#+MACRO:`, `#+LINK:`, `#+CATEGORY:`, babel
//! header arguments) and the special properties of an entry.

use super::{
    Property, PropertyOrigin, lead_in_file_drawer, merged_drawer_properties, property_from_raw,
    raw_from_node, source_slice,
};
use crate::ast::layout::with_node_text;
use crate::ast::nav;
use crate::ast::{
    AstNode, Block, BlockKind, BlockPart, Clock, ElementView, HasParts, HeadingPart, HeadingTitle,
    Headline, Keyword, Planning, PropertyDrawer, Tag, TagOrigin, Task, Timestamp, TimestampShape,
    TodoKeyword,
};
use crate::settings::{
    RawProperty, header_args_key_matches, parse_babel_header_args, parse_link_abbrev_value,
    parse_macro_value, raw_file_property_lines,
};
use crate::syntax::OrgSyntaxKind;
use crate::{Document, TextRange, TextSize};
use std::borrow::Cow;

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

/// Where one [`Document::header_args`] row came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HeaderArgsOrigin {
    /// A `header-args` property, from a drawer or a `#+PROPERTY:` line.
    Property {
        /// Where the property came from.
        from: PropertyOrigin,
    },
    /// A `#+HEADER:` line attached to the block.
    Affiliated {
        /// The `#+HEADER:` line.
        keyword: Keyword,
    },
    /// The src block's begin line.
    BeginLine {
        /// The src block.
        block: Block,
    },
}

/// The special properties of an entry, as listed in the Org manual
/// ("Special Properties"). They are computed, never read from a drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryFact {
    /// `ALLTAGS`: every tag, inherited ones included.
    AllTags,
    /// `BLOCKED`: whether TODO dependencies block the entry.
    Blocked,
    /// `CATEGORY`.
    Category,
    /// `CLOCKSUM`: total clocked time in the subtree.
    Clocksum,
    /// `CLOCKSUM_T`: time clocked today.
    ClocksumToday,
    /// `CLOSED`.
    Closed,
    /// `DEADLINE`.
    Deadline,
    /// `FILE`: the file the entry is in.
    File,
    /// `ITEM`: the headline text.
    Item,
    /// `PRIORITY`.
    Priority,
    /// `SCHEDULED`.
    Scheduled,
    /// `TAGS`: tags written on the headline.
    Tags,
    /// `TIMESTAMP`: the first active timestamp in the entry.
    Timestamp,
    /// `TIMESTAMP_IA`: the first inactive timestamp in the entry.
    TimestampIa,
    /// `TODO`.
    Todo,
}

/// The value of one [`EntryFact`], in its natural type.
///
/// [`Self::Text`] borrows from the source when the value is a slice of it,
/// and owns the string when it comes from a merged property.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryFactValue<'a> {
    /// Tags with their origins.
    TagChain(Vec<(Tag, TagOrigin)>),
    /// A timestamp.
    Timestamp(Timestamp),
    /// Plain text, such as the headline or the category.
    Text(Cow<'a, str>),
    /// A priority cookie.
    Priority(crate::ast::Priority),
    /// A TODO keyword.
    Todo(TodoKeyword),
}

/// Typed accessors for one entry: headline fields, planning, properties and
/// the values behind its special properties. From [`Document::entry_facts`].
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

    /// The entry's own `ORDERED` property, if any.
    #[must_use]
    pub fn ordered(&self) -> Option<Property> {
        self.doc.property(self.task, "ORDERED")
    }

    /// TODO keywords of the direct child outlines.
    pub fn child_todo_keywords(&self) -> impl Iterator<Item = TodoKeyword> + 'a {
        child_todos(self.task).into_iter()
    }

    /// The entry's own `CATEGORY` property, if any.
    #[must_use]
    pub fn category_property(&self) -> Option<Property> {
        self.doc.property(self.task, "CATEGORY")
    }

    /// The first active timestamp in the entry's own text, outside planning,
    /// clock lines and the property drawer.
    #[must_use]
    pub fn timestamp(&self) -> Option<Timestamp> {
        first_entry_timestamp(self.doc, self.task, true)
    }

    /// The first inactive timestamp in the entry's own text, outside
    /// planning, clock lines and the property drawer.
    #[must_use]
    pub fn timestamp_ia(&self) -> Option<Timestamp> {
        first_entry_timestamp(self.doc, self.task, false)
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

    /// Header arguments that apply to `block`, from least to most specific.
    ///
    /// The order is `#+PROPERTY:` lines, the file's property drawer, the
    /// ancestor outlines' drawers (outermost first), the enclosing entry's own
    /// drawer, the block's `#+HEADER:` lines, then a src block's begin line.
    /// Later rows override earlier ones; the rows are not merged. The
    /// enclosing entry is the nearest inline task, else the nearest outline;
    /// a block before the first heading has none.
    pub fn header_args<'a>(
        &'a self,
        block: &'a Block,
    ) -> impl Iterator<Item = (Property, HeaderArgsOrigin)> + 'a {
        header_args_vec(self, block).into_iter()
    }

    /// The special property `which` of `task`, or `None` when the entry has
    /// no such value.
    ///
    /// Always `None` for [`EntryFact::Blocked`], [`EntryFact::Clocksum`],
    /// [`EntryFact::ClocksumToday`] and [`EntryFact::File`]: they depend on
    /// options, the clock or the file system, which a parsed document does
    /// not know.
    #[must_use]
    pub fn special<'a>(&'a self, task: &'a Task, which: EntryFact) -> Option<EntryFactValue<'a>> {
        let facts = self.entry_facts(task);
        match which {
            EntryFact::AllTags => Some(EntryFactValue::TagChain(facts.tags_chain().collect())),
            EntryFact::Blocked
            | EntryFact::Clocksum
            | EntryFact::ClocksumToday
            | EntryFact::File => None,
            EntryFact::Category => facts
                .category_property()
                .map(|p| EntryFactValue::Text(Cow::Owned(p.value().to_string())))
                .or_else(|| {
                    self.category()
                        .map(|s| EntryFactValue::Text(Cow::Borrowed(s)))
                }),
            EntryFact::Closed => facts.closed().map(EntryFactValue::Timestamp),
            EntryFact::Deadline => facts.deadline().map(EntryFactValue::Timestamp),
            EntryFact::Scheduled => facts.scheduled().map(EntryFactValue::Timestamp),
            EntryFact::Item => {
                let r = task.part_content_range(HeadingPart::Title)?;
                Some(EntryFactValue::Text(Cow::Borrowed(source_slice(
                    self.source(),
                    r,
                ))))
            }
            EntryFact::Priority => facts.priority().map(EntryFactValue::Priority),
            EntryFact::Tags => {
                let tags = facts.tags_written()?;
                Some(EntryFactValue::TagChain(
                    tags.iter().map(|t| (t, TagOrigin::Written)).collect(),
                ))
            }
            EntryFact::Timestamp => facts.timestamp().map(EntryFactValue::Timestamp),
            EntryFact::TimestampIa => facts.timestamp_ia().map(EntryFactValue::Timestamp),
            EntryFact::Todo => facts.todo_keyword().map(EntryFactValue::Todo),
        }
    }

    /// Typed accessors for `task`'s fields, properties and special properties.
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

fn header_args_vec(doc: &Document, block: &Block) -> Vec<(Property, HeaderArgsOrigin)> {
    let mut out = Vec::new();
    let lang = block
        .part_content_range(BlockPart::Language)
        .map(|r| source_slice(doc.source(), r));

    for raw in raw_file_property_lines(doc.root()) {
        push_matching_raw(
            &mut out,
            &raw,
            lang,
            &HeaderArgsOrigin::Property {
                from: PropertyOrigin::FileKeyword,
            },
        );
    }
    if let Some(drawer) = lead_in_file_drawer(doc) {
        push_matching_drawer(
            &mut out,
            &drawer,
            lang,
            &HeaderArgsOrigin::Property {
                from: PropertyOrigin::FileDrawer,
            },
        );
    }

    let task = nav::enclosing::<crate::ast::InlineTask>(block.syntax())
        .map(Task::InlineTask)
        .or_else(|| nav::enclosing_outline(block.syntax()).map(Task::Outline));
    if let Some(task) = task {
        let inherited: Vec<_> = nav::inherited_outlines(task.syntax()).collect();
        for outline in inherited.into_iter().rev() {
            if let Some(drawer) = outline.property_drawer() {
                push_matching_drawer(
                    &mut out,
                    &drawer,
                    lang,
                    &HeaderArgsOrigin::Property {
                        from: PropertyOrigin::Inherited {
                            from: outline.clone(),
                        },
                    },
                );
            }
        }
        if let Some(drawer) = task.property_drawer() {
            push_matching_drawer(
                &mut out,
                &drawer,
                lang,
                &HeaderArgsOrigin::Property {
                    from: PropertyOrigin::Local,
                },
            );
        }
    }

    if let Some(run) = block.affiliated() {
        for kw in run.headers() {
            let val = source_slice(doc.source(), kw.value().content_range());
            push_parsed_args(
                &mut out,
                val,
                kw.value().content_range().start(),
                &HeaderArgsOrigin::Affiliated {
                    keyword: kw.clone(),
                },
            );
        }
    }

    if block.kind() == BlockKind::Src
        && let Some(r) = block.part_content_range(BlockPart::Parameters)
    {
        let val = source_slice(doc.source(), r);
        push_parsed_args(
            &mut out,
            val,
            r.start(),
            &HeaderArgsOrigin::BeginLine {
                block: block.clone(),
            },
        );
    }
    out
}

fn push_parsed_args(
    out: &mut Vec<(Property, HeaderArgsOrigin)>,
    val: &str,
    base: TextSize,
    origin: &HeaderArgsOrigin,
) {
    for p in parse_babel_header_args(val, base) {
        out.push((property_from_raw(p), origin.clone()));
    }
}

fn push_matching_raw(
    out: &mut Vec<(Property, HeaderArgsOrigin)>,
    raw: &RawProperty,
    lang: Option<&str>,
    origin: &HeaderArgsOrigin,
) {
    if header_args_key_matches(&raw.key, lang) {
        push_parsed_args(out, &raw.value, raw.range.start(), origin);
    }
}

fn push_matching_drawer(
    out: &mut Vec<(Property, HeaderArgsOrigin)>,
    drawer: &PropertyDrawer,
    lang: Option<&str>,
    origin: &HeaderArgsOrigin,
) {
    for np in drawer.properties() {
        if with_node_text(&np.key_node(), |s| header_args_key_matches(s, lang)) {
            let raw = raw_from_node(&np);
            push_parsed_args(out, &raw.value, raw.range.start(), origin);
        }
    }
}

fn local_properties(task: &Task) -> Vec<Property> {
    task.property_drawer()
        .map(|d| merged_drawer_properties(&d))
        .unwrap_or_default()
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

fn child_todos(task: &Task) -> Vec<TodoKeyword> {
    match task {
        Task::Outline(o) => o
            .children()
            .filter_map(|c| c.heading().todo_keyword())
            .collect(),
        Task::InlineTask(_) => Vec::new(),
    }
}

fn first_entry_timestamp(doc: &Document, task: &Task, active: bool) -> Option<Timestamp> {
    let span = task.entry_range();
    doc.map::<Timestamp>().find(|ts| {
        let ok_shape = if active {
            matches!(
                ts.shape(),
                TimestampShape::Active | TimestampShape::ActiveRange
            )
        } else {
            matches!(
                ts.shape(),
                TimestampShape::Inactive | TimestampShape::InactiveRange
            )
        };
        ok_shape
            && span.contains_range(ts.range())
            && !nav::ancestors(ts.syntax()).any(|n| {
                matches!(
                    n.kind(),
                    OrgSyntaxKind::CLOCK | OrgSyntaxKind::PLANNING | OrgSyntaxKind::PROPERTY_DRAWER
                )
            })
    })
}

#[cfg(test)]
mod tests {
    use super::super::span;
    use super::*;
    use crate::ast::*;
    use crate::parse;

    #[test]
    fn settings_macros_link_archive_category_header_args_facts() {
        let src = concat!(
            "#+MACRO: greet(name) Hello $1\n",
            "#+MACRO: bad(unclosed\n",
            "#+LINK: gh https://github.com/%s\n",
            "#+ARCHIVE: archive.org\n",
            "#+CATEGORY: first\n",
            "#+CATEGORY: filecat\n",
            ":PROPERTIES:\n:header-args+: :exports both\n:END:\n",
            "* H\nDEADLINE: <2020-01-01>\n:PROPERTIES:\n:header-args: :from local\n:CATEGORY: propcat\n:ORDERED: t\n:END:\n",
            "CLOCK: [2020-01-02]\n<2020-01-03>\n",
            "#+begin_src python :results output\nx\n#+end_src\n",
            "#+begin: dyn :foo bar\n#+end:\n",
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

        let src_block = doc
            .map::<Block>()
            .find(|b| b.kind() == crate::ast::BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc.header_args(&src_block).collect();
        assert!(chain.iter().any(|(_, o)| matches!(
            o,
            HeaderArgsOrigin::Property {
                from: PropertyOrigin::Local
            }
        )));
        assert!(
            chain
                .iter()
                .any(|(_, o)| matches!(o, HeaderArgsOrigin::BeginLine { .. }))
        );

        let drawer_only = parse(
            ":PROPERTIES:\n:header-args+: :exports both\n:END:\n\n#+begin_src python\nx\n#+end_src\n",
        );
        let b = drawer_only
            .map::<Block>()
            .find(|b| b.kind() == crate::ast::BlockKind::Src)
            .unwrap();
        let file_chain: Vec<_> = drawer_only.header_args(&b).collect();
        assert!(
            file_chain.iter().any(|(_, o)| matches!(
                o,
                HeaderArgsOrigin::Property {
                    from: PropertyOrigin::FileDrawer
                }
            )),
            "file drawer :header-args+: must appear: {file_chain:?}"
        );
        let dyn_block = doc
            .map::<Block>()
            .find(|b| b.kind() == crate::ast::BlockKind::Dynamic)
            .unwrap();
        let dyn_chain: Vec<_> = doc.header_args(&dyn_block).collect();
        assert!(
            !dyn_chain
                .iter()
                .any(|(_, o)| matches!(o, HeaderArgsOrigin::BeginLine { .. }))
        );

        let task = Task::from(doc.root_outlines().next().unwrap());
        assert!(matches!(
            doc.special(&task, EntryFact::Deadline),
            Some(EntryFactValue::Timestamp(_))
        ));
        let ts = match doc.special(&task, EntryFact::Timestamp) {
            Some(EntryFactValue::Timestamp(t)) => t,
            other => panic!("{other:?}"),
        };
        assert!(ts.range().intersect(span(src, "<2020-01-03>")).is_some());
        assert!(doc.special(&task, EntryFact::Clocksum).is_none());
        assert!(matches!(
            doc.special(&task, EntryFact::Category),
            Some(EntryFactValue::Text(ref s)) if s.as_ref() == "propcat"
        ));
        assert!(doc.special(&task, EntryFact::TimestampIa).is_none());
        let ia_doc = parse("* H\nCLOCK: [2020-01-02]\n[2020-01-04]\n");
        let ia_task = Task::from(ia_doc.root_outlines().next().unwrap());
        let ia = match ia_doc.special(&ia_task, EntryFact::TimestampIa) {
            Some(EntryFactValue::Timestamp(t)) => t,
            other => panic!("{other:?}"),
        };
        assert!(
            ia.range()
                .intersect(span(
                    "* H\nCLOCK: [2020-01-02]\n[2020-01-04]\n",
                    "[2020-01-04]"
                ))
                .is_some()
        );
        let facts = doc.entry_facts(&task);
        assert_eq!(facts.clocks_in_subtree().count(), 1);
        assert!(facts.ordered().is_some());
    }

    #[test]
    fn header_args_walk_enclosing_task_not_block() {
        let src = concat!(
            "* A\n:PROPERTIES:\n:header-args: :from a\n:END:\n",
            "** B\n#+begin_src python\nx\n#+end_src\n",
        );
        let doc = parse(src);
        let block = doc
            .map::<Block>()
            .find(|b| b.kind() == crate::ast::BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc.header_args(&block).collect();
        assert!(
            chain.iter().any(|(p, o)| {
                (p.key() == "from" || p.value().contains('a'))
                    && matches!(
                        o,
                        HeaderArgsOrigin::Property {
                            from: PropertyOrigin::Inherited { .. }
                        }
                    )
            }),
            "{chain:?}"
        );
    }

    #[test]
    fn special_category_uses_property_value_not_range() {
        let empty = parse("* H\n:PROPERTIES:\n:CATEGORY:\n:END:\n");
        let task = Task::from(empty.root_outlines().next().unwrap());
        assert!(matches!(
            empty.special(&task, EntryFact::Category),
            Some(EntryFactValue::Text(ref s)) if s.as_ref() == ""
        ));
        let merged = parse("* H\n:PROPERTIES:\n:CATEGORY: foo\n:category+: bar\n:END:\n");
        let task = Task::from(merged.root_outlines().next().unwrap());
        assert!(matches!(
            merged.special(&task, EntryFact::Category),
            Some(EntryFactValue::Text(ref s)) if s.as_ref() == "foo bar"
        ));
    }

    #[test]
    fn header_args_chain_order_and_origins() {
        let src = concat!(
            ":PROPERTIES:\n:header-args: :file drawer\n:END:\n",
            "#+PROPERTY: header-args :file kw\n",
            "* A\n:PROPERTIES:\n:header-args: :from parent\n:END:\n",
            "** B\n:PROPERTIES:\n:header-args: :from local\n:END:\n",
            "#+HEADER: :from header\n#+begin_src python :results output\nx\n#+end_src\n",
        );
        let doc = parse(src);
        let block = doc
            .map::<Block>()
            .find(|b| b.kind() == BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc
            .header_args(&block)
            .map(|(p, o)| (p.key().to_string(), p.value().to_string(), origin_tag(&o)))
            .collect();
        let tags: Vec<_> = chain.iter().map(|(_, _, t)| *t).collect();
        assert_eq!(
            tags,
            [
                "FileKeyword",
                "FileDrawer",
                "Inherited",
                "Local",
                "Affiliated",
                "BeginLine",
            ],
            "{chain:?}"
        );
        let values: Vec<_> = chain.iter().map(|(_, v, _)| v.as_str()).collect();
        assert_eq!(
            values,
            ["kw", "drawer", "parent", "local", "header", "output"],
            "{chain:?}"
        );

        let src = concat!(
            "* Outer\n:PROPERTIES:\n:header-args: :from outline\n:END:\n",
            "*************** T\n:PROPERTIES:\n:header-args: :from task\n:END:\n",
            "#+begin_src python\nx\n#+end_src\n",
            "*************** END\n",
        );
        let doc = parse(src);
        let block = doc
            .map::<Block>()
            .find(|b| b.kind() == BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc.header_args(&block).collect();
        assert!(chain.iter().any(|(p, o)| {
            p.value().contains("task")
                && matches!(
                    o,
                    HeaderArgsOrigin::Property {
                        from: PropertyOrigin::Local
                    }
                )
        }));

        let src = concat!(
            "#+begin_src python\nx\n#+end_src\n",
            "* Later\n:PROPERTIES:\n:header-args: :from later\n:END:\n",
        );
        let doc = parse(src);
        let block = doc
            .map::<Block>()
            .find(|b| b.kind() == BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc.header_args(&block).collect();
        assert!(
            !chain.iter().any(|(p, _)| p.value().contains("later")),
            "{chain:?}"
        );

        let src = "#+begin_export html :file x\nbody\n#+end_export\n#+begin_special foo :bar baz\n#+end_special\n";
        let doc = parse(src);
        let skipped: Vec<_> = doc
            .map::<Block>()
            .filter(|b| matches!(b.kind(), BlockKind::Export | BlockKind::Special))
            .collect();
        assert_eq!(skipped.len(), 2);
        for b in &skipped {
            assert!(
                !doc.header_args(b)
                    .any(|(_, o)| matches!(o, HeaderArgsOrigin::BeginLine { .. }))
            );
        }

        let src = "* H\n:PROPERTIES:\n:header-args:python: :lang py\n:END:\n#+begin_src python\nx\n#+end_src\n";
        let doc = parse(src);
        let block = doc
            .map::<Block>()
            .find(|b| b.kind() == BlockKind::Src)
            .unwrap();
        let chain: Vec<_> = doc.header_args(&block).collect();
        assert!(
            chain.iter().any(|(p, _)| p.value().contains("py")),
            "{chain:?}"
        );
    }

    fn origin_tag(o: &HeaderArgsOrigin) -> &'static str {
        match o {
            HeaderArgsOrigin::Property {
                from: PropertyOrigin::FileKeyword,
            } => "FileKeyword",
            HeaderArgsOrigin::Property {
                from: PropertyOrigin::FileDrawer,
            } => "FileDrawer",
            HeaderArgsOrigin::Property {
                from: PropertyOrigin::Inherited { .. },
            } => "Inherited",
            HeaderArgsOrigin::Property {
                from: PropertyOrigin::Local,
            } => "Local",
            HeaderArgsOrigin::Affiliated { .. } => "Affiliated",
            HeaderArgsOrigin::BeginLine { .. } => "BeginLine",
        }
    }
}
