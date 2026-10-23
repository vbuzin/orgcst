//! Reuse of a prior tree inside materialize.
//!
//! "Damage" is the part of a prior tree that a new text or config
//! invalidates. With the same bytes and no structural knob changed (see
//! [`parse_config_delta`]), equal TODO name sets keep the prior green tree and
//! a name-set change recarves only the damaged star lines. Different bytes
//! with equal name sets replace the nearest enclosing outline, or the zeroth
//! section when the edit is before the first outline. A structural-knob
//! change, or different bytes together with a name-set change, falls back to
//! a cold parse.
//!
//! Text and config damage meet at [`try_reuse`]. A producer handles them as
//! two separate hops (text, then config); there is no combined damage value.

mod config;
mod outline;

use crate::document::{Document, DocumentParts};
use crate::parser::{Diagnostic, ParseConfig, ParseConfigDelta, parse_config_delta};
use rowan::{GreenNode, GreenToken, NodeOrToken, TextRange, TextSize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `TextRange` is not `Ord`. Start then end is the splice-map order.
pub(super) fn span_key(range: TextRange) -> (u32, u32) {
    (u32::from(range.start()), u32::from(range.end()))
}

type GreenElement = NodeOrToken<GreenNode, GreenToken>;

/// Reuse `prior` when no structural knob changed.
///
/// Same bytes keep the tree, or recarve damaged star lines when the name
/// sets differ. Different bytes reuse one outline or the zeroth section when
/// the name sets match, and stay cold when they do not. `None` means the
/// cold path. Does not count as a cold build and does not resolve settings.
/// The caller layer is the config this hop was asked to carve under;
/// `materialize_resolved` replaces it.
pub(crate) fn try_reuse(
    text: &Arc<str>,
    config: &Arc<ParseConfig>,
    prior: &Document,
) -> Option<Document> {
    let delta: ParseConfigDelta = parse_config_delta(prior.config(), config.as_ref());
    if delta.caller_structural {
        return None;
    }
    // Empty means the name sets match. Recarve only when the sets differ on
    // unchanged bytes; a text change with a name-set change is the other hop.
    let (green, diagnostics) = if text.as_ref() == prior.source() {
        if delta.is_empty() {
            reused_parts(prior)
        } else {
            let names = delta.todo_keywords.as_ref()?;
            config::recarve(prior, config.as_ref(), names)?
        }
    } else if delta.is_empty() {
        outline::reuse(text.as_ref(), config.as_ref(), prior)?
    } else {
        return None;
    };
    Some(Document::from_parts(DocumentParts {
        source: Arc::clone(text),
        green,
        config: Arc::clone(config),
        caller: Arc::clone(config),
        diagnostics,
    }))
}

/// Move a diagnostic by `delta` bytes. `None` if either end leaves `TextSize`.
fn shift_diagnostic(diag: &Diagnostic, delta: i64) -> Option<Diagnostic> {
    let range = diag.range();
    let start = add_delta(range.start(), delta)?;
    let end = add_delta(range.end(), delta)?;
    Some(Diagnostic::new(
        diag.kind().clone(),
        TextRange::new(start, end),
    ))
}

fn add_delta(pos: TextSize, delta: i64) -> Option<TextSize> {
    let shifted = i64::from(u32::from(pos)).checked_add(delta)?;
    u32::try_from(shifted).ok().map(TextSize::from)
}

fn intersects(a: TextRange, b: TextRange) -> bool {
    a.start() < b.end() && b.start() < a.end()
}

fn reused_parts(prior: &Document) -> (GreenNode, Vec<Diagnostic>) {
    (
        prior.root().green().into_owned(),
        prior.diagnostics().to_vec(),
    )
}

/// Splice `replacements` into `root`.
///
/// Keys are absolute ranges. A node whose range equals a key, with no other
/// key strictly inside it and the same kind as the replacement, is that
/// replacement. A node with no key inside it is returned as the same `Arc`.
/// Anything else is rebuilt child by child. An inline-task star line is a
/// prefix of the task, not the task node: the walk swaps that prefix for the
/// recarved heading's children and splices the tail, so a nested damaged task
/// is not dropped.
///
/// `None` if a key does not end on a child boundary. Callers fall back to cold.
fn splice_replacements(
    root: GreenNode,
    replacements: &BTreeMap<(u32, u32), GreenNode>,
) -> Option<GreenNode> {
    splice_at(root, TextSize::from(0), replacements)
}

fn splice_at(
    node: GreenNode,
    start: TextSize,
    map: &BTreeMap<(u32, u32), GreenNode>,
) -> Option<GreenNode> {
    let end = start + node.text_len();
    let range = TextRange::new(start, end);
    if keys_inside(map, range).next().is_none() {
        return Some(node);
    }
    if let Some(repl) = map.get(&span_key(range))
        && repl.kind() == node.kind()
        && keys_inside(map, range).all(|key| key == range)
    {
        return Some(repl.clone());
    }

    let kind = node.kind();
    let old_len = node.text_len();
    let children: Vec<GreenElement> = node.children().map(rowan::NodeOrToken::to_owned).collect();
    let mut out = Vec::new();
    let mut offset = start;
    let mut i = 0;
    while i < children.len() {
        if let Some(key) = key_at(map, offset, end) {
            let repl = map.get(&span_key(key))?.clone();
            // A child that contains the key (a heading node, or a task that
            // is bigger than its star line) is spliced whole. The star line
            // itself is several children of the task; those are swapped for
            // the recarved heading's children.
            if let NodeOrToken::Node(child) = &children[i] {
                let child_end = offset + child.text_len();
                if child_end >= key.end() {
                    let spliced = splice_at(child.clone(), offset, map)?;
                    offset += spliced.text_len();
                    out.push(NodeOrToken::Node(spliced));
                    i += 1;
                    continue;
                }
            }
            let mut covered = offset;
            while i < children.len() && covered < key.end() {
                let len = children[i].text_len();
                if covered + len > key.end() {
                    debug_assert!(false, "star-line key must end on a child boundary");
                    return None;
                }
                covered += len;
                i += 1;
            }
            if covered != key.end() {
                debug_assert!(false, "star-line key must end on a child boundary");
                return None;
            }
            for child in repl.children() {
                out.push(child.to_owned());
            }
            offset = key.end();
            continue;
        }

        match &children[i] {
            NodeOrToken::Node(child) => {
                let spliced = splice_at(child.clone(), offset, map)?;
                offset += spliced.text_len();
                out.push(NodeOrToken::Node(spliced));
            }
            NodeOrToken::Token(token) => {
                offset += token.text_len();
                out.push(NodeOrToken::Token(token.clone()));
            }
        }
        i += 1;
    }

    let built = GreenNode::new(kind, out);
    if built.text_len() != old_len {
        debug_assert!(false, "splice changed the node length");
        return None;
    }
    Some(built)
}

fn keys_inside(
    map: &BTreeMap<(u32, u32), GreenNode>,
    range: TextRange,
) -> impl Iterator<Item = TextRange> + '_ {
    let start = u32::from(range.start());
    let end = u32::from(range.end());
    map.range((start, 0)..(end, 0))
        .filter(move |((key_start, key_end), _)| *key_end <= end && *key_start >= start)
        .map(|((key_start, key_end), _)| {
            TextRange::new(TextSize::from(*key_start), TextSize::from(*key_end))
        })
}

/// Key that begins at `offset` and ends at or before `limit`.
fn key_at(
    map: &BTreeMap<(u32, u32), GreenNode>,
    offset: TextSize,
    limit: TextSize,
) -> Option<TextRange> {
    let off = u32::from(offset);
    let limit = u32::from(limit);
    let (key, _) = map.range((off, 0)..(off.saturating_add(1), 0)).next()?;
    (key.0 == off && key.1 <= limit)
        .then_some(TextRange::new(TextSize::from(key.0), TextSize::from(key.1)))
}

#[cfg(test)]
pub(crate) use recarve_count::while_running as recarves_while;

#[cfg(test)]
mod recarve_count {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record() {
        COUNT.with(|c| c.set(c.get() + 1));
    }

    pub(crate) fn while_running<T>(f: impl FnOnce() -> T) -> (T, usize) {
        COUNT.with(|c| c.set(0));
        let value = f();
        (value, COUNT.with(Cell::get))
    }
}
