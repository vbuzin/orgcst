//! Property drawer and node-property lines.

use super::SynthesisError;

/// Whether `key` may appear as a node-property key (no trailing `+`).
///
/// Rejects empty keys, ASCII space/tab, newlines, a trailing `+` (that is
/// the append flag), and ASCII-caseless `END` (would close the drawer).
/// Interior colons and Unicode letters are allowed.
#[must_use]
pub fn is_valid_property_key(key: &str) -> bool {
    !key.is_empty()
        && !key.ends_with('+')
        && !key.eq_ignore_ascii_case("END")
        && !key
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NodePropertySpec<'a> {
    pub key: &'a str,
    pub value: Option<&'a str>,
    pub append: bool,
}

pub(crate) fn render_node_property(spec: NodePropertySpec<'_>) -> Result<String, SynthesisError> {
    if !is_valid_property_key(spec.key) {
        return Err(SynthesisError::InvalidPropertyKey(spec.key.to_string()));
    }
    match spec.value {
        Some("") => {
            return Err(SynthesisError::InvalidPropertyValue(spec.key.to_string()));
        }
        Some(v) if v.contains(['\n', '\r']) => {
            return Err(SynthesisError::InvalidPropertyValue(spec.key.to_string()));
        }
        _ => {}
    }
    let plus = if spec.append { "+" } else { "" };
    Ok(match spec.value {
        Some(v) => format!(":{}{}: {}\n", spec.key, plus, v),
        None => format!(":{}{}:\n", spec.key, plus),
    })
}

pub(crate) fn render_property_drawer(
    entries: &[NodePropertySpec<'_>],
) -> Result<String, SynthesisError> {
    let mut s = String::from(":PROPERTIES:\n");
    for spec in entries {
        s.push_str(&render_node_property(*spec)?);
    }
    s.push_str(":END:\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn property_key_charset() {
        assert!(is_valid_property_key("ID"));
        assert!(is_valid_property_key("CUSTOM_ID"));
        assert!(is_valid_property_key("k-e_y*2"));
        assert!(is_valid_property_key("a:b"));
        assert!(is_valid_property_key("caf\u{e9}"));
        assert!(!is_valid_property_key("END"));
        assert!(!is_valid_property_key("end"));
        assert!(!is_valid_property_key("End"));
        assert!(!is_valid_property_key("FOO+"));
        assert!(!is_valid_property_key("a b"));
        assert!(!is_valid_property_key(""));
        assert!(!is_valid_property_key("a\tb"));
        assert!(!is_valid_property_key("a\nb"));
    }

    #[test]
    fn node_property_recipes() {
        assert_eq!(
            render_node_property(NodePropertySpec {
                key: "KEY",
                value: Some("value"),
                append: false,
            })
            .unwrap(),
            ":KEY: value\n"
        );
        assert_eq!(
            render_node_property(NodePropertySpec {
                key: "KEY",
                value: None,
                append: false,
            })
            .unwrap(),
            ":KEY:\n"
        );
        assert_eq!(
            render_node_property(NodePropertySpec {
                key: "FOO",
                value: None,
                append: true,
            })
            .unwrap(),
            ":FOO+:\n"
        );
        assert_eq!(
            render_node_property(NodePropertySpec {
                key: "a:b",
                value: Some("v"),
                append: false,
            })
            .unwrap(),
            ":a:b: v\n"
        );
        assert_eq!(
            render_node_property(NodePropertySpec {
                key: "k-e_y*2",
                value: Some("v"),
                append: false,
            })
            .unwrap(),
            ":k-e_y*2: v\n"
        );
    }

    #[test]
    fn property_drawer_recipes() {
        assert_eq!(
            render_property_drawer(&[]).unwrap(),
            ":PROPERTIES:\n:END:\n"
        );
        assert_eq!(
            render_property_drawer(&[
                NodePropertySpec {
                    key: "ID",
                    value: Some("abc"),
                    append: false,
                },
                NodePropertySpec {
                    key: "CUSTOM_ID",
                    value: Some("xyz"),
                    append: false,
                },
            ])
            .unwrap(),
            ":PROPERTIES:\n:ID: abc\n:CUSTOM_ID: xyz\n:END:\n"
        );
        assert_eq!(
            render_property_drawer(&[NodePropertySpec {
                key: "FOO",
                value: None,
                append: true,
            }])
            .unwrap(),
            ":PROPERTIES:\n:FOO+:\n:END:\n"
        );
    }

    #[test]
    fn property_rejects() {
        assert!(matches!(
            render_node_property(NodePropertySpec {
                key: "END",
                value: Some("x"),
                append: false,
            }),
            Err(SynthesisError::InvalidPropertyKey(s)) if s == "END"
        ));
        assert!(matches!(
            render_node_property(NodePropertySpec {
                key: "FOO+",
                value: None,
                append: false,
            }),
            Err(SynthesisError::InvalidPropertyKey(_))
        ));
        assert!(matches!(
            render_node_property(NodePropertySpec {
                key: "KEY",
                value: Some(""),
                append: false,
            }),
            Err(SynthesisError::InvalidPropertyValue(_))
        ));
        assert!(matches!(
            render_node_property(NodePropertySpec {
                key: "KEY",
                value: Some("a\nb"),
                append: false,
            }),
            Err(SynthesisError::InvalidPropertyValue(s)) if s == "KEY"
        ));
    }

    #[test]
    fn property_drawer_parse_stability() {
        let entries = [
            NodePropertySpec {
                key: "FOO",
                value: None,
                append: true,
            },
            NodePropertySpec {
                key: "CUSTOM_ID",
                value: Some("xyz"),
                append: false,
            },
            NodePropertySpec {
                key: "a:b",
                value: Some("v"),
                append: false,
            },
            NodePropertySpec {
                key: "k-e_y*2",
                value: Some("v"),
                append: false,
            },
        ];
        let drawer = render_property_drawer(&entries).unwrap();
        let src = format!("* H\n{drawer}");
        let doc = parse(&src);
        super::super::equal::assert_property_drawer_eq(&doc, &entries);
        let empty = render_property_drawer(&[]).unwrap();
        let doc = parse(&format!("* H\n{empty}"));
        super::super::equal::assert_property_drawer_eq(&doc, &[]);
    }
}
