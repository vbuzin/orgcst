# Architecture

This file explains how orgcst is built: where things live, what the code
promises, and why the bigger choices were made. It is for someone about to
read or change the code. The API itself is documented on
[docs.rs](https://docs.rs/orgcst), and the detailed grammar rules live in the
module docs next to the code they describe; this file links to them rather
than repeating them.

## What the crate promises

Four promises shape almost every decision below.

- **Lossless.** Every byte of the input is in exactly one token, so
  `root().text() == source()` for any input, malformed or not. The cost is
  that the tree must hold whitespace, blank lines and broken syntax as real
  tokens, and every construct needs a rule for who owns them.
- **Faithful.** Recognition follows `org-element`, checked by a parity
  harness against a pinned Emacs. Where the crate differs on purpose, the
  difference is written down with a fixture. The cost is that many rules are
  Org's regexps translated by hand, quirks included.
- **Infallible and bounded.** `parse` never returns an error and never
  aborts the process on valid UTF-8 below `MAX_SOURCE_LEN` (4 GiB).
  Recoverable problems become diagnostics. Nesting is capped, so hostile
  input cannot exhaust the stack. The cost is a resource limit Org does not
  have.
- **Portable.** No I/O, one dependency (`rowan`), and the same output on
  `wasm32` as on the host. The cost is that anything touching files,
  threads or time belongs to the caller.

## Bird's-eye view

```text
source: &str
  -> lexer::lex             tokens (kind + absolute range, tiling the source)
                            and line headers (start, end, indent, blank)
  -> grammar                element rules (line-first) and object rules
                            (cursor-first) emit Events:
                            Start / Finish / Token / Tombstone
  -> builder::build         green tree (lengths only; shareable)
  -> SyntaxNode::new_root   red tree (absolute ranges, parent links)
  -> ast views              Heading, Keyword, Link, ... over red nodes
```

The grammar never touches rowan's `GreenNodeBuilder` and never mutates the
token vector. It produces a flat log of events; the builder replays that log
against the tokens. This keeps grammar code free of tree plumbing and lets
the builder check tiling in one place.

A `Document` is the unit a caller holds: the source (`Arc<str>`), the
effective `ParseConfig` the tree was built under, the caller's config it was
resolved over, the root node and the diagnostics. It is an immutable
snapshot. Changing it means `apply`, which returns a new snapshot and leaves
the old one as it was.

## Code map

Paths are under `src/`. Each module's own docs are the reference for its
rules; start there once you know which module you need.

| Path | What lives there |
| ---- | ---------------- |
| `lib.rs` | `parse`, `parse_with`, the public re-exports, `MAX_SOURCE_LEN` |
| `lexer.rs` | `lex`: the byte dispatch table, tokens and line headers |
| `syntax.rs` | `OrgSyntaxKind`, `OrgLanguage`, `ObjectSet` and the object restriction sets |
| `parser.rs`, `parser/` | the transient `Parser`: cursor, events, limits, positions (`pos.rs`), views (`view.rs`), consumption (`eat.rs`), lookahead (`peek.rs`), events and markers (`event.rs`), the builder (`builder.rs`), `ParseConfig` (`config.rs`), diagnostics |
| `parser/grammar/drive/` | the walk: `document.rs` (outline spine, sections), `decide.rs` (element recognition), `region.rs` (element and object loops, the nesting budget), `frame.rs` (closed frames and the END-line index), `atoms.rs` |
| `parser/grammar/forms/` | one module per construct family: headings, planning, drawers, blocks, lists, tables, keywords, affiliated keywords, links, markup, timestamps, footnotes, citations, LaTeX, entities, and the rest |
| `ast.rs`, `ast/` | typed views: `AstNode` and the capability traits (`ast.rs`), part geometry (`layout/`), one view module per family (`views/`), navigation (`nav.rs`), edit context (`context.rs`), document queries and joins (`document/`), `PortableDocument` (`portable.rs`) |
| `document.rs` | `Document`, `materialize` (the one tree producer) and the settings round |
| `settings.rs` | in-buffer settings read off the tree and resolved over the caller's config |
| `damage.rs`, `damage/` | reuse of a prior tree: config damage (`config.rs`) and text damage at outline granularity (`outline.rs`) |
| `edit.rs` | `Edit`, `EditSet`, `Anchor`, offset mapping |
| `operation.rs`, `operation/` | typed `Operation`s and `compile` |
| `writer.rs`, `writer/` | synthesis of new Org text for operations; escaping and quoting helpers |
| `entities.rs` | the entity table and `EntityDef` |
| `tests.rs` | shared test support, including the `TAXONOMY` fixture set |

Outside `src/`: `tests/determinism.rs`, `examples/wasm_behaviour.rs`,
`benches/baseline.rs`, `fuzz/` (a separate cargo-fuzz package), `parity/`
(a separate harness package), and `scripts/wasm-check.sh`.

`document.rs` does not import `ast`, `writer` or `operation`. A snapshot
never depends on the views over it, so the view layer can grow without
touching the producer.

## The grammar dialect

Grammar code follows a small set of conventions. Each one exists to make a
class of bug impossible rather than unlikely.

**Elements are line-first.** Every element family exposes a pure probe and
an emitter:

```rust
fn probe(line: LineView) -> Option<Shape>;   // pure; Shape is Copy
fn parse(p: &mut Parser, shape: Shape);      // consumes what probe measured
```

The probe measures, and the emitter consumes exactly that measurement, so
recognition never runs twice and the two can never disagree. All element
recognition goes through one ordered chain, `drive::decide::decide`, which
returns an `Element` row: a claim with its shape, a degrade, or nothing. The
dispatch order is Org's: comment, clock, inline task, drawer, fixed-width,
block, LaTeX environment, keyword (with affiliation), footnote definition,
horizontal rule, diary sexp, table, list, and paragraph as the fallback.

**Paragraph termination is a projection.** Whether a line ends an open
paragraph is computed from `decide` (a heading, a claim, or a degrade that
still interrupts), not from a second rule set that could drift from it.

**Objects are cursor-first.** Inline recognition works on a token cursor. A
restriction-gated candidate table tries rows in Org's order; the order is
product semantics and is pinned by a test. Like elements, an object row
probes for a shape and then emits it. Where `org-element--object-lex` has a
branch rather than a fallthrough (`_` tries underline before subscript; `\\`
goes to line break and nowhere else), the row reproduces the branch.

**Positions are typed and absolute.** Grammar code holds `Pos` (a token)
and `LineId` (a line), never signed offsets. They stay valid across
consumption, because the token and line tables never change during a parse.

**Views carry their own cap.** A `LineView` or `TokenView` holds the
exclusive token bound it may see, and every view derived from it inherits
that bound. A walk cannot widen its own visibility mid-way, and nothing past
a region's end is visible to a probe inside it. `Parser::full_line_at` is
the one unrestricted constructor; it exists for the END-line index below,
whose look-ups re-apply the region limit.

**Failure is two named recipes.** `Parser::attempt` is the only way to
rewind: it restores the cursor, events, limit and inline region, and keeps
diagnostics. Fail-soft object paths abandon their marker and leave the
opener tokens as plain text. Markers are drop bombs: each must be completed
or abandoned exactly once.

**Degrade reasons are tags.** An unclosed block, drawer or LaTeX environment
becomes an `Element::Degraded` holding a `Copy` reason. The name for the
diagnostic is re-read from the line at emit, so probing allocates nothing.

**Runs cannot hang.** Consecutive-token scans use `run_while`, which stops
when the view runs out. There are no open-coded `while kind(k) == ...`
loops to forget a stop condition in.

**Closedness is amortised.** Asking "is this `#+begin_src` closed?" per
opener made paragraph termination quadratic on files full of opener-shaped
lines. `drive/frame` builds a next-END index once per parse, lazily, in one
reverse pass over the lines:

| Table | Answers |
| ----- | ------- |
| `drawer_next` | first `:END:` after a line with no heading between |
| `dynamic_next` | first `#+end:` after a line |
| `underscore_ends` | `#+end_NAME` lines, keyed by name |
| `latex_ends` | `\end{NAME}` lines, keyed by name |
| `next_heading_after` | the section bound for LaTeX environments |

Name keys are a hash of the lowercased name and serve only as a filter;
every candidate is confirmed with an exact caseless compare. The index is
built from unrestricted views even under a nested region, and look-ups drop
ends past the active limit. That is sound because ends are monotone in line
order: if the nearest end is outside the region, so is every later one.

## Invariants

Each invariant names what enforces it. "Always on" means the check also
runs in release builds.

### Output

| Invariant | Enforced by |
| --------- | ----------- |
| `root.text() == source` for every input | the lexer tiles every byte; the builder asserts gap-free coverage to `source.len()` (always on); round-trip checks across the suite and in fuzzing |
| Every event range lies in one token, on char boundaries | builder asserts with a monotonic tile cursor (always on) |
| Parsing is infallible | no fallible public path; problems become `Diagnostic`s |
| Public ranges are absolute byte offsets | red-tree projection; green nodes store lengths only |
| Sources above `MAX_SOURCE_LEN` panic at the boundary instead of truncating | an assert in `parse` and `parse_with` |
| No input exhausts the stack | the nesting budget, below |

### Nesting budget

`ParseConfig::max_nesting_depth` (default 128) bounds how many levels may be
open at once. It is a resource bound, not a grammar rule: Org has no limit,
and past the budget this crate deliberately differs. An aborted process is
worse than a flattened tree.

Two counters share the number, because there are two hazards:

| Counter | Charged by | Hazard |
| ------- | ---------- | ------ |
| `Parser::nesting` | the element and object loops in `drive::region` | Rust stack during the parse |
| `Parser::outline_nesting` | opening and closing outlines | green tree depth: rowan's `Drop` recurses, so a deep tree aborts after `parse` returns, on the caller's stack |

They are separate so that deep outlines cannot starve the region budget and
silently flatten every paragraph in a deeply outlined file.

Past the budget, a region is flattened: its tokens become plain tiles under
the open node, with one `NestingLimitExceeded` diagnostic, and the cursor
lands where the recursive walk would have left it. A headline past the
budget attaches at the ceiling instead of below it; nothing is lost, so no
diagnostic. The budget gates emission, never recognition: depth cannot
change what a line is. Green tree depth is therefore bounded by a small
multiple of the budget, which tests pin.

Only four shapes nest without bound in Org: inline footnote bodies, plain
lists, greater blocks with distinct names, and outlines. Values far above
the default re-enable stack overflow and are for tests only.

### Progress and termination

| Invariant | Enforced by |
| --------- | ----------- |
| Each element-loop iteration consumes input or falls through to paragraph | `parse_one_element` compares `Parser::progress()` |
| Each object-loop iteration advances | a release-safe backstop eats one token on a claim with no progress (debug builds also assert) |
| The outline spine uses no Rust stack | an iterative spine with a heap stack of open outlines |

### Tree construction and recognition

| Invariant | Enforced by |
| --------- | ----------- |
| Every marker is completed or abandoned once | drop bomb, in debug and release |
| No child node without its completed parent | fail paths abandon inner markers first; pinned by `*_no_orphans` tests |
| One recognition chain per layer | `decide` for elements, the candidate table for objects |
| Recognition never measures twice | probes carry the shape emit consumes; the test harness uses the same path |
| Object scopes match `org-element-object-restrictions` | `ObjectSet` bitsets built from the standard set by `with` / `without` / `union` |
| A region can only shrink | region limits clamp to the current limit on entry |

### Diagnostics

| Invariant | Enforced by |
| --------- | ----------- |
| Diagnostics survive rewind | `attempt` does not restore them |
| One issue is reported once | dedupe on `(kind, range)` at the sink |
| Hostile input cannot inflate diagnostics | string fields are truncated to 64 bytes |

### Performance guards

Three superlinear shapes were found by measurement and fixed. Each has a
deterministic guard counting work, not a timing test:

| Shape | Guard |
| ----- | ----- |
| Paragraph termination over many frame-shaped lines | `unclosed_frame_openers_frame_end_work_is_linear` |
| Many unclosed `#+begin_a` against many `#+end_b` | `mismatched_underscore_ends_do_not_scale_with_opener_count` |
| Many unclosed `\(` / `\[` openers | `math_closer_work_is_linear_for_unclosed` (a miss memo of known-empty ranges) |

Plain prose parses at tens of MB/s in release on a laptop. That figure is a
sanity anchor, not a budget.

## Structure decisions

These are the choices a reader is most likely to question. Each says what
the tree does and why.

### Single structural pass

`parse` is one pass over the source. No later pass rewrites the tree, so
the tree a caller gets is a pure function of `(text, config)`. Anything
that needs document-wide knowledge after parsing is a query over the tree,
not tree structure. The main example: prose that matches a
`<<<radio target>>>` is not a link node. Matching it requires every radio
target in the file, which would need a second pass and would make one edit
able to restructure text anywhere. Target definitions are parsed; matching
is left to a query.

### The outline

`drive::document` builds an optional zeroth section, then `OUTLINE` nodes:
`HEADING`, then optional planning, optional property drawer, a `SECTION`,
and child outlines. A headline is an unindented star run plus a space, at a
level below `inline_task_min_level`; higher star runs are inline tasks,
recognised by `decide`. Headlines exist only on the spine: `* foo` inside a
greater block is paragraph text, as in Org. Planning must follow the
heading directly; a property drawer may follow the heading or the planning
line.

### Blank-line ownership

Blank lines are ordinary tokens, and ownership is parenthood:

| Region | Owner of the blank lines |
| ------ | ------------------------ |
| File lead-in | `DOCUMENT` |
| After heading, planning or property drawer, before the body | `OUTLINE` |
| After an element's last content line | that element |
| Between list items (one blank) | the previous `ITEM` |
| After a list's last item, including the ending double blank | `PLAIN_LIST` |
| Lead-in inside a container body | the container |

There are no blank-only paragraphs, and an element never absorbs blank
lines before it. This mirrors Org's `:post-blank`: an editor that deletes an
element deletes the blank lines Org considers part of it. One exception,
also Org's: a `\\` line break owns its trailing spaces and the newline.

### Affiliated keywords are structure

Org stores `#+NAME:`, `#+CAPTION:` and the rest as properties of the
element below them. Here a run of such lines that attaches becomes one
`AFFILIATED_KEYWORDS` node, the previous sibling of its target:

```text
SECTION
  AFFILIATED_KEYWORDS        emitted only when the run attaches
    KEYWORD   #+NAME: fig1
    KEYWORD   #+CAPTION: A *cap*
  PARAGRAPH                  the target; its range is unchanged
```

The run attaches unless a blank line, comment, clock line, star line, EOF
or a container end follows it; then there is no run node and each line is
a plain `KEYWORD` owning its blank lines. Attachment is decided once, in
`decide`: `keyword::probe` reads one line, and `affiliated::try_run` peeks
forward. Keeping attachment in the tree means a consumer asks "does this
element have a caption?" by looking at its previous sibling
(`ElementView::affiliated`), with no second parse, and an orphaned caption
can never be read as anyone's caption. The module docs of
`forms/affiliated.rs` list the run's invariants.

### Configuration is applied at parse time, and the list is closed

Some settings change what the tree is: TODO keywords decide whether a
heading's first word is a keyword, link types decide what a plain link is.
These are applied while parsing, not patched on afterwards, so a tree is
always internally consistent with the config it records.

The places the grammar reads `ParseConfig` are a closed, enumerated set:

| Knob | Read in |
| ---- | ------- |
| TODO vocabulary | `forms/heading.rs` |
| link types | `forms/link.rs` |
| user entities | `forms/entity.rs` |
| `inline_task_min_level` | `forms/heading.rs`, `drive/document.rs`, `drive/decide.rs` |
| alphabetical list bullets | `forms/list.rs` |
| `max_nesting_depth` | `drive/region.rs`, `drive/document.rs` |

This table is the input to change detection: when config changes, only
constructs in this table can be damaged. A new construct that reads
`ParseConfig` adds a row in the same change. A missing row is a correctness
bug, not a documentation lapse.

## Configuration and in-buffer settings

`ParseConfig` holds only the knobs above. It is built from `default()` and
`with_*` builders; there are no setters. Its `Eq` asks whether two configs
are the same value. A crate-internal delta asks what the grammar would
build differently. Both read one private diff that destructures every
field, so a new knob is a compile error until it is classified for both.

A document can declare settings. Only those that change the tree's shape
enter the effective config, and today that is the TODO vocabulary:
`#+TODO:`, `#+SEQ_TODO:` and `#+TYP_TODO:` replace the caller's vocabulary.
Everything else a document declares (`#+PRIORITIES:`, `#+TAGS:`,
`#+FILETAGS:`, `#+PROPERTY:`, `#+LINK:` abbreviations) is read from the
tree on demand by `Document` methods and never enters `ParseConfig`. So
changing a priority range is not damage.

Settings are read from the parsed tree, never from a separate line scanner.
That makes liveness a matter of tree shape: a directive inside a lesser
block is body text and never became a node, one inside a greater block did.
There is no second scanner to disagree with the parser.

`Document::config()` is the effective config the tree was built under. The
caller's layer is kept privately, because the in-buffer vocabulary replaces
the caller's and so cannot be recovered from the effective config, and a
re-resolve after an edit must layer over the same base.

## Edits and incremental reparse

### One producer

Every snapshot comes from one function:

```text
materialize(text, config, prior: Option<&Document>) -> Document
```

`parse`, `parse_with` and `apply` all go through it. Text changes and
config changes are two sources of damage handled by one model, not two
APIs. `prior` is a hint and may never change the result: materializing with
a prior is byte-identical, tree and diagnostics, to materializing without
one. The cold path is the permanent oracle and fallback.

Settings need a tree before they can be read, so the producer guesses the
effective config (the prior's, or the caller's on a fresh parse), parses
under the guess, reads the settings from that tree, and compares. A
confirmed guess is the result. A wrong guess becomes the prior of a
config-damage step. No parse is thrown away, and the result never descends
from a scratch tree.

### Edits

An `Edit` replaces a byte range of the old text. An `EditSet` is an
ordered, non-overlapping set of edits in pre-edit offsets;
`EditSet::new` checks bounds, char boundaries, order and overlap.
`Document::apply` splices the text, re-resolves settings from the new text,
and materializes with the old document as prior. Edits are the mutation
currency because text is canonical and the tree is derived from it; any
higher-level change, typed operation or keystroke, ends as an `EditSet`.

### Reuse

Two reuse paths live in `damage/`. Neither changes the result.

- **Same text, new vocabulary** (`damage/config.rs`). With equal TODO name
  sets, the prior green tree is reused as is. Otherwise a heading is
  damaged only if the first word on its star line changes membership
  between the old and new name sets; each damaged star line is re-lexed and
  spliced, and bodies are not touched. A change to any other knob is a
  cold parse.
- **New text, same vocabulary** (`damage/outline.rs`). The touched span is
  the common prefix and suffix of the old and new text, so several edits
  count as one span. The reparse unit is the smallest outline covering that
  span, or the zeroth section for an edit before the first heading. A star
  line added, removed or re-levelled in the span widens the unit to an
  ancestor shallower than every affected level. The unit is reparsed as a
  fragment on the new text through EOF, so a closer past the boundary is
  visible; if its extent does not end on the mapped boundary, the parent is
  tried, then a cold parse. Sibling outlines outside the unit keep the same
  green nodes. Diagnostics are dropped, shifted and spliced so the result
  matches a cold parse, order included.

When an edit also changes the vocabulary, the producer takes two steps:
text damage under the old config, then config damage. New text together
with a name-set change in one step is a cold parse. Tests pin how many cold
trees each case builds, so reuse cannot silently degrade to full reparses.

## Writer and operations

`Document::write()` returns the source, byte for byte, for every snapshot.
There is no pretty-printer. Byte order marks, `\r\n` and a missing final
newline survive.

New text comes from two places:

- **Verbatim.** `Document::slice` and `AstNode::text` copy existing parts.
- **Synthesis.** Crate-internal `render_*` functions in `writer/` produce
  canonical Org for new material. Synthesis never emits a byte order mark,
  ends new lines with one `\n`, and writes timestamps as `YYYY-MM-DD` with an
  English weekday.

Public helpers expose the writer's rules without its internals:
`is_valid_tag_name`, `title_problem`, `is_valid_property_key`,
`quote_lesser_body` and `unquote_lesser_body`, `escape_link_path`.

Typed changes are `Operation`s:

```text
Operation --compile--> EditSet --apply--> Document
           (fallible)            (infallible once the EditSet exists)
```

All validation happens in `compile`: unknown TODO keywords, priority
cookies outside the declared range, tag characters, property keys,
timestamps, promotion limits, stale or wrong-kind targets. `apply` of a
valid `EditSet` is a splice and a materialize. A `NodeRef` names a target by
anchor and kind, and is re-resolved on the new snapshot. `compile_all`
compiles several independent operations against one snapshot and merges
them, rejecting conflicting inserts. `Operation` is `#[non_exhaustive]`
because the catalogue grows; raw `EditSet`s stay public for keystrokes and
pastes.

## Typed views

Views are zero-cost newtypes over red nodes. `ast.rs` holds `AstNode` and
five capability traits (`HasParts`, `ElementView`, `HasElements`,
`HasObjects`, `Headline`). Every part of a construct has byte-exact spans
(construct, content, removal, full), and every part that is absent has an
`InsertPoint`, so an editor can always say where a missing piece would go.
Four exhaustive enums (`Element`, `Object`, `Container`, `Task`) let a
consumer match on everything a position can hold.

Facts that depend on context, such as a heading's TODO kind or an inherited
property, live on `Document` and say where they came from. The crate states
facts; applying a user option such as tag inheritance is the caller's
choice.

Censuses keep the views complete: every node kind is emitted by the
`TAXONOMY` fixtures or listed in `syntax::NEVER_EMITTED` with a reason, and
every kind has a view or a `NO_VIEW` reason (`ast/container_matrix.rs`).

## Threads and portability

A red `Document` is not `Send`: rowan's red nodes are reference-counted
without atomics. `PortableDocument` carries a snapshot across threads as
five `Send + Sync` parts (source, green tree, effective config, caller
config, diagnostics) and rebuilds the red root on the other side. There is
no `unsafe impl Send` and no lock.

The same text and config give the same tree on any thread and on wasm32.
`tests/determinism.rs` parses and applies on eight threads and compares a
preorder walk of kinds, ranges and diagnostics.
`examples/wasm_behaviour.rs` prints the same walk, and
`scripts/wasm-check.sh` runs it on the host and on `wasm32-wasip1` and diffs
the output. `cargo check --target wasm32-unknown-unknown` only typechecks
the library; it is not a substitute.

The library has no serialisation format and no `serde` feature. An owned
projection is the job of whoever binds the crate to another language.

## Testing

- **Unit tests** sit next to the code. Grammar tests use the same
  probe-then-emit path as production (`run_element`, `run_object` in
  `tests.rs`), so no test exercises a path production lacks.
- **Censuses** over `TAXONOMY` gate node kinds, views, and enum coverage.
- **Fuzzing** (`fuzz/`, nightly, `cargo-fuzz`): `parse` checks round trip
  and range nesting, `apply` checks edit validation and splicing, and
  `roundtrip` checks that a cold parse of the applied text matches `apply`.
  `fuzz/fuzz.sh` runs all three. The fuzz package is outside the crate's
  workspace, so `libfuzzer-sys` never enters the library's graph.
- **Parity** (`parity/`, Docker): `parity/parity.sh` parses the corpus with
  this crate and with `org-element` from a pinned Org commit
  (`parity/ORG_VERSION`) in a pinned image, and diffs projections of the two.
  `corpus/differential` and `corpus/realistic` must match;
  `corpus/diverge` holds the known differences.
- **Benches** (`benches/baseline.rs`, criterion, host only) are baselines,
  not a gate.

## Differences from org-element

The list of differences is `parity/exceptions.json`, mirrored in the README
table. Each entry has a fixture under `parity/corpus/diverge/<id>/` with an
`input.org` and a `reason.txt` whose first line is the id. The harness
fails if an entry has no fixture.

Each entry's `cites` field names why the difference exists:

| `cites` | Meaning |
| ------- | ------- |
| `design` | deliberate: one tree shape for editing, a query instead of structure, or a resource bound |
| `non-goal` | out of scope (table.el tables) |
| `known bug` | unintended; a fix is welcome |
| a test name | deliberate, and pinned by that test |

A difference moves from `known bug` to `design` only with a reason in its
`reason.txt`. A fixture whose trees start to agree fails the harness as a
stale exception, so the list cannot outlive the differences it describes.
