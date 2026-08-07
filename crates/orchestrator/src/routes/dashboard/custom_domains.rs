//! Custom domain management for a deployment.
//!
//! Every mutation re-renders the Traefik dynamic config (see
//! `crate::custom_domains`) so routing follows the database rather than
//! waiting for the next container restart.
//!
//! That re-render, and issuance, both run in background tasks — never on the
//! request path. Issuance because an ACME order takes tens of seconds, far
//! too long to hold an HTTP request open; the config write because Traefik
//! reloads when it lands, and dashboard responses travel back through that
//! same Traefik. Doing it inline tore down the connection carrying the reply,
//! surfacing as a browser network error on a mutation that had in fact
//! already committed.

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
    CanonicalUrls,
    CreateCustomDomainArgs,
    CustomDomain,
    CustomDomainArgs,
    ListCustomDomains,
    SetCanonicalUrlsArgs,
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
        .route(
            "/deployments/{deployment_id}/canonical_urls",
            get(get_canonical_urls),
        )
        .route(
            "/deployments/{deployment_id}/canonical_urls/set",
            post(set_canonical_urls),
        )
}

/// Resolves a deployment's derived origins from orchestrator config.
async fn canonical_urls_for(
    state: &OrchestratorState,
    deployment_id: i64,
) -> ApiResult<CanonicalUrls> {
    let deployment = state
        .storage
        .get_deployment(deployment_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound("deployment".into()))?;

    let (default_url, default_site_url) = crate::provisioner::default_origins(
        &deployment.name,
        &state.config.router_host,
        state.config.site_router_host.as_deref(),
        &state.config.router_public_scheme,
        state.config.router_public_port,
    );

    // What the deployment *would* get if recreated right now.
    let effective_url = deployment.desired_url.clone().unwrap_or_else(|| default_url.clone());
    let effective_site_url = deployment
        .desired_site_url
        .clone()
        .unwrap_or_else(|| default_site_url.clone());

    Ok(CanonicalUrls {
        restart_pending: effective_url != deployment.url
            || effective_site_url != deployment.site_url,
        current_url: deployment.url,
        current_site_url: deployment.site_url,
        desired_url: deployment.desired_url,
        desired_site_url: deployment.desired_site_url,
        default_url,
        default_site_url,
    })
}

#[utoipa::path(
    get,
    path = "/api/dashboard/deployments/{deployment_id}/canonical_urls",
    params(("deployment_id" = i64, Path)),
    responses((status = 200, body = CanonicalUrls)),
    tag = "dashboard",
)]
pub(crate) async fn get_canonical_urls(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
) -> ApiResult<Json<CanonicalUrls>> {
    Ok(Json(canonical_urls_for(&state, deployment_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/dashboard/deployments/{deployment_id}/canonical_urls/set",
    params(("deployment_id" = i64, Path)),
    request_body = SetCanonicalUrlsArgs,
    responses((status = 200, body = CanonicalUrls), (status = 400)),
    tag = "dashboard",
)]
pub(crate) async fn set_canonical_urls(
    _auth: AuthIdentity,
    State(state): State<OrchestratorState>,
    Path(deployment_id): Path<i64>,
    Json(args): Json<SetCanonicalUrlsArgs>,
) -> ApiResult<Json<CanonicalUrls>> {
    let current = canonical_urls_for(&state, deployment_id).await?;
    let domains = state
        .storage
        .list_custom_domains(deployment_id)
        .await
        .map_err(ApiError::Internal)?;

    // Only a hostname this orchestrator actually routes to this deployment
    // can be canonical. Anything else would hand clients a URL that resolves
    // somewhere else, or nowhere.
    let check = |value: &Option<String>, kind: &str, default: &str| -> Result<(), ApiError> {
        let Some(url) = value.as_deref() else {
            return Ok(());
        };
        if url == default {
            return Ok(());
        }
        let host = custom_domains::host_of(url);
        if domains.iter().any(|d| d.kind == kind && d.domain == host) {
            Ok(())
        } else {
            Err(ApiError::BadRequest(format!(
                "{host} is not a custom domain attached to this deployment for `{kind}`"
            )))
        }
    };
    check(&args.url, custom_domains::KIND_API, &current.default_url)?;
    check(
        &args.site_url,
        custom_domains::KIND_SITE,
        &current.default_site_url,
    )?;

    // Storing the default explicitly is the same as having no override, and
    // keeping it as NULL means the deployment follows the config if the
    // orchestrator's router host ever changes.
    let url = args.url.filter(|u| u != &current.default_url);
    let site_url = args.site_url.filter(|u| u != &current.default_site_url);

    state
        .storage
        .set_deployment_canonical_urls(deployment_id, url.as_deref(), site_url.as_deref())
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(canonical_urls_for(&state, deployment_id).await?))
}

fn to_api(record: crate::storage::CustomDomainRecord) -> CustomDomain {
    CustomDomain {
        id: record.id,
        deployment_id: record.deployment_id,
        domain: record.domain,
        cert_state: record.cert_state,
        created_at: record.created_at,
        kind: record.kind,
        tls_mode: record.tls_mode,
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
    let tls_mode = custom_domains::validate_tls_mode(
        args.tls_mode
            .as_deref()
            .unwrap_or(custom_domains::TLS_MODE_ACME),
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
        .create_custom_domain(deployment_id, &domain, &kind, &tls_mode)
        .await
        .map_err(ApiError::Internal)?;

    // Routing is synced off the request path either way. Writing the Traefik
    // dynamic config makes Traefik reload, and this response is travelling
    // back through that same Traefik on the `websecure` entrypoint — the
    // reload drops the connection before the reply is flushed, so the browser
    // sees a network error for a request that actually succeeded (the row is
    // already committed above; a reload of the dashboard shows it). Reply
    // first, reconfigure the proxy afterwards.
    if tls_mode == custom_domains::TLS_MODE_UPSTREAM {
        // Nothing to issue — the certificate lives in front of us. The domain
        // still needs its router, so sync without entering issuance.
        spawn_traefik_sync(state.clone());
    } else {
        spawn_issuance(state.clone(), domain);
    }

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

    // If this domain was the deployment's canonical URL, drop that too. A
    // canonical URL naming a hostname we no longer route is worse than none:
    // the row survives, and the next restart bakes the dead hostname into
    // CONVEX_CLOUD_ORIGIN / CONVEX_SITE_ORIGIN. Clearing falls the deployment
    // back to its derived hostname, which is always routed.
    if let Some(deployment) = state
        .storage
        .get_deployment(deployment_id)
        .await
        .map_err(ApiError::Internal)?
    {
        let keep_url = deployment
            .desired_url
            .clone()
            .filter(|u| custom_domains::host_of(u) != domain);
        let keep_site_url = deployment
            .desired_site_url
            .clone()
            .filter(|u| custom_domains::host_of(u) != domain);
        if keep_url != deployment.desired_url || keep_site_url != deployment.desired_site_url {
            state
                .storage
                .set_deployment_canonical_urls(
                    deployment_id,
                    keep_url.as_deref(),
                    keep_site_url.as_deref(),
                )
                .await
                .map_err(ApiError::Internal)?;
        }
    }

    // Off the request path for the same reason as `create` — the config
    // rewrite reloads the Traefik that is carrying this response. The domain
    // keeps routing for the few milliseconds until the task lands, which is
    // strictly better than losing the reply to a connection reset.
    spawn_traefik_sync(state);

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

    // Retrying issuance for a domain whose TLS is terminated upstream would
    // order a certificate that is never served, and burn ACME rate limit
    // doing it. Say so rather than silently doing nothing.
    let record = state
        .storage
        .get_custom_domain(&domain)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::BadRequest(format!("{domain} is not configured")))?;
    if record.tls_mode == custom_domains::TLS_MODE_UPSTREAM {
        return Err(ApiError::BadRequest(format!(
            "{domain} terminates TLS upstream, so there is no certificate to issue"
        )));
    }

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

/// Re-renders the Traefik config off the request path. Used where there is no
/// domain row left to record a failure against (deletion), so a failure can
/// only be logged — the next successful sync rewrites the file wholesale and
/// reconciles whatever this attempt missed.
fn spawn_traefik_sync(state: OrchestratorState) {
    tokio::spawn(async move {
        if let Err(e) = custom_domains::sync_traefik_config(&state).await {
            tracing::warn!(error = %format!("{e:#}"), "could not sync Traefik custom-domain config");
        }
    });
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
    let record = state
        .storage
        .get_custom_domain(domain)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{domain} is no longer configured"))?;

    // Last line of defence. The sweep filters these out in SQL and the retry
    // handler rejects them, but issuing for an upstream-terminated domain is
    // wasted ACME quota whichever path got here.
    anyhow::ensure!(
        record.tls_mode != custom_domains::TLS_MODE_UPSTREAM,
        "{domain} terminates TLS upstream; not ordering a certificate"
    );

    // Route before validating: the ACME server fetches the HTTP-01 challenge
    // over the domain itself, which only resolves once Traefik has the router
    // for it. This used to run inline in the create handler; doing it here
    // means `retry` also re-establishes routing rather than assuming an
    // earlier create left the config file intact.
    custom_domains::sync_traefik_config(state).await?;

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

