# Developer-storage detector catalog

Every detector under `crates/core/src/locations/` is read-only: it may
inspect environment variables, the home directory's conventional
layout, a small set of config files (parsed as plain text/simple
key-value lines, never a general-purpose parser for a format that
could hide code), and -- for a handful of detectors -- run one bounded,
read-only, allow-listed tool query (`crate::locations::ALLOWED_COMMANDS`).
No detector loads a shell init script, runs a project's own
config/build, or gates discovery on the tool's own executable being
present -- a leftover cache from an uninstalled tool is still found. See
`docs/architecture.md`'s "Location detector registry" section for the
implementation pattern, and `crate::external`'s doc comments for how a
detector-resolved location becomes a first-class external unit
(`swamp report --view external`).

`swamp scope --json` lists every detector's `id`, resolved locations,
`category`, `provenance`, and `status` for the machine actually running
it; the table below is the human-readable index, current as of catalog
version `2026-09-21.4`.

## Categories

`installation` (an installed program/runtime/SDK version), `downloads`
(a raw, re-downloadable cache of fetched archives/objects), `cache`
(derived/extracted/build-cache content -- still disposable, but
re-materializing it is a local extraction, not a network fetch),
`local-state` (config/state/logs, usually small), `environments`
(mutable per-instance data: a venv, a gemset, a simulator/emulator
device), `build-output` (a build's own artifacts, e.g. Xcode
DerivedData), `models` (model/dataset store), `unclassified` (built-in
default roots, or content this registry cannot honestly classify
further, e.g. Maven's local repository).

## Language version managers (#45, #46)

| Detector id | Locations | Overrides | Notes / source |
|---|---|---|---|
| `mise` | data dir (`local-state`) + `installs/` (`installation`), `downloads/` (`downloads`), `plugins/` (`installation`), `shims/` (`local-state`); `cache/` (`cache`); `config/` (`local-state`) | `MISE_DATA_DIR` (default `~/.local/share/mise` -- **not** `~/.mise`), `MISE_CACHE_DIR` (default `~/.cache/mise`), `MISE_CONFIG_DIR` (default `~/.config/mise`) | https://mise.jdx.dev/directories.html |
| `asdf` | `installs/` (`installation`), `downloads/` (`downloads`), `plugins/` (`installation`), `shims/` (`local-state`), data root (`local-state`) | `ASDF_DATA_DIR` (default `~/.asdf`) | https://asdf-vm.com/guide/getting-started.html |
| `pyenv` | `versions/` (`installation`), `cache/` (`downloads`, source tarballs used to build), `plugins/` (`installation`) | `PYENV_ROOT` (default `~/.pyenv`) | https://github.com/pyenv/pyenv |
| `uv` | Python installs (`installation`), tool envs (`environments`), cache (`cache`) | `UV_PYTHON_INSTALL_DIR` (default `~/.local/share/uv/python`), `UV_TOOL_DIR` (default `~/.local/share/uv/tools`), `UV_CACHE_DIR` (default `~/.cache/uv` -- **on macOS too**, verified against uv's own storage reference, not the general `~/Library/Caches` convention) | https://docs.astral.sh/uv/reference/storage/ |
| `conda` | `~/miniconda3`/`~/anaconda3`/`~/miniforge3` (`installation`, all three always proposed), `~/.conda/envs` (`environments`), `.condarc`'s `envs_dirs`/`pkgs_dirs` (parsed read-only as plain YAML-list lines) | `CONDA_PKGS_DIRS` (comma-separated) | https://docs.conda.io/projects/conda/en/stable/user-guide/configuration/custom-env-and-pkg-locations.html |
| `rbenv` | `versions/` (`installation`), `cache/` (`downloads`), `plugins/` (`installation`), `shims/` (`local-state`) | `RBENV_ROOT` (default `~/.rbenv`) | https://github.com/rbenv/rbenv |
| `rvm` | `rubies/` (`installation`), `gems/` (`environments`, gemsets), `archives/` (`downloads`), `src/` (`downloads`, extracted build sources) | `rvm_path` (lowercase; RVM's own variable name) (default `~/.rvm`) | https://rvm.io |
| `ruby-install` | `~/.rubies`, `/opt/rubies` (`installation`, both always proposed) | -- | https://github.com/postmodern/ruby-install ; chruby is config-only and switches between these same paths, so it has no detector of its own (https://github.com/postmodern/chruby) |
| `nvm` | `versions/node/` (`installation`), `.cache/` (`cache`), data root (`local-state`) | `NVM_DIR` (default `~/.nvm`) | https://github.com/nvm-sh/nvm |
| `rustup` | base (`local-state`), `toolchains/` (`installation`), `downloads/` (`downloads`), `tmp/` (`cache`, update-extraction scratch space) | `RUSTUP_HOME` (default `~/.rustup`) | https://rust-lang.github.io/rustup/environment-variables.html |

## Shared dependency and build caches (#47)

| Detector id | Locations | Overrides | Notes / source |
|---|---|---|---|
| `cargo-home` | base (`installation`: bin/, config.toml, credentials), `registry/cache` (`downloads`, raw `.crate` files), `registry/src` (`cache`, extracted sources), `registry/index` (`downloads`), `git/db` (`downloads`, bare object DBs), `git/checkouts` (`cache`, materialized worktrees) | `CARGO_HOME` (default `~/.cargo`) | Cargo Book, cargo-home |
| `npm` | whole cache dir (`cache`) | `npm_config_cache` / `NPM_CONFIG_CACHE` (default `~/.npm`) | https://docs.npmjs.com/cli/v11/commands/npm-cache/ |
| `pnpm` | content-addressable store (`cache`) | `PNPM_HOME` (store at `$PNPM_HOME/store`), else `~/.npmrc`'s `store-dir` (read-only), else per-platform convention (`~/Library/pnpm/store` macOS, `~/.local/share/pnpm/store` Linux) | https://pnpm.io/settings/store -- **limit:** pnpm's documented per-disk store (a volume without the home directory's own disk gets its own `<volume-root>/.pnpm-store`) is not enumerated; only the home-disk store is proposed |
| `gradle` | `caches/` (`cache`), `wrapper/dists/` (`installation`, extracted wrapper-downloaded distributions), `daemon/` (`local-state`), `native/` (`cache`) | `GRADLE_USER_HOME` (default `~/.gradle`) | https://docs.gradle.org/current/userguide/directory_layout.html |
| `maven` | local repository (`unclassified`) | `~/.m2/settings.xml`'s `<localRepository>` (read as plain text, one tag only), else `~/.m2/repository` | https://maven.apache.org/repositories/local.html -- **limit:** downloaded vs. locally-installed artifacts share this tree with no directory-level signal; per-artifact evidence (`_remote.repositories`/`*.lastUpdated`) is a future classification concern (#74), not this location |
| `go` | `GOMODCACHE/cache/download` (`downloads`, raw module `.zip`/`.info`/`.mod`), `GOMODCACHE` itself (`cache`, extracted read-only module tree), `GOCACHE` (`cache`, build cache) | `GOMODCACHE` env var, else `$GOPATH/pkg/mod` (`GOPATH` env var, else `~/go`); `GOCACHE` env var, else `~/Library/Caches/go-build` (macOS) / `~/.cache/go-build` (Linux); all three also read from the `GOENV` file (default `~/Library/Application Support/go/env` macOS, `~/.config/go/env` Linux) as plain `KEY=VALUE` lines -- `go env` is never executed | https://pkg.go.dev/cmd/go |
| `pip` | cache dir (`cache`) | `PIP_CACHE_DIR` (default `~/Library/Caches/pip` macOS, `~/.cache/pip` Linux) | pip user guide -- uv's own cache is `uv`'s detector, not this one; `__pycache__` is project-level (scattered per project, not one home-relative location) and is deliberately not a detector at all |

## Apple and Android developer tooling (#48)

| Detector id | Locations | Overrides | Notes / source |
|---|---|---|---|
| `xcode` | `DerivedData` (`build-output`), `Archives` (`build-output`, a developer may keep these deliberately, unlike DerivedData), `iOS DeviceSupport`/`watchOS DeviceSupport` (`cache`, shared per-device/OS-version symbol files -- not owned by any one project), `UserData` (`local-state`) | Custom `DerivedData` location via the bounded, allow-listed `defaults read com.apple.dt.Xcode IDECustomDerivedDataLocation` query (a failed/absent query still leaves the conventional path proposed) | Xcode component documentation; macOS only |
| `core-simulator` | `Devices/` (`environments`, mutable per-simulator instance data), `Caches/` (`cache`), `Profiles/Runtimes` (`installation`, per-user) | -- | `/Library/Developer/CoreSimulator/Volumes` (`installation`, system-wide, may be permission-gated -- reported as `RootStatus::Unreadable`, never silently absent); macOS only |
| `android` | `platforms/`, `system-images/`, `build-tools/`, `emulator/` (all `installation`); AVDs (`environments`, mutable emulator instance data) | `ANDROID_HOME`, else `ANDROID_SDK_ROOT` (deprecated, still honored), else `~/Library/Android/sdk`; AVDs: `ANDROID_AVD_HOME`, else `~/.android/avd` | https://developer.android.com/tools/variables -- Gradle's own caches (including Android Gradle Plugin downloads) are the `gradle` detector's job |

## Homebrew, model stores, and VM backing storage (#49)

| Detector id | Locations | Overrides | Notes / source |
|---|---|---|---|
| `homebrew` | prefix (`installation`), `Cellar/` (`installation`, installed formula versions), `Caskroom/` (`installation`, installed cask apps) under every resolved prefix, `~/Library/Caches/Homebrew` (`downloads`) | `HOMEBREW_PREFIX`, else `/opt/homebrew` and `/usr/local` (both always proposed), optionally corroborated by the bounded, read-only `brew --prefix` query | https://docs.brew.sh/Manpage ; macOS only. **Off by default** (stack/26): a system-wide install tree, not a per-user one -- `swamp scope` reports it `disabled (default off)`; `[scan] enabled_detectors = ["homebrew"]` turns it on. |
| `huggingface` | `HF_HOME` base (`local-state`), hub cache (`models`, blobs/snapshots -- blobs counted once because `resize_artifact` never follows symlinks), legacy datasets cache (`models`) | `HF_HOME` (default `~/.cache/huggingface`), `HF_HUB_CACHE` (default `$HF_HOME/hub`), `HF_DATASETS_CACHE` (default `$HF_HOME/datasets`) | https://huggingface.co/docs/huggingface_hub/guides/manage-cache |
| `ollama` | `~/.ollama` base (`local-state`: config/logs/keys), models dir (`models`: `blobs/` + `manifests/`, blobs referenced by filename digest, not symlinked, so one whole-directory measurement already counts each once) | `OLLAMA_MODELS` (default `~/.ollama/models`) | https://docs.ollama.com/faq |
| `docker-desktop` | VM backing-store directory (`local-state`, holding the sparse `Docker.raw`/`Docker.qcow2`; measured bytes are *allocated* disk blocks via the existing per-file `st_blocks * 512` accounting, never the much larger apparent/virtual disk size); OrbStack's data directory (`local-state`) | `~/Library/Containers/com.docker.docker/Data/vms/0/data`; `~/.orbstack/data` | https://docs.docker.com/desktop/troubleshoot-and-support/faqs/macfaqs/ -- **limit:** a disk image relocated via Docker Desktop's Settings > Resources > Advanced is not read (no reliably documented, version-stable settings-file field was found); never summed with `crate::docker`'s separate logical-object (image/container/volume) accounting; macOS only |

## The external-location double-measurement fix

A detector-resolved location that folds into another kept root as a
nested subdirectory (e.g. Homebrew's downloads cache inside the
built-in `~/Library/Caches` default, or Cargo home's own `registry`/
`git` subdirectories inside its own base directory) is pruned from that
root's ordinary walk (`scope::EffectiveScope::external_pruned_subtrees`)
and excluded from any *other* external candidate's own measurement
(`external::discover_and_measure`'s per-candidate nested-exclusion,
`walk::resize_artifact_excluding`) -- so the same bytes are counted
exactly once, never twice. See `docs/architecture.md`'s "External and
shared storage units" section and
`crates/core/tests/external_double_measurement.rs`.

## Known gaps (human review welcome)

- pnpm's per-volume stores (see above) are not enumerated.
- Maven's downloaded-vs-locally-installed split needs per-artifact
  evidence this registry does not inspect.
- Docker Desktop's relocated-disk-image preference is not read.
- The full catalog is Linux-target-gated where a detector's paths are
  macOS-specific (`Platform::MacOS` only); Linux's own equivalents
  (where they exist and differ) are a separate validation track, not
  claimed here.
