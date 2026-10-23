//! Document walk: `parse_document`, outlines, zeroth section, section loop.
//!
//! Outlines are built iteratively on an explicit stack (the "spine"), so
//! headline depth costs no Rust stack. Element recognition lives in
//! [`super::decide`]; parsing one element in [`super::region`].

use crate::parser::grammar::forms::{comment, drawer, heading};
use crate::parser::{Parser, event::Marker};
use crate::syntax::OrgSyntaxKind;

/// Outline level when the cursor is at line start and `ParseConfig` treats
/// that star line as a heading, otherwise `None`.
///
/// Section loops stop here and the outline spine opens here.
#[inline]
fn heading_level_at_cursor(parser: &Parser) -> Option<usize> {
    parser
        .line()
        .and_then(heading::probe)
        .and_then(heading::StarLine::as_heading)
}

/// Parse an entire document.
///
/// Emitted structure:
/// ```text
/// DOCUMENT
///   [SECTION]            // zeroth section (content before first heading)
///   OUTLINE*             // top-level outlines (no synthetic forest node)
/// ```
///
/// Leading blank lines are token children of DOCUMENT (outside any section).
/// The parser is infallible: every byte ends up somewhere in the tree.
pub(crate) fn parse_document(parser: &mut Parser) {
    let marker = parser.start();

    // File lead-in blanks: direct DOCUMENT children, outside any SECTION.
    parser.eat_blank_lines();

    // Content before the first heading is the "zeroth section".
    // It exists only if there is non-heading material at the top level.
    if parser.current() != OrgSyntaxKind::EOF && heading_level_at_cursor(parser).is_none() {
        parse_zeroth(parser);
    }

    // Parse top-level outlines. Each outline owns its heading, optional
    // section body, and deeper child OUTLINEs (heap frame stack, not Rust
    // recursion proportional to outline depth).
    parse_outlines(parser);

    debug_assert!(
        parser.nesting_balanced(),
        "nesting counters must return to zero at end of document"
    );
    marker.complete(parser, OrgSyntaxKind::DOCUMENT);
}

// ---------------------------------------------------------------------------
// Outlines (headings)
// ---------------------------------------------------------------------------

/// Open OUTLINE on the iterative spine: star level plus unfinished marker.
struct OutlineFrame {
    marker: Marker,
    level: usize,
}

/// Open one OUTLINE: HEADING, optional planning, post-heading blanks, optional
/// SECTION body up to the next heading. Leaves the OUTLINE marker open for
/// child OUTLINEs; the driver completes it via [`close_outline`].
///
/// Precondition: cursor at line-first and [`heading_level_at_cursor`] is
/// `Some(level)`.
///
/// Tree shape once completed. This deliberately differs from org-element,
/// which puts planning and the property drawer inside the section:
///
/// ```text
/// OUTLINE
///   HEADING
///   [PLANNING]           // metadata: only with no blank after HEADING
///   [PROPERTY_DRAWER]    // same adjacency slot (after planning blanks ok)
///   [SECTION]            // body elements only
///   OUTLINE*             // deeper children (completed later by the driver)
/// ```
///
/// Planning and property drawers are not recognised by
/// [`super::decide::decide`]: they are tried only right after the heading, so
/// `SCHEDULED:` / `:PROPERTIES:` lines elsewhere stay ordinary elements or
/// paragraph text.
fn open_outline(parser: &mut Parser, level: usize) -> OutlineFrame {
    parser.push_outline();
    let marker = parser.start();

    heading::parse(parser);

    // Headline metadata slot (Worg section 4.3.4 / org-element mode machine):
    // planning and a property drawer are adjacent to the heading - no blank
    // line between HEADING and either. After a planning line, blank lines
    // before the property drawer are tolerated (they live under OUTLINE).
    // The helper lives in the heading module so closed inline tasks share
    // it.
    heading::parse_metadata(parser);

    // Post-heading (and post-metadata) blanks live under OUTLINE, outside
    // SECTION (org-syntax: sections exclude blanks immediately after the
    // parent heading).
    parser.eat_blank_lines();

    // Direct section body (regular elements) up to the next heading of any
    // level. SECTION starts at the first non-blank body element (never
    // blank-only). The driver decides whether the next heading is a child
    // (deeper) or a terminator (same/shallower).
    if parser.current() != OrgSyntaxKind::EOF && heading_level_at_cursor(parser).is_none() {
        let section_m = parser.start();
        parse_elements_until_heading(parser);
        section_m.complete(parser, OrgSyntaxKind::SECTION);
    }

    OutlineFrame { marker, level }
}

/// Complete an OUTLINE opened by [`open_outline`].
///
/// The only place an OUTLINE marker is completed: when [`parse_outlines`]
/// meets a same-level or shallower headline, and at EOF.
fn close_outline(parser: &mut Parser, frame: OutlineFrame) {
    frame.marker.complete(parser, OrgSyntaxKind::OUTLINE);
    parser.pop_outline();
}

/// Parse one outline at the cursor, including nested outlines that stay inside it.
///
/// `outline_nesting` is the ancestor count, as in the file. Stops before a
/// heading that would close this outline or be re-parented above it, and does
/// not consume that heading, so a construct that runs past the old boundary
/// shows up as a longer extent instead of a clipped tree.
///
/// Returns false when the cursor is not a heading this budget can open here.
pub(crate) fn parse_outline_fragment(parser: &mut Parser) -> bool {
    let Some(root_level) = heading_level_at_cursor(parser) else {
        return false;
    };
    // The document spine opens a root headline even when the budget is already
    // spent, because there is no frame to close. A nested headline is opened
    // only when the ancestor count still has room.
    if !parser.outline_available() && parser.outline_nesting() > 0 {
        return false;
    }

    let mut stack = vec![open_outline(parser, root_level)];
    while parser.current() != OrgSyntaxKind::EOF {
        let Some(level) = heading_level_at_cursor(parser) else {
            super::region::parse_one_element(parser);
            continue;
        };
        if level <= root_level {
            break;
        }
        while stack.len() > 1 && stack.last().is_some_and(|frame| frame.level >= level) {
            if let Some(frame) = stack.pop() {
                close_outline(parser, frame);
            }
        }
        while !parser.outline_available() && stack.len() > 1 {
            if let Some(frame) = stack.pop() {
                close_outline(parser, frame);
            }
        }
        if !parser.outline_available() {
            break;
        }
        stack.push(open_outline(parser, level));
    }
    while let Some(frame) = stack.pop() {
        close_outline(parser, frame);
    }
    true
}

/// Single iterative document outline driver. Empty stack means DOCUMENT is the
/// parent; open frames nest child OUTLINEs under the innermost open marker.
///
/// Control flow (no Rust recursion proportional to headline depth):
/// - On a valid heading of level `L`, close open frames while
///   `frame.level >= L`, then [`open_outline`] and push the new frame.
/// - The stack never exceeds `ParseConfig::max_nesting_depth`: frames are closed
///   until the new one fits, so a headline past the ceiling becomes a sibling
///   there. This is a *tree depth* bound (green `Drop` recurses), not a stack
///   bound - the spine itself is already iterative.
/// - Non-heading tokens fall through to [`super::region::parse_one_element`] (defensive;
///   sections should normally have claimed them; keeps losslessness).
/// - At EOF, close every remaining open frame innermost-first.
///
/// Child OUTLINE nodes nest directly under the parent (DOCUMENT or parent
/// OUTLINE). There is no synthetic forest-wrapper OUTLINE.
///
/// Star lines that are not [`heading::StarLine::Heading`] (bad shape or
/// [`heading::StarLine::Task`]) are not outline spine: they fall through to
/// the element dispatcher like any other non-heading. Outline open requires
/// line-first ([`Parser::line`]); mid-line falls through to
/// [`super::region::parse_one_element`].
fn parse_outlines(parser: &mut Parser) {
    let mut stack: Vec<OutlineFrame> = Vec::with_capacity(16);

    while parser.current() != OrgSyntaxKind::EOF {
        match heading_level_at_cursor(parser) {
            Some(level) => {
                while let Some(frame) = stack.pop_if(|f| f.level >= level) {
                    close_outline(parser, frame);
                }
                // Nesting budget: a headline deeper than the ceiling attaches
                // *at* the ceiling instead of below it, keeping green tree
                // depth bounded (rowan's green Drop recurses over depth, so an
                // unbounded spine aborts on teardown even though the spine
                // itself costs no Rust stack). Nothing is lost - the headline
                // is still a HEADING under an OUTLINE, only re-parented, which
                // is the same silent re-parenting a level skip already does.
                while !parser.outline_available()
                    && let Some(frame) = stack.pop()
                {
                    close_outline(parser, frame);
                }
                stack.push(open_outline(parser, level));
            }
            // Non-heading material (including star lines that are not headlines
            // under the current config, or mid-line entry). Consume to keep
            // losslessness.
            None => {
                super::region::parse_one_element(parser);
            }
        }
    }

    while let Some(frame) = stack.pop() {
        close_outline(parser, frame);
    }
}

// ---------------------------------------------------------------------------
// Zeroth section
// ---------------------------------------------------------------------------

/// Parse the leading content section before any heading (the "zeroth section").
///
/// This is a SECTION node containing elements until the first heading or EOF.
/// File lead-in blanks were already claimed under DOCUMENT by the caller;
/// trailing blanks after body elements are claimed under those elements
/// ([`Parser::eat_blank_lines`] before their `complete`; orphan blanks use the
/// same helper under the open container).
///
/// Also the entry point for reparsing just the zeroth section. That caller
/// leaves the cursor on the section start (not on a file lead-in blank) and
/// reads how far the cursor moved. The next heading is not consumed.
pub(crate) fn parse_zeroth(parser: &mut Parser) {
    let marker = parser.start();

    // Zeroth-section property drawer slot (org: BEGINNING-OF-FILE
    // BLANK-LINES COMMENT PROPERTYDRAWER; blanks and the comment element are
    // optional). A leading comment element claims its own post-blank, so the
    // drawer probe runs right after it.
    if parser.line().is_some_and(|l| comment::probe(l).is_some()) {
        comment::parse(parser);
    }
    let shape = parser.line().and_then(drawer::probe_property);
    if let Some(shape) = shape {
        drawer::parse_property(parser, shape);
    }

    parse_elements_until_heading(parser);
    marker.complete(parser, OrgSyntaxKind::SECTION);
}

/// Consume regular elements until a headline or EOF.
///
/// Shared body loop for the zeroth SECTION and each outline SECTION opened in
/// [`open_outline`]. Stops at any valid heading (any level); the outline
/// driver decides whether that heading is a child or a terminator.
fn parse_elements_until_heading(parser: &mut Parser) {
    while parser.current() != OrgSyntaxKind::EOF && heading_level_at_cursor(parser).is_none() {
        super::region::parse_one_element(parser);
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::ParseConfig;
    use crate::syntax::OrgSyntaxKind;
    use crate::syntax::SyntaxNode;
    use crate::tests::{
        children_kinds, first_child_of_kind, format_tree, make_parser, node_count, node_text,
        nodes_of_kind, parse_document_tree, parse_document_tree_with_config, tree_text,
    };

    fn all_outlines(root: &SyntaxNode) -> Vec<SyntaxNode> {
        nodes_of_kind(root, OrgSyntaxKind::OUTLINE)
    }

    fn outline_children(node: &SyntaxNode) -> Vec<SyntaxNode> {
        node.children()
            .filter(|ch| ch.kind() == OrgSyntaxKind::OUTLINE)
            .collect()
    }

    fn outline_heading_contains(outline: &SyntaxNode, needle: &str) -> bool {
        first_child_of_kind(outline, OrgSyntaxKind::HEADING)
            .is_some_and(|h| tree_text(&h).contains(needle))
    }

    fn successive_deeper_headings(n: usize) -> String {
        let mut src = String::with_capacity(n * (n + 1) / 2 + n * 4);
        for depth in 1..=n {
            for _ in 0..depth {
                src.push('*');
            }
            src.push_str(" H\n");
        }
        src
    }

    /// ParseConfig for an `n`-deep outline fixture: star lines stay headlines, and
    /// the nesting budget is raised past `n` so the spine is what is under
    /// test rather than the clamp (which has its own tests below).
    fn deep_cfg(n: usize) -> ParseConfig {
        let level = u16::try_from(n.saturating_add(1)).unwrap_or(u16::MAX);
        ParseConfig::default()
            .with_inline_task_min_level(level)
            .with_max_nesting_depth(level)
    }

    fn find_outline_for_heading<'a>(
        outlines: &'a [SyntaxNode],
        needle: &str,
    ) -> Option<&'a SyntaxNode> {
        outlines
            .iter()
            .find(|o| outline_heading_contains(o, needle))
    }

    fn require_outline_with<'a>(
        outlines: &'a [SyntaxNode],
        needle: &str,
        root_for_dump: &SyntaxNode,
    ) -> &'a SyntaxNode {
        find_outline_for_heading(outlines, needle).unwrap_or_else(|| {
            panic!(
                "no outline owning a HEADING containing {:?}\nTREE:\n{}",
                needle,
                format_tree(root_for_dump)
            )
        })
    }

    fn collect_sections(root: &SyntaxNode) -> Vec<SyntaxNode> {
        nodes_of_kind(root, OrgSyntaxKind::SECTION)
    }

    #[test]
    fn outline_skips_heading_when_not_at_line_start() {
        // After leaving line.first, the cursor is not line-start.
        // The outline spine requires `Parser::line` (line-first).
        use crate::parser::grammar::forms::heading;

        let src = "* H\n";
        let mut p = make_parser(src);
        assert_eq!(
            p.line()
                .and_then(heading::probe)
                .and_then(heading::StarLine::as_heading),
            Some(1)
        );
        p.eat_any(); // first STAR; no longer at line.first
        assert!(p.line().is_none(), "mid-line entry has no line view");
        super::parse_outlines(&mut p);
        let (_, evs, _) = p.finish();
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::HEADING),
            0,
            "outline must not open HEADING off line-first"
        );
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::OUTLINE),
            0,
            "outline must not open OUTLINE off line-first"
        );
        assert_eq!(
            node_count(&evs, OrgSyntaxKind::PARAGRAPH),
            1,
            "mid-line headline line falls through to paragraph"
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::PARAGRAPH).as_deref(),
            Some(" H\n"),
            "residual after first STAR is paragraph text"
        );
    }

    #[test]
    fn threshold_filtered_star_line_is_inline_task_not_outline() {
        // Default inline_task_min_level is 15: fifteen stars is shape-ok but
        // not an outline headline; full document path must not open OUTLINE
        // and must emit INLINE_TASK via decide.
        let stars = "*".repeat(15);
        let src = format!("{stars} not heading\n");
        let root = parse_document_tree(&src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::HEADING).is_empty(),
            "no HEADING under default threshold"
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::OUTLINE).is_empty(),
            "no OUTLINE under default threshold"
        );
        let tasks = nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK);
        assert_eq!(tasks.len(), 1, "high-star line is INLINE_TASK");
        assert_eq!(tree_text(&tasks[0]), src);
    }

    #[test]
    fn outline_children_are_direct_only() {
        let root = parse_document_tree("* A\n** B\n*** C\n* D\n");
        let tops = outline_children(&root);
        assert_eq!(tops.len(), 2);
        assert!(outline_heading_contains(&tops[0], "A"));
        assert!(outline_heading_contains(&tops[1], "D"));
        // B is under A, not a DOCUMENT child.
        let a_kids = outline_children(&tops[0]);
        assert_eq!(a_kids.len(), 1);
        assert!(outline_heading_contains(&a_kids[0], "B"));
        assert_eq!(outline_children(&a_kids[0]).len(), 1);
        assert!(outline_heading_contains(
            &outline_children(&a_kids[0])[0],
            "C"
        ));
    }

    #[test]
    fn zeroth_section_is_direct_under_document_and_outlines_follow() {
        let src = "Intro text.\n\n* First\nbody\n";
        let root = parse_document_tree(src);
        assert_eq!(root.kind(), OrgSyntaxKind::DOCUMENT);

        // Hard shape: zeroth SECTION then exactly one top-level OUTLINE.
        // No synthetic forest wrapper under DOCUMENT.
        assert_eq!(
            children_kinds(&root),
            vec![OrgSyntaxKind::SECTION, OrgSyntaxKind::OUTLINE]
        );

        // The zeroth section text should contain the intro, not the heading.
        let zeroth = first_child_of_kind(&root, OrgSyntaxKind::SECTION).unwrap();
        let ztext = tree_text(&zeroth);
        assert!(ztext.contains("Intro text."), "zeroth should contain intro");
        assert!(
            !ztext.contains("First"),
            "zeroth must not swallow first heading"
        );
    }

    #[test]
    fn body_paragraph_attributed_to_section_under_its_outline() {
        // Core attribution test for the body loop inside open_outline.
        let src = "* H1\n\nThis is body.\n\nMore body.\n";
        let root = parse_document_tree(src);

        let outlines = all_outlines(&root);
        let top = require_outline_with(&outlines, "H1", &root);

        let kids = children_kinds(top);
        assert!(kids.contains(&OrgSyntaxKind::HEADING));
        assert!(
            kids.contains(&OrgSyntaxKind::SECTION),
            "body content must produce a SECTION under OUTLINE"
        );

        let sec = first_child_of_kind(top, OrgSyntaxKind::SECTION).unwrap();
        let stext = tree_text(&sec);
        assert!(
            stext.contains("This is body."),
            "section should contain the body paragraph"
        );
        assert!(
            stext.contains("More body."),
            "section should contain subsequent body"
        );
        // Post-heading blanks under OUTLINE, not SECTION.
        assert!(
            !stext.starts_with('\n'),
            "SECTION must not own post-heading blanks; got {stext:?}"
        );
        assert!(stext.starts_with("This is body."));
    }

    #[test]
    fn immediate_subheading_produces_no_section_for_parent() {
        // "* H1\n** H2" or with only blanks between: parent outline has no SECTION
        let src = "* H1\n** H2\nbody under h2\n";
        let root = parse_document_tree(src);

        // Top-level: exactly one OUTLINE under DOCUMENT (H1), no forest wrapper.
        assert_eq!(children_kinds(&root), vec![OrgSyntaxKind::OUTLINE]);

        let outlines = all_outlines(&root);
        let h1 = require_outline_with(&outlines, "H1", &root);

        let h1_kids = children_kinds(h1);
        // HEADING then direct child OUTLINE (H2) - no intermediate forest wrapper.
        assert_eq!(
            h1_kids,
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::OUTLINE]
        );
        let child_o = first_child_of_kind(h1, OrgSyntaxKind::OUTLINE).unwrap();
        let child_h = first_child_of_kind(&child_o, OrgSyntaxKind::HEADING).unwrap();
        assert!(
            tree_text(&child_h).contains("H2"),
            "child OUTLINE must own H2 directly"
        );
    }

    #[test]
    fn sibling_headings_do_not_leak_into_each_others_sections() {
        let src = "* A\npara under A\n\n* B\npara under B\n";
        let root = parse_document_tree(src);

        // Exactly two top-level OUTLINEs under DOCUMENT (no forest wrapper).
        assert_eq!(
            children_kinds(&root),
            vec![OrgSyntaxKind::OUTLINE, OrgSyntaxKind::OUTLINE]
        );

        let outlines = all_outlines(&root);
        let a = require_outline_with(&outlines, "A", &root);
        let b = require_outline_with(&outlines, "B", &root);

        let a_sec = first_child_of_kind(a, OrgSyntaxKind::SECTION);
        assert!(a_sec.is_some(), "A should have a section");
        let a_sec_text = tree_text(&a_sec.unwrap());
        assert!(a_sec_text.contains("para under A"));
        assert!(
            !a_sec_text.contains('B'),
            "A's section must not contain B's heading or content"
        );

        let b_sec = first_child_of_kind(b, OrgSyntaxKind::SECTION);
        let b_sec_text = b_sec.map(|s| tree_text(&s)).unwrap_or_default();
        assert!(b_sec_text.contains("para under B"));
        assert!(!b_sec_text.contains("para under A"));
    }

    #[test]
    fn body_before_subheading_goes_to_parents_section_not_child() {
        let src = "* Parent\n\nparent body here\n\n** Child\nchild only\n";
        let root = parse_document_tree(src);

        let outlines: Vec<_> = all_outlines(&root);
        let parent_o = require_outline_with(&outlines, "Parent", &root);

        // Parent must have a SECTION containing "parent body here"
        let psec = first_child_of_kind(parent_o, OrgSyntaxKind::SECTION)
            .expect("parent must have SECTION for its body");
        let psec_t = tree_text(&psec);
        assert!(psec_t.contains("parent body here"));
        assert!(
            !psec_t.contains("Child"),
            "parent section must not contain child heading"
        );

        // There must be a child OUTLINE under parent
        assert!(children_kinds(parent_o).contains(&OrgSyntaxKind::OUTLINE));
    }

    #[test]
    fn level_termination_same_level_sibling_stops_previous_body() {
        // When a same-level heading appears, previous outline's body loop stops.
        let src = "* L1\ncontent of L1\n* L1-sibling\ncontent of sibling\n";
        let root = parse_document_tree(src);

        let outs = all_outlines(&root);
        let first = require_outline_with(&outs, "L1", &root);

        let fsec = first_child_of_kind(first, OrgSyntaxKind::SECTION).expect("section for first");
        let ft = tree_text(&fsec);
        assert!(ft.contains("content of L1"));
        assert!(
            !ft.contains("L1-sibling"),
            "same level sibling heading terminates prior section"
        );
    }

    #[test]
    fn blanks_immediately_after_heading_are_not_part_of_its_section() {
        // Per org-syntax: "Sections do not include blank lines immediately following the parent heading."
        let src = "* H\n\n\nFirst content line.\n";
        let root = parse_document_tree(src);

        let outs = all_outlines(&root);
        let o = require_outline_with(&outs, "H", &root);
        let sec = first_child_of_kind(o, OrgSyntaxKind::SECTION);
        assert!(
            sec.is_some(),
            "expected a SECTION for heading with body after blanks"
        );
        let st = tree_text(&sec.unwrap());
        // The section text should not start with the blank lines; it should have the content.
        assert!(st.contains("First content line."));
        // It is acceptable for section text to have internal newlines from the para,
        // but the immediate post-heading blanks were consumed outside the section.
        // We assert that the reconstructed section does not have leading \n\n as its start content.
        let trimmed_start = st.trim_start_matches(['\n', '\r', ' ', '\t']);
        assert!(
            trimmed_start.starts_with("First"),
            "section should not be attributed the leading post-head blanks"
        );
    }

    #[test]
    fn document_with_only_headings_no_sections_created() {
        let src = "* A\n* B\n*** C\n";
        let root = parse_document_tree(src);
        // We still expect OUTLINE nodes, but their direct sections should be absent.
        let secs_under_outlines = collect_sections(&root)
            .into_iter()
            .filter(|s| {
                // a section whose parent is an OUTLINE (direct body section)
                s.parent()
                    .map_or_else(|| false, |p| p.kind() == OrgSyntaxKind::OUTLINE)
            })
            .count();
        // For this input there should be zero direct body sections (only blanks/immediate transitions).
        assert_eq!(
            secs_under_outlines, 0,
            "pure heading skeleton should not synthesize empty sections"
        );

        // Level-skip `*** C` nests under `* B` (not under DOCUMENT).
        let outs = all_outlines(&root);
        let b = require_outline_with(&outs, "B", &root);
        let b_kids = outline_children(b);
        assert_eq!(b_kids.len(), 1, "C under B");
        assert!(outline_heading_contains(&b_kids[0], "C"));
        assert_eq!(
            children_kinds(b),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::OUTLINE]
        );
    }

    #[test]
    fn mid_stack_shallower_close_leaves_parent_open() {
        let src = "* A\n** B\n*** C\nc-body\n** D\nd-body\n";
        let root = parse_document_tree(src);

        let outs = all_outlines(&root);
        let a = require_outline_with(&outs, "A", &root);
        let b = require_outline_with(&outs, "B", &root);
        let c = require_outline_with(&outs, "C", &root);
        let d = require_outline_with(&outs, "D", &root);

        // A owns two child OUTLINEs (B and D); D is not nested under B.
        let a_kids = outline_children(a);
        assert_eq!(a_kids.len(), 2, "A should have exactly two child OUTLINEs");
        assert!(
            outline_heading_contains(&a_kids[0], "B"),
            "first child of A is B"
        );
        assert!(
            outline_heading_contains(&a_kids[1], "D"),
            "second child of A is D"
        );

        // B owns C only; D must not appear under B.
        assert_eq!(
            children_kinds(b),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::OUTLINE]
        );
        let b_kids = outline_children(b);
        assert_eq!(b_kids.len(), 1);
        assert!(outline_heading_contains(&b_kids[0], "C"));
        assert!(
            !tree_text(b).contains("d-body"),
            "D body must not live under B after rise from 3 to 2"
        );

        // Bodies on the correct branches.
        let c_sec = first_child_of_kind(c, OrgSyntaxKind::SECTION).expect("C SECTION");
        assert!(tree_text(&c_sec).contains("c-body"));
        assert!(!tree_text(&c_sec).contains("d-body"));

        let d_sec = first_child_of_kind(d, OrgSyntaxKind::SECTION).expect("D SECTION");
        assert!(tree_text(&d_sec).contains("d-body"));
        assert!(!tree_text(&d_sec).contains("c-body"));
    }

    #[test]
    fn level_skip_then_mid_sibling_both_under_parent() {
        let src = "* A\n*** B\nb-body\n** C\nc-body\n";
        let root = parse_document_tree(src);

        let outs = all_outlines(&root);
        let a = require_outline_with(&outs, "A", &root);
        let b = require_outline_with(&outs, "B", &root);
        let c = require_outline_with(&outs, "C", &root);

        let a_kids = outline_children(a);
        assert_eq!(a_kids.len(), 2, "B and C both direct children of A");
        assert!(outline_heading_contains(&a_kids[0], "B"));
        assert!(outline_heading_contains(&a_kids[1], "C"));
        // Rising from 3 to 2 must not leave C under B.
        assert!(
            outline_children(&a_kids[0]).is_empty(),
            "{:?}",
            outline_children(&a_kids[0])
        );

        // Body attribution for the fixture lines (not decoration-only).
        let b_sec = first_child_of_kind(b, OrgSyntaxKind::SECTION).expect("B SECTION");
        assert!(tree_text(&b_sec).contains("b-body"));
        assert!(!tree_text(&b_sec).contains("c-body"));
        let c_sec = first_child_of_kind(c, OrgSyntaxKind::SECTION).expect("C SECTION");
        assert!(tree_text(&c_sec).contains("c-body"));
        assert!(!tree_text(&c_sec).contains("b-body"));
    }

    fn heading_star_count(heading: &SyntaxNode) -> usize {
        heading
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|t| t.kind() == OrgSyntaxKind::STAR)
            .count()
    }

    fn assert_successive_nest_chain(root: &SyntaxNode, n: usize) {
        let mut node = root.clone();
        for depth in 1..=n {
            let outline = first_child_of_kind(&node, OrgSyntaxKind::OUTLINE).unwrap_or_else(|| {
                panic!(
                    "missing OUTLINE at depth {depth}\nTREE head:\n{}",
                    format_tree(root)
                )
            });
            let heading =
                first_child_of_kind(&outline, OrgSyntaxKind::HEADING).expect("OUTLINE has HEADING");
            assert_eq!(
                heading_star_count(&heading),
                depth,
                "star count must equal depth {depth}"
            );
            assert!(
                tree_text(&heading).contains('H'),
                "heading text at depth {depth}"
            );
            if depth < n {
                assert_eq!(
                    children_kinds(&outline),
                    vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::OUTLINE],
                    "depth {depth} shape"
                );
            } else {
                assert_eq!(
                    children_kinds(&outline),
                    vec![OrgSyntaxKind::HEADING],
                    "deepest outline is heading-only"
                );
            }
            node = outline;
        }
        assert!(
            first_child_of_kind(&node, OrgSyntaxKind::OUTLINE).is_none(),
            "nesting stops at depth N"
        );
    }

    #[test]
    fn deep_outline_nesting_terminates_and_roundtrips() {
        // N=1024 needs an explicitly raised budget: the default ceiling is 128
        // precisely because rowan's green Drop recurses over tree depth, so an
        // unbounded spine aborts on teardown rather than in the parser. 1024
        // is still comfortably Drop-safe on a libtest thread stack.
        const N: usize = 1024;
        let src = successive_deeper_headings(N);
        let root = parse_document_tree_with_config(&src, &deep_cfg(N));

        assert_successive_nest_chain(&root, N);
    }

    /// Headlines deeper than the budget become siblings at the ceiling rather
    /// than nesting further: nothing is lost, only re-parented, so no
    /// diagnostic is emitted (a level skip re-parents silently too).
    #[test]
    fn outline_deeper_than_budget_attaches_at_the_ceiling() {
        const N: usize = 6;
        const BUDGET: u16 = 3;
        let src = successive_deeper_headings(N);
        let cfg = deep_cfg(N).with_max_nesting_depth(BUDGET);
        let out = crate::parse_with(&src, &cfg);

        assert_eq!(out.root().text().to_string(), src, "clamp stays lossless");
        assert!(
            out.diagnostics().is_empty(),
            "outline clamp re-parents silently: {:?}",
            out.diagnostics()
        );
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::HEADING).len(),
            N,
            "every headline is still a HEADING"
        );

        // Deepest chain is exactly BUDGET outlines.
        let mut depth = 0usize;
        let mut node = out.root().clone();
        while let Some(child) = first_child_of_kind(&node, OrgSyntaxKind::OUTLINE) {
            depth += 1;
            node = child;
        }
        assert_eq!(depth, usize::from(BUDGET), "outline depth capped at budget");

        // The innermost outline holds the over-budget headlines as siblings.
        let deepest_kids = outline_children(&node);
        assert!(
            deepest_kids.is_empty(),
            "no OUTLINE below the ceiling; tree=\n{}",
            format_tree(out.root())
        );
    }

    /// Budget zero still parses: no OUTLINE nesting at all, every headline a
    /// direct DOCUMENT child, and the source round-trips.
    #[test]
    fn outline_budget_zero_still_round_trips() {
        let src = successive_deeper_headings(4);
        let cfg = deep_cfg(4).with_max_nesting_depth(0);
        let out = crate::parse_with(&src, &cfg);

        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::OUTLINE).len(),
            4,
            "each headline still opens its own OUTLINE, none nested"
        );
        assert_eq!(
            outline_children(out.root()).len(),
            4,
            "all four are DOCUMENT children"
        );
    }

    #[test]
    fn deep_nest_body_then_sibling_closes_stack() {
        const N: usize = 64;
        let mut src = successive_deeper_headings(N);
        src.push_str("deep body\n");
        src.push_str("* sibling\nsibling body\n");

        let root = parse_document_tree_with_config(&src, &deep_cfg(N));

        assert_eq!(
            children_kinds(&root),
            vec![OrgSyntaxKind::OUTLINE, OrgSyntaxKind::OUTLINE],
            "sibling at level 1 must close the deep stack under DOCUMENT"
        );

        let tops = outline_children(&root);
        assert_eq!(tops.len(), 2);

        // Sibling shape and no leakage of deep body into the sibling outline.
        assert_eq!(
            children_kinds(&tops[1]),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::SECTION]
        );
        assert!(
            !tree_text(&tops[0]).contains("sibling body"),
            "first outline must not own sibling body"
        );

        // Intermediate frames along the nest are HEADING + child OUTLINE only
        // (no SECTION until the deepest body).
        let mut node = tops[0].clone();
        for depth in 1..N {
            assert_eq!(
                children_kinds(&node),
                vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::OUTLINE],
                "intermediate depth {depth} has no SECTION"
            );
            node = first_child_of_kind(&node, OrgSyntaxKind::OUTLINE).expect("child outline");
        }
        assert_eq!(
            children_kinds(&node),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::SECTION],
            "deepest outline owns body SECTION"
        );
        let deep_sec = first_child_of_kind(&node, OrgSyntaxKind::SECTION).expect("deepest SECTION");
        assert!(
            tree_text(&deep_sec).contains("deep body"),
            "body under deepest outline"
        );
        assert!(!tree_text(&deep_sec).contains("sibling body"));

        let sib_sec =
            first_child_of_kind(&tops[1], OrgSyntaxKind::SECTION).expect("sibling SECTION");
        assert!(tree_text(&sib_sec).contains("sibling body"));
        assert!(!tree_text(&sib_sec).contains("deep body"));

        // N nested + 1 sibling.
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::HEADING).len(),
            N + 1,
            "heading count"
        );
    }

    fn has_direct_newline_child(node: &SyntaxNode) -> bool {
        node.children_with_tokens().any(|c| {
            c.as_token()
                .is_some_and(|t| t.kind() == OrgSyntaxKind::NEWLINE)
        })
    }

    fn para_containing(root: &SyntaxNode, needle: &str) -> SyntaxNode {
        nodes_of_kind(root, OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .find(|p| tree_text(p).contains(needle))
            .unwrap_or_else(|| panic!("no PARAGRAPH containing {needle:?}\n{}", format_tree(root)))
    }

    #[test]
    fn blank_between_paragraphs_owned_by_first() {
        let src = "para1\n\npara2\n";
        let root = parse_document_tree(src);

        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2, "no pure-blank PARAGRAPH");
        assert_eq!(tree_text(&paras[0]), "para1\n\n");
        assert_eq!(tree_text(&paras[1]), "para2\n");
    }

    #[test]
    fn multi_blank_and_eof_post_blank_exact() {
        let src_multi = "p1\n\n\np2\n";
        let root_m = parse_document_tree(src_multi);
        assert_eq!(tree_text(&root_m), src_multi);
        let paras = nodes_of_kind(&root_m, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2);
        assert_eq!(tree_text(&paras[0]), "p1\n\n\n");
        assert_eq!(tree_text(&paras[1]), "p2\n");

        let src_eof = "para\n\n";
        let root_e = parse_document_tree(src_eof);
        assert_eq!(tree_text(&root_e), src_eof);
        let p = nodes_of_kind(&root_e, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(p.len(), 1);
        assert_eq!(tree_text(&p[0]), "para\n\n");
    }

    #[test]
    fn crlf_blank_between_paragraphs_owned_by_first() {
        let src = "para1\r\n\r\npara2\r\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2);
        assert_eq!(tree_text(&paras[0]), "para1\r\n\r\n");
        assert_eq!(tree_text(&paras[1]), "para2\r\n");
    }

    #[test]
    fn post_heading_blanks_under_outline_outside_section() {
        let src = "* H\n\npara\n";
        let root = parse_document_tree(src);

        let outline = first_child_of_kind(&root, OrgSyntaxKind::OUTLINE).expect("outline");
        let section = first_child_of_kind(&outline, OrgSyntaxKind::SECTION).expect("section");
        assert_eq!(tree_text(&section), "para\n");
        assert!(
            has_direct_newline_child(&outline),
            "OUTLINE should have direct blank token children; tree=\n{}",
            format_tree(&outline)
        );
        assert!(children_kinds(&outline).contains(&OrgSyntaxKind::SECTION));
    }

    #[test]
    fn post_planning_blanks_under_outline_outside_section() {
        let src = "* H\nSCHEDULED: <2020-01-01>\n\npara\n";
        let root = parse_document_tree(src);

        let outline = first_child_of_kind(&root, OrgSyntaxKind::OUTLINE).expect("outline");
        assert!(children_kinds(&outline).contains(&OrgSyntaxKind::PLANNING));
        let section = first_child_of_kind(&outline, OrgSyntaxKind::SECTION).expect("section");
        assert_eq!(tree_text(&section), "para\n");
        // Blank after PLANNING is under OUTLINE (between PLANNING and SECTION).
        let mut after_planning = false;
        let mut blank_after_planning = false;
        for c in outline.children_with_tokens() {
            if c.as_node()
                .is_some_and(|n| n.kind() == OrgSyntaxKind::PLANNING)
            {
                after_planning = true;
                continue;
            }
            if after_planning {
                if c.as_token()
                    .is_some_and(|t| t.kind() == OrgSyntaxKind::NEWLINE)
                {
                    blank_after_planning = true;
                    break;
                }
                if c.as_node()
                    .is_some_and(|n| n.kind() == OrgSyntaxKind::SECTION)
                {
                    break;
                }
            }
        }
        assert!(
            blank_after_planning,
            "post-planning blank under OUTLINE; tree=\n{}",
            format_tree(&outline)
        );
    }

    #[test]
    fn blanks_only_between_headings_no_blank_only_section() {
        let src = "* H\n\n* H2\n";
        let root = parse_document_tree(src);

        let outs = all_outlines(&root);
        let h = require_outline_with(&outs, "H", &root);
        assert!(
            first_child_of_kind(h, OrgSyntaxKind::SECTION).is_none(),
            "no blank-only SECTION between headings"
        );
        assert!(
            has_direct_newline_child(h),
            "post-heading blank under OUTLINE H; kids={}",
            format_tree(h)
        );
    }

    #[test]
    fn leading_blanks_under_document() {
        let src = "\n\npara\n";
        let root = parse_document_tree(src);
        assert_eq!(root.kind(), OrgSyntaxKind::DOCUMENT);

        let mut saw_section = false;
        let mut lead_newlines = 0usize;
        for c in root.children_with_tokens() {
            if c.as_node()
                .is_some_and(|n| n.kind() == OrgSyntaxKind::SECTION)
            {
                saw_section = true;
                break;
            }
            if c.as_token()
                .is_some_and(|t| t.kind() == OrgSyntaxKind::NEWLINE)
            {
                lead_newlines += 1;
            }
        }
        assert!(saw_section);
        assert_eq!(lead_newlines, 2, "lead-in blanks under DOCUMENT");
        let section = first_child_of_kind(&root, OrgSyntaxKind::SECTION).unwrap();
        assert_eq!(tree_text(&section), "para\n");
    }

    #[test]
    fn whitespace_lead_in_document_and_outline() {
        // Spaces-only lead-in under DOCUMENT (INDENT+NEWLINE blanks).
        let src_doc = "  \n\npara\n";
        let root_doc = parse_document_tree(src_doc);
        assert_eq!(tree_text(&root_doc), src_doc);
        let section = first_child_of_kind(&root_doc, OrgSyntaxKind::SECTION).expect("section");
        assert_eq!(tree_text(&section), "para\n");
        // Lead-in material is direct DOCUMENT children before SECTION.
        let mut lead = String::new();
        for c in root_doc.children_with_tokens() {
            if c.as_node()
                .is_some_and(|n| n.kind() == OrgSyntaxKind::SECTION)
            {
                break;
            }
            lead.push_str(&c.to_string());
        }
        assert_eq!(lead, "  \n\n");

        // Spaces/tabs after heading under OUTLINE, not SECTION.
        let src_o = "* H\n  \n\t\npara\n";
        let root_o = parse_document_tree(src_o);
        assert_eq!(tree_text(&root_o), src_o);
        let outline = first_child_of_kind(&root_o, OrgSyntaxKind::OUTLINE).expect("outline");
        let section = first_child_of_kind(&outline, OrgSyntaxKind::SECTION).expect("section");
        assert_eq!(tree_text(&section), "para\n");
        assert!(has_direct_newline_child(&outline));
    }

    #[test]
    fn blanks_after_block_end_under_block() {
        let src = "#+begin_quote\nQ\n#+end_quote\n\npara\n";
        let root = parse_document_tree(src);

        let quote = nodes_of_kind(&root, OrgSyntaxKind::QUOTE_BLOCK)
            .into_iter()
            .next()
            .expect("quote");
        assert_eq!(tree_text(&quote), "#+begin_quote\nQ\n#+end_quote\n\n");
        let para = para_containing(&root, "para");
        assert_eq!(tree_text(&para), "para\n");
    }

    #[test]
    fn greater_block_body_lead_in_blanks_under_block() {
        let src = "#+begin_quote\n\nQ\n#+end_quote\n";
        let root = parse_document_tree(src);
        let quote = nodes_of_kind(&root, OrgSyntaxKind::QUOTE_BLOCK)
            .into_iter()
            .next()
            .expect("quote");
        assert_eq!(tree_text(&quote), src);
        // Body lead-in blank is under the block; no pure-blank PARAGRAPH.
        let paras = nodes_of_kind(&quote, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1);
        assert_eq!(tree_text(&paras[0]), "Q\n");

        let src_ws = "#+begin_quote\n  \nQ\n#+end_quote\n";
        let root_ws = parse_document_tree(src_ws);
        assert_eq!(tree_text(&root_ws), src_ws);
        let quote_ws = nodes_of_kind(&root_ws, OrgSyntaxKind::QUOTE_BLOCK)
            .into_iter()
            .next()
            .expect("quote");
        let paras_ws = nodes_of_kind(&quote_ws, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras_ws.len(), 1);
        assert_eq!(tree_text(&paras_ws[0]), "Q\n");
        assert_eq!(tree_text(&quote_ws), src_ws);
    }

    #[test]
    fn element_post_blank_table_comment_fixed_hr_clock_setting_babel() {
        // (source, owner kind, exact owner tree_text including post-blank)
        let cases: &[(&str, OrgSyntaxKind, &str)] = &[
            ("# c\n\npara\n", OrgSyntaxKind::COMMENT, "# c\n\n"),
            (": fw\n\npara\n", OrgSyntaxKind::FIXED_WIDTH, ": fw\n\n"),
            (
                "-----\n\npara\n",
                OrgSyntaxKind::HORIZONTAL_RULE,
                "-----\n\n",
            ),
            (
                "CLOCK: [2020-01-01]\n\npara\n",
                OrgSyntaxKind::CLOCK,
                "CLOCK: [2020-01-01]\n\n",
            ),
            (
                "#+TODO: T | D\n\npara\n",
                OrgSyntaxKind::SETTING_KEYWORD,
                "#+TODO: T | D\n\n",
            ),
            (
                "#+CALL: f()\n\npara\n",
                OrgSyntaxKind::BABEL_CALL,
                "#+CALL: f()\n\n",
            ),
        ];
        for &(src, kind, owner_text) in cases {
            let root = parse_document_tree(src);
            let owners = nodes_of_kind(&root, kind);
            assert_eq!(owners.len(), 1, "one {kind:?} in {src:?}");
            assert_eq!(
                tree_text(&owners[0]),
                owner_text,
                "post-blank under {kind:?}"
            );
            let para = para_containing(&root, "para");
            assert_eq!(tree_text(&para), "para\n", "para after {kind:?}");
        }
    }

    #[test]
    fn whitespace_only_blank_lines_still_blank() {
        // Spaces/tabs-only lines are blank (at_blank INDENT+NEWLINE arm).
        let src = "para1\n  \n\t\npara2\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2, "no pure-blank PARAGRAPH for ws-only lines");
        assert_eq!(tree_text(&paras[0]), "para1\n  \n\t\n");
        assert_eq!(tree_text(&paras[1]), "para2\n");
    }

    #[test]
    fn eat_blank_lines_direct() {
        // Cursor-only harness: no builder (partial event coverage is fine).

        // Zero blanks: cursor unchanged.
        let mut p0 = make_parser("para\n");
        let key0 = p0.progress();
        p0.eat_blank_lines();
        assert_eq!(p0.progress(), key0, "zero blanks: no progress");
        assert_eq!(p0.text(), "para");

        // One blank: advances past NEWLINE to content.
        let mut p1 = make_parser("\npara\n");
        assert!(p1.at_blank());
        p1.eat_blank_lines();
        assert!(!p1.at_blank());
        assert_eq!(p1.text(), "para");

        // Multi blanks.
        let mut p2 = make_parser("\n\n\nx\n");
        p2.eat_blank_lines();
        assert_eq!(p2.text(), "x");

        // Ws-only blanks (INDENT+NEWLINE).
        let mut p3 = make_parser("  \n\t\nx\n");
        assert!(p3.at_blank());
        p3.eat_blank_lines();
        assert_eq!(p3.text(), "x");

        // Mid-tile: at_blank is false; eat_blank_lines is a no-op (no spin).
        let mut p4 = make_parser("ab\n");
        assert!(p4.eat_to(1)); // mid-tile on "ab"
        assert!(!p4.at_blank());
        let key4 = p4.progress();
        p4.eat_blank_lines();
        assert_eq!(p4.progress(), key4, "mid-tile: eat_blank_lines no-op");
    }

    #[test]
    fn blank_ownership_tiling_roundtrip() {
        let fixtures = [
            "para1\n\npara2\n",
            "p1\n\n\np2\n",
            "para\n\n",
            "para1\r\n\r\npara2\r\n",
            "* H\n\npara\n",
            "* H\nSCHEDULED: <2020-01-01>\n\npara\n",
            "* H\n\n* H2\n",
            "\n\npara\n",
            "  \n\npara\n",
            "* H\n  \n\t\npara\n",
            "#+CAPTION: x\npara\n",
            "#+CAPTION: x\n\npara\n",
            "#+begin_src\nx\n#+end_src\n\n",
            "#+begin_quote\n\nQ\n#+end_quote\n",
            "para1\n  \npara2\n",
            "# c\n\npara\n",
        ];
        for src in fixtures {
            let root = parse_document_tree(src);
            for p in nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH) {
                let t = tree_text(&p);
                assert!(
                    t.chars().any(|c| !c.is_whitespace()),
                    "pure-blank PARAGRAPH forbidden; text={t:?} in {src:?}"
                );
            }
        }
    }
}
