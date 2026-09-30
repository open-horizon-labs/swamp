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
version `2026-09-29.1`.

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
| `core-simulator` | `Devices/` (`environments`, mutable per-simulator instance data), `Caches/` (`cache`), `Profiles/Runtimes` (`installation`, per-user); system-wide (#166): `/Library/Developer/CoreSimulator/Volumes` (`installation`, the mounted runtime volumes), `Caches/` (`cache`), and `Images/`, `Cryptex/`, `Profiles/` (`local-state`), each its own unit | -- | Volumes may be permission-gated -- reported as `RootStatus::Unreadable`, never silently absent; macOS only. **`Volumes/` owns the simulator runtime bytes and `/System/Library/AssetsV2` is never proposed**: the mounted volumes are backed by disk images under `AssetsV2`, `/System` is firmlinked onto the data volume, so the same runtime is reachable by two paths. `Volumes/` is what Xcode and `simctl` present, the path this catalog has always reported and kept history for, and the one the runtime adapter names (`iOS_23F77`); `AssetsV2` is an OS-managed location for every kind of MobileAsset. The row is **mounted size**: the files inside the mounted runtime images, not the space their disk images take, and a runtime that is installed but not mounted is not measured (swamp does not read `AssetsV2` to find out; the row carries that as a limit). The numbers differ (about 39.9 GB through `Volumes/` and about 24.9 GB of `.dmg` files in `AssetsV2`, because a mounted image is measured by the files inside it) and are never summed; `locations::tests::the_simulator_runtimes_are_counted_through_one_path` fails if both paths are ever proposed. |
| `xcode-system` | `/Library/Developer/CommandLineTools` (`installation`, reinstall with `xcode-select --install`), `DeveloperDiskImages/` (`installation`), `CoreDevice/` and `DeviceKit/` (`unclassified`: no documented statement of how they are recreated, so none is made) | -- | System paths no `HOME` relocates (#166); macOS only. `/Library/Developer/PrivateFrameworks` and the loose files there are not measured: with `core-simulator`'s system directories, the units sum to `du` of `/Library/Developer` minus exactly that named remainder. |
| `android` | `platforms/`, `system-images/`, `build-tools/`, `emulator/`, `ndk/`, `cmdline-tools/`, `platform-tools/`, `cmake/` (all `installation`, #165); AVDs (`environments`, mutable emulator instance data) | `ANDROID_HOME`, else `ANDROID_SDK_ROOT` (deprecated, still honored), else `~/Library/Android/sdk`; an NDK that `ANDROID_NDK_HOME`/`ANDROID_NDK_ROOT` names outside the SDK root is a further `installation` (one inside it is already `ndk/`'s child and is not proposed again); AVDs: `ANDROID_AVD_HOME`, else `~/.android/avd` | https://developer.android.com/tools/variables -- Gradle's own caches (including Android Gradle Plugin downloads) are the `gradle` detector's job. `licenses/` (the record of accepted licences) and the loose files beside the folders (`ndk-install.log`, `.knownPackages`, `.temp`) are not measured, so a `du` of the SDK root exceeds the sum of its units by exactly those. A package folder that is a symlink into another SDK (Homebrew's `android-commandlinetools`) is measured at the path it resolves to, once. A missing folder (no NDK installed) is reported missing, not an error. Removing a package: reinstall with `sdkmanager`. |

## Vendor tool directories and agent scratch (#164, #172)

| Detector id | Locations | Overrides | Notes / source |
|---|---|---|---|
| `espressif` | `dist/` (`cache`, downloaded tool archives), `tools/` (`installation`, installed toolchains), `python_env/` (`environments`, one Python virtual environment per ESP-IDF version), each its own unit | `IDF_TOOLS_PATH`, else `~/.espressif` | ESP-IDF's `idf_tools.py`. Removing: reinstall with ESP-IDF's `install.sh` (or `idf_tools.py install` / `install-python-env`). `idf-env.json` and `espidf.constraints.*.txt` (a few KB) are not measured. The `~/esp` checkout (ESP-IDF source) is an ordinary project root the user declares, never proposed here. |
| `claude-code-scratch` | `/private/tmp/claude-<uid>` (`cache`), Claude Code's per-user session scratch, one unit | -- | macOS only; one exact known path, **no other part of `/private/tmp` is scanned or discovered**. A separate detector from `claude-code` on purpose: the agent layer takes a tool's first location as its home, and scratch must never be read as a session store. Removing it while a session runs breaks that session (swamp's own statement). Worktrees inside it are counted under their projects and subtracted here, with a note on the row. See `docs/agent-storage.md` for what is known about its lifetime. |

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
