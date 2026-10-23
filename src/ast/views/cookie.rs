//! Statistics cookies (`[1/3]`, `[33%]`) and the item or headline each one
//! counts for.
//!
//! The view does not apply `org-hierarchical-todo-statistics` or
//! `org-checkbox-hierarchical-statistics`; a caller that honours those options
//! decides what to count.

use super::list::Item;
use super::task::Task;
use crate::ast::AstNode;
use crate::ast::layout::{CookieLayout, cookie_layout, text_in, with_node_text};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};

crate::ast_node!(
    /// A statistics cookie object: `[done/total]` or `[n%]`.
    StatisticsCookie,
    OrgSyntaxKind::STATISTICS_COOKIE
);

/// Whether a cookie is written as a fraction or a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CookieShape {
    /// `[done/total]`.
    Fraction,
    /// `[n%]`.
    Percent,
}

/// The numbers inside a cookie. Each one may be missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CookieValue {
    /// `[/]`.
    Empty,
    /// `[done/total]`, with either number possibly missing, as in `[2/]`.
    Fraction {
        /// Number before the slash: items or entries done.
        done: Option<u32>,
        /// Number after the slash: items or entries counted.
        total: Option<u32>,
    },
    /// `[n%]`; `None` for `[%]`.
    Percent {
        /// The percentage.
        value: Option<u32>,
    },
}

/// What a headline cookie counts, from the entry's `COOKIE_DATA` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CookieData {
    /// Child entries with a TODO keyword. The default.
    Todo,
    /// Checkboxes.
    Checkbox,
}

impl StatisticsCookie {
    /// Whether this cookie is a fraction or a percentage.
    #[must_use]
    pub fn shape(&self) -> CookieShape {
        match layout(self) {
            CookieLayout::Fraction { .. } => CookieShape::Fraction,
            CookieLayout::Percent { .. } => CookieShape::Percent,
        }
    }

    /// Digits inside the brackets. `[/]` is [`CookieValue::Empty`].
    #[must_use]
    pub fn value(&self) -> CookieValue {
        match layout(self) {
            CookieLayout::Fraction {
                done: None,
                total: None,
            } => CookieValue::Empty,
            CookieLayout::Fraction { done, total } => CookieValue::Fraction {
                done: done.and_then(|r| parse_u32(self.syntax(), r)),
                total: total.and_then(|r| parse_u32(self.syntax(), r)),
            },
            CookieLayout::Percent { value } => CookieValue::Percent {
                value: value.and_then(|r| parse_u32(self.syntax(), r)),
            },
        }
    }

    /// The list item this cookie counts for: its nearest `ITEM` ancestor.
    ///
    /// An item takes precedence over the headline the item sits under.
    #[must_use]
    pub fn host_item(&self) -> Option<Item> {
        climb(self.syntax(), |n| Item::cast(n.clone()))
    }

    /// The headline this cookie counts for: the outline or inline task whose
    /// title holds it, unless it is inside a list item.
    ///
    /// A cookie in an entry's body counts for nothing. A closed inline task
    /// holds its body on the same node as its title, so the walk looks for the
    /// title rather than for any `INLINE_TASK` ancestor.
    #[must_use]
    pub fn host_task(&self) -> Option<Task> {
        if self.host_item().is_some() {
            return None;
        }
        climb(self.syntax(), |n| {
            if n.kind() != OrgSyntaxKind::HEADING_TITLE {
                return None;
            }
            n.parent().and_then(|p| match p.kind() {
                OrgSyntaxKind::HEADING => {
                    super::heading::Heading::cast(p).map(|h| Task::from(h.outline()))
                }
                OrgSyntaxKind::INLINE_TASK => {
                    super::inline_task::InlineTask::cast(p).map(Task::from)
                }
                _ => None,
            })
        })
    }

    /// What this headline cookie counts, from `COOKIE_DATA` in the
    /// [`Self::host_task`]'s own property drawer.
    ///
    /// As in Org, the value is a list of words: if one of them is `checkbox`
    /// (ASCII caseless) the cookie counts checkboxes, and otherwise it counts
    /// TODO entries. Other words, such as `recursive`, do not change the
    /// answer. `None` when the cookie has no [`Self::host_task`].
    #[must_use]
    pub fn cookie_data(&self) -> Option<CookieData> {
        let task = self.host_task()?;
        let Some(vnode) = task
            .property_drawer()
            .and_then(|p| p.get("COOKIE_DATA"))
            .and_then(|np| np.value_node())
        else {
            return Some(CookieData::Todo);
        };
        with_node_text(&vnode, |s| {
            if s.split_ascii_whitespace()
                .any(|w| w.eq_ignore_ascii_case("checkbox"))
            {
                Some(CookieData::Checkbox)
            } else {
                Some(CookieData::Todo)
            }
        })
    }
}

fn layout(cookie: &StatisticsCookie) -> CookieLayout {
    cookie_layout(cookie.syntax())
        .expect("grammar: STATISTICS_COOKIE is a bracketed fraction or percent (forms/cookie.rs)")
}

fn parse_u32(node: &SyntaxNode, range: crate::TextRange) -> Option<u32> {
    text_in(node, range).parse().ok()
}

fn climb<T>(start: &SyntaxNode, mut pred: impl FnMut(&SyntaxNode) -> Option<T>) -> Option<T> {
    let mut n = start.clone();
    while let Some(p) = n.parent() {
        if let Some(hit) = pred(&p) {
            return Some(hit);
        }
        n = p;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Outline;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;

    fn cookie(src: &str) -> (crate::Document, StatisticsCookie) {
        let doc = parse(src);
        let c = StatisticsCookie::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::STATISTICS_COOKIE)
                .into_iter()
                .next()
                .expect("cookie"),
        )
        .expect("cast");
        (doc, c)
    }

    #[test]
    fn cookie_shapes_and_values() {
        let (_, c) = cookie("[/]\n");
        assert_eq!(c.shape(), CookieShape::Fraction);
        assert_eq!(c.value(), CookieValue::Empty);
        let (_, c) = cookie("[1/3]\n");
        assert_eq!(
            c.value(),
            CookieValue::Fraction {
                done: Some(1),
                total: Some(3)
            }
        );
        let (_, c) = cookie("[50%]\n");
        assert_eq!(c.shape(), CookieShape::Percent);
        assert_eq!(c.value(), CookieValue::Percent { value: Some(50) });
        let (_, c) = cookie("[2/]\n");
        assert_eq!(
            c.value(),
            CookieValue::Fraction {
                done: Some(2),
                total: None
            }
        );
        let (_, c) = cookie("[/7]\n");
        assert_eq!(
            c.value(),
            CookieValue::Fraction {
                done: None,
                total: Some(7)
            }
        );
        let (_, c) = cookie("[%]\n");
        assert_eq!(c.shape(), CookieShape::Percent);
        assert_eq!(c.value(), CookieValue::Percent { value: None });
    }

    #[test]
    fn cookie_in_table_cell_is_not_carved() {
        let doc = parse("| [1/2] |\n");
        assert!(
            nodes_of_kind(doc.root(), OrgSyntaxKind::STATISTICS_COOKIE).is_empty(),
            "TABLE_CELL remq STATISTICS_COOKIE"
        );
    }

    #[test]
    fn host_item_wins_over_heading() {
        let src = "* H [1/2]\n- [ ] a [0/1]\n";
        let doc = parse(src);
        let cookies: Vec<_> = nodes_of_kind(doc.root(), OrgSyntaxKind::STATISTICS_COOKIE)
            .into_iter()
            .filter_map(StatisticsCookie::cast)
            .collect();
        assert_eq!(cookies.len(), 2);
        let heading_c = cookies
            .iter()
            .find(|c| c.host_task().is_some())
            .expect("heading cookie");
        assert!(heading_c.host_item().is_none());
        let item_c = cookies
            .iter()
            .find(|c| c.host_item().is_some())
            .expect("item cookie");
        assert!(item_c.host_task().is_none());
    }

    #[test]
    fn paragraph_cookie_is_neither_host() {
        let (_, c) = cookie("see [1/2]\n");
        assert!(c.host_item().is_none());
        assert!(c.host_task().is_none());
        assert!(c.cookie_data().is_none());
    }

    #[test]
    fn inline_task_title_cookie_is_task() {
        let src = "*************** T [1/2]\n";
        let doc = parse(src);
        let c = StatisticsCookie::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::STATISTICS_COOKIE)
                .into_iter()
                .next()
                .expect("cookie"),
        )
        .expect("cast");
        assert!(matches!(c.host_task(), Some(Task::InlineTask(_))));
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));
    }

    #[test]
    fn inline_task_body_cookie_is_neither_host() {
        let src = "*************** TASK\nsee [1/2]\n*************** END\n";
        let (_, c) = cookie(src);
        assert!(c.host_item().is_none());
        assert!(c.host_task().is_none());
        assert!(c.cookie_data().is_none());
    }

    #[test]
    fn cookie_data_mapping() {
        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: checkbox\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Checkbox));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: todo\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: junk\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: checkbox recursive\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Checkbox));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: recursive todo\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA:\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: TODO\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Todo));

        let src = "* H [1/2]\n:PROPERTIES:\n:COOKIE_DATA: CheckBox\n:END:\n";
        let (_, c) = cookie(src);
        assert_eq!(c.cookie_data(), Some(CookieData::Checkbox));
    }

    #[test]
    fn checkbox_walks_direct_includes_sublist_misses_child_outline() {
        let src = "* Parent [1/2]\n- [ ] a\n  - [ ] nested\n** Child\n- [ ] other\n";
        let doc = parse(src);
        let outline = Outline::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::OUTLINE)
                .into_iter()
                .next()
                .expect("parent outline"),
        )
        .expect("cast");
        let direct: Vec<_> = outline.direct_checkbox_items().collect();
        assert_eq!(direct.len(), 2, "section list plus its sublist");
        let desc: Vec<_> = outline.descendant_checkbox_items().collect();
        assert_eq!(desc.len(), 3, "plus the child outline's item");
        assert_eq!(outline.children().count(), 1);
    }

    #[test]
    fn item_direct_child_checkboxes_not_deeper() {
        let src = "- [ ] outer [0/1]\n  - [ ] mid\n    - [ ] deep\n";
        let doc = parse(src);
        let item = Item::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
                .into_iter()
                .next()
                .expect("outer"),
        )
        .expect("cast");
        let kids: Vec<_> = item.direct_child_checkbox_items().collect();
        assert_eq!(kids.len(), 1);
        let items: Vec<_> = nodes_of_kind(doc.root(), OrgSyntaxKind::ITEM)
            .into_iter()
            .filter_map(Item::cast)
            .collect();
        assert_eq!(items.len(), 3, "outer / mid / deep");
        assert_eq!(
            kids[0].syntax(),
            items[1].syntax(),
            "direct child is mid, not deep"
        );
    }
}
