//! Parse configuration (todo keywords, inline-task threshold, link types,
//! user entities).
//!
//! [`ParseConfig`] holds every input that can change the shape of the tree;
//! the stock Org defaults are public constants.

use crate::entities::{self, EntityDef};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Defaults (constants, not user fields)
// ---------------------------------------------------------------------------

/// Org default for `org-inlinetask-min-level` (`defcustom` in `org-inlinetask.el`).
///
/// The threshold is always applied: star lines at or above it parse as
/// `INLINE_TASK` elements, not headlines. There is no mode that treats every
/// star line as a headline.
pub const DEFAULT_INLINE_TASK_MIN_LEVEL: u16 = 15;

/// Default ceiling on simultaneously open nesting levels (see
/// [`ParseConfig::max_nesting_depth`]).
///
/// Org places no limit on nesting; this is a resource bound, not a syntax
/// rule. Recursive descent costs Rust stack per open region, and rowan's green
/// `Drop` costs stack per tree level, so an unbounded document would overflow
/// the stack instead of producing a `Document`.
///
/// 128 is derived from the worst measured per-level cost (nested lists, ~2.0
/// KiB per region level in a debug build) against the smallest supported stack
/// (1 MiB, the `wasm-ld` default): ~257 KiB, a quarter of the stack, with the
/// same shapes costing ~33 KiB in release. Real Org documents run one to two
/// orders of magnitude shallower.
pub const DEFAULT_MAX_NESTING_DEPTH: u16 = 128;

/// Built-in Org link types plus `id`, recognised in plain and angle links.
///
/// Recognition tries longer names first (ties by name), whatever the order
/// here.
pub const DEFAULT_LINK_TYPES: &[&str] = &[
    "https", "http", "mailto", "file", "ftp", "news", "shell", "elisp", "help", "id",
];

/// Default Org TODO sequence pairs used by [`ParseConfig::default`].
pub const DEFAULT_TODO_KEYWORDS: &[(&str, TodoKind)] =
    &[("TODO", TodoKind::Active), ("DONE", TodoKind::Done)];

// ---------------------------------------------------------------------------
// Link protocol registry (plain/angle recognition only)
// ---------------------------------------------------------------------------

/// Ordered set of link type strings for parse-time plain/angle recognition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolSet {
    /// Longest-first, then name ascending for stable ties.
    types: Vec<String>,
}

impl ProtocolSet {
    /// Build from type names; always sorts longest-first then name.
    pub(crate) fn new(iter: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut types: Vec<String> = iter.into_iter().map(Into::into).collect();
        types.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        types.dedup();
        Self { types }
    }

    /// Default Org built-ins + `id`.
    pub(crate) fn defaults() -> Self {
        Self::new(DEFAULT_LINK_TYPES.iter().copied())
    }

    /// Configured type names in match order (longest-first).
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.types.iter().map(String::as_str)
    }

    /// If `s` starts with a registered `TYPE:` (ASCII case-insensitive, longest
    /// first), return `(type_slice_from_source, byte_len_through_colon)`.
    pub(crate) fn match_protocol_prefix<'a>(&self, s: &'a str) -> Option<(&'a str, usize)> {
        let bytes = s.as_bytes();
        for ty in &self.types {
            let tlen = ty.len();
            if bytes.len() < tlen + 1 || bytes[tlen] != b':' {
                continue;
            }
            if !bytes[..tlen]
                .iter()
                .zip(ty.as_bytes())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            {
                continue;
            }
            return Some((&s[..tlen], tlen + 1));
        }
        None
    }
}

/// Whether a TODO keyword is an active or a done state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoKind {
    /// Keyword in the active sequence (e.g. `TODO`).
    Active,
    /// Keyword in the done sequence (e.g. `DONE`).
    Done,
}

// ---------------------------------------------------------------------------
// TODO sequences (authoritative vocabulary)
// ---------------------------------------------------------------------------

/// How a TODO sequence is meant to be cycled (`#+SEQ_TODO:` vs `#+TYP_TODO:`).
///
/// The parser ignores this: both kinds contribute the same names to the
/// keyword lookup. It is kept because code that cycles TODO states needs it,
/// and so that two different vocabularies do not compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TodoInterpretation {
    /// `#+TODO:` / `#+SEQ_TODO:`: states of one workflow, cycled in order.
    Sequence,
    /// `#+TYP_TODO:`: types (who/what), not stages.
    Type,
}

/// One TODO sequence: names in cycle order, split into active then done.
///
/// The split is stored as an index into one list, so the active and done
/// names can never disagree with [`Self::keywords`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoSequence {
    interpretation: TodoInterpretation,
    /// Active names first, then done names. Fast-access keys already stripped.
    keywords: Vec<String>,
    /// Index of the first done name; `keywords.len()` means "all active".
    done_from: usize,
}

impl TodoSequence {
    /// Build a sequence from names in cycle order and the index of the first
    /// done name (the bar position).
    ///
    /// `done_from` is clamped to the name count, so an out-of-range split
    /// yields an all-active sequence rather than a panic or a phantom done
    /// name. Empty and duplicate names are the caller's business; see
    /// [`ParseConfig::with_todo_sequences`] for how they are normalised.
    #[must_use]
    pub fn new<I, S>(interpretation: TodoInterpretation, keywords: I, done_from: usize) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let keywords: Vec<String> = keywords.into_iter().map(Into::into).collect();
        Self {
            interpretation,
            done_from: done_from.min(keywords.len()),
            keywords,
        }
    }

    /// Sequence vs type interpretation.
    #[inline]
    #[must_use]
    pub fn interpretation(&self) -> TodoInterpretation {
        self.interpretation
    }

    /// All names in cycle order (active then done).
    #[inline]
    #[must_use]
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Names before the bar.
    #[inline]
    #[must_use]
    pub fn active(&self) -> &[String] {
        &self.keywords[..self.done_from]
    }

    /// Names at or after the bar.
    #[inline]
    #[must_use]
    pub fn done(&self) -> &[String] {
        &self.keywords[self.done_from..]
    }

    /// Kind of the name at `index` in [`Self::keywords`].
    #[inline]
    fn kind_at(&self, index: usize) -> TodoKind {
        if index < self.done_from {
            TodoKind::Active
        } else {
            TodoKind::Done
        }
    }

    /// True when the sequence names nothing (contributes no vocabulary).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Priorities
// ---------------------------------------------------------------------------

/// One end of a priority range: a letter or a small number.
///
/// [`Self::Letter`] holds `A`..=`Z` and [`Self::Number`] holds `0`..=`64`;
/// values outside those bands are not produced by parsing and have no defined
/// meaning, but [`Self::rank`] stays total for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriorityAtom {
    /// Letter priority (`A`..=`Z`), as in `#+PRIORITIES: A C B`.
    Letter(char),
    /// Numeric priority (`0`..=`64`), as in `#+PRIORITIES: 1 9 5`.
    Number(u8),
}

impl PriorityAtom {
    /// Largest numeric priority Org accepts.
    ///
    /// One below `A` (65): [`Self::rank`] compares a number against a
    /// letter's character code, so numbers must stop before letters start for
    /// the two forms to be ordered against each other. Cookie recognition, the
    /// writer and the `#+PRIORITIES:` parser all use this constant.
    pub const MAX_NUMBER: u8 = 64;

    /// Whether `s` is a valid `[#s]` cookie value (`org-priority-value-regexp`).
    ///
    /// Single A-Z, or a decimal 0-64 with no leading zero on two-digit values
    /// (`[A-Z]|[0-9]|[1-5][0-9]|6[0-4]`). `[#A]` and `[#64]` match; `[#05]`,
    /// `[#00]`, `[#65]`, and `[#a]` do not. This is the inline cookie rule; a
    /// `#+PRIORITIES:` token differs in that leading zeros are insignificant
    /// (`05` is 5), see `settings::parse_priorities_value`.
    #[must_use]
    pub(crate) fn is_cookie_value(s: &str) -> bool {
        let bytes = s.as_bytes();
        match bytes.len() {
            1 => {
                let b = bytes[0];
                b.is_ascii_uppercase() || b.is_ascii_digit()
            }
            2 => {
                let b0 = bytes[0];
                let b1 = bytes[1];
                if b0.is_ascii_digit() && b1.is_ascii_digit() {
                    let n = u16::from(b0 - b'0') * 10 + u16::from(b1 - b'0');
                    (10..=u16::from(Self::MAX_NUMBER)).contains(&n)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Comparable value of this atom: **lower ranks higher**.
    ///
    /// Mirrors `org-priority-to-value`: a numeric priority is its own value, a
    /// letter is its character code. Numbers stop at 64 and letters start at
    /// `A` (65), so every numeric priority outranks every letter. Compare
    /// ranges through this function rather than by matching the variants.
    #[inline]
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            // Saturates instead of truncating: `Letter` is contracted to
            // A..=Z, and a wider char must not alias a low numeric rank.
            Self::Letter(c) => u8::try_from(u32::from(c)).unwrap_or(u8::MAX),
            Self::Number(n) => n,
        }
    }
}

/// Effective priority range (`org-priority-highest` / `-lowest` / `-default`).
///
/// Stored as parsed. Whether the three atoms are coherent (highest outranks
/// lowest, default between them) never changes the tree, so it is checked
/// later, where it produces diagnostics and constrains operations. A range
/// Org would accept is stored here even when that check rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PriorityRange {
    /// Highest (most urgent) priority; lowest [`PriorityAtom::rank`].
    pub highest: PriorityAtom,
    /// Lowest (least urgent) priority; highest [`PriorityAtom::rank`].
    pub lowest: PriorityAtom,
    /// Priority assumed for a headline with no cookie.
    pub default: PriorityAtom,
}

impl PriorityRange {
    /// Whether `atom` lies between [`Self::highest`] and [`Self::lowest`]
    /// inclusive, compared by [`PriorityAtom::rank`].
    #[must_use]
    pub fn contains(self, atom: PriorityAtom) -> bool {
        let rank = atom.rank();
        let hi = self.highest.rank();
        let lo = self.lowest.rank();
        hi <= rank && rank <= lo
    }
}

impl Default for PriorityRange {
    /// Org defaults: `A` highest, `C` lowest, `B` default.
    fn default() -> Self {
        Self {
            highest: PriorityAtom::Letter('A'),
            lowest: PriorityAtom::Letter('C'),
            default: PriorityAtom::Letter('B'),
        }
    }
}

/// Configuration for the parser: every input that can change the tree.
///
/// Start from [`ParseConfig::default`], which is stock Org, and chain the
/// `with_*` builders. Each builder replaces its table wholesale.
///
/// [`Self::todo_sequences`] is the source of truth for the TODO vocabulary;
/// the keyword lookup is derived from it by every builder.
///
/// Entity lookup ([`Self::entity_get`], [`Self::is_entity_name`]) checks user
/// entities first, then the builtin `org-entities` table, as `org-entity-get`
/// does.
///
/// In-buffer settings such as `#+TODO:` are resolved into a copy of the
/// caller's config, and [`crate::Document::config`] returns the result.
///
/// Only settings that some grammar rule consults belong here. Values a
/// document declares without changing its tree, such as its priority range,
/// are read from the document instead; presentation settings (themes,
/// folding, key bindings) belong to the application.
#[derive(Debug, Clone)]
pub struct ParseConfig {
    /// `org-inlinetask-min-level`. Always applied: levels >= this are not
    /// regular headlines (they emit `INLINE_TASK`). Default:
    /// [`DEFAULT_INLINE_TASK_MIN_LEVEL`] (15).
    inline_task_min_level: u16,

    /// Ceiling on simultaneously open nesting levels. Applied separately to
    /// region nesting and to outline nesting; see
    /// [`Self::max_nesting_depth`].
    max_nesting_depth: u16,

    /// Parallel TODO sequences in declaration order. **Authoritative**: the
    /// two fields below are functions of this one.
    todo_sequences: Vec<TodoSequence>,

    /// Derived: flattened name to kind lookup used by the headline parser.
    todo_keywords: HashMap<String, TodoKind>,

    /// Derived: length of the longest key in `todo_keywords`.
    /// Used by the headline parser for bounded scanning (prevents walking
    /// arbitrarily long non-terminated runs when no keyword can match).
    max_todo_keyword_len: usize,

    /// Link types for plain/angle recognition (longest-first `ProtocolSet`).
    link_types: ProtocolSet,

    /// `org-list-allow-alphabetical`: single-letter ordered bullets.
    list_allow_alphabetical: bool,

    /// User-defined entities (`org-entities-user`), keyed by exact name.
    /// Looked up before the builtin table.
    user_entities: HashMap<String, EntityDef>,
}

impl ParseConfig {
    /// Allow single-letter alphabetical ordered bullets (`a.` / `A)`).
    ///
    /// Mirrors `org-list-allow-alphabetical` (default nil in Org).
    #[must_use]
    pub fn with_list_alphabetical(mut self, allow: bool) -> Self {
        self.list_allow_alphabetical = allow;
        self
    }

    /// Whether alphabetical ordered bullets are recognised.
    #[must_use]
    pub fn list_alphabetical(&self) -> bool {
        self.list_allow_alphabetical
    }

    /// Set the ceiling on simultaneously open nesting levels.
    ///
    /// Small values are fine: even `0` (nothing may nest) parses losslessly,
    /// with deeper content kept as plain text.
    ///
    /// Values far above [`DEFAULT_MAX_NESTING_DEPTH`] (including `u16::MAX`)
    /// are unsupported: they allow a stack overflow on pathological nesting
    /// and when the green tree is dropped.
    #[must_use]
    pub fn with_max_nesting_depth(mut self, depth: u16) -> Self {
        self.max_nesting_depth = depth;
        self
    }

    /// Ceiling on simultaneously open nesting levels.
    ///
    /// Two independent counters are checked against this one number: open
    /// *regions* (object and element bodies, which cost Rust stack) and open
    /// *outlines* (which cost none, but add green tree depth). Past the
    /// ceiling a region is kept verbatim as plain tiles with a
    /// [`crate::DiagnosticKind::NestingLimitExceeded`] diagnostic, and a
    /// deeper headline attaches at the ceiling instead of below it.
    ///
    /// Bounds green tree depth at `max_nesting_depth * 3` plus a small
    /// constant, which keeps recursive consumers of the tree, rowan's own
    /// `Drop` included, safe on every input. See
    /// [`Self::with_max_nesting_depth`] for the supported range.
    #[inline]
    #[must_use]
    pub fn max_nesting_depth(&self) -> u16 {
        self.max_nesting_depth
    }

    /// Set the inline-task minimum level.
    #[must_use]
    pub fn with_inline_task_min_level(mut self, level: u16) -> Self {
        self.inline_task_min_level = level;
        self
    }

    /// `org-inlinetask-min-level` (always applied).
    #[inline]
    #[must_use]
    pub fn inline_task_min_level(&self) -> u16 {
        self.inline_task_min_level
    }

    /// Replace the TODO vocabulary with explicit sequences.
    ///
    /// Normalisation, so that two configs naming the same vocabulary compare
    /// equal: empty names are dropped and sequences left with no names at all
    /// are dropped (an empty sequence contributes nothing but would otherwise
    /// change equality).
    ///
    /// Duplicate names: the **last** sequence listing a name decides its kind,
    /// and within a sequence the last position wins. Org keeps `org-done-
    /// keywords` as a union across sequences, so a name listed done anywhere is
    /// done there; one flat map cannot hold both answers, and last-wins keeps
    /// the derived map a pure function of sequence order. Sequences themselves
    /// keep their full name lists either way.
    #[must_use]
    pub fn with_todo_sequences<I>(mut self, sequences: I) -> Self
    where
        I: IntoIterator<Item = TodoSequence>,
    {
        self.todo_sequences = sequences
            .into_iter()
            .map(normalise_sequence)
            .filter(|s| !s.is_empty())
            .collect();
        self.rebuild_todo_derivatives();
        self
    }

    /// TODO sequences in declaration order.
    #[inline]
    #[must_use]
    pub fn todo_sequences(&self) -> &[TodoSequence] {
        &self.todo_sequences
    }

    /// Replace the TODO vocabulary with one flat sequence of explicit kinds.
    ///
    /// Convenience over [`Self::with_todo_sequences`] for callers that have a
    /// vocabulary but no workflow: the names are partitioned into actives then
    /// dones, each group keeping iterator order, and the bar is placed between
    /// them. The kinds are taken as given: unlike a `#+TODO:` line, there is
    /// no "last name is done" rule, so an all-active input stays all-active.
    /// An empty input clears the vocabulary.
    #[must_use]
    pub fn with_todo_keywords<I, S>(self, keywords: I) -> Self
    where
        I: IntoIterator<Item = (S, TodoKind)>,
        S: Into<String>,
    {
        self.with_todo_sequences(sequence_from_kinds(keywords))
    }

    /// Replace link type names used for plain/angle recognition.
    #[must_use]
    pub fn with_link_types<I, S>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.link_types = ProtocolSet::new(types);
        self
    }

    /// Configured link type names in match order (longest-first).
    pub fn link_type_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.link_types.names()
    }

    /// Longest configured TODO keyword length (bytes).
    #[inline]
    pub(crate) fn max_todo_keyword_len(&self) -> usize {
        self.max_todo_keyword_len
    }

    /// Link protocol set for plain/angle recognition.
    #[inline]
    pub(crate) fn link_types(&self) -> &ProtocolSet {
        &self.link_types
    }

    /// Lookup whether `word` is a configured TODO keyword.
    ///
    /// Case-sensitive exact match, per Org ("KEYWORD ... Case is significant").
    /// Returns the kind (Active/Done) if recognised, or None if the word is
    /// ordinary title text.
    #[must_use]
    pub fn todo_kind(&self, word: &str) -> Option<TodoKind> {
        self.todo_keywords.get(word).copied()
    }

    /// Replace the user entity map (`org-entities-user`).
    ///
    /// Replaces the map from any previous call. Builtin entities are not
    /// stored here; [`Self::entity_get`] consults them after the user map.
    ///
    /// Only definitions whose name passes [`EntityDef::is_user_name_safe`] are
    /// kept. Others (digit start, punctuation, whitespace forms like `"_ "`,
    /// empty) are dropped silently; a builtin with the same name still
    /// resolves. Check names with [`EntityDef::is_user_name_safe`] first if
    /// you need to reject them.
    #[must_use]
    pub fn with_user_entities<I>(mut self, entities: I) -> Self
    where
        I: IntoIterator<Item = EntityDef>,
    {
        let mut map = HashMap::new();
        for e in entities {
            if EntityDef::is_user_name_safe(e.name()) {
                let key = e.name().to_owned();
                map.insert(key, e);
            }
        }
        self.user_entities = map;
        self
    }

    /// Look up an entity by exact name: user map first, then builtin table.
    ///
    /// Mirrors `org-entity-get`. The parser recognises entities through the
    /// same lookup, so a name it parsed as an entity always resolves here.
    ///
    /// Builtin rows are `'static` and coerce to the `&self` borrow, so one
    /// return type covers both sources (see [`EntityDef`]).
    #[inline]
    #[must_use]
    pub fn entity_get(&self, name: &str) -> Option<&EntityDef> {
        self.user_entities
            .get(name)
            .or_else(|| entities::builtin_entity_get(name))
    }

    /// True when `name` is a user or builtin entity name.
    #[inline]
    #[must_use]
    pub fn is_entity_name(&self, name: &str) -> bool {
        self.entity_get(name).is_some()
    }

    /// Rebuild the lookup map and length cache from `todo_sequences`.
    ///
    /// The only writer of either derivative. Sequences are applied in order so
    /// a later one overwrites an earlier one's answer for a duplicate name.
    fn rebuild_todo_derivatives(&mut self) {
        let mut map: HashMap<String, TodoKind> = HashMap::new();
        for seq in &self.todo_sequences {
            for (i, name) in seq.keywords().iter().enumerate() {
                map.insert(name.clone(), seq.kind_at(i));
            }
        }
        self.max_todo_keyword_len = max_key_len(&map);
        self.todo_keywords = map;
    }
}

/// Drop empty names, keeping the bar between the same two neighbours.
fn normalise_sequence(seq: TodoSequence) -> TodoSequence {
    if !seq.keywords().iter().any(String::is_empty) {
        return seq;
    }
    let done_from = seq.active().iter().filter(|n| !n.is_empty()).count();
    let kept = seq.keywords.into_iter().filter(|n| !n.is_empty());
    TodoSequence::new(seq.interpretation, kept, done_from)
}

/// One `Sequence` from explicit kinds: actives in order, then dones in order.
///
/// Yields nothing for an empty input, so `with_todo_keywords([])` clears the
/// vocabulary instead of storing a nameless sequence.
fn sequence_from_kinds<I, S>(keywords: I) -> Option<TodoSequence>
where
    I: IntoIterator<Item = (S, TodoKind)>,
    S: Into<String>,
{
    let (mut active, mut done) = (Vec::new(), Vec::new());
    for (name, kind) in keywords {
        match kind {
            TodoKind::Active => active.push(name.into()),
            TodoKind::Done => done.push(name.into()),
        }
    }
    if active.is_empty() && done.is_empty() {
        return None;
    }
    let done_from = active.len();
    active.extend(done);
    Some(TodoSequence::new(
        TodoInterpretation::Sequence,
        active,
        done_from,
    ))
}

fn max_key_len(map: &HashMap<String, TodoKind>) -> usize {
    map.keys().map(String::len).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Comparison: one field list, two policies
// ---------------------------------------------------------------------------

/// Every way two configs can differ, classified once.
///
/// Equality asks whether two configs are the same value; the damage delta
/// asks what the parser would build differently. Both read this struct, and
/// [`diff`] and its consumers destructure exhaustively, so a new field does
/// not compile until it is classified for both. Otherwise a forgotten field
/// would make a changed config look unchanged, with no failing test.
#[derive(Debug, Clone, Copy)]
struct ConfigDiff {
    /// Settings that affect many grammar rules and cannot be set in-buffer:
    /// inline-task level, nesting budget, link types, alphabetical bullets,
    /// user entities. Only the caller changes them, and a change needs a full
    /// reparse.
    structural: bool,
    /// The TODO vocabulary as a value: names, order, interpretation, and the
    /// active/done split.
    todo_sequences: bool,
}

/// Classify how `a` and `b` differ. The only place `ParseConfig` fields are
/// compared.
fn diff(a: &ParseConfig, b: &ParseConfig) -> ConfigDiff {
    let ParseConfig {
        inline_task_min_level,
        max_nesting_depth,
        todo_sequences,
        // Derivatives of `todo_sequences`: comparing them would be redundant,
        // and comparing them *instead* would lose order and interpretation.
        todo_keywords: _,
        max_todo_keyword_len: _,
        link_types,
        list_allow_alphabetical,
        user_entities,
    } = a;
    ConfigDiff {
        structural: *inline_task_min_level != b.inline_task_min_level
            || *max_nesting_depth != b.max_nesting_depth
            || *link_types != b.link_types
            || *list_allow_alphabetical != b.list_allow_alphabetical
            || *user_entities != b.user_entities,
        todo_sequences: *todo_sequences != b.todo_sequences,
    }
}

impl ConfigDiff {
    /// True when the two configs are the same value.
    fn unchanged(self) -> bool {
        let Self {
            structural,
            todo_sequences,
        } = self;
        !structural && !todo_sequences
    }
}

/// Equality of every setting, TODO sequence order and interpretation included.
///
/// Parsing relies on it: when the config resolved from a document's in-buffer
/// settings equals the one its tree was built under, the second parse is
/// skipped.
impl PartialEq for ParseConfig {
    fn eq(&self, other: &Self) -> bool {
        diff(self, other).unchanged()
    }
}

impl Eq for ParseConfig {}

impl Default for ParseConfig {
    /// Stock Org: `TODO | DONE`, inline-task level 15, no alphabetical
    /// bullets, the builtin link types and no user entities.
    ///
    /// A stock `.org` file therefore parses here as Emacs parses it with a
    /// stock configuration, which is what [`crate::parse`] uses. The one
    /// setting that is not an Org default is the nesting budget,
    /// [`DEFAULT_MAX_NESTING_DEPTH`], a resource bound.
    ///
    /// The default vocabulary is one bar-separated sequence over
    /// [`DEFAULT_TODO_KEYWORDS`], so a file that restates it
    /// (`#+TODO: TODO | DONE`) resolves to an equal config.
    fn default() -> Self {
        // The only struct literal: every field gets its default here.
        let mut config = Self {
            inline_task_min_level: DEFAULT_INLINE_TASK_MIN_LEVEL,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
            todo_sequences: sequence_from_kinds(DEFAULT_TODO_KEYWORDS.iter().copied())
                .into_iter()
                .collect(),
            // Filled from `todo_sequences` before this value escapes.
            todo_keywords: HashMap::new(),
            max_todo_keyword_len: 0,
            link_types: ProtocolSet::defaults(),
            list_allow_alphabetical: false,
            user_entities: HashMap::new(),
        };
        config.rebuild_todo_derivatives();
        config
    }
}

// ---------------------------------------------------------------------------
// ParseConfig delta (damage input; crate-internal)
// ---------------------------------------------------------------------------

/// Difference between two configs, as input to incremental reparsing.
///
/// Only the TODO keyword name set changes how existing text parses;
/// sequence order, interpretation and a name's kind do not. A structural
/// change needs a full reparse.
pub(crate) mod delta {

    use super::{ConfigDiff, ParseConfig};
    use std::collections::BTreeSet;

    /// Which settings changed between two configs, at the granularity
    /// incremental reparsing needs.
    ///
    /// It cannot tell an in-buffer `#+TODO:` line from a caller that replaced
    /// the keywords: both are keyword-set changes. The caller of
    /// [`parse_config_delta`] knows which it is: `parse` and `apply` hold the
    /// caller config fixed, so a keyword-set change there comes from the file.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ParseConfigDelta {
        /// Present only when the keyword sets differ. Sequence order and
        /// interpretation can change with equal sets (`#+SEQ_TODO:` becoming
        /// `#+TYP_TODO:`), and so can a name's kind; the parser asks only
        /// whether a word is in the vocabulary, so neither changes the tree.
        pub(crate) todo_keywords: Option<TodoKeywordDelta>,
        /// A setting that affects many grammar rules changed: link types,
        /// user entities, inline-task level, alphabetical bullets, or the
        /// nesting budget. These cannot be set in-buffer; a change needs a
        /// full reparse.
        pub(crate) caller_structural: bool,
    }

    /// Old and new TODO keyword sets.
    ///
    /// Consumed as a symmetric difference: a headline parses differently iff
    /// its first whitespace-delimited word after the stars lies in it.
    /// Ordered so that predicate is deterministic.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct TodoKeywordDelta {
        pub(crate) old: BTreeSet<String>,
        pub(crate) new: BTreeSet<String>,
    }

    impl ParseConfigDelta {
        /// True when nothing a consumer cares about changed.
        pub(crate) fn is_empty(&self) -> bool {
            self.todo_keywords.is_none() && !self.caller_structural
        }
    }

    /// Classify a config change for incremental reparsing.
    ///
    /// Reads [`super::diff`] rather than comparing fields again, and
    /// destructures its result exhaustively, so a new kind of difference must
    /// be classified here before it compiles.
    pub(crate) fn parse_config_delta(old: &ParseConfig, new: &ParseConfig) -> ParseConfigDelta {
        let ConfigDiff {
            structural,
            // Order, interpretation and a name's Active/Done kind change the
            // vocabulary's value, but the parser resolves every word the same
            // way under them. The name-set comparison below is what matters.
            todo_sequences: _,
        } = super::diff(old, new);

        let old_names: BTreeSet<String> = old.todo_keywords.keys().cloned().collect();
        let new_names: BTreeSet<String> = new.todo_keywords.keys().cloned().collect();
        let names_changed = old_names != new_names;
        ParseConfigDelta {
            todo_keywords: names_changed.then_some(TodoKeywordDelta {
                old: old_names,
                new: new_names,
            }),
            caller_structural: structural,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::delta::parse_config_delta;
    use super::*;
    use TodoInterpretation::{Sequence, Type};

    fn seq<const N: usize>(names: [&str; N], done_from: usize) -> TodoSequence {
        TodoSequence::new(Sequence, names, done_from)
    }

    // --- TODO sequences ----------------------------------------------------

    /// The split is an index, so `active` and `done` cannot disagree with
    /// `keywords`, and an out-of-range split degrades to all-active.
    #[test]
    fn sequence_splits_by_index() {
        let s = seq(["TODO", "NEXT", "DONE"], 2);
        assert_eq!(s.keywords(), ["TODO", "NEXT", "DONE"]);
        assert_eq!(s.active(), ["TODO", "NEXT"]);
        assert_eq!(s.done(), ["DONE"]);

        assert_eq!(seq(["A"], 0).done(), ["A"]);
        assert!(
            seq(["A"], 0).active().is_empty(),
            "{:?}",
            seq(["A"], 0).active()
        );
        assert_eq!(seq(["A", "B"], 99).active(), ["A", "B"], "clamped");
        assert!(seq([], 0).is_empty());
    }

    /// Sequences are the vocabulary's authority; the map and the length cache
    /// are functions of them, rebuilt by every mutator.
    #[test]
    fn derivatives_follow_the_sequences() {
        let cfg = ParseConfig::default().with_todo_sequences([
            seq(["BUG", "REVIEW"], 2),
            TodoSequence::new(Type, ["Fred", "CANCELLED"], 1),
        ]);
        assert_eq!(cfg.todo_sequences().len(), 2);
        for (name, kind) in [
            ("BUG", TodoKind::Active),
            ("REVIEW", TodoKind::Active),
            ("Fred", TodoKind::Active),
            ("CANCELLED", TodoKind::Done),
        ] {
            assert_eq!(cfg.todo_kind(name), Some(kind), "{name}");
        }
        assert_eq!(cfg.todo_kind("TODO"), None, "replace, not merge");
        assert_eq!(cfg.max_todo_keyword_len(), "CANCELLED".len());
    }

    /// Nameless sequences and empty names are dropped, so two configs naming
    /// the same vocabulary cannot compare unequal over padding.
    #[test]
    fn with_todo_sequences_normalises() {
        let padded = ParseConfig::default().with_todo_sequences([
            seq(["", "A", "", "B"], 3),
            seq([], 0),
            seq(["", ""], 1),
        ]);
        assert_eq!(padded.todo_sequences().len(), 1);
        assert_eq!(padded.todo_sequences()[0].active(), ["A"]);
        assert_eq!(padded.todo_sequences()[0].done(), ["B"]);
        assert_eq!(
            padded,
            ParseConfig::default().with_todo_sequences([seq(["A", "B"], 1)])
        );
    }

    /// `with_todo_keywords` partitions into one sequence and takes the kinds as
    /// given - there is no "last name is done" rule outside a `#+TODO:` line.
    #[test]
    fn with_todo_keywords_builds_one_partitioned_sequence() {
        let cfg = ParseConfig::default().with_todo_keywords([
            ("DONE", TodoKind::Done),
            ("TODO", TodoKind::Active),
            ("NEXT", TodoKind::Active),
        ]);
        assert_eq!(cfg.todo_sequences().len(), 1);
        assert_eq!(cfg.todo_sequences()[0].active(), ["TODO", "NEXT"]);
        assert_eq!(cfg.todo_sequences()[0].done(), ["DONE"]);

        // All-active stays all-active.
        let active_only = ParseConfig::default().with_todo_keywords([("A", TodoKind::Active)]);
        assert!(
            active_only.todo_sequences()[0].done().is_empty(),
            "{:?}",
            active_only.todo_sequences()[0].done()
        );
        assert_eq!(active_only.todo_kind("A"), Some(TodoKind::Active));

        // Empty input clears the vocabulary rather than storing a nameless
        // sequence, which would otherwise break equality against a cleared one.
        let cleared = ParseConfig::default().with_todo_keywords::<_, &str>([]);
        assert!(
            cleared.todo_sequences().is_empty(),
            "{:?}",
            cleared.todo_sequences()
        );
        assert_eq!(cleared.max_todo_keyword_len(), 0);
    }

    /// The default vocabulary must be expressible as a file line, or a file
    /// restating Org's defaults would never compare equal to the default
    /// config and would pay for a second parse.
    #[test]
    fn default_vocabulary_is_one_bar_separated_sequence() {
        let cfg = ParseConfig::default();
        assert_eq!(cfg.todo_sequences().len(), 1);
        assert_eq!(cfg.todo_sequences()[0].active(), ["TODO"]);
        assert_eq!(cfg.todo_sequences()[0].done(), ["DONE"]);
        assert_eq!(cfg.todo_sequences()[0].interpretation(), Sequence);
        assert_eq!(
            cfg,
            ParseConfig::default().with_todo_sequences([seq(["TODO", "DONE"], 1)])
        );
    }

    /// Numbers outrank every letter, and the bound is what makes that true:
    /// `rank` is the reason [`PriorityAtom::MAX_NUMBER`] is 64.
    #[test]
    fn priority_rank_orders_numbers_below_letters() {
        assert_eq!(PriorityAtom::Letter('A').rank(), b'A');
        assert_eq!(PriorityAtom::Number(64).rank(), 64);
        assert!(PriorityAtom::Number(64).rank() < PriorityAtom::Letter('A').rank());
        assert!(PriorityAtom::Letter('A').rank() < PriorityAtom::Letter('C').rank());
        assert_eq!(
            PriorityAtom::MAX_NUMBER + 1,
            b'A',
            "the bound is one below the first letter, which is why ranks compare"
        );
    }

    // --- Equality ----------------------------------------------------------

    /// Skipping the second parse relies on equality, so every setting must be
    /// visible to it.
    #[test]
    fn equality_covers_every_knob() {
        let base = ParseConfig::default();
        assert_eq!(base, ParseConfig::default());

        let entity = EntityDef::new("myent", "\\my", false, "&m;", "m", "m", "m");
        let variants = [
            base.clone().with_inline_task_min_level(3),
            base.clone().with_max_nesting_depth(4),
            base.clone()
                .with_todo_keywords([("NEXT", TodoKind::Active)]),
            base.clone().with_link_types(["doi"]),
            base.clone().with_list_alphabetical(true),
            base.clone().with_user_entities([entity]),
        ];
        for variant in &variants {
            assert_ne!(*variant, base, "setting must be visible to equality");
        }
    }

    /// Sequence order and interpretation are part of the vocabulary even when
    /// the flat name set is identical.
    #[test]
    fn equality_sees_order_and_interpretation() {
        let with_sequences = |s: [TodoSequence; 2]| ParseConfig::default().with_todo_sequences(s);
        let ab_cd = with_sequences([seq(["A", "B"], 1), seq(["C", "D"], 1)]);
        assert_ne!(
            ab_cd,
            with_sequences([seq(["C", "D"], 1), seq(["A", "B"], 1)])
        );

        assert_ne!(
            ParseConfig::default().with_todo_sequences([seq(["A", "B"], 1)]),
            ParseConfig::default().with_todo_sequences([TodoSequence::new(Type, ["A", "B"], 1)])
        );
    }

    // --- Delta -------------------------------------------------------------

    #[test]
    fn delta_is_empty_for_equal_configs() {
        let cfg = ParseConfig::default();
        assert!(parse_config_delta(&cfg, &cfg).is_empty());
    }

    /// A vocabulary whose names are unchanged reparses nothing, whatever its
    /// order, interpretation, or per-name kind.
    #[test]
    fn todo_delta_tracks_the_name_set_only() {
        let names = |c: &ParseConfig| {
            parse_config_delta(&ParseConfig::default(), c)
                .todo_keywords
                .map(|d| d.new)
        };
        let same_names_other_kinds = ParseConfig::default()
            .with_todo_keywords([("TODO", TodoKind::Done), ("DONE", TodoKind::Active)]);
        assert!(
            parse_config_delta(&ParseConfig::default(), &same_names_other_kinds)
                .todo_keywords
                .is_none(),
            "same names is no damage"
        );

        let grown = ParseConfig::default().with_todo_keywords([
            ("TODO", TodoKind::Active),
            ("NEXT", TodoKind::Active),
            ("DONE", TodoKind::Done),
        ]);
        let delta = parse_config_delta(&ParseConfig::default(), &grown).todo_keywords;
        let delta = delta.expect("name set grew");
        assert_eq!(
            delta.new.difference(&delta.old).collect::<Vec<_>>(),
            [&"NEXT".to_string()],
            "symmetric difference is the damaged word set"
        );
        assert!(names(&grown).is_some());
    }

    /// A caller-only setting is a structural change, not a vocabulary change.
    /// There is no third category: every field of `ParseConfig` affects
    /// parsing, so nothing is "different but harmless".
    #[test]
    fn delta_separates_structural_from_vocabulary_knobs() {
        let base = ParseConfig::default();

        let structural = parse_config_delta(&base, &base.clone().with_inline_task_min_level(3));
        assert!(structural.caller_structural);
        assert!(structural.todo_keywords.is_none());

        let vocabulary = parse_config_delta(
            &base,
            &base
                .clone()
                .with_todo_keywords([("NEXT", TodoKind::Active)]),
        );
        assert!(vocabulary.todo_keywords.is_some());
        assert!(!vocabulary.caller_structural);
    }

    /// Everything `ParseConfig::default()` must expose, in one place.
    ///
    /// The parse-level tests in `crate::tests` check the level-15 boundary
    /// (level 14 is a HEADING, 15 is not); this checks the accessor against
    /// the published constant.
    #[test]
    fn default_exposes_todo_link_and_threshold_tables() {
        let cfg = ParseConfig::default();

        assert_eq!(cfg.inline_task_min_level(), DEFAULT_INLINE_TASK_MIN_LEVEL);
        assert_eq!(DEFAULT_INLINE_TASK_MIN_LEVEL, 15);

        for (kw, kind) in DEFAULT_TODO_KEYWORDS {
            assert_eq!(cfg.todo_kind(kw), Some(*kind), "default keyword {kw}");
        }
        assert_eq!(cfg.max_todo_keyword_len(), 4); // "TODO" / "DONE"
        // Lookup is case-sensitive.
        assert!(cfg.todo_kind("todo").is_none());
        assert!(cfg.todo_kind("Todo").is_none());

        assert!(
            cfg.link_types()
                .match_protocol_prefix("https://x")
                .is_some()
        );
        assert!(cfg.link_types().match_protocol_prefix("id:abc").is_some());
        // Public read-back includes defaults (order is match order, not slice order).
        let names: Vec<&str> = cfg.link_type_names().collect();
        assert!(names.contains(&"https"));
        assert!(names.contains(&"id"));
        assert_eq!(names.len(), DEFAULT_LINK_TYPES.len());
    }

    #[test]
    fn default_plus_builders_is_the_construction_story() {
        // Each builder replaces its table wholesale and leaves the others.
        let cfg = ParseConfig::default()
            .with_inline_task_min_level(3)
            .with_todo_keywords([("NEXT", TodoKind::Active)])
            .with_link_types(["doi"]);
        assert_eq!(cfg.inline_task_min_level(), 3);
        assert_eq!(cfg.todo_kind("NEXT"), Some(TodoKind::Active));
        assert!(cfg.todo_kind("TODO").is_none(), "replace, not merge");
        assert!(cfg.link_types().match_protocol_prefix("doi:10").is_some());
        assert!(
            cfg.link_types()
                .match_protocol_prefix("https://x")
                .is_none(),
            "replace, not merge"
        );

        // A builder for one table leaves the others at their defaults.
        let only_level = ParseConfig::default().with_inline_task_min_level(5);
        assert_eq!(only_level.inline_task_min_level(), 5);
        assert_eq!(only_level.todo_kind("TODO"), Some(TodoKind::Active));
    }

    /// `max_todo_keyword_len` is a cache: every `with_todo_keywords` must
    /// recompute it, including down to zero.
    #[test]
    fn max_todo_keyword_len_tracks_the_current_table() {
        let cases: &[(&[(&str, TodoKind)], usize)] = &[
            (&[], 0),
            (&[("A", TodoKind::Active)], 1),
            (&[("A", TodoKind::Active), ("LONGER", TodoKind::Done)], 6),
            (&[("WAITING", TodoKind::Active)], 7),
        ];
        for &(kws, want) in cases {
            let cfg = ParseConfig::default().with_todo_keywords(kws.iter().copied());
            assert_eq!(cfg.max_todo_keyword_len(), want, "for {kws:?}");
            // The default table is gone in every case (replace, not merge).
            assert!(cfg.todo_kind("TODO").is_none() || kws.iter().any(|(k, _)| *k == "TODO"));
        }
    }

    #[test]
    fn with_link_types_replaces_registry() {
        let cfg = ParseConfig::default().with_link_types(["doi", "https"]);
        assert!(cfg.link_types().match_protocol_prefix("doi:10.1").is_some());
        assert!(
            cfg.link_types()
                .match_protocol_prefix("https://x")
                .is_some()
        );
        // Default-only protocols are gone after replace.
        assert!(cfg.link_types().match_protocol_prefix("mailto:a").is_none());
        let names: Vec<&str> = cfg.link_type_names().collect();
        assert_eq!(names, vec!["https", "doi"]); // longest-first
    }

    #[test]
    fn protocol_set_longest_first_prefers_https() {
        let set = ProtocolSet::defaults();
        let (ty, n) = set.match_protocol_prefix("https://ex.com").unwrap();
        assert_eq!(ty.to_ascii_lowercase(), "https");
        assert_eq!(n, 6);
    }

    #[test]
    fn protocol_set_case_insensitive() {
        let set = ProtocolSet::defaults();
        let (ty, _) = set.match_protocol_prefix("HTTPS://x").unwrap();
        assert!(ty.eq_ignore_ascii_case("https"));
    }

    #[test]
    fn protocol_set_unregistered_not_matched() {
        assert!(
            ProtocolSet::defaults()
                .match_protocol_prefix("doi:10.1")
                .is_none()
        );
    }

    #[test]
    fn entity_get_user_then_builtin() {
        let cfg = ParseConfig::default();
        assert!(cfg.is_entity_name("alpha"));
        assert_eq!(cfg.entity_get("alpha").unwrap().utf8(), "\u{3b1}");
        assert!(cfg.is_entity_name("_ "));
        assert!(!cfg.is_entity_name("alphax"));
        assert!(!cfg.is_entity_name("myent"));

        let custom = EntityDef::new("myent", "\\my", false, "&m;", "m", "m", "m");
        let cfg = ParseConfig::default().with_user_entities([custom]);
        assert!(cfg.is_entity_name("myent"));
        assert!(cfg.is_entity_name("alpha"), "builtins remain");
        assert_eq!(cfg.entity_get("myent").unwrap().html(), "&m;");

        // User shadows builtin by exact name.
        let shadow = EntityDef::new("alpha", "\\A", false, "A", "A", "A", "A");
        let cfg = ParseConfig::default().with_user_entities([shadow]);
        assert_eq!(cfg.entity_get("alpha").unwrap().html(), "A");
    }

    /// `with_user_entities` replaces the map wholesale (same as todo/link builders).
    #[test]
    fn with_user_entities_replaces_not_merges() {
        let a = EntityDef::new("enta", "\\a", false, "a", "a", "a", "a");
        let b = EntityDef::new("entb", "\\b", false, "b", "b", "b", "b");
        let cfg = ParseConfig::default()
            .with_user_entities([a])
            .with_user_entities([b]);
        assert!(cfg.is_entity_name("entb"));
        assert!(!cfg.is_entity_name("enta"), "previous user map replaced");
        assert!(cfg.is_entity_name("alpha"), "builtins still resolve");
    }

    /// Unsafe names never enter the user map: absent as user entries after
    /// `with_user_entities`, and cannot invent entities that are not builtin.
    #[test]
    fn with_user_entities_drops_unsafe_names() {
        let bad_ws = EntityDef::new("_ ", "X", false, "X", "X", "X", "X");
        let bad_digit = EntityDef::new("1abc", "Y", false, "Y", "Y", "Y", "Y");
        let bad_punct = EntityDef::new("foo-bar", "Z", false, "Z", "Z", "Z", "Z");
        let bad_empty = EntityDef::new("", "E", false, "E", "E", "E", "E");
        let good = EntityDef::new("ok1", "\\ok", false, "&ok;", "ok", "ok", "ok");
        let cfg = ParseConfig::default()
            .with_user_entities([bad_ws, bad_digit, bad_punct, bad_empty, good]);

        // Accepted name is present with user fields.
        assert_eq!(cfg.entity_get("ok1").unwrap().html(), "&ok;");

        // Dropped names are absent as user entities.
        assert!(!cfg.is_entity_name("1abc"));
        assert!(!cfg.is_entity_name("foo-bar"));
        assert!(!cfg.is_entity_name(""));

        // Whitespace form still resolves only from the builtin table, not the
        // dropped user def (latex would be "X" if the user row had registered).
        let ws = cfg.entity_get("_ ").expect("builtin whitespace entity");
        assert_ne!(ws.latex(), "X");
        assert!(ws.latex().contains("hspace"));
    }

    /// User shadow of a builtin name is visible end-to-end: parse green form,
    /// project name, then `entity_get` returns user fields.
    #[test]
    fn entity_shadow_parse_form_and_get() {
        use crate::entities::entity_form;
        use crate::syntax::OrgSyntaxKind;
        use crate::tests::{nodes_of_kind, parse_document_tree_with_config, tree_text};

        let shadow = EntityDef::new("alpha", "\\A", false, "A-html", "A", "A", "A");
        let cfg = ParseConfig::default().with_user_entities([shadow]);
        let src = "x\\alpha y\n";
        let root = parse_document_tree_with_config(src, &cfg);
        assert_eq!(tree_text(&root), src);
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 1);
        let text = ents[0].text().to_string();
        assert_eq!(text, "\\alpha");
        let form = entity_form(&text).expect("green form");
        assert_eq!(form.name, "alpha");
        assert!(!form.use_brackets);
        assert_eq!(cfg.entity_get(form.name).unwrap().html(), "A-html");
    }
}
