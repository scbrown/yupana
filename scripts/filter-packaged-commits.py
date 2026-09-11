#!/usr/bin/env python3
"""Keep stdin commit hashes that change a file shipped by `cargo package`."""

from __future__ import annotations

import subprocess
import sys


def run(*args: str, input_text: str | None = None) -> str:
    result = subprocess.run(
        args,
        input=input_text,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result.stdout


packaged = {
    line.strip()
    for line in run("cargo", "package", "--list", "--allow-dirty").splitlines()
    if line.strip() and line.strip() != ".cargo_vcs_info.json"
}
if not packaged:
    sys.stderr.write("ERROR: cargo package listed no repository files\n")
    raise SystemExit(2)

for commit in (line.strip() for line in sys.stdin):
    if not commit:
        continue
    changed = {
        line.strip()
        for line in run(
            "git",
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "-r",
            commit,
        ).splitlines()
        if line.strip()
    }
    if changed & packaged:
        print(commit)
