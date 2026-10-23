//! Events and markers - the decision log replayed by the builder.

use super::Parser;
use crate::syntax::OrgSyntaxKind;
use rowan::TextRange;

/// A flat record of grammar decisions.
///
/// The builder replays this stream against the source to produce a rowan green
/// tree. Keeping decisions separate from tree construction is what makes
/// backtracking O(1): on a failed alternative you simply truncate the `Vec`.
///
/// Leaf events always carry absolute source ranges. A single lex tile may
/// appear as several adjacent [`Event::Token`] pieces when a measured leaf
/// ends mid-tile (cursor stays at that exclusive end; the parent loop emits
/// the remainder as ordinary text).
///
/// `Tombstone` has two distinct roles:
/// - The placeholder pushed by `start()`. `complete()` later overwrites it.
/// - The residue left by `abandon()`. The builder skips `Tombstone`s.
#[derive(Debug)]
pub(crate) enum Event {
    /// Open a node. `start()` first pushes a `Tombstone` as a placeholder;
    /// `complete()` rewrites that slot into `Start { kind }`.
    Start { kind: OrgSyntaxKind },

    /// Emit a green token covering the absolute source `range`.
    ///
    /// `kind` labels the piece (usually the lex kind). Ranges tile the source
    /// gap-free under DOCUMENT (builder enforces sequential coverage).
    Token {
        kind: OrgSyntaxKind,
        range: TextRange,
    },

    /// Close the most recently opened node.
    Finish,

    /// Placeholder or abandoned node; see [`Event`] and `Marker::abandon`.
    Tombstone,
}

/// Panics on drop unless [`DropBomb::defuse`] was called.
///
/// Armed in release builds too: a leaked marker is a grammar bug, not bad
/// input. Stays quiet while the thread is already unwinding.
struct DropBomb {
    armed: bool,
}

impl DropBomb {
    fn armed() -> Self {
        Self { armed: true }
    }

    fn defuse(&mut self) {
        self.armed = false;
    }
}

impl Drop for DropBomb {
    // Not an `assert!`: the bomb must stay quiet when the thread is already unwinding.
    #[allow(clippy::manual_assert)]
    fn drop(&mut self) {
        if self.armed && !std::thread::panicking() {
            panic!(
                "Marker dropped without complete() or abandon() \
                 (event-parser lifecycle leak)"
            );
        }
    }
}

/// An open, not-yet-named node.
///
/// Created by [`Parser::start`]. Must be closed by [`Self::complete`] or
/// dropped by [`Self::abandon`]. Forgetting either leaves a tombstone and,
/// thanks to the drop bomb, panics when the marker goes out of scope.
///
/// The `pos` field is the event index of the placeholder `Tombstone`.
#[must_use = "Markers must be completed or abandoned"]
pub(crate) struct Marker {
    pos: usize,
    bomb: DropBomb,
}

impl Marker {
    /// Finish the node with the given kind and emit `Finish`.
    ///
    /// The placeholder `Tombstone` at this marker's index is rewritten into
    /// `Start { kind }`, then a `Finish` event is appended.
    pub(crate) fn complete(mut self, p: &mut Parser, kind: OrgSyntaxKind) {
        self.bomb.defuse();
        p.events[self.pos] = Event::Start { kind };
        p.events.push(Event::Finish);
    }

    /// Abandon the node.
    ///
    /// If nothing has been emitted after the placeholder, the `Tombstone` is
    /// popped for cleanliness. Otherwise it is left in place; the builder
    /// skips `Tombstone` events when constructing the green tree.
    ///
    /// Does **not** rewind the cursor or events after the marker; paths that
    /// may fail after consuming must run under `Parser::attempt`.
    pub(crate) fn abandon(mut self, p: &mut Parser) {
        self.bomb.defuse();
        if self.pos == p.events.len() - 1 {
            p.events.pop();
        }
    }
}

impl Parser<'_> {
    /// Begin a node.
    ///
    /// Returns a [`Marker`] that must later be completed or abandoned.
    pub(crate) fn start(&mut self) -> Marker {
        let event_pos = self.events.len();
        self.events.push(Event::Tombstone);
        Marker {
            pos: event_pos,
            bomb: DropBomb::armed(),
        }
    }
}
