//! The reader: markdown document model, parser and on-screen view.

pub mod access;
pub mod autolink;
pub mod doc;
pub mod fold;
pub mod highlight;
pub mod html;
pub mod layout;
pub mod model;
pub mod parse;
pub mod paths;
pub mod slug;
pub mod sort;
pub mod split;
#[cfg(feature = "ui")]
pub mod ui;
