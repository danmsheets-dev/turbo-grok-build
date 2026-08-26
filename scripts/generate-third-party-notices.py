#!/usr/bin/env python3
"""Regenerate THIRD-PARTY-NOTICES from cargo-about + the adapted-source header."""

from __future__ import annotations

import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
NOTICES = ROOT / "THIRD-PARTY-NOTICES"
TEMPLATE = ROOT / "about.hbs"
MANIFEST = ROOT / "crates" / "codegen" / "xai-grok-pager-bin" / "Cargo.toml"


def main() -> int:
    orig = NOTICES.read_text(encoding="utf-8")
    marker = "PART I"
    idx = orig.find(marker)
    if idx < 0:
        print("THIRD-PARTY-NOTICES is missing a PART I marker", file=sys.stderr)
        return 1
    part0 = orig[:idx].rstrip() + "\n\n"
    out = ROOT / "tmp-about-out.txt"
    cmd = [
        "cargo",
        "about",
        "generate",
        str(TEMPLATE),
        "--manifest-path",
        str(MANIFEST),
        "--features",
        "community-build",
        "--locked",
        "--fail",
        "-o",
        str(out),
    ]
    subprocess.check_call(cmd, cwd=ROOT)
    generated = out.read_text(encoding="utf-8")
    gidx = generated.find("PART I")
    if gidx < 0:
        print("cargo-about output is missing PART I", file=sys.stderr)
        return 1
    combined = part0 + generated[gidx:]
    combined = combined.replace("\r\n", "\n")
    if not combined.endswith("\n"):
        combined += "\n"
    NOTICES.write_text(combined, encoding="utf-8", newline="\n")
    print(f"wrote {NOTICES} ({len(combined)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
