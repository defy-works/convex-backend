//! Extractor that gates the instance-scoped `/api/admin` surface.
//!
//! Handlers take `SuperAdmin` instead of `AuthIdentity`, so the
//! authorization requirement is part of the function signature and visible
//! in review. The table-driven 403 test in `tests/integration.rs` is what
//! actually fails CI if a route is added without it.

use axum::{
    extract::{
        FromRef,
        FromRequestParts,
    },
    http::request::Parts,
};

use crate::{
    auth::identity::AuthIdentity,
    errors::ApiError,
    state::OrchestratorState,
};

/// Who performed an admin action, for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A real member holding `members.is_super_admin`.
    Member(i64),
    /// The synthetic bootstrap member — break-glass, no human attribution.
    Bootstrap,
}

impl Actor {
    /// Member id to store on an audit row, or `None` for break-glass.
    ///
    /// The bootstrap member does have a row, but recording it would put a
    /// synthetic account where an operator's name belongs and read as if a
    /// person did it. `None` plus the `actor` label in the metadata is the
    /// honest version.
    pub fn member_id(&self) -> Option<i64> {
        match self {
            Self::Member(id) => Some(*id),
            Self::Bootstrap => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Member(_) => "member",
            Self::Bootstrap => "bootstrap",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SuperAdmin {
    pub identity: AuthIdentity,
    pub actor: Actor,
}

impl<S> FromRequestParts<S> for SuperAdmin
where
    OrchestratorState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let identity = AuthIdentity::from_request_parts(parts, state).await?;
        let st: OrchestratorState = OrchestratorState::from_ref(state);

        // Break-glass: the bootstrap member is an instance operator by
        // definition, so an instance whose operator accounts are all locked
        // out can still be recovered without psql. Gated on the orchestrator
        // still having a bootstrap token configured, so an operator can shut
        // the path off by clearing it.
        let is_bootstrap = identity.is_bootstrap && st.config.bootstrap_token.is_some();

        if !identity.is_super_admin && !is_bootstrap {
            tracing::debug!(
                member_id = ?identity.member_id,
                token_kind = ?identity.token.kind,
                "admin: rejecting non-super-admin identity"
            );
            return Err(ApiError::Forbidden);
        }

        // Bootstrap wins the label even if the member also carries the
        // column, because the credential in play is the break-glass one and
        // the audit trail should say so.
        let actor = if is_bootstrap {
            Actor::Bootstrap
        } else {
            Actor::Member(identity.require_member()?)
        };

        Ok(Self { identity, actor })
    }
}
