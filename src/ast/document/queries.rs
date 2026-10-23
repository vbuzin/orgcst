//! Document-level queries: outlines, typed walks (`map`, `node_at`),
//! covering a range, and the ladder of enclosing ranges.
//!
//! A query at a caret offset (an empty range) is left-biased: where two
//! tokens meet, the left one counts. A query over a non-empty range starts
//! from the token that owns its first byte.

use super::region_hits;
use crate::ast::context::is_context_skip;
use crate::ast::nav;
use crate::ast::support::{child_views, dest, token_at_caret, token_owning_byte};
use crate::ast::{
    AstNode, Covering, Element, ElementView, InsertPoint, Item, Object, Outline, Section,
};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{Document, TextRange, TextSize};

// ---------------------------------------------------------------------------
// Outline entry points
// ---------------------------------------------------------------------------

impl Document {
    /// Zeroth section: the direct `SECTION` child of the document, if any.
    #[must_use]
    pub fn zeroth_section(&self) -> Option<Section> {
        self.root().children().find_map(Section::cast)
    }

    /// Every outline, pre-order.
    pub fn outlines(&self) -> impl Iterator<Item = Outline> + '_ {
        self.map()
    }

    /// Top-level outlines only.
    pub fn root_outlines(&self) -> impl Iterator<Item = Outline> + '_ {
        child_views(self.root())
    }

    /// Innermost outline containing a caret at `offset`, left-biased.
    ///
    /// `None` before the first heading.
    #[must_use]
    pub fn outline_at(&self, offset: TextSize) -> Option<Outline> {
        self.node_at(offset)
    }

    /// Innermost outline whose extent contains the whole `range`.
    ///
    /// An empty `range` is the caret query [`Self::outline_at`]; a non-empty
    /// one must lie inside the outline byte for byte.
    #[must_use]
    pub fn covering_outline(&self, range: TextRange) -> Option<Outline> {
        if range.is_empty() {
            return self.outline_at(range.start());
        }
        self.covering(range)
    }

    /// Outlines whose extent intersects `range`, in document order.
    ///
    /// An empty `range` is the caret query [`Self::outline_at`]; a non-empty
    /// one must overlap the outline by at least one byte.
    pub fn outlines_in(&self, range: TextRange) -> impl Iterator<Item = Outline> + '_ {
        let hits: Vec<Outline> = if range.is_empty() {
            self.outline_at(range.start()).into_iter().collect()
        } else {
            self.outlines()
                .filter(|o| o.range().intersect(range).is_some_and(|i| !i.is_empty()))
                .collect()
        };
        hits.into_iter()
    }

    /// Where a new first top-level outline is written: before the first
    /// one, else after the zeroth section, else at the end of the document.
    #[must_use]
    pub fn first_outline_insert_point(&self) -> InsertPoint {
        if let Some(first) = self.root_outlines().next() {
            return first.insert_before();
        }
        fallback_outline_insert_point(self)
    }

    /// Where a new last top-level outline is written: after the last one,
    /// else as for [`Self::first_outline_insert_point`].
    #[must_use]
    pub fn last_outline_insert_point(&self) -> InsertPoint {
        if let Some(last) = self.root_outlines().last() {
            return last.insert_after();
        }
        fallback_outline_insert_point(self)
    }

    /// Every body element in the document, in pre-order.
    ///
    /// Affiliated keywords and `#+TBLFM:` lines are not elements. The same as
    /// `map::<Element>()`.
    pub fn elements(&self) -> impl Iterator<Item = Element> + '_ {
        self.map()
    }

    /// Elements whose `full_range()` overlaps `range` by at least one byte.
    /// An empty `range` yields nothing.
    pub fn elements_in(&self, range: TextRange) -> impl Iterator<Item = Element> + '_ {
        let hits: Vec<Element> = if range.is_empty() {
            Vec::new()
        } else {
            self.elements()
                .filter(|e| {
                    e.full_range()
                        .intersect(range)
                        .is_some_and(|i| !i.is_empty())
                })
                .collect()
        };
        hits.into_iter()
    }

    /// List items that overlap `range` by at least one byte. An empty `range`
    /// yields nothing.
    pub fn items_in(&self, range: TextRange) -> impl Iterator<Item = Item> + '_ {
        region_hits(range, self.map())
    }

    /// Every node that casts to `T`, in pre-order from the root.
    ///
    /// For a view whose `cast` tests the kind only, this includes nodes of
    /// that kind in every role. The same walk as `nav::descendants(root, &[])`.
    pub fn map<'a, T: AstNode + 'a>(&'a self) -> impl Iterator<Item = T> + 'a {
        nav::descendants(self.root(), MAP_NO_RECURSE)
    }

    /// First pre-order `T` matching `pred`.
    #[must_use]
    pub fn first<T: AstNode>(&self, pred: impl FnMut(&T) -> bool) -> Option<T> {
        nav::first(self.root(), pred)
    }

    /// Innermost node around a caret at `offset` that casts to `T`.
    ///
    /// Inside an affiliated-keyword run this does not jump to the element the
    /// run attaches to; [`Self::element_at`] does.
    #[must_use]
    pub fn node_at<T: AstNode>(&self, offset: TextSize) -> Option<T> {
        let start = caret_start(self.root(), offset)?;
        std::iter::once(start.clone())
            .chain(nav::ancestors(&start))
            .find_map(T::cast)
    }

    /// Innermost node of `kind` around a caret at `offset`.
    ///
    /// Left-biased, so at the boundary where a construct starts this finds
    /// the construct before it.
    #[must_use]
    pub fn node_at_kind(&self, offset: TextSize, kind: OrgSyntaxKind) -> Option<SyntaxNode> {
        ancestor_of_kind(&caret_start(self.root(), offset)?, kind)
    }

    /// Innermost node of `kind` around the byte at `offset`.
    ///
    /// Unlike [`Self::node_at_kind`], the right-hand token wins where two meet.
    /// `None` when `offset >= source.len()`.
    #[must_use]
    pub(crate) fn node_owning_kind(
        &self,
        offset: TextSize,
        kind: OrgSyntaxKind,
    ) -> Option<SyntaxNode> {
        ancestor_of_kind(&byte_start(self.root(), offset)?, kind)
    }

    /// Innermost body element around a caret at `offset`; inside an
    /// affiliated-keyword run, the element the run attaches to.
    #[must_use]
    pub fn element_at(&self, offset: TextSize) -> Option<Element> {
        let start = caret_start(self.root(), offset)?;
        Element::cast(start.clone()).or_else(|| nav::enclosing_element(&start))
    }

    /// Innermost object around a caret at `offset`.
    ///
    /// Object kinds count wherever they sit, so a timestamp on a planning or
    /// clock line is found too.
    #[must_use]
    pub fn object_at(&self, offset: TextSize) -> Option<Object> {
        self.node_at(offset)
    }

    /// Innermost `T` whose covering span contains the whole `range`.
    ///
    /// What counts as the span comes from [`Covering`]: an [`Element`]
    /// includes its affiliated keywords, other views use [`AstNode::range`].
    /// An empty `range` is a caret query, not containment of a zero-width
    /// span.
    #[must_use]
    pub fn covering<T: Covering>(&self, range: TextRange) -> Option<T> {
        if range.is_empty() {
            let start = caret_start(self.root(), range.start())?;
            let seed = T::covering_seed(&start);
            return T::cast(seed.clone()).or_else(|| nav::ancestors(&seed).find_map(T::cast));
        }
        let start = byte_start(self.root(), range.start())?;
        let seed = T::covering_seed(&start);
        std::iter::once(seed.clone())
            .chain(nav::ancestors(&seed))
            .find_map(|n| {
                let t = T::cast(n)?;
                T::covering_span(&t).contains_range(range).then_some(t)
            })
    }

    /// Innermost body element whose `full_range()` contains `range`.
    ///
    /// The same as `covering::<Element>(range)`.
    #[must_use]
    pub fn covering_element(&self, range: TextRange) -> Option<Element> {
        self.covering(range)
    }

    /// The spans that contain `range`, innermost first, without duplicates,
    /// ending with the whole document: the steps of an expand-selection.
    ///
    /// For a caret (empty `range`) the steps are the part under the caret and
    /// the frames of [`Self::context_at`]. For a non-empty range they are the
    /// enclosing nodes whose span contains it. An element contributes both
    /// its own range and its range with affiliated keywords. Outline entry
    /// ranges are not steps.
    pub fn ancestor_ranges(&self, range: TextRange) -> impl Iterator<Item = TextRange> + '_ {
        ancestor_ranges_vec(self, range).into_iter()
    }

    /// The first of [`Self::ancestor_ranges`] that is strictly larger than
    /// `range`.
    #[must_use]
    pub fn expand(&self, range: TextRange) -> Option<TextRange> {
        self.ancestor_ranges(range)
            .find(|r| r.contains_range(range) && *r != range)
    }
}

fn fallback_outline_insert_point(doc: &Document) -> InsertPoint {
    if let Some(section) = doc.zeroth_section() {
        dest(doc.root(), section.range().end())
    } else {
        dest(doc.root(), TextSize::of(doc.source()))
    }
}

fn caret_start(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    token_at_caret(root, offset)?.parent()
}

fn byte_start(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    token_owning_byte(root, offset)?.parent()
}

fn ancestor_of_kind(start: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxNode> {
    std::iter::once(start.clone())
        .chain(nav::ancestors(start))
        .find(|n| n.kind() == kind)
}

const MAP_NO_RECURSE: &[OrgSyntaxKind] = &[];

fn element_full_range(node: &SyntaxNode) -> Option<TextRange> {
    Element::cast(node.clone()).map(|el| el.full_range())
}

fn emit_ladder_rungs(out: &mut Vec<TextRange>, node: &SyntaxNode, range: TextRange) {
    let construct = node.text_range();
    if construct.contains_range(range) {
        push_unique(out, construct);
    }
    if let Some(full) = element_full_range(node)
        && full.contains_range(range)
    {
        push_unique(out, full);
    }
}

fn push_unique(out: &mut Vec<TextRange>, range: TextRange) {
    if out.last() != Some(&range) {
        out.push(range);
    }
}

fn ancestor_ranges_vec(doc: &Document, range: TextRange) -> Vec<TextRange> {
    let doc_range = TextRange::up_to(TextSize::of(doc.source()));
    let mut out = Vec::new();
    if range.is_empty() {
        let ctx = doc.context_at(range.start());
        if let Some(part) = ctx.innermost().part_span_at(range.start()) {
            push_unique(&mut out, part);
        }
        for frame in ctx.frames() {
            if let Some(n) = frame.syntax() {
                emit_ladder_rungs(&mut out, n, range);
            }
        }
    } else if let Some(start) = byte_start(doc.root(), range.start()) {
        for n in std::iter::once(start.clone()).chain(nav::ancestors(&start)) {
            if is_context_skip(&n) {
                continue;
            }
            emit_ladder_rungs(&mut out, &n, range);
            if n.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS
                && let Some(el) = nav::enclosing_element(&n)
            {
                emit_ladder_rungs(&mut out, el.syntax(), range);
            }
        }
    }
    push_unique(&mut out, doc_range);
    out
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

    #[test]
    fn first_outline_table() {
        let empty = parse("");
        let ip = empty.first_outline_insert_point();
        assert_eq!(ip.offset(), TextSize::from(0));
        assert_eq!(ip.lead(), crate::ast::Padding::None);

        let blanks = parse("\n\n");
        let ip = blanks.first_outline_insert_point();
        assert_eq!(ip.offset(), TextSize::of("\n\n"));
        assert!(blanks.zeroth_section().is_none());

        let intro = parse("intro\n");
        let ip = intro.first_outline_insert_point();
        assert_eq!(ip.offset(), intro.zeroth_section().unwrap().range().end());

        let headed = parse("* H\n");
        let ip = headed.first_outline_insert_point();
        assert_eq!(
            ip.offset(),
            headed.root_outlines().next().unwrap().range().start()
        );
    }

    #[test]
    fn last_outline_table() {
        for src in ["", "\n\n", "intro\n"] {
            let doc = parse(src);
            assert_eq!(
                doc.last_outline_insert_point(),
                doc.first_outline_insert_point(),
                "no-root fallback must match first-outline: {src:?}"
            );
        }

        let headed = parse("* H\n");
        let last = headed.root_outlines().next().unwrap();
        assert_eq!(headed.last_outline_insert_point(), last.insert_after());

        let two = parse("* A\n* B\n");
        let b = two.root_outlines().nth(1).unwrap();
        assert_eq!(two.last_outline_insert_point(), b.insert_after());
        assert_ne!(
            two.last_outline_insert_point().offset(),
            two.first_outline_insert_point().offset()
        );

        let unterm = parse("* A");
        let ip = unterm.last_outline_insert_point();
        assert_eq!(ip.offset(), TextSize::of("* A"));
        assert_eq!(ip.lead(), crate::ast::Padding::Newline);
        assert_eq!(ip, unterm.root_outlines().next().unwrap().insert_after());
    }

    #[test]
    fn outline_at_lead_in_and_owned_blanks() {
        let doc = parse("\n\n* H\n\nbody\n");
        assert!(doc.outline_at(TextSize::from(0)).is_none());
        let o = doc.root_outlines().next().unwrap();
        let after_heading = o.heading().range().end();
        assert_eq!(
            doc.outline_at(after_heading).as_ref().map(Outline::syntax),
            Some(o.syntax())
        );
    }

    #[test]
    fn covering_outline_two_root_siblings_is_none() {
        let doc = parse("* A\ntext\n* B\nmore\n");
        let a = doc.root_outlines().next().unwrap();
        let b = a.next_sibling().unwrap();
        let span = TextRange::new(a.range().start(), b.range().end());
        assert!(doc.covering_outline(span).is_none());
    }

    #[test]
    fn outlines_in_empty_range_is_left_biased_at_seam() {
        let doc = parse("* A\n* B\n");
        let a = doc.root_outlines().next().unwrap();
        let b = a.next_sibling().unwrap();
        let seam = b.range().start();
        let empty = TextRange::empty(seam);
        assert_eq!(
            doc.outline_at(seam).as_ref().map(Outline::syntax),
            Some(a.syntax())
        );
        assert_eq!(
            doc.covering_outline(empty).as_ref().map(Outline::syntax),
            Some(a.syntax())
        );
        let in_range: Vec<_> = doc.outlines_in(empty).collect();
        assert_eq!(in_range.len(), 1);
        assert_eq!(in_range[0].syntax(), a.syntax());
    }

    #[test]
    fn outlines_in_non_empty_intersect() {
        let doc = parse("* A\ntext\n* B\nmore\n");
        let a = doc.root_outlines().next().unwrap();
        let b = a.next_sibling().unwrap();
        let span = TextRange::new(a.range().start(), b.range().end());
        let hits: Vec<_> = doc.outlines_in(span).collect();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn node_at_empty_tree_is_none() {
        let doc = parse("");
        assert!(doc.node_at::<Paragraph>(TextSize::from(0)).is_none());
        assert!(doc.element_at(TextSize::from(0)).is_none());
        assert!(doc.object_at(TextSize::from(0)).is_none());
        assert!(
            doc.node_at_kind(TextSize::from(0), OrgSyntaxKind::PARAGRAPH)
                .is_none()
        );
    }

    #[test]
    fn node_at_keyword_vs_element_at_on_caption_and_tblfm() {
        let src = "#+CAPTION: c\npara\n";
        let doc = parse(src);
        let cap = at(src, "CAPTION");
        let kw = doc.node_at::<Keyword>(cap).expect("caption keyword");
        assert_eq!(
            kw.syntax().parent().map(|p| p.kind()),
            Some(OrgSyntaxKind::AFFILIATED_KEYWORDS)
        );
        let para = doc.element_at(cap).expect("caption hops");
        assert!(matches!(para, Element::Paragraph(_)));
        assert_eq!(
            para.syntax(),
            &nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)[0]
        );
        assert!(doc.node_at::<Paragraph>(cap).is_none());

        let src = "| a |\n#+TBLFM: $1=1\n";
        let doc = parse(src);
        let k = at(src, "TBLFM");
        let tblfm = doc.node_at::<Keyword>(k).expect("tblfm");
        let table = nodes_of_kind(doc.root(), OrgSyntaxKind::TABLE)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(tblfm.syntax().parent().as_ref(), Some(&table));
        assert_eq!(
            doc.element_at(k).as_ref().map(Element::syntax),
            Some(&table)
        );
        assert!(doc.elements().all(|e| !matches!(e, Element::Keyword(_))));
    }

    #[test]
    fn node_at_eof_bold_x_is_paragraph_not_markup() {
        let src = "*bold*x";
        let doc = parse(src);
        let end = TextSize::of(src);
        assert!(doc.node_at::<Markup>(end).is_none());
        assert!(doc.node_at::<Paragraph>(end).is_some());
        assert!(matches!(doc.element_at(end), Some(Element::Paragraph(_))));
        assert!(doc.object_at(end).is_none());
    }

    #[test]
    fn node_at_and_context_at_eof_on_carved_markup_is_paragraph() {
        let src = "*bold*!";
        let doc = parse(src);
        let end = TextSize::of(src);
        assert!(doc.node_at::<Markup>(end).is_none());
        assert!(doc.object_at(end).is_none());
        assert!(doc.node_at::<Paragraph>(end).is_some());
        assert!(matches!(
            doc.context_at(end).innermost(),
            ContextFrame::Paragraph(_)
        ));
    }

    #[test]
    fn outline_at_len_on_valid_heading_is_last_outline() {
        let src = "* H\n";
        let doc = parse(src);
        let o = doc.root_outlines().next().unwrap();
        assert_eq!(
            doc.outline_at(TextSize::of(src))
                .as_ref()
                .map(Outline::syntax),
            Some(o.syntax())
        );
        let src = "* H";
        let doc = parse(src);
        let o = doc.root_outlines().next().unwrap();
        assert_eq!(
            doc.outline_at(TextSize::of(src))
                .as_ref()
                .map(Outline::syntax),
            Some(o.syntax())
        );
    }

    #[test]
    fn map_equals_elements_for_element() {
        let src = "#+CAPTION: c\npara\n| a |\n#+TBLFM: $1=1\n";
        let doc = parse(src);
        let via_map: Vec<_> = doc.map::<Element>().map(|e| e.range()).collect();
        let via_el: Vec<_> = doc.elements().map(|e| e.range()).collect();
        assert_eq!(via_map, via_el);
        assert_eq!(via_el.len(), 2);
    }

    #[test]
    fn covering_element_two_siblings() {
        let drawer = parse(":drawer:\none\n\ntwo\n:end:\n");
        let d = drawer.drawers().next().unwrap();
        let kids: Vec<_> = d.elements().collect();
        assert_eq!(kids.len(), 2);
        let span = TextRange::new(kids[0].range().start(), kids[1].range().end());
        assert_eq!(
            drawer.covering_element(span).as_ref().map(Element::syntax),
            Some(d.syntax())
        );
        assert_eq!(
            drawer
                .covering::<Element>(span)
                .as_ref()
                .map(Element::syntax),
            Some(d.syntax())
        );

        let src = "* H\none\n\ntwo\n";
        let doc = parse(src);
        let sec = doc.root_outlines().next().unwrap().section().unwrap();
        let kids: Vec<_> = sec.elements().collect();
        let span = TextRange::new(kids[0].range().start(), kids[1].range().end());
        assert!(doc.covering_element(span).is_none());
        assert_eq!(
            doc.covering::<Section>(span).as_ref().map(Section::syntax),
            Some(sec.syntax())
        );
        assert!(doc.covering_outline(span).is_some());

        let src = "- one\n\n  two\n";
        let doc = parse(src);
        let list = doc.lists().next().unwrap();
        let item = list.items().next().unwrap();
        let kids: Vec<_> = item.elements().collect();
        assert!(kids.len() >= 2);
        let span = TextRange::new(kids[0].range().start(), kids[1].range().end());
        assert_eq!(
            doc.covering_element(span).as_ref().map(Element::syntax),
            Some(list.syntax())
        );
        assert_eq!(
            doc.covering::<Item>(span).as_ref().map(Item::syntax),
            Some(item.syntax())
        );

        let src = "[fn:1] one\n\ntwo\n";
        let doc = parse(src);
        let def = doc.footnote_definitions().next().unwrap();
        let kids: Vec<_> = def.elements().collect();
        let span = TextRange::new(kids[0].range().start(), kids[1].range().end());
        assert_eq!(
            doc.covering_element(span).as_ref().map(Element::syntax),
            Some(def.syntax())
        );

        let src = "*************** T\none\n\ntwo\n*************** END\n";
        let doc = parse(src);
        let task = doc
            .tasks()
            .find_map(|t| match t {
                Task::InlineTask(t) => Some(t),
                Task::Outline(_) => None,
            })
            .unwrap();
        let kids: Vec<_> = task.elements().collect();
        let span = TextRange::new(kids[0].range().start(), kids[1].range().end());
        assert_eq!(
            doc.covering_element(span).as_ref().map(Element::syntax),
            Some(task.syntax())
        );
    }

    #[test]
    fn covering_caption_value_hops_element_not_paragraph() {
        let src = "#+CAPTION: *c*\npara\n";
        let doc = parse(src);
        let val = span(src, "*c*");
        let para = Paragraph::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            doc.covering_element(val).as_ref().map(Element::syntax),
            Some(para.syntax())
        );
        assert_eq!(
            doc.covering::<Element>(val).as_ref().map(Element::syntax),
            Some(para.syntax())
        );
        assert_eq!(
            doc.covering::<Element>(TextRange::empty(val.start()))
                .as_ref()
                .map(Element::syntax),
            Some(para.syntax())
        );
        assert!(para.full_range().contains_range(val));
        assert!(doc.covering::<Paragraph>(val).is_none());
        assert!(matches!(
            doc.element_at(val.start()),
            Some(Element::Paragraph(_))
        ));
        assert!(
            doc.covering::<Paragraph>(TextRange::empty(val.start()))
                .is_none()
        );
    }

    #[test]
    fn covering_outline_star_byte_after_lead_in() {
        let src = "\n\n* H\n";
        let doc = parse(src);
        let o = doc.root_outlines().next().unwrap();
        let star = span(src, "*");
        assert_eq!(
            doc.covering_outline(star).as_ref().map(Outline::syntax),
            Some(o.syntax())
        );
        assert_eq!(
            doc.covering_outline(o.range())
                .as_ref()
                .map(Outline::syntax),
            Some(o.syntax())
        );
        let ladder: Vec<_> = doc.ancestor_ranges(star).collect();
        assert!(ladder.iter().any(|r| *r == o.range()));
        let own: Vec<_> = doc.ancestor_ranges(o.range()).collect();
        assert!(own.iter().any(|r| *r == o.range()));
        let expanded = doc.expand(star).expect("expand");
        assert_ne!(expanded, TextRange::up_to(TextSize::of(src)));
        assert!(expanded.contains_range(star));
    }

    #[test]
    fn ancestor_ranges_nested_and_expand_reaches_document() {
        let src = "* see [[http://ex.com][*x*]]\n";
        let doc = parse(src);
        let caret = TextRange::empty(at(src, "*x*"));
        let ladder: Vec<_> = doc.ancestor_ranges(caret).collect();
        for w in ladder.windows(2) {
            assert!(
                w[1].contains_range(w[0]),
                "{:?} should cover {:?}",
                w[1],
                w[0]
            );
        }
        assert_eq!(*ladder.last().unwrap(), TextRange::up_to(TextSize::of(src)));
        let mut r = caret;
        let mut steps = 0;
        while let Some(next) = doc.expand(r) {
            steps += 1;
            assert!(next.contains_range(r) && next != r);
            r = next;
            assert!(steps <= 128 + 8);
        }
        assert_eq!(r, TextRange::up_to(TextSize::of(src)));
    }

    #[test]
    fn covering_caption_expand_construct_then_full_range() {
        let src = "#+CAPTION: c\npara\n";
        let doc = parse(src);
        let para = Paragraph::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let caret = TextRange::empty(at(src, "ara"));
        assert_eq!(doc.expand(caret), Some(para.range()));
        let full = para.full_range();
        assert!(full.start() < para.range().start());
        assert_eq!(doc.expand(para.range()), Some(full));
    }

    #[test]
    fn ancestor_ranges_closed_inner_markup_skips_outer_part() {
        let src = "*a /b/ c*\n";
        let doc = parse(src);
        let italic = nodes_of_kind(doc.root(), OrgSyntaxKind::ITALIC)
            .into_iter()
            .next()
            .unwrap();
        let at_close = italic.text_range().end();
        let caret = TextRange::empty(at_close);
        let italic_v = crate::ast::Markup::cast(italic.clone()).unwrap();
        assert!(italic_v.part_at(at_close).is_none());
        let first = doc.expand(caret).expect("expand");
        assert_eq!(first, italic.text_range());
        let ladder: Vec<_> = doc.ancestor_ranges(caret).collect();
        for w in ladder.windows(2) {
            assert!(
                w[1].contains_range(w[0]),
                "{:?} should cover {:?}",
                w[1],
                w[0]
            );
        }

        let src = "* see *bold* rest\n";
        let doc = parse(src);
        let bold = nodes_of_kind(doc.root(), OrgSyntaxKind::BOLD)
            .into_iter()
            .next()
            .unwrap();
        let caret = TextRange::empty(bold.text_range().end());
        assert_eq!(doc.expand(caret), Some(bold.text_range()));
    }

    #[test]
    fn node_at_kind_and_document_first() {
        let src = "[[http://ex.com][*x*]]\n* Alpha\n* Beta\n";
        let doc = parse(src);
        let bold = nodes_of_kind(doc.root(), OrgSyntaxKind::BOLD)
            .into_iter()
            .next()
            .unwrap();
        let o = bold.text_range().start() + TextSize::from(1);
        assert_eq!(
            doc.node_at_kind(o, OrgSyntaxKind::BOLD)
                .as_ref()
                .map(crate::SyntaxNode::text_range),
            Some(bold.text_range())
        );
        let link = nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            doc.node_at_kind(o, OrgSyntaxKind::LINK)
                .as_ref()
                .map(crate::SyntaxNode::text_range),
            Some(link.text_range())
        );
        assert!(doc.node_at_kind(o, OrgSyntaxKind::TABLE).is_none());
        let hit = doc
            .first::<Heading>(|h| h.title().syntax().text().to_string().contains("Beta"))
            .unwrap();
        assert!(hit.title().syntax().text().to_string().contains("Beta"));
        assert!(
            doc.first::<Heading>(|h| h.title().syntax().text().to_string().contains("Nope"))
                .is_none()
        );
    }

    #[test]
    fn node_owning_kind_at_heading_seam_is_not_caret() {
        let doc = parse("* Alpha\n* Beta\n");
        let hs: Vec<_> = doc.map::<Heading>().collect();
        assert_eq!(hs.len(), 2);
        let seam = hs[1].range().start();
        assert_eq!(
            doc.node_at_kind(seam, OrgSyntaxKind::HEADING)
                .map(|n| n.text_range()),
            Some(hs[0].range()),
            "caret at Beta's start is still Alpha"
        );
        assert_eq!(
            doc.node_owning_kind(seam, OrgSyntaxKind::HEADING)
                .map(|n| n.text_range()),
            Some(hs[1].range()),
            "owning byte at Beta's start is Beta"
        );
        assert!(
            doc.node_owning_kind(TextSize::of(doc.source()), OrgSyntaxKind::HEADING)
                .is_none()
        );
    }

    #[test]
    fn map_equals_nav_descendants() {
        let src = "- a\n  - nested\n";
        let doc = parse(src);
        let via_map: Vec<_> = doc
            .map::<crate::ast::PlainList>()
            .map(|l| l.range())
            .collect();
        let via_nav: Vec<_> = nav::descendants::<crate::ast::PlainList>(doc.root(), &[])
            .map(|l| l.range())
            .collect();
        assert_eq!(via_map, via_nav);
        assert_eq!(via_map.len(), 2);
    }

    #[test]
    fn covering_clock_caption_is_keyword_not_clock() {
        let src = "#+CAPTION: c\nCLOCK: [2020-01-01]\n";
        let doc = parse(src);
        let cap = at(src, "CAPTION");
        let el = doc.element_at(cap).expect("element");
        assert!(matches!(el, Element::Keyword(_)));
        assert!(matches!(
            doc.covering_element(TextRange::empty(cap)),
            Some(Element::Keyword(_))
        ));
        assert!(doc.covering::<Clock>(TextRange::empty(cap)).is_none());
        assert!(matches!(
            doc.context_at(cap).element(),
            Some(Element::Keyword(_))
        ));

        let src = "#+CAPTION: c\npara\n";
        let doc = parse(src);
        assert!(matches!(
            doc.element_at(at(src, "CAPTION")),
            Some(Element::Paragraph(_))
        ));
    }

    #[test]
    fn document_object_at_vs_view_object_at() {
        let src = "*a* /b/\n";
        let doc = parse(src);
        let para = Paragraph::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        let bold = nodes_of_kind(para.syntax(), OrgSyntaxKind::BOLD)
            .into_iter()
            .next()
            .unwrap();
        let italic = nodes_of_kind(para.syntax(), OrgSyntaxKind::ITALIC)
            .into_iter()
            .next()
            .unwrap();
        let start = bold.text_range().start();
        assert_eq!(
            doc.object_at(start).as_ref().map(Object::syntax),
            para.object_at(start).as_ref().map(Object::syntax)
        );
        let last_end = italic.text_range().end();
        assert!(doc.object_at(last_end).is_some());
        assert!(para.object_at(last_end).is_some());
        let empty = parse("");
        assert!(empty.object_at(TextSize::from(0)).is_none());
        let nested = parse("*/inner/*\n");
        let italic = nodes_of_kind(nested.root(), OrgSyntaxKind::ITALIC)
            .into_iter()
            .next()
            .unwrap();
        let o = italic.text_range().start() + TextSize::from(1);
        assert_eq!(nested.object_at(o).unwrap().kind(), OrgSyntaxKind::ITALIC);
        let para = Paragraph::cast(
            nodes_of_kind(nested.root(), OrgSyntaxKind::PARAGRAPH)
                .into_iter()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(para.object_at(o).unwrap().kind(), OrgSyntaxKind::BOLD);
    }
}
