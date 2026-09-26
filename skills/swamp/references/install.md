# Install the Swamp binary

The skill is instructions, not an executable. Check `command -v swamp`,
`type -a swamp`, and `swamp --version` first. Use an existing working copy
unless the task calls for an installation or update. Do not start observation,
scheduling, or cleanup as a side effect of installation.

## Choose the platform

Run `uname -s` and `uname -m`.

| OS / architecture | Published binary | Preferred installation |
| --- | --- | --- |
| Darwin / arm64 | `aarch64-apple-darwin` | Homebrew; archive if Homebrew is unavailable |
| Linux / x86_64 | `x86_64-unknown-linux-gnu` | Release archive; glibc, built on Ubuntu 24.04 |
| Anything else | No matching published binary | Explain the unsupported target; do not substitute another architecture |

On a Mac running an x86_64 shell under Rosetta, check
`sysctl -in sysctl.proc_translated`. A result of `1` indicates translation:
use native arm64 execution and native Homebrew. It does not mean an Intel Mac
is supported. Linux musl distributions are not supported by the glibc archive.

## Apple silicon macOS with Homebrew

```sh
brew install open-horizon-labs/tap/swamp
"$(brew --prefix)/bin/swamp" --version
```

For an existing formula installation, use `brew update` and `brew upgrade swamp`.
Do not install Homebrew itself merely to install Swamp; use the archive below
if it is absent. No sudo is needed for Swamp's per-user archive installation.

## Checksum-verified archive

This Bash block selects the published target, downloads the latest archive
into a fresh temporary directory, and installs only after checksum verification.
It replaces `~/.local/bin/swamp`; inspect any existing file or symlink there
before replacing a user-managed installation. Stop on a failed download,
checksum, extraction, or unsupported platform—do not continue with older files.

```bash
(
  set -eu
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    *) echo "No published Swamp binary for this platform" >&2; exit 1 ;;
  esac
  archive="swamp-$target"
  install_tmp=$(mktemp -d)
  cd "$install_tmp"
  base=https://github.com/open-horizon-labs/swamp/releases/latest/download
  curl -fLO "$base/$archive.tar.gz"
  curl -fLO "$base/$archive.tar.gz.sha256"
  case "$target" in
    aarch64-apple-darwin) shasum -a 256 -c "$archive.tar.gz.sha256" ;;
    x86_64-unknown-linux-gnu) sha256sum -c "$archive.tar.gz.sha256" ;;
  esac
  tar -xzf "$archive.tar.gz"
  mkdir -p "$HOME/.local/bin"
  install -m 755 "$archive/swamp" "$HOME/.local/bin/swamp"
  "$HOME/.local/bin/swamp" --version
  printf 'Download retained at %s\n' "$install_tmp"
)
```

The archive also contains `skills/swamp/`. No MCP binary is required.
If Linux reports a missing GLIBC symbol, report the compatibility failure;
do not replace system libraries. The source repository documents building
with its pinned Rust toolchain for environments without a compatible archive.

## Verify the command the user will run

Run `type -a swamp` and `swamp --version` after installation. Compare with the
explicit installed path (`~/.local/bin/swamp` or `$(brew --prefix)/bin/swamp`).
A previous manual binary can shadow Homebrew. Do not silently remove all copies:
identify the conflicting path and correct PATH or the intended installation.
For the current shell, `export PATH="$HOME/.local/bin:$PATH"` selects a manual
installation; make persistent shell changes only when requested.

An installation is verified when the intended binary executes and its help
matches the skill's commands. It does not establish that a scan has happened.
Start the requested investigation with `swamp scope --json` afterward.
