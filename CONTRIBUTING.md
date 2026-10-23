# Contributing

Thanks for your interest. Bug reports, Org snippets that parse wrongly, and
pull requests are all welcome.

## Reporting a parse bug

The most useful report is a short Org snippet, what this crate produces, and
what Emacs Org produces (`M-x org-element-parse-buffer`, or the parity harness
below). Please say which Org version you compared against.

## Before you open a pull request

Run the same checks CI runs:

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
say so in the pull request.

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
