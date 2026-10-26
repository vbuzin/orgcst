//! Headlines: [`Heading`], the [`Headline`] trait it shares with inline
//! tasks, and the views of the TODO keyword, priority, title and tags.
//!
//! Tree shape of a headline:
//!
//! ```text
//!   HEADING / INLINE_TASK
//!     [TODO_KEYWORD] [PRIORITY] [HEADING_COMMENT]
//!     HEADING_TITLE   (always present; may be empty)
//!     [TAGS]
//! ```
//!
//! The stars and the individual tags are tokens, not nodes.

use super::inline_task::InlineTask;
use crate::ast::layout::{
    content_of, first_line, first_line_end, first_token_of_kind, first_word_range,
    present_with_removal_after, priority_value_range, span_of, tag_runs, whitespace_ends_at,
};
use crate::ast::support::child_view;
use crate::ast::{
    AstNode, Element, HasObjects, HasParts, InsertPoint, Padding, PartSpan, PresentPart,
    SplitPoint, split_span,
};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{PriorityAtom, TextRange, TextSize};

crate::ast_node!(
    /// A headline: the stars, then the optional TODO keyword, priority and
    /// `COMMENT` marker, the title and the tags.
    Heading,
    OrgSyntaxKind::HEADING
);
crate::ast_node!(
    /// The TODO keyword of a headline, such as `TODO` or `DONE`.
    TodoKeyword,
    OrgSyntaxKind::TODO_KEYWORD
);
crate::ast_node!(
    /// The priority cookie of a headline, such as `[#A]`.
    Priority,
    OrgSyntaxKind::PRIORITY
);
crate::ast_node!(
    /// The title of a headline. Always present, possibly empty; it holds
    /// objects.
    HeadingTitle,
    OrgSyntaxKind::HEADING_TITLE
);
crate::ast_node!(
    /// The tag group at the end of a headline, as in `:work:urgent:`.
    Tags,
    OrgSyntaxKind::TAGS
);

impl HeadingTitle {
    /// Range of the first whitespace-delimited word, if any.
    ///
    /// Leading horizontal whitespace is skipped. Empty and whitespace-only
    /// titles yield `None`.
    #[must_use]
    pub fn first_word_range(&self) -> Option<TextRange> {
        first_word_range(self.syntax())
    }

    /// Cut the title at `offset`, moving the rest into a new sibling: an
    /// outline after this heading's subtree, or an inline task after this one.
    ///
    /// `None` when `offset` is outside the title content's half-open
    /// `[start, end)`.
    ///
    /// # Panics
    ///
    /// Panics if this title is not a child of `HEADING` or `INLINE_TASK`,
    /// which the parser never produces.
    #[must_use]
    pub fn split_at(&self, offset: TextSize) -> Option<SplitPoint> {
        let content = content_of(self.syntax());
        let dest = match self.syntax().parent() {
            Some(p) if p.kind() == OrgSyntaxKind::HEADING => Heading::cast(p)
                .expect("grammar: HEADING_TITLE under HEADING")
                .outline()
                .insert_after(),
            Some(p) if p.kind() == OrgSyntaxKind::INLINE_TASK => Element::from(
                InlineTask::cast(p).expect("grammar: HEADING_TITLE under INLINE_TASK"),
            )
            .insert_after(OrgSyntaxKind::INLINE_TASK),
            _ => panic!(
                "grammar: HEADING_TITLE is always a child of HEADING or INLINE_TASK \
                 (forms/heading.rs::emit_star_line_anatomy)"
            ),
        };
        split_span(content, offset, dest)
    }
}

impl HasObjects for HeadingTitle {}

// ---------------------------------------------------------------------------
// Parts, headline trait, tag value
// ---------------------------------------------------------------------------

/// Parts of a headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeadingPart {
    /// Leading `*` run.
    Stars,
    /// Optional TODO keyword.
    Todo,
    /// Optional priority cookie.
    Priority,
    /// Optional `COMMENT` marker.
    CommentMarker,
    /// The title; always present, possibly empty.
    Title,
    /// Optional tags, as in `:a:b:`.
    Tags,
}

/// Accessors shared by every headline: [`Heading`], [`crate::ast::InlineTask`]
/// and [`crate::ast::Task`].
///
/// The provided methods read [`Self::star_syntax`]. Only `Task` overrides it,
/// because its `syntax()` is the `OUTLINE` node rather than the headline.
pub trait Headline: HasParts<Part = HeadingPart> {
    /// The node that holds the headline: by default, this view's own node.
    fn star_syntax(&self) -> SyntaxNode {
        self.syntax().clone()
    }

    /// Number of stars.
    fn level(&self) -> usize {
        headline_level(&self.star_syntax())
    }

    /// Range of the leading stars.
    ///
    /// # Panics
    ///
    /// Panics if the line has no `STAR` token, which the parser never
    /// produces.
    fn stars_range(&self) -> TextRange {
        stars_span(&self.star_syntax())
    }

    /// The headline's first line, newline included (or through the end of
    /// input).
    ///
    /// For a [`Heading`] this is its whole [`AstNode::range`]; an inline task
    /// or a task also covers what follows the line.
    #[must_use]
    fn star_line_range(&self) -> TextRange {
        let syntax = self.star_syntax();
        TextRange::new(syntax.text_range().start(), first_line_end(&syntax))
    }

    /// The TODO keyword, if any.
    fn todo_keyword(&self) -> Option<TodoKeyword> {
        child_view(&self.star_syntax())
    }

    /// Priority cookie as written, when present.
    fn priority(&self) -> Option<Priority> {
        child_view(&self.star_syntax())
    }

    /// Whether the `COMMENT` marker is present.
    fn is_commented(&self) -> bool {
        comment_node(&self.star_syntax()).is_some()
    }

    /// Whether this headline's own tags include `ARCHIVE` (ASCII caseless).
    /// Inherited tags are not considered.
    fn is_archived(&self) -> bool {
        headline_is_archived(&self.star_syntax())
    }

    /// The title.
    ///
    /// # Panics
    ///
    /// Panics if `HEADING_TITLE` is missing, which the parser never produces.
    fn title(&self) -> HeadingTitle {
        heading_title(&self.star_syntax())
    }

    /// The tags written on this headline, if any.
    ///
    /// An empty tag group, as in `* T : :`, is still `Some`.
    fn tags(&self) -> Option<Tags> {
        child_view(&self.star_syntax())
    }

    /// The first whitespace-delimited word after the stars.
    ///
    /// This is the word the parser tests against the TODO keywords. When it is
    /// one, it is the [`Self::todo_keyword`]; otherwise it is a priority
    /// cookie, the `COMMENT` marker or the start of the title.
    fn post_stars_word_range(&self) -> Option<TextRange> {
        crate::ast::layout::post_stars_word_range(&self.star_syntax())
    }

    /// Where to write a title when the title is empty or only whitespace.
    ///
    /// The offset is the start of the title node. The lead is a space when a
    /// TODO keyword, priority or `COMMENT` comes right before it with no space
    /// in between; the trail is a space when tags follow the title node
    /// directly.
    #[must_use]
    fn title_fill_insert_point(&self) -> InsertPoint {
        title_fill_dest(&self.star_syntax())
    }
}

/// One tag as written: a name, its source range, and an optional fast key.
///
/// Written headline tags and `#+FILETAGS:` have no fast key. `#+TAGS:`
/// declarations may carry one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag {
    name: String,
    range: TextRange,
    fast_key: Option<char>,
}

impl Tag {
    pub(crate) fn new(name: impl Into<String>, range: TextRange, fast_key: Option<char>) -> Self {
        Self {
            name: name.into(),
            range,
            fast_key,
        }
    }

    /// Tag name as written.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Source range of the name.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// Fast-selection key, when a `#+TAGS:` line declared one.
    #[must_use]
    pub fn fast_key(&self) -> Option<char> {
        self.fast_key
    }
}

/// Where a tag in [`crate::Document::tags`] came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TagOrigin {
    /// Written on this headline.
    Written,
    /// Written on an ancestor heading (or, for an inline task, the enclosing
    /// outline's heading).
    Inherited {
        /// The heading the tag is written on.
        from: Heading,
    },
    /// From a live `#+FILETAGS:` line.
    FileTags,
}

// ---------------------------------------------------------------------------
// Priority value
// ---------------------------------------------------------------------------

impl Priority {
    /// The letter or number inside `[#X]`.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a `[#X]` cookie with a valid value, which the
    /// parser never produces.
    #[must_use]
    pub fn value(&self) -> PriorityAtom {
        let range = priority_value_range(self.syntax()).expect(
            "grammar: PRIORITY is L_BRACKET HASH TEXT R_BRACKET (forms/heading.rs::parse_priority)",
        );
        let tok = first_token_of_kind(self.syntax(), OrgSyntaxKind::TEXT).expect(
            "grammar: PRIORITY is L_BRACKET HASH TEXT R_BRACKET (forms/heading.rs::parse_priority)",
        );
        debug_assert_eq!(tok.text_range(), range);
        parse_priority_atom(tok.text())
            .expect("grammar: PRIORITY TEXT is A-Z or 0-64 (PriorityAtom::is_cookie_value)")
    }
}

fn parse_priority_atom(s: &str) -> Option<PriorityAtom> {
    if s.len() == 1 {
        let c = s.chars().next()?;
        if c.is_ascii_uppercase() {
            return Some(PriorityAtom::Letter(c));
        }
        if c.is_ascii_digit() {
            return Some(PriorityAtom::Number(c as u8 - b'0'));
        }
        return None;
    }
    let n: u8 = s.parse().ok()?;
    (n <= PriorityAtom::MAX_NUMBER).then_some(PriorityAtom::Number(n))
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

impl Tags {
    /// Individual tags between colons, in source order. Empty fragments
    /// (`::`) are skipped.
    pub fn iter(&self) -> impl Iterator<Item = Tag> + '_ {
        tag_runs(self.syntax()).map(|(range, name)| Tag::new(name, range, None))
    }
}

// ---------------------------------------------------------------------------
// Shared star-line reads
// ---------------------------------------------------------------------------

fn stars_span(syntax: &SyntaxNode) -> TextRange {
    // Only the opener line: an INLINE_TASK also owns the END line's stars.
    span_of(first_line(syntax).filter(|e| e.kind() == OrgSyntaxKind::STAR))
        .expect("grammar: star line always starts with STAR (forms/heading.rs::star_head)")
}

fn heading_title(syntax: &SyntaxNode) -> HeadingTitle {
    child_view(syntax)
        .expect("grammar always carves HEADING_TITLE (forms/heading.rs::emit_star_line_anatomy)")
}

fn comment_node(syntax: &SyntaxNode) -> Option<SyntaxNode> {
    syntax
        .children()
        .find(|c| c.kind() == OrgSyntaxKind::HEADING_COMMENT)
}

fn headline_is_archived(syntax: &SyntaxNode) -> bool {
    child_view::<Tags>(syntax).is_some_and(|tags| {
        tags.iter()
            .any(|t| t.name().eq_ignore_ascii_case("ARCHIVE"))
    })
}

fn headline_level(syntax: &SyntaxNode) -> usize {
    first_line(syntax)
        .filter(|e| e.kind() == OrgSyntaxKind::STAR)
        .count()
}

/// Shared `HasParts` implementation for [`Heading`] and [`crate::ast::InlineTask`].
pub(crate) fn headline_part_span(syntax: &SyntaxNode, part: HeadingPart) -> PartSpan {
    match part {
        HeadingPart::Stars => PartSpan::Present(PresentPart::identical(stars_span(syntax))),
        HeadingPart::Todo => match child_view::<TodoKeyword>(syntax) {
            Some(n) => present_with_removal_after(syntax, n.range()),
            None => absent_prefix(syntax, HeadingPart::Todo),
        },
        HeadingPart::Priority => match child_view::<Priority>(syntax) {
            Some(n) => present_with_removal_after(syntax, n.range()),
            None => absent_prefix(syntax, HeadingPart::Priority),
        },
        HeadingPart::CommentMarker => match comment_node(syntax) {
            Some(n) => present_with_removal_after(syntax, n.text_range()),
            None => absent_prefix(syntax, HeadingPart::CommentMarker),
        },
        HeadingPart::Title => {
            let title = heading_title(syntax);
            let r = title.range();
            let content = content_of(title.syntax());
            PartSpan::Present(PresentPart::new(r, content, r))
        }
        HeadingPart::Tags => {
            if let Some(tags) = child_view::<Tags>(syntax) {
                PartSpan::Present(PresentPart::identical(tags.range()))
            } else {
                let title = heading_title(syntax);
                let offset = content_of(title.syntax()).end();
                let lead = if whitespace_ends_at(title.syntax(), offset)
                    || whitespace_ends_at(syntax, offset)
                {
                    Padding::None
                } else {
                    Padding::Space
                };
                PartSpan::Absent(InsertPoint::new(offset, lead, Padding::None))
            }
        }
    }
}

/// Where a missing prefix goes: Org reads the prefixes only in the order
/// keyword, priority, `COMMENT` (`org-complex-heading-regexp`), so a
/// missing one goes before the first later prefix that is present, or at
/// the title's start when none is. Anywhere else it would be read as the
/// title's text.
fn absent_prefix(syntax: &SyntaxNode, part: HeadingPart) -> PartSpan {
    let later = match part {
        HeadingPart::Todo => [
            child_view::<Priority>(syntax).map(|n| n.range()),
            comment_node(syntax).map(|n| n.text_range()),
        ],
        HeadingPart::Priority => [comment_node(syntax).map(|n| n.text_range()), None],
        _ => [None, None],
    };
    match later.into_iter().flatten().next() {
        Some(next) => PartSpan::Absent(InsertPoint::new(
            next.start(),
            Padding::None,
            Padding::Space,
        )),
        None => absent_at_title_start(syntax),
    }
}

fn absent_at_title_start(syntax: &SyntaxNode) -> PartSpan {
    let title = heading_title(syntax);
    let offset = title.range().start();
    // Empty title with tags still needs a trail space so a prefix insert
    // cannot glue onto `:a:`. Bare `* \n` keeps trail None.
    let trail = if content_of(title.syntax()).is_empty() && child_view::<Tags>(syntax).is_none() {
        Padding::None
    } else {
        Padding::Space
    };
    // After a prefix with no space behind it (`* TODO`, an empty title),
    // the insert needs its own lead.
    let lead = if whitespace_ends_at(syntax, offset) {
        Padding::None
    } else {
        Padding::Space
    };
    PartSpan::Absent(InsertPoint::new(offset, lead, trail))
}

fn title_fill_dest(syntax: &SyntaxNode) -> InsertPoint {
    let title = heading_title(syntax);
    let offset = title.range().start();
    let prefix = child_view::<TodoKeyword>(syntax).is_some()
        || child_view::<Priority>(syntax).is_some()
        || comment_node(syntax).is_some();
    let lead = if prefix && !whitespace_ends_at(syntax, offset) {
        Padding::Space
    } else {
        Padding::None
    };
    let trail = match child_view::<Tags>(syntax) {
        Some(tags) if tags.range().start() == title.range().end() => Padding::Space,
        _ => Padding::None,
    };
    InsertPoint::new(offset, lead, trail)
}

pub(crate) const HEADLINE_PARTS: &[HeadingPart] = &[
    HeadingPart::Stars,
    HeadingPart::Todo,
    HeadingPart::Priority,
    HeadingPart::CommentMarker,
    HeadingPart::Title,
    HeadingPart::Tags,
];

// ---------------------------------------------------------------------------
// Heading
// ---------------------------------------------------------------------------

impl HasParts for Heading {
    type Part = HeadingPart;
    const PARTS: &'static [HeadingPart] = HEADLINE_PARTS;

    fn part_span(&self, part: HeadingPart) -> PartSpan {
        headline_part_span(self.syntax(), part)
    }
}

impl Headline for Heading {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice};

    fn heading(src: &str) -> (crate::Document, Heading) {
        let doc = parse(src);
        let h = Heading::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
                .into_iter()
                .next()
                .expect("heading"),
        )
        .expect("cast");
        (doc, h)
    }

    #[test]
    fn headline_field_insert_points() {
        let (_, h) = heading("* Title\n");
        let todo = h.part_insert_point(HeadingPart::Todo).unwrap();
        assert_eq!(todo.offset(), h.title().range().start());
        assert_eq!(todo.lead(), Padding::None);
        assert_eq!(todo.trail(), Padding::Space);

        let (_, h) = heading("* TODO Title\n");
        assert!(h.part_insert_point(HeadingPart::Todo).is_none());
        let pri = h.part_insert_point(HeadingPart::Priority).unwrap();
        assert_eq!(pri.offset(), h.title().range().start());

        let (_, h) = heading("* TODO [#A] Title\n");
        let comment = h.part_insert_point(HeadingPart::CommentMarker).unwrap();
        assert_eq!(comment.offset(), h.title().range().start());

        let (_, h) = heading("*  :a:b:\n");
        assert!(h.title().range().is_empty() || content_of(h.title().syntax()).is_empty());
        assert!(h.tags().is_some());

        let (_, h) = heading("* \n");
        let todo = h.part_insert_point(HeadingPart::Todo).unwrap();
        assert_eq!(todo.trail(), Padding::None);

        let (_, h) = heading("* :a:\n");
        let todo = h.part_insert_point(HeadingPart::Todo).unwrap();
        assert_eq!(todo.trail(), Padding::Space);
    }

    #[test]
    fn title_fill_insert_point_pads_prefix_and_adjacent_tags() {
        let (_, h) = heading("* TODO\n");
        let dest = h.title_fill_insert_point();
        assert_eq!(dest.offset(), h.title().range().start());
        assert_eq!(dest.lead(), Padding::Space);
        assert_eq!(dest.trail(), Padding::None);

        let (_, h) = heading("* :a:\n");
        let dest = h.title_fill_insert_point();
        assert_eq!(dest.lead(), Padding::None);
        assert_eq!(dest.trail(), Padding::Space);

        let (_, h) = heading("* TODO :a:\n");
        let dest = h.title_fill_insert_point();
        assert_eq!(dest.offset(), h.title().range().start());
        assert_eq!(dest.trail(), Padding::Space);

        let (_, h) = heading("* \n");
        let dest = h.title_fill_insert_point();
        assert_eq!(dest.lead(), Padding::None);
        assert_eq!(dest.trail(), Padding::None);
    }

    #[test]
    fn title_first_word_and_post_stars_word() {
        let (doc, h) = heading("* WAITING Title\n");
        let title_word = h.title().first_word_range().expect("title word");
        let post = h.post_stars_word_range().expect("post-stars");
        assert_eq!(source_slice(&doc, title_word), "WAITING");
        assert_eq!(source_slice(&doc, post), "WAITING");
        assert!(h.todo_keyword().is_none());

        let (doc, h) = heading("* TODO Title\n");
        assert_eq!(
            source_slice(&doc, h.post_stars_word_range().unwrap()),
            "TODO"
        );
        assert_eq!(
            source_slice(&doc, h.title().first_word_range().unwrap()),
            "Title"
        );

        let (doc, h) = heading("* [#A] WAITING\n");
        assert_eq!(
            source_slice(&doc, h.post_stars_word_range().unwrap()),
            "[#A]",
            "damage word is the post-stars run, not the title word"
        );
        assert_eq!(
            source_slice(&doc, h.title().first_word_range().unwrap()),
            "WAITING"
        );

        let (_, h) = heading("* \n");
        assert!(h.title().first_word_range().is_none());
        assert!(h.post_stars_word_range().is_none());
    }

    #[test]
    fn zero_width_titles() {
        for src in ["* \n", "* TODO\n", "* COMMENT\n", "*  :a:b:\n"] {
            let (_, h) = heading(src);
            crate::ast::check_part_contract(&h);
            assert!(content_of(h.title().syntax()).is_empty(), "{src:?}");
        }
    }

    #[test]
    fn title_content_span_with_and_without_tags() {
        let (doc, h) = heading("* Title :a:\n");
        assert_eq!(source_slice(&doc, h.title().range()).trim_end(), "Title");
        assert_eq!(
            source_slice(&doc, h.part_content_range(HeadingPart::Title).unwrap()),
            "Title"
        );

        let (doc, h) = heading("* just a title   \n");
        let carved = source_slice(&doc, h.title().range());
        assert!(carved.ends_with(' ') || carved == "just a title   ");
        assert_eq!(
            source_slice(&doc, h.part_content_range(HeadingPart::Title).unwrap()).trim(),
            "just a title"
        );
    }

    #[test]
    fn is_archived_is_caseless() {
        let (_, h) = heading("* H :ARCHIVE:\n");
        assert!(h.is_archived());
        let (_, h) = heading("* H :archive:\n");
        assert!(h.is_archived());
        let (_, h) = heading("* H :work:\n");
        assert!(!h.is_archived());
    }

    #[test]
    fn tags_iter_token_layout() {
        let (_, h) = heading("* H :a_b:@c:\n");
        let names: Vec<_> = h
            .tags()
            .unwrap()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        assert_eq!(names, ["a_b", "@c"]);
    }

    #[test]
    fn star_run_is_token_layout_despite_extra_space() {
        let (doc, h) = heading("*   Title\n");
        assert_eq!(h.level(), 1);
        assert_eq!(source_slice(&doc, h.stars_range()), "*");
        assert_eq!(
            source_slice(&doc, h.part_content_range(HeadingPart::Title).unwrap()),
            "Title"
        );
    }

    #[test]
    fn priority_value_letter_and_number() {
        let (_, h) = heading("* [#A] H\n");
        assert_eq!(h.priority().unwrap().value(), PriorityAtom::Letter('A'));
        let (_, h) = heading("* [#12] H\n");
        assert_eq!(h.priority().unwrap().value(), PriorityAtom::Number(12));
    }

    #[test]
    fn title_split_at_content_and_dest_after_subtree() {
        let src = "* Parent title here\n** Child\n";
        let (doc, h) = heading(src);
        let title = h.title();
        let content = content_of(title.syntax());
        assert_eq!(source_slice(&doc, content), "Parent title here");
        let offset = content.start() + crate::TextSize::from(7); // 't' of title
        let sp = title.split_at(offset).expect("inside");
        assert_eq!(source_slice(&doc, sp.keep()), "Parent ");
        assert_eq!(source_slice(&doc, sp.moved()), "title here");
        let after = h.outline().insert_after();
        assert_eq!(sp.dest().offset(), after.offset());
        assert!(sp.dest().offset() > h.range().end());
        assert!(title.split_at(content.end()).is_none());
        let sp = title.split_at(content.start()).expect("start");
        assert!(sp.keep().is_empty());

        let (doc, h) = heading("* Title :a:\n");
        let title = h.title();
        let content = content_of(title.syntax());
        assert_eq!(source_slice(&doc, content), "Title");
        assert!(title.split_at(content.end()).is_none());
    }

    #[test]
    fn title_split_at_inline_task_dest_after_whole_task() {
        let src = "*************** Title here\nbody\n*************** END\n";
        let doc = parse(src);
        let it = InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .expect("inline task"),
        )
        .expect("cast");
        let title = it.title();
        let content = content_of(title.syntax());
        assert_eq!(source_slice(&doc, content), "Title here");
        let offset = content.start() + crate::TextSize::from(6);
        let sp = title.split_at(offset).expect("inside");
        assert_eq!(source_slice(&doc, sp.keep()), "Title ");
        assert_eq!(source_slice(&doc, sp.moved()), "here");
        let after = Element::from(it.clone()).insert_after(OrgSyntaxKind::INLINE_TASK);
        assert_eq!(sp.dest().offset(), after.offset());
        assert!(sp.dest().offset() >= it.range().end());
        assert!(title.split_at(content.end()).is_none());
    }

    #[test]
    fn title_objects_left_bias() {
        use crate::ast::HasObjects;
        let (_, h) = heading("* *bold* plain\n");
        let title = h.title();
        let objs: Vec<_> = title.objects().collect();
        assert_eq!(objs.len(), 1);
        let br = objs[0].range();
        assert_eq!(title.object_at(br.end()).map(|o| o.range()), Some(br));
    }

    #[test]
    fn has_parts_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for node in nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING) {
                let h = Heading::cast(node).unwrap();
                crate::ast::check_part_contract(&h);
            }
        }
    }
}
