//! Inline object candidate table and per-cursor claim.
//!
//! The **walk** that re-enters the grammar lives in
//! [`crate::parser::grammar::drive::region`] (`inline_in` / `inline_to_limit`),
//! which charges the nesting budget. This module owns the table: probe adapters,
//! restriction gates, and [`try_object`] for one claim at the cursor. Nothing
//! else walks objects directly.
//!
//! **Probe, then emit (as in the element `decide`):** each table row has a pure
//! `probe: fn(&Parser, ObjectSet) -> Option<ObjectShape>`. When a probe returns
//! `Some(shape)`, emit is `shape.parse(parser)`. Copy layouts live on
//! per-family types wrapped by [`ObjectShape`].
//!
//! Two object-shape classes (same split as element `Element`):
//! - **Measured** - probe carries bounds consumed without rescan: markup
//!   closer, cookie token count, footnote ref layout, citation open+closer,
//!   plain `path_len`, angle closer, script body, line-break token count,
//!   export-snippet end, macro byte length, inline babel `CallLayout` /
//!   inline src `InlineSrcLayout` (when complete; TEXT-prefix family), entity
//!   / latex-fragment byte length.
//! - **Unit / prefilter** - probe only confirms the opener (or a cheap gate);
//!   emit still walks the interior/closer: bracket `[[`, target/radio opener
//!   run, timestamp active/inactive gate (full stamp interior is too large for
//!   a Copy probe and stays emit-side).
//!
//! **Candidate rules:**
//! - Opener filter: only rows whose `openers` match `parser.current()` run.
//!   Every row declares its openers.
//! - Within the angle family, longest match: radio `<<<` before target `<<`
//!   before angle link `<`.
//! - Bracket family (`[`) rows are independent once each probe is exact
//!   (cookie, footnote, citation, timestamp date-shape, bracket link `[[`).
//! - `_`, `^` and `\` are single rows that reproduce the *branches* of
//!   `org-element--object-lex` (see the row comments): underline-then-subscript
//!   on `_`, and second-byte dispatch on `\`.
//! - TEXT-prefix row co-owns `call_` and `src_` (call then src) before the
//!   plain-link TEXT row so PATHPLAIN never competes with those openers.
//!
//! **Progress:** each step of the walk in `drive::region` either emits an
//! object through a successful probe and parse (a parse may leave a bare
//! opener) or falls through to `eat_any()`, so the source cursor always
//! advances. Progress is [`Parser::progress`] (`pos` plus the mid-tile byte
//! offset), not the token index alone. An object parser that abandons after
//! consuming openers must still leave the cursor past the start. As a
//! backstop, in release builds too, the walk calls `eat_any()` when a probe
//! claimed but parse left `progress` unchanged, so it cannot loop; debug
//! builds also assert. The walk's `scope` selects `restrictions(scope)`, so
//! only objects allowed in that container can claim.
//!
//! Two similar names: `Restriction` on a row is that row's gate (`AnyOf` or
//! `AllOf` kinds), while `restrictions(scope)` is the container's allowance
//! set the gate is checked against. A row gate is a cheap prefilter; a probe
//! that reproduces an org branch re-checks each arm.
//!
//! Prose matching a radio target is not linked here: that match depends on
//! targets anywhere in the document, so it is a separate query over the
//! finished tree rather than part of this single structural pass.

use super::{
    call, citation, cookie, entity, export_snippet, footnote, inline_src, latex, line_break, link,
    macros, markup, script, target, timestamp,
};
use crate::parser::Parser;
use crate::syntax::{MARKUP_OBJECTS, ObjectSet, OrgSyntaxKind};

/// Copy payload from a successful object probe (the object counterpart of the
/// element dispatcher's `Element`).
///
/// Measured variants carry bounds used at emit without rescan; unit/prefilter
/// variants only confirm the opener (see module docs). [`ObjectShape::parse`]
/// dispatches to the family emit path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectShape {
    /// Measured: delimiter kinds + validated closer.
    Markup(markup::MarkupShape),
    /// Measured: marker kind + brace / paren / bare body bounds.
    Script(script::ScriptShape),
    /// Measured: `\\` + optional spaces + line terminator token count.
    LineBreak(line_break::LineBreakShape),
    /// Measured when `end` is `Some`; when `None`, the opener is claimed and
    /// left as bare tokens.
    ExportSnippet(export_snippet::ExportSnippetShape),
    /// Measured: full macro form byte length (`{{{NAME}}}` / `{{{NAME(args)}}}`).
    Macro(macros::MacroShape),
    /// Measured: full cookie token span.
    Cookie(cookie::CookieShape),
    /// Measured: standard or inline ref layout.
    FootnoteRef(footnote::FootnoteRefShape),
    /// Measured: open token count + outer `]` closer; interior at emit.
    Citation(citation::CitationShape),
    /// Unit/prefilter: active vs inactive opener; interior at emit.
    Timestamp(timestamp::TimestampShape),
    /// Unit: the `[[` opener; path and description at emit.
    BracketLink(link::BracketLinkShape),
    /// Measured when `layout` is `Some`; when `None`, only the opener matched
    /// and it stays bare tokens.
    InlineBabelCall(call::InlineCallShape),
    /// Measured when `layout` is `Some`; when `None`, only the opener matched
    /// and it stays bare tokens.
    InlineSrcBlock(inline_src::InlineSrcShape),
    /// Measured: exclusive PATHPLAIN byte length.
    PlainLink(link::PlainLinkShape),
    /// Unit: `<<` / `<<<` opener run (`TargetShape.kind` is RADIO vs TARGET).
    Target(target::TargetShape),
    /// Measured: protocol + closer position.
    AngleLink(link::AngleLinkShape),
    /// Measured: full entity form byte length (`\name` / `\name{}` / `\_`+spaces).
    Entity(entity::EntityShape),
    /// Measured: full latex-fragment form byte length (`\(...\)`, `\[...\]`,
    /// command+args, `$...$`, `$$...$$`).
    LatexFragment(latex::LatexFragmentShape),
}

impl ObjectShape {
    /// Emit this object from its probe payload.
    fn parse(self, parser: &mut Parser) {
        match self {
            Self::Markup(s) => markup::parse(parser, s),
            Self::Script(s) => script::parse(parser, s),
            Self::LineBreak(s) => line_break::parse(parser, s),
            Self::ExportSnippet(s) => export_snippet::parse(parser, s),
            Self::Macro(s) => macros::parse(parser, s),
            Self::Cookie(s) => cookie::parse(parser, s),
            Self::FootnoteRef(s) => footnote::parse_reference(parser, s),
            Self::Citation(s) => citation::parse(parser, s),
            Self::Timestamp(s) => timestamp::parse(parser, s),
            Self::BracketLink(s) => link::parse_bracket(parser, s),
            Self::InlineBabelCall(s) => call::parse_inline(parser, s),
            Self::InlineSrcBlock(s) => inline_src::parse(parser, s),
            Self::PlainLink(s) => link::parse_plain(parser, s),
            Self::Target(s) => target::parse(parser, s),
            Self::AngleLink(s) => link::parse_angle(parser, s),
            Self::Entity(s) => entity::parse(parser, s),
            Self::LatexFragment(s) => latex::parse_fragment(parser, s),
        }
    }
}

/// Pure probe for one table row.
///
/// The [`ObjectSet`] is the container's allowance list. Most rows ignore it
/// (the row's own `restriction` already gated them); rows that reproduce an
/// `org-element--object-lex` *branch* need it because the branch's arms have
/// different allowances - see the backslash row, where `standard-set-no-line-break`
/// permits entity but not line-break on the same opener.
type InlineProbe = fn(&Parser, ObjectSet) -> Option<ObjectShape>;

/// Restriction gate for one candidate row.
enum Restriction {
    /// At least one listed kind must be present in allowed (non-empty set).
    AnyOf(ObjectSet),
    /// Every listed kind must be present in allowed (non-empty set).
    AllOf(ObjectSet),
}

struct InlineCandidate {
    /// Stable row label, used by the progress assertion and the row-order
    /// test. The three LINK rows share a kind, so rows need names.
    name: &'static str,
    /// Token kinds that can start this object (never empty).
    openers: &'static [OrgSyntaxKind],
    restriction: Restriction,
    probe: InlineProbe,
}

impl InlineCandidate {
    fn restriction_ok(&self, allowed: ObjectSet) -> bool {
        match self.restriction {
            Restriction::AnyOf(needs) => {
                debug_assert!(
                    !needs.is_empty(),
                    "InlineCandidate::AnyOf must be non-empty"
                );
                allowed.intersects(needs)
            }
            Restriction::AllOf(needs) => {
                debug_assert!(
                    !needs.is_empty(),
                    "InlineCandidate::AllOf must be non-empty"
                );
                allowed.contains_all(needs)
            }
        }
    }

    fn opener_ok(&self, current: OrgSyntaxKind) -> bool {
        self.openers.contains(&current)
    }
}

// ---------------------------------------------------------------------------
// Probe adapters (module probe -> ObjectShape)
// ---------------------------------------------------------------------------

fn probe_markup(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    markup::probe(p).map(ObjectShape::Markup)
}
/// `?_` branch of `org-element--object-lex`:
/// `(or underline-parser subscript-parser)`. Underline wins when it can form -
/// `(_a_)` is UNDERLINE because `(` is an emphasis PRE byte, while `a_b_c` is
/// two SUBSCRIPTs because `a` is not.
fn probe_underline_or_subscript(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    markup::probe(p)
        .map(ObjectShape::Markup)
        .or_else(|| script::probe(p).map(ObjectShape::Script))
}
fn probe_superscript(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    script::probe(p).map(ObjectShape::Script)
}
fn probe_export_snippet(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    export_snippet::probe(p).map(ObjectShape::ExportSnippet)
}
fn probe_macro(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    macros::probe(p).map(ObjectShape::Macro)
}
/// `?\\` branch of `org-element--object-lex`: a *branch on the second byte*,
/// not an ordered fallthrough.
///
/// ```elisp
/// (if (eq (aref result 1) ?\\)
///     (and (memq 'line-break restriction) (org-element-line-break-parser))
///   (or entity latex-fragment))
/// ```
///
/// So `\\` reaches line-break and nothing else: a miss (or a scope without
/// line-break, such as `standard-set-no-line-break`) yields no object rather
/// than retrying entity or latex. The else arm is entity **then** latex-
/// fragment, each gated on its own allowance (`memq`). The per-arm checks are
/// why this probe takes `allowed`.
fn probe_backslash(p: &Parser, allowed: ObjectSet) -> Option<ObjectShape> {
    // Second-byte branch: `\\` reaches line-break only.
    if p.here()?.run_of(OrgSyntaxKind::BACKSLASH, 2) {
        return allowed
            .contains(OrgSyntaxKind::LINE_BREAK)
            .then(|| line_break::probe(p).map(ObjectShape::LineBreak))
            .flatten();
    }
    // Else arm: entity THEN latex-fragment (ordered or, not fallthrough of `\\`).
    if allowed.contains(OrgSyntaxKind::ENTITY)
        && let Some(s) = entity::probe(p)
    {
        return Some(ObjectShape::Entity(s));
    }
    if allowed.contains(OrgSyntaxKind::LATEX_FRAGMENT)
        && let Some(s) = latex::probe_fragment_backslash(p)
    {
        return Some(ObjectShape::LatexFragment(s));
    }
    None
}

fn probe_dollar(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    latex::probe_fragment_dollar(p).map(ObjectShape::LatexFragment)
}
fn probe_cookie(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    cookie::probe(p).map(ObjectShape::Cookie)
}
fn probe_footnote(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    footnote::probe(p).map(ObjectShape::FootnoteRef)
}
fn probe_citation(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    citation::probe(p).map(ObjectShape::Citation)
}
fn probe_timestamp(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    timestamp::probe(p).map(ObjectShape::Timestamp)
}
fn probe_bracket_link(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    link::probe_bracket(p).map(ObjectShape::BracketLink)
}
/// TEXT-prefix family: `call_` then `src_`, mirroring `org-element--object-lex`.
///
/// Each arm re-checks its own allowance (`memq`). Prefixes are disjoint, so
/// order matches org without fallthrough hazards. Plain-link stays a later
/// TEXT row.
fn probe_text_prefix(p: &Parser, allowed: ObjectSet) -> Option<ObjectShape> {
    if allowed.contains(OrgSyntaxKind::INLINE_BABEL_CALL)
        && let Some(s) = call::probe_inline(p)
    {
        return Some(ObjectShape::InlineBabelCall(s));
    }
    if allowed.contains(OrgSyntaxKind::INLINE_SRC_BLOCK)
        && let Some(s) = inline_src::probe(p)
    {
        return Some(ObjectShape::InlineSrcBlock(s));
    }
    None
}
fn probe_plain_link(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    link::probe_plain(p).map(ObjectShape::PlainLink)
}
fn probe_radio(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    target::probe_radio(p).map(ObjectShape::Target)
}
fn probe_target(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    target::probe_target(p).map(ObjectShape::Target)
}
fn probe_angle_link(p: &Parser, _allowed: ObjectSet) -> Option<ObjectShape> {
    link::probe_angle(p).map(ObjectShape::AngleLink)
}

/// Ordered inline object candidates.
///
/// Each row is a pure [`InlineProbe`]; emit is [`ObjectShape::parse`]. The
/// inline region start is parser state set by the walk, not a parameter.
static INLINE_OBJECTS: &[InlineCandidate] = &[
    // Emphasis family minus `_`: the underscore branch is its own row below,
    // because a failed underline must still reach subscript.
    InlineCandidate {
        name: "markup",
        openers: &[
            OrgSyntaxKind::STAR,
            OrgSyntaxKind::SLASH,
            OrgSyntaxKind::PLUS,
            OrgSyntaxKind::TILDE,
            OrgSyntaxKind::EQUALS,
        ],
        restriction: Restriction::AnyOf(MARKUP_OBJECTS),
        probe: probe_markup,
    },
    // `_` and `^`. Underline, subscript and superscript are allowed together
    // in every `org-element-object-restrictions` row (all are in
    // `minimal-set`), which is what makes `AllOf` exact here; pinned by
    // `underline_subscript_superscript_co_occur_in_every_scope` in `syntax.rs`.
    InlineCandidate {
        name: "underline-or-subscript",
        openers: &[OrgSyntaxKind::UNDERSCORE],
        restriction: Restriction::AllOf(ObjectSet::of(&[
            OrgSyntaxKind::UNDERLINE,
            OrgSyntaxKind::SUBSCRIPT,
        ])),
        probe: probe_underline_or_subscript,
    },
    InlineCandidate {
        name: "superscript",
        openers: &[OrgSyntaxKind::CARET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::SUPERSCRIPT])),
        probe: probe_superscript,
    },
    InlineCandidate {
        name: "statistics-cookie",
        openers: &[OrgSyntaxKind::L_BRACKET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::STATISTICS_COOKIE])),
        probe: probe_cookie,
    },
    InlineCandidate {
        name: "footnote-reference",
        openers: &[OrgSyntaxKind::L_BRACKET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::FOOTNOTE_REFERENCE])),
        probe: probe_footnote,
    },
    // After footnote, matching org's order on the second byte (`f` then `c`);
    // the two probes never both claim.
    InlineCandidate {
        name: "citation",
        openers: &[OrgSyntaxKind::L_BRACKET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::CITATION])),
        probe: probe_citation,
    },
    InlineCandidate {
        name: "timestamp",
        openers: &[OrgSyntaxKind::L_ANGLE, OrgSyntaxKind::L_BRACKET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::TIMESTAMP])),
        probe: probe_timestamp,
    },
    // LINK rows share the same allowance; form is which probe is installed
    // (bracket / plain / angle).
    InlineCandidate {
        name: "bracket-link",
        openers: &[OrgSyntaxKind::L_BRACKET],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::LINK])),
        probe: probe_bracket_link,
    },
    // TEXT-prefix family (`call_` / `src_`): one row, ordered call then src,
    // before plain-link so PATHPLAIN never competes with those openers.
    InlineCandidate {
        name: "text-prefix",
        openers: &[OrgSyntaxKind::TEXT],
        restriction: Restriction::AnyOf(ObjectSet::of(&[
            OrgSyntaxKind::INLINE_BABEL_CALL,
            OrgSyntaxKind::INLINE_SRC_BLOCK,
        ])),
        probe: probe_text_prefix,
    },
    InlineCandidate {
        name: "plain-link",
        openers: &[OrgSyntaxKind::TEXT],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::LINK])),
        probe: probe_plain_link,
    },
    InlineCandidate {
        name: "radio-target",
        openers: &[OrgSyntaxKind::L_ANGLE],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::RADIO_TARGET])),
        probe: probe_radio,
    },
    InlineCandidate {
        name: "target",
        openers: &[OrgSyntaxKind::L_ANGLE],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::TARGET])),
        probe: probe_target,
    },
    InlineCandidate {
        name: "angle-link",
        openers: &[OrgSyntaxKind::L_ANGLE],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::LINK])),
        probe: probe_angle_link,
    },
    InlineCandidate {
        name: "export-snippet",
        openers: &[OrgSyntaxKind::AT],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::EXPORT_SNIPPET])),
        probe: probe_export_snippet,
    },
    // Macro on `{` only (`org-element--object-lex` `?{` arm). Scripts open on
    // `_`/`^`; no PRE and silent miss on incomplete forms.
    InlineCandidate {
        name: "macro",
        openers: &[OrgSyntaxKind::L_BRACE],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::MACRO])),
        probe: probe_macro,
    },
    // Dollar latex forms (`$...$` / `$$...$$`): separate opener from `\`.
    InlineCandidate {
        name: "latex-dollar",
        openers: &[OrgSyntaxKind::DOLLAR],
        restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::LATEX_FRAGMENT])),
        probe: probe_dollar,
    },
    // One row for the whole backslash family: the branch on the second byte
    // lives inside `probe_backslash`, not in row order. Separate ordered rows
    // for entity / latex would let a `\\` that line-break declined fall
    // through, which org never does.
    InlineCandidate {
        name: "backslash",
        openers: &[OrgSyntaxKind::BACKSLASH],
        restriction: Restriction::AnyOf(ObjectSet::of(&[
            OrgSyntaxKind::ENTITY,
            OrgSyntaxKind::LINE_BREAK,
            OrgSyntaxKind::LATEX_FRAGMENT,
        ])),
        probe: probe_backslash,
    },
];

/// Try one object at the cursor under `allowed`.
///
/// Returns the candidate row name when a probe claims (and emit has run). The
/// walk in `drive::region` advances with `eat_any` on `None` or on a stalled
/// claim. Does not open a region or charge nesting - that is the walk's job.
pub(crate) fn try_object(parser: &mut Parser, allowed: ObjectSet) -> Option<&'static str> {
    let current = parser.current();
    for c in INLINE_OBJECTS {
        if !c.restriction_ok(allowed) {
            continue;
        }
        if !c.opener_ok(current) {
            continue;
        }
        if let Some(shape) = (c.probe)(parser, allowed) {
            shape.parse(parser);
            return Some(c.name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::grammar::drive::region;
    use crate::syntax::restrictions;
    use crate::tests::{make_parser, node_kinds};

    #[test]
    fn region_inline_sets_region_start_for_at_region_start() {
        // After an outer token that is not emphasis PRE, inline_in at STAR
        // still forms markup because the walk sets the region start to the
        // entry position.
        let mut p = make_parser("x*b*");
        p.eat_any(); // leave TEXT "x" outside the region
        assert_eq!(p.cursor().index(), 1);
        assert!(!p.at_region_start());
        let end = p.limit();
        region::inline_in(&mut p, end, OrgSyntaxKind::PARAGRAPH);
        assert_eq!(p.inline_region_start, None);
        let (_, ev, _) = p.finish();
        assert!(
            node_kinds(&ev).contains(&OrgSyntaxKind::BOLD),
            "inline_in sets region to entry pos for at_region_start"
        );
    }

    #[test]
    fn restriction_ok_any_of_kinds() {
        use crate::syntax::ObjectSet;
        let c = InlineCandidate {
            name: "markup",
            openers: &[OrgSyntaxKind::STAR],
            restriction: Restriction::AnyOf(MARKUP_OBJECTS),
            probe: probe_markup,
        };
        assert!(!c.restriction_ok(ObjectSet::EMPTY));
        assert!(c.restriction_ok(ObjectSet::of(&[OrgSyntaxKind::BOLD])));
        assert!(c.restriction_ok(ObjectSet::of(&[OrgSyntaxKind::LINK, OrgSyntaxKind::ITALIC])));
        assert!(!c.restriction_ok(ObjectSet::of(&[
            OrgSyntaxKind::LINK,
            OrgSyntaxKind::TIMESTAMP
        ])));
    }

    #[test]
    fn restriction_ok_all_of_kinds() {
        use crate::syntax::ObjectSet;
        let c = InlineCandidate {
            name: "bracket-link",
            openers: &[OrgSyntaxKind::L_BRACKET],
            restriction: Restriction::AllOf(ObjectSet::of(&[OrgSyntaxKind::LINK])),
            probe: probe_bracket_link,
        };
        assert!(!c.restriction_ok(ObjectSet::EMPTY));
        assert!(c.restriction_ok(ObjectSet::of(&[OrgSyntaxKind::LINK])));
        assert!(!c.restriction_ok(ObjectSet::of(&[OrgSyntaxKind::TIMESTAMP])));
        assert!(c.restriction_ok(ObjectSet::of(&[
            OrgSyntaxKind::TIMESTAMP,
            OrgSyntaxKind::LINK
        ])));
    }

    #[test]
    fn inline_objects_names_and_openers() {
        // Pin row identity, order and opener families by name.
        assert_eq!(
            INLINE_OBJECTS.iter().map(|c| c.name).collect::<Vec<_>>(),
            [
                "markup",
                "underline-or-subscript",
                "superscript",
                "statistics-cookie",
                "footnote-reference",
                "citation",
                "timestamp",
                "bracket-link",
                "text-prefix",
                "plain-link",
                "radio-target",
                "target",
                "angle-link",
                "export-snippet",
                "macro",
                "latex-dollar",
                "backslash",
            ]
        );
        // No freestanding citation-reference / AT-only citation row.
        assert!(
            !INLINE_OBJECTS
                .iter()
                .any(|c| c.name == "citation-reference"),
            "citation-reference is citation-owned, not a freestanding row"
        );
        let cite_row = INLINE_OBJECTS
            .iter()
            .find(|c| c.name == "citation")
            .expect("citation row");
        assert_eq!(cite_row.openers, &[OrgSyntaxKind::L_BRACKET]);
        // Macro owns L_BRACE alone (scripts open on _/^; no multi-arm on `{`).
        let macro_row = INLINE_OBJECTS
            .iter()
            .find(|c| c.name == "macro")
            .expect("macro row");
        assert_eq!(macro_row.openers, &[OrgSyntaxKind::L_BRACE]);
        assert!(macro_row.opener_ok(OrgSyntaxKind::L_BRACE));
        assert!(!macro_row.opener_ok(OrgSyntaxKind::TEXT));
        // TEXT-prefix before plain-link, so call_ / src_ win over PATHPLAIN.
        let names: Vec<_> = INLINE_OBJECTS.iter().map(|c| c.name).collect();
        let tp = names
            .iter()
            .position(|&n| n == "text-prefix")
            .expect("text-prefix");
        let pl = names
            .iter()
            .position(|&n| n == "plain-link")
            .expect("plain-link");
        assert!(tp < pl, "text-prefix must precede plain-link");
        // Angle longest-match order: radio before target before angle-link.
        assert_eq!(
            INLINE_OBJECTS
                .iter()
                .filter(|c| matches!(c.name, "radio-target" | "target" | "angle-link"))
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            ["radio-target", "target", "angle-link"]
        );

        for c in INLINE_OBJECTS {
            if let Restriction::AllOf(needs) = c.restriction {
                assert!(!needs.is_empty(), "no vacuous empty-AllOf row");
            }
            // Bracket openers only claim on `[` (timestamp also accepts `<`).
            if matches!(
                c.name,
                "statistics-cookie" | "footnote-reference" | "citation" | "bracket-link"
            ) {
                assert!(c.opener_ok(OrgSyntaxKind::L_BRACKET));
                assert!(!c.opener_ok(OrgSyntaxKind::TEXT));
            }
        }
    }

    /// Paragraph allowances: what `parse_inline` hands every row probe in the
    /// common case.
    const STD: ObjectSet = restrictions(OrgSyntaxKind::PARAGRAPH);

    #[test]
    #[allow(clippy::too_many_lines)] // one probe/parse case table
    fn probe_returns_shape_parse_no_remeasure() {
        // Cookie: probe carries token count; parse emits without re-walking.
        let p = make_parser("[1/2]");
        let shape = probe_cookie(&p, STD).expect("cookie");
        assert!(matches!(
            shape,
            ObjectShape::Cookie(cookie::CookieShape { tokens: 5 })
        ));
        let mut p2 = make_parser("[1/2]");
        let shape = probe_cookie(&p2, STD).expect("cookie");
        let before = p2.progress();
        shape.parse(&mut p2);
        assert_ne!(p2.progress(), before);

        // Plain link: probe is full PATHPLAIN measure (pin path_len).
        match probe_plain_link(&make_parser("https://x.com"), STD).expect("plain") {
            ObjectShape::PlainLink(link::PlainLinkShape { path_len }) => {
                assert_eq!(path_len, "https://x.com".len());
            }
            other => panic!("expected PlainLink, got {other:?}"),
        }
        assert!(probe_plain_link(&make_parser("https:..."), STD).is_none());
        assert!(probe_plain_link(&make_parser("not-a-link"), STD).is_none());
        let mut p_plain = make_parser("https://x.com");
        let shape = probe_plain_link(&p_plain, STD).expect("plain");
        shape.parse(&mut p_plain);
        assert_eq!(p_plain.current(), OrgSyntaxKind::EOF);

        // Angle: closer carried; parse advances past the form.
        let p3 = make_parser("<https://x.com>");
        match probe_angle_link(&p3, STD).expect("angle") {
            ObjectShape::AngleLink(link::AngleLinkShape { closer }) => {
                assert!(closer > p3.cursor());
            }
            other => panic!("expected AngleLink, got {other:?}"),
        }
        let mut p3e = make_parser("<https://x.com>");
        let shape = probe_angle_link(&p3e, STD).expect("angle");
        shape.parse(&mut p3e);
        assert_eq!(p3e.current(), OrgSyntaxKind::EOF);

        // Markup carries delimiter kinds and the validated closer.
        let p4 = make_parser("*bold*");
        match probe_markup(&p4, STD).expect("markup") {
            ObjectShape::Markup(m) => {
                assert_eq!(m.marker, OrgSyntaxKind::STAR);
                assert_eq!(m.node_kind, OrgSyntaxKind::BOLD);
                assert!(m.is_recursive);
                assert_eq!(m.closer, crate::tests::pos_ahead(&p4, 2));
            }
            other => panic!("expected Markup, got {other:?}"),
        }
        let mut p4e = make_parser("*bold*");
        let shape = probe_markup(&p4e, STD).expect("markup");
        shape.parse(&mut p4e);
        let (_, ev, _) = p4e.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::BOLD));

        // Footnote standard layout (column-0 unindented is a definition, not a ref).
        let mut p5 = make_parser(" [fn:1]");
        p5.eat_any(); // skip leading WHITESPACE so cursor is on `[`
        assert!(matches!(
            probe_footnote(&p5, STD),
            Some(ObjectShape::FootnoteRef(
                footnote::FootnoteRefShape::Standard { tokens: 5 }
            ))
        ));
        let mut p5e = make_parser(" [fn:1]");
        p5e.eat_any();
        let shape = probe_footnote(&p5e, STD).expect("fn ref");
        shape.parse(&mut p5e);
        let (_, ev, _) = p5e.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::FOOTNOTE_REFERENCE));

        // Citation: measured open_tokens + closer; silent miss when incomplete.
        match probe_citation(&make_parser("[cite:@k]"), STD).expect("citation") {
            ObjectShape::Citation(citation::CitationShape {
                open_tokens,
                closer,
            }) => {
                assert_eq!(open_tokens, 3); // `[` `cite` `:`
                assert!(closer > make_parser("[cite:@k]").cursor());
            }
            other => panic!("expected Citation, got {other:?}"),
        }
        assert!(probe_citation(&make_parser("[cite:]"), STD).is_none());
        assert!(probe_citation(&make_parser("[Cite:@k]"), STD).is_none());
        let mut p_cite = make_parser("[cite:@k]");
        let shape = probe_citation(&p_cite, STD).expect("citation");
        shape.parse(&mut p_cite);
        assert_eq!(p_cite.current(), OrgSyntaxKind::EOF);
        let (_, ev, _) = p_cite.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::CITATION));
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::CITATION_REFERENCE));

        // Timestamp active vs inactive (unit/prefilter class).
        assert!(matches!(
            probe_timestamp(&make_parser("<2026-01-01>"), STD),
            Some(ObjectShape::Timestamp(timestamp::TimestampShape {
                active: true
            }))
        ));
        assert!(matches!(
            probe_timestamp(&make_parser("[2026-01-01]"), STD),
            Some(ObjectShape::Timestamp(timestamp::TimestampShape {
                active: false
            }))
        ));
        assert!(probe_timestamp(&make_parser("a < b"), STD).is_none());
        let mut p_ts = make_parser("<2026-01-01>");
        let shape = probe_timestamp(&p_ts, STD).expect("ts");
        shape.parse(&mut p_ts);
        let (_, ev, _) = p_ts.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::TIMESTAMP));

        // Bracket unit claim; parse completes LINK.
        assert!(matches!(
            probe_bracket_link(&make_parser("[[a]]"), STD),
            Some(ObjectShape::BracketLink(_))
        ));
        let mut p_br = make_parser("[[a]]");
        let shape = probe_bracket_link(&p_br, STD).expect("bracket");
        shape.parse(&mut p_br);
        let (_, ev, _) = p_br.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::LINK));

        // Target/radio both map to ObjectShape::Target (kind distinguishes).
        match probe_radio(&make_parser("<<<a>>>"), STD).expect("radio") {
            ObjectShape::Target(t) => {
                assert_eq!(t.kind, OrgSyntaxKind::RADIO_TARGET);
                assert_eq!(t.open_n, 3);
            }
            other => panic!("expected Target radio, got {other:?}"),
        }
        match probe_target(&make_parser("<<a>>"), STD).expect("target") {
            ObjectShape::Target(t) => {
                assert_eq!(t.kind, OrgSyntaxKind::TARGET);
                assert_eq!(t.open_n, 2);
            }
            other => panic!("expected Target, got {other:?}"),
        }

        // Script: brace / paren / bare bodies all measured up front.
        let mut p_sub = make_parser("x_{a}");
        p_sub.eat_any();
        assert!(matches!(
            probe_underline_or_subscript(&p_sub, STD),
            Some(ObjectShape::Script(script::ScriptShape {
                kind: OrgSyntaxKind::SUBSCRIPT,
                body: script::ScriptBody::Braced { .. }
            }))
        ));

        // Line break, export snippet, and macro are flat measured leaves.
        assert!(matches!(
            probe_backslash(&make_parser("\\\\\n"), STD),
            Some(ObjectShape::LineBreak(line_break::LineBreakShape {
                tokens: 3
            }))
        ));
        assert!(matches!(
            probe_export_snippet(&make_parser("@@h:v@@"), STD),
            Some(ObjectShape::ExportSnippet(
                export_snippet::ExportSnippetShape { end: Some(_) }
            ))
        ));
        match probe_macro(&make_parser("{{{title}}}"), STD).expect("macro") {
            ObjectShape::Macro(macros::MacroShape { len }) => {
                assert_eq!(len, "{{{title}}}".len());
            }
            other => panic!("expected Macro, got {other:?}"),
        }
        assert!(probe_macro(&make_parser("{{{foo"), STD).is_none());
        let mut p_mac = make_parser("{{{title}}}");
        let shape = probe_macro(&p_mac, STD).expect("macro");
        shape.parse(&mut p_mac);
        assert_eq!(p_mac.current(), OrgSyntaxKind::EOF);
        let (_, ev, _) = p_mac.finish();
        assert!(node_kinds(&ev).contains(&OrgSyntaxKind::MACRO));

        // Latex dollar + backslash command fragment.
        assert!(matches!(
            probe_dollar(&make_parser("$a$"), STD),
            Some(ObjectShape::LatexFragment(latex::LatexFragmentShape {
                len: 3
            }))
        ));
        assert!(matches!(
            probe_backslash(&make_parser("\\foo"), STD),
            Some(ObjectShape::LatexFragment(_))
        ));
    }

    /// `?_` branch: underline first, subscript only when it cannot form.
    #[test]
    fn underscore_row_prefers_underline_then_subscript() {
        // `(_a_)`: `(` is an emphasis PRE byte, so underline wins even though
        // subscript would also match (its PRE only needs non-whitespace).
        let mut under = make_parser("(_a_)");
        under.eat_any(); // L_PAREN
        match probe_underline_or_subscript(&under, STD).expect("claim") {
            ObjectShape::Markup(m) => assert_eq!(m.node_kind, OrgSyntaxKind::UNDERLINE),
            other => panic!("expected UNDERLINE, got {other:?}"),
        }

        // `a_b_c`: `a` is not an emphasis PRE byte, so underline declines and
        // subscript claims the same `_`.
        let mut sub = make_parser("a_b_c");
        sub.eat_any(); // TEXT "a"
        match probe_underline_or_subscript(&sub, STD).expect("claim") {
            ObjectShape::Script(s) => assert_eq!(s.kind, OrgSyntaxKind::SUBSCRIPT),
            other => panic!("expected SUBSCRIPT, got {other:?}"),
        }

        // Neither: the dispatcher emits a bare underscore.
        let mut bare = make_parser("a_ b");
        bare.eat_any();
        assert!(probe_underline_or_subscript(&bare, STD).is_none());
    }

    /// `?\\` branch: `\\` reaches line-break and nothing else. In a scope
    /// without line-break the branch yields no object rather than retrying
    /// entity or latex, and the else arms honour their own allowance.
    #[test]
    fn backslash_row_branches_on_the_second_byte() {
        let no_break = restrictions(OrgSyntaxKind::HEADING_TITLE);
        assert!(!no_break.contains(OrgSyntaxKind::LINE_BREAK));
        assert!(no_break.contains(OrgSyntaxKind::ENTITY));
        assert!(no_break.contains(OrgSyntaxKind::LATEX_FRAGMENT));

        // `\\` under standard-set-no-line-break: nothing, not entity/latex.
        assert!(probe_backslash(&make_parser("\\\\\n"), no_break).is_none());
        // The same input under paragraph allowances is a line break.
        assert!(matches!(
            probe_backslash(&make_parser("\\\\\n"), STD),
            Some(ObjectShape::LineBreak(_))
        ));
        // Entity arm is unaffected by the line-break allowance.
        for allowed in [STD, no_break] {
            assert!(matches!(
                probe_backslash(&make_parser("\\alpha"), allowed),
                Some(ObjectShape::Entity(_))
            ));
        }
        // Entity miss, latex hit: unknown command.
        assert!(matches!(
            probe_backslash(&make_parser("\\alphaa"), STD),
            Some(ObjectShape::LatexFragment(_))
        ));
        // Latex allowed, entity not: `\alpha` is a command fragment (org still
        // runs latex-fragment when entity is remq'd).
        let latex_only = ObjectSet::of(&[OrgSyntaxKind::LATEX_FRAGMENT]);
        assert!(matches!(
            probe_backslash(&make_parser("\\alpha"), latex_only),
            Some(ObjectShape::LatexFragment(_))
        ));
        // Entity allowed, latex not: unknown command stays unclaimed.
        let entity_only = ObjectSet::of(&[OrgSyntaxKind::ENTITY]);
        assert!(probe_backslash(&make_parser("\\alphaa"), entity_only).is_none());
        // No arm allowed: no claim.
        assert!(probe_backslash(&make_parser("\\alpha"), ObjectSet::EMPTY).is_none());
        assert!(probe_backslash(&make_parser("\\alphaa"), ObjectSet::EMPTY).is_none());
    }

    #[test]
    fn text_prefix_babel_looking_at_layout_none() {
        // The opener matches (name + `(`) but the arguments are unclosed: the
        // shared text-prefix row returns layout None.
        let p = make_parser("call_n(");
        match probe_text_prefix(&p, STD).expect("opener claim") {
            ObjectShape::InlineBabelCall(call::InlineCallShape { layout: None }) => {}
            other => panic!("expected layout None, got {other:?}"),
        }
        // Emit leaves bare call_ (two tokens) without CALL_* children.
        let mut p2 = make_parser("call_n(");
        let shape = probe_text_prefix(&p2, STD).expect("claim");
        let before = p2.cursor().index();
        shape.parse(&mut p2);
        assert_eq!(p2.cursor().index(), before + 2, "eats call_ only");
        let (_, ev, _) = p2.finish();
        assert!(!node_kinds(&ev).contains(&OrgSyntaxKind::INLINE_BABEL_CALL));
        // Complete form has Some(layout).
        match probe_text_prefix(&make_parser("call_n()"), STD).expect("full") {
            ObjectShape::InlineBabelCall(call::InlineCallShape { layout: Some(_) }) => {}
            other => panic!("expected Some(layout), got {other:?}"),
        }
    }

    #[test]
    fn text_prefix_src_looking_at_layout_none() {
        for src in ["src_py[hdr]", "src_py{unclosed"] {
            match probe_text_prefix(&make_parser(src), STD).expect("opener claim") {
                ObjectShape::InlineSrcBlock(inline_src::InlineSrcShape { layout: None }) => {}
                other => panic!("expected src layout None for {src:?}, got {other:?}"),
            }
            let mut p = make_parser(src);
            let shape = probe_text_prefix(&p, STD).expect("claim");
            let before = p.cursor().index();
            shape.parse(&mut p);
            assert_eq!(p.cursor().index(), before + 2, "eats src_ only for {src:?}");
            let (_, ev, _) = p.finish();
            assert!(!node_kinds(&ev).contains(&OrgSyntaxKind::INLINE_SRC_BLOCK));
            assert!(!node_kinds(&ev).contains(&OrgSyntaxKind::SRC_LANGUAGE));
        }
        // Complete form.
        match probe_text_prefix(&make_parser("src_py{x}"), STD).expect("full") {
            ObjectShape::InlineSrcBlock(inline_src::InlineSrcShape { layout: Some(_) }) => {}
            other => panic!("expected Some(layout), got {other:?}"),
        }
    }

    #[test]
    fn text_prefix_per_arm_allowance() {
        let babel_only = ObjectSet::of(&[OrgSyntaxKind::INLINE_BABEL_CALL]);
        let src_only = ObjectSet::of(&[OrgSyntaxKind::INLINE_SRC_BLOCK]);
        assert!(matches!(
            probe_text_prefix(&make_parser("call_n()"), babel_only),
            Some(ObjectShape::InlineBabelCall(_))
        ));
        assert!(probe_text_prefix(&make_parser("src_py{x}"), babel_only).is_none());
        assert!(matches!(
            probe_text_prefix(&make_parser("src_py{x}"), src_only),
            Some(ObjectShape::InlineSrcBlock(_))
        ));
        assert!(probe_text_prefix(&make_parser("call_n()"), src_only).is_none());
        assert!(probe_text_prefix(&make_parser("call_n()"), ObjectSet::EMPTY).is_none());
    }

    #[test]
    fn probe_is_pure() {
        // Sources whose object starts at the cursor; the `_`/`^` rows need a
        // preceding character, so they are exercised in the loop below.
        for (src, probe) in [
            ("*x*", probe_markup as InlineProbe),
            ("[/]", probe_cookie),
            ("[fn:1]", probe_footnote),
            ("[[a]]", probe_bracket_link),
            ("https://x.com", probe_plain_link),
            ("<<<a>>>", probe_radio),
            ("<<a>>", probe_target),
            ("<https://x.com>", probe_angle_link),
            ("call_n()", probe_text_prefix),
            ("src_py{x}", probe_text_prefix),
            ("src_py[hdr]", probe_text_prefix),
            ("src_py{unclosed", probe_text_prefix),
            ("<2026-01-01>", probe_timestamp),
            ("\\alpha", probe_backslash),
            ("\\alphaa", probe_backslash),
            ("\\\\\n", probe_backslash),
            ("$a$", probe_dollar),
            ("$$x$$", probe_dollar),
            ("@@h:v@@", probe_export_snippet),
            ("@@h:unterminated", probe_export_snippet),
            ("{{{title}}}", probe_macro),
            ("{{{foo(x)}}}", probe_macro),
            ("{{{foo", probe_macro),
            ("{{{}}}", probe_macro),
        ] {
            let p = make_parser(src);
            let before = p.cursor();
            let _ = probe(&p, STD);
            assert_eq!(p.cursor(), before, "probe must not advance for {src:?}");
        }
        for (src, probe) in [
            ("x_2", probe_underline_or_subscript as InlineProbe),
            ("x^2", probe_superscript),
            ("x_{a}", probe_underline_or_subscript),
            ("x_", probe_underline_or_subscript),
        ] {
            let mut p = make_parser(src);
            p.eat_any(); // leading TEXT so PRE has a character to read
            let before = p.cursor();
            let _ = probe(&p, STD);
            assert_eq!(p.cursor(), before, "probe must not advance for {src:?}");
        }
    }
}
