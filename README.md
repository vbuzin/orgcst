# orgcst

[![crates.io](https://img.shields.io/crates/v/orgcst.svg)](https://crates.io/crates/orgcst)
[![docs.rs](https://docs.rs/orgcst/badge.svg)](https://docs.rs/orgcst)
[![CI](https://github.com/vbuzin/orgcst/actions/workflows/ci.yml/badge.svg)](https://github.com/vbuzin/orgcst/actions/workflows/ci.yml)

A lossless parser for [Org mode](https://orgmode.org) documents, for editors
and tools that need to read, understand and change `.org` files without
disturbing a byte they did not mean to touch.

- **Lossless.** Every byte of the input lives in exactly one token of a
  [rowan](https://crates.io/crates/rowan) concrete syntax tree, so the tree's
  text equals the source for any input, malformed or not.
- **Faithful.** Grammar decisions follow `org-element`, and a parity harness
  compares the tree with Emacs Org in a pinned Docker image. The few
  differences are listed below.
- **Typed.** Headings, planning, property drawers, timestamps, links, lists,
  tables, blocks, footnotes, citations and the rest have typed views, with
  byte-exact ranges for every part and an insertion point for every part that
  is absent.
- **Editable.** Structural operations (set a TODO state, change tags, move a
  subtree, toggle a checkbox, edit a link) compile to plain text edits.
  Applying them reparses incrementally and reuses untouched outlines.
- **Infallible and bounded.** Parsing never fails; problems are reported as
  diagnostics. Nesting depth is capped, so hostile input cannot overflow the
  stack.
- **Portable.** No I/O and a single dependency (`rowan`). Builds for, and
  behaves identically on, `wasm32`.

> This is an independent project, not affiliated with the Org mode
> maintainers. The Org syntax specification lives on
> [Worg](https://orgmode.org/worg/org-syntax.html).

## Status

Pre-1.0. The parser covers the Org syntax specification, is fuzzed, and is
checked against `org-element`, but the public API is still settling. Expect
breaking changes in 0.x minor releases; the [changelog](CHANGELOG.md) lists
them.

## Usage

```toml
[dependencies]
orgcst = "0.1"
```

Parse, then query through typed views:

```rust
use orgcst::{AstNode, Headline, TodoKind, parse};

let doc = parse("* TODO Ship it :work:\n\nSome /emphasised/ text.\n");

// Lossless: the tree holds every byte.
assert_eq!(doc.root().text().to_string(), doc.source());

let heading = doc.headings().next().unwrap();
assert_eq!(heading.level(), 1);
assert_eq!(doc.todo_kind(&heading), Some(TodoKind::Active));
```

Change a document with a typed operation. The operation compiles to text
edits, and `apply` returns a new snapshot while the old one stays as it was:

```rust
use orgcst::{NodeRef, Operation, compile, parse};

let doc = parse("* TODO Ship it\n");
let heading = doc.headings().next().unwrap();
let op = Operation::SetTodoState {
    target: NodeRef::of(&heading),
    state: Some("DONE".to_string()),
};
let edits = compile(&doc, &op).unwrap();
let next = doc.apply(&edits);
assert_eq!(next.write(), "* DONE Ship it\n");
assert_eq!(doc.write(), "* TODO Ship it\n");
```

In-buffer settings such as `#+TODO:` are honoured. A caller supplies its own
defaults with `parse_with` and `ParseConfig`.

## Design

- Hand-written recursive descent over a lossless token stream. No parser
  combinators and no regular expressions, so every rule is explicit.
- The grammar emits events, and a builder turns them into rowan green nodes.
- Ranges are absolute byte offsets at the API. Green nodes store lengths, so
  unchanged subtrees are shared between snapshots.
- A construct's range covers its full syntax, delimiters included.
- Configuration that changes the tree's shape (TODO keywords, link types,
  the inline task level, the nesting limit) is applied at parse time.
  Everything else a document declares is read from the tree on demand.
- A `Document` is an immutable snapshot. Red nodes are not `Send`;
  `PortableDocument` carries a snapshot across threads.

## Differences from org-element

| Input | org-element | This crate | Why |
| ----- | ----------- | ---------- | --- |
| A `+-` grid (a table.el table) | one table of type `table.el` | not a table.el table | out of scope |
| Prose that matches a `<<<radio target>>>` | a link | plain text | matching radio targets is a query, not tree structure |
| `\begin{eq} x \end{eq}` on one line | a LaTeX environment | not an environment | deliberate |
| `#+call[inside]: value` | a keyword | a babel call | deliberate |
| Inline objects nested more than 128 deep | nested | flattened past the limit | stack safety |
| `x^a\alpha-b` | entity under the superscript | no entity | known bug |
| An orphaned `#+CAPTION[short]: long` | a paragraph | a keyword | known bug |
| `$a$` followed by U+00A0, U+2013, U+2014, U+2019 or U+2026 | a LaTeX fragment | not a fragment | known bug |
| `<https://x` with `>` starting the next line | `https://x` as a plain link | no plain link | known bug |

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings -W clippy::pedantic
cargo check --target wasm32-unknown-unknown
scripts/wasm-check.sh           # native and wasm32-wasip1 output match; needs wasmtime
parity/parity.sh                # compare with org-element; needs Docker
cargo +nightly fuzz run parse   # needs cargo-fuzz
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for more.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 licence, shall be
dual licensed as above, without any additional terms or conditions.
