//! Tag charset.

/// Whether `name` is a legal Org headline tag atom.
///
/// Non-empty; every character is Unicode alphanumeric or `_` `@` `#` `%`.
/// Empty, colon, space, comma, `*`, and other ASCII punctuation fail.
#[must_use]
pub fn is_valid_tag_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{escape_code, unescape_code};

    fn escape(s: &str) -> String {
        escape_code(s).into_owned()
    }

    fn unescape(s: &str) -> String {
        unescape_code(s).into_owned()
    }
    use crate::ast::{AstNode, Block, BlockKind, HasParts, Heading, Headline};
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::{TAXONOMY, nodes_of_kind};

    #[test]
    fn tag_charset_accepts_and_rejects() {
        assert!(is_valid_tag_name("work"));
        assert!(is_valid_tag_name("ARCHIVE"));
        assert!(is_valid_tag_name("a_b"));
        assert!(is_valid_tag_name("a@b"));
        assert!(is_valid_tag_name("a#b"));
        assert!(is_valid_tag_name("a%b"));
        assert!(is_valid_tag_name("caf\u{e9}"));
        assert!(!is_valid_tag_name(""));
        assert!(!is_valid_tag_name("a:b"));
        assert!(!is_valid_tag_name("a b"));
        assert!(!is_valid_tag_name("a,b"));
        assert!(!is_valid_tag_name("a*b"));
        assert!(!is_valid_tag_name("a-b"));
        assert!(!is_valid_tag_name("a.b"));
        assert!(!is_valid_tag_name("a/b"));
        assert!(!is_valid_tag_name("a+b"));
        assert!(!is_valid_tag_name(" a"));
        assert!(!is_valid_tag_name("a "));
        assert!(!is_valid_tag_name("a\tb"));
    }

    #[test]
    fn every_taxonomy_headline_tag_is_valid() {
        for &(src, cfg) in TAXONOMY {
            let doc = crate::parse_with(src, &cfg.build());
            for n in nodes_of_kind(doc.root(), OrgSyntaxKind::HEADING) {
                let Some(h) = Heading::cast(n) else {
                    continue;
                };
                if let Some(tags) = h.tags() {
                    for tag in tags.iter() {
                        assert!(
                            is_valid_tag_name(tag.name()),
                            "TAXONOMY tag {:?} in {src:?}",
                            tag.name()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn comma_quoting_table() {
        let rows = [
            ("*not a heading", ",*not a heading"),
            ("#+end_example", ",#+end_example"),
            (",*already", ",,*already"),
            (",hello", ",hello"),
            ("  *indented", "  ,*indented"),
            ("\t#+x", "\t,#+x"),
            ("plain", "plain"),
            (",#+already", ",,#+already"),
            (",,*twice", ",,,*twice"),
            ("  ,,#+x", "  ,,,#+x"),
        ];
        for (raw, quoted) in rows {
            assert_eq!(escape(raw), quoted, "quote {raw:?}");
            assert_eq!(unescape(quoted), raw, "unquote {quoted:?}");
            assert_eq!(unescape(&escape(raw)), raw, "round-trip {raw:?}");
        }
    }

    #[test]
    fn comma_quoting_preserves_final_newline_and_blank_lines() {
        let src = "*h\n\n#+k\n";
        let quoted = escape(src);
        assert_eq!(quoted, ",*h\n\n,#+k\n");
        assert_eq!(unescape(&quoted), src);
        assert_eq!(escape(""), "");
        assert_eq!(unescape(""), "");
        assert_eq!(escape("\n"), "\n");
        assert_eq!(escape("*h\r\n"), ",*h\r\n");
        assert_eq!(escape("#+end_example\r"), ",#+end_example\r");
        assert_eq!(escape("* heading\r"), ",* heading\r");
        assert_eq!(escape("hello\r#+end_example"), "hello\r,#+end_example");
        assert_eq!(unescape("hello\r,#+end_example"), "hello\r#+end_example");
    }

    #[test]
    fn unquote_does_not_strip_a_comma_before_other_text() {
        assert_eq!(unescape(",hello"), ",hello");
        assert_eq!(unescape("  ,hello"), "  ,hello");
        assert_eq!(unescape(",#not-a-keyword"), ",#not-a-keyword");
    }

    #[test]
    fn verse_bodies_are_not_the_quote_path() {
        let src = "#+begin_verse\n*x\n#+end_verse\n";
        let doc = parse(src);
        let block = Block::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::VERSE_BLOCK)
                .into_iter()
                .next()
                .expect("verse"),
        )
        .expect("cast");
        assert_eq!(block.kind(), BlockKind::Verse);
        let body = doc.slice(block.part_range(crate::ast::BlockPart::Body).expect("body"));
        assert!(body.contains("*x"), "{body:?}");
        assert_ne!(
            escape("*x"),
            "*x",
            "escaping would add a comma; verse bodies are never escaped"
        );
    }
}
