#!/usr/bin/env bash
# Prints `version=<v>` for a release job: the tag without its `v` on a
# tag push, the workspace version otherwise (a workflow_dispatch or CI
# packaging run -- which must still produce archives whose binary reports
# exactly that version, so the smoke check holds).
set -euo pipefail
if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
    echo "version=${GITHUB_REF#refs/tags/v}"
else
    echo "version=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
fi
