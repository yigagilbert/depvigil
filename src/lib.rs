//! depvigil: graph-native dependency decisions.
//!
//! The library simulates a proposed dependency change in isolation, records
//! an approval bound to the resulting dependency state, and later invalidates
//! that approval when new advisory evidence arrives.

#![warn(missing_docs)]

pub mod ecosystem;
pub mod error;
pub mod model;

pub use error::{Error, Result};
