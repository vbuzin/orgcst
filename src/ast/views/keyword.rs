//! Keyword lines (`#+KEY: value`), affiliated-keyword runs and babel calls.
//!
//! Canonical names and the dual, multiple and parsed properties of a key
//! follow org-element's tables (`org-element-keyword-translation-alist`,
//! `org-element-dual-keywords`, `org-element-multiple-keywords`,
//! `org-element-parsed-keywords`). A dual key takes an optional `[...]` value
//! before the colon, as in `#+CAPTION[short]: long`.

use crate::TextRange;
use crate::TextSize;
use crate::ast::layout::{
    RemovalSide, content_of, delimited_interior, field_dest, removal_extending, text_in,
    with_node_text,
};
use crate::ast::support::{child_of_kind, child_view, child_views};
use crate::ast::{
    AstNode, Element, HasObjects, HasParts, InsertPoint, Padding, PartSpan, PresentPart,
};
use crate::parser::grammar::forms::affiliated;
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A keyword line, `#+KEY: value`, setting keywords such as `#+TODO:`
    /// included.
    Keyword,
    [OrgSyntaxKind::KEYWORD, OrgSyntaxKind::SETTING_KEYWORD]
);
crate::ast_node!(
    /// The value of a keyword line, after the colon.
    KeywordValue,
    OrgSyntaxKind::KEYWORD_VALUE
);
crate::ast_node!(
    /// The optional `[...]` value of a dual keyword, as in
    /// `#+CAPTION[short]: long`.
    KeywordDual,
    OrgSyntaxKind::KEYWORD_DUAL
);
crate::ast_node!(
    /// A run of keyword lines, such as `#+NAME:` or `#+CAPTION:`, that
    /// attaches to the element after it.
    AffiliatedKeywords,
    OrgSyntaxKind::AFFILIATED_KEYWORDS
);
crate::ast_node!(
    /// A babel call line: `#+CALL: NAME[HEADER](ARGS) HEADER`.
    BabelCall,
    OrgSyntaxKind::BABEL_CALL
);
crate::ast::impl_leaf_element_view!(Keyword, BabelCall);

/// Whether a keyword line is an in-buffer setting, as the parser decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordRole {
    /// Any other keyword, such as `#+TITLE:`.
    Plain,
    /// An in-buffer setting the parser reads, such as `#+TODO:`.
    Setting,
}

/// Parts of a keyword line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordPart {
    /// The key after `#+`.
    Key,
    /// The `[...]` before the colon; `NotApplicable` when the key is not dual.
    Dual,
    /// The value after the colon.
    Value,
}

/// Parts of a `#+CALL:` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BabelCallPart {
    /// The name of the called block.
    Name,
    /// `[HEADER]` after the name; optional.
    InsideHeader,
    /// `(ARGS)`.
    Arguments,
    /// The rest of the line after the arguments. `NotApplicable` until `[` or
    /// `(` ends the name, because text written right after the name would
    /// extend it.
    EndHeader,
}

impl Keyword {
    /// Whether this is a plain keyword or an in-buffer setting.
    #[must_use]
    pub fn role(&self) -> KeywordRole {
        match self.syntax().kind() {
            OrgSyntaxKind::SETTING_KEYWORD => KeywordRole::Setting,
            _ => KeywordRole::Plain,
        }
    }

    /// The `KEYWORD_KEY` child.
    ///
    /// # Panics
    ///
    /// Panics if `KEYWORD_KEY` is missing, which the parser never produces.
    #[must_use]
    pub(crate) fn key_node(&self) -> SyntaxNode {
        key_node(self.syntax())
    }

    /// The key as written.
    ///
    /// Allocates; to compare a key, use [`Self::key_eq`].
    ///
    /// # Panics
    ///
    /// Panics if `KEYWORD_KEY` is missing, which the parser never produces.
    #[must_use]
    pub fn key(&self) -> String {
        with_node_text(&self.key_node(), str::to_string)
    }

    /// Whether the key equals `want`, ignoring ASCII case.
    #[must_use]
    pub fn key_eq(&self, want: &str) -> bool {
        with_node_text(&self.key_node(), |s| s.eq_ignore_ascii_case(want))
    }

    /// The key's canonical name as org-element translates it; `HEADERS`
    /// becomes `HEADER`, for example.
    #[must_use]
    pub fn canonical_key(&self) -> String {
        with_node_text(&self.key_node(), |s| {
            affiliated::canonical_key(s).to_string()
        })
    }

    /// Whether repeated lines with this key accumulate instead of overriding
    /// each other (`CAPTION`, `HEADER`, every `ATTR_*`).
    #[must_use]
    pub fn is_multiple(&self) -> bool {
        with_node_text(&self.key_node(), affiliated::is_multiple_key)
    }

    /// Whether the value and dual value hold objects rather than plain text
    /// (`CAPTION`).
    #[must_use]
    pub fn is_parsed(&self) -> bool {
        with_node_text(&self.key_node(), affiliated::is_parsed_key)
    }

    /// Text of the dual value between the brackets, when a dual key has one.
    #[must_use]
    pub fn dual(&self) -> Option<String> {
        self.part_content_range(KeywordPart::Dual)
            .map(|r| text_in(self.syntax(), r))
    }

    /// The `[...]` node, if the line has one, even when the key is not dual.
    #[must_use]
    pub fn dual_node(&self) -> Option<KeywordDual> {
        child_view(self.syntax())
    }

    /// The value, which may be empty.
    ///
    /// # Panics
    ///
    /// Panics if `KEYWORD_VALUE` is missing, which the parser never produces.
    #[must_use]
    pub fn value(&self) -> KeywordValue {
        child_view(self.syntax()).expect(
            "grammar: KEYWORD always has KEYWORD_VALUE (forms/keyword.rs::parse_keyword_value)",
        )
    }

    /// Where to write a value when the value is empty.
    ///
    /// The offset is the start of the value node. The lead is a space unless
    /// whitespace already ends there.
    #[must_use]
    pub fn value_fill_insert_point(&self) -> InsertPoint {
        field_dest(self.syntax(), self.value().range().start(), Padding::None)
    }
}

impl KeywordValue {
    /// The value without trailing spaces and tabs: the span a rewrite
    /// replaces.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        content_of(self.syntax())
    }

    /// Whether the value holds objects; see [`Keyword::is_parsed`].
    #[must_use]
    pub fn is_object_region(&self) -> bool {
        parent_keyword(self.syntax()).is_parsed()
    }
}

impl KeywordDual {
    /// Interior of the `[...]` pair.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a bracket pair, which the parser never
    /// produces.
    #[must_use]
    pub fn content_range(&self) -> TextRange {
        delimited_interior(
            self.syntax(),
            OrgSyntaxKind::L_BRACKET,
            OrgSyntaxKind::R_BRACKET,
        )
        .expect("grammar: KEYWORD_DUAL is a bracket pair (forms/keyword.rs::parse_optional_dual)")
    }

    /// Whether the dual value holds objects; see [`Keyword::is_parsed`].
    #[must_use]
    pub fn is_object_region(&self) -> bool {
        parent_keyword(self.syntax()).is_parsed()
    }
}

impl HasObjects for KeywordValue {}
impl HasObjects for KeywordDual {}

fn parent_keyword(node: &SyntaxNode) -> Keyword {
    node.parent()
        .and_then(Keyword::cast)
        .expect("grammar: KEYWORD_VALUE / KEYWORD_DUAL is a child of KEYWORD (forms/keyword.rs)")
}

impl HasParts for Keyword {
    type Part = KeywordPart;
    const PARTS: &'static [KeywordPart] =
        &[KeywordPart::Key, KeywordPart::Dual, KeywordPart::Value];

    fn part_span(&self, part: KeywordPart) -> PartSpan {
        match part {
            KeywordPart::Key => {
                PartSpan::Present(PresentPart::identical(key_node(self.syntax()).text_range()))
            }
            KeywordPart::Dual => dual_part(self.syntax()),
            KeywordPart::Value => {
                let node = self.value();
                let carved = node.range();
                let content = node.content_range();
                let removal = removal_extending(self.syntax(), carved, RemovalSide::Before);
                PartSpan::Present(PresentPart::new(carved, content, removal))
            }
        }
    }
}

fn dual_part(syntax: &SyntaxNode) -> PartSpan {
    if !with_node_text(&key_node(syntax), affiliated::is_dual_key) {
        return PartSpan::NotApplicable;
    }
    match child_of_kind(syntax, OrgSyntaxKind::KEYWORD_DUAL) {
        Some(node) => {
            let carved = node.text_range();
            let content = delimited_interior(
                &node,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET,
            )
            .expect(
                "grammar: KEYWORD_DUAL is a bracket pair (forms/keyword.rs::parse_optional_dual)",
            );
            PartSpan::Present(PresentPart::new(carved, content, carved))
        }
        None => PartSpan::Absent(InsertPoint::new(
            key_node(syntax).text_range().end(),
            Padding::None,
            Padding::None,
        )),
    }
}

impl AffiliatedKeywords {
    /// The keyword lines of the run. They are not [`Element`]s.
    pub fn keywords(&self) -> impl Iterator<Item = Keyword> + '_ {
        child_views(self.syntax())
    }

    /// The element this run attaches to.
    ///
    /// # Panics
    ///
    /// Panics if the run has no next sibling or it is not a body element. The
    /// parser only produces a run directly before an element that accepts it.
    #[must_use]
    pub fn target(&self) -> Element {
        let sib = self.syntax().next_sibling().expect(
            "grammar: AFFILIATED_KEYWORDS always has a next sibling (forms/affiliated.rs I3/I4)",
        );
        Element::cast(sib).expect(
            "grammar: the next sibling of AFFILIATED_KEYWORDS always accepts affiliation (I3)",
        )
    }

    /// Members whose canonical name matches that of `canonical`, in source
    /// order.
    pub fn by_key<'a>(&'a self, canonical: &'a str) -> impl Iterator<Item = Keyword> + 'a {
        self.keywords()
            .filter(move |kw| keyword_matches_canonical(kw, canonical))
    }

    /// Members whose canonical name is `NAME`.
    pub fn names(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.by_key("NAME")
    }

    /// Members whose canonical name is `CAPTION`.
    pub fn captions(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.by_key("CAPTION")
    }

    /// Members whose canonical name is `HEADER`.
    pub fn headers(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.by_key("HEADER")
    }

    /// Members whose canonical name is `RESULTS`.
    pub fn results(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.by_key("RESULTS")
    }

    /// Every `ATTR_*` member, in source order.
    pub fn attrs(&self) -> impl Iterator<Item = Keyword> + '_ {
        self.keywords().filter(keyword_is_attr)
    }

    /// The last `ATTR_<backend>` member, matching `backend` ASCII caseless.
    #[must_use]
    pub fn attr(&self, backend: &str) -> Option<Keyword> {
        self.keywords()
            .filter(|kw| keyword_attr_backend_eq(kw, backend))
            .last()
    }
}

impl HasParts for BabelCall {
    type Part = BabelCallPart;
    const PARTS: &'static [BabelCallPart] = &[
        BabelCallPart::Name,
        BabelCallPart::InsideHeader,
        BabelCallPart::Arguments,
        BabelCallPart::EndHeader,
    ];

    fn part_span(&self, part: BabelCallPart) -> PartSpan {
        let syntax = self.syntax();
        let name_end = call_name_node(syntax).text_range().end();
        match part {
            BabelCallPart::Name => {
                let node = call_name_node(syntax);
                let carved = node.text_range();
                PartSpan::Present(PresentPart::new(carved, content_of(&node), carved))
            }
            BabelCallPart::InsideHeader => delimited_call_part(
                syntax,
                OrgSyntaxKind::CALL_INSIDE_HEADER,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::R_BRACKET,
                name_end,
            ),
            BabelCallPart::Arguments => delimited_call_part(
                syntax,
                OrgSyntaxKind::CALL_ARGUMENTS,
                OrgSyntaxKind::L_PAREN,
                OrgSyntaxKind::R_PAREN,
                child_of_kind(syntax, OrgSyntaxKind::CALL_INSIDE_HEADER)
                    .map_or(name_end, |n| n.text_range().end()),
            ),
            BabelCallPart::EndHeader => end_header_part(syntax),
        }
    }
}

fn delimited_call_part(
    syntax: &SyntaxNode,
    kind: OrgSyntaxKind,
    open: OrgSyntaxKind,
    close: OrgSyntaxKind,
    insert: TextSize,
) -> PartSpan {
    match child_of_kind(syntax, kind) {
        Some(node) => {
            let carved = node.text_range();
            let content = delimited_interior(&node, open, close).expect(
                "grammar: CALL_INSIDE_HEADER / CALL_ARGUMENTS include their delimiters (forms/call.rs::consume_call_value)",
            );
            PartSpan::Present(PresentPart::new(carved, content, carved))
        }
        None => PartSpan::Absent(InsertPoint::new(insert, Padding::None, Padding::None)),
    }
}

fn end_header_part(syntax: &SyntaxNode) -> PartSpan {
    if let Some(node) = child_of_kind(syntax, OrgSyntaxKind::CALL_END_HEADER) {
        let carved = node.text_range();
        return PartSpan::Present(PresentPart::identical(carved));
    }
    // Spaces are legal in CALL_NAME. The rest-of-line slot only exists once
    // `[` or `(` has terminated the name.
    match child_of_kind(syntax, OrgSyntaxKind::CALL_ARGUMENTS)
        .or_else(|| child_of_kind(syntax, OrgSyntaxKind::CALL_INSIDE_HEADER))
    {
        Some(term) => PartSpan::Absent(InsertPoint::new(
            term.text_range().end(),
            Padding::None,
            Padding::None,
        )),
        None => PartSpan::NotApplicable,
    }
}

fn key_node(kw: &SyntaxNode) -> SyntaxNode {
    child_of_kind(kw, OrgSyntaxKind::KEYWORD_KEY)
        .expect("grammar: KEYWORD always has KEYWORD_KEY (forms/keyword.rs::parse_interior)")
}

fn call_name_node(call: &SyntaxNode) -> SyntaxNode {
    child_of_kind(call, OrgSyntaxKind::CALL_NAME)
        .expect("grammar: BABEL_CALL always has CALL_NAME (forms/call.rs::consume_call_value)")
}

fn keyword_matches_canonical(kw: &Keyword, query: &str) -> bool {
    let want = affiliated::canonical_key(query);
    with_node_text(&kw.key_node(), |s| {
        affiliated::canonical_key(s).eq_ignore_ascii_case(want)
    })
}

fn keyword_is_attr(kw: &Keyword) -> bool {
    with_node_text(&kw.key_node(), |s| affiliated::attr_backend(s).is_some())
}

fn keyword_attr_backend_eq(kw: &Keyword, backend: &str) -> bool {
    with_node_text(&kw.key_node(), |s| {
        affiliated::attr_backend(s).is_some_and(|b| b.eq_ignore_ascii_case(backend))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        Block, BlockBody, Container, Element, ElementView, HasElements, HasParts, Padding, PartSpan,
    };
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice, written};

    fn keyword(src: &str, kind: OrgSyntaxKind) -> (crate::Document, Keyword) {
        let doc = parse(src);
        let kw = Keyword::cast(
            nodes_of_kind(doc.root(), kind)
                .into_iter()
                .next()
                .expect("keyword"),
        )
        .expect("cast");
        (doc, kw)
    }

    fn babel(src: &str) -> (crate::Document, BabelCall) {
        let doc = parse(src);
        let call = BabelCall::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::BABEL_CALL)
                .into_iter()
                .next()
                .expect("call"),
        )
        .expect("cast");
        (doc, call)
    }

    fn affiliated_run(src: &str) -> (crate::Document, AffiliatedKeywords) {
        let doc = parse(src);
        let run = AffiliatedKeywords::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::AFFILIATED_KEYWORDS)
                .into_iter()
                .next()
                .expect("run"),
        )
        .expect("cast");
        (doc, run)
    }

    #[test]
    fn keyword_and_setting_cast_as_leaf_elements() {
        let (doc, kw) = keyword("#+TITLE: x\n", OrgSyntaxKind::KEYWORD);
        assert_eq!(kw.role(), KeywordRole::Plain);
        assert_eq!(kw.key(), "TITLE");
        assert!(kw.key_eq("title"));
        assert!(!kw.key_eq("TODO"));
        assert_eq!(kw.canonical_key(), "TITLE");
        assert!(kw.content_range().is_none());
        assert!(!kw.is_greater());
        assert!(Element::cast(kw.syntax().clone()).is_some());
        assert_eq!(source_slice(&doc, kw.value().content_range()), "x");
        crate::ast::check_part_contract(&kw);

        let (doc, sk) = keyword("#+TODO: A | B\n", OrgSyntaxKind::SETTING_KEYWORD);
        assert_eq!(sk.role(), KeywordRole::Setting);
        assert_eq!(sk.key(), "TODO");
        assert!(sk.content_range().is_none());
        assert!(!sk.is_greater());
        assert!(Element::cast(sk.syntax().clone()).is_some());
        assert_eq!(source_slice(&doc, sk.value().content_range()), "A | B");
        assert!(matches!(
            sk.part_span(KeywordPart::Dual),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&sk);
    }

    #[test]
    fn name_table_flags_are_view_readers() {
        let (_, caption) = keyword("#+CAPTION: x\n", OrgSyntaxKind::KEYWORD);
        assert!(caption.is_multiple());
        assert!(caption.is_parsed());

        let (_, title) = keyword("#+TITLE: x\n", OrgSyntaxKind::KEYWORD);
        assert!(!title.is_multiple());
        assert!(!title.is_parsed());

        let (_, name) = keyword("#+NAME: x\n", OrgSyntaxKind::KEYWORD);
        assert!(!name.is_multiple());
        assert!(!name.is_parsed());

        let (_, headers) = keyword("#+HEADERS: a\n", OrgSyntaxKind::KEYWORD);
        assert!(headers.is_multiple());
        assert!(!headers.is_parsed());
        assert_eq!(headers.canonical_key(), "HEADER");

        let (_, attr) = keyword("#+ATTR_HTML: :x\n", OrgSyntaxKind::KEYWORD);
        assert!(attr.is_multiple());
        assert!(!attr.is_parsed());
    }

    #[test]
    fn tblfm_and_run_member_fail_element_cast() {
        let doc = parse("| a |\n#+TBLFM: $1=$1\n");
        let kw = nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
            .into_iter()
            .next()
            .expect("tblfm");
        assert!(Keyword::cast(kw.clone()).is_some());
        assert!(Element::cast(kw).is_none());

        let doc = parse("#+CAPTION: c\npara\n");
        let kw = nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
            .into_iter()
            .next()
            .expect("caption");
        assert!(Keyword::cast(kw.clone()).is_some());
        assert!(Element::cast(kw).is_none());
    }

    #[test]
    fn babel_call_is_a_leaf() {
        let (_, call) = babel("#+CALL: name()\n");
        assert!(call.content_range().is_none());
        assert!(!call.is_greater());
        crate::ast::check_part_contract(&call);
    }

    #[test]
    fn dual_caption_and_non_dual_not_applicable() {
        let (doc, kw) = keyword("#+CAPTION[src]: text\n", OrgSyntaxKind::KEYWORD);
        assert_eq!(kw.dual().as_deref(), Some("src"));
        assert_eq!(
            source_slice(&doc, kw.part_range(KeywordPart::Dual).unwrap()),
            "[src]"
        );
        assert_eq!(
            source_slice(&doc, kw.part_content_range(KeywordPart::Value).unwrap()),
            "text"
        );
        crate::ast::check_part_contract(&kw);

        let (doc, title) = keyword("#+TITLE: x\n", OrgSyntaxKind::KEYWORD);
        assert!(title.dual().is_none());
        assert!(matches!(
            title.part_span(KeywordPart::Dual),
            PartSpan::NotApplicable
        ));
        let rm = title.part_removal_range(KeywordPart::Value).unwrap();
        assert!(source_slice(&doc, rm).starts_with(' '));
        assert!(source_slice(&doc, rm).contains('x'));
        assert!(!source_slice(&doc, rm).contains('\n'));

        let (_, named) = keyword("#+NAME[s]: v\n", OrgSyntaxKind::KEYWORD);
        assert!(
            child_of_kind(named.syntax(), OrgSyntaxKind::KEYWORD_DUAL).is_some(),
            "the slot is still carved"
        );
        assert!(matches!(
            named.part_span(KeywordPart::Dual),
            PartSpan::NotApplicable
        ));
        assert!(named.dual().is_none());
        assert!(
            !named.dual_node().unwrap().is_object_region(),
            "NAME dual is carved geometry, not an object region"
        );
        crate::ast::check_part_contract(&named);
    }

    #[test]
    fn dual_absent_insert_and_greedy_interior() {
        let (doc, kw) = keyword("#+CAPTION: text\n", OrgSyntaxKind::KEYWORD);
        assert!(kw.dual().is_none());
        let ip = kw.part_insert_point(KeywordPart::Dual).unwrap();
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(ip.trail(), Padding::None);
        assert_eq!(written(doc.source(), ip, "[src]"), "#+CAPTION[src]: text\n");
        crate::ast::check_part_contract(&kw);

        let (doc, greedy) = keyword("#+CAPTION[a][b]: v\n", OrgSyntaxKind::KEYWORD);
        assert_eq!(greedy.dual().as_deref(), Some("a][b"));
        assert_eq!(
            source_slice(&doc, greedy.part_content_range(KeywordPart::Dual).unwrap()),
            "a][b"
        );
    }

    #[test]
    fn empty_dual_is_present_empty_not_absent() {
        let (doc, kw) = keyword("#+CAPTION[]: body\n", OrgSyntaxKind::KEYWORD);
        assert!(matches!(
            kw.part_span(KeywordPart::Dual),
            PartSpan::Present(_)
        ));
        assert!(kw.part_content_range(KeywordPart::Dual).unwrap().is_empty());
        assert_eq!(
            source_slice(&doc, kw.part_range(KeywordPart::Dual).unwrap()),
            "[]"
        );
        assert_eq!(kw.dual().as_deref(), Some(""));
        assert!(kw.part_insert_point(KeywordPart::Dual).is_none());
        crate::ast::check_part_contract(&kw);
    }

    #[test]
    fn dual_interior_keeps_inner_space() {
        let (doc, kw) = keyword("#+CAPTION[short cap]: v\n", OrgSyntaxKind::KEYWORD);
        assert_eq!(kw.dual().as_deref(), Some("short cap"));
        assert_eq!(
            source_slice(&doc, kw.part_content_range(KeywordPart::Dual).unwrap()),
            "short cap"
        );
        crate::ast::check_part_contract(&kw);

        let (doc, call) = babel("#+CALL: n[:in 1]()\n");
        assert_eq!(
            source_slice(
                &doc,
                call.part_content_range(BabelCallPart::InsideHeader)
                    .unwrap()
            ),
            ":in 1"
        );
        crate::ast::check_part_contract(&call);
    }

    #[test]
    fn value_content_strips_trailing_ws_and_empty_value_is_present() {
        let (doc, kw) = keyword("  #+TITLE:  hello  \n", OrgSyntaxKind::KEYWORD);
        assert_eq!(
            source_slice(&doc, kw.part_content_range(KeywordPart::Value).unwrap()),
            "hello"
        );
        assert_eq!(source_slice(&doc, kw.value().content_range()), "hello");
        crate::ast::check_part_contract(&kw);

        let (_, empty) = keyword("#+TITLE:\n", OrgSyntaxKind::KEYWORD);
        assert!(empty.value().content_range().is_empty());
        assert_eq!(
            empty.part_content_range(KeywordPart::Value),
            Some(empty.value().content_range())
        );
        assert!(
            empty
                .part_content_range(KeywordPart::Value)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            empty.part_span(KeywordPart::Value),
            PartSpan::Present(_)
        ));
        crate::ast::check_part_contract(&empty);
    }

    #[test]
    fn value_fill_insert_point_pads_bare_colon_not_existing_space() {
        let (_, bare) = keyword("#+TITLE:\n", OrgSyntaxKind::KEYWORD);
        let dest = bare.value_fill_insert_point();
        assert_eq!(dest.offset(), bare.value().range().start());
        assert_eq!(dest.lead(), Padding::Space);
        assert_eq!(dest.trail(), Padding::None);

        let (_, spaced) = keyword("#+TITLE: \n", OrgSyntaxKind::KEYWORD);
        let dest = spaced.value_fill_insert_point();
        assert_eq!(dest.offset(), spaced.value().range().start());
        assert_eq!(dest.lead(), Padding::None);
        assert_eq!(dest.trail(), Padding::None);
    }

    #[test]
    fn affiliated_target_full_range_and_not_an_element() {
        let src = "#+CAPTION: c\npara\n";
        let (doc, run) = affiliated_run(src);
        assert!(Element::cast(run.syntax().clone()).is_none());
        let member = run.keywords().next().expect("member");
        assert!(Element::cast(member.syntax().clone()).is_none());
        let target = run.target();
        assert_eq!(target.kind(), OrgSyntaxKind::PARAGRAPH);
        assert_eq!(target.full_range().start(), run.range().start());
        assert_eq!(target.range().start(), run.range().end());
        assert!(target.range().start() > target.full_range().start());
        assert_eq!(source_slice(&doc, target.full_range()), src);

        let els: Vec<_> = doc.elements().collect();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].kind(), OrgSyntaxKind::PARAGRAPH);
        assert!(
            els.iter()
                .all(|e| e.kind() != OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );
    }

    #[test]
    fn container_elements_skip_the_run() {
        let src = "#+begin_quote\n#+NAME: inner\nquoted\n#+end_quote\n";
        let doc = parse(src);
        let block = Block::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::QUOTE_BLOCK)
                .into_iter()
                .next()
                .expect("quote"),
        )
        .expect("cast");
        let BlockBody::Elements(container) = block.body() else {
            panic!("quote is container-mode");
        };
        let els: Vec<_> = container.elements().collect();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].kind(), OrgSyntaxKind::PARAGRAPH);
        assert!(Container::cast(block.syntax().clone()).is_some());
    }

    #[test]
    fn names_results_and_attrs_use_the_name_table() {
        let src = concat!(
            "#+RESNAME: fig\n",
            "#+NAME: second\n",
            "#+RESULT: out\n",
            "#+CAPTION: one\n",
            "#+CAPTION: two\n",
            "#+HEADER: h1\n",
            "#+HEADERS: h2\n",
            "#+ATTR_HTML: :class x\n",
            "#+ATTR_HTML: :id y\n",
            "#+ATTR_ORG: :radio\n",
            "para\n",
        );
        let (_, run) = affiliated_run(src);
        let names: Vec<_> = run.names().map(|k| k.key()).collect();
        assert_eq!(names, ["RESNAME", "NAME"]);
        assert_eq!(run.by_key("NAME").count(), 2);
        assert_eq!(
            run.names().last().map(|k| k.key()),
            Some("NAME".into()),
            "source-order last is last-wins for a non-multiple name"
        );

        let results: Vec<_> = run.results().map(|k| k.key()).collect();
        assert_eq!(results, ["RESULT"]);
        assert_eq!(run.by_key("RESULTS").count(), 1);

        let caption_vals: Vec<_> = run
            .captions()
            .map(|k| k.value().syntax().text().to_string())
            .collect();
        assert_eq!(caption_vals, ["one", "two"]);

        let headers: Vec<_> = run.headers().map(|k| k.key()).collect();
        assert_eq!(headers, ["HEADER", "HEADERS"]);

        let html = run.attr("html").expect("ATTR_HTML");
        assert!(html.key().eq_ignore_ascii_case("ATTR_HTML"));
        assert_eq!(
            html.value().syntax().text().to_string(),
            ":id y",
            "attr last-wins among matching backends"
        );
        let org = run.attr("org").expect("ATTR_ORG");
        assert!(org.key().eq_ignore_ascii_case("ATTR_ORG"));
        assert_eq!(org.value().syntax().text().to_string(), ":radio");
        assert_eq!(run.attrs().count(), 3);
        assert!(run.attr("latex").is_none());
    }

    #[test]
    fn babel_parts_present_absent_and_insert() {
        let (doc, call) = babel("#+CALL: name[in](a,b) leftover\n");
        assert_eq!(
            source_slice(&doc, call.part_content_range(BabelCallPart::Name).unwrap()),
            "name"
        );
        assert_eq!(
            source_slice(
                &doc,
                call.part_content_range(BabelCallPart::InsideHeader)
                    .unwrap()
            ),
            "in"
        );
        assert_eq!(
            source_slice(
                &doc,
                call.part_content_range(BabelCallPart::Arguments).unwrap()
            ),
            "a,b"
        );
        assert_eq!(
            source_slice(&doc, call.part_range(BabelCallPart::EndHeader).unwrap()),
            " leftover"
        );
        crate::ast::check_part_contract(&call);

        let (doc, bare) = babel("#+CALL: name()\n");
        assert!(
            bare.part_insert_point(BabelCallPart::InsideHeader)
                .is_some()
        );
        assert!(bare.part_range(BabelCallPart::Arguments).is_some());
        assert!(
            bare.part_content_range(BabelCallPart::Arguments)
                .unwrap()
                .is_empty()
        );
        assert!(bare.part_insert_point(BabelCallPart::EndHeader).is_some());
        let ip = bare.part_insert_point(BabelCallPart::InsideHeader).unwrap();
        assert_eq!(ip.lead(), Padding::None);
        assert_eq!(ip.trail(), Padding::None);
        assert_eq!(written(doc.source(), ip, "[in]"), "#+CALL: name[in]()\n");
        let end = bare.part_insert_point(BabelCallPart::EndHeader).unwrap();
        assert_eq!(
            written(doc.source(), end, " leftover"),
            "#+CALL: name() leftover\n"
        );
        crate::ast::check_part_contract(&bare);

        let (doc, no_args) = babel("#+CALL: name\n");
        assert!(matches!(
            no_args.part_span(BabelCallPart::EndHeader),
            PartSpan::NotApplicable
        ));
        assert!(
            no_args
                .part_insert_point(BabelCallPart::EndHeader)
                .is_none()
        );
        let args_ip = no_args.part_insert_point(BabelCallPart::Arguments).unwrap();
        assert_eq!(written(doc.source(), args_ip, "()"), "#+CALL: name()\n");
        crate::ast::check_part_contract(&no_args);

        let (doc, inside_only) = babel("#+CALL: name[in]\n");
        let end = inside_only
            .part_insert_point(BabelCallPart::EndHeader)
            .unwrap();
        assert_eq!(
            written(doc.source(), end, " leftover"),
            "#+CALL: name[in] leftover\n"
        );
        crate::ast::check_part_contract(&inside_only);

        let (doc, both) = babel("#+CALL: name[in]()\n");
        let end = both.part_insert_point(BabelCallPart::EndHeader).unwrap();
        assert_eq!(end.lead(), Padding::None);
        assert_eq!(end.trail(), Padding::None);
        assert_eq!(
            written(doc.source(), end, " leftover"),
            "#+CALL: name[in]() leftover\n"
        );
        crate::ast::check_part_contract(&both);

        let (_, empty_in) = babel("#+CALL: name[]()\n");
        assert!(
            empty_in
                .part_content_range(BabelCallPart::InsideHeader)
                .unwrap()
                .is_empty()
        );
        crate::ast::check_part_contract(&empty_in);

        let (_, empty_name) = babel("#+CALL:\n");
        assert!(
            empty_name
                .part_content_range(BabelCallPart::Name)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            empty_name.part_span(BabelCallPart::EndHeader),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&empty_name);
    }

    #[test]
    fn caption_dual_is_object_region_title_and_results_are_not() {
        use crate::ast::HasObjects;
        let (_, cap) = keyword("#+CAPTION[src *b*]: hello *x*\n", OrgSyntaxKind::KEYWORD);
        assert!(cap.is_parsed());
        assert!(cap.value().is_object_region());
        assert!(cap.value().objects().count() >= 1);
        let dual = cap.dual_node().expect("dual");
        assert!(dual.is_object_region());
        assert!(dual.objects().count() >= 1);

        let (_, title) = keyword("#+TITLE: x\n", OrgSyntaxKind::KEYWORD);
        assert!(!title.value().is_object_region());
        assert_eq!(title.value().objects().count(), 0);

        let (_, results) = keyword("#+RESULTS[hash]: v\n", OrgSyntaxKind::KEYWORD);
        assert!(results.dual_node().is_some());
        assert!(matches!(
            results.part_span(KeywordPart::Dual),
            PartSpan::Present(_)
        ));
        assert!(!results.dual_node().unwrap().is_object_region());
        assert_eq!(results.dual_node().unwrap().objects().count(), 0);
        assert!(!results.value().is_object_region());
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::KEYWORD)
                .into_iter()
                .chain(nodes_of_kind(doc.root(), OrgSyntaxKind::SETTING_KEYWORD))
            {
                let kw = Keyword::cast(node).unwrap();
                crate::ast::check_part_contract(&kw);
            }
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::BABEL_CALL) {
                let call = BabelCall::cast(node).unwrap();
                crate::ast::check_part_contract(&call);
            }
        }
    }
}
