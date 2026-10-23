//! The parts of the grammar that are not a single construct.
//!
//! - [`document`]: `parse_document`, outlines, zeroth section
//! - [`decide`]: `Element`, `decide`, `interrupts_paragraph`
//! - [`region`]: `parse_one_element`, `elements_in`, `inline_in` /
//!   `inline_to_limit` (object walks under the nesting budget),
//!   `opaque_frame_end`
//! - [`atoms`]: shared pure helpers (`is_digits`, `line_keyword_prefix`, ...)
//! - [`frame`]: `FrameShape` / `parse_frame` for blocks and drawers, and the
//!   cached index of their END lines

pub(crate) mod atoms;
pub(crate) mod decide;
pub(crate) mod document;
pub(crate) mod frame;
pub(crate) mod region;
