//! Org construct grammars (elements and objects).
//!
//! Each leaf owns recognition (`probe` / `opener` / `resolve`) and emission
//! (`parse`) for one construct family. Document walk, decide, regions, and
//! shared frame machinery live under [`super::drive`].

pub(crate) mod affiliated;
pub(crate) mod block;
pub(crate) mod call;
pub(crate) mod citation;
pub(crate) mod clock;
pub(crate) mod comment;
pub(crate) mod cookie;
pub(crate) mod diary;
pub(crate) mod drawer;
pub(crate) mod entity;
pub(crate) mod export_snippet;
pub(crate) mod fixed_width;
pub(crate) mod footnote;
pub(crate) mod heading;
pub(crate) mod horizontal_rule;
pub(crate) mod inline;
pub(crate) mod inline_src;
pub(crate) mod keyword;
pub(crate) mod latex;
pub(crate) mod line_break;
pub(crate) mod link;
pub(crate) mod list;
pub(crate) mod macros;
pub(crate) mod markup;
pub(crate) mod paragraph;
pub(crate) mod planning;
pub(crate) mod script;
pub(crate) mod table;
pub(crate) mod target;
pub(crate) mod timestamp;
