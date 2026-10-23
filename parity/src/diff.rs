//! Structural diff of two parity JSON trees.

use serde_json::{Map, Value};
use std::collections::BTreeSet;

const DIFF_LIMIT: usize = 30;

pub struct Diff {
    pub path: String,
    pub emacs: String,
    pub rust: String,
}

pub fn diff_trees(emacs: &Value, rust: &Value) -> Vec<Diff> {
    let mut out = Vec::new();
    walk(emacs, rust, "", &mut out);
    out
}

pub fn render(label: &str, coding: bool, diffs: &[Diff], verbose: bool) -> String {
    let mut text = String::new();
    let shown = diffs.len().min(DIFF_LIMIT);
    for diff in diffs.iter().take(shown) {
        if coding {
            text.push_str("coding ");
        }
        text.push_str(label);
        text.push_str(": ");
        text.push_str(&diff.path);
        text.push_str(": emacs=");
        text.push_str(&diff.emacs);
        text.push_str(" rust=");
        text.push_str(&diff.rust);
        text.push('\n');
    }
    if diffs.len() > shown && !verbose {
        text.push_str(&format!(
            "{label}: {more} more differences (pass --verbose)\n",
            more = diffs.len() - shown
        ));
    }
    text
}

fn walk(emacs: &Value, rust: &Value, path: &str, out: &mut Vec<Diff>) {
    match (emacs, rust) {
        (Value::Object(left), Value::Object(right)) => walk_object(left, right, path, out),
        (Value::Array(left), Value::Array(right)) => {
            let len = left.len().max(right.len());
            for index in 0..len {
                let child = format!("{path}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(emacs), Some(rust)) => walk(emacs, rust, &child, out),
                    (Some(emacs), None) => push(out, &child, &emacs.to_string(), "missing"),
                    (None, Some(rust)) => push(out, &child, "missing", &rust.to_string()),
                    (None, None) => {}
                }
            }
        }
        _ => {
            if emacs != rust {
                push(out, path, &emacs.to_string(), &rust.to_string());
            }
        }
    }
}

fn walk_object(
    left: &Map<String, Value>,
    right: &Map<String, Value>,
    path: &str,
    out: &mut Vec<Diff>,
) {
    let mut keys = BTreeSet::new();
    keys.extend(left.keys());
    keys.extend(right.keys());
    for key in keys {
        if path.is_empty() && (key == "org_version" || key == "emacs_version") {
            continue;
        }
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match (left.get(key), right.get(key)) {
            (Some(emacs), Some(rust)) => walk(emacs, rust, &child, out),
            (Some(emacs), None) => push(out, &child, &emacs.to_string(), "missing"),
            (None, Some(rust)) => push(out, &child, "missing", &rust.to_string()),
            (None, None) => {}
        }
    }
}

fn push(out: &mut Vec<Diff>, path: &str, emacs: &str, rust: &str) {
    out.push(Diff {
        path: if path.is_empty() {
            "<root>".to_string()
        } else {
            path.to_string()
        },
        emacs: emacs.to_string(),
        rust: rust.to_string(),
    });
}
