#!/usr/bin/env bash
# Emit `export ...` lines that enable sccache against the R2 compile cache, or
# emit nothing. Intended to be eval'd by the cargo layers in Dockerfile.backend:
#
#   eval "$(sccache-env)"
#
# Never exits non-zero and never writes to stdout unless sccache is genuinely
# usable. A compile cache is an optimisation; if anything about it is missing or
# broken the build must still run, just slower. Diagnostics go to stderr so they
# appear in the build log without being eval'd.
set -u

secret() {
  # BuildKit secret mounts land here; absent when the build runs without them
  # (a fork, or a local `docker build` with no --secret flags).
  [ -s "/run/secrets/$1" ] && tr -d '\n\r' < "/run/secrets/$1"
}

access_key="$(secret r2_access_key || true)"
secret_key="$(secret r2_secret_key || true)"
endpoint="$(secret r2_endpoint || true)"
bucket="$(secret r2_bucket || true)"

if [ -z "${access_key}" ] || [ -z "${secret_key}" ] \
  || [ -z "${endpoint}" ] || [ -z "${bucket}" ]; then
  echo "sccache: R2 cache not configured; compiling without a compile cache" >&2
  exit 0
fi

if ! command -v sccache >/dev/null 2>&1; then
  echo "sccache: binary not found; compiling without a compile cache" >&2
  exit 0
fi

export AWS_ACCESS_KEY_ID="${access_key}"
export AWS_SECRET_ACCESS_KEY="${secret_key}"
unset AWS_SESSION_TOKEN
export SCCACHE_ENDPOINT="${endpoint}"
export SCCACHE_BUCKET="${bucket}"
export SCCACHE_REGION=auto
# Never idle-exit: linking does not go through sccache, so a long link tail can
# exceed the default 600s idle timeout and kill the server mid-build, silently
# dropping the rest of the compile off the cache.
export SCCACHE_IDLE_TIMEOUT=0

if ! sccache --start-server >/dev/null 2>&1; then
  echo "sccache: server failed to start; compiling without a compile cache" >&2
  exit 0
fi

# Prove the bucket is actually reachable before committing the build to it. A
# server that starts but cannot talk to R2 would otherwise fail every single
# compile through the wrapper.
if ! sccache --show-stats >/dev/null 2>&1; then
  echo "sccache: cache unreachable; compiling without a compile cache" >&2
  sccache --stop-server >/dev/null 2>&1 || true
  exit 0
fi

echo "sccache: enabled against ${bucket}" >&2
cat <<EOF
export AWS_ACCESS_KEY_ID='${access_key}'
export AWS_SECRET_ACCESS_KEY='${secret_key}'
export SCCACHE_ENDPOINT='${endpoint}'
export SCCACHE_BUCKET='${bucket}'
export SCCACHE_REGION='auto'
export SCCACHE_IDLE_TIMEOUT='0'
export RUSTC_WRAPPER='sccache'
EOF
