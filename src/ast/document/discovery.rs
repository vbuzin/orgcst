//! Iterators over each kind of construct, and lookups by identity: ID,
//! custom ID, `#+NAME:`, footnote label, and the link address of a position.

use super::{region_hits, source_slice};
use crate::ast::{
    AstNode, Clock, Drawer, Element, ElementView, FootnoteDefinition, FootnotePart,
    FootnoteReference, HasParts, Heading, HeadingPart, Keyword, Link, Object, Outline, PlainList,
    RadioTarget, Table, Target, Task, Timestamp,
};
use crate::syntax::{OrgSyntaxKind, SyntaxNode, restrictions};
use crate::{Document, TextRange, TextSize};

/// Text that matches a radio target of the same file, which Org's syntax
/// reads as a radio link: the answer of [`Document::radio_links`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RadioLink {
    range: TextRange,
    target: RadioTarget,
}

impl RadioLink {
    /// The matched text's range.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// The radio target it matches.
    #[must_use]
    pub fn target(&self) -> &RadioTarget {
        &self.target
    }
}

/// How an internal link could point at a position: the answer of
/// [`Document::address_at`].
///
/// Building one allocates. This crate does not resolve fuzzy links back to
/// positions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InternalAddress {
    /// A `<<target>>` around the position.
    Target {
        /// Text between `<<` and `>>`.
        name: String,
        /// Range of the whole target.
        range: TextRange,
    },
    /// An element with a `#+NAME:`.
    Named {
        /// The `#+NAME:` value.
        name: String,
        /// The named element.
        element: Element,
    },
    /// An outline with a `CUSTOM_ID` property, for `[[#id]]`.
    CustomId {
        /// The `CUSTOM_ID` value.
        id: String,
        /// The outline with that property.
        outline: Outline,
    },
    /// An outline with an `ID` property, for `[[id:...]]`.
    Id {
        /// The `ID` value.
        id: String,
        /// The outline with that property.
        outline: Outline,
    },
    /// An outline reached by its title, for `[[*Title]]` or `[[Title]]`.
    FuzzyTitle {
        /// The heading's title text.
        title: String,
        /// The outline with that title.
        outline: Outline,
    },
}

/// A footnote label's definition and the references that use it.
///
/// Both are empty when nothing matches; references without a definition
/// leave [`Self::definition`] `None`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Footnote {
    definition: Option<FootnoteDefinition>,
    references: Vec<FootnoteReference>,
}

impl Footnote {
    /// The `[fn:label]` definition, if the document has one.
    #[must_use]
    pub fn definition(&self) -> Option<&FootnoteDefinition> {
        self.definition.as_ref()
    }

    /// References with this label, in document order.
    #[must_use]
    pub fn references(&self) -> &[FootnoteReference] {
        &self.references
    }
}

// ---------------------------------------------------------------------------
// Named discovery
// ---------------------------------------------------------------------------

impl Document {
    /// Every outline heading. Inline tasks are not included.
    pub fn headings(&self) -> impl Iterator<Item = Heading> + '_ {
        self.map()
    }

    /// Every [`Task`]: outlines and inline tasks.
    pub fn tasks(&self) -> impl Iterator<Item = Task> + '_ {
        self.map()
    }

    /// Every object in running text.
    ///
    /// Timestamps on clock and planning lines are left out; `map::<Object>()`
    /// includes them.
    pub fn objects(&self) -> impl Iterator<Item = Object> + '_ {
        self.map().filter(is_object_position)
    }

    /// Every link.
    pub fn links(&self) -> impl Iterator<Item = Link> + '_ {
        self.map()
    }

    /// Every timestamp in running text.
    ///
    /// Timestamps on clock and planning lines are left out;
    /// `map::<Timestamp>()` includes them.
    pub fn timestamps(&self) -> impl Iterator<Item = Timestamp> + '_ {
        self.map::<Timestamp>().filter(timestamp_is_object_position)
    }

    /// Every clock line.
    pub fn clocks(&self) -> impl Iterator<Item = Clock> + '_ {
        self.map()
    }

    /// Every table.
    pub fn tables(&self) -> impl Iterator<Item = Table> + '_ {
        self.map()
    }

    /// Every plain list, including nested lists.
    ///
    /// For outermost lists only, use
    /// `nav::descendants(root, &[OrgSyntaxKind::PLAIN_LIST])`.
    pub fn lists(&self) -> impl Iterator<Item = PlainList> + '_ {
        self.map()
    }

    /// Every keyword line, including affiliated keywords and `#+TBLFM:`
    /// lines, which [`Self::elements`] leaves out.
    pub fn keywords(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.map()
    }

    /// Every drawer except property drawers.
    pub fn drawers(&self) -> impl Iterator<Item = Drawer> + '_ {
        self.map()
    }

    /// Every `<<target>>`.
    pub fn targets(&self) -> impl Iterator<Item = Target> + '_ {
        self.map()
    }

    /// Every `<<<radio target>>>`. Their occurrences in text are not found
    /// here.
    pub fn radio_targets(&self) -> impl Iterator<Item = RadioTarget> + '_ {
        self.map()
    }

    /// Every radio link: each run of text that matches a radio target's key,
    /// in source order.
    ///
    /// As Org matches them: case is ignored, a run of whitespace in the key
    /// matches any run of whitespace (line breaks included), a key that
    /// starts or ends with a letter or digit does not match inside a word,
    /// the longest key wins where two could match, and matches never
    /// overlap. Only plain text where the scope admits a link is searched,
    /// so a target's own text, a link's description, verbatim text and
    /// keyword values are not. The radio links are a query: the tree never
    /// holds them as `LINK` nodes.
    #[must_use]
    pub fn radio_links(&self) -> Vec<RadioLink> {
        let mut keys: Vec<(Vec<char>, RadioTarget)> = self
            .radio_targets()
            .map(|t| (t.key().chars().collect::<Vec<_>>(), t))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        keys.sort_by_key(|k| std::cmp::Reverse(k.0.len()));
        let mut out = Vec::new();
        for node in self.root().descendants() {
            let kind = node.kind();
            if matches!(
                kind,
                OrgSyntaxKind::KEYWORD_VALUE | OrgSyntaxKind::KEYWORD_DUAL
            ) || !restrictions(kind).contains(OrgSyntaxKind::LINK)
            {
                continue;
            }
            for run in text_runs(&node) {
                scan_radio(self.source(), run, &keys, &mut out);
            }
        }
        out.sort_by_key(|l| l.range.start());
        out
    }

    /// Every footnote definition.
    pub fn footnote_definitions(&self) -> impl Iterator<Item = FootnoteDefinition> + '_ {
        self.map()
    }

    /// Every footnote reference: named, inline and anonymous. To match a
    /// label, use [`Self::footnote`].
    pub fn footnote_references(&self) -> impl Iterator<Item = FootnoteReference> + '_ {
        self.map()
    }

    /// Links that overlap `range` by at least one byte. An empty `range`
    /// yields nothing.
    pub fn links_in(&self, range: TextRange) -> impl Iterator<Item = Link> + '_ {
        region_hits(range, self.links())
    }

    /// Timestamps in running text that overlap `range` by at least one byte.
    /// An empty `range` yields nothing.
    pub fn timestamps_in(&self, range: TextRange) -> impl Iterator<Item = Timestamp> + '_ {
        region_hits(range, self.timestamps())
    }

    /// The first outline whose own `ID` property matches `id`, ignoring ASCII
    /// case. Inline tasks are not searched.
    #[must_use]
    pub fn by_id(&self, id: &str) -> Option<Outline> {
        self.outlines()
            .find(|o| property_value_eq(self, o, "ID", id))
    }

    /// The first outline whose own `CUSTOM_ID` property matches `id`,
    /// ignoring ASCII case.
    #[must_use]
    pub fn by_custom_id(&self, id: &str) -> Option<Outline> {
        self.outlines()
            .find(|o| property_value_eq(self, o, "CUSTOM_ID", id))
    }

    /// The first body element named `name` by `#+NAME:` or a synonym such as
    /// `#+TBLNAME:`. The comparison is exact, case included.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<Element> {
        self.elements().find(|el| {
            el.affiliated().is_some_and(|run| {
                run.names()
                    .any(|kw| source_slice(self.source(), kw.value().content_range()) == name)
            })
        })
    }

    /// The definition and references whose label equals `label` exactly.
    #[must_use]
    pub fn footnote(&self, label: &str) -> Footnote {
        let definition = self.footnote_definitions().find(|d| {
            d.part_content_range(FootnotePart::Label)
                .is_some_and(|r| source_slice(self.source(), r) == label)
        });
        let references: Vec<_> = self
            .footnote_references()
            .filter(|r| {
                r.label()
                    .is_some_and(|span| source_slice(self.source(), span) == label)
            })
            .collect();
        Footnote {
            definition,
            references,
        }
    }

    /// How an internal link could point at `offset`, preferring the most
    /// specific address.
    ///
    /// The order is an enclosing `<<target>>`, then the `#+NAME:` of the
    /// enclosing element, then the enclosing outline's `CUSTOM_ID`, its `ID`,
    /// and its title when not empty. Inside an inline task the result is
    /// `None` unless a target or name applies.
    #[must_use]
    pub fn address_at(&self, offset: TextSize) -> Option<InternalAddress> {
        if let Some(target) = self.node_at::<Target>(offset) {
            return Some(InternalAddress::Target {
                name: target.name(),
                range: target.range(),
            });
        }
        if let Some(el) = self.element_at(offset)
            && let Some(kw) = el.affiliated().and_then(|run| run.names().next())
        {
            let name = source_slice(self.source(), kw.value().content_range()).to_string();
            return Some(InternalAddress::Named { name, element: el });
        }
        let task = self.node_at::<Task>(offset)?;
        let outline = task.as_outline()?;
        if let Some(p) = self.property(&outline, "CUSTOM_ID") {
            return Some(InternalAddress::CustomId {
                id: p.value().to_string(),
                outline,
            });
        }
        if let Some(p) = self.property(&outline, "ID") {
            return Some(InternalAddress::Id {
                id: p.value().to_string(),
                outline,
            });
        }
        let title = outline
            .heading()
            .part_content_range(HeadingPart::Title)
            .map_or_default(|r| source_slice(self.source(), r).to_string());
        if title.is_empty() {
            return None;
        }
        Some(InternalAddress::FuzzyTitle { title, outline })
    }
}

fn is_object_position(obj: &Object) -> bool {
    match obj {
        Object::Timestamp(ts) => timestamp_is_object_position(ts),
        _ => true,
    }
}

fn timestamp_is_object_position(ts: &Timestamp) -> bool {
    !ts.syntax()
        .parent()
        .is_some_and(|p| matches!(p.kind(), OrgSyntaxKind::CLOCK | OrgSyntaxKind::PLANNING))
}

fn property_value_eq(doc: &Document, outline: &Outline, key: &str, id: &str) -> bool {
    doc.property(outline, key)
        .is_some_and(|p| p.value().eq_ignore_ascii_case(id))
}

/// Runs of a node's own tokens between its child nodes.
fn text_runs(node: &SyntaxNode) -> Vec<TextRange> {
    let mut runs = Vec::new();
    let mut run: Option<TextRange> = None;
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Token(t) => {
                let r = t.text_range();
                run = Some(run.map_or(r, |acc| acc.cover(r)));
            }
            rowan::NodeOrToken::Node(_) => {
                if let Some(r) = run.take() {
                    runs.push(r);
                }
            }
        }
    }
    if let Some(r) = run {
        runs.push(r);
    }
    runs
}

fn scan_radio(
    src: &str,
    run: TextRange,
    keys: &[(Vec<char>, RadioTarget)],
    out: &mut Vec<RadioLink>,
) {
    let start = usize::from(run.start());
    let end = usize::from(run.end());
    let mut i = start;
    while i < end {
        let before = src[..i].chars().next_back();
        let mut matched = None;
        for (key, target) in keys {
            if key[0].is_alphanumeric() && before.is_some_and(char::is_alphanumeric) {
                continue;
            }
            let Some(stop) = match_key(src, i, end, key) else {
                continue;
            };
            let after = src[stop..].chars().next();
            if key[key.len() - 1].is_alphanumeric() && after.is_some_and(char::is_alphanumeric) {
                continue;
            }
            matched = Some((stop, target));
            break;
        }
        if let Some((stop, target)) = matched {
            out.push(RadioLink {
                range: TextRange::new(
                    TextSize::try_from(i).expect("source fits TextSize"),
                    TextSize::try_from(stop).expect("source fits TextSize"),
                ),
                target: target.clone(),
            });
            i = stop;
        } else {
            i += src[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

/// Where a match of `key` starting at `at` ends, within `end`.
fn match_key(src: &str, at: usize, end: usize, key: &[char]) -> Option<usize> {
    let text = &src[at..end];
    let mut chars = text.char_indices().peekable();
    let mut k = 0;
    while k < key.len() {
        if key[k].is_whitespace() {
            while k < key.len() && key[k].is_whitespace() {
                k += 1;
            }
            let mut any = false;
            while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
                chars.next();
                any = true;
            }
            if !any {
                return None;
            }
            continue;
        }
        let (_, c) = chars.next()?;
        if !c.to_lowercase().eq(key[k].to_lowercase()) {
            return None;
        }
        k += 1;
    }
    Some(at + chars.peek().map_or(text.len(), |&(j, _)| j))
}

#[cfg(test)]
mod tests {
    use super::super::{at, span};
    use super::*;
    use crate::ast::nav;
    use crate::ast::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::*;

    fn radio_texts(src: &str) -> Vec<&str> {
        let doc = parse(src);
        let links = doc.radio_links();
        links
            .iter()
            .map(|l| &src[usize::from(l.range().start())..usize::from(l.range().end())])
            .collect()
    }

    #[test]
    fn radio_links_match_as_org_does() {
        let src = "<<<island>>> here.\n\nThe Island, an island and islands; mainland.\n";
        assert_eq!(radio_texts(src), vec!["Island", "island"]);
    }

    #[test]
    fn radio_links_whitespace_and_longest() {
        let src = "<<<oak floor>>> <<<oak>>>\n\nAn oak\nfloor, an oak  floor, oak.\n";
        assert_eq!(radio_texts(src), vec!["oak\nfloor", "oak  floor", "oak"]);
    }

    #[test]
    fn radio_links_skip_descriptions_verbatim_and_keywords() {
        let src = "#+TITLE: island\n<<<island>>>\n\n[[x][island]] =island= *island*\n";
        assert_eq!(radio_texts(src), vec!["island"]);
        let doc = parse(src);
        let links = doc.radio_links();
        assert_eq!(links[0].target().key(), "island");
        // The match is the one inside the bold.
        let bold = doc
            .root()
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::BOLD)
            .unwrap();
        assert!(bold.text_range().contains_range(links[0].range()));
    }

    #[test]
    fn radio_links_in_titles_and_cells() {
        let src = "* About the island\n<<<island>>>\n| island |\n";
        assert_eq!(radio_texts(src), vec!["island", "island"]);
    }

    #[test]
    fn radio_links_none_without_targets() {
        assert_eq!(parse("island\n").radio_links().len(), 0);
    }

    #[test]
    fn file_property_drawer_and_its_insert_point() {
        let doc = parse("# c\n:PROPERTIES:\n:ID: x\n:END:\n#+TITLE: T\n");
        assert!(doc.file_property_drawer().is_some());
        let doc = parse("#+TITLE: T\n:PROPERTIES:\n:ID: x\n:END:\n");
        assert!(doc.file_property_drawer().is_none());
        assert_eq!(
            doc.file_property_drawer_insert_point().offset(),
            TextSize::from(0)
        );
    }

    #[test]
    fn headings_vs_tasks_exactness() {
        let src = "* H\n*************** T\n*************** END\n";
        let doc = parse(src);
        let headings: Vec<_> = doc.headings().collect();
        let tasks: Vec<_> = doc.tasks().collect();
        assert_eq!(headings.len(), 1);
        assert_eq!(tasks.len(), 2);
        let heading_node = nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .unwrap();
        assert!(Task::cast(heading_node).is_none());
    }

    #[test]
    fn keywords_include_tblfm_and_affiliated_elements_omit() {
        let src = "#+CAPTION: c\npara\n| a |\n#+TBLFM: $1=1\n";
        let doc = parse(src);
        let kws: Vec<_> = doc.keywords().map(|k| k.key()).collect();
        assert!(kws.iter().any(|k| k.eq_ignore_ascii_case("CAPTION")));
        assert!(kws.iter().any(|k| k.eq_ignore_ascii_case("TBLFM")));
        assert_eq!(
            doc.elements()
                .filter(|e| matches!(e, Element::Keyword(_)))
                .count(),
            0
        );
    }

    #[test]
    fn keyword_shaped_text_in_opaque_and_verse_is_not_a_keyword() {
        let src = concat!(
            "#+begin_src elisp\n#+TITLE: no\n#+end_src\n",
            "#+begin_verse\n#+TITLE: no\n#+end_verse\n",
            "#+begin_example\n#+TITLE: no\n#+end_example\n",
            "#+begin_export html\n#+TITLE: no\n#+end_export\n",
            "#+begin_comment\n#+TITLE: no\n#+end_comment\n",
        );
        let doc = parse(src);
        assert_eq!(doc.keywords().count(), 0);
        assert_eq!(doc.map::<Keyword>().count(), 0);
    }

    #[test]
    fn objects_vs_timestamps_vs_clocks_vs_map() {
        let src = "* H\nSCHEDULED: <2020-01-01>\nCLOCK: [2020-01-02]\n<2020-01-03>\n";
        let doc = parse(src);
        let sched = span(src, "<2020-01-01>");
        let clock_line = nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK)
            .into_iter()
            .next()
            .unwrap();
        let clock_ts = nodes_of_kind(&clock_line, OrgSyntaxKind::TIMESTAMP)
            .into_iter()
            .next()
            .unwrap()
            .text_range();
        let body = span(src, "<2020-01-03>");

        let pos: Vec<_> = doc.timestamps().map(|t| t.range()).collect();
        assert_eq!(pos, [body]);
        let obj_ts: Vec<_> = doc
            .objects()
            .filter_map(|o| match o {
                Object::Timestamp(t) => Some(t.range()),
                _ => None,
            })
            .collect();
        assert_eq!(obj_ts, [body]);
        assert_eq!(doc.clocks().next().unwrap().syntax(), &clock_line);
        let mapped: Vec<_> = doc.map::<Timestamp>().map(|t| t.range()).collect();
        assert!(mapped.contains(&sched));
        assert!(mapped.contains(&clock_ts));
        assert!(mapped.contains(&body));

        let at_clock = doc.object_at(clock_ts.start() + TextSize::from(1)).unwrap();
        assert!(matches!(at_clock, Object::Timestamp(_)));
        assert_eq!(at_clock.range(), clock_ts);
        assert!(!obj_ts.contains(&clock_ts));
        let at_sched = doc.object_at(sched.start() + TextSize::from(1)).unwrap();
        assert!(matches!(at_sched, Object::Timestamp(_)));
        assert_eq!(at_sched.range(), sched);
        assert!(!obj_ts.contains(&sched));
    }

    #[test]
    fn lists_include_nested_links_in_empty_is_nothing() {
        let src = "- a\n  - nested\n";
        let doc = parse(src);
        assert_eq!(doc.lists().count(), 2);
        assert_eq!(
            nav::descendants::<crate::ast::PlainList>(doc.root(), &[OrgSyntaxKind::PLAIN_LIST])
                .count(),
            1
        );
        assert_eq!(doc.links_in(TextRange::empty(TextSize::from(0))).count(), 0);
        assert_eq!(
            doc.timestamps_in(TextRange::empty(TextSize::from(0)))
                .count(),
            0
        );
    }

    #[test]
    fn identity_by_id_named_footnote_address() {
        let src = concat!(
            "#+NAME: n1\npara\n",
            "* H\n:PROPERTIES:\n:ID: abc\n:CUSTOM_ID: cust\n:END:\n",
            "<<tgt>>\n",
            "#+TBLNAME: tbl\n| a |\n",
            "*************** T\n:PROPERTIES:\n:ID: inline\n:END:\n*************** END\n",
            "[fn:lab] def\nSee [fn:lab].\n",
            "* \n",
        );
        let doc = parse(src);
        let by = doc.by_id("ABC").expect("id");
        assert_eq!(by.heading().title().syntax().text().to_string().trim(), "H");
        assert!(doc.by_id("inline").is_none());
        assert!(doc.by_custom_id("cust").is_some());
        assert!(matches!(doc.named("n1"), Some(Element::Paragraph(_))));
        assert!(matches!(doc.named("tbl"), Some(Element::Table(_))));
        assert!(doc.named("N1").is_none());

        let fn_ = doc.footnote("lab");
        assert!(fn_.definition().is_some());
        assert_eq!(fn_.references().len(), 1);
        assert!(doc.footnote("missing").definition().is_none());
        assert!(doc.footnote("LAB").definition().is_none());

        let tgt = at(src, "tgt");
        assert!(matches!(
            doc.address_at(tgt),
            Some(InternalAddress::Target { .. })
        ));
        let named = at(src, "para");
        assert!(matches!(
            doc.address_at(named),
            Some(InternalAddress::Named { .. })
        ));
        let h = doc.by_id("abc").unwrap();
        let in_title = h.heading().range().start() + TextSize::from(2);
        assert!(matches!(
            doc.address_at(in_title),
            Some(InternalAddress::CustomId { .. })
        ));
        let empty = parse("* \n");
        let o = empty.root_outlines().next().unwrap();
        let off = o.heading().range().start() + TextSize::from(2);
        assert!(empty.address_at(off).is_none());
        let zero = parse("just text\n");
        assert!(zero.address_at(TextSize::from(0)).is_none());
    }

    #[test]
    fn identity_duplicate_id_fold_and_inline_task() {
        let src = "* A\n:PROPERTIES:\n:ID: same\n:END:\n* B\n:PROPERTIES:\n:ID: same\n:END:\n";
        let doc = parse(src);
        let a = doc.root_outlines().next().unwrap();
        assert_eq!(
            doc.by_id("same").as_ref().map(Outline::syntax),
            Some(a.syntax())
        );

        let id_only = parse("* OnlyId\n:PROPERTIES:\n:ID: xyz\n:END:\n");
        let h = id_only.root_outlines().next().unwrap();
        let off = h.heading().range().start() + TextSize::from(2);
        match id_only.address_at(off) {
            Some(InternalAddress::Id { id, outline }) => {
                assert_eq!(id, "xyz");
                assert_eq!(outline.syntax(), h.syntax());
            }
            other => panic!("{other:?}"),
        }

        let fuzzy = parse("* Fuzzy Title\n");
        let h = fuzzy.root_outlines().next().unwrap();
        let off = h.heading().range().start() + TextSize::from(2);
        match fuzzy.address_at(off) {
            Some(InternalAddress::FuzzyTitle { title, outline }) => {
                assert!(title.contains("Fuzzy"));
                assert_eq!(outline.syntax(), h.syntax());
            }
            other => panic!("{other:?}"),
        }

        let src = "* H\n:PROPERTIES:\n:CUSTOM_ID: cust\n:END:\n*************** T\n:PROPERTIES:\n:ID: inline\n:END:\n*************** END\n";
        let doc = parse(src);
        let task = nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
            .into_iter()
            .next()
            .unwrap();
        let interior = task.text_range().start() + TextSize::from(16);
        assert!(
            doc.address_at(interior).is_none(),
            "{:?}",
            doc.address_at(interior)
        );
    }

    #[test]
    fn links_in_timestamps_in_and_targets() {
        let src = "[[a]] then [[b]]\n";
        let doc = parse(src);
        let links: Vec<Link> = doc.links().collect();
        assert_eq!(links.len(), 2);
        let first = links[0].range();
        let both = TextRange::new(links[0].range().start(), links[1].range().end());
        assert_eq!(doc.links_in(first).count(), 1);
        assert_eq!(doc.links_in(TextRange::empty(first.start())).count(), 0);
        assert_eq!(doc.links_in(both).count(), 2);

        let src = "<2020-01-01> then <2020-01-02>\n";
        let doc = parse(src);
        let stamps: Vec<_> = doc.timestamps().collect();
        assert_eq!(stamps.len(), 2);
        assert_eq!(doc.timestamps_in(stamps[0].range()).count(), 1);
        assert_eq!(
            doc.timestamps_in(TextRange::empty(stamps[0].range().start()))
                .count(),
            0
        );
        let both = TextRange::new(stamps[0].range().start(), stamps[1].range().end());
        assert_eq!(doc.timestamps_in(both).count(), 2);

        let src = "<<t>> <<<r>>>\n";
        let doc = parse(src);
        let named: Vec<Target> = doc.targets().collect();
        let radios: Vec<RadioTarget> = doc.radio_targets().collect();
        assert_eq!(named.len(), 1);
        assert_eq!(radios.len(), 1);
    }
}
