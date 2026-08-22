//! Instance-scoped operator API at `/api/admin/...`.
//!
//! Every handler in this module takes the `SuperAdmin` extractor rather than
//! `AuthIdentity`. Nothing here is team-scoped — these routes exist to answer
//! questions about the instance as a whole, which no other surface can.

pub(crate) mod audit;
pub(crate) mod break_glass;
pub(crate) mod deployment_actions;
pub mod fleet;
pub(crate) mod health;
pub(crate) mod member_actions;
pub(crate) mod members;
pub(crate) mod overview;
pub(crate) mod team_actions;

use axum::{
    routing::{
        get,
        post,
    },
    Router,
};

use crate::{
    auth::super_admin::Actor,
    state::OrchestratorState,
};

/// Record an admin action on the instance audit log.
///
/// `actor.member_id()` is `None` for break-glass, so the metadata carries
/// the actor label too — a row with no member should still say why it has
/// no member.
///
/// Never fails the action it is recording. An admin action that succeeded
/// but reported failure because its audit write failed would be worse than
/// one that is unlogged, so the failure is logged loudly instead.
pub(crate) async fn audit_admin(
    state: &OrchestratorState,
    actor: &Actor,
    action: &str,
    mut metadata: serde_json::Value,
) {
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("actor".into(), serde_json::json!(actor.label()));
    }
    if let Err(e) = state
        .storage
        .append_instance_audit(actor.member_id(), action, &metadata)
        .await
    {
        tracing::error!(
            action,
            error = %e,
            "admin: failed to write instance audit event"
        );
    }
}

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
    ("POST", "/api/admin/deployments/{deployment_id}/pause"),
    ("POST", "/api/admin/deployments/{deployment_id}/resume"),
    ("POST", "/api/admin/deployments/{deployment_id}/restart"),
    ("POST", "/api/admin/deployments/{deployment_id}/tier"),
    ("POST", "/api/admin/deployments/{deployment_id}/delete"),
    ("POST", "/api/admin/members/{member_id}/suspend"),
    ("POST", "/api/admin/members/{member_id}/unsuspend"),
    ("POST", "/api/admin/members/{member_id}/super_admin"),
    ("POST", "/api/admin/members/{member_id}/delete"),
    ("POST", "/api/admin/deployments/{deployment_id}/access"),
    ("GET", "/api/admin/teams"),
    ("POST", "/api/admin/teams"),
    ("POST", "/api/admin/teams/{team_id}"),
    ("POST", "/api/admin/teams/{team_id}/delete"),
];

pub fn router() -> Router<OrchestratorState> {
    Router::new()
        .route("/health", get(health::admin_health))
        .route("/overview", get(overview::overview))
        .route("/fleet", get(fleet::fleet))
        .route("/members", get(members::list_members))
        .route("/audit", get(audit::instance_audit))
        .route(
            "/deployments/{deployment_id}/pause",
            post(deployment_actions::pause),
        )
        .route(
            "/deployments/{deployment_id}/resume",
            post(deployment_actions::resume),
        )
        .route(
            "/deployments/{deployment_id}/restart",
            post(deployment_actions::restart),
        )
        .route(
            "/deployments/{deployment_id}/tier",
            post(deployment_actions::set_tier),
        )
        .route(
            "/deployments/{deployment_id}/delete",
            post(deployment_actions::delete),
        )
        .route(
            "/members/{member_id}/suspend",
            post(member_actions::suspend),
        )
        .route(
            "/members/{member_id}/unsuspend",
            post(member_actions::unsuspend),
        )
        .route(
            "/members/{member_id}/super_admin",
            post(member_actions::set_super_admin),
        )
        .route("/members/{member_id}/delete", post(member_actions::delete))
        .route(
            "/deployments/{deployment_id}/access",
            post(break_glass::grant_access),
        )
        .route(
            "/teams",
            get(team_actions::list_teams).post(team_actions::create_team),
        )
        .route("/teams/{team_id}", post(team_actions::rename_team))
        .route("/teams/{team_id}/delete", post(team_actions::delete_team))
}
