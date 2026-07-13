//! Caller-attributed audit events for rust-junosmcp / rust-srxmcp.

mod schema;
mod scope;
pub mod testutil;

pub use schema::{AuditOutcome, AuditValue};
pub use scope::AuditScope;
// `init` module added in Task 2.
