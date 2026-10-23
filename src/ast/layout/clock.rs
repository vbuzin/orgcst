//! Clock duration token walk.

use super::span_of;
use crate::TextRange;
use crate::syntax::{OrgSyntaxKind, SyntaxElement, SyntaxNode};

// ---------------------------------------------------------------------------
// Clock duration token run
// ---------------------------------------------------------------------------

/// Kind sequence of `=> H:MM`. Must stay aligned with
/// `forms/clock.rs::duration_token_len`.
const CLOCK_DURATION_KINDS: [OrgSyntaxKind; 6] = [
    OrgSyntaxKind::EQUALS,
    OrgSyntaxKind::R_ANGLE,
    OrgSyntaxKind::WHITESPACE,
    OrgSyntaxKind::TEXT,
    OrgSyntaxKind::COLON,
    OrgSyntaxKind::TEXT,
];

fn clock_duration_run(clock: &SyntaxNode) -> Option<[SyntaxElement; 6]> {
    let kids: Vec<SyntaxElement> = clock.children_with_tokens().collect();
    let i = kids.windows(6).position(|w| {
        w.iter()
            .zip(CLOCK_DURATION_KINDS)
            .all(|(e, k)| e.kind() == k)
    })?;
    Some([
        kids[i].clone(),
        kids[i + 1].clone(),
        kids[i + 2].clone(),
        kids[i + 3].clone(),
        kids[i + 4].clone(),
        kids[i + 5].clone(),
    ])
}

/// Absolute span of the `=> H:MM` run inside a `CLOCK` node.
///
/// Shape (kinds only; grammar-proved by `forms/clock.rs::duration_token_len`):
/// `EQUALS R_ANGLE WHITESPACE TEXT COLON TEXT`.
/// Trailing whitespace after the minutes is not included.
#[must_use]
pub(crate) fn clock_duration_range(clock: &SyntaxNode) -> Option<TextRange> {
    span_of(clock_duration_run(clock)?)
}

/// Absolute span of the `H:MM` value (last three tokens of the duration run).
#[must_use]
pub(crate) fn clock_duration_value_range(clock: &SyntaxNode) -> Option<TextRange> {
    let run = clock_duration_run(clock)?;
    span_of(run[3..].iter().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::first_kind;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn clock_duration_range_three_shapes() {
        let running = first_kind("CLOCK: [2020-01-01 Wed 10:00]\n", OrgSyntaxKind::CLOCK);
        assert!(clock_duration_range(&running).is_none());

        let closed = first_kind(
            "CLOCK: [2020-01-01 Wed 10:00]--[2020-01-01 Wed 11:00] => 1:00\n",
            OrgSyntaxKind::CLOCK,
        );
        let doc = parse("CLOCK: [2020-01-01 Wed 10:00]--[2020-01-01 Wed 11:00] => 1:00\n");
        let carved = clock_duration_range(&closed).expect("duration");
        assert_eq!(&doc.source()[carved], "=> 1:00");
        let value = clock_duration_value_range(&closed).expect("value");
        assert_eq!(&doc.source()[value], "1:00");

        let only = first_kind("CLOCK: => 2:15\n", OrgSyntaxKind::CLOCK);
        let doc = parse("CLOCK: => 2:15\n");
        let carved = clock_duration_range(&only).expect("duration-only");
        assert_eq!(&doc.source()[carved], "=> 2:15");
        assert_eq!(
            &doc.source()[clock_duration_value_range(&only).expect("value-only")],
            "2:15"
        );
        assert!(clock_duration_value_range(&running).is_none());
    }
}
