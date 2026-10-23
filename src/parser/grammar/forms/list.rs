//! Plain-list grammar: items, bullets, counters, checkboxes, descriptive
//! tags, and the indentation state machine.
//!
//! Evidence (org-list.el / org-element.el):
//! - `org-item-re`: `-` / `+` and ordered `N.` / `N)` bullets at any indent;
//!   a `*` bullet only when indented (column-zero stars are headlines);
//!   after the bullet, whitespace or end of line. Single-letter alphabetical
//!   bullets only under `org-list-allow-alphabetical`
//!   ([`crate::parser::ParseConfig::with_list_alphabetical`], default off).
//! - `org-list-full-item-re`: optional counter `[@N]` / `[@start:N]` / `[@c]`
//!   (then optional ws), optional checkbox exactly `[ ]` / `[X]` / `[-]`
//!   (then required ws or eol), optional descriptive tag - the text before
//!   the LAST ` :: ` on the item's first line (unordered bullets only).
//! - `org-element--list-struct` (re-derived, not ported blindly): a line walk
//!   carrying per-item indent columns. A new item closes every open item at
//!   the same or deeper indent (its start is their end - inter-item blanks
//!   belong to the previous item). Two consecutive blank lines end the whole
//!   list at the first blank. A non-item line at an open item's indent or
//!   less closes it at the position after the last non-blank line; when the
//!   shallowest item closes this way the list is done. Complete blocks,
//!   drawers, and latex environments inside item contents are skipped
//!   opaquely via [`region::opaque_frame_end`] (continuation lines only).
//!   Column-zero **headlines** terminate lists when the indent machine visits
//!   them as ordinary non-item lines. Column-zero **inline tasks** are skipped
//!   by an early arm that, like org's list walk, looks only at high-star lines
//!   for the task's END. That differs from the parser's own END search
//!   (`heading::find_task_end`) and from `opaque_frame_end`.
//! - `org-element-plain-list-parser`: a PLAIN_LIST is a maximal run of
//!   same-indent sibling items chained start-to-end; deeper items form
//!   nested lists inside an item's contents (re-derived per region here).
//!
//! Emits:
//! ```text
//! PLAIN_LIST
//!   ITEM+                      // chained same-indent siblings
//!   [post-blank tokens]        // list-final blanks
//! ITEM
//!   [INDENT] BULLET [WHITESPACE [COUNTER] [WHITESPACE] [CHECKBOX [WHITESPACE]]
//!   [WHITESPACE ITEM_TAG WHITESPACE COLON COLON [WHITESPACE]]]
//!   ... contents ...           // element loop, stopping before item post-blank
//!   [blank lines]              // item post-blank, not the last paragraph
//! ```
//!
//! Contract: unit [`probe`] is a prefilter (item line or not; always
//! interrupts). The structure walk recognises every item line for indent
//! bounds, but stores first-line [`ItemShape`] only for same-indent siblings
//! of THIS list; nested deeper items re-measure when their own list parse
//! runs. [`parse`] emits from those stored shapes without re-measuring.

use crate::parser::grammar::drive::{atoms, region};
use crate::parser::grammar::forms::heading;
use crate::parser::{LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Item-line recognition (org-item-re + org-list-full-item-re)
// ---------------------------------------------------------------------------

/// Bullet form (content-relative token count; `N)` and `a)` span two tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bullet {
    /// `-` / `+` / indented `*` (one token).
    Unordered,
    /// `N.` / `a.` (one TEXT token) or `N)` / `a)` (TEXT + R_PAREN).
    Ordered { tokens: usize },
}

impl Bullet {
    fn tokens(self) -> usize {
        match self {
            Bullet::Unordered => 1,
            Bullet::Ordered { tokens } => tokens,
        }
    }
}

/// Measured first-line decomposition of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ItemShape {
    bullet: Bullet,
    /// Token count of a counter cookie right after the bullet (delimiters
    /// included), when present.
    counter_tokens: Option<usize>,
    /// Checkbox present (always three tokens: `[`, interior, `]`).
    checkbox: bool,
    /// Position of the WHITESPACE before the tag separator ` :: ` when this
    /// is a descriptive item (unordered bullets only).
    tag_sep: Option<Pos>,
}

/// Dispatcher / interrupt prefilter: true when the line is an item line.
pub(crate) fn probe(line: LineView) -> Option<()> {
    is_item_line(line).then_some(())
}

/// Cheap recognition: bullet + terminator only (no counter/checkbox/tag).
///
/// Structure walks use this for every candidate line. Full first-line
/// decomposition ([`item_shape`]) is reserved for same-indent siblings of
/// the list being emitted.
fn is_item_line(line: LineView) -> bool {
    let Some(bullet) = bullet_shape(line) else {
        return false;
    };
    line.kind(bullet.tokens()).is_line_end_or_space()
}

/// Recognise an item line and measure its first-line decomposition.
fn item_shape(line: LineView) -> Option<ItemShape> {
    let bullet = bullet_shape(line)?;
    let mut k = bullet.tokens();

    // Bullet terminator: whitespace or end of line ("- " or "-\n" or "-").
    match line.kind(k) {
        OrgSyntaxKind::WHITESPACE => k += 1,
        term if term.is_line_end() => {
            return Some(ItemShape {
                bullet,
                counter_tokens: None,
                checkbox: false,
                tag_sep: None,
            });
        }
        _ => return None,
    }

    // Optional counter `[@N]` / `[@start:N]` / `[@c]`, then optional ws.
    let counter_tokens = counter_shape(line, k);
    if let Some(n) = counter_tokens {
        k += n;
        if line.kind(k) == OrgSyntaxKind::WHITESPACE {
            k += 1;
        }
    }

    // Optional checkbox: `[ ]` / `[X]` / `[-]` + required ws or eol.
    // The separator space is not tag text. Leaving `k` on it makes
    // `- [ ] :: body` look like an empty descriptive tag.
    let mut checkbox = false;
    if checkbox_shape(line, k) && line.kind(k + 3).is_line_end_or_space() {
        checkbox = true;
        k += 3;
        if line.kind(k) == OrgSyntaxKind::WHITESPACE {
            k += 1;
        }
    }

    // Descriptive tag: the LAST `ws :: (ws|eol)` on the first line; unordered
    // bullets only (org ignores the tag shape for ordered items).
    let tag_sep = match bullet {
        Bullet::Unordered => find_tag_separator(line, k),
        Bullet::Ordered { .. } => None,
    };

    Some(ItemShape {
        bullet,
        counter_tokens,
        checkbox,
        tag_sep,
    })
}

/// Bullet at line content, or `None`.
fn bullet_shape(line: LineView) -> Option<Bullet> {
    match line.kind(0) {
        // `+` is its own token; `-` is a TEXT run that must be exactly "-".
        OrgSyntaxKind::PLUS => Some(Bullet::Unordered),
        OrgSyntaxKind::STAR => {
            // Column-zero stars are headline material, never bullets.
            (line.indent_cols() > 0).then_some(Bullet::Unordered)
        }
        OrgSyntaxKind::TEXT => {
            let text = line.text(0);
            if text == "-" {
                return Some(Bullet::Unordered);
            }
            // Ordered dot form is one TEXT token ("1." / "a."), because the
            // lexer keeps `.` inside TEXT runs.
            if let Some(body) = text.strip_suffix('.')
                && is_ordered_value(body, line.config().list_alphabetical())
            {
                return Some(Bullet::Ordered { tokens: 1 });
            }
            // Paren form: TEXT value + R_PAREN.
            if is_ordered_value(text, line.config().list_alphabetical())
                && line.kind(1) == OrgSyntaxKind::R_PAREN
            {
                return Some(Bullet::Ordered { tokens: 2 });
            }
            None
        }
        _ => None,
    }
}

/// Ordered bullet value: digits, or a single ASCII letter when allowed.
fn is_ordered_value(s: &str, alphabetical: bool) -> bool {
    if atoms::is_digits(s) {
        return true;
    }
    alphabetical && s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic()
}

/// Counter cookie at content token `k`: token count including brackets.
fn counter_shape(line: LineView, k: usize) -> Option<usize> {
    if line.kind(k) != OrgSyntaxKind::L_BRACKET || line.kind(k + 1) != OrgSyntaxKind::AT {
        return None;
    }
    // `[@start:N]` long form. Kind checks precede text so out-of-kind tiles
    // never call `text` / `is_counter_value`.
    if line.kind(k + 2) == OrgSyntaxKind::TEXT
        && line.text(k + 2).eq_ignore_ascii_case("start")
        && line.kind(k + 3) == OrgSyntaxKind::COLON
        && line.kind(k + 4) == OrgSyntaxKind::TEXT
        && is_counter_value(line.text(k + 4))
        && line.kind(k + 5) == OrgSyntaxKind::R_BRACKET
    {
        return Some(6);
    }
    // `[@N]` / `[@c]` short form.
    (line.kind(k + 2) == OrgSyntaxKind::TEXT
        && is_counter_value(line.text(k + 2))
        && line.kind(k + 3) == OrgSyntaxKind::R_BRACKET)
        .then_some(4)
}

/// Counter value: digits or one ASCII letter (org-list-full-item-re group 2).
fn is_counter_value(s: &str) -> bool {
    atoms::is_digits(s) || (s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic())
}

/// Checkbox tokens at content `k`: `[` + (space | `X` | `-`) + `]`.
fn checkbox_shape(line: LineView, k: usize) -> bool {
    line.kind(k) == OrgSyntaxKind::L_BRACKET
        && line.kind(k + 2) == OrgSyntaxKind::R_BRACKET
        && match line.kind(k + 1) {
            OrgSyntaxKind::WHITESPACE => line.text(k + 1) == " ",
            OrgSyntaxKind::TEXT => matches!(line.text(k + 1), "X" | "-"),
            _ => false,
        }
}

/// The WHITESPACE token position of the LAST ` :: ` separator on the line at
/// or after content token `from` (greedy tag per org-list-full-item-re).
fn find_tag_separator(line: LineView, from: usize) -> Option<Pos> {
    let mut found = None;
    let mut k = from;
    loop {
        match line.kind(k) {
            term if term.is_line_end() => return found,
            OrgSyntaxKind::WHITESPACE
                if line.kind(k + 1) == OrgSyntaxKind::COLON
                    && line.kind(k + 2) == OrgSyntaxKind::COLON
                    && line.kind(k + 3).is_line_end_or_space() =>
            {
                found = line.nth(k).map(|t| t.pos());
                k += 3;
            }
            _ => k += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Structure scan (org-element--list-struct re-derived)
// ---------------------------------------------------------------------------

/// One measured item: start line, indent column, exclusive end.
///
/// `shape` is set only for same-indent siblings of the list's first item
/// (the chain [`parse`] emits). Nested deeper items keep `shape: None` and
/// re-measure when their own list parse runs.
#[derive(Debug, Clone, Copy)]
struct ItemRec {
    start: Pos,
    indent: u32,
    end: Pos,
    shape: Option<ItemShape>,
}

/// Walk lines from the cursor's item line and measure every item's extent.
///
/// Records come back in document order. See the module docs for the
/// termination rules this encodes. Full first-line shapes are stored only
/// for the same-indent chain of the first item.
fn scan_list_struct(parser: &Parser) -> Vec<ItemRec> {
    let mut done: Vec<ItemRec> = Vec::new();
    // Open items: index into `done` (end patched when closed) + indent.
    let mut open: Vec<usize> = Vec::new();

    let Some(first) = parser.line() else {
        return done;
    };
    // Base indent of THIS list (same-indent sibling chain). Nested deeper
    // item lines are recognised for bounds but not fully shaped.
    let base_indent = first.indent_cols();
    // Exclusive end of the last non-blank line seen (blank backtracking).
    let mut last_nonblank_end = first.start();
    let mut cur = Some(first);

    while let Some(line) = cur {
        if line.is_blank() {
            // Two consecutive blank lines end the list at the first blank.
            let second_blank = line.next().is_some_and(|n| n.is_blank());
            let ends_region = line.next().is_none();
            if second_blank || ends_region {
                let at = if second_blank {
                    line.start()
                } else {
                    last_nonblank_end
                };
                for &i in &open {
                    done[i].end = at;
                }
                return done;
            }
            cur = line.next();
            continue;
        }

        // High-star inline task: skip it before the indent close, since
        // column-0 tasks never reach opaque_frame_end. The list's END search
        // looks only at high-star lines (see list_skip_task).
        if heading::probe(line).is_some_and(heading::StarLine::is_task) {
            let (last_included_end, next) = list_skip_task(line);
            last_nonblank_end = last_included_end;
            cur = next;
            continue;
        }

        if is_item_line(line) {
            let indent = line.indent_cols();
            // A new item ends every open item at the same or deeper indent;
            // blanks before a sibling belong to the previous item.
            while let Some(&top) = open.last() {
                if done[top].indent >= indent {
                    done[top].end = line.start();
                    open.pop();
                } else {
                    break;
                }
            }
            // Full first-line measure only for this list's same-indent chain.
            let shape = (indent == base_indent)
                .then(|| item_shape(line).expect("is_item_line implies item_shape"));
            open.push(done.len());
            done.push(ItemRec {
                start: line.start(),
                indent,
                end: line.end(), // patched on close; fallback for region end
                shape,
            });
            last_nonblank_end = line.end();
            cur = line.next();
            continue;
        }

        // Non-item line (true headlines land here as column-zero content): close
        // open items at this indent or less, ending after the last non-blank.
        let indent = line.indent_cols();
        while let Some(&top) = open.last() {
            if indent <= done[top].indent {
                done[top].end = last_nonblank_end;
                open.pop();
                if open.is_empty() {
                    return done;
                }
            } else {
                break;
            }
        }

        // Continuation line of the innermost open item: skip complete block
        // and drawer interiors opaquely (their lines never touch the machine).
        last_nonblank_end = line.end();
        let skip_to = region::opaque_frame_end(line);
        cur = match skip_to.and_then(|bound| parser.line_of(bound)) {
            Some(end_line) => {
                // The construct's END line is the last non-blank we know of.
                last_nonblank_end = end_line.end();
                end_line.next()
            }
            None => line.next(),
        };
    }

    // Region end: every open item ends after the last non-blank line.
    for &i in &open {
        done[i].end = last_nonblank_end;
    }
    done
}

/// Org list-struct skip past a high-star inlinetask open.
///
/// Returns `(last_nonblank_end, next_line)` where `last_nonblank_end` is the
/// exclusive-end `Pos` of the last line included in the skip (open task line,
/// or the high-star END line when found), and `next_line` is the line after
/// the skipped extent (`None` at region/EOF).
///
/// Algorithm (region-capped):
/// 1. Scan from the line after `open` for the first [`heading::StarLine::Task`].
///    Low-level star lines and ordinary prose are not stops for this scan.
/// 2. If found and `is_end_title_line`, include that END line.
/// 3. Else skip only the open task line.
///
/// This is not the parser's END search ([`heading::InlineTaskShape`]): a
/// low-level `* END` after a high-star open does not extend the skip, and the
/// list then sees it as a column-zero non-item and ends.
fn list_skip_task<'a, 's>(open: LineView<'a, 's>) -> (Pos, Option<LineView<'a, 's>>) {
    for line in open.lines_after() {
        if !heading::probe(line).is_some_and(heading::StarLine::is_task) {
            continue;
        }
        if heading::is_end_title_line(line) {
            return (line.end(), line.next());
        }
        // First high-star is not END: skip open only.
        return (open.end(), open.next());
    }
    (open.end(), open.next())
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// Parse a plain list at the cursor (dispatcher row; cursor at an item line).
///
/// Measures the whole structure once, then emits the maximal run of
/// same-indent siblings chained start-to-end. Deeper items nest as new
/// `PLAIN_LIST`s inside item contents via the recursive element loop.
pub(crate) fn parse(parser: &mut Parser) {
    let structure = scan_list_struct(parser);
    debug_assert!(!structure.is_empty(), "parse called off an item line");
    let Some(&first) = structure.first() else {
        return;
    };

    // Chain of same-indent siblings from the first item (shapes stored on walk).
    let mut chain: Vec<(Pos, ItemShape)> = Vec::new();
    let mut expect = first.start;
    for rec in &structure {
        if rec.indent == first.indent && rec.start == expect {
            let shape = rec
                .shape
                .expect("same-indent chain items carry a measured shape");
            chain.push((rec.end, shape));
            expect = rec.end;
        }
    }

    let marker = parser.start();
    for (end, shape) in chain {
        parse_item(parser, end, shape);
    }
    // Blanks after the last item's contents belong to the list (org list
    // post-blank; item ends exclude them via blank backtracking).
    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::PLAIN_LIST);
}

/// Parse one item from its structure-walk shape, then a bounded element loop.
fn parse_item(parser: &mut Parser, end: Pos, shape: ItemShape) {
    debug_assert_eq!(
        parser.line().and_then(item_shape),
        Some(shape),
        "parse_item shape must match the structure-walk probe"
    );

    // Org's item contents end before the trailing blank run. That run is the
    // item's post-blank; the last paragraph does not own it.
    let contents_end = match parser.line() {
        Some(first) => {
            let mut last_nonblank = first.start();
            let mut line = first;
            loop {
                if line.start() >= end {
                    break;
                }
                if !line.is_blank() {
                    last_nonblank = line.end();
                }
                match line.next() {
                    Some(next) => line = next,
                    None => break,
                }
            }
            if last_nonblank < end {
                last_nonblank
            } else {
                end
            }
        }
        None => end,
    };

    let marker = parser.start();
    parser.eat(OrgSyntaxKind::INDENT);
    parser.leaf(OrgSyntaxKind::BULLET, shape.bullet.tokens());

    if parser.current() == OrgSyntaxKind::WHITESPACE {
        parser.eat_any();
    }
    if let Some(n) = shape.counter_tokens {
        parser.leaf(OrgSyntaxKind::COUNTER, n);
        if parser.current() == OrgSyntaxKind::WHITESPACE {
            parser.eat_any();
        }
    }
    if shape.checkbox {
        parser.leaf(OrgSyntaxKind::CHECKBOX, 3);
        // Required separator. It is not the paragraph's first byte.
        if parser.current() == OrgSyntaxKind::WHITESPACE {
            parser.eat_any();
        }
    }
    if let Some(tag_sep) = shape.tag_sep {
        // Tag objects up to the separator; then ` :: ` as bare tokens.
        if parser.current() == OrgSyntaxKind::WHITESPACE {
            parser.eat_any();
        }
        let tag_m = parser.start();
        region::inline_in(parser, tag_sep, OrgSyntaxKind::ITEM_TAG);
        tag_m.complete(parser, OrgSyntaxKind::ITEM_TAG);
        parser.eat(OrgSyntaxKind::WHITESPACE);
        parser.eat(OrgSyntaxKind::COLON);
        parser.eat(OrgSyntaxKind::COLON);
        // Space after `::` is the separator, not definition text.
        if parser.current() == OrgSyntaxKind::WHITESPACE {
            parser.eat_any();
        }
    }

    // First-line terminator for empty leads ("-" / "- " alone) so the
    // contents loop starts at a line boundary and never opens a pure-blank
    // paragraph on the bullet's own newline.
    if parser.at_line_term() {
        parser.eat(OrgSyntaxKind::NEWLINE);
    }

    parser.with_limit_at(end, |p| {
        region::elements_in(p, contents_end);
        p.eat_blank_lines();
    });
    debug_assert_eq!(parser.cursor(), end, "item must consume its measured end");
    marker.complete(parser, OrgSyntaxKind::ITEM);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ParseConfig;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{
        format_tree, make_parser, nodes_of_kind, parse_document_tree,
        parse_document_tree_with_config, probe_line, tree_text,
    };

    fn lists(root: &crate::syntax::SyntaxNode) -> Vec<crate::syntax::SyntaxNode> {
        nodes_of_kind(root, OrgSyntaxKind::PLAIN_LIST)
    }

    fn items(node: &crate::syntax::SyntaxNode) -> Vec<crate::syntax::SyntaxNode> {
        node.children()
            .filter(|c| c.kind() == OrgSyntaxKind::ITEM)
            .collect()
    }

    // --- Item-line recognition ---------------------------------------------

    #[test]
    fn bullet_shapes_recognised() {
        assert!(probe_line("- item\n", item_shape).is_some());
        assert!(probe_line("+ item\n", item_shape).is_some());
        assert!(probe_line("  - indented\n", item_shape).is_some());
        assert!(probe_line("  * star indented\n", item_shape).is_some());
        assert!(probe_line("1. ordered dot\n", item_shape).is_some());
        assert!(probe_line("12) ordered paren\n", item_shape).is_some());
        assert!(probe_line("-\n", item_shape).is_some(), "bare dash + eol");
        assert!(probe_line("- \n", item_shape).is_some());
        assert!(probe_line("-", item_shape).is_some(), "dash at EOF");
    }

    #[test]
    fn non_items_rejected() {
        assert!(
            probe_line("* headline\n", item_shape).is_none(),
            "column-zero star"
        );
        assert!(probe_line("-item glued\n", item_shape).is_none());
        assert!(probe_line("-- two dashes\n", item_shape).is_none());
        assert!(probe_line("1.5 not a bullet\n", item_shape).is_none());
        assert!(probe_line("1x) not digits\n", item_shape).is_none());
        assert!(probe_line("a. alpha off by default\n", item_shape).is_none());
        assert!(probe_line("A) alpha off by default\n", item_shape).is_none());
        assert!(probe_line("plain text\n", item_shape).is_none());
        assert!(
            probe_line("-----\n", item_shape).is_none(),
            "horizontal rule"
        );
    }

    #[test]
    fn alphabetical_bullets_behind_config() {
        let cfg = ParseConfig::default().with_list_alphabetical(true);
        let p = crate::parser::Parser::with_config("a. alpha\n", &cfg);
        assert!(p.line().and_then(item_shape).is_some());
        let p2 = crate::parser::Parser::with_config("A) alpha\n", &cfg);
        assert!(p2.line().and_then(item_shape).is_some());
        // Multi-letter still rejected.
        let p3 = crate::parser::Parser::with_config("ab. nope\n", &cfg);
        assert!(p3.line().and_then(item_shape).is_none());

        // Full parse under the config.
        let root = parse_document_tree_with_config(
            "a. one\nb. two\n",
            &ParseConfig::default().with_list_alphabetical(true),
        );
        assert_eq!(items(&lists(&root)[0]).len(), 2);
    }

    #[test]
    fn counter_and_checkbox_shapes() {
        let s = probe_line("- [@5] counted\n", item_shape).expect("item");
        assert_eq!(s.counter_tokens, Some(4));
        let s2 = probe_line("- [@start:5] counted\n", item_shape).expect("item");
        assert_eq!(s2.counter_tokens, Some(6));
        let s3 = probe_line("- [@c] letter\n", item_shape).expect("item");
        assert_eq!(s3.counter_tokens, Some(4));

        assert!(
            probe_line("- [ ] todo\n", item_shape)
                .expect("item")
                .checkbox
        );
        assert!(
            probe_line("- [X] done\n", item_shape)
                .expect("item")
                .checkbox
        );
        assert!(
            probe_line("- [-] partial\n", item_shape)
                .expect("item")
                .checkbox
        );
        assert!(
            probe_line("- [@3] [X] both\n", item_shape)
                .expect("item")
                .checkbox
        );
        // Lowercase x / glued bracket are not checkboxes.
        assert!(
            !probe_line("- [x] nope\n", item_shape)
                .expect("item")
                .checkbox
        );
        assert!(
            !probe_line("- [ ]x glued\n", item_shape)
                .expect("item")
                .checkbox
        );
    }

    #[test]
    fn tag_separator_unordered_only_and_last_wins() {
        assert!(
            probe_line("- term :: def\n", item_shape)
                .expect("item")
                .tag_sep
                .is_some()
        );
        assert!(
            probe_line("- a :: b :: c\n", item_shape)
                .expect("item")
                .tag_sep
                .is_some()
        );
        assert!(
            probe_line("1. term :: def\n", item_shape)
                .expect("item")
                .tag_sep
                .is_none(),
            "ordered items have no tag"
        );
        assert!(
            probe_line("- no separator here\n", item_shape)
                .expect("item")
                .tag_sep
                .is_none()
        );
        // `::` glued to text is not a separator.
        assert!(
            probe_line("- a::b\n", item_shape)
                .expect("item")
                .tag_sep
                .is_none()
        );
    }

    // --- Basic structure ----------------------------------------------------

    #[test]
    fn flat_list_of_each_bullet_kind() {
        for src in [
            "- a\n- b\n- c\n",
            "+ a\n+ b\n",
            "1. a\n2. b\n",
            "1) a\n2) b\n",
        ] {
            let root = parse_document_tree(src);
            let ls = lists(&root);
            assert_eq!(ls.len(), 1, "one list for {src:?}");
            let n = src.lines().count();
            assert_eq!(items(&ls[0]).len(), n, "{n} items for {src:?}");
        }
    }

    #[test]
    fn item_decomposition_nodes() {
        let src = "- [@3] [X] tag words :: definition\n";
        let root = parse_document_tree(src);
        let item = &nodes_of_kind(&root, OrgSyntaxKind::ITEM)[0];
        assert_eq!(
            tree_text(&nodes_of_kind(item, OrgSyntaxKind::BULLET)[0]),
            "-"
        );
        assert_eq!(
            tree_text(&nodes_of_kind(item, OrgSyntaxKind::COUNTER)[0]),
            "[@3]"
        );
        assert_eq!(
            tree_text(&nodes_of_kind(item, OrgSyntaxKind::CHECKBOX)[0]),
            "[X]"
        );
        assert_eq!(
            tree_text(&nodes_of_kind(item, OrgSyntaxKind::ITEM_TAG)[0]),
            "tag words"
        );
    }

    #[test]
    fn item_multiline_contents_and_termination_by_indent() {
        // Continuation lines at deeper indent belong to the item; a
        // lesser-indent line ends the list.
        let src = "- first\n  continues\nafter list\n";
        let root = parse_document_tree(src);
        let ls = lists(&root);
        assert_eq!(ls.len(), 1);
        assert_eq!(tree_text(&ls[0]), "- first\n  continues\n");
        let after: Vec<_> = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .filter(|p| tree_text(p).contains("after list"))
            .collect();
        assert_eq!(after.len(), 1);
        assert_eq!(tree_text(&after[0]), "after list\n");
    }

    #[test]
    fn single_blank_continues_double_blank_terminates() {
        // One blank line between items: same list (blank under previous item).
        let src1 = "- a\n\n- b\n";
        let root1 = parse_document_tree(src1);
        assert_eq!(tree_text(&root1), src1);
        let ls1 = lists(&root1);
        assert_eq!(ls1.len(), 1);
        assert_eq!(items(&ls1[0]).len(), 2);
        assert_eq!(
            tree_text(&items(&ls1[0])[0]),
            "- a\n\n",
            "inter-item blank belongs to the previous item"
        );

        // Two consecutive blanks end the list; the second run is a new list.
        let src2 = "- a\n\n\n- b\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let ls2 = lists(&root2);
        assert_eq!(ls2.len(), 2, "double blank splits lists");
        assert_eq!(
            tree_text(&ls2[0]),
            "- a\n\n\n",
            "list owns its terminating blanks"
        );
    }

    #[test]
    fn item_body_excludes_separator_space_and_item_post_blank() {
        let src = "- a\n- b\n\n1. next\n";
        let root = parse_document_tree(src);
        let its = items(&lists(&root)[0]);
        let para = &nodes_of_kind(&its[1], OrgSyntaxKind::PARAGRAPH)[0];
        assert_eq!(tree_text(para), "b\n", "inter-item blank stays on the item");
        assert_eq!(tree_text(&its[1]), "- b\n\n");

        let tagged = "- [ ] unchecked\n- term :: definition\n";
        let root = parse_document_tree(tagged);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(tree_text(&paras[0]), "unchecked\n");
        assert_eq!(tree_text(&paras[1]), "definition\n");

        // A blank between two paragraphs of one item stays on the first.
        // Stopping the contents walk at the first blank would drop "second".
        let internal = "- first\n\n  second\n";
        let root = parse_document_tree(internal);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 2);
        assert_eq!(tree_text(&paras[0]), "first\n\n");
        assert_eq!(tree_text(&paras[1]), "  second\n");

        // The blank after a nested item belongs to the outer item, not the
        // nested paragraph. The continuation after that blank is still body.
        let nested = "- outer\n  - nested\n\n  after\n";
        let root = parse_document_tree(nested);
        let outer = &items(&lists(&root)[0])[0];
        let nested_list = &nodes_of_kind(outer, OrgSyntaxKind::PLAIN_LIST)[0];
        let nested_item = &items(nested_list)[0];
        let nested_para = &nodes_of_kind(nested_item, OrgSyntaxKind::PARAGRAPH)[0];
        assert_eq!(tree_text(nested_para), "nested\n");
        assert_eq!(tree_text(nested_item), "  - nested\n");
        assert!(tree_text(outer).contains("\n\n  after\n"));
        let after: Vec<_> = nodes_of_kind(outer, OrgSyntaxKind::PARAGRAPH)
            .into_iter()
            .filter(|p| tree_text(p).contains("after"))
            .collect();
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn checkbox_separator_is_not_an_empty_tag() {
        let bare = "- [ ] :: body\n";
        let root = parse_document_tree(bare);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::ITEM_TAG).is_empty(),
            "checkbox separator is not an empty tag"
        );
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(tree_text(&paras[0]), ":: body\n");

        let legal = "- [ ] tag :: def\n";
        let root = parse_document_tree(legal);
        let tag = &nodes_of_kind(&root, OrgSyntaxKind::ITEM_TAG)[0];
        assert_eq!(tree_text(tag), "tag");
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(tree_text(&paras[0]), "def\n");
    }

    #[test]
    fn nesting_three_deep_by_indentation() {
        let src = "- top\n  - mid\n    - deep\n- top2\n";
        let root = parse_document_tree(src);

        let top = &lists(&root)[0];
        let top_items = items(top);
        assert_eq!(top_items.len(), 2, "two top-level items");
        let mid_list = nodes_of_kind(&top_items[0], OrgSyntaxKind::PLAIN_LIST);
        assert!(!mid_list.is_empty(), "nested list inside first item");
        let mid_items = items(&mid_list[0]);
        assert_eq!(mid_items.len(), 1);
        assert!(
            !nodes_of_kind(&mid_items[0], OrgSyntaxKind::PLAIN_LIST).is_empty(),
            "third level nested"
        );
    }

    #[test]
    fn item_with_immediate_nested_list() {
        let src = "- top\n  - nested\n";
        let root = parse_document_tree(src);
        let top_items = items(&lists(&root)[0]);
        assert_eq!(top_items.len(), 1);
        assert_eq!(
            nodes_of_kind(&top_items[0], OrgSyntaxKind::PLAIN_LIST).len(),
            1
        );
    }

    #[test]
    fn item_containing_paragraph_nested_list_and_block() {
        let src = "- lead para\n  - sub\n  #+begin_example\n  raw\n  #+end_example\n";
        let root = parse_document_tree(src);
        let item = &items(&lists(&root)[0])[0];
        assert!(
            !nodes_of_kind(item, OrgSyntaxKind::PARAGRAPH).is_empty(),
            "{:?}",
            nodes_of_kind(item, OrgSyntaxKind::PARAGRAPH)
        );
        assert!(
            !nodes_of_kind(item, OrgSyntaxKind::PLAIN_LIST).is_empty(),
            "{:?}",
            nodes_of_kind(item, OrgSyntaxKind::PLAIN_LIST)
        );
        assert!(
            !nodes_of_kind(item, OrgSyntaxKind::EXAMPLE_BLOCK).is_empty(),
            "{:?}",
            nodes_of_kind(item, OrgSyntaxKind::EXAMPLE_BLOCK)
        );
    }

    #[test]
    fn list_inside_quote_block_and_drawer() {
        let src = "#+begin_quote\n- inside\n#+end_quote\n";
        let root = parse_document_tree(src);
        let quote = &nodes_of_kind(&root, OrgSyntaxKind::QUOTE_BLOCK)[0];
        assert_eq!(lists(quote).len(), 1);

        let src2 = ":LOG:\n- inside\n:END:\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let drawer = &nodes_of_kind(&root2, OrgSyntaxKind::DRAWER)[0];
        assert_eq!(lists(drawer).len(), 1);
    }

    #[test]
    fn heading_terminates_list() {
        let src = "- item\n* H\n";
        let root = parse_document_tree(src);
        let ls = lists(&root);
        assert_eq!(ls.len(), 1);
        assert_eq!(tree_text(&ls[0]), "- item\n");
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::HEADING).len(), 1);
    }

    #[test]
    fn list_as_section_last_element_owns_post_blank() {
        let src = "* H\n- item\n\n";
        let root = parse_document_tree(src);
        let ls = lists(&root);
        assert_eq!(tree_text(&ls[0]), "- item\n\n");
    }

    #[test]
    fn item_line_interrupts_paragraph() {
        let src = "text before\n- item\n";
        let root = parse_document_tree(src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert!(paras.iter().any(|p| tree_text(p) == "text before\n"));
        assert_eq!(lists(&root).len(), 1);
    }

    #[test]
    fn star_bullet_vs_headline_interplay() {
        // Indented star: item. Column-zero star: heading terminates the list.
        let src = "- a\n  * starred item\n* Heading\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::HEADING).len(), 1);
        let all_lists = lists(&root);
        assert_eq!(all_lists.len(), 2, "outer list + nested star list");
        assert!(tree_text(&all_lists[0]).contains("starred item"));
    }

    #[test]
    fn descriptive_item_with_objects_and_multiline_def() {
        let src = "- *term* :: definition\n  continues\n";
        let root = parse_document_tree(src);
        let tag = &nodes_of_kind(&root, OrgSyntaxKind::ITEM_TAG)[0];
        assert_eq!(tree_text(tag), "*term*");
        assert!(
            !nodes_of_kind(tag, OrgSyntaxKind::BOLD).is_empty(),
            "tag contents parse as objects"
        );
    }

    #[test]
    fn code_markup_colons_not_a_separator() {
        // `~::~` inside code markup: the raw `::` tokens still match the
        // separator scan only when preceded by whitespace and followed by
        // ws/eol; here they are, so org (and we) treat the LAST ` :: ` as the
        // separator. The tilde-wrapped one loses to the later real separator.
        let src = "- ~a~ :: def\n";
        let root = parse_document_tree(src);
        let tag = &nodes_of_kind(&root, OrgSyntaxKind::ITEM_TAG)[0];
        assert_eq!(tree_text(tag), "~a~");
        assert!(
            !nodes_of_kind(tag, OrgSyntaxKind::CODE).is_empty(),
            "{:?}",
            nodes_of_kind(tag, OrgSyntaxKind::CODE)
        );
    }

    #[test]
    fn empty_items_and_pre_blank_contents() {
        let src = "-\n- \n- x\n";
        let root = parse_document_tree(src);
        assert_eq!(items(&lists(&root)[0]).len(), 3);
        for p in nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH) {
            let t = tree_text(&p);
            assert!(
                t.chars().any(|c| !c.is_whitespace()),
                "no pure-blank paragraph from empty items: {t:?}"
            );
        }
    }

    #[test]
    fn cookie_on_item_and_tag() {
        let src = "- tasks [1/3] here\n";
        let root = parse_document_tree(src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            1
        );

        let src2 = "- progress [50%] :: def\n";
        let root2 = parse_document_tree(src2);
        let tag = &nodes_of_kind(&root2, OrgSyntaxKind::ITEM_TAG)[0];
        assert_eq!(
            nodes_of_kind(tag, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            1,
            "cookie inside item tag"
        );
    }

    #[test]
    fn checkbox_mid_paragraph_is_not_a_checkbox() {
        let src = "see [ ] mid paragraph\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CHECKBOX).len(), 0);
        assert_eq!(lists(&root).len(), 0);
    }

    #[test]
    fn mixed_bullets_same_indent_stay_one_run_per_org() {
        // org's struct does not split on bullet kind at the same indent: the
        // sibling chain is indentation-driven.
        let src = "- a\n+ b\n";
        let root = parse_document_tree(src);
        let ls = lists(&root);
        assert_eq!(ls.len(), 1);
        assert_eq!(items(&ls[0]).len(), 2);
    }

    #[test]
    fn deeper_then_shallower_item_closes_nested_only() {
        let src = "- a\n  - deep\n- b\n";
        let root = parse_document_tree(src);
        let top = &lists(&root)[0];
        let tops = items(top);
        assert_eq!(tops.len(), 2);
        assert!(tree_text(&tops[0]).contains("deep"));
        assert!(!tree_text(&tops[1]).contains("deep"));
    }

    #[test]
    fn list_struct_scan_direct() {
        let p = make_parser("- a\n  cont\n- b\nout\n");
        let recs = scan_list_struct(&p);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].indent, 0);
        // First item ends where the second begins.
        assert_eq!(recs[0].end, recs[1].start);
        // Same-indent chain stores full shapes matching a fresh measure.
        let line0 = p.line().expect("first item line");
        assert_eq!(recs[0].shape, Some(item_shape(line0).expect("item")));
        let line1 = p.line_of(recs[1].start).expect("second item line");
        assert_eq!(recs[1].shape, Some(item_shape(line1).expect("item")));
        assert_eq!(recs[0].shape.unwrap().bullet, Bullet::Unordered);
        assert_eq!(recs[1].shape.unwrap().bullet, Bullet::Unordered);
    }

    #[test]
    fn list_struct_scan_stores_heterogeneous_shapes() {
        let p = make_parser("- [@3] [X] tag :: def\n1. ordered\n");
        let recs = scan_list_struct(&p);
        assert_eq!(recs.len(), 2);
        let s0 = recs[0].shape.expect("top shape");
        let s1 = recs[1].shape.expect("top shape");
        assert_eq!(s0.bullet, Bullet::Unordered);
        assert_eq!(s0.counter_tokens, Some(4));
        assert!(s0.checkbox);
        assert!(s0.tag_sep.is_some());
        assert_eq!(s1.bullet, Bullet::Ordered { tokens: 1 });
        assert!(s1.counter_tokens.is_none());
        assert!(!s1.checkbox);
        assert!(s1.tag_sep.is_none());
        // Fresh measure of each start line matches the stored shape.
        assert_eq!(
            recs[0].shape,
            Some(item_shape(p.line_of(recs[0].start).expect("line")).expect("shape"))
        );
        assert_eq!(
            recs[1].shape,
            Some(item_shape(p.line_of(recs[1].start).expect("line")).expect("shape"))
        );
    }

    #[test]
    fn list_struct_scan_skips_nested_shapes() {
        // Nested deeper items are recognised for bounds but not shaped.
        let p = make_parser("- a\n  - deep\n- b\n");
        let recs = scan_list_struct(&p);
        assert_eq!(recs.len(), 3);
        assert!(recs[0].shape.is_some(), "top same-indent shaped");
        assert!(recs[1].shape.is_none(), "nested deeper not shaped");
        assert_eq!(recs[1].indent, 2);
        assert!(recs[2].shape.is_some(), "second top sibling shaped");
    }

    /// Shape fixtures: `(source, PLAIN_LIST count, ITEM count)`.
    ///
    /// Losslessness is not asserted: `builder::build` already checks gap-free
    /// tiling on every parse, in release builds too. These fixtures pin the
    /// item and list *grouping*, which the builder cannot see.
    const SHAPE_FIXTURES: &[(&str, usize, usize)] = &[
        ("- a\n- b\n", 1, 2),
        ("- a\n\n- b\n\n\nafter\n", 1, 2),
        ("1. x\n   deep\n2. y\n", 1, 2),
        ("- [ ] t\n- [X] d\n- [-] p\n", 1, 3),
        // Deeper indent opens a nested list under the outer item.
        ("  - indented list\n    - deeper\n", 2, 2),
        ("- a :: b\n", 1, 1),
        ("- \n\n  content after blank lead\n", 1, 1),
        ("* H\n- in section\n** H2\n", 1, 1),
        ("- crlf\r\n- second\r\n", 1, 2),
        ("#+begin_quote\n- q\n#+end_quote\n", 1, 1),
        ("- outer\n  :D:\n  x\n  :END:\n  after drawer\n", 1, 1),
    ];

    #[test]
    fn tiling_fixtures_produce_expected_list_shape() {
        for &(src, lists, items) in SHAPE_FIXTURES {
            let root = parse_document_tree(src);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::PLAIN_LIST).len(),
                lists,
                "PLAIN_LIST count for {src:?}\n{}",
                format_tree(&root)
            );
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::ITEM).len(),
                items,
                "ITEM count for {src:?}\n{}",
                format_tree(&root)
            );
        }
    }

    // --- Inline-task skip in the list walk (its END search differs) ---------

    fn high15() -> String {
        "*".repeat(15)
    }

    #[test]
    fn list_skips_closed_high_star_task_through_high_end() {
        let h = high15();
        let src = format!("- a\n{h} Task\nbody\n{h} END\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(
            ls.len(),
            1,
            "one list through high-star task\n{}",
            format_tree(&root)
        );
        assert_eq!(items(&ls[0]).len(), 2);
        let tasks = nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK);
        assert_eq!(tasks.len(), 1);
        let ttext = tree_text(&tasks[0]);
        assert!(ttext.contains("body"), "closed task owns body");
        assert!(ttext.contains("END"), "closed task owns high END");
    }

    #[test]
    fn list_skips_degenerate_high_star_open_only() {
        let h = high15();
        let src = format!("- a\n{h} Task\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(ls.len(), 1);
        assert_eq!(items(&ls[0]).len(), 2);
    }

    #[test]
    fn list_ends_at_low_level_end_after_high_task() {
        // List skip does not extend past high open (low-level END is not
        // StarLine::Task). Indented body continues the item; `* END` is column-0
        // non-item and ends the list. Under the item region cap the END is
        // outside the limit, so the in-list task is degenerate and `* END` is
        // a following HEADING. (Top-level task + `* END` still closes under
        // parser rules; pin that separately in heading tests.)
        let h = high15();
        let src = format!("- a\n{h} Task\n  body\n* END\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(
            ls.len(),
            2,
            "low-level END ends list; second item starts a new list\n{}",
            format_tree(&root)
        );
        let item0 = &items(&ls[0])[0];
        assert!(
            tree_text(item0).contains("body"),
            "first item includes indented body\n{}",
            format_tree(&root)
        );
        assert!(
            !tree_text(item0).contains("* END"),
            "first item ends before * END"
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK).len(), 1);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::HEADING).len(),
            1,
            "* END after list-capped degenerate task is a headline"
        );
    }

    #[test]
    fn list_skips_through_high_end_jumping_low_star_intermediate() {
        // List: high-star re-search jumps over `* Real` and includes high END.
        // CST: outer task is degenerate (first any-star_head is `* Real`);
        // trailing high END is its own task line. Intermediate `* Real` is not
        // a HEADING (true headlines stay on the outline spine; element walk
        // under the item absorbs the line as paragraph text after the
        // degenerate task).
        let h = high15();
        let src = format!("- a\n{h} Outer\n* Real\n{h} END\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(
            ls.len(),
            1,
            "list skip through high END jumps intermediate low star\n{}",
            format_tree(&root)
        );
        assert_eq!(items(&ls[0]).len(), 2);
        let tasks = nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK);
        assert_eq!(
            tasks.len(),
            2,
            "outer degenerate + trailing END task\n{}",
            format_tree(&root)
        );
        let outer = &tasks[0];
        assert!(
            !tree_text(outer).contains("Real"),
            "outer task excludes Real"
        );
        assert!(
            !tree_text(outer).contains("END"),
            "outer task excludes trailing END line"
        );
        assert!(
            tree_text(&tasks[1]).contains("END"),
            "second task is the high-star END line"
        );
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::HEADING).len(),
            0,
            "* Real mid list-skip is not HEADING (outline-spine-only headlines)"
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH)
                .iter()
                .any(|p| tree_text(p).contains("Real"))
                || tree_text(&items(&ls[0])[0]).contains("Real"),
            "* Real is paragraph/item text under list skip jump"
        );
    }

    #[test]
    fn list_skips_high_task_after_indented_continuation() {
        // Early arm: column-0 high-star after indented item body still skips.
        let h = high15();
        let src = format!("- a\n  cont\n{h} Task\n{h} END\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(
            ls.len(),
            1,
            "one list with continuation then high task\n{}",
            format_tree(&root)
        );
        assert_eq!(items(&ls[0]).len(), 2);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::INLINE_TASK).len(), 1);
    }

    #[test]
    fn list_ends_at_low_star_when_no_later_high_end() {
        let h = high15();
        let src = format!("- a\n{h} Outer\n* Real\n- b\n");
        let root = parse_document_tree(&src);
        let ls = lists(&root);
        assert_eq!(
            ls.len(),
            2,
            "skip open only; then * Real ends list\n{}",
            format_tree(&root)
        );
    }

    #[test]
    fn true_headline_between_items_terminates_list() {
        let src = "- a\n* H\n- b\n";
        let root = parse_document_tree(src);
        let ls = lists(&root);
        assert_eq!(ls.len(), 2, "true headline terminates list");
    }

    #[test]
    fn list_skip_task_direct() {
        let h = high15();
        // Closed: include END line.
        let closed = format!("{h} T\nbody\n{h} END\nafter\n");
        let p = make_parser(&closed);
        let open = p.line().expect("open");
        let (last, next) = super::list_skip_task(open);
        let end_line = p
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| super::heading::is_end_title_line(*l))
            .expect("END");
        assert_eq!(last, end_line.end());
        assert_eq!(next.map(|l| l.start()), end_line.next().map(|l| l.start()));

        // Degenerate: open only.
        let deg = format!("{h} T\nbody\n");
        let p2 = make_parser(&deg);
        let open2 = p2.line().expect("open");
        let (last2, next2) = super::list_skip_task(open2);
        assert_eq!(last2, open2.end());
        assert_eq!(next2.map(|l| l.start()), open2.next().map(|l| l.start()));

        // Low-level END does not extend list skip.
        let low = format!("{h} T\n* END\n");
        let p3 = make_parser(&low);
        let open3 = p3.line().expect("open");
        let (last3, next3) = super::list_skip_task(open3);
        assert_eq!(last3, open3.end());
        assert_eq!(next3.map(|l| l.start()), open3.next().map(|l| l.start()));
    }
}
