//! Native and `wasm32-wasip1` behaviour check: run it on both targets and
//! compare the printed fingerprints.
//!
//! An example, not part of the library, so the `wasm32-unknown-unknown`
//! library check does not build it. The only WASI use is `println`.

use std::fmt::Write as _;

use orgcst::{
    Document, EditSet, EntityDef, ParseConfig, SyntaxNode, TextRange, TextSize, TodoKind, parse,
    parse_with,
};

const EMPTY: &str = "";

const HEADLINE: &str = "\
* TODO [#A] Title :work:home:\n\
body\n\
\n\
** Child\n\
* [#a] bad\n";

const CUSTOM: &str = "* SHIP custom\n\\myent{} and \\zetaent\n";

fn main() {
    let empty = parse(EMPTY);
    let headline = parse(HEADLINE);
    let custom_cfg = ParseConfig::default()
        .with_todo_keywords([("SHIP", TodoKind::Active), ("SUNK", TodoKind::Done)])
        .with_user_entities([
            EntityDef::new("myent", r"\my", false, "&m;", "m", "m", "m"),
            EntityDef::new("zetaent", r"\z", false, "&z;", "z", "z", "z"),
        ]);
    let custom = parse_with(CUSTOM, &custom_cfg);

    let start = HEADLINE.find("Title").expect("headline contains Title");
    let end = start + "Title".len();
    let range = TextRange::new(
        TextSize::try_from(start).expect("start fits TextSize"),
        TextSize::try_from(end).expect("end fits TextSize"),
    );
    let edits = EditSet::single(HEADLINE, range, "Renamed").expect("title edit is in range");
    let applied = headline.apply(&edits);
    let cold = parse(applied.source());
    assert_eq!(
        fingerprint(&cold),
        fingerprint(&applied),
        "cold parse of the applied source diverged"
    );

    let mut out = String::new();
    out.push_str(&fingerprint(&empty));
    out.push_str(&fingerprint(&headline));
    out.push_str(&fingerprint(&custom));
    out.push_str(&fingerprint(&applied));
    println!("{out}");
}

// Same walk as `tests/determinism.rs` and `fuzz/src/lib.rs`: a walk rather
// than the green tree's `Debug` output, which can embed allocator addresses.
// Not public API.
fn fingerprint(doc: &Document) -> String {
    let mut out = String::new();
    walk_fingerprint(doc.root(), &mut out);
    for diag in doc.diagnostics() {
        writeln!(out, "{:?} {:?}", diag.kind(), diag.range()).expect("write to String");
    }
    out
}

fn walk_fingerprint(node: &SyntaxNode, out: &mut String) {
    writeln!(out, "{:?} {:?}", node.kind(), node.text_range()).expect("write to String");
    for element in node.children_with_tokens() {
        if let Some(child) = element.as_node() {
            walk_fingerprint(child, out);
        } else if let Some(token) = element.as_token() {
            writeln!(out, "{:?} {:?}", token.kind(), token.text_range()).expect("write to String");
        }
    }
}
