//! Link objects: bracket, angle, and plain written forms.
//!
//! Emits `LINK` / `LINK_PATH` / optional `LINK_DESCRIPTION`. The protocols
//! that plain and angle links recognise come from the parse config
//! (`ProtocolSet` via `parser.config.link_types()`). Target and radio-target
//! definitions (`<<>>` / `<<<>>>`) are parsed in [`super::target`]. Prose that
//! matches a radio target is not turned into a link here: that match depends
//! on targets anywhere in the document, so it is a separate query over the
//! finished tree.
//!
//! **Probe and emit per form:**
//! - Bracket: probe only checks the `[[` opener. When parse fails it leaves
//!   bare `[[` (and any path tokens already eaten under an abandoned marker)
//!   and advances, so no `LINK_PATH` / `LINK_DESCRIPTION` completes without a
//!   completed `LINK`.
//! - Angle: probe carries closer `Pos` (protocol + `>` under org-link-angle-re
//!   continuation rules); parse emits without rescanning for the closer.
//! - Plain: probe is the full PATHPLAIN measure (org-link-plain-re's path,
//!   see the PATHPLAIN section). Emit is `leaf_len` only; when probe is
//!   `None` the dispatcher eats one token. Cursor may sit mid-tile; trailing
//!   punctuation the path does not take is ordinary parent text.
//!
//! Path commit strategy is form-specific (bracket open-marker, angle
//! open-marker, plain measured leaf); the shared invariant is no completed
//! `LINK_PATH` without a completed `LINK`.

use crate::DiagnosticKind;
use crate::parser::event::Marker;
use crate::parser::grammar::drive::{atoms, region};
use crate::parser::{ParseConfig, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// PATHPLAIN - org-link-plain-re's path, without regex
// ---------------------------------------------------------------------------
//
// The path is one or more units followed by a final unit:
//
//   unit   = a character outside `][ \t\n()<>` | group
//   group  = opener (a character outside that set | inner)* closer
//   inner  = opener (a character outside that set)* closer
//   final  = a non-punctuation, non-space character | `-` | `/` | group
//
// with openers `<([` and closers `])>`, mixed freely (`(b]` is a group).
// Groups nest one level. The regexp takes the longest match, so the path is
// the longest run of at least two units whose last unit may be final.
// `\r` also stops a unit: the lexer makes it a line end.

/// A character that is a unit on its own: not a bracket, not whitespace.
fn is_path_char(c: char) -> bool {
    !matches!(
        c,
        '[' | ']' | '(' | ')' | '<' | '>' | ' ' | '\t' | '\n' | '\r'
    )
}

fn is_group_open(c: char) -> bool {
    matches!(c, '<' | '(' | '[')
}

fn is_group_close(c: char) -> bool {
    matches!(c, '>' | ')' | ']')
}

/// Whether a one-character unit may end the path.
///
/// Org's `[^[:punct:][:space:]\n]`, plus `-` and `/`. For ASCII, Emacs's
/// `[:punct:]` is every printable character that is not a letter or digit,
/// and `[:space:]` is whitespace syntax, so control characters other than
/// whitespace qualify. For other characters, `[:punct:]` is anything without
/// word syntax.
fn is_path_final_char(c: char) -> bool {
    match c {
        '-' | '/' => true,
        c if c.is_ascii() => {
            c.is_ascii_alphanumeric()
                || (c.is_ascii_control() && atoms::ascii_syntax(c) != Some(b' '))
        }
        c => atoms::is_word_char(c),
    }
}

/// Byte length of the group opening at `at`, or `None` if none closes there.
fn group_len(src: &str, at: usize) -> Option<usize> {
    let mut chars = src[at..].char_indices();
    let (_, open) = chars.next()?;
    debug_assert!(is_group_open(open));
    let mut inner = false;
    for (i, c) in chars {
        if is_group_close(c) {
            if !inner {
                return Some(i + c.len_utf8());
            }
            inner = false;
        } else if is_group_open(c) {
            if inner {
                return None;
            }
            inner = true;
        } else if !is_path_char(c) {
            return None;
        }
    }
    None
}

/// Exclusive end byte of PATHPLAIN starting at `p0` (just after `TYPE:`),
/// or `None` when no path matches there.
pub(crate) fn pathplain_end(src: &str, p0: usize) -> Option<usize> {
    let mut i = p0;
    let mut units = 0usize;
    let mut end = None;
    while let Some(c) = src.get(i..).and_then(|rest| rest.chars().next()) {
        let (len, may_end) = if is_group_open(c) {
            match group_len(src, i) {
                Some(len) => (len, true),
                None => break,
            }
        } else if is_path_char(c) {
            (c.len_utf8(), is_path_final_char(c))
        } else {
            break;
        };
        i += len;
        units += 1;
        if units >= 2 && may_end {
            end = Some(i);
        }
    }
    end
}

/// Whether `path` reads back as one whole plain link: a registered `TYPE:`
/// and a PATHPLAIN that ends exactly at the end of `path`.
///
/// Operations check this before writing a plain link, so a path whose tail
/// Org would leave outside the link (`https://x.com/a*`) is refused rather
/// than written as a shorter link. What precedes the link is not checked.
pub(crate) fn is_whole_plain_path(path: &str, config: &ParseConfig) -> bool {
    config
        .link_types()
        .match_protocol_prefix(path)
        .is_some_and(|(_, through_colon)| pathplain_end(path, through_colon) == Some(path.len()))
}

// ---------------------------------------------------------------------------
// Public surface: bracket
// ---------------------------------------------------------------------------

/// Bracket link claim: opener `[[` confirmed (path/desc measured at emit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BracketLinkShape;

/// Angle link: protocol + closer position measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AngleLinkShape {
    /// Exclusive end of LINK_PATH (position of `>`).
    pub(crate) closer: Pos,
}

/// Plain link: exclusive path length from the cursor (PATHPLAIN measured).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlainLinkShape {
    pub(crate) path_len: usize,
}

/// Pure probe: token pair at the cursor is `[[`.
pub(crate) fn probe_bracket(parser: &Parser) -> Option<BracketLinkShape> {
    parser
        .here()
        .filter(|t| t.seq(&[OrgSyntaxKind::L_BRACKET, OrgSyntaxKind::L_BRACKET]))
        .map(|_| BracketLinkShape)
}

/// Returns true if the token pair at the cursor is `[[`.
#[cfg(test)]
#[inline]
pub(crate) fn at_link(parser: &Parser) -> bool {
    probe_bracket(parser).is_some()
}

/// Parse a regular bracket link from a measured opener shape.
pub(crate) fn parse_bracket(parser: &mut Parser, _shape: BracketLinkShape) {
    let start = parser.cursor();
    let marker = parser.start();
    eat_double_bracket(parser, OrgSyntaxKind::L_BRACKET);

    // Empty path: immediate `]` or EOF/limit after `[[`.
    if parser.current() == OrgSyntaxKind::EOF || parser.current() == OrgSyntaxKind::R_BRACKET {
        fail_link(parser, marker, start, DiagnosticKind::EmptyLinkTarget);
        return;
    }

    // Non-empty path: first unescaped `]` is the target closer. Absence is
    // UnterminatedLink (empty path already handled above).
    let Some((path_end, after_closer)) =
        find_unescaped_rbracket(parser).map(|t| (t.pos(), t.next().map(|n| n.kind())))
    else {
        fail_link(parser, marker, start, DiagnosticKind::UnterminatedLink);
        return;
    };

    // Eat path under an open marker; complete LINK_PATH only when the whole
    // LINK is known to succeed (no orphan LINK_PATH after fail_link abandon).
    let path_m = parser.start();
    parser.eat_until(path_end);
    debug_assert_eq!(parser.current(), OrgSyntaxKind::R_BRACKET);

    match after_closer {
        Some(OrgSyntaxKind::R_BRACKET) => {
            path_m.complete(parser, OrgSyntaxKind::LINK_PATH);
            eat_double_bracket(parser, OrgSyntaxKind::R_BRACKET);
            marker.complete(parser, OrgSyntaxKind::LINK);
        }
        Some(OrgSyntaxKind::L_BRACKET) => {
            // Validate description closer `]]` before committing LINK_PATH.
            // Cursor is at the target `]`; description body starts two ahead.
            // The closer position is absolute, so it survives the separator
            // eats below.
            let Some(desc_end) = find_double_rbracket(parser, 2).map(|t| t.pos()) else {
                path_m.abandon(parser);
                fail_link(parser, marker, start, DiagnosticKind::UnterminatedLink);
                return;
            };
            path_m.complete(parser, OrgSyntaxKind::LINK_PATH);
            parser.eat_any(); // target-closing ]
            debug_assert_eq!(parser.current(), OrgSyntaxKind::L_BRACKET);
            parser.eat_any(); // description opener [

            // Always emit LINK_DESCRIPTION (possibly empty).
            let desc_m = parser.start();
            region::inline_in(parser, desc_end, OrgSyntaxKind::LINK_DESCRIPTION);
            desc_m.complete(parser, OrgSyntaxKind::LINK_DESCRIPTION);

            eat_double_bracket(parser, OrgSyntaxKind::R_BRACKET);
            marker.complete(parser, OrgSyntaxKind::LINK);
        }
        _ => {
            path_m.abandon(parser);
            fail_link(parser, marker, start, DiagnosticKind::UnterminatedLink);
        }
    }
}

fn fail_link(parser: &mut Parser, marker: Marker, start: Pos, kind: DiagnosticKind) {
    parser.error_span_to_cursor(start, kind);
    marker.abandon(parser);
}

// ---------------------------------------------------------------------------
// Angle
// ---------------------------------------------------------------------------

/// Pure probe: angle link `<type:path>` (not `<<` / `<<<`; protocol registered).
pub(crate) fn probe_angle(parser: &Parser) -> Option<AngleLinkShape> {
    let open = parser.here()?;
    if open.kind() != OrgSyntaxKind::L_ANGLE {
        return None;
    }
    // Longer openers win elsewhere; never claim << or <<<.
    if open.kind_at(1) == OrgSyntaxKind::L_ANGLE {
        return None;
    }
    // Protocol match on source after the opening `<`. rest_source is the
    // visible window from the cursor (mid-tile-aware); strip the opener tile.
    let rest = parser.rest_source();
    let after_open = rest.get(open.text().len()..).unwrap_or("");
    parser
        .config
        .link_types()
        .match_protocol_prefix(after_open)?;
    // Closer under org-link-angle-re continuation rules; carry its Pos.
    let closer = find_angle_closer(parser)?;
    // Empty path `<>` cannot pass (protocol prefix needs body).
    if closer <= parser.cursor() {
        return None;
    }
    Some(AngleLinkShape { closer })
}

/// First `>` closing an angle link path under org continuation rules.
///
/// org-link-angle-re (ol.el): multi-line path allows `\n[ \t]*[^> \t\n][^>\n]*`
/// per continuation - non-blank, and the first non-indent character must not
/// be `>`. A closer that is the first non-indent token after a newline is
/// rejected (e.g. `<https://x\n>` and `<https://x\n   >` are not links).
/// Blank continuations (newline with only indent before the next newline) also
/// fail. Same-line first `>` still closes.
fn find_angle_closer(parser: &Parser) -> Option<Pos> {
    // After a NEWLINE we must see non-indent content before accepting a `>`.
    let mut need_content = false;
    for t in parser.tokens_from_cursor().skip(1) {
        match t.kind() {
            OrgSyntaxKind::NEWLINE => {
                if need_content {
                    // Empty / blank continuation line.
                    return None;
                }
                need_content = true;
            }
            OrgSyntaxKind::INDENT | OrgSyntaxKind::WHITESPACE if need_content => {
                // Indent of the continuation; still waiting for content.
            }
            OrgSyntaxKind::R_ANGLE if need_content => {
                // First non-indent char is `>` - invalid continuation.
                return None;
            }
            OrgSyntaxKind::R_ANGLE => return Some(t.pos()),
            _ => {
                need_content = false;
            }
        }
    }
    None
}

/// Parse angle link from a measured closer (no remeasure).
pub(crate) fn parse_angle(parser: &mut Parser, shape: AngleLinkShape) {
    let marker = parser.start();
    parser.eat_any(); // <
    // Angle always commits path when closer is known (no abandon-after-path).
    parser.leaf_to(OrgSyntaxKind::LINK_PATH, shape.closer);
    debug_assert_eq!(parser.current(), OrgSyntaxKind::R_ANGLE);
    parser.eat_any(); // >
    marker.complete(parser, OrgSyntaxKind::LINK);
}

// ---------------------------------------------------------------------------
// Plain
// ---------------------------------------------------------------------------

/// Shared opener for plain links: word-start context + registered `TYPE:`.
///
/// Returns byte length through the colon, or `None` when PRE/protocol fail.
fn plain_protocol_open(parser: &Parser) -> Option<usize> {
    if !atoms::at_word_start(parser) || parser.current() == OrgSyntaxKind::EOF {
        return None;
    }
    let s = parser.rest_source();
    let (_ty, through_colon) = parser.config.link_types().match_protocol_prefix(s)?;
    Some(through_colon)
}

/// Full plain-path measure at the cursor: opener + PATHPLAIN.
///
/// Returns exclusive path length from the visible start, or `None`. Org has
/// no condition on what follows the path: the regexp already stops before
/// any trailing punctuation it does not take.
fn measure_plain_path(parser: &Parser) -> Option<usize> {
    let through_colon = plain_protocol_open(parser)?;
    pathplain_end(parser.rest_source(), through_colon)
}

/// Pure probe: full PATHPLAIN measure (no broader cheap prefilter).
///
/// When probe returns `None`, the dispatcher eats one token; parse has no
/// fallback of its own.
pub(crate) fn probe_plain(parser: &Parser) -> Option<PlainLinkShape> {
    measure_plain_path(parser).map(|path_len| PlainLinkShape { path_len })
}

/// True when a full plain link measures at the cursor.
#[cfg(test)]
#[inline]
pub(crate) fn at_plain_link(parser: &Parser) -> bool {
    probe_plain(parser).is_some()
}

/// Parse plain link from a measured path length (no remeasure).
///
/// Recipe: `start` -> `leaf_len(path_len)` -> `complete(LINK)`. Path end is
/// exclusive of trailing punctuation the path cannot end on. Mid-tile ends leave
/// the cursor on the trail; the parent object loop emits trail as ordinary text.
pub(crate) fn parse_plain(parser: &mut Parser, shape: PlainLinkShape) {
    let marker = parser.start();
    // path_len is the exclusive length from rest_source under this cursor and
    // limit; leaf_len uses the same end, so it cannot refuse here.
    let ok = parser.leaf_len(OrgSyntaxKind::LINK_PATH, shape.path_len);
    debug_assert!(ok, "plain link path fits the length probe measured");
    if !ok {
        // Unreachable; recover by consuming one token.
        marker.abandon(parser);
        parser.eat_any();
        return;
    }
    marker.complete(parser, OrgSyntaxKind::LINK);
}

/// Harness: probe then parse (unit tests / local walks).
#[cfg(test)]
pub(crate) fn parse_plain_link(parser: &mut Parser) {
    if let Some(shape) = probe_plain(parser) {
        parse_plain(parser, shape);
    }
}

/// Harness: probe then parse bracket link.
#[cfg(test)]
pub(crate) fn parse_link(parser: &mut Parser) {
    if let Some(shape) = probe_bracket(parser) {
        parse_bracket(parser, shape);
    }
}

// ---------------------------------------------------------------------------
// Scans
// ---------------------------------------------------------------------------

/// First unescaped `]` before EOF/limit (newlines OK), as a token view.
///
/// Escape rule (PATHREG-style): a `]` is escaped when an **odd** run of
/// consecutive `BACKSLASH` tokens immediately precedes it. Even length
/// (including zero) leaves `]` unescaped -- so `\\]` closes, `\]` does not.
fn find_unescaped_rbracket<'a, 's>(parser: &'a Parser<'s>) -> Option<TokenView<'a, 's>> {
    parser
        .tokens_from_cursor()
        .find(|t| is_unescaped_rbracket(*t))
}

/// Whether `t` is an unescaped `R_BRACKET`.
fn is_unescaped_rbracket(t: TokenView) -> bool {
    if t.kind() != OrgSyntaxKind::R_BRACKET {
        return false;
    }
    // Count consecutive BACKSLASH tokens immediately before it (lexer emits
    // each `\` as its own token).
    let mut n_bs = 0usize;
    let mut cur = t.prev();
    while let Some(prev) = cur {
        if prev.kind() != OrgSyntaxKind::BACKSLASH {
            break;
        }
        n_bs += 1;
        cur = prev.prev();
    }
    n_bs.is_multiple_of(2)
}

/// First literal `]]` opener at or after `skip` tokens ahead (no escape).
fn find_double_rbracket<'a, 's>(parser: &'a Parser<'s>, skip: usize) -> Option<TokenView<'a, 's>> {
    parser
        .tokens_from_cursor()
        .skip(skip)
        .find(|t| t.run_of(OrgSyntaxKind::R_BRACKET, 2))
}

fn eat_double_bracket(parser: &mut Parser, kind: OrgSyntaxKind) {
    debug_assert_eq!(parser.current(), kind);
    parser.eat_any();
    debug_assert_eq!(parser.current(), kind);
    parser.eat_any();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parse;
    use crate::tests::{
        ParseResult, diags_any, make_parser, node_count, node_kinds, node_text, nodes_of_kind,
        parse_document_tree, run, run_object, tree_text,
    };

    fn results_of(src: &str) -> ParseResult {
        run_object(src, probe_bracket, parse_bracket)
    }

    fn parses_as_link(src: &str) -> bool {
        node_kinds(&results_of(src).1).contains(&OrgSyntaxKind::LINK)
    }

    fn has_path(src: &str) -> bool {
        node_kinds(&results_of(src).1).contains(&OrgSyntaxKind::LINK_PATH)
    }

    /// LINK_PATH text of the first LINK under a full document parse.
    fn first_link_path(src: &str) -> Option<String> {
        let root = parse(src).root().clone();
        nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .and_then(|link| {
                link.children()
                    .find(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
                    .map(|n| n.text().to_string())
            })
    }

    fn link_count(src: &str) -> usize {
        nodes_of_kind(parse(src).root(), OrgSyntaxKind::LINK).len()
    }

    #[test]
    fn plain_bracket_carves_path() {
        let src = "[[https://example.com]]";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK));
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK_PATH));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK).as_deref(),
            Some("[[https://example.com]]")
        );
    }

    #[test]
    fn link_with_description() {
        let src = "[[https://example.com][site]]";
        let (_toks, evs, _) = results_of(src);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::LINK));
        assert!(kinds.contains(&OrgSyntaxKind::LINK_PATH));
        assert!(kinds.contains(&OrgSyntaxKind::LINK_DESCRIPTION));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK_DESCRIPTION).as_deref(),
            Some("site")
        );
        // Path is nested under LINK (full-doc child walk).
        assert_eq!(first_link_path(src).as_deref(), Some("https://example.com"));
        let root = parse(src).root().clone();
        let link = nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("link");
        assert!(
            link.children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
        );
        assert!(
            link.children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_DESCRIPTION)
        );
    }

    #[test]
    fn empty_description_emits_node() {
        let src = "[[t][]]";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK));
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK_DESCRIPTION));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("t")
        );
        let desc_count = node_kinds(&evs)
            .iter()
            .filter(|&&k| k == OrgSyntaxKind::LINK_DESCRIPTION)
            .count();
        assert_eq!(desc_count, 1);
        assert_eq!(first_link_path(src).as_deref(), Some("t"));
        let root = parse(src).root().clone();
        let link = nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("link");
        assert!(
            link.children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
        );
        assert!(
            link.children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_DESCRIPTION)
        );
    }

    #[test]
    fn multiline_path_allowed() {
        let src = "[[a\nb]]";
        assert!(parses_as_link(src));
        assert!(has_path(src));
    }

    #[test]
    fn whitespace_only_path_is_valid_fuzzy() {
        // Pin: whitespace-only is valid LINK (not EmptyLinkTarget).
        assert!(parses_as_link("[[ ]]"));
        assert!(has_path("[[ ]]"));
        let (_, _, diags) = results_of("[[ ]]");
        assert!(!diags_any(&diags, |k| {
            matches!(k, crate::DiagnosticKind::EmptyLinkTarget)
        }));
    }

    #[test]
    fn target_stops_at_first_unescaped_rbracket() {
        assert!(!parses_as_link("[[foo]bar]]"));
    }

    #[test]
    fn escaped_rbracket_is_preserved_in_target() {
        // Odd backslash run: `\]` does not close the path.
        let src = "[[foo\\]bar]]";
        let (_toks, evs, _) = results_of(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK).as_deref(),
            Some("[[foo\\]bar]]")
        );
    }

    #[test]
    fn even_backslash_run_does_not_escape_rbracket() {
        // PATHREG even/odd: `\\]` (two backslashes) leaves `]` unescaped.
        // Org source [[foo\\]] -- path is foo\\ then closing ]].
        let src = "[[foo\\\\]]";
        assert!(parses_as_link(src), "even BS run must allow closer");
        let (_toks, evs, _) = results_of(src);
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK).as_deref(),
            Some("[[foo\\\\]]")
        );
        // Four backslashes before closer: still even -> unescaped.
        let src4 = "[[foo\\\\\\\\]]";
        assert!(parses_as_link(src4));
        // Odd interior escape then real closer: [[a\]b]] path includes \]b.
        let src_odd = "[[a\\]b]]";
        assert!(parses_as_link(src_odd));
        let (_toks, evs, _) = results_of(src_odd);
        assert_eq!(
            node_text(&evs, src_odd, OrgSyntaxKind::LINK).as_deref(),
            Some("[[a\\]b]]")
        );
    }

    #[test]
    fn empty_target_is_invalid() {
        assert!(!parses_as_link("[[]]"));
        let (_, _, diags) = results_of("[[]]");
        assert!(diags_any(&diags, |k| {
            matches!(k, crate::DiagnosticKind::EmptyLinkTarget)
        }));
    }

    #[test]
    fn empty_target_immediate_eof_or_newline_after_opener() {
        assert!(!parses_as_link("[["));
        let (_, _, diags) = results_of("[[");
        assert!(diags_any(&diags, |k| matches!(
            k,
            crate::DiagnosticKind::EmptyLinkTarget
        )));
        // multiline empty-ish: [[\n]] is whitespace path -> valid
        assert!(parses_as_link("[[\n]]"));
    }

    #[test]
    fn unterminated_link_consumes_opener() {
        let mut p = make_parser("[[no close");
        let before = p.cursor().index();
        if at_link(&p) {
            parse_link(&mut p);
        }
        assert!(p.cursor().index() > before);
        let events = p.finish().1;
        assert!(!node_kinds(&events).contains(&OrgSyntaxKind::LINK));
    }

    #[test]
    fn unterminated_link_emits_diagnostic() {
        for src in ["[[no close", "[[t][desc", "[[t]oops", "[[t][desc]"] {
            let (_toks, evs, diags) = results_of(src);
            assert!(
                !node_kinds(&evs).contains(&OrgSyntaxKind::LINK),
                "expected no LINK for {src:?}"
            );
            assert!(
                diags_any(&diags, |k| {
                    matches!(k, crate::DiagnosticKind::UnterminatedLink)
                }),
                "expected UnterminatedLink for {src:?}, diags={diags:?}"
            );
        }
    }

    /// Failed bracket forms must not leave orphan LINK_PATH / LINK_DESCRIPTION
    /// nodes, as with failed inline babel calls.
    ///
    /// Progress table:
    /// - empty / unclosed target (no path body eaten) -> +2 past `[[`
    /// - path then fail (desc missing or non-`]`/`][` after path) -> cursor on
    ///   first target `]` (= +2 + path token count)
    #[test]
    fn bracket_fail_path_progress_and_no_orphans() {
        // (src, expected pos delta from entry)
        let cases: &[(&str, usize)] = &[
            ("[[", 2),
            ("[[]]", 2),
            ("[[no close", 2),
            // path body is single TEXT token "t"
            ("[[t]oops", 3),
            ("[[t][desc", 3),
            ("[[t][desc]", 3),
        ];
        for &(src, advance) in cases {
            let mut p = make_parser(src);
            assert!(at_link(&p), "looking-at should hit for {src:?}");
            let before = p.cursor().index();
            parse_link(&mut p);
            assert_eq!(
                p.cursor().index(),
                before + advance,
                "exact progress for {src:?}: expected +{advance}"
            );
            let (_toks, evs, _) = p.finish();
            assert_eq!(
                node_count(&evs, OrgSyntaxKind::LINK),
                0,
                "no LINK for {src:?}"
            );
            assert_eq!(
                node_count(&evs, OrgSyntaxKind::LINK_PATH),
                0,
                "no orphan LINK_PATH for {src:?}"
            );
            assert_eq!(
                node_count(&evs, OrgSyntaxKind::LINK_DESCRIPTION),
                0,
                "no orphan LINK_DESCRIPTION for {src:?}"
            );
        }

        // Full-document: orphans must not surface; UnterminatedLink still fires.
        for src in ["[[t][desc\n", "[[t]oops\n", "[[no close\n"] {
            let out = parse(src);
            assert_eq!(
                nodes_of_kind(out.root(), OrgSyntaxKind::LINK).len(),
                0,
                "doc: no LINK for {src:?}"
            );
            assert_eq!(
                nodes_of_kind(out.root(), OrgSyntaxKind::LINK_PATH).len(),
                0,
                "doc: no orphan LINK_PATH for {src:?}"
            );
            assert_eq!(
                nodes_of_kind(out.root(), OrgSyntaxKind::LINK_DESCRIPTION).len(),
                0,
                "doc: no orphan LINK_DESCRIPTION for {src:?}"
            );
            assert!(
                diags_any(out.diagnostics(), |k| {
                    matches!(k, crate::DiagnosticKind::UnterminatedLink)
                }),
                "doc: UnterminatedLink for {src:?}, diags={:?}",
                out.diagnostics()
            );
        }
    }

    /// Failure diagnostic spans cover the consumed tokens (entry..cursor).
    #[test]
    fn fail_link_diagnostic_spans_cover_consumed() {
        // EmptyLinkTarget after `[[` sitting on first `]`.
        let src = "[[]]";
        let (_, _, diags) = results_of(src);
        let d = diags
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::EmptyLinkTarget))
            .expect("EmptyLinkTarget");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "[[", "empty target span covers openers");

        // Unterminated with no path body: openers only.
        let src = "[[no close";
        let (_, _, diags) = results_of(src);
        let d = diags
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::UnterminatedLink))
            .expect("UnterminatedLink");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "[[", "unclosed span covers openers");

        // Path eaten under abandoned path_m: span includes path text.
        let src = "[[t]oops";
        let (_, _, diags) = results_of(src);
        let d = diags
            .iter()
            .find(|d| matches!(d.kind(), crate::DiagnosticKind::UnterminatedLink))
            .expect("UnterminatedLink");
        let s = usize::from(d.range().start());
        let e = usize::from(d.range().end());
        assert_eq!(&src[s..e], "[[t", "path-then-fail span covers path");
    }

    #[test]
    fn find_double_rbracket_direct() {
        fn found_at(src: &str, skip: usize) -> Option<usize> {
            let p = make_parser(src);
            find_double_rbracket(&p, skip).map(|t| t.pos().index())
        }
        // skip=0: `]]` at start.
        assert_eq!(found_at("]]", 0), Some(0));
        // skip=0: skip single `]` then find pair in `]x]]`.
        assert_eq!(found_at("]x]]", 0), Some(2));
        // Description-shaped after target `]`: cursor on first `]`, body
        // starts two ahead -> `][x]]` finds closer at 3.
        assert_eq!(found_at("][x]]", 2), Some(3));
        // skip=2 on `][]]` empty desc: closer at 2.
        assert_eq!(found_at("][]]", 2), Some(2));
        // Missing closer.
        assert_eq!(found_at("][x]", 2), None);
        assert_eq!(found_at("][x", 2), None);
        assert_eq!(found_at("abc", 0), None);
    }

    #[test]
    fn description_no_nested_regular_link() {
        let src = "[[t][see [[nested]]]]";
        let kinds = node_kinds(&results_of(src).1);
        assert!(kinds.contains(&OrgSyntaxKind::LINK));
        let link_count = kinds.iter().filter(|&&k| k == OrgSyntaxKind::LINK).count();
        assert_eq!(link_count, 1);
    }

    #[test]
    fn pathplain_trailing_dot() {
        let end = pathplain_end("https://x.com.", 6).unwrap();
        assert_eq!(&"https://x.com."[6..end], "//x.com");
    }

    #[test]
    fn pathplain_end_edges() {
        // Empty, out of range, or a non-unit at the start.
        assert_eq!(pathplain_end("", 0), None);
        assert_eq!(pathplain_end("abc", 10), None);
        assert_eq!(pathplain_end(" ", 0), None);
        assert_eq!(pathplain_end("]x", 0), None);
        // At least two units.
        assert_eq!(pathplain_end("a", 0), None);
        assert_eq!(pathplain_end("ab", 0), Some(2));
        assert_eq!(pathplain_end("(a)", 0), None, "one group is one unit");
        assert_eq!(pathplain_end("(a)b", 0), Some(4));
        assert_eq!(
            pathplain_end("[a]b", 0),
            Some(4),
            "a group may open the path"
        );
        // Groups: any opener with any closer, one level of nesting.
        assert_eq!(pathplain_end("a(b]", 0), Some(4));
        assert_eq!(pathplain_end("a(b(c)d)e", 0), Some(9));
        assert_eq!(pathplain_end("a((b))", 0), Some(6));
        assert_eq!(
            pathplain_end("ab(((c)))", 0),
            Some(2),
            "too deep: path ends"
        );
        assert_eq!(pathplain_end("ab(c", 0), Some(2), "unclosed: path ends");
        assert_eq!(pathplain_end("ab(c d)", 0), Some(2), "space in a group");
        assert_eq!(pathplain_end("ab)c", 0), Some(2), "stray closer");
        // Final unit: no trailing punctuation except `-` and `/`.
        assert_eq!(pathplain_end("ab.", 0), Some(2));
        assert_eq!(pathplain_end("ab*_%", 0), Some(2));
        assert_eq!(pathplain_end("ab-", 0), Some(3));
        assert_eq!(pathplain_end("ab/", 0), Some(3));
        assert_eq!(pathplain_end("a.b", 0), Some(3), "inner punctuation stays");
        assert_eq!(pathplain_end("...", 0), None);
        // `\r` ends a unit, as the lexer ends the line there.
        assert_eq!(pathplain_end("ab\r\n", 0), Some(2));
        // Non-ASCII: a word character may end the path, punctuation not.
        assert_eq!(pathplain_end("a\u{e9}", 0), Some(3));
        assert_eq!(pathplain_end("ab\u{bb}", 0), Some(2));
        assert_eq!(pathplain_end("a\u{a0}b", 0), Some(4), "NBSP inside stays");
    }

    #[test]
    fn link_object_appears_inside_paragraph() {
        let src = "see [[https://ex][here]] now\n";
        let (_toks, evs, _) =
            crate::tests::run(src, crate::parser::grammar::forms::paragraph::parse);
        let kinds = node_kinds(&evs);
        assert!(kinds.contains(&OrgSyntaxKind::PARAGRAPH));
        assert!(kinds.contains(&OrgSyntaxKind::LINK));
        assert!(kinds.contains(&OrgSyntaxKind::LINK_PATH));
        assert!(kinds.contains(&OrgSyntaxKind::LINK_DESCRIPTION));
    }

    // --- Full-document grammar fixtures (CST only; no typed link views) ------

    #[test]
    fn multiline_bracket_path_and_empty_description() {
        let src = "[[a\nb][]]\n";
        let root = parse(src).root().clone();
        let links = nodes_of_kind(&root, OrgSyntaxKind::LINK);
        assert_eq!(links.len(), 1);
        assert!(
            links[0]
                .children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
        );
        assert!(
            links[0]
                .children()
                .any(|n| n.kind() == OrgSyntaxKind::LINK_DESCRIPTION)
        );
    }

    #[test]
    fn nested_bracket_in_description_not_second_link() {
        let src = "[[t][see [[nested]]]]\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);
    }

    #[test]
    fn no_timestamp_object_inside_description() {
        let src = "[[t][<2020-01-01>]]\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).len(), 0);
    }

    #[test]
    fn description_rejects_target_objects() {
        let src = "[[t][see <<dest>> and <<<radio>>>]]\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TARGET).len(), 0);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET).len(), 0);
    }

    #[test]
    fn angle_vs_timestamp() {
        let src = "see <https://ex.com> and <2020-01-01>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::LINK).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TIMESTAMP).len(), 1);
        assert_eq!(first_link_path(src).as_deref(), Some("https://ex.com"));
    }

    /// org-link-angle-re continuation: first non-indent after newline must not
    /// be `>` and the continuation must be non-blank.
    #[test]
    fn angle_link_continuation_policy() {
        // Happy multi-line: content after indent on the continuation.
        let ok = "a <https://x\nnext>\n";
        assert_eq!(
            nodes_of_kind(&parse_document_tree(ok), OrgSyntaxKind::LINK).len(),
            1,
            "content continuation must form a link"
        );
        assert_eq!(
            first_link_path(ok).as_deref(),
            Some("https://x\nnext"),
            "path includes newline and continuation"
        );

        let ok_indent = "a <https://x\n\ty>\n";
        assert_eq!(
            nodes_of_kind(&parse_document_tree(ok_indent), OrgSyntaxKind::LINK).len(),
            1,
            "indented non-`>` content continuation must form a link"
        );

        // First non-indent of a continuation is `>`, or the continuation is
        // blank: no angle link. As in Org, the path after `<` is then a plain
        // link that stops at the newline.
        for (src, why) in [
            ("a <https://x\n   >\n", "`\\n   >`"),
            ("a <https://x\n>\n", "`\\n>`"),
            ("a <https://x\n\n>\n", "blank continuation"),
        ] {
            let links: Vec<String> = nodes_of_kind(&parse_document_tree(src), OrgSyntaxKind::LINK)
                .iter()
                .map(|n| n.text().to_string())
                .collect();
            assert_eq!(
                links,
                ["https://x"],
                "{why}: plain link only, no angle link"
            );
        }
    }

    #[test]
    fn find_angle_closer_direct() {
        fn closer_idx(src: &str) -> Option<usize> {
            let p = make_parser(src);
            // Cursor at `<`.
            find_angle_closer(&p).map(crate::parser::pos::Pos::index)
        }
        // Happy paths: exact closer token index (after opener + path tiles).
        let one_line = closer_idx("<https://x>").expect("same-line closer");
        assert!(one_line > 0, "closer is after opener");
        let multi = closer_idx("<https://x\nnext>").expect("multi-line closer");
        assert!(multi > one_line, "multi-line closer is further along");
        let indented = closer_idx("<https://x\n\ty>").expect("indented continuation");
        assert!(indented > 0);

        // Rejects.
        assert_eq!(closer_idx("<https://x\n>"), None);
        assert_eq!(closer_idx("<https://x\n   >"), None);
        assert_eq!(closer_idx("<https://x\n\n>"), None, "blank continuation");
        assert_eq!(closer_idx("<https://x"), None, "unclosed");
    }

    #[test]
    fn plain_trailing_dot_and_pre_mid_word() {
        let src = "Visit https://x.com. next\n";
        assert_eq!(link_count(src), 1);
        assert_eq!(first_link_path(src).as_deref(), Some("https://x.com"));
        let root = parse(src).root().clone();
        let link = nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("plain link");
        let slice = link.text().to_string();
        assert_eq!(slice, "https://x.com");
        assert!(!slice.ends_with('.'), "link span must exclude trailing dot");
        assert!(!slice.contains(" next"), "link span leaked: {slice:?}");

        // Mid-word: not a plain link (word-start PRE).
        assert_eq!(link_count("seehttps://x.com\n"), 0);
    }

    #[test]
    fn full_document_sequential_plain_links_with_trails() {
        let src = "See https://a.com. and https://b.com.\n";
        let root = parse(src).root().clone();
        let links = nodes_of_kind(&root, OrgSyntaxKind::LINK);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].text().to_string(), "https://a.com");
        assert_eq!(links[1].text().to_string(), "https://b.com");
        // Each trail is a bare token sibling under the paragraph parent.
        for link in &links {
            let parent = link.parent().expect("parent");
            let children: Vec<_> = parent.children_with_tokens().collect();
            let idx = children
                .iter()
                .position(|c| {
                    c.as_node()
                        .is_some_and(|n| n.text_range() == link.text_range())
                })
                .expect("link among siblings");
            assert_eq!(
                children.get(idx + 1).expect("trail").to_string(),
                ".",
                "PATHPLAIN trail sibling after {}",
                link.text()
            );
        }
    }

    #[test]
    fn plain_multi_char_trail_strip_full_parse() {
        let src = "https://x.com!?\n";
        assert_eq!(first_link_path(src).as_deref(), Some("https://x.com"));
        let root = parse(src).root().clone();
        let link = nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("LINK");
        assert_eq!(link.text().to_string(), "https://x.com");
        let parent = link.parent().expect("parent");
        let mut after = parent
            .children_with_tokens()
            .skip_while(|c| c.as_node().is_none_or(|n| n.kind() != OrgSyntaxKind::LINK));
        let _ = after.next();
        // Multi-char trail may be one or more bare tokens; joined text is "!?".
        let mut trail = String::new();
        for e in after {
            if e.as_node().is_some() {
                break;
            }
            trail.push_str(&e.to_string());
            if trail == "!?" {
                break;
            }
        }
        assert_eq!(trail, "!?", "multi-char PATHPLAIN trail under parent");
    }

    fn results_of_plain(src: &str) -> ParseResult {
        run(src, |p| {
            if at_plain_link(p) {
                parse_plain_link(p);
            } else if p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
            // Drain remainder (PATHPLAIN trail) as bare tokens under the open root.
            while p.current() != OrgSyntaxKind::EOF {
                p.eat_any();
            }
        })
    }

    #[test]
    fn parse_plain_link_guarded_mid_trail_and_non_match() {
        // Mid-tile trail: path ends before the final '.' inside the enclosing TEXT.
        let src = "https://x.com.";
        let mut p = make_parser(src);
        assert!(at_plain_link(&p));
        let before = p.progress();
        parse_plain_link(&mut p);
        assert_ne!(p.progress(), before, "plain link advances source cursor");
        assert_eq!(p.text(), ".", "cursor left on trail remainder");
        let (toks, evs, _) = results_of_plain(src);
        assert!(node_kinds(&evs).contains(&OrgSyntaxKind::LINK));
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK_PATH).as_deref(),
            Some("https://x.com")
        );
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::LINK).as_deref(),
            Some("https://x.com")
        );
        assert_eq!(toks.len(), lex(src).tokens.len());

        // Non-match: mid-word protocol is not a plain link; harness eats one token.
        let src2 = "seehttps://x.com";
        let (toks2, evs2, _) = results_of_plain(src2);
        assert!(!node_kinds(&evs2).contains(&OrgSyntaxKind::LINK));
        assert_eq!(toks2.len(), lex(src2).tokens.len());
    }

    /// The probe rejects paths that fail PATHPLAIN or the trail strip; there
    /// is no separate prefilter that could disagree with the measure.
    #[test]
    fn probe_plain_rejects_pathplain_misses() {
        // Trail-only body and a one-unit body after TYPE: - probe is exact.
        for src in ["https:...", "https:.,;", "https:x", "https:(a)"] {
            assert!(
                probe_plain(&make_parser(src)).is_none(),
                "full probe must reject {src:?}"
            );
            let (_toks, evs, _) = results_of_plain(src);
            let kinds = node_kinds(&evs);
            assert!(
                !kinds.contains(&OrgSyntaxKind::LINK),
                "no LINK on probe miss {src:?}"
            );
            assert!(
                !kinds.contains(&OrgSyntaxKind::LINK_PATH),
                "no LINK_PATH on probe miss {src:?}"
            );
        }
    }

    /// Body-start matrix: empty / ws / unclosed openers reject; a path needs
    /// two units, and a group may open it.
    #[test]
    fn at_plain_link_body_start_matrix() {
        assert!(!at_plain_link(&make_parser("https:")), "empty body");
        assert!(!at_plain_link(&make_parser("https: ")), "ws body");
        assert!(!at_plain_link(&make_parser("https:\t")), "tab body");
        assert!(!at_plain_link(&make_parser("https:[")), "bracket body");
        assert!(!at_plain_link(&make_parser("https:<")), "angle body");
        assert!(!at_plain_link(&make_parser("https:x")), "one unit");
        assert!(at_plain_link(&make_parser("https:xy")), "two units");
        assert!(at_plain_link(&make_parser("https:[a]b")), "group then unit");
        // Trail-only body measures empty after PATHPLAIN trail strip.
        assert!(
            !at_plain_link(&make_parser("https:.")),
            "trail-only fails full PATHPLAIN measure"
        );
    }

    #[test]
    fn measure_plain_path_is_pure() {
        let p = make_parser("https://example.com/path.");
        let before = p.cursor();
        let len = measure_plain_path(&p).expect("path");
        assert_eq!(p.cursor(), before, "measure must not advance");
        // Trail strip drops trailing `.` so exclusive end is before it.
        assert!(len > 0);
        assert_eq!(&p.rest_source()[..len], "https://example.com/path");
        assert!(measure_plain_path(&make_parser("https:")).is_none());
        assert!(measure_plain_path(&make_parser("not-a-link")).is_none());
    }

    #[test]
    fn at_plain_link_content_start_synthetic_region() {
        // After `$` (its own token, and a word character in Org's syntax
        // table): no word start unless the region starts at the path. Same
        // region-start check as the babel tests.
        let mut p = make_parser("$https://x.com");
        assert_eq!(p.current(), OrgSyntaxKind::DOLLAR);
        p.eat_any(); // cursor on the protocol TEXT
        assert!(!at_plain_link(&p), "no word start after `$` without region");
        p.with_inline_region_start(Some(crate::parser::Pos::from_index(1)), |pp| {
            assert!(
                at_plain_link(pp),
                "at_region_start must accept plain path at synthetic region start"
            );
            assert_eq!(pp.cursor().index(), 1, "at_plain_link is pure");
        });
        // BOF still accepts without synthetic region (Bol).
        assert!(at_plain_link(&make_parser("https://x.com")));
    }

    /// Plain path ending mid-TEXT then a later heading must round-trip
    /// (cursor remainder as parent text; lex token vector stays fixed).
    #[test]
    fn plain_trail_split_then_heading_roundtrips() {
        let src = "https://x.com.\n* H\n";
        let out = parse(src);
        assert_eq!(tree_text(out.root()), src);
        assert_eq!(out.root().kind(), OrgSyntaxKind::DOCUMENT);
        assert_eq!(first_link_path(src).as_deref(), Some("https://x.com"));
        assert_eq!(nodes_of_kind(out.root(), OrgSyntaxKind::HEADING).len(), 1);
    }

    #[test]
    fn unregistered_plain_not_link() {
        assert_eq!(link_count("see doi:10.1234/foo\n"), 0);
    }

    #[test]
    fn target_and_radio_target_forms() {
        let src = "here <<dest>> and <<<important>>>\n";
        let root = parse_document_tree(src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::TARGET).len(), 1);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET).len(), 1);
    }

    /// Prose that would match a radio target stays plain text; only the
    /// definition object is structural.
    #[test]
    fn radio_prose_stays_text_definition_parses() {
        let src = "remember important later\n<<<important>>>\nand important again\n";
        let root = parse(src).root().clone();
        let radios = nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET);
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].text().to_string(), "<<<important>>>");
        assert_eq!(
            link_count(src),
            0,
            "structural parse must not promote radio prose to LINK"
        );
        let full = tree_text(&root);
        // Prose occurrences of the radio key remain source text (not LINK).
        assert!(full.contains("remember important later"));
        assert!(full.contains("and important again"));
        // Definition line is the only RADIO_TARGET surface; key is not a path.
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::LINK_PATH).len(),
            0,
            "prose must not become LINK_PATH"
        );
        let def_line = full
            .lines()
            .find(|l| l.contains("<<<important>>>"))
            .expect("definition line");
        assert_eq!(def_line, "<<<important>>>");
    }

    #[test]
    fn radio_target_with_prose_and_heading_roundtrips() {
        let src = "important,\n<<<important>>>\n* H\n";
        let out = parse(src);
        assert_eq!(tree_text(out.root()), src);
        let radios = nodes_of_kind(out.root(), OrgSyntaxKind::RADIO_TARGET);
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].text().to_string(), "<<<important>>>");
        assert_eq!(nodes_of_kind(out.root(), OrgSyntaxKind::LINK).len(), 0);
        assert_eq!(nodes_of_kind(out.root(), OrgSyntaxKind::HEADING).len(), 1);
        // Leading prose line with trailing comma is not a link span.
        assert!(tree_text(out.root()).starts_with("important,"));
    }

    #[test]
    fn public_parse_roundtrip_with_links() {
        let src = "[[file:./a]] and https://x.com and <http://y>\n";
        let out = parse(src);
        assert_eq!(tree_text(out.root()), src);
        assert_eq!(out.root().kind(), OrgSyntaxKind::DOCUMENT);
        assert_eq!(link_count(src), 3);
    }

    #[test]
    fn plain_under_markup_limit_does_not_bleed_path() {
        // Closing * caps the markup region; path must not swallow past it.
        let src = "*https://x.com*\n";
        assert_eq!(first_link_path(src).as_deref(), Some("https://x.com"));
        let root = parse(src).root().clone();
        let link = nodes_of_kind(&root, OrgSyntaxKind::LINK)
            .into_iter()
            .next()
            .expect("plain link inside bold");
        assert_eq!(link.text().to_string(), "https://x.com");
    }

    /// Plain links after punctuation, as Org reports them (probed in the
    /// parity image). The character before the protocol decides: a word
    /// character blocks the link, anything else allows it.
    #[test]
    fn plain_link_word_start_matches_org() {
        let linked = [
            "(https://x.com)",
            "x(https://x.com)",
            "\"https://x.com\"",
            "foo:https://x.com",
            "a<https://x.com",
            "<https://x.com",
            "-https://x.com",
            "_https://x.com",
            "\u{ab}https://x.com",
            "\u{2014}https://x.com",
            "\u{2019}https://x.com",
            "\u{a9}https://x.com",
        ];
        for src in linked {
            let src = format!("{src}\n");
            assert_eq!(
                first_link_path(&src).as_deref(),
                Some("https://x.com"),
                "{src:?}"
            );
            assert_eq!(
                tree_text(&parse_document_tree(&src)),
                src,
                "lossless {src:?}"
            );
        }
        for src in [
            "abchttps://x.com",
            "\u{e9}https://x.com",
            "'https://x.com",
            "1https://x.com",
            "$https://x.com",
        ] {
            let src = format!("{src}\n");
            assert_eq!(link_count(&src), 0, "{src:?}");
        }
    }

    /// A rejected angle link falls back to the plain link inside it, as in
    /// Org, and an edit that removes the word character before a protocol
    /// reaches the same tree through `apply` as through a cold parse.
    #[test]
    fn plain_link_after_rejected_angle_and_after_edit() {
        let src = "Not a link <https://x\n>.\n";
        assert_eq!(first_link_path(src).as_deref(), Some("https://x"));

        let before = parse("See xhttps://x.com now.\n");
        assert!(before.links().next().is_none());
        let edit = crate::Edit {
            range: crate::TextRange::new(4.into(), 5.into()),
            replacement: "(".to_string(),
        };
        let edits = crate::EditSet::new(before.source(), vec![edit]).expect("valid edit");
        let after = before.apply(&edits);
        let cold = parse(after.source());
        let shape = |doc: &crate::Document| {
            doc.root()
                .descendants_with_tokens()
                .map(|e| (e.kind(), e.text_range()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&after), shape(&cold), "apply and cold parse agree");
        assert_eq!(after.links().count(), 1);
    }

    /// Plain-link paths as Org reports them (probed in the parity image,
    /// each followed by ` z`). `None` is no link.
    #[test]
    fn plain_link_path_end_matches_org() {
        let cases: &[(&str, Option<&str>)] = &[
            ("https:a", None),
            ("https:ab", Some("https:ab")),
            ("https://x.com/a*", Some("https://x.com/a")),
            ("https://x.com/a-", Some("https://x.com/a-")),
            ("https://x.com/a/", Some("https://x.com/a/")),
            ("https://x.com/a.", Some("https://x.com/a")),
            ("https://x.com/a.b.c.", Some("https://x.com/a.b.c")),
            ("https://x.com/a'", Some("https://x.com/a")),
            ("https://x.com/a$", Some("https://x.com/a")),
            ("https://x.com/a%", Some("https://x.com/a")),
            ("https://x.com/a`", Some("https://x.com/a")),
            ("https://x.com/a\\", Some("https://x.com/a")),
            ("https://x.com/a}", Some("https://x.com/a")),
            ("https://x.com/a_", Some("https://x.com/a")),
            ("https://x.com/a__b", Some("https://x.com/a__b")),
            ("https://x.com/a.-", Some("https://x.com/a.-")),
            ("https://x.com/a\u{e9}", Some("https://x.com/a\u{e9}")),
            ("https://x.com/a\u{ab}", Some("https://x.com/a")),
            ("https://x.com/a\u{2019}", Some("https://x.com/a")),
            ("https://x.com/a\u{a0}", Some("https://x.com/a")),
            (
                "https://x.com/wiki/Foo_(bar)",
                Some("https://x.com/wiki/Foo_(bar)"),
            ),
            (
                "https://x.com/wiki/Foo_(bar)).",
                Some("https://x.com/wiki/Foo_(bar)"),
            ),
            ("https://x.com/a(b", Some("https://x.com/a")),
            ("https://x.com/a(b c)", Some("https://x.com/a")),
            ("https://x.com/a[1].", Some("https://x.com/a[1]")),
            ("https://x.com/a<b>", Some("https://x.com/a<b>")),
            ("https://x.com/a(b]", Some("https://x.com/a(b]")),
            ("https://x.com/a((b))", Some("https://x.com/a((b))")),
            ("https://x.com/a(((b)))", Some("https://x.com/a")),
            ("https://x.com/a(b)(c)", Some("https://x.com/a(b)(c)")),
            ("https://x.com/a(b)c(d", Some("https://x.com/a(b)c")),
            ("https://x.com/a()", Some("https://x.com/a()")),
            ("https:(a)", None),
            ("https:(a)b", Some("https:(a)b")),
            ("https:[a]b", Some("https:[a]b")),
            ("https:/", None),
            ("https://", Some("https://")),
            ("https:-", None),
            ("https:--", Some("https:--")),
            ("mailto:a@b.c", Some("mailto:a@b.c")),
        ];
        for &(path, want) in cases {
            let src = format!("{path} z\n");
            let doc = parse(&src);
            let got = nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
                .first()
                .map(|n| n.text().to_string());
            assert_eq!(got.as_deref(), want, "{src:?}");
            assert_eq!(doc.write(), src, "lossless {src:?}");
        }
    }
}
