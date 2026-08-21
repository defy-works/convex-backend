//! Cross-tenant authorization tests.
//!
//! Builds independent tenants — separate members, teams, projects,
//! deployments — and asserts that one tenant's token is rejected on every
//! path-scoped route that names another tenant's resources.
//!
//! This is the regression guard for the authenticated-but-not-authorized
//! gap: `routes/helpers.rs` already documents the same bug being fixed once
//! for token routes, and the sweep stopped there.
//!
//! **The test owns the database** — it drops and recreates the schema. Point
//! `TEST_ORCHESTRATOR_DATABASE_URL` at a throwaway database.
//!
//! Run: `TEST_ORCHESTRATOR_DATABASE_URL=postgres://... cargo test -p
//! orchestrator --test cross_tenant -- --include-ignored`

use axum::{
    body::Body,
    http::{
        header::AUTHORIZATION,
        Request,
        StatusCode,
    },
    Router,
};
use orchestrator::{
    config::{
        OrchestratorConfig,
        ProvisionerMode,
        RegistrationMode,
    },
    provisioner::{
        ProvisionRequest,
        ProvisionResult,
        Provisioner,
    },
    router::build_router,
    state::OrchestratorState,
    storage::{
        access_tokens::NewAccessToken,
        deployments::NewDeployment,
        AccessTokenKind,
        DeploymentClass,
        DeploymentType,
        Storage,
        TeamRole,
    },
};
use tower::ServiceExt;

/// Stub provisioner so the tests don't depend on docker.
///
/// Duplicated from `integration.rs` rather than shared: Rust integration
/// test targets cannot import one another, and a `tests/common/` module
/// would be a third pattern this crate does not use.
struct StubProvisioner;

#[async_trait::async_trait]
impl Provisioner for StubProvisioner {
    async fn provision(&self, req: ProvisionRequest) -> anyhow::Result<ProvisionResult> {
        Ok(ProvisionResult {
            url: format!("http://{}.localhost:9000", req.deployment_name),
            site_url: format!("http://{}-site.localhost:9000", req.deployment_name),
            admin_key: format!("stub-admin-key-{}", req.deployment_name),
            admin_key_hash: "stub-hash".into(),
            admin_key_suffix: "stub".into(),
            instance_secret: "stub-instance-secret".into(),
            backend_instance_secret: "0".repeat(64),
            backend_pid: None,
            backend_port: 0,
            resolved_env: std::collections::BTreeMap::new(),
            sidecar_credentials: None,
        })
    }

    async fn teardown(&self, _deployment_name: &str, _storage_mode: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// One tenant's ids, plus a PAT that authenticates as its member.
struct Tenant {
    member_id: i64,
    team_id: i64,
    team_slug: String,
    project_id: i64,
    project_slug: String,
    deployment_id: i64,
    deployment_name: String,
    bearer: String,
}

async fn make_tenant(storage: &Storage, key: &str) -> Tenant {
    let member = storage
        .upsert_member(
            &format!("auth-{key}"),
            &format!("{key}@example.com"),
            Some(key),
        )
        .await
        .expect("member");
    let team = storage
        .create_team(key, key, Some(member.id))
        .await
        .expect("team");
    storage
        .add_team_member(team.id, member.id, TeamRole::Admin)
        .await
        .expect("membership");
    let project = storage
        .create_project(team.id, key, key, false)
        .await
        .expect("project");

    let name = format!("{key}-deployment");
    let empty = serde_json::json!({});
    let deployment = storage
        .create_deployment(NewDeployment {
            project_id: project.id,
            name: &name,
            deployment_type: DeploymentType::Prod,
            deployment_class: DeploymentClass::Standard,
            region: None,
            url: &format!("http://{name}.localhost"),
            site_url: &format!("http://{name}-site.localhost"),
            backend_pid: None,
            backend_port: 3210,
            creator_id: Some(member.id),
            preview_identifier: None,
            instance_secret: "",
            tier: "S16",
            knob_overrides: &empty,
            storage_mode: "volume-sqlite",
            pg_password: None,
            minio_root_user: None,
            minio_root_password: None,
            backend_instance_secret: None,
        })
        .await
        .expect("deployment");

    let secret = format!("{key}-secret");
    storage
        .create_access_token(NewAccessToken {
            public_id: &format!("{key}-public"),
            kind: AccessTokenKind::Pat,
            member_id: Some(member.id),
            team_id: Some(team.id),
            project_id: None,
            deployment_id: None,
            name: &format!("{key}-pat"),
            secret_hash: &orchestrator::auth::tokens::sha256_hex(&secret),
            secret_suffix: "cret",
            expiry: None,
        })
        .await
        .expect("pat");

    Tenant {
        member_id: member.id,
        team_id: team.id,
        team_slug: team.slug,
        project_id: project.id,
        project_slug: project.slug,
        deployment_id: deployment.id,
        deployment_name: name,
        bearer: format!("Bearer pat:{key}-public|{secret}"),
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    bearer: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(AUTHORIZATION, bearer);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        },
        None => Body::empty(),
    };
    app.clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap_or_else(|_| panic!("send {method} {path}"))
        .status()
}

/// `DROP SCHEMA public CASCADE; CREATE SCHEMA public;` so each run starts
/// clean. Plain `NoTls`; do not point at a TLS-only host without adapting.
async fn reset_public_schema(database_url: &str) {
    use tokio_postgres::NoTls;
    let (client, conn) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connect to test postgres for reset");
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("test postgres reset connection ended: {e}");
        }
    });
    client
        .batch_execute("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
        .await
        .expect("reset public schema");
}

fn test_config(database_url: String, data_root: std::path::PathBuf) -> OrchestratorConfig {
    OrchestratorConfig {
        database_url,
        data_root,
        public_origin: "http://localhost".into(),
        bootstrap_token: None,
        provisioner_mode: ProvisionerMode::External,
        service_key: None,
        admin_emails: Vec::new(),
        default_team_name: "Self-Hosted".into(),
        registration_mode: RegistrationMode::Allowlist,
        backend_image: "irrelevant".into(),
        backend_network: None,
        backend_container_prefix: "test-".into(),
        router_host: "localhost".into(),
        site_router_host: None,
        router_public_port: 9000,
        router_public_scheme: "http".into(),
        direct_backend_routing: true,
        enable_sidecars: false,
        postgres_image: "postgres:16-alpine".into(),
        minio_image: "quay.io/minio/minio:latest".into(),
        traefik_dynamic_dir: None,
        orchestrator_upstream: "orchestrator:8050".into(),
        traefik_cert_dir: "/dynamic".into(),
        acme_contact_email: None,
        acme_directory_url: None,
        reconcile_interval_secs: 0,
    }
}

/// `(method, path template, optional JSON body)`.
///
/// `{team}` / `{team_slug}` / `{project}` / `{project_slug}` /
/// `{deployment}` / `{deployment_name}` / `{member}` are substituted with
/// the **victim's** ids and called with the attacker's token.
///
/// A route belongs here if it names a tenant-owned resource in its path.
/// The `*_stub.rs` surfaces are excluded: they return canned data and own
/// no tenant state.
const CROSS_TENANT_ROUTES: &[(&str, &str, Option<&str>)] = &[
    // --- the two confirmed holes ---
    (
        "POST",
        "/api/dashboard/instances/{deployment_name}/auth",
        None,
    ),
    (
        "GET",
        "/api/dashboard/projects/{project}/environment_variables/list",
        None,
    ),
    (
        "POST",
        "/api/dashboard/projects/{project}/environment_variables/update_batch",
        Some(r#"{"variables":[{"name":"STOLEN","value":"x","deploymentTypes":["prod"]}]}"#),
    ),
    // --- dashboard: teams ---
    (
        "POST",
        "/api/dashboard/teams/{team}",
        Some(r#"{"name":"pwned"}"#),
    ),
    ("POST", "/api/dashboard/teams/{team}/delete", None),
    ("GET", "/api/dashboard/teams/{team}/members", None),
    (
        "POST",
        "/api/dashboard/teams/{team}/remove_member",
        Some(r#"{"memberId":1}"#),
    ),
    ("GET", "/api/dashboard/teams/{team}/invites", None),
    (
        "POST",
        "/api/dashboard/teams/{team}/invites",
        Some(r#"{"email":"intruder@example.com","role":"developer"}"#),
    ),
    (
        "GET",
        "/api/dashboard/teams/{team}/get_audit_log_events",
        None,
    ),
    ("GET", "/api/dashboard/teams/{team}/get_project_roles", None),
    ("GET", "/api/dashboard/teams/{team}/projects", None),
    // --- dashboard: projects ---
    ("GET", "/api/dashboard/projects/{project}", None),
    (
        "PUT",
        "/api/dashboard/projects/{project}",
        Some(r#"{"name":"pwned"}"#),
    ),
    ("POST", "/api/dashboard/delete_project/{project}", None),
    // --- dashboard: deployments and custom domains ---
    (
        "GET",
        "/api/dashboard/teams/{team}/deployments/{deployment}",
        None,
    ),
    (
        "GET",
        "/api/dashboard/deployments/{deployment}/canonical_urls",
        None,
    ),
    (
        "GET",
        "/api/dashboard/deployments/{deployment}/custom_domains/list",
        None,
    ),
    (
        "POST",
        "/api/dashboard/deployments/{deployment}/custom_domains/create",
        Some(r#"{"domain":"stolen.example.com","kind":"api"}"#),
    ),
    // --- management API ---
    ("GET", "/v1/projects/{project}", None),
    ("POST", "/v1/projects/{project}/delete", None),
    ("GET", "/v1/projects/{project}/settings", None),
    ("GET", "/v1/projects/{project}/list_deployments", None),
    (
        "GET",
        "/v1/projects/{project}/list_default_environment_variables",
        None,
    ),
    ("GET", "/v1/teams/{team}/list_projects", None),
    ("GET", "/v1/teams/{team}/list_members", None),
    ("GET", "/v1/teams/{team}/list_deployments", None),
    ("GET", "/v1/deployments/{deployment_name}", None),
    ("GET", "/v1/deployments/{deployment_name}/settings", None),
    ("POST", "/v1/deployments/{deployment_name}/delete", None),
    (
        "POST",
        "/v1/deployments/{deployment_name}/restart",
        Some("{}"),
    ),
];

/// One tenant must not touch another's resources on any route.
///
/// Each route gets a **freshly built victim**. Sharing one victim would let
/// an early destructive route that succeeds — `delete_team`, say — make
/// every later route 404 and look safe, masking the holes behind it.
///
/// Both 403 and 404 are accepted: some handlers legitimately prefer not to
/// confirm a resource exists. What is not accepted is 2xx.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn one_tenant_cannot_touch_another() {
    use std::sync::Arc;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir");
    let config = test_config(database_url, data_root.path().to_path_buf());
    let mut state = OrchestratorState::new(config).await.expect("state");
    state.provisioner = Arc::new(StubProvisioner);
    let app = build_router(state.clone());

    let attacker = make_tenant(&state.storage, "attacker").await;

    let mut leaks = Vec::new();
    // A route that answers 400 or 422 rejected the *request*, not the
    // caller — usually a malformed body in the table above. That is not
    // evidence of authorization, so it is reported separately rather than
    // being quietly counted as a pass.
    let mut inconclusive = Vec::new();
    for (i, (method, template, body)) in CROSS_TENANT_ROUTES.iter().enumerate() {
        let victim = make_tenant(&state.storage, &format!("victim{i}")).await;
        let path = template
            .replace("{team_slug}", &victim.team_slug)
            .replace("{team}", &victim.team_id.to_string())
            .replace("{project_slug}", &victim.project_slug)
            .replace("{project}", &victim.project_id.to_string())
            .replace("{deployment_name}", &victim.deployment_name)
            .replace("{deployment}", &victim.deployment_id.to_string())
            .replace("{member}", &victim.member_id.to_string());
        let body = body.map(|b| serde_json::from_str(b).expect("table body is valid JSON"));

        let status = send(&app, method, &path, &attacker.bearer, body).await;
        match status {
            s if s.is_success() => leaks.push(format!("{method} {template} -> {s}")),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {},
            s => inconclusive.push(format!("{method} {template} -> {s}")),
        }
    }

    // Sanity first, so a wholesale breakage cannot masquerade as a pass.
    let own = send(
        &app,
        "GET",
        &format!("/api/dashboard/projects/{}", attacker.project_id),
        &attacker.bearer,
        None,
    )
    .await;
    assert!(
        own.is_success(),
        "a tenant must still reach its own project, got {own}"
    );

    assert!(
        inconclusive.is_empty(),
        "{} route(s) answered with neither success nor a denial, so this test proves nothing \
         about them — fix the request body in CROSS_TENANT_ROUTES so the handler is actually \
         reached:\n  {}",
        inconclusive.len(),
        inconclusive.join("\n  ")
    );

    assert!(
        leaks.is_empty(),
        "{} of {} cross-tenant route(s) allowed access to another tenant's resources:\n  {}",
        leaks.len(),
        CROSS_TENANT_ROUTES.len(),
        leaks.join("\n  ")
    );
}

/// A deploy key is bound to one deployment. It must not act on another,
/// even one belonging to a different tenant — a key committed to CI has the
/// blast radius of its own deployment and nothing more.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn a_deploy_key_cannot_act_on_another_deployment() {
    use std::sync::Arc;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir");
    let config = test_config(database_url, data_root.path().to_path_buf());
    let mut state = OrchestratorState::new(config).await.expect("state");
    state.provisioner = Arc::new(StubProvisioner);
    let app = build_router(state.clone());

    let owner = make_tenant(&state.storage, "keyowner").await;
    let victim = make_tenant(&state.storage, "keyvictim").await;

    // A prod deploy key bound to the owner's deployment. Its wire form
    // carries the deployment name in the middle slot.
    let secret = "deploy-secret";
    state
        .storage
        .create_access_token(NewAccessToken {
            public_id: &owner.deployment_name,
            kind: AccessTokenKind::DeployProd,
            member_id: Some(owner.member_id),
            team_id: Some(owner.team_id),
            project_id: Some(owner.project_id),
            deployment_id: Some(owner.deployment_id),
            name: "prod-key",
            secret_hash: &orchestrator::auth::tokens::sha256_hex(secret),
            secret_suffix: "cret",
            expiry: None,
        })
        .await
        .expect("deploy key");
    let bearer = format!("Bearer prod:{}|{secret}", owner.deployment_name);

    let foreign = send(
        &app,
        "GET",
        &format!(
            "/api/deployment/{}/team_and_project",
            victim.deployment_name
        ),
        &bearer,
        None,
    )
    .await;
    assert!(
        !foreign.is_success(),
        "a deploy key reached another deployment's team_and_project: {foreign}"
    );

    // Minting an admin key for a deployment outside the key's own project
    // must be refused too — this route returns a real admin key.
    let foreign_auth = send(
        &app,
        "POST",
        "/api/deployment/authorize_within_current_project",
        &bearer,
        Some(serde_json::json!({
            "selectedDeploymentName": victim.deployment_name,
            "selectedDeploymentType": "prod",
        })),
    )
    .await;
    assert!(
        !foreign_auth.is_success(),
        "a deploy key minted an admin key for another tenant's deployment: {foreign_auth}"
    );

    // ...and it must still work against its own deployment, or the CLI is
    // broken rather than secured.
    let own = send(
        &app,
        "GET",
        &format!("/api/deployment/{}/team_and_project", owner.deployment_name),
        &bearer,
        None,
    )
    .await;
    assert!(
        own.is_success(),
        "a deploy key must still reach its own deployment, got {own}"
    );
}
