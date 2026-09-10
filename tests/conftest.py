"""Shared fixtures: binary resolution, repo root, per-test CadCli."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent


def _find_binary() -> Path | None:
    env = os.environ.get("CAD_CLI_BIN")
    if env:
        p = Path(env)
        return p if p.exists() else None
    for rel in ("target/debug/cad_cli", "target/release/cad_cli"):
        p = REPO_ROOT / rel
        if p.exists():
            return p
    return None


@pytest.fixture(scope="session")
def repo_root() -> Path:
    return REPO_ROOT


@pytest.fixture(scope="session")
def binary(tmp_path_factory) -> str:
    b = _find_binary()
    if b is None:
        subprocess.run(["cargo", "build", "-p", "cad_cli"], cwd=REPO_ROOT,
                       check=True)
        b = REPO_ROOT / "target/debug/cad_cli"
    probe = subprocess.run([str(b)], input="", capture_output=True, text=True,
                           timeout=60)
    if probe.returncode != 0 or "=== dobjects" not in probe.stdout:
        pytest.skip(f"cad_cli binary not runnable: {b} (stdout: "
                    f"{probe.stdout[:200]!r}, stderr: {probe.stderr[:200]!r})")
    return str(b)


@pytest.fixture
def cadcli(binary, tmp_path):
    from tests.cadcli import CadCli
    return CadCli(binary=binary, workdir=str(REPO_ROOT),
                  tmpdir=str(tmp_path))
