//! In-buffer settings: the file config layer, read off the CST and resolved
//! over the caller's config.
//!
//! A setting that "carves" changes how the tree is built (today only the TODO
//! vocabulary); the others (`#+PRIORITIES:`, `#+TAGS:`, `#+FILETAGS:`,
//! `#+PROPERTY:`, ...) are read off the tree and never enter `ParseConfig`.
//!
//! **Settings are read from the parsed tree.** There is no separate line
//! scanner: carving settings are the `SETTING_KEYWORD` nodes the grammar
//! emitted, and the other directives are live `KEYWORD` nodes. Inert
//! directives fall out of tree shape: one inside a lesser block is body text
//! and never became a node, one inside a greater block did. Nothing here
//! re-implements block fences.
//!
//! **Collection does not depend on the config.** Which `SETTING_KEYWORD` nodes
//! exist, and what their values say, is the same under any config: the only
//! in-buffer knob is the TODO vocabulary, and the grammar reads it only inside
//! an already-delimited headline. So a producer may collect from any tree over
//! the text, including a reused one.
//!
//! Each directive family has one hand-written value scan here, shared by the
//! carving readers ([`collect_file_layer`], [`resolve`]) and by the views that
//! read the other directives.

use crate::parser::grammar::forms::keyword::is_carving_key;
use crate::parser::{ParseConfig, PriorityAtom, PriorityRange, TodoInterpretation, TodoSequence};
use crate::syntax::OrgSyntaxKind;
use crate::syntax::SyntaxNode;
use crate::{TextRange, TextSize};
use std::sync::Arc;

/// Settings this document declares that are **parse inputs**.
///
/// That is the TODO vocabulary and nothing else: `ParseConfig` holds only what
/// shapes the CST, so a directive that does not carve never reaches
/// [`resolve`].
///
/// Empty means the file declared no vocabulary and the caller layer stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileLayer {
    /// Contributing TODO directive lines, in document order, one sequence
    /// each. Non-contributing lines are absent rather than empty, so
    /// `is_empty` answers "does the file define a vocabulary".
    pub(crate) todo: Vec<TodoSequence>,
}

impl FileLayer {
    /// True when the document declared nothing that resolve could layer.
    pub(crate) fn is_empty(&self) -> bool {
        self.todo.is_empty()
    }
}

/// One live setting directive, with ranges so projections can name a span.
///
/// Liveness is tree shape: a directive inside a lesser block is body text
/// and never became a `SETTING_KEYWORD`. An empty value is a live line, not
/// an absent one.
#[derive(Debug, Clone)]
pub(crate) struct SettingLine {
    pub key: String,
    pub value: String,
    pub key_range: TextRange,
    pub value_range: Option<TextRange>,
}

fn setting_lines(root: &SyntaxNode) -> impl Iterator<Item = SettingLine> + '_ {
    live_keyword_nodes(root).filter_map(|node| {
        let key_node = child_node(&node, OrgSyntaxKind::KEYWORD_KEY)?;
        if !key_is_carving(&key_node) {
            return None;
        }
        Some(setting_line(&node, &key_node))
    })
}

/// Live `KEYWORD` and `SETTING_KEYWORD` lines. `#+TAGS:` / `#+FILETAGS:` /
/// `#+PROPERTY:` are plain `KEYWORD` nodes (they do not carve); the TODO
/// family and `#+PRIORITIES:` are `SETTING_KEYWORD`.
fn keyword_lines(root: &SyntaxNode) -> impl Iterator<Item = SettingLine> + '_ {
    live_keyword_nodes(root).filter_map(|node| {
        let key_node = child_node(&node, OrgSyntaxKind::KEYWORD_KEY)?;
        Some(setting_line(&node, &key_node))
    })
}

fn live_keyword_nodes(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + '_ {
    root.descendants().filter(|node| {
        matches!(
            node.kind(),
            OrgSyntaxKind::SETTING_KEYWORD | OrgSyntaxKind::KEYWORD
        )
    })
}

fn setting_line(node: &SyntaxNode, key_node: &SyntaxNode) -> SettingLine {
    let value_node = child_node(node, OrgSyntaxKind::KEYWORD_VALUE);
    SettingLine {
        key: key_node.text().to_string(),
        value: value_node
            .as_ref()
            .map(|n| n.text().to_string())
            .unwrap_or_default(),
        key_range: key_node.text_range(),
        value_range: value_node.map(|n| n.text_range()),
    }
}

/// The lexer splits at `_`, so `SEQ_TODO` is three tokens. A single-token key
/// is classified without allocating.
fn key_is_carving(key_node: &SyntaxNode) -> bool {
    with_key_text(key_node, is_carving_key)
}

fn with_key_text<R>(key_node: &SyntaxNode, f: impl FnOnce(&str) -> R) -> R {
    let mut tokens = key_node
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token);
    match (tokens.next(), tokens.next()) {
        (Some(only), None) => f(only.text()),
        (Some(first), Some(second)) => {
            let mut s = String::from(first.text());
            s.push_str(second.text());
            for t in tokens {
                s.push_str(t.text());
            }
            f(&s)
        }
        (None, _) => f(""),
    }
}

/// Every live setting directive, in document order, as `(key, value)`.
///
/// Built on [`setting_lines`] so resolve and the priority-range reader share
/// one walk.
fn setting_directives(root: &SyntaxNode) -> impl Iterator<Item = (String, String)> + '_ {
    setting_lines(root).map(|line| (line.key, line.value))
}

fn child_node(node: &SyntaxNode, kind: OrgSyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|child| child.kind() == kind)
}

/// Walk a parsed tree and read the settings that carve.
///
/// Document order: TODO lines accumulate as parallel sequences (manual 5.2.5).
/// Placement is not restricted to the head of the file or to column zero: any
/// live node counts, and a directive after the first headline still applies
/// buffer-wide.
pub(crate) fn collect_file_layer(root: &SyntaxNode) -> FileLayer {
    let mut layer = FileLayer::default();
    for (key, value) in setting_directives(root) {
        if let Some(interpretation) = todo_interpretation(&key)
            && let Some(sequence) = parse_todo_value(&value, interpretation)
        {
            layer.todo.push(sequence);
        }
    }
    layer
}

/// Layer the file's settings over the caller's.
///
/// Only the TODO vocabulary is layered: it is the only parse input a document
/// can declare.
///
/// The TODO vocabulary **replaces** rather than merges: when a file declares
/// any contributing sequence, its sequences alone define the vocabulary, which
/// is what `org-set-regexps-and-options` does. A file that declares none keeps
/// the caller's, so junk directives cannot wipe a working vocabulary.
///
/// A file's own vocabulary always applies, so re-feeding an effective config
/// as the caller's behaves like any other caller config.
///
/// A document that declares nothing gets the caller's own allocation back.
/// Most files declare no settings, and this turns the producer's "did
/// anything resolve" test into a pointer comparison.
pub(crate) fn resolve(caller: &Arc<ParseConfig>, file: &FileLayer) -> Arc<ParseConfig> {
    if file.is_empty() {
        return Arc::clone(caller);
    }
    Arc::new(
        (**caller)
            .clone()
            .with_todo_sequences(file.todo.iter().cloned()),
    )
}

/// Interpretation for a TODO-family key, or `None` for any other key.
///
/// Caseless: `#+seq_todo:` is a setting keyword like `#+SEQ_TODO:`.
fn todo_interpretation(key: &str) -> Option<TodoInterpretation> {
    if key.eq_ignore_ascii_case("todo") || key.eq_ignore_ascii_case("seq_todo") {
        Some(TodoInterpretation::Sequence)
    } else if key.eq_ignore_ascii_case("typ_todo") {
        Some(TodoInterpretation::Type)
    } else {
        None
    }
}

/// One `#+TODO:`-family value as a sequence, or `None` when it names nothing.
///
/// Whitespace-separated tokens. `|` splits active from done; without one, the
/// last name is the done state (so a lone `#+TODO: WAITING` declares a
/// vocabulary with no active keyword, exactly as Org's `(last names)` does).
/// Only the first bar splits; later ones are dropped like any other non-name.
///
/// A line that names nothing after stripping (`#+TODO:`, `#+TODO: |`,
/// `#+TODO: ()`) is **not contributing**: returning `None` here is what keeps
/// it from replacing the caller's vocabulary with an empty one.
fn parse_todo_value(value: &str, interpretation: TodoInterpretation) -> Option<TodoSequence> {
    let mut names: Vec<String> = Vec::new();
    let mut done_from: Option<usize> = None;

    for token in value.split_ascii_whitespace() {
        if token == "|" {
            done_from.get_or_insert(names.len());
            continue;
        }
        let name = strip_fast_access(token);
        if !name.is_empty() {
            names.push(name.to_owned());
        }
    }

    if names.is_empty() {
        return None;
    }
    let done_from = done_from.unwrap_or(names.len() - 1);
    Some(TodoSequence::new(interpretation, names, done_from))
}

/// Drop a fast-access key from a keyword token: `TODO(t)` -> `TODO`.
///
/// Mirrors `org-remove-keyword-keys`, which strips the first `(...)` pair and
/// keeps what precedes it. A `(` with no `)` after it is ordinary name
/// material, not a malformed key, so `TODO(t` stays whole. Log-note markers
/// (`@`, `!`, `/`) live inside the parens and never reach the name.
fn strip_fast_access(token: &str) -> &str {
    match token.find('(') {
        Some(open) if token[open + 1..].contains(')') => &token[..open],
        _ => token,
    }
}

// ---------------------------------------------------------------------------
// Value grammars for directives that do not carve (never feed resolve)
// ---------------------------------------------------------------------------

/// A `#+PRIORITIES:` value as a range, or `None` when the first three tokens
/// are not atoms.
///
/// Whitespace split matches Emacs `split-string` with no arguments
/// ([`split_priority_whitespace`]). Three or more tokens use the first three
/// and ignore the tail, even when a later token is not an atom. Fewer than
/// three is `None`: Org's A/C/B default is left to the caller.
///
/// Each atom is an all-digit token in `0`..=[`PriorityAtom::MAX_NUMBER`]
/// (leading zeros insignificant) or one ASCII uppercase letter. This is not
/// the cookie regexp and not `org-priority-to-value`'s unanchored scan: a
/// digit token is never a letter, and `A1` / `1A` / `a` / `AA` are `None`.
/// The three atoms are stored as written; checking that they are coherent is
/// left to the reader.
///
/// This is the value scan alone. [`declared_priority_range`] finds the line
/// in a tree.
pub(crate) fn parse_priorities_value(value: &str) -> Option<PriorityRange> {
    let mut tokens = split_priority_whitespace(value);
    let highest = priority_atom(tokens.next()?)?;
    let lowest = priority_atom(tokens.next()?)?;
    let default = priority_atom(tokens.next()?)?;
    Some(PriorityRange {
        highest,
        lowest,
        default,
    })
}

/// Emacs `split-string` with no arguments: `[ \f\t\n\r\v]+`, dropping empties.
///
/// Vertical tab is the byte `split_ascii_whitespace` does not treat as
/// whitespace, which is why this is not that function.
fn split_priority_whitespace(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = value;
    std::iter::from_fn(move || {
        let bytes = rest.as_bytes();
        let mut start = 0;
        while start < bytes.len() && is_emacs_split_ws(bytes[start]) {
            start += 1;
        }
        if start == bytes.len() {
            rest = "";
            return None;
        }
        let mut end = start + 1;
        while end < bytes.len() && !is_emacs_split_ws(bytes[end]) {
            end += 1;
        }
        let token = &rest[start..end];
        rest = &rest[end..];
        Some(token)
    })
}

/// Space, form feed, tab, LF, CR, vertical tab. The Emacs `split-string` set.
fn is_emacs_split_ws(b: u8) -> bool {
    matches!(b, b' ' | 0x0c | b'\t' | b'\n' | b'\r' | 0x0b)
}

/// One priority atom: all-digit tokens are numbers, single letters are letters.
fn priority_atom(token: &str) -> Option<PriorityAtom> {
    if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) {
        // Out-of-band numbers (and anything that overflows) are not priorities.
        // A digit token is never re-read as a letter.
        let value: u32 = token.parse().ok()?;
        return u8::try_from(value)
            .ok()
            .filter(|n| *n <= PriorityAtom::MAX_NUMBER)
            .map(PriorityAtom::Number);
    }
    let mut chars = token.chars();
    let first = chars.next()?;
    (chars.next().is_none() && first.is_ascii_uppercase()).then_some(PriorityAtom::Letter(first))
}

/// The priority range this document declares, if any.
///
/// First live `#+PRIORITIES:` in document order owns the slot: later lines are
/// ignored even when the first does not parse (Org collects the line, then
/// reads it). Returns `None` when the file declares no line, or the first
/// line's first three tokens are not atoms. A tail after those three does
/// not reject the line, and a later line does not rescue a bad first one.
///
/// A pure tree read: it never layers into [`ParseConfig`]. The public entry is
/// [`crate::Document::priority_range`].
pub(crate) fn declared_priority_range(root: &SyntaxNode) -> Option<PriorityRange> {
    for (key, value) in setting_directives(root) {
        if key.eq_ignore_ascii_case("priorities") {
            return parse_priorities_value(&value);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// #+TAGS: / #+FILETAGS: / #+PROPERTY: (read off the tree; never feed resolve)
// ---------------------------------------------------------------------------

/// A tag name with its source range and optional fast key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawTag {
    pub name: String,
    pub range: TextRange,
    pub fast_key: Option<char>,
}

/// One `#+TAGS:` group: exclusive `{ a b }` or group-tag `[name : children]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawTagGroup {
    pub exclusive: bool,
    pub name: Option<RawTag>,
    pub members: Vec<RawTag>,
}

/// A `#+PROPERTY:` line after the key/value split, before merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawProperty {
    pub key: String,
    pub value: String,
    pub range: TextRange,
}

/// Every live `#+TAGS:` line, or `None` when the file declares none.
///
/// Returns groups and free tags. Duplicate free tags collapse, and
/// `[work : children]` is a group tag with its members.
pub(crate) fn parse_tags_from_settings(
    root: &SyntaxNode,
) -> Option<(Vec<RawTagGroup>, Vec<RawTag>)> {
    let mut groups = Vec::new();
    let mut free = Vec::new();
    let mut seen_free: Vec<String> = Vec::new();
    let mut any = false;
    for line in keyword_lines(root) {
        if !line.key.eq_ignore_ascii_case("tags") {
            continue;
        }
        any = true;
        let base = line
            .value_range
            .map_or(line.key_range.end(), rowan::TextRange::start);
        parse_tags_value(&line.value, base, &mut groups, &mut free, &mut seen_free);
    }
    any.then_some((groups, free))
}

fn parse_tags_value(
    value: &str,
    base: TextSize,
    groups: &mut Vec<RawTagGroup>,
    free: &mut Vec<RawTag>,
    seen_free: &mut Vec<String>,
) {
    let mut mode = TagsMode::Free;
    let mut group_name: Option<RawTag> = None;
    let mut members: Vec<RawTag> = Vec::new();
    let mut exclusive = true;

    for (rel, token) in ws_tokens(value) {
        for (tok_rel, token) in expand_group_punct(token, rel) {
            match token {
                "{" => {
                    flush_group(groups, &mut mode, &mut group_name, &mut members, exclusive);
                    mode = TagsMode::Exclusive;
                    exclusive = true;
                }
                "}" | "]" => {
                    flush_group(groups, &mut mode, &mut group_name, &mut members, exclusive);
                    mode = TagsMode::Free;
                }
                "[" => {
                    flush_group(groups, &mut mode, &mut group_name, &mut members, exclusive);
                    mode = TagsMode::GroupTagName;
                    exclusive = false;
                }
                ":" if matches!(mode, TagsMode::GroupTagName | TagsMode::GroupTagMembers) => {
                    mode = TagsMode::GroupTagMembers;
                }
                _ => {
                    let Some(tag) = parse_tag_token(token, base, tok_rel) else {
                        continue;
                    };
                    match mode {
                        TagsMode::Free => {
                            if !seen_free.iter().any(|n| n == &tag.name) {
                                seen_free.push(tag.name.clone());
                                free.push(tag);
                            }
                        }
                        TagsMode::Exclusive | TagsMode::GroupTagMembers => members.push(tag),
                        TagsMode::GroupTagName => {
                            if group_name.is_none() {
                                group_name = Some(tag);
                            } else {
                                members.push(tag);
                            }
                        }
                    }
                }
            }
        }
    }
    flush_group(groups, &mut mode, &mut group_name, &mut members, exclusive);
}

/// Split glued group punctuation so `[work` and `home]` still parse.
fn expand_group_punct(token: &str, rel: usize) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut t = token;
    let mut off = rel;
    while let Some(first) = t.chars().next() {
        if matches!(first, '{' | '[' | ']' | '}') && t.len() > 1 {
            let n = first.len_utf8();
            out.push((off, &t[..n]));
            t = &t[n..];
            off += n;
            continue;
        }
        if t.len() > 1
            && let Some(last) = t.chars().last()
            && matches!(last, '{' | '[' | ']' | '}')
        {
            let n = last.len_utf8();
            let split = t.len() - n;
            if !t[..split].is_empty() {
                out.push((off, &t[..split]));
            }
            out.push((off + split, &t[split..]));
            return out;
        }
        out.push((off, t));
        break;
    }
    out
}

#[derive(Clone, Copy)]
enum TagsMode {
    Free,
    Exclusive,
    GroupTagName,
    GroupTagMembers,
}

fn flush_group(
    groups: &mut Vec<RawTagGroup>,
    mode: &mut TagsMode,
    name: &mut Option<RawTag>,
    members: &mut Vec<RawTag>,
    exclusive: bool,
) {
    if matches!(
        *mode,
        TagsMode::Exclusive | TagsMode::GroupTagName | TagsMode::GroupTagMembers
    ) && (name.is_some() || !members.is_empty())
    {
        groups.push(RawTagGroup {
            exclusive,
            name: name.take(),
            members: std::mem::take(members),
        });
    } else {
        name.take();
        members.clear();
    }
    *mode = TagsMode::Free;
}

fn parse_tag_token(token: &str, base: TextSize, rel: usize) -> Option<RawTag> {
    let (name, fast_key, name_len) = split_fast_key(token)?;
    if name.is_empty() {
        return None;
    }
    let start = base + TextSize::from(rel as u32);
    Some(RawTag {
        name: name.to_string(),
        range: TextRange::at(start, TextSize::from(name_len as u32)),
        fast_key,
    })
}

/// `tag` or `tag(k)` -> (name, key, name byte length).
fn split_fast_key(token: &str) -> Option<(&str, Option<char>, usize)> {
    if let Some(open) = token.find('(')
        && token.ends_with(')')
        && open + 3 == token.len()
    {
        let key = token[open + 1..open + 2].chars().next()?;
        return Some((&token[..open], Some(key), open));
    }
    Some((token, None, token.len()))
}

/// Every live `#+FILETAGS:` line, both `:a:b:` and `a b` spellings.
pub(crate) fn parse_filetags_from_settings(root: &SyntaxNode) -> Vec<RawTag> {
    let mut out = Vec::new();
    for line in keyword_lines(root) {
        if !line.key.eq_ignore_ascii_case("filetags") {
            continue;
        }
        let base = line
            .value_range
            .map_or(line.key_range.end(), rowan::TextRange::start);
        out.extend(parse_filetags_value(&line.value, base));
    }
    out
}

fn parse_filetags_value(value: &str, base: TextSize) -> Vec<RawTag> {
    let mut out = Vec::new();
    for (rel, token) in ws_tokens(value) {
        for (part_rel, part) in colon_parts(token, rel) {
            if part.is_empty() {
                continue;
            }
            let start = base + TextSize::from(part_rel as u32);
            out.push(RawTag {
                name: part.to_string(),
                range: TextRange::at(start, TextSize::from(part.len() as u32)),
                fast_key: None,
            });
        }
    }
    out
}

/// Merged `#+PROPERTY:` lines (caseless key; `+` appends; later key replaces).
pub(crate) fn file_keyword_properties(root: &SyntaxNode) -> Vec<RawProperty> {
    merge_properties(keyword_lines(root).filter_map(|line| {
        if !line.key.eq_ignore_ascii_case("property") {
            return None;
        }
        let base = line
            .value_range
            .map_or(line.key_range.end(), rowan::TextRange::start);
        parse_property_value(&line.value, base)
    }))
}

/// Caseless compare of a written key to a query.
///
/// Strips a trailing `+` from `written` only (`:FOO+:` matches query
/// `"FOO"`). Does **not** strip `query`. Query `"FOO+"` matches nothing
/// unless some line's base key is literally `FOO+`.
#[must_use]
pub(crate) fn property_key_eq(written: &str, query: &str) -> bool {
    written
        .strip_suffix('+')
        .unwrap_or(written)
        .eq_ignore_ascii_case(query)
}

/// Invert of `property_key_eq(written, "header-args:{language}")` without
/// allocating a lookup key. Bare `header-args` always matches; a language
/// suffix matches only when `language` is present.
#[must_use]
pub(crate) fn header_args_key_matches(written: &str, language: Option<&str>) -> bool {
    if property_key_eq(written, "header-args") {
        return true;
    }
    let Some(language) = language else {
        return false;
    };
    let base = written.strip_suffix('+').unwrap_or(written);
    let Some(suffix) = strip_ascii_prefix_caseless(base, "header-args:") else {
        return false;
    };
    suffix.eq_ignore_ascii_case(language)
}

fn strip_ascii_prefix_caseless<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let (head, tail) = s.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

/// Caseless-key merge for `#+PROPERTY:` lines and drawer `NODE_PROPERTY`s.
///
/// `+` appends with a space; a later bare key replaces; the surviving range
/// is the last contributing value.
pub(crate) fn merge_properties(entries: impl IntoIterator<Item = RawProperty>) -> Vec<RawProperty> {
    let mut acc: Vec<RawProperty> = Vec::new();
    for entry in entries {
        let append = entry.key.ends_with('+');
        let base = entry
            .key
            .strip_suffix('+')
            .unwrap_or(&entry.key)
            .to_string();
        if let Some(existing) = acc.iter_mut().find(|p| property_key_eq(&p.key, &base)) {
            if append {
                if !existing.value.is_empty() && !entry.value.is_empty() {
                    existing.value.push(' ');
                }
                existing.value.push_str(&entry.value);
                existing.range = entry.range;
            } else {
                *existing = entry;
            }
        } else {
            acc.push(entry);
        }
    }
    acc
}

/// Unmerged live `#+PROPERTY:` lines, document order (no caseless merge).
pub(crate) fn raw_file_property_lines(root: &SyntaxNode) -> impl Iterator<Item = RawProperty> + '_ {
    keyword_lines(root).filter_map(|line| {
        if !line.key.eq_ignore_ascii_case("property") {
            return None;
        }
        let base = line
            .value_range
            .map_or(line.key_range.end(), rowan::TextRange::start);
        parse_property_value(&line.value, base)
    })
}

/// `#+MACRO:` value: name, arg names, body. Unclosed `(` skips the line.
pub(crate) fn parse_macro_value(value: &str) -> Option<(String, Vec<String>, String)> {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'(' {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let name = value[name_start..i].to_string();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut args = Vec::new();
    if i < bytes.len() && bytes[i] == b'(' {
        let rest = &value[i + 1..];
        let rel = rest.find(')')?;
        let close = i + 1 + rel;
        args = value[i + 1..close]
            .split(',')
            .map(|s| s.trim_matches(|c: char| c.is_ascii_whitespace()))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        i = close + 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    Some((name, args, value[i..].to_string()))
}

/// `#+LINK:` value: first token is the key, remainder is the template.
pub(crate) fn parse_link_abbrev_value(value: &str) -> Option<(String, String)> {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let key_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let key = value[key_start..i].to_string();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    Some((key, value[i..].to_string()))
}

/// Babel `:key val` scan over a value string. Malformed quotes skip the rest.
pub(crate) fn parse_babel_header_args(value: &str, base: TextSize) -> Vec<RawProperty> {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut out = Vec::new();
    while i < bytes.len() {
        let Some(colon) = find_babel_key_colon(bytes, i) else {
            break;
        };
        i = colon + 1;
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == key_start {
            continue;
        }
        let key = value[key_start..i].to_string();
        let key_range = TextRange::at(
            base + TextSize::from(key_start as u32),
            TextSize::from(key.len() as u32),
        );
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            out.push(RawProperty {
                key,
                value: String::new(),
                range: key_range,
            });
            break;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            match read_babel_quoted(value, i) {
                None => break,
                Some((decoded, next)) => {
                    let raw_len = next - i;
                    let range = if decoded.is_empty() {
                        key_range
                    } else {
                        TextRange::at(
                            base + TextSize::from(i as u32),
                            TextSize::from(raw_len as u32),
                        )
                    };
                    out.push(RawProperty {
                        key,
                        value: decoded,
                        range,
                    });
                    i = next;
                }
            }
        } else {
            let val_start = i;
            let val_end = unquoted_babel_value_end(bytes, i);
            let val = value[val_start..val_end].to_string();
            let range = if val.is_empty() {
                key_range
            } else {
                TextRange::at(
                    base + TextSize::from(val_start as u32),
                    TextSize::from(val.len() as u32),
                )
            };
            out.push(RawProperty {
                key,
                value: val,
                range,
            });
            i = val_end;
        }
    }
    out
}

fn find_babel_key_colon(bytes: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == b':' && (j == 0 || bytes[j - 1].is_ascii_whitespace()) {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn unquoted_babel_value_end(bytes: &[u8], from: usize) -> usize {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j].is_ascii_whitespace() {
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b':' {
                return j;
            }
        }
        j += 1;
    }
    bytes.len()
}

fn read_babel_quoted(value: &str, start: usize) -> Option<(String, usize)> {
    let bytes = value.as_bytes();
    let quote = bytes[start];
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 1;
            if i >= bytes.len() {
                return None;
            }
            let ch = value[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if bytes[i] == quote {
            return Some((out, i + 1));
        }
        let ch = value[i..].chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }
    None
}

/// First non-whitespace token is the key; the rest of the line is the value.
/// A key with no following whitespace does not parse (Org's `\S+[ \t]+(.*)`).
pub(crate) fn parse_property_value(value: &str, base: TextSize) -> Option<RawProperty> {
    let key_rel = value.find(|c: char| !c.is_ascii_whitespace())?;
    let rest = &value[key_rel..];
    let key_len = rest.find(|c: char| c.is_ascii_whitespace())?;
    let key = &rest[..key_len];
    if key.is_empty() {
        return None;
    }
    let after = &rest[key_len..];
    let val_off = after
        .find(|c: char| !c.is_ascii_whitespace())
        .unwrap_or(after.len());
    let val = &after[val_off..];
    let val_rel = key_rel + key_len + val_off;
    let range = if val.is_empty() {
        TextRange::at(
            base + TextSize::from(key_rel as u32),
            TextSize::from(key.len() as u32),
        )
    } else {
        TextRange::at(
            base + TextSize::from(val_rel as u32),
            TextSize::from(val.len() as u32),
        )
    };
    Some(RawProperty {
        key: key.to_string(),
        value: val.to_string(),
        range,
    })
}

fn ws_tokens(s: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    let mut i = 0;
    let bytes = s.as_bytes();
    std::iter::from_fn(move || {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        Some((start, &s[start..i]))
    })
}

fn colon_parts(token: &str, token_start: usize) -> Vec<(usize, &str)> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, ch) in token.char_indices() {
        if ch == ':' {
            parts.push((token_start + start, &token[start..i]));
            start = i + 1;
        }
    }
    parts.push((token_start + start, &token[start..]));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TodoKind;
    use crate::parse_with;
    use TodoInterpretation::{Sequence, Type};

    fn names(sequence: &TodoSequence) -> (Vec<&str>, Vec<&str>) {
        (
            sequence.active().iter().map(String::as_str).collect(),
            sequence.done().iter().map(String::as_str).collect(),
        )
    }

    fn collect(src: &str) -> FileLayer {
        collect_file_layer(parse_with(src, &ParseConfig::default()).root())
    }

    /// Resolve `src`'s declared settings over `caller`, by value.
    fn resolve_over(caller: &ParseConfig, src: &str) -> ParseConfig {
        let caller = Arc::new(caller.clone());
        (*resolve(&caller, &collect(src))).clone()
    }

    // --- TODO value grammar ------------------------------------------------

    #[test]
    fn bar_splits_active_from_done() {
        let seq = parse_todo_value("TODO NEXT | DONE CANCELLED", Sequence).expect("names");
        assert_eq!(
            names(&seq),
            (vec!["TODO", "NEXT"], vec!["DONE", "CANCELLED"])
        );
        assert_eq!(seq.interpretation(), Sequence);
    }

    /// Org's no-bar rule, including the one-name case: `(last names)` makes the
    /// single keyword the done state, leaving no active keyword at all.
    #[test]
    fn without_a_bar_the_last_name_is_done() {
        let seq = parse_todo_value("TODO NEXT DONE", Sequence).expect("names");
        assert_eq!(names(&seq), (vec!["TODO", "NEXT"], vec!["DONE"]));

        let lone = parse_todo_value("WAITING", Sequence).expect("one name");
        assert_eq!(names(&lone), (vec![], vec!["WAITING"]));
    }

    #[test]
    fn bar_at_either_end_is_legal() {
        let all_done = parse_todo_value("| DONE", Sequence).expect("names");
        assert_eq!(names(&all_done), (vec![], vec!["DONE"]));

        let all_active = parse_todo_value("TODO |", Sequence).expect("names");
        assert_eq!(names(&all_active), (vec!["TODO"], vec![]));
    }

    /// Only the first bar splits; a second one is dropped like any non-name.
    #[test]
    fn later_bars_are_dropped() {
        let seq = parse_todo_value("A | B | C", Sequence).expect("names");
        assert_eq!(names(&seq), (vec!["A"], vec!["B", "C"]));
    }

    #[test]
    fn fast_access_keys_are_stripped() {
        let seq = parse_todo_value("TODO(t) WAIT(w@/!) | DONE(d!)", Sequence).expect("names");
        assert_eq!(names(&seq), (vec!["TODO", "WAIT"], vec!["DONE"]));

        // A `(` with no closer is name material, matching org-remove-keyword-keys.
        let unclosed = parse_todo_value("TODO(t", Sequence).expect("names");
        assert_eq!(names(&unclosed), (vec![], vec!["TODO(t"]));
    }

    #[test]
    fn lines_that_name_nothing_do_not_contribute() {
        for value in ["", "   ", "|", "| |", "()", "(x) ()"] {
            assert_eq!(
                parse_todo_value(value, Sequence),
                None,
                "value {value:?} names nothing"
            );
        }
    }

    #[test]
    fn typ_todo_keeps_its_interpretation() {
        let seq = parse_todo_value("Fred Sara | DONE", Type).expect("names");
        assert_eq!(seq.interpretation(), Type);
        assert_eq!(names(&seq), (vec!["Fred", "Sara"], vec!["DONE"]));
    }

    // --- PRIORITIES value grammar (not a resolve input) --------------------

    #[test]
    fn priorities_letters_numbers_and_mixed() {
        assert_eq!(
            parse_priorities_value("A C B"),
            Some(PriorityRange {
                highest: PriorityAtom::Letter('A'),
                lowest: PriorityAtom::Letter('C'),
                default: PriorityAtom::Letter('B'),
            })
        );
        assert_eq!(
            parse_priorities_value("1 64 5"),
            Some(PriorityRange {
                highest: PriorityAtom::Number(1),
                lowest: PriorityAtom::Number(64),
                default: PriorityAtom::Number(5),
            })
        );
        // Mixed forms are comparable through rank, so they are accepted.
        assert_eq!(
            parse_priorities_value("1 C B").map(|r| r.highest),
            Some(PriorityAtom::Number(1))
        );
        // Multi-digit numbers are one atom. Leading zeros are insignificant.
        assert_eq!(
            parse_priorities_value("007 9 8").map(|r| r.highest),
            Some(PriorityAtom::Number(7))
        );
        assert_eq!(
            parse_priorities_value("05 9 8"),
            Some(PriorityRange {
                highest: PriorityAtom::Number(5),
                lowest: PriorityAtom::Number(9),
                default: PriorityAtom::Number(8),
            })
        );
        assert_eq!(
            parse_priorities_value("00 1 2").map(|r| r.highest),
            Some(PriorityAtom::Number(0))
        );
        assert_eq!(
            parse_priorities_value("0064 1 2").map(|r| r.highest),
            Some(PriorityAtom::Number(64))
        );
        assert_eq!(
            parse_priorities_value("005 3 4").map(|r| r.highest),
            Some(PriorityAtom::Number(5))
        );
        assert_eq!(
            parse_priorities_value("G C B").map(|r| r.highest),
            Some(PriorityAtom::Letter('G'))
        );
    }

    #[test]
    fn priorities_rejects_short_lines_and_non_atoms() {
        for value in [
            "",
            "A",
            "A C", // fewer than three; no invented A/C/B
            "AA C B",
            "a C B",
            "A ? B", // not atoms
            "65 C B",
            "100 C B",
            "300 C B", // digit tokens are numbers, never letters
            "A1 C B",
            "1A C B", // no unanchored digit scan
            "A C a",
            "A C 65 extra", // a bad third token rejects the line; the tail cannot rescue it
        ] {
            assert_eq!(
                parse_priorities_value(value),
                None,
                "value {value:?} is not a range"
            );
        }
    }

    /// The first three tokens are the range. A later token is ignored, even
    /// when it is not an atom.
    #[test]
    fn priorities_uses_the_first_three_tokens() {
        let expected = PriorityRange {
            highest: PriorityAtom::Letter('A'),
            lowest: PriorityAtom::Letter('C'),
            default: PriorityAtom::Letter('B'),
        };
        for value in ["A C B D", "A C B nope", "A C B 65", "A  C\tB\nD extra"] {
            assert_eq!(
                parse_priorities_value(value),
                Some(expected),
                "value {value:?} should keep the first three"
            );
        }
    }

    /// Emacs `split-string` includes vertical tab. A non-breaking space does not.
    #[test]
    fn priorities_split_on_emacs_whitespace_only() {
        let expected = PriorityRange {
            highest: PriorityAtom::Letter('A'),
            lowest: PriorityAtom::Letter('C'),
            default: PriorityAtom::Letter('B'),
        };
        assert_eq!(
            parse_priorities_value("A\u{000b}C\u{000b}B"),
            Some(expected)
        );
        assert_eq!(
            parse_priorities_value("A\u{000b}C\u{000b}B\u{000b}nope"),
            Some(expected)
        );
        assert_eq!(
            parse_priorities_value("A\u{000c}C\u{000c}B"),
            Some(expected)
        );
        // U+00A0 is not in the Emacs set, so the first token is not an atom.
        assert_eq!(parse_priorities_value("A\u{00a0}C B D"), None);
    }

    /// An incoherent range is returned as written, not rejected. Checking it
    /// is the reader's job.
    #[test]
    fn priorities_are_stored_as_written() {
        // Default outside the range, and highest below lowest.
        assert!(parse_priorities_value("A C D").is_some());
        assert!(parse_priorities_value("C A B").is_some());
    }

    /// `#+PRIORITIES:` is unique: the first live line owns the slot even when
    /// it does not parse (Org collects the line, then reads it).
    #[test]
    fn first_priorities_line_wins() {
        let root = parse_with(
            "#+PRIORITIES: A E C\n#+PRIORITIES: 1 9 5\n",
            &ParseConfig::default(),
        )
        .root()
        .clone();
        assert_eq!(
            declared_priority_range(&root).map(|r| r.lowest),
            Some(PriorityAtom::Letter('E'))
        );

        // A tail does not void the first line, and the second line still loses.
        let tailed = parse_with(
            "#+PRIORITIES: A C B extra\n#+PRIORITIES: 1 9 5\n",
            &ParseConfig::default(),
        )
        .root()
        .clone();
        assert_eq!(
            declared_priority_range(&tailed),
            Some(PriorityRange {
                highest: PriorityAtom::Letter('A'),
                lowest: PriorityAtom::Letter('C'),
                default: PriorityAtom::Letter('B'),
            })
        );

        for src in [
            "#+PRIORITIES: nope\n#+PRIORITIES: A C B\n",
            "#+PRIORITIES:\n#+PRIORITIES: A C B\n",
        ] {
            let root = parse_with(src, &ParseConfig::default()).root().clone();
            assert_eq!(
                declared_priority_range(&root),
                None,
                "an unreadable first line still takes the slot: {src:?}"
            );
        }
    }

    /// Declaring a range is a tree fact, not a parse input: the file layer
    /// stays empty.
    #[test]
    fn declared_priority_range_does_not_enter_resolve() {
        let src = "#+PRIORITIES: 1 9 5\n";
        let root = parse_with(src, &ParseConfig::default()).root().clone();
        assert_eq!(
            declared_priority_range(&root).map(|r| r.default),
            Some(PriorityAtom::Number(5))
        );
        assert!(collect(src).is_empty());
    }

    // --- Collection over the CST -------------------------------------------

    #[test]
    fn collects_todo_family_in_document_order() {
        let layer = collect("#+TODO: A | B\n#+TYP_TODO: X | Y\n#+seq_todo: P | Q\n");
        assert_eq!(layer.todo.len(), 3);
        assert_eq!(names(&layer.todo[0]), (vec!["A"], vec!["B"]));
        assert_eq!(layer.todo[1].interpretation(), Type);
        assert_eq!(names(&layer.todo[2]), (vec!["P"], vec!["Q"]));
    }

    #[test]
    fn directive_after_the_first_headline_is_still_collected() {
        let layer = collect("* head\n#+TODO: BUG | FIXED\n");
        assert_eq!(layer.todo.len(), 1);
        assert_eq!(names(&layer.todo[0]), (vec!["BUG"], vec!["FIXED"]));
    }

    /// Liveness is tree shape: a lesser block body is verbatim text and never
    /// became a node, a greater block's contents are elements.
    #[test]
    fn lesser_block_bodies_are_inert_and_greater_block_bodies_are_live() {
        let inert = collect("#+begin_example\n#+TODO: A | B\n#+end_example\n");
        assert_eq!(inert, FileLayer::default());

        let live = collect("#+begin_quote\n#+TODO: A | B\n#+end_quote\n");
        assert_eq!(live.todo.len(), 1);
    }

    #[test]
    fn indented_directives_are_live() {
        assert_eq!(collect("   #+TODO: A | B\n").todo.len(), 1);
    }

    #[test]
    fn plain_keywords_are_not_settings() {
        assert_eq!(collect("#+TITLE: t\n#+CAPTION: c\n"), FileLayer::default());
    }

    #[test]
    fn carving_membership_agrees_with_is_carving_key_and_rejects_startup() {
        let src = concat!(
            "#+TODO: A | B\n",
            "#+SEQ_TODO: C | D\n",
            "#+TYP_TODO: E | F\n",
            "#+PRIORITIES: A B C\n",
            "#+STARTUP: hidestars\n",
            "#+COLUMNS: %TODO\n",
            "#+TITLE: t\n",
            "#+ATTR_LATEX: :width 5cm\n",
        );
        let root = parse_with(src, &ParseConfig::default()).root().clone();
        let keys: Vec<String> = setting_lines(&root).map(|l| l.key).collect();
        assert_eq!(
            keys.len(),
            4,
            "startup/columns/title/attr_latex must not contribute"
        );
        for k in &keys {
            assert!(is_carving_key(k), "setting_lines key {k:?} must carve");
        }
        assert!(keys.iter().any(|k| k.eq_ignore_ascii_case("todo")));
        assert!(keys.iter().any(|k| k.eq_ignore_ascii_case("seq_todo")));
        assert!(keys.iter().any(|k| k.eq_ignore_ascii_case("typ_todo")));
        assert!(keys.iter().any(|k| k.eq_ignore_ascii_case("priorities")));
        assert!(!is_carving_key("startup"));
        assert!(!is_carving_key("columns"));
        assert!(declared_priority_range(&root).is_some());
        assert_eq!(collect(src).todo.len(), 3);
    }

    #[test]
    fn with_key_text_borrows_one_token_and_concatenates_split_keys() {
        fn key_node(src: &str) -> SyntaxNode {
            parse_with(src, &ParseConfig::default())
                .root()
                .descendants()
                .find(|n| n.kind() == OrgSyntaxKind::KEYWORD_KEY)
                .expect("KEYWORD_KEY")
        }
        fn token_count(node: &SyntaxNode) -> usize {
            node.children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .count()
        }

        let todo = key_node("#+TODO: A | B\n");
        assert_eq!(token_count(&todo), 1);
        with_key_text(&todo, |s| assert_eq!(s, "TODO"));
        assert!(key_is_carving(&todo));

        let pri = key_node("#+PRIORITIES: A B C\n");
        assert_eq!(token_count(&pri), 1);
        with_key_text(&pri, |s| assert_eq!(s, "PRIORITIES"));
        assert!(key_is_carving(&pri));

        let seq = key_node("#+SEQ_TODO: C | D\n");
        assert_eq!(token_count(&seq), 3, "lexer splits _");
        with_key_text(&seq, |s| assert_eq!(s, "SEQ_TODO"));
        assert!(key_is_carving(&seq));

        let typ = key_node("#+TYP_TODO: E | F\n");
        assert_eq!(token_count(&typ), 3, "lexer splits _");
        with_key_text(&typ, |s| assert_eq!(s, "TYP_TODO"));
        assert!(key_is_carving(&typ));

        let attr = key_node("#+ATTR_LATEX: :width 5cm\n");
        assert_eq!(token_count(&attr), 3, "lexer splits _");
        with_key_text(&attr, |s| assert_eq!(s, "ATTR_LATEX"));
        assert!(!key_is_carving(&attr));

        let startup = key_node("#+STARTUP: hidestars\n");
        assert_eq!(token_count(&startup), 1);
        with_key_text(&startup, |s| assert_eq!(s, "STARTUP"));
        assert!(!key_is_carving(&startup));
    }

    // --- Resolve -----------------------------------------------------------

    #[test]
    fn file_todo_replaces_caller_todo() {
        let resolved = resolve_over(&ParseConfig::default(), "#+TODO: BUG | FIXED\n");
        assert_eq!(resolved.todo_kind("BUG"), Some(TodoKind::Active));
        assert_eq!(resolved.todo_kind("FIXED"), Some(TodoKind::Done));
        assert_eq!(resolved.todo_kind("TODO"), None, "replace, not merge");
    }

    #[test]
    fn a_file_with_no_contributing_line_keeps_the_caller_vocabulary() {
        let caller = ParseConfig::default();
        for src in ["#+TODO:\n", "#+TODO: |\n", "#+TODO: ()\n", "#+TITLE: x\n"] {
            assert_eq!(
                resolve_over(&caller, src),
                caller,
                "{src:?} must not change the vocabulary"
            );
        }
    }

    #[test]
    fn multiple_lines_accumulate_as_parallel_sequences() {
        let resolved = resolve_over(&ParseConfig::default(), "#+TODO: A | B\n#+TODO: C | D\n");
        assert_eq!(resolved.todo_sequences().len(), 2);
        for name in ["A", "C"] {
            assert_eq!(resolved.todo_kind(name), Some(TodoKind::Active));
        }
        for name in ["B", "D"] {
            assert_eq!(resolved.todo_kind(name), Some(TodoKind::Done));
        }
    }

    /// Duplicate across sequences: the last sequence decides the derived kind,
    /// and both sequences keep their full name lists.
    #[test]
    fn duplicate_name_takes_its_kind_from_the_last_sequence() {
        let resolved = resolve_over(&ParseConfig::default(), "#+TODO: X | B\n#+TODO: A | X\n");
        assert_eq!(resolved.todo_kind("X"), Some(TodoKind::Done));
        assert_eq!(resolved.todo_sequences().len(), 2);

        let flipped = resolve_over(&ParseConfig::default(), "#+TODO: A | X\n#+TODO: X | B\n");
        assert_eq!(flipped.todo_kind("X"), Some(TodoKind::Active));
    }

    /// The refresh-skip case: a file that only restates Org's defaults must
    /// resolve back to the default config, or every such open pays a reparse.
    #[test]
    fn restating_the_defaults_resolves_to_the_default_config() {
        let caller = ParseConfig::default();
        let src = "#+TODO: TODO | DONE\n#+PRIORITIES: A C B\n";
        assert_eq!(resolve_over(&caller, src), caller);
    }

    /// A document that declares nothing gets the caller's own allocation back.
    /// Most documents declare nothing, so this case must not copy a config
    /// (entity map, protocol registry, sequences) to say "unchanged".
    #[test]
    fn resolving_nothing_shares_the_caller_allocation() {
        let caller = Arc::new(ParseConfig::default());
        for src in ["", "* just a heading\n", "#+TITLE: x\n", "#+TODO: |\n"] {
            let resolved = resolve(&caller, &collect(src));
            assert!(Arc::ptr_eq(&caller, &resolved), "{src:?} resolves nothing");
        }
        // A file that *does* declare something allocates, even when what it
        // declares matches the caller: knowing that needs the comparison.
        let restated = resolve(&caller, &collect("#+TODO: TODO | DONE\n"));
        assert!(!Arc::ptr_eq(&caller, &restated));
        assert_eq!(*restated, *caller);
    }

    /// A file that declares only a range resolves to the caller's own
    /// allocation: there is nothing for `resolve` to layer, so the producer's
    /// pointer test short-circuits and no refresh parse is spent.
    #[test]
    fn a_priorities_only_file_resolves_to_the_caller_allocation() {
        let caller = Arc::new(ParseConfig::default());
        let layer = collect("#+PRIORITIES: 1 9 5\n");
        assert!(layer.is_empty(), "a priority range is not a parse input");
        assert!(Arc::ptr_eq(&caller, &resolve(&caller, &layer)));
    }

    #[test]
    fn resolve_preserves_caller_knobs_the_file_does_not_speak_to() {
        let caller = ParseConfig::default().with_inline_task_min_level(3);
        let resolved = resolve_over(&caller, "#+TODO: BUG | FIXED\n");
        assert_eq!(resolved.inline_task_min_level(), 3);
        assert_eq!(resolved.todo_kind("BUG"), Some(TodoKind::Active));
    }

    #[test]
    fn property_key_eq_strips_written_only() {
        assert!(property_key_eq("FOO", "foo"));
        assert!(property_key_eq("FOO+", "FOO"));
        assert!(property_key_eq("foo+", "FOO"));
        assert!(!property_key_eq("FOO", "FOO+"));
        assert!(!property_key_eq("FOO+", "FOO+"));
        assert!(property_key_eq("FOO++", "FOO+"));
    }

    #[test]
    fn header_args_key_matches_bare_and_language_suffix() {
        assert!(header_args_key_matches("header-args", None));
        assert!(header_args_key_matches("HEADER-ARGS+", None));
        assert!(!header_args_key_matches("header-args:python", None));
        assert!(header_args_key_matches(
            "header-args:python",
            Some("python")
        ));
        assert!(header_args_key_matches(
            "HEADER-ARGS:Python+",
            Some("python")
        ));
        assert!(!header_args_key_matches(
            "header-args:python",
            Some("elisp")
        ));
        assert!(!header_args_key_matches("header-args:", Some("python")));
    }

    #[test]
    fn merge_properties_append_replace_caseless() {
        let r = |start, end| TextRange::new(TextSize::from(start), TextSize::from(end));
        let merged = merge_properties([
            RawProperty {
                key: "FOO".into(),
                value: "bar".into(),
                range: r(0, 3),
            },
            RawProperty {
                key: "foo+".into(),
                value: "baz".into(),
                range: r(4, 7),
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].value, "bar baz");
        assert_eq!(merged[0].range, r(4, 7));

        let replaced = merge_properties([
            RawProperty {
                key: "FOO".into(),
                value: "bar".into(),
                range: r(0, 3),
            },
            RawProperty {
                key: "FOO".into(),
                value: "new".into(),
                range: r(8, 11),
            },
        ]);
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].value, "new");
        assert_eq!(replaced[0].range, r(8, 11));
    }

    #[test]
    fn parse_macro_value_name_args_body_and_unclosed_skip() {
        let (name, args, body) = parse_macro_value(" greet(name,  place ) Hello $1").unwrap();
        assert_eq!(name, "greet");
        assert_eq!(args, ["name", "place"]);
        assert_eq!(body, "Hello $1");
        assert!(parse_macro_value("foo(bar").is_none());
        assert!(parse_macro_value("   ").is_none());
        let (name, args, body) = parse_macro_value("bare").unwrap();
        assert_eq!(name, "bare");
        assert!(args.is_empty(), "{args:?}");
        assert_eq!(body, "");
    }

    #[test]
    fn parse_link_abbrev_value_key_and_empty_template() {
        let (key, template) = parse_link_abbrev_value("  gh  https://github.com/%s").unwrap();
        assert_eq!(key, "gh");
        assert_eq!(template, "https://github.com/%s");
        let (key, template) = parse_link_abbrev_value("abbrev").unwrap();
        assert_eq!(key, "abbrev");
        assert_eq!(template, "");
        assert!(parse_link_abbrev_value("   ").is_none());
    }

    #[test]
    fn parse_babel_header_args_unquoted_quoted_and_lone_colon() {
        let base = TextSize::from(10);
        let rows = parse_babel_header_args(":results output code :exports both", base);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "results");
        assert_eq!(rows[0].value, "output code");
        assert_eq!(rows[1].key, "exports");
        assert_eq!(rows[1].value, "both");

        let quoted = parse_babel_header_args(r#":file "a b.txt""#, TextSize::from(0));
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].key, "file");
        assert_eq!(quoted[0].value, "a b.txt");

        let skip_rest = parse_babel_header_args(":file \"unclosed", TextSize::from(0));
        assert!(skip_rest.is_empty(), "{skip_rest:?}");

        let lone = parse_babel_header_args(": :exports both", TextSize::from(0));
        assert_eq!(lone.len(), 1);
        assert_eq!(lone[0].key, "exports");
        assert_eq!(lone[0].value, "both");

        let v = ":results output code :exports both";
        let rows = parse_babel_header_args(v, base);
        let output_at = v.find("output").unwrap() as u32;
        assert_eq!(rows[0].range.start(), base + TextSize::from(output_at));

        let empty_val = parse_babel_header_args(":exports", TextSize::from(0));
        assert_eq!(empty_val.len(), 1);
        assert_eq!(empty_val[0].key, "exports");
        assert_eq!(empty_val[0].value, "");

        let single = parse_babel_header_args(":file 'a b'", TextSize::from(0));
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].value, "a b");

        let escaped = parse_babel_header_args(r#":file "a\"b""#, TextSize::from(0));
        assert_eq!(escaped.len(), 1);
        assert_eq!(escaped[0].value, "a\"b");

        let trailing_bs = parse_babel_header_args(":file \"ab\\", TextSize::from(0));
        assert!(trailing_bs.is_empty(), "{trailing_bs:?}");
    }

    #[test]
    fn raw_file_property_lines_unmerged_document_order() {
        let doc = crate::parse("#+PROPERTY: FOO a\n#+PROPERTY: foo+ b\n");
        let rows: Vec<_> = raw_file_property_lines(doc.root()).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "FOO");
        assert_eq!(rows[0].value, "a");
        assert_eq!(rows[1].key, "foo+");
        assert_eq!(rows[1].value, "b");
    }

    /// Collection reads the same file layer whichever vocabulary the tree was
    /// carved under. Producers rely on this to collect from a reused tree.
    #[test]
    fn collection_does_not_depend_on_the_config_the_tree_was_parsed_under() {
        let src = concat!(
            "#+TODO: BUG | FIXED\n",
            "* BUG one\n",
            "#+PRIORITIES: 1 9 5\n",
            "** NEXT two\n",
        );
        let a = ParseConfig::default();
        let b = ParseConfig::default()
            .with_todo_keywords([("NEXT", TodoKind::Active), ("BUG", TodoKind::Done)]);
        assert_eq!(
            collect_file_layer(parse_with(src, &a).root()),
            collect_file_layer(parse_with(src, &b).root())
        );
    }
}
