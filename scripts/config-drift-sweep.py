#!/usr/bin/env python3
"""Run Yupana's config representation guard over files that bypassed hooks."""

import json
import pathlib
import subprocess
import sys


def main() -> int:
    binary = "yupana"
    findings = 0
    for raw in sys.argv[1:]:
        path = pathlib.Path(raw).resolve()
        try:
            content = path.read_text()
        except OSError as error:
            print(f"yupana config drift: UNKNOWN for `{path}` ({error})")
            findings += 1
            continue
        payload = json.dumps(
            {
                "session_id": "config-drift-sweep",
                "cwd": str(path.parent),
                "tool_name": "Write",
                "tool_input": {"file_path": str(path), "content": content},
            }
        )
        result = subprocess.run(
            [binary, "hook", "pre-edit"],
            input=payload,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.stdout.strip():
            print(result.stdout.strip())
            findings += 1
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
