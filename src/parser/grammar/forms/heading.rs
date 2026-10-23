//! Star-line family: HEADING (outline) and INLINE_TASK (element).
//!
//! Shared form: unindented `STARS` + literal space, then
//! `[KEYWORD] [PRIORITY] [COMMENT] TITLE [TAGS]` on one line.
//!
//! **HEADING** (outline spine via [`probe`] / [`parse`]):
//! ```text
//! HEADING
//!   [TODO_KEYWORD]
//!   [PRIORITY]
//!   [HEADING_COMMENT]
//!   HEADING_TITLE   (always; may be empty; STANDARD_NO_LINE_BREAK)
//!   [TAGS]
//! ```
//!
//! **INLINE_TASK** (element via [`resolve_task`] / [`parse_task`]): same title
//! anatomy under an `INLINE_TASK` root (no OUTLINE/SECTION). The task's END
//! line is the first later star-head, at any level, whose title is a bare
//! case-insensitive `END`. When one exists, optional planning and property
//! drawer, body elements, and the END line as opaque tokens sit under the
//! task. Without it the task is degenerate: valid, with no diagnostic.
//!
//! TODO_KEYWORD is the only config-dependent carve: a leading non-terminator
//! run (may span lexer-split tokens, e.g. `NEXT/1`) is matched only when its
//! total length is <= the longest configured keyword and the exact source
//! slice is a known keyword followed by a terminator. PRIORITY and COMMENT
//! are purely syntactic.
//!
//! Entry points:
//! - [`star_head`] is the config-free star shape (`^\*+ `, space only).
//! - [`probe`] classifies a star-head as [`StarLine::Heading`] or
//!   [`StarLine::Task`] by `inline_task_min_level`; every star-head is
//!   exactly one of the two.
//! - [`resolve_task`] finds a task's END line for the element dispatcher.
//! - [`is_end_title_line`] is shared by the END search and the list parser's
//!   skip over high-star lines.
//! - [`parse_metadata`] parses the planning line and property drawer that may
//!   follow a heading, or a task that has an END line.

use crate::DiagnosticKind;
use crate::parser::grammar::drive::{atoms, region};
use crate::parser::grammar::forms::{drawer, planning};
use crate::parser::{Diagnostic, LineView, ParseConfig, Parser, Pos, PriorityAtom, TokenView};
use crate::syntax::OrgSyntaxKind;
use rowan::GreenNode;

/// Token count of a full priority cookie: `[` `#` VALUE `]`.
const PRIORITY_COOKIE_TOKENS: usize = 4;

// ---------------------------------------------------------------------------
// Pure star shape and classification
// ---------------------------------------------------------------------------

/// Star-line shape that does not read `ParseConfig`: unindented STAR run
/// plus a literal space after the last star. Returns the star count.
///
/// Matches org-element `org-element-headline-re` (`^\*+ `). Tab after stars is
/// not a star-head. Used by parser END detection, footnote body stop, and as
/// the base for [`probe`].
pub(crate) fn star_head(line: LineView) -> Option<usize> {
    let first = line.first()?;
    if first.kind() != OrgSyntaxKind::STAR {
        return None;
    }
    let mut level = 1usize;
    let mut last = first;
    while let Some(next) = last.next() {
        if next.kind() != OrgSyntaxKind::STAR {
            break;
        }
        level += 1;
        last = next;
    }

    // org-element-headline-re is `\*+ ` - literal space after the last star.
    if last.next_byte() != Some(b' ') {
        return None;
    }
    Some(level)
}

/// Star-head role under `ParseConfig`: outline heading or inline task.
///
/// Every [`star_head`] is exactly one of the two, split by
/// `inline_task_min_level`. Both variants carry the star count (level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StarLine {
    /// Outline headline: `level < inline_task_min_level`.
    Heading(usize),
    /// Inline-task opener: `level >= inline_task_min_level`.
    Task(usize),
}

impl StarLine {
    /// Outline level when this is a heading; `None` for tasks.
    pub(crate) fn as_heading(self) -> Option<usize> {
        match self {
            Self::Heading(n) => Some(n),
            Self::Task(_) => None,
        }
    }

    /// Whether this is an inline-task opener.
    pub(crate) fn is_task(self) -> bool {
        matches!(self, Self::Task(_))
    }
}

/// Classify a star-head line as outline heading or inline task.
///
/// `None` when the line is not a star-head. When `Some`, the variant is the
/// exclusive partition by `inline_task_min_level` (always applied).
pub(crate) fn probe(line: LineView) -> Option<StarLine> {
    let level = star_head(line)?;
    let min = usize::from(line.config().inline_task_min_level());
    if level < min {
        Some(StarLine::Heading(level))
    } else {
        Some(StarLine::Task(level))
    }
}

/// Whether a star-head line's title is bare case-insensitive `END`.
///
/// After the required space from `^\*+ `, remaining tiles must match
/// optional whitespace, TEXT `END` (ASCII case-insensitive), optional
/// whitespace, then line end. No tags, TODO, or other text. Shared by the END
/// search and the list parser's high-star skip; the element dispatcher does
/// not call it.
pub(crate) fn is_end_title_line(line: LineView) -> bool {
    let Some(level) = star_head(line) else {
        return false;
    };
    // Content-relative: after STAR run (level tokens), required post-star
    // WHITESPACE (may include further spaces/tabs in one lexer run).
    let mut k = level;
    if line.kind(k) != OrgSyntaxKind::WHITESPACE {
        return false;
    }
    k += 1;
    // Extra WHITESPACE tokens after the post-star run (usually none).
    while line.kind(k) == OrgSyntaxKind::WHITESPACE {
        k += 1;
    }
    if line.kind(k) != OrgSyntaxKind::TEXT || !line.text_eq_caseless(k, "END") {
        return false;
    }
    k += 1;
    // Trailing whitespace after END (spaces/tabs; may be multiple tokens).
    while line.kind(k) == OrgSyntaxKind::WHITESPACE {
        k += 1;
    }
    line.kind(k).is_line_end()
}

/// Parse-ready inline task.
///
/// `end_line` is the absolute start of the END line when the END search
/// succeeds (the first later [`star_head`] has a bare END title), or `None`
/// for a degenerate task. It bounds the body for `elements_in` and marks where
/// the END tokens start. List parsing measures its own skip extent over
/// high-star lines and does not use this.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InlineTaskShape {
    /// `Some(start of END line)` when the task is closed; else `None`.
    pub(crate) end_line: Option<Pos>,
}

/// First subsequent any-level star-head line whose title is bare END, or
/// `None` when the first star-head is not END or none exists (degenerate).
///
/// Like org-element, it never skips a non-END star-head to reach a later END.
/// `lines_after` keeps the search inside the current region.
fn find_task_end(open: LineView) -> Option<Pos> {
    for line in open.lines_after() {
        if star_head(line).is_none() {
            continue;
        }
        // First star-head only.
        return is_end_title_line(line).then(|| line.start());
    }
    None
}

/// Parse-ready task for the element dispatcher; `None` for a heading.
pub(crate) fn resolve_task(line: LineView) -> Option<InlineTaskShape> {
    match probe(line)? {
        StarLine::Task(_) => Some(InlineTaskShape {
            end_line: find_task_end(line),
        }),
        StarLine::Heading(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Title anatomy + emit
// ---------------------------------------------------------------------------

/// Shared star-line title anatomy under the open HEADING or INLINE_TASK marker.
///
/// Stars, post-star whitespace, optional TODO/priority/COMMENT, always
/// HEADING_TITLE, optional TAGS, then trailing line whitespace and NEWLINE.
pub(crate) fn emit_star_line_anatomy(parser: &mut Parser) {
    parser.eat_run(OrgSyntaxKind::STAR);

    // After the stars: the lexer produces exactly one WHITESPACE token for the
    // run of spaces/tabs (INDENT is only emitted at line-start, before the first
    // content token of a line; the STARs themselves are that first content).
    parser.eat_run(OrgSyntaxKind::WHITESPACE);

    parse_todo(parser);
    parse_priority(parser);
    parse_comment(parser);

    // Title region: always emit HEADING_TITLE (may be empty) wrapping the
    // inline content up to tags or line term.
    let title_m = parser.start();
    let title_end = find_title_end(parser);
    if title_end > parser.cursor() {
        region::inline_in(parser, title_end, OrgSyntaxKind::HEADING_TITLE);
    }
    title_m.complete(parser, OrgSyntaxKind::HEADING_TITLE);

    if parser.here().is_some_and(tags_group_at) {
        parse_tags(parser);
    }

    // Trailing whitespace on the line (before NL) and the NL itself.
    let _ = parser.eat_to_line_end();
    parser.eat(OrgSyntaxKind::NEWLINE);
}

/// Parse one star line as a `HEADING` green node.
///
/// `line_src` is the star-line slice. `region_nesting` is how many
/// `elements_in` frames were already open in the file (0 for a heading).
/// Diagnostic ranges are relative to `line_src`.
pub(crate) fn recarve_star_line(
    line_src: &str,
    config: &ParseConfig,
    region_nesting: u16,
) -> (GreenNode, Vec<Diagnostic>) {
    let mut parser = Parser::with_config(line_src, config);
    // A fresh parser starts at 0. An inline-task title was carved inside
    // frames that had already charged the region budget.
    parser.set_region_nesting(region_nesting);
    let marker = parser.start();
    emit_star_line_anatomy(&mut parser);
    marker.complete(&mut parser, OrgSyntaxKind::HEADING);
    let (tokens, events, diagnostics) = parser.finish();
    let green = crate::parser::builder::build(line_src, &tokens, events);
    (green, diagnostics)
}

/// Optional planning line + property drawer after a star-line opener.
///
/// Shared metadata adjacency for outline open and closed [`parse_task`]:
/// planning only when the next line is non-blank and `planning::probe`; blanks
/// after planning are tolerated before a property drawer; property drawer only
/// on a non-blank next line. Callers own post-metadata blanks under their
/// container.
///
/// Unlike the `try_parse_*` helpers (timestamp, clock), this never rewinds:
/// it advances only over lines that match.
pub(crate) fn parse_metadata(parser: &mut Parser) {
    let mut had_planning = false;
    if parser.current() != OrgSyntaxKind::EOF
        && !parser.at_blank()
        && parser.line().is_some_and(|l| planning::probe(l).is_some())
    {
        planning::parse(parser);
        had_planning = true;
    }
    if had_planning {
        parser.eat_blank_lines();
    }
    if !parser.at_blank() {
        let shape = parser.line().and_then(drawer::probe_property);
        if let Some(shape) = shape {
            drawer::parse_property(parser, shape);
        }
    }
}

/// Parse one headline line (precondition: line-first and [`StarLine::Heading`]).
///
/// Carve order: optional TODO_KEYWORD, PRIORITY, HEADING_COMMENT, then always
/// HEADING_TITLE, then optional TAGS. Node is always HEADING.
pub(crate) fn parse(parser: &mut Parser) {
    debug_assert!(
        parser
            .line()
            .and_then(probe)
            .and_then(StarLine::as_heading)
            .is_some(),
        "heading::parse called without StarLine::Heading (outline contract)"
    );

    let marker = parser.start();
    emit_star_line_anatomy(parser);
    marker.complete(parser, OrgSyntaxKind::HEADING);
}

/// Emit INLINE_TASK (precondition: decide / [`resolve_task`] on this line).
///
/// Closed tasks (END present) take optional metadata ([`parse_metadata`]),
/// body elements until the END line, then opaque END tiles. Degenerate tasks
/// are the task line plus post-blank only.
pub(crate) fn parse_task(parser: &mut Parser, shape: InlineTaskShape) {
    debug_assert!(
        parser.line().and_then(probe).is_some_and(StarLine::is_task),
        "heading::parse_task called without StarLine::Task"
    );

    let marker = parser.start();
    emit_star_line_anatomy(parser);

    if let Some(end_at) = shape.end_line {
        // As in org-element, metadata is read only when the task has an END.
        parse_metadata(parser);
        parser.eat_blank_lines();
        region::elements_in(parser, end_at);
        debug_assert!(
            parser.line().is_some_and(is_end_title_line),
            "parse_task closed body must land on END line"
        );
        parser.eat_line();
    }

    parser.eat_blank_lines();
    marker.complete(parser, OrgSyntaxKind::INLINE_TASK);
}

/// Exclusive end of the title span (line terminator or tags start).
fn find_title_end(parser: &Parser) -> Pos {
    for t in parser.tokens_from_cursor() {
        if t.kind() == OrgSyntaxKind::NEWLINE || tags_group_at(t) {
            return t.pos();
        }
    }
    parser.limit()
}

// ---------------------------------------------------------------------------
// TODO keyword (config-driven)
// ---------------------------------------------------------------------------

/// Carve an optional TODO_KEYWORD node (if present).
///
/// A leading run of non-terminator tokens is considered only if its total
/// byte length is at most the longest configured keyword. We then take the
/// exact source slice for that run and do a single lookup. This is both
/// simpler and safe against extremely long non-terminated runs.
///
/// On success we emit TODO_KEYWORD over the run and consume the following
/// whitespace so that priority / comment see a clean start.
fn parse_todo(parser: &mut Parser) {
    if parser.at_line_term() {
        return;
    }

    let config = parser.config;
    let max_len = config.max_todo_keyword_len();

    // Fast path: a single token longer than every keyword can never match.
    if max_len == 0 || parser.text().len() > max_len {
        return;
    }

    // Find how far the leading non-terminator run goes, but bounded by max_len.
    // If the run is empty, too long, or never hits a terminator within the
    // bound, we treat the material as ordinary title text.
    let Some(n) = scan_bounded_nonterm_count(parser, max_len) else {
        return;
    };

    let candidate = parser.source_of_n(n);

    if config.todo_kind(candidate).is_none() {
        return;
    }

    // We have a match. Carve the tokens we counted and eat the trailing WS.
    parser.leaf(OrgSyntaxKind::TODO_KEYWORD, n);
    parser.eat_run(OrgSyntaxKind::WHITESPACE);
}

/// Returns the number of leading non-terminator tokens if we reach a
/// terminator (WS/NL/EOF) while the accumulated byte length stays <= `max_len`.
///
/// Returns None if:
/// - the run is empty, or
/// - we would exceed `max_len` before seeing a terminator.
fn scan_bounded_nonterm_count(parser: &Parser, max_len: usize) -> Option<usize> {
    let mut n = 0usize;
    let mut covered = 0usize;

    for t in parser.tokens_from_cursor() {
        match t.kind() {
            OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::NEWLINE => {
                return (n > 0).then_some(n);
            }
            _ => {
                let tlen = t.text().len();
                if covered + tlen > max_len {
                    // This run is already longer than any configured keyword
                    // and we have not yet reached a terminator.
                    return None;
                }
                covered += tlen;
                n += 1;
            }
        }
    }
    // Ran to the visible end: that end is a terminator (EOF).
    (n > 0).then_some(n)
}

// ---------------------------------------------------------------------------
// Priority (purely syntactic)
// ---------------------------------------------------------------------------

/// Try to carve a priority cookie `[#X]` at the current position.
///
/// A valid priority cookie is one of:
/// - `[#A]` .. `[#Z]` (single ASCII uppercase letter)
/// - `[#0]` .. `[#9]` or `[#10]` .. `[#64]` (numbers 0-64 per Org's
///   `org-priority-value-regexp`)
///
/// This is the syntactic form used by the headline parser (see
/// `org-element--headline-parse-title`). The *configured* priority range
/// (from defaults or `#+PRIORITIES:`) does not gate carving here; out-of-range
/// letters are accepted (Org tolerates them at the cookie level). Numbers
/// >64 are never cookies.
///
/// We require the precise token shape `[ # <single TEXT value> ]` because
/// the lexer produces one TEXT token for the consecutive alphanum run, and
/// anything else (wrong length, lowercase, too many digits, symbols, spaces
/// inside) is not a valid cookie and must fall into the title.
fn parse_priority(parser: &mut Parser) {
    let Some(open) = parser.here() else {
        return;
    };
    // Exact token shape `[ # <TEXT value> ]` - rejects "[#]", "[# ]",
    // "[#A extra]", multi-token junk, etc.
    if !open.seq(&[
        OrgSyntaxKind::L_BRACKET,
        OrgSyntaxKind::HASH,
        OrgSyntaxKind::TEXT,
        OrgSyntaxKind::R_BRACKET,
    ]) {
        return;
    }

    let value = open.text_at(2);
    if !PriorityAtom::is_cookie_value(value) {
        // Cookie shape matched but value is not Org's priority grammar; leave
        // as title text and record a diagnostic (range covers `[#VALUE]`).
        parser.error_ahead(
            PRIORITY_COOKIE_TOKENS,
            DiagnosticKind::invalid_priority(value),
        );
        return;
    }

    let m = parser.start();
    parser.eat(OrgSyntaxKind::L_BRACKET);
    parser.eat(OrgSyntaxKind::HASH);
    parser.eat_any(); // the value TEXT
    parser.eat(OrgSyntaxKind::R_BRACKET);
    m.complete(parser, OrgSyntaxKind::PRIORITY);
    parser.eat_run(OrgSyntaxKind::WHITESPACE);
}

// ---------------------------------------------------------------------------
// COMMENT marker (purely syntactic)
// ---------------------------------------------------------------------------

fn parse_comment(parser: &mut Parser) {
    if parser.text() != "COMMENT" {
        return;
    }

    // Per org-element--headline-comment-re (and the parallel TODO check):
    //   (concat "COMMENT" "\\(?: \\|$\\)")
    // Space or EOL only; tab does not qualify. Case is significant.
    if !parser.here().is_some_and(atoms::space_or_line_end) {
        return;
    }

    parser.leaf(OrgSyntaxKind::HEADING_COMMENT, 1);
    parser.eat_run(OrgSyntaxKind::WHITESPACE);
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// The trailing tag group, from its leading `:` to end of line. Precondition:
/// the cursor is at the group's leading `:` ([`tags_group_at`] holds there).
fn parse_tags(parser: &mut Parser) {
    let marker = parser.start();
    let _ = parser
        .eat_while(|p| p.current() == OrgSyntaxKind::COLON || is_tag_atom(p.current(), p.text()));
    marker.complete(parser, OrgSyntaxKind::TAGS);
}

/// Does a trailing tag group begin at token `t`?
///
/// A `:` preceded by narrow WHITESPACE only (not INDENT/NEWLINE), followed by
/// one or more glued tag atoms (or `:`), ending with `:`, followed by
/// whitespace, line end, or the region end.
fn tags_group_at(t: TokenView) -> bool {
    if t.kind() != OrgSyntaxKind::COLON || !t.after_space() {
        return false;
    }

    let mut atoms = 0usize;
    let mut last_was_colon = false;
    let mut cur = t.next();
    while let Some(next) = cur {
        let kind = next.kind();
        if kind != OrgSyntaxKind::COLON && !is_tag_atom(kind, next.text()) {
            // Terminator: group is valid only before whitespace or line end.
            return atoms >= 2
                && last_was_colon
                && matches!(kind, OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::NEWLINE);
        }
        last_was_colon = kind == OrgSyntaxKind::COLON;
        atoms += 1;
        cur = next.next();
    }

    // Ran to the region/source end: that end is a valid group terminator.
    atoms >= 2 && last_was_colon
}

/// Whether a token of `kind` with `text` can contribute to a tag name.
///
/// Org syntax (per worg and the `[:alnum:]_@#%` regexp) allows Unicode
/// alphanumeric characters (letters and decimal numbers per Unicode
/// `General_Category`) in addition to the four ASCII puncts.
///
/// Because the lexer splits `_ @ # %` into their own token kinds, a `TEXT`
/// token contains only other characters; we therefore accept a `TEXT` iff
/// every char is alphanumeric.
fn is_tag_atom(kind: OrgSyntaxKind, text: &str) -> bool {
    match kind {
        OrgSyntaxKind::UNDERSCORE
        | OrgSyntaxKind::AT
        | OrgSyntaxKind::HASH
        | OrgSyntaxKind::PERCENT => true,
        OrgSyntaxKind::TEXT => text.chars().all(char::is_alphanumeric),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiagnosticKind;
    use crate::parser::{ParseConfig, TodoKind};
    use crate::tests::{
        ParseResult, config_with_todo_keywords, diags_any, first_child_of_kind, make_parser,
        node_kinds, node_text, nodes_of_kind, parse_document_tree_with_config, probe_line, run,
        run_with_config, token_count,
    };

    /// Parse a headline line with the default parser and return (tokens, events, diags).
    fn results_of(src: &str) -> ParseResult {
        run(src, parse)
    }

    /// Parse a headline line under a custom ParseConfig (used by most TODO tests).
    fn heading_with_config(src: &str, cfg: &ParseConfig) -> ParseResult {
        run_with_config(src, cfg, parse)
    }

    /// Outline level when [`probe`] classifies a heading.
    fn outline_level(line: LineView<'_, '_>) -> Option<usize> {
        probe(line).and_then(StarLine::as_heading)
    }

    // --- Headline recognition (stars + level) -------------------------------

    #[test]
    fn level_is_the_star_count() {
        let (_, events, _) = results_of("* x\n");
        assert_eq!(token_count(&events, OrgSyntaxKind::STAR), 1);

        let (_, events, _) = results_of("*** Sub\n");
        assert_eq!(token_count(&events, OrgSyntaxKind::STAR), 3);
    }

    #[test]
    fn probe_handles_more_than_u16_max_stars() {
        // 2^16 stars must terminate with a correct usize walk (no wrap/spin)
        // and stay below-threshold-filtered as INLINE_TASK.
        let n = (u16::MAX as usize) + 1;
        let src = format!("{} x\n", "*".repeat(n));
        let p = make_parser(&src);
        // Default inline-task threshold is 15: not a regular outline heading.
        assert!(p.line().and_then(outline_level).is_none());
        assert!(p.line().and_then(probe).is_some_and(StarLine::is_task));
        // Full parse must still terminate and round-trip as INLINE_TASK.
        let out = crate::parse(&src);
        assert_eq!(out.root().text().to_string(), src);
        assert_eq!(
            out.root()
                .descendants()
                .filter(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
                .count(),
            1
        );
        assert_eq!(
            out.root()
                .descendants()
                .filter(|n| n.kind() == OrgSyntaxKind::HEADING)
                .count(),
            0
        );
    }

    #[test]
    fn star_head_requires_a_following_space() {
        // Must be `* ` (org-element-headline-re). No space or tab after stars.
        assert!(probe_line("*foo\n", star_head).is_none());
        assert!(probe_line("*\tx\n", star_head).is_none());
        assert!(probe_line("*foo\n", probe).is_none());
        assert!(probe_line("*\tx\n", probe).is_none());
        // Indented stars are list bullets / text, never star-heads.
        assert!(probe_line("  * indented\n", star_head).is_none());
        assert!(probe_line("  * indented\n", probe).is_none());
        assert!(probe_line("  *************** x\n", probe).is_none());
    }

    #[test]
    fn star_head_returns_star_count() {
        assert_eq!(probe_line("* x\n", star_head), Some(1));
        assert_eq!(probe_line("*** Sub\n", star_head), Some(3));
        assert_eq!(
            probe_line(&format!("{} x\n", "*".repeat(15)), star_head),
            Some(15)
        );
        assert_eq!(
            probe_line(&format!("{} x\n", "*".repeat(16)), star_head),
            Some(16)
        );
    }

    #[test]
    fn recognises_headlines_at_any_line_start() {
        // The probe works on any line the cursor starts, not only offset 0.
        let mut p = make_parser("* H1\n** H2");
        assert_eq!(p.line().and_then(outline_level), Some(1));
        while p.current() != OrgSyntaxKind::NEWLINE {
            p.eat_any();
        }
        p.eat_any();
        assert_eq!(p.line().and_then(outline_level), Some(2));
    }

    // --- TODO carving and title contract -----------------------------------

    #[test]
    fn empty_title_is_stars_and_newline_only() {
        let ev = results_of("* \n");
        // Always emit HEADING_TITLE for the complete shape (may be empty).
        let ks = node_kinds(&ev.1);
        assert!(ks.contains(&OrgSyntaxKind::HEADING) && ks.contains(&OrgSyntaxKind::HEADING_TITLE));
        assert_eq!(token_count(&ev.1, OrgSyntaxKind::STAR), 1);
    }

    #[test]
    fn title_at_eof_without_newline() {
        // Title region ends at the visible limit when no trailing NEWLINE.
        let src = "* Title text";
        let ev = results_of(src);
        let ks = node_kinds(&ev.1);
        assert!(ks.contains(&OrgSyntaxKind::HEADING) && ks.contains(&OrgSyntaxKind::HEADING_TITLE));
        let t = node_text(&ev.1, src, OrgSyntaxKind::HEADING_TITLE);
        assert!(t.as_deref().unwrap_or("").contains("Title text"));
    }

    #[test]
    fn prologue_is_carved_with_config() {
        // Full default mix: TODO (config), priority + comment (syntactic),
        // title (always), tags (structural).
        let src = "* TODO [#A] COMMENT Big Title :a:b:  \n";
        let ev = results_of(src);
        let kinds = node_kinds(&ev.1);
        assert!(kinds.contains(&OrgSyntaxKind::HEADING));
        assert!(kinds.contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(kinds.contains(&OrgSyntaxKind::PRIORITY));
        assert!(kinds.contains(&OrgSyntaxKind::HEADING_COMMENT));
        assert!(kinds.contains(&OrgSyntaxKind::HEADING_TITLE));
        assert!(kinds.contains(&OrgSyntaxKind::TAGS));
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("TODO")
        );
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::PRIORITY).as_deref(),
            Some("[#A]")
        );
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::HEADING_COMMENT).as_deref(),
            Some("COMMENT")
        );
        assert!(
            node_text(&ev.1, src, OrgSyntaxKind::HEADING_TITLE)
                .unwrap()
                .contains("Big Title")
        );
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":a:b:")
        );
    }

    #[test]
    fn non_keyword_word_falls_into_title() {
        let src = "* Hello world :t:\n";
        let ev = results_of(src);
        // No TODO_KEYWORD (Hello not in default), title contains the text.
        let kinds = node_kinds(&ev.1);
        assert!(!kinds.contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(kinds.contains(&OrgSyntaxKind::HEADING_TITLE));
        let t = node_text(&ev.1, src, OrgSyntaxKind::HEADING_TITLE).unwrap();
        assert!(t.contains("Hello world"));
    }

    #[test]
    fn comment_marker_requires_exact_space_not_tab() {
        // The boundary must be a literal ' ' or EOL; a tab does not make
        // "COMMENT" a marker (org-element--headline-comment-re).
        let ev_space = results_of("* COMMENT foo\n");
        assert!(node_kinds(&ev_space.1).contains(&OrgSyntaxKind::HEADING_COMMENT));

        let ev_eol = results_of("* COMMENT\n");
        assert!(node_kinds(&ev_eol.1).contains(&OrgSyntaxKind::HEADING_COMMENT));

        let src_tab = "* COMMENT\tfoo\n";
        let ev_tab = results_of(src_tab);
        let ks = node_kinds(&ev_tab.1);
        assert!(!ks.contains(&OrgSyntaxKind::HEADING_COMMENT));
        assert!(ks.contains(&OrgSyntaxKind::HEADING_TITLE));
        // The "COMMENT" text falls into title when tab follows.
        let t = node_text(&ev_tab.1, src_tab, OrgSyntaxKind::HEADING_TITLE).unwrap();
        assert!(t.contains("COMMENT"));
    }

    #[test]
    fn priority_cookie_must_be_valid_value() {
        // Only well-formed cookies per org-priority-value-regexp become
        // PRIORITY nodes. Garbage, wrong length, lowercase, or out-of-range
        // numbers fall through into the title.
        let good_letter = results_of("* [#A] Task\n");
        assert!(node_kinds(&good_letter.1).contains(&OrgSyntaxKind::PRIORITY));
        assert_eq!(
            node_text(&good_letter.1, "* [#A] Task\n", OrgSyntaxKind::PRIORITY).as_deref(),
            Some("[#A]")
        );

        // Out-of-range letter (default A-C) is still a valid syntactic cookie.
        let out_of_range_letter = results_of("* [#D] Task\n");
        assert!(node_kinds(&out_of_range_letter.1).contains(&OrgSyntaxKind::PRIORITY));

        let good_num = results_of("* [#42] Task\n");
        assert!(node_kinds(&good_num.1).contains(&OrgSyntaxKind::PRIORITY));

        // Two-digit endpoints of org-priority-value-regexp (10..=64).
        let good_10 = results_of("* [#10] Task\n");
        assert!(
            node_kinds(&good_10.1).contains(&OrgSyntaxKind::PRIORITY),
            "[#10] is the low two-digit endpoint"
        );
        let good_64 = results_of("* [#64] Task\n");
        assert!(
            node_kinds(&good_64.1).contains(&OrgSyntaxKind::PRIORITY),
            "[#64] is the high numeric endpoint"
        );

        // Too many digits / too high number: not a cookie.
        let bad_long = results_of("* [#786876876876] Garbage\n");
        let ks = node_kinds(&bad_long.1);
        assert!(!ks.contains(&OrgSyntaxKind::PRIORITY));
        assert!(ks.contains(&OrgSyntaxKind::HEADING_TITLE));
        let title = node_text(
            &bad_long.1,
            "* [#786876876876] Garbage\n",
            OrgSyntaxKind::HEADING_TITLE,
        )
        .unwrap();
        assert!(title.contains("786876876876"));

        // Invalid forms - value-validation path emits InvalidPriorityCookie.
        // `[#]` never reaches it (no TEXT token for the value).
        let bad_empty = results_of("* [#] foo\n");
        assert!(!node_kinds(&bad_empty.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(
            bad_empty.2.is_empty(),
            "empty cookie is not a near-miss value"
        );

        let bad_lower = results_of("* [#a] foo\n");
        assert!(!node_kinds(&bad_lower.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(diags_any(&bad_lower.2, |k| {
            matches!(k, DiagnosticKind::InvalidPriorityCookie { value } if value == "a")
        }));

        let bad_two_digit_leading_zero = results_of("* [#05] foo\n");
        assert!(!node_kinds(&bad_two_digit_leading_zero.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(diags_any(&bad_two_digit_leading_zero.2, |k| {
            matches!(k, DiagnosticKind::InvalidPriorityCookie { value } if value == "05")
        }));

        // One digit zero is a cookie. `[#00]` is not that value.
        let good_zero = results_of("* [#0] foo\n");
        assert!(node_kinds(&good_zero.1).contains(&OrgSyntaxKind::PRIORITY));
        assert_eq!(
            node_text(&good_zero.1, "* [#0] foo\n", OrgSyntaxKind::PRIORITY).as_deref(),
            Some("[#0]")
        );

        // `[#00]` is the same leading-zero rejection as `[#05]`, not number 0.
        let bad_double_zero = results_of("* [#00] foo\n");
        assert!(!node_kinds(&bad_double_zero.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(diags_any(&bad_double_zero.2, |k| {
            matches!(k, DiagnosticKind::InvalidPriorityCookie { value } if value == "00")
        }));

        // First value above the numeric ceiling.
        let bad_65 = results_of("* [#65] foo\n");
        assert!(!node_kinds(&bad_65.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(diags_any(&bad_65.2, |k| {
            matches!(k, DiagnosticKind::InvalidPriorityCookie { value } if value == "65")
        }));

        let bad_too_high_num = results_of("* [#99] foo\n");
        assert!(!node_kinds(&bad_too_high_num.1).contains(&OrgSyntaxKind::PRIORITY));
        assert!(diags_any(&bad_too_high_num.2, |k| {
            matches!(k, DiagnosticKind::InvalidPriorityCookie { value } if value == "99")
        }));
    }

    #[test]
    fn custom_config_carves_todo_keyword_and_splits_title() {
        // With a custom keyword the run is carved as TODO_KEYWORD and the
        // remainder (including a syntactic priority) becomes the title.
        let cfg = config_with_todo_keywords([("NEXT".to_string(), TodoKind::Active)]);
        let src = "* NEXT [#A] Task here :tag:\n";
        let (_toks, evs, _) = heading_with_config(src, &cfg);
        let ks = node_kinds(&evs);
        assert!(ks.contains(&OrgSyntaxKind::HEADING));
        assert!(ks.contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(ks.contains(&OrgSyntaxKind::PRIORITY));
        assert!(ks.contains(&OrgSyntaxKind::HEADING_TITLE));
        assert!(ks.contains(&OrgSyntaxKind::TAGS));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("NEXT")
        );
        let title = node_text(&evs, src, OrgSyntaxKind::HEADING_TITLE).unwrap();
        assert!(title.contains("Task here"));
        assert!(!title.contains("NEXT"));
    }

    /// A config-carved keyword leaves the rest of the anatomy intact: the
    /// priority cookie, title and tags are still carved after the title start
    /// moved past `NEXT`, and no COMMENT marker appears from nowhere.
    #[test]
    fn heading_anatomy_survives_a_config_carved_keyword() {
        let cfg = config_with_todo_keywords([("NEXT".to_string(), TodoKind::Active)]);
        let src = "* NEXT [#A] Big Title :t:\n";
        let root = parse_document_tree_with_config(src, &cfg);
        let heading = nodes_of_kind(&root, OrgSyntaxKind::HEADING)
            .into_iter()
            .next()
            .expect("heading");
        let child = |kind| first_child_of_kind(&heading, kind);

        assert_eq!(
            child(OrgSyntaxKind::TODO_KEYWORD).map(|n| n.text().to_string()),
            Some("NEXT".to_string()),
            "the custom vocabulary carved"
        );
        assert!(child(OrgSyntaxKind::PRIORITY).is_some());
        assert!(child(OrgSyntaxKind::HEADING_COMMENT).is_none());
        assert!(child(OrgSyntaxKind::TAGS).is_some());
        let title = child(OrgSyntaxKind::HEADING_TITLE).expect("title");
        assert!(title.text().to_string().contains("Big Title"));
    }

    /// Grammar contract: **every** `HEADING` carves a `HEADING_TITLE`, empty
    /// ones included (stars plus a space; org-outline requires the space).
    ///
    /// `Heading::title()` panics if this invariant breaks, so the invariant is
    /// pinned here in the grammar rather than in the view that relies on it.
    #[test]
    fn every_heading_carves_a_title_region() {
        let src = "* One\n** Two\n*** \n* Four\n";
        let out = crate::parse(src);
        let headings = nodes_of_kind(out.root(), OrgSyntaxKind::HEADING);
        assert_eq!(headings.len(), 4);
        let titles: Vec<_> = headings
            .iter()
            .map(|h| {
                first_child_of_kind(h, OrgSyntaxKind::HEADING_TITLE)
                    .expect("every HEADING carves a HEADING_TITLE")
            })
            .collect();
        let texts: Vec<String> = titles.iter().map(|t| t.text().to_string()).collect();
        assert!(texts[0].contains("One"));
        assert!(texts[1].contains("Two"));
        // Empty title still returns a HEADING_TITLE node via public API.
        assert!(
            texts[2].trim().is_empty(),
            "stars-only heading must expose empty title via title(): got {:?}",
            texts[2]
        );
        assert!(texts[3].contains("Four"));
    }

    #[test]
    fn case_sensitive_todo_and_glued_boundary() {
        let ev_lower = results_of("* todo foo\n");
        assert!(!node_kinds(&ev_lower.1).contains(&OrgSyntaxKind::TODO_KEYWORD));

        // "TODOfoo" is one run; it does not equal the keyword "TODO".
        let src_glued = "* TODOfoo bar\n";
        let ev_glued = results_of(src_glued);
        assert!(!node_kinds(&ev_glued.1).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(
            node_text(&ev_glued.1, src_glued, OrgSyntaxKind::HEADING_TITLE)
                .unwrap()
                .contains("TODOfoo")
        );
    }

    #[test]
    fn default_done_keyword_is_carved() {
        // The CST only records that DONE is a TODO_KEYWORD node; whether it
        // is a todo or done state is not part of the tree.
        let src = "* DONE Finished\n";
        let ev = results_of(src);
        assert!(node_kinds(&ev.1).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("DONE")
        );
    }

    #[test]
    fn todo_keyword_immediately_before_eol() {
        // Keyword with no whitespace before the line terminator: empty title.
        let src = "* DONE\n";
        let ev = results_of(src);
        assert!(node_kinds(&ev.1).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(node_kinds(&ev.1).contains(&OrgSyntaxKind::HEADING_TITLE));
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::HEADING_TITLE)
                .as_deref()
                .map_or("x", str::trim),
            "",
            "TODO last on the line yields empty HEADING_TITLE"
        );
    }

    #[test]
    fn prefix_keywords_longer_wins() {
        // When keywords are prefixes of each other, only an *exact* match on
        // the whole first run succeeds.
        let cfg = config_with_todo_keywords([
            ("TODO".to_string(), TodoKind::Active),
            ("TODOS".to_string(), TodoKind::Active),
        ]);
        let src_long = "* TODOS something\n";
        let ev_long = heading_with_config(src_long, &cfg);
        assert_eq!(
            node_text(&ev_long.1, src_long, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("TODOS")
        );

        let src_short = "* TODO something\n";
        let ev_short = heading_with_config(src_short, &cfg);
        assert_eq!(
            node_text(&ev_short.1, src_short, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("TODO")
        );
    }

    #[test]
    fn todo_keyword_followed_by_tab() {
        let src = "* NEXT\tTask\n";
        let cfg = config_with_todo_keywords([("NEXT".to_string(), TodoKind::Active)]);
        let ev = heading_with_config(src, &cfg);
        assert!(node_kinds(&ev.1).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("NEXT")
        );
        let title = node_text(&ev.1, src, OrgSyntaxKind::HEADING_TITLE).unwrap();
        assert!(title.contains("Task"));
    }

    // --- Per-keyword matching edge cases ------------------------------------

    #[test]
    fn todo_bound_is_driven_by_configured_keywords() {
        // A long configured keyword is supported.
        let long_kw = "N".repeat(80);
        let cfg = config_with_todo_keywords([(long_kw.clone(), TodoKind::Active)]);
        let src = format!("* {long_kw} task\n");
        let (_t, events, _) = heading_with_config(&src, &cfg);
        assert!(node_kinds(&events).contains(&OrgSyntaxKind::TODO_KEYWORD));

        // A first token longer than every configured keyword is rejected
        // without even trying to match.
        let short_cfg = config_with_todo_keywords([("TODO".to_string(), TodoKind::Active)]);
        let too_long = format!("* {}X task\n", "Y".repeat(100));
        let (_t, ev, _) = heading_with_config(&too_long, &short_cfg);
        assert!(!node_kinds(&ev).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::HEADING_TITLE));
    }

    #[test]
    fn todo_carve_supports_multi_token_candidate_via_concat() {
        // "NEXT/1" is stored as three tokens (TEXT + SLASH + TEXT). We still
        // recognise it via source_of_n + exact match.
        let glued = "NEXT/1";
        let cfg = config_with_todo_keywords([(glued.to_string(), TodoKind::Active)]);
        let src = "* NEXT/1 Task\n";
        let (_toks, evs, _) = heading_with_config(src, &cfg);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::TODO_KEYWORD).as_deref(),
            Some("NEXT/1")
        );
        let title = node_text(&evs, src, OrgSyntaxKind::HEADING_TITLE).unwrap();
        assert!(title.contains("Task"));
        assert!(!title.contains("NEXT/1"));
    }

    // --- Tags ---------------------------------------------------------------

    #[test]
    fn trailing_tags_form_a_tags_node() {
        let src = "* Title :work:project:\n";
        let ev = results_of(src);
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":work:project:")
        );
    }

    #[test]
    fn tags_allow_org_special_chars() {
        let src = "* T :tag_1:@home:#42:%done:\n";
        let ev = results_of(src);
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":tag_1:@home:#42:%done:")
        );
    }

    #[test]
    fn tags_allow_unicode_alphanumerics() {
        // Unicode alphanumerics are allowed in tags (per org-tag--valid-char-set).
        let src = "* T :café:日本語:αβγ:é2:\n";
        let ev = results_of(src);
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":café:日本語:αβγ:é2:")
        );
    }

    #[test]
    fn only_tags_has_empty_title() {
        let src = "* :only:tags:\n";
        let ev = results_of(src);
        assert_eq!(
            node_kinds(&ev.1),
            vec![
                OrgSyntaxKind::HEADING,
                OrgSyntaxKind::HEADING_TITLE,
                OrgSyntaxKind::TAGS
            ]
        );
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":only:tags:")
        );
    }

    #[test]
    fn space_inside_prevents_tag_group() {
        let ev = results_of("* Title :bad tag:\n");
        assert_eq!(
            node_kinds(&ev.1),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::HEADING_TITLE]
        );
    }

    #[test]
    fn title_can_have_colons() {
        // Embedded spaced colons in the title must not cause a Tags node.
        let ev = results_of("* Title : some stuff : with colons\n");
        assert_eq!(
            node_kinds(&ev.1),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::HEADING_TITLE]
        );
    }

    #[test]
    fn tags_require_a_preceding_space() {
        let ev = results_of("* Title:tag:\n");
        assert_eq!(
            node_kinds(&ev.1),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::HEADING_TITLE]
        ); // glued to title -> no Tags
    }

    #[test]
    fn empty_tag_slots_inside_group_are_accepted() {
        // Both leading and trailing empty slots are preserved in the TAGS node.
        let src = "* T :tag::\n";
        let ev = results_of(src);
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":tag::")
        );

        // leading empty slot
        let src = "* T :::tag1:\n";
        let ev = results_of(src);
        assert_eq!(
            node_text(&ev.1, src, OrgSyntaxKind::TAGS).as_deref(),
            Some(":::tag1:")
        );
    }

    #[test]
    fn bare_double_colon_does_not_form_a_tags_group() {
        let ev = results_of("* Headline ::\n");
        assert_eq!(
            node_kinds(&ev.1),
            vec![OrgSyntaxKind::HEADING, OrgSyntaxKind::HEADING_TITLE]
        ); // no Tags
    }

    // --- Inline-task threshold (structural config only) ----------------------

    #[test]
    fn inline_task_threshold_is_the_classification_seam() {
        let cfg = ParseConfig::default().with_inline_task_min_level(2);
        let two = Parser::with_config("** x\n", &cfg);
        let one = Parser::with_config("* x\n", &cfg);
        // One probe partitions by min level: Heading vs Task.
        assert_eq!(one.line().and_then(outline_level), Some(1));
        assert_eq!(two.line().and_then(outline_level), None); // 2 stars >= threshold
        assert_eq!(two.line().and_then(probe), Some(StarLine::Task(2)));

        // Default min level 15: levels 1 and 2 are headlines; 14 accept / 15 task.
        let line = |stars: usize| format!("{} x\n", "*".repeat(stars));
        assert_eq!(probe_line("* x\n", outline_level), Some(1));
        assert_eq!(probe_line("** x\n", outline_level), Some(2));
        assert_eq!(probe_line(&line(14), outline_level), Some(14));
        assert_eq!(probe_line(&line(15), outline_level), None);
        assert_eq!(probe_line(&line(16), outline_level), None);
        assert_eq!(probe_line(&line(15), probe), Some(StarLine::Task(15)));
        assert_eq!(probe_line(&line(16), probe), Some(StarLine::Task(16)));
        assert_eq!(probe_line(&line(14), probe), Some(StarLine::Heading(14)));
    }

    // --- INLINE_TASK emit ----------------------------------------------------

    fn high(n: usize) -> String {
        "*".repeat(n)
    }

    fn n_kind(root: &crate::syntax::SyntaxNode, kind: OrgSyntaxKind) -> usize {
        root.descendants().filter(|n| n.kind() == kind).count()
    }

    fn first_task(root: &crate::syntax::SyntaxNode) -> crate::syntax::SyntaxNode {
        root.descendants()
            .find(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
            .expect("INLINE_TASK")
    }

    #[test]
    fn task_boundary_default_min_level_15() {
        let fourteen = crate::parse(&format!("{} x\n", high(14)));
        let fifteen = crate::parse(&format!("{} x\n", high(15)));
        let sixteen = crate::parse(&format!("{} x\n", high(16)));
        assert_eq!(n_kind(fourteen.root(), OrgSyntaxKind::HEADING), 1);
        assert_eq!(n_kind(fourteen.root(), OrgSyntaxKind::INLINE_TASK), 0);
        assert_eq!(n_kind(fifteen.root(), OrgSyntaxKind::HEADING), 0);
        assert_eq!(n_kind(fifteen.root(), OrgSyntaxKind::INLINE_TASK), 1);
        assert_eq!(n_kind(sixteen.root(), OrgSyntaxKind::HEADING), 0);
        assert_eq!(n_kind(sixteen.root(), OrgSyntaxKind::INLINE_TASK), 1);
        assert_eq!(
            fifteen.root().text().to_string(),
            format!("{} x\n", high(15))
        );
    }

    #[test]
    fn task_boundary_custom_min_level_2() {
        let cfg = ParseConfig::default().with_inline_task_min_level(2);
        let src = "** two\n* one\n";
        let out = crate::parse_with(src, &cfg);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::HEADING), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 1);
        assert_eq!(out.root().text().to_string(), src);
    }

    #[test]
    fn task_title_anatomy_matches_headline_kinds() {
        let src = format!("{} TODO [#A] COMMENT Big Title :a:\n", high(15));
        let out = crate::parse(&src);
        let task = first_task(out.root());
        let kids: Vec<_> = task.children().map(|c| c.kind()).collect();
        assert!(kids.contains(&OrgSyntaxKind::TODO_KEYWORD));
        assert!(kids.contains(&OrgSyntaxKind::PRIORITY));
        assert!(kids.contains(&OrgSyntaxKind::HEADING_COMMENT));
        assert!(kids.contains(&OrgSyntaxKind::HEADING_TITLE));
        assert!(kids.contains(&OrgSyntaxKind::TAGS));
        assert!(!kids.contains(&OrgSyntaxKind::SECTION));
        assert!(!kids.contains(&OrgSyntaxKind::OUTLINE));
        assert!(!kids.contains(&OrgSyntaxKind::HEADING));
    }

    #[test]
    fn task_empty_title_emits_heading_title() {
        let src = format!("{} \n", high(15));
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            task.children()
                .any(|c| c.kind() == OrgSyntaxKind::HEADING_TITLE)
        );
    }

    #[test]
    fn closed_task_body_and_opaque_end() {
        let h = high(15);
        let src = format!("{h} Task\nbody para\n{h} END\n");
        let out = crate::parse(&src);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 1);
        let task = first_task(out.root());
        assert!(
            task.children()
                .any(|c| c.kind() == OrgSyntaxKind::PARAGRAPH),
            "body elements are direct children"
        );
        assert!(
            !task.children().any(|c| c.kind() == OrgSyntaxKind::SECTION),
            "no SECTION under task"
        );
        // END tiles sit under the task (opaque; no END kind).
        let ttext = task.text().to_string();
        assert!(ttext.contains("END"));
        assert!(ttext.contains("body para"));
        assert_eq!(out.root().text().to_string(), src);
    }

    #[test]
    fn degenerate_task_following_paragraph_is_sibling() {
        let h = high(15);
        let src = format!("{h} Task\nfollowing\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            !task.text().to_string().contains("following"),
            "degenerate task does not swallow following paragraph"
        );
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::PARAGRAPH), 1);
    }

    #[test]
    fn closed_task_accepts_low_level_end() {
        let h = high(15);
        let src = format!("{h} Task\nbody\n* END\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(task.text().to_string().contains("body"));
        assert!(task.text().to_string().contains("* END"));
        // Low-level END is consumed by the task, not a HEADING.
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::HEADING), 0);
    }

    #[test]
    fn first_star_head_not_end_is_degenerate() {
        // Outer high-star open, then low headline, then high END: outer
        // degenerate (first any-star_head is not END). Trailing high END is a
        // separate INLINE_TASK with bare END title (degenerate single line).
        let h = high(15);
        let src = format!("{h} Outer\n* Real\n{h} END\n");
        let out = crate::parse(&src);
        let tasks: Vec<_> = out
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
            .collect();
        assert_eq!(tasks.len(), 2, "outer degenerate + trailing END task");
        let outer = &tasks[0];
        assert!(
            !outer.text().to_string().contains("Real"),
            "degenerate outer does not own Real headline"
        );
        assert!(
            !outer.text().to_string().contains("END"),
            "degenerate outer does not own trailing END line"
        );
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::HEADING), 1);
        let end_task = &tasks[1];
        assert!(
            end_task.text().to_string().contains("END"),
            "second task is the high-star END line"
        );
    }

    #[test]
    fn end_title_is_case_insensitive() {
        let h = high(15);
        for end in ["END", "end", "End"] {
            let src = format!("{h} Task\nbody\n{h} {end}\n");
            let out = crate::parse(&src);
            let task = first_task(out.root());
            assert!(
                task.text().to_string().contains("body"),
                "case {end} must close"
            );
            assert_eq!(n_kind(out.root(), OrgSyntaxKind::PARAGRAPH), 1);
        }
    }

    #[test]
    fn end_title_rejects_extra_material() {
        let h = high(15);
        // Tags after END: not bare END title.
        let src = format!("{h} Task\n{h} END :tag:\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            !task.text().to_string().contains(":tag:"),
            "tagged END line is not a closer; outer is degenerate"
        );
        // TODO END is not bare END.
        let src2 = format!("{h} Task\n{h} TODO END\n");
        let out2 = crate::parse(&src2);
        let task2 = first_task(out2.root());
        assert!(!task2.text().to_string().contains("TODO END"));
    }

    #[test]
    fn is_end_title_line_pins() {
        let h = high(15);
        assert!(
            probe_line(&format!("{h} END\n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_some()
        );
        assert!(
            probe_line(&format!("{h} end\n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_some()
        );
        assert!(
            probe_line(&format!("{h}  END  \n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_some()
        );
        // Trailing tab after END is still bare END title (WHITESPACE tile).
        assert!(
            probe_line(&format!("{h} END\t\n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_some()
        );
        assert!(
            probe_line(&format!("{h} END :t:\n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_none()
        );
        assert!(
            probe_line(&format!("{h} TODO END\n"), |l| {
                is_end_title_line(l).then_some(())
            })
            .is_none()
        );
        assert!(probe_line("* END\n", |l| { is_end_title_line(l).then_some(()) }).is_some());
        assert!(probe_line("* Real\n", |l| { is_end_title_line(l).then_some(()) }).is_none());
    }

    #[test]
    fn standalone_end_headline_is_heading_not_task() {
        let out = crate::parse("* END\n");
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::HEADING), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 0);
    }

    #[test]
    fn closed_task_planning_and_property_drawer() {
        let h = high(15);
        let src = format!(
            "{h} Task\nDEADLINE: <2020-01-01 Wed>\n:PROPERTIES:\n:ID: x\n:END:\nbody\n{h} END\n"
        );
        let out = crate::parse(&src);
        let task = first_task(out.root());
        let kids: Vec<_> = task.children().map(|c| c.kind()).collect();
        assert!(kids.contains(&OrgSyntaxKind::PLANNING));
        assert!(kids.contains(&OrgSyntaxKind::PROPERTY_DRAWER));
        assert!(kids.contains(&OrgSyntaxKind::PARAGRAPH));
    }

    #[test]
    fn closed_task_property_drawer_without_planning() {
        let h = high(15);
        let src = format!("{h} Task\n:PROPERTIES:\n:ID: x\n:END:\nbody\n{h} END\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        let kids: Vec<_> = task.children().map(|c| c.kind()).collect();
        assert!(
            kids.contains(&OrgSyntaxKind::PROPERTY_DRAWER),
            "property drawer under closed task"
        );
        assert!(
            !kids.contains(&OrgSyntaxKind::PLANNING),
            "no planning when none present"
        );
        assert!(kids.contains(&OrgSyntaxKind::PARAGRAPH));
    }

    #[test]
    fn degenerate_task_post_blank_under_task() {
        let h = high(15);
        let src = format!("{h} Task\n\nfollowing\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            !task.text().to_string().contains("following"),
            "following paragraph is sibling"
        );
        // Post-blank after degenerate task line is under INLINE_TASK.
        let ttext = task.text().to_string();
        assert!(
            ttext.ends_with("\n\n") || ttext.contains("\n\n"),
            "post-blank owned by task; text={ttext:?}"
        );
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::PARAGRAPH), 1);
    }

    #[test]
    fn closed_task_empty_body_post_blank_under_task() {
        let h = high(15);
        let src = format!("{h} Task\n{h} END\n\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(task.text().to_string().contains("END"));
        // Post-blank after END is under INLINE_TASK.
        assert!(
            task.text().to_string().ends_with("\n\n") || task.text().to_string().ends_with('\n'),
            "task owns trailing blanks; text={:?}",
            task.text().to_string()
        );
        // Source trailing blank after END must round-trip under the task or root.
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            task.text().to_string().ends_with("\n\n"),
            "post-END blank under task"
        );
    }

    #[test]
    fn star_head_inside_src_body_degenerates_outer_task() {
        // First any-star_head in END search is inside lesser body; org first-match
        // makes the outer task degenerate even though a later high END exists.
        let h = high(15);
        let src = format!("{h} Outer\n#+begin_src\n* not end\n#+end_src\n{h} END\n");
        let out = crate::parse(&src);
        let tasks: Vec<_> = out
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
            .collect();
        assert_eq!(tasks.len(), 2, "outer degenerate + trailing END task");
        let outer = &tasks[0];
        assert!(
            outer.text().to_string().contains("Outer"),
            "first task is Outer"
        );
        assert!(
            !outer.text().to_string().contains("not end"),
            "degenerate outer does not own src body"
        );
        assert!(
            !outer.text().to_string().contains("END"),
            "degenerate outer does not own later END"
        );
        let end_task = &tasks[1];
        assert!(
            end_task.text().to_string().contains("END"),
            "second task is the high-star END line"
        );
        assert!(
            !end_task.text().to_string().contains("Outer"),
            "END task is separate from Outer"
        );
    }

    #[test]
    fn degenerate_task_does_not_take_planning() {
        let h = high(15);
        let src = format!("{h} Task\nDEADLINE: <2020-01-01 Wed>\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            !task.children().any(|c| c.kind() == OrgSyntaxKind::PLANNING),
            "planning only when END present"
        );
        // DEADLINE decoy becomes paragraph (planning not in decide).
        assert!(n_kind(out.root(), OrgSyntaxKind::PARAGRAPH) >= 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::PLANNING), 0);
    }

    #[test]
    fn blank_between_task_and_planning_skips_slot() {
        let h = high(15);
        let src = format!("{h} Task\n\nDEADLINE: <2020-01-01 Wed>\n{h} END\n");
        let out = crate::parse(&src);
        let task = first_task(out.root());
        assert!(
            !task.children().any(|c| c.kind() == OrgSyntaxKind::PLANNING),
            "blank breaks planning adjacency"
        );
    }

    #[test]
    fn high_star_interrupts_paragraph() {
        let h = high(15);
        let src = format!("lead paragraph\n{h} Task\n");
        let out = crate::parse(&src);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::PARAGRAPH), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 1);
        let para = out
            .root()
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::PARAGRAPH)
            .expect("para");
        assert!(!para.text().to_string().contains("Task"));
    }

    #[test]
    fn task_in_zeroth_section_then_heading() {
        let h = high(15);
        let src = format!("{h} Task\n* H\n");
        let out = crate::parse(&src);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::HEADING), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::OUTLINE), 1);
        // Task is under zeroth SECTION, not OUTLINE.
        let section = out
            .root()
            .children()
            .find(|c| c.kind() == OrgSyntaxKind::SECTION)
            .expect("zeroth section");
        assert!(
            section
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
        );
    }

    #[test]
    fn task_inside_section_does_not_push_outline() {
        let h = high(15);
        let src = format!("* H\n{h} Task\nbody\n{h} END\n");
        let out = crate::parse(&src);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::OUTLINE), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 1);
        let outline = out
            .root()
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::OUTLINE)
            .expect("outline");
        let section = outline
            .children()
            .find(|c| c.kind() == OrgSyntaxKind::SECTION)
            .expect("section");
        assert!(
            section
                .descendants()
                .any(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
        );
    }

    #[test]
    fn latex_env_section_bound_ignores_interior_task() {
        // High-star task inside latex body must not act as next_heading_after.
        let h = high(15);
        let src = format!("\\begin{{eq}}\n{h} Task\n{h} END\nx\n\\end{{eq}}\n");
        let out = crate::parse(&src);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT), 1);
        assert_eq!(n_kind(out.root(), OrgSyntaxKind::INLINE_TASK), 0);
        // Task is opaque body tiles under the env (BodyMode::Opaque).
        let env = out
            .root()
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LATEX_ENVIRONMENT)
            .expect("env");
        assert!(env.text().to_string().contains("Task"));
        assert!(env.text().to_string().contains("\\end{eq}"));
    }

    #[test]
    fn nested_high_star_open_before_end_degenerates_outer() {
        let h = high(15);
        let src = format!("{h} Outer\n{h} Inner\n{h} END\n");
        let out = crate::parse(&src);
        let tasks: Vec<_> = out
            .root()
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::INLINE_TASK)
            .collect();
        // Outer degenerate (first star-head after open is Inner, not END).
        // Inner closes with the following high END.
        assert_eq!(tasks.len(), 2, "outer degenerate + closed inner");
        assert!(
            !tasks[0].text().to_string().contains("Inner"),
            "outer does not own nested open"
        );
        assert!(
            !tasks[0].text().to_string().contains("END"),
            "outer does not own END"
        );
        let inner = &tasks[1];
        assert!(
            inner.text().to_string().contains("Inner"),
            "second task is Inner"
        );
        assert!(
            inner.text().to_string().contains("END"),
            "inner is closed with END"
        );
    }

    #[test]
    fn resolve_task_end_line_pure() {
        let h = high(15);
        let closed = format!("{h} T\nbody\n{h} END\n");
        let p = make_parser(&closed);
        let shape = p.line().and_then(resolve_task).expect("task");
        assert!(shape.end_line.is_some());

        let deg = format!("{h} T\nbody\n");
        let p2 = make_parser(&deg);
        let shape2 = p2.line().and_then(resolve_task).expect("task");
        assert!(shape2.end_line.is_none());

        let first_not_end = format!("{h} T\n* Real\n{h} END\n");
        let p3 = make_parser(&first_not_end);
        let shape3 = p3.line().and_then(resolve_task).expect("task");
        assert!(
            shape3.end_line.is_none(),
            "first star-head not END => degenerate"
        );
    }

    // TODO carving happens first (config-driven), then the purely syntactic
    // PRIORITY and COMMENT forms. HEADING_TITLE is emitted unconditionally.
    // The order matches org-element--headline-parse-title.
}
