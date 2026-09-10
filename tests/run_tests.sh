#!/usr/bin/env bash
# Full black-box test suite for the headless cad_cli binary.
#
# Usage (from the repo root):  ./tests/run_tests.sh
#
# Steps:
#   1. build target/debug/cad_cli if missing (or stale: pass --rebuild)
#   2. create .venv + install tests/requirements.txt if missing
#   3. run pytest with --junitxml=test-results.xml
#   4. generate tests/report/report.md + tests/report/failures.txt
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--rebuild" ]]; then
    cargo build -p cad_cli
elif [[ ! -x target/debug/cad_cli ]]; then
    echo "==> building target/debug/cad_cli"
    cargo build -p cad_cli
fi

if [[ ! -x .venv/bin/python ]]; then
    echo "==> creating .venv"
    python3 -m venv .venv
    .venv/bin/pip install --quiet -r tests/requirements.txt
fi

echo "==> running pytest"
.venv/bin/python -m pytest tests/ -q --junitxml=test-results.xml "$@"

echo "==> generating report"
.venv/bin/python tests/make_report.py test-results.xml

echo "==> done: tests/report/report.md"
