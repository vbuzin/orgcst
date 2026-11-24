# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0,
a minor release may break the API.

## [Unreleased]

### Fixed

- `WrapInElement` and `SetElementName` refuse a result whose block would not
  keep the elements it was given: a verse holds only text, so a list or a
  block wrapped in one, or a quote holding them renamed to one, is
  `NotAllowedHere`.

### Added

- `Operation::ExtendElement`: the element right after a block or a drawer
  made part of it, its end line moved past that element, code escaped as
  it comes into a src, example or export block. Its edit set touches only
  what changes, so an offset in the element taken in maps to where it is.
- `Block::interior_range`: every line between a block's begin and end
  lines, blank lines included, as `Drawer::body_range` is for a drawer.

## [0.2.0] - 2026-11-24

### Added

- `Block::block_name()`: the name a block is known by, Org's own for its
  seven kinds in lower case, else the name written for a special or
  dynamic block.
- `Document::element_lines()` and `ElementInput`: the lines a block, a
  drawer, a plain list, a table (`TableRowInput`) or a horizontal rule is
  written as, by Org's names, checked by reading them back, for a caller
  that places them itself. Code in a src, example or export block is
  escaped; Org text that would hold a heading or end its block is refused.
- `Operation::WrapInElement`, `Operation::UnwrapElement` and
  `Operation::SetElementName`: a run of sibling elements made the contents
  of a new block or drawer, a block or drawer taken out again keeping its
  contents, and a block or drawer renamed, its contents escaped or
  unescaped when it moves between code and Org text.
- `timestamp()`: a timestamp as Org writes one, from a `TimestampInput`,
  the text the operations that set a timestamp write.
- `Block::value()`: the body of a src, example or export block with its
  comma escaping undone, as `org-element` gives a block's `:value`;
  `unescape_code()` and `escape_code()` are Org's
  `org-unescape-code-in-string` and `org-escape-code-in-string`.
- `Operation::InsertKeyword`: a keyword line written at the head of the
  file, after its own property drawer and the keyword lines that lead it.
- `Document::heading_line()`: the headline line `Operation::InsertHeading`
  would write, checked the same way, for a caller that places the line
  itself.
- `PriorityAtom::from_cookie_value()`: the atom a `[#s]` cookie value names.
- `tag_group()`: a tag group as Org writes one, `:a:b:`.
- `LinkSearch`, with `LinkSearch::parse()`: a search inside a file read by
  its marker (`*` a heading, `#` a custom id, `(name)` a code reference,
  `/regexp/`, digits a line), as Org's `org-link-search` reads it, and
  written back by its `Display`; `file_link_path()` writes a `file:` link's
  path with its search.
- `Operation::InsertObject`, `Operation::WrapInObject` and
  `Operation::UnwrapObject`, with `ObjectInput`: a link, a radio target or a
  target written from Org's names at an offset or around a range, and
  removed again keeping its words. Each insert is checked against Org's
  object restrictions and by a trial parse; refusals are the new
  `OperationError::NotAllowedHere { kind, scope }` and
  `OperationError::CrossesObject`.
- `Operation::SetProperty` on a target of kind `DOCUMENT` edits the file's
  own property drawer, writing a new one at the start of the file.
  `Document::file_property_drawer()` and
  `Document::file_property_drawer_insert_point()` read it.
- `Document::radio_links()`: the text that matches a radio target of the
  file, as Org's syntax reads radio links, with `RadioLink`.
  `RadioTarget::key_range()` and `Target::name_range()`.
- `KeywordValue::words()`: a keyword's value split on ASCII whitespace, as
  Org's `split-string` splits a setting, each `ValueWord` with its text and
  range. The words carry no meaning; what one does is the caller's.

### Removed

- `StatisticsCookie::cookie_data()` and `CookieData`, deprecated in 0.1.5:
  what a property means is the caller's. Read `COOKIE_DATA` from the host
  task's property drawer instead.
- `quote_lesser_body()` and `unquote_lesser_body()`: the same rule as
  `escape_code()` and `unescape_code()`, which are now the one place it
  is written.

### Changed

- `escape_code()` and `unescape_code()` end a line where the parser
  does: at `\r\n`, `\r` or `\n`.

### Fixed

- A block (`#+begin_NAME` or `#+BEGIN:`) no longer ends past the next
  headline: Org finds headlines before anything inside them, so a block
  whose end line lies beyond one is unterminated and its opener reads as
  a paragraph, as Emacs reads it. Checked by the parity corpus
  (`heading-in-block`, `heading-in-dynamic-block`).


## [0.1.4] - 2026-11-12

### Fixed

- A headline's tags must end the line, as in Org: `* a :b: c` is all title,
  and in `* a :b: :c:` only `:c:` is tags. A tag group followed by more text
  was read as tags. `title_problem` no longer refuses a title holding such a
  group, since it reads back as title.

## [0.1.3] - 2026-11-11

### Added

- `Operation::SplitHeading`: the title after an offset becomes the title
  of a new heading at the same level after the subtree; the heading keeps
  its own text and children.
- `Operation::InsertSubtree`: Org text holding one or more subtrees is
  inserted at a `SubtreeDest`, every heading shifted by the same number of
  levels so the shallowest lands at the destination's level.
- `Operation::ReorderChildren`: an outline's direct children reordered by
  a permutation, each with its subtree and trailing blank lines.
- `Operation::ConvertToText`: a heading's title becomes a paragraph where
  the headline was, its metadata is dropped and its descendants are
  promoted one level; a title that would not read back as a paragraph is
  refused.
- `relevel_text`: Org text with every heading shifted so the shallowest is
  at a given level, read under a document's configuration.
- `OperationError::InvalidOffset`, `InvalidSubtree`, `InvalidOrder` and
  `InvalidText` for the new operations.

## [0.1.2] - 2026-11-08

### Fixed

- A missing headline prefix is inserted where Org reads it. Org reads the
  prefixes only in the order keyword, priority, `COMMENT`, but the insert
  point of every missing one was the title's start, so `SetTodoState` on
  `* [#A] Title` wrote `* [#A] TODO Title` (a title starting with `TODO`)
  and `SetPriority` on `* COMMENT Title` wrote `* COMMENT [#A] Title`. A
  missing prefix now goes before the first later prefix that is present:
  `* TODO [#A] Title`, `* [#A] COMMENT Title`.
- An insert at the title's start after a prefix with no space behind it
  (`* TODO`, an empty title) leads with a space, so `SetPriority` writes
  `* TODO [#A]` instead of `* TODO[#A]`.
- Typing or deleting a star in a headline re-nests it at once. The
  incremental reparse counted the star levels of the touched lines on both
  sides, but an empty old span (a pure insertion) counted none, so the
  widening stopped at the outline that was moving: after `** T` became
  `*** T` the heading stayed its parent's sibling until a full parse.

## [0.1.1] - 2026-10-26

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

## [0.1.0] - 2026-10-23

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

[Unreleased]: https://github.com/vbuzin/orgcst/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/vbuzin/orgcst/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/vbuzin/orgcst/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/vbuzin/orgcst/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/vbuzin/orgcst/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vbuzin/orgcst/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vbuzin/orgcst/releases/tag/v0.1.0
