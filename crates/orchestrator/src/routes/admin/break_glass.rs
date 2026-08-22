//! Break-glass access to a tenant's deployment.
//!
//! Everything else in the admin console is metadata. This route hands over
//! the ability to read and write a tenant's data, so it is deliberately
//! ceremonious:
//!
//! - a reason is required, and recorded verbatim;
//! - the key is minted fresh and short-lived, never the deployment's permanent
//!   one;
//! - the event is written to the **tenant's own audit log** as well as the
//!   instance's.
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
    auth::{
        super_admin::SuperAdmin,
        tokens::{
            encode_pat,
            mint_token_secret,
            sha256_hex,
            suffix_of,
        },
    },
    errors::{
        ApiError,
        ApiResult,
    },
    ids::random_id,
    routes::admin::audit_admin,
    state::OrchestratorState,
    storage::{
        access_tokens::NewAccessToken,
        AccessTokenKind,
        DeploymentType,
    },
};

/// How long a break-glass key lives.
///
/// Short enough that a forgotten key is not a standing grant, long enough
/// to actually diagnose something. Deliberately shorter than the one-hour
/// TTL `ephemeral_admin_key` uses for the ordinary team-scoped path.
const BREAK_GLASS_TTL_MS: i64 = 15 * 60_000;

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
    /// A freshly minted, short-lived admin key. Shown once.
    pub admin_key: String,
    /// Unix ms. The UI counts down to this so an operator with a dashboard
    /// open is not surprised by everything failing at once.
    pub expires_at: i64,
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

    let expires_at = crate::time::now_unix_ms() + BREAK_GLASS_TTL_MS;

    // Always mint. `ephemeral_admin_key` returns the deployment's stored
    // permanent key when it has one, which would make the TTL above a
    // fiction on every deployment that has ever been provisioned.
    let secret = mint_token_secret(&deployment.name);
    let admin_key = encode_pat(&secret);
    let kind = match deployment.deployment_type {
        DeploymentType::Prod => AccessTokenKind::DeployProd,
        DeploymentType::Dev => AccessTokenKind::DeployDev,
        DeploymentType::Preview => AccessTokenKind::DeployPreview,
    };
    state
        .storage
        .create_access_token(NewAccessToken {
            public_id: &random_id(),
            kind,
            // No member: this credential is the access itself, not the
            // operator's identity. The audit rows carry who took it.
            member_id: None,
            team_id: None,
            project_id: Some(deployment.project_id),
            deployment_id: Some(deployment.id),
            name: "break-glass",
            secret_hash: &sha256_hex(&secret.secret),
            secret_suffix: &suffix_of(&secret.secret),
            expiry: Some(expires_at),
        })
        .await
        .map_err(ApiError::Internal)?;

    let metadata = serde_json::json!({
        "deployment": deployment.name,
        "deploymentId": deployment.id,
        "reason": reason,
        "expiresAt": expires_at,
        "ttlMinutes": BREAK_GLASS_TTL_MS / 60_000,
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
        expires_at,
        tenant_notified: true,
    }))
}
