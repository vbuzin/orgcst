//! Lossy projection of the CST into the org-element JSON shape `dump.el`
//! prints. Not a public grammar.

use orgcst::{
    AffiliatedKeywords, AstNode, BabelCall, BabelCallPart, Block, BlockBody, BlockKind,
    CheckboxState, Citation, CitationReference, CitationReferencePart, Clock, ClockPart, Counter,
    CounterForm, Drawer, Element, FootnoteDefinition, FootnotePart, FootnoteReference, HasElements,
    HasParts, Headline, InlineBabelCall, InlineBabelCallPart, InlineSrcBlock, InlineSrcPart,
    InlineTask, Item, Keyword, Link, LinkShape, Markup, MarkupBody, MarkupKind, NodeProperty,
    OrgSyntaxKind, Outline, Planning, PropertyDrawer, SyntaxNode, Table, TextRange, TextSize,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub type ProjectError = String;

pub fn project(source: &str, config: &orgcst::ParseConfig) -> Result<Value, ProjectError> {
    let doc = orgcst::parse_with(source, config);
    let root = doc.root();
    for node in root.descendants() {
        if node.kind() == OrgSyntaxKind::ERROR {
            return Err(format!(
                "ERROR node at {}",
                u32::from(node.text_range().start())
            ));
        }
    }
    emit(root, source)?.ok_or_else(|| "document root was not emitted".to_string())
}

struct Node {
    ty: &'static str,
    begin: u32,
    end: u32,
    properties: Map<String, Value>,
    children: Vec<Value>,
    secondary: Map<String, Value>,
    affiliated: Map<String, Value>,
}

impl Node {
    fn new(ty: &'static str, begin: TextSize, end: TextSize) -> Self {
        Self {
            ty,
            begin: u32::from(begin),
            end: u32::from(end),
            properties: Map::new(),
            children: Vec::new(),
            secondary: Map::new(),
            affiliated: Map::new(),
        }
    }

    fn into_value(self) -> Value {
        let mut map = Map::new();
        map.insert("type".to_string(), json!(self.ty));
        map.insert("begin".to_string(), json!(self.begin));
        map.insert("end".to_string(), json!(self.end));
        map.insert("properties".to_string(), Value::Object(self.properties));
        map.insert("children".to_string(), Value::Array(self.children));
        if !self.secondary.is_empty() {
            map.insert("secondary".to_string(), Value::Object(self.secondary));
        }
        if !self.affiliated.is_empty() {
            map.insert("affiliated".to_string(), Value::Object(self.affiliated));
        }
        Value::Object(map)
    }
}

fn emit(syntax: &SyntaxNode, src: &str) -> Result<Option<Value>, ProjectError> {
    let kind = syntax.kind();
    if kind.is_token() {
        return Ok(None);
    }
    if is_skip(kind) {
        return Ok(None);
    }
    let node = match kind {
        OrgSyntaxKind::DOCUMENT => emit_document(syntax, src)?,
        OrgSyntaxKind::SECTION => emit_zeroth_section(syntax, src)?,
        OrgSyntaxKind::OUTLINE => emit_outline(&cast(syntax)?, src)?,
        OrgSyntaxKind::INLINE_TASK => emit_inline_task(&cast(syntax)?, src)?,
        OrgSyntaxKind::PLANNING => emit_planning(&cast(syntax)?, extended_end(syntax), src)?,
        OrgSyntaxKind::PROPERTY_DRAWER => {
            let end = if metadata_parent(syntax) {
                extended_end(syntax)
            } else {
                syntax.text_range().end()
            };
            emit_property_drawer(&cast(syntax)?, end, src)?
        }
        OrgSyntaxKind::CLOCK => emit_clock(&cast(syntax)?, src)?,
        OrgSyntaxKind::TABLE => emit_table(&cast(syntax)?, src)?,
        OrgSyntaxKind::CITATION => emit_citation(&cast(syntax)?, src)?,
        OrgSyntaxKind::CITATION_REFERENCE => emit_citation_reference(&cast(syntax)?, src)?,
        OrgSyntaxKind::ITEM => emit_item(&cast(syntax)?, src)?,
        OrgSyntaxKind::LINK => emit_link(&cast(syntax)?, src)?,
        OrgSyntaxKind::KEYWORD | OrgSyntaxKind::SETTING_KEYWORD => {
            emit_keyword(&cast(syntax)?, src)?
        }
        OrgSyntaxKind::BABEL_CALL => emit_babel(&cast(syntax)?, src)?,
        OrgSyntaxKind::INLINE_BABEL_CALL => emit_inline_babel(&cast(syntax)?, src)?,
        OrgSyntaxKind::INLINE_SRC_BLOCK => emit_inline_src(&cast(syntax)?, src)?,
        OrgSyntaxKind::TIMESTAMP => emit_timestamp(syntax, src)?,
        OrgSyntaxKind::BOLD
        | OrgSyntaxKind::ITALIC
        | OrgSyntaxKind::UNDERLINE
        | OrgSyntaxKind::STRIKE_THROUGH
        | OrgSyntaxKind::VERBATIM
        | OrgSyntaxKind::CODE => emit_markup(&cast(syntax)?, src)?,
        OrgSyntaxKind::SUBSCRIPT => emit_objects_node("subscript", syntax, src)?,
        OrgSyntaxKind::SUPERSCRIPT => emit_objects_node("superscript", syntax, src)?,
        OrgSyntaxKind::ENTITY => leaf("entity", syntax, src)?,
        OrgSyntaxKind::EXPORT_SNIPPET => leaf("export-snippet", syntax, src)?,
        OrgSyntaxKind::STATISTICS_COOKIE => leaf("statistics-cookie", syntax, src)?,
        OrgSyntaxKind::MACRO => leaf("macro", syntax, src)?,
        OrgSyntaxKind::LINE_BREAK => leaf("line-break", syntax, src)?,
        OrgSyntaxKind::LATEX_FRAGMENT => span_only("latex-fragment", syntax, src)?,
        OrgSyntaxKind::RADIO_TARGET => emit_objects_node("radio-target", syntax, src)?,
        OrgSyntaxKind::TARGET => emit_objects_node("target", syntax, src)?,
        OrgSyntaxKind::PARAGRAPH => emit_objects_node("paragraph", syntax, src)?,
        OrgSyntaxKind::COMMENT => span_only("comment", syntax, src)?,
        OrgSyntaxKind::FIXED_WIDTH => span_only("fixed-width", syntax, src)?,
        OrgSyntaxKind::HORIZONTAL_RULE => span_only("horizontal-rule", syntax, src)?,
        OrgSyntaxKind::FOOTNOTE_DEFINITION => emit_footnote_def(&cast(syntax)?, src)?,
        OrgSyntaxKind::FOOTNOTE_REFERENCE => emit_footnote_ref(&cast(syntax)?, src)?,
        OrgSyntaxKind::PLAIN_LIST => emit_objects_node("plain-list", syntax, src)?,
        OrgSyntaxKind::TABLE_ROW | OrgSyntaxKind::TABLE_RULE_ROW => emit_row(&cast(syntax)?, src)?,
        OrgSyntaxKind::TABLE_CELL => emit_objects_node("table-cell", syntax, src)?,
        OrgSyntaxKind::DRAWER => emit_drawer(&cast(syntax)?, src)?,
        OrgSyntaxKind::NODE_PROPERTY => emit_node_property(&cast(syntax)?, src)?,
        OrgSyntaxKind::DIARY_SEXP => span_only("diary-sexp", syntax, src)?,
        OrgSyntaxKind::LATEX_ENVIRONMENT => span_only("latex-environment", syntax, src)?,
        OrgSyntaxKind::CENTER_BLOCK
        | OrgSyntaxKind::QUOTE_BLOCK
        | OrgSyntaxKind::SPECIAL_BLOCK
        | OrgSyntaxKind::DYNAMIC_BLOCK
        | OrgSyntaxKind::COMMENT_BLOCK
        | OrgSyntaxKind::EXAMPLE_BLOCK
        | OrgSyntaxKind::EXPORT_BLOCK
        | OrgSyntaxKind::SRC_BLOCK
        | OrgSyntaxKind::VERSE_BLOCK => emit_block(&cast(syntax)?, src)?,
        OrgSyntaxKind::ERROR => {
            return Err(format!(
                "ERROR node at {}",
                u32::from(syntax.text_range().start())
            ));
        }
        other => {
            return Err(format!("unmapped syntax kind {other:?}"));
        }
    };
    Ok(Some(node))
}

fn is_skip(kind: OrgSyntaxKind) -> bool {
    matches!(
        kind,
        OrgSyntaxKind::HEADING
            | OrgSyntaxKind::TODO_KEYWORD
            | OrgSyntaxKind::PRIORITY
            | OrgSyntaxKind::HEADING_COMMENT
            | OrgSyntaxKind::HEADING_TITLE
            | OrgSyntaxKind::TAGS
            | OrgSyntaxKind::BULLET
            | OrgSyntaxKind::COUNTER
            | OrgSyntaxKind::CHECKBOX
            | OrgSyntaxKind::ITEM_TAG
            | OrgSyntaxKind::NODE_PROPERTY_KEY
            | OrgSyntaxKind::NODE_PROPERTY_VALUE
            | OrgSyntaxKind::DRAWER_NAME
            | OrgSyntaxKind::AFFILIATED_KEYWORDS
            | OrgSyntaxKind::KEYWORD_KEY
            | OrgSyntaxKind::KEYWORD_DUAL
            | OrgSyntaxKind::KEYWORD_VALUE
            | OrgSyntaxKind::CALL_NAME
            | OrgSyntaxKind::CALL_INSIDE_HEADER
            | OrgSyntaxKind::CALL_ARGUMENTS
            | OrgSyntaxKind::CALL_END_HEADER
            | OrgSyntaxKind::SRC_LANGUAGE
            | OrgSyntaxKind::SRC_PARAMETERS
            | OrgSyntaxKind::SRC_BODY
            | OrgSyntaxKind::LINK_PATH
            | OrgSyntaxKind::LINK_DESCRIPTION
            | OrgSyntaxKind::DATE
            | OrgSyntaxKind::TIME
            | OrgSyntaxKind::TIME_RANGE
            | OrgSyntaxKind::ROD
    )
}

fn cast<T: AstNode>(syntax: &SyntaxNode) -> Result<T, ProjectError> {
    T::cast(syntax.clone()).ok_or_else(|| format!("kind {:?} did not cast", syntax.kind()))
}

fn text_of(syntax: &SyntaxNode) -> String {
    syntax.text().to_string()
}

fn slice(src: &str, range: TextRange) -> &str {
    &src[usize::from(range.start())..usize::from(range.end())]
}

fn emit_child_nodes(syntax: &SyntaxNode, src: &str) -> Result<Vec<Value>, ProjectError> {
    let mut out = Vec::new();
    for child in syntax.children() {
        if let Some(value) = emit(&child, src)? {
            out.push(value);
        }
    }
    Ok(out)
}

fn emit_elements(syntax: &SyntaxNode, src: &str) -> Result<Vec<Value>, ProjectError> {
    let mut out = Vec::new();
    for child in syntax.children() {
        if Element::cast(child.clone()).is_none() {
            continue;
        }
        if let Some(value) = emit(&child, src)? {
            out.push(value);
        }
    }
    Ok(out)
}

fn finish(mut node: Node, syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    if let Some(prev) = syntax.prev_sibling()
        && prev.kind() == OrgSyntaxKind::AFFILIATED_KEYWORDS
    {
        let run: AffiliatedKeywords = cast(&prev)?;
        node.begin = u32::from(run.range().start());
        node.properties.insert(
            "post_affiliated".to_string(),
            json!(u32::from(syntax.text_range().start())),
        );
        node.affiliated = affiliated_map(&run, src)?;
    }
    Ok(node.into_value())
}

fn metadata_parent(syntax: &SyntaxNode) -> bool {
    syntax.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            OrgSyntaxKind::OUTLINE | OrgSyntaxKind::INLINE_TASK
        )
    })
}

/// Blanks the metadata slot owns sit after the node, as token siblings.
fn extended_end(syntax: &SyntaxNode) -> TextSize {
    let mut end = syntax.text_range().end();
    let Some(parent) = syntax.parent() else {
        return end;
    };
    let mut seen = false;
    for element in parent.children_with_tokens() {
        if !seen {
            if element.as_node().is_some_and(|node| *node == *syntax) {
                seen = true;
            }
            continue;
        }
        match element {
            orgcst::SyntaxElement::Node(_) => break,
            orgcst::SyntaxElement::Token(token) => {
                if matches!(
                    token.kind(),
                    OrgSyntaxKind::WHITESPACE | OrgSyntaxKind::INDENT | OrgSyntaxKind::NEWLINE
                ) {
                    end = token.text_range().end();
                } else {
                    break;
                }
            }
        }
    }
    end
}

fn emit_document(syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    let end = TextSize::try_from(src.len()).map_err(|_| "source longer than u32".to_string())?;
    let mut node = Node::new("org-data", TextSize::from(0), end);
    for child in syntax.children() {
        match child.kind() {
            OrgSyntaxKind::SECTION | OrgSyntaxKind::OUTLINE => {
                if let Some(value) = emit(&child, src)? {
                    node.children.push(value);
                }
            }
            other if other.is_node() => {
                return Err(format!("unexpected document child {other:?}"));
            }
            _ => {}
        }
    }
    Ok(node.into_value())
}

fn emit_zeroth_section(syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "section",
        syntax.text_range().start(),
        syntax.text_range().end(),
    );
    node.children = emit_elements(syntax, src)?;
    finish(node, syntax, src)
}

fn emit_outline(outline: &Outline, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("headline", outline.range().start(), outline.range().end());
    fill_headline(&mut node, &outline.heading(), src)?;
    if let Some(section) = section_value(outline, src)? {
        node.children.push(section);
    }
    for child in outline.children() {
        node.children.push(emit_outline(&child, src)?);
    }
    Ok(node.into_value())
}

fn section_value(outline: &Outline, src: &str) -> Result<Option<Value>, ProjectError> {
    let planning = outline.planning();
    let drawer = outline.property_drawer();
    let body = outline.section();
    if planning.is_none() && drawer.is_none() && body.is_none() {
        return Ok(None);
    }
    let mut begin: Option<TextSize> = None;
    let mut end: Option<TextSize> = None;
    let mut note = |start: TextSize, finish: TextSize| {
        begin = Some(begin.map_or(start, |have| have.min(start)));
        end = Some(end.map_or(finish, |have| have.max(finish)));
    };
    if let Some(planning) = &planning {
        note(planning.range().start(), extended_end(planning.syntax()));
    }
    if let Some(drawer) = &drawer {
        note(drawer.range().start(), extended_end(drawer.syntax()));
    }
    if let Some(body) = &body {
        note(body.range().start(), body.range().end());
    }
    let (begin, end) = match (begin, end) {
        (Some(begin), Some(end)) => (begin, end),
        _ => return Ok(None),
    };
    let mut node = Node::new("section", begin, end);
    if let Some(planning) = &planning {
        node.children.push(emit_planning(
            planning,
            extended_end(planning.syntax()),
            src,
        )?);
    }
    if let Some(drawer) = &drawer {
        node.children.push(emit_property_drawer(
            drawer,
            extended_end(drawer.syntax()),
            src,
        )?);
    }
    if let Some(body) = &body {
        node.children.extend(emit_elements(body.syntax(), src)?);
    }
    Ok(Some(node.into_value()))
}

fn emit_inline_task(task: &InlineTask, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("inlinetask", task.range().start(), task.range().end());
    fill_headline(&mut node, task, src)?;
    if let Some(planning) = task.planning() {
        node.children.push(emit_planning(
            &planning,
            extended_end(planning.syntax()),
            src,
        )?);
    }
    if let Some(drawer) = task.property_drawer() {
        node.children.push(emit_property_drawer(
            &drawer,
            extended_end(drawer.syntax()),
            src,
        )?);
    }
    node.children.extend(emit_elements(task.syntax(), src)?);
    finish(node, task.syntax(), src)
}

fn fill_headline<H: Headline>(
    node: &mut Node,
    headline: &H,
    src: &str,
) -> Result<(), ProjectError> {
    node.properties.insert(
        "level".to_string(),
        json!(u64::try_from(headline.level()).unwrap_or(0)),
    );
    node.properties.insert(
        "todo_keyword".to_string(),
        match headline.todo_keyword() {
            Some(todo) => json!(text_of(todo.syntax())),
            None => Value::Null,
        },
    );
    node.properties.insert(
        "priority".to_string(),
        match headline.priority() {
            Some(priority) => json!(i64::from(priority.value().rank())),
            None => Value::Null,
        },
    );
    let tags: Vec<String> = headline
        .tags()
        .map(|tags| tags.iter().map(|tag| tag.name().to_string()).collect())
        .unwrap_or_default();
    node.properties.insert("tags".to_string(), json!(tags));
    node.properties
        .insert("commented".to_string(), json!(headline.is_commented()));
    let title = headline.title();
    node.properties.insert(
        "title_begin".to_string(),
        json!(u32::from(title.range().start())),
    );
    node.properties.insert(
        "title_end".to_string(),
        json!(u32::from(title.range().end())),
    );
    let objects = emit_child_nodes(title.syntax(), src)?;
    if !objects.is_empty() {
        node.secondary
            .insert("title".to_string(), Value::Array(objects));
    }
    Ok(())
}

fn emit_planning(planning: &Planning, end: TextSize, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("planning", planning.range().start(), end);
    node.properties.insert(
        "scheduled".to_string(),
        stamp_value(planning.scheduled(), src)?,
    );
    node.properties.insert(
        "deadline".to_string(),
        stamp_value(planning.deadline(), src)?,
    );
    node.properties
        .insert("closed".to_string(), stamp_value(planning.closed(), src)?);
    Ok(node.into_value())
}

fn stamp_value(stamp: Option<orgcst::Timestamp>, src: &str) -> Result<Value, ProjectError> {
    match stamp {
        Some(stamp) => {
            emit(stamp.syntax(), src)?.ok_or_else(|| "timestamp was not emitted".to_string())
        }
        None => Ok(Value::Null),
    }
}

fn emit_property_drawer(
    drawer: &PropertyDrawer,
    end: TextSize,
    src: &str,
) -> Result<Value, ProjectError> {
    let mut node = Node::new("property-drawer", drawer.range().start(), end);
    for property in drawer.properties() {
        node.children.push(emit_node_property(&property, src)?);
    }
    finish(node, drawer.syntax(), src)
}

fn emit_node_property(property: &NodeProperty, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "node-property",
        property.range().start(),
        property.range().end(),
    );
    node.properties
        .insert("key".to_string(), json!(property.key()));
    node.properties.insert(
        "value".to_string(),
        match property.value() {
            Some(value) => json!(value),
            None => Value::Null,
        },
    );
    let _ = src;
    Ok(node.into_value())
}

fn emit_clock(clock: &Clock, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("clock", clock.range().start(), clock.range().end());
    node.properties
        .insert("value".to_string(), stamp_value(clock.timestamp(), src)?);
    node.properties.insert(
        "duration".to_string(),
        match clock.part_content_range(ClockPart::Duration) {
            Some(range) => json!(slice(src, range)),
            None => Value::Null,
        },
    );
    finish(node, clock.syntax(), src)
}

fn emit_table(table: &Table, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("table", table.range().start(), table.range().end());
    for row in table.rows() {
        node.children.push(emit_row(&row, src)?);
    }
    let formulas: Vec<String> = table
        .formulas()
        .filter(|keyword| keyword.key_eq("tblfm"))
        .map(|keyword| keyword.value().text().to_string())
        .collect();
    if !formulas.is_empty() {
        node.properties.insert("tblfm".to_string(), json!(formulas));
    }
    finish(node, table.syntax(), src)
}

fn emit_row(row: &orgcst::TableRow, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("table-row", row.range().start(), row.range().end());
    if row.is_rule() {
        node.properties.insert("rule".to_string(), json!(true));
    }
    for cell in row.cells() {
        node.children
            .push(emit_objects_node("table-cell", cell.syntax(), src)?);
    }
    Ok(node.into_value())
}

fn emit_citation(citation: &Citation, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("citation", citation.range().start(), citation.range().end());
    for reference in citation.references() {
        node.children
            .push(emit_citation_reference(&reference, src)?);
    }
    let prefix = citation
        .global_prefix_objects()
        .map(|object| emit(object.syntax(), src))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let suffix = citation
        .global_suffix_objects()
        .map(|object| emit(object.syntax(), src))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !prefix.is_empty() {
        node.secondary
            .insert("prefix".to_string(), Value::Array(prefix));
    }
    if !suffix.is_empty() {
        node.secondary
            .insert("suffix".to_string(), Value::Array(suffix));
    }
    finish(node, citation.syntax(), src)
}

fn emit_citation_reference(
    reference: &CitationReference,
    src: &str,
) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "citation-reference",
        reference.range().start(),
        reference.range().end(),
    );
    insert_affix(
        &mut node.secondary,
        "prefix",
        reference.syntax(),
        reference.part_content_range(CitationReferencePart::Prefix),
        src,
    )?;
    insert_affix(
        &mut node.secondary,
        "suffix",
        reference.syntax(),
        reference.part_content_range(CitationReferencePart::Suffix),
        src,
    )?;
    Ok(node.into_value())
}

fn insert_affix(
    secondary: &mut Map<String, Value>,
    key: &str,
    syntax: &SyntaxNode,
    span: Option<TextRange>,
    src: &str,
) -> Result<(), ProjectError> {
    let Some(span) = span.filter(|span| !span.is_empty()) else {
        return Ok(());
    };
    let mut objects = Vec::new();
    for child in syntax.children() {
        let range = child.text_range();
        if range.start() >= span.start()
            && range.end() <= span.end()
            && let Some(value) = emit(&child, src)?
        {
            objects.push(value);
        }
    }
    if !objects.is_empty() {
        secondary.insert(key.to_string(), Value::Array(objects));
    }
    Ok(())
}

fn emit_item(item: &Item, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("item", item.range().start(), item.range().end());
    node.properties
        .insert("bullet".to_string(), json!(item.bullet().text()));
    node.properties.insert(
        "checkbox".to_string(),
        match item.checkbox() {
            None => Value::Null,
            Some(CheckboxState::Unchecked) => json!("off"),
            Some(CheckboxState::Checked) => json!("on"),
            Some(CheckboxState::Partial) => json!("trans"),
        },
    );
    node.properties.insert(
        "counter".to_string(),
        match item.counter() {
            None => Value::Null,
            Some(counter) => json!(counter_string(&counter)),
        },
    );
    if let Some(tag) = item.tag() {
        let objects = emit_child_nodes(tag.syntax(), src)?;
        if !objects.is_empty() {
            node.secondary
                .insert("tag".to_string(), Value::Array(objects));
        }
    }
    node.children = item
        .elements()
        .map(|element| emit(element.syntax(), src))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    finish(node, item.syntax(), src)
}

fn counter_string(counter: &Counter) -> String {
    match counter.form() {
        Some(CounterForm::Number(number)) => number.to_string(),
        // Org stores [@c] and [@C] as the integer 3, not the written letter.
        Some(CounterForm::Letter(letter)) => {
            let index = u32::from(letter.to_ascii_uppercase()) - u32::from(b'@');
            index.to_string()
        }
        None => counter.written().to_string(),
    }
}

fn emit_link(link: &Link, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("link", link.range().start(), link.range().end());
    node.properties
        .insert("path".to_string(), json!(text_of(&link.path_node())));
    if link.shape() == LinkShape::Bracket {
        let raw = text_of(&link.path_node());
        let unescaped =
            orgcst::unescape_link_path(&orgcst::normalize_bracket_link_ws(&raw)).into_owned();
        node.properties
            .insert("unescaped".to_string(), json!(unescaped));
    }
    if matches!(link.shape(), LinkShape::Plain | LinkShape::Angle) {
        node.properties
            .insert("type".to_string(), json!(link.type_name()));
    }
    if let Some(description) = link.description() {
        node.properties.insert(
            "description_begin".to_string(),
            json!(u32::from(description.range().start())),
        );
        node.properties.insert(
            "description_end".to_string(),
            json!(u32::from(description.range().end())),
        );
        node.children = emit_child_nodes(description.syntax(), src)?;
    }
    finish(node, link.syntax(), src)
}

fn emit_keyword(keyword: &Keyword, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("keyword", keyword.range().start(), keyword.range().end());
    node.properties
        .insert("key".to_string(), json!(keyword.key()));
    node.properties.insert(
        "value".to_string(),
        json!(keyword.value().text().to_string()),
    );
    finish(node, keyword.syntax(), src)
}

fn emit_babel(call: &BabelCall, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("babel-call", call.range().start(), call.range().end());
    insert_part(&mut node, "name", call, BabelCallPart::Name, src);
    insert_part(
        &mut node,
        "inside_header",
        call,
        BabelCallPart::InsideHeader,
        src,
    );
    insert_part(&mut node, "arguments", call, BabelCallPart::Arguments, src);
    insert_part(&mut node, "end_header", call, BabelCallPart::EndHeader, src);
    finish(node, call.syntax(), src)
}

fn emit_inline_babel(call: &InlineBabelCall, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "inline-babel-call",
        call.range().start(),
        call.range().end(),
    );
    insert_part(&mut node, "name", call, InlineBabelCallPart::Name, src);
    insert_part(
        &mut node,
        "inside_header",
        call,
        InlineBabelCallPart::InsideHeader,
        src,
    );
    insert_part(
        &mut node,
        "arguments",
        call,
        InlineBabelCallPart::Arguments,
        src,
    );
    insert_part(
        &mut node,
        "end_header",
        call,
        InlineBabelCallPart::EndHeader,
        src,
    );
    finish(node, call.syntax(), src)
}

fn emit_inline_src(block: &InlineSrcBlock, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "inline-src-block",
        block.range().start(),
        block.range().end(),
    );
    insert_part(&mut node, "language", block, InlineSrcPart::Language, src);
    insert_part(
        &mut node,
        "parameters",
        block,
        InlineSrcPart::Parameters,
        src,
    );
    insert_part(&mut node, "body", block, InlineSrcPart::Body, src);
    finish(node, block.syntax(), src)
}

fn insert_part<P>(node: &mut Node, key: &str, owner: &P, part: P::Part, src: &str)
where
    P: HasParts,
{
    let value = match owner.part_content_range(part) {
        Some(range) => json!(slice(src, range)),
        None => Value::Null,
    };
    node.properties.insert(key.to_string(), value);
}

fn emit_timestamp(syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "timestamp",
        syntax.text_range().start(),
        syntax.text_range().end(),
    );
    node.properties
        .insert("raw".to_string(), json!(text_of(syntax)));
    node.children = emit_child_nodes(syntax, src)?;
    finish(node, syntax, src)
}

fn emit_markup(markup: &Markup, src: &str) -> Result<Value, ProjectError> {
    let ty = match markup.kind() {
        MarkupKind::Bold => "bold",
        MarkupKind::Italic => "italic",
        MarkupKind::Underline => "underline",
        MarkupKind::StrikeThrough => "strike-through",
        MarkupKind::Code => "code",
        MarkupKind::Verbatim => "verbatim",
    };
    let mut node = Node::new(ty, markup.range().start(), markup.range().end());
    match markup.body() {
        MarkupBody::Emphasis(emphasis) => {
            node.children = emit_child_nodes(emphasis.syntax(), src)?;
        }
        MarkupBody::Raw(_) => {
            node.properties
                .insert("value".to_string(), json!(text_of(markup.syntax())));
        }
    }
    finish(node, markup.syntax(), src)
}

fn emit_objects_node(
    ty: &'static str,
    syntax: &SyntaxNode,
    src: &str,
) -> Result<Value, ProjectError> {
    let mut node = Node::new(ty, syntax.text_range().start(), syntax.text_range().end());
    node.children = emit_child_nodes(syntax, src)?;
    finish(node, syntax, src)
}

fn leaf(ty: &'static str, syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(ty, syntax.text_range().start(), syntax.text_range().end());
    node.properties
        .insert("value".to_string(), json!(text_of(syntax)));
    finish(node, syntax, src)
}

fn span_only(ty: &'static str, syntax: &SyntaxNode, src: &str) -> Result<Value, ProjectError> {
    let node = Node::new(ty, syntax.text_range().start(), syntax.text_range().end());
    finish(node, syntax, src)
}

fn emit_footnote_def(definition: &FootnoteDefinition, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "footnote-definition",
        definition.range().start(),
        definition.range().end(),
    );
    node.properties.insert(
        "label".to_string(),
        match definition.part_content_range(FootnotePart::Label) {
            Some(range) => json!(slice(src, range)),
            None => Value::Null,
        },
    );
    node.children = definition
        .elements()
        .map(|element| emit(element.syntax(), src))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    finish(node, definition.syntax(), src)
}

fn emit_footnote_ref(reference: &FootnoteReference, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new(
        "footnote-reference",
        reference.range().start(),
        reference.range().end(),
    );
    node.properties.insert(
        "label".to_string(),
        match reference.label() {
            Some(range) => json!(slice(src, range)),
            None => Value::Null,
        },
    );
    node.children = emit_child_nodes(reference.syntax(), src)?;
    finish(node, reference.syntax(), src)
}

fn emit_drawer(drawer: &Drawer, src: &str) -> Result<Value, ProjectError> {
    let mut node = Node::new("drawer", drawer.range().start(), drawer.range().end());
    node.properties
        .insert("name".to_string(), json!(drawer.name()));
    node.children = drawer
        .elements()
        .map(|element| emit(element.syntax(), src))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    finish(node, drawer.syntax(), src)
}

fn emit_block(block: &Block, src: &str) -> Result<Value, ProjectError> {
    let ty = match block.kind() {
        BlockKind::Src => "src-block",
        BlockKind::Example => "example-block",
        BlockKind::Export => "export-block",
        BlockKind::Comment => "comment-block",
        BlockKind::Verse => "verse-block",
        BlockKind::Quote => "quote-block",
        BlockKind::Center => "center-block",
        BlockKind::Special => "special-block",
        BlockKind::Dynamic => "dynamic-block",
    };
    let mut node = Node::new(ty, block.range().start(), block.range().end());
    if matches!(block.kind(), BlockKind::Special | BlockKind::Dynamic)
        && let Some(name) = block.name()
    {
        node.properties.insert("name".to_string(), json!(name));
    }
    if block.kind() == BlockKind::Src
        && let Some(language) = block.language()
    {
        node.properties
            .insert("language".to_string(), json!(language));
    }
    node.children = match block.body() {
        BlockBody::Verbatim(_) => Vec::new(),
        BlockBody::Objects(verse) => emit_child_nodes(verse.syntax(), src)?,
        BlockBody::Elements(container) => container
            .elements()
            .map(|element| emit(element.syntax(), src))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect(),
    };
    finish(node, block.syntax(), src)
}

fn affiliated_map(run: &AffiliatedKeywords, src: &str) -> Result<Map<String, Value>, ProjectError> {
    let mut captions: Vec<Value> = Vec::new();
    let mut caption_duals: Vec<Value> = Vec::new();
    let mut saw_caption_dual = false;
    let mut multiple: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut single: BTreeMap<String, String> = BTreeMap::new();
    let mut single_dual: BTreeMap<String, String> = BTreeMap::new();
    for keyword in run.keywords() {
        let key = keyword.canonical_key().to_ascii_lowercase();
        if keyword.is_parsed() {
            captions.push(Value::Array(emit_child_nodes(
                keyword.value().syntax(),
                src,
            )?));
            if let Some(dual) = keyword.dual_node() {
                saw_caption_dual = true;
                caption_duals.push(Value::Array(emit_child_nodes(dual.syntax(), src)?));
            } else {
                caption_duals.push(Value::Null);
            }
        } else if keyword.is_multiple() {
            multiple
                .entry(key)
                .or_default()
                .push(keyword.value().text().to_string());
        } else {
            single.insert(key.clone(), keyword.value().text().to_string());
            let dual_key = format!("{key}_dual");
            if let Some(dual) = keyword.dual() {
                single_dual.insert(dual_key, dual);
            } else {
                single_dual.remove(&dual_key);
            }
        }
    }
    let mut map = Map::new();
    if !captions.is_empty() {
        map.insert("caption".to_string(), Value::Array(captions));
    }
    if saw_caption_dual {
        map.insert("caption_dual".to_string(), Value::Array(caption_duals));
    }
    for (key, values) in multiple {
        map.insert(key, json!(values));
    }
    for (key, value) in single {
        map.insert(key, json!(value));
    }
    for (key, value) in single_dual {
        map.insert(key, json!(value));
    }
    Ok(map)
}
