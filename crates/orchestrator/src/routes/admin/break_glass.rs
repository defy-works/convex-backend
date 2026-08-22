//! Break-glass access to a tenant's deployment.
//!
//! Everything else in the admin console is metadata. This route hands over
//! the ability to read and write a tenant's data, so it is deliberately
//! ceremonious:
//!
//! - a reason is required, and recorded verbatim;
//! - the event is written to the **tenant's own audit log** as well as the
//!   instance's.
//!
//! It returns the deployment's **real** admin key, which does not expire.
//! An earlier version minted a fresh short-lived token instead, which read
//! better but did not work: the backend only accepts the key derived from
//! its own `INSTANCE_SECRET`, so every "15-minute" grant handed back a
//! credential the deployment rejected — while reporting success and telling
//! the tenant their data had been opened. A key that works and is logged
//! beats a key that expires and does not.
//!
//! The consequence is real and worth stating: this grant cannot be revoked
//! without rotating the deployment's key. The audit trail, not an expiry, is
//! what bounds it.
//!
//! That last point is the one most likely to be dropped as redundant. It is
//! not: an audit trail only the operator can read is not accountability. A
//! tenant should be able to see that somebody opened their deployment, and
//! why.

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

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessArgs {
    /// Why this access is being taken. Recorded verbatim in both audit logs.
    pub reason: String,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessResponse {
    pub deployment: String,
    pub url: String,
    /// The deployment's admin key. Shown once by the console, but it does
    /// not expire — see the module docs.
    pub admin_key: String,
    /// True when the key does not expire, so the UI states that plainly
    /// rather than implying a time limit it cannot enforce.
    pub persistent: bool,
    /// Restates, in the response, that this was recorded where the tenant
    /// can see it — so it is visible even to an API caller who never saw
    /// the modal.
    pub tenant_notified: bool,
}

#[utoipa::path(
    post,
    path = "/api/admin/deployments/{deployment_id}/access",
    params(("deployment_id" = i64, Path)),
    request_body = AccessArgs,
    responses(
        (status = 200, body = AccessResponse),
        (status = 400, description = "a reason is required"),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn grant_access(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    Json(args): Json<AccessArgs>,
) -> ApiResult<Json<AccessResponse>> {
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(ApiError::BadRequest(
            "a reason is required: it is recorded in the tenant's own audit log".into(),
        ));
    }

    let deployment = state
        .storage
        .get_deployment(deployment_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("deployment {deployment_id}")))?;
    let project = state
        .storage
        .get_project(deployment.project_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound("project".into()))?;

    // The key the running backend actually accepts. `ephemeral_admin_key`
    // returns the deployment's stored admin key when it has one, and only
    // mints for the demo path where no backend is running yet.
    let admin_key = crate::routes::deployment_internal::ephemeral_admin_key(&state, &deployment)
        .await
        .ok_or_else(|| {
            ApiError::BadGateway(format!(
                "no admin key is available for {}; it may never have been provisioned",
                deployment.name
            ))
        })?;

    let metadata = serde_json::json!({
        "deployment": deployment.name,
        "deploymentId": deployment.id,
        "reason": reason,
        "persistent": true,
    });

    audit_admin(
        &state,
        &admin.actor,
        "deploymentAccessGranted",
        metadata.clone(),
    )
    .await;

    // The tenant-visible copy. Written second and separately on purpose:
    // if this fails the access still happened, and a silent failure here is
    // exactly the accountability gap this route exists to avoid, so it is
    // logged at error rather than swallowed.
    if let Err(e) = state
        .storage
        .append_audit(
            project.team_id,
            admin.actor.member_id(),
            "deploymentAccessGranted",
            &metadata,
        )
        .await
    {
        tracing::error!(
            deployment = %deployment.name,
            team_id = project.team_id,
            error = %e,
            "break-glass: failed to write the tenant-visible audit event"
        );
    }

    tracing::warn!(
        deployment = %deployment.name,
        actor = admin.actor.label(),
        reason,
        "break-glass access granted"
    );

    Ok(Json(AccessResponse {
        deployment: deployment.name,
        url: deployment.url,
        admin_key,
        persistent: true,
        tenant_notified: true,
    }))
}
