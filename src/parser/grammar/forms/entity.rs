//! Entity object: `\NAME`, `\NAME{}`, and whitespace `\_` + spaces.
//!
//! Evidence:
//! - Worg 5.1 Entities / appendix 6.1
//! - `org-element-entity-parser` / `org-entity-get` / `org-entities`
//!
//! Recognition looks the name up through [`ParseConfig::is_entity_name`]
//! (user entities, then the builtin table). Emit is one
//! [`OrgSyntaxKind::ENTITY`] leaf of the measured byte length, which may end
//! mid-tile.
//!
//! **Backslash family** (`org-element--object-lex`): the dispatch branches on
//! the character after the backslash, it does not fall through. `\\` reaches
//! line-break and nothing else; every other `\X` tries entity, then
//! latex-fragment. This module covers the entity; the latex fragment lives in
//! `forms/latex.rs`, and `forms/inline.rs` dispatches between them.
//!
//! [`probe`] is pure and returns the full byte length; [`parse`] emits the
//! leaf from it.

use crate::parser::{ParseConfig, Parser};
use crate::syntax::OrgSyntaxKind;

/// Measured entity: exclusive source-byte length from the leading `\`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntityShape {
    /// Bytes from cursor through name (and optional `{}`).
    pub(crate) len: usize,
}

/// Pure measure of an entity at the cursor.
pub(crate) fn probe(parser: &Parser) -> Option<EntityShape> {
    if parser.current() != OrgSyntaxKind::BACKSLASH {
        return None;
    }
    let rest = parser.rest_source();
    // rest includes the leading `\` (cursor on backslash).
    let after_bs = rest.get(1..)?;
    let (name, name_end_in_after) = measure_entity_name(after_bs, parser.config)?;
    // Optional `{}` terminator (not part of whitespace entities).
    let mut end = 1 + name_end_in_after;
    if !name.starts_with('_') {
        let tail = rest.get(end..).unwrap_or("");
        if tail.starts_with("{}") {
            end += 2;
        }
    }
    Some(EntityShape { len: end })
}

/// Emit one entity from a measured shape (no remeasure).
pub(crate) fn parse(parser: &mut Parser, shape: EntityShape) {
    let ok = parser.leaf_len(OrgSyntaxKind::ENTITY, shape.len);
    debug_assert!(ok, "entity leaf fits the length probe measured");
}

/// Measure the entity NAME after a leading backslash.
///
/// Returns `(name_slice, exclusive_end_offset_in_after_bs)` or `None` when no
/// dictionary name matches with a legal POST.
///
/// Name paths (no regex; dictionary-driven):
/// 1. `"_" + (1+ spaces)` (spaces only, not tabs). Length is not capped here;
///    unknown lengths fail via [`ParseConfig::is_entity_name`] (builtin table has
///    1..=20 spaces). Org's `_ +` branch carries no POST group, so what
///    follows the spaces never matters.
/// 2. Longest leading ASCII alphanumeric run, then walk end from max down to
///    1 and accept the first name with a legal POST that is in the dictionary.
///    Digit-bearing builtin names (`there4`, `sup1`, `frac12`, ...) are table
///    data only, with no special case.
///
/// POST for non-whitespace names: end of input, or the next character is not a
/// letter (digits, punctuation and `{}` are all fine). The caller may then
/// consume `{}`.
///
/// **Deliberate superset over `org-element-entity-parser`.** Org's regexp can
/// only produce `there4 | sup[123] | frac[13][24] | [a-zA-Z]+` as the name, and
/// a dictionary miss aborts the parse (`throw 'no-object`) with no retry at a
/// shorter length. The walk above is equivalent for every builtin name: on a
/// pure-letter run only the full run has a legal POST, so shorter prefixes are
/// rejected before the dictionary is consulted, and the three digit-bearing
/// specials behave the same either way. It differs only for user entities whose
/// names contain digits. Org accepts those in `org-entities-user`
/// (`org-entities--user-safe-p` allows `[a-zA-Z][a-zA-Z0-9]*`) yet can never
/// match one: `\my2ent` yields the name `my`, which misses the dictionary and
/// aborts. This parser matches `\my2ent`, so documents relying on that parse
/// differently in Org.
fn measure_entity_name<'a>(after_bs: &'a str, config: &ParseConfig) -> Option<(&'a str, usize)> {
    let b = after_bs.as_bytes();
    if b.is_empty() {
        return None;
    }

    // Whitespace entity: _ + one or more spaces. Dictionary filters length.
    if b[0] == b'_' {
        let mut n = 0usize;
        while 1 + n < b.len() && b[1 + n] == b' ' {
            n += 1;
        }
        if n == 0 {
            return None;
        }
        let end = 1 + n;
        let name = &after_bs[..end];
        if config.is_entity_name(name) {
            return Some((name, end));
        }
        return None;
    }

    // Longest ASCII alphanumeric run; then longest dictionary prefix with
    // legal POST (next character not a letter, or end of input).
    let mut max = 0usize;
    while max < b.len() && b[max].is_ascii_alphanumeric() {
        max += 1;
    }
    if max == 0 {
        return None;
    }
    for end in (1..=max).rev() {
        // POST first: it is a single char test, and on a pure-letter run only
        // `end == max` can pass it, so this drops every other candidate without
        // a dictionary probe. The two checks are independent, so order is free.
        if !name_terminator_ok(after_bs, end) {
            continue;
        }
        let name = &after_bs[..end];
        if config.is_entity_name(name) {
            return Some((name, end));
        }
    }
    None
}

/// After a non-whitespace name, the next character must not be a letter.
///
/// End of input, `{}`, digits and punctuation are all legal POST. Org's rx
/// group is `(or eol "{}" (not letter))`, where `(not letter)` is
/// `[^[:alpha:]]` -- the Unicode letter category, not ASCII, so `\alpha` is not
/// an entity in `\alphax`, `\alphae`-acute, or `\alpha` + any other letter.
/// [`char::is_alphabetic`] is Unicode Alphabetic (letter categories plus `Nl` /
/// `Other_Alphabetic`); the difference is not reachable for an entity POST.
///
/// `end` always lands on an ASCII boundary (the caller walks alphanumeric
/// bytes), so the slice never splits a character.
fn name_terminator_ok(after_bs: &str, end: usize) -> bool {
    match after_bs.get(end..).and_then(|rest| rest.chars().next()) {
        None => true,
        Some(c) => !c.is_alphabetic(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::EntityDef;
    use crate::parser::ParseConfig;
    use crate::tests::{
        make_parser, node_text, nodes_of_kind, parse_document_tree,
        parse_document_tree_with_config, run_object, run_with_config, tree_text,
    };

    #[test]
    fn measure_longest_dictionary_prefix() {
        let cfg = ParseConfig::default();

        // Digit-bearing builtins are table data, not special cases.
        let (n, e) = measure_entity_name("there4", &cfg).expect("there4");
        assert_eq!((n, e), ("there4", 6));
        let (n, e) = measure_entity_name("sup1", &cfg).expect("sup1");
        assert_eq!((n, e), ("sup1", 4));
        let (n, e) = measure_entity_name("frac12", &cfg).expect("frac12");
        assert_eq!((n, e), ("frac12", 6));

        // Longest alnum then walk down: alpha is entity, POST digit is fine.
        let (n, e) = measure_entity_name("alpha2", &cfg).expect("alpha2");
        assert_eq!((n, e), ("alpha", 5));

        // alpha is a prefix of alphax, but POST continues as a letter -> reject
        // (no shorter dictionary name with legal POST).
        assert!(measure_entity_name("alphax", &cfg).is_none());

        // sup1x: "sup1" has a letter POST; the walk down accepts "sup", whose
        // POST is a digit.
        let (n, e) = measure_entity_name("sup1x", &cfg).expect("sup via walk-down");
        assert_eq!((n, e), ("sup", 3));

        // Unknown pure letter run.
        assert!(measure_entity_name("foo", &cfg).is_none());

        // Whitespace: known length only (table has 1..=20 spaces).
        let (n, e) = measure_entity_name("_  ", &cfg).expect("ws");
        assert_eq!(n, "_  ");
        assert_eq!(e, 3);
        assert!(measure_entity_name("_", &cfg).is_none());
        let twenty = format!("_{}", " ".repeat(20));
        let (n, e) = measure_entity_name(&twenty, &cfg).expect("20 spaces");
        assert_eq!(n.len(), 21);
        assert_eq!(e, 21);
        let long = format!("_{}", " ".repeat(21));
        assert!(measure_entity_name(&long, &cfg).is_none());
    }

    /// Pins the deliberate superset over Org: Org's regexp stops the
    /// name at the first digit, so `\my2ent` resolves the name `my`, misses the
    /// dictionary and aborts. See [`measure_entity_name`].
    #[test]
    fn measure_user_alnum_names() {
        let cfg = ParseConfig::default().with_user_entities([
            EntityDef::new("my2ent", "\\m", false, "m", "m", "m", "m"),
            EntityDef::new("alpha2", "\\a2", false, "a2", "a2", "a2", "a2"),
            EntityDef::new("my", "\\my", false, "my", "my", "my", "my"),
            EntityDef::new("myent", "\\me", false, "me", "me", "me", "me"),
        ]);

        let (n, e) = measure_entity_name("my2ent!", &cfg).expect("user");
        assert_eq!((n, e), ("my2ent", 6));
        // Without registration, no match.
        assert!(measure_entity_name("my2ent!", &ParseConfig::default()).is_none());

        // Longer user prefix wins over builtin alpha on the same alnum run.
        let (n, e) = measure_entity_name("alpha2", &cfg).expect("user alpha2");
        assert_eq!((n, e), ("alpha2", 6));
        assert_eq!(
            measure_entity_name("alpha2", &ParseConfig::default())
                .unwrap()
                .0,
            "alpha",
            "default still stops at builtin alpha"
        );

        // User "my" is a prefix of "myent"; letter POST rejects both shorter
        // and longer when the full run continues as a letter after a match.
        // "myentx": myent has letter POST; my has letter POST; reject.
        assert!(measure_entity_name("myentx", &cfg).is_none());
        // Exact user name with non-letter POST is fine.
        let (n, e) = measure_entity_name("myent!", &cfg).expect("myent");
        assert_eq!((n, e), ("myent", 5));
    }

    #[test]
    fn entity_shapes_accept() {
        let cases = [
            ("\\alpha", "\\alpha".len()),
            ("\\alpha{}", "\\alpha{}".len()),
            ("\\alpha{}x", "\\alpha{}".len()),
            ("\\Alpha", "\\Alpha".len()),
            ("\\there4", "\\there4".len()),
            ("\\there4{}", "\\there4{}".len()),
            ("\\sup2", "\\sup2".len()),
            ("\\frac12", "\\frac12".len()),
            ("\\cent.", "\\cent".len()),
            ("\\alpha2", "\\alpha".len()),
            ("\\_ ", "\\_ ".len()),
            ("\\_   ", "\\_   ".len()),
            // Whitespace entities never consume trailing {}.
            ("\\_  {}", "\\_  ".len()),
        ];
        for (src, want_len) in cases {
            let shape =
                probe(&make_parser(src)).unwrap_or_else(|| panic!("expected entity for {src:?}"));
            assert_eq!(shape.len, want_len, "len for {src:?}");
        }
    }

    #[test]
    fn entity_shapes_reject() {
        for src in [
            "\\alphax", "\\", "alpha", "\\\\", "\\_	", // tab after _ is not a space entity
            "\\begin", "\\foo",
        ] {
            assert!(probe(&make_parser(src)).is_none(), "not an entity: {src:?}");
        }
    }

    #[test]
    fn entity_sup1x_walks_down_to_sup() {
        let shape = probe(&make_parser("\\sup1x")).expect("sup");
        assert_eq!(shape.len, "\\sup".len());
    }

    #[test]
    fn entity_probe_pure_and_parse_uses_shape() {
        let p = make_parser("\\alpha{}x");
        let before = p.cursor();
        let shape = probe(&p).expect("entity");
        assert_eq!(shape.len, "\\alpha{}".len());
        assert_eq!(p.cursor(), before, "probe must not advance");

        let mut p2 = make_parser("\\alpha{}x");
        let shape = probe(&p2).expect("entity");
        parse(&mut p2, shape);
        let (_, evs, _) = p2.finish();
        assert_eq!(
            node_text(&evs, "\\alpha{}x", OrgSyntaxKind::ENTITY).as_deref(),
            Some("\\alpha{}")
        );
    }

    #[test]
    fn entity_mid_tile_alpha2() {
        let shape = probe(&make_parser("\\alpha2")).expect("entity");
        assert_eq!(shape.len, "\\alpha".len());
        let (_, evs, _) = run_object("\\alpha2", probe, parse);
        assert_eq!(
            node_text(&evs, "\\alpha2", OrgSyntaxKind::ENTITY).as_deref(),
            Some("\\alpha")
        );
    }

    #[test]
    fn document_roundtrip_entities() {
        let src = "1\\cent. and \\alpha{}x and \\alphax\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 2, "cent + alpha{{}}, not alphax");
        assert_eq!(ents[0].text().to_string(), "\\cent");
        assert_eq!(ents[1].text().to_string(), "\\alpha{}");
    }

    #[test]
    fn whitespace_entity() {
        let src = "a\\_  b\n";
        let root = parse_document_tree(src);
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].text().to_string(), "\\_  ");
    }

    #[test]
    fn user_entity_via_config() {
        let cfg = ParseConfig::default()
            .with_user_entities([EntityDef::new("myent", "\\m", false, "m", "m", "m", "m")]);
        let (_, evs, _) = run_with_config("\\myent!", &cfg, |p| {
            if let Some(s) = probe(p) {
                parse(p, s);
            }
        });
        assert_eq!(
            node_text(&evs, "\\myent!", OrgSyntaxKind::ENTITY).as_deref(),
            Some("\\myent")
        );

        // Full document materialize with the same config.
        let src = "x\\myent y\n";
        let root = parse_document_tree_with_config(src, &cfg);
        assert_eq!(tree_text(&root), src);
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].text().to_string(), "\\myent");
    }

    /// Org POST is `(not letter)` over the Unicode letter category, so any
    /// letter continuing the run rejects, not just ASCII a-zA-Z.
    #[test]
    fn entity_post_rejects_unicode_letters() {
        for src in [
            "\\alphax",        // ASCII letter
            "\\alpha\u{e9}",   // e-acute (Ll)
            "\\alpha\u{3b2}",  // Greek beta (Ll)
            "\\alpha\u{4e2d}", // CJK ideograph (Lo)
        ] {
            assert!(
                probe(&make_parser(src)).is_none(),
                "letter POST must reject: {src:?}"
            );
        }
        // Non-letter POST stays legal.
        for src in ["\\alpha\u{2014}", "\\alpha\u{ab}", "\\alpha2"] {
            assert!(
                probe(&make_parser(src)).is_some(),
                "non-letter POST must accept: {src:?}"
            );
        }
    }

    /// Entity is in org `minimal-set`, so every scope taking minimal or better
    /// parses it: radio targets and link descriptions (minimal), table cells
    /// (standard minus babel/cookie).
    #[test]
    fn entity_in_minimal_and_cell_scopes() {
        let src = "<<<r \\alpha>>>\n[[https://x.com][d \\beta]]\n| c \\gamma |\n";
        let root = parse_document_tree(src);
        assert_eq!(tree_text(&root), src);

        for (scope, want) in [
            (OrgSyntaxKind::RADIO_TARGET, "\\alpha"),
            (OrgSyntaxKind::LINK_DESCRIPTION, "\\beta"),
            (OrgSyntaxKind::TABLE_CELL, "\\gamma"),
        ] {
            let scopes = nodes_of_kind(&root, scope);
            assert_eq!(scopes.len(), 1, "one {scope:?}");
            let ents = nodes_of_kind(&scopes[0], OrgSyntaxKind::ENTITY);
            assert_eq!(ents.len(), 1, "entity inside {scope:?}");
            assert_eq!(ents[0].text().to_string(), want);
        }
    }

    #[test]
    fn entity_in_markup_and_title() {
        let src = "* title \\alpha\n*bold \\beta*\n";
        let root = parse_document_tree(src);
        let ents = nodes_of_kind(&root, OrgSyntaxKind::ENTITY);
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0].text().to_string(), "\\alpha");
        assert_eq!(ents[1].text().to_string(), "\\beta");
        assert_eq!(tree_text(&root), src);
    }
}
