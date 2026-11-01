//! Pure procurement domain types and deterministic rules.
//! No I/O, no database, no AI: everything here is testable in isolation.

pub mod approval;
pub mod comparison;
pub mod history;
pub mod invoicing;
pub mod listing;
pub mod matching;
pub mod money;
pub mod rbac;
pub mod shop;
pub mod workflow;
