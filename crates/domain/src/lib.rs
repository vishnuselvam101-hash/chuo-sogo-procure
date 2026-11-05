//! Pure procurement domain types and deterministic rules.
//! No I/O, no database, no AI: everything here is testable in isolation.

pub mod agents;
pub mod aliases;
pub mod approval;
pub mod comparison;
pub mod connector;
pub mod decision;
pub mod history;
pub mod invoicing;
pub mod listing;
pub mod matching;
pub mod money;
pub mod ocr_table;
pub mod opportunity;
pub mod rbac;
pub mod request_import;
pub mod savings;
pub mod shop;
pub mod shortlist;
pub mod workflow;
