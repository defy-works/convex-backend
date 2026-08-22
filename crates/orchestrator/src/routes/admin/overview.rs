//! GET /api/admin/overview
//!
//! Backs the admin overview cards: host capacity, deployment counts by
//! state, team and member counts.
//!
//! The full host-capacity figures live here rather than on the dashboard
//! route, which every authenticated user can reach. Fleet inventory is
//! operator information.

use std::collections::BTreeMap;

use axum::{
    extract::State,
    Json,
};
use serde::Serialize;

use crate::{
    auth::super_admin::SuperAdmin,
    errors::{
        ApiError,
        ApiResult,
    },
    state::OrchestratorState,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewResponse {
    pub total_memory_mb: u64,
    pub total_cpus: u32,
    pub allocated_memory_mb: u64,
    pub allocated_cpus: f32,
    pub deployment_count: u32,
    /// Deployment counts keyed by the intended state recorded in the
    /// database: `running`, `paused`, `disabled`, `provisioning`.
    pub deployments_by_state: BTreeMap<String, u32>,
    pub team_count: u32,
    pub member_count: u32,
}

#[utoipa::path(
    get,
    path = "/api/admin/overview",
    responses(
        (status = 200, body = OverviewResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn overview(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
) -> ApiResult<Json<OverviewResponse>> {
    let host = state.host_capacity.read();
    let tiers = state
        .storage
        .list_deployment_tiers()
        .await
        .map_err(ApiError::Internal)?;

    let mut allocated_memory: u64 = 0;
    let mut allocated_cpus: f32 = 0.0;
    for t in &tiers {
        if let Some(tier) = crate::provisioner::tiers::resolve(t) {
            // Unbounded tiers consume the entire host; reflect that so the
            // overview shows "fully booked" when one exists.
            if tier.unbounded {
                allocated_memory += host.total_memory_mb;
                allocated_cpus += host.total_cpus as f32;
            } else {
                allocated_memory += u64::from(tier.memory_mb);
                allocated_cpus += tier.cpus;
            }
        }
    }

    let deployments = state
        .storage
        .list_all_deployments_with_owners()
        .await
        .map_err(ApiError::Internal)?;
    let mut deployments_by_state: BTreeMap<String, u32> = BTreeMap::new();
    for d in &deployments {
        *deployments_by_state
            .entry(d.intended_state.clone())
            .or_insert(0) += 1;
    }

    let teams = state
        .storage
        .list_all_teams()
        .await
        .map_err(ApiError::Internal)?;
    let member_count = state
        .storage
        .count_members()
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(OverviewResponse {
        total_memory_mb: host.total_memory_mb,
        total_cpus: host.total_cpus,
        allocated_memory_mb: allocated_memory,
        allocated_cpus,
        deployment_count: tiers.len() as u32,
        deployments_by_state,
        team_count: teams.len() as u32,
        member_count: member_count as u32,
    }))
}
