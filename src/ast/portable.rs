//! A [`Document`] that can cross threads.
//!
//! A `Document` holds a rowan red tree, which is neither `Send` nor `Sync`.
//! [`PortableDocument`] keeps only the thread-safe parts and rebuilds the red
//! tree in O(1) on the other side, without parsing again.

use crate::document::{Document, DocumentParts};

/// A [`Document`] without its red tree, so it is `Send + Sync`.
///
/// It holds the source, the green tree, the configuration passed to the
/// parser, the effective configuration after in-buffer settings, and the
/// diagnostics. The fields are private so a caller cannot pair a source with
/// another document's tree. Rebuild with [`Self::into_document`].
///
/// [`Document`], the typed views and [`crate::EditContext`] hold red nodes, so
/// they are neither `Send` nor `Sync`.
pub struct PortableDocument {
    parts: DocumentParts,
}

impl Document {
    /// A portable copy; this document stays usable. Cheap: the parts are
    /// reference-counted.
    #[must_use]
    pub fn to_portable(&self) -> PortableDocument {
        PortableDocument {
            parts: self.parts(),
        }
    }

    /// Convert into a portable document, dropping the red tree.
    #[must_use]
    pub fn into_portable(self) -> PortableDocument {
        PortableDocument {
            parts: self.into_parts(),
        }
    }
}

impl PortableDocument {
    /// Rebuild the document on the current thread. This is O(1): nothing is
    /// parsed again.
    #[must_use]
    pub fn into_document(self) -> Document {
        Document::from_parts(self.parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diagnostic, ParseConfig, TodoKind};
    use std::sync::Arc;

    #[test]
    fn portable_document_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PortableDocument>();
        assert_send_sync::<Arc<str>>();
        assert_send_sync::<Arc<ParseConfig>>();
        assert_send_sync::<Vec<Diagnostic>>();
    }

    #[test]
    fn to_portable_does_not_consume() {
        let original = crate::parse("* a\n");
        let _portable = original.to_portable();
        assert!(original.headings().next().is_some());
    }

    #[test]
    fn into_portable_round_trip_empty_keeps_two_arcs() {
        let original = crate::parse("");
        let before = original.parts();
        assert!(
            Arc::ptr_eq(&before.config, &before.caller),
            "empty parse layers start as one allocation"
        );
        let rebuilt = original.into_portable().into_document();
        let after = rebuilt.parts();
        assert!(
            Arc::ptr_eq(&after.config, &after.caller),
            "reconstruct still stores both layers (they may share the Arc)"
        );
        assert_eq!(after.config.as_ref(), before.config.as_ref());
        assert_eq!(after.caller.as_ref(), before.caller.as_ref());
    }

    #[test]
    fn portable_round_trip_preserves_five_parts_across_a_thread() {
        use crate::Headline;
        let caller = ParseConfig::default().with_todo_keywords([("NEXT", TodoKind::Active)]);
        let src = "#+TODO: BUG | FIXED\nCLOCK: not-a-stamp\n* NEXT one\n";
        let original = crate::parse_with(src, &caller);
        let want_green = original.root().green().into_owned();
        let want_diags = original.diagnostics().to_vec();
        assert!(
            !want_diags.is_empty(),
            "fixture must carry a sticky diagnostic so reconstruct cannot drop the vec unnoticed"
        );
        let want_parts = original.parts();
        let want_config = want_parts.config;
        let want_caller = want_parts.caller;
        assert_ne!(
            want_config.as_ref(),
            want_caller.as_ref(),
            "file vocab must differ from caller so a dropped caller layer would fail"
        );
        let portable = original.to_portable();
        // Document is !Send; assert on the worker and return ().
        std::thread::spawn(move || {
            let rebuilt = portable.into_document();
            assert_eq!(rebuilt.source(), src);
            assert_eq!(rebuilt.root().green().into_owned(), want_green);
            assert_eq!(rebuilt.diagnostics(), want_diags);
            assert_eq!(rebuilt.parts().config.as_ref(), want_config.as_ref());
            assert_eq!(rebuilt.parts().caller.as_ref(), want_caller.as_ref());
            let heading = rebuilt.headings().next().expect("heading");
            assert!(heading.todo_keyword().is_none());
        })
        .join()
        .expect("thread");
    }
}
