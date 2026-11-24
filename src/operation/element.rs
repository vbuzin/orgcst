//! Elements by Org's names: the lines a new element is written as, wrapping
//! a run of elements in a block or a drawer, taking it out again, and
//! renaming one.
//!
//! Every write is checked by a trial parse: the new text must read back as
//! the element it was written as, holding what it was given, and every
//! other node must read back as it did.

use std::borrow::Cow;

use crate::ast::holds_code;
use crate::ast::{AstNode, Block, BlockPart, Drawer, Element, ElementView, HasParts, escape_code};
use crate::edit::{Edit, EditSet};
use crate::parser::grammar::forms::block::block_kind;
use crate::{Document, NodeRef, OperationError, OrgSyntaxKind, SyntaxNode, TextRange, TextSize};

/// An element to write, by Org's names. Text arguments are content: core
/// writes the markers and escapes what Org escapes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElementInput {
    /// A block, `#+begin_NAME PARAMETERS` ... `#+end_NAME`. Org's seven
    /// names (`src`, `example`, `export`, `comment`, `verse`, `quote`,
    /// `center`, in any case) are their own kinds; any other name is a
    /// special block. `parameters` is the rest of the begin line as written
    /// (a src block's language, switches and header arguments, an export
    /// block's backend), empty for none. `contents` is the body: in a src,
    /// example or export block it is code, and a line that would read as a
    /// heading or a keyword is escaped with a comma ([`escape_code`]); in the
    /// others it is Org text, written as it is.
    Block {
        /// The block's name, as written after `#+begin_`.
        name: String,
        /// The rest of the begin line.
        parameters: String,
        /// The body.
        contents: String,
    },
    /// A drawer, `:NAME:` ... `:END:`, holding Org text.
    Drawer {
        /// The name between the colons.
        name: String,
        /// The body.
        contents: String,
    },
    /// A plain list with one item per entry of `items`, each on one line:
    /// `- text`, or `1. text` numbered from one, with an empty check box
    /// (`- [ ] text`) when `checkbox` is set. An empty item is written as
    /// its bullet and a space.
    PlainList {
        /// Numbered (`1.`) rather than unordered (`-`).
        numbered: bool,
        /// Each item starts with an empty check box.
        checkbox: bool,
        /// Each item's text.
        items: Vec<String>,
    },
    /// A table, one line per row. Each column is padded with spaces to its
    /// widest cell, counted in characters, and a rule row spans the same
    /// widths (`|---+---|`).
    Table {
        /// The rows, in order.
        rows: Vec<TableRowInput>,
    },
    /// A horizontal rule, five dashes.
    HorizontalRule,
}

/// A row of an [`ElementInput::Table`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TableRowInput {
    /// A row of cells, each its text. A cell may not hold `|` or a line
    /// break.
    Cells(Vec<String>),
    /// A rule row.
    Rule,
}

impl Document {
    /// The lines `element` is written as, ending with one `\n`, for a caller
    /// that places them itself.
    ///
    /// # Errors
    ///
    /// [`OperationError::InvalidText`] when the text would not read back as
    /// that element holding what it was given: a name that is empty or holds
    /// whitespace, Org text in a block or a drawer that would end it early or
    /// hold a heading, an item or a cell with a line break, a cell with `|`,
    /// a table with no rows or a list with no items.
    pub fn element_lines(&self, element: &ElementInput) -> Result<String, OperationError> {
        let text = render(element, "")?;
        let doc = crate::parse_with(&text, self.config());
        let section = doc
            .root()
            .children()
            .find(|n| n.kind() == OrgSyntaxKind::SECTION);
        let mut elements = section
            .iter()
            .flat_map(SyntaxNode::children)
            .filter(|n| n.kind() != OrgSyntaxKind::AFFILIATED_KEYWORDS);
        let (Some(only), None) = (elements.next(), elements.next()) else {
            return Err(OperationError::InvalidText);
        };
        if only.text_range() != TextRange::up_to(TextSize::of(text.as_str()))
            || !reads_as(&doc, &only, element)
        {
            return Err(OperationError::InvalidText);
        }
        Ok(text)
    }
}

/// Whether `node` is the element `input` describes, holding what it was
/// given.
fn reads_as(doc: &Document, node: &SyntaxNode, input: &ElementInput) -> bool {
    match input {
        ElementInput::Block { name, contents, .. } => {
            let kind = block_kind(name, false);
            node.kind() == kind
                && named(node, name)
                && (holds_code(kind) || !holds_heading(doc, contents))
                && Block::cast(node.clone()).is_some_and(|b| body_text(doc, &b) == ended(contents))
        }
        ElementInput::Drawer { name, contents } => {
            named(node, name)
                && !holds_heading(doc, contents)
                && Drawer::cast(node.clone())
                    .is_some_and(|d| doc.slice(d.body_range()) == ended(contents))
        }
        ElementInput::PlainList { items, .. } => {
            node.kind() == OrgSyntaxKind::PLAIN_LIST
                && node
                    .children()
                    .filter(|n| n.kind() == OrgSyntaxKind::ITEM)
                    .count()
                    == items.len()
        }
        ElementInput::Table { rows } => {
            node.kind() == OrgSyntaxKind::TABLE
                && node
                    .children()
                    .filter(|n| {
                        matches!(
                            n.kind(),
                            OrgSyntaxKind::TABLE_ROW | OrgSyntaxKind::TABLE_RULE_ROW
                        )
                    })
                    .count()
                    == rows.len()
        }
        ElementInput::HorizontalRule => node.kind() == OrgSyntaxKind::HORIZONTAL_RULE,
    }
}

/// A block's body as given to [`ElementInput::Block`]: code unescaped, Org
/// text as written.
fn body_text(doc: &Document, block: &Block) -> String {
    block
        .value()
        .unwrap_or_else(|| doc.slice(block_interior(doc, block)).to_owned())
}

/// Everything between a block's begin line and its end line.
fn block_interior(doc: &Document, block: &Block) -> TextRange {
    let whole = block.range();
    let begin = block
        .part_range(BlockPart::BeginMarker)
        .unwrap_or_else(|| TextRange::empty(whole.start()));
    let end = block
        .part_range(BlockPart::EndMarker)
        .unwrap_or_else(|| TextRange::empty(whole.end()));
    let start = line_end(doc.source(), usize::from(begin.start()));
    TextRange::new(start, line_start(doc.source(), end.start()).max(start))
}

/// Whether Org text, read on its own, holds a heading, which no block or
/// drawer may hold.
fn holds_heading(doc: &Document, text: &str) -> bool {
    crate::parse_with(text, doc.config())
        .root()
        .children()
        .any(|n| n.kind() == OrgSyntaxKind::OUTLINE)
}

/// `s` ending with a line break, unless it is empty.
fn ended(s: &str) -> Cow<'_, str> {
    if s.is_empty() || s.ends_with('\n') {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(format!("{s}\n"))
    }
}

/// The text of `element`, each marker line indented by `indent`.
fn render(element: &ElementInput, indent: &str) -> Result<String, OperationError> {
    match element {
        ElementInput::Block {
            name,
            parameters,
            contents,
        } => {
            let (open, close) = block_markers(name, parameters, indent);
            Ok(format!(
                "{open}{}{close}\n",
                block_body(block_kind(name, false), contents)
            ))
        }
        ElementInput::Drawer { name, contents } => {
            let (open, close) = drawer_markers(name, indent);
            Ok(format!("{open}{}{close}\n", ended(contents)))
        }
        ElementInput::PlainList {
            numbered,
            checkbox,
            items,
        } => {
            if items.is_empty() {
                return Err(OperationError::InvalidText);
            }
            let mut out = String::new();
            for (n, item) in items.iter().enumerate() {
                if item.contains(['\n', '\r']) {
                    return Err(OperationError::InvalidText);
                }
                out.push_str(indent);
                if *numbered {
                    out.push_str(&(n + 1).to_string());
                    out.push_str(". ");
                } else {
                    out.push_str("- ");
                }
                if *checkbox {
                    out.push_str("[ ] ");
                }
                out.push_str(item);
                out.push('\n');
            }
            Ok(out)
        }
        ElementInput::Table { rows } => render_table(rows, indent),
        ElementInput::HorizontalRule => Ok(format!("{indent}-----\n")),
    }
}

/// A block's body as written: code escaped, Org text as it is, ending with
/// a line break unless empty.
fn block_body(kind: OrgSyntaxKind, contents: &str) -> String {
    let contents = ended(contents);
    if holds_code(kind) {
        escape_code(&contents).into_owned()
    } else {
        contents.into_owned()
    }
}

/// A block's begin line, with its line break, and its end line, without.
/// Whether they read back as that block is the caller's trial.
fn block_markers(name: &str, parameters: &str, indent: &str) -> (String, String) {
    let parameters = parameters.trim();
    let open = if parameters.is_empty() {
        format!("{indent}#+begin_{name}\n")
    } else {
        format!("{indent}#+begin_{name} {parameters}\n")
    };
    (open, format!("{indent}#+end_{name}"))
}

/// A drawer's opener line, with its line break, and its `:END:` line,
/// without. Whether they read back as that drawer is the caller's trial.
fn drawer_markers(name: &str, indent: &str) -> (String, String) {
    (format!("{indent}:{name}:\n"), format!("{indent}:END:"))
}

/// Whether `node` is a block or drawer named `name` as written: Org's own
/// block names compare without case, as the parser reads them.
fn named(node: &SyntaxNode, name: &str) -> bool {
    if let Some(d) = Drawer::cast(node.clone()) {
        return d.name() == name;
    }
    Block::cast(node.clone()).is_some_and(|b| {
        let read = b.block_name();
        if b.kind() == crate::BlockKind::Special {
            read == name
        } else {
            read.eq_ignore_ascii_case(name)
        }
    })
}

/// Refuse a name that would not be written back as a block or drawer of
/// that name, by the same trial as [`Document::element_lines`].
fn check_written(doc: &Document, element: &ElementInput) -> Result<(), OperationError> {
    let empty = match element {
        ElementInput::Block {
            name, parameters, ..
        } => ElementInput::Block {
            name: name.clone(),
            parameters: parameters.clone(),
            contents: String::new(),
        },
        ElementInput::Drawer { name, .. } => ElementInput::Drawer {
            name: name.clone(),
            contents: String::new(),
        },
        _ => return Err(OperationError::InvalidText),
    };
    doc.element_lines(&empty).map(|_| ())
}

fn render_table(rows: &[TableRowInput], indent: &str) -> Result<String, OperationError> {
    if rows.is_empty() {
        return Err(OperationError::InvalidText);
    }
    let mut widths: Vec<usize> = Vec::new();
    for row in rows {
        if let TableRowInput::Cells(cells) = row {
            for (i, cell) in cells.iter().enumerate() {
                if cell.contains(['|', '\n', '\r']) {
                    return Err(OperationError::InvalidText);
                }
                let width = cell.trim().chars().count().max(1);
                match widths.get_mut(i) {
                    Some(w) => *w = (*w).max(width),
                    None => widths.push(width),
                }
            }
        }
    }
    if widths.is_empty() {
        widths.push(1);
    }
    let mut out = String::new();
    for row in rows {
        out.push_str(indent);
        match row {
            TableRowInput::Cells(cells) => {
                out.push('|');
                for (i, width) in widths.iter().enumerate() {
                    let cell = cells.get(i).map_or("", |c| c.trim());
                    let pad = width - cell.chars().count();
                    out.push(' ');
                    out.push_str(cell);
                    out.push_str(&" ".repeat(pad));
                    out.push_str(" |");
                }
            }
            TableRowInput::Rule => {
                out.push('|');
                let dashes: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
                out.push_str(&dashes.join("+"));
                out.push('|');
            }
        }
        out.push('\n');
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Wrap, unwrap, rename
// ---------------------------------------------------------------------------

/// The kinds [`crate::Operation::UnwrapElement`] and
/// [`crate::Operation::SetElementName`] accept.
const NAMED_KINDS: &[OrgSyntaxKind] = &[
    OrgSyntaxKind::SRC_BLOCK,
    OrgSyntaxKind::EXAMPLE_BLOCK,
    OrgSyntaxKind::EXPORT_BLOCK,
    OrgSyntaxKind::COMMENT_BLOCK,
    OrgSyntaxKind::VERSE_BLOCK,
    OrgSyntaxKind::QUOTE_BLOCK,
    OrgSyntaxKind::CENTER_BLOCK,
    OrgSyntaxKind::SPECIAL_BLOCK,
    OrgSyntaxKind::DRAWER,
];

pub(super) fn compile_wrap_in_element(
    doc: &Document,
    range: TextRange,
    element: &ElementInput,
) -> Result<EditSet, OperationError> {
    let (first, last) = element_run(doc, range)?;
    check_written(doc, element)?;
    let start = range.start();
    // Blank lines after the run stay after the new element.
    let end = content_end(doc, &last);
    let indent = line_indent(doc, first.full_range().start());
    let text = doc.slice(TextRange::new(start, end));
    let at_eof = !text.ends_with('\n');
    let (open, close, body, kind) = match element {
        ElementInput::Block {
            name,
            parameters,
            contents,
        } if contents.is_empty() => {
            let (open, close) = block_markers(name, parameters, indent);
            let kind = block_kind(name, false);
            let body = if holds_code(kind) {
                escape_code(text)
            } else {
                Cow::Borrowed(text)
            };
            (open, close, body, kind)
        }
        ElementInput::Drawer { name, contents } if contents.is_empty() => {
            let (open, close) = drawer_markers(name, indent);
            (open, close, Cow::Borrowed(text), OrgSyntaxKind::DRAWER)
        }
        _ => return Err(OperationError::InvalidText),
    };
    let close = if at_eof {
        format!("\n{close}")
    } else {
        format!("{close}\n")
    };
    let edits = match body {
        Cow::Borrowed(_) => vec![
            Edit {
                range: TextRange::empty(start),
                replacement: open.clone(),
            },
            Edit {
                range: TextRange::empty(end),
                replacement: close.clone(),
            },
        ],
        Cow::Owned(ref escaped) => vec![Edit {
            range: TextRange::new(start, end),
            replacement: format!("{open}{escaped}{close}"),
        }],
    };
    let set = EditSet::new(doc.source(), edits)?;
    let written =
        TextSize::of(open.as_str()) + TextSize::of(body.as_ref()) + TextSize::of(close.as_str());
    let refused = OperationError::NotAllowedHere {
        kind,
        scope: first
            .syntax()
            .parent()
            .map_or(OrgSyntaxKind::DOCUMENT, |p| p.kind()),
    };
    let next = doc.apply(&set);
    let Some(new) = next
        .root()
        .descendants()
        .find(|n| n.kind() == kind && n.text_range().start() == start)
    else {
        return Err(refused);
    };
    let holds = match kind {
        OrgSyntaxKind::DRAWER => {
            Drawer::cast(new.clone()).is_some_and(|d| next.slice(d.body_range()) == ended(text))
        }
        _ => Block::cast(new.clone()).is_some_and(|b| body_text(&next, &b) == ended(text)),
    };
    let wrapped = TextRange::at(start, written);
    if !holds
        || !new.text_range().contains_range(wrapped)
        || !same_outside(
            doc,
            &first,
            TextRange::new(start, last.full_range().end()),
            &next,
            &new,
        )
    {
        return Err(refused);
    }
    Ok(set)
}

pub(super) fn compile_extend_element(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let node = resolve_named(doc, target)?;
    let next = Element::cast(node.clone())
        .and_then(|e| e.next_element())
        .ok_or(OperationError::InvalidOffset)?;
    let (_, body, close) = element_lines(doc, &node);
    // The end line moves past the next element; blank lines after that
    // element stay after the block.
    let start = next.full_range().start();
    let end = content_end(doc, &next);
    let moved = doc.slice(TextRange::new(close.start(), end));
    let taken = &moved[usize::from(start - close.start())..];
    let end_line = doc.slice(close);
    let code = holds_code(node.kind());
    let taken_text = if code {
        escape_code(taken)
    } else {
        Cow::Borrowed(taken)
    };
    let gap = &moved[usize::from(close.len())..usize::from(start - close.start())];
    let (taken_text, end_line) = if taken_text.ends_with('\n') || end_line.ends_with('\n') {
        (
            ended(&taken_text).into_owned(),
            end_line.trim_end_matches(['\n', '\r']).to_owned() + "\n",
        )
    } else {
        (taken_text.into_owned(), end_line.to_owned())
    };
    let set = EditSet::single(
        doc.source(),
        TextRange::new(close.start(), end),
        format!("{gap}{taken_text}{end_line}"),
    )?;
    let next_doc = doc.apply(&set);
    let refused = OperationError::NotAllowedHere {
        kind: node.kind(),
        scope: node.parent().map_or(OrgSyntaxKind::DOCUMENT, |p| p.kind()),
    };
    let grown = next_doc
        .root()
        .descendants()
        .find(|n| n.kind() == node.kind() && n.text_range().start() == node.text_range().start())
        .ok_or_else(|| refused.clone())?;
    let old_body = match Block::cast(node.clone()) {
        Some(b) => body_text(doc, &b),
        None => doc.slice(body).to_owned(),
    };
    let new_body = match Block::cast(grown.clone()) {
        Some(b) => body_text(&next_doc, &b),
        None => Drawer::cast(grown.clone())
            .map_or_else(String::new, |d| next_doc.slice(d.body_range()).to_owned()),
    };
    let want = format!("{old_body}{gap}{}", ended(taken));
    if new_body != want || (!code && holds_heading(doc, taken)) {
        return Err(refused);
    }
    Ok(set)
}

pub(super) fn compile_unwrap_element(
    doc: &Document,
    target: NodeRef,
) -> Result<EditSet, OperationError> {
    let node = resolve_named(doc, target)?;
    let (open, _, close) = element_lines(doc, &node);
    let edits = match Block::cast(node.clone()) {
        Some(block) if holds_code(block.syntax().kind()) => vec![Edit {
            range: TextRange::new(open.start(), close.end()),
            replacement: body_text(doc, &block),
        }],
        _ => vec![
            Edit {
                range: open,
                replacement: String::new(),
            },
            Edit {
                range: close,
                replacement: String::new(),
            },
        ],
    };
    Ok(EditSet::new(doc.source(), edits)?)
}

pub(super) fn compile_set_element_name(
    doc: &Document,
    target: NodeRef,
    name: &str,
) -> Result<EditSet, OperationError> {
    let node = resolve_named(doc, target)?;
    let from = node.kind();
    let (to, edits) = if from == OrgSyntaxKind::DRAWER {
        check_written(
            doc,
            &ElementInput::Drawer {
                name: name.to_owned(),
                contents: String::new(),
            },
        )?;
        let drawer = Drawer::cast(node.clone()).expect("grammar: DRAWER kind always casts");
        let range = drawer.name_node().text_range();
        if doc.slice(range) == name {
            return Ok(super::empty_edit_set(doc.source()));
        }
        (
            OrgSyntaxKind::DRAWER,
            vec![Edit {
                range,
                replacement: name.to_owned(),
            }],
        )
    } else {
        check_written(
            doc,
            &ElementInput::Block {
                name: name.to_owned(),
                parameters: String::new(),
                contents: String::new(),
            },
        )?;
        let to = block_kind(name, false);
        let (begin, end) = block_name_ranges(doc, &node);
        let mut edits = vec![Edit {
            range: begin,
            replacement: name.to_owned(),
        }];
        if holds_code(from) != holds_code(to) {
            let block = Block::cast(node.clone()).expect("grammar: block kind always casts");
            let (_, body, _) = element_lines(doc, &node);
            let text = body_text(doc, &block);
            let rewritten = if holds_code(to) {
                escape_code(&text).into_owned()
            } else if holds_heading(doc, &text) {
                return Err(OperationError::NotAllowedHere {
                    kind: to,
                    scope: node.parent().map_or(OrgSyntaxKind::DOCUMENT, |p| p.kind()),
                });
            } else {
                text
            };
            if rewritten != doc.slice(body) {
                edits.push(Edit {
                    range: body,
                    replacement: rewritten,
                });
            }
        }
        edits.push(Edit {
            range: end,
            replacement: name.to_owned(),
        });
        if doc.slice(begin) == name && doc.slice(end) == name && edits.len() == 2 {
            return Ok(super::empty_edit_set(doc.source()));
        }
        (to, edits)
    };
    let set = EditSet::new(doc.source(), edits)?;
    let next = doc.apply(&set);
    let start = node.text_range().start();
    let renamed = next
        .root()
        .descendants()
        .find(|n| n.kind() == to && n.text_range().start() == start);
    let Some(renamed) = renamed else {
        return Err(OperationError::NotAllowedHere {
            kind: to,
            scope: node.parent().map_or(OrgSyntaxKind::DOCUMENT, |p| p.kind()),
        });
    };
    let _ = renamed;
    Ok(set)
}

/// The block or drawer `target` names.
fn resolve_named(doc: &Document, target: NodeRef) -> Result<SyntaxNode, OperationError> {
    let node = target.resolve(doc).ok_or(OperationError::StaleTarget)?;
    if !NAMED_KINDS.contains(&node.kind()) {
        return Err(OperationError::WrongKind {
            expected: NAMED_KINDS,
            actual: node.kind(),
        });
    }
    Ok(node)
}

/// A block's or drawer's begin line, body and end line, each line with its
/// line break; blank lines after the end line are not part of it.
fn element_lines(doc: &Document, node: &SyntaxNode) -> (TextRange, TextRange, TextRange) {
    let whole = node.text_range();
    let (body, end_marker) = if let Some(drawer) = Drawer::cast(node.clone()) {
        let body = drawer.body_range();
        (body, body.end())
    } else {
        let block = Block::cast(node.clone()).expect("grammar: named kinds are blocks");
        let body = block_interior(doc, &block);
        (body, body.end())
    };
    let close_end = line_end(doc.source(), usize::from(end_marker)).min(whole.end());
    (
        TextRange::new(whole.start(), body.start()),
        body,
        TextRange::new(body.end(), close_end),
    )
}

/// The name on a block's begin line and on its end line.
fn block_name_ranges(doc: &Document, node: &SyntaxNode) -> (TextRange, TextRange) {
    let block = Block::cast(node.clone()).expect("grammar: named kinds are blocks");
    let marker = block
        .part_range(BlockPart::BeginMarker)
        .unwrap_or_else(|| TextRange::empty(node.text_range().start()));
    let closer = block
        .part_range(BlockPart::EndMarker)
        .unwrap_or_else(|| TextRange::empty(node.text_range().end()));
    let name_after = |from: TextSize, prefix: usize| {
        let start = usize::from(from) + prefix;
        let src = doc.source();
        let len = src[start..]
            .find(|c: char| c.is_whitespace())
            .unwrap_or(src.len() - start);
        TextRange::at(TextSize::from(start as u32), TextSize::from(len as u32))
    };
    (
        name_after(marker.start(), "#+begin_".len()),
        name_after(closer.start(), "#+end_".len()),
    )
}

/// The first and last element of the sibling run `range` holds exactly, from
/// the first one's affiliated keywords to the last one's end.
fn element_run(doc: &Document, range: TextRange) -> Result<(Element, Element), OperationError> {
    if range.is_empty() || range.end() > TextSize::of(doc.source()) {
        return Err(OperationError::InvalidOffset);
    }
    let first = doc
        .root()
        .descendants()
        .filter_map(Element::cast)
        .find(|e| e.full_range().start() == range.start())
        .ok_or(OperationError::InvalidOffset)?;
    let mut last = first.clone();
    loop {
        let r = last.full_range();
        if r.end() == range.end() || content_end(doc, &last) == range.end() {
            return Ok((first, last));
        }
        if r.end() > range.end() {
            return Err(OperationError::InvalidOffset);
        }
        last = last.next_element().ok_or(OperationError::InvalidOffset)?;
    }
}

/// Where an element's text ends, before the blank lines it owns.
fn content_end(doc: &Document, element: &Element) -> TextSize {
    let r = element.full_range();
    let text = doc.slice(r);
    let trimmed = text.trim_end_matches(['\n', '\r', ' ', '\t']);
    let keep = text[trimmed.len()..]
        .find('\n')
        .map_or(text.len(), |i| trimmed.len() + i + 1);
    r.start() + TextSize::from(keep as u32)
}

/// The spaces and tabs that start the line `at` is on.
fn line_indent(doc: &Document, at: TextSize) -> &str {
    let src = doc.source();
    let at = usize::from(at);
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let rest = &src[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// The start of the line holding `at`.
fn line_start(src: &str, at: TextSize) -> TextSize {
    let at = usize::from(at);
    TextSize::from(src[..at].rfind('\n').map_or(0, |i| i + 1) as u32)
}

/// The end of the line holding byte `at`, its line break included.
fn line_end(src: &str, at: usize) -> TextSize {
    let end = src[at..].find('\n').map_or(src.len(), |i| at + i + 1);
    TextSize::from(end as u32)
}

/// Whether every node of `next` outside `new` reads back as the nodes of
/// `doc` outside `range`, the nodes holding `first` included.
fn same_outside(
    doc: &Document,
    first: &Element,
    range: TextRange,
    next: &Document,
    new: &SyntaxNode,
) -> bool {
    let holding: Vec<SyntaxNode> = first
        .syntax()
        .parent()
        .map_or_else(Vec::new, |p| p.ancestors().collect());
    let before: Vec<OrgSyntaxKind> = doc
        .root()
        .descendants()
        .filter(|n| !range.contains_range(n.text_range()) || holding.contains(n))
        .map(|n| n.kind())
        .collect();
    let after: Vec<OrgSyntaxKind> = next
        .root()
        .descendants()
        .filter(|n| n != new && !n.ancestors().any(|a| a == *new))
        .map(|n| n.kind())
        .collect();
    before == after
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Operation, compile_write};
    use crate::parse;

    fn lines(element: &ElementInput) -> Result<String, OperationError> {
        parse("").element_lines(element)
    }

    fn block(name: &str, parameters: &str, contents: &str) -> ElementInput {
        ElementInput::Block {
            name: name.to_owned(),
            parameters: parameters.to_owned(),
            contents: contents.to_owned(),
        }
    }

    fn drawer(name: &str, contents: &str) -> ElementInput {
        ElementInput::Drawer {
            name: name.to_owned(),
            contents: contents.to_owned(),
        }
    }

    fn cells(cells: &[&str]) -> TableRowInput {
        TableRowInput::Cells(cells.iter().map(|c| (*c).to_owned()).collect())
    }

    fn run(src: &str, op: &Operation) -> Result<String, OperationError> {
        let doc = parse(src);
        compile(&doc, op)?;
        Ok(compile_write(&doc, op).1)
    }

    fn compile(doc: &Document, op: &Operation) -> Result<EditSet, OperationError> {
        crate::compile(doc, op)
    }

    #[test]
    fn writes_blocks_by_name() {
        assert_eq!(
            lines(&block("quote", "", "\n")).unwrap(),
            "#+begin_quote\n\n#+end_quote\n"
        );
        assert_eq!(
            lines(&block("note", "", "")).unwrap(),
            "#+begin_note\n#+end_note\n"
        );
        assert_eq!(
            lines(&block("src", "python :results output", "print(1)")).unwrap(),
            "#+begin_src python :results output\nprint(1)\n#+end_src\n"
        );
    }

    #[test]
    fn escapes_code_and_not_org_text() {
        assert_eq!(
            lines(&block("src", "org", "* Heading\n#+TITLE: x\n")).unwrap(),
            "#+begin_src org\n,* Heading\n,#+TITLE: x\n#+end_src\n"
        );
        // Org text that would hold a heading or end the block is refused.
        assert_eq!(
            lines(&block("quote", "", "* Heading\n")),
            Err(OperationError::InvalidText)
        );
        assert_eq!(
            lines(&block("quote", "", "#+end_quote\n")),
            Err(OperationError::InvalidText)
        );
    }

    #[test]
    fn refuses_names_that_cannot_be_written() {
        assert_eq!(lines(&block("", "", "")), Err(OperationError::InvalidText));
        assert_eq!(
            lines(&block("a b", "", "")),
            Err(OperationError::InvalidText)
        );
        assert_eq!(lines(&drawer("a:b", "")), Err(OperationError::InvalidText));
    }

    #[test]
    fn writes_drawers() {
        assert_eq!(
            lines(&drawer("NOTES", "Hi")).unwrap(),
            ":NOTES:\nHi\n:END:\n"
        );
        assert_eq!(lines(&drawer("x", "")).unwrap(), ":x:\n:END:\n");
    }

    #[test]
    fn writes_lists() {
        let list = |numbered, checkbox, items: &[&str]| ElementInput::PlainList {
            numbered,
            checkbox,
            items: items.iter().map(|i| (*i).to_owned()).collect(),
        };
        assert_eq!(lines(&list(false, false, &[""])).unwrap(), "- \n");
        assert_eq!(
            lines(&list(true, false, &["a", "b"])).unwrap(),
            "1. a\n2. b\n"
        );
        assert_eq!(lines(&list(false, true, &[""])).unwrap(), "- [ ] \n");
        assert_eq!(
            lines(&list(false, false, &[])),
            Err(OperationError::InvalidText)
        );
        assert_eq!(
            lines(&list(false, false, &["a\nb"])),
            Err(OperationError::InvalidText)
        );
    }

    #[test]
    fn writes_tables_padded_to_columns() {
        let table = ElementInput::Table {
            rows: vec![cells(&["", ""]), TableRowInput::Rule, cells(&["", ""])],
        };
        assert_eq!(lines(&table).unwrap(), "|   |   |\n|---+---|\n|   |   |\n");
        let table = ElementInput::Table {
            rows: vec![
                cells(&["Name", "Qty"]),
                TableRowInput::Rule,
                cells(&["Tea", "2"]),
            ],
        };
        assert_eq!(
            lines(&table).unwrap(),
            "| Name | Qty |\n|------+-----|\n| Tea  | 2   |\n"
        );
        let bad = ElementInput::Table {
            rows: vec![cells(&["a|b"])],
        };
        assert_eq!(lines(&bad), Err(OperationError::InvalidText));
    }

    #[test]
    fn writes_a_rule() {
        assert_eq!(lines(&ElementInput::HorizontalRule).unwrap(), "-----\n");
    }

    fn wrap(range: (u32, u32), element: ElementInput) -> Operation {
        Operation::WrapInElement {
            range: TextRange::new(range.0.into(), range.1.into()),
            element,
        }
    }

    #[test]
    fn wraps_paragraphs_in_a_quote() {
        let src = "Before.\n\nOne.\n\nTwo.\n\nAfter.\n";
        // "One.\n\nTwo.\n" runs from 9 to 20, its blank line to 21.
        assert_eq!(
            run(src, &wrap((9, 20), block("quote", "", ""))).unwrap(),
            "Before.\n\n#+begin_quote\nOne.\n\nTwo.\n#+end_quote\n\nAfter.\n"
        );
        // With the trailing blank line, the same.
        assert_eq!(
            run(src, &wrap((9, 21), block("quote", "", ""))).unwrap(),
            "Before.\n\n#+begin_quote\nOne.\n\nTwo.\n#+end_quote\n\nAfter.\n"
        );
    }

    #[test]
    fn wraps_in_code_with_escaping() {
        let src = "#+TITLE: x\n";
        assert_eq!(
            run(src, &wrap((0, 11), block("example", "", ""))).unwrap(),
            "#+begin_example\n,#+TITLE: x\n#+end_example\n"
        );
    }

    #[test]
    fn wraps_in_a_drawer_and_at_the_end_of_the_file() {
        assert_eq!(
            run("One.", &wrap((0, 4), drawer("NOTES", ""))).unwrap(),
            ":NOTES:\nOne.\n:END:"
        );
    }

    #[test]
    fn wrap_refuses_part_of_an_element_and_contents() {
        let src = "One two.\n";
        assert_eq!(
            run(src, &wrap((0, 3), block("quote", "", ""))),
            Err(OperationError::InvalidOffset)
        );
        assert_eq!(
            run(src, &wrap((0, 9), block("quote", "", "x"))),
            Err(OperationError::InvalidText)
        );
        assert_eq!(
            run(src, &wrap((0, 9), ElementInput::HorizontalRule)),
            Err(OperationError::InvalidText)
        );
    }

    #[test]
    fn wrap_keeps_indentation_in_an_item() {
        let src = "- Item\n\n  Para.\n";
        // "  Para.\n" is the item's second element, from 8 to 16.
        assert_eq!(
            run(src, &wrap((8, 16), block("quote", "", ""))).unwrap(),
            "- Item\n\n  #+begin_quote\n  Para.\n  #+end_quote\n"
        );
    }

    fn unwrap(doc_src: &str) -> Result<String, OperationError> {
        let doc = parse(doc_src);
        let node = doc
            .root()
            .descendants()
            .find(|n| NAMED_KINDS.contains(&n.kind()) || n.kind() == OrgSyntaxKind::DYNAMIC_BLOCK)
            .expect("a block or drawer");
        run(
            doc_src,
            &Operation::UnwrapElement {
                target: NodeRef::at(node.text_range().start(), node.kind()),
            },
        )
    }

    #[test]
    fn unwraps_blocks_and_drawers() {
        assert_eq!(
            unwrap("#+begin_quote\nOne.\n#+end_quote\n\nAfter.\n").unwrap(),
            "One.\n\nAfter.\n"
        );
        assert_eq!(
            unwrap("#+begin_src org\n,* Heading\n#+end_src\n").unwrap(),
            "* Heading\n"
        );
        assert_eq!(unwrap(":NOTES:\nHi\n:END:\n").unwrap(), "Hi\n");
        assert_eq!(unwrap("#+begin_note\n#+end_note\n").unwrap(), "");
        assert!(matches!(
            unwrap("#+BEGIN: clocktable\n#+END:\n"),
            Err(OperationError::WrongKind { .. })
        ));
    }

    fn rename(src: &str, name: &str) -> Result<String, OperationError> {
        let doc = parse(src);
        let node = doc
            .root()
            .descendants()
            .find(|n| NAMED_KINDS.contains(&n.kind()))
            .expect("a block or drawer");
        run(
            src,
            &Operation::SetElementName {
                target: NodeRef::at(node.text_range().start(), node.kind()),
                name: name.to_owned(),
            },
        )
    }

    #[test]
    fn renames_blocks_and_drawers() {
        assert_eq!(
            rename("#+begin_note\nOne.\n#+end_note\n", "tip").unwrap(),
            "#+begin_tip\nOne.\n#+end_tip\n"
        );
        assert_eq!(
            rename("#+BEGIN_QUOTE\nOne.\n#+END_QUOTE\n", "warning").unwrap(),
            "#+BEGIN_warning\nOne.\n#+END_warning\n"
        );
        assert_eq!(
            rename(":NOTES:\nHi\n:END:\n", "LOG").unwrap(),
            ":LOG:\nHi\n:END:\n"
        );
    }

    #[test]
    fn renaming_between_code_and_text_escapes_the_body() {
        assert_eq!(
            rename("#+begin_quote\n#+TITLE: x\n#+end_quote\n", "example").unwrap(),
            "#+begin_example\n,#+TITLE: x\n#+end_example\n"
        );
        assert_eq!(
            rename("#+begin_example\n,#+TITLE: x\n#+end_example\n", "note").unwrap(),
            "#+begin_note\n#+TITLE: x\n#+end_note\n"
        );
    }

    #[test]
    fn rename_refuses_bad_names() {
        assert_eq!(
            rename("#+begin_note\n#+end_note\n", "a b"),
            Err(OperationError::InvalidText)
        );
        assert_eq!(
            rename(":NOTES:\n:END:\n", "a:b"),
            Err(OperationError::InvalidText)
        );
    }

    #[test]
    fn wrap_refuses_a_drawer_in_a_drawer() {
        let src = ":A:\nx\n:END:\n";
        assert!(matches!(
            run(src, &wrap((0, 12), drawer("B", ""))),
            Err(OperationError::NotAllowedHere { .. })
        ));
    }

    #[test]
    fn rename_refuses_code_that_would_become_a_heading() {
        assert!(matches!(
            rename("#+begin_example\n,* x\n#+end_example\n", "quote"),
            Err(OperationError::NotAllowedHere { .. })
        ));
    }

    fn extend(src: &str) -> Result<String, OperationError> {
        let doc = parse(src);
        let node = doc
            .root()
            .descendants()
            .find(|n| NAMED_KINDS.contains(&n.kind()) || n.kind() == OrgSyntaxKind::DYNAMIC_BLOCK)
            .expect("a block or drawer");
        run(
            src,
            &Operation::ExtendElement {
                target: NodeRef::at(node.text_range().start(), node.kind()),
            },
        )
    }

    #[test]
    fn extends_a_block_over_the_next_element() {
        assert_eq!(
            extend("#+begin_quote\nOne\n#+end_quote\nAfter.\n\nMore.\n").unwrap(),
            "#+begin_quote\nOne\nAfter.\n#+end_quote\n\nMore.\n"
        );
        // Blank lines between stay inside, between the two.
        assert_eq!(
            extend("#+begin_quote\nOne\n#+end_quote\n\nAfter.\n").unwrap(),
            "#+begin_quote\nOne\n\nAfter.\n#+end_quote\n"
        );
        assert_eq!(
            extend(":NOTES:\nOne\n:END:\nAfter.\n").unwrap(),
            ":NOTES:\nOne\nAfter.\n:END:\n"
        );
        // Code is escaped as it comes in.
        assert_eq!(
            extend("#+begin_example\nx\n#+end_example\n#+TITLE: t\n").unwrap(),
            "#+begin_example\nx\n,#+TITLE: t\n#+end_example\n"
        );
    }

    #[test]
    fn extend_refuses_with_nothing_after_or_a_drawer_into_a_drawer() {
        assert_eq!(
            extend("#+begin_quote\nOne\n#+end_quote\n"),
            Err(OperationError::InvalidOffset)
        );
        assert!(matches!(
            extend(":A:\nx\n:END:\n:B:\ny\n:END:\n"),
            Err(OperationError::NotAllowedHere { .. })
        ));
    }
}
