//! Checkbox, keyword-value, and link field ops.

use super::empty_edit_set;
use super::field::{optional_part_edit, replace_content_or_fill};
use super::locate::resolve_view;
use super::map_synthesis;
use crate::ast::{
    AstNode, CheckboxState, HasParts, Item, ItemPart, Keyword, KeywordPart, Link, LinkPart,
    LinkShape, PartSpan,
};
use crate::edit::EditSet;
use crate::parser::grammar::forms::link::is_whole_plain_path;
use crate::writer::{
    invalid_link_description, render_checkbox, render_link, render_link_description_insert,
};
use crate::{
    Document, NodeRef, OperationError, OrgSyntaxKind, TextRange, escape_link_path,
    unescape_link_path,
};

const ITEM_KINDS: &[OrgSyntaxKind] = &[OrgSyntaxKind::ITEM];
const KEYWORD_KINDS: &[OrgSyntaxKind] = &[OrgSyntaxKind::KEYWORD, OrgSyntaxKind::SETTING_KEYWORD];
const LINK_KINDS: &[OrgSyntaxKind] = &[OrgSyntaxKind::LINK];

pub(super) fn compile_toggle_checkbox(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let item = resolve_view::<Item>(doc, target, ITEM_KINDS)?;
    let span = item.part_span(ItemPart::Checkbox);
    match span {
        PartSpan::Present(part) => {
            let next = match item
                .checkbox()
                .expect("grammar: present CHECKBOX has a state")
            {
                CheckboxState::Checked => " ",
                CheckboxState::Unchecked | CheckboxState::Partial => "X",
            };
            Ok(EditSet::single(doc.source(), part.content(), next)?)
        }
        PartSpan::Absent(_) => {
            let frag = render_checkbox(CheckboxState::Checked);
            optional_part_edit(doc, span, Some(&frag))?.into_edit_set(doc)
        }
        PartSpan::NotApplicable => {
            unreachable!("grammar: item checkbox is present or absent")
        }
    }
}

pub(super) fn compile_set_keyword(
    doc: &Document,
    target: NodeRef,
    value: &str,
) -> Result<EditSet, OperationError> {
    let keyword = resolve_view::<Keyword>(doc, target, KEYWORD_KINDS)?;
    if value.contains(['\n', '\r']) {
        return Err(OperationError::InvalidKeyword);
    }
    let span = keyword.part_span(KeywordPart::Value);
    let PartSpan::Present(part) = span else {
        unreachable!("grammar: KEYWORD_VALUE is always carved");
    };
    if value.is_empty() {
        if part.content().is_empty() {
            return Ok(empty_edit_set(doc.source()));
        }
        return optional_part_edit(doc, span, None)?.into_edit_set(doc);
    }
    replace_content_or_fill(doc, part, keyword.value_fill_insert_point(), value)
}

pub(super) fn compile_set_link_path(
    doc: &Document,
    target: NodeRef,
    path: &str,
) -> Result<EditSet, OperationError> {
    let link = resolve_view::<Link>(doc, target, LINK_KINDS)?;
    if path.is_empty() || path.contains(['\n', '\r']) {
        return Err(OperationError::InvalidLink);
    }
    let path_range = link.path_node().text_range();
    let current = doc.slice(path_range);
    let replacement = match link.shape() {
        LinkShape::Radio => return Err(OperationError::InvalidLink),
        LinkShape::Bracket => {
            if unescape_link_path(current).as_ref() == path {
                return Ok(empty_edit_set(doc.source()));
            }
            escape_link_path(path).into_owned()
        }
        LinkShape::Angle => {
            if path.contains('>') {
                return Err(OperationError::InvalidLink);
            }
            if current == path {
                return Ok(empty_edit_set(doc.source()));
            }
            path.to_string()
        }
        LinkShape::Plain => {
            let rendered = render_link(LinkShape::Plain, path, None).map_err(map_synthesis)?;
            if !is_whole_plain_path(path, doc.config()) {
                return Err(OperationError::InvalidLink);
            }
            if current == path {
                return Ok(empty_edit_set(doc.source()));
            }
            rendered
        }
    };
    Ok(EditSet::single(doc.source(), path_range, replacement)?)
}

pub(super) fn compile_set_link_description(
    doc: &Document,
    target: NodeRef,
    description: Option<&str>,
) -> Result<EditSet, OperationError> {
    let link = resolve_view::<Link>(doc, target, LINK_KINDS)?;
    let span = link.part_span(LinkPart::Description);
    if matches!(span, PartSpan::NotApplicable) {
        return Err(OperationError::PartNotApplicable);
    }
    let Some(d) = description else {
        return optional_part_edit(doc, span, None)?.into_edit_set(doc);
    };
    match span {
        PartSpan::Present(part) => {
            if invalid_link_description(d) {
                return Err(OperationError::InvalidLink);
            }
            if doc.slice(part.content()) == d {
                return Ok(empty_edit_set(doc.source()));
            }
            Ok(EditSet::single(doc.source(), part.content(), d)?)
        }
        PartSpan::Absent(point) => {
            let frag = render_link_description_insert(d).map_err(map_synthesis)?;
            Ok(EditSet::single(
                doc.source(),
                TextRange::empty(point.offset()),
                frag,
            )?)
        }
        PartSpan::NotApplicable => {
            unreachable!("rejected above")
        }
    }
}

pub(super) fn compile_convert_link_shape(
    doc: &Document,
    target: NodeRef,
    to: LinkShape,
) -> Result<EditSet, OperationError> {
    let link = resolve_view::<Link>(doc, target, LINK_KINDS)?;
    if to == LinkShape::Radio {
        return Err(OperationError::InvalidLink);
    }
    if link.shape() == to {
        return Ok(empty_edit_set(doc.source()));
    }
    let unescaped = unescape_link_path(doc.slice(link.path_node().text_range()));
    let desc = if to == LinkShape::Bracket {
        link.part_content_range(LinkPart::Description)
            .map(|r| doc.slice(r))
    } else {
        None
    };
    let replacement = render_link(to, unescaped.as_ref(), desc).map_err(map_synthesis)?;
    if to == LinkShape::Plain && !is_whole_plain_path(unescaped.as_ref(), doc.config()) {
        return Err(OperationError::InvalidLink);
    }
    Ok(EditSet::single(doc.source(), link.range(), replacement)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, HasParts, Heading, Headline, ItemPart, LinkPart};
    use crate::edit::{Bias, EditSetError};
    use crate::operation::compile_write;
    use crate::writer::wrap_insert;
    use crate::{
        Operation, ParseConfig, TodoKind, apply_locating, compile, compile_all, parse, parse_with,
    };

    fn first_item(doc: &Document) -> Item {
        doc.map::<Item>().next().expect("item")
    }

    fn first_keyword(doc: &Document) -> Keyword {
        doc.map::<Keyword>().next().expect("keyword")
    }

    fn first_link(doc: &Document) -> Link {
        doc.map::<Link>().next().expect("link")
    }

    fn first_heading(doc: &Document) -> Heading {
        doc.map::<Heading>().next().expect("heading")
    }

    fn toggle(target: NodeRef) -> Operation {
        Operation::ToggleCheckbox { target }
    }

    fn set_keyword(target: NodeRef, value: &str) -> Operation {
        Operation::SetKeyword {
            target,
            value: value.to_string(),
        }
    }

    fn set_path(target: NodeRef, path: &str) -> Operation {
        Operation::SetLinkPath {
            target,
            path: path.to_string(),
        }
    }

    fn set_desc(target: NodeRef, description: Option<&str>) -> Operation {
        Operation::SetLinkDescription {
            target,
            description: description.map(str::to_string),
        }
    }

    fn convert(target: NodeRef, to: LinkShape) -> Operation {
        Operation::ConvertLinkShape { target, to }
    }

    fn set_todo(target: NodeRef, state: Option<&str>) -> Operation {
        Operation::SetTodoState {
            target,
            state: state.map(str::to_string),
        }
    }

    fn radio_doc() -> Document {
        // `+` in the registered type carves a plain LINK that the TYPE:
        // prefix test rejects, so the view classifies it as Radio.
        parse_with(
            "file+sys:~/a\n",
            &ParseConfig::default().with_link_types(["file+sys"]),
        )
    }

    #[test]
    fn toggle_checkbox_inserts_checked_on_bare_item() {
        let doc = parse("- item\n");
        let item = first_item(&doc);
        let dest = item
            .part_insert_point(ItemPart::Checkbox)
            .expect("absent checkbox");
        let (set, text) = compile_write(&doc, &toggle(NodeRef::of(&item)));
        assert_eq!(set.len(), 1);
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert_eq!(
            set.edits()[0].replacement,
            wrap_insert(dest, &render_checkbox(CheckboxState::Checked))
        );
        assert_eq!(text, "- [X] item\n");
    }

    #[test]
    fn toggle_checkbox_cycles_present_and_partial() {
        let doc = parse("- [ ] item\n");
        let item = first_item(&doc);
        let (set, text) = compile_write(&doc, &toggle(NodeRef::of(&item)));
        assert_eq!(doc.slice(set.edits()[0].range), " ");
        assert_eq!(set.edits()[0].replacement, "X");
        assert_eq!(text, "- [X] item\n");

        let doc = parse("- [X] item\n");
        let (_, text) = compile_write(&doc, &toggle(NodeRef::of(&first_item(&doc))));
        assert_eq!(text, "- [ ] item\n");

        let doc = parse("- [-] item\n");
        let (_, text) = compile_write(&doc, &toggle(NodeRef::of(&first_item(&doc))));
        assert_eq!(text, "- [X] item\n");
    }

    #[test]
    fn toggle_checkbox_wrong_kind_on_heading() {
        let doc = parse("* H\n");
        let err = compile(&doc, &toggle(NodeRef::of(&first_heading(&doc)))).unwrap_err();
        assert!(matches!(
            err,
            OperationError::WrongKind { actual, .. } if actual == OrgSyntaxKind::HEADING
        ));
    }

    #[test]
    fn set_keyword_replace_clear_newline_and_noop() {
        let doc = parse("#+TITLE: hello\n");
        let kw = first_keyword(&doc);
        let (set, text) = compile_write(&doc, &set_keyword(NodeRef::of(&kw), "world"));
        assert_eq!(doc.slice(set.edits()[0].range), "hello");
        assert_eq!(text, "#+TITLE: world\n");

        let doc = parse("#+TITLE: hello\n");
        let kw = first_keyword(&doc);
        let (set, text) = compile_write(&doc, &set_keyword(NodeRef::of(&kw), ""));
        assert_eq!(doc.slice(set.edits()[0].range), " hello");
        assert!(
            set.edits()[0].replacement.is_empty(),
            "{:?}",
            set.edits()[0].replacement
        );
        assert_eq!(text, "#+TITLE:\n");

        let doc = parse("#+CAPTION[src]: old\nparagraph\n");
        let kw = first_keyword(&doc);
        let (_, text) = compile_write(&doc, &set_keyword(NodeRef::of(&kw), "new"));
        assert_eq!(text, "#+CAPTION[src]: new\nparagraph\n");

        let doc = parse("#+TITLE: hello\n");
        let err = compile(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "a\nb"),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidKeyword);

        let doc = parse("#+TITLE: hello\n");
        let set = compile(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "hello"),
        )
        .expect("no-op");
        assert!(set.is_empty());

        let doc = parse("#+TITLE:\n");
        let set = compile(&doc, &set_keyword(NodeRef::of(&first_keyword(&doc)), ""))
            .expect("empty already empty");
        assert!(set.is_empty());

        let doc = parse("#+CAPTION[src]: old\nparagraph\n");
        let (_, text) = compile_write(&doc, &set_keyword(NodeRef::of(&first_keyword(&doc)), ""));
        assert_eq!(text, "#+CAPTION[src]:\nparagraph\n");

        let doc = parse("#+TITLE:\n");
        let (_, text) = compile_write(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "hello"),
        );
        assert_eq!(text, "#+TITLE: hello\n");

        let doc = parse("#+CAPTION[src]:\nparagraph\n");
        let (_, text) = compile_write(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "hello"),
        );
        assert_eq!(text, "#+CAPTION[src]: hello\nparagraph\n");

        let src = "#+TITLE: \n";
        let doc = parse(src);
        let set = compile(&doc, &set_keyword(NodeRef::of(&first_keyword(&doc)), ""))
            .expect("whitespace-only content is already empty");
        assert!(set.is_empty());
        assert_eq!(set.apply_to_text(doc.source()), src);

        let doc = parse("#+TITLE: \n");
        let (_, text) = compile_write(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "hello"),
        );
        assert_eq!(text, "#+TITLE: hello\n");

        let doc = parse("#+CAPTION[src]: \nparagraph\n");
        let (_, text) = compile_write(
            &doc,
            &set_keyword(NodeRef::of(&first_keyword(&doc)), "hello"),
        );
        assert_eq!(text, "#+CAPTION[src]: hello\nparagraph\n");
    }

    #[test]
    fn set_keyword_setting_kind_and_stale_exact_kind() {
        let doc = parse("#+TODO: WAIT | DONE\n* Title\n");
        let kw = first_keyword(&doc);
        assert_eq!(kw.kind(), OrgSyntaxKind::SETTING_KEYWORD);
        let (set, text) = compile_write(&doc, &set_keyword(NodeRef::of(&kw), "BUG | FIXED"));
        assert!(!set.is_empty());
        assert_eq!(text, "#+TODO: BUG | FIXED\n* Title\n");
        let next = doc.apply(&set);
        assert_eq!(next.config().todo_kind("BUG"), Some(TodoKind::Active));
        assert_eq!(next.config().todo_kind("FIXED"), Some(TodoKind::Done));
        assert_eq!(next.config().todo_kind("WAIT"), None);

        let err = compile(
            &doc,
            &set_keyword(
                NodeRef::at(kw.range().start(), OrgSyntaxKind::KEYWORD),
                "BUG | FIXED",
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);
    }

    #[test]
    fn compile_all_set_keyword_then_new_vocab_todo_is_unknown() {
        let doc = parse("#+TODO: WAIT | DONE\n* Title\n");
        let kw = first_keyword(&doc);
        let heading = first_heading(&doc);
        let err = compile_all(
            &doc,
            &[
                set_keyword(NodeRef::of(&kw), "BUG | FIXED"),
                set_todo(NodeRef::of(&heading), Some("BUG")),
            ],
        )
        .unwrap_err();
        assert_eq!(err, OperationError::UnknownTodoKeyword("BUG".into()));
    }

    #[test]
    fn sequential_set_keyword_then_new_vocab_todo_succeeds() {
        let doc = parse("#+TODO: WAIT | DONE\n* Title\n");
        let kw = first_keyword(&doc);
        let heading = first_heading(&doc);
        let set = compile(&doc, &set_keyword(NodeRef::of(&kw), "BUG | FIXED")).expect("keyword");
        let next = doc.apply(&set);
        assert_eq!(next.config().todo_kind("BUG"), Some(TodoKind::Active));
        let heading = apply_locating(&NodeRef::of(&heading), &set, &next).expect("heading remains");
        let todo_set = compile(&next, &set_todo(heading, Some("BUG"))).expect("todo");
        super::super::assert_compile_apply(&next, &todo_set);
        assert_eq!(
            todo_set.apply_to_text(next.source()),
            "#+TODO: BUG | FIXED\n* BUG Title\n"
        );
        let applied = next.apply(&todo_set);
        let h = first_heading(&applied);
        assert_eq!(h.todo_keyword().unwrap().text().to_string(), "BUG");
        assert_eq!(applied.todo_kind(&h), Some(TodoKind::Active));
        assert_eq!(applied.config().todo_kind("WAIT"), None);
    }

    #[test]
    fn set_link_path_escape_rejects_and_noop() {
        let doc = parse("[[https://x.com]]\n");
        let link = first_link(&doc);
        let (_, text) = compile_write(&doc, &set_path(NodeRef::of(&link), "a[b]"));
        assert_eq!(text, "[[a\\[b\\]]]\n");

        let doc = parse("[[a\\[b\\]]]\n");
        let set = compile(&doc, &set_path(NodeRef::of(&first_link(&doc)), "a[b]")).expect("no-op");
        assert!(set.is_empty());

        let doc = parse("<https://x.com>\n");
        let err = compile(&doc, &set_path(NodeRef::of(&first_link(&doc)), "a>b")).unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let (_, text) = compile_write(
            &doc,
            &set_path(NodeRef::of(&first_link(&doc)), "https://y.com"),
        );
        assert_eq!(text, "<https://y.com>\n");

        let doc = parse("https://x.com\n");
        let err = compile(
            &doc,
            &set_path(NodeRef::of(&first_link(&doc)), "* Injected"),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let (_, text) = compile_write(
            &doc,
            &set_path(NodeRef::of(&first_link(&doc)), "https://y.com"),
        );
        assert_eq!(text, "https://y.com\n");

        let doc = parse("[[https://x.com]]\n");
        let link = first_link(&doc);
        let err = compile(&doc, &set_path(NodeRef::of(&link), "")).unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let err = compile(&doc, &set_path(NodeRef::of(&link), "a\nb")).unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);

        let set = compile(&doc, &set_path(NodeRef::of(&link), "https://x.com")).expect("no-op");
        assert!(set.is_empty());

        let doc = parse("[[https://x.com][site]]\n");
        let (_, text) = compile_write(
            &doc,
            &set_path(NodeRef::of(&first_link(&doc)), "https://y.com"),
        );
        assert_eq!(text, "[[https://y.com][site]]\n");
        let doc = parse("[[https://x.com][site]]\n");
        let (_, text) = compile_write(&doc, &set_path(NodeRef::of(&first_link(&doc)), "a[b]"));
        assert_eq!(text, "[[a\\[b\\]][site]]\n");

        let doc = parse("https://x.com\n");
        let err = compile(
            &doc,
            &set_path(
                NodeRef::of(&first_link(&doc)),
                "https://x.com [[https://evil][click]]",
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let err = compile(
            &doc,
            &set_path(
                NodeRef::of(&first_link(&doc)),
                "https://x.com <https://evil>",
            ),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
    }

    #[test]
    fn set_link_path_radio_is_invalid() {
        let doc = radio_doc();
        let link = first_link(&doc);
        assert_eq!(link.shape(), LinkShape::Radio);
        let err = compile(&doc, &set_path(NodeRef::of(&link), "file+sys:~/b")).unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
    }

    #[test]
    fn set_link_description_insert_clear_and_rejects() {
        let doc = parse("[[path]]\n");
        let link = first_link(&doc);
        let dest = link
            .part_insert_point(LinkPart::Description)
            .expect("absent desc");
        let (set, text) = compile_write(&doc, &set_desc(NodeRef::of(&link), Some("site")));
        assert!(set.edits()[0].range.is_empty());
        assert_eq!(set.edits()[0].range.start(), dest.offset());
        assert_eq!(
            set.edits()[0].replacement,
            render_link_description_insert("site").expect("writable")
        );
        assert_eq!(text, "[[path][site]]\n");

        let doc = parse("[[path][site]]\n");
        let link = first_link(&doc);
        let (set, text) = compile_write(&doc, &set_desc(NodeRef::of(&link), None));
        assert_eq!(doc.slice(set.edits()[0].range), "][site");
        assert!(
            set.edits()[0].replacement.is_empty(),
            "{:?}",
            set.edits()[0].replacement
        );
        assert_eq!(text, "[[path]]\n");

        let doc = parse("[[t][]]\n");
        let (_, text) = compile_write(&doc, &set_desc(NodeRef::of(&first_link(&doc)), None));
        assert_eq!(text, "[[t]]\n");

        let doc = parse("[[path]]\n");
        let set = compile(&doc, &set_desc(NodeRef::of(&first_link(&doc)), None)).expect("absent");
        assert!(set.is_empty());

        let doc = parse("[[path][site]]\n");
        let set = compile(
            &doc,
            &set_desc(NodeRef::of(&first_link(&doc)), Some("site")),
        )
        .expect("same");
        assert!(set.is_empty());

        let doc = parse("[[path][site]]\n");
        let err = compile(
            &doc,
            &set_desc(NodeRef::of(&first_link(&doc)), Some("a]]b")),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let err = compile(
            &doc,
            &set_desc(NodeRef::of(&first_link(&doc)), Some("a\nb")),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        for desc in ["See [1]", "foo]", "]"] {
            let err =
                compile(&doc, &set_desc(NodeRef::of(&first_link(&doc)), Some(desc))).unwrap_err();
            assert_eq!(err, OperationError::InvalidLink, "{desc:?}");
        }

        let doc = parse("[[path][site]]\n");
        let (set, text) = compile_write(
            &doc,
            &set_desc(NodeRef::of(&first_link(&doc)), Some("other")),
        );
        assert_eq!(doc.slice(set.edits()[0].range), "site");
        assert_eq!(text, "[[path][other]]\n");

        let doc = parse("[[path][site]]\n");
        let (_, text) = compile_write(&doc, &set_desc(NodeRef::of(&first_link(&doc)), Some("a]b")));
        assert_eq!(text, "[[path][a]b]]\n");

        let doc = parse("[[t][]]\n");
        let (_, text) = compile_write(
            &doc,
            &set_desc(NodeRef::of(&first_link(&doc)), Some("site")),
        );
        assert_eq!(text, "[[t][site]]\n");

        let doc = parse("[[path]]\n");
        let (_, text) = compile_write(&doc, &set_desc(NodeRef::of(&first_link(&doc)), Some("")));
        assert_eq!(text, "[[path][]]\n");
    }

    #[test]
    fn set_link_description_not_applicable_on_angle_plain_radio() {
        let radio = radio_doc();
        for (doc, why) in [
            (parse("<https://x.com>\n"), "angle"),
            (parse("https://x.com\n"), "plain"),
            (radio, "radio"),
        ] {
            let link = first_link(&doc);
            let err = compile(&doc, &set_desc(NodeRef::of(&link), Some("site"))).unwrap_err();
            assert_eq!(err, OperationError::PartNotApplicable, "{why}");
            let err = compile(&doc, &set_desc(NodeRef::of(&link), None)).unwrap_err();
            assert_eq!(err, OperationError::PartNotApplicable, "{why}");
        }
    }

    #[test]
    fn convert_link_shape_rebuilds_and_drops_description() {
        let doc = parse("[[https://x.com][site]]\n");
        let link = first_link(&doc);
        let (set, text) = compile_write(&doc, &convert(NodeRef::of(&link), LinkShape::Angle));
        assert_eq!(set.edits()[0].range, link.range());
        assert_eq!(text, "<https://x.com>\n");

        let doc = parse("[[https://x.com][site]]\n");
        let (_, text) = compile_write(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Plain),
        );
        assert_eq!(text, "https://x.com\n");

        let doc = parse("<https://x.com>\n");
        let set = compile(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Angle),
        )
        .expect("same shape");
        assert!(set.is_empty());

        let doc = parse("[[https://x.com]]\n");
        let err = compile(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Radio),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);

        let doc = radio_doc();
        let link = first_link(&doc);
        assert_eq!(link.shape(), LinkShape::Radio);
        let (_, text) = compile_write(&doc, &convert(NodeRef::of(&link), LinkShape::Bracket));
        assert_eq!(text, "[[file+sys:~/a]]\n");

        let doc = parse("[[* Injected]]\n");
        let err = compile(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Plain),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::InvalidLink);
        let (_, text) = compile_write(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Angle),
        );
        assert_eq!(text, "<* Injected>\n");

        let doc = parse("<https://x.com>\n");
        let (_, text) = compile_write(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Bracket),
        );
        assert_eq!(text, "[[https://x.com]]\n");
        let doc = parse("https://x.com\n");
        let (_, text) = compile_write(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Bracket),
        );
        assert_eq!(text, "[[https://x.com]]\n");
        let doc = parse("<https://x.com>\n");
        let (_, text) = compile_write(
            &doc,
            &convert(NodeRef::of(&first_link(&doc)), LinkShape::Plain),
        );
        assert_eq!(text, "https://x.com\n");
    }

    /// A plain link is written only when the path reads back as the whole
    /// link: Org leaves trailing punctuation such as `*` or `.` outside it,
    /// and a path without a registered type is no plain link at all.
    #[test]
    fn plain_link_writes_refuse_paths_that_would_not_read_back() {
        let doc = parse("https://x.com/a\n");
        let target = NodeRef::of(&first_link(&doc));
        for path in ["https://x.com/a*", "https://x.com/a.", "https:a", "nope/x"] {
            assert_eq!(
                compile(&doc, &set_path(target, path)).unwrap_err(),
                OperationError::InvalidLink,
                "{path:?}"
            );
        }
        for path in ["https://x.com/a-", "https://x.com/b/", "https://x.com/(c)"] {
            let (_, text) = compile_write(&doc, &set_path(target, path));
            assert_eq!(text, format!("{path}\n"));
            let reparsed = parse(&text);
            let link = reparsed.links().next().expect("plain link");
            assert_eq!(link.syntax().text().to_string(), path);
        }

        for src in ["[[https://x.com/a.]]\n", "[[nope]]\n"] {
            let doc = parse(src);
            let err = compile(
                &doc,
                &convert(NodeRef::of(&first_link(&doc)), LinkShape::Plain),
            )
            .unwrap_err();
            assert_eq!(err, OperationError::InvalidLink, "{src:?}");
        }
    }

    #[test]
    fn convert_link_shape_locating_right_is_replacement_end() {
        let doc = parse("[[https://x.com][site]] https://y.com\n");
        let links: Vec<Link> = doc.map::<Link>().collect();
        assert_eq!(links.len(), 2);
        let link = &links[0];
        let old = NodeRef::of(link);
        assert_eq!(old.anchor.bias, Bias::Right);
        let op = convert(old, LinkShape::Angle);
        let set = compile(&doc, &op).expect("convert");
        super::super::assert_compile_apply(&doc, &set);
        super::super::assert_envelope(&doc, &op, &set);
        let next = doc.apply(&set);
        assert_eq!(next.write(), "<https://x.com> https://y.com\n");

        let start = set.map_offset(link.range().start(), Bias::Left);
        let end = set.map_offset(link.range().start(), Bias::Right);
        assert_eq!(end, start + crate::TextSize::of("<https://x.com>"));
        assert!(
            apply_locating(&old, &set, &next).is_none(),
            "Right at exclusive end must not resolve, including to the following LINK"
        );
        let rewritten = NodeRef::at(start, OrgSyntaxKind::LINK)
            .resolve(&next)
            .expect("Left at construct start finds the rewritten link");
        assert_eq!(rewritten.text().to_string(), "<https://x.com>");
    }

    #[test]
    fn object_stale_target() {
        let doc = parse("- item\n");
        let err = compile(
            &doc,
            &toggle(NodeRef::at(crate::TextSize::from(99), OrgSyntaxKind::ITEM)),
        )
        .unwrap_err();
        assert_eq!(err, OperationError::StaleTarget);
    }

    #[test]
    fn compile_all_checkbox_and_link_batches() {
        let doc = parse("- item\n");
        let item = first_item(&doc);
        let dest = item
            .part_insert_point(ItemPart::Checkbox)
            .expect("absent checkbox");
        let err = compile_all(
            &doc,
            &[toggle(NodeRef::of(&item)), toggle(NodeRef::of(&item))],
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: dest.offset()
            }
        );

        let doc = parse("- a\n- b\n");
        let items: Vec<Item> = doc.map::<Item>().collect();
        assert_eq!(items.len(), 2);
        let set = compile_all(
            &doc,
            &[
                toggle(NodeRef::of(&items[0])),
                toggle(NodeRef::of(&items[1])),
            ],
        )
        .expect("disjoint items");
        assert_eq!(set.apply_to_text(doc.source()), "- [X] a\n- [X] b\n");

        let doc = parse("[[path]]\n");
        let link = first_link(&doc);
        let dest = link
            .part_insert_point(LinkPart::Description)
            .expect("absent desc");
        let err = compile_all(
            &doc,
            &[
                set_desc(NodeRef::of(&link), Some("a")),
                set_desc(NodeRef::of(&link), Some("b")),
            ],
        )
        .unwrap_err();
        assert_eq!(
            err,
            OperationError::ConflictingInserts {
                offset: dest.offset()
            }
        );

        let doc = parse("[[https://x.com][site]]\n");
        let link = first_link(&doc);
        let set = compile_all(
            &doc,
            &[
                set_path(NodeRef::of(&link), "https://y.com"),
                set_desc(NodeRef::of(&link), Some("other")),
            ],
        )
        .expect("path replace and present desc");
        assert_eq!(
            set.apply_to_text(doc.source()),
            "[[https://y.com][other]]\n"
        );

        let doc = parse("[[https://x.com]]\n");
        let link = first_link(&doc);
        let set = compile_all(
            &doc,
            &[
                set_path(NodeRef::of(&link), "https://y.com"),
                set_desc(NodeRef::of(&link), Some("site")),
            ],
        )
        .expect("path replace and absent desc insert");
        assert_eq!(set.apply_to_text(doc.source()), "[[https://y.com][site]]\n");

        let doc = parse("[[https://x.com]]\n");
        let link = first_link(&doc);
        let err = compile_all(
            &doc,
            &[
                convert(NodeRef::of(&link), LinkShape::Angle),
                set_path(NodeRef::of(&link), "https://y.com"),
            ],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            OperationError::EditSet(EditSetError::Overlap { .. })
        ));
    }
}
