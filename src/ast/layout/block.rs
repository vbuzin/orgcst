//! Block begin-line, closer, and token interior.

use super::{
    first_line, first_line_end, is_horizontal_ws, is_word_break, line_start_backing_indent,
    token_text_eq_caseless,
};
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode};
use crate::{TextRange, TextSize};

// ---------------------------------------------------------------------------
// Block first-line, closer, token interior
// ---------------------------------------------------------------------------

/// Which first-line slots a `*_BLOCK` kind may have.
///
/// Layout chooses the walk from this; the view maps `NotApplicable` from it.
/// Not [`crate::ast::BlockKind`]: that enum is the public view kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockHeader {
    Bare,
    Special,
    Dynamic,
    Export,
    Src,
}

impl BlockHeader {
    #[must_use]
    pub(crate) const fn has_name(self) -> bool {
        matches!(self, Self::Special | Self::Dynamic)
    }

    #[must_use]
    pub(crate) const fn has_language(self) -> bool {
        matches!(self, Self::Src | Self::Export)
    }

    #[must_use]
    pub(crate) const fn has_switches(self) -> bool {
        matches!(self, Self::Src)
    }

    #[must_use]
    pub(crate) const fn has_parameters(self) -> bool {
        matches!(self, Self::Src | Self::Export | Self::Dynamic)
    }
}

#[must_use]
pub(crate) fn block_header(kind: OrgSyntaxKind) -> BlockHeader {
    match kind {
        OrgSyntaxKind::SRC_BLOCK => BlockHeader::Src,
        OrgSyntaxKind::EXPORT_BLOCK => BlockHeader::Export,
        OrgSyntaxKind::SPECIAL_BLOCK => BlockHeader::Special,
        OrgSyntaxKind::DYNAMIC_BLOCK => BlockHeader::Dynamic,
        _ => BlockHeader::Bare,
    }
}

/// First-line fields of a `*_BLOCK` node. Token runs on [`first_line`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockFirstLine {
    pub begin_marker: TextRange,
    pub name: Option<TextRange>,
    pub language: Option<TextRange>,
    pub switches: Option<TextRange>,
    pub parameters: Option<TextRange>,
}

impl BlockFirstLine {
    /// After language, else after the begin marker.
    #[must_use]
    pub(crate) fn after_language(self) -> TextSize {
        self.language
            .map_or(self.begin_marker.end(), rowan::TextRange::end)
    }

    /// After the last present prefix before parameters (switches, language, or name).
    #[must_use]
    pub(crate) fn after_present_prefix(self) -> TextSize {
        self.switches
            .or(self.language)
            .or(self.name)
            .map_or(self.begin_marker.end(), rowan::TextRange::end)
    }
}

/// Closer of a `*_BLOCK`: `#+end` token run and the start of that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockCloser {
    pub end_marker: TextRange,
    pub line_start: TextSize,
}

/// First-line fields. Does not walk the closer or the body.
///
/// # Panics
///
/// Panics if the begin marker run is missing (`forms/block.rs::marker_shape`).
#[must_use]
pub(crate) fn block_first_line(node: &SyntaxNode) -> BlockFirstLine {
    let line: Vec<SyntaxElement> = first_line(node).collect();
    let header = block_header(node.kind());
    let mut i = 0usize;
    skip_indent(&line, &mut i);
    let begin_marker = take_begin_marker(&line, &mut i, header);
    let mut name = None;
    let mut language = None;
    let mut switches = None;
    let mut parameters = None;
    match header {
        BlockHeader::Special => {
            name = take_word(&line, &mut i);
        }
        BlockHeader::Dynamic => {
            skip_ws(&line, &mut i);
            name = take_word(&line, &mut i);
            parameters = rest_trimmed(&line, i);
        }
        BlockHeader::Src => {
            skip_ws(&line, &mut i);
            language = take_word(&line, &mut i);
            switches = take_switch_run(&line, &mut i);
            parameters = rest_trimmed(&line, i);
        }
        BlockHeader::Export => {
            skip_ws(&line, &mut i);
            language = take_word(&line, &mut i);
            parameters = rest_trimmed(&line, i);
        }
        BlockHeader::Bare => {}
    }
    BlockFirstLine {
        begin_marker,
        name,
        language,
        switches,
        parameters,
    }
}

/// Last `#+end` / `#+end_` / `#+end:` run, walking backward from the last
/// child. Skips decoy `#+end` in the body (no `_` / `:`). No Vec of the body.
///
/// # Panics
///
/// Panics if no closer exists (`forms/block.rs::resolve`).
#[must_use]
pub(crate) fn block_closer(node: &SyntaxNode) -> BlockCloser {
    let mut cur = node.last_child_or_token();
    while let Some(e) = cur {
        if e.kind() == OrgSyntaxKind::HASH
            && let Some(found) = end_marker_from_hash(&e)
        {
            return found;
        }
        cur = e.prev_sibling_or_token();
    }
    panic!("grammar: *_BLOCK has an end line (forms/block.rs::resolve)");
}

/// After the begin line, before the closer line. Does not parse switches.
#[must_use]
pub(crate) fn block_token_interior(node: &SyntaxNode) -> TextRange {
    let start = first_line_end(node);
    let closer_start = block_closer(node).line_start;
    if closer_start < start {
        TextRange::empty(start)
    } else {
        TextRange::new(start, closer_start)
    }
}

fn take_begin_marker(line: &[SyntaxElement], i: &mut usize, header: BlockHeader) -> TextRange {
    let start = must_elem(
        line,
        *i,
        "grammar: block begin line has tokens (forms/block.rs::marker_shape)",
    )
    .text_range()
    .start();
    expect_kind(
        line,
        i,
        OrgSyntaxKind::HASH,
        "grammar: #+ begin (forms/block.rs::marker_shape)",
    );
    expect_kind(
        line,
        i,
        OrgSyntaxKind::PLUS,
        "grammar: #+ begin (forms/block.rs::marker_shape)",
    );
    expect_text_caseless(
        line,
        i,
        "begin",
        "grammar: #+begin (forms/block.rs::marker_shape)",
    );
    match header {
        BlockHeader::Dynamic => {
            expect_kind(
                line,
                i,
                OrgSyntaxKind::COLON,
                "grammar: #+begin: (forms/block.rs::marker_shape)",
            );
            TextRange::new(start, line[*i - 1].text_range().end())
        }
        BlockHeader::Special => {
            expect_kind(
                line,
                i,
                OrgSyntaxKind::UNDERSCORE,
                "grammar: #+begin_ (forms/block.rs::marker_shape)",
            );
            TextRange::new(start, line[*i - 1].text_range().end())
        }
        _ => {
            expect_kind(
                line,
                i,
                OrgSyntaxKind::UNDERSCORE,
                "grammar: #+begin_ (forms/block.rs::marker_shape)",
            );
            let name = take_word(line, i)
                .expect("grammar: underscore block has a type name (forms/block.rs::name_range)");
            TextRange::new(start, name.end())
        }
    }
}

fn end_marker_from_hash(hash: &SyntaxElement) -> Option<BlockCloser> {
    let plus = hash.next_sibling_or_token()?;
    if plus.kind() != OrgSyntaxKind::PLUS {
        return None;
    }
    let word = plus.next_sibling_or_token()?;
    if !token_text_eq_caseless(&word, "end") {
        return None;
    }
    let after = word.next_sibling_or_token()?;
    let marker_end = match after.kind() {
        OrgSyntaxKind::COLON => after.text_range().end(),
        OrgSyntaxKind::UNDERSCORE => {
            let mut last = after.text_range().end();
            let mut n = after.next_sibling_or_token();
            while let Some(e) = n {
                if is_word_break(&e) {
                    break;
                }
                last = e.text_range().end();
                n = e.next_sibling_or_token();
            }
            last
        }
        _ => return None,
    };
    Some(BlockCloser {
        end_marker: TextRange::new(hash.text_range().start(), marker_end),
        line_start: line_start_backing_indent(hash),
    })
}

fn take_switch_run(line: &[SyntaxElement], i: &mut usize) -> Option<TextRange> {
    let mut start = None;
    let mut end = None;
    loop {
        let mut probe = *i;
        skip_ws(line, &mut probe);
        let switch_at = probe;
        if !take_one_switch(line, &mut probe) {
            break;
        }
        start.get_or_insert(line[switch_at].text_range().start());
        end = Some(line[probe - 1].text_range().end());
        *i = probe;
    }
    Some(TextRange::new(start?, end?))
}

/// One src switch at `i` (no leading whitespace). org-syntax 4.3.1 / org-element
/// `org-element-src-block-parser`: `-l "FORMAT"`, `+n NUMBER`, `-n NUMBER`,
/// `-r`, `-i`, `-k`. NUMBER is optional digits with optional horizontal ws.
fn take_one_switch(line: &[SyntaxElement], i: &mut usize) -> bool {
    let Some(t) = line.get(*i).and_then(SyntaxElement::as_token) else {
        return false;
    };
    match t.kind() {
        OrgSyntaxKind::TEXT => {
            if is_short_switch(t.text()) {
                *i += 1;
                return true;
            }
            if t.text().strip_prefix('-').is_some_and(is_n_arg) {
                *i += 1;
                take_optional_switch_number(line, i);
                return true;
            }
            if t.text().eq_ignore_ascii_case("-l") {
                let saved = *i;
                *i += 1;
                skip_ws(line, i);
                if take_quoted_switch_arg(line, i) {
                    return true;
                }
                *i = saved;
            }
            false
        }
        OrgSyntaxKind::PLUS => {
            if let Some(next) = line.get(*i + 1).and_then(SyntaxElement::as_token)
                && next.kind() == OrgSyntaxKind::TEXT
                && is_n_arg(next.text())
            {
                *i += 2;
                take_optional_switch_number(line, i);
                return true;
            }
            false
        }
        _ => false,
    }
}

fn is_short_switch(s: &str) -> bool {
    s.eq_ignore_ascii_case("-i") || s.eq_ignore_ascii_case("-k") || s.eq_ignore_ascii_case("-r")
}

fn is_n_arg(s: &str) -> bool {
    s.strip_prefix(['n', 'N'])
        .is_some_and(|rest| rest.is_empty() || rest.bytes().all(|b| b.is_ascii_digit()))
}

fn take_optional_switch_number(line: &[SyntaxElement], i: &mut usize) {
    let saved = *i;
    skip_ws(line, i);
    if let Some(t) = line.get(*i).and_then(SyntaxElement::as_token)
        && t.kind() == OrgSyntaxKind::TEXT
        && !t.text().is_empty()
        && t.text().bytes().all(|b| b.is_ascii_digit())
    {
        *i += 1;
        return;
    }
    *i = saved;
}

fn take_quoted_switch_arg(line: &[SyntaxElement], i: &mut usize) -> bool {
    let Some(t) = line.get(*i).and_then(SyntaxElement::as_token) else {
        return false;
    };
    if t.kind() != OrgSyntaxKind::TEXT || !t.text().starts_with('"') {
        return false;
    }
    let start = *i;
    *i += 1;
    if t.text().len() >= 2 && t.text().ends_with('"') {
        return true;
    }
    while *i < line.len() && line[*i].kind() != OrgSyntaxKind::NEWLINE {
        let Some(tok) = line[*i].as_token() else {
            *i = start;
            return false;
        };
        *i += 1;
        if tok.text().ends_with('"') {
            return true;
        }
    }
    *i = start;
    false
}

fn take_word(line: &[SyntaxElement], i: &mut usize) -> Option<TextRange> {
    if *i >= line.len() || is_word_break(&line[*i]) {
        return None;
    }
    let start = line[*i].text_range().start();
    let mut end = line[*i].text_range().end();
    *i += 1;
    while *i < line.len() && !is_word_break(&line[*i]) {
        end = line[*i].text_range().end();
        *i += 1;
    }
    Some(TextRange::new(start, end))
}

fn rest_trimmed(line: &[SyntaxElement], mut i: usize) -> Option<TextRange> {
    skip_ws(line, &mut i);
    let mut end = line.len();
    if end > i && line[end - 1].kind() == OrgSyntaxKind::NEWLINE {
        end -= 1;
    }
    while end > i && is_horizontal_ws(&line[end - 1]) {
        end -= 1;
    }
    if i >= end {
        return None;
    }
    Some(TextRange::new(
        line[i].text_range().start(),
        line[end - 1].text_range().end(),
    ))
}

fn skip_indent(line: &[SyntaxElement], i: &mut usize) {
    if line
        .get(*i)
        .is_some_and(|e| e.kind() == OrgSyntaxKind::INDENT)
    {
        *i += 1;
    }
}

fn skip_ws(line: &[SyntaxElement], i: &mut usize) {
    while line.get(*i).is_some_and(is_horizontal_ws) {
        *i += 1;
    }
}

fn expect_kind(line: &[SyntaxElement], i: &mut usize, kind: OrgSyntaxKind, why: &str) {
    assert_eq!(must_elem(line, *i, why).kind(), kind, "{why}");
    *i += 1;
}

fn expect_text_caseless(line: &[SyntaxElement], i: &mut usize, want: &str, why: &str) {
    let e = must_elem(line, *i, why);
    assert!(token_text_eq_caseless(e, want), "{why}");
    *i += 1;
}

fn must_elem<'a>(line: &'a [SyntaxElement], i: usize, why: &str) -> &'a SyntaxElement {
    line.get(i).unwrap_or_else(|| panic!("{why}"))
}
