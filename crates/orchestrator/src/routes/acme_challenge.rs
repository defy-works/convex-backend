//! Serves ACME HTTP-01 challenge responses.
//!
//! Traefik routes `/.well-known/acme-challenge/` for every custom domain here
//! on the plain HTTP entrypoint (see `custom_domains::render_config`), which
//! is what lets a domain be validated *before* it has a certificate.
//!
//! Deliberately unauthenticated: the ACME server is an anonymous client, and
//! the tokens are single-use, high-entropy values that only exist in memory
//! while an order is in flight.

use axum::{
    extract::{
        Path,
        State,
    },
    http::{
        header,
        HeaderMap,
        StatusCode,
    },
    routing::get,
    Router,
};

use crate::state::OrchestratorState;

pub fn router() -> Router<OrchestratorState> {
    Router::new()
        .route(
            "/.well-known/acme-challenge/{token}",
            get(serve_challenge),
        )
        .route(
            crate::custom_domains::DOMAIN_VERIFICATION_PATH,
            get(serve_domain_verification),
        )
}

/// Returns the verification token for whichever custom domain the request
/// arrived on.
///
/// Traefik routes this path to the orchestrator for every custom domain at a
/// priority above the per-domain router, so the deployment never sees it — an
/// operator's own HTTP action on this path cannot answer instead. That is what
/// makes the token meaningful: only the orchestrator knows it, and only a
/// request that genuinely reached the orchestrator over that hostname gets it
/// back.
///
/// Unauthenticated, like the ACME challenge beside it. The token proves nothing
/// about the caller and grants nothing — it exists solely so the orchestrator
/// can recognise its own routing from the outside.
pub(crate) async fn serve_domain_verification(
    State(state): State<OrchestratorState>,
    headers: HeaderMap,
) -> Result<String, StatusCode> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        // Strip any port; Host carries one when the entrypoint is non-standard.
        .map(|h| h.split(':').next().unwrap_or(h).trim().to_ascii_lowercase())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let record = state
        .storage
        .get_custom_domain(&host)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    record.verification_token.ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(
    get,
    path = "/.well-known/acme-challenge/{token}",
    params(("token" = String, Path)),
    responses(
        (status = 200, description = "Key authorization for an in-flight challenge"),
        (status = 404),
    ),
    tag = "acme",
)]
pub(crate) async fn serve_challenge(
    State(state): State<OrchestratorState>,
    Path(token): Path<String>,
) -> Result<String, StatusCode> {
    // Unknown tokens are indistinguishable from expired ones; 404 either way.
    state.challenges.get(&token).ok_or(StatusCode::NOT_FOUND)
}
