# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0,
a minor release may break the API.

## [Unreleased]

### Fixed

- Plain links, inline babel calls (`call_`) and inline source blocks
  (`src_`) start wherever Org's word-start rule allows: after punctuation,
  quotes and brackets, not only after whitespace. `(https://example.com)`,
  `"https://example.com"`, `see:https://example.com` and `(call_f(1))` now
  parse as Org parses them, as does the plain link inside a rejected angle
  link (`<https://x` with `>` starting the next line).
- A plain link's path follows Org's rule for where it ends. Trailing
  punctuation other than `-` and `/` stays outside the link
  (`https://x.com/a*` is `https://x.com/a`), any of `<([` with any of `])>`
  makes a group (`a[1]`, `a(b]`), an unclosed or too-deep group ends the path
  instead of rejecting the link, and a path needs at least two characters.
- `SetLinkPath` on a plain link and `ConvertLinkShape` to a plain link refuse
  a path that would not read back as the whole link, instead of writing a
  shorter one.

## [0.1.0] - unreleased

First public release.

### Added

- Lossless rowan CST parser for Org mode, covering the Org syntax
  specification: outlines, headings, inline tasks, planning, clocks, drawers
  and property drawers, blocks, LaTeX environments and fragments, keywords and
  affiliated keywords, babel calls, plain lists, tables, footnotes, links,
  citations, timestamps, markup, entities, macros, targets, export snippets
  and inline source blocks.
- Diagnostics for recoverable problems; parsing is infallible.
- In-buffer `#+TODO:`, `#+SEQ_TODO:` and `#+TYP_TODO:` vocabularies, plus a
  caller-supplied `ParseConfig`.
- Typed views over the tree, with ranges and insertion points for every part.
- Document queries: TODO state, priorities, tags with inheritance, properties
  with inheritance, macros, link abbreviations, footnotes, IDs.
- `Edit` and `EditSet`, `Document::apply`, and incremental reparse at outline
  granularity.
- Typed `Operation`s that compile to an `EditSet`.
- `PortableDocument` for moving a snapshot across threads.

[Unreleased]: https://github.com/vbuzin/orgcst/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/vbuzin/orgcst/releases/tag/v0.1.0
