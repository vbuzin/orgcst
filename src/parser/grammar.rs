//! The Org grammar.
//!
//! [`forms`] holds one module per construct (elements and objects); [`drive`]
//! holds the document walk, element dispatch, regions and the shared helpers
//! the forms use.

pub(crate) mod drive;
pub(crate) mod forms;

pub(crate) use drive::document::parse_document;
