//! Tag charset, lesser-block comma-quoting.

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

/// Prefix lesser-block body lines that would look like a heading or `#+` line.
///
/// After optional leading ASCII spaces/tabs, a remainder that is any run of
/// commas (possibly none) followed by `*` or `#+` gets one more `,` in front,
/// as `org-escape-code-in-string` does. Do not use it for verse blocks,
/// which Org does not quote.
#[must_use]
pub fn quote_lesser_body(src: &str) -> String {
    map_lines(src, quote_line)
}

/// Inverse of [`quote_lesser_body`]: strip one comma when the remainder after
/// it is something quoting would prefix (commas, then `*` or `#+`).
///
/// A comma before other text (`,hello`) is left alone.
#[must_use]
pub fn unquote_lesser_body(src: &str) -> String {
    map_lines(src, unquote_line)
}

fn map_lines(src: &str, each: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    for_each_line(src, |line, term| {
        out.push_str(&each(line));
        out.push_str(term);
    });
    out
}

/// Split `src` on lexer line terminators (`\r\n` as one, then `\r`, then `\n`).
///
/// `term` is the original terminator, or `""` for a last line with none.
pub(crate) fn for_each_line(src: &str, mut each: impl FnMut(&str, &str)) {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
            i += 1;
        }
        let line = &src[start..i];
        let term = if i >= bytes.len() {
            ""
        } else if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            i += 2;
            "\r\n"
        } else if bytes[i] == b'\r' {
            i += 1;
            "\r"
        } else {
            i += 1;
            "\n"
        };
        each(line, term);
    }
}

fn indent_len(line: &str) -> usize {
    line.bytes()
        .position(|b| b != b' ' && b != b'\t')
        .unwrap_or(line.len())
}

fn quote_line(line: &str) -> String {
    let i = indent_len(line);
    let rest = &line[i..];
    if needs_quote(rest) {
        let mut s = String::with_capacity(line.len() + 1);
        s.push_str(&line[..i]);
        s.push(',');
        s.push_str(rest);
        s
    } else {
        line.to_string()
    }
}

fn needs_quote(rest: &str) -> bool {
    let rest = rest.trim_start_matches(',');
    rest.starts_with('*') || rest.starts_with("#+")
}

fn unquote_line(line: &str) -> String {
    let i = indent_len(line);
    let rest = &line[i..];
    // Inverse of quote: a leading comma was added by quoting when the
    // remainder after it is something quoting would have prefixed.
    if let Some(after) = rest.strip_prefix(',')
        && needs_quote(after)
    {
        let mut s = String::with_capacity(line.len().saturating_sub(1));
        s.push_str(&line[..i]);
        s.push_str(after);
        s
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert_eq!(quote_lesser_body(raw), quoted, "quote {raw:?}");
            assert_eq!(unquote_lesser_body(quoted), raw, "unquote {quoted:?}");
            assert_eq!(
                unquote_lesser_body(&quote_lesser_body(raw)),
                raw,
                "round-trip {raw:?}"
            );
        }
    }

    #[test]
    fn comma_quoting_preserves_final_newline_and_blank_lines() {
        let src = "*h\n\n#+k\n";
        let quoted = quote_lesser_body(src);
        assert_eq!(quoted, ",*h\n\n,#+k\n");
        assert_eq!(unquote_lesser_body(&quoted), src);
        assert_eq!(quote_lesser_body(""), "");
        assert_eq!(unquote_lesser_body(""), "");
        assert_eq!(quote_lesser_body("\n"), "\n");
        assert_eq!(quote_lesser_body("*h\r\n"), ",*h\r\n");
        assert_eq!(quote_lesser_body("#+end_example\r"), ",#+end_example\r");
        assert_eq!(quote_lesser_body("* heading\r"), ",* heading\r");
        assert_eq!(
            quote_lesser_body("hello\r#+end_example"),
            "hello\r,#+end_example"
        );
        assert_eq!(
            unquote_lesser_body("hello\r,#+end_example"),
            "hello\r#+end_example"
        );
    }

    #[test]
    fn unquote_does_not_strip_a_comma_before_other_text() {
        assert_eq!(unquote_lesser_body(",hello"), ",hello");
        assert_eq!(unquote_lesser_body("  ,hello"), "  ,hello");
        assert_eq!(unquote_lesser_body(",#not-a-keyword"), ",#not-a-keyword");
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
            quote_lesser_body("*x"),
            "*x",
            "the helper would quote; verse callers must not invoke it"
        );
    }
}
