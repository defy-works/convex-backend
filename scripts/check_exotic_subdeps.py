#!/usr/bin/env python3
"""Assert that every exotic dependency in pnpm-lock.yaml is explicitly allowed.

`blockExoticSubdeps` is pnpm 11's guard against a transitive dependency pulling
code from a git remote or a bare tarball URL instead of a registry. This fork
turns it off (see the comment on the setting in npm-packages/pnpm-workspace.yaml):
it is a binary switch with no per-package allowlist, and leaving it on breaks
both dashboard image builds, because `pnpm deploy` re-resolves dashboard-common
as a subdependency and its `saffron` dependency stops being direct.

Turning a security check off and leaving nothing behind is how a genuinely
untrusted dependency slips in later. This restores the property that actually
mattered -- no *new* exotic dependency appears unreviewed -- as an explicit
allowlist, checked in CI ahead of the release image builds.

Adding an entry here is a deliberate act. An exotic dependency must be pinned to
an immutable commit SHA (not a branch or tag, which can be moved under us) and
carry an integrity hash in the lockfile, so `--frozen-lockfile` installs are
reproducible and tamper-evident.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

LOCKFILE = Path(__file__).resolve().parent.parent / "npm-packages" / "pnpm-lock.yaml"

# Exotic resolutions this repo has reviewed and accepts, as they appear in the
# lockfile's `resolution: {...}` tarball URL. Keep pinned to a 40-hex commit SHA.
ALLOWED_EXOTIC = {
    "https://codeload.github.com/get-convex/saffron/tar.gz/af61963a3840ddd6e72f44cf320f56ff4b8d0b39",
}

# Matches the resolution line pnpm writes for git-hosted or bare-tarball packages.
RESOLUTION_RE = re.compile(
    r"^\s*'?(?P<pkg>[^']+?)'?:\s*$|^\s*resolution:\s*\{(?P<body>[^}]*)\}"
)
TARBALL_RE = re.compile(r"tarball:\s*(?P<url>\S+?)\s*[,}]?$")


def find_exotic(text: str) -> list[tuple[str, str]]:
    """Return (package_key, tarball_url) for every exotic resolution."""
    found: list[tuple[str, str]] = []
    current_pkg = "<unknown>"
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.endswith(":") and "resolution:" not in stripped:
            current_pkg = stripped.rstrip(":").strip("'\"") or current_pkg
            continue
        if "resolution:" not in stripped:
            continue
        # Only git-hosted or bare-tarball resolutions are "exotic"; registry
        # packages resolve with an `integrity:` and no tarball/gitHosted marker.
        if "gitHosted: true" not in stripped and "tarball:" not in stripped:
            continue
        match = TARBALL_RE.search(stripped.rstrip("}"))
        url = match.group("url").rstrip(",}") if match else stripped
        found.append((current_pkg, url))
    return found


def main() -> int:
    if not LOCKFILE.exists():
        print(f"ERROR: {LOCKFILE} not found", file=sys.stderr)
        return 1

    exotic = find_exotic(LOCKFILE.read_text(encoding="utf-8"))
    unexpected = [(pkg, url) for pkg, url in exotic if url not in ALLOWED_EXOTIC]

    if unexpected:
        print(
            "ERROR: unreviewed exotic (git/tarball) dependency in "
            "npm-packages/pnpm-lock.yaml.\n",
            file=sys.stderr,
        )
        for pkg, url in unexpected:
            print(f"  {pkg}\n    -> {url}", file=sys.stderr)
        print(
            "\nThis fork disables pnpm's `blockExoticSubdeps`, so this check is the\n"
            "only thing standing between a git-resolved dependency and a release\n"
            "image. If the dependency is legitimate, pin it to an immutable commit\n"
            "SHA and add its tarball URL to ALLOWED_EXOTIC in "
            f"{Path(__file__).name}.",
            file=sys.stderr,
        )
        return 1

    # A stale allowlist is its own hazard: an entry that no longer matches the
    # lockfile (say, after an upstream bumps the pin) silently stops protecting
    # anything, and the next bump would be waved through by the leftover entry.
    seen = {url for _, url in exotic}
    stale = ALLOWED_EXOTIC - seen
    if stale:
        print(
            "ERROR: ALLOWED_EXOTIC has entries not present in the lockfile.\n"
            "Remove them, or update the pin if the dependency moved:\n",
            file=sys.stderr,
        )
        for url in sorted(stale):
            print(f"  {url}", file=sys.stderr)
        return 1

    print(f"OK: {len(exotic)} exotic dependency(ies), all allowlisted.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
