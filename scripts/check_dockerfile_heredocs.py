#!/usr/bin/env python3
"""Reject backslash-escaped `$` inside unquoted Dockerfile heredocs.

A `RUN <<EOF` body is expanded by the Dockerfile parser before the shell sees
it. Writing `\\$(cmd)` there to "escape" the dollar does not do what it looks
like: the backslash survives into the shell, `"\\$(cmd)"` is a literal string
rather than a command substitution, and the command silently never runs.

That cost a full release. `export PROTOC="\\$(mise which protoc)"` set PROTOC to
the seven-character string `$(mise...)`, and the build died minutes later in
prost-build with "Could not find protoc" -- an error naming neither the heredoc
nor the escape.

The fix is a quoted delimiter, `RUN <<'EOF'`, which disables Dockerfile
expansion entirely so the shell receives the body verbatim and plain `$(cmd)`
and `${var}` work as written. This check enforces that: inside an unquoted
heredoc, `\\$` is always a bug.

It exists because the Docker daemon is not usable on the machine this repo is
developed from, so nothing local catches it -- the first signal is a red
release ~15 minutes in.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCKERFILE_DIR = ROOT / "self-hosted" / "docker-build"

# `RUN ... <<EOF` / `<<-EOF`, capturing the delimiter and whether it is quoted.
HEREDOC_START = re.compile(r"<<-?\s*(?P<quote>['\"])?(?P<delim>[A-Za-z_][A-Za-z0-9_]*)(?P=quote)?")


def dockerfiles() -> list[Path]:
    return sorted(DOCKERFILE_DIR.glob("Dockerfile*"))


def check(path: Path) -> list[str]:
    problems: list[str] = []
    lines = path.read_text(encoding="utf-8").splitlines()
    rel = path.relative_to(ROOT).as_posix()

    delim: str | None = None
    start_line = 0

    for n, line in enumerate(lines, start=1):
        # Dockerfile comments are not heredoc bodies, and this file's own
        # commentary quotes `<<EOF` and `\$` while explaining them. Skipping
        # comments keeps the check from flagging its own documentation.
        if line.lstrip().startswith("#"):
            continue

        if delim is None:
            m = HEREDOC_START.search(line)
            if m:
                delim = m.group("delim")
                start_line = n
            continue

        if line.strip() == delim:
            delim = None
            continue

        # Flagged regardless of whether the delimiter was quoted. Quoting only
        # controls Dockerfile-level expansion; `\$` is a *shell* escape, so in
        # `"\$(cmd)"` bash produces a literal `$(cmd)` either way. Neither form
        # of heredoc makes it mean command substitution.
        if r"\$" in line:
            problems.append(
                f"{rel}:{n}: `\\$` inside the heredoc opened on line "
                f"{start_line}.\n      {line.strip()}\n"
                f"    bash reads `\\$` as a literal `$`, so the substitution "
                f"never runs. Use `<<'{delim}'` and a plain `$`."
            )

    return problems


def main() -> int:
    files = dockerfiles()
    if not files:
        print(f"ERROR: no Dockerfiles under {DOCKERFILE_DIR}", file=sys.stderr)
        return 1

    problems = [p for f in files for p in check(f)]
    if problems:
        print("Dockerfile heredoc check failed:\n", file=sys.stderr)
        for p in problems:
            print(f"  - {p}\n", file=sys.stderr)
        return 1

    print(f"OK: checked heredocs in {len(files)} Dockerfiles.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
