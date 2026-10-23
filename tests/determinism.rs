//! Cross-thread determinism of parse and apply.
//!
//! Public API only. Each thread builds its own `ParseConfig`: the todo-keyword
//! map and the user-entity map are lookup tables, and this is what shows the
//! output does not walk them in hasher order.

use std::fmt::Write as _;
use std::thread;

use orgcst::{
    AstNode, DiagnosticKind, Document, EditSet, EntityDef, Headline, OrgSyntaxKind, ParseConfig,
    SyntaxNode, TextRange, TextSize, TodoKind, parse, parse_with,
};

/// Taxonomy headline row, plus an invalid cookie so the diagnostic half of the
/// walk is compared rather than left empty.
const HEADLINE: &str = "\
* TODO [#A] Title :work:home:\n\
body\n\
\n\
** Child\n\
* [#a] bad\n";

/// In-buffer vocabulary differs from the caller layer built in [`once`].
const TODO_FILE: &str = "\
#+TODO: WAIT | STOP\n\
* WAIT filed\n\
* TODO plain\n";

/// Custom TODO word plus two user entities, so a map walk would be order-sensitive.
const CUSTOM: &str = "* SHIP custom\n\\myent{} and \\zetaent\n";

const THREADS: usize = 8;

#[test]
fn same_fingerprint_on_eight_threads() {
    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        handles.push(thread::spawn(once));
    }
    let mut fingerprints = Vec::with_capacity(THREADS);
    for handle in handles {
        fingerprints.push(handle.join().expect("determinism thread panicked"));
    }
    let expected = &fingerprints[0];
    for (i, got) in fingerprints.iter().enumerate().skip(1) {
        assert_eq!(got, expected, "thread {i} fingerprint diverged");
    }
}

/// One thread: own configs, the shared edit shape, cold parse of the result.
fn once() -> Vec<String> {
    let empty = parse("");
    let headline = parse(HEADLINE);
    // Built here, not in the parent. A clone of one parent map would share
    // hasher iteration order and hide a walk.
    let caller = ParseConfig::default()
        .with_todo_keywords([("NEXT", TodoKind::Active), ("CANCELLED", TodoKind::Done)]);
    let todo_file = parse_with(TODO_FILE, &caller);
    let custom_cfg = ParseConfig::default()
        .with_todo_keywords([("SHIP", TodoKind::Active), ("SUNK", TodoKind::Done)])
        .with_user_entities([
            EntityDef::new("myent", r"\my", false, "&m;", "m", "m", "m"),
            EntityDef::new("zetaent", r"\z", false, "&z;", "z", "z", "z"),
        ]);
    let custom = parse_with(CUSTOM, &custom_cfg);

    let mut headings = todo_file.headings();
    let filed = headings.next().expect("WAIT heading");
    let keyword = filed
        .todo_keyword()
        .expect("file #+TODO: carves WAIT over the caller layer");
    assert_eq!(todo_file.slice(keyword.range()), "WAIT");
    let plain = headings.next().expect("second heading");
    assert!(
        plain.todo_keyword().is_none(),
        "caller vocabulary must not carve TODO"
    );

    let ship = custom.headings().next().expect("SHIP heading");
    let ship_kw = ship.todo_keyword().expect("custom TODO word is carved");
    assert_eq!(custom.slice(ship_kw.range()), "SHIP");
    let entities = custom
        .root()
        .descendants()
        .filter(|node| node.kind() == OrgSyntaxKind::ENTITY)
        .count();
    assert_eq!(entities, 2, "both user entities are carved");
    assert!(
        headline.diagnostics().iter().any(|diag| {
            matches!(
                diag.kind(),
                DiagnosticKind::InvalidPriorityCookie { value } if value == "a"
            )
        }),
        "invalid cookie is a diagnostic"
    );

    let start = HEADLINE.find("Title").expect("headline contains Title");
    let end = start + "Title".len();
    let range = TextRange::new(
        TextSize::try_from(start).expect("start fits TextSize"),
        TextSize::try_from(end).expect("end fits TextSize"),
    );
    let edits = EditSet::single(HEADLINE, range, "Renamed").expect("title edit is in range");
    let applied = headline.apply(&edits);
    assert!(applied.source().contains("Renamed"), "edit did not apply");
    assert!(
        !applied.source().contains("Title"),
        "title replacement left the old word"
    );
    let cold = parse(applied.source());

    let empty_fp = fingerprint(&empty);
    let headline_fp = fingerprint(&headline);
    let todo_fp = fingerprint(&todo_file);
    let custom_fp = fingerprint(&custom);
    let applied_fp = fingerprint(&applied);
    assert!(headline_fp.contains("STAR "), "fingerprint dropped tokens");
    assert!(
        headline_fp.contains("InvalidPriorityCookie"),
        "fingerprint dropped diagnostics"
    );
    assert!(
        todo_fp.contains("TODO_KEYWORD "),
        "fingerprint dropped the carved keyword"
    );
    assert!(
        custom_fp.contains("ENTITY "),
        "fingerprint dropped a user entity"
    );
    assert_eq!(
        fingerprint(&cold),
        applied_fp,
        "cold parse of the applied source diverged"
    );
    vec![empty_fp, headline_fp, todo_fp, custom_fp, applied_fp]
}

// Same walk as `examples/wasm_behaviour.rs` and `fuzz/src/lib.rs`: a walk
// rather than the green tree's `Debug` output, which can embed allocator
// addresses. Not public API.
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
