//! Event stream to rowan green tree.
//!
//! Replays the parser's `Event`s, taking each leaf's text from the absolute
//! range on its [`Event::Token`]. A single lex tile may appear as several
//! adjacent green tokens when a measured leaf ends inside it.
//!
//! Checked in release builds too: every token range is in bounds, sits on
//! UTF-8 char boundaries, lies inside one lex tile, and the ranges tile the
//! source in order with no gaps or overlaps. A violation panics; the builder
//! never invents structure or drops bytes. Tile containment is checked with a
//! tile index that only moves forward as the events do.

use crate::lexer::LexedToken;
use crate::parser::Event;
use crate::syntax::OrgSyntaxKind;
use rowan::{GreenNodeBuilder, TextRange};

/// Build a green tree from `source`, its tokens, and the grammar event stream.
pub(crate) fn build(source: &str, tokens: &[LexedToken], events: Vec<Event>) -> rowan::GreenNode {
    let mut builder = GreenNodeBuilder::new();
    let mut next_byte = 0usize;
    // Monotonic lex-tile cursor: advances only forward as event ranges move.
    let mut tile_i = 0usize;

    for event in events {
        match event {
            Event::Start { kind } => {
                builder.start_node(kind.into());
            }
            Event::Finish => {
                builder.finish_node();
            }
            Event::Tombstone => {}
            Event::Token { kind, range } => {
                advance_tile_cursor(tokens, range, &mut tile_i);
                next_byte = emit_token_checked(&mut builder, source, kind, range, next_byte);
            }
        }
    }

    assert_eq!(
        next_byte,
        source.len(),
        "builder event ranges must cover the full source (got next_byte={next_byte}, len={})",
        source.len()
    );

    // Cross-check: lex tiles also cover the full source (fixed tiling).
    if let Some(last) = tokens.last() {
        assert_eq!(
            usize::from(last.range.end()),
            source.len(),
            "lex tiles must end at source.len()"
        );
    } else {
        assert!(source.is_empty(), "empty token list only for empty source");
    }

    builder.finish()
}

/// Advance `tile_i` until `range` can sit in `tokens[tile_i]`, then require
/// containment. Multi-piece ranges on the same tile leave `tile_i` unchanged;
/// ranges past the current tile advance it. Cross-tile spans and ranges with
/// no containing tile panic (always on).
fn advance_tile_cursor(tokens: &[LexedToken], range: TextRange, tile_i: &mut usize) {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    while *tile_i < tokens.len() {
        let te = usize::from(tokens[*tile_i].range.end());
        if start >= te {
            *tile_i += 1;
            continue;
        }
        break;
    }
    assert!(
        *tile_i < tokens.len(),
        "Token event range {start}..{end} is past all lex tiles"
    );
    let ts = usize::from(tokens[*tile_i].range.start());
    let te = usize::from(tokens[*tile_i].range.end());
    assert!(
        start >= ts && end <= te,
        "Token event range {start}..{end} must lie inside a single lex tile \
         (current tile {ts}..{te})"
    );
}

/// Emit one range-bearing token; enforce sequential gap-free coverage.
fn emit_token_checked(
    builder: &mut GreenNodeBuilder,
    source: &str,
    kind: OrgSyntaxKind,
    range: TextRange,
    next_byte: usize,
) -> usize {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    assert!(
        start < end,
        "Token range must be non-empty (got {start}..{end})"
    );
    assert!(
        end <= source.len(),
        "Token range end {end} past source len {}",
        source.len()
    );
    assert!(
        source.is_char_boundary(start) && source.is_char_boundary(end),
        "Token range {start}..{end} must sit on UTF-8 char boundaries"
    );
    assert_eq!(
        start, next_byte,
        "Token ranges must tile sequentially without gaps or overlaps \
         (expected start {next_byte}, got {start})"
    );
    builder.token(kind.into(), &source[start..end]);
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::Event;
    use crate::syntax::OrgSyntaxKind;
    use rowan::{TextRange, TextSize};

    fn text_of(green: &rowan::GreenNode) -> String {
        let node = rowan::SyntaxNode::<crate::syntax::OrgLanguage>::new_root(green.clone());
        node.text().to_string()
    }

    /// Wrap events in a DOCUMENT root so `GreenNodeBuilder::finish` is valid.
    fn with_document(inner: Vec<Event>) -> Vec<Event> {
        let mut evs = Vec::with_capacity(inner.len() + 2);
        evs.push(Event::Start {
            kind: OrgSyntaxKind::DOCUMENT,
        });
        evs.extend(inner);
        evs.push(Event::Finish);
        evs
    }

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(TextSize::from(start), TextSize::from(end))
    }

    #[test]
    fn mid_token_pieces_roundtrip() {
        // Lex tiles "ab." as one TEXT; path "ab" + bare "." under DOCUMENT.
        let src = "ab.";
        let tokens = lex(src).tokens;
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, OrgSyntaxKind::TEXT);

        let events = with_document(vec![
            Event::Start {
                kind: OrgSyntaxKind::LINK,
            },
            Event::Start {
                kind: OrgSyntaxKind::LINK_PATH,
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 2),
            },
            Event::Finish,
            Event::Finish,
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(2, 3),
            },
        ]);
        let green = build(src, &tokens, events);
        assert_eq!(text_of(&green), src);

        let root = rowan::SyntaxNode::<crate::syntax::OrgLanguage>::new_root(green);
        let path = root
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK_PATH)
            .expect("LINK_PATH");
        assert_eq!(path.text().to_string(), "ab");
        let link = root
            .descendants()
            .find(|n| n.kind() == OrgSyntaxKind::LINK)
            .expect("LINK");
        assert_eq!(link.text().to_string(), "ab");
        let doc_children: Vec<_> = root.children_with_tokens().collect();
        let texts: Vec<String> = doc_children.iter().map(ToString::to_string).collect();
        assert!(
            texts.iter().any(|t| t == "."),
            "trail '.' must be bare under DOCUMENT, got {texts:?}"
        );
    }

    #[test]
    fn full_tokens_only_roundtrip() {
        let src = "a:b";
        let tokens = lex(src).tokens;
        let inner: Vec<Event> = tokens
            .iter()
            .map(|t| Event::Token {
                kind: t.kind,
                range: t.range,
            })
            .collect();
        let green = build(src, &tokens, with_document(inner));
        assert_eq!(text_of(&green), src);
    }

    #[test]
    fn multi_piece_same_tile_and_multi_tile_sequential() {
        // One TEXT tile split into three pieces, then full WHITESPACE + TEXT tiles.
        // Exercises monotonic tile_i: same index for pieces, then advance.
        let src = "abc de";
        let tokens = lex(src).tokens;
        assert!(tokens.len() >= 3, "TEXT WS TEXT");
        let t0_end = u32::from(tokens[0].range.end());
        assert_eq!(t0_end, 3); // "abc"

        let mut inner = vec![
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 1),
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(1, 2),
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(2, 3),
            },
        ];
        for t in tokens.iter().skip(1) {
            inner.push(Event::Token {
                kind: t.kind,
                range: t.range,
            });
        }
        let green = build(src, &tokens, with_document(inner));
        assert_eq!(text_of(&green), src);
    }

    #[test]
    fn mid_token_then_full_tokens_after() {
        // "xy. z" -- path "xy", trail ".", then WHITESPACE + TEXT.
        let src = "xy. z";
        let tokens = lex(src).tokens;
        let mut inner = vec![
            Event::Start {
                kind: OrgSyntaxKind::LINK_PATH,
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 2),
            },
            Event::Finish,
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(2, 3),
            },
        ];
        for t in tokens.iter().skip(1) {
            inner.push(Event::Token {
                kind: t.kind,
                range: t.range,
            });
        }
        let green = build(src, &tokens, with_document(inner));
        assert_eq!(text_of(&green), src);
    }

    #[test]
    fn adjacent_pieces_equal_original_tile() {
        let src = "hello.";
        let tokens = lex(src).tokens;
        assert_eq!(tokens.len(), 1);
        let events = with_document(vec![
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 5),
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(5, 6),
            },
        ]);
        let green = build(src, &tokens, events);
        assert_eq!(text_of(&green), src);
        assert_eq!(&src[..5], "hello");
        assert_eq!(&src[5..], ".");
    }

    /// The crate-wide losslessness guarantee, asserted once.
    ///
    /// `build` enforces gap-free sequential coverage of the whole source on
    /// every parse (with `assert!`, so in release builds too) and takes leaf
    /// text from `&source[start..end]`. Together those imply
    /// `root.text() == source` for any tree `build` returns, so construct
    /// tests need not repeat the check and can assert shape instead.
    ///
    /// Fixtures come from [`crate::tests::TAXONOMY`], which also feeds the
    /// test that every node kind is emitted.
    #[test]
    fn every_construct_round_trips_through_the_public_parse() {
        for &(src, cfg) in crate::tests::TAXONOMY {
            let root = crate::parse_with(src, &cfg.build()).root().clone();
            assert_eq!(
                root.text().to_string(),
                src,
                "public parse must be lossless for {src:?}"
            );
            assert_eq!(root.kind(), OrgSyntaxKind::DOCUMENT);
        }
    }

    #[test]
    #[should_panic(expected = "tile sequentially")]
    fn gap_in_ranges_panics() {
        let src = "ab";
        let tokens = lex(src).tokens;
        let events = with_document(vec![Event::Token {
            kind: OrgSyntaxKind::TEXT,
            range: range(1, 2),
        }]);
        let _ = build(src, &tokens, events);
    }

    #[test]
    #[should_panic(expected = "cover the full source")]
    fn under_coverage_panics() {
        let src = "ab";
        let tokens = lex(src).tokens;
        let events = with_document(vec![Event::Token {
            kind: OrgSyntaxKind::TEXT,
            range: range(0, 1),
        }]);
        let _ = build(src, &tokens, events);
    }

    #[test]
    #[should_panic(expected = "non-empty")]
    fn empty_range_panics() {
        let src = "ab";
        let tokens = lex(src).tokens;
        let events = with_document(vec![
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 0),
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, 2),
            },
        ]);
        let _ = build(src, &tokens, events);
    }

    #[test]
    #[should_panic(expected = "single lex tile")]
    fn range_past_source_panics() {
        // Past source and not inside any tile: tile containment fires first.
        let src = "ab";
        let tokens = lex(src).tokens;
        let events = with_document(vec![Event::Token {
            kind: OrgSyntaxKind::TEXT,
            range: range(0, 99),
        }]);
        let _ = build(src, &tokens, events);
    }

    #[test]
    #[should_panic(expected = "UTF-8 char boundaries")]
    fn mid_utf8_char_range_panics() {
        // "e\u{e9}" -- split the multi-byte acute e mid-code-unit.
        let src = "e\u{e9}";
        let tokens = lex(src).tokens;
        let acute = src.find('\u{e9}').expect("acute");
        let mid = acute + 1;
        assert!(!src.is_char_boundary(mid));
        let events = with_document(vec![
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(0, mid as u32),
            },
            Event::Token {
                kind: OrgSyntaxKind::TEXT,
                range: range(mid as u32, src.len() as u32),
            },
        ]);
        let _ = build(src, &tokens, events);
    }

    #[test]
    #[should_panic(expected = "single lex tile")]
    fn range_spanning_two_lex_tiles_panics() {
        let src = "a b";
        let tokens = lex(src).tokens;
        assert!(tokens.len() >= 2);
        // Span from first tile into second (illegal single event).
        let events = with_document(vec![Event::Token {
            kind: OrgSyntaxKind::TEXT,
            range: range(0, src.len() as u32),
        }]);
        let _ = build(src, &tokens, events);
    }
}
