//! Custom domain management for a deployment.
//!
//! Every mutation re-renders the Traefik dynamic config (see
//! `crate::custom_domains`) so routing follows the database immediately
//! rather than at the next container restart. Issuance runs in the
//! background: an ACME order takes tens of seconds, far too long to hold an
//! HTTP request open.

use axum::{
    extract::{
        Path,
        State,
    },
    http::StatusCode,
    routing::{
        get,
        post,
    },
    Json,
    Router,
};
use orchestrator_api_types::dashboard::{
    CreateCustomDomainArgs,
    CustomDomain,
    CustomDomainArgs,
    ListCustomDomains,
    VerifyCustomDomainResponse,
};

use crate::{
    acme,
    auth::identity::AuthIdentity,
    custom_domains,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
};

pub fn router() -> Router<OrchestratorState> {
    Router::new()
        .route(
            "/deployments/{deployment_id}/custom_domains/list",
            get(list_custom_domains),
        )
        .route(
            "/deployments/{deployment_id}/custom_domains/create",
            post(create_custom_domain),
        )
        .route(
            "/deployments/{deployment_id}/custom_domains/delete",
            post(delete_custom_domain),
        )
        .route(
            "/deployments/{deployment_id}/custom_domains/verify",
            post(verify_custom_domain),
        )
        .route(
            "/deployments/{deployment_id}/custom_domains/retry",
            post(retry_custom_domain),
        )
}

fn to_api(record: crate::storage::CustomDomainRecord) -> CustomDomain {
    CustomDomain {
        id: record.id,
        deployment_id: record.deployment_id,
        domain: record.domain,
        cert_state: record.cert_state,
        created_at: record.created_at,
        kind: record.kind,
        last_error: record.last_error,
    }
}

// ---------- Custom domains ----------

#[utoipa::path(
    get,
    path = "/api/dashboard/deployments/{deployment_id}/custom_domains/list",
    params(("deployment_id" = i64, Path)),
    responses((status = 200, body = ListCustomDomains)),
    tag = "dashboard",
)]
pub(crate) async fn list_custom_domains(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
) -> ApiResult<Json<ListCustomDomains>> {
    let domains = state
        .storage
        .list_custom_domains(deployment_id)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(ListCustomDomains {
        domains: domains.into_iter().map(to_api).collect(),
        target_host: state.config.router_host.clone(),
        routing_enabled: state.config.traefik_dynamic_dir.is_some(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/dashboard/deployments/{deployment_id}/custom_domains/create",
    params(("deployment_id" = i64, Path)),
    request_body = CreateCustomDomainArgs,
    responses((status = 200, body = CustomDomain), (status = 400)),
    tag = "dashboard",
)]
pub(crate) async fn create_custom_domain(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    Json(args): Json<CreateCustomDomainArgs>,
) -> ApiResult<Json<CustomDomain>> {
    let domain = custom_domains::validate_domain(&args.domain)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let kind = custom_domains::validate_kind(
        args.kind.as_deref().unwrap_or(custom_domains::KIND_API),
    )
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // `domain` is globally UNIQUE — two deployments can't both claim it, and
    // Traefik couldn't route it if they did. Translate the constraint
    // violation into a message the dashboard can show verbatim.
    if state
        .storage
        .get_custom_domain(&domain)
        .await
        .map_err(ApiError::Internal)?
        .is_some()
    {
        return Err(ApiError::BadRequest(format!(
            "{domain} is already attached to a deployment"
        )));
    }

    let record = state
        .storage
        .create_custom_domain(deployment_id, &domain, &kind)
        .await
        .map_err(ApiError::Internal)?;

    // Route first so the ACME HTTP-01 challenge path resolves, then issue.
    custom_domains::sync_traefik_config(&state)
        .await
        .map_err(ApiError::Internal)?;
    spawn_issuance(state.clone(), domain);

    Ok(Json(to_api(record)))
}

#[utoipa::path(
    post,
    path = "/api/dashboard/deployments/{deployment_id}/custom_domains/delete",
    params(("deployment_id" = i64, Path)),
    request_body = CustomDomainArgs,
    responses((status = 200)),
    tag = "dashboard",
)]
pub(crate) async fn delete_custom_domain(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    Json(args): Json<CustomDomainArgs>,
) -> ApiResult<StatusCode> {
    // Normalize so a domain stored lowercase is still matched when the caller
    // sends it back with different casing.
    let domain = custom_domains::validate_domain(&args.domain)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    state
        .storage
        .delete_custom_domain(deployment_id, &domain)
        .await
        .map_err(ApiError::Internal)?;
    state
        .storage
        .delete_certificate(&domain)
        .await
        .map_err(ApiError::Internal)?;

    custom_domains::sync_traefik_config(&state)
        .await
        .map_err(ApiError::Internal)?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/dashboard/deployments/{deployment_id}/custom_domains/retry",
    params(("deployment_id" = i64, Path)),
    request_body = CustomDomainArgs,
    responses((status = 200)),
    tag = "dashboard",
)]
pub(crate) async fn retry_custom_domain(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(_deployment_id): Path<i64>,
    Json(args): Json<CustomDomainArgs>,
) -> ApiResult<StatusCode> {
    let domain = custom_domains::validate_domain(&args.domain)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    spawn_issuance(state, domain);
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    post,
    path = "/api/dashboard/deployments/{deployment_id}/custom_domains/verify",
    params(("deployment_id" = i64, Path)),
    request_body = CustomDomainArgs,
    responses((status = 200, body = VerifyCustomDomainResponse)),
    tag = "dashboard",
)]
pub(crate) async fn verify_custom_domain(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(_deployment_id): Path<i64>,
    Json(args): Json<CustomDomainArgs>,
) -> ApiResult<Json<VerifyCustomDomainResponse>> {
    let domain = custom_domains::validate_domain(&args.domain)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let (cert_state, error) = custom_domains::probe_domain(&domain).await;

    state
        .storage
        .set_custom_domain_status(&domain, &cert_state, error.as_deref())
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(VerifyCustomDomainResponse {
        domain,
        cert_state,
        error,
    }))
}

/// Issues (or renews) a certificate off the request path, recording the
/// outcome — including the failure reason — on the domain row.
pub fn spawn_issuance(state: OrchestratorState, domain: String) {
    tokio::spawn(async move {
        if let Err(e) = state
            .storage
            .set_custom_domain_status(&domain, "issuing", None)
            .await
        {
            tracing::warn!(error = %e, %domain, "could not mark domain as issuing");
        }

        match issue_now(&state, &domain).await {
            Ok(()) => {
                tracing::info!(%domain, "issued certificate");
            },
            Err(e) => {
                // `{e:#}` includes the anyhow context chain, which is where
                // the actionable part usually lives (which zone, which token).
                let message = format!("{e:#}");
                tracing::warn!(error = %message, %domain, "certificate issuance failed");
                if let Err(e) = state
                    .storage
                    .set_custom_domain_status(&domain, "failed", Some(&message))
                    .await
                {
                    tracing::warn!(error = %e, %domain, "could not record issuance failure");
                }
            },
        }
    });
}

async fn issue_now(state: &OrchestratorState, domain: &str) -> anyhow::Result<()> {
    state
        .storage
        .get_custom_domain(domain)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{domain} is no longer configured"))?;

    let issued = acme::issue(state, domain).await?;

    state
        .storage
        .upsert_certificate(
            domain,
            &issued.cert_pem,
            &issued.key_pem,
            issued.issued_at,
            issued.renew_after,
        )
        .await?;

    // Publish the new certificate to Traefik, then confirm it's actually
    // being served before calling the domain active.
    custom_domains::sync_traefik_config(state).await?;

    let (cert_state, error) = custom_domains::probe_domain(domain).await;
    state
        .storage
        .set_custom_domain_status(domain, &cert_state, error.as_deref())
        .await?;

    Ok(())
}

