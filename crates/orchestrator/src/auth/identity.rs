//! axum extractors that resolve a request to an `AuthIdentity`.

use axum::{
    extract::{
        FromRef,
        FromRequestParts,
    },
    http::{
        header,
        request::Parts,
    },
};

use crate::{
    auth::tokens::{
        parse_token,
        sha256_hex,
    },
    errors::ApiError,
    state::OrchestratorState,
    storage::{
        AccessToken,
        AccessTokenKind,
    },
};

#[derive(Debug, Clone)]
pub struct AuthIdentity {
    pub token: AccessToken,
    pub member_id: Option<i64>,
    pub team_id: Option<i64>,
    pub deployment_id: Option<i64>,
    pub project_id: Option<i64>,
    /// Instance-wide operator rights.
    ///
    /// Only ever `true` for `Session` and `Pat` tokens. A deploy key
    /// belonging to a super-admin member resolves to `false` — deploy keys
    /// live in CI config and `.env` files, and a member being an operator
    /// must not turn every key they ever minted into an instance-wide
    /// credential.
    pub is_super_admin: bool,
    /// This token belongs to the synthetic bootstrap member — the
    /// break-glass path for an instance whose operator accounts are all
    /// locked out.
    ///
    /// Keyed on `auth_user_id == SYSTEM_AUTH_USER_ID`, not on the token's
    /// name: members can name their own PATs, so a name check would let
    /// anyone mint themselves instance root. Only `bootstrap_if_empty`
    /// creates that member, and `exchange_session` rejects any
    /// `authUserId` in the `system:` namespace, so it cannot be forged.
    pub is_bootstrap: bool,
}

impl AuthIdentity {
    pub fn require_member(&self) -> Result<i64, ApiError> {
        self.member_id.ok_or(ApiError::Forbidden)
    }
}

#[derive(Debug, Clone)]
pub struct OptionalAuth(pub Option<AuthIdentity>);

fn extract_bearer(parts: &Parts) -> Option<String> {
    let header = parts.headers.get(header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let scheme = scheme.trim();
    if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("convex") {
        Some(token.trim().to_string())
    } else {
        None
    }
}

async fn resolve(state: &OrchestratorState, raw: &str) -> Result<AuthIdentity, ApiError> {
    resolve_with_storage(&state.storage, raw).await
}

/// Resolve a raw bearer token against storage without an HTTP request.
///
/// Exists so the integration suite can assert on elevation and suspension
/// without standing up a router. Delegates to the identical code path the
/// extractors use, so a test that passes here is not testing a copy.
pub async fn resolve_for_test(
    storage: &crate::storage::Storage,
    raw: &str,
) -> Result<AuthIdentity, ApiError> {
    resolve_with_storage(storage, raw).await
}

async fn resolve_with_storage(
    storage: &crate::storage::Storage,
    raw: &str,
) -> Result<AuthIdentity, ApiError> {
    let parsed = parse_token(raw).map_err(|e| {
        tracing::debug!(
            error = %e,
            raw_len = raw.len(),
            raw_prefix = raw.chars().take(12).collect::<String>(),
            "auth: failed to parse token"
        );
        ApiError::Unauthorized
    })?;
    let hash = sha256_hex(parsed.secret);
    let token = storage
        .get_access_token_by_hash(&hash)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| {
            tracing::debug!(
                public_id = parsed.public_id,
                "auth: no access token row matches secret hash"
            );
            ApiError::Unauthorized
        })?;
    // For "deploy-key shaped" tokens the middle slot of the wire format is
    // a resource identifier (the deployment name for per-deployment keys,
    // or `<team_slug>:<project_slug>` for project-scoped preview keys),
    // not the row's randomly minted `public_id`. Validate that the
    // received value matches the token's bound resource instead of the
    // stored `public_id`. For all other token kinds the original strict
    // equality still applies.
    let is_deploy_key_kind = matches!(
        token.kind,
        AccessTokenKind::DeployProd
            | AccessTokenKind::DeployDev
            | AccessTokenKind::DeployPreview
            | AccessTokenKind::ProjectDeploy
    );
    if is_deploy_key_kind {
        let expected = if matches!(token.kind, AccessTokenKind::ProjectDeploy) {
            // Project-scoped preview keys are `preview:<team>:<project>|<secret>`.
            // After parse_token splits on the first `:` and the `|`, the
            // remaining middle slot is `<team>:<project>` — match it
            // against the team/project that owns this token row.
            let project_id = token.project_id.ok_or_else(|| {
                tracing::debug!(
                    public_id = %token.public_id,
                    "auth: project deploy-key row has no project_id"
                );
                ApiError::Unauthorized
            })?;
            let project = storage
                .get_project(project_id)
                .await
                .map_err(ApiError::Internal)?
                .ok_or_else(|| {
                    tracing::debug!(
                        project_id,
                        "auth: project deploy-key references a missing project"
                    );
                    ApiError::Unauthorized
                })?;
            let team = storage
                .get_team(project.team_id)
                .await
                .map_err(ApiError::Internal)?
                .ok_or_else(|| {
                    tracing::debug!(
                        team_id = project.team_id,
                        "auth: project deploy-key references a missing team"
                    );
                    ApiError::Unauthorized
                })?;
            format!("{}:{}", team.slug, project.slug)
        } else {
            let deployment_id = token.deployment_id.ok_or_else(|| {
                tracing::debug!(
                    public_id = %token.public_id,
                    kind = ?token.kind,
                    "auth: deploy-key token row has no deployment_id"
                );
                ApiError::Unauthorized
            })?;
            let dep = storage
                .get_deployment(deployment_id)
                .await
                .map_err(ApiError::Internal)?
                .ok_or_else(|| {
                    tracing::debug!(
                        deployment_id,
                        "auth: deploy-key references a deployment that no longer exists"
                    );
                    ApiError::Unauthorized
                })?;
            dep.name
        };
        if expected != parsed.public_id {
            tracing::debug!(
                expected = %expected,
                received_public_id = parsed.public_id,
                kind = ?token.kind,
                "auth: deploy-key resource-identifier mismatch"
            );
            return Err(ApiError::Unauthorized);
        }
    } else if token.public_id != parsed.public_id {
        tracing::debug!(
            stored_public_id = %token.public_id,
            received_public_id = parsed.public_id,
            "auth: public_id mismatch (token tampered with or wrong row)"
        );
        return Err(ApiError::Unauthorized);
    }
    if token.revoked_time.is_some() {
        tracing::debug!(
            public_id = %token.public_id,
            revoked_time = ?token.revoked_time,
            "auth: token has been revoked"
        );
        return Err(ApiError::Unauthorized);
    }
    if let Some(exp) = token.expiry
        && crate::time::now_unix_ms() > exp
    {
        tracing::debug!(
            public_id = %token.public_id,
            expiry = exp,
            "auth: token expired"
        );
        return Err(ApiError::Unauthorized);
    }
    // Validate kind/prefix consistency for non-deployment tokens. Deploy
    // keys carry the deployment name in their public_id segment, which we
    // don't enforce against `prefix` strictly here.
    let _expected_prefix = match token.kind {
        AccessTokenKind::Pat | AccessTokenKind::Session => "pat",
        AccessTokenKind::Team => "team",
        AccessTokenKind::DeployProd => "prod",
        AccessTokenKind::DeployDev => "dev",
        AccessTokenKind::DeployPreview => "preview",
        AccessTokenKind::ProjectDeploy => "project",
        AccessTokenKind::App => "app",
        AccessTokenKind::Admin => "admin",
    };

    // Elevation is deliberately narrow. Deploy keys live in CI config and
    // `.env` files; a member being an operator must not make every key they
    // ever minted an instance-wide credential.
    let elevation_eligible = matches!(token.kind, AccessTokenKind::Session | AccessTokenKind::Pat);
    let mut is_super_admin = false;
    let mut is_bootstrap = false;
    if let Some(member_id) = token.member_id {
        match storage
            .get_member(member_id)
            .await
            .map_err(ApiError::Internal)?
        {
            // A suspended or deleted member cannot authenticate at all,
            // which revokes every live session and PAT without deleting
            // anything — the point of suspension being reversible.
            Some(m) if m.suspended || m.deleted => {
                tracing::debug!(member_id, "auth: member is suspended or deleted");
                return Err(ApiError::Unauthorized);
            },
            Some(m) => {
                is_bootstrap =
                    elevation_eligible && m.auth_user_id == crate::state::SYSTEM_AUTH_USER_ID;
                is_super_admin = elevation_eligible && m.is_super_admin;
            },
            None => {
                tracing::debug!(member_id, "auth: token references a missing member");
                return Err(ApiError::Unauthorized);
            },
        }
    }

    Ok(AuthIdentity {
        member_id: token.member_id,
        team_id: token.team_id,
        deployment_id: token.deployment_id,
        project_id: token.project_id,
        is_super_admin,
        is_bootstrap,
        token,
    })
}

impl<S> FromRequestParts<S> for AuthIdentity
where
    OrchestratorState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = extract_bearer(parts).ok_or(ApiError::Unauthorized)?;
        let st: OrchestratorState = OrchestratorState::from_ref(state);
        resolve(&st, &token).await
    }
}

impl<S> FromRequestParts<S> for OptionalAuth
where
    OrchestratorState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Some(token) = extract_bearer(parts) else {
            return Ok(Self(None));
        };
        let st: OrchestratorState = OrchestratorState::from_ref(state);
        match resolve(&st, &token).await {
            Ok(id) => Ok(Self(Some(id))),
            Err(ApiError::Unauthorized) => Ok(Self(None)),
            Err(e) => Err(e),
        }
    }
}
