//! The `:key value` notation of Babel's header arguments (a src block's
//! begin line, `#+HEADER:`, a `header-args` property) and of an element's
//! `#+ATTR_<BACKEND>:` attributes: one argument set, changed or removed in a
//! list, everything else in the list kept as written.

use crate::settings::babel_arguments;

/// Why an argument cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArgumentProblem {
    /// The key is empty, starts with a colon or holds whitespace.
    InvalidKey,
    /// The value holds a line break, or both whitespace and a double quote,
    /// so it would not read back as one value.
    InvalidValue,
}

/// The arguments of a `:key value` list, in order, each key without its
/// colon and each value with its quotes and backslashes undone; a key with
/// no value has an empty one. A key is a run after a colon that starts the
/// list or follows whitespace, and a value runs to the next such colon or
/// is one quoted string.
///
/// ```
/// use orgcst::arguments;
///
/// assert_eq!(arguments(r#"python :results output :file "a b""#),
///            [("results".to_owned(), "output".to_owned()),
///             ("file".to_owned(), "a b".to_owned())]);
/// ```
#[must_use]
pub fn arguments(list: &str) -> Vec<(String, String)> {
    babel_arguments(list)
        .into_iter()
        .map(|a| (list[a.key.clone()].to_owned(), a.value))
        .collect()
}

/// The words of one argument's value, as Babel splits a value of choices
/// (`:results output table replace`) at whitespace.
///
/// ```
/// use orgcst::argument_words;
///
/// assert_eq!(argument_words(" output  table "), ["output", "table"]);
/// ```
#[must_use]
pub fn argument_words(value: &str) -> Vec<&str> {
    value.split_whitespace().collect()
}

/// The assignments of a `:var` argument's value, each a name and its value
/// as written, split as Babel splits them (`org-babel-parse-multiple-vars`):
/// at spaces outside balanced brackets, parentheses and double quotes,
/// pieces either side of an `=` joined again, the name before the first
/// `=`. Babel keeps a comma that ends an assignment as part of its value.
/// An assignment with no `=` has an empty value.
///
/// ```
/// use orgcst::variables;
///
/// assert_eq!(variables("x=1 rows=table[1, 2] s=\"a b\""),
///            [("x".to_owned(), "1".to_owned()),
///             ("rows".to_owned(), "table[1, 2]".to_owned()),
///             ("s".to_owned(), "\"a b\"".to_owned())]);
/// assert_eq!(variables("x = 1"), [("x".to_owned(), "1".to_owned())]);
/// ```
#[must_use]
pub fn variables(value: &str) -> Vec<(String, String)> {
    let mut joined: Vec<String> = Vec::new();
    for piece in balanced_pieces(value) {
        match joined.last_mut() {
            Some(last) if last.ends_with('=') || piece.starts_with('=') => last.push_str(piece),
            _ => joined.push(piece.to_owned()),
        }
    }
    joined
        .iter()
        .map(|p| match p.trim().split_once('=') {
            Some((name, v)) => (name.trim().to_owned(), v.trim().to_owned()),
            None => (p.trim().to_owned(), String::new()),
        })
        .collect()
}

/// `value` split at spaces outside balanced brackets, parentheses and
/// double quotes, as `org-babel-balanced-split` splits on a space; an
/// opening that never closes is an ordinary character.
fn balanced_pieces(value: &str) -> Vec<&str> {
    let bytes = value.as_bytes();
    let mut pieces = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b' ' => {
                if start < i {
                    pieces.push(&value[start..i]);
                }
                start = i + 1;
                i += 1;
            }
            b'(' | b'[' => i = balanced_end(bytes, i).unwrap_or(i + 1),
            b'"' if i == 0 || bytes[i - 1] != b'\\' => {
                i = closing_quote(bytes, i).map_or(i + 1, |end| end + 1);
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        pieces.push(&value[start..]);
    }
    pieces
}

/// Just past the bracket or parenthesis that closes the one at `open`.
fn balanced_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut stack = vec![bytes[open]];
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            c @ (b'(' | b'[') => stack.push(c),
            b']' if stack.last() == Some(&b'[') => {
                stack.pop();
            }
            b')' if stack.last() == Some(&b'(') => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
        if stack.is_empty() {
            return Some(i);
        }
    }
    None
}

/// The double quote that closes the one at `open`, not after a backslash.
fn closing_quote(bytes: &[u8], open: usize) -> Option<usize> {
    (open + 1..bytes.len()).find(|&i| bytes[i] == b'"' && bytes[i - 1] != b'\\')
}

/// `arguments` with the argument `key` set to `value`, or removed when
/// `value` is `None`.
///
/// `key` is written without its colon (`results`). A key compares exactly,
/// as Babel compares keys. An argument the list already has takes the new
/// value in place: the last one when it is written more than once, the
/// earlier ones kept as written. A new one is added at the end, after one space. A value
/// holding whitespace is written in double quotes; an empty value writes
/// the key alone. Removing an argument removes it with the whitespace
/// before it (after it, for the first), every time it is written. Nothing
/// else in the list changes.
///
/// ```
/// use orgcst::set_argument;
///
/// assert_eq!(set_argument("python :results output", "session", Some("main")).unwrap(),
///            "python :results output :session main");
/// assert_eq!(set_argument(":results output :session main", "results", None).unwrap(),
///            ":session main");
/// ```
///
/// # Errors
///
/// [`ArgumentProblem`] when `key` or `value` would not read back as written.
pub fn set_argument(
    arguments: &str,
    key: &str,
    value: Option<&str>,
) -> Result<String, ArgumentProblem> {
    if key.is_empty() || key.starts_with(':') || key.chars().any(char::is_whitespace) {
        return Err(ArgumentProblem::InvalidKey);
    }
    let written = match value {
        None => None,
        Some(v) if v.contains(['\n', '\r']) => return Err(ArgumentProblem::InvalidValue),
        Some(v) if v.chars().any(char::is_whitespace) => {
            if v.contains('"') {
                return Err(ArgumentProblem::InvalidValue);
            }
            Some(format!(":{key} \"{v}\""))
        }
        Some("") => Some(format!(":{key}")),
        Some(v) => Some(format!(":{key} {v}")),
    };
    let items = babel_arguments(arguments);
    let same: Vec<_> = items
        .iter()
        .filter(|a| &arguments[a.key.clone()] == key)
        .collect();
    let mut out = arguments.to_owned();
    // Edits run from the end so earlier offsets stay true.
    for (n, item) in same.iter().enumerate().rev() {
        let last = n + 1 == same.len();
        match (&written, last) {
            (Some(w), true) => out.replace_range(item.extent.clone(), w),
            (Some(_), false) => {}
            (None, _) => remove(&mut out, item.extent.clone()),
        }
    }
    if same.is_empty()
        && let Some(w) = written
    {
        let kept = out.trim_end_matches([' ', '\t']).len();
        out.truncate(kept);
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w);
    }
    Ok(out)
}

/// Remove `extent` from `s` with the whitespace before it, or after it when
/// nothing but whitespace is before it.
fn remove(s: &mut String, extent: std::ops::Range<usize>) {
    let before = s[..extent.start].trim_end_matches([' ', '\t']).len();
    if before > 0 {
        s.replace_range(before..extent.end, "");
    } else {
        let rest = &s[extent.end..];
        let after = extent.end + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
        s.replace_range(extent.start..after, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_changes_and_removes_one_argument() {
        assert_eq!(set_argument("", "wrap", Some("yes")).unwrap(), ":wrap yes");
        assert_eq!(
            set_argument("python :results output", "results", Some("value")).unwrap(),
            "python :results value"
        );
        assert_eq!(
            set_argument("python :results output  ", "session", Some("main")).unwrap(),
            "python :results output :session main"
        );
        assert_eq!(
            set_argument(":a 1 :b 2 :c 3", "b", None).unwrap(),
            ":a 1 :c 3"
        );
        assert_eq!(set_argument(":a 1 :b 2", "a", None).unwrap(), ":b 2");
        assert_eq!(set_argument(":wrap yes", "wrap", None).unwrap(), "");
        assert_eq!(set_argument(":a 1", "z", None).unwrap(), ":a 1");
    }

    #[test]
    fn keeps_everything_else_as_written() {
        assert_eq!(
            set_argument(r#"sh -n :file "a b.txt" :dir /tmp"#, "dir", Some("/srv")).unwrap(),
            r#"sh -n :file "a b.txt" :dir /srv"#
        );
    }

    #[test]
    fn a_repeated_key_changes_its_last_and_removes_every_one() {
        assert_eq!(
            set_argument(":var x=1 :var y=2", "var", Some("z=3")).unwrap(),
            ":var x=1 :var z=3"
        );
        assert_eq!(
            set_argument(":var x=1 :a 1 :var y=2", "var", None).unwrap(),
            ":a 1"
        );
    }

    #[test]
    fn quotes_a_value_with_spaces_and_writes_a_bare_key() {
        assert_eq!(
            set_argument("", "file", Some("a b.txt")).unwrap(),
            r#":file "a b.txt""#
        );
        assert_eq!(
            set_argument("", "noweb-ref", Some("")).unwrap(),
            ":noweb-ref"
        );
    }

    #[test]
    fn refuses_what_would_not_read_back() {
        assert_eq!(
            set_argument("", "", Some("x")),
            Err(ArgumentProblem::InvalidKey)
        );
        assert_eq!(
            set_argument("", "a b", Some("x")),
            Err(ArgumentProblem::InvalidKey)
        );
        assert_eq!(
            set_argument("", "a", Some("x\ny")),
            Err(ArgumentProblem::InvalidValue)
        );
        assert_eq!(
            set_argument("", "a", Some(r#"say "hi" there"#)),
            Err(ArgumentProblem::InvalidValue)
        );
    }

    #[test]
    fn variables_split_as_babel_splits_them() {
        let v = |s: &str| variables(s);
        let pair = |n: &str, x: &str| (n.to_owned(), x.to_owned());
        // Babel keeps the comma of `foo=1, bar=2` in the first value.
        assert_eq!(v("x=1, y=2"), [pair("x", "1,"), pair("y", "2")]);
        assert_eq!(v("x=1,y=2"), [pair("x", "1,y=2")]);
        assert_eq!(
            v("t=f(a=1, b=2) u"),
            [pair("t", "f(a=1, b=2)"), pair("u", "")]
        );
        assert_eq!(v("x= (1"), [pair("x", "(1")]);
        assert_eq!(v(""), []);
    }
}
