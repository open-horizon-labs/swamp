# Release checklist

Ordered so that nothing is published which the install path cannot yet
consume. The steps that matter are the ones where a release and a
package manager have to change together.

## Before tagging

1. `scripts/check.sh` passes: fmt, the workspace test suite, strict
   clippy, every source audit, the named runtime guards, and the grep
   layers.
2. `CHANGELOG.md` has an entry under the version being released, and it
   describes behaviour rather than commits.
3. `docs/usage.md` installation commands contain **no hardcoded
   version**. The install instructions follow "latest"; a pinned version
   in the docs goes stale the moment the next release lands.
4. `docs/agent-storage.md`'s support matrix matches
   `agents::matrix` and the registry, and every `Unverified` tool shows
   no actions. `crates/core/tests/agent_matrix_matches_docs.rs` is the
   executable form of this; read its output rather than the table.

## The Homebrew tap cutover (blocks the MCP-removal release)

**This is a coordination step, not a follow-up.** The remote
`Formula/swamp.rb` in the tap installs two binaries, `swamp` and
`swamp-mcp`. The release archives no longer contain `swamp-mcp`, and
they now also carry `skills/swamp/`.

A release published before the formula is updated points Homebrew at an
archive its own `install` block cannot consume: the install fails on the
missing `swamp-mcp`, and the skill directory is never installed even
when it succeeds.

So, in this order:

1. Update the tap formula to install `swamp` **and** `skills/swamp/`,
   and to stop installing `swamp-mcp`.
2. Verify the updated formula against the *previous* release's archive
   if it still contains `swamp-mcp` — Homebrew must tolerate both
   shapes for exactly one release, or the two changes cannot be
   sequenced at all.
3. Publish the new release.
4. `brew update && brew upgrade swamp` on a clean machine, then confirm
   `swamp --version`, `swamp scope --json`, and that the skill is
   present where the SKILL.md install instructions say it is.

Do not edit the tap as part of a code change. It is a separate
repository with its own review, and a code change that silently
depends on an unreviewed tap edit is how the two get out of step.

## After publishing

5. Download the published archive and verify its checksum against the
   release's own recorded value — not the locally built one. A locally
   overwritten old-version executable is not evidence that the
   published release works.
6. Update the issues the release closes with the actual evidence: the
   release URL, the checksum, and the command whose output you read.
