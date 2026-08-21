use tokio_postgres::Row;

use super::Storage;
use crate::time::now_unix_ms;

#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub id: i64,
    pub auth_user_id: String,
    pub primary_email: String,
    pub name: Option<String>,
    pub creation_time: i64,
    pub deleted: bool,
    /// Instance-wide operator. See `auth::super_admin`.
    pub is_super_admin: bool,
    /// Reversible authentication block. Distinct from `deleted`.
    pub suspended: bool,
}

impl Storage {
    /// Find-or-create a member by their BetterAuth `auth_user_id`. Email and
    /// name are updated if they've changed upstream.
    pub async fn upsert_member(
        &self,
        auth_user_id: &str,
        email: &str,
        name: Option<&str>,
    ) -> anyhow::Result<MemberRecord> {
        let now = now_unix_ms();
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_one(
                "INSERT INTO members (auth_user_id, primary_email, name, creation_time)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (auth_user_id) DO UPDATE SET
                     primary_email = EXCLUDED.primary_email,
                     name = COALESCE(EXCLUDED.name, members.name)
                 RETURNING id, auth_user_id, primary_email, name, creation_time, deleted,
                        is_super_admin, suspended",
                &[&auth_user_id, &email, &name, &now],
            )
            .await?;
        Ok(map_member(row))
    }

    pub async fn get_member(&self, id: i64) -> anyhow::Result<Option<MemberRecord>> {
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_opt(
                "SELECT id, auth_user_id, primary_email, name, creation_time, deleted,
                        is_super_admin, suspended
                 FROM members WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(row.map(map_member))
    }

    pub async fn get_member_by_auth_user_id(
        &self,
        auth_user_id: &str,
    ) -> anyhow::Result<Option<MemberRecord>> {
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_opt(
                "SELECT id, auth_user_id, primary_email, name, creation_time, deleted,
                        is_super_admin, suspended
                 FROM members WHERE auth_user_id = $1 AND deleted = FALSE",
                &[&auth_user_id],
            )
            .await?;
        Ok(row.map(map_member))
    }

    pub async fn get_member_by_email(&self, email: &str) -> anyhow::Result<Option<MemberRecord>> {
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_opt(
                "SELECT id, auth_user_id, primary_email, name, creation_time, deleted,
                        is_super_admin, suspended
                 FROM members WHERE primary_email = $1 AND deleted = FALSE",
                &[&email],
            )
            .await?;
        Ok(row.map(map_member))
    }

    pub async fn count_members(&self) -> anyhow::Result<i64> {
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_one(
                "SELECT COUNT(*)::BIGINT FROM members WHERE deleted = FALSE",
                &[],
            )
            .await?;
        Ok(row.get(0))
    }

    pub async fn update_member_name(&self, id: i64, name: &str) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        conn.client()
            .execute("UPDATE members SET name = $1 WHERE id = $2", &[&name, &id])
            .await?;
        Ok(())
    }

    pub async fn delete_member(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        conn.client()
            .execute("UPDATE members SET deleted = TRUE WHERE id = $1", &[&id])
            .await?;
        Ok(())
    }

    /// Grant or revoke instance-wide operator rights.
    ///
    /// Revocation is guarded against removing the last remaining
    /// super-admin. The guard is a subquery inside the same statement, not
    /// a read-then-write, so two concurrent revokes cannot both observe a
    /// count of 2 and both succeed.
    pub async fn set_super_admin(&self, member_id: i64, value: bool) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        let affected = if value {
            conn.client()
                .execute(
                    "UPDATE members SET is_super_admin = TRUE
                      WHERE id = $1 AND deleted = FALSE",
                    &[&member_id],
                )
                .await?
        } else {
            conn.client()
                .execute(
                    "UPDATE members SET is_super_admin = FALSE
                      WHERE id = $1
                        AND deleted = FALSE
                        AND (SELECT count(*) FROM members
                              WHERE is_super_admin = TRUE AND deleted = FALSE) > 1",
                    &[&member_id],
                )
                .await?
        };
        if affected == 0 {
            if value {
                anyhow::bail!("member {member_id} not found");
            }
            anyhow::bail!(
                "refusing to revoke the last super-admin (member {member_id}); grant another \
                 operator first"
            );
        }
        Ok(())
    }

    /// Suspend or unsuspend a member. Suspension blocks authentication but
    /// preserves team membership, projects, and audit history.
    pub async fn set_member_suspended(&self, member_id: i64, value: bool) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        let affected = conn
            .client()
            .execute(
                "UPDATE members SET suspended = $2 WHERE id = $1 AND deleted = FALSE",
                &[&member_id, &value],
            )
            .await?;
        if affected == 0 {
            anyhow::bail!("member {member_id} not found");
        }
        Ok(())
    }
}

fn map_member(row: Row) -> MemberRecord {
    MemberRecord {
        id: row.get(0),
        auth_user_id: row.get(1),
        primary_email: row.get(2),
        name: row.get(3),
        creation_time: row.get(4),
        deleted: row.get(5),
        is_super_admin: row.get(6),
        suspended: row.get(7),
    }
}
