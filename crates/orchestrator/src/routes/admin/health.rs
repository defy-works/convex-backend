//! GET /api/admin/health
//!
//! The detailed counterpart to `/ready`. `/ready` is unauthenticated and so
//! reports status only; this route is behind `SuperAdmin` and can therefore
//! name what is actually broken.

use std::time::{
    Duration,
    Instant,
};

use axum::{
    extract::State,
    Json,
};
use serde::Serialize;

use crate::{
    auth::super_admin::SuperAdmin,
    config::ProvisionerMode,
    errors::ApiResult,
    state::OrchestratorState,
};

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseHealth {
    pub reachable: bool,
    pub ping_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionerHealth {
    pub mode: String,
    /// `None` when the mode does not own containers, so there is no socket
    /// to check. Reporting `false` there would read as a fault when nothing
    /// is wrong.
    pub docker_reachable: Option<bool>,
    pub error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminHealthResponse {
    pub version: String,
    pub database: DatabaseHealth,
    pub provisioner: ProvisionerHealth,
    pub reconcile_interval_secs: u64,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[utoipa::path(
    get,
    path = "/api/admin/health",
    responses(
        (status = 200, body = AdminHealthResponse),
        (status = 403, description = "not a super-admin"),
    ),
    tag = "admin",
)]
pub(crate) async fn admin_health(
    _admin: SuperAdmin,
    State(state): State<OrchestratorState>,
) -> ApiResult<Json<AdminHealthResponse>> {
    let database = probe_database(&state).await;
    let provisioner = probe_provisioner(&state).await;
    Ok(Json(AdminHealthResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        database,
        provisioner,
        reconcile_interval_secs: state.config.reconcile_interval_secs,
    }))
}

async fn probe_database(state: &OrchestratorState) -> DatabaseHealth {
    let started = Instant::now();
    let probe = async {
        let conn = state.storage.pool().acquire().await?;
        conn.client().simple_query("SELECT 1").await?;
        Ok::<_, anyhow::Error>(())
    };
    match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
        Ok(Ok(())) => DatabaseHealth {
            reachable: true,
            ping_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(Err(e)) => DatabaseHealth {
            reachable: false,
            ping_ms: None,
            error: Some(e.to_string()),
        },
        Err(_) => DatabaseHealth {
            reachable: false,
            ping_ms: None,
            error: Some(format!("probe timed out after {PROBE_TIMEOUT:?}")),
        },
    }
}

/// A broken docker socket currently only surfaces as failed provisions, with
/// nothing pointing at the cause. Check it directly so the console can say
/// so.
async fn probe_provisioner(state: &OrchestratorState) -> ProvisionerHealth {
    let mode = format!("{:?}", state.config.provisioner_mode).to_lowercase();
    if !matches!(state.config.provisioner_mode, ProvisionerMode::Docker) {
        return ProvisionerHealth {
            mode,
            docker_reachable: None,
            error: None,
        };
    }
    let probe = tokio::process::Command::new("docker")
        .arg("version")
        .arg("--format")
        .arg("{{.Server.Version}}")
        .output();
    match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
        Ok(Ok(out)) if out.status.success() => ProvisionerHealth {
            mode,
            docker_reachable: Some(true),
            error: None,
        },
        Ok(Ok(out)) => ProvisionerHealth {
            mode,
            docker_reachable: Some(false),
            error: Some(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        },
        Ok(Err(e)) => ProvisionerHealth {
            mode,
            docker_reachable: Some(false),
            error: Some(e.to_string()),
        },
        Err(_) => ProvisionerHealth {
            mode,
            docker_reachable: Some(false),
            error: Some(format!("docker probe timed out after {PROBE_TIMEOUT:?}")),
        },
    }
}
