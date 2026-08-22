//! GET /api/admin/members
//!
//! Every member on the instance with their team memberships and flags.
//! Read-only in Phase 1; the grant/revoke and suspend mutations land in
//! Phase 2.

use axum::{
    extract::State,
    Json,
};
use serde::Serialize;

use crate::{
    auth::super_admin::SuperAdmin,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
    storage::AdminMemberRow,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMembersResponse {
    pub members: Vec<AdminMemberRow>,
}

#[utoipa::path(
    get,
    path = "/api/admin/members",
    responses(
        (status = 200, body = AdminMembersResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn list_members(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
) -> ApiResult<Json<AdminMembersResponse>> {
    let members = state
        .storage
        .list_all_members()
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(AdminMembersResponse { members }))
}
