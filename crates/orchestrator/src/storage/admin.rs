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

/// A team as the admin console lists it, with the counts the delete
/// confirmation needs.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminTeamCounts {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub creation_time: i64,
    pub member_count: i64,
    pub project_count: i64,
    pub deployment_count: i64,
}

impl Storage {
    /// Every team with its member, project, and deployment counts.
    ///
    /// Correlated subqueries rather than joins: a join across three
    /// one-to-many relations multiplies rows, and getting three independent
    /// counts out of that needs `count(DISTINCT ...)` on each, which is both
    /// slower and easier to get subtly wrong.
    pub async fn list_all_teams_with_counts(&self) -> anyhow::Result<Vec<AdminTeamCounts>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT t.id, t.name, t.slug, t.creation_time,
                        (SELECT count(*) FROM team_members tm WHERE tm.team_id = t.id),
                        (SELECT count(*) FROM projects p
                          WHERE p.team_id = t.id AND p.deleted = FALSE),
                        (SELECT count(*) FROM deployments d
                           JOIN projects p2 ON p2.id = d.project_id
                          WHERE p2.team_id = t.id AND p2.deleted = FALSE)
                   FROM teams t
                  ORDER BY t.creation_time ASC, t.id ASC",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| AdminTeamCounts {
                id: r.get(0),
                name: r.get(1),
                slug: r.get(2),
                creation_time: r.get(3),
                member_count: r.get(4),
                project_count: r.get(5),
                deployment_count: r.get(6),
            })
            .collect())
    }
}

/// An instance audit event with the actor's email resolved.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditRow {
    pub id: i64,
    pub member_id: Option<i64>,
    /// `None` for break-glass (no member) or a member since deleted.
    pub member_email: Option<String>,
    pub action: String,
    pub metadata: serde_json::Value,
    pub creation_time: i64,
}

impl Storage {
    /// Instance audit events, newest first, with actor emails joined.
    ///
    /// Joined here rather than looked up per row by the client: an audit
    /// page is exactly the place where N+1 requests show up as a visibly
    /// slow table.
    pub async fn list_instance_audit_with_actors(
        &self,
        limit: i64,
    ) -> anyhow::Result<Vec<AdminAuditRow>> {
        let limit = limit.clamp(1, 1000);
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT a.id, a.member_id, m.primary_email, a.action, a.metadata,
                        a.creation_time
                   FROM audit_log_events a
                   LEFT JOIN members m ON m.id = a.member_id
                  WHERE a.scope = 'instance'
                  ORDER BY a.creation_time DESC, a.id DESC
                  LIMIT $1",
                &[&limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| AdminAuditRow {
                id: r.get(0),
                member_id: r.get(1),
                member_email: r.get(2),
                action: r.get(3),
                metadata: r.get(4),
                creation_time: r.get(5),
            })
            .collect())
    }
}
