//! Citation object family: `[cite...]` + nested `CITATION_REFERENCE`.
//!
//! Evidence:
//! - Worg 5.5-5.6 / `org-element-citation-parser` /
//!   `org-element-citation-reference-parser` / `org-element-citation-prefix-re`
//!   / `org-element-citation-key-re` / `org-element-object-restrictions`
//!
//! Forms: `[cite:@key]`, `[cite/STYLE: ...]`, multi-ref `;`-separated, optional
//! global and per-reference prefix/suffix object regions. Style and key stay
//! token tiles (no `STYLE` / `CITE_KEY` child kinds); the typed views read them.
//!
//! Incomplete or keyless forms are a silent miss: not a citation and no
//! diagnostic, as org-element returns nil. `CITATION_REFERENCE` is never
//! dispatched on its own; the citation walks its references. A `;` can sit
//! inside a token (`see;`), so reference regions combine `inline_in` up to
//! token boundaries with `eat_to` for byte-exact ends.
//!
//! Column-0 `[cite:@k]` is still an object (contrast footnote definitions).

use crate::parser::grammar::drive::region;
use crate::parser::{PairNewlines, Parser, Pos, TokenView};
use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// Measured complete citation; incomplete forms return `None` from probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CitationShape {
    /// Token count of the open run: `[` `cite` optional `/STYLE` `:` and
    /// trailing blanks matched by the prefix-re (`WHITESPACE`/`NEWLINE`/`INDENT`).
    pub(crate) open_tokens: usize,
    /// Absolute position of the matching outer `]`.
    pub(crate) closer: Pos,
}

// ---------------------------------------------------------------------------
// Pure helpers (shared by probe and emit)
// ---------------------------------------------------------------------------

/// Open blanks after `:`: org prefix-re `(any "\t\n ")` plus line-start
/// [`OrgSyntaxKind::INDENT`] that follows a newline inside the form.
#[inline]
fn is_cite_blank(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::NEWLINE | OrgSyntaxKind::INDENT
    )
}

/// Style atom after `/` (ASCII only): `/`, `_`, or TEXT of ASCII alnum / `-`.
///
/// Matches elisp style group `(any "/_-" alnum)+` with ASCII `alnum`. Not shared
/// with key measure (different charset).
#[inline]
fn is_style_atom(kind: OrgSyntaxKind, text: &str) -> bool {
    match kind {
        OrgSyntaxKind::SLASH | OrgSyntaxKind::UNDERSCORE => true,
        OrgSyntaxKind::TEXT => {
            !text.is_empty() && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        }
        _ => false,
    }
}

/// Exclusive token count of style atoms from relative offset `from`.
#[inline]
fn measure_style_atoms(t: TokenView<'_, '_>, from: usize) -> usize {
    t.run_while(from, is_style_atom)
}

/// Org citation-key body character class after the leading `@`.
///
/// Unicode alphanumeric (word) plus the elisp punct set
/// `-.:?!'/*@+|(){}<>&_^$#%~` and backtick. Newline is not a key character.
#[inline]
fn is_citation_key_char(c: char) -> bool {
    if c.is_alphanumeric() {
        return true;
    }
    matches!(
        c,
        '-' | '.'
            | ':'
            | '?'
            | '!'
            | '`'
            | '\''
            | '/'
            | '*'
            | '@'
            | '+'
            | '|'
            | '('
            | ')'
            | '{'
            | '}'
            | '<'
            | '>'
            | '&'
            | '_'
            | '^'
            | '$'
            | '#'
            | '%'
            | '~'
    )
}

/// Measure a citation key at absolute source byte `byte_off`.
///
/// Requires a leading `@` and at least one key character. Returns exclusive
/// `(key_start, key_end)` byte ends including the `@`.
///
/// The citation layout view calls this too, so probe, emit and the view agree
/// on the key charset.
pub(crate) fn measure_citation_key_at(source: &str, byte_off: usize) -> Option<(usize, usize)> {
    let b = source.as_bytes();
    if byte_off >= b.len() || b[byte_off] != b'@' {
        return None;
    }
    // `byte_off < len` => `after <= len`; empty rest is a bare-`@` reject below.
    let after = byte_off + 1;
    let rest = &source[after..];
    let mut chars = rest.chars();
    let first_ch = chars.next()?;
    if !is_citation_key_char(first_ch) {
        return None;
    }
    let mut end = after + first_ch.len_utf8();
    for ch in chars {
        if !is_citation_key_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    Some((byte_off, end))
}

/// First citation key in absolute half-open `[from, to)`.
fn find_next_citation_key(source: &str, from: usize, to: usize) -> Option<(usize, usize)> {
    if from >= to || to > source.len() {
        return None;
    }
    let b = source.as_bytes();
    let mut i = from;
    while i < to {
        if b[i] == b'@'
            && let Some((ks, ke)) = measure_citation_key_at(source, i)
            && ke <= to
        {
            return Some((ks, ke));
        }
        // Step one whole char at a time so `i` stays on a UTF-8 boundary for
        // the slice above.
        if let Some(ch) = source[i..].chars().next() {
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    None
}

/// First `;` byte in absolute half-open `[from, to)`.
fn find_first_semi(source: &str, from: usize, to: usize) -> Option<usize> {
    if from >= to || to > source.len() {
        return None;
    }
    source.as_bytes()[from..to]
        .iter()
        .position(|&c| c == b';')
        .map(|rel| from + rel)
}

/// Last `;` byte in absolute half-open `[from, to)`.
fn find_last_semi(source: &str, from: usize, to: usize) -> Option<usize> {
    if from >= to || to > source.len() {
        return None;
    }
    source.as_bytes()[from..to]
        .iter()
        .rposition(|&c| c == b';')
        .map(|rel| from + rel)
}

/// Exclusive end after trimming trailing ASCII whitespace from `[start, end)`.
fn trim_trailing_cite_blanks(source: &str, start: usize, end: usize) -> usize {
    let b = source.as_bytes();
    let mut e = end.min(b.len());
    while e > start && b[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    e
}

/// Absolute source-byte start of the full tile at `pos`, ignoring any mid-tile
/// offset.
#[inline]
fn tile_start(parser: &Parser, pos: Pos) -> usize {
    usize::from(parser.token_range(pos.index()).start())
}

/// Absolute source-byte start of the token `open_tokens` steps from the cursor
/// (or `visible_source_end` when that index is at/past the limit).
fn open_end_byte(parser: &Parser, open_tokens: usize) -> Option<usize> {
    let base = parser.cursor().index();
    let abs = base.checked_add(open_tokens)?;
    if abs >= parser.limit().index() {
        return Some(parser.visible_source_end());
    }
    Some(tile_start(parser, Pos::from_index(abs)))
}

/// First token at or after the cursor token index whose full-tile range starts
/// at absolute `byte` (cursor-forward under the active limit).
///
/// `;` and key ends are measured on source bytes, so emit needs this map back
/// to tokens. Only citations need it, so it lives here rather than on `Parser`.
fn pos_starting_at(parser: &Parser, byte: usize) -> Option<Pos> {
    let mut i = parser.cursor().index();
    let lim = parser.limit().index();
    while i < lim {
        let start = usize::from(parser.token_range(i).start());
        if start == byte {
            return Some(Pos::from_index(i));
        }
        if start > byte {
            return None;
        }
        i += 1;
    }
    None
}

/// First token at or after the cursor token index whose full-tile range
/// contains absolute `byte` (`start <= byte < end`).
fn token_containing_byte(parser: &Parser, byte: usize) -> Option<Pos> {
    let mut i = parser.cursor().index();
    let lim = parser.limit().index();
    while i < lim {
        let r = parser.token_range(i);
        let start = usize::from(r.start());
        let end = usize::from(r.end());
        if start <= byte && byte < end {
            return Some(Pos::from_index(i));
        }
        if start > byte {
            return None;
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

/// Pure measure of a complete citation at the cursor.
///
/// No PRE: citations may open at BOL, after space, after letters, and at
/// column 0. Cursor is never mutated.
pub(crate) fn probe(parser: &Parser) -> Option<CitationShape> {
    let t = parser.here()?;
    if t.kind() != OrgSyntaxKind::L_BRACKET {
        return None;
    }
    // Case-sensitive literal `cite` (elisp looking-at).
    if t.kind_at(1) != OrgSyntaxKind::TEXT || t.text_at(1) != "cite" {
        return None;
    }

    let mut i = 2usize;
    if t.kind_at(i) == OrgSyntaxKind::SLASH {
        i += 1;
        let style_len = measure_style_atoms(t, i);
        if style_len == 0 {
            // bare `[cite/:` is not a citation
            return None;
        }
        i += style_len;
    }
    if t.kind_at(i) != OrgSyntaxKind::COLON {
        return None;
    }
    i += 1;
    while is_cite_blank(t.kind_at(i)) {
        i += 1;
    }
    let open_tokens = i;

    let closer = parser.find_paired_at(
        parser.cursor(),
        OrgSyntaxKind::L_BRACKET,
        OrgSyntaxKind::R_BRACKET,
        PairNewlines::Allow,
    )?;
    let after_open = t.nth(open_tokens).map(|n| n.pos())?;
    if closer.index() < after_open.index() {
        return None;
    }

    let contents_start = open_end_byte(parser, open_tokens)?;
    let closer_start = tile_start(parser, closer);
    find_next_citation_key(parser.source(), contents_start, closer_start)?;

    Some(CitationShape {
        open_tokens,
        closer,
    })
}

// ---------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------

/// Emit an object region up to exclusive `end_byte` under the open parent,
/// optionally consuming the `;` that follows it.
///
/// When `end_byte` is a token boundary, `inline_in` runs to that token. When
/// it falls inside a token, `inline_in` covers the whole tokens before that
/// token and `eat_to(end_byte)` takes the plain remainder.
///
/// With `consume_semi`, a cursor left exactly at `end_byte` also takes the
/// `;` (a global or between-reference separator); a cursor anywhere else
/// skips that step. Without it, `end_byte` is only a bound: a key suffix
/// before a global-suffix `;`, or a blank-trimmed end.
///
/// Returns `false` only when an `eat_to` refuses. Callers ignore the result:
/// the reference loop guards its own progress and drains leftovers.
fn emit_secondary_to(parser: &mut Parser, end_byte: usize, consume_semi: bool) -> bool {
    let Some(start) = parser.source_start_here() else {
        return true;
    };
    if start < end_byte {
        // Token-boundary end: `inline_in` alone is exact.
        if let Some(end_pos) = pos_starting_at(parser, end_byte) {
            region::inline_in(parser, end_pos, OrgSyntaxKind::CITATION_REFERENCE);
        } else {
            // Mid-tile end: objects before the containing token, then the
            // plain remainder.
            if let Some(t_pos) = token_containing_byte(parser, end_byte) {
                let t_start = tile_start(parser, t_pos);
                if start < t_start {
                    region::inline_in(parser, t_pos, OrgSyntaxKind::CITATION_REFERENCE);
                }
            }
            if parser.source_start_here().is_some_and(|s| s < end_byte) && !parser.eat_to(end_byte)
            {
                return false;
            }
        }
    }
    if consume_semi {
        let semi_end = end_byte + 1;
        if parser.source_start_here().is_some_and(|s| s == end_byte) && !parser.eat_to(semi_end) {
            return false;
        }
    }
    true
}

/// One nested citation-reference under the open CITATION marker.
///
/// `contents_end_byte` is the exclusive source-byte end of the ref sequence
/// (global-suffix `;` start when present, else blank-trimmed end). Returns
/// false when no key remains or emit could not advance (defensive leftover path).
fn parse_one_citation_reference(parser: &mut Parser, contents_end_byte: usize) -> bool {
    let Some(cursor_byte) = parser.source_start_here() else {
        return false;
    };
    if cursor_byte >= contents_end_byte {
        return false;
    }
    let Some((key_start, key_end)) =
        find_next_citation_key(parser.source(), cursor_byte, contents_end_byte)
    else {
        return false;
    };

    // A between-reference `;` in (key_end, contents_end) belongs to this
    // reference. A global-suffix `;` sits at contents_end, outside the span.
    let between_semi = find_first_semi(parser.source(), key_end, contents_end_byte);

    let before = parser.progress();
    let marker = parser.start();

    // Key prefix objects before `@key` (`@` always starts a token).
    if let Some(key_pos) = pos_starting_at(parser, key_start)
        && parser.source_start_here().is_some_and(|s| s < key_start)
    {
        region::inline_in(parser, key_pos, OrgSyntaxKind::CITATION_REFERENCE);
    }

    // AT + key tiles (may end mid-tile before glued `;`).
    if !parser.eat_to(key_end) {
        // Measure said the key is here; reject is an invariant miss. Abandon
        // the open marker and fail the ref so the parent loop can drain.
        marker.abandon(parser);
        return false;
    }

    // Key suffix. The result is ignored: the progress check below and the
    // parent's drain handle a stuck separator.
    if let Some(semi) = between_semi {
        let _ = emit_secondary_to(parser, semi, true);
    } else if parser
        .source_start_here()
        .is_some_and(|s| s < contents_end_byte)
    {
        let _ = emit_secondary_to(parser, contents_end_byte, false);
    }

    marker.complete(parser, OrgSyntaxKind::CITATION_REFERENCE);
    // A completed reference must have moved the source cursor (checked in
    // release builds too).
    parser.progress() != before
}

/// Tile remaining content under the open CITATION up to exclusive `end_byte`.
///
/// Used when the reference loop cannot claim more keys. A refused `eat_to`
/// leaves the cursor unchanged; callers exit the loop either way.
fn drain_citation_contents(parser: &mut Parser, end_byte: usize) {
    if parser.source_start_here().is_some_and(|s| s < end_byte) {
        let _ = parser.eat_to(end_byte);
    }
}

/// Emit one citation from a measured shape (no remeasure of the outer form).
pub(crate) fn parse(parser: &mut Parser, shape: CitationShape) {
    let marker = parser.start();
    parser.eat_n(shape.open_tokens);

    let contents_start = parser
        .source_start_here()
        .expect("open tokens leave a content cursor when probe succeeded");
    let closer_start = tile_start(parser, shape.closer);
    let source = parser.source();

    // Probe found a key; find it again for its byte range.
    let first_key = find_next_citation_key(source, contents_start, closer_start)
        .expect("probe required at least one key");

    // Global prefix: `;` between contents start and first key end. The result
    // is ignored: the reference loop and drain still guard progress.
    if let Some(semi) = find_last_semi(source, contents_start, first_key.1) {
        // Always consume under CITATION (empty or non-empty prefix span).
        let _ = emit_secondary_to(parser, semi, true);
    }

    let after_prefix = parser
        .source_start_here()
        .expect("cursor after open / prefix");
    let trimmed_end = trim_trailing_cite_blanks(source, after_prefix, closer_start);

    // Global suffix: last `;` after first key with no further key after it.
    let (contents_end_byte, has_global_suffix) =
        match find_last_semi(source, first_key.1, trimmed_end) {
            Some(semi) if find_next_citation_key(source, semi + 1, trimmed_end).is_none() => {
                (semi, true)
            }
            _ => (trimmed_end, false),
        };

    // The citation walks its own references instead of calling `inline_in`.
    // As in the object dispatcher, a claim that does not advance must not
    // loop, in release builds too.
    while parser
        .source_start_here()
        .is_some_and(|s| s < contents_end_byte)
    {
        let before = parser.progress();
        if !parse_one_citation_reference(parser, contents_end_byte) {
            drain_citation_contents(parser, contents_end_byte);
            break;
        }
        if parser.progress() == before {
            debug_assert!(
                false,
                "citation reference claim made no source-cursor progress"
            );
            drain_citation_contents(parser, contents_end_byte);
            break;
        }
    }

    if has_global_suffix {
        // contents_end_byte is the suffix `;` (the references' exclusive
        // bound). The result is ignored: eating the closer still ends the form.
        let _ = emit_secondary_to(parser, contents_end_byte, true);
        if parser.cursor().index() < shape.closer.index() {
            region::inline_in(parser, shape.closer, OrgSyntaxKind::CITATION_REFERENCE);
        }
    } else {
        // Trailing blanks before `]` as plain tiles under CITATION.
        drain_to_closer(parser, shape.closer);
    }

    let ok = parser.eat(OrgSyntaxKind::R_BRACKET);
    debug_assert!(ok, "closer guaranteed by probe");
    marker.complete(parser, OrgSyntaxKind::CITATION);
}

/// Consume remaining source up to exclusive `closer` as plain tiles under the
/// open parent (token-granular, then mid-tile if needed).
fn drain_to_closer(parser: &mut Parser, closer: Pos) {
    if parser.cursor().index() < closer.index() {
        parser.eat_until(closer);
    }
    let closer_start = tile_start(parser, closer);
    if parser.source_start_here().is_some_and(|s| s < closer_start) {
        let _ = parser.eat_to(closer_start);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree, run_object, tree_text,
    };

    // -- pure helpers -------------------------------------------------------

    #[test]
    fn measure_citation_key_charset() {
        // Accept shapes (punct set beyond * _ @).
        assert_eq!(measure_citation_key_at("@k", 0), Some((0, 2)));
        assert_eq!(measure_citation_key_at("@a*b", 0), Some((0, 4)));
        assert_eq!(measure_citation_key_at("@a_b@c", 0), Some((0, 6)));
        assert_eq!(measure_citation_key_at("@a-b.c", 0), Some((0, 6)));
        assert_eq!(measure_citation_key_at("@a:b?c!", 0), Some((0, 7)));
        assert_eq!(measure_citation_key_at("@a+b|c", 0), Some((0, 6)));
        assert_eq!(measure_citation_key_at("@a(b){c}", 0), Some((0, 8)));
        assert_eq!(measure_citation_key_at("@a<b>&c", 0), Some((0, 7)));
        assert_eq!(measure_citation_key_at("@a^b$c#d%e~", 0), Some((0, 11)));
        // Stop boundaries.
        assert_eq!(measure_citation_key_at("@a;b", 0), Some((0, 2)));
        assert_eq!(measure_citation_key_at("@a b", 0), Some((0, 2)));
        assert_eq!(measure_citation_key_at("@a\nb", 0), Some((0, 2)));
        assert_eq!(measure_citation_key_at("@a]", 0), Some((0, 2)));
        // Rejects.
        assert!(measure_citation_key_at("@", 0).is_none());
        assert!(measure_citation_key_at("@ ", 0).is_none());
        assert!(measure_citation_key_at("@\n", 0).is_none());
        assert!(measure_citation_key_at("k", 0).is_none());
        assert!(measure_citation_key_at("", 0).is_none());
        // Mid-string offset.
        assert_eq!(
            measure_citation_key_at("xx@key;yy", 2),
            Some((2, 6)),
            "key at mid-string offset"
        );
        assert!(measure_citation_key_at("xx@key", 0).is_none());
        // Unicode word continuation.
        assert_eq!(
            measure_citation_key_at("@a\u{03b1}", 0),
            Some((0, "@a\u{03b1}".len()))
        );
    }

    #[test]
    fn measure_style_atoms_ascii_only() {
        let p = make_parser("[cite/foo/bar:@k]");
        let t = p.here().expect("bracket");
        // After `[` `cite` `/` -> style from relative 3: TEXT foo, SLASH, TEXT bar
        assert_eq!(measure_style_atoms(t, 3), 3);

        // Underscore and hyphen style segments.
        let p_us = make_parser("[cite/a_b-c:@k]");
        let t_us = p_us.here().expect("bracket");
        // The hyphen stays in TEXT; the underscore is its own token.
        assert!(measure_style_atoms(t_us, 3) >= 1);
        assert!(probe(&p_us).is_some(), "underscore/hyphen style accepted");

        // Single-segment style.
        let p_slash = make_parser("[cite/n:@k]");
        assert_eq!(measure_style_atoms(p_slash.here().expect("b"), 3), 1);

        // Empty style after slash.
        let p2 = make_parser("[cite/:@k]");
        let t2 = p2.here().expect("bracket");
        assert_eq!(measure_style_atoms(t2, 3), 0);

        // Unicode letter in style is not an ASCII style atom (TEXT may still
        // hold it as one tile; run_while rejects non-ASCII TEXT content).
        let p3 = make_parser("[cite/\u{03b1}:@k]");
        let t3 = p3.here().expect("bracket");
        assert_eq!(measure_style_atoms(t3, 3), 0, "Unicode not style");
        assert!(probe(&p3).is_none(), "Unicode style is silent miss");
    }

    #[test]
    fn is_cite_blank_kinds() {
        assert!(is_cite_blank(OrgSyntaxKind::WHITESPACE));
        assert!(is_cite_blank(OrgSyntaxKind::NEWLINE));
        assert!(is_cite_blank(OrgSyntaxKind::INDENT));
        assert!(!is_cite_blank(OrgSyntaxKind::TEXT));
        assert!(!is_cite_blank(OrgSyntaxKind::AT));
    }

    #[test]
    fn find_next_citation_key_bounds() {
        let s = "see @a;@b tail";
        assert_eq!(find_next_citation_key(s, 0, s.len()), Some((4, 6)));
        assert_eq!(find_next_citation_key(s, 6, s.len()), Some((7, 9)));
        assert!(find_next_citation_key(s, 0, 4).is_none());
        assert!(find_next_citation_key(s, 9, s.len()).is_none());
        // Half-open: key must end within bound.
        assert!(
            find_next_citation_key(s, 4, 5).is_none(),
            "only '@' in window"
        );
        assert_eq!(find_next_citation_key(s, 4, 6), Some((4, 6)));
        // Empty / inverted / past end.
        assert!(find_next_citation_key(s, 3, 3).is_none());
        assert!(find_next_citation_key(s, 5, 4).is_none());
        assert!(find_next_citation_key(s, 0, s.len() + 1).is_none());
        // No key.
        assert!(find_next_citation_key("no key here", 0, 11).is_none());
    }

    #[test]
    fn find_semi_and_trim_bounds() {
        let s = "a;b;c";
        assert_eq!(find_first_semi(s, 0, 5), Some(1));
        assert_eq!(find_first_semi(s, 2, 5), Some(3));
        assert!(find_first_semi(s, 0, 1).is_none());
        assert!(find_first_semi(s, 4, 5).is_none());
        assert_eq!(find_last_semi(s, 0, 5), Some(3));
        assert_eq!(find_last_semi(s, 0, 3), Some(1));
        assert!(find_last_semi(s, 0, 1).is_none());
        // Empty / inverted / past end.
        assert!(find_first_semi(s, 2, 2).is_none());
        assert!(find_last_semi(s, 2, 2).is_none());
        assert!(find_first_semi(s, 0, 99).is_none());

        let t = "body  \n\t";
        assert_eq!(trim_trailing_cite_blanks(t, 0, t.len()), 4);
        assert_eq!(trim_trailing_cite_blanks(t, 0, 4), 4);
        assert_eq!(trim_trailing_cite_blanks(t, 2, 2), 2);
        assert_eq!(trim_trailing_cite_blanks("  ", 0, 2), 0);
        // Clamp end past len.
        assert_eq!(trim_trailing_cite_blanks("ab", 0, 99), 2);
    }

    // -- probe accept / reject / purity -------------------------------------

    #[test]
    fn citation_probe_accept() {
        for src in [
            "[cite:@k]",
            "[cite/n:@k]",
            "[cite/foo/bar:@k]",
            "[cite: @k]",
            "[cite:@a;@b]",
            "[cite:see;@a]",
            "[cite:;@a]",
            "[cite:@a;]",
            "[cite:@a*b]",
        ] {
            let shape = probe(&make_parser(src)).unwrap_or_else(|| panic!("accept {src}"));
            assert!(shape.open_tokens >= 3, "open for {src}");
            assert_eq!(
                make_parser(src).token_kind(shape.closer.index()),
                OrgSyntaxKind::R_BRACKET
            );
        }
    }

    #[test]
    fn citation_probe_multi_line_indent_blank() {
        // Tokens: `[` TEXT("cite") `:` NEWLINE INDENT AT TEXT("k") `]`
        // open absorbs NEWLINE + INDENT after `:` => open_tokens == 5.
        let src = "[cite:\n  @k]";
        let shape = probe(&make_parser(src)).expect("multi-line open blanks");
        assert_eq!(shape.open_tokens, 5, "open must include NEWLINE+INDENT");

        let root = parse_document_tree(&format!("{src}\n"));
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        assert_eq!(cites.len(), 1);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].text().to_string(),
            "@k",
            "INDENT stays in open, not under the ref"
        );
    }

    #[test]
    fn citation_probe_reject() {
        for src in [
            "[cite:]",
            "[cite: no key]",
            "[cite:@]",  // bare @, no key chars
            "[cite:@ ]", // @ then blank (not a key)
            "[cite:@k",  // no closer
            "[cite/@k]",
            "[cite/:@k]",  // empty style after /
            "[cite/ :@k]", // whitespace after / is not style
            "[citenot:@k]",
            "[Cite:@k]",
            "[cite/\u{03b1}:@k]", // non-ASCII style
            "[fn:1]",
            "[[a]]",
            "[1/2]",
            "@k",
            "plain",
        ] {
            assert!(
                probe(&make_parser(src)).is_none(),
                "not a citation: {src:?}"
            );
        }
        // Full-document silent miss for composition rejects + non-ASCII style.
        for src in [
            "[cite:@]\n",
            "[cite:@ ]\n",
            "[cite/ :@k]\n",
            "[cite/\u{03b1}:@k]\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::CITATION).is_empty(),
                "no CITATION for reject {src:?}"
            );
        }
    }

    #[test]
    fn citation_probe_is_pure() {
        for src in ["[cite:@k]", "[cite:@k", "[cite:]", "[Cite:@k]", "["] {
            let p = make_parser(src);
            let before = p.cursor();
            let _ = probe(&p);
            assert_eq!(p.cursor(), before, "probe must not advance for {src:?}");
        }
    }

    // -- emit happy paths ---------------------------------------------------

    fn citation_nodes(
        src: &str,
    ) -> (
        rowan::SyntaxNode<crate::syntax::OrgLanguage>,
        Vec<rowan::SyntaxNode<crate::syntax::OrgLanguage>>,
    ) {
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "tiling {src:?}");
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        (root, cites)
    }

    #[test]
    fn simplest_citation_emits() {
        let src = "[cite:@k]";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].text().to_string(), src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@k");
    }

    #[test]
    fn style_tokens_under_citation() {
        let src = "[cite/n:@k]\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].text().to_string(), "[cite/n:@k]");
        // Style stays as tiles; case preserved (no child STYLE kind).
        assert!(cites[0].text().to_string().contains("/n:"));

        let src2 = "[cite/foo/bar:@k]\n";
        let (_, cites2) = citation_nodes(src2);
        assert_eq!(cites2[0].text().to_string(), "[cite/foo/bar:@k]");
    }

    #[test]
    fn blanks_after_colon_and_multi_line_body() {
        let src = "[cite: @k]\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);

        let src2 = "[cite:\n  @k]\n";
        let (_, cites2) = citation_nodes(src2);
        assert_eq!(cites2.len(), 1);
        assert!(cites2[0].text().to_string().contains('\n'));

        // Multi-line after open (PairNewlines::Allow). Without `;`, one ref owns
        // all content after the first key (including a later `@b` as plain tiles).
        let src3 = "[cite:@a\n@b]\n";
        let (_, cites3) = citation_nodes(src3);
        assert_eq!(cites3.len(), 1);
        let refs = nodes_of_kind(&cites3[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1, "no ; => single ref owns trailing content");
    }

    #[test]
    fn multi_ref_mid_tile_semi() {
        let src = "[cite:@a;@b]\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].text().to_string(), "@a;");
        assert_eq!(refs[1].text().to_string(), "@b");
    }

    #[test]
    fn global_prefix_space_and_glued() {
        // Space before `;`.
        let src = "[cite: see ;@a]\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@a");
        // `;` under CITATION, not under the ref.
        assert!(cites[0].text().to_string().contains(';'));
        assert!(!refs[0].text().to_string().contains(';'));

        // Glued mid-tile prefix: TEXT("see;")
        let src2 = "[cite:see;@a]\n";
        let (_, cites2) = citation_nodes(src2);
        let refs2 = nodes_of_kind(&cites2[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].text().to_string(), "@a");
        assert_eq!(cites2[0].text().to_string(), "[cite:see;@a]");

        // Glued with leading space then TEXT("foo;")
        let src3 = "[cite: foo;@a]\n";
        let (_, cites3) = citation_nodes(src3);
        let refs3 = nodes_of_kind(&cites3[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs3.len(), 1);
        assert_eq!(refs3[0].text().to_string(), "@a");
    }

    #[test]
    fn key_prefix_without_global_semi() {
        let src = "[cite: see @a]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        // Open blanks after `:` absorb the space; ref is key-prefix "see " + @a.
        assert_eq!(refs[0].text().to_string(), "see @a");
    }

    #[test]
    fn glued_key_suffix_between_ref_and_global_suffix() {
        // Glued key-suffix then between-ref `;`
        let src = "[cite:@a more;@b]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].text().to_string(), "@a more;");
        assert_eq!(refs[1].text().to_string(), "@b");

        // Glued key-suffix; global-suffix `;` under CITATION
        let src2 = "[cite:@a more; tail]\n";
        let (_, cites2) = citation_nodes(src2);
        let refs2 = nodes_of_kind(&cites2[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].text().to_string(), "@a more");
        assert!(
            !refs2[0].text().to_string().contains(';'),
            "global-suffix ; not under last ref"
        );
        assert!(cites2[0].text().to_string().contains("; tail"));
    }

    #[test]
    fn global_suffix_forms() {
        let src = "[cite:@a; p. 12]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@a");
        assert!(cites[0].text().to_string().contains("; p. 12"));

        let src2 = "[cite:@a p. 12]\n";
        let (_, cites2) = citation_nodes(src2);
        let refs2 = nodes_of_kind(&cites2[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].text().to_string(), "@a p. 12");

        let src3 = "[cite:@a;@b; tail]\n";
        let (_, cites3) = citation_nodes(src3);
        let refs3 = nodes_of_kind(&cites3[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs3.len(), 2);
        assert_eq!(refs3[0].text().to_string(), "@a;");
        assert_eq!(refs3[1].text().to_string(), "@b");
        assert!(cites3[0].text().to_string().contains("; tail"));
    }

    #[test]
    fn empty_global_prefix_and_suffix() {
        let src = "[cite:;@a]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@a");

        let src2 = "[cite:  ;@a]\n";
        let (_, cites2) = citation_nodes(src2);
        let refs2 = nodes_of_kind(&cites2[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].text().to_string(), "@a");

        let src3 = "[cite:@a;]\n";
        let (_, cites3) = citation_nodes(src3);
        let refs3 = nodes_of_kind(&cites3[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs3.len(), 1);
        assert_eq!(refs3[0].text().to_string(), "@a");
        // trailing ; under CITATION
        assert!(cites3[0].text().to_string().ends_with(";]"));
    }

    #[test]
    fn global_prefix_plus_ref_key_suffix() {
        let src = "[cite: before ;@a after]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@a after");
    }

    #[test]
    fn nested_objects_in_prefix_allowed_set() {
        let src = "[cite: *bold*;@k]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        assert_eq!(cites.len(), 1);
        let bolds = nodes_of_kind(&cites[0], OrgSyntaxKind::BOLD);
        assert_eq!(bolds.len(), 1);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn nested_brackets_in_prefix_balanced() {
        let src = "[cite: [[a]];@k]\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        // Nested link forbidden under citation-reference restriction.
        let links = nodes_of_kind(&cites[0], OrgSyntaxKind::LINK);
        assert!(links.is_empty(), "no LINK in citation prefix");
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn key_across_delimiter_tiles() {
        let src = "[cite:@a*b]\n";
        let (_, cites) = citation_nodes(src);
        let refs = nodes_of_kind(&cites[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text().to_string(), "@a*b");

        let src2 = "[cite:@a_b@c]\n";
        let (_, cites2) = citation_nodes(src2);
        let refs2 = nodes_of_kind(&cites2[0], OrgSyntaxKind::CITATION_REFERENCE);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].text().to_string(), "@a_b@c");
    }

    #[test]
    fn citation_in_paragraph_and_two_citations() {
        let src = "para [cite:@k] tail\n";
        let (_, cites) = citation_nodes(src);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].text().to_string(), "[cite:@k]");

        let src2 = "[cite:@a] and [cite:@b]\n";
        let (_, cites2) = citation_nodes(src2);
        assert_eq!(cites2.len(), 2);
    }

    // -- reject / silent miss full document ---------------------------------

    #[test]
    fn rejected_citation_forms_round_trip() {
        for src in [
            "[cite:]\n",
            "[cite: no key]\n",
            "[cite:@k\n",
            "[cite/@k]\n",
            "[cite/:@k]\n",
            "[cite/ :@k]\n",
            "[cite:@]\n",
            "[cite:@ ]\n",
            "[citenot:@k]\n",
            "[Cite:@k]\n",
            "@k alone\n",
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling {src:?}");
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::CITATION).is_empty(),
                "no CITATION for reject {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::CITATION_REFERENCE).is_empty(),
                "no freestanding CITATION_REFERENCE for {src:?}"
            );
        }
    }

    // -- scopes -------------------------------------------------------------

    #[test]
    fn citation_scope_link_desc_table_radio() {
        // Link description: no citation.
        let src = "[[t][d [cite:@k]]]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::CITATION).is_empty(),
            "no CITATION inside link description"
        );

        // Table cell: citation yes.
        let src2 = "| [cite:@k] |\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::CITATION).len(),
            1,
            "citation in table cell"
        );

        // Radio target: minimal set, no citation. Interior cannot lead/trail
        // with whitespace (target rules), so use non-blank edges.
        let src3 = "<<<x[cite:@k]y>>>\n";
        let root3 = parse_document_tree(src3);
        assert_eq!(tree_text(&root3), src3);
        assert_eq!(
            nodes_of_kind(&root3, OrgSyntaxKind::RADIO_TARGET).len(),
            1,
            "radio target must form for this fixture"
        );
        assert!(
            nodes_of_kind(&root3, OrgSyntaxKind::CITATION).is_empty(),
            "no CITATION inside radio target"
        );
    }

    #[test]
    fn citation_in_heading_title_emphasis_and_item_tag() {
        let src = "* title [cite:@k]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CITATION).len(), 1);

        let src2 = "*bold [cite:@k]*\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::CITATION).len(), 1);

        // ITEM_TAG is standard-set-no-line-break (includes citation).
        let src3 = "- tag [cite:@k] :: body\n";
        let root3 = parse_document_tree(src3);
        assert_eq!(tree_text(&root3), src3);
        assert_eq!(nodes_of_kind(&root3, OrgSyntaxKind::ITEM_TAG).len(), 1);
        assert_eq!(nodes_of_kind(&root3, OrgSyntaxKind::CITATION).len(), 1);
    }

    #[test]
    fn secondary_forbids_nested_cite_fn_link_allows_macro_export_markup() {
        // Nested [cite:@x] cannot form as a child CITATION under outer (secondary
        // set excludes citation). First key is still @x; inner open tiles as
        // key-prefix text under the first ref.
        let src = "[cite: [cite:@x];@k]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        assert_eq!(cites.len(), 1, "only outer citation");
        // nodes_of_kind includes self; require no *descendant* CITATION.
        let nested = cites[0]
            .descendants()
            .filter(|n| n.kind() == OrgSyntaxKind::CITATION && n != &cites[0])
            .count();
        assert_eq!(nested, 0, "no nested CITATION under outer");

        // Footnote in prefix: not FOOTNOTE_REFERENCE under secondary.
        let src2 = "[cite: [fn:1];@k]\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        let cites2 = nodes_of_kind(&root2, OrgSyntaxKind::CITATION);
        assert_eq!(cites2.len(), 1);
        assert!(
            nodes_of_kind(&cites2[0], OrgSyntaxKind::FOOTNOTE_REFERENCE).is_empty(),
            "no footnote-ref in citation prefix"
        );

        // Allowed secondaries: macro in global prefix (no `@` before first key
        // so key search is not stolen by export `@@`); export in key-suffix
        // after a real `@key` (@@ after key is suffix content, not a key lead).
        let src3 = "[cite: {{{m}}} ;@k @@html:x@@]\n";
        let root3 = parse_document_tree(src3);
        assert_eq!(tree_text(&root3), src3);
        let cites3 = nodes_of_kind(&root3, OrgSyntaxKind::CITATION);
        assert_eq!(cites3.len(), 1);
        assert_eq!(nodes_of_kind(&cites3[0], OrgSyntaxKind::MACRO).len(), 1);
        assert_eq!(
            nodes_of_kind(&cites3[0], OrgSyntaxKind::EXPORT_SNIPPET).len(),
            1
        );
    }

    // -- coexistence with other bracket forms -------------------------------

    #[test]
    fn non_theft_footnote_cookie_link_timestamp_export_list() {
        // Column-0 citation is still an object under a paragraph.
        let src = "[cite:@k]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::CITATION).len(), 1);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::FOOTNOTE_DEFINITION)
        );

        // Column-0 footnote definition unchanged.
        let src_fn = "[fn:lab] def\n";
        let root_fn = parse_document_tree(src_fn);
        assert_eq!(tree_text(&root_fn), src_fn);
        assert_eq!(
            nodes_of_kind(&root_fn, OrgSyntaxKind::FOOTNOTE_DEFINITION).len(),
            1
        );
        assert!(
            nodes_of_kind(&root_fn, OrgSyntaxKind::CITATION).is_empty(),
            "{:?}",
            nodes_of_kind(&root_fn, OrgSyntaxKind::CITATION)
        );

        // Footnote ref (not col-0), cookie, link, timestamp win their forms.
        let src2 = "see [fn:1] [1/2] [[a]] [2020-01-01 Mon]\n";
        let root2 = parse_document_tree(src2);
        assert_eq!(tree_text(&root2), src2);
        assert!(
            nodes_of_kind(&root2, OrgSyntaxKind::CITATION).is_empty(),
            "{:?}",
            nodes_of_kind(&root2, OrgSyntaxKind::CITATION)
        );
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::FOOTNOTE_REFERENCE).len(),
            1
        );
        assert_eq!(
            nodes_of_kind(&root2, OrgSyntaxKind::STATISTICS_COOKIE).len(),
            1
        );
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::LINK).len(), 1);
        assert_eq!(nodes_of_kind(&root2, OrgSyntaxKind::TIMESTAMP).len(), 1);

        // Export snippet on @@.
        let src3 = "@@html:@x@@\n";
        let root3 = parse_document_tree(src3);
        assert_eq!(tree_text(&root3), src3);
        assert_eq!(
            nodes_of_kind(&root3, OrgSyntaxKind::EXPORT_SNIPPET).len(),
            1
        );
        assert!(
            nodes_of_kind(&root3, OrgSyntaxKind::CITATION).is_empty(),
            "{:?}",
            nodes_of_kind(&root3, OrgSyntaxKind::CITATION)
        );

        // List counter cookie anatomy, not citation.
        let src4 = "1. [@2] item\n";
        let root4 = parse_document_tree(src4);
        assert_eq!(tree_text(&root4), src4);
        assert!(
            nodes_of_kind(&root4, OrgSyntaxKind::CITATION).is_empty(),
            "{:?}",
            nodes_of_kind(&root4, OrgSyntaxKind::CITATION)
        );
    }

    // -- tiling / progress / harness helpers --------------------------------

    #[test]
    fn post_blank_outside_citation_extent() {
        let src = "[cite:@k]  tail\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].text().to_string(), "[cite:@k]");
    }

    #[test]
    fn parse_uses_shape_without_remeasure() {
        let src = "[cite:@k]";
        let shape = probe(&make_parser(src)).expect("citation");
        let mut p = make_parser(src);
        parse(&mut p, shape);
        assert_eq!(p.current(), OrgSyntaxKind::EOF);
        let (_, evs, _) = p.finish();
        assert_eq!(
            node_text(&evs, src, OrgSyntaxKind::CITATION).as_deref(),
            Some(src)
        );

        let (_, evs2, _) = run_object(src, probe, parse);
        assert_eq!(
            node_text(&evs2, src, OrgSyntaxKind::CITATION).as_deref(),
            Some(src)
        );
    }

    #[test]
    fn caption_keyword_allows_citation() {
        // CAPTION is STANDARD_NO_FOOTNOTE (citation stays); keyword values
        // re-enter objects under that scope.
        let src = "#+CAPTION: see [cite:@k]\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let cites = nodes_of_kind(&root, OrgSyntaxKind::CITATION);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].text().to_string(), "[cite:@k]");
    }
}
