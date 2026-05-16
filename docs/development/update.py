#!/usr/bin/env python3
"""Refresh copied development guidance from the canonical source repository."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


SOURCE_REPO = "https://github.com/joshka/practice.git"


def explicit_source() -> Path | None:
    configured = os.environ.get("DEVELOPMENT_PREFERENCES_DIR")
    if not configured:
        return None
    source = Path(configured).expanduser().resolve()
    generator = source / "scripts" / "generate_downstream_template.py"
    if not generator.exists():
        raise SystemExit(
            f"DEVELOPMENT_PREFERENCES_DIR does not contain {generator.relative_to(source)}"
        )
    return source


def clone_source() -> tuple[tempfile.TemporaryDirectory[str], Path]:
    if shutil.which("git") is None:
        raise SystemExit(
            "git is required to refresh guidance from GitHub. Install git or set "
            "DEVELOPMENT_PREFERENCES_DIR to a local checkout."
        )

    temp = tempfile.TemporaryDirectory(prefix="development-preferences-")
    source = Path(temp.name) / "practice"
    subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "clone", "--depth", "1", SOURCE_REPO, str(source)],
        check=True,
    )
    return temp, source


def main() -> int:
    repo_root = Path(__file__).resolve().parents[2]
    temp: tempfile.TemporaryDirectory[str] | None = None
    source = explicit_source()
    if source is None:
        temp, source = clone_source()

    try:
        generator = source / "scripts" / "generate_downstream_template.py"
        subprocess.run(
            [sys.executable, str(generator), "--output", str(repo_root), "--preserve-agents"],
            check=True,
        )
    finally:
        if temp is not None:
            temp.cleanup()

    print(f"Refreshed development guidance from {source}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
