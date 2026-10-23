//! LaTeX family: fragment objects (five org forms) and environments.
//!
//! Evidence:
//! - Worg 5.2 LaTeX Fragments; 4.3.9 LaTeX Environments
//! - `org-element-latex-fragment-parser` / `org-element--object-lex` (`?\\` else
//!   arm after entity; separate `?$` arm)
//! - `org-element-latex-environment-parser` / `org-element--latex-end-environment`
//!
//! ## Fragments
//!
//! Forms recognised:
//! 1. `\(...\)`  math parens
//! 2. `\[...\]`  math brackets
//! 3. `\NAME*` plus zero or more `[]` / `{}` args (structural command; entity
//!    already claimed dictionary names on the same backslash arm)
//! 4. `$$...$$`
//! 5. `$...$` with PRE / open-forbid / BORDER2 / POST rules
//!
//! **Backslash family:** `forms/inline.rs` `probe_backslash` tries entity then
//! this module's [`probe_fragment_backslash`]. `\\` never falls through.
//! Dollar forms use a separate candidate row ([`probe_fragment_dollar`]).
//!
//! Emit is one flat [`OrgSyntaxKind::LATEX_FRAGMENT`] leaf of the measured
//! length via [`Parser::leaf_len`]. It covers the delimiters and may end
//! mid-tile. Which of the five forms a fragment is can be read back from its
//! text.
//!
//! Malformed fragments stay plain text, with no diagnostic.
//!
//! ## Environments
//!
//! Line-first element: `^[ \t]*\\begin{NAME}` with non-empty `NAME` of
//! `[A-Za-z0-9*]+`, closed by a line that **ends** with `\end{NAME}[ \t]*`
//! (prefix content before `\end` allowed; ASCII caseless name match). End
//! look-up is heading-section half-open (`opener < end < next_heading`) via
//! amortised [`crate::parser::grammar::drive::frame::latex_env_end_after`].
//! Unclosed / heading-blocked openers degrade with
//! [`decide::DegradeReason::UnterminatedLatexEnvironment`] (`interrupts: false`).
//! Body is opaque (`FrameShape.body: BodyMode::Opaque`). Mid-paragraph `\begin{...}`
//! remains a fragment (command + brace arg), never an environment.
//!
//! Fragment probes return the exclusive byte length of the form and
//! [`parse_fragment`] emits the leaf from it. Environments parse through
//! [`frame::parse_frame`] with the whole begin line as the opener.
//!
//! Pure fragment measures (math, command, dollar) and the math-closer miss
//! memo ([`MathCloserMissMemo`] on [`Parser`]) live in this module.

use crate::parser::grammar::drive::{decide, frame};
use crate::parser::{LineView, Parser, Pos};
use crate::syntax::OrgSyntaxKind;

/// Measured latex fragment: exclusive source-byte length from the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LatexFragmentShape {
    /// Bytes from cursor through closer (opener inclusive).
    pub(crate) len: usize,
}

/// Pure measure of a backslash-started latex fragment at the cursor.
///
/// Precondition: cursor on `BACKSLASH`. Caller already branched away from
/// `\\` (line-break only); this still returns `None` on a double backslash so
/// a direct call cannot mis-claim line-break openers.
pub(crate) fn probe_fragment_backslash(parser: &Parser) -> Option<LatexFragmentShape> {
    if parser.current() != OrgSyntaxKind::BACKSLASH {
        return None;
    }
    // Safety: latex never claims `\\` (math needs `(`/`[`; command needs letters).
    if parser.here()?.run_of(OrgSyntaxKind::BACKSLASH, 2) {
        return None;
    }
    let rest = parser.rest_source();
    let after_bs = rest.get(1..)?;
    // Absolute byte start of `after_bs` for the math-closer miss memo.
    let after_bs_abs = parser.source_start_here()? + 1;
    let body = measure_backslash_body(after_bs, after_bs_abs, parser)?;
    Some(LatexFragmentShape { len: 1 + body })
}

/// Pure measure of a dollar-started latex fragment at the cursor.
pub(crate) fn probe_fragment_dollar(parser: &Parser) -> Option<LatexFragmentShape> {
    if parser.current() != OrgSyntaxKind::DOLLAR {
        return None;
    }
    let rest = parser.rest_source();
    if !rest.starts_with('$') {
        // Empty under the active cap, or non-dollar (should not happen on DOLLAR).
        return None;
    }
    // `$$...$$` first so PRE on the second `$` of an opener pair never runs.
    if rest.starts_with("$$") {
        let len = measure_double_dollar(rest)?;
        return Some(LatexFragmentShape { len });
    }
    // Single `$`: PRE is "byte before opener is not `$`".
    if parser.prev_byte() == Some(b'$') {
        return None;
    }
    let len = measure_single_dollar(rest)?;
    Some(LatexFragmentShape { len })
}

/// Emit one latex fragment from a measured shape (no remeasure).
pub(crate) fn parse_fragment(parser: &mut Parser, shape: LatexFragmentShape) {
    let ok = parser.leaf_len(OrgSyntaxKind::LATEX_FRAGMENT, shape.len);
    debug_assert!(ok, "latex fragment leaf fits the length probe measured");
}

// ---------------------------------------------------------------------------
// Environment (element)
// ---------------------------------------------------------------------------

/// Cheap opener-line claim: `\begin{NAME}` at line content after optional indent.
///
/// Carries the NAME token span so [`resolve`] does not re-scan. Empty name
/// (`\begin{}`) is **not** an opener -- [`opener`] returns `None` and the
/// line stays ordinary paragraph text (no empty-name diagnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LatexEnvOpener {
    /// Content-relative token index of the first NAME token (after `{`).
    name_from: usize,
    /// Token count of NAME (empty name is rejected by [`opener`]).
    name_tokens: usize,
}

impl LatexEnvOpener {
    fn name<'s>(self, line: LineView<'_, 's>) -> &'s str {
        line.text_range(self.name_from, self.name_from + self.name_tokens)
    }
}

/// Parse-ready environment: opaque frame through matching end line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LatexEnvShape {
    frame: frame::FrameShape,
}

impl LatexEnvShape {
    /// Exclusive body end: the start of the end line. List parsing uses it to
    /// skip the whole frame.
    #[inline]
    pub(crate) fn body_end(self) -> Pos {
        self.frame.body_end()
    }
}

/// Pure: whether `line` is a latex-environment begin line with non-empty NAME.
///
/// Shape: optional leading INDENT (already skipped by content indexing), then
/// `\begin{NAME}` with `NAME` matching `[A-Za-z0-9*]+`. Trailing material after
/// the closing brace is allowed.
pub(crate) fn opener(line: LineView) -> Option<LatexEnvOpener> {
    let (name_from, name_tokens) = begin_name_span(line)?;
    let op = LatexEnvOpener {
        name_from,
        name_tokens,
    };
    is_env_name(op.name(line)).then_some(op)
}

/// Source text of NAME inside `\begin{NAME}` on a begin line, or `""` if not.
///
/// Used to build the diagnostic for a degraded opener, so
/// [`decide::DegradeReason`] does not need to carry the name. [`resolve`]
/// reads the carried opener instead.
pub(crate) fn opener_name<'s>(line: LineView<'_, 's>) -> &'s str {
    begin_name(line).unwrap_or("")
}

/// Whether `line` ends with `\end{NAME}[ \t]*` (org end form; prefix allowed).
///
/// Returns the NAME slice when the line is an end candidate. Empty name
/// (`\end{}`) is not a candidate.
pub(crate) fn end_name<'s>(line: LineView<'_, 's>) -> Option<&'s str> {
    let text = line_content_text(line)?;
    end_name_in(text)
}

/// Whether a pre-validated end line's name matches `want` (ASCII caseless).
pub(crate) fn end_name_matches(line: LineView, want: &str) -> bool {
    end_name(line).is_some_and(|n| n.eq_ignore_ascii_case(want))
}

/// Validate an opener into a parse-ready shape, or a Copy degrade reason.
///
/// The end lookup is amortised ([`frame::latex_env_end_after`]) and bounded by
/// the next heading. It does not allocate; the diagnostic message is built
/// later from the reason, re-reading the name from the opener line.
pub(crate) fn resolve(
    line: LineView,
    op: LatexEnvOpener,
) -> Result<LatexEnvShape, decide::DegradeReason> {
    let name = op.name(line);
    match frame::latex_env_end_after(line.parser(), line, name) {
        Some(body_end) => Ok(LatexEnvShape {
            frame: frame::FrameShape {
                kind: OrgSyntaxKind::LATEX_ENVIRONMENT,
                body_end,
                body: frame::BodyMode::Opaque,
                end_line: true,
            },
        }),
        None => Err(decide::DegradeReason::UnterminatedLatexEnvironment),
    }
}

/// Combined recognition for tests: opener plus successful [`resolve`].
#[cfg(test)]
pub(crate) fn probe_env(line: LineView) -> Option<LatexEnvShape> {
    resolve(line, opener(line)?).ok()
}

/// Parse a complete latex environment from its measured shape (no fail path).
///
/// Emits [`OrgSyntaxKind::LATEX_ENVIRONMENT`]: whole begin line, opaque body
/// tokens, end line, and post-blank via [`frame::parse_frame`].
pub(crate) fn parse_env(parser: &mut Parser, shape: LatexEnvShape) {
    frame::parse_frame(parser, shape.frame, |p| {
        p.eat_line();
    });
}

/// `\begin{NAME}` at content start: returns NAME (may be empty for `\begin{}`).
fn begin_name<'s>(line: LineView<'_, 's>) -> Option<&'s str> {
    let (from, n) = begin_name_span(line)?;
    Some(line.text_range(from, from + n))
}

/// Token span of NAME inside `\begin{NAME}` (empty span for `\begin{}`).
fn begin_name_span(line: LineView) -> Option<(usize, usize)> {
    if line.kind(0) != OrgSyntaxKind::BACKSLASH {
        return None;
    }
    if !line.text_eq_caseless(1, "begin") {
        return None;
    }
    brace_group_span(line, 2)
}

/// Source text between braces starting at content token `brace_k` (`L_BRACE`).
///
/// `None` when the brace group does not close on this line.
#[cfg(test)]
fn brace_group_name<'s>(line: LineView<'_, 's>, brace_k: usize) -> Option<&'s str> {
    let (from, n) = brace_group_span(line, brace_k)?;
    Some(line.text_range(from, from + n))
}

fn brace_group_span(line: LineView, brace_k: usize) -> Option<(usize, usize)> {
    if line.kind(brace_k) != OrgSyntaxKind::L_BRACE {
        return None;
    }
    // Tokens inside the group until `}` or the line terminator (nth-bounded).
    let inner = line.run_while(brace_k + 1, |kind, _| {
        kind != OrgSyntaxKind::R_BRACE && !kind.is_line_end()
    });
    let close_k = brace_k + 1 + inner;
    if line.kind(close_k) != OrgSyntaxKind::R_BRACE {
        return None;
    }
    Some((brace_k + 1, inner))
}

/// Contiguous content-token text of the line (excludes leading INDENT and the
/// line terminator).
fn line_content_text<'s>(line: LineView<'_, 's>) -> Option<&'s str> {
    let end_k = line.run_while(0, |kind, _| !kind.is_line_end());
    if end_k == 0 {
        return None;
    }
    Some(line.text_range(0, end_k))
}

/// Trailing org end form on a content-text slice: `\end{NAME}[ \t]*$`.
///
/// `text` is the line content (no leading indent token; may include spaces).
/// Returns NAME when the form matches and NAME is non-empty `[A-Za-z0-9*]+`.
///
/// Walks and compares on **bytes** so multi-byte UTF-8 before `{NAME}` cannot
/// panic via `str` indexing on non-char-boundary offsets. The NAME itself is
/// restricted to ASCII `[A-Za-z0-9*]+`, so the final name slice is safe.
fn end_name_in(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    let mut end = b.len();
    while end > 0 && (b[end - 1] == b' ' || b[end - 1] == b'\t') {
        end -= 1;
    }
    if end == 0 || b[end - 1] != b'}' {
        return None;
    }
    // `}` at end-1; walk back over name bytes.
    let name_end = end - 1;
    let mut name_start = name_end;
    while name_start > 0 && is_env_name_byte(b[name_start - 1]) {
        name_start -= 1;
    }
    if name_start == name_end {
        // Empty name: `\end{}`.
        return None;
    }
    // Need `\end{` immediately before the name.
    // Layout: ... \ e n d { NAME }
    //         ... ^-5   ^-1 ^name_start  ^name_end
    if name_start < 5 {
        return None;
    }
    if b[name_start - 1] != b'{' {
        return None;
    }
    // Byte compare: offsets come from an as_bytes walk and need not be char
    // boundaries once multi-byte UTF-8 sits in the window before `{`.
    if !b[name_start - 4..name_start - 1].eq_ignore_ascii_case(b"end") {
        return None;
    }
    if b[name_start - 5] != b'\\' {
        return None;
    }
    // NAME is pure ASCII env chars (is_env_name_byte), so this slice is safe.
    let name = &text[name_start..name_end];
    is_env_name(name).then_some(name)
}

/// Org env NAME class: non-empty `[A-Za-z0-9*]+`.
#[inline]
fn is_env_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(is_env_name_byte)
}

#[inline]
fn is_env_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'*'
}

// ---------------------------------------------------------------------------
// Math closer miss memo
// ---------------------------------------------------------------------------

/// Math closer kind for the unclosed-scan miss memo (`\)` vs `\]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MathCloserKind {
    Paren,
    Bracket,
}

/// Absolute source ranges known to contain no math closer of a kind.
///
/// After a miss searching `source[from..end)` (half-open absolute byte range),
/// later searches of the same kind whose window starts at or after `from` and
/// ends at or before `end` are O(1) misses. Restores linear work for N unclosed
/// `\(`/`\[` openers in one region (each would otherwise rescan the remaining
/// paragraph independently).
///
/// Held on [`Parser`] and updated only by miss paths; successful finds leave
/// the memo unchanged. A miss already covered by a wider recorded span is a
/// no-op (nested limits). Any other miss **adopts** the new window so a later
/// region (e.g. the next paragraph) can re-linearise after the prior span no
/// longer covers its searches.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MathCloserMissMemo {
    /// No `\)` in `source[from..end)`.
    paren: Option<(usize, usize)>,
    /// No `\]` in `source[from..end)`.
    bracket: Option<(usize, usize)>,
}

impl MathCloserMissMemo {
    #[inline]
    fn slot(&self, kind: MathCloserKind) -> Option<(usize, usize)> {
        match kind {
            MathCloserKind::Paren => self.paren,
            MathCloserKind::Bracket => self.bracket,
        }
    }

    #[inline]
    fn known_miss(&self, kind: MathCloserKind, from: usize, end: usize) -> bool {
        matches!(self.slot(kind), Some((f, e)) if from >= f && end <= e)
    }

    /// Record that `source[from..end)` has no closer of `kind`.
    ///
    /// - Already covered by a wider-or-equal known-empty span: no-op (nested
    ///   limit under an outer miss stays O(1) without shrinking the outer).
    /// - Otherwise adopt `(from, end)`: expand, replace a prior region, or
    ///   first write. Multi-paragraph unclosed runs therefore stay linear
    ///   (each paragraph's first miss becomes the active span).
    fn record_miss(&mut self, kind: MathCloserKind, from: usize, end: usize) {
        let slot = match kind {
            MathCloserKind::Paren => &mut self.paren,
            MathCloserKind::Bracket => &mut self.bracket,
        };
        match *slot {
            Some((f, e)) if from >= f && end <= e => {}
            _ => *slot = Some((from, end)),
        }
    }
}

// ---------------------------------------------------------------------------
// Backslash family measures
// ---------------------------------------------------------------------------

/// After a leading `\`, exclusive length of the fragment body, or `None`.
///
/// `after_bs_abs` is the absolute source-byte start of `after_bs` (for the
/// math-closer miss memo). `parser` supplies the shared memo and test work
/// counter.
fn measure_backslash_body(after_bs: &str, after_bs_abs: usize, parser: &Parser) -> Option<usize> {
    let b = after_bs.as_bytes();
    if b.is_empty() {
        return None;
    }
    match b[0] {
        b'(' => measure_math_closer(
            after_bs,
            br"\)",
            after_bs_abs,
            MathCloserKind::Paren,
            Some(parser),
        ),
        b'[' => measure_math_closer(
            after_bs,
            br"\]",
            after_bs_abs,
            MathCloserKind::Bracket,
            Some(parser),
        ),
        _ => measure_command(after_bs),
    }
}

/// `\(...\)` / `\[...\]`: find the first `\` + closer byte after the opener char.
///
/// `closer_tail` is the two-byte sequence starting at the closing backslash
/// (`\)` or `\]`). Search is on the source slice (region-capped by the caller).
///
/// When `parser` is `Some`, a miss records a per-kind known-empty range
/// `source[haystack_abs..window_end)` on the parser so later unclosed openers
/// in the same window fail without rescanning.
fn measure_math_closer(
    after_bs: &str,
    closer_tail: &[u8],
    after_bs_abs: usize,
    kind: MathCloserKind,
    parser: Option<&Parser>,
) -> Option<usize> {
    // Interior starts after the opener `(` or `[`.
    let haystack = after_bs.as_bytes().get(1..)?;
    let haystack_abs = after_bs_abs + 1;
    let window_end = haystack_abs + haystack.len();

    if let Some(p) = parser
        && p.math_closer_miss
            .get()
            .known_miss(kind, haystack_abs, window_end)
    {
        return None;
    }

    #[cfg(test)]
    if let Some(p) = parser {
        // Charge the haystack length once per real scan (read by the memo tests).
        p.math_closer_work
            .set(p.math_closer_work.get().saturating_add(haystack.len()));
    }

    let Some(rel) = find_bytes(haystack, closer_tail) else {
        if let Some(p) = parser {
            let mut memo = p.math_closer_miss.get();
            memo.record_miss(kind, haystack_abs, window_end);
            p.math_closer_miss.set(memo);
        }
        return None;
    };
    // Exclusive end in after_bs: skip opener (1) + interior (rel) + closer.
    Some(1 + rel + closer_tail.len())
}

/// Pure measure without parser memo (unit tests of closed/unclosed tables).
#[cfg(test)]
fn measure_math_closer_pure(after_bs: &str, closer_tail: &[u8]) -> Option<usize> {
    let kind = if closer_tail == br"\]" {
        MathCloserKind::Bracket
    } else {
        MathCloserKind::Paren
    };
    measure_math_closer(after_bs, closer_tail, 0, kind, None)
}

/// Pure memoised search for unit tests: the same miss rules as the parser
/// path, with an explicit work counter (bytes examined on real scans).
#[cfg(test)]
fn find_math_closer_memo(
    haystack: &[u8],
    closer_tail: &[u8],
    haystack_abs: usize,
    kind: MathCloserKind,
    memo: &mut MathCloserMissMemo,
    work: &mut usize,
) -> Option<usize> {
    let window_end = haystack_abs + haystack.len();
    if memo.known_miss(kind, haystack_abs, window_end) {
        return None;
    }
    *work = work.saturating_add(haystack.len());
    if let Some(rel) = find_bytes(haystack, closer_tail) {
        Some(rel)
    } else {
        memo.record_miss(kind, haystack_abs, window_end);
        None
    }
}

/// Command form: `[a-zA-Z]+` then optional `*` then zero or more `[]`/`{}` args.
fn measure_command(after_bs: &str) -> Option<usize> {
    let name_end = measure_command_name(after_bs)?;
    let args_end = measure_command_args(&after_bs[name_end..]);
    Some(name_end + args_end)
}

/// Leading ASCII letters, then optional `*`. Returns exclusive end offset.
fn measure_command_name(after_bs: &str) -> Option<usize> {
    let b = after_bs.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    if i < b.len() && b[i] == b'*' {
        i += 1;
    }
    Some(i)
}

/// Zero or more consecutive `[]` or `{}` args; stops at the first incomplete
/// or non-arg character. Returns exclusive end offset in `rest`.
fn measure_command_args(rest: &str) -> usize {
    let mut end = 0usize;
    loop {
        let tail = &rest[end..];
        if let Some(n) = measure_bracket_arg(tail) {
            end += n;
        } else if let Some(n) = measure_brace_arg(tail) {
            end += n;
        } else {
            break;
        }
    }
    end
}

/// `[` ... `]` with interior free of `]`, `[`, `{`, `}`, and newline/CR.
fn measure_bracket_arg(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if b.first().copied() != Some(b'[') {
        return None;
    }
    let mut i = 1usize;
    while i < b.len() {
        match b[i] {
            b']' => return Some(i + 1),
            b'[' | b'{' | b'}' | b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
    None
}

/// `{` ... `}` with interior free of `{`, `}`, and newline/CR.
fn measure_brace_arg(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if b.first().copied() != Some(b'{') {
        return None;
    }
    let mut i = 1usize;
    while i < b.len() {
        match b[i] {
            b'}' => return Some(i + 1),
            b'{' | b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Dollar family measures
// ---------------------------------------------------------------------------

/// `$$...$$` on a slice that starts with `$$`. Exclusive length through closer.
fn measure_double_dollar(rest: &str) -> Option<usize> {
    debug_assert!(rest.starts_with("$$"));
    let after_open = rest.get(2..)?;
    let rel = find_bytes(after_open.as_bytes(), b"$$")?;
    Some(2 + rel + 2)
}

/// Single `$...$` on a slice that starts with one `$` (not `$$`).
///
/// Org's algorithm (where Worg differs, the elisp is followed):
/// 1. After opener, next byte must not be in the forbidden post-opener set.
/// 2. Closer is the first `$` strictly after the opener.
/// 3. Char immediately before the closer must pass [`is_single_dollar_border`].
/// 4. Char immediately after the closer (or end of region) must pass
///    [`is_dollar_post`].
///
/// PRE (prev byte not `$`) is checked by the caller.
fn measure_single_dollar(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if b.first().copied() != Some(b'$') {
        return None;
    }
    let after_open = b.get(1).copied()?;
    if is_forbidden_after_open_dollar(after_open) {
        return None;
    }
    // First `$` after the opener.
    let rel = find_bytes(&b[1..], b"$")?;
    let closer = 1 + rel;
    // BORDER2: char immediately before the closer.
    let before = rest.get(..closer)?.chars().next_back()?;
    if !is_single_dollar_border(before) {
        return None;
    }
    // POST: char after closer, or end of region.
    let post = rest.get(closer + 1..).and_then(|s| s.chars().next());
    if !is_dollar_post(post) {
        return None;
    }
    Some(closer + 1)
}

/// Bytes forbidden immediately after the opening single `$`.
///
/// Org: space, tab, newline, `,`, `.`, `;`.
#[inline]
fn is_forbidden_after_open_dollar(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b',' | b'.' | b';')
}

/// Char immediately before the closing single `$` (BORDER2).
///
/// Rejects space, tab, newline/CR, `,`, `.`.
#[inline]
fn is_single_dollar_border(c: char) -> bool {
    !matches!(c, ' ' | '\t' | '\n' | '\r' | ',' | '.')
}

/// `char-syntax` for bytes `0x00..=0x7F` in an `org-mode` buffer, as reported
/// by Emacs.
///
/// Index is the byte. The class bytes are Emacs syntax: `.` punctuation,
/// ` ` whitespace, `(` open, `)` close, `"` string, `w` word, `_` symbol.
const DOLLAR_POST_SYNTAX: &[u8; 128] = b".........  .  .................. .\".ww_w()__._._wwwwwwwwww..(_)..wwwwwwwwwwwwwwwwwwwwwwwwww(_)._.wwwwwwwwwwwwwwwwwwwwwwwwww(_)_.";

/// POST after closing single `$`, or true at end of region.
///
/// Accepted classes from [`DOLLAR_POST_SYNTAX`]: punctuation `.`, whitespace
/// ` `, open `(`, close `)`, string `"`. Apostrophe is word class `w`;
/// Org's follower regexp also matches a literal `'`. Symbol class `_` is
/// not a follower. Non-ASCII is not in the table.
#[inline]
fn is_dollar_post(c: Option<char>) -> bool {
    let Some(c) = c else {
        return true;
    };
    if !c.is_ascii() {
        return false;
    }
    let class = DOLLAR_POST_SYNTAX[c as usize];
    matches!(class, b'.' | b' ' | b'(' | b')' | b'"') || c == '\''
}

/// First index of `needle` in `haystack`, or `None`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {

    use crate::syntax::restrictions;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree, run_object, tree_text,
    };

    fn run_bs(src: &str) -> crate::tests::ParseResult {
        run_object(src, probe_fragment_backslash, parse_fragment)
    }

    // --- accept matrix -----------------------------------------------------

    #[test]
    fn fragment_backslash_shapes_accept() {
        let cases = [
            ("\\(a\\)", "\\(a\\)".len()),
            ("\\[a\\]", "\\[a\\]".len()),
            ("\\foo", "\\foo".len()),
            ("\\foo*", "\\foo*".len()),
            ("\\foo[opt]{arg}", "\\foo[opt]{arg}".len()),
            ("\\foo[opt]{arg}x", "\\foo[opt]{arg}".len()),
            ("\\begin{align}", "\\begin{align}".len()),
            ("\\alphaa", "\\alphaa".len()),
            // Incomplete arg: command name only.
            ("\\foo[unclosed", "\\foo".len()),
            // Digits after letters are outside the command name.
            ("\\foo2", "\\foo".len()),
        ];
        for (src, want_len) in cases {
            let shape = probe_fragment_backslash(&make_parser(src))
                .unwrap_or_else(|| panic!("expected fragment for {src:?}"));
            assert_eq!(shape.len, want_len, "len for {src:?}");
            let (_, evs, _) = run_bs(src);
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::LATEX_FRAGMENT)
                    .as_deref()
                    .map(str::len),
                Some(want_len),
                "node for {src:?}"
            );
        }
    }

    #[test]
    fn dollar_fragment_document_smoke() {
        let src = "see $a$. and $a$x and $a b$.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let texts: Vec<_> = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT)
            .iter()
            .map(|n| n.text().to_string())
            .collect();
        assert_eq!(texts, vec!["$a$".to_string(), "$a b$".to_string()]);
    }

    #[test]
    fn fragment_math_and_command_in_document() {
        let src = "see \\(a\\) and \\[b\\] and \\foo[opt]{arg} and $$x$$ and $y$.\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        let texts: Vec<_> = frags.iter().map(|n| n.text().to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "\\(a\\)".to_string(),
                "\\[b\\]".to_string(),
                "\\foo[opt]{arg}".to_string(),
                "$$x$$".to_string(),
                "$y$".to_string(),
            ]
        );
    }

    #[test]
    fn multi_line_fragments_in_paragraph() {
        // Single `$`, math parens/brackets, and `$$` all search across newlines
        // inside one paragraph (region-capped source search).
        let src = "p $a\nb$ and \\(x\ny\\) and \\[u\nv\\] and $$m\nn$$ q\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        let texts: Vec<_> = frags.iter().map(|n| n.text().to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "$a\nb$".to_string(),
                "\\(x\ny\\)".to_string(),
                "\\[u\nv\\]".to_string(),
                "$$m\nn$$".to_string(),
            ]
        );
        assert_eq!(nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH).len(), 1);
    }

    #[test]
    fn mid_paragraph_begin_is_fragment_not_environment() {
        // The element dispatcher only looks at line starts, so a mid-paragraph
        // `\begin{align}` is command + brace arg: a LATEX_FRAGMENT, never an
        // environment.
        let src = "prose \\begin{align} more\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].text().to_string(), "\\begin{align}");
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT)
        );
    }

    // --- reject matrix -----------------------------------------------------

    #[test]
    fn fragment_backslash_shapes_reject() {
        for src in [
            "\\",    // bare backslash
            "\\\\",  // line-break opener; never latex
            "\\(",   // unclosed math
            "\\[a",  // unclosed math brackets
            "\\(a",  // unclosed
            "\\1",   // no letter name
            "plain", // not backslash
            "$a$",   // wrong opener family
        ] {
            assert!(
                probe_fragment_backslash(&make_parser(src)).is_none(),
                "not a backslash fragment: {src:?}"
            );
        }
        // Newline inside brace arg: name-only still claims when letters present.
        // Reject the *arg* measure; the command form still accepts `\foo`.
        let shape = probe_fragment_backslash(&make_parser("\\foo{a\nb}")).expect("name only");
        assert_eq!(shape.len, "\\foo".len());
    }

    // --- entity vs command -------------------------------------------------

    #[test]
    fn entity_wins_over_latex_command() {
        // Dictionary entities (letter and digit-bearing builtins) win before
        // the latex command form on the same backslash arm.
        for (src, want_entity) in [
            ("\\alpha\n", "\\alpha"),
            ("\\alpha{}\n", "\\alpha{}"),
            ("1\\cent.\n", "\\cent"),
            ("\\frac12\n", "\\frac12"),
            ("\\there4\n", "\\there4"),
            ("\\sup1\n", "\\sup1"),
        ] {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling {src:?}");
            let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
            assert_eq!(ents.len(), 1, "entity must win: {src:?}");
            assert_eq!(
                ents[0].text().to_string(),
                want_entity,
                "entity text {src:?}"
            );
            assert!(
                nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
                "not a fragment when entity matches: {src:?}"
            );
        }
    }

    #[test]
    fn unknown_command_is_fragment_not_entity() {
        let src = "\\alphaa and \\foo\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY)
        );
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0].text().to_string(), "\\alphaa");
        assert_eq!(frags[1].text().to_string(), "\\foo");
    }

    // --- backslash branch / allowance --------------------------------------
    //
    // Independent memq arms (entity remq'd vs latex remq'd) are pinned in
    // `inline.rs::backslash_row_branches_on_the_second_byte`.

    #[test]
    fn double_backslash_never_latex_when_line_break_declined() {
        // Heading title: line-break not allowed; `\\` must not become entity
        // or latex either (org second-byte branch, no fallthrough).
        let src = "* t \\\\\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT)
        );
    }

    #[test]
    fn three_backslashes_then_newline_never_object() {
        let src = "a\\\\\\\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::ENTITY)
        );
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT)
        );
    }

    // --- scopes / MINIMAL --------------------------------------------------

    #[test]
    fn fragment_in_radio_link_desc_and_table_cell() {
        let src = "<<<r $a$>>>\n[[https://x.com][d \\foo]]\n| c \\(x\\) |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);

        let radio = nodes_of_kind(&root, OrgSyntaxKind::RADIO_TARGET);
        assert_eq!(radio.len(), 1);
        assert_eq!(
            nodes_of_kind(&radio[0], OrgSyntaxKind::LATEX_FRAGMENT).len(),
            1
        );
        assert_eq!(
            nodes_of_kind(&radio[0], OrgSyntaxKind::LATEX_FRAGMENT)[0]
                .text()
                .to_string(),
            "$a$"
        );

        let descs = nodes_of_kind(&root, OrgSyntaxKind::LINK_DESCRIPTION);
        assert_eq!(descs.len(), 1);
        assert_eq!(
            nodes_of_kind(&descs[0], OrgSyntaxKind::LATEX_FRAGMENT).len(),
            1
        );
        assert_eq!(
            nodes_of_kind(&descs[0], OrgSyntaxKind::LATEX_FRAGMENT)[0]
                .text()
                .to_string(),
            "\\foo"
        );

        let cells = nodes_of_kind(&root, OrgSyntaxKind::TABLE_CELL);
        let cell_frags: Vec<_> = cells
            .iter()
            .flat_map(|c| nodes_of_kind(c, OrgSyntaxKind::LATEX_FRAGMENT))
            .collect();
        assert_eq!(cell_frags.len(), 1, "one fragment in table cells");
        assert_eq!(cell_frags[0].text().to_string(), "\\(x\\)");
    }

    /// CAPTION value is a limited inline region (no trailing newline). Unclosed
    /// math under that cap stays plain text; a closed form still claims.
    #[test]
    fn unclosed_math_soft_fails_under_caption_cap() {
        let open = "#+CAPTION: before \\(unclosed\nA paragraph.\n";
        let root = parse_document_tree(open);
        assert_eq!(tree_text(&root), open);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "unclosed math under CAPTION must not claim"
        );

        let closed = "#+CAPTION: before \\(a\\) after\nA paragraph.\n";
        let root = parse_document_tree(closed);
        assert_eq!(tree_text(&root), closed);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].text().to_string(), "\\(a\\)");
    }

    #[test]
    fn fragment_in_heading_title() {
        // standard-set-no-line-break includes latex-fragment via MINIMAL.
        assert!(restrictions(OrgSyntaxKind::HEADING_TITLE).contains(OrgSyntaxKind::LATEX_FRAGMENT));
        let src = "* title $a$ and \\foo\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0].text().to_string(), "$a$");
        assert_eq!(frags[1].text().to_string(), "\\foo");
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LINE_BREAK)
        );
    }

    // --- Known gap: objects inside a bare script body ----------------------

    /// A bare script body can end mid-tile, and an object that would start
    /// inside the body and end in the trailing tile is lost. Org has an ENTITY
    /// inside the superscript; this parser stays lossless but emits no entity
    /// child.
    #[test]
    fn bare_script_mid_tile_entity_known_gap() {
        let src = "x^a\\alpha-b\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "tiling holds");
        let sups = nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT);
        assert_eq!(sups.len(), 1);
        assert_eq!(sups[0].text().to_string(), "^a\\alpha");
        // Known gap: no entity (or latex) child under the script.
        assert!(
            nodes_of_kind(&sups[0], OrgSyntaxKind::ENTITY).is_empty(),
            "known gap: mid-tile entity not reparsed inside bare script"
        );
        assert!(
            nodes_of_kind(&sups[0], OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "known gap: mid-tile latex not reparsed inside bare script"
        );
    }

    #[test]
    fn bare_script_mid_tile_latex_command_known_gap() {
        let src = "x^a\\foo-b\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src, "tiling holds");
        let sups = nodes_of_kind(&root, OrgSyntaxKind::SUPERSCRIPT);
        assert_eq!(sups.len(), 1);
        assert_eq!(sups[0].text().to_string(), "^a\\foo");
        assert!(
            nodes_of_kind(&sups[0], OrgSyntaxKind::LATEX_FRAGMENT).is_empty(),
            "known gap: latex command mid-tile lost inside bare script"
        );
    }

    // --- probe purity / emit -----------------------------------------------

    #[test]
    fn probe_pure_and_parse_uses_shape() {
        type ProbeFn = fn(&Parser<'_>) -> Option<LatexFragmentShape>;
        let cases: &[(&str, ProbeFn)] = &[
            ("\\(a\\)", probe_fragment_backslash),
            ("\\foo{arg}", probe_fragment_backslash),
            ("$a$", probe_fragment_dollar),
            ("$$x$$", probe_fragment_dollar),
        ];
        for &(src, probe) in cases {
            let p = make_parser(src);
            let before = p.cursor();
            let shape = probe(&p).unwrap_or_else(|| panic!("shape for {src:?}"));
            assert_eq!(p.cursor(), before, "probe must not advance for {src:?}");
            assert!(shape.len > 0);

            let mut p2 = make_parser(src);
            let shape = probe(&p2).expect("shape");
            parse_fragment(&mut p2, shape);
            let (_, evs, _) = p2.finish();
            assert_eq!(
                node_text(&evs, src, OrgSyntaxKind::LATEX_FRAGMENT)
                    .as_deref()
                    .map(str::len),
                Some(shape.len),
                "emit for {src:?}"
            );
        }
    }

    #[test]
    fn fragment_mid_tile_command_trail() {
        // `\foo2`: name ends before digit; trail stays for the parent loop.
        let shape = probe_fragment_backslash(&make_parser("\\foo2")).expect("cmd");
        assert_eq!(shape.len, "\\foo".len());
        let (_, evs, _) = run_bs("\\foo2");
        assert_eq!(
            node_text(&evs, "\\foo2", OrgSyntaxKind::LATEX_FRAGMENT).as_deref(),
            Some("\\foo")
        );
    }

    // =======================================================================
    // Environments
    // =======================================================================

    use crate::parser::grammar::drive::decide::interrupts_paragraph;
    use crate::tests::{diags_any, probe_line};

    #[test]
    fn env_opener_accepts_name_star_indent_and_rejects_empty() {
        assert!(probe_line("\\begin{equation}\n", opener).is_some());
        assert!(probe_line("\\begin{align*}\n", opener).is_some());
        assert!(probe_line("  \\begin{eq}\n", opener).is_some());
        assert!(probe_line("\\begin{equation} trailing\n", opener).is_some());
        assert!(probe_line("\\BEGIN{Eq}\n", opener).is_some()); // caseless begin
        // Empty name is silent (not a degrade reason).
        assert!(probe_line("\\begin{}\n", opener).is_none());
        // Not line-first begin shape.
        assert!(probe_line("prose \\begin{eq}\n", opener).is_none());
        assert!(probe_line("\\begin{bad-name}\n", opener).is_none());
        assert!(probe_line("\\begin{eq\n", opener).is_none()); // unclosed brace
        assert!(probe_line("\\end{eq}\n", opener).is_none());
    }

    #[test]
    fn env_opener_name_remeasures() {
        let p = make_parser("\\begin{align*}\n");
        let line = p.line().expect("line");
        assert_eq!(opener_name(line), "align*");
        assert_eq!(opener_name(make_parser("not\n").line().expect("l")), "");
        assert_eq!(
            opener_name(make_parser("\\begin{}\n").line().expect("l")),
            ""
        );
    }

    #[test]
    fn env_opener_carries_name() {
        let p = make_parser("\\begin{align*}\n");
        let line = p.line().expect("line");
        let op = opener(line).expect("opener");
        assert_eq!(op.name(line), "align*");
    }

    #[test]
    fn end_name_org_form_prefix_and_trailing_ws() {
        assert_eq!(
            end_name(make_parser("\\end{equation}\n").line().expect("l")),
            Some("equation")
        );
        assert_eq!(
            end_name(make_parser("text\\end{equation}\n").line().expect("l")),
            Some("equation")
        );
        assert_eq!(
            end_name(make_parser("  prefix \\end{eq}  \n").line().expect("l")),
            Some("eq")
        );
        assert_eq!(
            end_name(make_parser("\\end{align*}\n").line().expect("l")),
            Some("align*")
        );
        // Content after `}` (other than space/tab) rejects.
        assert!(end_name(make_parser("\\end{eq}x\n").line().expect("l")).is_none());
        assert!(end_name(make_parser("\\end{}\n").line().expect("l")).is_none());
        assert!(end_name(make_parser("\\begin{eq}\n").line().expect("l")).is_none());
        assert!(end_name_matches(
            make_parser("\\end{EQ}\n").line().expect("l"),
            "eq"
        ));
    }

    #[test]
    fn end_name_in_pure_helper() {
        assert_eq!(end_name_in("\\end{foo}"), Some("foo"));
        assert_eq!(end_name_in("x\\end{foo}\t "), Some("foo"));
        assert_eq!(end_name_in("\\end{FooBar*}"), Some("FooBar*"));
        // Caseless `end` keyword on the byte-path compare (not only the name).
        assert_eq!(end_name_in("\\END{foo}"), Some("foo"));
        assert_eq!(end_name_in("\\End{bar}"), Some("bar"));
        assert_eq!(end_name_in("\\eNd{Baz*}"), Some("Baz*"));
        assert!(end_name_in("\\end{}").is_none());
        assert!(end_name_in("\\end{foo}x").is_none());
        assert!(end_name_in("end{foo}").is_none());
        assert!(end_name_in("").is_none());
    }

    #[test]
    fn end_name_caseless_end_keyword_on_line() {
        assert_eq!(
            end_name(make_parser("\\END{eq}\n").line().expect("l")),
            Some("eq")
        );
        assert_eq!(
            end_name(make_parser("prefix \\End{eq}\t\n").line().expect("l")),
            Some("eq")
        );
        assert!(end_name_matches(
            make_parser("\\END{EQ}\n").line().expect("l"),
            "eq"
        ));
    }

    /// Parser path: N unclosed math openers in one paragraph charge
    /// `math_closer_work` O(source), not O(N * source), via the miss memo.
    #[test]
    fn math_closer_work_is_linear_for_unclosed() {
        for (label, chunk) in [("paren", "\\(x "), ("bracket", "\\[x ")] {
            let n = 128usize;
            let src = chunk.repeat(n);
            let mut p = make_parser(&src);
            p.math_closer_work.set(0);
            crate::parser::grammar::drive::document::parse_document(&mut p);
            let work = p.math_closer_work.get();
            let len = src.len();
            assert!(
                work > 0,
                "{label}: parser path must charge math_closer_work (got 0)"
            );
            // First real scan ~ remaining haystack; memo hits cost 0.
            assert!(
                work <= len.saturating_mul(3),
                "{label}: work={work} len={len} (expected O(len), not O(N*len))"
            );
            // Naive per-opener rescan lower bound grows ~N^2.
            let naive_lower = (n * (n + 1) / 2) * (chunk.len().saturating_sub(2)) / 4;
            assert!(
                work < naive_lower,
                "{label}: work={work} should be far below naive ~{naive_lower}"
            );
            let (_t, _e, diags) = p.finish();
            assert!(
                diags.is_empty(),
                "{label}: unclosed math stays silent plain text, diags={diags:?}"
            );
            // Closed forms still measure under the parser path.
            let closed = if label == "paren" {
                "\\(a\\)"
            } else {
                "\\[a\\]"
            };
            let root = parse_document_tree(closed);
            assert_eq!(tree_text(&root), closed);
            assert_eq!(
                nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT).len(),
                1,
                "{label}: closed math still forms a fragment"
            );
        }
    }

    /// Two paragraphs of unclosed math: each region re-adopts the miss memo so
    /// work stays O(source), not O(N^2) in the second paragraph alone.
    #[test]
    fn math_closer_work_linear_two_paragraphs() {
        for (label, chunk) in [("paren", "\\(x "), ("bracket", "\\[x ")] {
            let n = 128usize;
            let para = chunk.repeat(n);
            let src = format!("{para}\n\n{para}");
            let mut p = make_parser(&src);
            p.math_closer_work.set(0);
            crate::parser::grammar::drive::document::parse_document(&mut p);
            let work = p.math_closer_work.get();
            let len = src.len();
            assert!(
                work > 0,
                "{label}: two-paragraph path must charge math_closer_work"
            );
            // ~one full scan per paragraph (memo adopt on first miss of each).
            assert!(
                work <= len.saturating_mul(3),
                "{label}: work={work} len={len} (expected O(len) across regions)"
            );
            let naive_one_para = (n * (n + 1) / 2) * (chunk.len().saturating_sub(2)) / 4;
            assert!(
                work < naive_one_para,
                "{label}: work={work} should be far below one-paragraph naive ~{naive_one_para}"
            );
            let (_t, _e, diags) = p.finish();
            assert!(
                diags.is_empty(),
                "{label}: unclosed math stays silent plain text, diags={diags:?}"
            );
            assert_eq!(tree_text(&parse_document_tree(&src)), src);
        }
    }

    /// Multi-byte UTF-8 before a trailing `{name}` must not panic: the walker
    /// uses byte offsets that are not always char boundaries.
    #[test]
    fn end_name_in_non_ascii_prefix_no_panic() {
        // 2-byte, 3-byte, and 4-byte codepoints immediately before `{foo}`.
        assert!(end_name_in("\u{e9}ab{foo}").is_none());
        assert!(end_name_in("\u{20AC}ab{foo}").is_none()); // euro sign, 3 bytes
        assert!(end_name_in("\u{1F600}{foo}").is_none());
        // Real end form after multi-byte prefix still matches (incl. caseless).
        assert_eq!(end_name_in("\u{e9}\\end{foo}"), Some("foo"));
        assert_eq!(end_name_in("\u{20AC}\\END{foo}"), Some("foo"));
        assert_eq!(end_name_in("\u{1F600}\\End{bar}"), Some("bar"));
    }

    /// `brace_group_name` and `line_content_text` on begin, end and blank lines.
    #[test]
    fn brace_group_and_line_content_run_while_helpers() {
        // Closed group on a begin line.
        let p = make_parser("\\begin{align*}\n");
        let line = p.line().expect("line");
        assert_eq!(brace_group_name(line, 2), Some("align*"));
        assert_eq!(begin_name(line), Some("align*"));

        // Empty group stays empty (opener rejects via is_env_name).
        let p = make_parser("\\begin{}\n");
        let line = p.line().expect("line");
        assert_eq!(brace_group_name(line, 2), Some(""));
        assert!(begin_name(line).is_some_and(str::is_empty));
        assert!(opener(line).is_none());

        // Unclosed brace on the line: no group.
        let p = make_parser("\\begin{align\n");
        let line = p.line().expect("line");
        assert!(brace_group_name(line, 2).is_none());
        assert!(begin_name(line).is_none());

        // line_content_text drops leading INDENT and stops at NEWLINE.
        let p = make_parser("  text\\end{eq}\n");
        let line = p.line().expect("line");
        assert_eq!(line_content_text(line), Some("text\\end{eq}"));
        assert_eq!(end_name(line), Some("eq"));

        // Empty content (newline-only blank line): no content text.
        let p = make_parser("\n");
        let line = p.line().expect("line");
        assert!(line_content_text(line).is_none());
    }

    /// `FrameEndCache::build` calls `end_name` on every line, so any frame
    /// opener plus a later line ending in `{...}` after a multi-byte character
    /// exercises the byte walk, with no LaTeX content at all.
    #[test]
    fn frame_end_cache_build_multibyte_brace_line_no_panic() {
        let cases = [
            // Src block with a multi-byte character before `{foo}`.
            "#+begin_src\n\u{e9}ab{foo}\n#+end_src\n",
            // Drawer opener is enough to build FrameEndCache.
            ":LOGBOOK:\n\u{e9}ab{foo}\n:END:\n",
            // 4-byte character before a trailing brace group.
            "#+begin_src\n\u{1F600}{foo}\n#+end_src\n",
        ];
        for src in cases {
            let out = crate::parse(src);
            assert_eq!(
                out.root().text().to_string(),
                src,
                "tiling / round-trip for {src:?}"
            );
        }
    }

    #[test]
    fn closed_env_happy_path_star_caseless_indent() {
        let cases = [
            "\\begin{equation}\nx\n\\end{equation}\n",
            "\\begin{align*}\ny\n\\end{align*}\n",
            "\\begin{eq}\nz\n\\end{EQ}\n",
            "  \\begin{equation}\nw\n\\end{equation}\n",
        ];
        for src in cases {
            let root = parse_document_tree(src);
            assert_eq!(tree_text(&root), src, "tiling for {src:?}");
            let envs = nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT);
            assert_eq!(envs.len(), 1, "one env for {src:?}");
            // Full span equality: begin through end (no trailing post-blank here).
            assert_eq!(tree_text(&envs[0]), src, "full env extent for {src:?}");
        }
    }

    #[test]
    fn prefix_before_end_is_valid() {
        let src = "\\begin{equation}\nx\ntext\\end{equation}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT).len(),
            1
        );
        assert_eq!(
            tree_text(&nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT)[0]),
            src
        );
    }

    #[test]
    fn heading_between_begin_and_end_degrades() {
        use crate::DiagnosticKind;
        let src = "\\begin{equation}\nx\n* Heading\n\\end{equation}\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "no env across heading"
        );
        assert_eq!(
            nodes_of_kind(out.root(), OrgSyntaxKind::HEADING).len(),
            1,
            "heading stays outline"
        );
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(
                k,
                DiagnosticKind::UnterminatedLatexEnvironment { name }
                    if name == "equation"
            )
        }));
        // End after heading is ordinary section material (not paired).
        let after = nodes_of_kind(out.root(), OrgSyntaxKind::SECTION);
        assert!(
            after
                .iter()
                .any(|s| tree_text(s).contains("\\end{equation}")),
            "lone end after heading is section text"
        );
    }

    #[test]
    fn end_on_heading_degrades() {
        use crate::DiagnosticKind;
        let src = "\\begin{equation}\nx\n* Results \\end{equation}\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "{:?}",
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT)
        );
        assert_eq!(nodes_of_kind(out.root(), OrgSyntaxKind::HEADING).len(), 1);
        assert!(
            tree_text(&nodes_of_kind(out.root(), OrgSyntaxKind::HEADING)[0])
                .contains("\\end{equation}"),
            "end form sits on the heading line"
        );
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(
                k,
                DiagnosticKind::UnterminatedLatexEnvironment { name }
                    if name == "equation"
            )
        }));
    }

    #[test]
    fn unterminated_env_does_not_interrupt_prior_paragraph() {
        use crate::DiagnosticKind;
        let src = "text before\n\\begin{eq}\nstill same paragraph\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        assert!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "{:?}",
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT)
        );
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(paras.len(), 1, "single running paragraph");
        assert_eq!(tree_text(&paras[0]), src);

        let p = make_parser("\\begin{eq}\nno end\n");
        let line = p.line().expect("line");
        assert!(!interrupts_paragraph(line));
        let out = crate::parse("\\begin{eq}\nno end\n");
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(
                k,
                DiagnosticKind::UnterminatedLatexEnvironment { name } if name == "eq"
            )
        }));
    }

    #[test]
    fn closed_env_interrupts_paragraph() {
        let src = "text before\n\\begin{eq}\nx\n\\end{eq}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let paras = nodes_of_kind(&root, OrgSyntaxKind::PARAGRAPH);
        assert!(paras.iter().any(|p| tree_text(p) == "text before\n"));
        assert_eq!(
            nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT).len(),
            1
        );
        let p = make_parser("\\begin{eq}\nx\n\\end{eq}\n");
        let line = p.line().expect("line");
        assert!(interrupts_paragraph(line));
    }

    #[test]
    fn nested_list_opaque_frame_skips_env_body() {
        // List structure skips closed env interiors via opaque_frame_end.
        let src = "- item\n  \\begin{eq}\n  body\n  \\end{eq}\n- next\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let lists = nodes_of_kind(&root, OrgSyntaxKind::PLAIN_LIST);
        assert_eq!(lists.len(), 1);
        let items = nodes_of_kind(&lists[0], OrgSyntaxKind::ITEM);
        assert_eq!(items.len(), 2, "two items; env body not new items");
        assert_eq!(
            nodes_of_kind(&items[0], OrgSyntaxKind::LATEX_ENVIRONMENT).len(),
            1
        );
    }

    #[test]
    fn affiliation_attaches_above_closed_env() {
        let src = "#+CAPTION: fig\n#+ATTR_LATEX: :width 5cm\n\\begin{eq}\nx\n\\end{eq}\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let envs = nodes_of_kind(&root, OrgSyntaxKind::LATEX_ENVIRONMENT);
        assert_eq!(envs.len(), 1);
        let run = envs[0]
            .prev_sibling()
            .expect("affiliated run above the env");
        assert_eq!(run.kind(), OrgSyntaxKind::AFFILIATED_KEYWORDS);
        assert_eq!(
            run.children().count(),
            2,
            "CAPTION + ATTR_LATEX attach as run members"
        );
    }

    #[test]
    fn same_line_begin_end_is_not_environment() {
        // Known gap: the end lookup requires end_line > opener_line, so a
        // same-line `\begin{eq}...\end{eq}` never closes as LATEX_ENVIRONMENT.
        // Org-element can accept same-line forms; this parser treats the line
        // as an unterminated begin (degraded) plus fragment objects inside the
        // paragraph that absorbs it.
        use crate::DiagnosticKind;
        let src = "\\begin{eq} x \\end{eq}\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "same-line begin/end is not an environment"
        );
        assert!(diags_any(out.diagnostics(), |k| {
            matches!(
                k,
                DiagnosticKind::UnterminatedLatexEnvironment { name } if name == "eq"
            )
        }));
    }

    #[test]
    fn empty_begin_name_is_silent_paragraph() {
        let src = "\\begin{}\nx\n";
        let out = crate::parse(src);
        assert_eq!(out.root().text().to_string(), src);
        assert!(
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT).is_empty(),
            "{:?}",
            nodes_of_kind(out.root(), OrgSyntaxKind::LATEX_ENVIRONMENT)
        );
        assert!(
            out.diagnostics().is_empty(),
            "empty \\begin{{}} is not a degrade reason: {:?}",
            out.diagnostics()
        );
    }

    #[test]
    fn resolve_probe_env_pure() {
        let src = "\\begin{eq}\nbody\n\\end{eq}\n";
        assert!(probe_line(src, probe_env).is_some());
        assert!(probe_line("\\begin{eq}\nno end\n", probe_env).is_none());
        let p = make_parser("\\begin{eq}\nno end\n");
        let line = p.line().expect("line");
        let op = opener(line).expect("opener");
        assert!(matches!(
            resolve(line, op),
            Err(crate::parser::grammar::drive::decide::DegradeReason::UnterminatedLatexEnvironment)
        ));
    }

    // --- pure measure / miss memo ---

    use super::*;

    #[test]
    fn measure_command_name_letters_and_star() {
        assert_eq!(measure_command_name("foo"), Some(3));
        assert_eq!(measure_command_name("foo*"), Some(4));
        assert_eq!(measure_command_name("foo*[x]"), Some(4));
        assert_eq!(measure_command_name("begin{align}"), Some(5));
        assert_eq!(measure_command_name("Alpha"), Some(5));
        // Digits are not part of the name (entity may claim digit-bearing names).
        assert_eq!(measure_command_name("frac12"), Some(4));
        assert!(measure_command_name("").is_none());
        assert!(measure_command_name("1foo").is_none());
        assert!(measure_command_name("*foo").is_none());
        assert!(measure_command_name("(a)").is_none());
    }

    #[test]
    fn measure_bracket_and_brace_args() {
        assert_eq!(measure_bracket_arg("[opt]"), Some(5));
        assert_eq!(measure_bracket_arg("[a b]x"), Some(5));
        assert!(measure_bracket_arg("[a[b]]").is_none());
        assert!(measure_bracket_arg("[a{b}]").is_none());
        assert!(measure_bracket_arg("[a\nb]").is_none());
        assert!(measure_bracket_arg("[unclosed").is_none());
        assert!(measure_bracket_arg("{x}").is_none());

        assert_eq!(measure_brace_arg("{arg}"), Some(5));
        assert_eq!(measure_brace_arg("{a b}x"), Some(5));
        assert!(measure_brace_arg("{a{b}}").is_none());
        assert!(measure_brace_arg("{a\nb}").is_none());
        assert!(measure_brace_arg("{unclosed").is_none());
        assert!(measure_brace_arg("[x]").is_none());
    }

    #[test]
    fn measure_command_args_runs_and_stops() {
        assert_eq!(measure_command_args(""), 0);
        assert_eq!(measure_command_args(" tail"), 0);
        assert_eq!(measure_command_args("[opt]{arg}"), 10);
        assert_eq!(measure_command_args("[a][b]{c}"), 9);
        // Incomplete arg is not consumed; name-only length stays 0 here.
        assert_eq!(measure_command_args("[unclosed"), 0);
        assert_eq!(measure_command_args("{a\nb}"), 0);
        assert_eq!(measure_command_args("{a} rest"), 3);
    }

    #[test]
    fn measure_math_closer_table() {
        assert_eq!(measure_math_closer_pure("(a\\)", br"\)"), Some(4));
        assert_eq!(measure_math_closer_pure("[a\\]", br"\]"), Some(4));
        assert!(measure_math_closer_pure("(unclosed", br"\)").is_none());
        assert!(measure_math_closer_pure("[a", br"\]").is_none());
        // Wrong closer sequence does not match.
        assert!(measure_math_closer_pure("(a\\]", br"\)").is_none());
    }

    /// Unclosed math openers share a miss memo: N searches on one haystack
    /// cost O(haystack) scans, not O(N * haystack). Both closer kinds.
    #[test]
    fn math_closer_miss_memo_is_linear_in_opener_count() {
        for (kind, opener, closer_tail, chunk_str) in [
            (MathCloserKind::Paren, b'(', br"\)" as &[u8], "(x "),
            (MathCloserKind::Bracket, b'[', br"\]", "[x "),
        ] {
            let n = 256usize;
            let after_bs = chunk_str.repeat(n);
            let chunk = chunk_str.len();
            assert_eq!(after_bs.len(), n * chunk);
            assert_eq!(after_bs.as_bytes()[0], opener);

            let mut memo = MathCloserMissMemo::default();
            let mut work = 0usize;
            for i in 0..n {
                let start = i * chunk;
                let slice = &after_bs[start..];
                // Interior haystack (skip opener char), absolute = start + 1.
                let haystack = slice.as_bytes().get(1..).expect("opener char");
                let rel = find_math_closer_memo(
                    haystack,
                    closer_tail,
                    start + 1,
                    kind,
                    &mut memo,
                    &mut work,
                );
                assert!(rel.is_none(), "{kind:?} opener {i} must miss");
            }
            // First scan examines ~len; later openers hit the memo (O(1)).
            assert!(
                work <= after_bs.len().saturating_mul(2),
                "{kind:?}: work={work} len={} (expected one full scan, not N rescans)",
                after_bs.len()
            );
            // Quadratic lower bound for a naive per-opener rescan of the remainder.
            let naive_lower = (n * (n + 1) / 2) * (chunk - 1) / 2;
            assert!(
                work < naive_lower,
                "{kind:?}: work={work} should be far below naive ~{naive_lower}"
            );
        }
    }

    /// Pure memo policy edges (structure only). The parser-path hit-leaves-memo case
    /// is pinned below with `Some(parser)`.
    #[test]
    fn math_closer_miss_memo_policy_edges() {
        let mut memo = MathCloserMissMemo::default();

        // Miss on a wide window.
        memo.record_miss(MathCloserKind::Paren, 10, 100);
        assert!(memo.known_miss(MathCloserKind::Paren, 10, 100));
        assert!(memo.known_miss(MathCloserKind::Paren, 20, 80)); // nested smaller
        assert!(!memo.known_miss(MathCloserKind::Paren, 10, 101)); // wider end
        assert!(!memo.known_miss(MathCloserKind::Paren, 5, 100)); // earlier start

        // Nested miss is already covered: no-op, outer span kept.
        memo.record_miss(MathCloserKind::Paren, 20, 50);
        assert!(
            memo.known_miss(MathCloserKind::Paren, 10, 100),
            "outer span must survive nested miss"
        );

        // Later region (disjoint or partial past the prior end): adopt new.
        memo.record_miss(MathCloserKind::Paren, 100, 200);
        assert!(
            memo.known_miss(MathCloserKind::Paren, 100, 200),
            "new region after prior span is adopted"
        );
        assert!(
            !memo.known_miss(MathCloserKind::Paren, 10, 100),
            "prior region span is replaced once a later miss is recorded"
        );

        // Expand / replace when a new miss fully contains the old one.
        memo.record_miss(MathCloserKind::Paren, 0, 300);
        assert!(memo.known_miss(MathCloserKind::Paren, 0, 300));
        assert!(memo.known_miss(MathCloserKind::Paren, 100, 200));

        // Bracket remains independent of paren policy updates.
        assert!(!memo.known_miss(MathCloserKind::Bracket, 0, 300));
    }

    /// Parser path: a successful closer hit must not clear or rewrite the memo.
    #[test]
    fn math_closer_hit_leaves_memo() {
        use crate::tests::make_parser;

        let p = make_parser("(a\\)");
        // Seed a known-empty paren span that does **not** cover the closed
        // measure window (haystack ~[1,4)), so the hit path actually scans.
        let mut memo = p.math_closer_miss.get();
        memo.record_miss(MathCloserKind::Paren, 100, 200);
        p.math_closer_miss.set(memo);

        assert_eq!(
            measure_math_closer("(a\\)", br"\)", 0, MathCloserKind::Paren, Some(&p)),
            Some(4),
            "closed math still measures under the parser path"
        );
        assert!(
            p.math_closer_miss
                .get()
                .known_miss(MathCloserKind::Paren, 100, 200),
            "successful hit must leave prior miss memo intact"
        );
        // Bracket slot stays empty.
        assert!(
            !p.math_closer_miss
                .get()
                .known_miss(MathCloserKind::Bracket, 100, 200)
        );
    }

    /// Parser path: paren miss must not block a later bracket closer in the
    /// same visible region (kinds are independent on Parser memo).
    #[test]
    fn math_closer_kinds_independent_same_region() {
        use crate::syntax::OrgSyntaxKind;
        use crate::tests::{make_parser, nodes_of_kind, parse_document_tree, tree_text};

        // Unclosed `\(` then closed `\[...\]` in one paragraph.
        let src = "\\(x \\[a\\]";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let frags = nodes_of_kind(&root, OrgSyntaxKind::LATEX_FRAGMENT);
        assert_eq!(frags.len(), 1, "only the closed bracket form is a fragment");
        assert_eq!(tree_text(&frags[0]), "\\[a\\]");

        // Memo path: after a paren miss, bracket measure still finds its closer.
        let p = make_parser("\\(x \\[a\\]");
        let after_bs = p.rest_source().get(1..).expect("after bs");
        let abs = p.source_start_here().expect("start") + 1;
        assert!(
            measure_backslash_body(after_bs, abs, &p).is_none(),
            "unclosed paren is not a fragment"
        );
        // The cursor need not move: measure a bracket-shaped after_bs on the
        // same parser to check the memo.
        let br_after = "[a\\]";
        assert_eq!(
            measure_math_closer(br_after, br"\]", 0, MathCloserKind::Bracket, Some(&p)),
            Some(4),
            "bracket closer still found after paren miss on same Parser"
        );
    }

    #[test]
    fn measure_double_dollar_table() {
        assert_eq!(measure_double_dollar("$$x$$"), Some(5));
        assert_eq!(measure_double_dollar("$$$$"), Some(4));
        assert_eq!(measure_double_dollar("$$a\nb$$"), Some(7));
        assert!(measure_double_dollar("$$x").is_none());
        assert!(measure_double_dollar("$$").is_none());
        // Cursor must sit on the first `$` of the double form.
        let mut p = make_parser("a$$b$$c");
        p.eat_any();
        let shape = probe_fragment_dollar(&p).expect("double dollar");
        assert_eq!(shape.len, "$$b$$".len());
        assert_eq!(
            probe_fragment_dollar(&make_parser("$$x$$")).map(|s| s.len),
            Some("$$x$$".len())
        );
        assert!(probe_fragment_dollar(&make_parser("$$x")).is_none());
        assert!(probe_fragment_dollar(&make_parser("$$")).is_none());
    }

    /// One table for single-`$` boundaries. ASCII followers are
    /// [`DOLLAR_POST_SYNTAX`]; border and opener rejects sit in the same
    /// function.
    #[test]
    fn single_dollar_table() {
        assert_eq!(
            DOLLAR_POST_SYNTAX[b'_' as usize], b'_',
            "underscore is symbol class"
        );
        assert_eq!(
            DOLLAR_POST_SYNTAX[b'^' as usize], b'.',
            "caret is punctuation class"
        );

        for (b, &class) in DOLLAR_POST_SYNTAX.iter().enumerate() {
            // The table is one slot per ASCII byte, so the index fits in `u8`.
            let byte = u8::try_from(b).expect("DOLLAR_POST_SYNTAX is indexed by a byte");
            let accept = matches!(class, b'.' | b' ' | b'(' | b')' | b'"') || byte == b'\'';
            let ch = char::from(byte);
            assert_eq!(is_dollar_post(Some(ch)), accept, "post {byte:#x}");
            let src = format!("$a${ch}");
            let measured = measure_single_dollar(&src);
            let probed = probe_fragment_dollar(&make_parser(&src)).map(|s| s.len);
            if accept {
                assert_eq!(measured, Some(3), "{src:?}");
                assert_eq!(probed, Some(3), "{src:?}");
            } else {
                assert_eq!(measured, None, "{src:?}");
                assert_eq!(probed, None, "{src:?}");
            }
        }
        assert!(is_dollar_post(None), "end of region is a follower");
        assert_eq!(measure_single_dollar("$a$"), Some(3));
        assert_eq!(
            probe_fragment_dollar(&make_parser("$a$")).map(|s| s.len),
            Some(3)
        );

        // Not follower rows: interior border, forbidden opener, unclosed.
        let shapes: &[(&str, Option<usize>)] = &[
            ("$a b$", Some(5)),
            ("$a;$", Some(4)),
            ("$a$.x", Some(3)),
            ("$ a$", None),
            ("$\ta$", None),
            ("$;a$", None),
            ("$,a$", None),
            ("$.a$", None),
            ("$\ra$", None),
            ("$a,$", None),
            ("$a $", None),
            ("$a\t$", None),
            ("$a\n$", None),
            ("$a", None),
            ("$5 or $6", None),
        ];
        for &(src, want) in shapes {
            assert_eq!(measure_single_dollar(src), want, "{src:?}");
            assert_eq!(
                probe_fragment_dollar(&make_parser(src)).map(|s| s.len),
                want,
                "probe {src:?}"
            );
        }
    }
}
