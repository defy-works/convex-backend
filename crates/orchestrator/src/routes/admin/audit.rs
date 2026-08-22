//! GET /api/admin/audit
//!
//! The instance-scoped audit log — operator actions that belong to no single
//! team. Team-scoped events stay on the per-team dashboard route.

use axum::{
    extract::{
        Query,
        State,
    },
    Json,
};
use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    auth::super_admin::SuperAdmin,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
};

/// Default page size, and a ceiling so a console that forgets to paginate
/// cannot pull the whole table.
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AuditFilters {
    pub limit: Option<i64>,
}

/// One instance-scoped event on the wire.
///
/// Named distinctly from `storage::AuditEntry`, which is the team-scoped
/// row type and carries a `team_id` this surface has no use for.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceAuditEvent {
    pub id: i64,
    /// `None` for actions taken through the break-glass bootstrap
    /// credential, which has no human behind it.
    pub member_id: Option<i64>,
    /// Resolved server-side. `None` when there is no member, or the member
    /// has since been deleted.
    pub member_email: Option<String>,
    pub action: String,
    pub metadata: serde_json::Value,
    pub creation_time: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditResponse {
    pub events: Vec<InstanceAuditEvent>,
}

#[utoipa::path(
    get,
    path = "/api/admin/audit",
    params(AuditFilters),
    responses(
        (status = 200, body = AdminAuditResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn instance_audit(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Query(q): Query<AuditFilters>,
) -> ApiResult<Json<AdminAuditResponse>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let records = state
        .storage
        .list_instance_audit_with_actors(limit)
        .await
        .map_err(ApiError::Internal)?;
    let events = records
        .into_iter()
        .map(|r| InstanceAuditEvent {
            id: r.id,
            member_id: r.member_id,
            member_email: r.member_email,
            action: r.action,
            metadata: r.metadata,
            creation_time: r.creation_time,
        })
        .collect();
    Ok(Json(AdminAuditResponse { events }))
}
