//! Cross-tenant reads for the instance admin console.
//!
//! Every other query module in `storage` scopes to a team, project, or
//! deployment. These deliberately do not — they exist to answer "what is on
//! this instance?" and are only ever reachable behind the `SuperAdmin`
//! extractor.

use serde::Serialize;

use super::{
    teams::{
        map_team,
        TeamRecord,
    },
    Storage,
};

/// One team membership, flattened for the admin member table.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberTeamRef {
    pub team_id: i64,
    pub team_slug: String,
    pub team_name: String,
    pub role: String,
}

/// A member as the admin console sees them: identity, flags, and every team
/// they belong to.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMemberRow {
    pub id: i64,
    pub primary_email: String,
    pub name: Option<String>,
    pub creation_time: i64,
    pub is_super_admin: bool,
    pub suspended: bool,
    pub teams: Vec<MemberTeamRef>,
}

/// A deployment as the admin console sees it, with its owning team and
/// project resolved.
///
/// `intended_state` is what the database says should be running. What is
/// *actually* running is a docker question, filled in by the fleet route —
/// this query cannot know it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminDeploymentRow {
    pub id: i64,
    pub name: String,
    pub deployment_type: String,
    pub intended_state: String,
    pub tier: String,
    pub url: String,
    pub creation_time: i64,
    pub team_id: i64,
    pub team_slug: String,
    pub project_id: i64,
    pub project_slug: String,
}

impl Storage {
    /// Every team on the instance, oldest first.
    pub async fn list_all_teams(&self) -> anyhow::Result<Vec<TeamRecord>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT id, name, slug, creator_id, creation_time
                   FROM teams
                  ORDER BY creation_time ASC, id ASC",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(map_team).collect())
    }

    /// Every non-deleted member, with their team memberships attached.
    ///
    /// One LEFT JOIN rather than N+1: the console renders the whole table at
    /// once and an instance can have hundreds of members.
    ///
    /// Ordered by `m.id`, not `m.creation_time`. Creation times are
    /// millisecond-precision, so two members registered in the same
    /// millisecond could interleave and the run-grouping below would split
    /// one member across two rows. `id` is unique, so the grouping is exact.
    pub async fn list_all_members(&self) -> anyhow::Result<Vec<AdminMemberRow>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT m.id, m.primary_email, m.name, m.creation_time,
                        m.is_super_admin, m.suspended,
                        t.id, t.slug, t.name, tm.role
                   FROM members m
                   LEFT JOIN team_members tm ON tm.member_id = m.id
                   LEFT JOIN teams t ON t.id = tm.team_id
                  WHERE m.deleted = FALSE
                  ORDER BY m.id ASC, t.slug ASC",
                &[],
            )
            .await?;

        let mut out: Vec<AdminMemberRow> = Vec::new();
        for row in rows {
            let id: i64 = row.get(0);
            if out.last().map(|m| m.id) != Some(id) {
                out.push(AdminMemberRow {
                    id,
                    primary_email: row.get(1),
                    name: row.get(2),
                    creation_time: row.get(3),
                    is_super_admin: row.get(4),
                    suspended: row.get(5),
                    teams: Vec::new(),
                });
            }
            // NULL when the member belongs to no team, which the LEFT JOIN
            // still emits one row for.
            let team_id: Option<i64> = row.get(6);
            if let Some(team_id) = team_id {
                out.last_mut()
                    .expect("a row was just pushed")
                    .teams
                    .push(MemberTeamRef {
                        team_id,
                        team_slug: row.get(7),
                        team_name: row.get(8),
                        role: row.get(9),
                    });
            }
        }
        Ok(out)
    }

    /// Every deployment on the instance with its owning team and project.
    pub async fn list_all_deployments_with_owners(
        &self,
    ) -> anyhow::Result<Vec<AdminDeploymentRow>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT d.id, d.name, d.deployment_type, d.state, d.tier, d.url,
                        d.creation_time,
                        t.id, t.slug, p.id, p.slug
                   FROM deployments d
                   JOIN projects p ON p.id = d.project_id
                   JOIN teams t ON t.id = p.team_id
                  ORDER BY d.creation_time ASC, d.id ASC",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| AdminDeploymentRow {
                id: row.get(0),
                name: row.get(1),
                deployment_type: row.get(2),
                intended_state: row.get(3),
                tier: row.get(4),
                url: row.get(5),
                creation_time: row.get(6),
                team_id: row.get(7),
                team_slug: row.get(8),
                project_id: row.get(9),
                project_slug: row.get(10),
            })
            .collect())
    }
}
