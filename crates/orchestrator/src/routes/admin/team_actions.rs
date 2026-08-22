//! Team governance for the instance admin console.
//!
//! Deleting a team is the most destructive action here: it cascades to
//! every project and every deployment underneath. The response reports how
//! many deployments were torn down, and the count is available *before* the
//! operator confirms, so "delete this team" is never a surprise.

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
    ids::slugify,
    routes::admin::audit_admin,
    state::OrchestratorState,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminTeamRow {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub creation_time: i64,
    pub member_count: u32,
    pub project_count: u32,
    /// Shown in the delete confirmation, so the blast radius is visible
    /// before the operator commits to it.
    pub deployment_count: u32,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminTeamsResponse {
    pub teams: Vec<AdminTeamRow>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamArgs {
    pub name: String,
    /// Derived from the name when omitted.
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameTeamArgs {
    pub name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamActionResponse {
    pub team_id: i64,
    pub slug: String,
    /// How many deployments the action tore down. Zero for anything but a
    /// delete.
    pub deployments_removed: u32,
}

#[utoipa::path(
    get,
    path = "/api/admin/teams",
    responses(
        (status = 200, body = AdminTeamsResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn list_teams(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
) -> ApiResult<Json<AdminTeamsResponse>> {
    let teams = state
        .storage
        .list_all_teams_with_counts()
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(AdminTeamsResponse {
        teams: teams
            .into_iter()
            .map(|t| AdminTeamRow {
                id: t.id,
                name: t.name,
                slug: t.slug,
                creation_time: t.creation_time,
                member_count: t.member_count as u32,
                project_count: t.project_count as u32,
                deployment_count: t.deployment_count as u32,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/teams",
    request_body = CreateTeamArgs,
    responses(
        (status = 200, body = TeamActionResponse),
        (status = 400, description = "invalid name or slug already taken"),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn create_team(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Json(args): Json<CreateTeamArgs>,
) -> ApiResult<Json<TeamActionResponse>> {
    let name = args.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("a team name is required".into()));
    }
    let slug = args
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| slugify(name));

    let team = state
        .storage
        // The operator is not made a member: this is instance administration,
        // not "give myself a team". Membership is granted separately.
        .create_team(name, &slug, None)
        .await
        .map_err(|e| ApiError::BadRequest(format!("could not create team {slug}: {e}")))?;

    audit_admin(
        &state,
        &admin.actor,
        "teamCreated",
        serde_json::json!({ "teamId": team.id, "slug": team.slug, "name": team.name }),
    )
    .await;

    Ok(Json(TeamActionResponse {
        team_id: team.id,
        slug: team.slug,
        deployments_removed: 0,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/teams/{team_id}",
    params(("team_id" = i64, Path)),
    request_body = RenameTeamArgs,
    responses(
        (status = 200, body = TeamActionResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn rename_team(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(team_id): Path<i64>,
    Json(args): Json<RenameTeamArgs>,
) -> ApiResult<Json<TeamActionResponse>> {
    let name = args.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("a team name is required".into()));
    }
    let team = state
        .storage
        .get_team(team_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("team {team_id}")))?;

    // The slug is left alone deliberately: it is baked into deploy keys,
    // CLI config, and bookmarked URLs, so renaming the display name must
    // not silently break them.
    state
        .storage
        .update_team(team_id, Some(name), None)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        "teamRenamed",
        serde_json::json!({
            "teamId": team_id,
            "slug": team.slug,
            "from": team.name,
            "to": name,
        }),
    )
    .await;

    Ok(Json(TeamActionResponse {
        team_id,
        slug: team.slug,
        deployments_removed: 0,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/teams/{team_id}/delete",
    params(("team_id" = i64, Path)),
    responses(
        (status = 200, body = TeamActionResponse),
        (status = 403, description = "not a super-admin"),
        (status = 502, description = "a deployment could not be torn down"),
    ),
    tag = "admin",
)]
pub(crate) async fn delete_team(
    admin: SuperAdmin,
    State(state): State<OrchestratorState>,
    Path(team_id): Path<i64>,
) -> ApiResult<Json<TeamActionResponse>> {
    let team = state
        .storage
        .get_team(team_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("team {team_id}")))?;

    let projects = state
        .storage
        .list_projects(team_id)
        .await
        .map_err(ApiError::Internal)?;

    // Cascade explicitly rather than relying on the foreign keys. The rows
    // would go either way, but the *containers* would not: deleting the team
    // row alone orphans every backend it owned, still running and still
    // holding its ports and volumes, with nothing left to list them.
    let mut removed = 0u32;
    for p in &projects {
        let deployments = state
            .storage
            .list_deployments(p.id)
            .await
            .map_err(ApiError::Internal)?;
        removed += deployments.len() as u32;
        crate::routes::helpers::cascade_delete_project(&state, p.id)
            .await
            .map_err(|e| {
                ApiError::BadGateway(format!(
                    "tearing down project {} failed, so team {} was not deleted: {e}",
                    p.slug, team.slug
                ))
            })?;
    }

    state
        .storage
        .delete_team(team_id)
        .await
        .map_err(ApiError::Internal)?;

    audit_admin(
        &state,
        &admin.actor,
        "teamDeleted",
        serde_json::json!({
            "teamId": team_id,
            "slug": team.slug,
            "name": team.name,
            "projectsRemoved": projects.len(),
            "deploymentsRemoved": removed,
        }),
    )
    .await;

    Ok(Json(TeamActionResponse {
        team_id,
        slug: team.slug,
        deployments_removed: removed,
    }))
}
