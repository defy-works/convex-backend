//! Deployment lifecycle actions for the instance admin console.
//!
//! Two rules apply to every handler here.
//!
//! **The database row is the source of truth; container work is
//! best-effort.** If `docker stop` fails after the row is marked paused, the
//! action *did* take effect and the reconciler will converge — so the
//! response is a success carrying `containerWarning`, not a failure. A pause
//! that reports failure but did pause is worse than one that reports a
//! warning, because the operator retries and gets more confused.
//!
//! **Delete is the exception.** It must not report success while an orphan
//! container survives and the row is gone, because nothing will ever show
//! that container again.

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
    config::ProvisionerMode,
    errors::{
        ApiError,
        ApiResult,
    },
    provisioner::lifecycle,
    routes::admin::audit_admin,
    state::OrchestratorState,
    storage::DeploymentRecord,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionResponse {
    pub deployment: String,
    /// The row's state after the action.
    pub state: String,
    /// Set when the database changed but the container work did not fully
    /// succeed. The action still took effect; the reconciler will converge.
    pub container_warning: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestartArgs {
    /// Bypass the host capacity check, as the management route does.
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TierArgs {
    pub tier: String,
}

async fn load(state: &OrchestratorState, id: i64) -> ApiResult<DeploymentRecord> {
    state
        .storage
        .get_deployment(id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("deployment {id}")))
}

/// True when this provisioner actually owns containers. In `external` and
/// `process` modes there is nothing to stop or start, and pretending
/// otherwise would produce warnings about a daemon that was never involved.
fn owns_containers(state: &OrchestratorState) -> bool {
    matches!(state.config.provisioner_mode, ProvisionerMode::Docker)
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/pause",
    params(("deployment_id" = i64, Path)),
    responses(
        (status = 200, body = ActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 409, description = "deployment is still provisioning"),
    ),
    tag = "admin",
)]
pub(crate) async fn pause(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
) -> ApiResult<Json<ActionResponse>> {
    let d = load(&state, deployment_id).await?;
    state
        .storage
        .pause_deployment(d.id)
        .await
        .map_err(transition_error)?;

    let container_warning = if owns_containers(&state) {
        stop_all(&state, &d).await
    } else {
        None
    };

    audit_admin(
        &state,
        &admin.actor,
        "deploymentPaused",
        serde_json::json!({
            "deployment": d.name,
            "deploymentId": d.id,
            "containerWarning": container_warning,
        }),
    )
    .await;

    Ok(Json(ActionResponse {
        deployment: d.name,
        state: "paused".into(),
        container_warning,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/resume",
    params(("deployment_id" = i64, Path)),
    responses(
        (status = 200, body = ActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 409, description = "deployment is still provisioning"),
    ),
    tag = "admin",
)]
pub(crate) async fn resume(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
) -> ApiResult<Json<ActionResponse>> {
    let d = load(&state, deployment_id).await?;
    state
        .storage
        .resume_deployment(d.id)
        .await
        .map_err(transition_error)?;

    let container_warning = if owns_containers(&state) {
        start_all(&state, &d).await
    } else {
        None
    };

    audit_admin(
        &state,
        &admin.actor,
        "deploymentResumed",
        serde_json::json!({
            "deployment": d.name,
            "deploymentId": d.id,
            "containerWarning": container_warning,
        }),
    )
    .await;

    Ok(Json(ActionResponse {
        deployment: d.name,
        state: "running".into(),
        container_warning,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/restart",
    params(("deployment_id" = i64, Path)),
    request_body = RestartArgs,
    responses(
        (status = 200, body = ActionResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn restart(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    // Optional: every field is optional, so a bare POST means "restart with
    // defaults". Requiring a body would make callers send `{}` and get a 415
    // if they forgot the content-type header.
    args: Option<Json<RestartArgs>>,
) -> ApiResult<Json<ActionResponse>> {
    let args = args.map(|Json(a)| a).unwrap_or(RestartArgs { force: None });
    let d = load(&state, deployment_id).await?;
    // Delegates to the same routine the management API uses, so capacity
    // checks, tier resolution, and sidecar credential reuse cannot drift
    // between the two entry points.
    let updated = crate::routes::management::deployments::respawn_deployment(
        &state,
        &d,
        args.force.unwrap_or(false),
    )
    .await?;

    audit_admin(
        &state,
        &admin.actor,
        "deploymentRestarted",
        serde_json::json!({
            "deployment": updated.name,
            "deploymentId": updated.id,
            "force": args.force.unwrap_or(false),
        }),
    )
    .await;

    Ok(Json(ActionResponse {
        deployment: updated.name,
        state: updated.state.to_string(),
        container_warning: None,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/tier",
    params(("deployment_id" = i64, Path)),
    request_body = TierArgs,
    responses(
        (status = 200, body = ActionResponse),
        (status = 400, description = "unknown tier"),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn set_tier(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    Json(args): Json<TierArgs>,
) -> ApiResult<Json<ActionResponse>> {
    let d = load(&state, deployment_id).await?;
    if crate::provisioner::tiers::resolve(&args.tier).is_none() {
        return Err(ApiError::BadRequest(format!("unknown tier {}", args.tier)));
    }

    let previous = d.desired_tier.clone().unwrap_or_else(|| d.tier.clone());
    state
        .storage
        .update_deployment_settings(d.id, Some(Some(args.tier.as_str())), None)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        "deploymentTierChanged",
        serde_json::json!({
            "deployment": d.name,
            "deploymentId": d.id,
            "from": previous,
            "to": args.tier,
        }),
    )
    .await;

    Ok(Json(ActionResponse {
        deployment: d.name,
        state: d.state.to_string(),
        // The tier is `desired` until the container is recreated, which is a
        // separate deliberate action. Say so rather than implying it is live.
        container_warning: Some(
            "tier recorded as desired; restart the deployment to apply it".into(),
        ),
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/delete",
    params(("deployment_id" = i64, Path)),
    responses(
        (status = 200, body = ActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 502, description = "teardown failed; the deployment was not removed"),
    ),
    tag = "admin",
)]
pub(crate) async fn delete(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
) -> ApiResult<Json<ActionResponse>> {
    let d = load(&state, deployment_id).await?;

    // Unlike pause/resume, this one fails loudly. Removing the row while a
    // container survives orphans it: nothing in the console lists it again,
    // and it keeps holding its port and its volume.
    if owns_containers(&state)
        && let Err(e) = state.provisioner.teardown(&d.name, &d.storage_mode).await
    {
        tracing::error!(deployment = %d.name, error = %e, "admin: teardown failed; keeping the row");
        return Err(ApiError::BadGateway(format!(
            "teardown of {} failed, so the deployment was not removed: {e}",
            d.name
        )));
    }

    state
        .storage
        .delete_deployment(d.id)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        "deploymentDeleted",
        serde_json::json!({ "deployment": d.name, "deploymentId": d.id }),
    )
    .await;

    Ok(Json(ActionResponse {
        deployment: d.name,
        state: "deleted".into(),
        container_warning: None,
    }))
}

/// A refused transition is the caller's problem (the deployment is mid-
/// provision), not an internal fault, so it must not surface as a 500.
fn transition_error(e: anyhow::Error) -> ApiError {
    let msg = e.to_string();
    if msg.contains("provisioning") {
        ApiError::Conflict(msg)
    } else {
        ApiError::Internal(e)
    }
}

async fn stop_all(state: &OrchestratorState, d: &DeploymentRecord) -> Option<String> {
    let mut failures = Vec::new();
    for name in lifecycle::containers_for_pause(
        &state.config.backend_container_prefix,
        &d.name,
        &d.storage_mode,
    ) {
        if let Err(e) = lifecycle::stop_container(&name).await {
            failures.push(format!("{name}: {e}"));
        }
    }
    warning(failures)
}

async fn start_all(state: &OrchestratorState, d: &DeploymentRecord) -> Option<String> {
    let mut failures = Vec::new();
    for name in lifecycle::containers_for_resume(
        &state.config.backend_container_prefix,
        &d.name,
        &d.storage_mode,
    ) {
        if let Err(e) = crate::reconcile::start_container(&name).await {
            failures.push(format!("{name}: {e}"));
        }
    }
    warning(failures)
}

fn warning(failures: Vec<String>) -> Option<String> {
    if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    }
}
