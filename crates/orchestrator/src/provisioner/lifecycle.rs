//! Stopping and starting a deployment's containers without destroying them.
//!
//! Distinct from `teardown`, which removes containers and volumes, and from
//! `respawn`, which recreates them. Pause has to *preserve* the container's
//! configuration — image, env, volumes, resource limits — because resume
//! reuses it. `docker stop` does; `docker rm` does not, which is why
//! `docker::stop_and_remove_container` is the wrong primitive here despite
//! the similar name.

use tokio::process::Command;

use super::sidecar;

/// Containers to stop when pausing, in the order to stop them.
///
/// Backend first: it holds connections to its sidecars, and stopping
/// Postgres out from under an in-flight request produces errors a user sees
/// rather than a clean shutdown.
pub fn containers_for_pause(
    container_prefix: &str,
    deployment_name: &str,
    storage_mode: &str,
) -> Vec<String> {
    let backend = format!("{container_prefix}{deployment_name}");
    if storage_mode != "sidecar" {
        // volume-sqlite: the backend owns its own storage, so there is
        // nothing else to stop.
        return vec![backend];
    }
    vec![
        backend,
        sidecar::pg_container_name(container_prefix, deployment_name),
        sidecar::minio_container_name(container_prefix, deployment_name),
    ]
}

/// Containers to start when resuming: the exact reverse of the pause order,
/// so the backend comes up last and finds its dependencies already running.
pub fn containers_for_resume(
    container_prefix: &str,
    deployment_name: &str,
    storage_mode: &str,
) -> Vec<String> {
    let mut v = containers_for_pause(container_prefix, deployment_name, storage_mode);
    v.reverse();
    v
}

/// `docker stop`, with the same grace period the provisioner uses on
/// teardown.
///
/// A container that is already stopped or missing is not an error: pause is
/// idempotent, and an operator retrying after a partial failure should
/// converge rather than get stuck on a container that is already where they
/// want it.
pub async fn stop_container(name: &str) -> anyhow::Result<()> {
    let output = Command::new("docker")
        .args(["stop", "--time", "10", name])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("docker stop {name}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such container") {
        tracing::debug!(container = %name, "docker stop: container already gone");
        return Ok(());
    }
    anyhow::bail!("docker stop {name} failed: {}", stderr.trim())
}
