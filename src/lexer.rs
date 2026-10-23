//! The tokenizer: `&str` -> tiled tokens + line headers.
//!
//! Every byte of the source is covered by exactly one token (whitespace and
//! newlines included), which is the foundation of losslessness: the builder later
//! replays these tokens into the green tree verbatim, so
//! `concat(token.text for token in lex(s).tokens) == s` holds for *all* input.
//!
//! ## Shape
//!
//! The loop is a thin driver over the dispatch table below: index the table by
//! the current byte, then either emit a one-byte delimiter token or scan a run
//! (newline / whitespace / text). The token *set* and which bytes are special
//! live in the table; the loop only knows *how to scan* each of the four
//! classes. Adding a delimiter does not touch the scan loop.
//!
//! ## Line headers (fused)
//!
//! While tiling, the same walk records one [`Line`] per source line: token-index
//! range, leading indent columns, blankness, and content start, so the parser
//! can make line-level decisions without rescanning tokens. Indent columns
//! follow Org list rules (space=1; tab to next multiple of 8).
//!
//! ## The one bit of context: line starts
//!
//! A whitespace run at the start of a line is emitted as `INDENT` rather than
//! `WHITESPACE`. That is the only token-kind state the loop carries (`bol`),
//! flipped true after every `NEWLINE`.
//!
//! ## Coordinates
//!
//! Tokens carry an absolute byte range into the source. They are rebuilt on
//! every parse and dropped once the green tree, which stores only lengths, is
//! built.

use rowan::{TextRange, TextSize};

use crate::syntax::OrgSyntaxKind;

// ---------------------------------------------------------------------------
// Dispatch table: `DISPATCH[byte]` answers, in one array load, "what kind of
// token starts with this byte?"
//
// ### Why this is UTF-8 safe
//
// Every byte we mark special is ASCII (`< 0x80`). UTF-8 guarantees that no byte
// of a multi-byte sequence is ever an ASCII byte (continuation bytes are
// `0x80..=0xBF`, lead bytes `0xC0..`). So every non-ASCII byte falls through to
// `Dispatch::Text`, and a `Text` run started anywhere can only ever *end* at an
// ASCII boundary - it can never split a character. The table makes UTF-8
// correctness a structural property, not a runtime check.
// ---------------------------------------------------------------------------

/// What the lexer does when a token begins with a given byte.
#[derive(Debug, Clone, Copy)]
enum Dispatch {
    /// `\n` / `\r` - scan one line terminator (`\r\n` counts as one).
    Newline,
    /// space / tab - scan a run (becomes `Indent` at line start, else `Whitespace`).
    Space,
    /// A delimiter that is always exactly one byte of the given kind.
    Single(OrgSyntaxKind),
    /// Everything else - scan a maximal run of ordinary bytes.
    Text,
}

/// Build the 256-entry table at compile time. Default is `Text`; only the bytes
/// Org gives lexical meaning to are overridden. `-` and `.` stay `Text` (see
/// the `syntax` module docs).
const fn dispatch_table() -> [Dispatch; 256] {
    // The table names every one-byte delimiter kind.
    #[allow(clippy::enum_glob_use)]
    use OrgSyntaxKind::*;

    let mut t = [Dispatch::Text; 256];

    // Line structure.
    t[b'\n' as usize] = Dispatch::Newline;
    t[b'\r' as usize] = Dispatch::Newline;
    t[b' ' as usize] = Dispatch::Space;
    t[b'\t' as usize] = Dispatch::Space;

    // Emphasis / markup markers.
    t[b'*' as usize] = Dispatch::Single(STAR);
    t[b'/' as usize] = Dispatch::Single(SLASH);
    t[b'_' as usize] = Dispatch::Single(UNDERSCORE);
    t[b'=' as usize] = Dispatch::Single(EQUALS);
    t[b'~' as usize] = Dispatch::Single(TILDE);
    t[b'+' as usize] = Dispatch::Single(PLUS);

    // Line-level / structural punctuation.
    t[b':' as usize] = Dispatch::Single(COLON);
    t[b'#' as usize] = Dispatch::Single(HASH);
    t[b'|' as usize] = Dispatch::Single(PIPE);

    // Brackets, braces, angles, parens.
    t[b'[' as usize] = Dispatch::Single(L_BRACKET);
    t[b']' as usize] = Dispatch::Single(R_BRACKET);
    t[b'{' as usize] = Dispatch::Single(L_BRACE);
    t[b'}' as usize] = Dispatch::Single(R_BRACE);
    t[b'<' as usize] = Dispatch::Single(L_ANGLE);
    t[b'>' as usize] = Dispatch::Single(R_ANGLE);
    t[b'(' as usize] = Dispatch::Single(L_PAREN);
    t[b')' as usize] = Dispatch::Single(R_PAREN);

    // Escapes & sundry object leads.
    t[b'\\' as usize] = Dispatch::Single(BACKSLASH);
    t[b'@' as usize] = Dispatch::Single(AT);
    t[b'$' as usize] = Dispatch::Single(DOLLAR);
    t[b'^' as usize] = Dispatch::Single(CARET);
    t[b'%' as usize] = Dispatch::Single(PERCENT);

    t
}

/// The dispatch table, built at compile time.
const DISPATCH: [Dispatch; 256] = dispatch_table();

/// Does `b` continue a `Text` run? Read from `DISPATCH` so it always matches
/// the delimiter set.
#[inline]
fn is_text(b: u8) -> bool {
    matches!(DISPATCH[b as usize], Dispatch::Text)
}

/// Does `b` continue a whitespace run (space or tab)?
#[inline]
fn is_space(b: u8) -> bool {
    matches!(DISPATCH[b as usize], Dispatch::Space)
}

/// One primitive token with an absolute byte range into the source. Contiguous by
/// construction: the ranges of `lex(s).tokens` tile `[0, s.len())` with no gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LexedToken {
    pub kind: OrgSyntaxKind,
    pub range: TextRange,
}

/// One source line as a contiguous slice of the lex token vector.
///
/// `first..end` is exclusive-end in token indices. Lines tile the token vector:
/// `line[i].end == line[i+1].first`, and the last `end` equals `tokens.len()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Line {
    /// Inclusive first token index of the line.
    pub first: u32,
    /// Exclusive end token index (start of next line, or `tokens.len()`).
    pub end: u32,
    /// Token index of the first non-`INDENT` token (the `NEWLINE` on a blank
    /// line). An indent-only last line with no newline keeps `first`.
    pub content: u32,
    /// Leading indent in columns (space=1, tab stops every 8). Zero if no
    /// leading `INDENT` token.
    pub indent_cols: u32,
    /// True when the line is only optional `INDENT` + `NEWLINE`, or a lone
    /// `NEWLINE`. Indent-only at EOF without a newline is **not** blank.
    pub is_blank: bool,
}

/// Full lex output: lossless token tiling plus line headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lexed {
    pub tokens: Vec<LexedToken>,
    pub lines: Vec<Line>,
}

/// In-progress line while tiling (closed into [`Line`] at NEWLINE or EOF).
struct OpenLine {
    first: u32,
    /// First non-INDENT token index once known; otherwise `first`.
    content: u32,
    indent_cols: u32,
    /// True after any non-INDENT, non-NEWLINE token on this line.
    had_content: bool,
}

impl OpenLine {
    fn new(first: u32) -> Self {
        Self {
            first,
            content: first,
            indent_cols: 0,
            had_content: false,
        }
    }

    fn finish(self, end: u32, is_blank: bool) -> Line {
        // content always in [first, end). Blank lines point at the NEWLINE;
        // indent-only EOF (not blank) keeps first.
        let content = if self.had_content {
            self.content
        } else if is_blank {
            end - 1
        } else {
            self.first
        };
        Line {
            first: self.first,
            end,
            content,
            indent_cols: self.indent_cols,
            is_blank,
        }
    }
}

/// Tokenize `source` and build the line index in one walk.
#[must_use]
pub(crate) fn lex(source: &str) -> Lexed {
    let bytes = source.as_bytes();
    let mut tokens = Vec::with_capacity(source.len() / 2 + 8);
    let mut lines = Vec::with_capacity(8);
    let mut pos = 0usize;
    let mut bol = true;
    let mut open: Option<OpenLine> = None;

    while pos < bytes.len() {
        let start = pos;
        let kind = match DISPATCH[bytes[pos] as usize] {
            Dispatch::Newline => {
                pos = eat_newline(bytes, pos);
                OrgSyntaxKind::NEWLINE
            }
            Dispatch::Space => {
                pos = eat_while(bytes, pos, is_space);
                if bol {
                    OrgSyntaxKind::INDENT
                } else {
                    OrgSyntaxKind::WHITESPACE
                }
            }
            Dispatch::Single(kind) => {
                pos += 1;
                kind
            }
            Dispatch::Text => {
                pos = eat_while(bytes, pos, is_text);
                OrgSyntaxKind::TEXT
            }
        };

        let tok_i = tokens.len() as u32;
        let line = open.get_or_insert_with(|| OpenLine::new(tok_i));

        match kind {
            OrgSyntaxKind::INDENT => {
                line.indent_cols = indent_columns(&bytes[start..pos]);
                // Next token is content (or the NEWLINE on a blank indented line).
                line.content = tok_i + 1;
            }
            OrgSyntaxKind::NEWLINE => {}
            _ => line.had_content = true,
        }

        tokens.push(LexedToken {
            kind,
            range: span(start, pos),
        });

        if kind == OrgSyntaxKind::NEWLINE {
            // Blank = only optional INDENT + this NEWLINE.
            if let Some(line) = open.take() {
                let is_blank = !line.had_content;
                lines.push(line.finish(tokens.len() as u32, is_blank));
            }
            bol = true;
        } else {
            bol = false;
        }
    }

    if let Some(line) = open {
        // No final NEWLINE: never a structural blank (even indent-only).
        lines.push(line.finish(tokens.len() as u32, false));
    }

    Lexed { tokens, lines }
}

/// Advance over one line terminator. `bytes[pos]` is `\n` or `\r`; a `\r\n` pair
/// is a single terminator.
#[inline]
fn eat_newline(bytes: &[u8], pos: usize) -> usize {
    if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') {
        pos + 2
    } else {
        pos + 1
    }
}

/// Advance while `pred` holds.
#[inline]
fn eat_while(bytes: &[u8], mut pos: usize, pred: fn(u8) -> bool) -> usize {
    while pos < bytes.len() && pred(bytes[pos]) {
        pos += 1;
    }
    pos
}

#[inline]
fn span(from: usize, to: usize) -> TextRange {
    TextRange::new(TextSize::from(from as u32), TextSize::from(to as u32))
}

/// Columns for a leading indent run (space=1; tab to next multiple of 8).
fn indent_columns(indent_bytes: &[u8]) -> u32 {
    let mut cols = 0u32;
    for &b in indent_bytes {
        if b == b'\t' {
            cols = (cols / 8 + 1) * 8;
        } else {
            cols = cols.saturating_add(1);
        }
    }
    cols
}

/// Largest line index with `lines[i].first <= abs`, or `None` if empty / past end.
#[inline]
pub(crate) fn line_at_token(lines: &[Line], abs: usize) -> Option<usize> {
    if lines.is_empty() || abs >= lines[lines.len() - 1].end as usize {
        return None;
    }
    // First index with first > abs, then step back.
    lines
        .partition_point(|l| (l.first as usize) <= abs)
        .checked_sub(1)
}

#[cfg(test)]
mod tests {
    // The string literals below contain deliberate non-ASCII text (accented
    // letters, an em dash, emoji) to exercise UTF-8 handling.
    use super::*;

    fn kinds(src: &str) -> Vec<OrgSyntaxKind> {
        lex(src).tokens.into_iter().map(|t| t.kind).collect()
    }

    fn assert_lossless(src: &str) {
        let mut cursor = 0usize;
        let mut rebuilt = String::new();
        let out = lex(src);
        for t in &out.tokens {
            let start = usize::from(t.range.start());
            let end = usize::from(t.range.end());
            assert_eq!(start, cursor, "gap or overlap before {t:?} in {src:?}");
            assert!(start < end, "zero-length token {t:?} in {src:?}");
            assert!(
                src.is_char_boundary(start) && src.is_char_boundary(end),
                "token {t:?} splits a UTF-8 char in {src:?}"
            );
            rebuilt.push_str(&src[start..end]);
            cursor = end;
        }
        assert_eq!(cursor, src.len(), "tokens do not cover {src:?}");
        assert_eq!(rebuilt, src, "round-trip mismatch for {src:?}");
        assert_tile(&out.tokens, &out.lines);
    }

    fn assert_tile(tokens: &[LexedToken], lines: &[Line]) {
        if tokens.is_empty() {
            assert!(lines.is_empty(), "{lines:?}");
            return;
        }
        assert!(!lines.is_empty(), "{lines:?}");
        assert_eq!(lines[0].first, 0);
        assert_eq!(lines[lines.len() - 1].end as usize, tokens.len());
        for w in lines.windows(2) {
            assert_eq!(w[0].end, w[1].first);
        }
        for (abs, _) in tokens.iter().enumerate() {
            let li = line_at_token(lines, abs).expect("token must belong to a line");
            let line = lines[li];
            assert!((line.first as usize) <= abs && abs < (line.end as usize));
        }
        assert!(line_at_token(lines, tokens.len()).is_none());
    }

    #[test]
    fn covers_and_round_trips_every_shape() {
        for src in [
            "",
            "\n",
            "\r\n",
            "* TODO [#A] Buy milk :work:home:\n",
            "  - nested item\n",
            "plain paragraph text\n",
            "/italic/ *bold* =verb= ~code~ +strike+ _under_\n",
            "[[https://example.com][link]]\n",
            "a <2026-06-24 Wed> b [2026-06-24]\n",
            "#+TITLE: Hello World\n",
            ": fixed width line\n",
            "| a | b |\n|---+---|\n",
            "café ☕ déjà-vu — emoji 🎉 mixed\n",
            "no trailing newline",
            "tab\tseparated\tvalues\n",
            "   \n",
            "\\alpha \\\\ end\n",
            "{{{macro(arg)}}} @@html:<b>@@ x^2 $a$ 50%\n",
        ] {
            assert_lossless(src);
        }
    }

    /// `BOF` / `EOF` are cursor sentinels owned by the parser, never lex tiles.
    /// A leaked sentinel would break tiling (they carry no source bytes).
    #[test]
    fn lexer_never_produces_cursor_sentinels() {
        for s in ["", "hello", "* H :t:1:\n  body\n", "   # c\nx\n"] {
            for t in &lex(s).tokens {
                assert_ne!(t.kind, OrgSyntaxKind::BOF, "lexer produced BOF on {s:?}");
                assert_ne!(t.kind, OrgSyntaxKind::EOF, "lexer produced EOF on {s:?}");
            }
        }
    }

    #[test]
    fn utf8_runs_are_never_split() {
        assert_lossless("*café*");
        assert_lossless("☕:tag:");
        assert_lossless("a—b/c—d");
        assert_eq!(kinds("café"), vec![OrgSyntaxKind::TEXT]);
        assert_eq!(kinds("🎉"), vec![OrgSyntaxKind::TEXT]);
    }

    #[test]
    fn leading_whitespace_is_indent_inner_is_whitespace() {
        assert_eq!(
            kinds("  a b\n"),
            vec![
                OrgSyntaxKind::INDENT,
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::WHITESPACE,
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::NEWLINE
            ]
        );
    }

    #[test]
    fn whitespace_only_line_is_indent_then_newline() {
        assert_eq!(
            kinds("   \n"),
            vec![OrgSyntaxKind::INDENT, OrgSyntaxKind::NEWLINE]
        );
    }

    #[test]
    fn headline_stars_are_single_tokens() {
        assert_eq!(
            kinds("*** x\n"),
            vec![
                OrgSyntaxKind::STAR,
                OrgSyntaxKind::STAR,
                OrgSyntaxKind::STAR,
                OrgSyntaxKind::WHITESPACE,
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::NEWLINE
            ]
        );
    }

    #[test]
    fn crlf_is_a_single_newline_token() {
        let toks = &lex("a\r\nb").tokens;
        assert_eq!(toks[1].kind, OrgSyntaxKind::NEWLINE);
        let start = usize::from(toks[1].range.start());
        let end = usize::from(toks[1].range.end());
        assert_eq!(&"a\r\nb"[start..end], "\r\n");
    }

    #[test]
    fn delimiters_break_text_runs() {
        assert_eq!(
            kinds("a:b"),
            vec![
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::COLON,
                OrgSyntaxKind::TEXT
            ]
        );
        assert_eq!(
            kinds("[[x]]"),
            vec![
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::L_BRACKET,
                OrgSyntaxKind::TEXT,
                OrgSyntaxKind::R_BRACKET,
                OrgSyntaxKind::R_BRACKET
            ]
        );
    }

    #[test]
    fn hyphen_and_dot_stay_in_text() {
        assert_eq!(kinds("well-known"), vec![OrgSyntaxKind::TEXT]);
        assert_eq!(kinds("3.14"), vec![OrgSyntaxKind::TEXT]);
        assert_eq!(kinds("-----"), vec![OrgSyntaxKind::TEXT]);
    }

    #[test]
    fn line_headers_blank_indent_content() {
        let out = lex("a\n\n  \nb\n");
        assert_eq!(out.lines.len(), 4);
        assert!(!out.lines[0].is_blank);
        assert!(out.lines[1].is_blank);
        assert!(out.lines[2].is_blank);
        assert_eq!(out.lines[2].indent_cols, 2);
        assert!(!out.lines[3].is_blank);

        let ind = lex("  # c\n");
        assert_eq!(ind.lines[0].indent_cols, 2);
        assert_eq!(ind.lines[0].content, 1);
        assert_eq!(ind.tokens[1].kind, OrgSyntaxKind::HASH);

        let bare = lex("# c\n");
        assert_eq!(bare.lines[0].content, 0);

        // Blank: content points at the NEWLINE tile (inside first..end).
        let blank = lex("\n");
        assert!(blank.lines[0].is_blank);
        assert_eq!(blank.lines[0].content, 0);
        assert_eq!(
            blank.tokens[blank.lines[0].content as usize].kind,
            OrgSyntaxKind::NEWLINE
        );

        let blank_ind = lex("  \n");
        assert!(blank_ind.lines[0].is_blank);
        assert_eq!(blank_ind.lines[0].content, 1);
        assert_eq!(
            blank_ind.tokens[blank_ind.lines[0].content as usize].kind,
            OrgSyntaxKind::NEWLINE
        );

        let eof_indent = lex("   ");
        assert_eq!(eof_indent.lines.len(), 1);
        assert!(!eof_indent.lines[0].is_blank);
        assert_eq!(eof_indent.lines[0].indent_cols, 3);
        assert_eq!(eof_indent.lines[0].content, 0); // no past-end content index

        let tab = lex("\tx\n");
        assert_eq!(tab.lines[0].indent_cols, 8);
    }
}
