//! Member governance for the instance admin console.
//!
//! Suspension is reversible and preserves the member's teams, projects, and
//! audit history; deletion is the permanent case. Both are audited, as is
//! every change to who can operate the instance.

use axum::{
    extract::{
        Path,
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
    routes::admin::audit_admin,
    state::OrchestratorState,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberActionResponse {
    pub member_id: i64,
    pub email: String,
    pub is_super_admin: bool,
    pub suspended: bool,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuperAdminArgs {
    pub grant: bool,
}

async fn reload(state: &OrchestratorState, id: i64) -> ApiResult<MemberActionResponse> {
    let m = state
        .storage
        .get_member(id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("member {id}")))?;
    Ok(MemberActionResponse {
        member_id: m.id,
        email: m.primary_email,
        is_super_admin: m.is_super_admin,
        suspended: m.suspended,
    })
}

#[utoipa::path(
    post,
    path = "/api/admin/members/{member_id}/suspend",
    params(("member_id" = i64, Path)),
    responses(
        (status = 200, body = MemberActionResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn suspend(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(member_id): Path<i64>,
) -> ApiResult<Json<MemberActionResponse>> {
    set_suspended(admin, state, member_id, true).await
}

#[utoipa::path(
    post,
    path = "/api/admin/members/{member_id}/unsuspend",
    params(("member_id" = i64, Path)),
    responses(
        (status = 200, body = MemberActionResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn unsuspend(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(member_id): Path<i64>,
) -> ApiResult<Json<MemberActionResponse>> {
    set_suspended(admin, state, member_id, false).await
}

async fn set_suspended(
    admin: SuperAdmin,
    state: OrchestratorState,
    member_id: i64,
    value: bool,
) -> ApiResult<Json<MemberActionResponse>> {
    let before = reload(&state, member_id).await?;
    state
        .storage
        .set_member_suspended(member_id, value)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        if value {
            "memberSuspended"
        } else {
            "memberUnsuspended"
        },
        serde_json::json!({ "memberId": member_id, "email": before.email }),
    )
    .await;

    Ok(Json(reload(&state, member_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/admin/members/{member_id}/super_admin",
    params(("member_id" = i64, Path)),
    request_body = SuperAdminArgs,
    responses(
        (status = 200, body = MemberActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 409, description = "would revoke the last super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn set_super_admin(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(member_id): Path<i64>,
    Json(args): Json<SuperAdminArgs>,
) -> ApiResult<Json<MemberActionResponse>> {
    let before = reload(&state, member_id).await?;
    state
        .storage
        .set_super_admin(member_id, args.grant)
        .await
        // The storage guard refuses to remove the last operator. That is the
        // caller's problem, not an internal fault, and the message says what
        // to do about it — so keep it and map to 409 rather than losing it
        // inside a 500.
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("last super-admin") {
                ApiError::Conflict(msg)
            } else {
                ApiError::Internal(e)
            }
        })?;

    audit_admin(
        &state,
        &admin.actor,
        if args.grant {
            "superAdminGranted"
        } else {
            "superAdminRevoked"
        },
        serde_json::json!({ "memberId": member_id, "email": before.email }),
    )
    .await;

    Ok(Json(reload(&state, member_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/admin/members/{member_id}/delete",
    params(("member_id" = i64, Path)),
    responses(
        (status = 200, body = MemberActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 409, description = "would remove the last super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn delete(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(member_id): Path<i64>,
) -> ApiResult<Json<MemberActionResponse>> {
    let before = reload(&state, member_id).await?;

    // Deleting an operator is a revoke with extra steps, so it has to clear
    // the flag first and inherit the same last-operator guard. Without this
    // the console would happily delete its way to an instance nobody can
    // administer.
    if before.is_super_admin {
        state
            .storage
            .set_super_admin(member_id, false)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("last super-admin") {
                    ApiError::Conflict(format!(
                        "{msg} (deleting this member would leave the instance with no operator)"
                    ))
                } else {
                    ApiError::Internal(e)
                }
            })?;
    }

    state
        .storage
        .delete_member(member_id)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        "memberDeleted",
        serde_json::json!({ "memberId": member_id, "email": before.email }),
    )
    .await;

    Ok(Json(MemberActionResponse {
        member_id,
        email: before.email,
        is_super_admin: false,
        suspended: before.suspended,
    }))
}
