//! Object inserts by Org's names: insert at an offset, wrap a range, and
//! unwrap an object back to its words.
//!
//! Every insert is checked by a trial parse: the new text must hold the
//! object exactly where it was written, and every other node must read back
//! as it did. A site whose scope does not admit the object, or a range that
//! cuts through another object, is refused before the trial.

use super::empty_edit_set;
use super::map_synthesis;
use crate::ast::{AstNode, Link, LinkShape, RadioTarget, Target};
use crate::edit::{Edit, EditSet};
use crate::syntax::{ObjectSet, restrictions};
use crate::writer::{invalid_link_description, render_link};
use crate::{
    Document, NodeRef, OperationError, OrgSyntaxKind, SyntaxNode, TextRange, TextSize,
    escape_link_path, unescape_link_path,
};

/// An object to write, by Org's names. Text arguments are content; core
/// writes the delimiters and escapes what needs escaping.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ObjectInput {
    /// A link. `path` is the whole link path, type included (`id:...`,
    /// `https://...`, `*Title`, `file:x.org::*Title`), unescaped.
    /// [`LinkShape::Radio`] is [`OperationError::InvalidLink`]: a radio link
    /// is text that matches a radio target, never written as a link.
    Link {
        /// How the link is written.
        shape: LinkShape,
        /// The path, unescaped.
        path: String,
        /// The description of a bracket link. When wrapping, the wrapped
        /// text is the description and this must be `None`.
        description: Option<String>,
    },
    /// A radio target, `<<<text>>>`. When wrapping, the wrapped text is the
    /// target's text and `text` must be empty.
    RadioTarget {
        /// The target's text.
        text: String,
    },
    /// A dedicated target, `<<name>>`. When wrapping, the wrapped text is
    /// the name and `name` must be empty.
    Target {
        /// The target's name.
        name: String,
    },
}

impl ObjectInput {
    /// The kind of node this input writes.
    #[must_use]
    pub fn kind(&self) -> OrgSyntaxKind {
        match self {
            Self::Link { .. } => OrgSyntaxKind::LINK,
            Self::RadioTarget { .. } => OrgSyntaxKind::RADIO_TARGET,
            Self::Target { .. } => OrgSyntaxKind::TARGET,
        }
    }

    /// The node kind whose restrictions the wrapped text must meet, and the
    /// wrapper node the parser puts around it (if any).
    fn content_scope(&self) -> (OrgSyntaxKind, Option<OrgSyntaxKind>) {
        match self {
            Self::Link { .. } => (
                OrgSyntaxKind::LINK_DESCRIPTION,
                Some(OrgSyntaxKind::LINK_DESCRIPTION),
            ),
            Self::RadioTarget { .. } => (OrgSyntaxKind::RADIO_TARGET, None),
            Self::Target { .. } => (OrgSyntaxKind::TARGET, None),
        }
    }
}

const UNWRAP_KINDS: &[OrgSyntaxKind] = &[
    OrgSyntaxKind::LINK,
    OrgSyntaxKind::RADIO_TARGET,
    OrgSyntaxKind::TARGET,
];

pub(super) fn compile_insert_object(
    doc: &Document,
    at: TextSize,
    object: &ObjectInput,
) -> Result<EditSet, OperationError> {
    if at > TextSize::of(doc.source()) {
        return Err(OperationError::InvalidOffset);
    }
    let text = render_whole(object)?;
    let kind = object.kind();
    let scope = scope_at(doc, TextRange::empty(at));
    let allowed = restrictions(scope.kind());
    // A site among elements (a blank line, the end of the note) holds no
    // objects of its own: the insert starts a paragraph there, which the
    // trial checks. Anywhere else the scope's restrictions decide.
    let among_elements = allowed == ObjectSet::EMPTY && !is_object(scope.kind());
    if !allowed.contains(kind) && !among_elements {
        return Err(OperationError::NotAllowedHere {
            kind,
            scope: scope.kind(),
        });
    }
    let set = EditSet::single(doc.source(), TextRange::empty(at), text.clone())?;
    let written = TextRange::at(at, TextSize::of(text.as_str()));
    let blank = blank_line(doc.source(), usize::from(at));
    trial(
        doc,
        &set,
        kind,
        written,
        None,
        scope.kind(),
        among_elements || blank,
    )?;
    Ok(set)
}

pub(super) fn compile_wrap_in_object(
    doc: &Document,
    range: TextRange,
    object: &ObjectInput,
) -> Result<EditSet, OperationError> {
    if range.is_empty() || range.end() > TextSize::of(doc.source()) {
        return Err(OperationError::InvalidOffset);
    }
    let kind = object.kind();
    let scope = scope_at(doc, range);
    if !restrictions(scope.kind()).contains(kind) {
        return Err(OperationError::NotAllowedHere {
            kind,
            scope: scope.kind(),
        });
    }
    let (content_scope, wrapper) = object.content_scope();
    let allowed = restrictions(content_scope);
    for node in scope.descendants().skip(1) {
        let r = node.text_range();
        let inside = range.contains_range(r);
        let apart = r.end() <= range.start() || r.start() >= range.end();
        if !inside && !apart {
            return Err(OperationError::CrossesObject);
        }
        // Direct objects of the wrapped run must be allowed in the new
        // object's contents; deeper nodes were checked by their own parent.
        if inside && node.parent().is_some_and(|p| p == scope) && !allowed.contains(node.kind()) {
            return Err(OperationError::NotAllowedHere {
                kind: node.kind(),
                scope: content_scope,
            });
        }
    }
    let (open, close) = render_around(object, doc.slice(range))?;
    let open_len = TextSize::of(open.as_str());
    let set = EditSet::new(
        doc.source(),
        vec![
            Edit {
                range: TextRange::empty(range.start()),
                replacement: open,
            },
            Edit {
                range: TextRange::empty(range.end()),
                replacement: close.clone(),
            },
        ],
    )?;
    let written = TextRange::new(
        range.start(),
        range.end() + open_len + TextSize::of(close.as_str()),
    );
    let content = TextRange::at(range.start() + open_len, range.len());
    trial(
        doc,
        &set,
        kind,
        written,
        Some((content, wrapper)),
        scope.kind(),
        false,
    )?;
    Ok(set)
}

pub(super) fn compile_unwrap_object(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let node = target.resolve(doc).ok_or(OperationError::StaleTarget)?;
    let range = node.text_range();
    match node.kind() {
        OrgSyntaxKind::LINK => {
            let link = Link::cast(node).expect("grammar: LINK kind always casts");
            if let Some(desc) = link.description() {
                let inner = desc.syntax().text_range();
                return Ok(EditSet::new(
                    doc.source(),
                    vec![
                        Edit {
                            range: TextRange::new(range.start(), inner.start()),
                            replacement: String::new(),
                        },
                        Edit {
                            range: TextRange::new(inner.end(), range.end()),
                            replacement: String::new(),
                        },
                    ],
                )?);
            }
            let words = match link.shape() {
                LinkShape::Bracket => {
                    unescape_link_path(doc.slice(link.path_node().text_range())).into_owned()
                }
                LinkShape::Angle => doc.slice(link.path_node().text_range()).to_string(),
                LinkShape::Plain | LinkShape::Radio => return Err(OperationError::InvalidLink),
            };
            let set = EditSet::single(doc.source(), range, words.clone())?;
            // An address that is a link wherever it is written cannot be
            // unwrapped.
            let next = doc.apply(&set);
            let written = TextRange::at(range.start(), TextSize::of(words.as_str()));
            if next.root().descendants().any(|n| {
                n.kind() == OrgSyntaxKind::LINK
                    && n.text_range()
                        .intersect(written)
                        .is_some_and(|r| !r.is_empty())
            }) {
                return Err(OperationError::InvalidLink);
            }
            Ok(set)
        }
        OrgSyntaxKind::RADIO_TARGET | OrgSyntaxKind::TARGET => {
            let inner = match RadioTarget::cast(node.clone()) {
                Some(radio) => radio.key_range(),
                None => Target::cast(node)
                    .expect("grammar: TARGET kind always casts")
                    .name_range(),
            };
            if inner == range {
                return Ok(empty_edit_set(doc.source()));
            }
            Ok(EditSet::new(
                doc.source(),
                vec![
                    Edit {
                        range: TextRange::new(range.start(), inner.start()),
                        replacement: String::new(),
                    },
                    Edit {
                        range: TextRange::new(inner.end(), range.end()),
                        replacement: String::new(),
                    },
                ],
            )?)
        }
        actual => Err(OperationError::WrongKind {
            expected: UNWRAP_KINDS,
            actual,
        }),
    }
}

/// The whole object as text, for an insert.
fn render_whole(object: &ObjectInput) -> Result<String, OperationError> {
    match object {
        ObjectInput::Link {
            shape,
            path,
            description,
        } => render_link(*shape, path, description.as_deref()).map_err(map_synthesis),
        ObjectInput::RadioTarget { text } => {
            check_target_text(text)?;
            Ok(format!("<<<{text}>>>"))
        }
        ObjectInput::Target { name } => {
            check_target_text(name)?;
            Ok(format!("<<{name}>>"))
        }
    }
}

/// The opener and closer written around `content`, for a wrap.
fn render_around(object: &ObjectInput, content: &str) -> Result<(String, String), OperationError> {
    match object {
        ObjectInput::Link {
            shape,
            path,
            description,
        } => {
            if *shape != LinkShape::Bracket || description.is_some() {
                return Err(OperationError::InvalidLink);
            }
            if path.is_empty() || path.contains(['\n', '\r']) || invalid_link_description(content) {
                return Err(OperationError::InvalidLink);
            }
            Ok((format!("[[{}][", escape_link_path(path)), "]]".to_string()))
        }
        ObjectInput::RadioTarget { text } => {
            if !text.is_empty() {
                return Err(OperationError::InvalidText);
            }
            check_target_text(content)?;
            Ok(("<<<".to_string(), ">>>".to_string()))
        }
        ObjectInput::Target { name } => {
            if !name.is_empty() {
                return Err(OperationError::InvalidText);
            }
            check_target_text(content)?;
            Ok(("<<".to_string(), ">>".to_string()))
        }
    }
}

/// A target's text: not empty, on one line, no angle brackets, and not
/// starting or ending with whitespace, as Org's target syntax requires.
fn check_target_text(text: &str) -> Result<(), OperationError> {
    let edge = |c: Option<char>| c.is_none_or(char::is_whitespace);
    if text.contains(['\n', '\r', '<', '>'])
        || edge(text.chars().next())
        || edge(text.chars().next_back())
    {
        return Err(OperationError::InvalidText);
    }
    Ok(())
}

/// The innermost node holding `range` whose contents are objects, or the
/// innermost node holding it when none is. A node holds an empty range at
/// its end only if it is the last node to end there, so an insert at a
/// paragraph's end lands in the paragraph, while one at the end of a link
/// lands beside it.
fn scope_at(doc: &Document, range: TextRange) -> SyntaxNode {
    let root = doc.root();
    let mut node = root.clone();
    'descend: loop {
        for child in node.children() {
            let r = child.text_range();
            let holds = if range.is_empty() {
                let at = range.start();
                r.start() <= at && at < r.end()
                    || (at == r.end() && at > r.start() && ends_line(doc, &child))
            } else {
                r.contains_range(range)
            };
            if holds {
                node = child;
                continue 'descend;
            }
        }
        break;
    }
    node
}

/// Whether `node` ends where its line ends, or at the end of the source, so
/// an empty range at its end still counts as inside it (the end of a title
/// or a paragraph's last line before its newline).
fn ends_line(doc: &Document, node: &SyntaxNode) -> bool {
    if is_object(node.kind()) {
        return false;
    }
    let end = usize::from(node.text_range().end());
    let src = doc.source();
    end == src.len() || src[end..].starts_with(['\n', '\r'])
}

/// Whether the line holding byte `at` is empty or only blanks.
fn blank_line(src: &str, at: usize) -> bool {
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = src[at..].find('\n').map_or(src.len(), |i| at + i);
    src[start..end].trim().is_empty()
}

fn is_object(kind: OrgSyntaxKind) -> bool {
    crate::ast::Object::can_cast(kind)
}

/// Apply `set` and check the new text: a node of `kind` exactly at
/// `written`; with `content`, its contents at that range (inside the named
/// wrapper node); and every other node reading back as before. With
/// `new_paragraph`, the object may also sit in a paragraph of its own that
/// the insert made.
fn trial(
    doc: &Document,
    set: &EditSet,
    kind: OrgSyntaxKind,
    written: TextRange,
    content: Option<(TextRange, Option<OrgSyntaxKind>)>,
    scope: OrgSyntaxKind,
    new_paragraph: bool,
) -> Result<(), OperationError> {
    let refused = OperationError::NotAllowedHere { kind, scope };
    let next = doc.apply(set);
    let Some(new) = next
        .root()
        .descendants()
        .find(|n| n.kind() == kind && n.text_range() == written)
    else {
        return Err(refused);
    };
    let skip = |n: &SyntaxNode| {
        let r = n.text_range();
        if *n != new && !n.ancestors().any(|a| a == new) {
            return false;
        }
        match content {
            None => true,
            Some((inner, wrapper)) => {
                *n == new || !inner.contains_range(r) || (r == inner && Some(n.kind()) == wrapper)
            }
        }
    };
    let before: Vec<OrgSyntaxKind> = doc.root().descendants().map(|n| n.kind()).collect();
    let kinds = |also: &[SyntaxNode]| -> Vec<OrgSyntaxKind> {
        next.root()
            .descendants()
            .filter(|n| !skip(n) && !also.contains(n))
            .map(|n| n.kind())
            .collect()
    };
    // The paragraph (and section) the insert made, each holding only the
    // new object.
    let mut made = Vec::new();
    if new_paragraph {
        let mut up = new.parent();
        while let Some(p) = up {
            if !matches!(p.kind(), OrgSyntaxKind::PARAGRAPH | OrgSyntaxKind::SECTION)
                || p.children().count() != 1
            {
                break;
            }
            up = p.parent();
            made.push(p);
        }
    }
    if !(0..=made.len()).any(|k| before == kinds(&made[..k])) {
        return Err(refused);
    }
    if let Some((inner, Some(wrapper))) = content
        && !new
            .children()
            .any(|c| c.kind() == wrapper && c.text_range() == inner)
    {
        return Err(refused);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Operation, compile_write};
    use crate::{parse, parse_with};

    fn insert(src: &str, at: u32, object: ObjectInput) -> Result<String, OperationError> {
        let doc = parse(src);
        let op = Operation::InsertObject {
            at: TextSize::from(at),
            object,
        };
        crate::compile(&doc, &op)?;
        Ok(compile_write(&doc, &op).1)
    }

    fn wrap(src: &str, from: &str, object: ObjectInput) -> Result<String, OperationError> {
        let doc = parse(src);
        let start = u32::try_from(src.find(from).expect("in source")).expect("small");
        let len = u32::try_from(from.len()).expect("small");
        let op = Operation::WrapInObject {
            range: TextRange::at(TextSize::from(start), TextSize::from(len)),
            object,
        };
        crate::compile(&doc, &op)?;
        Ok(compile_write(&doc, &op).1)
    }

    fn link(path: &str, description: Option<&str>) -> ObjectInput {
        ObjectInput::Link {
            shape: LinkShape::Bracket,
            path: path.to_string(),
            description: description.map(str::to_string),
        }
    }

    fn bare_link(path: &str) -> ObjectInput {
        link(path, None)
    }

    #[test]
    fn insert_link_in_paragraph_title_and_cell() {
        assert_eq!(
            insert("See  now.\n", 4, link("id:abc", Some("this"))).unwrap(),
            "See [[id:abc][this]] now.\n"
        );
        assert_eq!(
            insert("* Title\n", 7, link("https://example.com", None)).unwrap(),
            "* Title[[https://example.com]]\n"
        );
        assert_eq!(
            insert("| a |\n", 3, link("*H", Some("h"))).unwrap(),
            "| a[[*H][h]] |\n"
        );
    }

    #[test]
    fn insert_link_at_paragraph_end_and_source_end() {
        assert_eq!(
            insert("Words\n", 5, bare_link("x")).unwrap(),
            "Words[[x]]\n"
        );
        assert_eq!(insert("Words", 5, bare_link("x")).unwrap(), "Words[[x]]");
    }

    #[test]
    fn insert_link_escapes_its_path() {
        assert_eq!(
            insert("a \n", 2, bare_link("file:a[b].org")).unwrap(),
            "a [[file:a\\[b\\].org]]\n"
        );
    }

    #[test]
    fn insert_refused_where_links_are_not_allowed() {
        let src = "#+begin_src sh\necho \n#+end_src\n";
        let at = u32::try_from(src.find("echo ").unwrap() + 5).unwrap();
        assert!(matches!(
            insert(src, at, bare_link("x")),
            Err(OperationError::NotAllowedHere {
                kind: OrgSyntaxKind::LINK,
                ..
            })
        ));
        // Inside a link's description.
        let src = "[[a][word]]\n";
        assert!(matches!(
            insert(src, 6, bare_link("x")),
            Err(OperationError::NotAllowedHere {
                kind: OrgSyntaxKind::LINK,
                scope: OrgSyntaxKind::LINK_DESCRIPTION
            })
        ));
        // Inside verbatim.
        assert!(matches!(
            insert("a =b c= d\n", 4, bare_link("x")),
            Err(OperationError::NotAllowedHere { .. })
        ));
    }

    #[test]
    fn insert_on_a_blank_line_starts_a_paragraph() {
        assert_eq!(insert("", 0, bare_link("x")).unwrap(), "[[x]]");
        assert_eq!(
            insert("a\n\n\n\nb\n", 3, bare_link("x")).unwrap(),
            "a\n\n[[x]]\n\nb\n"
        );
        // Between two paragraphs with one blank line, the text would join
        // them into one: refused.
        assert!(matches!(
            insert("a\n\nb\n", 2, bare_link("x")),
            Err(OperationError::NotAllowedHere { .. })
        ));
    }

    #[test]
    fn insert_beside_a_link_at_its_end() {
        assert_eq!(
            insert("[[a]] b\n", 5, bare_link("x")).unwrap(),
            "[[a]][[x]] b\n"
        );
    }

    #[test]
    fn insert_radio_target_and_target() {
        assert_eq!(
            insert(
                "The  here.\n",
                4,
                ObjectInput::RadioTarget {
                    text: "island".into()
                }
            )
            .unwrap(),
            "The <<<island>>> here.\n"
        );
        assert_eq!(
            insert("The  here.\n", 4, ObjectInput::Target { name: "t".into() }).unwrap(),
            "The <<t>> here.\n"
        );
        assert!(matches!(
            insert("a\n", 0, ObjectInput::RadioTarget { text: " x".into() }),
            Err(OperationError::InvalidText)
        ));
    }

    #[test]
    fn wrap_words_in_a_link() {
        assert_eq!(
            wrap("See the tiles now.\n", "the tiles", bare_link("id:t")).unwrap(),
            "See [[id:t][the tiles]] now.\n"
        );
        assert_eq!(
            wrap("* A *bold* word\n", "*bold* word", bare_link("x")).unwrap(),
            "* A [[x][*bold* word]]\n"
        );
    }

    #[test]
    fn wrap_inside_bold_is_allowed() {
        assert_eq!(
            wrap("*one two*\n", "two", bare_link("x")).unwrap(),
            "*one [[x][two]]*\n"
        );
    }

    #[test]
    fn wrap_refuses_crossing_an_object() {
        assert_eq!(
            wrap("a *b c* d\n", "c* d", bare_link("x")),
            Err(OperationError::CrossesObject)
        );
        assert_eq!(
            wrap("see [[a][b]] c\n", "b]] c", bare_link("x")),
            Err(OperationError::CrossesObject)
        );
    }

    #[test]
    fn wrap_refuses_a_link_inside() {
        assert_eq!(
            wrap("see [[a]] c\n", "see [[a]]", bare_link("x")),
            Err(OperationError::NotAllowedHere {
                kind: OrgSyntaxKind::LINK,
                scope: OrgSyntaxKind::LINK_DESCRIPTION
            })
        );
    }

    #[test]
    fn wrap_refuses_lines_and_brackets() {
        assert_eq!(
            wrap("one\ntwo\n", "one\ntwo", bare_link("x")),
            Err(OperationError::InvalidLink)
        );
        assert_eq!(
            wrap("a] b\n", "a]", bare_link("x")),
            Err(OperationError::InvalidLink)
        );
    }

    #[test]
    fn wrap_radio_target() {
        assert_eq!(
            wrap(
                "The island.\n",
                "island",
                ObjectInput::RadioTarget {
                    text: String::new()
                }
            )
            .unwrap(),
            "The <<<island>>>.\n"
        );
        assert_eq!(
            wrap(
                "The is land.\n",
                "is land.",
                ObjectInput::RadioTarget {
                    text: String::new()
                }
            )
            .unwrap(),
            "The <<<is land.>>>\n"
        );
    }

    #[test]
    fn unwrap_link_keeps_its_words() {
        let doc = parse("See [[id:x][the *tiles*]] now.\n");
        let link = doc.links().next().unwrap();
        let op = Operation::UnwrapObject {
            target: NodeRef::of(&link),
        };
        assert_eq!(compile_write(&doc, &op).1, "See the *tiles* now.\n");

        let doc = parse("See [[Some title]] now.\n");
        let link = doc.links().next().unwrap();
        let op = Operation::UnwrapObject {
            target: NodeRef::of(&link),
        };
        assert_eq!(compile_write(&doc, &op).1, "See Some title now.\n");
    }

    #[test]
    fn unwrap_refuses_an_address_that_links_anyway() {
        let doc = parse("See [[https://example.com]] now.\n");
        let link = doc.links().next().unwrap();
        let op = Operation::UnwrapObject {
            target: NodeRef::of(&link),
        };
        assert_eq!(
            crate::compile(&doc, &op).err(),
            Some(OperationError::InvalidLink)
        );
    }

    #[test]
    fn unwrap_radio_target() {
        let doc = parse("The <<<island>>> here.\n");
        let target = doc.radio_targets().next().unwrap();
        let op = Operation::UnwrapObject {
            target: NodeRef::of(&target),
        };
        assert_eq!(compile_write(&doc, &op).1, "The island here.\n");
    }

    #[test]
    fn insert_respects_config() {
        // A plain link only reads back when its type is registered.
        let cfg = crate::ParseConfig::default();
        let doc = parse_with("a \n", &cfg);
        let op = Operation::InsertObject {
            at: TextSize::from(2),
            object: ObjectInput::Link {
                shape: LinkShape::Plain,
                path: "zzz:abc".into(),
                description: None,
            },
        };
        assert!(crate::compile(&doc, &op).is_err());
    }
}
