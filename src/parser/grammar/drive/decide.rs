//! Element recognition: [`Element`], [`decide`], paragraph interrupt.
//!
//! Every element is recognised here, by one ordered chain of probes. Whether
//! a block, drawer or LaTeX environment opener is closed is also decided
//! here; the END line look-ups are cached in [`super::frame`].

use crate::DiagnosticKind;
use crate::parser::grammar::forms::{
    affiliated, block, clock, comment, diary, drawer, fixed_width, footnote, heading,
    horizontal_rule, keyword, latex, list, table,
};
use crate::parser::{LineView, Parser, Pos};

// ---------------------------------------------------------------------------
// Element dispatcher
// ---------------------------------------------------------------------------

// Affiliation is decided here, once, for a run of keyword lines;
// [`keyword::probe`] reads only its own line. A run that attaches becomes
// [`Element::Affiliated`] and is emitted as one AFFILIATED_KEYWORDS node.
// Otherwise the line is a plain KEYWORD that owns its trailing blank lines.
// Nothing later re-derives attachment.

/// Why a block, drawer or LaTeX environment opener falls back to paragraph
/// text: it is unclosed, or a block has no name.
///
/// Returned by [`drawer::resolve`] / [`block::resolve`] / [`latex::resolve`]
/// without allocating. It carries no name: [`DegradeReason::into_diagnostic`]
/// re-reads the name from the line when [`Element::parse`] records the
/// diagnostic, so probing for paragraph interruption never builds strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DegradeReason {
    /// `:NAME:` with no matching `:END:` before the next heading / region end.
    UnterminatedDrawer,
    /// `#+begin_NAME` / `#+begin: NAME` with no matching end under the window.
    UnterminatedBlock { dynamic: bool },
    /// Bare `#+begin_` / `#+begin:` with no name tokens.
    EmptyBlockName { dynamic: bool },
    /// `\begin{NAME}` with no matching `\end{NAME}` under the heading section.
    UnterminatedLatexEnvironment,
}

impl DegradeReason {
    /// Build the emit-time diagnostic, re-reading the name from `line`.
    ///
    /// The name is empty only when the opener no longer measures (it always
    /// does on the emit path: this runs on the same line `decide` claimed).
    fn into_diagnostic(self, line: LineView<'_, '_>) -> DiagnosticKind {
        match self {
            Self::UnterminatedDrawer => {
                DiagnosticKind::unterminated_drawer(drawer::opener_name(line))
            }
            Self::UnterminatedBlock { dynamic } => {
                DiagnosticKind::unterminated_block(block::name_text(line, dynamic), dynamic)
            }
            Self::EmptyBlockName { dynamic } => DiagnosticKind::EmptyBlockName { dynamic },
            Self::UnterminatedLatexEnvironment => {
                DiagnosticKind::unterminated_latex_environment(latex::opener_name(line))
            }
        }
    }
}

/// One recognised element at a line.
///
/// Produced by [`decide`]; parsed by [`Element::parse`]. A unit variant is a
/// whole-line form, or a prefilter whose parse measures the rest itself; the
/// others carry what the probe measured. The order of probes in `decide`
/// matters where shapes overlap (blocks before keywords) and follows
/// org-element's recognition order. Headlines are not elements: the document
/// loop handles them ([`heading::StarLine::Heading`]), while star lines at the
/// inline-task level become [`Element::InlineTask`]
/// ([`heading::StarLine::Task`]).
///
/// `Copy`, so the paragraph-interrupt check and the parse can each hold one.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Element {
    /// Unit: pure whole-line form.
    Comment,
    /// Prefilter: `CLOCK:` marker; parse may fail and rewind.
    Clock,
    /// Measured inline task, up to its END line when it has one.
    InlineTask(heading::InlineTaskShape),
    /// Measured.
    Drawer(drawer::DrawerShape),
    /// Unit: pure whole-line form.
    FixedWidth,
    /// Measured.
    Block(block::BlockShape),
    /// Measured opaque frame: `\begin{NAME}` ... `\end{NAME}`.
    LatexEnvironment(latex::LatexEnvShape),
    /// Measured run of keyword lines that attaches to the element below it.
    /// Limit-sensitive: the same line degrades under a narrower window.
    Affiliated(affiliated::AffiliatedRun),
    /// Measured.
    Keyword(keyword::KeywordShape),
    /// Prefilter: column-0 `[fn:LABEL]`; parse measures the body.
    FootnoteDefinition(footnote::FootnoteOpener),
    /// Unit: pure whole-line form.
    HorizontalRule,
    /// Unit: pure whole-line form (column-0 `%%(` diary sexp).
    DiarySexp,
    /// Unit: parse walks the rows itself.
    Table,
    /// Prefilter: item line; parse measures the whole list.
    List,
    /// An opener shape matched but the construct degrades to paragraph text
    /// (unterminated / unnamed block, unterminated drawer, or unterminated
    /// latex environment). [`Element::parse`] only records the diagnostic;
    /// the paragraph fallback takes the line. `interrupts` says whether the
    /// line still ends a preceding paragraph, as in org-element.
    Degraded {
        at: Pos,
        reason: DegradeReason,
        interrupts: bool,
    },
}

impl Element {
    /// Parse a [`decide`] result: emit the element, or only the degradation
    /// diagnostic.
    ///
    /// Most arms consume input; `Clock` may rewind and `Degraded` only records
    /// a diagnostic. The caller checks [`Parser::progress`] and falls through
    /// to a paragraph when nothing was consumed.
    pub(crate) fn parse(self, parser: &mut Parser) {
        match self {
            Element::Comment => comment::parse(parser),
            Element::Clock => clock::parse(parser),
            Element::InlineTask(shape) => heading::parse_task(parser, shape),
            Element::Drawer(shape) => drawer::parse(parser, shape),
            Element::FixedWidth => fixed_width::parse(parser),
            Element::Block(shape) => block::parse(parser, shape),
            Element::LatexEnvironment(shape) => latex::parse_env(parser, shape),
            Element::Affiliated(run) => affiliated::parse(parser, run),
            Element::Keyword(shape) => keyword::parse(parser, shape),
            Element::FootnoteDefinition(shape) => footnote::parse(parser, shape),
            Element::HorizontalRule => horizontal_rule::parse(parser),
            Element::DiarySexp => diary::parse(parser),
            Element::Table => table::parse(parser),
            Element::List => list::parse(parser),
            Element::Degraded { at, reason, .. } => {
                // Name strings are built only here, never while probing for
                // paragraph interruption.
                let kind = match parser.line_of(at) {
                    Some(line) => reason.into_diagnostic(line),
                    // `at` is the claimed line's own start, so this is
                    // unreachable in practice; stay infallible anyway.
                    None => DiagnosticKind::EmptyBlockName { dynamic: false },
                };
                parser.error_line_at(at, kind);
            }
        }
    }
}

/// Probe the line for an element, without consuming or allocating.
///
/// [`interrupts_paragraph`] uses the same probe, so recognition and paragraph
/// termination cannot disagree.
pub(crate) fn decide(line: LineView) -> Option<Element> {
    if comment::probe(line).is_some() {
        return Some(Element::Comment);
    }
    if clock::probe(line).is_some() {
        return Some(Element::Clock);
    }
    // Inline tasks: a star run cannot also open a drawer, block or keyword.
    // The END line is searched for under the active limit.
    if let Some(shape) = heading::resolve_task(line) {
        return Some(Element::InlineTask(shape));
    }
    if let Some(op) = drawer::opener(line) {
        // Drawer-shaped lines parse as a drawer or degrade to paragraph; the
        // fixed-width row cannot match them (`:` needs a following space).
        // Unclosed drawers do not interrupt paragraphs.
        return Some(match drawer::resolve(line, op) {
            Ok(shape) => Element::Drawer(shape),
            Err(reason) => Element::Degraded {
                at: line.start(),
                reason,
                interrupts: false,
            },
        });
    }
    if fixed_width::probe(line).is_some() {
        return Some(Element::FixedWidth);
    }
    if let Some(opener) = block::opener(line) {
        // Begin-shaped lines never fall through to the keyword row (the
        // dynamic form is keyword-shaped): they parse as a block or degrade.
        // Underscore form interrupts only when closed; dynamic always does.
        return Some(match block::resolve(line, opener) {
            Ok(shape) => Element::Block(shape),
            Err(reason) => Element::Degraded {
                at: line.start(),
                reason,
                interrupts: opener.dynamic,
            },
        });
    }
    if let Some(op) = latex::opener(line) {
        // Line-first `\begin{NAME}`: a closed environment is opaque; an
        // unclosed one (or one cut off by a heading) degrades without ending a
        // preceding paragraph, as in Org.
        return Some(match latex::resolve(line, op) {
            Ok(shape) => Element::LatexEnvironment(shape),
            Err(reason) => Element::Degraded {
                at: line.start(),
                reason,
                interrupts: false,
            },
        });
    }
    // The keyword shape covers this line alone. `try_run` looks ahead and
    // returns `Some` only when the run attaches; otherwise the line is a plain
    // KEYWORD. A multi-line run that does not attach emits its first line, and
    // the next `parse_one_element` re-measures the shorter run.
    if let Some(shape) = keyword::probe(line) {
        if let Some(run) = affiliated::try_run(line, shape) {
            return Some(Element::Affiliated(run));
        }
        return Some(Element::Keyword(shape));
    }
    // Column-0 `[fn:LABEL]`, after the `#+` forms and before horizontal rules
    // as in org-element. The body is measured in parse, as for lists.
    if let Some(op) = footnote::opener(line) {
        return Some(Element::FootnoteDefinition(op));
    }
    if horizontal_rule::probe(line).is_some() {
        return Some(Element::HorizontalRule);
    }
    // Column-0 `%%(`, after horizontal rules and before tables as in
    // org-element.
    if diary::probe(line).is_some() {
        return Some(Element::DiarySexp);
    }
    if table::probe(line).is_some() {
        return Some(Element::Table);
    }
    if list::probe(line).is_some() {
        return Some(Element::List);
    }
    None
}

/// Whether `line` begins material that terminates a paragraph.
///
/// Headings, and anything [`decide`] recognises, interrupt, except
/// a `#+(\S-+)[...]:` line whose name is not the written spelling `CAPTION`
/// or `RESULTS`. The name is the text before the rightmost `[` in the
/// leading non-whitespace run (`#+CAPTION[a b][c]:` ends the paragraph;
/// `#+CAPTION[a][b]:` does not). `RESULT` does not end a paragraph. A
/// degraded opener interrupts only when [`decide`] set its `interrupts` flag
/// (dynamic `#+begin:` always; unclosed underscore blocks, drawers, and latex
/// environments never).
pub(crate) fn interrupts_paragraph(line: LineView<'_, '_>) -> bool {
    if heading::probe(line)
        .and_then(heading::StarLine::as_heading)
        .is_some()
    {
        return true;
    }
    match decide(line) {
        Some(Element::Degraded { interrupts, .. }) => interrupts,
        Some(Element::Keyword(shape)) => shape.interrupts,
        // An attaching `#+RESULT[h]:` is still a claim. It must not end an
        // open paragraph; only written CAPTION and RESULTS do.
        Some(Element::Affiliated(run)) => run.interrupts(),
        Some(_) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::grammar::drive::atoms;
    use crate::parser::grammar::forms::heading;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::make_parser;

    fn element_arm(e: Option<super::Element>) -> &'static str {
        match e {
            None => "None",
            Some(super::Element::Comment) => "Comment",
            Some(super::Element::Clock) => "Clock",
            Some(super::Element::InlineTask(shape)) => {
                if shape.end_line.is_some() {
                    "InlineTaskClosed"
                } else {
                    "InlineTaskDegenerate"
                }
            }
            Some(super::Element::Drawer(_)) => "Drawer",
            Some(super::Element::FixedWidth) => "FixedWidth",
            Some(super::Element::Block(_)) => "Block",
            Some(super::Element::LatexEnvironment(_)) => "LatexEnvironment",
            Some(super::Element::Affiliated(_)) => "Affiliated",
            Some(super::Element::Keyword(_)) => "Keyword",
            Some(super::Element::FootnoteDefinition(_)) => "FootnoteDefinition",
            Some(super::Element::HorizontalRule) => "HorizontalRule",
            Some(super::Element::DiarySexp) => "DiarySexp",
            Some(super::Element::Table) => "Table",
            Some(super::Element::List) => "List",
            Some(super::Element::Degraded {
                interrupts: true, ..
            }) => "DegradeT",
            Some(super::Element::Degraded {
                interrupts: false, ..
            }) => "DegradeF",
        }
    }

    #[test]
    fn interrupts_agrees_with_decide_on_closed_and_unclosed_openers() {
        use super::interrupts_paragraph;
        use crate::tests::make_parser;

        /// Expected interrupt result for a non-heading line under `decide`.
        #[derive(Debug, Clone, Copy)]
        enum Expect {
            /// `decide` returns this claim variant (non-Degraded).
            Claim(&'static str),
            /// `decide` returns Degraded with the given interrupt flag.
            Degrade { interrupts: bool },
            /// `decide` returns None (and no heading).
            None,
        }

        // Fifteen stars: default inline_task_min_level boundary (degenerate + closed).
        let cases: &[(&str, Expect)] = &[
            ("# comment\n", Expect::Claim("Comment")),
            ("  # c\n", Expect::Claim("Comment")), // indented content still claims
            (": fixed\n", Expect::Claim("FixedWidth")),
            ("-----\n", Expect::Claim("HorizontalRule")),
            // Diary sexp: column-0 lead claims; indented is paragraph (None).
            ("%%(org-calendar-holiday)\n", Expect::Claim("DiarySexp")),
            ("  %%(foo)\n", Expect::None),
            ("#+TITLE: x\n", Expect::Claim("Keyword")),
            ("[fn:1] note\n", Expect::Claim("FootnoteDefinition")),
            ("- item\n", Expect::Claim("List")),
            ("| a |\n", Expect::Claim("Table")),
            ("CLOCK: [2020-01-01 Wed]\n", Expect::Claim("Clock")),
            // Prefilter claim: marker alone interrupts even when body is bad.
            ("CLOCK: not-a-stamp\n", Expect::Claim("Clock")),
            // High-star task (degenerate and closed) interrupts via decide claim.
            (
                "*************** task\n",
                Expect::Claim("InlineTaskDegenerate"),
            ),
            (
                "*************** task\nbody\n*************** END\n",
                Expect::Claim("InlineTaskClosed"),
            ),
            // Closed block / drawer / latex-env claim.
            ("#+begin_src\nx\n#+end_src\n", Expect::Claim("Block")),
            ("#+begin: d\nbody\n#+end:\n", Expect::Claim("Block")),
            (":D:\n:END:\n", Expect::Claim("Drawer")),
            (
                "\\begin{eq}\nx\n\\end{eq}\n",
                Expect::Claim("LatexEnvironment"),
            ),
            // Unclosed underscore block: degrade, does not interrupt.
            ("#+begin_src\n", Expect::Degrade { interrupts: false }),
            // Empty underscore name degrades without interrupt.
            ("#+begin_\n", Expect::Degrade { interrupts: false }),
            // Dynamic form always interrupts, even without an end line / empty name.
            ("#+begin: d\n", Expect::Degrade { interrupts: true }),
            ("#+begin:\n", Expect::Degrade { interrupts: true }),
            // Unclosed drawer: degrade, does not interrupt.
            (":D:\nno end\n", Expect::Degrade { interrupts: false }),
            // Unclosed latex env: degrade, does not interrupt.
            ("\\begin{eq}\n", Expect::Degrade { interrupts: false }),
            ("just text\n", Expect::None),
        ];

        for &(src, expect) in cases {
            let p = make_parser(src);
            let line = p.line().expect("line start");
            let decision = super::decide(line);
            let interrupts = interrupts_paragraph(line);

            match expect {
                Expect::Claim(want_arm) => match decision {
                    Some(e) if !matches!(e, super::Element::Degraded { .. }) => {
                        assert_eq!(element_arm(Some(e)), want_arm, "claim arm for {src:?}");
                        assert!(interrupts, "claim must interrupt for {src:?}");
                    }
                    other => panic!("expected claim {want_arm} for {src:?}, got {other:?}"),
                },
                Expect::Degrade {
                    interrupts: want_flag,
                } => match decision {
                    Some(super::Element::Degraded {
                        interrupts: flag, ..
                    }) => {
                        assert_eq!(flag, want_flag, "degrade interrupt flag for {src:?}");
                        assert_eq!(
                            interrupts, want_flag,
                            "interrupts_paragraph must project degrade flag for {src:?}"
                        );
                    }
                    other => panic!("expected Degraded for {src:?}, got {other:?}"),
                },
                Expect::None => {
                    assert!(decision.is_none(), "expected no decide for {src:?}");
                    assert!(!interrupts, "none must not interrupt for {src:?}");
                }
            }
        }

        // Headings interrupt without going through decide's claim arms.
        let p = make_parser("* H\n");
        let line = p.line().expect("line");
        assert!(
            matches!(heading::probe(line), Some(heading::StarLine::Heading(1))),
            "heading probe matches outline level 1"
        );
        assert!(
            super::decide(line).is_none(),
            "headings are not dispatcher rows"
        );
        assert!(
            interrupts_paragraph(line),
            "heading interrupts via projection, not decide"
        );
    }

    #[test]
    fn bracketed_non_dual_keyword_does_not_interrupt() {
        use super::interrupts_paragraph;

        let cases: &[(&str, bool)] = &[
            ("#+FOO[bar]: baz\n", false),
            ("#+FOO[bar]:baz: more\n", false),
            ("#+FOO:bar[baz]: qux\n", false),
            ("#+A:B[c]: d\n", false),
            ("#+call:name[inside]: extra\n", false),
            ("#+NAME[s]: v\n", false),
            ("#+CAPTION[short]: long\n", true),
            ("#+CAPTION[a b][c]: v\n", true),
            ("#+CAPTION[a][b]: v\n", false),
            ("#+RESULT[a b][c]: v\n", false),
            ("#+RESULTS[h]: v\n", true),
            ("#+RESULT[h]: v\n", false),
            ("#+FOO: bar\n", true),
            ("#+CAPTION: long\n", true),
        ];
        for &(src, want) in cases {
            let p = make_parser(src);
            let line = p.line().expect(src);
            assert_eq!(interrupts_paragraph(line), want, "{src}");
            assert!(
                super::decide(line).is_some(),
                "standing line is still a claim for {src}"
            );
        }
    }

    #[test]
    fn dynamic_begin_interrupt_is_decide_projection() {
        use super::interrupts_paragraph;
        for src in ["#+begin: d\n", "#+begin:\n", "#+begin: d\nbody\n#+end:\n"] {
            let p = make_parser(src);
            let line = p.line().expect("line");
            let decision = super::decide(line);
            let flag = match decision {
                Some(super::Element::Block(_)) => true,
                Some(super::Element::Degraded { interrupts, .. }) => interrupts,
                other => panic!("expected block or degrade for {src:?}, got {other:?}"),
            };
            assert_eq!(
                interrupts_paragraph(line),
                flag,
                "interrupts must equal decide flag for {src:?}"
            );
            assert!(flag, "dynamic begin must interrupt for {src:?}");
        }
    }

    #[test]
    fn interrupt_path_leaves_diagnostics_empty() {
        use super::interrupts_paragraph;
        use std::fmt::Write;
        // Unclosed drawer openers: paragraph_end decides every line; no emit.
        let mut src = String::from("lead\n");
        for i in 0..16 {
            let _ = writeln!(src, ":D{i}:");
        }
        src.push_str("tail\n");
        let p = make_parser(&src);
        let mut line = p.line();
        while let Some(l) = line {
            let _ = interrupts_paragraph(l);
            line = l.next();
        }
        let (_toks, _evs, diags) = p.finish();
        assert!(
            diags.is_empty(),
            "pure interrupt must not emit diagnostics, got {diags:?}"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one degradation case table
    fn decide_degrades_when_the_active_limit_hides_end() {
        // Drawer: closed under full input; open under a cap that hides `:END:`.
        let src = ":D:\nbody\n:END:\n";
        let mut p = make_parser(src);
        let line = p.line().expect("opener");
        assert!(matches!(
            super::decide(line),
            Some(super::Element::Drawer(_))
        ));
        let end_line = p
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| l.kind(0) == OrgSyntaxKind::COLON && l.text(1).eq_ignore_ascii_case("end"))
            .expect("END line");
        let cap = end_line.start();
        p.with_limit_at(cap, |pp| {
            let line = pp.line().expect("opener under cap");
            assert!(
                matches!(
                    super::decide(line),
                    Some(super::Element::Degraded {
                        interrupts: false,
                        ..
                    })
                ),
                "under cap hiding :END: must degrade, got {:?}",
                super::decide(line)
            );
        });
        let line = p.line().expect("opener restored");
        assert!(
            matches!(super::decide(line), Some(super::Element::Drawer(_))),
            "outer limit still sees closed drawer"
        );

        // Block twin: cap hides `#+end_src`.
        let src_b = "#+begin_src\nbody\n#+end_src\n";
        let mut pb = make_parser(src_b);
        let line_b = pb.line().expect("begin");
        assert!(matches!(
            super::decide(line_b),
            Some(super::Element::Block(_))
        ));
        let end_b = pb
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| atoms::line_keyword_prefix(*l).is_some_and(|k| l.text_eq_caseless(k, "end")))
            .expect("end_src line");
        let cap_b = end_b.start();
        pb.with_limit_at(cap_b, |pp| {
            let line = pp.line().expect("begin under cap");
            assert!(
                matches!(
                    super::decide(line),
                    Some(super::Element::Degraded {
                        interrupts: false,
                        ..
                    })
                ),
                "under cap hiding #+end_src must degrade, got {:?}",
                super::decide(line)
            );
        });
        assert!(
            matches!(
                super::decide(pb.line().expect("begin restored")),
                Some(super::Element::Block(_))
            ),
            "outer limit still sees closed block"
        );

        // Latex env twin: cap hides `\end{eq}`.
        let src_l = "\\begin{eq}\nbody\n\\end{eq}\n";
        let mut pl = make_parser(src_l);
        let line_l = pl.line().expect("begin");
        assert!(matches!(
            super::decide(line_l),
            Some(super::Element::LatexEnvironment(_))
        ));
        let end_l = pl
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| l.kind(0) == OrgSyntaxKind::BACKSLASH && l.text_eq_caseless(1, "end"))
            .expect("end eq line");
        let cap_l = end_l.start();
        pl.with_limit_at(cap_l, |pp| {
            let line = pp.line().expect("begin under cap");
            assert!(
                matches!(
                    super::decide(line),
                    Some(super::Element::Degraded {
                        interrupts: false,
                        ..
                    })
                ),
                "under cap hiding \\end{{eq}} must degrade, got {:?}",
                super::decide(line)
            );
        });
        assert!(
            matches!(
                super::decide(pl.line().expect("begin restored")),
                Some(super::Element::LatexEnvironment(_))
            ),
            "outer limit still sees closed latex env"
        );

        // Inline task twin: closed under full input; END hidden under cap.
        let high = "*".repeat(15);
        let src_t = format!("{high} task\nbody\n{high} END\n");
        let mut pt = make_parser(&src_t);
        let line_t = pt.line().expect("task open");
        match super::decide(line_t) {
            Some(super::Element::InlineTask(shape)) => {
                assert!(shape.end_line.is_some(), "full input sees closed task");
            }
            other => panic!("expected InlineTask closed, got {other:?}"),
        }
        let end_t = pt
            .line()
            .expect("l0")
            .lines_after()
            .find(|l| heading::is_end_title_line(*l))
            .expect("END line");
        let cap_t = end_t.start();
        pt.with_limit_at(cap_t, |pp| {
            let line = pp.line().expect("task under cap");
            match super::decide(line) {
                Some(super::Element::InlineTask(shape)) => {
                    assert!(
                        shape.end_line.is_none(),
                        "under cap hiding END must be degenerate, got {:?}",
                        super::decide(line)
                    );
                }
                other => panic!("expected InlineTask degenerate under cap, got {other:?}"),
            }
        });
        match super::decide(pt.line().expect("task restored")) {
            Some(super::Element::InlineTask(shape)) => {
                assert!(
                    shape.end_line.is_some(),
                    "outer limit still sees closed task"
                );
            }
            other => panic!("expected restored closed InlineTask, got {other:?}"),
        }
    }
}
