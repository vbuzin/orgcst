//! Text-damage reuse at outline granularity.
//!
//! One touched span, from the common prefix and suffix. The reparse unit is
//! the nearest enclosing outline, or the zeroth section before the first
//! outline. A heading-structure change widens to a shallower ancestor. A
//! fragment that does not end on the mapped boundary widens to the parent
//! outline, then the cold path.
//!
//! Name-set and structural-knob config changes are not handled here; a
//! producer handles them as a separate hop.

use super::{intersects, shift_diagnostic};
use crate::ast::{AstNode, Outline};
use crate::document::Document;
use crate::parser::grammar::drive::document::{parse_outline_fragment, parse_zeroth};
use crate::parser::grammar::forms::heading::star_head;
use crate::parser::{Diagnostic, Event, ParseConfig, Parser};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use rowan::{GreenNode, TextRange, TextSize};

pub(super) fn reuse(
    text: &str,
    config: &ParseConfig,
    prior: &Document,
) -> Option<(GreenNode, Vec<Diagnostic>)> {
    let old = prior.source();
    let (old_span, new_span) = touched(old, text)?;
    let mut unit = covering_unit(prior.root(), old_span)?;
    if let Some(min_level) = heading_change(old, text, old_span, new_span, config) {
        unit = stable_ancestor(unit, min_level)?;
    }

    let delta = i64::try_from(text.len()).ok()? - i64::try_from(old.len()).ok()?;
    loop {
        let old_range = unit.text_range();
        if let Some((green, fragment_diags)) = reparse(&unit, old.len(), text, config) {
            let diagnostics =
                merge_diagnostics(prior.diagnostics(), old_range, fragment_diags, delta)?;
            let root = unit.replace_with(green);
            if usize::from(root.text_len()) != text.len() {
                return None;
            }
            debug_assert_eq!(SyntaxNode::new_root(root.clone()).text(), text);
            return Some((root, diagnostics));
        }
        unit = parent_outline(&unit)?;
    }
}

fn touched(old: &str, new: &str) -> Option<(TextRange, TextRange)> {
    let prefix = common_prefix(old, new);
    let suffix = common_suffix(old, new, prefix);
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    Some((
        TextRange::new(text_size(prefix)?, text_size(old_end)?),
        TextRange::new(text_size(prefix)?, text_size(new_end)?),
    ))
}

fn common_prefix(old: &str, new: &str) -> usize {
    let old_b = old.as_bytes();
    let new_b = new.as_bytes();
    let mut i = 0;
    let max = old_b.len().min(new_b.len());
    while i < max && old_b[i] == new_b[i] {
        i += 1;
    }
    while i > 0 && !old.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn common_suffix(old: &str, new: &str, prefix: usize) -> usize {
    let old_b = old.as_bytes();
    let new_b = new.as_bytes();
    let max = old_b.len().min(new_b.len()) - prefix;
    let mut suffix = 0;
    while suffix < max && old_b[old_b.len() - 1 - suffix] == new_b[new_b.len() - 1 - suffix] {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    suffix
}

fn text_size(n: usize) -> Option<TextSize> {
    u32::try_from(n).ok().map(TextSize::from)
}

fn covering_unit(root: &SyntaxNode, span: TextRange) -> Option<SyntaxNode> {
    let mut best: Option<SyntaxNode> = None;
    consider_units(root, span, &mut best);
    best
}

fn consider_units(node: &SyntaxNode, span: TextRange, best: &mut Option<SyntaxNode>) {
    for child in node.children() {
        if child.kind() == OrgSyntaxKind::OUTLINE {
            if covers(child.text_range(), span) {
                prefer(best, child.clone());
                consider_units(&child, span, best);
            }
        } else if child.kind() == OrgSyntaxKind::SECTION
            && node.kind() == OrgSyntaxKind::DOCUMENT
            && covers(child.text_range(), span)
        {
            prefer(best, child);
        }
    }
}

fn covers(range: TextRange, span: TextRange) -> bool {
    !range.is_empty() && range.start() <= span.start() && span.end() <= range.end()
}

fn prefer(best: &mut Option<SyntaxNode>, candidate: SyntaxNode) {
    let replace = match best {
        None => true,
        Some(current) => {
            let next = candidate.text_range();
            let have = current.text_range();
            next.len() < have.len() || (next.len() == have.len() && next.start() >= have.start())
        }
    };
    if replace {
        *best = Some(candidate);
    }
}

/// Minimum star level when the touched lines were added, removed, or re-levelled.
fn heading_change(
    old: &str,
    new: &str,
    old_span: TextRange,
    new_span: TextRange,
    config: &ParseConfig,
) -> Option<usize> {
    let old_levels = star_levels(old, old_span, config);
    let new_levels = star_levels(new, new_span, config);
    if old_levels == new_levels {
        return None;
    }
    old_levels.into_iter().chain(new_levels).min()
}

fn star_levels(src: &str, span: TextRange, config: &ParseConfig) -> Vec<usize> {
    let start = usize::from(span.start());
    let end = usize::from(span.end());
    if start >= end || start > src.len() {
        return Vec::new();
    }
    let end = end.min(src.len());
    let bytes = src.as_bytes();
    let mut line_start = start.min(bytes.len());
    while line_start > 0 && !at_line_start(bytes, line_start) {
        line_start -= 1;
    }
    let mut levels = Vec::new();
    while line_start < end && line_start < bytes.len() {
        let next = line_end(bytes, line_start);
        // One lex per touched line so the scan is `star_head`, not a second
        // star rule.
        if bytes[line_start] == b'*'
            && let Some(level) = star_level(&src[line_start..next], config)
        {
            levels.push(level);
        }
        if next <= line_start {
            break;
        }
        line_start = next;
    }
    levels
}

/// `index` is the first byte of a line.
///
/// The lexer newline is `\n`, `\r\n`, or a lone `\r`. A `\r` followed by
/// `\n` is one terminator, so the next line starts after the `\n`.
fn at_line_start(bytes: &[u8], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    match bytes[index - 1] {
        b'\n' => true,
        b'\r' => bytes.get(index) != Some(&b'\n'),
        _ => false,
    }
}

fn line_end(bytes: &[u8], line_start: usize) -> usize {
    let mut i = line_start;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            return i + 1;
        }
        if bytes[i] == b'\r' {
            return if bytes.get(i + 1) == Some(&b'\n') {
                i + 2
            } else {
                i + 1
            };
        }
        i += 1;
    }
    bytes.len()
}

fn star_level(line: &str, config: &ParseConfig) -> Option<usize> {
    let parser = Parser::with_config(line, config);
    star_head(parser.line()?)
}

fn stable_ancestor(mut node: SyntaxNode, min_level: usize) -> Option<SyntaxNode> {
    loop {
        if node.kind() == OrgSyntaxKind::DOCUMENT {
            return None;
        }
        if node.kind() == OrgSyntaxKind::OUTLINE
            && Outline::cast(node.clone()).is_some_and(|outline| outline.level() < min_level)
        {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn parent_outline(node: &SyntaxNode) -> Option<SyntaxNode> {
    let parent = node.parent()?;
    (parent.kind() == OrgSyntaxKind::OUTLINE).then_some(parent)
}

fn ancestor_depth(node: &SyntaxNode) -> Option<u16> {
    let mut depth = 0u16;
    let mut parent = node.parent();
    while let Some(node) = parent {
        if node.kind() == OrgSyntaxKind::OUTLINE {
            depth = depth.checked_add(1)?;
        }
        parent = node.parent();
    }
    Some(depth)
}

fn reparse(
    unit: &SyntaxNode,
    old_len: usize,
    text: &str,
    config: &ParseConfig,
) -> Option<(GreenNode, Vec<Diagnostic>)> {
    let kind = unit.kind();
    if kind != OrgSyntaxKind::OUTLINE && kind != OrgSyntaxKind::SECTION {
        return None;
    }
    let (start, len) = fragment_bounds(unit.text_range(), old_len, text.len())?;
    if start > text.len() || !text.is_char_boundary(start) || start + len > text.len() {
        return None;
    }
    let end = start + len;
    if !text.is_char_boundary(end) {
        return None;
    }
    // A blank line at the start of the zeroth section is a file lead-in, not
    // section content. The document parser hoists it; a section node cannot.
    if kind == OrgSyntaxKind::SECTION && starts_with_blank_line(&text[start..end]) {
        return None;
    }

    let suffix = &text[start..];
    let mut parser = Parser::with_config(suffix, config);
    // Outline and the zeroth section sit outside `elements_in`. The region
    // counter starts at 0; outline nesting starts at the ancestor count.
    parser.set_region_nesting(0);
    let depth = if kind == OrgSyntaxKind::OUTLINE {
        ancestor_depth(unit)?
    } else {
        0
    };
    parser.set_outline_nesting(depth);
    // The suffix runs through EOF so a closer past the boundary is visible.
    // Accepting the slice alone would clip that construct.
    let parsed = if kind == OrgSyntaxKind::OUTLINE {
        parse_outline_fragment(&mut parser)
    } else {
        parse_zeroth(&mut parser);
        true
    };
    if !parsed {
        return None;
    }
    debug_assert_eq!(parser.region_nesting(), 0);
    debug_assert_eq!(parser.outline_nesting(), depth);

    let consumed = parser.source_start_here().unwrap_or(suffix.len());
    if consumed != len {
        return None;
    }
    let (mut tokens, events, diagnostics) = parser.finish();
    if events.iter().any(|event| match event {
        Event::Token { range, .. } => usize::from(range.end()) > len,
        _ => false,
    }) {
        return None;
    }
    if tokens.iter().any(|tok| {
        let tok_start = usize::from(tok.range.start());
        let tok_end = usize::from(tok.range.end());
        tok_start < len && tok_end > len
    }) {
        return None;
    }
    tokens.retain(|tok| usize::from(tok.range.end()) <= len);
    if tokens
        .last()
        .is_none_or(|tok| usize::from(tok.range.end()) != len)
    {
        return None;
    }
    let green = crate::parser::builder::build(&suffix[..len], &tokens, events);
    if green.kind() != kind.into() || usize::from(green.text_len()) != len {
        return None;
    }
    // A diagnostic past the fragment means the grammar recorded a span the
    // slice does not own. That is a leak, not a range to clip.
    if diagnostics
        .iter()
        .any(|diag| usize::from(diag.range().end()) > len)
    {
        return None;
    }
    let shifted = diagnostics
        .into_iter()
        .map(|diag| shift_diagnostic(&diag, i64::from(u32::from(unit.text_range().start()))))
        .collect::<Option<Vec<_>>>()?;
    Some((green, shifted))
}

fn fragment_bounds(unit: TextRange, old_len: usize, new_len: usize) -> Option<(usize, usize)> {
    let start = usize::from(unit.start());
    let old_end = i64::from(u32::from(unit.end()));
    let delta = i64::try_from(new_len).ok()? - i64::try_from(old_len).ok()?;
    let new_end = usize::try_from(old_end.checked_add(delta)?).ok()?;
    if start > new_len || new_end > new_len || new_end <= start {
        return None;
    }
    Some((start, new_end - start))
}

fn starts_with_blank_line(src: &str) -> bool {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i == bytes.len() {
        return false;
    }
    // Same terminator as the lexer: `\n`, `\r\n`, or a lone `\r`.
    bytes[i] == b'\n' || bytes[i] == b'\r'
}

fn merge_diagnostics(
    prior: &[Diagnostic],
    old_unit: TextRange,
    fragment: Vec<Diagnostic>,
    delta: i64,
) -> Option<Vec<Diagnostic>> {
    let mut merged = Vec::new();
    let mut inserted = false;
    for diag in prior {
        if intersects(old_unit, diag.range()) {
            continue;
        }
        let after = diag.range().start() >= old_unit.end();
        if after && !inserted {
            merged.extend(fragment.iter().cloned());
            inserted = true;
        }
        if after {
            merged.push(shift_diagnostic(diag, delta)?);
        } else {
            merged.push(diag.clone());
        }
    }
    if !inserted {
        merged.extend(fragment);
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{DiagnosticKind, TodoKind};
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::{EditSet, ParseConfig, parse, parse_with};
    use rowan::TextSize;
    use std::sync::Arc;

    fn materialize(src: &str, config: ParseConfig, prior: Option<&Document>) -> Document {
        crate::document::materialize(src, Arc::new(config), prior)
    }

    fn cold_like(warm: &Document) -> Document {
        materialize(warm.source(), warm.config().clone(), None)
    }

    fn assert_matches_cold(warm: &Document) {
        let cold = cold_like(warm);
        assert_eq!(warm.root().green(), cold.root().green(), "tree");
        assert_eq!(warm.diagnostics(), cold.diagnostics(), "diagnostics");
    }

    fn green_eq(a: &SyntaxNode, b: &SyntaxNode) -> bool {
        std::ptr::eq(a.green().as_ref(), b.green().as_ref())
    }

    fn first_newline_parent(doc: &Document) -> OrgSyntaxKind {
        fn walk(node: &SyntaxNode) -> Option<OrgSyntaxKind> {
            for el in node.children_with_tokens() {
                match el {
                    rowan::NodeOrToken::Token(tok) if tok.kind() == OrgSyntaxKind::NEWLINE => {
                        return Some(tok.parent().expect("newline parent").kind());
                    }
                    rowan::NodeOrToken::Node(child) => {
                        if let Some(kind) = walk(&child) {
                            return Some(kind);
                        }
                    }
                    rowan::NodeOrToken::Token(_) => {}
                }
            }
            None
        }
        walk(doc.root()).expect("newline")
    }

    fn outline(doc: &Document, heading: &str) -> SyntaxNode {
        nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE)
            .into_iter()
            .find(|node| {
                node.children().any(|child| {
                    child.kind() == OrgSyntaxKind::HEADING
                        && child.text().to_string().starts_with(heading)
                })
            })
            .unwrap_or_else(|| panic!("missing outline {heading}"))
    }

    fn replace(src: &str, from: &str, to: &str) -> String {
        let start = src.find(from).unwrap_or_else(|| panic!("missing {from:?}"));
        let mut out = String::with_capacity(src.len() - from.len() + to.len());
        out.push_str(&src[..start]);
        out.push_str(to);
        out.push_str(&src[start + from.len()..]);
        out
    }

    fn edit_set(src: &str, from: &str, to: &str) -> EditSet {
        let start = src.find(from).unwrap_or_else(|| panic!("missing {from:?}"));
        let range = TextRange::new(
            TextSize::try_from(start).expect("start fits"),
            TextSize::try_from(start + from.len()).expect("end fits"),
        );
        EditSet::single(src, range, to).expect("edit")
    }

    #[test]
    fn body_edit_reuses_only_the_other_outline() {
        // Blank line so the block is an element, not paragraph text. An
        // underscore opener does not interrupt a paragraph.
        let src = "intro\n* A\nalpha\n\n#+begin_src rust\nx\n* B\nCLOCK: not-a-stamp\nmore\n";
        let prior = parse(src);
        assert!(
            prior
                .diagnostics()
                .iter()
                .any(|diag| { matches!(diag.kind(), DiagnosticKind::UnterminatedBlock { .. }) }),
            "{:?}",
            prior.diagnostics()
        );
        assert!(
            prior
                .diagnostics()
                .iter()
                .any(|diag| matches!(diag.kind(), DiagnosticKind::MalformedClock))
        );

        let edited_a = replace(src, "alpha", "alpha!");
        let (warm_a, trees_a) = crate::document::tree_builds::while_running(|| {
            materialize(&edited_a, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees_a, 0);
        assert_matches_cold(&warm_a);
        assert!(green_eq(
            &outline(&prior, "* B\n"),
            &outline(&warm_a, "* B\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm_a, "* A\n")
        ));
        let prior_clock = prior
            .diagnostics()
            .iter()
            .find(|diag| matches!(diag.kind(), DiagnosticKind::MalformedClock))
            .expect("clock");
        let warm_clock = warm_a
            .diagnostics()
            .iter()
            .find(|diag| matches!(diag.kind(), DiagnosticKind::MalformedClock))
            .expect("clock");
        assert_ne!(prior_clock.range(), warm_clock.range(), "later diag shifts");

        let edited_b = replace(src, "more", "more!");
        let (warm_b, trees_b) = crate::document::tree_builds::while_running(|| {
            materialize(&edited_b, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees_b, 0);
        assert_matches_cold(&warm_b);
        assert!(green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm_b, "* A\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* B\n"),
            &outline(&warm_b, "* B\n")
        ));
    }

    #[test]
    fn deeper_heading_stays_inside_the_nested_outline() {
        // `tail` does not start with stars, so the common prefix stops at the
        // inserted heading instead of eating the next outline's star run.
        let src = "* A\n** B\nbody\ntail\n** B2\nkeep\n* C\nrest\n";
        let prior = parse(src);
        let edited = replace(src, "body", "body\n*** D");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees, 0);
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "** B2\n"),
            &outline(&warm, "** B2\n")
        ));
        assert!(green_eq(
            &outline(&prior, "* C\n"),
            &outline(&warm, "* C\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "** B\n"),
            &outline(&warm, "** B\n")
        ));
    }

    #[test]
    fn same_level_heading_widens_and_shares_the_top_level_sibling() {
        let src = "* A\n** B\nbody\n** B2\nkeep\n* C\nrest\n";
        let prior = parse(src);
        let edited = replace(src, "body", "body\n** S");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees, 0);
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "* C\n"),
            &outline(&warm, "* C\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm, "* A\n")
        ));
    }

    #[test]
    fn top_level_heading_insertion_is_cold() {
        let src = "* A\nbody\n* C\nrest\n";
        let prior = parse(src);
        let edited = replace(src, "body", "body\n* N");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(
            trees, 1,
            "no ancestor outline can contain a new level-1 heading"
        );
        assert_matches_cold(&warm);
    }

    #[test]
    fn zeroth_section_edit_reuses_later_outlines() {
        let src = "para\n* A\nrest\n* B\nmore\n";
        let prior = parse(src);
        let edited = replace(src, "para", "para!");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees, 0);
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm, "* A\n")
        ));
        assert!(green_eq(
            &outline(&prior, "* B\n"),
            &outline(&warm, "* B\n")
        ));
    }

    #[test]
    fn opened_drawer_reuses_the_other_outline() {
        let src = "* A\nnote\n* B\nkept\n";
        let prior = parse(src);
        let edited = replace(src, "note", ":LOGBOOK:\nnote");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees, 0, "an unclosed drawer stops at the next heading");
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "* B\n"),
            &outline(&warm, "* B\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm, "* A\n")
        ));
    }

    #[test]
    fn deleted_top_level_block_closer_is_cold() {
        let src = "* A\n#+begin_src\ncode\n#+end_src\n* B\n#+begin_src\ninner\n#+end_src\n";
        let prior = parse(src);
        let edited = replace(src, "#+end_src\n", "");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(
            trees, 1,
            "the block runs into the next outline and that outline has no parent"
        );
        assert_matches_cold(&warm);
    }

    #[test]
    fn deleted_inner_block_closer_retries_the_parent_outline() {
        let src = "\
* A
** B
#+begin_src
code
#+end_src
** C
#+begin_src
inner
#+end_src
* D
other
";
        let prior = parse(src);
        let edited = replace(src, "#+end_src\n", "");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(
            trees, 0,
            "the inner fragment leaks; the parent outline holds"
        );
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "* D\n"),
            &outline(&warm, "* D\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm, "* A\n")
        ));
    }

    #[test]
    fn zeroth_section_cr_lead_in_is_cold() {
        let src = "para\n* A\nrest\n";
        let prior = parse(src);
        for lead in ["\r", " \r"] {
            let set = EditSet::single(src, TextRange::empty(TextSize::from(0)), lead)
                .expect("insert lead-in");
            let (warm, trees) = crate::document::tree_builds::while_running(|| prior.apply(&set));
            assert_eq!(trees, 1, "{lead:?} is a file lead-in, not section content");
            assert_eq!(warm.source(), format!("{lead}{src}"));
            assert_matches_cold(&warm);
            assert_eq!(
                first_newline_parent(&warm),
                OrgSyntaxKind::DOCUMENT,
                "{lead:?}"
            );
        }
    }

    #[test]
    fn cr_only_relevel_widens_to_the_shallower_ancestor() {
        let src = "* A\r** B\r*** C\rc-body\r** D\r";
        let prior = parse(src);
        let edited = replace(src, "*** C", "** X");
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize(&edited, ParseConfig::default(), Some(&prior))
        });
        assert_eq!(trees, 0);
        assert_matches_cold(&warm);
        let parent = outline(&warm, "** X")
            .parent()
            .expect("** X sits under an outline");
        assert!(
            parent.children().any(|child| {
                child.kind() == OrgSyntaxKind::HEADING
                    && child.text().to_string().starts_with("* A")
            }),
            "parent of ** X is * A"
        );
    }

    #[test]
    fn todo_edit_recarves_a_heading_outside_the_text_unit() {
        let src = "#+TODO: TODO NEXT | DONE\n* TODO stays\nalpha\n* NEXT flips\nbeta\n";
        let prior = parse(src);
        let set = edit_set(src, "#+TODO: TODO NEXT | DONE\n", "#+TODO: TODO | DONE\n");
        let ((warm, carved), trees) = crate::document::tree_builds::while_running(|| {
            crate::damage::recarves_while(|| prior.apply(&set))
        });
        assert_eq!(trees, 0, "outline hop then star-line recarve");
        assert_eq!(carved, 1, "NEXT leaves the set; TODO is in both");
        assert_matches_cold(&warm);
        assert!(
            green_eq(
                &outline(&prior, "* TODO stays\n"),
                &outline(&warm, "* TODO stays\n")
            ),
            "a word in both name sets keeps the prior green"
        );
        assert!(
            !green_eq(
                &outline(&prior, "* NEXT flips\n"),
                &outline(&warm, "* NEXT flips\n")
            ),
            "a heading outside the text unit is recarved when its word leaves the set"
        );
    }

    #[test]
    fn name_set_change_with_a_text_change_is_cold() {
        let src = "* TODO a\n* NEXT b\n";
        let prior = materialize(src, ParseConfig::default(), None);
        let next = ParseConfig::default().with_todo_keywords([("BUG", TodoKind::Active)]);
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize("* TODO a!\n* NEXT b\n", next.clone(), Some(&prior))
        });
        assert_eq!(trees, 1);
        assert_matches_cold(&warm);
        assert_eq!(warm.config().todo_kind("BUG"), Some(TodoKind::Active));
    }

    #[test]
    fn sequence_only_config_with_a_text_change_reuses() {
        let src = "* A\nkeep\n* B\nstay\n";
        let prior = materialize(src, ParseConfig::default(), None);
        let swapped = ParseConfig::default()
            .with_todo_keywords([("TODO", TodoKind::Done), ("DONE", TodoKind::Active)]);
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize("* A\nkept\n* B\nstay\n", swapped, Some(&prior))
        });
        assert_eq!(trees, 0);
        assert_eq!(warm.config().todo_kind("TODO"), Some(TodoKind::Done));
        assert_matches_cold(&warm);
        assert!(green_eq(
            &outline(&prior, "* B\n"),
            &outline(&warm, "* B\n")
        ));
        assert!(!green_eq(
            &outline(&prior, "* A\n"),
            &outline(&warm, "* A\n")
        ));
    }

    #[test]
    fn caller_structural_text_change_is_cold() {
        let src = "* A\nkeep\n* B\nstay\n";
        let prior = parse(src);
        let next = ParseConfig::default().with_max_nesting_depth(4);
        let (warm, trees) = crate::document::tree_builds::while_running(|| {
            materialize("* A\nkept\n* B\nstay\n", next, Some(&prior))
        });
        assert_eq!(trees, 1);
        assert_matches_cold(&warm);
    }

    #[test]
    fn tight_outline_budget_does_not_nest_a_reparented_sibling() {
        let cfg = ParseConfig::default().with_max_nesting_depth(2);
        let src = "* A\n** B\n*** C\nbody\n**** D\n* E\n";
        let prior = parse_with(src, &cfg);
        let edited = replace(src, "body", "body!");
        let (warm, trees) =
            crate::document::tree_builds::while_running(|| materialize(&edited, cfg, Some(&prior)));
        assert_eq!(trees, 0);
        assert_matches_cold(&warm);
        assert!(!green_eq(
            &outline(&prior, "*** C\n"),
            &outline(&warm, "*** C\n")
        ));
        assert!(green_eq(
            &outline(&prior, "**** D\n"),
            &outline(&warm, "**** D\n")
        ));
        assert!(green_eq(
            &outline(&prior, "* E\n"),
            &outline(&warm, "* E\n")
        ));
    }

    /// Four headings in the `benches/baseline.rs` template. The bench cannot
    /// see `tree_builds`; a cold count here means that setup is wrong.
    #[test]
    fn baseline_template_edits_are_not_cold() {
        use std::fmt::Write;

        let body = "body with *bold* and [[https://example.com][link]].\n";
        let mut todos = String::from("#+TODO: TODO | DONE\n");
        let mut nexts = String::from("#+TODO: TODO NEXT | DONE\n");
        for index in 0..4 {
            write!(todos, "* TODO [#A] Item {index} :tag:\n{body}")
                .expect("writing into a String cannot fail");
            let keyword = if index == 2 { "NEXT" } else { "TODO" };
            write!(nexts, "* {keyword} [#A] Item {index} :tag:\n{body}")
                .expect("writing into a String cannot fail");
        }

        let prior = parse(&todos);
        let mut search = 0;
        let mut at = 0;
        for _ in 0..=2 {
            let rel = todos[search..]
                .find("with")
                .expect("each body contains the replaced word");
            at = search + rel;
            search = at + "with".len();
        }
        let range = TextRange::new(
            TextSize::try_from(at).expect("start fits"),
            TextSize::try_from(at + "with".len()).expect("end fits"),
        );
        let body_edit = EditSet::single(&todos, range, "plus").expect("body word");
        let (warm_body, trees_body) =
            crate::document::tree_builds::while_running(|| prior.apply(&body_edit));
        assert_eq!(trees_body, 0, "one body word stays on the outline path");
        assert_matches_cold(&warm_body);
        assert!(green_eq(
            &outline(&prior, "* TODO [#A] Item 0"),
            &outline(&warm_body, "* TODO [#A] Item 0")
        ));
        assert!(green_eq(
            &outline(&prior, "* TODO [#A] Item 1"),
            &outline(&warm_body, "* TODO [#A] Item 1")
        ));
        assert!(!green_eq(
            &outline(&prior, "* TODO [#A] Item 2"),
            &outline(&warm_body, "* TODO [#A] Item 2")
        ));
        assert!(green_eq(
            &outline(&prior, "* TODO [#A] Item 3"),
            &outline(&warm_body, "* TODO [#A] Item 3")
        ));

        let next_prior = parse(&nexts);
        let drop_next = edit_set(
            &nexts,
            "#+TODO: TODO NEXT | DONE\n",
            "#+TODO: TODO | DONE\n",
        );
        let ((warm_one, carved_one), trees_one) =
            crate::document::tree_builds::while_running(|| {
                crate::damage::recarves_while(|| next_prior.apply(&drop_next))
            });
        assert_eq!(trees_one, 0, "keyword line, then one star-line recarve");
        assert_eq!(carved_one, 1, "only NEXT leaves the set");
        assert_matches_cold(&warm_one);
        assert!(green_eq(
            &outline(&next_prior, "* TODO [#A] Item 0"),
            &outline(&warm_one, "* TODO [#A] Item 0")
        ));
        assert!(green_eq(
            &outline(&next_prior, "* TODO [#A] Item 1"),
            &outline(&warm_one, "* TODO [#A] Item 1")
        ));
        assert!(!green_eq(
            &outline(&next_prior, "* NEXT [#A] Item 2"),
            &outline(&warm_one, "* NEXT [#A] Item 2")
        ));
        assert!(green_eq(
            &outline(&next_prior, "* TODO [#A] Item 3"),
            &outline(&warm_one, "* TODO [#A] Item 3")
        ));

        let rename = edit_set(&todos, "#+TODO: TODO | DONE\n", "#+TODO: TASK | DONE\n");
        let ((warm_all, carved_all), trees_all) =
            crate::document::tree_builds::while_running(|| {
                crate::damage::recarves_while(|| prior.apply(&rename))
            });
        assert_eq!(trees_all, 0, "worst-case recarve is still not cold");
        assert_eq!(carved_all, 4, "every TODO heading leaves the set");
        assert_matches_cold(&warm_all);
    }
}
