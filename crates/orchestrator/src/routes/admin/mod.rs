//! Instance-scoped operator API at `/api/admin/...`.
//!
//! Every handler in this module takes the `SuperAdmin` extractor rather than
//! `AuthIdentity`. Nothing here is team-scoped — these routes exist to answer
//! questions about the instance as a whole, which no other surface can.

pub(crate) mod audit;
pub mod fleet;
pub(crate) mod health;
pub(crate) mod members;
pub(crate) mod overview;

use axum::{
    routing::get,
    Router,
};

use crate::state::OrchestratorState;

/// Every path this module serves, as `(method, absolute path)`.
///
/// The table-driven authorization test in `tests/integration.rs` iterates
/// this, so a route added here without a `SuperAdmin` extractor fails CI
/// rather than shipping open. Keep it in sync with `router()` below.
pub const ADMIN_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/admin/health"),
    ("GET", "/api/admin/overview"),
    ("GET", "/api/admin/fleet"),
    ("GET", "/api/admin/members"),
    ("GET", "/api/admin/audit"),
];

pub fn router() -> Router<OrchestratorState> {
    Router::new()
        .route("/health", get(health::admin_health))
        .route("/overview", get(overview::overview))
        .route("/fleet", get(fleet::fleet))
        .route("/members", get(members::list_members))
        .route("/audit", get(audit::instance_audit))
}
