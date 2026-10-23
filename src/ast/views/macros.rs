//! Macro objects: `{{{name}}}` and `{{{name(args)}}}`.

use crate::ast::AstNode;
use crate::ast::layout::{MacroSpans, macro_spans, text_in};
use crate::syntax::OrgSyntaxKind;

crate::ast_node!(
    /// A macro object: `{{{name}}}` or `{{{name(args)}}}`.
    Macro,
    OrgSyntaxKind::MACRO
);

impl Macro {
    /// Macro name as written.
    #[must_use]
    pub fn name(&self) -> String {
        text_in(self.syntax(), spans(self).name)
    }

    /// Arguments, split at commas.
    ///
    /// A backslash makes the next character literal, so `\,` is a comma
    /// inside an argument. A leading `:Key` word is lowercased. Empty `()`
    /// yields no arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        match spans(self).args {
            None => Vec::new(),
            Some(r) => split_macro_args(&text_in(self.syntax(), r)),
        }
    }
}

fn spans(m: &Macro) -> MacroSpans {
    macro_spans(m.syntax())
        .expect("grammar: MACRO is {{{name}}} or {{{name(args)}}} (forms/macros.rs)")
}

/// Commas split; backslash emits the next character; a trailing backslash is
/// kept; empty input yields no arguments. A leading `:Key` run is downcased.
fn split_macro_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                cur.push(n);
            } else {
                cur.push('\\');
            }
            continue;
        }
        if c == ',' {
            out.push(fold_macro_arg(&cur));
            cur.clear();
            continue;
        }
        cur.push(c);
    }
    if out.is_empty() && cur.is_empty() {
        return Vec::new();
    }
    out.push(fold_macro_arg(&cur));
    out
}

fn fold_macro_arg(s: &str) -> String {
    let Some(rest) = s.strip_prefix(':') else {
        return s.to_string();
    };
    let (key, tail) = match rest.find(|c: char| c.is_ascii_whitespace()) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    format!(":{}{tail}", key.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::syntax::OrgSyntaxKind;
    use crate::tests::nodes_of_kind;

    fn mac(src: &str) -> Macro {
        let doc = parse(src);
        Macro::cast(
            nodes_of_kind(doc.root(), OrgSyntaxKind::MACRO)
                .into_iter()
                .next()
                .expect("macro"),
        )
        .expect("cast")
    }

    #[test]
    fn macro_name_and_args() {
        let m = mac("{{{title}}}\n");
        assert_eq!(m.name(), "title");
        assert!(m.arguments().is_empty(), "{:?}", m.arguments());

        let m = mac("{{{kw(a, b)}}}\n");
        assert_eq!(m.name(), "kw");
        assert_eq!(m.arguments(), vec!["a", " b"]);

        let m = mac("{{{kw(a\\,b,c)}}}\n");
        assert_eq!(m.arguments(), vec!["a,b", "c"]);

        let m = mac("{{{kw()}}}\n");
        assert!(m.arguments().is_empty(), "{:?}", m.arguments());

        let m = mac("{{{kw(:Key value,Plain)}}}\n");
        assert_eq!(m.arguments(), vec![":key value", "Plain"]);
    }

    #[test]
    fn split_macro_args_empty_and_escape() {
        assert!(
            split_macro_args("").is_empty(),
            "{:?}",
            split_macro_args("")
        );
        assert_eq!(split_macro_args("a"), vec!["a"]);
        assert_eq!(split_macro_args("a\\\\b"), vec!["a\\b"]);
        assert_eq!(split_macro_args("\\"), vec!["\\"]);
        assert_eq!(split_macro_args("a,"), vec!["a", ""]);
        assert_eq!(split_macro_args("a,,b"), vec!["a", "", "b"]);
        assert_eq!(split_macro_args(":Foo"), vec![":foo"]);
        assert_eq!(split_macro_args(":Foo Bar"), vec![":foo Bar"]);
    }
}
