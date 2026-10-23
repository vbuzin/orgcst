//! Property checks for the three cargo-fuzz bins.
//!
//! `libfuzzer-sys` stays in this package, so the library never depends on it.

use orgcst::{Document, Edit, EditSet, SyntaxNode, TextRange, TextSize, parse};

/// Inputs above this return before `parse`. Stays under `MAX_SOURCE_LEN`,
/// so the `u32` size panic on `parse` is not a fuzz input.
const MAX_BYTES: usize = 64 * 1024;

/// A handful of replacements, not an edit grammar.
const MAX_EDITS: usize = 4;

struct Batch {
    source: String,
    edits: Vec<Edit>,
}

#[allow(clippy::missing_panics_doc)]
pub fn fuzz_parse(data: &[u8]) {
    if data.len() > MAX_BYTES {
        return;
    }
    // Discard. A lossy conversion would hide a panic in a different string.
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let doc = parse(input);
    assert_eq!(doc.write(), doc.source(), "write() is not source()");
    assert_eq!(doc.source(), input, "source() is not the input");
    check_tiling(doc.root(), input.len());
}

#[allow(clippy::missing_panics_doc)]
pub fn fuzz_apply(data: &[u8]) {
    let Some((doc, spliced)) = apply_batch(data) else {
        return;
    };
    assert_eq!(doc.write(), spliced);
}

#[allow(clippy::missing_panics_doc)]
pub fn fuzz_roundtrip(data: &[u8]) {
    let Some((doc, _)) = apply_batch(data) else {
        return;
    };
    let cold = parse(doc.source());
    assert_eq!(fingerprint(&cold), fingerprint(&doc));
}

fn apply_batch(data: &[u8]) -> Option<(Document, String)> {
    let batch = decode_batch(data)?;
    let doc = parse(&batch.source);
    // `new` must not panic on a batch it rejects. Only the accepted ones apply.
    let Ok(set) = EditSet::new(&batch.source, batch.edits) else {
        return None;
    };
    let spliced = set.apply_to_text(&batch.source);
    let doc = doc.apply(&set);
    Some((doc, spliced))
}

fn decode_batch(data: &[u8]) -> Option<Batch> {
    if data.len() > MAX_BYTES {
        return None;
    }
    if data.is_empty() {
        return Some(Batch {
            source: String::new(),
            edits: Vec::new(),
        });
    }
    let n = usize::from(data[0]) % (MAX_EDITS + 1);
    let mut i = 1;
    let mut pending = Vec::with_capacity(n);
    for _ in 0..n {
        if i + 5 > data.len() {
            break;
        }
        let start_raw = u16::from_le_bytes([data[i], data[i + 1]]);
        let end_raw = u16::from_le_bytes([data[i + 2], data[i + 3]]);
        let repl_len = usize::from(data[i + 4]);
        let repl_at = i + 5;
        let Some(repl_end) = repl_at.checked_add(repl_len) else {
            break;
        };
        if repl_end > data.len() {
            break;
        }
        pending.push((start_raw, end_raw, repl_at..repl_end));
        i = repl_end;
    }
    let source = std::str::from_utf8(&data[i..]).ok()?.to_owned();
    let mut edits = Vec::with_capacity(pending.len());
    for (start_raw, end_raw, repl_at) in pending {
        let repl = std::str::from_utf8(&data[repl_at]).ok()?.to_owned();
        let start = map_offset(start_raw, source.len());
        let end = map_offset(end_raw, source.len());
        // `TextRange::new` panics when end < start. That is not `EditSet::new`
        // rejecting a range, so an inverted span is not an edit.
        if start > end {
            continue;
        }
        edits.push(Edit {
            range: TextRange::new(TextSize::from(start), TextSize::from(end)),
            replacement: repl,
        });
    }
    Some(Batch { source, edits })
}

/// An unscaled integer almost never lands inside the 64 KiB cap, so
/// `apply` would not run. Fold into `0..=len`. `u16::MAX` stays one past
/// the end so `EditSet::new` still sees an out-of-bounds range.
fn map_offset(raw: u16, len: usize) -> u32 {
    let Ok(len_u) = u32::try_from(len) else {
        return u32::from(raw);
    };
    if len_u == 0 {
        return u32::from(raw);
    }
    if raw == u16::MAX {
        return len_u.saturating_add(1);
    }
    u32::from(raw) % len_u.saturating_add(1)
}

fn check_tiling(root: &SyntaxNode, len: usize) {
    let mut cursor = 0usize;
    walk_tiling(root, None, &mut cursor);
    assert_eq!(cursor, len, "tokens do not cover 0..{len}");
}

fn walk_tiling(node: &SyntaxNode, parent: Option<TextRange>, cursor: &mut usize) {
    let range = node.text_range();
    if let Some(parent) = parent {
        assert!(
            range.start() >= parent.start() && range.end() <= parent.end(),
            "node {:?} {range:?} sits outside {parent:?}",
            node.kind(),
        );
    }
    for element in node.children_with_tokens() {
        if let Some(child) = element.as_node() {
            walk_tiling(child, Some(range), cursor);
        } else if let Some(token) = element.as_token() {
            let token_range = token.text_range();
            assert!(
                token_range.start() >= range.start() && token_range.end() <= range.end(),
                "token {:?} {token_range:?} sits outside {range:?}",
                token.kind(),
            );
            let start = usize::from(token_range.start());
            let end = usize::from(token_range.end());
            assert_eq!(
                start,
                *cursor,
                "token {:?} {token_range:?} is out of order or overlapping at {cursor}",
                token.kind(),
            );
            *cursor = end;
        }
    }
}

use std::fmt::Write;

// Same walk as `tests/determinism.rs` and `examples/wasm_behaviour.rs`: a
// walk rather than the green tree's `Debug` output, which can embed allocator
// addresses. Not public API.
fn fingerprint(doc: &Document) -> String {
    let mut out = String::new();
    walk_fingerprint(doc.root(), &mut out);
    for diag in doc.diagnostics() {
        let _ = writeln!(out, "{:?} {:?}", diag.kind(), diag.range());
    }
    out
}

fn walk_fingerprint(node: &SyntaxNode, out: &mut String) {
    let _ = writeln!(out, "{:?} {:?}", node.kind(), node.text_range());
    for element in node.children_with_tokens() {
        if let Some(child) = element.as_node() {
            walk_fingerprint(child, out);
        } else if let Some(token) = element.as_token() {
            let _ = writeln!(out, "{:?} {:?}", token.kind(), token.text_range());
        }
    }
}
