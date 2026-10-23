//! [`Task`]: an outline or an inline task, the two constructs that carry a
//! headline with its planning and properties.
//!
//! `Task` overrides [`Headline::star_syntax`] because its [`AstNode::syntax`]
//! is the `OUTLINE` node, not the `HEADING` that holds the stars. A bare
//! `HEADING` does not cast.

use super::heading::{HEADLINE_PARTS, HeadingPart, Headline, headline_part_span};
use super::inline_task::InlineTask;
use super::outline::Outline;
use super::planning::Planning;
use super::property::PropertyDrawer;
use crate::TextRange;
use crate::ast::{AstNode, HasParts, InsertPoint, PartSpan};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

/// An [`Outline`] or an [`InlineTask`].
///
/// Use it where planning, properties and log entries matter whichever kind
/// of headline carries them, as [`crate::Document::property`] does. An inline
/// task without an `END` line is still a task; it has no planning and no
/// property drawer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Task {
    /// A headline that starts a subtree.
    Outline(Outline),
    /// An inline task, with or without an `END` line.
    InlineTask(InlineTask),
}

impl AstNode for Task {
    fn can_cast(kind: OrgSyntaxKind) -> bool {
        kind == OrgSyntaxKind::OUTLINE || kind == OrgSyntaxKind::INLINE_TASK
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            OrgSyntaxKind::OUTLINE => Outline::cast(node).map(Self::Outline),
            OrgSyntaxKind::INLINE_TASK => InlineTask::cast(node).map(Self::InlineTask),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Outline(o) => o.syntax(),
            Self::InlineTask(t) => t.syntax(),
        }
    }
}

impl crate::ast::Covering for Task {}

impl From<Outline> for Task {
    fn from(outline: Outline) -> Self {
        Self::Outline(outline)
    }
}

impl From<InlineTask> for Task {
    fn from(task: InlineTask) -> Self {
        Self::InlineTask(task)
    }
}

impl From<&Outline> for Task {
    fn from(outline: &Outline) -> Self {
        Self::Outline(outline.clone())
    }
}

impl From<&InlineTask> for Task {
    fn from(task: &InlineTask) -> Self {
        Self::InlineTask(task.clone())
    }
}

impl From<&Task> for Task {
    fn from(task: &Task) -> Self {
        task.clone()
    }
}

impl Task {
    /// The outline, when this task is one.
    #[must_use]
    pub fn as_outline(&self) -> Option<Outline> {
        match self {
            Self::Outline(o) => Some(o.clone()),
            Self::InlineTask(_) => None,
        }
    }

    /// The planning line, if any. Always `None` for an inline task without an
    /// `END` line.
    #[must_use]
    pub fn planning(&self) -> Option<Planning> {
        match self {
            Self::Outline(o) => o.planning(),
            Self::InlineTask(t) => t.planning(),
        }
    }

    /// The property drawer, if any.
    ///
    /// Lines inside the drawer are [`PropertyDrawer::properties`].
    #[must_use]
    pub fn property_drawer(&self) -> Option<PropertyDrawer> {
        match self {
            Self::Outline(o) => o.property_drawer(),
            Self::InlineTask(t) => t.property_drawer(),
        }
    }

    /// The entry's own text: for an outline, the heading through its section,
    /// without child outlines or trailing blank lines; for an inline task, the
    /// whole node.
    #[must_use]
    pub fn entry_range(&self) -> TextRange {
        match self {
            Self::Outline(o) => o.entry_range(),
            Self::InlineTask(t) => t.entry_range(),
        }
    }

    /// Where a planning line is written. See
    /// [`Outline::planning_insert_point`] and
    /// [`InlineTask::planning_insert_point`].
    #[must_use]
    pub fn planning_insert_point(&self) -> InsertPoint {
        match self {
            Self::Outline(o) => o.planning_insert_point(),
            Self::InlineTask(t) => t.planning_insert_point(),
        }
    }

    /// Where a property drawer is written. See
    /// [`Outline::property_drawer_insert_point`] and
    /// [`InlineTask::property_drawer_insert_point`].
    #[must_use]
    pub fn property_drawer_insert_point(&self) -> InsertPoint {
        match self {
            Self::Outline(o) => o.property_drawer_insert_point(),
            Self::InlineTask(t) => t.property_drawer_insert_point(),
        }
    }

    /// Where a new drawer such as `LOGBOOK` is written. See
    /// [`Outline::log_drawer_insert_point`] and
    /// [`InlineTask::log_drawer_insert_point`].
    #[must_use]
    pub fn log_drawer_insert_point(&self) -> InsertPoint {
        match self {
            Self::Outline(o) => o.log_drawer_insert_point(),
            Self::InlineTask(t) => t.log_drawer_insert_point(),
        }
    }

    /// Where a log entry is written when entries run newest first. See
    /// [`Outline::log_item_insert_point`] and
    /// [`InlineTask::log_item_insert_point`].
    #[must_use]
    pub fn log_item_insert_point(&self) -> InsertPoint {
        match self {
            Self::Outline(o) => o.log_item_insert_point(),
            Self::InlineTask(t) => t.log_item_insert_point(),
        }
    }
}

impl HasParts for Task {
    type Part = HeadingPart;
    const PARTS: &'static [HeadingPart] = HEADLINE_PARTS;

    /// Parts of the headline. An offset in the section or in a child outline
    /// belongs to no part.
    fn part_span(&self, part: HeadingPart) -> PartSpan {
        headline_part_span(&self.star_syntax(), part)
    }
}

impl Headline for Task {
    fn star_syntax(&self) -> SyntaxNode {
        match self {
            Self::Outline(o) => o.heading().syntax().clone(),
            Self::InlineTask(t) => t.syntax().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{nodes_of_kind, source_slice};

    fn first_inline_task(doc: &crate::Document) -> InlineTask {
        InlineTask::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::INLINE_TASK)
                .into_iter()
                .next()
                .expect("inline task"),
        )
        .expect("cast")
    }

    #[test]
    fn cast_outline_and_inline_task_not_heading() {
        let doc = parse("* H\n*************** T\n*************** END\n");
        let outline = doc.root_outlines().next().unwrap();
        let heading = outline.heading();

        let from_outline = Task::cast(outline.syntax().clone()).unwrap();
        assert!(matches!(from_outline, Task::Outline(_)));
        assert!(Task::cast(heading.syntax().clone()).is_none());
        assert!(!Task::can_cast(OrgSyntaxKind::HEADING));
        assert!(Task::can_cast(OrgSyntaxKind::OUTLINE));
        assert!(Task::can_cast(OrgSyntaxKind::INLINE_TASK));

        let it = first_inline_task(&doc);
        let from_task = Task::cast(it.syntax().clone()).unwrap();
        assert!(matches!(from_task, Task::InlineTask(_)));
    }

    #[test]
    fn headline_answers_todo_and_title_on_both_arms() {
        let doc = parse("* TODO Alpha\n*************** TODO Beta\n*************** END\n");
        let outline = doc.root_outlines().next().unwrap();
        let task = Task::from(&outline);
        assert_eq!(
            task.todo_keyword().unwrap().text().to_string(),
            outline.heading().todo_keyword().unwrap().text().to_string()
        );
        assert_eq!(
            task.title().text().to_string(),
            outline.heading().title().text().to_string()
        );
        assert_eq!(task.level(), 1);

        let it = first_inline_task(&doc);
        let task = Task::from(&it);
        assert_eq!(
            task.todo_keyword().unwrap().text().to_string(),
            it.todo_keyword().unwrap().text().to_string()
        );
        assert_eq!(
            source_slice(&doc, task.title().range()).trim(),
            source_slice(&doc, it.title().range()).trim()
        );
        assert_eq!(
            task.title().text().to_string(),
            it.title().text().to_string()
        );
    }

    #[test]
    fn dests_match_outline_and_inline_task() {
        let src = "* H\nSCHEDULED: <2024-01-01 Mon>\n:PROPERTIES:\n:FOO: 1\n:END:\nbody\n";
        let doc = parse(src);
        let o = doc.root_outlines().next().unwrap();
        let t = Task::from(&o);
        assert_eq!(t.planning_insert_point(), o.planning_insert_point());
        assert_eq!(
            t.property_drawer_insert_point(),
            o.property_drawer_insert_point()
        );
        assert_eq!(t.log_drawer_insert_point(), o.log_drawer_insert_point());
        assert_eq!(t.log_item_insert_point(), o.log_item_insert_point());
        assert_eq!(
            t.planning().as_ref().map(AstNode::syntax),
            o.planning().as_ref().map(AstNode::syntax)
        );
        assert_eq!(
            t.property_drawer().as_ref().map(AstNode::syntax),
            o.property_drawer().as_ref().map(AstNode::syntax)
        );
        assert_eq!(t.entry_range(), o.entry_range());
        assert!(t.as_outline().is_some());

        let src = "*************** T\nSCHEDULED: <2024-01-01 Mon>\n:PROPERTIES:\n:ID: x\n:END:\npara\n*************** END\n";
        let doc = parse(src);
        let it = first_inline_task(&doc);
        let t = Task::from(&it);
        assert_eq!(t.planning_insert_point(), it.planning_insert_point());
        assert_eq!(
            t.property_drawer_insert_point(),
            it.property_drawer_insert_point()
        );
        assert_eq!(t.log_drawer_insert_point(), it.log_drawer_insert_point());
        assert_eq!(t.log_item_insert_point(), it.log_item_insert_point());
        assert_eq!(t.entry_range(), it.entry_range());
        assert!(t.as_outline().is_none());

        let src = "*************** T\n\n";
        let doc = parse(src);
        let it = first_inline_task(&doc);
        let t = Task::from(&it);
        assert!(t.planning().is_none());
        assert_eq!(t.log_item_insert_point(), it.log_item_insert_point());
        assert_eq!(t.entry_range(), it.range());
    }

    #[test]
    fn has_parts_on_task_delegates_to_star_line() {
        let doc = parse("* TODO Title :a:\n*************** DONE T\n*************** END\n");
        let o = doc.root_outlines().next().unwrap();
        let t = Task::from(&o);
        crate::ast::check_part_contract(&t);
        assert_eq!(
            t.part_range(HeadingPart::Todo),
            o.heading().part_range(HeadingPart::Todo)
        );

        let it = first_inline_task(&doc);
        let t = Task::from(&it);
        crate::ast::check_part_contract(&t);
        assert_eq!(
            t.part_range(HeadingPart::Todo),
            it.part_range(HeadingPart::Todo)
        );
    }
}
