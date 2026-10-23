//! Heading line and field fragments; title re-parse safety.

use super::{SynthesisError, is_valid_tag_name};
use crate::parser::{ParseConfig, PriorityAtom};

/// Why a title cannot be written as headline title content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TitleProblem {
    /// Contains `\n` or `\r`.
    Newline,
    /// Leading or trailing ASCII space/tab; the lexer would absorb it.
    AbsorbedWhitespace,
    /// First non-terminator run is a TODO keyword under the given config.
    TodoKeyword,
    /// Starts with a `[#A]` / `[#0]`-style cookie the grammar would carve.
    PriorityCookie,
    /// Exact `COMMENT`, or `COMMENT` followed by ASCII space.
    CommentMarker,
    /// A trailing tag group the grammar would carve (`:a:`, `foo :a:`).
    TrailingTagGroup,
}

/// Headline prefix slots (TODO, priority, `COMMENT`) that are already filled.
///
/// A title word that looks like a filled slot's content cannot be read as
/// it, so it is not rejected. `Newline`, `AbsorbedWhitespace` and
/// `TrailingTagGroup` apply regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OccupiedHeadlinePrefix {
    pub todo: bool,
    pub priority: bool,
    pub commented: bool,
}

impl OccupiedHeadlinePrefix {
    pub const UNOCCUPIED: Self = Self {
        todo: false,
        priority: false,
        commented: false,
    };
}

/// Why `title` would not round-trip as the title of `* {title}\n` under
/// `config`, or `None` if it would.
///
/// This assumes a heading with no TODO keyword, priority or `COMMENT`.
/// Operations that set a title check against the slots the heading already
/// has filled.
#[must_use]
pub fn title_problem(title: &str, config: &ParseConfig) -> Option<TitleProblem> {
    title_problem_given(title, config, OccupiedHeadlinePrefix::UNOCCUPIED)
}

/// True when [`title_problem`] finds nothing.
#[must_use]
pub fn title_is_writable(title: &str, config: &ParseConfig) -> bool {
    title_problem(title, config).is_none()
}

/// Like [`title_problem`], but skip the TODO, priority and `COMMENT` checks
/// for slots the headline already fills.
#[must_use]
pub(crate) fn title_problem_given(
    title: &str,
    config: &ParseConfig,
    occupied: OccupiedHeadlinePrefix,
) -> Option<TitleProblem> {
    if title.contains(['\n', '\r']) {
        return Some(TitleProblem::Newline);
    }
    if starts_or_ends_with_ascii_ws(title) {
        return Some(TitleProblem::AbsorbedWhitespace);
    }
    if !occupied.todo {
        let run = first_nonterm_run(title);
        if !run.is_empty()
            && run.len() <= config.max_todo_keyword_len()
            && config.todo_kind(run).is_some()
        {
            return Some(TitleProblem::TodoKeyword);
        }
    }
    if !occupied.priority && starts_with_priority_cookie(title) {
        return Some(TitleProblem::PriorityCookie);
    }
    if !occupied.commented && (title == "COMMENT" || title.starts_with("COMMENT ")) {
        return Some(TitleProblem::CommentMarker);
    }
    if has_trailing_tag_group(title) {
        return Some(TitleProblem::TrailingTagGroup);
    }
    None
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HeadingSpec<'a> {
    pub level: usize,
    pub todo: Option<&'a str>,
    pub priority: Option<PriorityAtom>,
    pub commented: bool,
    pub title: &'a str,
    pub tags: &'a [&'a str],
}

pub(crate) fn render_stars(n: usize) -> String {
    debug_assert!(n >= 1, "zero stars is render_heading InvalidLevel");
    "*".repeat(n)
}

pub(crate) fn render_todo(keyword: &str) -> String {
    keyword.to_string()
}

pub(crate) fn render_comment() -> String {
    "COMMENT".to_string()
}

pub(crate) fn render_priority(atom: PriorityAtom) -> Result<String, SynthesisError> {
    match atom {
        PriorityAtom::Letter(c) if c.is_ascii_uppercase() => Ok(format!("[#{c}]")),
        PriorityAtom::Number(n) if n <= PriorityAtom::MAX_NUMBER => Ok(format!("[#{n}]")),
        _ => Err(SynthesisError::InvalidPriority),
    }
}

pub(crate) fn render_tags(names: &[&str]) -> Result<String, SynthesisError> {
    if names.is_empty() {
        return Ok(String::new());
    }
    let mut s = String::from(":");
    for name in names {
        if !is_valid_tag_name(name) {
            return Err(SynthesisError::InvalidTag((*name).to_string()));
        }
        s.push_str(name);
        s.push(':');
    }
    Ok(s)
}

/// Whole headline line, ending with exactly one `\\n`.
///
/// `config` supplies the TODO vocabulary the title check consults; the
/// filled slots come from the spec.
pub(crate) fn render_heading(
    spec: HeadingSpec<'_>,
    config: &ParseConfig,
) -> Result<String, SynthesisError> {
    if spec.level == 0 {
        return Err(SynthesisError::InvalidLevel);
    }
    if let Some(todo) = spec.todo
        && (todo.is_empty()
            || todo.contains(['\n', '\r', ' ', '\t'])
            || config.todo_kind(todo).is_none())
    {
        return Err(SynthesisError::InvalidTodo);
    }
    let occupied = OccupiedHeadlinePrefix {
        todo: spec.todo.is_some(),
        priority: spec.priority.is_some(),
        commented: spec.commented,
    };
    if let Some(problem) = title_problem_given(spec.title, config, occupied) {
        return Err(SynthesisError::InvalidTitle(problem));
    }
    let tags = if spec.tags.is_empty() {
        None
    } else {
        Some(render_tags(spec.tags)?)
    };
    let pri = match spec.priority {
        Some(p) => Some(render_priority(p)?),
        None => None,
    };

    // Stars + one space + present parts joined by a single space. Empty title
    // with tags is just another part (`* :a:\n`), not a second space.
    let mut parts = Vec::new();
    if let Some(todo) = spec.todo {
        parts.push(todo);
    }
    if let Some(ref p) = pri {
        parts.push(p.as_str());
    }
    if spec.commented {
        parts.push("COMMENT");
    }
    if !spec.title.is_empty() {
        parts.push(spec.title);
    }
    if let Some(ref t) = tags {
        parts.push(t.as_str());
    }

    let mut s = render_stars(spec.level);
    s.push(' ');
    s.push_str(&parts.join(" "));
    s.push('\n');
    Ok(s)
}

fn starts_or_ends_with_ascii_ws(s: &str) -> bool {
    s.as_bytes()
        .first()
        .is_some_and(|b| *b == b' ' || *b == b'\t')
        || s.as_bytes()
            .last()
            .is_some_and(|b| *b == b' ' || *b == b'\t')
}

fn first_nonterm_run(s: &str) -> &str {
    match s.find([' ', '\t']) {
        Some(i) => &s[..i],
        None => s,
    }
}

fn starts_with_priority_cookie(title: &str) -> bool {
    let Some(rest) = title.strip_prefix("[#") else {
        return false;
    };
    let Some(end) = rest.find(']') else {
        return false;
    };
    PriorityAtom::is_cookie_value(&rest[..end])
}

fn has_trailing_tag_group(title: &str) -> bool {
    let bytes = title.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && (i == 0 || matches!(bytes[i - 1], b' ' | b'\t')) && tag_group_from(title, i)
        {
            return true;
        }
    }
    false
}

/// Mirrors the grammar's `tags_group_at`, starting at the colon at `colon_at`.
///
/// Atoms after that colon are colons, `_@#%`, or alphanumeric TEXT runs.
/// A valid group has at least two such atoms, ends on a colon, and is
/// followed by ASCII space/tab or end-of-string.
fn tag_group_from(title: &str, colon_at: usize) -> bool {
    let rest = &title[colon_at + 1..];
    let mut atoms = 0usize;
    let mut last_was_colon = false;
    let mut chars = rest.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ':' {
            chars.next();
            last_was_colon = true;
            atoms += 1;
        } else if matches!(c, '_' | '@' | '#' | '%') {
            chars.next();
            last_was_colon = false;
            atoms += 1;
        } else if c.is_alphanumeric() {
            while chars.peek().is_some_and(|n| n.is_alphanumeric()) {
                chars.next();
            }
            last_was_colon = false;
            atoms += 1;
        } else if c == ' ' || c == '\t' {
            return atoms >= 2 && last_was_colon;
        } else {
            return false;
        }
    }
    atoms >= 2 && last_was_colon
}

#[cfg(test)]
mod tests {
    use super::super::equal::assert_heading_eq;
    use super::*;
    use crate::ast::{AstNode, HasParts, Heading, HeadingPart, Headline};
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;
    use crate::{ParseConfig, TodoKind};

    fn spec<'a>(
        level: usize,
        todo: Option<&'a str>,
        priority: Option<PriorityAtom>,
        commented: bool,
        title: &'a str,
        tags: &'a [&'a str],
    ) -> HeadingSpec<'a> {
        HeadingSpec {
            level,
            todo,
            priority,
            commented,
            title,
            tags,
        }
    }

    fn first_heading(src: &str) -> (crate::Document, Heading) {
        let doc = parse(src);
        let h = Heading::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
                .into_iter()
                .next()
                .expect("heading"),
        )
        .expect("cast");
        (doc, h)
    }

    #[test]
    fn fragments() {
        assert_eq!(render_stars(1), "*");
        assert_eq!(render_stars(3), "***");
        assert_eq!(render_todo("TODO"), "TODO");
        assert_eq!(render_comment(), "COMMENT");
        assert_eq!(render_priority(PriorityAtom::Letter('A')).unwrap(), "[#A]");
        assert_eq!(render_priority(PriorityAtom::Number(0)).unwrap(), "[#0]");
        assert_eq!(render_priority(PriorityAtom::Number(12)).unwrap(), "[#12]");
        assert_eq!(render_tags(&["a", "b"]).unwrap(), ":a:b:");
        assert_eq!(render_tags(&[]).unwrap(), "");
        assert_eq!(render_tags(&["ARCHIVE"]).unwrap(), ":ARCHIVE:");
        assert!(matches!(
            render_priority(PriorityAtom::Letter('a')),
            Err(SynthesisError::InvalidPriority)
        ));
        assert!(matches!(
            render_priority(PriorityAtom::Number(65)),
            Err(SynthesisError::InvalidPriority)
        ));
        assert!(matches!(
            render_tags(&["a b"]),
            Err(SynthesisError::InvalidTag(s)) if s == "a b"
        ));
        assert!(matches!(
            render_tags(&[""]),
            Err(SynthesisError::InvalidTag(_))
        ));
    }

    #[test]
    fn heading_byte_recipes() {
        let cfg = ParseConfig::default();
        let rows: &[(HeadingSpec<'_>, &str)] = &[
            (spec(1, None, None, false, "Title", &[]), "* Title\n"),
            (
                spec(1, Some("TODO"), None, false, "Title", &[]),
                "* TODO Title\n",
            ),
            (
                spec(1, Some("TODO"), None, false, "DONE", &[]),
                "* TODO DONE\n",
            ),
            (
                spec(
                    1,
                    Some("TODO"),
                    Some(PriorityAtom::Letter('A')),
                    false,
                    "Title",
                    &["work", "home"],
                ),
                "* TODO [#A] Title :work:home:\n",
            ),
            (spec(1, None, None, true, "Title", &[]), "* COMMENT Title\n"),
            (spec(1, None, None, true, "", &[]), "* COMMENT\n"),
            (spec(1, None, None, false, "", &["a"]), "* :a:\n"),
            (
                spec(1, Some("TODO"), None, false, "", &["a"]),
                "* TODO :a:\n",
            ),
            (spec(1, None, None, true, "", &["a"]), "* COMMENT :a:\n"),
            (spec(1, None, None, false, "", &[]), "* \n"),
            (spec(3, None, None, false, "Child", &[]), "*** Child\n"),
        ];
        for (spec, want) in rows {
            let got = render_heading(*spec, &cfg).expect("writable");
            assert_eq!(got, *want, "bytes for title {:?}", spec.title);
            let doc = parse_with(&got, &cfg);
            assert_heading_eq(&doc, spec);
        }
    }

    #[test]
    fn empty_title_content_is_empty_not_whitespace() {
        let cfg = ParseConfig::default();
        for src in [
            render_heading(spec(1, None, None, false, "", &["a"]), &cfg).unwrap(),
            render_heading(spec(1, Some("TODO"), None, false, "", &["a"]), &cfg).unwrap(),
            render_heading(spec(1, None, None, true, "", &[]), &cfg).unwrap(),
        ] {
            let (doc, h) = first_heading(&src);
            let range = h.part_content_range(HeadingPart::Title).expect("title");
            assert_eq!(doc.slice(range), "", "{src:?}");
        }
    }

    #[test]
    fn heading_rejects() {
        let cfg = ParseConfig::default();
        assert!(matches!(
            render_heading(spec(0, None, None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidLevel)
        ));
        assert!(matches!(
            render_heading(spec(1, None, None, false, "TODO x", &[]), &cfg),
            Err(SynthesisError::InvalidTitle(TitleProblem::TodoKeyword))
        ));
        assert!(matches!(
            render_heading(spec(1, Some(""), None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidTodo)
        ));
        assert!(matches!(
            render_heading(spec(1, Some("TODO\n* X"), None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidTodo)
        ));
        assert!(matches!(
            render_heading(spec(1, Some("TODO\r* X"), None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidTodo)
        ));
        assert!(matches!(
            render_heading(spec(1, Some("TO DO"), None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidTodo)
        ));
        assert!(matches!(
            render_heading(spec(1, Some("NEXT"), None, false, "T", &[]), &cfg),
            Err(SynthesisError::InvalidTodo)
        ));
        assert!(matches!(
            render_heading(spec(1, None, None, false, "T", &["a b"]), &cfg),
            Err(SynthesisError::InvalidTag(_))
        ));
        assert!(matches!(
            render_heading(
                spec(1, None, Some(PriorityAtom::Letter('a')), false, "T", &[]),
                &cfg
            ),
            Err(SynthesisError::InvalidPriority)
        ));
    }

    #[test]
    fn custom_todo_title_uses_the_supplied_config() {
        let cfg = ParseConfig::default()
            .with_todo_keywords([("NEXT", TodoKind::Active), ("CANCELLED", TodoKind::Done)]);
        assert!(matches!(
            render_heading(spec(1, None, None, false, "NEXT x", &[]), &cfg),
            Err(SynthesisError::InvalidTitle(TitleProblem::TodoKeyword))
        ));
        let bytes = render_heading(spec(1, Some("NEXT"), None, false, "do it", &[]), &cfg)
            .expect("NEXT as the todo field is fine");
        assert_eq!(bytes, "* NEXT do it\n");
        assert_heading_eq(
            &parse_with(&bytes, &cfg),
            &spec(1, Some("NEXT"), None, false, "do it", &[]),
        );
    }

    #[test]
    fn title_oracle_agrees_with_parse() {
        let default = ParseConfig::default();
        let custom = ParseConfig::default()
            .with_todo_keywords([("NEXT", TodoKind::Active), ("CANCELLED", TodoKind::Done)]);
        let rows: &[(&str, &ParseConfig, Option<TitleProblem>)] = &[
            ("TODO\u{a0}x", &default, None),
            ("TODO x", &default, Some(TitleProblem::TodoKeyword)),
            ("TODO\tx", &default, Some(TitleProblem::TodoKeyword)),
            (" COMMENT", &default, Some(TitleProblem::AbsorbedWhitespace)),
            (" TODO", &default, Some(TitleProblem::AbsorbedWhitespace)),
            (
                " [#A] rest",
                &default,
                Some(TitleProblem::AbsorbedWhitespace),
            ),
            ("foo ", &default, Some(TitleProblem::AbsorbedWhitespace)),
            ("COMMENT\tfoo", &default, None),
            ("COMMENT foo", &default, Some(TitleProblem::CommentMarker)),
            ("[#a]", &default, None),
            ("[#A] rest", &default, Some(TitleProblem::PriorityCookie)),
            ("foo :a:", &default, Some(TitleProblem::TrailingTagGroup)),
            (":a:", &default, Some(TitleProblem::TrailingTagGroup)),
            ("A :: B", &default, None),
            ("NEXT x", &default, None),
            ("NEXT x", &custom, Some(TitleProblem::TodoKeyword)),
            ("DONE", &default, Some(TitleProblem::TodoKeyword)),
            ("[#64]", &default, Some(TitleProblem::PriorityCookie)),
            ("[#65]", &default, None),
            ("[#09]", &default, None),
            ("COMMENT", &default, Some(TitleProblem::CommentMarker)),
            ("COMMENTfoo", &default, None),
            (
                "foo :a: bar",
                &default,
                Some(TitleProblem::TrailingTagGroup),
            ),
            ("WAIT/1", &default, None),
        ];
        for &(title, cfg, want) in rows {
            assert_eq!(title_problem(title, cfg), want, "variant {title:?}");
            assert_eq!(
                title_problem_given(title, cfg, OccupiedHeadlinePrefix::UNOCCUPIED),
                want,
                "public title_problem is the unoccupied scan {title:?}"
            );
            assert_eq!(title_is_writable(title, cfg), want.is_none());
            let src = format!("* {title}\n");
            let doc = parse_with(&src, cfg);
            let h = Heading::cast(
                nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
                    .into_iter()
                    .next()
                    .expect("heading"),
            )
            .expect("cast");
            let stolen = h.todo_keyword().is_some()
                || h.priority().is_some()
                || h.is_commented()
                || h.tags().is_some();
            let absorbed = starts_or_ends_with_ascii_ws(title);
            assert_eq!(
                title_problem(title, cfg).is_some(),
                stolen || absorbed,
                "oracle {title:?} stolen={stolen} absorbed={absorbed}"
            );
        }
        assert_eq!(title_problem("a\nb", &default), Some(TitleProblem::Newline));
        assert_eq!(title_problem("a\rb", &default), Some(TitleProblem::Newline));
    }

    #[test]
    fn occupied_prefix_oracle() {
        let cfg = ParseConfig::default();
        let todo = OccupiedHeadlinePrefix {
            todo: true,
            ..OccupiedHeadlinePrefix::UNOCCUPIED
        };
        let priority = OccupiedHeadlinePrefix {
            priority: true,
            ..OccupiedHeadlinePrefix::UNOCCUPIED
        };
        let commented = OccupiedHeadlinePrefix {
            commented: true,
            ..OccupiedHeadlinePrefix::UNOCCUPIED
        };

        let rows: &[(&str, OccupiedHeadlinePrefix, Option<TitleProblem>, &str)] = &[
            ("DONE", todo, None, "* TODO DONE\n"),
            (
                "DONE",
                OccupiedHeadlinePrefix::UNOCCUPIED,
                Some(TitleProblem::TodoKeyword),
                "* DONE\n",
            ),
            (
                "[#A] rest",
                OccupiedHeadlinePrefix::UNOCCUPIED,
                Some(TitleProblem::PriorityCookie),
                "* [#A] rest\n",
            ),
            ("[#A] rest", priority, None, "* [#A] [#A] rest\n"),
            (
                "[#A] rest",
                todo,
                Some(TitleProblem::PriorityCookie),
                "* TODO [#A] rest\n",
            ),
            (
                "COMMENT foo",
                OccupiedHeadlinePrefix::UNOCCUPIED,
                Some(TitleProblem::CommentMarker),
                "* COMMENT foo\n",
            ),
            ("COMMENT foo", commented, None, "* COMMENT COMMENT foo\n"),
            (
                "COMMENT foo",
                todo,
                Some(TitleProblem::CommentMarker),
                "* TODO COMMENT foo\n",
            ),
            (
                "foo :a:",
                todo,
                Some(TitleProblem::TrailingTagGroup),
                "* TODO foo :a:\n",
            ),
            (
                " DONE",
                todo,
                Some(TitleProblem::AbsorbedWhitespace),
                "* TODO  DONE\n",
            ),
        ];
        for &(title, occupied, want, fixture) in rows {
            assert_eq!(
                title_problem_given(title, &cfg, occupied),
                want,
                "occupied oracle {title:?} {occupied:?}"
            );
            if want.is_some() {
                continue;
            }
            let doc = parse_with(fixture, &cfg);
            let h = Heading::cast(
                nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING)
                    .into_iter()
                    .next()
                    .expect("heading"),
            )
            .expect("cast");
            let content = h.part_content_range(HeadingPart::Title).expect("title");
            assert_eq!(doc.slice(content), title, "fixture {fixture:?}");
            assert_eq!(h.todo_keyword().is_some(), occupied.todo, "{fixture:?}");
            assert_eq!(h.priority().is_some(), occupied.priority, "{fixture:?}");
            assert_eq!(h.is_commented(), occupied.commented, "{fixture:?}");
        }
    }

    #[test]
    fn title_with_markup_is_writable_and_carves() {
        let cfg = ParseConfig::default();
        let spec = spec(1, None, None, false, "*bold*", &[]);
        let bytes = render_heading(spec, &cfg).unwrap();
        assert_eq!(bytes, "* *bold*\n");
        let doc = parse_with(&bytes, &cfg);
        assert_heading_eq(&doc, &spec);
        let (_, h) = first_heading(&bytes);
        assert!(h.todo_keyword().is_none());
    }
}
