#!/usr/bin/env python3
"""Select the two executables built inside this installation's private target."""
import json
import sys
from pathlib import Path


def artifacts(log, target):
    target = Path(target).resolve()
    found = {}
    for line in Path(log).read_text().splitlines():
        record = json.loads(line)
        if record.get("reason") != "compiler-artifact" or not record.get("executable"):
            continue
        name = record.get("target", {}).get("name")
        if name not in ("yupana", "install-contract"):
            continue
        path = Path(record["executable"]).resolve()
        if not path.is_relative_to(target) or not path.is_file():
            raise ValueError(f"artifact {name} is missing or outside the private target")
        if name in found:
            raise ValueError(f"duplicate executable artifact: {name}")
        found[name] = path
    if set(found) != {"yupana", "install-contract"}:
        raise ValueError("Cargo did not report both yupana and install-contract executables")
    return found["yupana"], found["install-contract"]


if __name__ == "__main__":
    try:
        for artifact in artifacts(*sys.argv[1:]):
            print(artifact)
    except (ValueError, OSError) as error:
        sys.exit(f"ERROR: {error}")
