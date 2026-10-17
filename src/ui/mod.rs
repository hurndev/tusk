//! Full-screen interfaces: the prose editor and the prose reader.
//!
//! Both share the same terminal handling and the same wrapping engine in
//! [`crate::text`], so what you write and what you read are laid out
//! identically. Nothing here is ever the only way to do something — the plain
//! command output stays scriptable, and the editor can be swapped for your own
//! with `--external`.

pub mod editor;
pub mod frame;
pub mod reader;
pub mod spell;

pub use frame::Prose;
