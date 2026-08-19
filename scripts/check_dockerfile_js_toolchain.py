#!/usr/bin/env python3
"""Every Dockerfile that installs JS dependencies must install yarn first.

`npm-packages/docs` depends on docusaurus-openapi-docs from a git URL. pnpm
builds git-hosted packages by running their `prepare` script, and that
package's prepare script shells out to `yarn install`. So any image that runs
`just install-js` (or pnpm directly) needs yarn on PATH, or the build dies
with:

    ERR_PNPM_PREPARE_PACKAGE  Failed to prepare git-hosted package ...
    root@ yarn-install: `yarn install`
    spawn ENOENT

CI runners happen to preinstall yarn, so this only ever fails inside Docker —
and only after several minutes of downloading. It has now been fixed three
separate times, once per Dockerfile, because nothing connected the dependency
to the images that have to satisfy it. This check is that connection.

Run it directly, or via the Dockerfile Checks workflow.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Commands that trigger a pnpm install of the workspace, which is what pulls
# and prepares the git-hosted dependency.
JS_INSTALL_PATTERN = re.compile(r"install-js|pnpm\s+install|just\s+pnpm\b")

# Ways of putting yarn on PATH that actually work in these images.
YARN_INSTALL_PATTERN = re.compile(r"npm\s+install\s+-g\s+yarn|corepack\s+enable")


def is_dockerfile(name: str) -> bool:
    """`Dockerfile`, `Dockerfile.backend`, `foo.dockerfile` — but not
    `dockerfile_checks.yml`, which merely starts with the word."""
    lowered = name.lower()
    return (
        lowered == "dockerfile"
        or lowered.startswith("dockerfile.")
        or lowered.endswith(".dockerfile")
    )


def dockerfiles() -> list[Path]:
    """Every tracked Dockerfile, so a newly added one is covered automatically."""
    out = subprocess.run(
        ["git", "ls-files"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()
    return [REPO_ROOT / p for p in out if is_dockerfile(Path(p).name)]


def first_match(lines: list[str], pattern: re.Pattern[str]) -> int | None:
    """1-based line number of the first match, or None."""
    for i, line in enumerate(lines, start=1):
        # Ignore comments: the explanatory comment above the fix mentions both
        # `pnpm install` and `yarn`, and must not count as either.
        if line.lstrip().startswith("#"):
            continue
        if pattern.search(line):
            return i
    return None


def main() -> int:
    failures: list[str] = []

    for path in dockerfiles():
        lines = path.read_text(encoding="utf-8").splitlines()
        rel = path.relative_to(REPO_ROOT).as_posix()

        js_line = first_match(lines, JS_INSTALL_PATTERN)
        if js_line is None:
            # Nothing to satisfy — e.g. Dockerfile.orchestrator is Rust only.
            continue

        yarn_line = first_match(lines, YARN_INSTALL_PATTERN)
        if yarn_line is None:
            failures.append(
                f"{rel}:{js_line} installs JS dependencies but never installs "
                f"yarn.\n    Add `RUN npm install -g yarn` before that line."
            )
        elif yarn_line > js_line:
            failures.append(
                f"{rel}: installs yarn on line {yarn_line}, which is after the "
                f"JS install on line {js_line}.\n    Move the yarn install "
                f"above it."
            )

    if failures:
        print("Dockerfile JS toolchain check failed:\n", file=sys.stderr)
        for f in failures:
            print(f"  - {f}\n", file=sys.stderr)
        print(
            "pnpm prepares the git-hosted docusaurus-openapi-docs dependency by\n"
            "running its `prepare` script, which calls `yarn install`. Without\n"
            "yarn on PATH the image build fails with ERR_PNPM_PREPARE_PACKAGE /\n"
            "spawn ENOENT, several minutes in.",
            file=sys.stderr,
        )
        return 1

    print(f"OK: checked {len(dockerfiles())} Dockerfiles.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
