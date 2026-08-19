//! Shared helpers used by route handlers.

use orchestrator_api_types::dashboard::{
    DeploymentResponse,
    ProjectResponse,
    TeamResponse,
};
use orchestrator_api_types::management::{
    PlatformDeploymentResponse,
    PlatformProjectDetails,
};

use crate::{
    auth::identity::AuthIdentity,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
    storage::{
        AccessToken,
        DeploymentRecord,
        ProjectRecord,
        TeamRecord,
    },
};

/// Resolve the caller to a member and confirm they belong to `team_id`.
///
/// Every route that reads or mutates team-owned state has to do this. Several
/// token routes took `_auth: AuthIdentity` and threw it away, which authenticated
/// the caller but never authorized them — any valid token in the system could
/// read or revoke another team's tokens.
pub async fn require_team_member(
    state: &OrchestratorState,
    auth: &AuthIdentity,
    team_id: i64,
) -> ApiResult<i64> {
    let member_id = auth.require_member()?;
    if state
        .storage
        .get_team_role(team_id, member_id)
        .await
        .map_err(ApiError::Internal)?
        .is_none()
    {
        return Err(ApiError::Forbidden);
    }
    Ok(member_id)
}

/// Same, but for a project: resolves the project's owning team first.
pub async fn require_project_member(
    state: &OrchestratorState,
    auth: &AuthIdentity,
    project_id: i64,
) -> ApiResult<i64> {
    let project = state
        .storage
        .get_project(project_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id}")))?;
    require_team_member(state, auth, project.team_id).await
}

/// The team that ultimately owns an access token, whichever scope it carries.
/// `None` means the token is bound to nothing team-shaped (a bare personal
/// token), so ownership is decided by `member_id` alone.
async fn owning_team_of_token(
    state: &OrchestratorState,
    token: &AccessToken,
) -> ApiResult<Option<i64>> {
    if let Some(team_id) = token.team_id {
        return Ok(Some(team_id));
    }
    let project_id = match (token.project_id, token.deployment_id) {
        (Some(project_id), _) => Some(project_id),
        (None, Some(deployment_id)) => state
            .storage
            .get_deployment(deployment_id)
            .await
            .map_err(ApiError::Internal)?
            .map(|d| d.project_id),
        (None, None) => None,
    };
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    Ok(state
        .storage
        .get_project(project_id)
        .await
        .map_err(ApiError::Internal)?
        .map(|p| p.team_id))
}

/// Authorize revoking one access token.
///
/// A caller may revoke their own tokens, and tokens owned by a team they belong
/// to (deploy keys are team infrastructure, so a teammate revoking one is
/// legitimate). Anything else is someone else's credential.
pub async fn require_can_revoke_token(
    state: &OrchestratorState,
    auth: &AuthIdentity,
    public_id: &str,
) -> ApiResult<()> {
    let member_id = auth.require_member()?;
    let token = state
        .storage
        .get_access_token_by_public_id(public_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound("access token".into()))?;

    if token.member_id == Some(member_id) {
        return Ok(());
    }
    match owning_team_of_token(state, &token).await? {
        Some(team_id) => {
            require_team_member(state, auth, team_id).await?;
            Ok(())
        },
        // Not ours and not attached to any team we could belong to.
        None => Err(ApiError::Forbidden),
    }
}

/// Soft-delete a project AND tear down every backend container that
/// belongs to it. Called from each delete-project route (dashboard,
/// deployment_internal, management) so cleanup is uniform.
///
/// Teardown errors are logged but don't block the project delete — once
/// the project row is marked deleted the orphaned container is invisible
/// to the dashboard, and `docker rm -f` is best-effort anyway.
pub async fn cascade_delete_project(
    state: &OrchestratorState,
    project_id: i64,
) -> anyhow::Result<()> {
    let deployments = state.storage.list_deployments(project_id).await?;
    for d in deployments {
        if let Err(e) = state.provisioner.teardown(&d.name, &d.storage_mode).await {
            tracing::warn!(
                project_id,
                deployment = %d.name,
                error = %e,
                "teardown failed during cascade project delete; continuing",
            );
        }
        if let Err(e) = state.storage.delete_deployment(d.id).await {
            tracing::warn!(
                project_id,
                deployment = %d.name,
                error = %e,
                "deployment row delete failed during cascade; continuing",
            );
        }
    }
    state.storage.delete_project(project_id).await?;
    Ok(())
}

pub fn team_to_response(t: &TeamRecord) -> TeamResponse {
    TeamResponse {
        id: t.id as u64,
        name: t.name.clone(),
        slug: t.slug.clone(),
        creator: t.creator_id.map(|c| c as u64),
    }
}

pub fn project_to_response(p: &ProjectRecord) -> ProjectResponse {
    ProjectResponse {
        id: p.id as u64,
        team_id: p.team_id as u64,
        name: p.name.clone(),
        slug: p.slug.clone(),
        is_demo: p.is_demo,
        creation_time: p.creation_time as f64,
    }
}

pub fn project_to_platform(p: &ProjectRecord) -> PlatformProjectDetails {
    PlatformProjectDetails {
        id: p.id as u64,
        team_id: p.team_id as u64,
        name: p.name.clone(),
        slug: p.slug.clone(),
        is_demo: p.is_demo,
        creation_time: p.creation_time as f64,
    }
}

pub fn deployment_to_response(d: &DeploymentRecord) -> DeploymentResponse {
    DeploymentResponse {
        id: d.id as u64,
        project_id: d.project_id as u64,
        name: d.name.clone(),
        deployment_type: d.deployment_type.to_string(),
        deployment_class: d.deployment_class.to_string(),
        url: d.url.clone(),
        site_url: d.site_url.clone(),
        state: d.state.to_string(),
        creation_time: d.creation_time as f64,
        region: d.region.clone(),
        preview_identifier: d.preview_identifier.clone(),
    }
}

pub fn deployment_to_platform(d: &DeploymentRecord) -> PlatformDeploymentResponse {
    PlatformDeploymentResponse {
        id: d.id as u64,
        project_id: d.project_id as u64,
        name: d.name.clone(),
        kind: d.deployment_type.to_string(),
        deployment_class: d.deployment_class.to_string(),
        url: d.url.clone(),
        site_url: d.site_url.clone(),
        state: d.state.to_string(),
        creation_time: d.creation_time as f64,
        region: d.region.clone(),
        preview_identifier: d.preview_identifier.clone(),
        tier: d.tier.clone(),
    }
}
