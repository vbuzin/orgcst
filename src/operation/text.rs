//! Compile path for [`super::Operation::ReplaceText`].

use crate::edit::EditSet;
use crate::{Document, OperationError, TextRange};

pub(super) fn compile_replace_text(
    doc: &Document,
    range: TextRange,
    replacement: &str,
) -> Result<EditSet, OperationError> {
    let set = EditSet::single(doc.source(), range, replacement)?;
    if doc.slice(range) == replacement {
        return Ok(super::empty_edit_set(doc.source()));
    }
    Ok(set)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::EditSetError;
    use crate::{Operation, compile, parse};
    use rowan::{TextRange, TextSize};

    fn ts(n: u32) -> TextSize {
        TextSize::from(n)
    }

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(ts(start), ts(end))
    }

    fn replace(start: u32, end: u32, repl: &str) -> Operation {
        Operation::ReplaceText {
            range: range(start, end),
            replacement: repl.to_string(),
        }
    }

    #[test]
    fn replace_text_happy_path() {
        let doc = parse("hello");
        let set = compile(&doc, &replace(1, 4, "ipp")).expect("in bounds");
        assert_eq!(set.len(), 1);
        assert_eq!(set.apply_to_text(doc.source()), "hippo");
        let next = doc.apply(&set);
        assert_eq!(next.write(), "hippo");
        assert!(std::ptr::eq(next.write(), next.source()));
    }

    #[test]
    fn replace_text_identical_bytes_is_empty_set() {
        let doc = parse("hello");
        let set = compile(&doc, &replace(0, 5, "hello")).expect("identical is a no-op");
        assert!(set.is_empty());
        assert_eq!(set.apply_to_text(doc.source()), doc.source());
    }

    #[test]
    fn replace_text_oob_is_editset_error_not_panic() {
        let doc = parse("abc");
        let err = compile(&doc, &replace(0, 4, "x")).unwrap_err();
        assert!(matches!(
            err,
            OperationError::EditSet(EditSetError::OutOfBounds(_))
        ));
    }

    #[test]
    fn replace_text_mid_char_is_editset_error_not_panic() {
        // "cafe" with U+00E9: bytes c a f C3 A9
        let src = "caf\u{e9}";
        assert_eq!(src.len(), 5);
        let doc = parse(src);
        let err = compile(&doc, &replace(0, 4, "x")).unwrap_err();
        assert!(matches!(
            err,
            OperationError::EditSet(EditSetError::NotCharBoundary(_))
        ));
    }

    #[test]
    fn replace_text_empty_insert_is_not_a_no_op() {
        let doc = parse("ab");
        let set = compile(&doc, &replace(1, 1, "X")).expect("insert");
        assert!(!set.is_empty());
        assert_eq!(set.apply_to_text(doc.source()), "aXb");
    }
}
