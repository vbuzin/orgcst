//! Coverage tests over every kind the parser emits, and the container matrix.
//!
//! The coverage tests check that each emitted node kind has a view or a
//! [`NO_VIEW`] reason, and that the `Element` and `Object` unions match the
//! grammar. The matrix wraps each element family in each container and checks
//! which body children the parser admits.

use super::*;
use crate::ast::support::{
    element_children, element_kinds, is_body_element, is_container_kind, is_object_kind,
    object_kinds,
};
use crate::parse;
use crate::syntax::OrgSyntaxKind;
use crate::tests::{Cfg, TAXONOMY, nodes_of_kind};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Coverage: views, Element, Object
// ---------------------------------------------------------------------------

const ELEMENT_WALK_EXCEPTIONS: &[(OrgSyntaxKind, &str)] = &[(
    OrgSyntaxKind::AFFILIATED_KEYWORDS,
    "run wrapper; skipped by element_children; reached through affiliated()",
)];

const NOT_OBJECT_IN_KIND_BLOCK: &[OrgSyntaxKind] = &[
    OrgSyntaxKind::LINK_PATH,
    OrgSyntaxKind::LINK_DESCRIPTION,
    OrgSyntaxKind::DATE,
    OrgSyntaxKind::TIME,
    OrgSyntaxKind::TIME_RANGE,
    OrgSyntaxKind::ROD,
];

fn no_view_reason_is_sanctioned(kind: OrgSyntaxKind, reason: &str) -> bool {
    if kind == OrgSyntaxKind::DOCUMENT {
        return reason == "owning snapshot root; Document is not an AstNode";
    }
    reason
        .strip_prefix("reached as ")
        .is_some_and(|rest| rest.contains("Part::"))
}

fn restriction_union() -> crate::syntax::ObjectSet {
    use crate::syntax::{OrgLanguage, restrictions};
    use rowan::Language;
    let mut u = crate::syntax::ObjectSet::EMPTY;
    for raw in 0..OrgSyntaxKind::__LAST as u16 {
        let k = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
        u = u.union(restrictions(k));
    }
    u
}

#[test]
fn every_emitted_node_kind_has_a_primary_view_or_a_no_view_reason() {
    let mut roots = Vec::new();
    for_each_corpus(|src, cfg| {
        roots.push(parse_with_cfg(src, cfg).root().clone());
    });
    let missing = missing_view_kinds(roots.iter());
    assert!(
        missing.is_empty(),
        "kinds with neither a primary view nor a NO_VIEW reason: {missing:?}"
    );
}

#[test]
fn no_view_entries_are_honest() {
    let mut seen = BTreeSet::new();
    for_each_corpus(|src, cfg| {
        let root = parse_with_cfg(src, cfg).root().clone();
        for n in root.descendants() {
            seen.insert(n.kind());
        }
    });
    for &(kind, reason) in NO_VIEW {
        assert_ne!(
            kind,
            OrgSyntaxKind::ERROR,
            "ERROR is NEVER_EMITTED, not a NO_VIEW"
        );
        assert!(
            !reason.trim().is_empty(),
            "{kind:?} in NO_VIEW needs a reason"
        );
        assert!(
            kind.is_node(),
            "{kind:?} in NO_VIEW is a token; only node kinds are listed"
        );
        assert!(
            !primary_view_covers(kind),
            "{kind:?} is in NO_VIEW but primary_view_covers is true; remove the entry"
        );
        assert!(
            no_view_reason_is_sanctioned(kind, reason),
            "{kind:?} NO_VIEW reason is not `reached as ...Part::` form: {reason:?}"
        );
        if kind != OrgSyntaxKind::DOCUMENT {
            assert!(
                seen.contains(&kind),
                "{kind:?} in NO_VIEW is not in the corpus (speculative exception)"
            );
        }
    }
}

#[test]
fn every_body_child_of_an_element_container_has_an_element_arm() {
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            if !is_container_kind(node.kind()) {
                continue;
            }
            for child in element_children(&node) {
                let kind = child.kind();
                assert!(
                    Element::can_cast(kind),
                    "{kind:?} observed under {:?} in {src:?} is not an Element kind",
                    node.kind()
                );
                assert!(
                    Element::cast(child.clone()).is_some(),
                    "{kind:?} under {:?} in {src:?}: can_cast but cast failed \
                     (dual-use parent-aware refusal of a body child?)",
                    node.kind()
                );
            }
        }
    });
}

#[test]
fn element_walk_exceptions_are_honest() {
    let observed = observe_pairs();
    for &(kind, reason) in ELEMENT_WALK_EXCEPTIONS {
        assert!(
            !reason.trim().is_empty(),
            "{kind:?} walk exception needs a reason"
        );
        assert!(
            observed.green.values().any(|set| set.contains(&kind)),
            "{kind:?} walk exception never seen as a green child of a container"
        );
        for key in ContainerKey::ALL {
            assert!(
                !observed.body[&key].contains(&kind),
                "{kind:?} was yielded by element_children under {key:?}"
            );
        }
        assert!(
            !Element::can_cast(kind),
            "{kind:?} walk exception must not be an Element kind"
        );
    }
}

#[test]
fn every_element_kind_is_observed_under_at_least_one_container() {
    let observed = observe_pairs();
    for &kind in element_kinds() {
        let seen = ContainerKey::ALL
            .iter()
            .any(|key| observed.body[key].contains(&kind));
        assert!(
            seen,
            "{kind:?} is an element kind but never a body child of any container"
        );
    }
}

#[test]
fn container_green_children_are_body_affiliated_or_anatomy() {
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            if !is_container_kind(node.kind()) {
                continue;
            }
            let Some(key) = container_key(&node) else {
                continue;
            };
            for child in node.children() {
                let kind = child.kind();
                if is_body_element(&child) {
                    continue;
                }
                if kind == OrgSyntaxKind::AFFILIATED_KEYWORDS {
                    continue;
                }
                assert!(
                    anatomy_allows(key, kind),
                    "unexpected green child {kind:?} of {key:?} in {src:?}"
                );
            }
        }
    });
}

#[test]
fn every_restriction_member_has_an_object_arm() {
    use crate::syntax::OrgLanguage;
    use rowan::Language;
    let union = restriction_union();
    let base = OrgSyntaxKind::BOLD as u16;
    let last = OrgSyntaxKind::LINE_BREAK as u16;
    for raw in base..=last {
        let kind = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
        if union.contains(kind) {
            assert!(
                Object::can_cast(kind),
                "{kind:?} is admitted by a restriction set but has no Object arm"
            );
        } else {
            assert!(
                !Object::can_cast(kind),
                "{kind:?} is in BOLD..=LINE_BREAK but not in any restriction set; \
                 it must not be an Object arm (path/desc/date/time/rod anatomy)"
            );
        }
    }
    let mut first = BTreeMap::new();
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            first.entry(node.kind()).or_insert_with(|| node.clone());
        }
    });
    for &kind in object_kinds() {
        assert!(
            union.contains(kind),
            "{kind:?} in object_kinds but not in restriction union"
        );
        let node = first
            .get(&kind)
            .unwrap_or_else(|| panic!("corpus gap: no {kind:?} node to Object::cast"));
        assert!(
            Object::cast(node.clone()).is_some(),
            "{kind:?} restriction member failed Object::cast"
        );
    }
}

#[test]
fn is_object_kind_equals_restriction_union() {
    use crate::syntax::OrgLanguage;
    use rowan::Language;
    let union = restriction_union();
    for raw in 0..OrgSyntaxKind::__LAST as u16 {
        let kind = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
        if !kind.is_node() {
            continue;
        }
        assert_eq!(
            is_object_kind(kind),
            union.contains(kind),
            "{kind:?}: is_object_kind drifted from restriction union"
        );
    }
}

#[test]
fn not_object_in_kind_block_is_a_total_partition() {
    use crate::syntax::OrgLanguage;
    use rowan::Language;
    let union = restriction_union();
    let base = OrgSyntaxKind::BOLD as u16;
    let last = OrgSyntaxKind::LINE_BREAK as u16;
    let mut listed = BTreeSet::new();
    for &kind in NOT_OBJECT_IN_KIND_BLOCK {
        let raw = kind as u16;
        assert!(
            raw >= base && raw <= last,
            "{kind:?} twin kind is not in BOLD..=LINE_BREAK"
        );
        assert!(
            !union.contains(kind),
            "{kind:?} is listed as non-object but is in the restriction union"
        );
        assert!(
            !Object::can_cast(kind),
            "{kind:?} anatomy kind must not be an Object arm"
        );
        listed.insert(kind);
    }
    for raw in base..=last {
        let kind = OrgLanguage::kind_from_raw(rowan::SyntaxKind(raw));
        let in_union = union.contains(kind);
        let in_list = listed.contains(&kind);
        assert!(
            in_union ^ in_list,
            "{kind:?} must be exactly one of restriction-union or NOT_OBJECT_IN_KIND_BLOCK"
        );
    }
}

// ---------------------------------------------------------------------------
// Container matrix corpus
// ---------------------------------------------------------------------------

/// Element-container identity, with `SECTION` split by parent.
///
/// Ten keys match `container_kinds()` with zeroth vs outline `SECTION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ContainerKey {
    ZerothSection,
    OutlineSection,
    Drawer,
    CenterBlock,
    QuoteBlock,
    SpecialBlock,
    DynamicBlock,
    Item,
    FootnoteDefinition,
    InlineTask,
}

impl ContainerKey {
    const ALL: [Self; 10] = [
        Self::ZerothSection,
        Self::OutlineSection,
        Self::Drawer,
        Self::CenterBlock,
        Self::QuoteBlock,
        Self::SpecialBlock,
        Self::DynamicBlock,
        Self::Item,
        Self::FootnoteDefinition,
        Self::InlineTask,
    ];
}

fn container_key(node: &SyntaxNode) -> Option<ContainerKey> {
    match node.kind() {
        OrgSyntaxKind::SECTION => {
            let parent = node.parent().expect("SECTION always has a parent");
            match parent.kind() {
                OrgSyntaxKind::DOCUMENT => Some(ContainerKey::ZerothSection),
                OrgSyntaxKind::OUTLINE => Some(ContainerKey::OutlineSection),
                other => {
                    panic!("SECTION parent is {other:?}; grammar only emits DOCUMENT or OUTLINE")
                }
            }
        }
        OrgSyntaxKind::DRAWER => Some(ContainerKey::Drawer),
        OrgSyntaxKind::CENTER_BLOCK => Some(ContainerKey::CenterBlock),
        OrgSyntaxKind::QUOTE_BLOCK => Some(ContainerKey::QuoteBlock),
        OrgSyntaxKind::SPECIAL_BLOCK => Some(ContainerKey::SpecialBlock),
        OrgSyntaxKind::DYNAMIC_BLOCK => Some(ContainerKey::DynamicBlock),
        OrgSyntaxKind::ITEM => Some(ContainerKey::Item),
        OrgSyntaxKind::FOOTNOTE_DEFINITION => Some(ContainerKey::FootnoteDefinition),
        OrgSyntaxKind::INLINE_TASK => Some(ContainerKey::InlineTask),
        other if is_container_kind(other) => {
            panic!("container kind {other:?} has no ContainerKey")
        }
        _ => None,
    }
}

/// Pairwise refusals. `PROPERTY_DRAWER` is a slot (every key except
/// [`ContainerKey::ZerothSection`]), not nine rows; see [`unadmitted`].
///
/// `Item x INLINE_TASK` is omitted: `scan_list_struct` skips high-star tasks
/// into the item extent (`list.rs`); column-0 `[fn:]` and diary still terminate.
const UNADMITTED: &[(ContainerKey, OrgSyntaxKind, &str)] = &[
    (
        ContainerKey::Item,
        OrgSyntaxKind::FOOTNOTE_DEFINITION,
        "definitions are unindented (footnote.rs: indent_cols() == 0); item bodies are indented",
    ),
    (
        ContainerKey::Item,
        OrgSyntaxKind::DIARY_SEXP,
        "diary element is a column-0 line take (diary.rs)",
    ),
    (
        ContainerKey::FootnoteDefinition,
        OrgSyntaxKind::FOOTNOTE_DEFINITION,
        "measure_body_end stops at the next [fn:] opener (footnote.rs)",
    ),
    (
        ContainerKey::FootnoteDefinition,
        OrgSyntaxKind::INLINE_TASK,
        "measure_body_end stops on every star_head (footnote.rs)",
    ),
    (
        ContainerKey::InlineTask,
        OrgSyntaxKind::INLINE_TASK,
        "find_task_end is first-star-head-must-be-END (heading.rs); inner opener degenerates the outer",
    ),
    (
        ContainerKey::Drawer,
        OrgSyntaxKind::DRAWER,
        "first :END: wins (drawer.rs); inner opener is paragraph text",
    ),
    (
        ContainerKey::QuoteBlock,
        OrgSyntaxKind::QUOTE_BLOCK,
        "first matching #+end_quote (frame.rs::keyed_end_after)",
    ),
    (
        ContainerKey::CenterBlock,
        OrgSyntaxKind::CENTER_BLOCK,
        "first matching #+end_center (frame.rs::keyed_end_after)",
    ),
    (
        ContainerKey::DynamicBlock,
        OrgSyntaxKind::DYNAMIC_BLOCK,
        "first #+end: (frame.rs::dynamic_next, not name-keyed)",
    ),
];

fn property_drawer_cite(key: ContainerKey) -> &'static str {
    match key {
        ContainerKey::InlineTask => {
            "inline-task drawer is metadata (heading::parse_metadata); refused by is_body_element"
        }
        ContainerKey::OutlineSection => {
            "file-level slot is parse_zeroth only; outline drawer is metadata (heading::parse_metadata)"
        }
        _ => "file-level slot is parse_zeroth only",
    }
}

fn unadmitted_reason(key: ContainerKey, kind: OrgSyntaxKind) -> Option<&'static str> {
    if kind == OrgSyntaxKind::PROPERTY_DRAWER && key != ContainerKey::ZerothSection {
        return Some(property_drawer_cite(key));
    }
    UNADMITTED
        .iter()
        .find_map(|&(k, kn, reason)| (k == key && kn == kind).then_some(reason))
}

fn unadmitted(key: ContainerKey, kind: OrgSyntaxKind) -> bool {
    unadmitted_reason(key, kind).is_some()
}

fn unadmitted_entries() -> Vec<(ContainerKey, OrgSyntaxKind, &'static str)> {
    let mut rows: Vec<_> = ContainerKey::ALL
        .into_iter()
        .filter(|&key| key != ContainerKey::ZerothSection)
        .map(|key| {
            (
                key,
                OrgSyntaxKind::PROPERTY_DRAWER,
                property_drawer_cite(key),
            )
        })
        .collect();
    rows.extend(UNADMITTED.iter().copied());
    rows
}

/// Extra green node children of a container that are not body elements and
/// not `AFFILIATED_KEYWORDS`. Empty extra means the key has none.
const CONTAINER_ANATOMY: &[(ContainerKey, OrgSyntaxKind)] = &[
    (ContainerKey::Drawer, OrgSyntaxKind::DRAWER_NAME),
    (ContainerKey::Item, OrgSyntaxKind::BULLET),
    (ContainerKey::Item, OrgSyntaxKind::COUNTER),
    (ContainerKey::Item, OrgSyntaxKind::CHECKBOX),
    (ContainerKey::Item, OrgSyntaxKind::ITEM_TAG),
    (ContainerKey::InlineTask, OrgSyntaxKind::HEADING_TITLE),
    (ContainerKey::InlineTask, OrgSyntaxKind::TODO_KEYWORD),
    (ContainerKey::InlineTask, OrgSyntaxKind::PRIORITY),
    (ContainerKey::InlineTask, OrgSyntaxKind::HEADING_COMMENT),
    (ContainerKey::InlineTask, OrgSyntaxKind::TAGS),
    (ContainerKey::InlineTask, OrgSyntaxKind::PLANNING),
    (ContainerKey::InlineTask, OrgSyntaxKind::PROPERTY_DRAWER),
];

fn anatomy_allows(key: ContainerKey, kind: OrgSyntaxKind) -> bool {
    CONTAINER_ANATOMY
        .iter()
        .any(|&(k, kn)| k == key && kn == kind)
}

/// Hand-written Org for one element family, affiliated paragraph first,
/// swallowers last. The pair oracle still observes parsed kinds.
///
/// `PROPERTY_DRAWER` is wrap-only on the zeroth section (the file-level slot).
const FAMILY_SNIPPETS: &[(OrgSyntaxKind, &str)] = &[
    (OrgSyntaxKind::PARAGRAPH, "#+CAPTION: c\npara\n"),
    (OrgSyntaxKind::PLAIN_LIST, "- list\n"),
    (OrgSyntaxKind::TABLE, "| t |\n"),
    (OrgSyntaxKind::DRAWER, ":D:\nd\n:END:\n"),
    (
        OrgSyntaxKind::CENTER_BLOCK,
        "#+begin_center\ncenter\n#+end_center\n",
    ),
    (
        OrgSyntaxKind::QUOTE_BLOCK,
        "#+begin_quote\nquote\n#+end_quote\n",
    ),
    (
        OrgSyntaxKind::SPECIAL_BLOCK,
        "#+begin_foo\nfoo\n#+end_foo\n",
    ),
    (
        OrgSyntaxKind::DYNAMIC_BLOCK,
        "#+BEGIN: clocktable\n#+END:\n",
    ),
    (
        OrgSyntaxKind::COMMENT_BLOCK,
        "#+begin_comment\nhidden\n#+end_comment\n",
    ),
    (
        OrgSyntaxKind::EXAMPLE_BLOCK,
        "#+begin_example\nex\n#+end_example\n",
    ),
    (
        OrgSyntaxKind::EXPORT_BLOCK,
        "#+begin_export html\n<p/>\n#+end_export\n",
    ),
    (
        OrgSyntaxKind::SRC_BLOCK,
        "#+begin_src elisp\n(+ 1 2)\n#+end_src\n",
    ),
    (
        OrgSyntaxKind::VERSE_BLOCK,
        "#+begin_verse\nverse\n#+end_verse\n",
    ),
    (OrgSyntaxKind::KEYWORD, "#+TITLE: t\n"),
    (OrgSyntaxKind::SETTING_KEYWORD, "#+PRIORITIES: A B C\n"),
    (OrgSyntaxKind::COMMENT, "# body-comment\n"),
    (OrgSyntaxKind::FIXED_WIDTH, ": fixed\n"),
    (OrgSyntaxKind::HORIZONTAL_RULE, "-----\n"),
    (OrgSyntaxKind::CLOCK, "CLOCK: [2020-01-01 Wed 10:00]\n"),
    (OrgSyntaxKind::BABEL_CALL, "#+CALL: name()\n"),
    (
        OrgSyntaxKind::LATEX_ENVIRONMENT,
        "\\begin{equation}\nx\n\\end{equation}\n",
    ),
    (OrgSyntaxKind::DIARY_SEXP, "%%(diary-float t 4 2)\n"),
    (OrgSyntaxKind::FOOTNOTE_DEFINITION, "[fn:1] fn body\n"),
    (
        OrgSyntaxKind::INLINE_TASK,
        "*************** TASK\ntask\n*************** END\n",
    ),
];

const ADVERSARIAL: &[(&str, Cfg)] = &[
    (
        "#+begin_quote\n:NOTES:\ninside\n:END:\n#+end_quote\n",
        Cfg::Default,
    ),
    (":D:\n#+begin_quote\nq\n#+end_quote\n:END:\n", Cfg::Default),
    (":NOTES:\n[fn:1] body\n:END:\n", Cfg::Default),
    (
        "- item\n*************** TASK\nbody\n*************** END\n",
        Cfg::Default,
    ),
];

fn indent_lines(src: &str) -> String {
    src.split_inclusive('\n')
        .map(|line| {
            if line.is_empty() || line == "\n" {
                line.to_string()
            } else {
                format!("  {line}")
            }
        })
        .collect()
}

fn wrap_sink(key: ContainerKey, body: &str) -> String {
    match key {
        ContainerKey::ZerothSection => {
            format!("# comment\n:PROPERTIES:\n:ID: z\n:END:\n{body}")
        }
        ContainerKey::OutlineSection => {
            format!("* Heading\n:PROPERTIES:\n:ID: o\n:END:\n{body}")
        }
        ContainerKey::Drawer => format!(":SINK:\n{body}:END:\n"),
        ContainerKey::CenterBlock => format!("#+begin_center\n{body}#+end_center\n"),
        ContainerKey::QuoteBlock => format!("#+begin_quote\n{body}#+end_quote\n"),
        ContainerKey::SpecialBlock => format!("#+begin_foo\n{body}#+end_foo\n"),
        ContainerKey::DynamicBlock => format!("#+BEGIN: clocktable\n{body}#+END:\n"),
        ContainerKey::Item => format!("- [@1] [ ] tag ::\n{body}"),
        ContainerKey::FootnoteDefinition => format!("[fn:1]\n{body}"),
        ContainerKey::InlineTask => format!(
            "*************** TODO [#A] COMMENT Title :tag:\n\
             SCHEDULED: <2020-01-01 Wed>\n\
             :PROPERTIES:\n:ID: t\n:END:\n\
             {body}*************** END\n"
        ),
    }
}

fn kitchen_sink(key: ContainerKey) -> String {
    let mut body = String::new();
    for &(kind, snippet) in FAMILY_SNIPPETS {
        if unadmitted(key, kind) {
            continue;
        }
        let snippet = match (key, kind) {
            (ContainerKey::SpecialBlock, OrgSyntaxKind::SPECIAL_BLOCK) => {
                "#+begin_bar\nbar\n#+end_bar\n"
            }
            _ => snippet,
        };
        if key == ContainerKey::Item && kind != OrgSyntaxKind::INLINE_TASK {
            body.push_str(&indent_lines(snippet));
        } else {
            body.push_str(snippet);
        }
    }
    wrap_sink(key, &body)
}

fn for_each_matrix(mut visit: impl FnMut(&str, Cfg)) {
    for key in ContainerKey::ALL {
        let src = kitchen_sink(key);
        visit(&src, Cfg::Default);
    }
    for &(src, cfg) in ADVERSARIAL {
        visit(src, cfg);
    }
}

pub(super) fn for_each_corpus(mut visit: impl FnMut(&str, Cfg)) {
    for &(src, cfg) in TAXONOMY {
        visit(src, cfg);
    }
    for_each_matrix(&mut visit);
}

pub(super) fn parse_with_cfg(src: &str, cfg: Cfg) -> crate::Document {
    crate::parse_with(src, &cfg.build())
}

struct Observed {
    body: BTreeMap<ContainerKey, BTreeSet<OrgSyntaxKind>>,
    affiliated: BTreeSet<ContainerKey>,
    green: BTreeMap<ContainerKey, BTreeSet<OrgSyntaxKind>>,
}

fn observe_pairs() -> Observed {
    let mut body: BTreeMap<ContainerKey, BTreeSet<OrgSyntaxKind>> = BTreeMap::new();
    let mut affiliated = BTreeSet::new();
    let mut green: BTreeMap<ContainerKey, BTreeSet<OrgSyntaxKind>> = BTreeMap::new();
    for key in ContainerKey::ALL {
        body.insert(key, BTreeSet::new());
        green.insert(key, BTreeSet::new());
    }
    for_each_corpus(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        for node in doc.root().descendants() {
            if !is_container_kind(node.kind()) {
                continue;
            }
            let Some(key) = container_key(&node) else {
                continue;
            };
            let body_set = body.get_mut(&key).expect("ALL keys pre-inserted");
            for child in element_children(&node) {
                body_set.insert(child.kind());
            }
            let green_set = green.get_mut(&key).expect("ALL keys pre-inserted");
            for child in node.children() {
                green_set.insert(child.kind());
                if child.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS {
                    affiliated.insert(key);
                }
            }
        }
    });
    Observed {
        body,
        affiliated,
        green,
    }
}

#[test]
fn container_matrix_parses_and_walks() {
    for_each_matrix(|src, cfg| {
        let doc = parse_with_cfg(src, cfg);
        assert_eq!(doc.root().text().to_string(), doc.source(), "src={src:?}");
    });
}

#[test]
fn family_snippets_cover_element_kinds() {
    let kinds: BTreeSet<_> = FAMILY_SNIPPETS.iter().map(|&(k, _)| k).collect();
    assert_eq!(
        kinds.len(),
        FAMILY_SNIPPETS.len(),
        "duplicate family snippet"
    );
    for &kind in element_kinds() {
        if kind == OrgSyntaxKind::PROPERTY_DRAWER {
            continue;
        }
        assert!(
            kinds.contains(&kind),
            "FAMILY_SNIPPETS missing {kind:?} (PROPERTY_DRAWER is wrap-only on zeroth)"
        );
    }
}

#[test]
fn container_matrix_observes_every_admitted_pair() {
    let observed = observe_pairs();
    let mut missing = Vec::new();
    for key in ContainerKey::ALL {
        let have = &observed.body[&key];
        for &kind in element_kinds() {
            if unadmitted(key, kind) {
                assert!(
                    !have.contains(&kind),
                    "{key:?} unexpectedly contains unadmitted {kind:?}"
                );
            } else if !have.contains(&kind) {
                missing.push((key, kind));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "missing admitted body children (corpus gap: add to the kitchen-sink, \
         or document UNADMITTED with a probe cite): {missing:?}"
    );
}

#[test]
fn unadmitted_pairs_are_not_observed() {
    let observed = observe_pairs();
    for (key, kind, reason) in unadmitted_entries() {
        assert!(
            !reason.trim().is_empty(),
            "{key:?} x {kind:?} UNADMITTED needs a grammar cite"
        );
        assert!(
            !observed.body[&key].contains(&kind),
            "{key:?} x {kind:?} is listed UNADMITTED ({reason}) but the corpus emits it"
        );
    }
}

#[test]
fn container_anatomy_kinds_are_observed_and_not_body_children() {
    let observed = observe_pairs();
    for &(key, kind) in CONTAINER_ANATOMY {
        assert!(
            observed.green[&key].contains(&kind),
            "{key:?} anatomy {kind:?} never seen as a green child (corpus gap)"
        );
        assert!(
            !observed.body[&key].contains(&kind),
            "{key:?} anatomy {kind:?} was yielded by element_children"
        );
    }
    assert!(
        !observed.green[&ContainerKey::FootnoteDefinition]
            .iter()
            .any(|&k| anatomy_allows(ContainerKey::FootnoteDefinition, k)),
        "FootnoteDefinition extra anatomy must stay empty of nodes"
    );
}

#[test]
fn affiliated_keywords_are_green_children_of_several_keys() {
    let observed = observe_pairs();
    assert!(
        observed.affiliated.contains(&ContainerKey::ZerothSection),
        "zeroth kitchen-sink must carry an affiliated run"
    );
    let others = observed
        .affiliated
        .iter()
        .filter(|k| **k != ContainerKey::ZerothSection)
        .count();
    assert!(
        others >= 4,
        "AFFILIATED_KEYWORDS must be a green child of more than zeroth section; \
         seen {seen:?}",
        seen = observed.affiliated
    );
}

#[test]
fn inline_task_inside_an_item_is_a_body_child() {
    // List-struct skip includes high-star tasks in the item extent, so the
    // task parses as a body child. Column-0 [fn:] / diary still terminate
    // the list; this pin is the task case only.
    let src = "- item\n*************** TASK\nbody\n*************** END\n";
    let doc = parse(src);
    let item = nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
        .into_iter()
        .next()
        .expect("item");
    let kids: Vec<_> = element_children(&item).map(|n| n.kind()).collect();
    assert!(
        kids.contains(&OrgSyntaxKind::INLINE_TASK),
        "high-star task is a body child of the item (list-struct skip); got {kids:?}"
    );
    let parent = nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
        .into_iter()
        .next()
        .expect("task")
        .parent()
        .map(|p| p.kind());
    assert_eq!(parent, Some(OrgSyntaxKind::ITEM));
}

#[test]
fn footnote_definition_inside_a_drawer_is_a_body_child() {
    let src = ":NOTES:\n[fn:1] body\n:END:\n";
    let doc = parse(src);
    let drawer = nodes_of_kind(doc.root(), OrgSyntaxKind::DRAWER)
        .into_iter()
        .next()
        .expect("drawer");
    let kids: Vec<_> = element_children(&drawer).map(|n| n.kind()).collect();
    assert!(
        kids.contains(&OrgSyntaxKind::FOOTNOTE_DEFINITION),
        "footnote definition should be a body child of the drawer; got {kids:?}"
    );
}
