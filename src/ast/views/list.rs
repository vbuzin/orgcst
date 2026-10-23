//! Plain lists and their items.
//!
//! Tree shape:
//!
//! ```text
//!   PLAIN_LIST
//!     ITEM+
//!     [post-blank]
//!   ITEM
//!     [INDENT] BULLET [WHITESPACE [COUNTER] [WHITESPACE] [CHECKBOX]
//!     [WHITESPACE ITEM_TAG WHITESPACE COLON COLON]]
//!     [NEWLINE]
//!     ... element children (nested PLAIN_LIST included)
//! ```
//!
//! Bullets, counters and checkboxes are read as plain values ([`Bullet`],
//! [`Counter`], [`CheckboxState`]), not as views.

use crate::TextRange;
use crate::TextSize;
use crate::ast::layout::{
    RemovalSide, checkbox_interior_token, content_of, counter_value_token, first_line_end,
    first_token_of_kind, item_tag_removal_range, present_with_removal_after, removal_extending,
};
use crate::ast::support::{
    child_of_kind, child_view, child_views, dest, element_run_range, field_dest, next_typed,
    prev_typed,
};
use crate::ast::{
    AstNode, Element, ElementView, HasElements, HasObjects, HasParts, InsertPoint, Padding,
    PartSpan, PresentPart, SplitPoint, split_span,
};
use crate::syntax::{OrgSyntaxKind, SyntaxNode, SyntaxToken};

crate::ast_node!(
    /// A plain list: a run of items at one indentation.
    PlainList,
    OrgSyntaxKind::PLAIN_LIST
);
crate::ast_node!(
    /// A list item: its bullet, optional counter, checkbox and tag, and its
    /// contents.
    Item,
    OrgSyntaxKind::ITEM
);
crate::ast_node!(
    /// The tag of a descriptive list item, before ` :: `; it holds objects.
    ItemTag,
    OrgSyntaxKind::ITEM_TAG
);

impl HasObjects for ItemTag {}

/// Parsed form of a `COUNTER` cookie: digits that fit in `u32`, or one ASCII letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CounterForm {
    /// `[@20]` or `[@start:20]`.
    Number(u32),
    /// `[@b]`, when alphabetical lists are enabled.
    Letter(char),
}

/// A counter cookie such as `[@20]`, as written, with its value when it parses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Counter {
    range: TextRange,
    written: String,
    form: Option<CounterForm>,
}

impl Counter {
    /// Span of the cookie.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// The cookie as written: `[@20]`, `[@start:20]`, `[@b]`.
    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    /// The cookie's value.
    ///
    /// `None` when the number overflows `u32` or the value does not parse;
    /// [`Self::written`] still has the text.
    #[must_use]
    pub fn form(&self) -> Option<CounterForm> {
        self.form
    }
}

/// An item's bullet: `-`, `+`, `*`, `1.`, `a)`, `A)` and so on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bullet {
    range: TextRange,
    text: String,
}

impl Bullet {
    /// Span of the bullet.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// The bullet as written.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    fn is_ordered(&self) -> bool {
        let t = self.text.as_str();
        t.ends_with('.') || t.ends_with(')')
    }
}

/// State of an item's checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckboxState {
    /// `[ ]`.
    Unchecked,
    /// `[X]`.
    Checked,
    /// `[-]`.
    Partial,
}

/// Kind of a plain list, decided by its first item as in org-element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKind {
    /// Bullets such as `-` or `+`.
    Unordered,
    /// Bullets such as `1.` or `a)`.
    Ordered,
    /// The first item has a tag: `- term :: definition`.
    Descriptive,
}

/// Parts of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemPart {
    /// The bullet.
    Bullet,
    /// The counter cookie, `[@N]`.
    Counter,
    /// The checkbox; its content is the state character.
    Checkbox,
    /// The tag of a descriptive item; its removal includes the `::`.
    Tag,
    /// The body elements.
    Body,
}

impl HasElements for Item {}

impl PlainList {
    /// Kind from the first item: descriptive if it has a tag, else ordered
    /// if its bullet is the ordered form, else unordered.
    #[must_use]
    pub fn kind(&self) -> ListKind {
        let Some(first) = self.items().next() else {
            return ListKind::Unordered;
        };
        if first.tag().is_some() {
            return ListKind::Descriptive;
        }
        if first.bullet().is_ordered() {
            ListKind::Ordered
        } else {
            ListKind::Unordered
        }
    }

    /// Direct item children, source order.
    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        child_views(self.syntax())
    }

    /// This list's items and the items of its nested sublists, in pre-order.
    pub fn descendant_items(&self) -> impl Iterator<Item = Item> + '_ {
        let mut out = Vec::new();
        collect_descendants(self, &mut out);
        out.into_iter()
    }

    /// Where a new first item is written: the start of the list.
    #[must_use]
    pub fn first_item_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.range().start())
    }

    /// Where a new last item is written: after the last item, before the
    /// blank lines that end the list.
    #[must_use]
    pub fn append_item_insert_point(&self) -> InsertPoint {
        match self.items().last() {
            Some(last) => dest(self.syntax(), last.range().end()),
            None => dest(self.syntax(), self.range().start()),
        }
    }

    fn items_span(&self) -> TextRange {
        let mut items = self.items();
        let Some(first) = items.next() else {
            return TextRange::empty(self.range().start());
        };
        let end = items
            .last()
            .map_or_else(|| first.range().end(), |last| last.range().end());
        TextRange::new(first.range().start(), end)
    }
}

fn collect_descendants(list: &PlainList, out: &mut Vec<Item>) {
    for item in list.items() {
        out.push(item.clone());
        for sub in item.sublists() {
            collect_descendants(&sub, out);
        }
    }
}

impl ElementView for PlainList {
    fn content_range(&self) -> Option<TextRange> {
        Some(self.items_span())
    }

    fn is_greater(&self) -> bool {
        true
    }
}

impl Item {
    /// The list this item belongs to.
    ///
    /// # Panics
    ///
    /// Panics if the item's parent is not `PLAIN_LIST`, which the parser never
    /// produces.
    #[must_use]
    pub fn list(&self) -> PlainList {
        self.syntax()
            .parent()
            .and_then(PlainList::cast)
            .expect("grammar: ITEM is always a child of PLAIN_LIST (forms/list.rs::parse)")
    }

    /// The item whose sublist holds this item, if any.
    #[must_use]
    pub fn parent_item(&self) -> Option<Item> {
        self.list().syntax().parent().and_then(Item::cast)
    }

    /// Previous sibling item under the same list.
    #[must_use]
    pub fn prev_sibling(&self) -> Option<Item> {
        prev_typed(self.syntax())
    }

    /// Next sibling item under the same list.
    #[must_use]
    pub fn next_sibling(&self) -> Option<Item> {
        next_typed(self.syntax())
    }

    /// Nested lists that are direct element children of this item.
    pub fn sublists(&self) -> impl Iterator<Item = PlainList> + '_ {
        self.elements().filter_map(|e| match e {
            Element::PlainList(list) => Some(list),
            _ => None,
        })
    }

    /// The bullet.
    ///
    /// # Panics
    ///
    /// Panics if the item has no `BULLET`, which the parser never produces.
    #[must_use]
    pub fn bullet(&self) -> Bullet {
        let node = self.bullet_node();
        Bullet {
            range: node.text_range(),
            text: node.text().to_string(),
        }
    }

    /// The counter cookie, if any.
    #[must_use]
    pub fn counter(&self) -> Option<Counter> {
        let node = child_of_kind(self.syntax(), OrgSyntaxKind::COUNTER)?;
        Some(counter_from_node(&node))
    }

    /// The checkbox state, if any.
    #[must_use]
    pub fn checkbox(&self) -> Option<CheckboxState> {
        let node = child_of_kind(self.syntax(), OrgSyntaxKind::CHECKBOX)?;
        checkbox_state(&node)
    }

    /// The tag of a descriptive item (`term` in `- term :: definition`), if
    /// any.
    #[must_use]
    pub fn tag(&self) -> Option<ItemTag> {
        child_view(self.syntax())
    }

    /// The indentation before the bullet, to copy when writing a sibling.
    /// `None` at column 0.
    #[must_use]
    pub fn indent(&self) -> Option<SyntaxToken> {
        let tok = first_token_of_kind(self.syntax(), OrgSyntaxKind::INDENT)?;
        (!tok.text().is_empty()).then_some(tok)
    }

    /// Where a new sibling item before this one is written.
    #[must_use]
    pub fn insert_before(&self) -> InsertPoint {
        dest(self.syntax(), self.range().start())
    }

    /// Where a new sibling item after this one is written: after its
    /// sublists.
    #[must_use]
    pub fn insert_after(&self) -> InsertPoint {
        dest(self.syntax(), self.range().end())
    }

    /// Items with a checkbox in this item's direct sublists, not in deeper
    /// ones.
    pub fn direct_child_checkbox_items(&self) -> impl Iterator<Item = Item> + '_ {
        let mut items = Vec::new();
        for e in self.elements() {
            if let Element::PlainList(list) = e {
                items.extend(list.items().filter(|i| i.checkbox().is_some()));
            }
        }
        items.into_iter()
    }

    /// Where a first nested item is written: after the last body element, or
    /// in the empty body.
    #[must_use]
    pub fn first_child_item_insert_point(&self) -> InsertPoint {
        dest(self.syntax(), self.content_range().end())
    }

    /// Cut the body or tag content at `offset`.
    ///
    /// `None` unless [`HasParts::part_at`] is `Body` or `Tag` and `offset`
    /// lies in that part's content span `[start, end)`. Offset at `start`
    /// yields an empty keep.
    #[must_use]
    pub fn split_at(&self, offset: TextSize) -> Option<SplitPoint> {
        let part = self.part_at(offset)?;
        if !matches!(part, ItemPart::Body | ItemPart::Tag) {
            return None;
        }
        split_span(self.part_content_range(part)?, offset, self.insert_after())
    }

    /// The body: from the first body element, its affiliated keywords
    /// included, through the last; zero-width after the first line when
    /// empty.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        element_run_range(self.syntax())
            .unwrap_or_else(|| TextRange::empty(first_line_end(self.syntax())))
    }

    fn bullet_node(&self) -> SyntaxNode {
        child_of_kind(self.syntax(), OrgSyntaxKind::BULLET)
            .expect("grammar: ITEM always has BULLET (forms/list.rs::parse_item)")
    }
}

impl HasParts for Item {
    type Part = ItemPart;
    const PARTS: &'static [ItemPart] = &[
        ItemPart::Bullet,
        ItemPart::Counter,
        ItemPart::Checkbox,
        ItemPart::Tag,
        ItemPart::Body,
    ];

    fn part_span(&self, part: ItemPart) -> PartSpan {
        match part {
            ItemPart::Bullet => {
                let r = self.bullet().range();
                PartSpan::Present(PresentPart::identical(r))
            }
            ItemPart::Counter => match child_of_kind(self.syntax(), OrgSyntaxKind::COUNTER) {
                Some(node) => present_with_removal_after(self.syntax(), node.text_range()),
                None => PartSpan::Absent(field_dest(
                    self.syntax(),
                    self.bullet_node().text_range().end(),
                    Padding::None,
                )),
            },
            ItemPart::Checkbox => {
                if let Some(node) = child_of_kind(self.syntax(), OrgSyntaxKind::CHECKBOX) {
                    let carved = node.text_range();
                    let content = checkbox_interior_token(&node).map_or(carved, |t| t.text_range());
                    let removal = removal_extending(self.syntax(), carved, RemovalSide::After);
                    PartSpan::Present(PresentPart::new(carved, content, removal))
                } else {
                    let anchor = child_of_kind(self.syntax(), OrgSyntaxKind::COUNTER).map_or_else(
                        || self.bullet_node().text_range().end(),
                        |n| n.text_range().end(),
                    );
                    PartSpan::Absent(field_dest(self.syntax(), anchor, Padding::None))
                }
            }
            ItemPart::Tag => {
                if let Some(tag) = self.tag() {
                    let carved = tag.range();
                    let content = content_of(tag.syntax());
                    let removal = item_tag_removal_range(self.syntax(), carved);
                    PartSpan::Present(PresentPart::new(carved, content, removal))
                } else {
                    let anchor = child_of_kind(self.syntax(), OrgSyntaxKind::CHECKBOX)
                        .or_else(|| child_of_kind(self.syntax(), OrgSyntaxKind::COUNTER))
                        .map_or_else(
                            || self.bullet_node().text_range().end(),
                            |n| n.text_range().end(),
                        );
                    PartSpan::Absent(field_dest(self.syntax(), anchor, Padding::None))
                }
            }
            ItemPart::Body => {
                let r = self.content_range();
                PartSpan::Present(PresentPart::new(r, r, r))
            }
        }
    }
}

fn counter_from_node(node: &SyntaxNode) -> Counter {
    let written = node.text().to_string();
    let form = counter_value_token(node).and_then(|tok| parse_counter_form(tok.text()));
    Counter {
        range: node.text_range(),
        written,
        form,
    }
}

pub(crate) fn parse_counter_form(value: &str) -> Option<CounterForm> {
    if value.len() == 1 {
        let c = value.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(CounterForm::Letter(c));
        }
    }
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        value.parse::<u32>().ok().map(CounterForm::Number)
    } else {
        None
    }
}

fn checkbox_state(node: &SyntaxNode) -> Option<CheckboxState> {
    match checkbox_interior_token(node)?.text() {
        " " => Some(CheckboxState::Unchecked),
        "X" => Some(CheckboxState::Checked),
        "-" => Some(CheckboxState::Partial),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParseConfig;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn first_list(src: &str) -> (crate::Document, PlainList) {
        let doc = parse(src);
        let list = PlainList::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PLAIN_LIST)
                .into_iter()
                .next()
                .expect("list"),
        )
        .expect("cast");
        (doc, list)
    }

    fn first_item(src: &str) -> (crate::Document, Item) {
        let doc = parse(src);
        let item = Item::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
                .into_iter()
                .next()
                .expect("item"),
        )
        .expect("cast");
        (doc, item)
    }

    #[test]
    fn item_extent_includes_nested_sublist() {
        let src = "- outer\n  - inner\n";
        let (doc, list) = first_list(src);
        let outer = list.items().next().unwrap();
        assert!(
            source_slice(&doc, outer.range()).contains("inner"),
            "nested sublist lives inside the parent item"
        );
        assert_eq!(outer.sublists().count(), 1);
        assert_eq!(list.descendant_items().count(), 2);
        assert!(outer.parent_item().is_none());
        let inner = list.descendant_items().nth(1).unwrap();
        assert_eq!(
            inner.parent_item().as_ref().map(AstNode::syntax),
            Some(outer.syntax())
        );
        let (_, list) = first_list("- a\n  - inner\n- b\n");
        let items: Vec<_> = list.items().collect();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].next_sibling().as_ref().map(AstNode::syntax),
            Some(items[1].syntax())
        );
        let inner = items[0].sublists().next().unwrap().items().next().unwrap();
        assert!(inner.prev_sibling().is_none());
        assert!(inner.next_sibling().is_none());
        assert_ne!(
            items[0].next_sibling().as_ref().map(AstNode::syntax),
            Some(inner.syntax())
        );
    }

    #[test]
    fn inter_item_blank_belongs_to_previous_item_list_final_to_list() {
        let src = "- a\n\n- b\n\n";
        let (doc, list) = first_list(src);
        let items: Vec<_> = list.items().collect();
        assert_eq!(items.len(), 2);
        assert!(
            source_slice(&doc, items[0].range()).ends_with("\n\n"),
            "inter-item blank on previous item"
        );
        let content = list.content_range().unwrap();
        assert_eq!(content.end(), items[1].range().end());
        assert!(
            list.range().end() > content.end(),
            "list-final blanks belong to the list"
        );
        assert!(list.is_greater());
    }

    #[test]
    fn counter_overflow_form_is_none_written_present() {
        let (_, item) = first_item("- [@4294967296] x\n");
        let c = item.counter().expect("counter node");
        assert_eq!(c.written(), "[@4294967296]");
        assert!(c.form().is_none());
        crate::ast::check_part_contract(&item);
    }

    #[test]
    fn counter_numeric_alphabetic_and_start_form() {
        let (_, item) = first_item("- [@20] x\n");
        assert_eq!(
            item.counter().unwrap().form(),
            Some(CounterForm::Number(20))
        );

        let (_, item) = first_item("- [@start:20] x\n");
        assert_eq!(
            item.counter().unwrap().form(),
            Some(CounterForm::Number(20))
        );
        assert_eq!(item.counter().unwrap().written(), "[@start:20]");
        crate::ast::check_part_contract(&item);

        let doc = parse_with(
            "- [@b] x\n",
            &ParseConfig::default().with_list_alphabetical(true),
        );
        let item = Item::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
                .into_iter()
                .next()
                .expect("item"),
        )
        .expect("cast");
        assert_eq!(
            item.counter().unwrap().form(),
            Some(CounterForm::Letter('b'))
        );
        crate::ast::check_part_contract(&item);
    }

    #[test]
    fn checkbox_states_and_indent_token() {
        let (doc, item) = first_item("- [ ] a\n");
        assert_eq!(item.checkbox(), Some(CheckboxState::Unchecked));
        assert!(item.indent().is_none());
        let content = item.part_content_range(ItemPart::Checkbox).unwrap();
        assert_eq!(source_slice(&doc, content), " ");

        let (_, item) = first_item("- [X] a\n");
        assert_eq!(item.checkbox(), Some(CheckboxState::Checked));

        let (_, item) = first_item("- [-] a\n");
        assert_eq!(item.checkbox(), Some(CheckboxState::Partial));

        let src = "  - nested\n";
        let (doc, item) = first_item(src);
        let indent = item.indent().expect("indent");
        assert_eq!(indent.text(), "  ");
        assert_eq!(source_slice(&doc, indent.text_range()), "  ");
    }

    #[test]
    fn whitespace_perturbed_item_parts() {
        use crate::ast::Object;

        let (doc, item) = first_item("-    [X] extra spaces\n");
        assert_eq!(item.checkbox(), Some(CheckboxState::Checked));
        assert_eq!(
            source_slice(&doc, item.part_content_range(ItemPart::Checkbox).unwrap()),
            "X"
        );
        crate::ast::check_part_contract(&item);

        let src = "- term  ::  def\n";
        let (doc, item) = first_item(src);
        assert_eq!(item.list().kind(), ListKind::Descriptive);
        let tag = item.tag().expect("tag");
        assert_eq!(source_slice(&doc, tag.range()).trim(), "term");
        let removal = source_slice(&doc, item.part_removal_range(ItemPart::Tag).unwrap());
        assert!(removal.contains("::"), "tag removal covers the separator");
        assert!(
            !removal.contains("def"),
            "tag removal must not swallow the definition"
        );
        crate::ast::check_part_contract(&item);

        let src = "- *b* :: def\n";
        let (_, item) = first_item(src);
        let tag = item.tag().expect("tag");
        let objs: Vec<_> = tag.objects().collect();
        assert_eq!(objs.len(), 1);
        assert!(matches!(objs[0], Object::Markup(_)));

        let src = "\t- tabbed\n";
        let (doc, item) = first_item(src);
        let indent = item.indent().expect("tab indent");
        assert_eq!(indent.text(), "\t");
        assert_eq!(source_slice(&doc, indent.text_range()), "\t");
        crate::ast::check_part_contract(&item);
    }

    #[test]
    fn list_kind_from_first_item() {
        let (_, list) = first_list("- a\n- b\n");
        assert_eq!(list.kind(), ListKind::Unordered);

        let (_, list) = first_list("1. a\n2. b\n");
        assert_eq!(list.kind(), ListKind::Ordered);

        let (_, list) = first_list("- tag :: def\n");
        assert_eq!(list.kind(), ListKind::Descriptive);
    }

    #[test]
    fn item_split_at_body_and_outside() {
        let (doc, item) = first_item("- hello world\n");
        let body = item.part_content_range(ItemPart::Body).unwrap();
        let offset = body.start() + TextSize::from(6);
        let sp = item.split_at(offset).expect("body");
        assert_eq!(source_slice(&doc, sp.keep()), "hello ");
        assert_eq!(source_slice(&doc, sp.moved()), "world\n");
        assert_eq!(sp.dest().offset(), item.range().end());

        assert!(item.split_at(item.bullet().range().start()).is_none());
        assert!(item.split_at(body.end()).is_none());

        let sp = item.split_at(body.start()).expect("body start");
        assert!(sp.keep().is_empty());
        assert_eq!(source_slice(&doc, sp.moved()), "hello world\n");
    }

    #[test]
    fn item_split_at_tag_content() {
        let src = "- term :: def\n";
        let (doc, item) = first_item(src);
        let tag = item.part_content_range(ItemPart::Tag).unwrap();
        let offset = tag.start() + TextSize::from(2); // inside "term"
        let sp = item.split_at(offset).expect("tag");
        assert_eq!(source_slice(&doc, sp.keep()), "te");
        assert_eq!(source_slice(&doc, sp.moved()), "rm");
        assert_eq!(sp.dest().offset(), item.insert_after().offset());

        let sp = item.split_at(tag.start()).expect("tag start");
        assert!(sp.keep().is_empty());

        let text = doc.source();
        let colons = text.find("::").expect("separator");
        assert!(item.split_at(TextSize::from(colons as u32)).is_none());
        assert!(item.split_at(tag.end()).is_none());
    }

    #[test]
    fn empty_item_body_is_zero_width() {
        let (_, item) = first_item("-\n");
        let body = item.content_range();
        assert!(body.is_empty());
        assert_eq!(body.start(), first_line_end(item.syntax()));
        crate::ast::check_part_contract(&item);
        assert!(item.elements().next().is_none());
        assert_eq!(
            item.first_child_item_insert_point().offset(),
            body.start(),
            "empty item first-child dest is the zero-width body slot"
        );
        assert!(item.split_at(body.start()).is_none());
    }

    #[test]
    #[allow(clippy::similar_names)] // `list` is the plain list; `last` is its last item
    fn item_dests_append_after_sublist_and_before_list_final_blanks() {
        let src = "- outer\n  - inner\n\n";
        let (doc, list) = first_list(src);
        let outer = list.items().next().unwrap();
        assert!(
            source_slice(&doc, outer.range()).contains("inner"),
            "insert_after sits after the nested sublist"
        );
        assert_eq!(outer.insert_after().offset(), outer.range().end());
        let last = list.items().last().unwrap();
        assert_eq!(list.append_item_insert_point().offset(), last.range().end());
        assert!(
            last.range().end() < list.range().end(),
            "append is before list-final blanks"
        );
        assert_eq!(
            list.first_item_insert_point().offset(),
            list.range().start()
        );
    }

    #[test]
    fn list_affiliated_hops_attr_org() {
        let src = "#+ATTR_ORG: :radio\n- [ ] a\n- [ ] b\n";
        let (doc, list) = first_list(src);
        let org = list
            .affiliated()
            .and_then(|r| r.attr("org"))
            .expect("ATTR_ORG");
        assert!(org.key().eq_ignore_ascii_case("ATTR_ORG"));
        assert_eq!(org.value().syntax().text().to_string(), ":radio");
        assert!(Element::cast(org.syntax().clone()).is_none());
        let boxes: Vec<_> = list
            .items()
            .filter_map(|it| {
                it.checkbox()
                    .and_then(|_| it.part_content_range(ItemPart::Checkbox))
            })
            .collect();
        assert_eq!(boxes.len(), 2);
        assert_eq!(source_slice(&doc, boxes[0]), " ");
    }

    #[test]
    fn descendant_items_are_list_repair_inputs() {
        let src = "- a\n  - [@2] inner\n";
        let (doc, list) = first_list(src);
        let rows: Vec<_> = list
            .descendant_items()
            .map(|it| {
                (
                    it.bullet().text().to_string(),
                    it.counter().map(|c| c.written().to_string()),
                    it.indent().map(|t| t.text().to_string()),
                )
            })
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "-");
        assert!(rows[0].1.is_none());
        assert!(rows[0].2.is_none());
        assert_eq!(rows[1].1.as_deref(), Some("[@2]"));
        assert_eq!(rows[1].2.as_deref(), Some("  "));
        assert!(source_slice(&doc, list.range()).contains("inner"));
    }

    #[test]
    fn parse_counter_form_table() {
        assert_eq!(parse_counter_form("20"), Some(CounterForm::Number(20)));
        assert_eq!(
            parse_counter_form("4294967295"),
            Some(CounterForm::Number(u32::MAX))
        );
        assert_eq!(parse_counter_form("4294967296"), None);
        assert_eq!(parse_counter_form(""), None);
        assert_eq!(parse_counter_form("ab"), None);
        assert_eq!(parse_counter_form("1a"), None);
        assert_eq!(parse_counter_form("b"), Some(CounterForm::Letter('b')));
        assert_eq!(parse_counter_form("A"), Some(CounterForm::Letter('A')));
    }

    #[test]
    fn has_parts_over_taxonomy_and_siblings() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM) {
                let item = Item::cast(node).unwrap();
                crate::ast::check_part_contract(&item);
            }
        }

        let src = "- a\n- b\n- c\n";
        let (_, list) = first_list(src);
        let items: Vec<_> = list.items().collect();
        assert!(items[0].prev_sibling().is_none());
        assert_eq!(items[1].prev_sibling().unwrap().syntax(), items[0].syntax());
        assert_eq!(items[1].next_sibling().unwrap().syntax(), items[2].syntax());
        assert!(items[2].next_sibling().is_none());
    }

    #[test]
    fn bullet_texts() {
        let (_, item) = first_item("- a\n");
        assert_eq!(item.bullet().text(), "-");
        let (_, item) = first_item("+ a\n");
        assert_eq!(item.bullet().text(), "+");
        let (_, item) = first_item("1. a\n");
        assert_eq!(item.bullet().text(), "1.");
        let (_, item) = first_item("1) a\n");
        assert_eq!(item.bullet().text(), "1)");
    }
}
