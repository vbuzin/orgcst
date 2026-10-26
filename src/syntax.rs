//! The syntax vocabulary: `OrgSyntaxKind` and the `OrgLanguage` binding.
//!
//! This is the single, flat set of kinds the whole pipeline speaks. rowan stores
//! a homogeneous tree of `(kind, children)` nodes and `(kind, text)` tokens; there
//! is no per-node payload type. Typed structure (a headline's level, its tags) is
//! recovered by the AST view layer (`crate::ast`), never stored on the node.
//!
//! Two disjoint families live in one enum, exactly as rust-analyzer keeps them:
//!
//! - **Node kinds** (internal) - the structural constructs the parser builds.
//!   The set mirrors Org's element and object taxonomy
//!   (`org-element-all-elements` and `org-element-all-objects` in
//!   `org-element.el`). The parser emits every node kind except `ERROR`, which
//!   is reserved: parsing never fails, so nothing is ever wrapped in it. A
//!   crate test checks this against a corpus covering every construct.
//! - **Token kinds** (leaves) - what the lexer emits and the builder writes as
//!   text. Every byte of the source lands in exactly one token, so
//!   `node.text() == source[node.range()]` holds for every node.
//!
//! ## Tokens
//!
//! The lexer is context-free apart from one bit of line awareness (line-start
//! whitespace becomes `INDENT`, not `WHITESPACE`). Every byte Org gives inline or
//! structural meaning to is its own single-character token (`STAR`, `COLON`,
//! `L_BRACKET`, ...); runs of ordinary bytes are `TEXT`. This follows orgize
//! and org-element's two-phase model: the lexer supplies atoms and line
//! structure, and deciding whether `*foo*` is really bold is the parser's job.
//! `-` and `.` are not delimiters: they matter only at line start (bullets,
//! rules, ordered counters), and keeping them in `TEXT` avoids splitting prose
//! like `well-known` or `3.14`.

use rowan::SyntaxKind;

/// Every kind the lexer, parser, builder, and AST layer share.
///
/// `#[repr(u16)]` so a kind round-trips through `rowan::SyntaxKind(u16)` by
/// transmute. Layout contract: **all node kinds first, then all token kinds, then
/// `__LAST`**. `is_token` relies on the token block being the contiguous tail, and
/// `kind_from_raw` asserts against `__LAST`. Keep new tokens together at the end.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum OrgSyntaxKind {
    // ===== Nodes: document & heading structure =============================
    /// The document root sentinel. Always the green tree's root node.
    DOCUMENT = 0,
    /// The body under a headline (or before the first one): a run of elements.
    SECTION,
    /// A heading with its metadata (planning, properties), section and
    /// sub-outlines.
    OUTLINE,

    /// A heading line: `STARS [KEYWORD] [PRIORITY] [COMMENT] TITLE [TAGS]`.
    HEADING,
    /// The TODO keyword of a headline, such as `TODO` or `DONE` (org-element
    /// `:todo-keyword`). Only keywords in the effective vocabulary are carved.
    TODO_KEYWORD,
    /// The priority cookie of a headline, such as `[#A]`.
    PRIORITY,
    /// The `COMMENT` keyword that marks a headline as commented.
    HEADING_COMMENT,
    /// The title of a headline. Always present, possibly empty; contents are
    /// objects.
    HEADING_TITLE,
    /// The tag group at the end of a headline, as in `:work:urgent:`. The tags
    /// themselves are tokens.
    TAGS,

    /// An inline task: star count at/above `org-inlinetask-min-level`, optional
    /// END line, title anatomy shared with [`OrgSyntaxKind::HEADING`].
    INLINE_TASK,
    /// A `SCHEDULED:` / `DEADLINE:` / `CLOSED:` line under a headline (OUTLINE
    /// metadata) or a closed inline task.
    PLANNING,
    /// A `CLOCK:` line with an (inactive) timestamp and optional duration.
    CLOCK,
    /// A `:PROPERTIES:` ... `:END:` drawer under a headline/planning (OUTLINE
    /// metadata) or a closed inline task.
    PROPERTY_DRAWER,
    /// One `:KEY: VALUE` line inside a property drawer.
    NODE_PROPERTY,
    /// The key name portion of a node property (between the colons; the raw
    /// key including a trailing `+` on additive properties).
    NODE_PROPERTY_KEY,
    /// The value portion of a node property (whitespace-trimmed).
    NODE_PROPERTY_VALUE,
    /// A generic `:NAME:` ... `:END:` drawer.
    DRAWER,
    /// The name portion of a drawer opener line (between the colons; also
    /// emitted for the `PROPERTIES` opener of a property drawer).
    DRAWER_NAME,

    // ===== Nodes: lists ======================================================
    /// A plain list: a maximal run of items at one indentation.
    PLAIN_LIST,
    /// One list item, including its bullet/counter, checkbox, tag, and contents.
    ITEM,
    /// The bullet marker of an item (`-`, `+`, indented `*`, `N.`, `N)`).
    BULLET,
    /// A counter set cookie after the bullet: `[@5]` / `[@start:5]` / `[@c]`.
    COUNTER,
    /// An item checkbox: exactly `[ ]`, `[X]`, or `[-]`.
    CHECKBOX,
    /// The descriptive-item tag (before ` :: `); contents are objects.
    ITEM_TAG,

    // ===== Nodes: greater blocks & lesser blocks =============================
    /// `#+BEGIN_CENTER` ... `#+END_CENTER` (org-element `center-block`);
    /// contents are elements.
    CENTER_BLOCK,
    /// `#+BEGIN_QUOTE` ... `#+END_QUOTE` (org-element `quote-block`); contents
    /// are elements.
    QUOTE_BLOCK,
    /// `#+BEGIN_<name>` ... `#+END_<name>` with an unrecognised name.
    SPECIAL_BLOCK,
    /// `#+BEGIN: <name> ...` ... `#+END:` (dynamic block).
    DYNAMIC_BLOCK,
    /// `#+BEGIN_COMMENT` ... `#+END_COMMENT` (org-element `comment-block`);
    /// contents are verbatim.
    COMMENT_BLOCK,
    /// `#+BEGIN_EXAMPLE` ... `#+END_EXAMPLE` (org-element `example-block`);
    /// contents are verbatim.
    EXAMPLE_BLOCK,
    /// `#+BEGIN_EXPORT backend` ... `#+END_EXPORT` (org-element
    /// `export-block`); contents are verbatim.
    EXPORT_BLOCK,
    /// `#+BEGIN_SRC lang` ... `#+END_SRC` (org-element `src-block`); contents
    /// are verbatim.
    SRC_BLOCK,
    /// `#+BEGIN_VERSE` ... `#+END_VERSE` (org-element `verse-block`); contents
    /// are objects.
    VERSE_BLOCK,

    // ===== Nodes: tables =====================================================
    /// An Org table (org-element `table`): its rows, then any directly
    /// following `#+TBLFM:` lines.
    TABLE,
    /// A standard table row (`| ... |`): contents are `TABLE_CELL`s.
    TABLE_ROW,
    /// A rule row (`|---+---|`): no cells.
    TABLE_RULE_ROW,
    /// A table cell - also an object container (its contents are objects).
    TABLE_CELL,

    // ===== Nodes: other elements =============================================
    /// Fallback element: a run of objects; content ends before a blank line;
    /// trailing blank lines are post-blank token children of the paragraph.
    PARAGRAPH,
    /// A footnote definition, `[fn:label] contents` at the start of a line
    /// (org-element `footnote-definition`).
    FOOTNOTE_DEFINITION,
    /// A `# ...` line comment (or a run of them).
    COMMENT,
    /// A `: ...` fixed-width area.
    FIXED_WIDTH,
    /// Five or more hyphens on their own line.
    HORIZONTAL_RULE,

    /// A `#+KEY: VALUE` keyword line.
    KEYWORD,
    /// `#+TODO:` / `#+SEQ_TODO:` /  `#+TYP_TODO:` / `#+PRIORITIES:`
    SETTING_KEYWORD,
    /// A `#+CALL: ...` babel call line.
    BABEL_CALL,
    /// A run of `#+KEY: VALUE` lines that affiliates with the element below it.
    ///
    /// Emitted only when the run attaches: one or more `KEYWORD` children, no
    /// blank line inside or after it, and always followed by the element it
    /// affiliates with. A run cut off by a blank, comment, clock or star line,
    /// by the end of input, or by the end of its container is not wrapped:
    /// its lines stay plain `KEYWORD` elements.
    AFFILIATED_KEYWORDS,

    // Parts of a keyword line, the same for every keyword. A keyword in an
    // affiliated run has the same shape, under an AFFILIATED_KEYWORDS parent.
    /// The key name portion of a `#+KEY` line (under KEYWORD/SETTING/BABEL).
    KEYWORD_KEY,
    /// Optional `[...]` dual (under keyword line; content may be inlined for CAPTION).
    KEYWORD_DUAL,
    /// Value portion after `:` (flat tokens or inlined objects for CAPTION).
    KEYWORD_VALUE,

    /// Babel call name (under BABEL_CALL / INLINE_BABEL_CALL). May be empty on
    /// the element form (`#+call:` alone) for lossless tiling.
    CALL_NAME,
    /// Optional `[...]` inside header; delimiters included; interior opaque.
    CALL_INSIDE_HEADER,
    /// `(...)` argument list; delimiters included. An empty `()` is kept as a
    /// node; org-element's collapse of it to nil (`org-string-nw-p`) is left
    /// to the typed views.
    CALL_ARGUMENTS,
    /// End header: element = rest of line after name/inside/args; inline = paired `[]`.
    CALL_END_HEADER,

    /// Language name under INLINE_SRC_BLOCK (no delimiters; may be multi-token).
    SRC_LANGUAGE,
    /// Optional `[...]` parameters under INLINE_SRC_BLOCK; delimiters included;
    /// interior opaque. An empty `[]` is kept as a node, as for
    /// CALL_INSIDE_HEADER and CALL_ARGUMENTS.
    SRC_PARAMETERS,
    /// Required `{...}` body under INLINE_SRC_BLOCK; delimiters included;
    /// interior opaque (no nested objects). An empty `{}` is kept as a node.
    SRC_BODY,

    /// Diary s-expression (`%%(...)`), as an element or inside a timestamp.
    ///
    /// The two positions have different extents:
    ///
    /// | Position | Parent | Extent |
    /// | -------- | ------ | ------ |
    /// | Lesser **element** | `SECTION` / element body (e.g. footnote definition) | Full line: `%%`, body, line end, then trailing blank lines |
    /// | Timestamp **child** | `TIMESTAMP` | Interior `(...)` only; the leading `%%` stays under `TIMESTAMP` |
    ///
    /// Tell them apart by parent. The element form is any line starting with
    /// `%%(` at column 0 and is never diagnosed; the timestamp form reports
    /// [`crate::DiagnosticKind::UnbalancedDiarySexp`] when the parentheses do
    /// not balance.
    DIARY_SEXP,
    /// A `\begin{...}` ... `\end{...}` LaTeX environment.
    LATEX_ENVIRONMENT,

    // ===== Nodes: objects (inline markup) ====================================
    /// `*bold*` (org-element `bold`).
    BOLD,
    /// `/italic/` (org-element `italic`).
    ITALIC,
    /// `_underline_` (org-element `underline`).
    UNDERLINE,
    /// `=verbatim=` (org-element `verbatim`); contents are plain text.
    VERBATIM,
    /// `~code~` (org-element `code`); contents are plain text.
    CODE,
    /// `+strike-through+` (org-element `strike-through`).
    STRIKE_THROUGH,
    /// `_x`, `_{x}` or `_(x)` after a base character (org-element
    /// `subscript`); starts at the `_`.
    SUBSCRIPT,
    /// `^x`, `^{x}` or `^(x)` after a base character (org-element
    /// `superscript`); starts at the `^`.
    SUPERSCRIPT,
    /// A `\name` / `\name{}` character entity.
    ENTITY,
    /// LaTeX fragment: `\(...\)`, `\[...\]`, `\NAME*` with optional `[]`/`{}`
    /// args, `$...$`, or `$$...$$`.
    LATEX_FRAGMENT,
    /// `@@backend:value@@`.
    EXPORT_SNIPPET,
    /// `[fn:label]` / `[fn:label:def]` / `[fn::def]`.
    FOOTNOTE_REFERENCE,
    /// `call_name(args)` inline babel call.
    INLINE_BABEL_CALL,
    /// `src_lang{body}` / `src_lang[headers]{body}` inline source block.
    INLINE_SRC_BLOCK,

    /// Written link: `[[path][desc]]`, `<proto:...>`, or plain `type:path`.
    LINK,
    /// The raw address under [`Self::LINK`], without the bracket or angle
    /// delimiters.
    LINK_PATH,
    /// The description of a bracket link, between the second pair of brackets
    /// (delimiters excluded); contents are objects.
    LINK_DESCRIPTION,

    /// `<<<radio target>>>`.
    RADIO_TARGET,
    /// `<<target>>`.
    TARGET,
    /// `{{{name(args)}}}`.
    MACRO,
    /// `[n/m]` or `[p%]`.
    STATISTICS_COOKIE,

    /// `<...>` active or `[...]` inactive timestamp (and ranges).
    TIMESTAMP,
    /// YYYY-MM-DD portion (under TIMESTAMP).
    DATE,
    /// HH:MM (or shorthand range HH:MM-HH:MM) (under TIMESTAMP).
    TIME,
    /// Classified shorthand time range (under TIMESTAMP).
    TIME_RANGE,
    /// Repeater (+/++/ .+) or delay (-/--) (under TIMESTAMP).
    ROD,

    /// `[cite:@key]` citation.
    CITATION,
    /// One `@key` reference inside a citation.
    CITATION_REFERENCE,
    /// A `\\` line break.
    LINE_BREAK,

    // ===== Nodes: error recovery =============================================
    /// Reserved for unparseable input. The parser never emits it: anything it
    /// cannot recognise is kept as text.
    ERROR,

    // ===== Tokens: whitespace & line structure ==============================
    // (Token block starts here; everything below is lexer-emitted. Keep contiguous.)
    /// A run of spaces/tabs *not* at line start.
    WHITESPACE,
    /// A run of spaces/tabs at the *start of a line*: indentation. **Not**
    /// trivia: Org list and block structure is defined by indentation, so the
    /// parser must see it.
    INDENT,
    /// `\n`, `\r\n`, or a lone `\r`. **Not** trivia: Org structure depends on
    /// line boundaries.
    NEWLINE,

    // ===== Tokens: text ======================================================
    /// A maximal run of ordinary bytes (no delimiter, whitespace, or newline). The
    /// catch-all leaf: words, numbers, `-`, `.`, and all multi-byte UTF-8 text.
    TEXT,

    // ===== Tokens: single-char delimiters ===================================
    /// `*` - emphasis (bold), headline marker, or list bullet.
    STAR,
    /// `/` - emphasis (italic).
    SLASH,
    /// `_` - emphasis (underline) or subscript marker.
    UNDERSCORE,
    /// `=` - verbatim marker.
    EQUALS,
    /// `~` - code marker.
    TILDE,
    /// `+` - emphasis (strike-through), list bullet, or `#+` keyword lead.
    PLUS,
    /// `:` - tags, drawers, fixed-width, planning, timestamps.
    COLON,
    /// `#` - comments, keywords, blocks (`#+`).
    HASH,
    /// `|` - table column separator.
    PIPE,
    /// `[` - links, footnotes, timestamps, cookies, citations.
    L_BRACKET,
    /// `]`.
    R_BRACKET,
    /// `{` - macros, inline src/babel bodies.
    L_BRACE,
    /// `}`.
    R_BRACE,
    /// `<` - active timestamps, angle links, targets.
    L_ANGLE,
    /// `>`.
    R_ANGLE,
    /// `(` - macro/babel arguments.
    L_PAREN,
    /// `)` - macro/babel arguments, ordered-list counter terminator.
    R_PAREN,
    /// `\` - entities, LaTeX, line breaks.
    BACKSLASH,
    /// `@` - export snippets (`@@`).
    AT,
    /// `$` - LaTeX fragments.
    DOLLAR,
    /// `^` - superscript marker.
    CARET,
    /// `%` - statistics cookies, diary s-expressions.
    PERCENT,

    /// Parser sentinel for looking behind the first token. Never produced by
    /// the lexer or placed in the tree; it lets token inspection return a
    /// plain `OrgSyntaxKind` instead of an `Option`. `is_token` reports it as
    /// a token.
    BOF,

    /// Parser sentinel for looking past the end of input. Never produced by
    /// the lexer or placed in the tree; it lets token inspection return a
    /// plain `OrgSyntaxKind` instead of an `Option`. `is_token` reports it as
    /// a token.
    EOF,

    /// Assertion sentinel; must stay last. Not a real kind.
    __LAST,
}

impl OrgSyntaxKind {
    /// True for leaf (lexer-emitted) kinds. Relies on the layout contract: tokens
    /// are the contiguous block from `WHITESPACE` up to (but not including)
    /// `__LAST`.
    #[must_use]
    pub fn is_token(self) -> bool {
        let raw = self as u16;
        raw >= Self::WHITESPACE as u16 && raw < Self::__LAST as u16
    }

    /// True for internal (grammar-built) kinds: everything before the token block.
    #[must_use]
    pub fn is_node(self) -> bool {
        (self as u16) < Self::WHITESPACE as u16
    }

    /// Line terminator as the parser sees it: `NEWLINE` or the `EOF` sentinel.
    ///
    /// Used by `Parser::at_line_term` and by one-token "end of this line"
    /// checks after a measured run. `EOF` is included because lookahead past
    /// the line or the visible limit reports `EOF`.
    #[inline]
    #[must_use]
    pub(crate) fn is_line_end(self) -> bool {
        matches!(self, Self::NEWLINE | Self::EOF)
    }

    /// Terminator of a glued run inside a line: `WHITESPACE` or a line end.
    ///
    /// Ends bullets, checkboxes, drawer, block and node-property names, and
    /// item tags. Unlike [`Self::is_separator`] it excludes `INDENT` and
    /// includes `EOF`.
    #[inline]
    #[must_use]
    pub(crate) fn is_line_end_or_space(self) -> bool {
        self == Self::WHITESPACE || self.is_line_end()
    }

    /// Separator token: `WHITESPACE`, `INDENT`, or `NEWLINE`. Excludes `EOF`.
    #[inline]
    #[must_use]
    pub(crate) fn is_separator(self) -> bool {
        matches!(self, Self::WHITESPACE | Self::INDENT | Self::NEWLINE)
    }
}

impl From<OrgSyntaxKind> for rowan::SyntaxKind {
    fn from(value: OrgSyntaxKind) -> Self {
        Self(value as u16)
    }
}

/// The rowan `Language` binding for Org. Ties `OrgSyntaxKind` to rowan's raw
/// `u16` kinds so `SyntaxNode<OrgLanguage>` reports our kinds directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrgLanguage;

impl rowan::Language for OrgLanguage {
    type Kind = OrgSyntaxKind;

    #[allow(unsafe_code)]
    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(raw.0 < OrgSyntaxKind::__LAST as u16);
        // SAFETY: `OrgSyntaxKind` is `repr(u16)` with contiguous discriminants
        // `0..__LAST`, and `raw.0` is in range by the assertion above.
        // Required by rowan's Language trait; the only unsafe in this crate.
        unsafe { std::mem::transmute::<u16, OrgSyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        SyntaxKind::from(kind)
    }
}

/// Red-tree node over [`OrgLanguage`]: a positioned, absolute-ranged view of a
/// green node.
///
/// This is what the parser produces and a [`crate::Document`] owns; the typed
/// views in [`crate::ast`] are built over it.
///
/// Rowan's red tree uses a non-atomic refcount, so this and anything holding
/// it are `!Send`. The green tree is the part that can cross threads.
pub type SyntaxNode = rowan::SyntaxNode<OrgLanguage>;

/// Red-tree token over [`OrgLanguage`]: a leaf with text and an absolute range.
pub type SyntaxToken = rowan::SyntaxToken<OrgLanguage>;

/// Either a [`SyntaxNode`] or a [`SyntaxToken`], as yielded by child walks that
/// include leaves.
pub type SyntaxElement = rowan::SyntaxElement<OrgLanguage>;

/// Bitset of object kinds allowed in an inline scope.
///
/// Object kinds are the contiguous enum block
/// [`OrgSyntaxKind::BOLD`]..=[`OrgSyntaxKind::LINE_BREAK`]. A kind's bit is
/// `1 << (kind as u16 - BOLD as u16)`; the test
/// `object_kinds_are_contiguous_bitset_base` checks the layout.
///
/// To add an object kind:
/// 1. Keep it inside the `BOLD`..=`LINE_BREAK` block.
/// 2. Add it to its primary group (`MARKUP_OBJECTS` or `STANDARD_EXTRAS`).
///    Sets derived from STANDARD with `without` follow automatically; sets
///    built up from MINIMAL (`LINK_DESCRIPTION`) need an explicit `.with` if
///    Org allows the kind there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ObjectSet(u64);

impl ObjectSet {
    /// Empty set (no objects allowed).
    pub(crate) const EMPTY: Self = Self(0);

    const fn bit(k: OrgSyntaxKind) -> u64 {
        let raw = k as u16;
        let base = OrgSyntaxKind::BOLD as u16;
        let last = OrgSyntaxKind::LINE_BREAK as u16;
        assert!(
            raw >= base && raw <= last,
            "ObjectSet only accepts kinds in BOLD..=LINE_BREAK"
        );
        1u64 << ((raw - base) as u32)
    }

    /// Set containing exactly the listed kinds.
    pub(crate) const fn of(kinds: &[OrgSyntaxKind]) -> Self {
        let mut bits = 0u64;
        let mut i = 0;
        while i < kinds.len() {
            bits |= Self::bit(kinds[i]);
            i += 1;
        }
        Self(bits)
    }

    /// `self` with `k` added.
    pub(crate) const fn with(self, k: OrgSyntaxKind) -> Self {
        Self(self.0 | Self::bit(k))
    }

    /// `self` with `k` removed.
    pub(crate) const fn without(self, k: OrgSyntaxKind) -> Self {
        Self(self.0 & !Self::bit(k))
    }

    /// Bitwise union.
    pub(crate) const fn union(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }

    /// Whether `k` is a member.
    ///
    /// Returns `false` for non-object kinds (outside `BOLD`..=`LINE_BREAK`).
    /// The constructors (`of` / `with` / `without`) panic on those kinds
    /// instead, so a wrongly built set panics (at compile time for the `const`
    /// sets below) while queries stay total.
    pub(crate) const fn contains(self, k: OrgSyntaxKind) -> bool {
        let raw = k as u16;
        let base = OrgSyntaxKind::BOLD as u16;
        let last = OrgSyntaxKind::LINE_BREAK as u16;
        if raw < base || raw > last {
            return false;
        }
        (self.0 & (1u64 << ((raw - base) as u32))) != 0
    }

    /// Whether any kind is in both sets.
    pub(crate) const fn intersects(self, o: Self) -> bool {
        (self.0 & o.0) != 0
    }

    /// Whether every kind in `needs` is in `self`.
    pub(crate) const fn contains_all(self, needs: Self) -> bool {
        (self.0 & needs.0) == needs.0
    }

    /// True when no bits are set.
    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Markup object kinds (emphasis + code/verbatim).
///
/// Used by the inline `AnyMarkup` check; also the markup part of
/// [`MINIMAL`].
pub(crate) const MARKUP_OBJECTS: ObjectSet = ObjectSet::of(&[
    OrgSyntaxKind::BOLD,
    OrgSyntaxKind::ITALIC,
    OrgSyntaxKind::UNDERLINE,
    OrgSyntaxKind::STRIKE_THROUGH,
    OrgSyntaxKind::CODE,
    OrgSyntaxKind::VERBATIM,
]);

/// Org-element `minimal-set` (minus kinds not yet implemented as green
/// objects): markup + entity + latex-fragment + sub/superscript.
const MINIMAL: ObjectSet = MARKUP_OBJECTS
    .with(OrgSyntaxKind::ENTITY)
    .with(OrgSyntaxKind::LATEX_FRAGMENT)
    .with(OrgSyntaxKind::SUBSCRIPT)
    .with(OrgSyntaxKind::SUPERSCRIPT);

/// Org-element `standard-set` extras beyond the minimal set (minus kinds not
/// yet implemented as green objects). Includes `CITATION` but not
/// `CITATION_REFERENCE`, which Org removes from the standard set because it
/// only occurs inside a citation.
const STANDARD_EXTRAS: ObjectSet = ObjectSet::of(&[
    OrgSyntaxKind::LINK,
    OrgSyntaxKind::INLINE_BABEL_CALL,
    OrgSyntaxKind::INLINE_SRC_BLOCK,
    OrgSyntaxKind::TIMESTAMP,
    OrgSyntaxKind::TARGET,
    OrgSyntaxKind::RADIO_TARGET,
    OrgSyntaxKind::STATISTICS_COOKIE,
    OrgSyntaxKind::FOOTNOTE_REFERENCE,
    OrgSyntaxKind::EXPORT_SNIPPET,
    OrgSyntaxKind::LINE_BREAK,
    OrgSyntaxKind::MACRO,
    OrgSyntaxKind::CITATION,
]);

/// Paragraph / emphasis / footnote-ref body (org `standard-set`).
const STANDARD: ObjectSet = MINIMAL.union(STANDARD_EXTRAS);

/// Headline and inline-task titles, item tags: org
/// `standard-set-no-line-break`.
const STANDARD_NO_LINE_BREAK: ObjectSet = STANDARD.without(OrgSyntaxKind::LINE_BREAK);

/// Keyword values (org `keyword` row: standard without footnote-reference).
const STANDARD_NO_FOOTNOTE: ObjectSet = STANDARD.without(OrgSyntaxKind::FOOTNOTE_REFERENCE);

/// Org `link` object-restrictions row (minus kinds not yet implemented):
/// minimal + export snippet + inline babel + inline src + cookie + macro. No
/// nested link, targets, timestamp, footnote-reference, citation, or line
/// break.
const LINK_DESCRIPTION: ObjectSet = MINIMAL
    .with(OrgSyntaxKind::EXPORT_SNIPPET)
    .with(OrgSyntaxKind::INLINE_BABEL_CALL)
    .with(OrgSyntaxKind::INLINE_SRC_BLOCK)
    .with(OrgSyntaxKind::STATISTICS_COOKIE)
    .with(OrgSyntaxKind::MACRO);

/// Table cells: standard without babel call, inline src, statistics cookie,
/// or line break. Macro and citation stay (org table-cell row; not remq'd).
const TABLE_CELL: ObjectSet = STANDARD
    .without(OrgSyntaxKind::INLINE_BABEL_CALL)
    .without(OrgSyntaxKind::INLINE_SRC_BLOCK)
    .without(OrgSyntaxKind::STATISTICS_COOKIE)
    .without(OrgSyntaxKind::LINE_BREAK);

/// Org `standard-set-for-citations`: standard-set-no-line-break minus
/// citation, citation-reference, footnote-reference, and link. Used as the
/// secondary object set for citation global / per-ref prefix and suffix.
const STANDARD_FOR_CITATIONS: ObjectSet = STANDARD_NO_LINE_BREAK
    .without(OrgSyntaxKind::CITATION)
    .without(OrgSyntaxKind::CITATION_REFERENCE)
    .without(OrgSyntaxKind::FOOTNOTE_REFERENCE)
    .without(OrgSyntaxKind::LINK);

/// The objects permitted directly inside `scope`.
///
/// Mirrors `org-element-object-restrictions`. Code and verbatim are absent as
/// *containers* (their contents are opaque), so they have no scope of their own.
/// Inline babel calls and inline src blocks are included for paragraph, headline
/// title, emphasis, and link descriptions (listed explicitly in Org's `link`
/// row); both are excluded from table cells. Macro and citation are in the
/// standard set and table cells but not the minimal set (radio targets);
/// citation is excluded from link descriptions. Citation-reference occurs only
/// as a child of `CITATION`, and its prefix and suffix take
/// `standard-set-for-citations`.
pub(crate) const fn restrictions(scope: OrgSyntaxKind) -> ObjectSet {
    match scope {
        OrgSyntaxKind::PARAGRAPH
        | OrgSyntaxKind::BOLD
        | OrgSyntaxKind::ITALIC
        | OrgSyntaxKind::UNDERLINE
        | OrgSyntaxKind::STRIKE_THROUGH
        // Sub/superscript bodies are recursive containers (org
        // `org-element-recursive-objects`) and take the standard set.
        | OrgSyntaxKind::SUBSCRIPT
        | OrgSyntaxKind::SUPERSCRIPT
        // Inline footnote definitions re-parse under the standard set.
        | OrgSyntaxKind::FOOTNOTE_REFERENCE
        // Verse bodies: org `(verse-block ,@standard-set)`. The only block
        // whose body holds objects; other lesser blocks are opaque and stay
        // `EMPTY`.
        | OrgSyntaxKind::VERSE_BLOCK => STANDARD,
        // Headline titles and item tags take `standard-set-no-line-break` (org
        // `headline` / `item` / `inlinetask` rows).
        OrgSyntaxKind::HEADING_TITLE | OrgSyntaxKind::ITEM_TAG => STANDARD_NO_LINE_BREAK,
        // Org keyword row (standard minus footnote-reference). Only keywords
        // that parse objects, such as CAPTION (`KeywordClass::parses_inlines`),
        // consult this; other `#+KEY:` values stay plain text.
        OrgSyntaxKind::KEYWORD_DUAL | OrgSyntaxKind::KEYWORD_VALUE => STANDARD_NO_FOOTNOTE,
        OrgSyntaxKind::LINK_DESCRIPTION => LINK_DESCRIPTION,
        OrgSyntaxKind::TABLE_CELL => TABLE_CELL,
        // org radio-target row is the minimal set (markup + entity + latex + scripts).
        OrgSyntaxKind::RADIO_TARGET => MINIMAL,
        // Citation contents: only citation-reference children. The citation
        // parser walks its references itself; this arm keeps the table
        // faithful to Org.
        OrgSyntaxKind::CITATION => ObjectSet::of(&[OrgSyntaxKind::CITATION_REFERENCE]),
        // Prefix/suffix under a citation-reference: standard-set-for-citations.
        OrgSyntaxKind::CITATION_REFERENCE => STANDARD_FOR_CITATIONS,
        _ => ObjectSet::EMPTY,
    }
}

/// Node kinds the parser deliberately never emits, each with its reason.
///
/// Every other node kind must appear somewhere in [`crate::tests::TAXONOMY`],
/// as checked by
/// [`crate::tests::every_node_kind_is_emitted_or_documented_as_never_emitted`].
/// A kind missing from the corpus is a parser or corpus gap; listing it here
/// does not fix that.
#[cfg(test)]
pub(crate) const NEVER_EMITTED: &[(OrgSyntaxKind, &str)] = &[(
    OrgSyntaxKind::ERROR,
    "recovery kind; parsing is infallible and degrades to text",
)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restriction_supersets_share_markup_objects() {
        let standard = restrictions(OrgSyntaxKind::PARAGRAPH);
        let desc = restrictions(OrgSyntaxKind::LINK_DESCRIPTION);
        let radio = restrictions(OrgSyntaxKind::RADIO_TARGET);
        // Every markup bit is present in the supersets.
        assert!(standard.contains_all(MARKUP_OBJECTS));
        assert!(desc.contains_all(MARKUP_OBJECTS));
        assert!(radio.contains_all(MARKUP_OBJECTS));
        // Radio is the minimal set (markup + entity + latex-fragment + sub/sup).
        assert_eq!(radio, MINIMAL);
        assert!(radio.contains(OrgSyntaxKind::ENTITY));
        assert!(radio.contains(OrgSyntaxKind::LATEX_FRAGMENT));
        assert!(radio.contains(OrgSyntaxKind::SUBSCRIPT));
        assert!(radio.contains(OrgSyntaxKind::SUPERSCRIPT));
        assert!(!radio.contains(OrgSyntaxKind::LINE_BREAK));
        assert!(!radio.contains(OrgSyntaxKind::EXPORT_SNIPPET));
        // STANDARD has the extras radio lacks.
        assert!(standard.contains(OrgSyntaxKind::TARGET));
        assert!(standard.contains(OrgSyntaxKind::RADIO_TARGET));
        assert!(standard.contains(OrgSyntaxKind::LINK));
        assert!(standard.contains(OrgSyntaxKind::TIMESTAMP));
        assert!(standard.contains(OrgSyntaxKind::INLINE_BABEL_CALL));
        assert!(standard.contains(OrgSyntaxKind::INLINE_SRC_BLOCK));
        assert!(standard.contains(OrgSyntaxKind::FOOTNOTE_REFERENCE));
        assert!(standard.contains(OrgSyntaxKind::ENTITY));
        assert!(standard.contains(OrgSyntaxKind::LATEX_FRAGMENT));
        assert!(standard.contains(OrgSyntaxKind::EXPORT_SNIPPET));
        assert!(standard.contains(OrgSyntaxKind::LINE_BREAK));
        // Description includes entity + latex (minimal), export snippet, babel,
        // and inline src, but excludes targets/links/ts and line breaks.
        // MACRO membership is checked by `macro_restriction_membership`.
        assert!(desc.contains(OrgSyntaxKind::ENTITY));
        assert!(desc.contains(OrgSyntaxKind::LATEX_FRAGMENT));
        assert!(desc.contains(OrgSyntaxKind::EXPORT_SNIPPET));
        assert!(desc.contains(OrgSyntaxKind::INLINE_BABEL_CALL));
        assert!(desc.contains(OrgSyntaxKind::INLINE_SRC_BLOCK));
        assert!(!desc.contains(OrgSyntaxKind::LINE_BREAK));
        assert!(!desc.contains(OrgSyntaxKind::TARGET));
        assert!(!desc.contains(OrgSyntaxKind::RADIO_TARGET));
        assert!(!desc.contains(OrgSyntaxKind::LINK));
        assert!(!desc.contains(OrgSyntaxKind::FOOTNOTE_REFERENCE));
        assert!(!desc.contains(OrgSyntaxKind::TIMESTAMP));
        // Radio (MINIMAL) excludes babel and src.
        assert!(!radio.contains(OrgSyntaxKind::INLINE_BABEL_CALL));
        assert!(!radio.contains(OrgSyntaxKind::INLINE_SRC_BLOCK));
        // Keyword dual/value (org keyword row) exclude footnotes; table cells
        // include them.
        let kw_val = restrictions(OrgSyntaxKind::KEYWORD_VALUE);
        assert!(!kw_val.contains(OrgSyntaxKind::FOOTNOTE_REFERENCE));
        let cell = restrictions(OrgSyntaxKind::TABLE_CELL);
        assert!(cell.contains(OrgSyntaxKind::FOOTNOTE_REFERENCE));
        assert!(cell.contains(OrgSyntaxKind::LATEX_FRAGMENT));
        // Explicit membership: set equality alone would pass if both sides
        // omitted a kind.
        assert!(!cell.contains(OrgSyntaxKind::INLINE_BABEL_CALL));
        assert!(!cell.contains(OrgSyntaxKind::INLINE_SRC_BLOCK));
        // Inline footnote body uses the standard set.
        assert_eq!(
            restrictions(OrgSyntaxKind::FOOTNOTE_REFERENCE),
            restrictions(OrgSyntaxKind::PARAGRAPH)
        );
    }

    /// Org `(verse-block ,@standard-set)`: verse bodies take paragraph's set.
    ///
    /// Verse is the only block whose body holds objects (`BodyMode::Objects`);
    /// without its arm the scope would fall through to `EMPTY` and the body
    /// would silently parse as bare tokens. Explicit membership catches a
    /// kind missing from STANDARD, which equality with PARAGRAPH cannot.
    #[test]
    fn verse_block_is_standard_set() {
        let verse = restrictions(OrgSyntaxKind::VERSE_BLOCK);
        assert_eq!(verse, restrictions(OrgSyntaxKind::PARAGRAPH));
        for kind in [
            OrgSyntaxKind::BOLD,
            OrgSyntaxKind::LINK,
            OrgSyntaxKind::TARGET,
            OrgSyntaxKind::RADIO_TARGET,
            OrgSyntaxKind::LINE_BREAK,
            OrgSyntaxKind::ENTITY,
            OrgSyntaxKind::LATEX_FRAGMENT,
            OrgSyntaxKind::MACRO,
            OrgSyntaxKind::CITATION,
            OrgSyntaxKind::FOOTNOTE_REFERENCE,
            OrgSyntaxKind::TIMESTAMP,
        ] {
            assert!(verse.contains(kind), "verse body must allow {kind:?}");
        }
        // Other lesser blocks are opaque, so they have no object scope.
        for kind in [
            OrgSyntaxKind::SRC_BLOCK,
            OrgSyntaxKind::EXAMPLE_BLOCK,
            OrgSyntaxKind::EXPORT_BLOCK,
            OrgSyntaxKind::COMMENT_BLOCK,
        ] {
            assert!(
                restrictions(kind).is_empty(),
                "{kind:?} is opaque; it must not be a restriction scope"
            );
        }
    }

    /// Macro membership: STANDARD / LINK_DESCRIPTION / TABLE_CELL include it;
    /// MINIMAL / radio exclude it. The set-equality test for TABLE_CELL would
    /// still pass if MACRO were missing from STANDARD; this one would not.
    #[test]
    fn macro_restriction_membership() {
        let standard = restrictions(OrgSyntaxKind::PARAGRAPH);
        let desc = restrictions(OrgSyntaxKind::LINK_DESCRIPTION);
        let radio = restrictions(OrgSyntaxKind::RADIO_TARGET);
        let cell = restrictions(OrgSyntaxKind::TABLE_CELL);

        assert!(standard.contains(OrgSyntaxKind::MACRO));
        assert!(desc.contains(OrgSyntaxKind::MACRO));
        assert!(cell.contains(OrgSyntaxKind::MACRO));
        assert!(!radio.contains(OrgSyntaxKind::MACRO));
        assert!(!MINIMAL.contains(OrgSyntaxKind::MACRO));
    }

    /// CITATION is allowed in STANDARD-based scopes; CITATION_REFERENCE never
    /// stands alone. Explicit membership, since set equality would pass if
    /// both sides omitted a kind.
    #[test]
    fn citation_restriction_membership() {
        let standard = restrictions(OrgSyntaxKind::PARAGRAPH);
        let desc = restrictions(OrgSyntaxKind::LINK_DESCRIPTION);
        let radio = restrictions(OrgSyntaxKind::RADIO_TARGET);
        let cell = restrictions(OrgSyntaxKind::TABLE_CELL);
        let title = restrictions(OrgSyntaxKind::HEADING_TITLE);
        let tag = restrictions(OrgSyntaxKind::ITEM_TAG);
        let keyword_value = restrictions(OrgSyntaxKind::KEYWORD_VALUE);

        // Freestanding citation object.
        assert!(standard.contains(OrgSyntaxKind::CITATION));
        assert!(cell.contains(OrgSyntaxKind::CITATION));
        assert!(title.contains(OrgSyntaxKind::CITATION));
        assert!(tag.contains(OrgSyntaxKind::CITATION));
        assert!(keyword_value.contains(OrgSyntaxKind::CITATION));
        assert!(!desc.contains(OrgSyntaxKind::CITATION));
        assert!(!radio.contains(OrgSyntaxKind::CITATION));
        assert!(!MINIMAL.contains(OrgSyntaxKind::CITATION));

        // Freestanding citation-reference is never in STANDARD.
        assert!(!standard.contains(OrgSyntaxKind::CITATION_REFERENCE));
        assert!(!cell.contains(OrgSyntaxKind::CITATION_REFERENCE));
        assert!(!desc.contains(OrgSyntaxKind::CITATION_REFERENCE));
        assert!(!title.contains(OrgSyntaxKind::CITATION_REFERENCE));
    }

    /// Org citation / citation-reference restriction rows.
    #[test]
    fn citation_restriction_scope_equality() {
        assert_eq!(
            restrictions(OrgSyntaxKind::CITATION),
            ObjectSet::of(&[OrgSyntaxKind::CITATION_REFERENCE]),
            "citation contents allow only citation-reference"
        );
        assert_eq!(
            restrictions(OrgSyntaxKind::CITATION_REFERENCE),
            restrictions(OrgSyntaxKind::PARAGRAPH)
                .without(OrgSyntaxKind::LINE_BREAK)
                .without(OrgSyntaxKind::CITATION)
                .without(OrgSyntaxKind::CITATION_REFERENCE)
                .without(OrgSyntaxKind::FOOTNOTE_REFERENCE)
                .without(OrgSyntaxKind::LINK),
            "citation-reference secondary is standard-set-for-citations"
        );

        let secondary = restrictions(OrgSyntaxKind::CITATION_REFERENCE);
        assert!(!secondary.contains(OrgSyntaxKind::CITATION));
        assert!(!secondary.contains(OrgSyntaxKind::CITATION_REFERENCE));
        assert!(!secondary.contains(OrgSyntaxKind::FOOTNOTE_REFERENCE));
        assert!(!secondary.contains(OrgSyntaxKind::LINK));
        assert!(!secondary.contains(OrgSyntaxKind::LINE_BREAK));
        // Allowed secondaries (org-element, not Worg minimal).
        assert!(secondary.contains(OrgSyntaxKind::EXPORT_SNIPPET));
        assert!(secondary.contains(OrgSyntaxKind::MACRO));
        assert!(secondary.contains(OrgSyntaxKind::BOLD));
        assert!(secondary.contains(OrgSyntaxKind::ENTITY));
    }

    #[test]
    fn keyword_dual_and_value_are_standard_minus_footnote_reference() {
        let expected =
            restrictions(OrgSyntaxKind::PARAGRAPH).without(OrgSyntaxKind::FOOTNOTE_REFERENCE);
        assert_eq!(
            restrictions(OrgSyntaxKind::KEYWORD_VALUE),
            expected,
            "keyword value regions use the org keyword row: STANDARD minus footnote-reference"
        );
        assert_eq!(
            restrictions(OrgSyntaxKind::KEYWORD_DUAL),
            expected,
            "keyword dual regions use the same org keyword row as values"
        );
    }

    #[test]
    fn table_cell_is_standard_minus_babel_src_cookie_and_line_break() {
        assert_eq!(
            restrictions(OrgSyntaxKind::TABLE_CELL),
            restrictions(OrgSyntaxKind::PARAGRAPH)
                .without(OrgSyntaxKind::INLINE_BABEL_CALL)
                .without(OrgSyntaxKind::INLINE_SRC_BLOCK)
                .without(OrgSyntaxKind::STATISTICS_COOKIE)
                .without(OrgSyntaxKind::LINE_BREAK),
            "TABLE_CELL is STANDARD minus babel call, inline src, statistics \
             cookie and line break (for the currently implemented object set)"
        );
    }

    /// Org `headline` / `item` / `inlinetask` rows are
    /// `standard-set-no-line-break`: everything paragraphs allow except `\\`.
    #[test]
    fn title_and_item_tag_are_standard_minus_line_break() {
        let standard = restrictions(OrgSyntaxKind::PARAGRAPH);
        for scope in [OrgSyntaxKind::HEADING_TITLE, OrgSyntaxKind::ITEM_TAG] {
            assert_eq!(
                restrictions(scope),
                standard.without(OrgSyntaxKind::LINE_BREAK),
                "{scope:?} is standard-set-no-line-break"
            );
            assert!(!restrictions(scope).contains(OrgSyntaxKind::LINE_BREAK));
            assert!(restrictions(scope).contains(OrgSyntaxKind::ENTITY));
        }
    }

    /// The inline dispatcher checks underline and subscript together for `_`,
    /// and superscript alone for `^`. That is exact only because no
    /// `org-element-object-restrictions` row splits the three: every row is
    /// built from `minimal-set`, which has all of them. If a scope ever allows
    /// one without the others, this fails and the `_` check must test each
    /// branch separately.
    #[test]
    fn underline_subscript_superscript_co_occur_in_every_scope() {
        use rowan::Language;
        for raw in 0..OrgSyntaxKind::__LAST as u16 {
            let scope = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
            let allowed = restrictions(scope);
            let u = allowed.contains(OrgSyntaxKind::UNDERLINE);
            assert_eq!(
                u,
                allowed.contains(OrgSyntaxKind::SUBSCRIPT),
                "{scope:?} splits underline from subscript"
            );
            assert_eq!(
                u,
                allowed.contains(OrgSyntaxKind::SUPERSCRIPT),
                "{scope:?} splits underline from superscript"
            );
        }
    }

    /// Org `minimal-set` has both entity and latex-fragment, so every scope
    /// built from it allows both. The `\` dispatcher still checks each
    /// separately (see `backslash_row_branches_on_the_second_byte` in the
    /// inline module), so a scope that allowed only one would also work.
    #[test]
    fn entity_and_latex_fragment_co_occur_in_every_scope() {
        use rowan::Language;
        for raw in 0..OrgSyntaxKind::__LAST as u16 {
            let scope = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
            let allowed = restrictions(scope);
            assert_eq!(
                allowed.contains(OrgSyntaxKind::ENTITY),
                allowed.contains(OrgSyntaxKind::LATEX_FRAGMENT),
                "{scope:?} splits entity from latex-fragment"
            );
        }
    }

    #[test]
    fn object_set_algebra_basics() {
        let empty = ObjectSet::EMPTY;
        assert!(empty.is_empty());
        assert!(!empty.contains(OrgSyntaxKind::BOLD));
        // Non-object kinds are never members.
        assert!(!MARKUP_OBJECTS.contains(OrgSyntaxKind::PARAGRAPH));
        assert!(!MARKUP_OBJECTS.contains(OrgSyntaxKind::WHITESPACE));

        let bold = ObjectSet::of(&[OrgSyntaxKind::BOLD]);
        assert!(bold.contains(OrgSyntaxKind::BOLD));
        assert!(!bold.contains(OrgSyntaxKind::ITALIC));
        assert!(MARKUP_OBJECTS.contains_all(bold));
        assert!(MARKUP_OBJECTS.intersects(bold));
        assert!(!bold.intersects(ObjectSet::of(&[OrgSyntaxKind::LINK])));

        let both = bold.with(OrgSyntaxKind::ITALIC);
        assert!(both.contains(OrgSyntaxKind::ITALIC));
        assert!(
            !both
                .without(OrgSyntaxKind::BOLD)
                .contains(OrgSyntaxKind::BOLD)
        );
        assert_eq!(bold.union(ObjectSet::of(&[OrgSyntaxKind::ITALIC])), both);
    }

    #[test]
    fn object_kinds_are_contiguous_bitset_base() {
        use rowan::Language;
        let base = OrgSyntaxKind::BOLD as u16;
        let last = OrgSyntaxKind::LINE_BREAK as u16;
        assert!(last >= base, "LINE_BREAK must not precede BOLD in the enum");
        let count = u32::from(last - base) + 1;
        assert!(
            count <= 64,
            "object block BOLD..=LINE_BREAK has {count} kinds; ObjectSet is u64"
        );
        // Contiguous node kinds only (no tokens in the bitset base).
        for raw in base..=last {
            let kind = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
            assert!(
                kind.is_node(),
                "{kind:?} in object bitset base must be a node"
            );
        }
        // Discriminants are dense: every raw in the range is a real kind.
        assert_eq!(
            OrgLanguage::kind_from_raw(rowan::SyntaxKind(base)),
            OrgSyntaxKind::BOLD
        );
        assert_eq!(
            OrgLanguage::kind_from_raw(rowan::SyntaxKind(last)),
            OrgSyntaxKind::LINE_BREAK
        );
    }

    #[test]
    fn token_node_partition_is_total_and_disjoint() {
        use rowan::Language;
        // Walk every real discriminant; each is exactly one of node/token.
        for raw in 0..OrgSyntaxKind::__LAST as u16 {
            let kind = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
            assert!(
                kind.is_node() ^ kind.is_token(),
                "{kind:?} must be exactly one of node/token"
            );
        }
    }

    #[test]
    fn delimiters_are_tokens() {
        for tok in [
            OrgSyntaxKind::STAR,
            OrgSyntaxKind::PERCENT,
            OrgSyntaxKind::TEXT,
            OrgSyntaxKind::NEWLINE,
        ] {
            assert!(tok.is_token());
        }
        for node in [
            OrgSyntaxKind::DOCUMENT,
            OrgSyntaxKind::HEADING,
            OrgSyntaxKind::BOLD,
            OrgSyntaxKind::ERROR,
        ] {
            assert!(node.is_node());
        }
    }

    #[test]
    fn is_line_end_and_is_separator() {
        assert!(OrgSyntaxKind::NEWLINE.is_line_end());
        assert!(OrgSyntaxKind::EOF.is_line_end());
        assert!(!OrgSyntaxKind::WHITESPACE.is_line_end());
        assert!(!OrgSyntaxKind::INDENT.is_line_end());
        assert!(!OrgSyntaxKind::TEXT.is_line_end());

        assert!(OrgSyntaxKind::WHITESPACE.is_separator());
        assert!(OrgSyntaxKind::INDENT.is_separator());
        assert!(OrgSyntaxKind::NEWLINE.is_separator());
        assert!(!OrgSyntaxKind::EOF.is_separator());
        assert!(!OrgSyntaxKind::TEXT.is_separator());
    }

    /// `EOF` ends a run, since lookahead past the line reports it.
    #[test]
    fn is_line_end_or_space_covers_run_terminators() {
        assert!(OrgSyntaxKind::WHITESPACE.is_line_end_or_space());
        assert!(OrgSyntaxKind::NEWLINE.is_line_end_or_space());
        assert!(OrgSyntaxKind::EOF.is_line_end_or_space());
        // INDENT is line-leading trivia, never an interior run terminator.
        assert!(!OrgSyntaxKind::INDENT.is_line_end_or_space());
        assert!(!OrgSyntaxKind::TEXT.is_line_end_or_space());
        assert!(!OrgSyntaxKind::COLON.is_line_end_or_space());
    }
}
