//! Typed absolute positions: opaque token and line handles for grammar rules.
//!
//! A [`Pos`] or [`LineId`] is obtained from views and scans, never from
//! arithmetic. Positions are absolute token/line indices under the hood, so
//! they stay valid across consumption (the lex token vector and line table are
//! fixed for the whole parse). Region caps take a `Pos`; the exclusive visible
//! end ([`super::Parser::limit`]) is itself a `Pos`.
//!
//! The raw index accessors are for the parser's own views, consumption and
//! diagnostics; grammar rules pass the handles through unopened.

/// Absolute token index as an opaque ordered handle.
///
/// `Pos` at the visible cap equals [`super::Parser::limit`] (exclusive end).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Pos(u32);

impl Pos {
    /// Wrap an absolute token index.
    #[inline]
    pub(crate) fn from_index(i: usize) -> Self {
        debug_assert!(u32::try_from(i).is_ok(), "token index exceeds u32");
        Pos(i as u32)
    }

    /// Raw absolute token index.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Absolute line index as an opaque ordered handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LineId(u32);

impl LineId {
    /// Wrap an absolute line index.
    #[inline]
    pub(crate) fn from_index(i: usize) -> Self {
        debug_assert!(u32::try_from(i).is_ok(), "line index exceeds u32");
        LineId(i as u32)
    }

    /// Raw absolute line index (for line identity: drawer `end_line`,
    /// frame tables).
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Opaque source-progress key: token index plus mid-tile byte offset.
///
/// A rule that claims to have parsed something must advance it. Token index alone is not
/// enough when a measured leaf ends mid-tile without advancing the token
/// cursor. Compare for equality only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Progress {
    pos: u32,
    byte_off: u32,
}

impl Progress {
    #[inline]
    pub(crate) fn new(pos: usize, byte_off: usize) -> Self {
        debug_assert!(
            u32::try_from(pos).is_ok(),
            "progress token index exceeds u32"
        );
        debug_assert!(
            u32::try_from(byte_off).is_ok(),
            "progress byte offset exceeds u32"
        );
        Progress {
            pos: pos as u32,
            byte_off: byte_off as u32,
        }
    }

    /// Mid-tile byte offset component (test assertions on measured ends).
    #[cfg(test)]
    pub(crate) fn mid_offset(self) -> usize {
        self.byte_off as usize
    }
}
