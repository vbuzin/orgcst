//! Headline layout: priority cookie slicer, tag runs, post-stars word.

use super::{first_line, first_word_range, is_word_break, word_run};
use crate::syntax::{OrgSyntaxKind, SyntaxNode};
use crate::{TextRange, TextSize};

/// Absolute range of the value text inside a `PRIORITY` cookie (`[#X]`).
///
/// Grammar shape: `L_BRACKET` `HASH` `TEXT` `R_BRACKET`. Missing pieces yield
/// `None`.
#[must_use]
pub(crate) fn priority_value_range(priority: &SyntaxNode) -> Option<TextRange> {
    if priority.kind() != OrgSyntaxKind::PRIORITY {
        return None;
    }
    let tokens: Vec<_> = priority.children_with_tokens().collect();
    if tokens.len() != 4 {
        return None;
    }
    if tokens[0].kind() != OrgSyntaxKind::L_BRACKET
        || tokens[1].kind() != OrgSyntaxKind::HASH
        || tokens[2].kind() != OrgSyntaxKind::TEXT
        || tokens[3].kind() != OrgSyntaxKind::R_BRACKET
    {
        return None;
    }
    Some(tokens[2].text_range())
}

/// First whitespace-delimited word after the star run on a headline line.
///
/// This is the candidate `forms/heading.rs::parse_todo` looks up. A carved
/// `TODO_KEYWORD` *is* this word; when the keyword is absent the word is
/// still here (title text, a priority cookie, or a `COMMENT` marker).
#[must_use]
pub(crate) fn post_stars_word_range(syntax: &SyntaxNode) -> Option<TextRange> {
    let mut past_stars = false;
    let mut elems = first_line(syntax).filter(move |el| {
        if !past_stars {
            if el.kind() == OrgSyntaxKind::STAR {
                return false;
            }
            past_stars = true;
        }
        true
    });
    let first = loop {
        let el = elems.next()?;
        if !is_word_break(&el) {
            break el;
        }
    };
    if let Some(node) = first.as_node() {
        if node.kind() == OrgSyntaxKind::HEADING_TITLE {
            return first_word_range(node);
        }
        return Some(node.text_range());
    }
    word_run(std::iter::once(first).chain(elems))
}

/// Individual tag names inside a `TAGS` node, in source order.
///
/// Splits on `COLON` and concatenates everything between. Empty fragments
/// (`::`) are skipped. The grammar only emits `COLON` and tag atoms here
/// (`forms/heading.rs::parse_tags`); this is a layout read, not a second parse.
pub(crate) fn tag_runs(node: &SyntaxNode) -> impl Iterator<Item = (TextRange, String)> {
    let mut current: Option<(TextSize, String)> = None;
    let mut out = Vec::new();
    for el in node.children_with_tokens() {
        let Some(tok) = el.into_token() else {
            continue;
        };
        if tok.kind() == OrgSyntaxKind::COLON {
            if let Some((start, name)) = current.take()
                && !name.is_empty()
            {
                out.push((TextRange::new(start, tok.text_range().start()), name));
            }
            current = Some((tok.text_range().end(), String::new()));
            continue;
        }
        if let Some((_, ref mut name)) = current {
            name.push_str(tok.text());
        }
    }
    out.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::layout::{first_kind, synth};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;

    #[test]
    fn priority_value_range_happy() {
        let pr = first_kind("* [#A] T\n", OrgSyntaxKind::PRIORITY);
        let r = priority_value_range(&pr).expect("value");
        let doc = parse("* [#A] T\n");
        assert_eq!(&doc.source()[r], "A");
    }
    #[test]
    fn priority_value_numeric() {
        let pr = first_kind("* [#10] T\n", OrgSyntaxKind::PRIORITY);
        let r = priority_value_range(&pr).expect("value");
        let doc = parse("* [#10] T\n");
        assert_eq!(&doc.source()[r], "10");
    }
    #[test]
    fn priority_value_rejects_wrong_kind() {
        let p = first_kind("paragraph\n", OrgSyntaxKind::PARAGRAPH);
        assert!(priority_value_range(&p).is_none());
    }
    #[test]
    fn priority_value_range_rejects_degenerate_shapes() {
        // Too few children: `[#]` with no value token.
        let no_value = synth(
            OrgSyntaxKind::PRIORITY,
            &[
                (OrgSyntaxKind::L_BRACKET, "["),
                (OrgSyntaxKind::HASH, "#"),
                (OrgSyntaxKind::R_BRACKET, "]"),
            ],
        );
        assert!(priority_value_range(&no_value).is_none(), "no value token");
        // Right arity, wrong order.
        let swapped = synth(
            OrgSyntaxKind::PRIORITY,
            &[
                (OrgSyntaxKind::L_BRACKET, "["),
                (OrgSyntaxKind::TEXT, "A"),
                (OrgSyntaxKind::HASH, "#"),
                (OrgSyntaxKind::R_BRACKET, "]"),
            ],
        );
        assert!(priority_value_range(&swapped).is_none(), "hash after value");
        // Unclosed: three tokens, no R_BRACKET.
        let unclosed = synth(
            OrgSyntaxKind::PRIORITY,
            &[
                (OrgSyntaxKind::L_BRACKET, "["),
                (OrgSyntaxKind::HASH, "#"),
                (OrgSyntaxKind::TEXT, "A"),
            ],
        );
        assert!(priority_value_range(&unclosed).is_none(), "unclosed cookie");
    }

    #[test]
    fn tag_runs_splits_on_colon() {
        let tags = first_kind("* H :a_b:@c:\n", OrgSyntaxKind::TAGS);
        let names: Vec<_> = tag_runs(&tags).map(|(_, n)| n).collect();
        assert_eq!(names, ["a_b", "@c"]);
    }

    #[test]
    fn tag_runs_skips_empty_fragments() {
        let tags = first_kind("* H :a::b:\n", OrgSyntaxKind::TAGS);
        let names: Vec<_> = tag_runs(&tags).map(|(_, n)| n).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn post_stars_word_uncarved_is_title_first_word() {
        let h = first_kind("* WAITING Title\n", OrgSyntaxKind::HEADING);
        let r = post_stars_word_range(&h).expect("word");
        let doc = parse("* WAITING Title\n");
        assert_eq!(&doc.source()[r], "WAITING");
    }

    #[test]
    fn post_stars_word_is_todo_when_carved() {
        let h = first_kind("* TODO Title\n", OrgSyntaxKind::HEADING);
        let r = post_stars_word_range(&h).expect("word");
        let doc = parse("* TODO Title\n");
        assert_eq!(&doc.source()[r], "TODO");
    }

    #[test]
    fn post_stars_word_is_priority_cookie_when_no_todo() {
        let h = first_kind("* [#A] WAITING\n", OrgSyntaxKind::HEADING);
        let r = post_stars_word_range(&h).expect("word");
        let doc = parse("* [#A] WAITING\n");
        assert_eq!(&doc.source()[r], "[#A]");
    }
}
