//! AI layer: providers and the capabilities built on them. AI output is only ever a
//! suggestion; promoting it into requests, products or quotations is a human action in the
//! server (ARCHITECTURE D6).

pub mod extract;
pub mod mock;
pub mod provider;
pub mod testing;

pub use provider::{AiError, ModelCall, Provider};
