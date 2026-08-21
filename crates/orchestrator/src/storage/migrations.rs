use super::{
    pool::PgPool,
    schema::SCHEMA_SQL,
};

pub async fn run(pool: &PgPool) -> anyhow::Result<()> {
    let conn = pool.acquire().await?;
    conn.client().batch_execute(SCHEMA_SQL).await?;
    // One-shot cleanup: earlier builds soft-deleted projects (`deleted = TRUE`),
    // which kept the `UNIQUE(team_id, slug)` slot busy and blocked re-creating
    // a project with the same slug. We now hard-delete on Delete Project; sweep
    // any leftover tombstones here so existing deployments stop "holding" slugs
    // they no longer use.
    conn.client()
        .batch_execute("DELETE FROM projects WHERE deleted = TRUE")
        .await?;
    // Project-backend-knobs migration. Idempotent — `IF NOT EXISTS` on each
    // column means re-running this against an already-migrated DB is a no-op.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE projects
              ADD COLUMN IF NOT EXISTS tier TEXT NOT NULL DEFAULT 'S16',
              ADD COLUMN IF NOT EXISTS knob_overrides JSONB NOT NULL DEFAULT '{}'::jsonb;
            ALTER TABLE deployments
              ADD COLUMN IF NOT EXISTS tier TEXT NOT NULL DEFAULT 'S16',
              ADD COLUMN IF NOT EXISTS knob_overrides JSONB NOT NULL DEFAULT '{}'::jsonb;
            "#,
        )
        .await?;
    // Phase B migration: per-deployment desired settings columns.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE deployments
              ADD COLUMN IF NOT EXISTS desired_tier TEXT,
              ADD COLUMN IF NOT EXISTS desired_overrides JSONB NOT NULL DEFAULT '{}'::jsonb;
            "#,
        )
        .await?;
    // v3 migration: per-deployment storage mode + sidecar credentials.
    // Existing rows default to 'volume-sqlite' (v2 behavior preserved).
    // Credentials are plaintext at rest — encrypt the orchestrator DB at
    // the disk layer. The CHECK constraint from schema.rs only applies to
    // fresh DB inits; orchestrator-side validation enforces the same
    // invariant for ALTERed DBs.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE deployments
              ADD COLUMN IF NOT EXISTS storage_mode TEXT NOT NULL DEFAULT 'volume-sqlite',
              ADD COLUMN IF NOT EXISTS pg_password TEXT,
              ADD COLUMN IF NOT EXISTS minio_root_user TEXT,
              ADD COLUMN IF NOT EXISTS minio_root_password TEXT;
            "#,
        )
        .await?;
    // Hotfix migration: persist the 64-hex INSTANCE_SECRET env var
    // separately from the `instance_secret` column (which actually holds
    // the backend-produced admin key). Without this, restart_deployment
    // would feed the admin key back as INSTANCE_SECRET and the backend
    // would fail to hex-decode it.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE deployments
              ADD COLUMN IF NOT EXISTS backend_instance_secret TEXT;
            "#,
        )
        .await?;
    // Operator-chosen canonical URLs. NULL keeps the derived
    // `<name>.<router_host>` behaviour, so existing deployments are untouched.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE deployments
              ADD COLUMN IF NOT EXISTS desired_url TEXT,
              ADD COLUMN IF NOT EXISTS desired_site_url TEXT;
            "#,
        )
        .await?;
    // Custom-domain certificate management. `last_error` carries the reason
    // the last issuance attempt failed, verbatim, for the dashboard to show.
    //
    // The DNS-01 columns are dropped rather than added: a short-lived build
    // shipped `challenge_type` and `dns_credential_id` (the latter with a
    // foreign key to `dns_provider_credentials`) before DNS-01 was removed.
    // Both drops and the table drop are idempotent, and no live domain ever
    // used anything but http-01, so nothing is lost. Ordering matters —
    // the column referencing the table must go before the table itself.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE custom_domains
              ADD COLUMN IF NOT EXISTS last_error TEXT,
              ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'api',
              ADD COLUMN IF NOT EXISTS tls_mode TEXT NOT NULL DEFAULT 'acme',
              ADD COLUMN IF NOT EXISTS verification_token TEXT,
              DROP COLUMN IF EXISTS challenge_type,
              DROP COLUMN IF EXISTS dns_credential_id;
            DROP TABLE IF EXISTS dns_provider_credentials;
            -- Backfill rows added before verification existed. gen_random_uuid()
            -- is in core Postgres from 13 on, so no extension is required.
            UPDATE custom_domains
               SET verification_token = replace(gen_random_uuid()::text, '-', '')
             WHERE verification_token IS NULL;
            "#,
        )
        .await?;
    // P1 admin-console migration. `is_super_admin` is the runtime authority
    // for instance-wide operators (ADMIN_EMAILS only seeds it at sign-in).
    // `suspended` is a reversible block, distinct from the permanent
    // `deleted`. Both default FALSE so every existing row is unaffected.
    //
    // The audit change makes `team_id` nullable so instance-scoped events
    // have somewhere to live. Existing per-team queries filter on
    // `team_id = $1` and so continue to exclude them without modification.
    //
    // The `CHECK (scope IN ('team','instance'))` in schema.rs applies only to
    // fresh inits, matching the convention already used for `storage_mode`
    // above; orchestrator-side code is what enforces the invariant on
    // ALTERed databases.
    conn.client()
        .batch_execute(
            r#"
            ALTER TABLE members
              ADD COLUMN IF NOT EXISTS is_super_admin BOOLEAN NOT NULL DEFAULT FALSE,
              ADD COLUMN IF NOT EXISTS suspended BOOLEAN NOT NULL DEFAULT FALSE;
            ALTER TABLE audit_log_events
              ALTER COLUMN team_id DROP NOT NULL,
              ADD COLUMN IF NOT EXISTS scope TEXT NOT NULL DEFAULT 'team';
            CREATE INDEX IF NOT EXISTS audit_instance_time_idx
              ON audit_log_events(creation_time) WHERE scope = 'instance';
            "#,
        )
        .await?;
    Ok(())
}
