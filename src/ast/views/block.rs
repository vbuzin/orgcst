//! Blocks (`#+begin_NAME` ... `#+end_NAME`) and dynamic blocks
//! (`#+BEGIN: name` ... `#+END:`). One view covers all nine kinds.
//!
//! The kind fixes what the body holds: verbatim text (src, example, export,
//! comment), objects (verse) or elements (quote, center, special, dynamic).
//! [`Block`] implements neither `HasElements` nor `HasObjects`; reach the body
//! through [`Block::body`], which says which of the three it is.

use crate::TextRange;
use crate::ast::layout::{
    block_closer, block_first_line, block_header, block_token_interior, first_line_end,
    optional_after, text_in,
};
use crate::ast::support::element_run_range;
use crate::ast::{AstNode, Container, ElementView, HasObjects, HasParts, PartSpan, PresentPart};
use crate::parser::grammar::drive::frame::BodyMode;
use crate::parser::grammar::forms::block::block_body_mode;
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A block, `#+begin_NAME` ... `#+end_NAME`, or a dynamic block,
    /// `#+BEGIN: name` ... `#+END:`.
    Block,
    [
        OrgSyntaxKind::SRC_BLOCK,
        OrgSyntaxKind::EXAMPLE_BLOCK,
        OrgSyntaxKind::EXPORT_BLOCK,
        OrgSyntaxKind::COMMENT_BLOCK,
        OrgSyntaxKind::VERSE_BLOCK,
        OrgSyntaxKind::QUOTE_BLOCK,
        OrgSyntaxKind::CENTER_BLOCK,
        OrgSyntaxKind::SPECIAL_BLOCK,
        OrgSyntaxKind::DYNAMIC_BLOCK,
    ]
);

crate::ast_node!(
    /// The body of a verse block, which holds objects; reached through
    /// [`Block::body`].
    VerseBody,
    OrgSyntaxKind::VERSE_BLOCK
);

impl HasObjects for VerseBody {}

/// Which kind of block a [`Block`] is. The kind fixes what the body holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// `#+begin_src`.
    Src,
    /// `#+begin_example`.
    Example,
    /// `#+begin_export`.
    Export,
    /// `#+begin_comment`.
    Comment,
    /// `#+begin_verse`.
    Verse,
    /// `#+begin_quote`.
    Quote,
    /// `#+begin_center`.
    Center,
    /// Any other `#+begin_NAME`.
    Special,
    /// `#+BEGIN: name`.
    Dynamic,
}

/// The body of a block, by what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlockBody {
    /// Src, example, export and comment blocks: the body's range, plain text.
    /// Equal to the content of [`BlockPart::Body`].
    Verbatim(TextRange),
    /// Verse blocks: the body holds objects.
    Objects(VerseBody),
    /// Quote, center, special and dynamic blocks: the body holds elements.
    Elements(Container),
}

/// Parts of a block.
///
/// [`HasParts::PARTS`] lists them in this fixed order, not in source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockPart {
    /// The opening marker, such as `#+begin_src`, `#+begin_` before a special
    /// block's name, or `#+BEGIN:`; not the whole line.
    BeginMarker,
    /// Special-block name after `#+begin_`, or dynamic-block name after
    /// `#+BEGIN:`.
    Name,
    /// The rest of the begin line after the fields before it: a src block's
    /// header arguments, an export block's extra text, or a dynamic block's
    /// arguments.
    Parameters,
    /// Src language or export backend.
    Language,
    /// Src switches (`-n`, `+n`, `-r`, `-i`, `-k`, `-l "..."`). Distinct from
    /// [`Self::Parameters`].
    Switches,
    /// The body between the begin and end lines; always present, zero-width
    /// when empty.
    Body,
    /// The closing marker, such as `#+end_src` or `#+END:`; not the whole
    /// line.
    EndMarker,
}

impl Block {
    /// Kind of this block.
    #[must_use]
    pub fn kind(&self) -> BlockKind {
        match self.syntax().kind() {
            OrgSyntaxKind::SRC_BLOCK => BlockKind::Src,
            OrgSyntaxKind::EXAMPLE_BLOCK => BlockKind::Example,
            OrgSyntaxKind::EXPORT_BLOCK => BlockKind::Export,
            OrgSyntaxKind::COMMENT_BLOCK => BlockKind::Comment,
            OrgSyntaxKind::VERSE_BLOCK => BlockKind::Verse,
            OrgSyntaxKind::QUOTE_BLOCK => BlockKind::Quote,
            OrgSyntaxKind::CENTER_BLOCK => BlockKind::Center,
            OrgSyntaxKind::SPECIAL_BLOCK => BlockKind::Special,
            OrgSyntaxKind::DYNAMIC_BLOCK => BlockKind::Dynamic,
            other => unreachable!("grammar: Block only wraps *_BLOCK kinds, got {other:?}"),
        }
    }

    /// The body, typed by what it holds.
    ///
    /// # Panics
    ///
    /// Panics only if the parser's body mode for this kind disagrees with the
    /// views, which the crate's tests rule out.
    #[must_use]
    pub fn body(&self) -> BlockBody {
        match block_body_mode(self.syntax().kind()) {
            BodyMode::Opaque => BlockBody::Verbatim(block_token_interior(self.syntax())),
            BodyMode::Objects => BlockBody::Objects(
                VerseBody::cast(self.syntax().clone())
                    .expect("grammar: verse kind (forms/block.rs::block_body_modes_cover_every_block_kind)"),
            ),
            BodyMode::Elements => BlockBody::Elements(
                Container::cast(self.syntax().clone()).expect(
                    "grammar: container-mode kind (forms/block.rs::block_body_modes_cover_every_block_kind)",
                ),
            ),
        }
    }

    /// Span of a container block's body elements; zero-width after the begin
    /// line when empty.
    ///
    /// Child elements own their line terminators, so the span comes from the
    /// elements rather than from the block's own tokens.
    #[must_use]
    pub(crate) fn container_body_range(&self) -> TextRange {
        element_run_range(self.syntax())
            .unwrap_or_else(|| TextRange::empty(first_line_end(self.syntax())))
    }

    /// Src language or export backend, when that slot is present.
    #[must_use]
    pub fn language(&self) -> Option<String> {
        self.part_content_range(BlockPart::Language)
            .map(|r| text_in(self.syntax(), r))
    }

    /// Src switches as written (`-n -r`, `-l "fmt"`), when that slot is present.
    #[must_use]
    pub fn switches(&self) -> Option<String> {
        self.part_content_range(BlockPart::Switches)
            .map(|r| text_in(self.syntax(), r))
    }

    /// Begin-line arguments: a src block's header arguments, an export
    /// block's extra text, or a dynamic block's arguments. `None` when there
    /// are none, never an empty string.
    #[must_use]
    pub fn parameters(&self) -> Option<String> {
        self.part_content_range(BlockPart::Parameters)
            .map(|r| text_in(self.syntax(), r))
    }

    /// Special-block name (`#+begin_NAME`) or dynamic-block name after
    /// `#+BEGIN:`.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.part_content_range(BlockPart::Name)
            .map(|r| text_in(self.syntax(), r))
    }
}

impl HasParts for Block {
    type Part = BlockPart;
    const PARTS: &'static [BlockPart] = &[
        BlockPart::BeginMarker,
        BlockPart::Name,
        BlockPart::Parameters,
        BlockPart::Language,
        BlockPart::Switches,
        BlockPart::Body,
        BlockPart::EndMarker,
    ];

    fn part_span(&self, part: BlockPart) -> PartSpan {
        match part {
            BlockPart::BeginMarker => {
                let first = block_first_line(self.syntax());
                PartSpan::Present(PresentPart::identical(first.begin_marker))
            }
            BlockPart::EndMarker => PartSpan::Present(PresentPart::identical(
                block_closer(self.syntax()).end_marker,
            )),
            BlockPart::Body => {
                let r = match block_body_mode(self.syntax().kind()) {
                    BodyMode::Elements => self.container_body_range(),
                    BodyMode::Opaque | BodyMode::Objects => block_token_interior(self.syntax()),
                };
                PartSpan::Present(PresentPart::identical(r))
            }
            BlockPart::Name | BlockPart::Language | BlockPart::Switches | BlockPart::Parameters => {
                first_line_part(self.syntax(), part)
            }
        }
    }
}

impl ElementView for Block {
    fn content_range(&self) -> Option<TextRange> {
        match block_body_mode(self.syntax().kind()) {
            BodyMode::Opaque => None,
            BodyMode::Objects => Some(block_token_interior(self.syntax())),
            BodyMode::Elements => Some(self.container_body_range()),
        }
    }

    fn is_greater(&self) -> bool {
        matches!(block_body_mode(self.syntax().kind()), BodyMode::Elements)
    }
}

fn first_line_part(syntax: &SyntaxNode, part: BlockPart) -> PartSpan {
    let header = block_header(syntax.kind());
    let open = match part {
        BlockPart::Name => header.has_name(),
        BlockPart::Language => header.has_language(),
        BlockPart::Switches => header.has_switches(),
        BlockPart::Parameters => header.has_parameters(),
        _ => false,
    };
    if !open {
        return PartSpan::NotApplicable;
    }
    let first = block_first_line(syntax);
    let (carved, dest) = match part {
        BlockPart::Name => (first.name, first.begin_marker.end()),
        BlockPart::Language => (first.language, first.begin_marker.end()),
        BlockPart::Switches => (first.switches, first.after_language()),
        BlockPart::Parameters => (first.parameters, first.after_present_prefix()),
        _ => return PartSpan::NotApplicable,
    };
    optional_after(syntax, carved, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::block_header;
    use crate::ast::{HasParts, Padding, PartSpan};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, source_slice, written};

    fn block(src: &str, kind: OrgSyntaxKind) -> (crate::Document, Block) {
        let doc = parse(src);
        let b = Block::cast(
            nodes_of_kind(doc.root(), kind)
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("missing {kind:?}")),
        )
        .expect("cast");
        (doc, b)
    }

    #[test]
    fn kind_and_body_match_grammar_table() {
        let cases = [
            (
                "#+begin_src elisp\n(+ 1)\n#+end_src\n",
                OrgSyntaxKind::SRC_BLOCK,
                BlockKind::Src,
            ),
            (
                "#+begin_example\nx\n#+end_example\n",
                OrgSyntaxKind::EXAMPLE_BLOCK,
                BlockKind::Example,
            ),
            (
                "#+begin_export html\n<p/>\n#+end_export\n",
                OrgSyntaxKind::EXPORT_BLOCK,
                BlockKind::Export,
            ),
            (
                "#+begin_comment\nx\n#+end_comment\n",
                OrgSyntaxKind::COMMENT_BLOCK,
                BlockKind::Comment,
            ),
            (
                "#+begin_verse\nline\n#+end_verse\n",
                OrgSyntaxKind::VERSE_BLOCK,
                BlockKind::Verse,
            ),
            (
                "#+begin_quote\nq\n#+end_quote\n",
                OrgSyntaxKind::QUOTE_BLOCK,
                BlockKind::Quote,
            ),
            (
                "#+begin_center\nc\n#+end_center\n",
                OrgSyntaxKind::CENTER_BLOCK,
                BlockKind::Center,
            ),
            (
                "#+begin_foo\nb\n#+end_foo\n",
                OrgSyntaxKind::SPECIAL_BLOCK,
                BlockKind::Special,
            ),
            (
                "#+BEGIN: clocktable\n#+END:\n",
                OrgSyntaxKind::DYNAMIC_BLOCK,
                BlockKind::Dynamic,
            ),
        ];
        for (src, kind, want) in cases {
            let (doc, b) = block(src, kind);
            assert_eq!(b.kind(), want, "{src:?}");
            match (block_body_mode(kind), b.body()) {
                (BodyMode::Opaque, BlockBody::Verbatim(span)) => {
                    assert!(b.content_range().is_none(), "{src:?}");
                    assert!(!b.is_greater());
                    assert!(
                        b.syntax().children().next().is_none(),
                        "opaque body has no child nodes: {src:?}"
                    );
                    let text = source_slice(&doc, span);
                    assert!(
                        !text.contains("#+begin") && !text.contains("#+end"),
                        "verbatim span is the field, not the markers: {text:?}"
                    );
                }
                (BodyMode::Objects, BlockBody::Objects(_)) => {
                    assert!(b.content_range().is_some(), "{src:?}");
                    assert!(!b.is_greater());
                    assert!(Container::cast(b.syntax().clone()).is_none());
                }
                (BodyMode::Elements, BlockBody::Elements(_)) => {
                    assert!(b.content_range().is_some(), "{src:?}");
                    assert!(b.is_greater());
                    assert!(Container::cast(b.syntax().clone()).is_some());
                }
                (mode, body) => panic!("{src:?}: mode {mode:?} body {body:?}"),
            }
        }
    }

    #[test]
    fn empty_quote_content_is_zero_width() {
        let (doc, b) = block("#+begin_quote\n#+end_quote\n", OrgSyntaxKind::QUOTE_BLOCK);
        let r = b.content_range().expect("container-mode");
        assert!(r.is_empty());
        assert_eq!(r, b.container_body_range());
        assert_eq!(r.start(), first_line_end(b.syntax()));
        assert_eq!(&doc.source()[..usize::from(r.start())], "#+begin_quote\n");
    }

    #[test]
    fn quote_content_range_is_the_element_run() {
        let (doc, b) = block(
            "#+begin_quote\ninner\n#+end_quote\n",
            OrgSyntaxKind::QUOTE_BLOCK,
        );
        let r = b.content_range().expect("container-mode");
        assert!(!r.is_empty());
        assert_eq!(r, b.container_body_range());
        assert_eq!(source_slice(&doc, r).trim(), "inner");
    }

    #[test]
    fn src_content_range_is_none_verbatim_still_bounds() {
        let (doc, b) = block(
            "#+begin_src elisp\n(+ 1 2)\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert!(b.content_range().is_none());
        match b.body() {
            BlockBody::Verbatim(span) => {
                assert_eq!(source_slice(&doc, span), "(+ 1 2)\n");
                assert_eq!(
                    b.part_content_range(BlockPart::Body).expect("body field"),
                    span
                );
            }
            other => panic!("expected Verbatim, got {other:?}"),
        }
        crate::ast::check_part_contract(&b);
    }

    fn na_parts(kind: OrgSyntaxKind) -> Vec<BlockPart> {
        let h = block_header(kind);
        let mut out = Vec::new();
        if !h.has_name() {
            out.push(BlockPart::Name);
        }
        if !h.has_language() {
            out.push(BlockPart::Language);
        }
        if !h.has_switches() {
            out.push(BlockPart::Switches);
        }
        if !h.has_parameters() {
            out.push(BlockPart::Parameters);
        }
        out
    }

    fn assert_na(b: &Block, parts: &[BlockPart]) {
        for &part in parts {
            assert!(
                matches!(b.part_span(part), PartSpan::NotApplicable),
                "{:?} should be NotApplicable on {:?}",
                part,
                b.kind()
            );
        }
    }

    fn assert_present(b: &Block, part: BlockPart) {
        assert!(
            matches!(b.part_span(part), PartSpan::Present(_)),
            "{part:?} Present on {:?}",
            b.kind()
        );
    }

    fn assert_absent(b: &Block, part: BlockPart) {
        assert!(
            matches!(b.part_span(part), PartSpan::Absent(_)),
            "{part:?} Absent on {:?}",
            b.kind()
        );
    }

    #[test]
    fn nine_kinds_one_layout_path_not_applicable_map() {
        let cases: &[(&str, OrgSyntaxKind, BlockKind)] = &[
            (
                "#+begin_src python\n(+ 1)\n#+end_src\n",
                OrgSyntaxKind::SRC_BLOCK,
                BlockKind::Src,
            ),
            (
                "#+begin_example\nx\n#+end_example\n",
                OrgSyntaxKind::EXAMPLE_BLOCK,
                BlockKind::Example,
            ),
            (
                "#+begin_export html\n<p/>\n#+end_export\n",
                OrgSyntaxKind::EXPORT_BLOCK,
                BlockKind::Export,
            ),
            (
                "#+begin_comment\nx\n#+end_comment\n",
                OrgSyntaxKind::COMMENT_BLOCK,
                BlockKind::Comment,
            ),
            (
                "#+begin_verse\nline\n#+end_verse\n",
                OrgSyntaxKind::VERSE_BLOCK,
                BlockKind::Verse,
            ),
            (
                "#+begin_quote\nq\n#+end_quote\n",
                OrgSyntaxKind::QUOTE_BLOCK,
                BlockKind::Quote,
            ),
            (
                "#+begin_center\nc\n#+end_center\n",
                OrgSyntaxKind::CENTER_BLOCK,
                BlockKind::Center,
            ),
            (
                "#+begin_foo\nb\n#+end_foo\n",
                OrgSyntaxKind::SPECIAL_BLOCK,
                BlockKind::Special,
            ),
            (
                "#+BEGIN: clocktable\n#+END:\n",
                OrgSyntaxKind::DYNAMIC_BLOCK,
                BlockKind::Dynamic,
            ),
        ];
        for &(src, kind, want) in cases {
            let (doc, b) = block(src, kind);
            assert_eq!(b.kind(), want, "{src:?}");
            let begin = source_slice(&doc, b.part_range(BlockPart::BeginMarker).unwrap());
            let end = source_slice(&doc, b.part_range(BlockPart::EndMarker).unwrap());
            assert!(
                begin.to_ascii_lowercase().contains("#+begin"),
                "begin marker is the #+begin run, not the whole line: {begin:?}"
            );
            assert!(
                !begin.contains('\n'),
                "begin marker is not the whole line: {begin:?}"
            );
            assert!(
                end.to_ascii_lowercase().contains("#+end"),
                "end marker is the #+end run: {end:?}"
            );
            assert!(
                !end.contains('\n'),
                "end marker is not the whole line: {end:?}"
            );
            match b.part_span(BlockPart::Body) {
                PartSpan::Present(p) => {
                    let text = source_slice(&doc, p.range());
                    assert!(
                        !text.to_ascii_lowercase().contains("#+begin")
                            && !text.to_ascii_lowercase().contains("#+end"),
                        "body field is the interior: {text:?}"
                    );
                }
                other => panic!("Body always Present, got {other:?} for {src:?}"),
            }
            assert_na(&b, &na_parts(kind));
            match b.kind() {
                BlockKind::Src => {
                    assert_present(&b, BlockPart::Language);
                    assert_absent(&b, BlockPart::Switches);
                    assert_absent(&b, BlockPart::Parameters);
                }
                BlockKind::Export => {
                    assert_present(&b, BlockPart::Language);
                    assert_absent(&b, BlockPart::Parameters);
                }
                BlockKind::Dynamic => {
                    assert_present(&b, BlockPart::Name);
                    assert_absent(&b, BlockPart::Parameters);
                }
                BlockKind::Special => assert_present(&b, BlockPart::Name),
                _ => {}
            }
            crate::ast::check_part_contract(&b);
        }
    }

    #[test]
    fn src_switches_are_not_parameters_awkward_whitespace() {
        let src = "#+begin_src    python   -n   -r    :exports both\n(+ 1)\n#+end_src\n";
        let (doc, b) = block(src, OrgSyntaxKind::SRC_BLOCK);
        assert_eq!(b.language().as_deref(), Some("python"));
        assert_eq!(b.switches().as_deref(), Some("-n   -r"));
        assert_eq!(b.parameters().as_deref(), Some(":exports both"));
        assert!(!b.parameters().unwrap().contains("-n"));
        assert!(!b.parameters().unwrap().contains("-r"));
        assert_eq!(
            source_slice(&doc, b.part_range(BlockPart::BeginMarker).unwrap()),
            "#+begin_src"
        );
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn src_quoted_switch_and_plus_n() {
        let src = "#+begin_src python -n +n -r -i -k -l \"fmt\" :exports both\ncode\n#+end_src\n";
        let (_, b) = block(src, OrgSyntaxKind::SRC_BLOCK);
        assert_eq!(b.language().as_deref(), Some("python"));
        assert_eq!(b.switches().as_deref(), Some("-n +n -r -i -k -l \"fmt\""));
        assert_eq!(b.parameters().as_deref(), Some(":exports both"));
        crate::ast::check_part_contract(&b);

        let src = "#+begin_src python -l \"a b\"\ncode\n#+end_src\n";
        let (_, b) = block(src, OrgSyntaxKind::SRC_BLOCK);
        assert_eq!(b.switches().as_deref(), Some("-l \"a b\""));
        assert!(b.parameters().is_none());
        crate::ast::check_part_contract(&b);

        let src = "#+begin_src python -l   \"fmt\"\ncode\n#+end_src\n";
        let (_, b) = block(src, OrgSyntaxKind::SRC_BLOCK);
        assert_eq!(b.switches().as_deref(), Some("-l   \"fmt\""));
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn empty_src_and_empty_quote_body_present_zero_width() {
        let (_, src) = block("#+begin_src python\n#+end_src\n", OrgSyntaxKind::SRC_BLOCK);
        let body = src.part_content_range(BlockPart::Body).expect("body");
        assert!(body.is_empty());
        assert!(src.content_range().is_none());
        crate::ast::check_part_contract(&src);

        let (_, quote) = block("#+begin_quote\n#+end_quote\n", OrgSyntaxKind::QUOTE_BLOCK);
        let body = quote.part_content_range(BlockPart::Body).expect("body");
        assert!(body.is_empty());
        assert_eq!(quote.content_range(), Some(body));
        crate::ast::check_part_contract(&quote);

        let (_, verse) = block("#+begin_verse\n#+end_verse\n", OrgSyntaxKind::VERSE_BLOCK);
        let body = verse.part_content_range(BlockPart::Body).expect("body");
        assert!(body.is_empty());
        assert_eq!(verse.content_range(), Some(body));
        crate::ast::check_part_contract(&verse);
    }

    #[test]
    fn verse_body_objects() {
        use crate::ast::{HasObjects, Object};
        let (_, verse) = block(
            "#+begin_verse\n*bold*\n#+end_verse\n",
            OrgSyntaxKind::VERSE_BLOCK,
        );
        let BlockBody::Objects(body) = verse.body() else {
            panic!("verse objects door");
        };
        let objs: Vec<_> = body.objects().collect();
        assert_eq!(objs.len(), 1);
        assert!(matches!(objs[0], Object::Markup(_)));
    }

    #[test]
    fn absent_first_line_dests_written() {
        let (doc, b) = block("#+begin_src\nx\n#+end_src\n", OrgSyntaxKind::SRC_BLOCK);
        let ip = b.part_insert_point(BlockPart::Language).unwrap();
        assert_eq!(
            ip.offset(),
            b.part_range(BlockPart::BeginMarker).unwrap().end()
        );
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, "python"),
            "#+begin_src python\nx\n#+end_src\n"
        );

        let (doc, b) = block(
            "#+begin_src python :exports both\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        let ip = b.part_insert_point(BlockPart::Switches).unwrap();
        assert_eq!(
            ip.offset(),
            b.part_range(BlockPart::Language).unwrap().end()
        );
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, "-n"),
            "#+begin_src python -n :exports both\nx\n#+end_src\n"
        );

        let (doc, b) = block(
            "#+begin_src python -n\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        let ip = b.part_insert_point(BlockPart::Parameters).unwrap();
        assert_eq!(
            ip.offset(),
            b.part_range(BlockPart::Switches).unwrap().end()
        );
        assert_eq!(ip.lead(), Padding::Space);
        assert_eq!(
            written(doc.source(), ip, ":exports both"),
            "#+begin_src python -n :exports both\nx\n#+end_src\n"
        );
    }

    #[test]
    fn src_without_language_slots_are_absent() {
        let (_, b) = block("#+begin_src\nx\n#+end_src\n", OrgSyntaxKind::SRC_BLOCK);
        assert!(b.language().is_none());
        assert!(matches!(
            b.part_span(BlockPart::Language),
            PartSpan::Absent(_)
        ));
        assert!(matches!(
            b.part_span(BlockPart::Switches),
            PartSpan::Absent(_)
        ));
        assert!(matches!(
            b.part_span(BlockPart::Parameters),
            PartSpan::Absent(_)
        ));
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn leading_dash_n_is_language_not_switches() {
        let (_, b) = block("#+begin_src -n\nx\n#+end_src\n", OrgSyntaxKind::SRC_BLOCK);
        assert_eq!(b.language().as_deref(), Some("-n"));
        assert_present(&b, BlockPart::Language);
        assert_absent(&b, BlockPart::Switches);
        assert_absent(&b, BlockPart::Parameters);
        crate::ast::check_part_contract(&b);

        let (_, b) = block(
            "#+begin_src -n -r :exports both\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_eq!(b.language().as_deref(), Some("-n"));
        assert_eq!(b.switches().as_deref(), Some("-r"));
        assert_eq!(b.parameters().as_deref(), Some(":exports both"));
        assert_present(&b, BlockPart::Language);
        assert_present(&b, BlockPart::Switches);
        assert_present(&b, BlockPart::Parameters);
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn numbered_and_glued_src_switches() {
        let cases = [
            (
                "#+begin_src rust -n 10 :results output\nx\n#+end_src\n",
                "-n 10",
                ":results output",
            ),
            (
                "#+begin_src rust -n   10 :results output\nx\n#+end_src\n",
                "-n   10",
                ":results output",
            ),
            (
                "#+begin_src rust +n  5 :results output\nx\n#+end_src\n",
                "+n  5",
                ":results output",
            ),
            (
                "#+begin_src rust -n10 :results output\nx\n#+end_src\n",
                "-n10",
                ":results output",
            ),
            (
                "#+begin_src rust +n10 :results output\nx\n#+end_src\n",
                "+n10",
                ":results output",
            ),
        ];
        for (src, switches, parameters) in cases {
            let (_, b) = block(src, OrgSyntaxKind::SRC_BLOCK);
            assert_eq!(b.switches().as_deref(), Some(switches), "{src:?}");
            assert_eq!(b.parameters().as_deref(), Some(parameters), "{src:?}");
            crate::ast::check_part_contract(&b);
        }
    }

    #[test]
    fn switch_rewind_unknown_unquoted_unclosed_and_colon_stop() {
        let (_, b) = block(
            "#+begin_src python -x :exports both\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_absent(&b, BlockPart::Switches);
        assert_eq!(b.parameters().as_deref(), Some("-x :exports both"));
        crate::ast::check_part_contract(&b);

        let (_, ok) = block(
            "#+begin_src python -l \"fmt\"\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_eq!(ok.switches().as_deref(), Some("-l \"fmt\""));
        assert_absent(&ok, BlockPart::Parameters);

        let (_, miss) = block(
            "#+begin_src python -l fmt\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_absent(&miss, BlockPart::Switches);
        assert_eq!(miss.parameters().as_deref(), Some("-l fmt"));
        crate::ast::check_part_contract(&miss);

        let (_, unclosed) = block(
            "#+begin_src python -l \"fmt\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_absent(&unclosed, BlockPart::Switches);
        assert_eq!(unclosed.parameters().as_deref(), Some("-l \"fmt"));
        crate::ast::check_part_contract(&unclosed);

        let (_, b) = block(
            "#+begin_src python -n :exports -r\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_eq!(b.switches().as_deref(), Some("-n"));
        assert_eq!(b.parameters().as_deref(), Some(":exports -r"));
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn export_language_and_parameters_no_switches() {
        let (_, b) = block(
            "#+begin_export   html   :foo bar\n<p/>\n#+end_export\n",
            OrgSyntaxKind::EXPORT_BLOCK,
        );
        assert_eq!(b.language().as_deref(), Some("html"));
        assert_eq!(b.parameters().as_deref(), Some(":foo bar"));
        assert!(matches!(
            b.part_span(BlockPart::Switches),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn special_and_dynamic_name_parameters_body() {
        let (_, sp) = block(
            "#+begin_foo_bar\ninner\n#+end_foo_bar\n",
            OrgSyntaxKind::SPECIAL_BLOCK,
        );
        assert_eq!(sp.name().as_deref(), Some("foo_bar"));
        assert!(sp.language().is_none());
        assert!(sp.parameters().is_none());
        assert!(matches!(
            sp.part_span(BlockPart::Language),
            PartSpan::NotApplicable
        ));
        crate::ast::check_part_contract(&sp);

        let (doc, dynb) = block(
            "#+begin:   clocktable   :scope file\nbody\n#+end:\n",
            OrgSyntaxKind::DYNAMIC_BLOCK,
        );
        assert_eq!(dynb.name().as_deref(), Some("clocktable"));
        assert_eq!(dynb.parameters().as_deref(), Some(":scope file"));
        assert!(dynb.content_range().is_some());
        assert_eq!(
            source_slice(&doc, dynb.part_content_range(BlockPart::Body).unwrap()).trim(),
            "body"
        );
        crate::ast::check_part_contract(&dynb);
    }

    #[test]
    fn opaque_body_with_post_blank_excludes_end_line() {
        let (doc, b) = block(
            "#+begin_src elisp\n(+ 1)\n#+end_src\n\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        let body = b.part_content_range(BlockPart::Body).expect("body");
        assert_eq!(source_slice(&doc, body), "(+ 1)\n");
        match b.body() {
            BlockBody::Verbatim(span) => assert_eq!(span, body),
            other => panic!("expected Verbatim, got {other:?}"),
        }
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn indented_begin_marker_skips_indent() {
        let (doc, b) = block(
            "  #+begin_src python\n  x\n  #+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_eq!(
            source_slice(&doc, b.part_range(BlockPart::BeginMarker).unwrap()),
            "#+begin_src"
        );
        assert_eq!(b.language().as_deref(), Some("python"));
        assert_eq!(
            source_slice(&doc, b.part_range(BlockPart::EndMarker).unwrap()),
            "#+end_src"
        );
        let body = b.part_content_range(BlockPart::Body).expect("body");
        assert_eq!(source_slice(&doc, body), "  x\n");
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn container_post_blank_and_opaque_decoy_end_do_not_steal_closer() {
        let (doc, q) = block(
            "#+begin_quote\ninner\n#+end_quote\n\n",
            OrgSyntaxKind::QUOTE_BLOCK,
        );
        assert_eq!(
            source_slice(&doc, q.part_range(BlockPart::EndMarker).unwrap()),
            "#+end_quote"
        );
        let body = q.part_content_range(BlockPart::Body).expect("body");
        assert_eq!(q.content_range(), Some(body));
        assert_eq!(source_slice(&doc, body).trim(), "inner");
        crate::ast::check_part_contract(&q);

        let (doc, d) = block(
            "#+begin: clocktable\nbody\n#+end:\n\n",
            OrgSyntaxKind::DYNAMIC_BLOCK,
        );
        assert_eq!(
            source_slice(&doc, d.part_range(BlockPart::EndMarker).unwrap()),
            "#+end:"
        );
        assert_eq!(
            source_slice(&doc, d.part_content_range(BlockPart::Body).unwrap()).trim(),
            "body"
        );
        crate::ast::check_part_contract(&d);

        let (doc, b) = block(
            "#+begin_src python\n#+end\n#+end_foo\nx\n#+end_src\n",
            OrgSyntaxKind::SRC_BLOCK,
        );
        assert_eq!(
            source_slice(&doc, b.part_range(BlockPart::EndMarker).unwrap()),
            "#+end_src"
        );
        let body = source_slice(&doc, b.part_content_range(BlockPart::Body).unwrap());
        assert!(body.contains("#+end\n"));
        assert!(body.contains("#+end_foo"));
        crate::ast::check_part_contract(&b);
    }

    #[test]
    fn example_and_special_tails_are_gaps_not_parts() {
        let (_, ex) = block(
            "#+begin_example -n\nx\n#+end_example\n",
            OrgSyntaxKind::EXAMPLE_BLOCK,
        );
        assert_na(&ex, &na_parts(OrgSyntaxKind::EXAMPLE_BLOCK));
        assert!(ex.switches().is_none());
        assert!(ex.parameters().is_none());
        crate::ast::check_part_contract(&ex);

        let (doc, sp) = block(
            "#+begin_foo extra\nb\n#+end_foo\n",
            OrgSyntaxKind::SPECIAL_BLOCK,
        );
        assert_eq!(sp.name().as_deref(), Some("foo"));
        assert!(matches!(
            sp.part_span(BlockPart::Parameters),
            PartSpan::NotApplicable
        ));
        let extra = doc.source().find("extra").expect("extra");
        assert_eq!(
            sp.part_at(crate::TextSize::from(extra as u32)),
            None,
            "special extra first-line text is a gap"
        );
        crate::ast::check_part_contract(&sp);

        let (doc, q) = block(
            "#+begin_quote extra\nq\n#+end_quote\n",
            OrgSyntaxKind::QUOTE_BLOCK,
        );
        let extra = doc.source().find("extra").expect("extra");
        assert_eq!(q.part_at(crate::TextSize::from(extra as u32)), None);
        crate::ast::check_part_contract(&q);
    }

    #[test]
    fn has_parts_over_taxonomy() {
        use crate::parse_with;
        use crate::tests::TAXONOMY;
        let kinds = [
            OrgSyntaxKind::SRC_BLOCK,
            OrgSyntaxKind::EXAMPLE_BLOCK,
            OrgSyntaxKind::EXPORT_BLOCK,
            OrgSyntaxKind::COMMENT_BLOCK,
            OrgSyntaxKind::VERSE_BLOCK,
            OrgSyntaxKind::QUOTE_BLOCK,
            OrgSyntaxKind::CENTER_BLOCK,
            OrgSyntaxKind::SPECIAL_BLOCK,
            OrgSyntaxKind::DYNAMIC_BLOCK,
        ];
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for kind in kinds {
                for node in nodes_of_kind(doc.root(), kind) {
                    let b = Block::cast(node).expect("cast");
                    crate::ast::check_part_contract(&b);
                }
            }
        }
    }
}
