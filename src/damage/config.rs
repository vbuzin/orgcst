//! File-layer TODO damage: which star lines to recarve, and how diagnostics merge.
//!
//! The candidate is the leading non-terminator run `parse_todo` looks up.
//! Bodies are not re-lexed.

use super::{intersects, reused_parts, shift_diagnostic, span_key, splice_replacements};
use crate::ast::Headline;
use crate::document::Document;
use crate::parser::grammar::forms::heading::recarve_star_line;
use crate::parser::{Diagnostic, ParseConfig, TodoKeywordDelta};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use rowan::{GreenNode, TextRange};
use std::collections::BTreeMap;

struct DamagedLine {
    range: TextRange,
    /// `elements_in` frames already open when this star line was carved.
    /// Headings are 0. The title's own `inline_in` is not included.
    nesting: u16,
}

pub(super) fn recarve(
    prior: &Document,
    config: &ParseConfig,
    names: &TodoKeywordDelta,
) -> Option<(GreenNode, Vec<Diagnostic>)> {
    let damaged = damaged_lines(prior, &names.old, &names.new);
    if damaged.is_empty() {
        return Some(reused_parts(prior));
    }

    let mut replacements = BTreeMap::new();
    let mut fresh_lines = Vec::with_capacity(damaged.len());
    for line in &damaged {
        #[cfg(test)]
        super::recarve_count::record();
        let slice = prior.slice(line.range);
        let (green, diags) = recarve_star_line(slice, config, line.nesting);
        let mut shifted = Vec::with_capacity(diags.len());
        for diag in diags {
            shifted.push(shift_diagnostic(
                &diag,
                i64::from(u32::from(line.range.start())),
            )?);
        }
        replacements.insert(span_key(line.range), green);
        fresh_lines.push((line.range, shifted));
    }

    let green = splice_replacements(prior.root().green().into_owned(), &replacements)?;
    Some((green, merge_diagnostics(prior.diagnostics(), &fresh_lines)))
}

fn damaged_lines(
    prior: &Document,
    old: &std::collections::BTreeSet<String>,
    new: &std::collections::BTreeSet<String>,
) -> Vec<DamagedLine> {
    let mut out = Vec::new();
    for task in prior.tasks() {
        let line = task.star_line_range();
        let Some(word) = todo_candidate(prior.slice(line)) else {
            continue;
        };
        if old.contains(word) == new.contains(word) {
            continue;
        }
        let syntax = task.star_syntax();
        out.push(DamagedLine {
            range: line,
            nesting: region_nesting_at_star_line(&syntax),
        });
    }
    out
}

/// Leading non-terminator run `parse_todo` looks up.
///
/// After the stars and the post-star space or tab run, bytes until space,
/// tab, or newline. A carved `PRIORITY` can be a proper prefix of that run
/// (`*[#A]GLUED`); `post_stars_word_range` stops at the node and would miss it.
fn todo_candidate(star_line: &str) -> Option<&str> {
    let bytes = star_line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b'*' {
        i += 1;
    }
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    let start = i;
    while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    (start < i).then_some(&star_line[start..i])
}

/// Region nesting already charged when this star line's title was parsed.
///
/// Headings are carved on the outline spine, which charges `outline_nesting`
/// rather than the region counter, so they pass 0. An inline task passes the
/// number of enclosing `elements_in` frames: `ITEM` (`forms/list.rs`), a
/// parent `INLINE_TASK` body (`forms/heading.rs`, after the opener), and
/// `BodyMode::Elements` frames (`drive/frame.rs`: drawer, footnote definition,
/// quote / center / dynamic / special via `block_body_mode`). `PLAIN_LIST`
/// and section bodies do not charge. The title's own `inline_in` runs inside
/// `emit_star_line_anatomy` and is not part of this count.
fn region_nesting_at_star_line(syntax: &SyntaxNode) -> u16 {
    if syntax.kind() != OrgSyntaxKind::INLINE_TASK {
        return 0;
    }
    let mut depth = 0u16;
    let mut parent = syntax.parent();
    while let Some(node) = parent {
        if element_region_frame(node.kind()) {
            depth += 1;
        }
        parent = node.parent();
    }
    depth
}

fn element_region_frame(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::ITEM
            | OrgSyntaxKind::INLINE_TASK
            | OrgSyntaxKind::DRAWER
            | OrgSyntaxKind::FOOTNOTE_DEFINITION
            | OrgSyntaxKind::QUOTE_BLOCK
            | OrgSyntaxKind::CENTER_BLOCK
            | OrgSyntaxKind::DYNAMIC_BLOCK
            | OrgSyntaxKind::SPECIAL_BLOCK
    )
}

fn merge_diagnostics(
    prior: &[Diagnostic],
    fresh_lines: &[(TextRange, Vec<Diagnostic>)],
) -> Vec<Diagnostic> {
    // Emit order, not start order. A failed CLOCK records the timestamp
    // diagnostic before the clock one, even though the clock range starts
    // earlier. Splice each line's fragment diagnostics, in fragment emit
    // order, before the first survivor that starts at or after that line's end.
    let mut merged = Vec::new();
    let mut next = 0;
    for diag in prior {
        if fresh_lines
            .iter()
            .any(|(line, _)| intersects(*line, diag.range()))
        {
            continue;
        }
        while next < fresh_lines.len() && diag.range().start() >= fresh_lines[next].0.end() {
            merged.extend(fresh_lines[next].1.iter().cloned());
            next += 1;
        }
        merged.push(diag.clone());
    }
    while next < fresh_lines.len() {
        merged.extend(fresh_lines[next].1.iter().cloned());
        next += 1;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Headline, InlineTask};
    use crate::parser::TodoKind;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::{ParseConfig, TodoInterpretation, TodoSequence, parse_with};
    use rowan::GreenNode;
    use std::sync::Arc;

    fn materialize(src: &str, config: ParseConfig, prior: Option<&Document>) -> Document {
        crate::document::materialize(src, Arc::new(config), prior)
    }

    fn same_green(a: &SyntaxNode, b: &SyntaxNode) -> bool {
        let left = a.green();
        let right = b.green();
        std::ptr::eq(left.as_ref(), right.as_ref())
    }

    fn stars(n: usize) -> String {
        "*".repeat(n)
    }

    /// Children of `syntax` that tile `line`, wrapped as a `HEADING`.
    fn star_line_as_heading(syntax: &SyntaxNode, line: TextRange) -> GreenNode {
        let green = syntax.green();
        let mut children = Vec::new();
        let mut at = syntax.text_range().start();
        for child in green.children() {
            let len = child.text_len();
            let next = at + len;
            if at >= line.end() {
                break;
            }
            if at >= line.start() && next <= line.end() {
                children.push(child.to_owned());
            }
            at = next;
        }
        GreenNode::new(OrgSyntaxKind::HEADING.into(), children)
    }

    fn assert_same_config_star_line(doc: &Document, syntax: &SyntaxNode, line: TextRange) {
        let depth = region_nesting_at_star_line(syntax);
        let (fragment, diags) = recarve_star_line(doc.slice(line), doc.config(), depth);
        let wrapped = star_line_as_heading(syntax, line);
        assert_eq!(
            &*wrapped, &*fragment,
            "same-config recarve diverged at {line:?} depth {depth}"
        );
        let shifted: Vec<Diagnostic> = diags
            .into_iter()
            .map(|diag| {
                shift_diagnostic(&diag, i64::from(u32::from(line.start())))
                    .expect("diag shift stays in range")
            })
            .collect();
        let on_line: Vec<Diagnostic> = doc
            .diagnostics()
            .iter()
            .filter(|diag| intersects(diag.range(), line))
            .cloned()
            .collect();
        assert_eq!(shifted, on_line, "star-line diagnostics at {line:?}");
    }

    #[test]
    fn same_config_recarve_matches_taxonomy_star_lines() {
        for &(src, cfg_tag) in crate::tests::TAXONOMY {
            let doc = parse_with(src, &cfg_tag.build());
            for task in doc.tasks() {
                let syntax = task.star_syntax();
                assert_same_config_star_line(&doc, &syntax, task.star_line_range());
            }
        }
    }

    /// Title object nesting that hits the ceiling in the file must hit it in
    /// the fragment too. A fragment that starts at 0 parses the inner italic.
    #[test]
    fn tight_budget_fragment_matches_in_file_star_line() {
        let title = "*x /y/ z*";
        // A closed task's body contains no star-head (find_task_end stops at
        // the first one), so a task cannot sit inside another task. Depth 2
        // is a quote whose item holds the task: both frames charge, the list
        // node does not.
        let cases: &[(&str, u16, u16, OrgSyntaxKind)] = &[
            ("list-item", 2, 1, OrgSyntaxKind::ITEM),
            ("drawer", 2, 1, OrgSyntaxKind::DRAWER),
            ("quote", 2, 1, OrgSyntaxKind::QUOTE_BLOCK),
            ("quote-item", 3, 2, OrgSyntaxKind::ITEM),
        ];
        for &(name, budget, depth_want, parent_kind) in cases {
            let src = tight_source(name, title);
            let cfg = ParseConfig::default().with_max_nesting_depth(budget);
            let doc = parse_with(&src, &cfg);
            let task = inner_task(&doc, title);
            assert_eq!(
                task.parent().expect("task parent").kind(),
                parent_kind,
                "{name}: fixture must sit in the frame that charges nesting"
            );
            let depth = region_nesting_at_star_line(&task);
            assert_eq!(depth, depth_want, "{name}: region nesting");
            let line = InlineTask::cast(task.clone())
                .expect("inline task")
                .star_line_range();
            // The ceiling is real: the file did not carve the inner italic.
            let title_node = nodes_of_kind(&task, OrgSyntaxKind::HEADING_TITLE)
                .into_iter()
                .next()
                .expect("title");
            assert_eq!(
                nodes_of_kind(&title_node, OrgSyntaxKind::BOLD).len(),
                1,
                "{name}: the title region itself must still open"
            );
            assert!(
                nodes_of_kind(&title_node, OrgSyntaxKind::ITALIC).is_empty(),
                "{name}: budget {budget} depth {depth} should flatten /y/"
            );
            assert_same_config_star_line(&doc, &task, line);

            // Legal counterpart: the same bytes under a budget that fits
            // carve the italic, and the fragment still matches.
            let loose = parse_with(&src, &ParseConfig::default());
            let loose_task = inner_task(&loose, title);
            let loose_title = nodes_of_kind(&loose_task, OrgSyntaxKind::HEADING_TITLE)
                .into_iter()
                .next()
                .expect("title");
            assert_eq!(
                nodes_of_kind(&loose_title, OrgSyntaxKind::ITALIC).len(),
                1,
                "{name}: loose budget must carve the inner italic"
            );
            let loose_line = InlineTask::cast(loose_task.clone())
                .expect("inline task")
                .star_line_range();
            assert_same_config_star_line(&loose, &loose_task, loose_line);
        }
    }

    fn tight_source(name: &str, title: &str) -> String {
        let h = stars(15);
        match name {
            "list-item" => format!("- a\n{h} {title}\n{h} END\n- b\n"),
            "drawer" => format!(":LOG:\n{h} {title}\n{h} END\n:END:\n"),
            "quote" => format!("#+begin_quote\n{h} {title}\n{h} END\n#+end_quote\n"),
            "quote-item" => {
                format!("#+begin_quote\n- a\n{h} {title}\n{h} END\n#+end_quote\n")
            }
            _ => unreachable!("fixture name"),
        }
    }

    fn inner_task(doc: &Document, title: &str) -> SyntaxNode {
        nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
            .into_iter()
            .find(|node| node.text().to_string().contains(title))
            .expect("inner inline task")
    }

    #[test]
    fn sequence_only_change_is_visible_to_todo_kind() {
        let src = "* TODO x\n";
        let prior = materialize(src, ParseConfig::default(), None);
        let swapped = ParseConfig::default()
            .with_todo_keywords([("TODO", TodoKind::Done), ("DONE", TodoKind::Active)]);
        let warm = materialize(src, swapped.clone(), Some(&prior));
        let cold = materialize(src, swapped, None);
        assert_eq!(warm.root().green(), cold.root().green());
        assert!(same_green(prior.root(), warm.root()));
        let task = warm.tasks().next().expect("task");
        assert_eq!(warm.todo_kind(&task), Some(TodoKind::Done));

        let typed = ParseConfig::default().with_todo_sequences([TodoSequence::new(
            TodoInterpretation::Type,
            ["TODO", "DONE"],
            1,
        )]);
        let warm_typed = materialize(src, typed, Some(&prior));
        assert!(same_green(prior.root(), warm_typed.root()));
        let task = warm_typed.tasks().next().expect("task");
        assert_eq!(
            warm_typed
                .todo_sequence(&task)
                .expect("sequence")
                .interpretation(),
            TodoInterpretation::Type
        );
    }

    #[test]
    fn nested_damaged_tasks_keep_undamaged_tails() {
        let h = stars(15);
        let src = format!(
            "{h} TODO outer\nkept paragraph\n{h} BUG inner\ninner body\n{h} END\n{h} END\n"
        );
        let prior_cfg = ParseConfig::default();
        let next_cfg = ParseConfig::default()
            .with_todo_keywords([("BUG", TodoKind::Active), ("FIXED", TodoKind::Done)]);
        let prior = materialize(&src, prior_cfg.clone(), None);
        let warm = materialize(&src, next_cfg.clone(), Some(&prior));
        let cold = materialize(&src, next_cfg.clone(), None);
        assert_eq!(warm.root().green(), cold.root().green());
        assert_eq!(warm.diagnostics(), cold.diagnostics());

        let warm_inner = inner_task(&warm, "BUG inner");
        let cold_inner = inner_task(&cold, "BUG inner");
        assert_eq!(warm_inner.green(), cold_inner.green());

        let kept = paragraph_containing(&warm, "kept paragraph");
        let prior_kept = paragraph_containing(&prior, "kept paragraph");
        assert!(
            same_green(&kept, &prior_kept),
            "undamaged outer body must stay the prior Arc"
        );
        let inner_body = paragraph_containing(&warm, "inner body");
        let prior_inner_body = paragraph_containing(&prior, "inner body");
        assert!(
            same_green(&inner_body, &prior_inner_body),
            "undamaged inner body must stay the prior Arc"
        );

        // Item body contains the task. A star-head ends a parent task before
        // its body opens (`find_task_end`), so this is the nesting the grammar
        // actually builds. The paragraph before the task is the undamaged tail
        // of that body.
        let h = stars(15);
        let contained =
            format!("- lead\n  kept in item\n{h} TODO inside\nbody stays\n{h} END\n* BUG also\n");
        let prior = materialize(&contained, prior_cfg, None);
        let warm = materialize(&contained, next_cfg.clone(), Some(&prior));
        let cold = materialize(&contained, next_cfg, None);
        assert_eq!(warm.root().green(), cold.root().green());
        let warm_task = inner_task(&warm, "TODO inside");
        assert_eq!(
            warm_task.parent().expect("parent").kind(),
            OrgSyntaxKind::ITEM
        );
        assert_eq!(warm_task.green(), inner_task(&cold, "TODO inside").green());
        assert!(same_green(
            &paragraph_containing(&warm, "kept in item"),
            &paragraph_containing(&prior, "kept in item")
        ));
        assert!(same_green(
            &paragraph_containing(&warm, "body stays"),
            &paragraph_containing(&prior, "body stays")
        ));
    }

    fn paragraph_containing(doc: &Document, text: &str) -> SyntaxNode {
        nodes_of_kind(doc.root(), OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .find(|node| node.text().to_string().contains(text))
            .expect(text)
    }

    #[test]
    fn one_of_two_outlines_stays_pointer_equal() {
        let src = "* KEEP one\n* BUG two\n";
        let prior = materialize(src, ParseConfig::default(), None);
        let next_cfg = ParseConfig::default().with_todo_keywords([
            ("TODO", TodoKind::Active),
            ("DONE", TodoKind::Done),
            ("BUG", TodoKind::Active),
        ]);
        let warm = materialize(src, next_cfg.clone(), Some(&prior));
        let cold = materialize(src, next_cfg, None);
        assert_eq!(warm.root().green(), cold.root().green());
        let prior_outlines = nodes_of_kind(prior.root(), OrgSyntaxKind::OUTLINE);
        let warm_outlines = nodes_of_kind(warm.root(), OrgSyntaxKind::OUTLINE);
        assert_eq!(prior_outlines.len(), 2);
        assert!(same_green(&prior_outlines[0], &warm_outlines[0]));
        assert!(!same_green(&prior_outlines[1], &warm_outlines[1]));
    }
}
