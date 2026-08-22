//! Integration tests for `convex-orchestrator`.
//!
//! Two layers, per the plan in
//! `docs/superpowers/specs/2026-05-02-convex-orchestrator-plan.md`:
//!
//! 1. **Default-run, no DB.** Asserts that the public Management API surface
//!    (`/v1/...`) advertised by `--print-openapi` matches the wire contract the
//!    dashboard's typed clients and `crates/big_brain_client` already expect,
//!    and round-trips the load-bearing deployment-internal DTOs through
//!    `serde_json` against their upstream definitions in
//!    `big_brain_private_api_types`.
//!
//! 2. **`#[ignore]`-gated, requires `TEST_ORCHESTRATOR_DATABASE_URL`.** Spins
//!    up a real `OrchestratorState` against a test Postgres, swaps in a stub
//!    provisioner, builds the axum router, and exercises every load-bearing
//!    deployment-internal endpoint end-to-end with `tower::oneshot`. Any
//!    response that fails to deserialize back into the upstream
//!    `big_brain_private_api_types` shape fails the test.
//!
//! Run only the default suite: `cargo test -p orchestrator --test integration`.
//! Run the DB-backed tests too:
//!   `TEST_ORCHESTRATOR_DATABASE_URL=postgres://... cargo test -p orchestrator
//!     --test integration -- --include-ignored`.

use orchestrator::{
    provisioner::{
        ProvisionRequest,
        ProvisionResult,
        Provisioner,
    },
    router::OrchestratorOpenApi,
};

// ---------------------------------------------------------------------------
// Layer 1: contract checks (no DB)
// ---------------------------------------------------------------------------

/// Documented `(method, path)` pairs that must appear in the spec.
///
/// This list is the wire contract. Adding a new public endpoint means adding
/// the route + a line here; renaming or dropping one breaks this test.
const EXPECTED_MANAGEMENT_OPERATIONS: &[(&str, &str)] = &[
    // tokens
    ("get", "/v1/list_personal_access_tokens"),
    ("post", "/v1/create_personal_access_token"),
    ("post", "/v1/delete_personal_access_token"),
    ("get", "/v1/token_details"),
    // teams
    ("post", "/v1/teams/create_team"),
    ("get", "/v1/teams/{team_id}/list_members"),
    ("post", "/v1/teams/{team_id}/invite_team_member"),
    ("post", "/v1/teams/{team_id}/create_access_token"),
    // projects
    ("post", "/v1/teams/{team_id}/create_project"),
    ("get", "/v1/teams/{team_id}/list_projects"),
    ("get", "/v1/projects/{project_id}"),
    ("get", "/v1/teams/{team_id_or_slug}/projects/{project_slug}"),
    ("post", "/v1/projects/{project_id}/delete"),
    ("get", "/v1/projects/{project_id}/settings"),
    ("patch", "/v1/projects/{project_id}/settings"),
    // deployments
    ("get", "/v1/projects/{project_id}/list_deployments"),
    ("post", "/v1/projects/{project_id}/create_deployment"),
    ("get", "/v1/projects/{project_id}/deployment"),
    (
        "get",
        "/v1/teams/{team_id_or_slug}/projects/{project_slug}/deployment",
    ),
    ("get", "/v1/teams/{team_id}/list_deployments"),
    ("get", "/v1/teams/{team_id}/list_local_deployments"),
    ("get", "/v1/teams/{team_id}/list_deployment_classes"),
    ("get", "/v1/teams/{team_id}/list_deployment_regions"),
    ("get", "/v1/deployments/{deployment_name}"),
    ("post", "/v1/deployments/{deployment_name}/delete"),
    ("post", "/v1/deployments/{deployment_name}/transfer"),
    ("get", "/v1/deployments/{deployment_name}/settings"),
    ("patch", "/v1/deployments/{deployment_name}/settings"),
    ("post", "/v1/deployments/{deployment_name}/restart"),
    // env vars
    (
        "get",
        "/v1/projects/{project_id}/list_default_environment_variables",
    ),
    (
        "post",
        "/v1/projects/{project_id}/update_default_environment_variables",
    ),
];

#[test]
fn openapi_exposes_all_management_endpoints() {
    use utoipa::OpenApi;
    let spec =
        serde_json::to_value(OrchestratorOpenApi::openapi()).expect("serialize openapi spec");
    let paths = spec
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("openapi spec has a `paths` object");

    let mut missing = Vec::new();
    for (method, path) in EXPECTED_MANAGEMENT_OPERATIONS {
        match paths.get(*path).and_then(|item| item.get(*method)) {
            Some(_) => {},
            None => missing.push(format!("{} {}", method.to_uppercase(), path)),
        }
    }
    assert!(
        missing.is_empty(),
        "OpenAPI spec is missing these documented operations:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn openapi_does_not_expose_undocumented_management_endpoints() {
    use utoipa::OpenApi;
    let spec =
        serde_json::to_value(OrchestratorOpenApi::openapi()).expect("serialize openapi spec");
    let paths = spec
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("openapi spec has a `paths` object");

    let expected: std::collections::HashSet<(&str, &str)> =
        EXPECTED_MANAGEMENT_OPERATIONS.iter().copied().collect();

    // Only enforce the contract on `/v1/...` paths. Dashboard/internal/
    // deployment-internal endpoints are also annotated but are not part of
    // the public Management API surface; their wire shape is governed by the
    // dashboard / CLI / `big_brain_client` deserializers, not this test.
    let mut extra = Vec::new();
    for (path, item) in paths {
        if !path.starts_with("/v1/") {
            continue;
        }
        let item = item.as_object().expect("path item must be an object");
        for method in item.keys() {
            // Filter to HTTP methods only (ignore parameters/summary/etc.).
            if !matches!(
                method.as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
            ) {
                continue;
            }
            if !expected.contains(&(method.as_str(), path.as_str())) {
                extra.push(format!("{} {}", method.to_uppercase(), path));
            }
        }
    }
    assert!(
        extra.is_empty(),
        "OpenAPI spec advertises /v1 operations not in the documented contract (add them to \
         EXPECTED_MANAGEMENT_OPERATIONS):\n  {}",
        extra.join("\n  ")
    );
}

#[test]
fn deployment_internal_dto_wire_format() {
    // Pin the wire format of the orchestrator's own deployment-internal DTOs
    // to camelCase, since the CLI and `big_brain_client` decode by field name.
    use orchestrator_api_types::deployment::{
        CreateProjectArgs,
        CreateProjectResponse,
        HasProjectsResponse,
        TeamSummary,
        UrlForKeyArgs,
        UrlForKeyResponse,
    };

    let v = serde_json::to_value(HasProjectsResponse { has_projects: true }).unwrap();
    assert_eq!(v, serde_json::json!({"hasProjects": true}));

    let v = serde_json::to_value(TeamSummary {
        id: 7,
        name: "Self-Hosted".into(),
        slug: "self-hosted".into(),
    })
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"id": 7, "name": "Self-Hosted", "slug": "self-hosted"})
    );

    // Args with optional fields must omit them (`#[serde(default)]` is
    // pointless on the wire if `serde(skip_serializing_if)` isn't set, but
    // the CLI sends only what the user supplied — verify both directions).
    let parsed: CreateProjectArgs = serde_json::from_value(serde_json::json!({
        "team": "self-hosted",
        "projectName": "Demo",
    }))
    .unwrap();
    assert_eq!(parsed.team, "self-hosted");
    assert_eq!(parsed.project_name, "Demo");
    assert!(parsed.deployment_type.is_none());

    let v = serde_json::to_value(CreateProjectResponse {
        project_id: 1,
        project_slug: "demo".into(),
        team_slug: "self-hosted".into(),
        deployment_name: Some("happy-otter-1".into()),
        url: Some("http://happy-otter-1.localhost:9000".into()),
        admin_key: None,
    })
    .unwrap();
    assert_eq!(v["projectId"], 1);
    assert_eq!(v["projectSlug"], "demo");
    assert_eq!(v["teamSlug"], "self-hosted");
    assert_eq!(v["deploymentName"], "happy-otter-1");

    let v = serde_json::to_value(UrlForKeyArgs {
        deploy_key: "prod:happy-otter-1|secret".into(),
    })
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"deployKey": "prod:happy-otter-1|secret"})
    );

    let v = serde_json::to_value(UrlForKeyResponse {
        url: "http://happy-otter-1.localhost:9000".into(),
        deployment_name: "happy-otter-1".into(),
    })
    .unwrap();
    assert_eq!(v["url"], "http://happy-otter-1.localhost:9000");
    assert_eq!(v["deploymentName"], "happy-otter-1");
}

#[test]
fn deployment_auth_response_is_byte_identical_to_upstream() {
    // The orchestrator re-exports `big_brain_private_api_types` for
    // deployment-internal credential exchange so the CLI and
    // `big_brain_client` see byte-identical wire types. If somebody swaps the
    // re-export for a fork, this round-trip will fail.
    use big_brain_private_api_types as upstream;
    use orchestrator_api_types::deployment as exported;

    let upstream_json = serde_json::json!({
        "deploymentName": "happy-otter-123",
        "adminKey": "prod:happy-otter-123|s_secret",
        "url": "http://happy-otter-123.localhost:9000",
        "deploymentType": "prod",
        "reference": null,
        "isDefault": false,
    });
    let via_upstream: upstream::DeploymentAuthResponse =
        serde_json::from_value(upstream_json.clone()).expect("upstream parse");
    let via_exported: exported::DeploymentAuthResponse =
        serde_json::from_value(upstream_json).expect("re-export parse");

    let a = serde_json::to_value(&via_upstream).unwrap();
    let b = serde_json::to_value(&via_exported).unwrap();
    assert_eq!(
        a, b,
        "DeploymentAuthResponse re-export drifted from upstream"
    );
}

// ---------------------------------------------------------------------------
// Layer 2: full DB integration (gated)
// ---------------------------------------------------------------------------

/// Stub provisioner that returns canned `ProvisionResult`s, so the integration
/// test doesn't depend on docker / a real backend.
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

/// Full deployment-internal flow against a real Postgres. Set
/// `TEST_ORCHESTRATOR_DATABASE_URL` to enable.
///
/// **The test owns the database** — it drops and recreates the orchestrator's
/// schema. Point it at a throwaway database, never your dev one.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn deployment_internal_flow_against_real_db() {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
            StatusCode,
        },
    };
    use http_body_util::BodyExt;
    use orchestrator::{
        router::build_router,
        state::OrchestratorState,
    };
    use orchestrator_api_types::{
        dashboard::{
            DeviceAuthorizeArgs,
            DeviceAuthorizeResponse,
        },
        deployment::{
            CreateProjectArgs,
            CreateProjectResponse,
            HasProjectsResponse,
            TeamSummary,
        },
    };
    use tower::ServiceExt;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    let bootstrap_token = format!("test-bootstrap-{}", uuid_like());

    // Wipe the orchestrator's schema so the test is idempotent across runs.
    // The docstring already warns the caller that this owns the DB.
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir for data root");
    let mut config = test_config(database_url, data_root.path().to_path_buf());
    config.bootstrap_token = Some(bootstrap_token.clone());

    // Construct OrchestratorState the public way, then swap in the stub
    // provisioner so we can exercise create_deployment without docker.
    let mut state = OrchestratorState::new(config)
        .await
        .expect("orchestrator state");
    state.provisioner = Arc::new(StubProvisioner);

    let app = build_router(state.clone());

    let authorize_args = DeviceAuthorizeArgs {
        device_name: "integration-test".into(),
        email: None,
        password: None,
        bootstrap_token: Some(bootstrap_token.clone()),
    };
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/authorize")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&authorize_args).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send /api/authorize");
    assert_eq!(resp.status(), StatusCode::OK, "POST /api/authorize");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let auth: DeviceAuthorizeResponse =
        serde_json::from_slice(&body).expect("DeviceAuthorizeResponse shape");
    let bearer = format!("Bearer {}", auth.access_token);

    // 1. GET /api/teams should return the bootstrap team and deserialize as
    //    Vec<TeamSummary>.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/teams")
                .header(AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send /api/teams");
    assert_eq!(resp.status(), StatusCode::OK, "GET /api/teams");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let teams: Vec<TeamSummary> = serde_json::from_slice(&body).expect("Vec<TeamSummary> shape");
    assert!(!teams.is_empty(), "bootstrap team should be present");
    let team_slug = teams[0].slug.clone();

    // 2. GET /api/has_projects → HasProjectsResponse { hasProjects: false }.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/has_projects")
                .header(AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send /api/has_projects");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let _: HasProjectsResponse = serde_json::from_slice(&body).expect("HasProjectsResponse shape");

    // 3. POST /api/create_project → CreateProjectResponse.
    let create_args = CreateProjectArgs {
        team: team_slug.clone(),
        project_name: "Integration Test Project".into(),
        deployment_type: Some("dev".into()),
        region: None,
        tier: None,
        provisioning_mode: None,
        knob_overrides: None,
    };
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/create_project")
                .header(AUTHORIZATION, &bearer)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_args).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send /api/create_project");
    assert_eq!(resp.status(), StatusCode::OK, "POST /api/create_project");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let project: CreateProjectResponse =
        serde_json::from_slice(&body).expect("CreateProjectResponse shape");
    assert_eq!(project.team_slug, team_slug);

    // 4. has_projects flips to true.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/has_projects")
                .header(AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send /api/has_projects (post-create)");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let after: HasProjectsResponse = serde_json::from_slice(&body).unwrap();
    assert!(
        after.has_projects,
        "has_projects should be true after create"
    );
}

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn allowlist_rejects_uninvited_non_admin_session_exchange() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Allowlist,
        vec!["owner@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state);

    let resp = exchange_session(&app, "stranger@example.com", None).await.0;

    assert_eq!(resp, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn invite_accept_rejects_signed_in_member_with_different_email() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Open,
        vec!["owner@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state.clone());

    let owner = exchange_session(&app, "owner@example.com", None).await;
    assert_eq!(owner.0, StatusCode::OK);
    let team = state
        .storage
        .get_team_by_slug("self-hosted")
        .await
        .expect("load default team")
        .expect("default team exists");
    let invite_code = format!("invite-{}", uuid_like());
    state
        .storage
        .create_invitation(team.id, "invited@example.com", "admin", &invite_code, None)
        .await
        .expect("create invitation");

    let attacker = exchange_session(&app, "attacker@example.com", None).await;
    assert_eq!(attacker.0, StatusCode::OK);
    let attacker_token = attacker
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("attacker exchange returns accessToken");

    let resp = post_with_bearer(
        &app,
        &format!("/api/dashboard/invites/{invite_code}/accept"),
        attacker_token,
        serde_json::Value::Null,
    )
    .await;

    assert_eq!(resp, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn non_members_cannot_list_team_invite_codes() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Open,
        vec!["owner@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state.clone());

    let owner = exchange_session(&app, "owner@example.com", None).await;
    assert_eq!(owner.0, StatusCode::OK);
    let owner_member = state
        .storage
        .get_member_by_email("owner@example.com")
        .await
        .expect("load owner")
        .expect("owner member exists");
    let private_team = state
        .storage
        .create_team("Private Team", "private-team", Some(owner_member.id))
        .await
        .expect("create private team");
    state
        .storage
        .create_invitation(
            private_team.id,
            "invited@example.com",
            "developer",
            &format!("invite-{}", uuid_like()),
            Some(owner_member.id),
        )
        .await
        .expect("create invitation");

    let outsider = exchange_session(&app, "outsider@example.com", None).await;
    assert_eq!(outsider.0, StatusCode::OK);
    let outsider_token = outsider
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("outsider exchange returns accessToken");

    let resp = get_with_bearer(
        &app,
        &format!("/api/dashboard/teams/{}/invites", private_team.id),
        outsider_token,
    )
    .await;

    assert_eq!(resp, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Authorization on the access-token routes.
//
// These routes used to take `_auth: AuthIdentity` and discard it, which
// authenticated the caller but never authorized them.
// `create_team_access_token` took no identity at all — and `team_id` is a
// sequential BIGSERIAL, so anyone who could reach the API could mint a
// team-wide token by guessing a small integer.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn minting_a_team_access_token_requires_membership() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Open,
        vec!["owner@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state.clone());

    let owner = exchange_session(&app, "owner@example.com", None).await;
    assert_eq!(owner.0, StatusCode::OK);
    let owner_token = owner
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("owner exchange returns accessToken")
        .to_string();
    let owner_member = state
        .storage
        .get_member_by_email("owner@example.com")
        .await
        .expect("load owner")
        .expect("owner member exists");
    let team = state
        .storage
        .create_team("Private Team", "private-team", Some(owner_member.id))
        .await
        .expect("create private team");

    let uri = format!("/v1/teams/{}/create_access_token", team.id);

    // No credentials at all: this was the open door.
    assert_eq!(
        post_without_auth(&app, &uri, serde_json::json!({})).await,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must not be able to mint a team access token"
    );

    // Authenticated, but not a member of this team.
    let outsider = exchange_session(&app, "outsider@example.com", None).await;
    assert_eq!(outsider.0, StatusCode::OK);
    let outsider_token = outsider
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("outsider exchange returns accessToken")
        .to_string();
    assert_eq!(
        post_with_bearer(&app, &uri, &outsider_token, serde_json::json!({})).await,
        StatusCode::FORBIDDEN,
        "a non-member must not be able to mint a team access token"
    );

    // A real member still can — guards against over-tightening.
    assert_eq!(
        post_with_bearer(&app, &uri, &owner_token, serde_json::json!({})).await,
        StatusCode::CREATED,
        "a team member must still be able to mint a team access token"
    );
}

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn a_member_cannot_revoke_another_members_personal_access_token() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Open,
        vec!["alice@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state.clone());

    let alice = exchange_session(&app, "alice@example.com", None).await;
    assert_eq!(alice.0, StatusCode::OK);
    let alice_token = alice
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("alice accessToken")
        .to_string();

    // Alice mints a PAT of her own.
    let created = post_with_bearer_json(
        &app,
        "/v1/create_personal_access_token",
        &alice_token,
        serde_json::json!({ "name": "alice-ci" }),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    let pat_id = created
        .1
        .get("id")
        .and_then(|v| v.as_str())
        .expect("create returns the token's public id")
        .to_string();

    // Bob is a legitimate, signed-in user — just not the owner of that token.
    let bob = exchange_session(&app, "bob@example.com", None).await;
    assert_eq!(bob.0, StatusCode::OK);
    let bob_token = bob
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("bob accessToken")
        .to_string();

    assert_eq!(
        post_with_bearer(
            &app,
            "/v1/delete_personal_access_token",
            &bob_token,
            serde_json::json!({ "id": pat_id }),
        )
        .await,
        StatusCode::FORBIDDEN,
        "Bob must not be able to revoke Alice's personal access token"
    );

    // And it really is still live, not merely reported as forbidden.
    let still_there = state
        .storage
        .get_access_token_by_public_id(&pat_id)
        .await
        .expect("load token")
        .expect("token row exists");
    assert!(
        still_there.revoked_time.is_none(),
        "the rejected revoke must not have taken effect"
    );

    // Alice can revoke her own.
    assert_eq!(
        post_with_bearer(
            &app,
            "/v1/delete_personal_access_token",
            &alice_token,
            serde_json::json!({ "id": pat_id }),
        )
        .await,
        StatusCode::OK,
        "the owner must still be able to revoke their own token"
    );
    let revoked = state
        .storage
        .get_access_token_by_public_id(&pat_id)
        .await
        .expect("load token")
        .expect("token row exists");
    assert!(
        revoked.revoked_time.is_some(),
        "own revoke must take effect"
    );
}

#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn non_members_cannot_list_another_teams_access_tokens() {
    use axum::http::StatusCode;
    use orchestrator::config::RegistrationMode;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let state = test_state(
        database_url,
        RegistrationMode::Open,
        vec!["owner@example.com".into()],
        "service-key",
    )
    .await;
    let app = orchestrator::router::build_router(state.clone());

    let owner = exchange_session(&app, "owner@example.com", None).await;
    assert_eq!(owner.0, StatusCode::OK);
    let owner_member = state
        .storage
        .get_member_by_email("owner@example.com")
        .await
        .expect("load owner")
        .expect("owner member exists");
    let team = state
        .storage
        .create_team("Private Team", "private-team", Some(owner_member.id))
        .await
        .expect("create private team");

    let outsider = exchange_session(&app, "outsider@example.com", None).await;
    assert_eq!(outsider.0, StatusCode::OK);
    let outsider_token = outsider
        .1
        .get("accessToken")
        .and_then(|v| v.as_str())
        .expect("outsider accessToken")
        .to_string();

    assert_eq!(
        get_with_bearer(
            &app,
            &format!("/api/dashboard/teams/{}/access_tokens", team.id),
            &outsider_token,
        )
        .await,
        StatusCode::FORBIDDEN,
        "a non-member must not be able to enumerate a team's access tokens"
    );
}

#[cfg(test)]
fn uuid_like() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:032x}")
}

/// Build a config for tests.
///
/// The single place a new `OrchestratorConfig` field has to be added, rather
/// than every test growing its own struct literal. Callers that need a
/// different value mutate the returned struct.
///
/// Defaults are deliberately inert: `External` provisioner so nothing tries
/// to reach docker, and `reconcile_interval_secs: 0` so no test starts a
/// background loop that outlives it.
#[cfg(test)]
fn test_config(
    database_url: String,
    data_root: std::path::PathBuf,
) -> orchestrator::config::OrchestratorConfig {
    use orchestrator::config::{
        OrchestratorConfig,
        ProvisionerMode,
        RegistrationMode,
    };

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

#[cfg(test)]
async fn test_state(
    database_url: String,
    registration_mode: orchestrator::config::RegistrationMode,
    admin_emails: Vec<String>,
    service_key: &str,
) -> orchestrator::state::OrchestratorState {
    use orchestrator::state::OrchestratorState;

    let data_root = tempfile::tempdir().expect("tempdir for data root");
    let mut config = test_config(database_url, data_root.path().to_path_buf());
    config.service_key = Some(service_key.into());
    config.admin_emails = admin_emails;
    config.registration_mode = registration_mode;

    OrchestratorState::new(config)
        .await
        .expect("orchestrator state")
}

#[cfg(test)]
async fn exchange_session(
    app: &axum::Router,
    email: &str,
    invite_code: Option<&str>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::{
        body::Body,
        http::Request,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut body = serde_json::json!({
        "authUserId": format!("auth:{email}"),
        "email": email,
        "name": email.split('@').next().unwrap_or(email),
    });
    if let Some(code) = invite_code {
        body["inviteCode"] = serde_json::Value::String(code.to_string());
    }
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/internal/exchange_session")
                .header("x-service-key", "service-key")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send /api/internal/exchange_session");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[cfg(test)]
async fn post_with_bearer(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> axum::http::StatusCode {
    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
        },
    };
    use tower::ServiceExt;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send authenticated POST")
        .status()
}

/// Like `post_with_bearer`, but returns the parsed body too — needed when a
/// later assertion has to reference the id the route just minted.
#[cfg(test)]
async fn post_with_bearer_json(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
        },
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send authenticated POST");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// A request with no `Authorization` header at all, for asserting that a route
/// actually requires credentials.
#[cfg(test)]
async fn post_without_auth(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> axum::http::StatusCode {
    use axum::{
        body::Body,
        http::Request,
    };
    use tower::ServiceExt;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("send unauthenticated POST")
        .status()
}

#[cfg(test)]
async fn get_with_bearer(app: &axum::Router, uri: &str, token: &str) -> axum::http::StatusCode {
    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
        },
    };
    use tower::ServiceExt;

    app.clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send authenticated GET")
        .status()
}

/// `DROP SCHEMA public CASCADE; CREATE SCHEMA public;` so the next migration
/// run starts from a clean slate. Plain `tokio_postgres::NoTls`; do not point
/// at a TLS-only host without adapting this.
#[cfg(test)]
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

// ---------------------------------------------------------------------------
// Admin console, Phase 1
// ---------------------------------------------------------------------------

/// The P1 migration must add the super-admin/suspension columns and make
/// audit events instance-scopable. Asserts against `information_schema` so
/// it fails loudly if a later migration drops one.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn p1_migration_adds_admin_columns() {
    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let pool = orchestrator::storage::pool::PgPool::connect(&database_url)
        .await
        .expect("connect test pool");
    orchestrator::storage::migrations::run(&pool)
        .await
        .expect("run migrations");

    let conn = pool.acquire().await.expect("acquire");
    let rows = conn
        .client()
        .query(
            "SELECT table_name, column_name, is_nullable
               FROM information_schema.columns
              WHERE table_schema = 'public'
                AND (   (table_name = 'members' AND column_name IN ('is_super_admin','suspended'))
                     OR (table_name = 'audit_log_events' AND column_name IN ('scope','team_id')))",
            &[],
        )
        .await
        .expect("query information_schema");

    let found: Vec<(String, String, String)> = rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();

    assert!(
        found
            .iter()
            .any(|(t, c, _)| t == "members" && c == "is_super_admin"),
        "members.is_super_admin missing: {found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(t, c, _)| t == "members" && c == "suspended"),
        "members.suspended missing: {found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(t, c, _)| t == "audit_log_events" && c == "scope"),
        "audit_log_events.scope missing: {found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(t, c, n)| t == "audit_log_events" && c == "team_id" && n == "YES"),
        "audit_log_events.team_id must be nullable: {found:?}"
    );

    // Idempotence: running migrations twice must not error.
    orchestrator::storage::migrations::run(&pool)
        .await
        .expect("migrations are idempotent");
}

/// `set_super_admin` must refuse to clear the last remaining super-admin,
/// and must do so atomically rather than as a read-then-write.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn cannot_revoke_last_super_admin() {
    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let storage = orchestrator::storage::Storage::connect(&database_url)
        .await
        .expect("connect storage");

    let a = storage
        .upsert_member("auth-a", "a@example.com", Some("A"))
        .await
        .expect("member a");
    let b = storage
        .upsert_member("auth-b", "b@example.com", Some("B"))
        .await
        .expect("member b");

    storage.set_super_admin(a.id, true).await.expect("grant a");
    storage.set_super_admin(b.id, true).await.expect("grant b");

    // Two admins: revoking one is fine.
    storage
        .set_super_admin(b.id, false)
        .await
        .expect("revoke b");

    // One admin left: revoking is refused.
    let err = storage
        .set_super_admin(a.id, false)
        .await
        .expect_err("revoking the last super-admin must fail");
    assert!(
        err.to_string().contains("last super-admin"),
        "unexpected error: {err}"
    );

    let reloaded = storage
        .get_member(a.id)
        .await
        .expect("get a")
        .expect("a exists");
    assert!(reloaded.is_super_admin, "a must still be a super-admin");
    assert!(!reloaded.suspended, "suspension defaults to false");
}

/// Elevation must ride only `Session` and `Pat` tokens, and a suspended
/// member must stop authenticating entirely.
///
/// The ineligible case is exercised with a `Team` token rather than a deploy
/// key because both sit in the same `matches!` arm in `resolve_with_storage`
/// and a team token needs no deployment row to construct. The deploy-key
/// kinds are the reason the gate exists - a key checked into CI must not be
/// an instance-wide credential - so a dedicated deploy-key case is still
/// worth adding once the fixtures for one are cheap.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn elevation_is_limited_to_session_and_pat_tokens() {
    use orchestrator::storage::{
        access_tokens::NewAccessToken,
        AccessTokenKind,
    };

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let storage = orchestrator::storage::Storage::connect(&database_url)
        .await
        .expect("connect storage");
    let member = storage
        .upsert_member("auth-admin", "admin@example.com", Some("Admin"))
        .await
        .expect("member");
    let team = storage
        .create_team("Ops", "ops", Some(member.id))
        .await
        .expect("team");
    storage
        .set_super_admin(member.id, true)
        .await
        .expect("grant");

    // A PAT for that member elevates.
    let pat_secret = "pat-secret-for-test";
    storage
        .create_access_token(NewAccessToken {
            public_id: "pat-public",
            kind: AccessTokenKind::Pat,
            member_id: Some(member.id),
            team_id: None,
            project_id: None,
            deployment_id: None,
            name: "test-pat",
            secret_hash: &orchestrator::auth::tokens::sha256_hex(pat_secret),
            secret_suffix: "test",
            expiry: None,
        })
        .await
        .expect("create pat");

    let identity = orchestrator::auth::identity::resolve_for_test(
        &storage,
        &format!("pat:pat-public|{pat_secret}"),
    )
    .await
    .expect("resolve pat");
    assert!(
        identity.is_super_admin,
        "a PAT for a super-admin must elevate"
    );
    assert!(
        !identity.is_bootstrap,
        "an ordinary member is not the bootstrap identity"
    );

    // A team token for the same member does not.
    let team_secret = "team-secret-for-test";
    storage
        .create_access_token(NewAccessToken {
            public_id: "team-public",
            kind: AccessTokenKind::Team,
            member_id: Some(member.id),
            team_id: Some(team.id),
            project_id: None,
            deployment_id: None,
            name: "test-team-token",
            secret_hash: &orchestrator::auth::tokens::sha256_hex(team_secret),
            secret_suffix: "test",
            expiry: None,
        })
        .await
        .expect("create team token");

    let identity = orchestrator::auth::identity::resolve_for_test(
        &storage,
        &format!("team:team-public|{team_secret}"),
    )
    .await
    .expect("resolve team token");
    assert!(
        !identity.is_super_admin,
        "a non-session/PAT token must never elevate, even for an operator"
    );

    // A suspended member's token stops resolving entirely.
    storage
        .set_member_suspended(member.id, true)
        .await
        .expect("suspend");
    let err = orchestrator::auth::identity::resolve_for_test(
        &storage,
        &format!("pat:pat-public|{pat_secret}"),
    )
    .await
    .expect_err("a suspended member must not authenticate");
    assert!(
        matches!(err, orchestrator::errors::ApiError::Unauthorized),
        "expected Unauthorized, got {err:?}"
    );
}

/// Instance-scoped audit events must not leak into per-team audit queries,
/// and team-scoped queries must be unaffected by team_id becoming nullable.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn instance_audit_events_are_isolated_from_team_queries() {
    use orchestrator::storage::AuditQuery;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let storage = orchestrator::storage::Storage::connect(&database_url)
        .await
        .expect("connect storage");
    let member = storage
        .upsert_member("auth-op", "op@example.com", Some("Op"))
        .await
        .expect("member");
    let team = storage
        .create_team("Test Team", "test-team", Some(member.id))
        .await
        .expect("team");

    storage
        .append_audit(
            team.id,
            Some(member.id),
            "teamThing",
            &serde_json::json!({ "k": "v" }),
        )
        .await
        .expect("team event");
    storage
        .append_instance_audit(
            Some(member.id),
            "instanceThing",
            &serde_json::json!({ "k": "v" }),
        )
        .await
        .expect("instance event");
    // Break-glass events carry no member attribution.
    storage
        .append_instance_audit(None, "bootstrapThing", &serde_json::json!({}))
        .await
        .expect("bootstrap event");

    let team_events = storage
        .query_audit(&AuditQuery {
            team_id: team.id,
            ..Default::default()
        })
        .await
        .expect("query team audit");
    assert_eq!(
        team_events.len(),
        1,
        "team query must see only its own event, got {team_events:?}"
    );
    assert_eq!(team_events[0].action, "teamThing");

    let instance_events = storage
        .list_instance_audit(100)
        .await
        .expect("list instance audit");
    assert_eq!(
        instance_events.len(),
        2,
        "instance query sees only instance events"
    );
    // Newest first.
    assert_eq!(instance_events[0].action, "bootstrapThing");
    assert_eq!(instance_events[0].member_id, None);
    assert_eq!(instance_events[1].action, "instanceThing");
    assert_eq!(instance_events[1].member_id, Some(member.id));
}

/// The readiness response must actually differ by probe result.
///
/// Asserts on the pure mapping rather than end to end, because a `PgPool`
/// cannot be constructed against a dead host - `connect` probes and fails -
/// so there is no way to hold a live state whose database is down.
#[test]
fn ready_response_maps_probe_result_to_status() {
    use axum::http::StatusCode;

    let (status, body) = orchestrator::router::ready_response(true);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.0["status"], "ready");

    let (status, body) = orchestrator::router::ready_response(false);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body.0["status"], "not_ready");
    assert!(
        body.0.get("error").is_none(),
        "the unauthenticated readiness body must not carry error detail"
    );
}

/// `0` means boot-only, anything positive means keep going.
#[test]
fn reconcile_interval_zero_disables_the_loop() {
    assert!(
        !orchestrator::reconcile::periodic_enabled(0),
        "0 must disable the periodic loop"
    );
    assert!(
        orchestrator::reconcile::periodic_enabled(60),
        "a positive interval must enable it"
    );
}

/// `/health` is liveness and must never depend on Postgres. `/ready` is
/// readiness and must. Both answer 200 against a live database; the
/// distinction between them is asserted by
/// `ready_response_maps_probe_result_to_status`, which does not need one.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn health_and_ready_both_answer_against_a_live_database() {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{
            Request,
            StatusCode,
        },
    };
    use orchestrator::{
        router::build_router,
        state::OrchestratorState,
    };
    use tower::ServiceExt;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir for data root");
    let config = test_config(database_url, data_root.path().to_path_buf());
    let mut state = OrchestratorState::new(config).await.expect("state");
    state.provisioner = Arc::new(StubProvisioner);
    let app = build_router(state.clone());

    for path in ["/health", "/ready"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap_or_else(|_| panic!("send GET {path}"));
        assert_eq!(resp.status(), StatusCode::OK, "GET {path} with a live DB");
    }
}

/// Drift is "the container is not doing what the database says". The
/// load-bearing case is intended=running, actual=missing.
#[test]
fn drift_is_intended_versus_actual() {
    use orchestrator::routes::admin::fleet::is_drifted;

    // The case that matters: the DB says it should be up, it is gone.
    assert!(is_drifted("running", "missing"));
    assert!(is_drifted("running", "stopped"));

    // Agreement in either direction is not drift.
    assert!(!is_drifted("running", "running"));
    assert!(!is_drifted("paused", "missing"));
    assert!(!is_drifted("disabled", "stopped"));

    // Should be down but is up is also drift.
    assert!(is_drifted("paused", "running"));

    // A probe we could not run is never drift.
    assert!(!is_drifted("running", "unknown"));
    assert!(!is_drifted("paused", "unknown"));
}

/// The admin surface must see across team boundaries - that is the entire
/// point. Two teams with one member each; the admin queries return both.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn admin_queries_span_all_teams() {
    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let storage = orchestrator::storage::Storage::connect(&database_url)
        .await
        .expect("connect storage");

    let alice = storage
        .upsert_member("auth-alice", "alice@example.com", Some("Alice"))
        .await
        .expect("alice");
    let bob = storage
        .upsert_member("auth-bob", "bob@example.com", Some("Bob"))
        .await
        .expect("bob");
    // A member on no team at all: the LEFT JOIN still emits a row for them,
    // and they must come back with an empty `teams` rather than being
    // dropped or inheriting somebody else's.
    let _carol = storage
        .upsert_member("auth-carol", "carol@example.com", None)
        .await
        .expect("carol");

    let t1 = storage
        .create_team("One", "one", Some(alice.id))
        .await
        .expect("team one");
    let t2 = storage
        .create_team("Two", "two", Some(bob.id))
        .await
        .expect("team two");
    storage
        .add_team_member(t1.id, alice.id, orchestrator::storage::TeamRole::Admin)
        .await
        .expect("alice in one");
    storage
        .add_team_member(t2.id, bob.id, orchestrator::storage::TeamRole::Developer)
        .await
        .expect("bob in two");
    // Alice is on both teams, so her row must carry two memberships.
    storage
        .add_team_member(t2.id, alice.id, orchestrator::storage::TeamRole::Developer)
        .await
        .expect("alice in two");

    let teams = storage.list_all_teams().await.expect("all teams");
    assert_eq!(teams.len(), 2, "must see both teams");

    let members = storage.list_all_members().await.expect("all members");
    assert_eq!(members.len(), 3, "must see all three members");

    let alice_row = members
        .iter()
        .find(|m| m.primary_email == "alice@example.com")
        .expect("alice present");
    assert_eq!(alice_row.teams.len(), 2, "alice is on two teams");
    assert_eq!(alice_row.teams[0].team_slug, "one");
    assert_eq!(alice_row.teams[0].role, "admin");
    assert_eq!(alice_row.teams[1].team_slug, "two");
    assert_eq!(alice_row.teams[1].role, "developer");

    let carol_row = members
        .iter()
        .find(|m| m.primary_email == "carol@example.com")
        .expect("carol present");
    assert!(
        carol_row.teams.is_empty(),
        "a member on no team must have no memberships, got {:?}",
        carol_row.teams
    );
}

/// Every `/api/admin` route must reject a non-super-admin identity.
///
/// Driven off `ADMIN_ROUTES` rather than a hand-maintained list here, so a
/// route added without a `SuperAdmin` extractor fails this test instead of
/// shipping open.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn every_admin_route_rejects_non_super_admins() {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
            StatusCode,
        },
    };
    use orchestrator::{
        router::build_router,
        routes::admin::ADMIN_ROUTES,
        state::OrchestratorState,
        storage::{
            access_tokens::NewAccessToken,
            AccessTokenKind,
        },
    };
    use tower::ServiceExt;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir for data root");
    let config = test_config(database_url, data_root.path().to_path_buf());
    let mut state = OrchestratorState::new(config).await.expect("state");
    state.provisioner = Arc::new(StubProvisioner);
    let app = build_router(state.clone());

    // An ordinary member with an ordinary PAT.
    let plain = state
        .storage
        .upsert_member("auth-plain", "plain@example.com", Some("Plain"))
        .await
        .expect("plain member");
    let plain_secret = "plain-secret";
    state
        .storage
        .create_access_token(NewAccessToken {
            public_id: "plain-public",
            kind: AccessTokenKind::Pat,
            member_id: Some(plain.id),
            team_id: None,
            project_id: None,
            deployment_id: None,
            name: "plain-pat",
            secret_hash: &orchestrator::auth::tokens::sha256_hex(plain_secret),
            secret_suffix: "cret",
            expiry: None,
        })
        .await
        .expect("plain pat");
    let plain_bearer = format!("Bearer pat:plain-public|{plain_secret}");

    assert!(!ADMIN_ROUTES.is_empty(), "ADMIN_ROUTES must not be empty");

    for (method, path) in ADMIN_ROUTES {
        // Unauthenticated.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(*method)
                    .uri(*path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap_or_else(|_| panic!("send {method} {path}"));
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path} must be 401 without a token"
        );

        // Authenticated but not a super-admin.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(*method)
                    .uri(*path)
                    .header(AUTHORIZATION, &plain_bearer)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap_or_else(|_| panic!("send {method} {path} as plain member"));
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{method} {path} must be 403 for a non-super-admin"
        );
    }

    // Granting the bit opens every route.
    state
        .storage
        .set_super_admin(plain.id, true)
        .await
        .expect("grant");
    for (method, path) in ADMIN_ROUTES {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(*method)
                    .uri(*path)
                    .header(AUTHORIZATION, &plain_bearer)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap_or_else(|_| panic!("send {method} {path} as super-admin"));
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{method} {path} must be 200 for a super-admin"
        );
    }
}

/// The route table and the router must not drift apart. `ADMIN_ROUTES` is
/// what the authorization test iterates, so a route present in one and not
/// the other is a hole rather than a cosmetic mismatch.
#[test]
fn admin_route_table_matches_the_openapi_surface() {
    use orchestrator::{
        router::OrchestratorOpenApi,
        routes::admin::ADMIN_ROUTES,
    };
    use utoipa::OpenApi;

    let spec =
        serde_json::to_value(OrchestratorOpenApi::openapi()).expect("serialize openapi spec");
    let paths = spec
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("openapi spec has a `paths` object");

    // Every documented /api/admin operation is in the table.
    let mut undocumented_in_table = Vec::new();
    for (path, item) in paths {
        if !path.starts_with("/api/admin/") {
            continue;
        }
        let item = item.as_object().expect("path item must be an object");
        for method in item.keys() {
            if !matches!(
                method.as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
            ) {
                continue;
            }
            let wanted = (method.to_uppercase(), path.to_string());
            if !ADMIN_ROUTES
                .iter()
                .any(|(m, p)| *m == wanted.0 && *p == wanted.1)
            {
                undocumented_in_table.push(format!("{} {}", wanted.0, wanted.1));
            }
        }
    }
    assert!(
        undocumented_in_table.is_empty(),
        "these /api/admin operations are routed but missing from ADMIN_ROUTES, so the \
         authorization test does not cover them:\n  {}",
        undocumented_in_table.join("\n  ")
    );

    // And every table entry is actually documented.
    let mut missing_from_spec = Vec::new();
    for (method, path) in ADMIN_ROUTES {
        let found = paths
            .get(*path)
            .and_then(|item| item.get(method.to_lowercase()))
            .is_some();
        if !found {
            missing_from_spec.push(format!("{method} {path}"));
        }
    }
    assert!(
        missing_from_spec.is_empty(),
        "these ADMIN_ROUTES entries are not in the OpenAPI spec:\n  {}",
        missing_from_spec.join("\n  ")
    );
}

/// The fleet response's wire shape must match what `adminApi.ts`'s zod
/// schema parses.
///
/// `FleetEntry` puts `#[serde(flatten)]` on its `AdminDeploymentRow`, so the
/// deployment's fields have to land as flat camelCase siblings of
/// `actualState` and `drifted`. If serde nested them under a `deployment`
/// key instead, every fleet request would fail zod parsing in the browser
/// and no status-code assertion would notice.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn fleet_response_wire_shape_is_flat_camel_case() {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{
            header::AUTHORIZATION,
            Request,
            StatusCode,
        },
    };
    use http_body_util::BodyExt;
    use orchestrator::{
        router::build_router,
        state::OrchestratorState,
        storage::{
            access_tokens::NewAccessToken,
            AccessTokenKind,
        },
    };
    use tower::ServiceExt;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;

    let data_root = tempfile::tempdir().expect("tempdir for data root");
    let config = test_config(database_url, data_root.path().to_path_buf());
    let mut state = OrchestratorState::new(config).await.expect("state");
    state.provisioner = Arc::new(StubProvisioner);
    let app = build_router(state.clone());

    let op = state
        .storage
        .upsert_member("auth-op", "op@example.com", Some("Op"))
        .await
        .expect("member");
    state
        .storage
        .set_super_admin(op.id, true)
        .await
        .expect("grant");
    let secret = "fleet-secret";
    state
        .storage
        .create_access_token(NewAccessToken {
            public_id: "fleet-public",
            kind: AccessTokenKind::Pat,
            member_id: Some(op.id),
            team_id: None,
            project_id: None,
            deployment_id: None,
            name: "fleet-pat",
            secret_hash: &orchestrator::auth::tokens::sha256_hex(secret),
            secret_suffix: "cret",
            expiry: None,
        })
        .await
        .expect("pat");

    // A team, project, and deployment so the fleet has a row to serialize.
    let team = state
        .storage
        .create_team("Ops", "ops", Some(op.id))
        .await
        .expect("team");
    let project = state
        .storage
        .create_project(team.id, "Demo", "demo", false)
        .await
        .expect("project");
    let empty_knobs = serde_json::json!({});
    state
        .storage
        .create_deployment(orchestrator::storage::deployments::NewDeployment {
            project_id: project.id,
            name: "happy-otter-123",
            deployment_type: orchestrator::storage::DeploymentType::Prod,
            deployment_class: orchestrator::storage::DeploymentClass::Standard,
            region: None,
            url: "http://happy-otter-123.localhost",
            site_url: "http://happy-otter-123-site.localhost",
            backend_pid: None,
            backend_port: 3210,
            creator_id: Some(op.id),
            preview_identifier: None,
            instance_secret: "",
            tier: "S16",
            knob_overrides: &empty_knobs,
            storage_mode: "volume-sqlite",
            pg_password: None,
            minio_root_user: None,
            minio_root_password: None,
            backend_instance_secret: None,
        })
        .await
        .expect("deployment");

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/admin/fleet")
                .header(AUTHORIZATION, format!("Bearer pat:fleet-public|{secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send GET /api/admin/fleet");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("fleet response is JSON");

    assert!(
        v.get("containerStatesAvailable").is_some(),
        "top level must be camelCase, got {v}"
    );
    let entry = v
        .get("deployments")
        .and_then(|d| d.get(0))
        .unwrap_or_else(|| panic!("expected one fleet entry, got {v}"));

    assert!(
        entry.get("deployment").is_none(),
        "the deployment must be flattened, not nested under `deployment`: {entry}"
    );

    // Exactly the keys `fleetEntrySchema` in adminApi.ts declares.
    for key in [
        "id",
        "name",
        "deploymentType",
        "intendedState",
        "tier",
        "url",
        "creationTime",
        "teamId",
        "teamSlug",
        "projectId",
        "projectSlug",
        "actualState",
        "drifted",
    ] {
        assert!(
            entry.get(key).is_some(),
            "fleet entry is missing `{key}`, which adminApi.ts requires: {entry}"
        );
    }

    assert_eq!(entry["name"], "happy-otter-123");
    assert_eq!(entry["teamSlug"], "ops");
    assert_eq!(entry["projectSlug"], "demo");
    // External provisioner owns no containers, so state is unknown and
    // nothing is reported as drifted.
    assert_eq!(entry["actualState"], "unknown");
    assert_eq!(entry["drifted"], false);
    assert_eq!(v["containerStatesAvailable"], false);
    assert_eq!(v["driftCount"], 0);
}

/// Pausing and resuming must be idempotent, and must refuse a deployment
/// that is still provisioning.
///
/// `reconcile::plan` treats `paused` as "not running by intent" and leaves
/// it alone, so pausing a half-built deployment would strand it there
/// permanently.
#[tokio::test]
#[ignore = "needs TEST_ORCHESTRATOR_DATABASE_URL"]
async fn deployment_state_transitions_are_guarded() {
    use orchestrator::storage::DeploymentState;

    let database_url = std::env::var("TEST_ORCHESTRATOR_DATABASE_URL")
        .expect("TEST_ORCHESTRATOR_DATABASE_URL not set (this test is `#[ignore]` by default)");
    reset_public_schema(&database_url).await;
    let storage = orchestrator::storage::Storage::connect(&database_url)
        .await
        .expect("connect storage");

    let member = storage
        .upsert_member("auth-op", "op@example.com", Some("Op"))
        .await
        .expect("member");
    let team = storage
        .create_team("Ops", "ops", Some(member.id))
        .await
        .expect("team");
    let project = storage
        .create_project(team.id, "Demo", "demo", false)
        .await
        .expect("project");
    let empty = serde_json::json!({});
    let d = storage
        .create_deployment(orchestrator::storage::deployments::NewDeployment {
            project_id: project.id,
            name: "state-test",
            deployment_type: orchestrator::storage::DeploymentType::Prod,
            deployment_class: orchestrator::storage::DeploymentClass::Standard,
            region: None,
            url: "http://state-test.localhost",
            site_url: "http://state-test-site.localhost",
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

    // running -> paused, and pausing again is a no-op rather than an error:
    // an operator double-clicking Pause should not be shown a failure.
    storage.pause_deployment(d.id).await.expect("pause");
    storage
        .pause_deployment(d.id)
        .await
        .expect("pause is idempotent");
    let reloaded = storage
        .get_deployment(d.id)
        .await
        .expect("get")
        .expect("exists");
    assert!(matches!(reloaded.state, DeploymentState::Paused));

    storage.resume_deployment(d.id).await.expect("resume");
    storage
        .resume_deployment(d.id)
        .await
        .expect("resume is idempotent");
    let reloaded = storage
        .get_deployment(d.id)
        .await
        .expect("get")
        .expect("exists");
    assert!(matches!(reloaded.state, DeploymentState::Running));

    // A deployment mid-provision must not be pausable.
    storage
        .update_deployment_state(d.id, DeploymentState::Provisioning)
        .await
        .expect("set provisioning");
    let err = storage
        .pause_deployment(d.id)
        .await
        .expect_err("pausing a provisioning deployment must fail");
    assert!(
        err.to_string().contains("provisioning"),
        "the error must say why, got: {err}"
    );

    // A missing deployment is a clear error, not a silent no-op.
    let err = storage
        .pause_deployment(999_999)
        .await
        .expect_err("pausing a nonexistent deployment must fail");
    assert!(
        err.to_string().contains("not found"),
        "unexpected error: {err}"
    );
}

/// Which containers a pause has to touch, and in what order.
///
/// Pure so it is testable without a docker daemon — the shell-out around it
/// is the only part this machine cannot exercise. Order matters: the backend
/// stops before its sidecars so it never sees its database vanish
/// mid-request, and starts after them so it never comes up without one.
#[test]
fn pause_and_resume_touch_containers_in_dependency_order() {
    use orchestrator::provisioner::lifecycle::{
        containers_for_pause,
        containers_for_resume,
    };

    // volume-sqlite: the backend owns its own storage, so it is alone.
    assert_eq!(
        containers_for_pause("orch-", "happy-otter", "volume-sqlite"),
        vec!["orch-happy-otter"]
    );
    assert_eq!(
        containers_for_resume("orch-", "happy-otter", "volume-sqlite"),
        vec!["orch-happy-otter"]
    );

    // sidecar: backend first on the way down, last on the way up.
    let down = containers_for_pause("orch-", "happy-otter", "sidecar");
    assert_eq!(down.first().map(String::as_str), Some("orch-happy-otter"));
    assert_eq!(down.len(), 3, "backend + postgres + minio, got {down:?}");

    let up = containers_for_resume("orch-", "happy-otter", "sidecar");
    assert_eq!(up.last().map(String::as_str), Some("orch-happy-otter"));
    assert_eq!(
        up,
        down.iter().rev().cloned().collect::<Vec<_>>(),
        "resume must be the exact reverse of pause"
    );
}
