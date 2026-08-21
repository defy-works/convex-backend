//! Periodic reconcile of tenant backends.
//!
//! Spawned backends are created with `--restart unless-stopped`, so a host
//! reboot or docker-daemon restart normally brings them back on its own. What
//! `unless-stopped` deliberately does *not* do is restart a container that was
//! stopped explicitly — and that covers the common operator flows:
//! `docker stop`, `docker compose down` on the orchestrator stack (which tears
//! down the shared network and takes the tenants with it), or a host that came
//! up with the tenants halted. In all of those the orchestrator would serve its
//! API happily while every tenant deployment stayed dark until somebody hit
//! Restart in the dashboard, one deployment at a time.
//!
//! This module closes that gap: on boot and every `reconcile_interval_secs`
//! thereafter, for every deployment row, make the container match what the
//! database says should be running.
//!
//! Deliberately narrow: it **starts containers that already exist** and does
//! not create missing ones. Recreating a container goes through
//! `DockerProvisioner::provision`, which branches on the process-wide
//! `--enable-sidecars` setting rather than the row's `storage_mode`. Doing that
//! in bulk at boot would turn a single mismatched flag into a fleet-wide event
//! that silently orphans every `volume-sqlite` deployment's data. A missing
//! container is logged at `error` for the operator instead — that case needs a
//! human deciding on the storage mode, not a loop.

use std::{
    sync::Arc,
    time::Duration,
};

use futures::stream::{
    self,
    StreamExt,
};
use tokio::process::Command;

use crate::{
    config::ProvisionerMode,
    provisioner::sidecar,
    state::OrchestratorState,
    storage::{
        DeploymentRecord,
        DeploymentState,
    },
};

/// Tenants reconciled at once. Each sidecar-mode deployment can spend up to
/// ~60s in each of the Postgres and MinIO readiness waits, so this is about
/// keeping a big fleet's total boot time sane without handing docker a
/// thundering herd of simultaneous container starts.
const RECONCILE_CONCURRENCY: usize = 4;

/// Grace period before the first sweep. `OrchestratorState::new` has already
/// run migrations by the time we get here, but the docker daemon on a freshly
/// booted host is often still settling, and `--restart unless-stopped`
/// containers are coming up on their own in parallel. Waiting lets those land
/// so we mostly observe a settled world and skip work.
const STARTUP_DELAY: Duration = Duration::from_secs(5);

/// The `deployments.storage_mode` value meaning "backend plus Postgres and
/// MinIO sidecars". The other value, `volume-sqlite`, keeps everything in one
/// container with a named volume and has no sidecars to start.
const SIDECAR_STORAGE_MODE: &str = "sidecar";

/// What docker currently thinks of a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerStatus {
    /// No container by that name.
    Missing,
    /// Exists and its state is `running`.
    Running,
    /// Exists in any non-running state (`exited`, `created`, `paused`, …).
    Stopped,
}

/// What to do about one deployment. Separated from the doing so the policy —
/// which is the part with the sharp edges — can be tested without a docker
/// daemon or a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconcileAction {
    /// Already in the desired shape, or deliberately left alone.
    Leave(LeaveReason),
    /// Exists but is down: start it.
    Start,
    /// Should be running but has no container. Needs an operator.
    ReportMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaveReason {
    AlreadyRunning,
    NotRunningByIntent,
}

/// The whole policy. `state` is what the database says the deployment should
/// be; `status` is what docker says it is.
fn plan(state: DeploymentState, status: ContainerStatus) -> ReconcileAction {
    match state {
        // A deployment an operator paused or disabled stays down. Nothing
        // writes these today, but starting something somebody deliberately
        // stopped is a far worse surprise than leaving it down.
        DeploymentState::Paused | DeploymentState::Disabled => {
            ReconcileAction::Leave(LeaveReason::NotRunningByIntent)
        },
        // `Provisioning` is a row mid-create. If a container already exists it
        // is a crashed or interrupted provision, and starting it is right;
        // if it doesn't, the provision never finished and only a re-provision
        // can fix it.
        DeploymentState::Running | DeploymentState::Provisioning => match status {
            ContainerStatus::Running => ReconcileAction::Leave(LeaveReason::AlreadyRunning),
            ContainerStatus::Stopped => ReconcileAction::Start,
            ContainerStatus::Missing => ReconcileAction::ReportMissing,
        },
    }
}

/// Whether a given interval means "keep reconciling".
///
/// `0` restores the original boot-only behaviour, which is the escape hatch
/// for an operator who wants to drive reconciliation by hand.
pub fn periodic_enabled(interval_secs: u64) -> bool {
    interval_secs > 0
}

/// Reconcile on an interval in the background.
///
/// This used to run once at boot and exit, which left any drift after
/// startup both invisible and uncorrected — a container stopped by hand at
/// 09:00 stayed down until somebody noticed. Mirrors `acme::renewal::spawn`:
/// never blocks the API from coming up, and a failure is logged rather than
/// propagated — a tenant that won't start must not stop the orchestrator
/// from serving the dashboard that reports it.
pub fn spawn(state: OrchestratorState) {
    if !matches!(state.config.provisioner_mode, ProvisionerMode::Docker) {
        // `External` (the default) and `Process` don't own containers, so
        // there is nothing to reconcile and every attempt would fail.
        tracing::info!(
            mode = ?state.config.provisioner_mode,
            "skipping deployment reconcile: provisioner does not manage containers"
        );
        return;
    }
    let interval_secs = state.config.reconcile_interval_secs;
    tokio::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            if let Err(e) = reconcile_all(&state).await {
                tracing::error!(error = %e, "deployment reconcile failed");
            }
            if !periodic_enabled(interval_secs) {
                tracing::info!("periodic reconcile disabled; ran once at boot");
                return;
            }
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        }
    });
}

/// Start every deployment that should be running and isn't.
pub async fn reconcile_all(state: &OrchestratorState) -> anyhow::Result<()> {
    let deployments = state.storage.list_all_deployments().await?;
    if deployments.is_empty() {
        tracing::info!("deployment reconcile: no deployments registered");
        return Ok(());
    }

    let prefix = Arc::new(state.config.backend_container_prefix.clone());
    tracing::info!(
        count = deployments.len(),
        "deployment reconcile: checking tenant backends"
    );

    let outcomes = stream::iter(deployments.into_iter().map(|d| {
        let prefix = prefix.clone();
        async move { (d.name.clone(), reconcile_one(&d, &prefix).await) }
    }))
    .buffer_unordered(RECONCILE_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut started = 0usize;
    let mut failed = 0usize;
    for (name, outcome) in outcomes {
        match outcome {
            Ok(true) => started += 1,
            Ok(false) => {},
            Err(e) => {
                failed += 1;
                tracing::error!(
                    deployment = %name,
                    error = %e,
                    "deployment reconcile: failed to start tenant backend"
                );
            },
        }
    }
    tracing::info!(started, failed, "deployment reconcile: finished");
    Ok(())
}

/// Reconcile one deployment. `Ok(true)` means this call started something.
async fn reconcile_one(
    deployment: &DeploymentRecord,
    container_prefix: &str,
) -> anyhow::Result<bool> {
    let backend = format!("{container_prefix}{}", deployment.name);
    // A paused/disabled row never needs a docker call at all, so decide on
    // intent before paying for `docker inspect`.
    let status = match deployment.state {
        DeploymentState::Paused | DeploymentState::Disabled => ContainerStatus::Missing,
        DeploymentState::Running | DeploymentState::Provisioning => {
            container_status(&backend).await?
        },
    };
    let backend_action = plan(deployment.state, status);

    if let ReconcileAction::Leave(LeaveReason::NotRunningByIntent) = backend_action {
        tracing::debug!(
            deployment = %deployment.name,
            state = %deployment.state,
            "deployment reconcile: skipping, not in running state"
        );
        return Ok(false);
    }
    if let ReconcileAction::ReportMissing = backend_action {
        // See the module docs: recreating this is a storage-mode decision.
        tracing::error!(
            deployment = %deployment.name,
            container = %backend,
            storage_mode = %deployment.storage_mode,
            "deployment reconcile: container does not exist; not recreating it \
             automatically. Use Restart in the dashboard (or POST \
             /v1/deployments/{}/restart) to re-provision it.",
            deployment.name
        );
        return Ok(false);
    }

    // Sidecars are reconciled whether or not the backend itself needs starting.
    // A backend that is up while its Postgres is down is broken, and deciding
    // this off the backend's state alone walked straight past that case — which
    // is reachable by stopping only the sidecars, or by a host reboot where
    // `unless-stopped` revived the backend but the sidecars had been stopped
    // explicitly and stayed down.
    let mut started_sidecar = false;
    if should_reconcile_sidecars(&deployment.storage_mode, status) {
        started_sidecar = ensure_sidecars_ready(&deployment.name, container_prefix).await?;
    }

    if let ReconcileAction::Leave(LeaveReason::AlreadyRunning) = backend_action {
        if started_sidecar {
            // The backend's Postgres pool reconnects on its own, so bouncing a
            // running backend would cost connections for nothing.
            tracing::info!(
                deployment = %deployment.name,
                "deployment reconcile: restarted sidecars under an already-running backend"
            );
        } else {
            tracing::debug!(
                deployment = %deployment.name,
                "deployment reconcile: already running"
            );
        }
        return Ok(started_sidecar);
    }

    tracing::info!(
        deployment = %deployment.name,
        container = %backend,
        "deployment reconcile: starting stopped tenant backend"
    );
    start_container(&backend).await?;
    Ok(true)
}

/// Whether this deployment's Postgres and MinIO sidecars are ours to reconcile.
///
/// Only sidecar-mode deployments have them, and only when the backend container
/// still exists — starting sidecars for a deployment whose backend is gone just
/// burns readiness waits on something an operator has to re-provision anyway.
fn should_reconcile_sidecars(storage_mode: &str, backend: ContainerStatus) -> bool {
    storage_mode == SIDECAR_STORAGE_MODE && backend != ContainerStatus::Missing
}

/// Bring a sidecar-mode deployment's Postgres and MinIO up and wait for both to
/// report ready. `Ok(true)` if this call started at least one of them.
async fn ensure_sidecars_ready(
    deployment_name: &str,
    container_prefix: &str,
) -> anyhow::Result<bool> {
    let pg = sidecar::pg_container_name(container_prefix, deployment_name);
    let minio = sidecar::minio_container_name(container_prefix, deployment_name);

    let mut started = false;
    for (kind, container) in [("postgres", &pg), ("minio", &minio)] {
        match container_status(container).await? {
            ContainerStatus::Running => {},
            ContainerStatus::Stopped => {
                tracing::info!(
                    deployment = %deployment_name,
                    container = %container,
                    "deployment reconcile: starting {kind} sidecar"
                );
                start_container(container).await?;
                started = true;
            },
            ContainerStatus::Missing => {
                // Deliberately fatal for this deployment: bringing a backend up
                // with no database behind it is worse than leaving it down.
                anyhow::bail!(
                    "{kind} sidecar {container} does not exist; deployment {deployment_name} \
                     needs a full restart to re-provision it"
                );
            },
        }
    }

    // Both probes are `docker exec`-based and return on their first successful
    // attempt with no sleep, so running them even when nothing was started
    // costs one exec each and confirms the pair is actually serving.
    sidecar::wait_for_postgres(&pg).await?;
    sidecar::wait_for_minio(&minio).await?;
    Ok(started)
}

/// `docker start`. Distinct from the provisioner's `docker run`: it reuses the
/// container's existing configuration — image, env, volumes, network,
/// resource limits — so a reconcile can never silently re-provision a
/// deployment under different settings than it was created with.
async fn start_container(name: &str) -> anyhow::Result<()> {
    let output = Command::new("docker")
        .args(["start", name])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("docker start {name}: {e}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "docker start {name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Read a container's state via `docker inspect`. A non-zero exit means no
/// such container, which is the `Missing` case rather than an error.
async fn container_status(name: &str) -> anyhow::Result<ContainerStatus> {
    let output = Command::new("docker")
        .args(["inspect", "--format", "{{.State.Status}}", name])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("docker inspect {name}: {e}"))?;
    if !output.status.success() {
        return Ok(ContainerStatus::Missing);
    }
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if status == "running" {
        ContainerStatus::Running
    } else {
        ContainerStatus::Stopped
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_deployment_gets_started() {
        // The whole point: this is the `docker compose down` / host-halt case
        // that `--restart unless-stopped` does not cover.
        assert_eq!(
            plan(DeploymentState::Running, ContainerStatus::Stopped),
            ReconcileAction::Start
        );
    }

    #[test]
    fn a_running_deployment_is_left_alone() {
        // Restarting a healthy tenant would drop its connections and cost up
        // to 30s in the admin-key wait, on every orchestrator boot.
        assert_eq!(
            plan(DeploymentState::Running, ContainerStatus::Running),
            ReconcileAction::Leave(LeaveReason::AlreadyRunning)
        );
    }

    #[test]
    fn a_missing_container_is_never_recreated_automatically() {
        // Re-provisioning branches on the process-wide sidecar flag rather
        // than the row's storage_mode, so a bulk recreate could orphan every
        // volume-sqlite deployment's data.
        assert_eq!(
            plan(DeploymentState::Running, ContainerStatus::Missing),
            ReconcileAction::ReportMissing
        );
    }

    #[test]
    fn paused_and_disabled_deployments_stay_down() {
        for state in [DeploymentState::Paused, DeploymentState::Disabled] {
            for status in [
                ContainerStatus::Stopped,
                ContainerStatus::Missing,
                ContainerStatus::Running,
            ] {
                assert_eq!(
                    plan(state, status),
                    ReconcileAction::Leave(LeaveReason::NotRunningByIntent),
                    "{state:?} + {status:?} must not be started"
                );
            }
        }
    }

    #[test]
    fn sidecars_are_reconciled_even_when_the_backend_is_already_running() {
        // The bug this guards: deciding sidecar work off the backend's state
        // meant a live backend with a dead Postgres was skipped entirely.
        assert!(should_reconcile_sidecars(
            SIDECAR_STORAGE_MODE,
            ContainerStatus::Running
        ));
        assert!(should_reconcile_sidecars(
            SIDECAR_STORAGE_MODE,
            ContainerStatus::Stopped
        ));
    }

    #[test]
    fn volume_sqlite_deployments_have_no_sidecars_to_reconcile() {
        for status in [
            ContainerStatus::Running,
            ContainerStatus::Stopped,
            ContainerStatus::Missing,
        ] {
            assert!(!should_reconcile_sidecars("volume-sqlite", status));
        }
    }

    #[test]
    fn sidecars_are_left_alone_when_the_backend_container_is_gone() {
        // That deployment needs re-provisioning; starting its sidecars would
        // just burn two readiness waits.
        assert!(!should_reconcile_sidecars(
            SIDECAR_STORAGE_MODE,
            ContainerStatus::Missing
        ));
    }

    #[test]
    fn an_interrupted_provision_is_started_if_its_container_survived() {
        assert_eq!(
            plan(DeploymentState::Provisioning, ContainerStatus::Stopped),
            ReconcileAction::Start
        );
        assert_eq!(
            plan(DeploymentState::Provisioning, ContainerStatus::Missing),
            ReconcileAction::ReportMissing
        );
    }
}
