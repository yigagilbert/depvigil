//! depvigil: graph-native dependency decisions.
//!
//! The library simulates a proposed dependency change in isolation, records
//! an approval bound to the resulting dependency state, and later invalidates
//! that approval when new advisory evidence arrives.
//!
//! - [`ecosystem`] is the boundary to a package manager; Cargo is the one
//!   implementation.
//! - [`store`] keeps the graph in a Relate directory store.
//! - [`engine`] provides each command as a library function.

#![warn(missing_docs)]

pub mod advisory;
mod compressed;
pub mod ecosystem;
pub mod engine;
pub mod error;
pub mod mirror;
pub mod model;
pub mod policy;
pub mod store;
pub mod time;

pub use engine::Engine;
pub use error::{Error, Result};
