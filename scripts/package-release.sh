#!/usr/bin/env bash
# Packages one built swamp binary as a release archive (#88).
#
#   scripts/package-release.sh <target-triple> <version> <binary> <out-dir>
#
# Writes, into <out-dir>:
#   swamp-<version>-<target>.tar.gz  (+ .sha256)  the versioned archive
#   swamp-<target>.tar.gz            (+ .sha256)  the "latest" name the
#                                                  install docs download
# Each archive holds one directory: the binary, README.md and the agent
# skill (skills/swamp/). There is no MCP server binary (#104); the CLI's
# --json output plus the skill is the agent interface.
#
# The same script runs in ci.yml (every push, so packaging is exercised
# long before a tag) and release.yml (the tag), so the archive CI smoke-
# tests is the archive a release publishes.
set -euo pipefail

target="${1:?target triple}"
version="${2:?version}"
binary="${3:?built binary}"
out="${4:?output directory}"

[ -x "$binary" ] || { echo "not an executable: $binary" >&2; exit 1; }
[ -f skills/swamp/SKILL.md ] || { echo "run from the repository root (skills/swamp/SKILL.md)" >&2; exit 1; }

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi
}

mkdir -p "$out"
for name in "swamp-${version}-${target}" "swamp-${target}"; do
    rm -rf "${out:?}/${name}" "${out}/${name}.tar.gz" "${out}/${name}.tar.gz.sha256"
    mkdir -p "${out}/${name}"
    cp "$binary" "${out}/${name}/swamp"
    cp README.md "${out}/${name}/"
    mkdir -p "${out}/${name}/skills"
    cp -R skills/swamp "${out}/${name}/skills/swamp"
    tar -C "$out" -czf "${out}/${name}.tar.gz" "$name"
    (cd "$out" && sha256 "${name}.tar.gz" > "${name}.tar.gz.sha256")
    rm -rf "${out:?}/${name}"
done
ls -l "$out"
