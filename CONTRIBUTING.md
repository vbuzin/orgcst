# Contributing

Thanks for your interest. Bug reports, Org snippets that parse wrongly, and
pull requests are all welcome.

## Reporting a parse bug

The most useful report is a short Org snippet, what this crate produces, and
what Emacs Org produces (`M-x org-element-parse-buffer`, or the parity harness
below). Please say which Org version you compared against.

## Before you open a pull request

Run the same checks CI runs. `rust-toolchain.toml` pins the toolchain for
both, so a check that passes here passes in CI:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings -W clippy::pedantic
cargo test
cargo doc --no-deps
cargo check --target wasm32-unknown-unknown
```

For grammar changes, also run the parity harness if you can (it needs Docker):

```bash
parity/parity.sh
```

It parses the corpus under `parity/corpus/` with this crate and with
`org-element` from a pinned Org commit, and diffs the two. Known differences
are listed in `parity/exceptions.json`, each with a fixture under
`parity/corpus/diverge/`.

## Ground rules for the parser

These keep the parser predictable. A change that needs to break one should
say so in the pull request. [ARCHITECTURE.md](ARCHITECTURE.md) explains them
and the code layout in more depth.

- **Lossless.** Every source byte belongs to exactly one token. Round trips are
  byte-exact.
- **Infallible.** Parsing never returns an error and never panics on valid
  UTF-8 within `MAX_SOURCE_LEN`. Recoverable problems become diagnostics.
- **Bounded.** Recursion is limited by `ParseConfig::max_nesting_depth`.
- **Hand-written.** Recursive descent over the token stream; no parser
  combinators and no regular expressions in the library.
- **One dependency.** The library depends on `rowan` only, and must build for
  `wasm32-unknown-unknown`. New dependencies need a good reason.
- **No `unsafe`** beyond the existing rowan kind conversion.
- **Typed errors** in the public API; no `unwrap` or `expect` in library code
  except on invariants the parser itself establishes.

## Ground rules for the public API

The API is read by editors, tools and agents this crate has never heard of,
so an addition must be right for all of them, not convenient for one.

- **Org's syntax, and only that.** An addition is Org's syntax, the notation
  of its declarations, or writing it back. It can be answered without knowing
  what Org does with an option; what an option does belongs to the caller.
- **In Org's terms, for any caller.** A type, method or behaviour is named and
  shaped by what Org says, never by one caller's loop, screen or workflow. A
  need that is not Org's syntax stays in the caller.
- **A snapshot holds what parsing makes.** `Document` keeps its source, tree,
  configuration and diagnostics. No field, cache or lazily filled value is
  added to it for speed or convenience: what a file declares is read from the
  tree when asked (ARCHITECTURE.md, "Configuration and in-buffer settings").
- **A cost is answered by the API's shape.** When answering many questions
  repeats work, the API offers a value the caller asks for once and holds,
  borrowing the snapshot so it cannot outlive it, as
  `Document::declarations` does for inherited tags and properties. Never
  hidden state.
- **One rule, one place.** A new entry point and an old one answer through
  the same function; Org's rule is never written twice.
- **Consistent with its siblings.** A new operation or writer agrees with the
  ones beside it (how an empty value is written, which origin is reported).
- **Docs claim what Org does.** A comment that says what Org does is true of
  Org's manual or source, and says nothing of any caller's use.

A change that adds to the API says in its pull request which of these it was
checked against.

## Fuzzing

The fuzz targets live in `fuzz/` and use `cargo-fuzz`:

```bash
cargo +nightly fuzz run parse
cargo +nightly fuzz run roundtrip
cargo +nightly fuzz run apply
```

## Licence

By contributing, you agree that your contributions are dual licensed under
MIT OR Apache-2.0, as described in the README.
