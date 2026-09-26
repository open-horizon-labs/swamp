//! The explicit major-tool matrix #90/#91 require: every named tool is
//! listed here, always -- never a silent omission and never an empty
//! placeholder adapter that claims support it does not have. Each row
//! states its home-path evidence and primary source(s), researched
//! during implementation (never guessed, never learned from a real
//! `~/.claude`-style directory on this machine).
//!
//! ## Every row used to say `Supported`. Two of them had not earned it.
//!
//! The 2026-09-21 review's objection: having an adapter is not the same
//! as having confirmed the layout that adapter models, and this table
//! conflated the two -- while several rows' own `note` fields admitted
//! an *assumed* Windsurf shape and *unconfirmed* Cline/Roo Code project
//! fields. [`SupportLevel::Unverified`] exists so those are a level
//! rather than a footnote, and
//! `crate::agents::discover_and_measure` withholds every action and
//! every project link for an `Unverified` tool. Identification still
//! runs: knowing roughly where the bytes are is useful, offering to move
//! them on an unconfirmed layout is not.
//!
//! Each row now carries [`MatrixEntry::verification`]: what was checked,
//! against which upstream path and commit, on what date. A row without
//! one cannot be `Supported`, and the test at the bottom of this file
//! enforces that. `docs/agent-storage.md` renders this same table as a
//! doc and `crates/core/tests/agent_matrix_matches_docs.rs` parses the
//! doc back and compares it with this constant, so the two cannot
//! drift.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentToolId {
    ClaudeCode,
    Codex,
    /// The Codex **desktop app** (`Codex.app`), a materially different
    /// client from the Codex CLI with its own storage -- modeled as its
    /// own row rather than folded into `Codex`'s, per #93's explicit
    /// "do not extrapolate one client's schema to all clients"
    /// acceptance.
    CodexDesktop,
    OhMyPi,
    OpenCode,
    GeminiCli,
    Pi,
    Aider,
    GithubCopilotCli,
    Cursor,
    Windsurf,
    Cline,
    RooCode,
    Continue,
}

impl AgentToolId {
    pub fn slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::CodexDesktop => "codex-desktop",
            Self::OhMyPi => "oh-my-pi",
            Self::OpenCode => "opencode",
            Self::GeminiCli => "gemini-cli",
            Self::Pi => "pi",
            Self::Aider => "aider",
            Self::GithubCopilotCli => "github-copilot-cli",
            Self::Cursor => "cursor",
            Self::Windsurf => "windsurf",
            Self::Cline => "cline",
            Self::RooCode => "roo-code",
            Self::Continue => "continue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportLevel {
    /// Real identification code exists (`crate::agents::<tool>` +, for
    /// the home directory itself, a `crate::locations` detector) **and**
    /// the layout it models is confirmed against that tool's own source
    /// or documentation, cited in [`MatrixEntry::verification`].
    Supported,
    /// Identification code exists, but the layout it models could not be
    /// confirmed against the tool's own source or documentation.
    /// Units are still identified and measured; no action is offered and
    /// project linkage is reported `Unresolved`, because both would rest
    /// on the layout this level says is unconfirmed. Enforced in
    /// `crate::agents::discover_and_measure`, not left to each adapter.
    Unverified,
    /// Home-path evidence is researched and recorded below; no
    /// identification code exists yet. Never rendered as "unknown" (an
    /// unresearched gap) or silently omitted from the matrix.
    Planned,
}

impl SupportLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unverified => "unverified",
            Self::Planned => "planned",
        }
    }

    /// Whether any selective action may be offered for this tool's
    /// units. The single place that question is answered.
    pub fn actions_available(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// What was checked, where, and when -- so a later implementer re-runs
/// the check instead of trusting this table blind. `checked` names the
/// specific claim; `source` is the exact upstream repo path or doc URL;
/// `revision` is a commit SHA, a branch plus a retrieval date, or a
/// retrieval date for a doc page.
#[derive(Debug, Clone, Serialize)]
pub struct Verification {
    pub checked: &'static str,
    pub source: &'static str,
    pub revision: &'static str,
}

/// One row of the required major-tool matrix. Serialize-only: this table
/// is a compiled-in constant, never parsed back from JSON.
#[derive(Debug, Clone, Serialize)]
pub struct MatrixEntry {
    pub id: AgentToolId,
    pub display_name: &'static str,
    pub support: SupportLevel,
    /// Documented home directory / override, in prose (paths vary by
    /// platform for several of these tools; see `note`).
    pub home_note: &'static str,
    /// Primary sources consulted for `home_note`, recorded so a later
    /// implementer can re-verify against the current documentation
    /// rather than trusting this table blind.
    pub sources: &'static [&'static str],
    /// Caveats: version-dependence, platform variance, or "community-
    /// sourced, no single official layout doc found" honesty notes.
    pub note: &'static str,
    /// The checks behind `support`. A `Supported` row must have at least
    /// one; an `Unverified` row records what was *attempted* and did not
    /// confirm, which is the more useful half.
    pub verification: &'static [Verification],
}

/// This tool's support level, by its registry/detector id, or `None` for
/// an id with no matrix row.
pub fn support_for(tool_id: &str) -> Option<SupportLevel> {
    MATRIX
        .iter()
        .find(|e| e.id.slug() == tool_id)
        .map(|e| e.support)
}

/// The full required matrix, in the epic's stated priority order
/// (Claude Code, Codex, Oh My Pi, OpenCode, then the remaining named
/// tools). Extending this list for a *new* tool beyond the named set is
/// ordinary catalog review (#90's own text); this constant is not itself
/// meant to be exhaustive of every coding agent that will ever exist.
pub const MATRIX: &[MatrixEntry] = &[
    MatrixEntry {
        id: AgentToolId::ClaudeCode,
        display_name: "Claude Code",
        support: SupportLevel::Supported,
        home_note: "~/.claude, or $CLAUDE_CONFIG_DIR if set; a sibling ~/.claude.json also \
                     exists outside the home directory and is not modeled (see \
                     crate::locations::claude_code doc comment)",
        sources: &[
            "https://code.claude.com/docs/en/claude-directory",
            "https://code.claude.com/docs/en/settings",
            "https://code.claude.com/docs/en/checkpointing",
            "https://code.claude.com/docs/en/authentication",
        ],
        note: "current documented layout as of this chunk; the transcript JSONL schema itself \
               is explicitly documented upstream as internal/unstable across versions",
        verification: &[
            Verification {
                checked: "the ~/.claude directory table (projects/, \
                          sessions/, file-history/, shell-snapshots/, \
                          plugins/, settings.json, .credentials.json) and \
                          the CLAUDE_CONFIG_DIR override, which the page \
                          states re-roots every path on it",
                source: "docs/en/claude-directory",
                revision: "code.claude.com, retrieved 2026-09-22",
            },
            Verification {
                checked: "todos/, statsig/ and logs/ are documented in one \
                          row as legacy directories from older versions, \
                          no longer written -- so they are NOT \
                          auto-regenerating caches, and the what-you-lose \
                          table answers Nothing for all of them plus \
                          image-cache/. This adapter previously called \
                          statsig community-documented and modelled it as \
                          regenerating; both halves were wrong",
                source: "docs/en/claude-directory",
                revision: "code.claude.com, retrieved 2026-09-22",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::Codex,
        display_name: "Codex",
        support: SupportLevel::Supported,
        home_note: "CODEX_HOME, default ~/.codex; sessions/ and archived_sessions/ (year/month/ \
                     day rollout-*.jsonl trees), auth.json, history.jsonl, config.toml, log/ \
                     (overridable by the log_dir config key), a deprecated skills/ (current \
                     root: ~/.agents/skills), seven SQLite state stores \
                     (state_5/logs_2/goals_1/memories_1/memories_v2_1/queue_1/ \
                     thread_history_1.sqlite) relocatable via the separate CODEX_SQLITE_HOME",
        sources: &[
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/utils/home-dir/src/lib.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/rollout/src/lib.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/rollout/src/list.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/rollout/src/rollout_file_name.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/rollout/src/metadata.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/protocol/src/protocol.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/state/src/sqlite.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/state/src/lib.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/config/src/config_toml.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/core/src/config/mod.rs",
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/ext/skills/src/host_roots.rs",
        ],
        note: "#93; crate::agents::codex implements identification. Session linkage uses the \
               read-only state index's exact rollout_path/cwd columns; missing or conflicting \
               entries remain unresolved, with no transcript-content fallback. No managed-worktree \
               creation by the CLI itself is confirmed. skills/ \
               under CODEX_HOME is upstream-deprecated in favour of ~/.agents/skills, and log/ \
               can be moved by config -- both are identified where they are, never assumed",
        verification: &[
            Verification {
                checked: "CODEX_HOME and the ~/.codex default \
                          (find_codex_home)",
                source: "codex-rs/utils/home-dir/src/lib.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "SESSIONS_SUBDIR / ARCHIVED_SESSIONS_SUBDIR, and \
                          the year/month/day directory walk beneath them",
                source: "codex-rs/rollout/src/lib.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "the YYYY/MM/DD rollout tree is walked as three \
                          nested numeric directory levels",
                source: "codex-rs/rollout/src/list.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "the rollout filename shape \
                          rollout-{timestamp}-{thread_id}.jsonl",
                source: "codex-rs/rollout/src/rollout_file_name.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "a session's declared working directory is \
                          session_meta.meta.cwd",
                source: "codex-rs/rollout/src/metadata.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "SessionMeta declares cwd: PathBuf",
                source: "codex-rs/protocol/src/protocol.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "RUNTIME_DBS is declared [RuntimeDbSpec; 7] -- \
                          SEVEN runtime databases, each \
                          codex_home.join(filename): state_5, logs_2, \
                          goals_1, memories_1, memories_v2_1, queue_1, \
                          thread_history_1. This row said six; \
                          memories_v2_1.sqlite is easy to miss because its \
                          filename is an inline literal rather than one of \
                          the six *_DB_FILENAME consts, and it was neither \
                          folded with its -wal/-shm sidecars nor protected",
                source: "codex-rs/state/src/sqlite.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "CODEX_SQLITE_HOME is a real env var \
                          (SQLITE_HOME_ENV) and this file holds only that \
                          -- NOT the database filenames, which this row \
                          previously cited it for",
                source: "codex-rs/state/src/lib.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "the sqlite_home and log_dir config keys, the \
                          latter documented as defaulting to \
                          $CODEX_HOME/log",
                source: "codex-rs/config/src/config_toml.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "log/ resolves to codex_home.join(\"log\") when \
                          log_dir is unset",
                source: "codex-rs/core/src/config/mod.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "skills/ under CODEX_HOME carries the upstream \
                          comment Deprecated user skills location; the \
                          current root is ~/.agents/skills \
                          (AGENTS_DIR_NAME + SKILLS_DIR_NAME)",
                source: "codex-rs/ext/skills/src/host_roots.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::CodexDesktop,
        display_name: "Codex desktop app",
        support: SupportLevel::Supported,
        home_note: "logs only, and only the two platforms upstream confirms: macOS \
                     ~/Library/Logs/<identity> and Windows %LOCALAPPDATA%/Codex/Logs, each \
                     day-partitioned YYYY/MM/DD. Linux is genuinely unconfirmed (the upstream \
                     match returns None for it). Settings/session storage beyond logs is not \
                     confirmed by primary source and is not modeled -- logs-only support, \
                     stated explicitly rather than silently treated as empty. This adapter \
                     identifies the macOS root; the Windows one is confirmed by source but \
                     belongs to the Windows track",
        sources: &[
            "https://github.com/openai/codex/blob/ac7634b9f73ec1bf96466be7a5869f0949d20b30/codex-rs/cli/src/doctor/desktop.rs",
        ],
        note: "#93; crate::agents::codex_desktop implements identification for the confirmed \
               log directory only, a deliberately partial Supported row",
        verification: &[
            Verification {
                checked: "desktop_log_root is defined in \
                          doctor/desktop.rs, not \
                          doctor/desktop/platform.rs -- which contains no \
                          log_root at all and is what this row previously \
                          cited",
                source: "codex-rs/cli/src/doctor/desktop.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
            Verification {
                checked: "the function matches exactly two platforms: \
                          macos -> $HOME/Library/Logs/<identity>, windows \
                          -> %LOCALAPPDATA%/Codex/Logs (falling back to \
                          %USERPROFILE%/AppData/Local). Both are \
                          confirmed, so this row's previous \
                          no-Windows-build-confirmed claim was false; \
                          every other platform returns None, so Linux is \
                          the only genuinely unconfirmed one. Both roots \
                          are then day-partitioned YYYY/MM/DD",
                source: "codex-rs/cli/src/doctor/desktop.rs",
                revision: "openai/codex main @ \
                           ac7634b9f73ec1bf96466be7a5869f0949d20b30",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::OhMyPi,
        display_name: "Oh My Pi",
        support: SupportLevel::Supported,
        home_note: "~/.omp/agent (user-confirmed identity: Oh My Pi, a fork of badlogic/ \
                     pi-mono), or the whole of PI_CODING_AGENT_DIR when set; sessions/ \
                     <encoded-cwd>/<ts>_<session-id>.jsonl, content-addressed blobs/<sha256>, \
                     terminal-sessions/, config.yml/config.yaml, models.yml, agent.db (SQLite \
                     auth store). Two upstream relocations are UNMODELLED and stated rather \
                     than left silent: a named profile (--profile / OMP_PROFILE) re-roots \
                     everything under ~/.omp/profiles/<name>/, and an existing XDG \
                     data/state/cache directory relocates the corresponding subtrees",
        sources: &[
            "https://github.com/can1357/oh-my-pi/blob/fd3f8e3c569b511611081e16b181b740a4c98599/packages/utils/src/dirs.ts",
            "https://github.com/can1357/oh-my-pi/blob/fd3f8e3c569b511611081e16b181b740a4c98599/docs/config-usage.md",
        ],
        note: "#94; crate::agents::oh_my_pi implements identification, including a content-marker \
               check that reports an explicit unknown-format unit rather than guessing when \
               ~/.omp is not actually Oh My Pi's own layout, and bounded per-session blob-\
               reference accounting that never offers blob removal in this chunk",
        verification: &[
            Verification {
                checked: "the whole directory layout in upstream's own \
                          path helpers: CONFIG_DIR_NAME = .omp, \
                          getAgentDir, getSessionsDir, getBlobsDir, \
                          getLogsDir, getReportsDir, and the \
                          agent.db/history.db/models.db SQLite stores",
                source: "packages/utils/src/dirs.ts",
                revision: "can1357/oh-my-pi main @ \
                           fd3f8e3c569b511611081e16b181b740a4c98599",
            },
            Verification {
                checked: "PI_CODING_AGENT_DIR is honoured for the default \
                          profile only; a named profile (--profile / \
                          OMP_PROFILE) re-roots everything under \
                          ~/.omp/profiles/<name>/, and an existing XDG \
                          data/state/cache directory can relocate the \
                          corresponding subtrees. Neither is modelled by \
                          this adapter, which is why this row's home_note \
                          now says so",
                source: "docs/config-usage.md",
                revision: "can1357/oh-my-pi main @ \
                           fd3f8e3c569b511611081e16b181b740a4c98599",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::OpenCode,
        display_name: "OpenCode",
        support: SupportLevel::Supported,
        home_note: "data: ${XDG_DATA_HOME:-~/.local/share}/opencode (auth.json, log/, \
                     storage/{session,message,part,project}/ directories plus \
                     storage/session_diff/<session-id>.json FILES, or opencode.db depending on \
                     version, snapshot/<project-id>/<hash> git-backed checkpoints); config \
                     ${XDG_CONFIG_HOME:-~/.config}/opencode, also settable by \
                     OPENCODE_CONFIG_DIR (opaque external unit); cache \
                     ${XDG_CACHE_HOME:-~/.cache}/opencode (opaque external unit). Upstream \
                     defines more roots than this catalog models: state \
                     (${XDG_STATE_HOME:-~/.local/state}/opencode) and tmp are UNMODELLED, \
                     stated here rather than left silent",
        sources: &[
            "https://github.com/sst/opencode/blob/fe3f3a41f79ad292cc3c7c629567385a20ec5130/packages/core/src/global.ts",
            "https://github.com/sst/opencode/blob/fe3f3a41f79ad292cc3c7c629567385a20ec5130/packages/opencode/src/storage/storage.ts",
            "https://github.com/sst/opencode/blob/fe3f3a41f79ad292cc3c7c629567385a20ec5130/packages/opencode/src/session/revert.ts",
            "https://github.com/sst/opencode/blob/fe3f3a41f79ad292cc3c7c629567385a20ec5130/packages/core/src/database/database.ts",
            "https://opencode.ai/docs/config/",
        ],
        note: "#95; crate::agents::opencode implements identification for both the older file- \
               tree layout and the newer SQLite-backed one, version-gated by which markers are \
               present on disk; an unrecognized layout is reported as one explicit \
               unsupported-version unit rather than guessed at either schema. This row \
               previously named an OPENCODE_DATA_DIR override 'honored defensively'; no such \
               variable exists upstream and the claim is withdrawn",
        verification: &[
            Verification {
                checked: "the roots, and there are more than the three \
                          this row claimed: data \
                          ($XDG_DATA_HOME/opencode), config, cache, state \
                          ($XDG_STATE_HOME/opencode) and tmp, plus derived \
                          bin (under cache), log and repos. state/ is real \
                          and is NOT modelled by this catalog -- listed \
                          explicitly rather than left silent. There is no \
                          OPENCODE_DATA_DIR",
                source: "packages/core/src/global.ts",
                revision: "sst/opencode dev @ \
                           fe3f3a41f79ad292cc3c7c629567385a20ec5130",
            },
            Verification {
                checked: "every storage key becomes path.join(dir, ...key) \
                          + \".json\", so \
                          storage/session_diff/<session-id> is a FILE and \
                          not, as this row said, a companion directory -- \
                          the adapter gated on is_dir() and those bytes \
                          appeared in no unit at all",
                source: "packages/opencode/src/storage/storage.ts",
                revision: "sst/opencode dev @ \
                           fe3f3a41f79ad292cc3c7c629567385a20ec5130",
            },
            Verification {
                checked: "the runtime writer is \
                          storage.write([session_diff, sessionID], diffs), \
                          which is the same key-to-path builder",
                source: "packages/opencode/src/session/revert.ts",
                revision: "sst/opencode dev @ \
                           fe3f3a41f79ad292cc3c7c629567385a20ec5130",
            },
            Verification {
                checked: "the newer SQLite-backed layout puts opencode.db \
                          in the data root (or opencode-<channel>.db, or \
                          OPENCODE_DB)",
                source: "packages/core/src/database/database.ts",
                revision: "sst/opencode dev @ \
                           fe3f3a41f79ad292cc3c7c629567385a20ec5130",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::GeminiCli,
        display_name: "Gemini CLI",
        support: SupportLevel::Supported,
        home_note: "~/.gemini, or the whole of GEMINI_CLI_HOME when set (settings.json, \
                     GEMINI.md, extensions/, trustedFolders.json, oauth_creds.json, and \
                     tmp/bin -- NOT bin/, which no version writes); \
                     tmp/<project-id>/ (shell_history, checkpoints/, chats/) and \
                     history/<project-id>/ (shadow Git checkpoint repos), where <project-id> is \
                     a legacy sha256 of the project root or, in current versions, a short slug \
                     registered in projects.json. Under SANDBOX=sandbox-exec the whole runtime \
                     dir (tmp/, history/, projects.json) moves to ~/.cache/.gemini while \
                     settings and credentials stay put",
        sources: &[
            "https://github.com/google-gemini/gemini-cli/blob/d5b3e3accb26000d273abf16e0f1dd83aa5428a9/packages/core/src/config/storage.ts",
            "https://github.com/google-gemini/gemini-cli/blob/d5b3e3accb26000d273abf16e0f1dd83aa5428a9/packages/core/src/utils/paths.ts",
            "https://github.com/google-gemini/gemini-cli/blob/d5b3e3accb26000d273abf16e0f1dd83aa5428a9/packages/core/src/config/projectRegistry.ts",
        ],
        note: "#96; crate::agents::gemini_cli implements identification. GEMINI_CLI_HOME is the \
               real override; GEMINI_DIR is a plain '.gemini' constant upstream, not an env \
               var. The per-project directory name is one-way (sha256 in the legacy form, a \
               registry slug in the current one), so tmp/history directories carry an honest \
               Unresolved linkage rather than a guess; projects.json is the upstream mapping \
               and is not read by this adapter",
        verification: &[
            Verification {
                checked: "the downloaded-tools cache is at tmp/bin, not \
                          bin: getGlobalBinDir() = \
                          join(getGlobalTempDir(), BIN_DIR_NAME) and \
                          getGlobalTempDir() = join(getGlobalRuntimeDir(), \
                          TMP_DIR_NAME). This row and the adapter both \
                          said ~/.gemini/bin, a path no version writes",
                source: "packages/core/src/config/storage.ts",
                revision: "google-gemini/gemini-cli main @ \
                           d5b3e3accb26000d273abf16e0f1dd83aa5428a9",
            },
            Verification {
                checked: "under SANDBOX=sandbox-exec getGlobalRuntimeDir() \
                          returns ~/.cache/.gemini, so tmp/ (with \
                          tmp/bin), history/ and projects.json move there \
                          while settings and credentials stay at the home \
                          root. Modelled by crate::locations::gemini_cli \
                          as a second location, proposed only when this \
                          process is itself under that sandbox",
                source: "packages/core/src/config/storage.ts",
                revision: "google-gemini/gemini-cli main @ \
                           d5b3e3accb26000d273abf16e0f1dd83aa5428a9",
            },
            Verification {
                checked: "GEMINI_DIR is a plain '.gemini' constant, not an \
                          env var",
                source: "packages/core/src/utils/paths.ts",
                revision: "google-gemini/gemini-cli main @ \
                           d5b3e3accb26000d273abf16e0f1dd83aa5428a9",
            },
            Verification {
                checked: "the current per-project id is a slug from a \
                          registry at <runtimeDir>/projects.json \
                          (slugify); the legacy form is a sha256 of the \
                          project root, migrated across. Both are one-way \
                          from the directory name alone, so linkage stays \
                          Unresolved rather than guessed",
                source: "packages/core/src/config/projectRegistry.ts",
                revision: "google-gemini/gemini-cli main @ \
                           d5b3e3accb26000d273abf16e0f1dd83aa5428a9",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::Pi,
        display_name: "Pi",
        support: SupportLevel::Supported,
        home_note: "~/.pi/agent/ by default, overridable via PI_CODING_AGENT_DIR (confirmed \
                     primary-source name, shared with Oh My Pi's own override -- see \
                     crate::locations::pi; PI_AGENT_DIR, the issue text's name, is honored as an \
                     unconfirmed secondary override); sessions/ (organized by working directory), \
                     settings.json, trust.json, models.json, auth.json, tools/, bin/, \
                     prompts/, npm/, and the single debug log pi-debug.log (there is no logs/ \
                     directory). Upstream repository: earendil-works/pi",
        sources: &[
            "https://github.com/earendil-works/pi/blob/d201760ffee16564aa8d9a759e0c85b70db33674/packages/coding-agent/docs/environment-variables.md",
            "https://github.com/earendil-works/pi/blob/d201760ffee16564aa8d9a759e0c85b70db33674/packages/coding-agent/src/config.ts",
            "https://github.com/earendil-works/pi/blob/d201760ffee16564aa8d9a759e0c85b70db33674/packages/coding-agent/docs/sessions.md",
        ],
        note: "#96; crate::agents::pi implements identification, distinct from Oh My Pi (a fork \
               of this project) even though the two share an override variable name -- see \
               crate::locations::pi's doc comment for the disclosed collision risk. Session \
               linkage parses only Pi's own documented offset-zero JSON header; a header it \
               cannot parse is an explicit unknown-format outcome. This row previously \
               described a fallback to Oh My Pi's 256-byte title-slot shape; that fallback is \
               removed, because one tool's format change must never silently change another \
               tool's identification. The shared byte-offset mechanics live in the neutral \
               crate::agents::pi_family, which names no tool",
        verification: &[
            Verification {
                checked: "PI_CODING_AGENT_DIR overrides the config \
                          directory, default ~/.pi/agent. This row \
                          previously cited settings.md, a file that does \
                          not contain the string at all -- the claim was \
                          true, the citation did not establish it",
                source: "packages/coding-agent/docs/environment-variables.md",
                revision: "earendil-works/pi main @ \
                           d201760ffee16564aa8d9a759e0c85b70db33674",
            },
            Verification {
                checked: "the layout in upstream's own path helpers: \
                          CONFIG_DIR_NAME = .pi, getAgentDir() = \
                          ~/.pi/agent, getSessionsDir, getToolsDir, \
                          getBinDir, getPromptsDir, settings.json, \
                          models.json, auth.json",
                source: "packages/coding-agent/src/config.ts",
                revision: "earendil-works/pi main @ \
                           d201760ffee16564aa8d9a759e0c85b70db33674",
            },
            Verification {
                checked: "sessions are JSONL files under \
                          ~/.pi/agent/sessions/, organised by working \
                          directory",
                source: "packages/coding-agent/docs/sessions.md",
                revision: "earendil-works/pi main @ \
                           d201760ffee16564aa8d9a759e0c85b70db33674",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::Aider,
        display_name: "Aider",
        support: SupportLevel::Supported,
        home_note: "~/.aider/caches (model_prices_and_context_window.json, versioncheck; both \
                     wholly re-downloadable), plus an optional home-level .aider.conf.yml; \
                     per-repo .aider.chat.history.md / .aider.input.history and \
                     .aider.tags.cache.v{3,4}/ at the git root -- project-local, not under the \
                     home directory. .aider.llm.history is opt-in upstream (--llm-history-file \
                     defaults to None), so it is identified where present and never assumed",
        sources: &[
            "https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/args.py",
            "https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/repomap.py",
            "https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/models.py",
            "https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/main.py",
        ],
        note: "#96; crate::agents::aider implements both halves: identify() for the home-level \
               caches, and the adapter's declared project_local_units capability, called per \
               known project worktree root, for the per-repo files -- this issue's own explicit \
               'attach to the worktree artifact model' acceptance",
        verification: &[
            Verification {
                checked: "the per-repo history filenames \
                          .aider.input.history and .aider.chat.history.md, \
                          both defaulting to the git root. \
                          .aider.llm.history is opt-in (default None) and \
                          appears only inside a help string, so it is not \
                          an always-present file",
                source: "aider/args.py",
                revision: "Aider-AI/aider main @ \
                           5dc9490bb35f9729ef2c95d00a19ccd30c26339c",
            },
            Verification {
                checked: "TAGS_CACHE_DIR = \
                          .aider.tags.cache.v{CACHE_VERSION}, v3 or v4 \
                          depending on the tree-sitter pack, at the repo \
                          root",
                source: "aider/repomap.py",
                revision: "Aider-AI/aider main @ \
                           5dc9490bb35f9729ef2c95d00a19ccd30c26339c",
            },
            Verification {
                checked: "the home-level cache \
                          ~/.aider/caches/model_prices_and_context_window.json, \
                          with a 24-hour TTL -- wholly re-downloadable",
                source: "aider/models.py",
                revision: "Aider-AI/aider main @ \
                           5dc9490bb35f9729ef2c95d00a19ccd30c26339c",
            },
            Verification {
                checked: "the home-level .aider.conf.yml, searched cwd \
                          then git root then home",
                source: "aider/main.py",
                revision: "Aider-AI/aider main @ \
                           5dc9490bb35f9729ef2c95d00a19ccd30c26339c",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::GithubCopilotCli,
        display_name: "GitHub Copilot CLI",
        support: SupportLevel::Supported,
        home_note: "~/.copilot, overridable via COPILOT_HOME (config.json, settings.json, \
                     mcp-config.json, permissions-config.json, providers.json, \
                     copilot-instructions.md, agents/, hooks/, skills/, extensions/, \
                     installed-plugins/, plugin-data/, mcp-oauth-config/, mcp-secrets/, ide/, \
                     session-state/, command-history-state/, session-store.db, logs/); separate \
                     platform-conventional cache (~/Library/Caches/copilot on macOS), \
                     overridable via COPILOT_CACHE_HOME, independent of COPILOT_HOME. The \
                     directory names are documented; what is inside session-state/ is not, so \
                     project linkage is Unresolved and no selective action is offered on it",
        sources: &[
            "https://github.com/github/docs/blob/72e940d15a9aff06b6e84216f3c97dac25c47d9b/content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md",
            "https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference",
        ],
        note: "#97; crate::agents::copilot_cli implements identification. session-state/ and \
               command-history-state/ are the real directory names, correcting this row's own \
               prior history-session-state/ guess",
        verification: &[
            Verification {
                checked: "the ~/.copilot directory listing -- agents/, \
                          config.json, logs/, session-state/, \
                          command-history-state/, session-store.db, \
                          settings.json, mcp-config.json, \
                          permissions-config.json and the rest -- and the \
                          --config-dir > COPILOT_HOME > ~/.copilot \
                          precedence",
                source: "content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md",
                revision: "github/docs main @ \
                           72e940d15a9aff06b6e84216f3c97dac25c47d9b",
            },
            Verification {
                checked: "what it does NOT contain: any field name inside \
                          session state. The page documents directory \
                          names only; re-checked 2026-09-22 across four \
                          pinned pages (this one, \
                          cli-command-reference.md, chronicle.md, \
                          acp-server.md), workspaceFolder and \
                          workingDirectory appear zero times and every cwd \
                          hit is the /cwd slash command, prose, an MCP \
                          launch key or an ACP wire parameter. The CLI \
                          itself is closed source. So the \
                          cwd/workspace/workspaceFolder fields this \
                          adapter parsed were a guess at an undocumented \
                          schema: linkage is now Unresolved and no \
                          selective action is offered on session state",
                source: "content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md",
                revision: "github/docs main @ \
                           72e940d15a9aff06b6e84216f3c97dac25c47d9b",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::Cursor,
        display_name: "Cursor",
        support: SupportLevel::Unverified,
        home_note: "editor-profile storage, macOS only this chunk: ~/Library/Application \
                     Support/Cursor (User/globalStorage/state.vscdb, \
                     User/workspaceStorage/<id>/{state.vscdb,workspace.json}, User/History, \
                     Cache/CachedData/CachedExtensionVSIXs/logs), plus a separate ~/.cursor/ \
                     (chats/, projects/, CLI state, not decomposed)",
        sources: &[
            "https://cursor.com/docs/troubleshooting/troubleshooting-guide",
            "https://github.com/thomas-pedersen/cursor-chat-browser",
            "https://github.com/Callum-Ward/cursaves/blob/main/docs/how-cursor-stores-chats.md",
        ],
        note: "#98; crate::agents::cursor identifies through crate::agents::vscode_family, but \
               no official Cursor documentation names any of these paths, so this row is \
               Unverified: units are identified and measured, no action is offered and project \
               linkage is reported Unresolved. The layout is inherited from VS Code and is very \
               probably right -- 'very probably' is not the bar for moving a developer's chat \
               history. Linux (~/.config/Cursor/...) is deferred to the independent Linux track \
               (#77-#89)",
        verification: &[Verification {
            checked: "searched Cursor's official documentation for the profile layout: the \
                      troubleshooting guide says data is cached locally but names no path, and \
                      contains none of 'Application Support/Cursor', 'globalStorage', \
                      'workspaceStorage' or 'state.vscdb'. Only forum.cursor.com community \
                      threads corroborate it",
            source: "https://cursor.com/docs/troubleshooting/troubleshooting-guide",
            revision: "retrieved 2026-09-21 -- NOT CONFIRMED",
        }],
    },
    MatrixEntry {
        id: AgentToolId::Windsurf,
        display_name: "Windsurf",
        support: SupportLevel::Unverified,
        home_note: "macOS only this chunk: the current VS-Code-fork editor profile at \
                     ~/Library/Application Support/Devin AND the legacy one at .../Windsurf \
                     (both modelled -- an installation mid-migration has bytes in each), plus \
                     ~/.codeium/windsurf (MCP/workflow/skills/bin config, not decomposed), \
                     which upstream states is NOT changing in the rename and stays read-write",
        sources: &[
            "https://docs.devin.ai/desktop/devin-desktop-faq",
            "https://registry.coder.com/modules/coder/windsurf",
        ],
        note: "#98; crate::agents::windsurf identifies through crate::agents::vscode_family. The \
               profile root and User/globalStorage are now primary-source confirmed, but \
               User/workspaceStorage and the Cache/CachedData/CachedExtensionVSIXs/logs \
               siblings this adapter also models are not named by that page, so the row stays \
               Unverified: no action is offered and project linkage is reported Unresolved. An \
               installation whose actual layout differs surfaces as an honest '(unsupported \
               layout version)' residual rather than a silent miscount",
        verification: &[Verification {
            checked: "the per-user IDE data directory table: macOS ~/Library/Application \
                      Support/Windsurf (legacy) and .../Devin (current), listing \
                      User/settings.json, User/keybindings.json, User/snippets/, globalStorage/, \
                      Workspaces/ and argv.json. 'workspaceStorage' does not appear on the page, \
                      and neither do the Cache/logs siblings -- PARTIALLY CONFIRMED. \
                      docs.windsurf.com 307-redirects here",
            source: "https://docs.devin.ai/desktop/devin-desktop-faq",
            revision: "retrieved 2026-09-21",
        }],
    },
    MatrixEntry {
        id: AgentToolId::Cline,
        display_name: "Cline",
        support: SupportLevel::Supported,
        home_note: "VS Code extension global storage, one location per known editor host \
                     (macOS: Code, Code - Insiders, Cursor, Windsurf, plus \
                     ~/.vscode-server/data/... for a remote/devcontainer target): \
                     globalStorage/saoudrizwan.claude-dev/tasks/<task-id>/ holding \
                     api_conversation_history.json, ui_messages.json, task_metadata.json, \
                     context_history.json, checkpoints, plus state/taskHistory.json -- the \
                     task-history array this adapter reads linkage from, one bounded read per \
                     host. Cline 4.x adds a second root -- CLINE_DATA_DIR, else CLINE_DIR/data, \
                     else ~/.cline/data -- which is now modelled as its own location; upstream \
                     states it does NOT hold the VS Code host's task history",
        sources: &[
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/apps/vscode/src/sdk/legacy-state-reader.ts",
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/apps/vscode/src/hosts/vscode/vscode-to-file-migration.ts",
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/apps/vscode/src/shared/HistoryItem.ts",
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/apps/vscode/src/shared/storage/storage-context.ts",
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/sdk/packages/shared/src/storage/paths.ts",
            "https://github.com/cline/cline/blob/254f40c4b592d1e662b84f2ba06fe45dca77cab3/.clinerules/storage.md",
        ],
        note: "#99; crate::agents::cline identifies through crate::agents::vscode_family, \
               decomposing every resolved host location rather than just the first (the \
               adapter's own decomposes_every_location capability). This row previously claimed \
               project linkage from a task_metadata.json 'workspace' field; that field does not \
               exist in Cline's schema and the claim is withdrawn -- tasks now carry an \
               Unresolved link whose reason names the store that does hold the answer",
        verification: &[
            Verification {
                checked: "the per-task files under tasks/<taskId>/: \
                          api_conversation_history.json, ui_messages.json, \
                          context_history.json, task_metadata.json -- and \
                          the task-history store at \
                          <dataDir>/state/taskHistory.json",
                source: "apps/vscode/src/sdk/legacy-state-reader.ts",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
            Verification {
                checked: "taskHistory is NOT migrated to the shared store: \
                          it uses its own file-based storage at \
                          {globalStorageFsPath}/state/taskHistory.json, \
                          and for VS Code globalStorageFsPath is the \
                          VS-Code-managed path, NOT ~/.cline/data. So the \
                          store this adapter needs is inside the directory \
                          it already walks, and the previous \
                          it-lives-in-state.vscdb reason was wrong",
                source: "apps/vscode/src/hosts/vscode/vscode-to-file-migration.ts",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
            Verification {
                checked: "the working-directory field is \
                          HistoryItem.cwdOnTaskInitialization, and it is \
                          optional -- not cwd, workspace or \
                          workspaceFolder, none of which exist in the type",
                source: "apps/vscode/src/shared/HistoryItem.ts",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
            Verification {
                checked: "the second root: CLINE_DATA_DIR, else CLINE_DIR \
                          + /data, else ~/.cline/data",
                source: "apps/vscode/src/shared/storage/storage-context.ts",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
            Verification {
                checked: "the same resolution order in the SDK \
                          (resolveClineDir / resolveClineDataDir), so the \
                          two stores cannot drift",
                source: "sdk/packages/shared/src/storage/paths.ts",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
            Verification {
                checked: "upstream's own spelling is ambiguous and this \
                          row records it rather than picking one: the File \
                          Layout here says \
                          ~/.cline/data/tasks/taskHistory.json, \
                          vscode-to-file-migration.ts's comment says \
                          {globalStorage}/state/taskHistory.json and its \
                          skip list says tasks/taskHistory.json. The \
                          adapter follows the executable code (state/) and \
                          says so in the unresolved reason",
                source: ".clinerules/storage.md",
                revision: "cline/cline main @ \
                           254f40c4b592d1e662b84f2ba06fe45dca77cab3",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::RooCode,
        display_name: "Roo Code",
        support: SupportLevel::Supported,
        home_note: "VS Code extension global storage, same per-host modeling as Cline: \
                     globalStorage/rooveterinaryinc.roo-cline/tasks/<task-id>/ \
                     (api_conversation_history.json, ui_messages.json, task_metadata.json, \
                     history_item.json, plus tasks/_index.json); remote/server hosting \
                     ~/.vscode-server/data/User/globalStorage/... . The roo-cline. \
                     customStoragePath setting can relocate tasks/ entirely, in which case this \
                     catalog simply does not find it",
        sources: &[
            "https://github.com/RooCodeInc/Roo-Code/blob/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16/src/utils/storage.ts",
            "https://github.com/RooCodeInc/Roo-Code/blob/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16/src/shared/globalFileNames.ts",
            "https://github.com/RooCodeInc/Roo-Code/blob/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16/packages/types/src/history.ts",
            "https://github.com/RooCodeInc/Roo-Code/blob/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16/src/core/task-persistence/TaskHistoryStore.ts",
        ],
        note: "#99; crate::agents::roo_code identifies through crate::agents::vscode_family. \
               Per-task project linkage is now read from the confirmed \
               tasks/<id>/history_item.json 'workspace' field, correcting this row's own prior \
               task_metadata.json claim. A task directory has been reported to embed a full Git \
               checkpoint repo, so its folded byte total is not necessarily small",
        verification: &[
            Verification {
                checked: "getTaskDirectoryPath places tasks at \
                          <globalStorage>/tasks/<taskId>, through \
                          getStorageBasePath -- which honours a \
                          customStoragePath setting, so a relocated store \
                          is simply not found by this catalog",
                source: "src/utils/storage.ts",
                revision: "RooCodeInc/Roo-Code main @ \
                           b867ec9145750d0ae1ff7f02d35406e9bf2a0b16",
            },
            Verification {
                checked: "the per-task filenames \
                          api_conversation_history.json, ui_messages.json, \
                          task_metadata.json, history_item.json and the \
                          tasks/_index.json index",
                source: "src/shared/globalFileNames.ts",
                revision: "RooCodeInc/Roo-Code main @ \
                           b867ec9145750d0ae1ff7f02d35406e9bf2a0b16",
            },
            Verification {
                checked: "HistoryItem carries an optional workspace field \
                          -- which is what this adapter reads, from \
                          history_item.json, not from task_metadata.json",
                source: "packages/types/src/history.ts",
                revision: "RooCodeInc/Roo-Code main @ \
                           b867ec9145750d0ae1ff7f02d35406e9bf2a0b16",
            },
            Verification {
                checked: "upstream keys its own linkage off the same field \
                          (getByWorkspace), written into history_item.json",
                source: "src/core/task-persistence/TaskHistoryStore.ts",
                revision: "RooCodeInc/Roo-Code main @ \
                           b867ec9145750d0ae1ff7f02d35406e9bf2a0b16",
            },
        ],
    },
    MatrixEntry {
        id: AgentToolId::Continue,
        display_name: "Continue",
        support: SupportLevel::Supported,
        home_note: "~/.continue, or CONTINUE_GLOBAL_DIR when set (config.yaml/config.json, \
                     sessions/<session-id>.json plus a sessions/sessions.json index, index/ \
                     embeddings/tag caches, dev_data/ anonymized usage events with \
                     devdata.sqlite)",
        sources: &[
            "https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/index.d.ts",
            "https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/util/history.ts",
            "https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/util/paths.ts",
        ],
        note: "#99; crate::agents::continue_dev implements identification. This row previously \
               said no confirmed per-session workspace-linkage field was found; one exists and \
               is now read -- Session.workspaceDirectory, written into each session file and \
               mirrored into the sessions index",
        verification: &[
            Verification {
                checked: "Session.workspaceDirectory is a REQUIRED string \
                          field (no ?), and BaseSessionMetadata carries it \
                          too",
                source: "core/index.d.ts",
                revision: "continuedev/continue main @ \
                           5522c6f44ca0ac3528b37244818fbfa39b5af470",
            },
            Verification {
                checked: "upstream itself writes workspaceDirectory: \"\" \
                          from the catch of load(sessionId), so an empty \
                          string is an expected on-disk value. It resolves \
                          to Unresolved, never Missing -- Missing would \
                          claim a path was named and has since disappeared",
                source: "core/util/history.ts",
                revision: "continuedev/continue main @ \
                           5522c6f44ca0ac3528b37244818fbfa39b5af470",
            },
            Verification {
                checked: "the layout and the override: ~/.continue or \
                          CONTINUE_GLOBAL_DIR, sessions/<id>.json and the \
                          sessions/sessions.json index",
                source: "core/util/paths.ts",
                revision: "continuedev/continue main @ \
                           5522c6f44ca0ac3528b37244818fbfa39b5af470",
            },
        ],
    },
];

pub fn entry(id: AgentToolId) -> &'static MatrixEntry {
    MATRIX
        .iter()
        .find(|e| e.id == id)
        .expect("every AgentToolId has a MATRIX row")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_tool_is_present_exactly_once() {
        let ids = [
            AgentToolId::ClaudeCode,
            AgentToolId::Codex,
            AgentToolId::CodexDesktop,
            AgentToolId::OhMyPi,
            AgentToolId::OpenCode,
            AgentToolId::GeminiCli,
            AgentToolId::Pi,
            AgentToolId::Aider,
            AgentToolId::GithubCopilotCli,
            AgentToolId::Cursor,
            AgentToolId::Windsurf,
            AgentToolId::Cline,
            AgentToolId::RooCode,
            AgentToolId::Continue,
        ];
        assert_eq!(
            MATRIX.len(),
            ids.len(),
            "matrix must list every named tool, no more no less"
        );
        for id in ids {
            let matches = MATRIX.iter().filter(|e| e.id == id).count();
            assert_eq!(matches, 1, "{id:?} must appear exactly once");
        }
    }

    /// The support level of every tool, stated one row at a time.
    ///
    /// This replaced a loop asserting `Supported` for everything, which
    /// could not fail and therefore said nothing -- and which was wrong
    /// for two rows. A level change now has to be made here, in a diff,
    /// beside the `Verification` that justifies it.
    const EXPECTED_LEVELS: &[(AgentToolId, SupportLevel)] = &[
        (AgentToolId::ClaudeCode, SupportLevel::Supported),
        (AgentToolId::Codex, SupportLevel::Supported),
        (AgentToolId::CodexDesktop, SupportLevel::Supported),
        (AgentToolId::OhMyPi, SupportLevel::Supported),
        (AgentToolId::OpenCode, SupportLevel::Supported),
        (AgentToolId::GeminiCli, SupportLevel::Supported),
        (AgentToolId::Pi, SupportLevel::Supported),
        (AgentToolId::Aider, SupportLevel::Supported),
        (AgentToolId::GithubCopilotCli, SupportLevel::Supported),
        // No official Cursor documentation names the profile layout.
        (AgentToolId::Cursor, SupportLevel::Unverified),
        // The profile root and globalStorage are confirmed; the
        // workspaceStorage and cache/log siblings this adapter also
        // models are not.
        (AgentToolId::Windsurf, SupportLevel::Unverified),
        (AgentToolId::Cline, SupportLevel::Supported),
        (AgentToolId::RooCode, SupportLevel::Supported),
        (AgentToolId::Continue, SupportLevel::Supported),
    ];

    #[test]
    fn every_tool_has_the_support_level_this_test_names() {
        assert_eq!(MATRIX.len(), EXPECTED_LEVELS.len());
        for (id, level) in EXPECTED_LEVELS {
            assert_eq!(
                entry(*id).support,
                *level,
                "{id:?}'s support level changed; if that is deliberate, change it here too and \
                 say why in its Verification"
            );
        }
        assert!(
            MATRIX.iter().any(|e| e.support == SupportLevel::Unverified),
            "the Unverified level must actually be in use, or this table is back to claiming \
             everything is supported"
        );
    }

    #[test]
    fn a_supported_row_cites_what_confirmed_it() {
        for e in MATRIX {
            if e.support != SupportLevel::Supported {
                continue;
            }
            assert!(
                !e.verification.is_empty(),
                "{:?} is Supported with nothing recorded as having confirmed it",
                e.id
            );
            for v in e.verification {
                assert!(!v.checked.trim().is_empty(), "{:?}", e.id);
                assert!(!v.source.trim().is_empty(), "{:?}", e.id);
                assert!(
                    !v.revision.trim().is_empty(),
                    "{:?} cites {} with no commit or retrieval date",
                    e.id,
                    v.source
                );
            }
        }
    }

    #[test]
    fn an_unverified_row_records_what_was_attempted() {
        for e in MATRIX {
            if e.support != SupportLevel::Unverified {
                continue;
            }
            assert!(
                !e.verification.is_empty(),
                "{:?} is Unverified without saying what was checked and did not confirm",
                e.id
            );
            assert!(
                !e.support.actions_available(),
                "{:?} is Unverified yet actions would be offered",
                e.id
            );
        }
    }

    #[test]
    fn every_row_has_at_least_one_source_and_a_nonempty_home_note() {
        for e in MATRIX {
            assert!(!e.sources.is_empty(), "{:?} has no recorded source", e.id);
            assert!(
                !e.home_note.trim().is_empty(),
                "{:?} has no home_note",
                e.id
            );
            for s in e.sources {
                assert!(
                    s.starts_with("https://"),
                    "{:?} source {s:?} is not a URL",
                    e.id
                );
            }
        }
    }
}
