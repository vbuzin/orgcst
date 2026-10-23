//! Link objects in their three written forms: `[[path][description]]`,
//! `<type:path>` and plain `type:path`.
//!
//! Classifying a link (file, ID, URL, ...) reads the path text alone, so
//! [`classify_link`] also works on paths that did not come from the tree.
//! Radio links, plain-text matches of a `<<<radio target>>>`, are not parsed as
//! `LINK` nodes.

use crate::ast::layout::{LinkOpeners, link_description_removal, link_openers};
use crate::ast::support::{child_of_kind, child_view};
use crate::ast::{AstNode, HasObjects, HasParts, InsertPoint, Padding, PartSpan, PresentPart};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use std::borrow::Cow;

crate::ast_node!(
    /// A link object: `[[path][description]]`, `<type:path>` or plain
    /// `type:path`.
    Link,
    OrgSyntaxKind::LINK
);
crate::ast_node!(
    /// The description of a bracket link, between the second pair of
    /// brackets; it holds objects.
    LinkDescription,
    OrgSyntaxKind::LINK_DESCRIPTION
);

/// How a link is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkShape {
    /// `[[path]]` or `[[path][description]]`.
    Bracket,
    /// `<type:path>`.
    Angle,
    /// `type:path` in running text.
    Plain,
    /// No delimiters and no `TYPE:` prefix: a radio-link occurrence. The
    /// parser never produces such a `LINK` node; the variant is for callers of
    /// [`classify_link`] that find radio links themselves.
    Radio,
}

/// Parts of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkPart {
    /// The path, which says where the link points.
    Path,
    /// The description of a bracket link; not applicable to other shapes.
    Description,
}

/// What a link points to, by type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// A `file:` link, or a path starting with `/`, `./`, `../` or a drive
    /// letter.
    File {
        /// The file path, without the search option.
        path: String,
        /// Text after `::`.
        search_option: Option<String>,
        /// `APP` in `file+APP:`.
        application: Option<String>,
        /// Whether `file:` (or `file+APP:`) was written.
        type_explicit: bool,
    },
    /// `id:ID`.
    Id {
        /// The ID after `id:`.
        path: String,
    },
    /// `#custom-id`.
    CustomId {
        /// The custom ID, without the `#`.
        path: String,
    },
    /// `(ref)`, a reference to a code line label.
    Coderef {
        /// The label, without the parentheses.
        path: String,
    },
    /// `http`, `https`, `ftp`, `mailto` or `news`.
    Url {
        /// The scheme, lowercased, without the colon.
        scheme: String,
        /// Everything after `scheme:`.
        path: String,
    },
    /// Text without a type, such as `[[Some heading]]`, which Org resolves by
    /// searching.
    Fuzzy {
        /// The unescaped path, keeping a leading `*` (a heading search).
        path: String,
    },
    /// A radio-link occurrence; see [`LinkShape::Radio`].
    Radio {
        /// The text of the occurrence.
        path: String,
    },
    /// Any other `TYPE:`, such as `doi:`; the type is lowercased.
    Other {
        /// The link type, lowercased, without the colon.
        type_name: String,
        /// Everything after `TYPE:`.
        path: String,
    },
}

/// The classification of a link path, from [`classify_link`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClassifiedLink {
    /// The link type, lowercased: `file`, `id`, `https`, `fuzzy`, ...
    pub type_name: String,
    /// Whether the path wrote its `TYPE:` prefix.
    pub type_explicit: bool,
    /// The path after the type; for a file link, without the search option.
    pub path: String,
    /// A file link's text after `::`.
    pub search_option: Option<String>,
    /// `APP` in a `file+APP:` link.
    pub application: Option<String>,
    /// The same classification as a [`LinkTarget`].
    pub target: LinkTarget,
}

impl Link {
    /// How this link is written.
    #[must_use]
    pub fn shape(&self) -> LinkShape {
        shape_from_openers(link_openers(self.syntax()), &path_raw(self))
    }

    /// What this link points to.
    #[must_use]
    pub fn target(&self) -> LinkTarget {
        self.classified().target
    }

    /// Org's `:raw-link`: the path with bracket escapes removed and line
    /// breaks normalised.
    #[must_use]
    pub fn raw_link(&self) -> String {
        raw_link_of(self.shape(), &path_raw(self))
    }

    /// The link type, lowercased: `file`, `id`, `https`, `fuzzy`, ...
    #[must_use]
    pub fn type_name(&self) -> String {
        self.classified().type_name
    }

    /// Whether the path wrote an explicit `TYPE:` prefix.
    #[must_use]
    pub fn type_explicit(&self) -> bool {
        self.classified().type_explicit
    }

    /// The path after the type; for a file link, without the search option.
    #[must_use]
    pub fn path(&self) -> String {
        self.classified().path
    }

    /// A file link's search option after `::`, when present.
    #[must_use]
    pub fn search_option(&self) -> Option<String> {
        self.classified().search_option
    }

    /// `APP` in a `file+APP:` link, when present.
    #[must_use]
    pub fn application(&self) -> Option<String> {
        self.classified().application
    }

    /// The description of a bracket link, if any.
    #[must_use]
    pub fn description(&self) -> Option<LinkDescription> {
        child_view(self.syntax())
    }

    /// The `LINK_PATH` node.
    ///
    /// # Panics
    ///
    /// Panics if `LINK_PATH` is missing, which the parser never produces.
    #[must_use]
    pub fn path_node(&self) -> SyntaxNode {
        child_of_kind(self.syntax(), OrgSyntaxKind::LINK_PATH)
            .expect("grammar: every LINK has exactly one LINK_PATH (forms/link.rs)")
    }

    /// The whole classification at once.
    ///
    /// [`Self::path`], [`Self::type_name`] and [`Self::target`] each classify
    /// again, so prefer this when you need more than one of them.
    #[must_use]
    pub fn classified(&self) -> ClassifiedLink {
        classify_link(self.shape(), &path_raw(self))
    }
}

impl HasParts for Link {
    type Part = LinkPart;
    const PARTS: &'static [LinkPart] = &[LinkPart::Path, LinkPart::Description];

    fn part_span(&self, part: LinkPart) -> PartSpan {
        match part {
            LinkPart::Path => {
                let carved = self.path_node().text_range();
                PartSpan::Present(PresentPart::identical(carved))
            }
            LinkPart::Description => match self.shape() {
                LinkShape::Angle | LinkShape::Plain | LinkShape::Radio => PartSpan::NotApplicable,
                LinkShape::Bracket => match self.description() {
                    Some(desc) => {
                        let carved = desc.range();
                        let removal = link_description_removal(self.syntax()).unwrap_or(carved);
                        PartSpan::Present(PresentPart::new(carved, carved, removal))
                    }
                    None => PartSpan::Absent(InsertPoint::new(
                        self.path_node().text_range().end(),
                        Padding::None,
                        Padding::None,
                    )),
                },
            },
        }
    }
}

impl HasObjects for LinkDescription {}

fn path_raw(link: &Link) -> String {
    link.path_node().text().to_string()
}

/// Whether `path_raw` starts with a type: an ASCII letter, then letters,
/// digits or `-`, then `:`.
#[must_use]
pub(crate) fn has_explicit_type_prefix(path_raw: &str) -> bool {
    let Some(end) = type_run_end(path_raw) else {
        return false;
    };
    path_raw.as_bytes().get(end) == Some(&b':')
}

/// Shape from the link's delimiters plus the `TYPE:` test.
///
/// Separate from [`Link::shape`] so the radio branch, which no parsed link
/// reaches, can be tested.
#[must_use]
pub(crate) fn shape_from_openers(openers: LinkOpeners, path_raw: &str) -> LinkShape {
    match openers {
        LinkOpeners::Bracket => LinkShape::Bracket,
        LinkOpeners::Angle => LinkShape::Angle,
        LinkOpeners::Plain => {
            if has_explicit_type_prefix(path_raw) {
                LinkShape::Plain
            } else {
                LinkShape::Radio
            }
        }
    }
}

/// Undo the escaping of a bracket link path, as Org's `org-link-unescape`
/// does.
///
/// A run of backslashes directly before `[`, `]` or the end of the path is
/// halved, rounding down; the odd one escaped the bracket. Backslashes
/// anywhere else are literal and stay.
#[must_use]
pub fn unescape_link_path(raw: &str) -> Cow<'_, str> {
    map_backslash_runs(raw, |run, _| run / 2)
}

/// Escape a path for a bracket link, as Org's `org-link-escape` does.
///
/// A run of backslashes directly before `[`, `]` or the end of the path is
/// doubled, and each bracket gets one more backslash in front. Backslashes
/// anywhere else are left alone, so `a\b` stays `a\b`.
///
/// `unescape_link_path(&escape_link_path(s)) == s` for every `s`.
#[must_use]
pub fn escape_link_path(path: &str) -> Cow<'_, str> {
    map_backslash_runs(path, |run, before_bracket| {
        run * 2 + usize::from(before_bracket)
    })
}

/// Rewrite every run of backslashes (possibly empty) that sits directly
/// before `[`, `]` or the end of `s`. `count(run, before_bracket)` is the
/// number of backslashes to write in its place.
fn map_backslash_runs(s: &str, count: impl Fn(usize, bool) -> usize) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut i = 0;
    while i <= bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] == b'\\' {
            i += 1;
        }
        let run = i - start;
        let at_end = i == bytes.len();
        let before_bracket = !at_end && matches!(bytes[i], b'[' | b']');
        if (at_end || before_bracket) && count(run, before_bracket) != run {
            let o = out.get_or_insert_with(|| String::with_capacity(s.len() + 4));
            o.push_str(&s[copied..start]);
            o.extend(std::iter::repeat_n('\\', count(run, before_bracket)));
            copied = i;
        }
        if at_end {
            break;
        }
        // Step past the byte that ended the run. It is not a backslash, and
        // edits only ever cut at a backslash or a bracket, which are ASCII,
        // so stepping into a multi-byte character is harmless.
        i += 1;
    }
    match out {
        Some(mut o) => {
            o.push_str(&s[copied..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(s),
    }
}

/// Normalise line breaks in a bracket link path as Org's `:raw-link` does:
/// each line break, with the spaces and tabs around it, becomes one space.
#[must_use]
pub fn normalize_bracket_link_ws(raw: &str) -> Cow<'_, str> {
    if !raw.contains(['\n', '\r']) {
        return Cow::Borrowed(raw);
    }
    Cow::Owned(collapse_line_wraps(raw, true))
}

/// Normalise line breaks in an angle link path: each line break, with the
/// spaces and tabs around it, is deleted.
#[must_use]
pub fn normalize_angle_link_ws(raw: &str) -> Cow<'_, str> {
    if !raw.contains(['\n', '\r']) {
        return Cow::Borrowed(raw);
    }
    Cow::Owned(collapse_line_wraps(raw, false))
}

fn collapse_line_wraps(raw: &str, insert_space: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            while matches!(out.chars().last(), Some(' ' | '\t')) {
                out.pop();
            }
            while matches!(chars.peek(), Some(' ' | '\t')) {
                chars.next();
            }
            if insert_space {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Classify a link path written in `shape`.
///
/// The rules do not depend on which link types are registered. A radio shape
/// is a radio link; a path starting with `/`, `./`, `../` or a drive letter is
/// a file; next an explicit `TYPE:` decides; then `(ref)` is a coderef, `#id`
/// a custom ID, and anything else is fuzzy.
#[must_use]
pub fn classify_link(shape: LinkShape, path_raw: &str) -> ClassifiedLink {
    let s = raw_link_of(shape, path_raw);
    if shape == LinkShape::Radio {
        return ClassifiedLink {
            type_name: "radio".to_string(),
            type_explicit: false,
            path: s.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Radio { path: s },
        };
    }
    if is_absolute_or_relative_file(&s) {
        return file_classified(s, false, None);
    }
    if let Some(cls) = classify_explicit_type(&s) {
        return cls;
    }
    if s.len() >= 2 && s.starts_with('(') && s.ends_with(')') {
        let inner = s[1..s.len() - 1].to_string();
        return ClassifiedLink {
            type_name: "coderef".to_string(),
            type_explicit: false,
            path: inner.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Coderef { path: inner },
        };
    }
    if let Some(rest) = s.strip_prefix('#') {
        return ClassifiedLink {
            type_name: "custom-id".to_string(),
            type_explicit: false,
            path: rest.to_string(),
            search_option: None,
            application: None,
            target: LinkTarget::CustomId {
                path: rest.to_string(),
            },
        };
    }
    ClassifiedLink {
        type_name: "fuzzy".to_string(),
        type_explicit: false,
        path: s.clone(),
        search_option: None,
        application: None,
        target: LinkTarget::Fuzzy { path: s },
    }
}

fn raw_link_of(shape: LinkShape, path_raw: &str) -> String {
    match shape {
        LinkShape::Bracket => {
            let u = unescape_link_path(path_raw);
            normalize_bracket_link_ws(u.as_ref()).into_owned()
        }
        LinkShape::Angle => normalize_angle_link_ws(path_raw).into_owned(),
        LinkShape::Plain | LinkShape::Radio => path_raw.to_string(),
    }
}

fn type_run_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_alphabetic() {
        return None;
    }
    let mut i = 1usize;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
        i += 1;
    }
    Some(i)
}

fn is_absolute_or_relative_file(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("./") || s.starts_with("../") || is_windows_drive(s)
}

fn is_windows_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

fn classify_explicit_type(s: &str) -> Option<ClassifiedLink> {
    let type_end = type_run_end(s)?;
    let b = s.as_bytes();
    if b.get(type_end) == Some(&b'+') && s[..type_end].eq_ignore_ascii_case("file") {
        let rest = &s[type_end + 1..];
        let app_end = type_run_end(rest)?;
        if rest.as_bytes().get(app_end) != Some(&b':') {
            return None;
        }
        let application = Some(rest[..app_end].to_string());
        let path_rest = rest[app_end + 1..].to_string();
        return Some(file_classified(path_rest, true, application));
    }
    if b.get(type_end) != Some(&b':') {
        return None;
    }
    let type_name = s[..type_end].to_ascii_lowercase();
    let path_rest = s[type_end + 1..].to_string();
    Some(map_scheme(&type_name, path_rest, true, None))
}

fn map_scheme(
    type_name: &str,
    path: String,
    type_explicit: bool,
    application: Option<String>,
) -> ClassifiedLink {
    match type_name {
        "http" | "https" | "ftp" | "mailto" | "news" => ClassifiedLink {
            type_name: type_name.to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Url {
                scheme: type_name.to_string(),
                path,
            },
        },
        "file" => file_classified(path, type_explicit, application),
        "id" => ClassifiedLink {
            type_name: "id".to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Id { path },
        },
        "coderef" => ClassifiedLink {
            type_name: "coderef".to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Coderef { path },
        },
        "custom-id" => ClassifiedLink {
            type_name: "custom-id".to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::CustomId { path },
        },
        "fuzzy" => ClassifiedLink {
            type_name: "fuzzy".to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Fuzzy { path },
        },
        "radio" => ClassifiedLink {
            type_name: "radio".to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Radio { path },
        },
        _ => ClassifiedLink {
            type_name: type_name.to_string(),
            type_explicit,
            path: path.clone(),
            search_option: None,
            application: None,
            target: LinkTarget::Other {
                type_name: type_name.to_string(),
                path,
            },
        },
    }
}

fn file_classified(
    path: String,
    type_explicit: bool,
    application: Option<String>,
) -> ClassifiedLink {
    let (path, search_option) = match path.split_once("::") {
        Some((left, right)) => (left.to_string(), Some(right.to_string())),
        None => (path, None),
    };
    ClassifiedLink {
        type_name: "file".to_string(),
        type_explicit,
        path: path.clone(),
        search_option: search_option.clone(),
        application: application.clone(),
        target: LinkTarget::File {
            path,
            search_option,
            application,
            type_explicit,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::LinkOpeners;
    use crate::ast::{HasObjects, HasParts, Object, PartSpan};
    use crate::parse;
    use crate::parse_with;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind, source_slice, written};
    use crate::{DEFAULT_LINK_TYPES, ParseConfig};

    fn first_link(src: &str) -> (crate::Document, Link) {
        let doc = parse(src);
        let link = Link::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
                .into_iter()
                .next()
                .expect("link"),
        )
        .expect("cast");
        (doc, link)
    }

    #[test]
    fn has_explicit_type_prefix_scan() {
        assert!(has_explicit_type_prefix("https://ex.com"));
        assert!(has_explicit_type_prefix("doi:10.1"));
        assert!(has_explicit_type_prefix("a:"));
        assert!(!has_explicit_type_prefix("file+sys:~/a"));
        assert!(!has_explicit_type_prefix("123:x"));
        assert!(!has_explicit_type_prefix("a"));
        assert!(!has_explicit_type_prefix(""));
        assert!(has_explicit_type_prefix("org-id:x"));
        assert!(has_explicit_type_prefix("a-b:"));
        assert!(!has_explicit_type_prefix("foo_bar:x"));
        assert!(!has_explicit_type_prefix("a.b:x"));
    }

    #[test]
    fn shape_from_openers_plain_vs_radio() {
        assert_eq!(
            shape_from_openers(LinkOpeners::Plain, "https://x"),
            LinkShape::Plain
        );
        assert_eq!(
            shape_from_openers(LinkOpeners::Plain, "hello world"),
            LinkShape::Radio
        );
        assert_eq!(
            shape_from_openers(LinkOpeners::Bracket, "hello"),
            LinkShape::Bracket
        );
    }

    #[test]
    fn unescape_follows_org_link_unescape() {
        assert_eq!(unescape_link_path("a\\[b\\]c"), "a[b]c");
        assert_eq!(unescape_link_path("a\\\\\\[b"), "a\\[b");
        assert_eq!(unescape_link_path("a\\\\b"), "a\\\\b");
        assert_eq!(unescape_link_path("a\\nb"), "a\\nb");
        assert_eq!(unescape_link_path("a\\\\"), "a\\");
        assert_eq!(unescape_link_path("a\\"), "a");
        assert!(matches!(
            unescape_link_path("plain"),
            Cow::Borrowed("plain")
        ));
    }

    #[test]
    fn escape_follows_org_link_escape() {
        for s in [
            "",
            "plain",
            "a[b]c",
            "a\\b",
            "a\\\\b",
            "[]\\",
            "\\[\\]",
            "x\\",
            "https://ex.com/a[b]",
            "café[x]\\",
            "C:\\dir\\file",
        ] {
            assert_eq!(unescape_link_path(&escape_link_path(s)), s, "{s:?}");
        }
        assert!(matches!(escape_link_path("plain"), Cow::Borrowed("plain")));
        assert!(matches!(escape_link_path("a\\b"), Cow::Borrowed("a\\b")));
        assert_eq!(escape_link_path("a[b]c"), "a\\[b\\]c");
        assert_eq!(escape_link_path("a\\[b"), "a\\\\\\[b");
        assert_eq!(escape_link_path("a\\"), "a\\\\");
        // A literal backslash before a letter is written as is.
        assert_eq!(escape_link_path("\\n"), "\\n");
    }

    #[test]
    fn bracket_multiline_raw_link_collapses_ws() {
        assert_eq!(normalize_bracket_link_ws("a\nb"), "a b");
        assert_eq!(normalize_bracket_link_ws("a  \n  b"), "a b");
        assert_eq!(normalize_bracket_link_ws("a\r\nb"), "a b");
        assert!(matches!(
            normalize_bracket_link_ws("ab"),
            Cow::Borrowed("ab")
        ));
    }

    #[test]
    fn angle_multiline_path_strips_indent_ws() {
        assert_eq!(normalize_angle_link_ws("a\nb"), "ab");
        assert_eq!(normalize_angle_link_ws("a  \n  b"), "ab");
        assert!(matches!(normalize_angle_link_ws("ab"), Cow::Borrowed("ab")));
    }

    #[test]
    fn classify_schemes_and_file_post_process() {
        let c = classify_link(LinkShape::Plain, "https://ex.com");
        assert_eq!(c.type_name, "https");
        assert!(c.type_explicit);
        assert!(matches!(c.target, LinkTarget::Url { .. }));

        let c = classify_link(LinkShape::Bracket, "file:a.org::*H");
        assert_eq!(c.path, "a.org");
        assert_eq!(c.search_option.as_deref(), Some("*H"));
        assert!(c.type_explicit);

        let c = classify_link(LinkShape::Plain, "./a::search");
        assert!(!c.type_explicit);
        assert_eq!(c.path, "./a");
        assert_eq!(c.search_option.as_deref(), Some("search"));

        let c = classify_link(LinkShape::Bracket, "file+sys:~/x");
        assert_eq!(c.application.as_deref(), Some("sys"));
        assert_eq!(c.path, "~/x");

        let c = classify_link(LinkShape::Bracket, "(ref)");
        assert!(matches!(c.target, LinkTarget::Coderef { .. }));
        assert_eq!(c.path, "ref");

        let c = classify_link(LinkShape::Bracket, "#id");
        assert!(matches!(c.target, LinkTarget::CustomId { .. }));
        assert_eq!(c.path, "id");

        let c = classify_link(LinkShape::Bracket, "   ");
        assert!(matches!(c.target, LinkTarget::Fuzzy { .. }));

        let c = classify_link(LinkShape::Radio, "phrase");
        assert!(matches!(c.target, LinkTarget::Radio { .. }));
        assert_eq!(c.type_name, "radio");

        let c = classify_link(LinkShape::Plain, "shell:ls");
        assert!(matches!(c.target, LinkTarget::Other { .. }));

        for scheme in ["http", "ftp", "mailto", "news"] {
            let c = classify_link(LinkShape::Plain, &format!("{scheme}:x"));
            assert!(
                matches!(c.target, LinkTarget::Url { .. }),
                "{scheme} is Url"
            );
        }
        for scheme in ["elisp", "help"] {
            let c = classify_link(LinkShape::Plain, &format!("{scheme}:x"));
            assert!(
                matches!(c.target, LinkTarget::Other { .. }),
                "{scheme} is Other"
            );
        }

        let c = classify_link(LinkShape::Plain, "id:uuid");
        assert!(matches!(c.target, LinkTarget::Id { .. }));

        let c = classify_link(LinkShape::Bracket, "C:/a.org");
        assert!(matches!(
            c.target,
            LinkTarget::File {
                type_explicit: false,
                ..
            }
        ));
        let c = classify_link(LinkShape::Bracket, "C:foo");
        assert!(
            !matches!(c.target, LinkTarget::File { .. }),
            "lone drive letter without slash is not a file path"
        );
    }

    #[test]
    fn shapes_and_plain_range_equality() {
        let (_, br) = first_link("[[path][site]]\n");
        assert_eq!(br.shape(), LinkShape::Bracket);
        crate::ast::check_part_contract(&br);
        assert!(br.description().is_some());

        let (_, ang) = first_link("<https://ex.com>\n");
        assert_eq!(ang.shape(), LinkShape::Angle);
        assert!(matches!(
            ang.part_span(LinkPart::Description),
            PartSpan::NotApplicable
        ));

        let (_, plain) = first_link("https://ex.com\n");
        assert_eq!(plain.shape(), LinkShape::Plain);
        assert_eq!(plain.range(), plain.path_node().text_range());
        assert!(matches!(
            plain.part_span(LinkPart::Description),
            PartSpan::NotApplicable
        ));
    }

    #[test]
    fn custom_protocol_plain_shape_not_radio() {
        let types: Vec<&str> = DEFAULT_LINK_TYPES
            .iter()
            .copied()
            .chain(std::iter::once("doi"))
            .collect();
        let cfg = ParseConfig::default().with_link_types(types);
        let doc = parse_with("doi:10.1/x\n", &cfg);
        let link = Link::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LINK)
                .into_iter()
                .next()
                .expect("doi link"),
        )
        .expect("cast");
        assert_eq!(link.shape(), LinkShape::Plain);
        assert!(matches!(
            link.target(),
            LinkTarget::Other { ref type_name, .. } if type_name == "doi"
        ));
    }

    #[test]
    fn description_removal_and_absent_insert() {
        let (doc, link) = first_link("[[path][site]]\n");
        let rm = link.part_removal_range(LinkPart::Description).unwrap();
        assert_eq!(source_slice(&doc, rm), "][site");
        crate::ast::check_part_contract(&link);

        let (doc, empty) = first_link("[[t][]]\n");
        assert!(empty.description().is_some());
        assert_eq!(
            source_slice(
                &doc,
                empty.part_removal_range(LinkPart::Description).unwrap()
            ),
            "]["
        );
        assert!(
            empty
                .part_content_range(LinkPart::Description)
                .unwrap()
                .is_empty()
        );

        let (doc, none) = first_link("[[path]]\n");
        let ip = none.part_insert_point(LinkPart::Description).unwrap();
        assert_eq!(written(doc.source(), ip, "][site"), "[[path][site]]\n");
    }

    #[test]
    fn bracket_newline_and_escape_and_ws_only_path() {
        let (_, link) = first_link("[[a\nb]]\n");
        assert_eq!(link.raw_link(), "a b");
        crate::ast::check_part_contract(&link);

        let (_, esc) = first_link("[[a\\]b]]\n");
        assert_eq!(esc.raw_link(), "a]b");

        let (_, ws) = first_link("[[   ]]\n");
        assert!(matches!(ws.target(), LinkTarget::Fuzzy { .. }));
    }

    #[test]
    fn description_has_objects() {
        let (_, link) = first_link("[[t][*bold*]]\n");
        let desc = link.description().expect("desc");
        let objs: Vec<_> = desc.objects().collect();
        assert_eq!(objs.len(), 1);
        assert!(matches!(objs[0], Object::Markup(_)));
    }

    #[test]
    fn nested_bracket_in_description_is_not_a_link_object() {
        let (doc, link) = first_link("[[t][see [[nested]]]]\n");
        assert_eq!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::LINK).len(),
            1,
            "restriction: nested [[ ]] is not a LINK"
        );
        let desc = link.description().expect("desc");
        assert!(
            !desc.objects().any(|o| matches!(o, Object::Link(_))),
            "description objects exclude nested link syntax"
        );
    }

    #[test]
    fn check_part_contract_over_taxonomy() {
        for &(src, cfg) in TAXONOMY {
            let doc = parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::LINK) {
                if let Some(link) = Link::cast(n) {
                    crate::ast::check_part_contract(&link);
                }
            }
        }
    }

    #[test]
    fn part_at_empty_description() {
        let (_, link) = first_link("[[t][]]\n");
        let desc = link.part_range(LinkPart::Description).unwrap();
        assert_eq!(link.part_at(desc.start()), Some(LinkPart::Description));
    }
}
