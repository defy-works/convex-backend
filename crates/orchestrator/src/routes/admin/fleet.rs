//! GET /api/admin/fleet
//!
//! Every deployment on the instance with intended state (from the database)
//! next to actual state (from docker).
//!
//! Reuses `reconcile`'s primitives rather than shelling out to docker a
//! second way, so the console and the reconciler can never disagree about
//! what "drifted" means.

use axum::{
    extract::State,
    Json,
};
use futures::stream::{
    self,
    StreamExt,
};
use serde::Serialize;

use crate::{
    auth::super_admin::SuperAdmin,
    config::ProvisionerMode,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
    storage::AdminDeploymentRow,
};

/// Matches `reconcile`'s concurrency: enough to keep the sweep fast, few
/// enough not to storm the docker socket.
const PROBE_CONCURRENCY: usize = 8;

/// Reported when the provisioner does not own containers, or when the probe
/// itself failed. Distinct from `stopped` on purpose.
const STATE_UNKNOWN: &str = "unknown";

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FleetEntry {
    #[serde(flatten)]
    pub deployment: AdminDeploymentRow,
    /// `running` / `stopped` / `missing`, or `unknown` when there was
    /// nothing to inspect or the inspection failed.
    pub actual_state: String,
    /// True when the container is not doing what the database says it
    /// should. Always false when `actual_state` is `unknown` — we do not
    /// report drift we could not observe.
    pub drifted: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FleetResponse {
    pub deployments: Vec<FleetEntry>,
    pub drift_count: u32,
    /// False when the provisioner does not manage containers, so the UI can
    /// say "not applicable" rather than rendering a column of `unknown`.
    pub container_states_available: bool,
}

/// Is the container doing something other than what the database intends?
///
/// Pure so it can be tested without a docker daemon. `unknown` is never
/// drift: a probe we could not run is not evidence that anything is wrong,
/// and reporting it as drift would light up the console every time the
/// socket hiccuped.
pub fn is_drifted(intended_state: &str, actual_state: &str) -> bool {
    match actual_state {
        STATE_UNKNOWN => false,
        "running" => intended_state != "running",
        // `stopped` or `missing`: drift only if it was supposed to be up. A
        // paused deployment with no container is exactly correct.
        _ => intended_state == "running",
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/fleet",
    responses(
        (status = 200, body = FleetResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn fleet(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
) -> ApiResult<Json<FleetResponse>> {
    let rows = state
        .storage
        .list_all_deployments_with_owners()
        .await
        .map_err(ApiError::Internal)?;

    let container_states_available =
        matches!(state.config.provisioner_mode, ProvisionerMode::Docker);

    if !container_states_available {
        let deployments = rows
            .into_iter()
            .map(|deployment| FleetEntry {
                deployment,
                actual_state: STATE_UNKNOWN.to_string(),
                drifted: false,
            })
            .collect();
        return Ok(Json(FleetResponse {
            deployments,
            drift_count: 0,
            container_states_available,
        }));
    }

    let prefix = state.config.backend_container_prefix.clone();
    let deployments: Vec<FleetEntry> = stream::iter(rows.into_iter().map(|row| {
        let prefix = prefix.clone();
        async move {
            let container = format!("{prefix}{}", row.name);
            let actual_state = match crate::reconcile::container_status(&container).await {
                Ok(s) => format!("{s:?}").to_lowercase(),
                // A probe that errored is not evidence of drift — say so
                // rather than reporting a deployment as broken because the
                // socket hiccuped.
                Err(e) => {
                    tracing::warn!(
                        deployment = %row.name,
                        error = %e,
                        "admin fleet: container status probe failed"
                    );
                    STATE_UNKNOWN.to_string()
                },
            };
            let drifted = is_drifted(&row.intended_state, &actual_state);
            FleetEntry {
                deployment: row,
                actual_state,
                drifted,
            }
        }
    }))
    .buffer_unordered(PROBE_CONCURRENCY)
    .collect()
    .await;

    let drift_count = deployments.iter().filter(|d| d.drifted).count() as u32;
    Ok(Json(FleetResponse {
        deployments,
        drift_count,
        container_states_available,
    }))
}
