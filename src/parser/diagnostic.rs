//! Recoverable parse diagnostics (parsing itself is infallible).
//!
//! A diagnostic is emitted when a construct's opening matched but the rest
//! did not (a near miss), or when a construct is kept with degraded meaning
//! (e.g. planning without a timestamp). Plain non-matches and markup PRE/POST
//! failures stay silent. Diagnostics never abort the parse or change the tree.
//!
//! Diagnostics survive backtracking and are deduplicated where they are
//! recorded ([`emit`], [`super::Parser::error_range`]), so grammar rules
//! always report a near miss and leave duplicates to the sink.
//!
//! Severity (see also [`Severity`]):
//! - **Error**: an abandoned construct that falls through to plain text
//!   (blocks, clock, links), or a diary `%%(` with unbalanced parentheses.
//! - **Warning**: a construct kept with degraded meaning (planning without a
//!   timestamp, invalid priority) or a date-like timestamp near miss.
//!
//! [`kind`] holds the kinds, messages and truncating constructors; [`emit`]
//! holds the helpers grammar rules call, all on [`super::Parser`]:
//! - [`error_ahead`](super::Parser::error_ahead) -- next `count` tokens from
//!   the cursor (multi-token reject before any eats).
//! - [`error_line_at`](super::Parser::error_line_at) -- line span from a typed
//!   entry [`super::Pos`] through the last non-NEWLINE token.
//! - [`error_span`](super::Parser::error_span) -- tokens `[a, b)` as typed
//!   positions.
//! - [`error_span_to_cursor`](super::Parser::error_span_to_cursor) -- construct
//!   entry through the cursor, with line-span fallback when still at entry.
//! - [`error_range`](super::Parser::error_range) -- low-level sink and dedupe
//!   point (explicit [`rowan::TextRange`]).

mod emit;
mod kind;

pub use kind::{Diagnostic, DiagnosticKind, Severity};
