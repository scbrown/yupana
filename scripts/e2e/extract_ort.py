#!/usr/bin/env python3
"""Extract libonnxruntime.so from a downloaded `onnxruntime` PyPI wheel.

quipu's `ort` crate is built `load-dynamic`, so it needs an ONNX Runtime
shared library at runtime (`$ORT_DYLIB_PATH`). Sandboxes that block the
usual runtime downloads almost always allow PyPI — and the manylinux wheel
carries the exact library. Called by `just e2e f1`; safe no-op when the
wheel is absent or the lib is already in place.
"""

from __future__ import annotations

import glob
import shutil
import zipfile
from pathlib import Path

MODELS = Path(__file__).resolve().parents[2] / "target" / "models"


def main() -> int:
    target = MODELS / "libonnxruntime.so"
    if target.exists():
        return 0
    wheels = sorted(glob.glob(str(MODELS / ".ortwheel" / "*.whl")))
    if not wheels:
        print("extract_ort: no onnxruntime wheel found; semantic arm stays off")
        return 0
    bundle = zipfile.ZipFile(wheels[0])
    libs = [
        n
        for n in bundle.namelist()
        if "libonnxruntime.so" in n and "providers" not in n
    ]
    if not libs:
        print(f"extract_ort: no libonnxruntime.so inside {wheels[0]}")
        return 0
    extracted = bundle.extract(libs[0], MODELS / ".ortlib")
    shutil.copy(extracted, target)
    print(f"extract_ort: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
