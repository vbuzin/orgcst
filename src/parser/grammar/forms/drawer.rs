//! Drawer grammar: generic `:NAME:` ... `:END:` drawers, the `:PROPERTIES:`
//! property drawer, and node-property lines.
//!
//! Evidence (org-element.el / org.el):
//! - `org-element-drawer-re`: `^[ \t]*:([-_ word]+):[ \t]*$` - a whole line;
//!   the name is word constituents plus `-` and `_`.
//! - `org-element-drawer-parser`: the FIRST following `^[ \t]*:END:[ \t]*$`
//!   (case-insensitive) closes the drawer; no `:END:` before the section end
//!   degrades the opener to paragraph text. "No nested drawers" is emergent:
//!   the outer end-search finds the first `:END:`, so an inner opener never
//!   has a closer left inside the outer region.
//! - `org-property-drawer-re`: `:PROPERTIES:` line, then zero or more
//!   node-property lines (`[ \t]*:\S-+:([ \t].*)?[ \t]*$` - key is any
//!   non-whitespace run whose last token is the closing colon), then the
//!   `:END:` line. The WHOLE drawer validates at recognition; one bad
//!   interior line means "not a property drawer" and the generic drawer
//!   grammar takes the region instead.
//! - Placement (org-element mode machine): property drawers live in the
//!   star-line metadata slot (directly after HEADING with no blank line, or
//!   after PLANNING with blank lines tolerated - planning owns post-blank),
//!   under closed INLINE_TASK via the same
//!   [`super::heading::parse_metadata`] helper, and in the zeroth
//!   section (optionally after leading comments). See `open_outline` /
//!   `parse_task` / `parse_zeroth`.
//!
//! Emits:
//! ```text
//! DRAWER | PROPERTY_DRAWER
//!   [INDENT] COLON DRAWER_NAME COLON [WHITESPACE] NEWLINE
//!   ... contents ...            // DRAWER: full element loop
//!                               // PROPERTY_DRAWER: NODE_PROPERTY lines
//!   [INDENT] end-line tokens NEWLINE
//!   [post-blank tokens]         // generic drawer only; the property
//!                               // drawer completes without post-blank in
//!                               // the metadata slot (OUTLINE owns blanks)
//! NODE_PROPERTY
//!   [INDENT] COLON NODE_PROPERTY_KEY COLON [WHITESPACE
//!   [NODE_PROPERTY_VALUE] [WHITESPACE]] NEWLINE
//! ```
//!
//! Recognition follows the block grammar's split:
//! - [`opener`] measures the whole-line opener shape cheaply.
//! - [`resolve`] finds the end line or returns a [`decide::DegradeReason`].
//!   The lookup uses the amortised frame-end index: the next `:END:` before a
//!   heading, as org bounds the search at the section end. The diagnostic
//!   message is built only when the drawer degrades.
//! - [`parse`] runs [`frame::parse_frame`] with an opener callback that emits
//!   `DRAWER_NAME`.
//! - Only a resolved drawer interrupts a paragraph. Unclosed openers degrade
//!   with `interrupts: false` in [`decide::decide`], and
//!   [`decide::interrupts_paragraph`] reads that flag.
//! - [`probe_property`] validates the whole property drawer up front. Only
//!   the metadata slots call it, never the element dispatcher.

use crate::parser::{LineId, LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

use super::heading;
use crate::parser::grammar::drive::{atoms, decide, frame};

// ---------------------------------------------------------------------------
// Generic drawers
// ---------------------------------------------------------------------------

/// Opener-line measurement: `:NAME:` plus trailing whitespace only.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrawerOpener {
    /// Content-relative token count of the name run (after the first colon).
    name_tokens: usize,
}

/// Parse-ready drawer measurement.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrawerShape {
    opener: DrawerOpener,
    /// Shared closed frame (`DRAWER`, `BodyMode::Elements` body, end line).
    frame: frame::FrameShape,
}

impl DrawerShape {
    /// Exclusive body end: the start of the `:END:` line. List parsing uses it
    /// to skip the whole frame.
    #[inline]
    pub(crate) fn body_end(self) -> Pos {
        self.frame.body_end()
    }
}

/// Whole-line drawer opener/closer shape: `[INDENT] :NAME: [ws]` to the line
/// end. NAME is one or more tokens of word constituents, `-` (inside TEXT),
/// or `_`.
pub(crate) fn opener(line: LineView) -> Option<DrawerOpener> {
    if line.kind(0) != OrgSyntaxKind::COLON {
        return None;
    }
    // Name run: word constituents / `-` TEXT / `_` (same class as footnote labels).
    let name_tokens = line.run_while(1, atoms::is_name_atom);
    if name_tokens == 0 {
        return None;
    }
    let mut k = 1 + name_tokens;
    if line.kind(k) != OrgSyntaxKind::COLON {
        return None;
    }
    k += 1;
    if line.kind(k) == OrgSyntaxKind::WHITESPACE {
        k += 1;
    }
    line.kind(k)
        .is_line_end()
        .then_some(DrawerOpener { name_tokens })
}

/// Source text of the opener's name run for a measured opener.
fn opener_name_of<'s>(line: LineView<'_, 's>, op: DrawerOpener) -> &'s str {
    line.text_range(1, 1 + op.name_tokens)
}

/// Source text of the drawer name on `line`, or `""` when it is not an opener.
///
/// Used to build the diagnostic for a degraded opener, so
/// [`decide::DegradeReason`] does not need to carry the name.
pub(crate) fn opener_name<'s>(line: LineView<'_, 's>) -> &'s str {
    opener(line).map_or("", |op| opener_name_of(line, op))
}

/// Whether `line` is a drawer end line: opener shape named `end` (caseless).
pub(crate) fn is_end_line(line: LineView) -> bool {
    opener(line).is_some_and(|op| opener_name_of(line, op).eq_ignore_ascii_case("end"))
}

/// Validate an opener into a parse-ready shape, or a Copy degrade reason.
///
/// The end lookup is amortised ([`crate::parser::grammar::drive::frame`]):
/// the next `:END:` after this line with no heading between, matching org's
/// section-end bound. It does not allocate; the message is built only when
/// the [`decide::DegradeReason`] becomes a [`crate::DiagnosticKind`].
pub(crate) fn resolve(
    line: LineView,
    op: DrawerOpener,
) -> Result<DrawerShape, decide::DegradeReason> {
    match frame::drawer_end_after(line.parser(), line) {
        Some(body_end) => Ok(DrawerShape {
            opener: op,
            frame: frame::FrameShape {
                kind: OrgSyntaxKind::DRAWER,
                body_end,
                body: frame::BodyMode::Elements,
                end_line: true,
            },
        }),
        None => Err(decide::DegradeReason::UnterminatedDrawer),
    }
}

/// Parse a complete generic drawer from its measured shape (no fail path).
///
/// Contents run the full element loop between the opener and end lines
/// (org-syntax 4.2.2: any element except another drawer; the exclusion is
/// emergent from first-`:END:`-wins). Trailing blanks belong to the drawer.
/// Body / end line / blanks via [`frame::parse_frame`].
pub(crate) fn parse(parser: &mut Parser, shape: DrawerShape) {
    frame::parse_frame(parser, shape.frame, |p| parse_opener_line(p, shape.opener));
}

/// Consume the opener line: `[INDENT] COLON DRAWER_NAME COLON [ws] NEWLINE`.
fn parse_opener_line(parser: &mut Parser, op: DrawerOpener) {
    parser.eat(OrgSyntaxKind::INDENT);
    parser.eat(OrgSyntaxKind::COLON);
    parser.leaf(OrgSyntaxKind::DRAWER_NAME, op.name_tokens);
    parser.eat(OrgSyntaxKind::COLON);
    let _ = parser.eat_to_line_end();
    parser.eat(OrgSyntaxKind::NEWLINE);
}

// ---------------------------------------------------------------------------
// Property drawers (context slots only)
// ---------------------------------------------------------------------------

/// Validated property-drawer measurement.
///
/// Interiors are the probe's `Copy` node-property shapes, carried into parse
/// so emit does not re-measure. Empty drawer: `interiors` is empty.
#[derive(Debug, Clone)]
pub(crate) struct PropertyDrawerShape {
    opener: DrawerOpener,
    end_line: LineId,
    interiors: Vec<NodePropertyShape>,
}

/// Whole-drawer recognition: `:PROPERTIES:` opener, every interior line a
/// node property, and a closing `:END:` line (org-property-drawer-re).
///
/// A blank or non-property interior line fails the probe; the region then
/// belongs to the generic drawer grammar (org parses a broken `:PROPERTIES:`
/// as a plain drawer). Context slots call this; the dispatcher never does.
pub(crate) fn probe_property(line: LineView) -> Option<PropertyDrawerShape> {
    let op = opener(line)?;
    if !opener_name_of(line, op).eq_ignore_ascii_case("properties") {
        return None;
    }
    let mut interiors = Vec::new();
    for l in line.lines_after().take_while(|l| {
        heading::probe(*l)
            .and_then(heading::StarLine::as_heading)
            .is_none()
    }) {
        if is_end_line(l) {
            return Some(PropertyDrawerShape {
                opener: op,
                end_line: l.id(),
                interiors,
            });
        }
        interiors.push(node_property_shape(l)?);
    }
    None
}

/// Parse a validated property drawer.
///
/// Completes without claiming trailing blanks: in the outline metadata slot
/// the following blanks belong to OUTLINE (same policy as PLANNING); in the
/// zeroth section the element loop claims them under SECTION.
pub(crate) fn parse_property(parser: &mut Parser, shape: PropertyDrawerShape) {
    let marker = parser.start();
    parse_opener_line(parser, shape.opener);

    for interior in shape.interiors {
        parse_node_property(parser, interior);
    }

    debug_assert!(
        parser.line().is_some_and(|l| l.id() == shape.end_line),
        "probe_property promised :END: immediately after the carried interiors"
    );
    parser.eat_line();
    marker.complete(parser, OrgSyntaxKind::PROPERTY_DRAWER);
}

/// Node-property line measurement.
///
/// Shape (`org-property-re` / the whole-drawer interior line): `[INDENT] :`
/// then a non-whitespace run whose last token is the closing `:`, then
/// end-of-line or whitespace + optional value.
#[derive(Debug, Clone, Copy)]
struct NodePropertyShape {
    /// Tokens in the key run (between the colons; at least one).
    key_tokens: usize,
    /// Exclusive end of the whitespace-trimmed value, when a value exists.
    value_end: Option<Pos>,
}

/// Recognise one node-property line.
fn node_property_shape(line: LineView) -> Option<NodePropertyShape> {
    if line.kind(0) != OrgSyntaxKind::COLON {
        return None;
    }
    // Non-whitespace run after the opening colon; its last token must be the
    // closing colon and the key between must be non-empty.
    let key_run = line.run_while(1, |kind, _| !kind.is_line_end_or_space());
    let k = 1 + key_run;
    let last_was_colon = key_run > 0 && line.kind(k - 1) == OrgSyntaxKind::COLON;
    let key_tokens = k.checked_sub(2).filter(|_| last_was_colon && k >= 3)?;

    // Value: optional; must be separated from the closer by whitespace.
    // Trailing whitespace stays outside the value node.
    let mut value_end = None;
    if line.kind(k) == OrgSyntaxKind::WHITESPACE {
        let value_start = k + 1;
        let n = line.run_while(value_start, |kind, _| !kind.is_line_end());
        let mut last_content = None;
        for i in 0..n {
            let idx = value_start + i;
            if line.kind(idx) != OrgSyntaxKind::WHITESPACE {
                last_content = line.nth(idx);
            }
        }
        value_end = last_content.map(|t| t.end());
    }

    Some(NodePropertyShape {
        key_tokens,
        value_end,
    })
}

/// Consume one node-property line from a shape [`probe_property`] already
/// measured, without re-measuring it.
fn parse_node_property(parser: &mut Parser, shape: NodePropertyShape) {
    let marker = parser.start();
    parser.eat(OrgSyntaxKind::INDENT);
    parser.eat(OrgSyntaxKind::COLON);
    parser.leaf(OrgSyntaxKind::NODE_PROPERTY_KEY, shape.key_tokens);
    parser.eat(OrgSyntaxKind::COLON);

    if let Some(value_end) = shape.value_end {
        parser.eat(OrgSyntaxKind::WHITESPACE);
        parser.leaf_to(OrgSyntaxKind::NODE_PROPERTY_VALUE, value_end);
    }

    let _ = parser.eat_to_line_end();
    parser.eat(OrgSyntaxKind::NEWLINE);
    marker.complete(parser, OrgSyntaxKind::NODE_PROPERTY);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        ParseResult, diags_any, first_child_of_kind, make_parser, node_count, node_kinds,
        node_text, nodes_of_kind, parse_document_tree, probe_line, tree_text,
    };

    fn drawer_probe(line: LineView) -> Option<DrawerShape> {
        resolve(line, opener(line)?).ok()
    }

    fn results_of(src: &str) -> ParseResult {
        crate::tests::run_element(src, drawer_probe, parse)
    }

    // --- Opener shape -------------------------------------------------------

    #[test]
    fn opener_accepts_word_hyphen_underscore_names() {
        assert!(probe_line(":LOGBOOK:\n", opener).is_some());
        assert!(probe_line(":my-drawer:\n", opener).is_some());
        assert!(probe_line(":my_drawer:\n", opener).is_some());
        assert!(probe_line(":D1:\n", opener).is_some());
        assert!(probe_line("  :indented:\n", opener).is_some());
        assert!(probe_line(":name:   \n", opener).is_some()); // trailing ws
        assert!(probe_line(":END:\n", opener).is_some()); // end shape is opener-shaped
        assert!(probe_line(":café:\n", opener).is_some()); // unicode word chars
    }

    #[test]
    fn opener_rejects_non_drawer_lines() {
        assert!(probe_line(":not a drawer:\n", opener).is_none()); // space in name
        assert!(probe_line(":foo.bar:\n", opener).is_none()); // dot not a word char
        assert!(probe_line("::\n", opener).is_none()); // empty name
        assert!(probe_line(":name\n", opener).is_none()); // missing closer
        assert!(probe_line(":name: trailing\n", opener).is_none()); // content after
        assert!(probe_line("name:\n", opener).is_none()); // no leading colon
        assert!(probe_line(": spaced:\n", opener).is_none()); // fixed-width shape
    }

    // --- Resolution / degradation ------------------------------------------

    #[test]
    fn resolve_finds_case_insensitive_end() {
        assert!(probe_line(":D:\nbody\n:END:\n", drawer_probe).is_some());
        assert!(probe_line(":D:\nbody\n:end:\n", drawer_probe).is_some());
        assert!(probe_line(":D:\nbody\n  :End:  \n", drawer_probe).is_some());
        assert!(probe_line(":D:\n:END:\n", drawer_probe).is_some()); // empty drawer
    }

    #[test]
    fn resolve_degrades_unterminated_and_heading_cut() {
        let p = make_parser(":D:\nbody\n");
        let line = p.line().expect("line");
        let op = opener(line).expect("opener");
        assert!(matches!(
            resolve(line, op),
            Err(crate::parser::grammar::drive::decide::DegradeReason::UnterminatedDrawer)
        ));

        // A heading bounds the end search (org limit = section end).
        let p2 = make_parser(":D:\nbody\n* H\n:END:\n");
        let line2 = p2.line().expect("line");
        let op2 = opener(line2).expect("opener");
        assert!(resolve(line2, op2).is_err());
    }

    #[test]
    fn closed_drawer_interrupts_unclosed_does_not() {
        use crate::parser::grammar::drive::decide::interrupts_paragraph;
        fn interrupts_first(src: &str) -> bool {
            let p = make_parser(src);
            let line = p.line().expect("line");
            interrupts_paragraph(line)
        }
        assert!(interrupts_first(":D:\n:END:\n"));
        assert!(!interrupts_first(":D:\nno end\n"));
        assert!(!interrupts_first("text\n"));
    }

    // --- Generic drawer structure ------------------------------------------

    #[test]
    fn drawer_emits_name_and_recursive_contents() {
        let src = ":LOGBOOK:\npara *bold* text\n:END:\n";
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::DRAWER));
        assert!(kinds.contains(&OrgSyntaxKind::PARAGRAPH));
        assert!(kinds.contains(&OrgSyntaxKind::BOLD));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::DRAWER_NAME).as_deref(),
            Some("LOGBOOK")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::DRAWER).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn drawer_owns_trailing_blanks() {
        let src = ":D:\nx\n:END:\n\npara\n";
        let root = parse_document_tree(src);
        let drawers = nodes_of_kind(&root, OrgSyntaxKind::DRAWER);
        assert_eq!(drawers.len(), 1);
        assert_eq!(tree_text(&drawers[0]), ":D:\nx\n:END:\n\n");
    }

    #[test]
    fn first_end_wins_no_nested_drawers() {
        // Inner opener has no closer left inside the outer region -> inner
        // degrades to paragraph text; trailing lone :END: line becomes an
        // unterminated opener named END -> paragraph.
        let src = ":OUTER:\n:INNER:\nx\n:END:\n:END:\n";
        let root = parse_document_tree(src);
        let drawers = nodes_of_kind(&root, OrgSyntaxKind::DRAWER);
        assert_eq!(drawers.len(), 1, "one drawer only");
        assert_eq!(tree_text(&drawers[0]), ":OUTER:\n:INNER:\nx\n:END:\n");
        // The inner opener line is paragraph text inside the outer drawer.
        let inner_para = nodes_of_kind(&drawers[0], OrgSyntaxKind::PARAGRAPH);
        assert!(
            inner_para.iter().any(|p| tree_text(p).contains(":INNER:")),
            "inner opener stays paragraph text"
        );
    }

    #[test]
    fn unterminated_drawer_degrades_via_dispatcher() {
        use crate::DiagnosticKind;
        let src = ":D:\nbody\n";
        let mut p = make_parser(src);
        crate::parser::grammar::drive::region::parse_one_element(&mut p);
        let (_, evs, diags) = p.finish();
        assert_eq!(node_count(&evs, OrgSyntaxKind::DRAWER), 0);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::PARAGRAPH));
        assert!(diags_any(&diags, |k| matches!(
            k,
            DiagnosticKind::UnterminatedDrawer { name } if name == "D"
        )));
    }

    #[test]
    fn unterminated_opener_absorbed_by_running_paragraph() {
        // org paragraph rule: an unclosed drawer opener does not separate.
        let src = "text before\n:D:\nstill same paragraph\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1, "one paragraph absorbs the opener");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::DRAWER).len(), 0);
    }

    #[test]
    fn closed_drawer_terminates_paragraph() {
        let src = "text before\n:D:\nx\n:END:\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::DRAWER).len(), 1);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert!(
            paras.iter().any(|p| tree_text(p) == "text before\n"),
            "paragraph stops before the closed drawer"
        );
    }

    #[test]
    fn drawer_inside_section_and_containing_block() {
        let src = "* H\n:LOG:\n#+begin_example\nraw\n#+end_example\n:END:\n";
        let root = parse_document_tree(src);
        let drawer = nodes_of_kind(&root, OrgSyntaxKind::DRAWER);
        assert_eq!(drawer.len(), 1);
        assert!(
            !nodes_of_kind(&drawer[0], OrgSyntaxKind::EXAMPLE_BLOCK).is_empty(),
            "block parses inside drawer contents"
        );
    }

    // --- Node properties ----------------------------------------------------

    #[test]
    fn node_property_shapes() {
        fn shape(src: &str) -> Option<(usize, bool)> {
            probe_line(src, node_property_shape).map(|s| (s.key_tokens, s.value_end.is_some()))
        }
        assert_eq!(shape(":KEY: value\n"), Some((1, true)));
        assert_eq!(shape(":KEY:\n"), Some((1, false)));
        assert_eq!(shape(":KEY:   \n"), Some((1, false))); // ws-only value is empty
        assert_eq!(shape(":KEY+: v\n"), Some((2, true))); // additive: `+` in key
        assert_eq!(shape(":k-e_y*2: v\n").map(|s| s.1), Some(true)); // \S-+ key charset
        assert_eq!(shape(":a:b: v\n"), Some((3, true))); // interior colon in key
        assert_eq!(shape(":KEY:value\n"), None); // value must follow whitespace
        assert_eq!(shape(":KEY : v\n"), None); // space inside key run
        assert_eq!(shape(":: v\n"), None); // empty key
        assert_eq!(shape("KEY: v\n"), None);
        assert_eq!(shape("\n"), None);
    }

    #[test]
    fn probe_property_validates_whole_drawer() {
        assert!(probe_line(":PROPERTIES:\n:A: 1\n:END:\n", probe_property).is_some());
        assert!(probe_line(":properties:\n:END:\n", probe_property).is_some()); // empty + caseless
        assert!(probe_line(":PROPERTIES:\n:A: 1\n:B+:\n:END:\n", probe_property).is_some());
        // Bad interior line: not a property drawer (generic drawer instead).
        assert!(probe_line(":PROPERTIES:\ntext\n:END:\n", probe_property).is_none());
        // Blank interior line breaks the whole-drawer match.
        assert!(probe_line(":PROPERTIES:\n\n:END:\n", probe_property).is_none());
        // Unterminated.
        assert!(probe_line(":PROPERTIES:\n:A: 1\n", probe_property).is_none());
        // Wrong name.
        assert!(probe_line(":PROPS:\n:A: 1\n:END:\n", probe_property).is_none());

        let empty = probe_line(":properties:\n:END:\n", probe_property).expect("empty");
        assert!(empty.interiors.is_empty());
        let two = probe_line(":PROPERTIES:\n:A: 1\n:B+:\n:END:\n", probe_property).expect("two");
        assert_eq!(two.interiors.len(), 2);
    }

    #[test]
    fn property_drawer_after_heading() {
        let src = "* H\n:PROPERTIES:\n:ID: abc\n:CUSTOM_ID: xyz\n:END:\nbody\n";
        let root = parse_document_tree(src);
        let outline = first_child_of_kind(&root, OrgSyntaxKind::OUTLINE).expect("outline");
        let pd = first_child_of_kind(&outline, OrgSyntaxKind::PROPERTY_DRAWER)
            .expect("property drawer under OUTLINE");
        let props = nodes_of_kind(&pd, OrgSyntaxKind::NODE_PROPERTY);
        assert_eq!(props.len(), 2);
        assert_eq!(
            tree_text(&nodes_of_kind(&props[0], OrgSyntaxKind::NODE_PROPERTY_KEY)[0]),
            "ID"
        );
        assert_eq!(
            tree_text(&nodes_of_kind(&props[0], OrgSyntaxKind::NODE_PROPERTY_VALUE)[0]),
            "abc"
        );
        assert_eq!(
            tree_text(&nodes_of_kind(&props[1], OrgSyntaxKind::NODE_PROPERTY_KEY)[0]),
            "CUSTOM_ID"
        );
        // Body is SECTION content, not drawer content.
        let section = first_child_of_kind(&outline, OrgSyntaxKind::SECTION).expect("section");
        assert_eq!(tree_text(&section), "body\n");
    }

    #[test]
    fn property_drawer_after_planning_with_blanks() {
        // Blanks between planning and property drawer are tolerated
        // (org planning post-blank; mode persists).
        let src = "* H\nSCHEDULED: <2020-01-01 Wed>\n\n:PROPERTIES:\n:A: 1\n:END:\n";
        let root = parse_document_tree(src);
        let outline = first_child_of_kind(&root, OrgSyntaxKind::OUTLINE).expect("outline");
        assert!(first_child_of_kind(&outline, OrgSyntaxKind::PLANNING).is_some());
        assert!(
            first_child_of_kind(&outline, OrgSyntaxKind::PROPERTY_DRAWER).is_some(),
            "property drawer after planning + blanks"
        );
    }

    #[test]
    fn blank_after_heading_blocks_property_drawer() {
        // No planning: the drawer must be adjacent to the heading.
        let src = "* H\n\n:PROPERTIES:\n:A: 1\n:END:\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            0,
            "non-adjacent :PROPERTIES: is not a property drawer"
        );
        // It is a generic drawer in the section instead.
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::DRAWER).len(), 1);
    }

    #[test]
    fn broken_property_drawer_is_generic_drawer() {
        // An interior non-property line fails probe_property; the generic
        // drawer grammar takes the region (org parity).
        let src = "* H\n:PROPERTIES:\nnot a property\n:END:\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            0
        );
        let drawers = nodes_of_kind(&root, OrgSyntaxKind::DRAWER);
        assert_eq!(drawers.len(), 1);
        assert!(tree_text(&drawers[0]).contains("PROPERTIES"));
    }

    #[test]
    fn zeroth_section_property_drawer() {
        // Directly at file start.
        let src = ":PROPERTIES:\n:ID: root\n:END:\npara\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            1
        );

        // After leading comments (org: BEGINNING-OF-FILE BLANK-LINES COMMENT
        // PROPERTYDRAWER).
        let src2 = "# comment\n:PROPERTIES:\n:ID: root\n:END:\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            1
        );

        // With blank lines before the comment.
        let src3 = "\n\n# c\n\n:PROPERTIES:\n:A: 1\n:END:\n";
        let root3 = parse_document_tree(src3);
        assert_eq!(tree_text(&root3), src3);
        assert_eq!(
            nodes_of_kind(&root3, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            1
        );

        // A paragraph before it means no zeroth property drawer.
        let src4 = "para\n:PROPERTIES:\n:A: 1\n:END:\n";
        let root4 = parse_document_tree(src4);
        assert_eq!(tree_text(&root4), src4);
        assert_eq!(
            nodes_of_kind(&root4, OrgSyntaxKind::PROPERTY_DRAWER).len(),
            0
        );
        assert_eq!(nodes_of_kind(&root4, OrgSyntaxKind::DRAWER).len(), 1);
    }

    #[test]
    fn property_drawer_value_trims_trailing_whitespace() {
        let src = "* H\n:PROPERTIES:\n:K: spaced value   \n:END:\n";
        let root = parse_document_tree(src);
        let values = nodes_of_kind(&root, OrgSyntaxKind::NODE_PROPERTY_VALUE);
        assert_eq!(values.len(), 1);
        assert_eq!(tree_text(&values[0]), "spaced value");
    }

    /// Shape fixtures: `(source, DRAWER count, PROPERTY_DRAWER count)`.
    ///
    /// Losslessness is not asserted: `builder::build` already checks gap-free
    /// tiling on every parse, in release builds too. These fixtures pin which
    /// opener the walk claimed and where it closed, which the builder cannot
    /// see.
    const SHAPE_FIXTURES: &[(&str, usize, usize)] = &[
        (":D:\n:END:\n", 1, 0),
        (":D:\n:END:", 1, 0), // no trailing newline
        ("* H\n:PROPERTIES:\n:A: 1\n:END:\n* H2\n", 0, 1),
        (
            "* H\nSCHEDULED: <2020-01-01>\n:PROPERTIES:\n:A: 1\n:END:\nbody\n",
            0,
            1,
        ),
        (":a:\ntext\n\ntext2\n:end:\nafter\n", 1, 0), // caseless end, blank inside
        ("para\n:D:\nx\n:END:\n", 1, 0),
        (":D:\r\nx\r\n:END:\r\n", 1, 0), // CRLF
    ];

    #[test]
    fn tiling_fixtures_produce_expected_drawer_shape() {
        for &(src, drawers, property_drawers) in SHAPE_FIXTURES {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::DRAWER).len(),
                drawers,
                "DRAWER count for {src:?}"
            );
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::PROPERTY_DRAWER).len(),
                property_drawers,
                "PROPERTY_DRAWER count for {src:?}"
            );
        }
    }
}
