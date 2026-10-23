//! Diagnostic taxonomy: kinds, severity, messages, and string-field constructors.
//!
//! Unit-like variants (`MalformedClock`, `UnterminatedLink`, ...) are constructed
//! directly at call sites. Truncating constructors exist only for string-bearing
//! kinds so hostile multi-MB fields cannot inflate diagnostic storage.

use std::borrow::Cow;
use std::fmt;

use rowan::TextRange;

/// Cap for user-derived strings stored on diagnostic kinds (block name, cookie
/// value, planning keyword). Full source is retained on the document; kinds only
/// need a short display slice. Truncation is UTF-8 safe and appends `...`.
const DIAG_FIELD_MAX_BYTES: usize = 64;

/// How serious a diagnostic is. Even [`Severity::Error`] does not abort parsing.
///
/// Non-exhaustive so new severity levels can be added in a minor release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Severity {
    /// Recognised construct abandoned or structurally broken (often fallthrough).
    Error,
    /// Construct kept with degraded meaning, or a near-miss the author likely
    /// intended as a valid form.
    Warning,
}

/// Machine-matchable diagnostic category.
///
/// Human-readable text is derived in [`DiagnosticKind::message`]. Prefer matching
/// on `kind` in tests and tools rather than message substrings.
///
/// # Truncation
///
/// In diagnostics produced by the parser, string fields are truncated to at
/// most 64 bytes (on a UTF-8 boundary, with a trailing `...` when cut), so a
/// huge single-line name cannot inflate diagnostic storage. The full text is
/// still in the document. A kind built directly from its public variant is
/// not truncated, so do not assume a field length.
///
/// Non-exhaustive so new kinds can be added in a minor release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// Priority cookie shape `[#VALUE]` where VALUE fails Org's value grammar
    /// (e.g. lowercase letter, number above 64, leading zero on two digits).
    InvalidPriorityCookie {
        /// The invalid value text as written (truncated when parser-emitted).
        value: String,
    },
    /// `#+begin_NAME` / `#+begin: NAME` with no matching end line.
    UnterminatedBlock {
        /// Block name as written (truncated when parser-emitted). Empty-name
        /// openers use [`DiagnosticKind::EmptyBlockName`] instead.
        name: String,
        /// `true` for the dynamic `#+begin:` form.
        dynamic: bool,
    },
    /// `#+begin_` / `#+begin:` with no name token.
    EmptyBlockName {
        /// `true` for the dynamic `#+begin:` form.
        dynamic: bool,
    },
    /// Drawer opener `:NAME:` with no matching `:END:` line before the next
    /// heading or the region end.
    UnterminatedDrawer {
        /// Drawer name as written (truncated when parser-emitted).
        name: String,
    },
    /// `\begin{NAME}` with no matching `\end{NAME}` before the next heading
    /// or the region end (heading-section half-open bound).
    UnterminatedLatexEnvironment {
        /// Environment name as written (truncated when parser-emitted).
        name: String,
    },
    /// Line opens with `CLOCK:` but the body is not a legal clock shape.
    ///
    /// The line falls through to a paragraph, as an unterminated block does.
    MalformedClock,
    /// Planning keyword with no timestamp opener after it.
    ///
    /// Not used when an opener is present but the stamp fails; that case is
    /// [`DiagnosticKind::MalformedTimestamp`] or
    /// [`DiagnosticKind::UnbalancedDiarySexp`]. An opener that does not look
    /// like a timestamp at all (`<not-a-date>`, `[fn:1]`) produces neither.
    PlanningMissingTimestamp {
        /// Keyword text as written (`SCHEDULED`, `DEADLINE`, or `CLOSED`).
        /// Truncated when parser-emitted.
        keyword: String,
    },
    /// Regular link `[[...` started but not completed (missing closers, etc.).
    UnterminatedLink,
    /// Regular link with an empty target (`[[]]` or `[[` then newline).
    EmptyLinkTarget,
    /// Timestamp opener where the body looked date-like but the stamp failed.
    MalformedTimestamp,
    /// Diary form `<%%(...` with unbalanced parentheses or missing closer.
    ///
    /// Severity is [`Severity::Error`]: `%%(` unambiguously starts a diary
    /// sexp, unlike a date-like near miss, which is a warning.
    UnbalancedDiarySexp,
    /// `@@BACKEND:` opened an export snippet with no closing `@@` in the
    /// element.
    ///
    /// Org itself is silent here (the text is simply not a snippet). It is
    /// reported for the same reason as [`Self::UnterminatedLink`]: `@@`, a
    /// valid back-end name and `:` are unlikely to be prose.
    UnterminatedExportSnippet,
    /// Nesting budget (`ParseConfig::max_nesting_depth`) exhausted: the region's
    /// bytes are kept verbatim as plain tiles, but its inner structure is not
    /// parsed.
    ///
    /// Emitted once per over-budget region, since nothing inside it is parsed
    /// further. A headline deeper than the outline budget is instead attached
    /// at the deepest allowed level, silently: only nesting is lost there.
    NestingLimitExceeded,
}

impl DiagnosticKind {
    /// Priority cookie with invalid value (truncates `value`).
    #[must_use]
    pub(crate) fn invalid_priority(value: &str) -> Self {
        Self::InvalidPriorityCookie {
            value: truncate_diag_field(value),
        }
    }

    /// Unterminated block opener (truncates `name`).
    #[must_use]
    pub(crate) fn unterminated_block(name: &str, dynamic: bool) -> Self {
        Self::UnterminatedBlock {
            name: truncate_diag_field(name),
            dynamic,
        }
    }

    /// Unterminated drawer opener (truncates `name`).
    #[must_use]
    pub(crate) fn unterminated_drawer(name: &str) -> Self {
        Self::UnterminatedDrawer {
            name: truncate_diag_field(name),
        }
    }

    /// Unterminated latex-environment opener (truncates `name`).
    #[must_use]
    pub(crate) fn unterminated_latex_environment(name: &str) -> Self {
        Self::UnterminatedLatexEnvironment {
            name: truncate_diag_field(name),
        }
    }

    /// Planning keyword with no timestamp opener (truncates `keyword`).
    #[must_use]
    pub(crate) fn planning_missing(keyword: &str) -> Self {
        Self::PlanningMissingTimestamp {
            keyword: truncate_diag_field(keyword),
        }
    }

    /// Severity for this kind.
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self {
            Self::InvalidPriorityCookie { .. }
            | Self::PlanningMissingTimestamp { .. }
            | Self::MalformedTimestamp => Severity::Warning,
            Self::UnterminatedBlock { .. }
            | Self::EmptyBlockName { .. }
            | Self::UnterminatedDrawer { .. }
            | Self::UnterminatedLatexEnvironment { .. }
            | Self::MalformedClock
            | Self::UnterminatedLink
            | Self::EmptyLinkTarget
            | Self::UnterminatedExportSnippet
            // Inner constructs are not parsed, even though every byte is
            // still in the tree.
            | Self::NestingLimitExceeded
            // `%%(` unambiguously starts a diary sexp.
            | Self::UnbalancedDiarySexp => Severity::Error,
        }
    }

    /// Human-readable description (stable wording for tooling).
    ///
    /// Static arms borrow; formatted arms allocate. Prefer matching on
    /// [`DiagnosticKind`] rather than message text.
    ///
    /// Not HTML- or log-safe: user-derived fields are interpolated without
    /// escaping control characters, ANSI, or markup. Consumers that display or
    /// log this string must treat it as untrusted and escape as needed.
    #[must_use]
    pub fn message(&self) -> Cow<'static, str> {
        match self {
            Self::InvalidPriorityCookie { value } => {
                Cow::Owned(format!("invalid priority cookie value: {value}"))
            }
            Self::UnterminatedDrawer { name } => Cow::Owned(format!(
                "unterminated drawer `:{name}:`; treated as a paragraph"
            )),
            Self::UnterminatedLatexEnvironment { name } => Cow::Owned(format!(
                "unterminated LaTeX environment `\\begin{{{name}}}`; treated as a paragraph"
            )),
            Self::UnterminatedBlock { name, dynamic } => {
                if *dynamic {
                    Cow::Owned(format!(
                        "unterminated block `#+begin: {name}`; treated as a paragraph"
                    ))
                } else {
                    Cow::Owned(format!(
                        "unterminated block `#+begin_{name}`; treated as a paragraph"
                    ))
                }
            }
            Self::EmptyBlockName { dynamic } => {
                if *dynamic {
                    Cow::Borrowed("block opener `#+begin:` has no name; treated as a paragraph")
                } else {
                    Cow::Borrowed("block opener `#+begin_` has no name; treated as a paragraph")
                }
            }
            Self::MalformedClock => Cow::Borrowed("malformed CLOCK: line; treated as a paragraph"),
            Self::PlanningMissingTimestamp { keyword } => {
                Cow::Owned(format!("planning keyword `{keyword}` has no timestamp"))
            }
            Self::UnterminatedLink => Cow::Borrowed("unterminated link"),
            Self::EmptyLinkTarget => Cow::Borrowed("empty link target"),
            Self::MalformedTimestamp => Cow::Borrowed("malformed timestamp"),
            Self::UnbalancedDiarySexp => Cow::Borrowed("unbalanced diary sexp timestamp"),
            Self::UnterminatedExportSnippet => {
                Cow::Borrowed("unterminated export snippet; expected a closing `@@`")
            }
            Self::NestingLimitExceeded => {
                Cow::Borrowed("nesting limit exceeded; inner structure kept as plain text")
            }
        }
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// Truncate a user-derived field for storage on a diagnostic kind.
///
/// Grammar rules call the truncating constructors on [`DiagnosticKind`]
/// instead.
fn truncate_diag_field(s: &str) -> String {
    if s.len() <= DIAG_FIELD_MAX_BYTES {
        return s.to_string();
    }
    let mut end = DIAG_FIELD_MAX_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    out.push_str(&s[..end]);
    out.push_str("...");
    out
}

/// A single recoverable issue, located in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    range: TextRange,
}

impl Diagnostic {
    /// Construct a diagnostic.
    pub(crate) fn new(kind: DiagnosticKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// Machine-matchable category.
    #[must_use]
    pub fn kind(&self) -> &DiagnosticKind {
        &self.kind
    }

    /// Severity derived from the kind.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.kind.severity()
    }

    /// Human-readable message derived from the kind.
    ///
    /// Static-message kinds return a borrowed `Cow`; formatted kinds allocate.
    /// Not HTML- or log-safe -- see [`DiagnosticKind::message`].
    #[must_use]
    pub fn message(&self) -> Cow<'static, str> {
        self.kind.message()
    }

    /// Source range associated with the diagnostic.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Write through the kind so static messages avoid a second String alloc.
        fmt::Display::fmt(&self.kind, f)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_constructors_truncate_long_fields() {
        let long = "a".repeat(100);
        for kind in [
            DiagnosticKind::invalid_priority(&long),
            DiagnosticKind::planning_missing(&long),
            DiagnosticKind::unterminated_block(&long, false),
            DiagnosticKind::unterminated_drawer(&long),
            DiagnosticKind::unterminated_latex_environment(&long),
        ] {
            let field = match &kind {
                DiagnosticKind::InvalidPriorityCookie { value } => value.as_str(),
                DiagnosticKind::PlanningMissingTimestamp { keyword } => keyword.as_str(),
                DiagnosticKind::UnterminatedBlock { name, .. }
                | DiagnosticKind::UnterminatedDrawer { name }
                | DiagnosticKind::UnterminatedLatexEnvironment { name } => name.as_str(),
                other => panic!("unexpected kind: {other:?}"),
            };
            assert!(field.ends_with("..."), "kind={kind:?}");
            assert!(
                field.len() <= DIAG_FIELD_MAX_BYTES + 3,
                "field len {} for {kind:?}",
                field.len()
            );
        }
        let short = DiagnosticKind::planning_missing("SCHEDULED");
        match short {
            DiagnosticKind::PlanningMissingTimestamp { keyword } => {
                assert_eq!(keyword, "SCHEDULED");
            }
            other => panic!("expected PlanningMissingTimestamp, got {other:?}"),
        }
        let block = DiagnosticKind::unterminated_block("src", false);
        match block {
            DiagnosticKind::UnterminatedBlock {
                name,
                dynamic: false,
            } => {
                assert_eq!(name, "src");
            }
            other => panic!("expected UnterminatedBlock, got {other:?}"),
        }
        let env = DiagnosticKind::unterminated_latex_environment("equation");
        match &env {
            DiagnosticKind::UnterminatedLatexEnvironment { name } => {
                assert_eq!(name, "equation");
            }
            other => panic!("expected UnterminatedLatexEnvironment, got {other:?}"),
        }
        assert!(
            env.message().contains("\\begin{equation}"),
            "message={}",
            env.message()
        );
    }

    #[test]
    fn truncate_diag_field_caps_long_input() {
        let long = "a".repeat(100);
        let t = truncate_diag_field(&long);
        assert!(t.ends_with("..."));
        assert!(t.len() <= DIAG_FIELD_MAX_BYTES + 3);
        assert_eq!(truncate_diag_field("short"), "short");
    }

    #[test]
    fn truncate_diag_field_utf8_straddle_is_char_safe() {
        let mut s = "a".repeat(63);
        s.push('\u{00e9}'); // 2-byte UTF-8; byte 64 is mid-codepoint
        assert_eq!(s.len(), 65);
        let t = truncate_diag_field(&s);
        assert!(t.ends_with("..."));
        assert!(t.is_char_boundary(t.len() - 3));
        assert!(t.len() <= DIAG_FIELD_MAX_BYTES + 3);
        // Cut backs up to the last char boundary (all 63 ASCII a's).
        assert_eq!(&t[..t.len() - 3], "a".repeat(63));
        // Round-trip through UTF-8 (String is always valid; assert explicitly).
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn severity_table_is_exhaustive() {
        // Every kind maps to the documented severity. String-bearing variants
        // are built with the truncating constructors, as the parser does.
        let cases: &[(DiagnosticKind, Severity)] = &[
            (DiagnosticKind::invalid_priority("a"), Severity::Warning),
            (
                DiagnosticKind::unterminated_block("src", false),
                Severity::Error,
            ),
            (
                DiagnosticKind::EmptyBlockName { dynamic: false },
                Severity::Error,
            ),
            (DiagnosticKind::unterminated_drawer("LOG"), Severity::Error),
            (
                DiagnosticKind::unterminated_latex_environment("equation"),
                Severity::Error,
            ),
            (DiagnosticKind::MalformedClock, Severity::Error),
            (
                DiagnosticKind::planning_missing("SCHEDULED"),
                Severity::Warning,
            ),
            (DiagnosticKind::UnterminatedLink, Severity::Error),
            (DiagnosticKind::EmptyLinkTarget, Severity::Error),
            (DiagnosticKind::MalformedTimestamp, Severity::Warning),
            (DiagnosticKind::UnbalancedDiarySexp, Severity::Error),
            (DiagnosticKind::UnterminatedExportSnippet, Severity::Error),
            (DiagnosticKind::NestingLimitExceeded, Severity::Error),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.severity(), *expected, "severity mismatch for {kind:?}");
        }
    }
}
