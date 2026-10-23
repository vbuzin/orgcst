//! What surrounds a caret: [`Document::context_at`] returns an
//! [`EditContext`], the stack of [`ContextFrame`]s from the innermost
//! construct out to the document.
//!
//! Frames hold views, so building a context allocates no strings and
//! classifies no links.

use crate::ast::nav;
use crate::ast::support::token_at_caret;
use crate::ast::{
    AffiliatedKeywords, AstNode, BabelCall, BabelCallPart, Block, BlockPart, Citation,
    CitationPart, CitationReference, CitationReferencePart, Clock, ClockPart, Comment, DiarySexp,
    Drawer, DrawerPart, Element, Entity, ExportSnippet, ExportSnippetPart, FixedWidth,
    FootnoteDefinition, FootnotePart, FootnoteReference, FootnoteReferencePart, HasParts, Heading,
    HeadingPart, HorizontalRule, InlineBabelCall, InlineBabelCallPart, InlineSrcBlock,
    InlineSrcPart, InlineTask, Item, ItemPart, Keyword, KeywordPart, LatexEnvironment,
    LatexEnvironmentPart, LatexFragment, LineBreak, Link, LinkPart, Macro, Markup, MarkupPart,
    NodeProperty, Outline, Paragraph, PlainList, Planning, PlanningPart, PropertyDrawer,
    PropertyPart, RadioTarget, ScriptPart, Section, StatisticsCookie, Subscript, Superscript,
    Table, TableCell, TableRow, Target, Task, Timestamp, TimestampPart,
};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{Document, TextRange, TextSize};
use std::fmt;

/// The constructs around a caret, innermost first.
///
/// Never empty: the last frame is [`ContextFrame::Document`]. Like the views
/// it holds, it is not `Send`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EditContext {
    frames: Vec<ContextFrame>,
}

impl fmt::Debug for EditContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.frames).finish()
    }
}

impl EditContext {
    fn from_frames(mut frames: Vec<ContextFrame>) -> Self {
        if frames.is_empty() {
            frames.push(ContextFrame::Document);
        }
        Self { frames }
    }

    /// The innermost frame; [`ContextFrame::Document`] in an empty file.
    ///
    /// # Panics
    ///
    /// Panics if the frame list is empty. Construction always keeps a
    /// [`ContextFrame::Document`] frame, so a context this crate builds does not.
    #[must_use]
    pub fn innermost(&self) -> &ContextFrame {
        self.frames.first().expect("EditContext is never empty")
    }

    /// All frames, innermost first, ending with [`ContextFrame::Document`].
    pub fn frames(&self) -> impl Iterator<Item = &ContextFrame> + '_ {
        self.frames.iter()
    }

    /// First frame whose view kind **exactly** equals `kind`.
    ///
    /// `KEYWORD` does not match `SETTING_KEYWORD`; to match either, use
    /// `nearest::<Keyword>()`.
    #[must_use]
    pub fn nearest_kind(&self, kind: OrgSyntaxKind) -> Option<&ContextFrame> {
        self.frames.iter().find(|f| f.kind() == Some(kind))
    }

    /// The innermost frame whose node casts to `T`.
    #[must_use]
    pub fn nearest<T: AstNode>(&self) -> Option<T> {
        self.frames
            .iter()
            .find_map(|f| f.syntax().cloned().and_then(T::cast))
    }

    /// Nearest outline frame.
    #[must_use]
    pub fn outline(&self) -> Option<Outline> {
        self.nearest()
    }

    /// The innermost body element or, inside an affiliated-keyword run, the
    /// element the run attaches to.
    #[must_use]
    pub fn element(&self) -> Option<Element> {
        self.nearest::<Element>().or_else(|| {
            let inner = self.innermost().syntax()?;
            nav::enclosing_element(inner)
        })
    }

    /// The innermost outline or inline task, as `Document::node_at::<Task>`
    /// finds it.
    #[must_use]
    pub fn entry(&self) -> Option<Task> {
        self.nearest()
    }
}

/// Defines [`ContextFrame`] with its name and syntax accessors from one list.
///
/// [`frame_from`] maps node kinds to frames; `part_span_at` covers the frames
/// that carry a part.
macro_rules! context_frames {
    (
        $(
            $(#[$meta:meta])*
            $arm:ident ( $view:ty $(, $part:ty)? )
        ),+ $(,)?
    ) => {
        /// One construct around a caret: its view and, for a construct with
        /// parts, the part under the caret (`None` in a gap between parts).
        ///
        /// [`Self::Document`] ends every context and carries no view. A
        /// `Heading` frame appears only on the headline line; an `InlineTask`
        /// frame appears anywhere inside the task. At the exclusive end of
        /// markup the frame is still `Markup`, with no part.
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub enum ContextFrame {
            /// The whole document; always the last frame.
            Document,
            $(
                $(#[$meta])*
                $arm($view $(, Option<$part>)?),
            )+
        }

        impl ContextFrame {
            fn arm_name(&self) -> &'static str {
                match self {
                    Self::Document => "Document",
                    $(Self::$arm(..) => stringify!($arm),)+
                }
            }

            /// Syntax kind of the frame's node; `None` for [`Self::Document`].
            #[must_use]
            pub fn kind(&self) -> Option<OrgSyntaxKind> {
                self.syntax().map(|n| n.kind())
            }

            /// The frame's node; `None` for [`Self::Document`].
            #[must_use]
            pub fn syntax(&self) -> Option<&SyntaxNode> {
                Some(match self {
                    Self::Document => return None,
                    $(Self::$arm(n, ..) => n.syntax(),)+
                })
            }
        }
    };
}

context_frames! {
    /// An outline.
    Outline(Outline),
    /// A section.
    Section(Section),
    /// A heading line, with its part.
    Heading(Heading, HeadingPart),
    /// An inline task, with the headline part under the caret, if any.
    InlineTask(InlineTask, HeadingPart),
    /// A planning line, with its part.
    Planning(Planning, PlanningPart),
    /// A property drawer.
    PropertyDrawer(PropertyDrawer),
    /// A property line, with its part.
    Property(NodeProperty, PropertyPart),
    /// A paragraph.
    Paragraph(Paragraph),
    /// A plain list.
    List(PlainList),
    /// A list item, with its part.
    ListItem(Item, ItemPart),
    /// A table.
    Table(Table),
    /// A table row.
    TableRow(TableRow),
    /// A table cell.
    TableCell(TableCell),
    /// A drawer, with its part.
    Drawer(Drawer, DrawerPart),
    /// A block, with its part.
    Block(Block, BlockPart),
    /// A keyword line, with its part.
    Keyword(Keyword, KeywordPart),
    /// An affiliated-keyword run.
    AffiliatedKeywords(AffiliatedKeywords),
    /// A `#+CALL:` line, with its part.
    BabelCall(BabelCall, BabelCallPart),
    /// A comment.
    Comment(Comment),
    /// A fixed-width area.
    FixedWidth(FixedWidth),
    /// A horizontal rule.
    HorizontalRule(HorizontalRule),
    /// A clock line, with its part.
    Clock(Clock, ClockPart),
    /// A footnote definition, with its part.
    FootnoteDefinition(FootnoteDefinition, FootnotePart),
    /// A LaTeX environment, with its part.
    LatexEnvironment(LatexEnvironment, LatexEnvironmentPart),
    /// A diary sexp.
    DiarySexp(DiarySexp),
    /// A link, with its part.
    Link(Link, LinkPart),
    /// A timestamp, with its part.
    Timestamp(Timestamp, TimestampPart),
    /// A markup object, with its part.
    Markup(Markup, MarkupPart),
    /// A target.
    Target(Target),
    /// A radio target.
    RadioTarget(RadioTarget),
    /// A statistics cookie.
    StatisticsCookie(StatisticsCookie),
    /// An entity.
    Entity(Entity),
    /// A LaTeX fragment.
    LatexFragment(LatexFragment),
    /// A macro.
    Macro(Macro),
    /// An export snippet, with its part.
    ExportSnippet(ExportSnippet, ExportSnippetPart),
    /// A subscript, with its part.
    Subscript(Subscript, ScriptPart),
    /// A superscript, with its part.
    Superscript(Superscript, ScriptPart),
    /// A line break.
    LineBreak(LineBreak),
    /// A citation, with its part.
    Citation(Citation, CitationPart),
    /// A citation reference, with its part.
    CitationReference(CitationReference, CitationReferencePart),
    /// A footnote reference, with its part.
    FootnoteReference(FootnoteReference, FootnoteReferencePart),
    /// An inline source block, with its part.
    InlineSrcBlock(InlineSrcBlock, InlineSrcPart),
    /// An inline babel call, with its part.
    InlineBabelCall(InlineBabelCall, InlineBabelCallPart),
}

impl fmt::Debug for ContextFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.syntax() {
            None => write!(f, "Document"),
            Some(n) => write!(
                f,
                "{}({:?}, {:?})",
                self.arm_name(),
                n.kind(),
                n.text_range()
            ),
        }
    }
}

impl ContextFrame {
    pub(crate) fn part_span_at(&self, offset: TextSize) -> Option<TextRange> {
        match self {
            Self::Heading(n, _) => part_range(n, offset),
            Self::InlineTask(n, _) => part_range(n, offset),
            Self::Planning(n, _) => part_range(n, offset),
            Self::Property(n, _) => part_range(n, offset),
            Self::ListItem(n, _) => part_range(n, offset),
            Self::Drawer(n, _) => part_range(n, offset),
            Self::Block(n, _) => part_range(n, offset),
            Self::Keyword(n, _) => part_range(n, offset),
            Self::BabelCall(n, _) => part_range(n, offset),
            Self::Clock(n, _) => part_range(n, offset),
            Self::FootnoteDefinition(n, _) => part_range(n, offset),
            Self::LatexEnvironment(n, _) => part_range(n, offset),
            Self::Link(n, _) => part_range(n, offset),
            Self::Timestamp(n, _) => part_range(n, offset),
            Self::Markup(n, _) => part_range(n, offset),
            Self::ExportSnippet(n, _) => part_range(n, offset),
            Self::Subscript(n, _) => part_range(n, offset),
            Self::Superscript(n, _) => part_range(n, offset),
            Self::Citation(n, _) => part_range(n, offset),
            Self::CitationReference(n, _) => part_range(n, offset),
            Self::FootnoteReference(n, _) => part_range(n, offset),
            Self::InlineSrcBlock(n, _) => part_range(n, offset),
            Self::InlineBabelCall(n, _) => part_range(n, offset),
            _ => None,
        }
    }
}

fn part_range<T: HasParts>(node: &T, offset: TextSize) -> Option<TextRange> {
    node.part_range(node.part_at(offset)?)
}

impl Document {
    /// The constructs around a caret at `offset`, innermost first.
    ///
    /// Where two tokens meet, the caret belongs to the left one. Inside an
    /// affiliated-keyword run the context also includes the element the run
    /// attaches to. An offset past the end is clamped. For a selection, use
    /// [`Document::covering`] instead.
    #[must_use]
    pub fn context_at(&self, offset: TextSize) -> EditContext {
        if self.source().is_empty() {
            return EditContext::from_frames(vec![ContextFrame::Document]);
        }
        let offset = offset.min(TextSize::of(self.source()));
        let Some(tok) = token_at_caret(self.root(), offset) else {
            return EditContext::from_frames(vec![ContextFrame::Document]);
        };
        let Some(start) = tok.parent() else {
            return EditContext::from_frames(vec![ContextFrame::Document]);
        };
        let mut frames = Vec::new();
        for node in std::iter::once(start.clone()).chain(nav::ancestors(&start)) {
            let aff = node.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS;
            let aff_node = aff.then(|| node.clone());
            let Some(frame) = frame_from(node, offset) else {
                continue;
            };
            frames.push(frame);
            if let Some(aff_node) = aff_node
                && let Some(el) = nav::enclosing_element(&aff_node)
                && !frames
                    .iter()
                    .any(|f| f.syntax().is_some_and(|n| n == el.syntax()))
                && let Some(synth) = frame_from(el.syntax().clone(), offset)
            {
                frames.push(synth);
            }
        }
        frames.push(ContextFrame::Document);
        EditContext::from_frames(frames)
    }
}

/// Whether `node` gets no frame of its own in an [`EditContext`].
pub(crate) fn is_context_skip(node: &SyntaxNode) -> bool {
    frame_from(node.clone(), node.text_range().start()).is_none()
}

fn frame_parts<T: HasParts>(
    node: SyntaxNode,
    offset: TextSize,
    arm: impl FnOnce(T, Option<T::Part>) -> ContextFrame,
) -> Option<ContextFrame> {
    let v = T::cast(node)?;
    let part = v.part_at(offset);
    Some(arm(v, part))
}

fn frame_from(node: SyntaxNode, offset: TextSize) -> Option<ContextFrame> {
    match node.kind() {
        OrgSyntaxKind::OUTLINE => Outline::cast(node).map(ContextFrame::Outline),
        OrgSyntaxKind::SECTION => Section::cast(node).map(ContextFrame::Section),
        OrgSyntaxKind::HEADING => frame_parts(node, offset, ContextFrame::Heading),
        OrgSyntaxKind::INLINE_TASK => frame_parts(node, offset, ContextFrame::InlineTask),
        OrgSyntaxKind::PLANNING => frame_parts(node, offset, ContextFrame::Planning),
        OrgSyntaxKind::PROPERTY_DRAWER => {
            PropertyDrawer::cast(node).map(ContextFrame::PropertyDrawer)
        }
        OrgSyntaxKind::NODE_PROPERTY => frame_parts(node, offset, ContextFrame::Property),
        OrgSyntaxKind::PARAGRAPH => Paragraph::cast(node).map(ContextFrame::Paragraph),
        OrgSyntaxKind::PLAIN_LIST => PlainList::cast(node).map(ContextFrame::List),
        OrgSyntaxKind::ITEM => frame_parts(node, offset, ContextFrame::ListItem),
        OrgSyntaxKind::TABLE => Table::cast(node).map(ContextFrame::Table),
        OrgSyntaxKind::TABLE_ROW | OrgSyntaxKind::TABLE_RULE_ROW => {
            TableRow::cast(node).map(ContextFrame::TableRow)
        }
        OrgSyntaxKind::TABLE_CELL => TableCell::cast(node).map(ContextFrame::TableCell),
        OrgSyntaxKind::DRAWER => frame_parts(node, offset, ContextFrame::Drawer),
        k if Block::can_cast(k) => frame_parts(node, offset, ContextFrame::Block),
        OrgSyntaxKind::KEYWORD | OrgSyntaxKind::SETTING_KEYWORD => {
            frame_parts(node, offset, ContextFrame::Keyword)
        }
        OrgSyntaxKind::AFFILIATED_KEYWORDS => {
            AffiliatedKeywords::cast(node).map(ContextFrame::AffiliatedKeywords)
        }
        OrgSyntaxKind::BABEL_CALL => frame_parts(node, offset, ContextFrame::BabelCall),
        OrgSyntaxKind::COMMENT => Comment::cast(node).map(ContextFrame::Comment),
        OrgSyntaxKind::FIXED_WIDTH => FixedWidth::cast(node).map(ContextFrame::FixedWidth),
        OrgSyntaxKind::HORIZONTAL_RULE => {
            HorizontalRule::cast(node).map(ContextFrame::HorizontalRule)
        }
        OrgSyntaxKind::CLOCK => frame_parts(node, offset, ContextFrame::Clock),
        OrgSyntaxKind::FOOTNOTE_DEFINITION => {
            frame_parts(node, offset, ContextFrame::FootnoteDefinition)
        }
        OrgSyntaxKind::LATEX_ENVIRONMENT => {
            frame_parts(node, offset, ContextFrame::LatexEnvironment)
        }
        OrgSyntaxKind::DIARY_SEXP => {
            if node
                .parent()
                .is_some_and(|p| p.kind() == OrgSyntaxKind::TIMESTAMP)
            {
                None
            } else {
                DiarySexp::cast(node).map(ContextFrame::DiarySexp)
            }
        }
        OrgSyntaxKind::LINK => frame_parts(node, offset, ContextFrame::Link),
        OrgSyntaxKind::TIMESTAMP => frame_parts(node, offset, ContextFrame::Timestamp),
        k if Markup::can_cast(k) => frame_parts(node, offset, ContextFrame::Markup),
        OrgSyntaxKind::TARGET => Target::cast(node).map(ContextFrame::Target),
        OrgSyntaxKind::RADIO_TARGET => RadioTarget::cast(node).map(ContextFrame::RadioTarget),
        OrgSyntaxKind::STATISTICS_COOKIE => {
            StatisticsCookie::cast(node).map(ContextFrame::StatisticsCookie)
        }
        OrgSyntaxKind::ENTITY => Entity::cast(node).map(ContextFrame::Entity),
        OrgSyntaxKind::LATEX_FRAGMENT => LatexFragment::cast(node).map(ContextFrame::LatexFragment),
        OrgSyntaxKind::MACRO => Macro::cast(node).map(ContextFrame::Macro),
        OrgSyntaxKind::EXPORT_SNIPPET => frame_parts(node, offset, ContextFrame::ExportSnippet),
        OrgSyntaxKind::SUBSCRIPT => frame_parts(node, offset, ContextFrame::Subscript),
        OrgSyntaxKind::SUPERSCRIPT => frame_parts(node, offset, ContextFrame::Superscript),
        OrgSyntaxKind::LINE_BREAK => LineBreak::cast(node).map(ContextFrame::LineBreak),
        OrgSyntaxKind::CITATION => frame_parts(node, offset, ContextFrame::Citation),
        OrgSyntaxKind::CITATION_REFERENCE => {
            frame_parts(node, offset, ContextFrame::CitationReference)
        }
        OrgSyntaxKind::FOOTNOTE_REFERENCE => {
            frame_parts(node, offset, ContextFrame::FootnoteReference)
        }
        OrgSyntaxKind::INLINE_SRC_BLOCK => frame_parts(node, offset, ContextFrame::InlineSrcBlock),
        OrgSyntaxKind::INLINE_BABEL_CALL => {
            frame_parts(node, offset, ContextFrame::InlineBabelCall)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ast::{
        Block, BlockKind, BlockPart, Element, HasParts, Headline, KeywordRole, MarkupPart,
    };
    use crate::parse;
    use crate::tests::nodes_of_kind;

    fn at(src: &str, pat: &str) -> TextSize {
        TextSize::from(src.find(pat).expect(pat) as u32)
    }

    fn interior(range: TextRange) -> TextSize {
        if range.len() > 1.into() {
            range.start() + TextSize::from(1)
        } else {
            range.start()
        }
    }

    fn arms(ctx: &EditContext) -> Vec<&'static str> {
        ctx.frames().map(super::ContextFrame::arm_name).collect()
    }

    #[test]
    fn empty_document_is_document_sentinel() {
        let doc = parse("");
        for o in [TextSize::from(0), TextSize::of(doc.source())] {
            let ctx = doc.context_at(o);
            assert_eq!(arms(&ctx), ["Document"]);
            assert!(matches!(ctx.innermost(), ContextFrame::Document));
            assert!(ctx.entry().is_none());
            assert!(ctx.element().is_none());
        }
    }

    #[test]
    fn closed_markup_names_frame_and_part_at_is_none() {
        let src = "*bold* next";
        let doc = parse(src);
        let o = at(src, " next");
        let ctx = doc.context_at(o);
        assert_eq!(arms(&ctx)[0], "Markup");
        let ContextFrame::Markup(m, part) = ctx.innermost() else {
            panic!("expected Markup, got {:?}", ctx.innermost());
        };
        assert!(part.is_none());
        assert!(m.part_at(o).is_none());
        assert_eq!(
            doc.context_at(TextSize::of(src)).innermost().arm_name(),
            "Paragraph"
        );
    }

    #[test]
    fn headline_gap_is_heading_with_part_none() {
        let src = "* TODO Title\n";
        let doc = parse(src);
        let heading = doc.root_outlines().next().unwrap().heading();
        let todo_end = heading.todo_keyword().unwrap().range().end();
        let ctx = doc.context_at(todo_end);
        assert_eq!(ctx.innermost().arm_name(), "Heading");
        let ContextFrame::Heading(h, part) = ctx.innermost() else {
            panic!("heading");
        };
        assert!(part.is_none());
        assert!(h.part_at(todo_end).is_none());
    }

    #[test]
    fn heading_first_file_starts_on_stars() {
        let src = "* Title\n";
        let doc = parse(src);
        let ctx = doc.context_at(TextSize::from(0));
        let ContextFrame::Heading(_, Some(HeadingPart::Stars)) = ctx.innermost() else {
            panic!("stars, got {:?}", ctx.innermost());
        };
    }

    #[test]
    fn heading_after_paragraph_left_bias_is_not_heading() {
        let src = "text\n* H\n";
        let doc = parse(src);
        let star = at(src, "* H");
        let ctx = doc.context_at(star);
        assert_eq!(ctx.innermost().arm_name(), "Paragraph");
        assert!(!arms(&ctx).contains(&"Heading"));
    }

    #[test]
    fn body_caret_has_no_heading_frame() {
        let src = "* H\nbody\n";
        let doc = parse(src);
        let ctx = doc.context_at(at(src, "ody"));
        assert!(!arms(&ctx).contains(&"Heading"));
        assert!(matches!(ctx.entry(), Some(Task::Outline(_))));
        assert_eq!(ctx.innermost().arm_name(), "Paragraph");
        assert!(arms(&ctx).contains(&"Section"));
        assert!(arms(&ctx).contains(&"Outline"));
    }

    #[test]
    fn adjacent_objects_left_wins() {
        let src = "*foo*[[http://example.com]]\n";
        let doc = parse(src);
        let link = nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("link");
        let ctx = doc.context_at(link.text_range().start());
        assert_eq!(
            ctx.innermost().arm_name(),
            "Markup",
            "left object at the seam, got {:?}",
            ctx.innermost()
        );
    }

    #[test]
    fn caption_synthesises_target_element() {
        let src = "#+CAPTION: c\npara\n";
        let doc = parse(src);
        let ctx = doc.context_at(at(src, "CAPTION"));
        assert!(arms(&ctx).contains(&"Keyword"));
        assert!(arms(&ctx).contains(&"AffiliatedKeywords"));
        assert!(arms(&ctx).contains(&"Paragraph"));
        assert!(matches!(ctx.element(), Some(Element::Paragraph(_))));
    }

    #[test]
    fn dispatch_heading_parts() {
        let src = "* TODO [#A] COMMENT Title :tag:\n";
        let doc = parse(src);
        let h = doc.root_outlines().next().unwrap().heading();
        for part in [
            HeadingPart::Stars,
            HeadingPart::Todo,
            HeadingPart::Priority,
            HeadingPart::CommentMarker,
            HeadingPart::Title,
            HeadingPart::Tags,
        ] {
            let r = h.part_range(part).expect("present");
            let ctx = doc.context_at(interior(r));
            let ContextFrame::Heading(_, Some(got)) = ctx.innermost() else {
                panic!("{part:?} -> {:?}", ctx.innermost());
            };
            assert_eq!(*got, part);
        }
    }

    #[test]
    fn dispatch_item_parts() {
        let src = "- [@2] [ ] tag :: body\n";
        let doc = parse(src);
        let item = Item::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        for part in [
            ItemPart::Bullet,
            ItemPart::Counter,
            ItemPart::Checkbox,
            ItemPart::Tag,
            ItemPart::Body,
        ] {
            let Some(r) = item.part_range(part) else {
                panic!("{part:?} absent in {src:?}");
            };
            let ctx = doc.context_at(interior(r));
            let got = ctx.frames().find_map(|f| match f {
                ContextFrame::ListItem(_, p) => *p,
                _ => None,
            });
            assert_eq!(got, Some(part), "{part:?} -> {:?}", ctx.innermost());
        }
    }

    #[test]
    fn dispatch_table_cell_rule_tblfm() {
        let src = "| a |\n|---|\n#+TBLFM: $1=1\n";
        let doc = parse(src);
        let cell = at(src, "a");
        assert_eq!(doc.context_at(cell).innermost().arm_name(), "TableCell");
        let rule = at(src, "---");
        assert_eq!(doc.context_at(rule).innermost().arm_name(), "TableRow");
        let tblfm = at(src, "TBLFM");
        let ctx = doc.context_at(tblfm);
        assert_eq!(ctx.innermost().arm_name(), "Keyword");
        assert!(arms(&ctx).contains(&"Table"));
    }

    #[test]
    fn dispatch_block_vs_line_comment() {
        let src = concat!(
            "#+begin_src python -n :results output\nbody\n#+end_src\n",
            "#+begin_example\nex\n#+end_example\n",
            "#+begin_export html\nex\n#+end_export\n",
            "#+begin_verse\nv\n#+end_verse\n",
            "#+begin_quote\nq\n#+end_quote\n",
            "#+begin: dyn :foo bar\n#+end:\n",
            "#+begin_comment\nx\n#+end_comment\n",
            "# unique-line-comment\n",
        );
        let doc = parse(src);
        let src_block = doc
            .map::<Block>()
            .find(|b| b.kind() == BlockKind::Src)
            .unwrap();
        for part in [BlockPart::Language, BlockPart::Switches, BlockPart::Body] {
            let r = src_block.part_range(part).expect("src part");
            let ctx = doc.context_at(interior(r));
            let ContextFrame::Block(b, Some(got)) = ctx.innermost() else {
                panic!("{part:?} -> {:?}", ctx.innermost());
            };
            assert_eq!(b.kind(), BlockKind::Src);
            assert_eq!(*got, part);
        }
        let kinds = [
            (BlockKind::Example, OrgSyntaxKind::EXAMPLE_BLOCK),
            (BlockKind::Export, OrgSyntaxKind::EXPORT_BLOCK),
            (BlockKind::Verse, OrgSyntaxKind::VERSE_BLOCK),
            (BlockKind::Quote, OrgSyntaxKind::QUOTE_BLOCK),
            (BlockKind::Dynamic, OrgSyntaxKind::DYNAMIC_BLOCK),
            (BlockKind::Comment, OrgSyntaxKind::COMMENT_BLOCK),
        ];
        for (kind, syn) in kinds {
            let node = nodes_of_kind(doc.root(), syn).into_iter().next().unwrap();
            let ctx = doc.context_at(interior(node.text_range()));
            let ContextFrame::Block(b, _) = ctx.innermost() else {
                panic!("{kind:?} -> {:?}", ctx.innermost());
            };
            assert_eq!(b.kind(), kind);
        }
        let line = doc.context_at(at(src, "unique-line-comment"));
        assert_eq!(line.innermost().arm_name(), "Comment");
    }

    #[test]
    fn dispatch_timestamp_planning_clock() {
        let src = "* H\nDEADLINE: <2020-01-01>\nCLOCK: [2020-01-02]\nCLOCK: => 0:10\n";
        let doc = parse(src);
        let deadline = doc.context_at(at(src, "2020-01-01"));
        assert_eq!(deadline.innermost().arm_name(), "Timestamp");
        assert!(arms(&deadline).contains(&"Planning"));
        let clock = nodes_of_kind(doc.root(), OrgSyntaxKind::CLOCK);
        assert!(clock.len() >= 2);
        let ts = nodes_of_kind(&clock[0], OrgSyntaxKind::TIMESTAMP)
            .into_iter()
            .next()
            .expect("clock stamp");
        let clock_ts = doc.context_at(interior(ts.text_range()));
        assert_eq!(clock_ts.innermost().arm_name(), "Timestamp");
        assert!(arms(&clock_ts).contains(&"Clock"), "{:?}", arms(&clock_ts));
        let dur = doc.context_at(interior(clock[1].text_range()));
        assert_eq!(dur.innermost().arm_name(), "Clock");
        assert!(!arms(&dur).contains(&"Timestamp"));
    }

    #[test]
    fn dispatch_property_key_value_fence() {
        let src = "* H\n:PROPERTIES:\n:ID: x\n:END:\n";
        let doc = parse(src);
        let key = doc.context_at(at(src, "ID"));
        let ContextFrame::Property(_, Some(PropertyPart::Key)) = key.innermost() else {
            panic!("{:?}", key.innermost());
        };
        let val = doc.context_at(at(src, "x\n:END:"));
        assert!(
            matches!(
                val.innermost(),
                ContextFrame::Property(_, Some(PropertyPart::Value))
            ),
            "{:?}",
            val.innermost()
        );
        let fence = doc.context_at(at(src, "PROPERTIES"));
        assert_eq!(fence.innermost().arm_name(), "PropertyDrawer");
        assert!(!arms(&fence).contains(&"Property"));
    }

    #[test]
    fn dispatch_footnote_ref_vs_def() {
        let src = "[fn:1] body\n\nSee [fn:1] and [fn:lab:inline].\n";
        let doc = parse(src);
        let def = doc.context_at(at(src, "fn:1] body"));
        assert_eq!(def.innermost().arm_name(), "FootnoteDefinition");
        let named = doc.context_at(at(src, "fn:1] and"));
        let ContextFrame::FootnoteReference(r, _) = named.innermost() else {
            panic!("{:?}", named.innermost());
        };
        assert_eq!(r.shape(), crate::ast::FootnoteShape::Named);
        let inline = doc.context_at(at(src, "inline"));
        let ContextFrame::FootnoteReference(r, part) = inline.innermost() else {
            panic!("{:?}", inline.innermost());
        };
        assert_eq!(r.shape(), crate::ast::FootnoteShape::Inline);
        assert_eq!(*part, Some(FootnoteReferencePart::Definition));
    }

    #[test]
    fn dispatch_targets_radio_link() {
        let src = "<<t>> <<<r>>> [[http://ex.com]]\n";
        let doc = parse(src);
        assert_eq!(
            doc.context_at(at(src, "<<t")).innermost().arm_name(),
            "Target"
        );
        let radio = nodes_of_kind(doc.root(), OrgSyntaxKind::RADIO_TARGET)
            .into_iter()
            .next()
            .expect("radio");
        assert_eq!(
            doc.context_at(interior(radio.text_range()))
                .innermost()
                .arm_name(),
            "RadioTarget"
        );
        assert_eq!(
            doc.context_at(at(src, "http://ex.com"))
                .innermost()
                .arm_name(),
            "Link"
        );
    }

    #[test]
    fn dispatch_cookie() {
        let src = "* [1/2] H\n";
        let doc = parse(src);
        assert_eq!(
            doc.context_at(at(src, "1/2")).innermost().arm_name(),
            "StatisticsCookie"
        );
    }

    #[test]
    fn dispatch_keyword_roles_and_nearest_kind() {
        let src = "#+TITLE: t\n#+TODO: A | B\n#+CAPTION: c\npara\n#+CALL: f()\n";
        let doc = parse(src);
        let title = doc.context_at(at(src, "TITLE"));
        let ContextFrame::Keyword(k, _) = title.innermost() else {
            panic!("title");
        };
        assert_eq!(k.role(), KeywordRole::Plain);
        assert!(title.nearest_kind(OrgSyntaxKind::KEYWORD).is_some());
        assert!(title.nearest_kind(OrgSyntaxKind::SETTING_KEYWORD).is_none());
        assert!(title.nearest::<Keyword>().is_some());

        let setting = doc.context_at(at(src, "TODO"));
        let ContextFrame::Keyword(k, _) = setting.innermost() else {
            panic!("todo");
        };
        assert_eq!(k.role(), KeywordRole::Setting);
        assert!(
            setting
                .nearest_kind(OrgSyntaxKind::SETTING_KEYWORD)
                .is_some()
        );
        assert!(setting.nearest_kind(OrgSyntaxKind::KEYWORD).is_none());
        assert!(setting.nearest::<Keyword>().is_some());

        let cap = doc.context_at(at(src, "CAPTION"));
        assert!(arms(&cap).contains(&"AffiliatedKeywords"));
        let call = doc.context_at(at(src, "CALL"));
        assert_eq!(call.innermost().arm_name(), "BabelCall");
    }

    #[test]
    fn dispatch_markup_close_gap() {
        let src = "*bold* next";
        let doc = parse(src);
        let o = at(src, " next");
        let ctx = doc.context_at(o);
        let ContextFrame::Markup(m, None) = ctx.innermost() else {
            panic!("{:?}", ctx.innermost());
        };
        assert!(m.part_at(o).is_none());
        assert_ne!(m.part_at(o), Some(MarkupPart::CloseMarker));
    }

    #[test]
    fn nested_link_in_heading_title() {
        let src = "* see [[http://ex.com][x]]\n";
        let doc = parse(src);
        let ctx = doc.context_at(at(src, "http"));
        assert_eq!(ctx.innermost().arm_name(), "Link");
        assert!(arms(&ctx).contains(&"Heading"));
        assert!(arms(&ctx).contains(&"Outline"));
    }

    #[test]
    fn nested_bold_in_link_description() {
        let src = "[[http://ex.com][*x*]]\n";
        let doc = parse(src);
        let bold = nodes_of_kind(doc.root(), OrgSyntaxKind::BOLD)
            .into_iter()
            .next()
            .expect("bold");
        let ctx = doc.context_at(interior(bold.text_range()));
        assert_eq!(ctx.innermost().arm_name(), "Markup");
        assert!(arms(&ctx).contains(&"Link"));
    }

    #[test]
    fn nested_cell_in_table_in_section() {
        let src = "* H\n| a |\n";
        let doc = parse(src);
        let ctx = doc.context_at(at(src, "a"));
        assert_eq!(ctx.innermost().arm_name(), "TableCell");
        assert!(arms(&ctx).contains(&"Table"));
        assert!(arms(&ctx).contains(&"Section"));
        assert!(arms(&ctx).contains(&"Outline"));
    }

    #[test]
    fn lead_in_caret_is_document_not_heading() {
        let src = "\n\n* H\n";
        let doc = parse(src);
        assert_eq!(arms(&doc.context_at(TextSize::from(0))), ["Document"]);
        let star = at(src, "* H");
        assert_ne!(doc.context_at(star).innermost().arm_name(), "Heading");
        assert_eq!(doc.context_at(star).innermost().arm_name(), "Document");
    }

    #[test]
    fn inline_task_body_caret_keeps_inline_task_frame() {
        let src = "*************** T\nbody\n*************** END\n";
        let doc = parse(src);
        let ctx = doc.context_at(at(src, "ody"));
        assert_eq!(ctx.innermost().arm_name(), "Paragraph");
        assert!(arms(&ctx).contains(&"InlineTask"));
        let ContextFrame::InlineTask(_, part) =
            ctx.frames().find(|f| f.arm_name() == "InlineTask").unwrap()
        else {
            panic!("inline task frame");
        };
        assert!(part.is_none());
        assert!(matches!(ctx.entry(), Some(Task::InlineTask(_))));
    }

    #[test]
    fn diary_dual_use_skip_vs_element() {
        let stamp = parse("<%%(diary-float t 4 2)>\n");
        let inner = nodes_of_kind(stamp.root(), OrgSyntaxKind::DIARY_SEXP)
            .into_iter()
            .next()
            .unwrap();
        assert!(is_context_skip(&inner));
        let ctx = stamp.context_at(at("<%%(diary-float t 4 2)>\n", "diary-float"));
        assert_eq!(ctx.innermost().arm_name(), "Timestamp");
        assert!(!arms(&ctx).contains(&"DiarySexp"));

        let line = parse("%%(diary-float t 4 2)\n");
        let el = nodes_of_kind(line.root(), OrgSyntaxKind::DIARY_SEXP)
            .into_iter()
            .next()
            .unwrap();
        assert!(!is_context_skip(&el));
        assert!(is_context_skip(line.root()));
        let ctx = line.context_at(at("%%(diary-float t 4 2)\n", "diary-float"));
        assert!(matches!(ctx.innermost(), ContextFrame::DiarySexp(_)));
        assert!(matches!(ctx.element(), Some(Element::DiarySexp(_))));
    }
}
