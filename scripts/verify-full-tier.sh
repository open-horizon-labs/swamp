#!/usr/bin/env bash
# The release gate for the full tier (`scripts/check-full.sh`, run by
# `check-full.yml`): succeeds only when a `check-full.yml` run for exactly
# <sha> succeeded, so a release is never published from a commit the full
# tier has not passed. `check-full.yml` runs on every push to main, so a
# tag on a merged commit normally finds a finished or in-progress run and
# only waits for it; the tier's ~27 minutes are not spent a second time.
#
#   * a successful run for <sha>           -> pass
#   * a run queued or in progress          -> wait for it
#   * runs that failed or timed out        -> fail (never re-run over a real failure)
#   * no run, or only cancelled runs       -> dispatch one on <ref> and wait for it
#
# usage: verify-full-tier.sh <sha> <ref> [repo]
#   GH                 the gh binary (default: gh)
#   VERIFY_TIMEOUT     seconds to wait in total (default 5400)
#   VERIFY_POLL        seconds between polls (default 30)
set -euo pipefail

sha=${1:?usage: verify-full-tier.sh <sha> <ref> [repo]}
ref=${2:?usage: verify-full-tier.sh <sha> <ref> [repo]}
repo=${3:-${GITHUB_REPOSITORY:-}}
gh=${GH:-gh}
timeout=${VERIFY_TIMEOUT:-5400}
poll=${VERIFY_POLL:-30}
repo_args=()
[ -n "$repo" ] && repo_args=(--repo "$repo")

runs() {
  "$gh" run list "${repo_args[@]}" --workflow check-full.yml --commit "$sha" \
    --limit 50 --json databaseId,status,conclusion,url \
    --jq '.[] | [.databaseId, .status, (.conclusion // "-"), .url] | @tsv'
}

start=$(date +%s)
dispatched=0
while :; do
  listing=$(runs)
  success=$(printf '%s\n' "$listing" | awk -F'\t' '$3=="success"{print $4; exit}')
  if [ -n "$success" ]; then
    echo "full tier passed for $sha: $success"
    exit 0
  fi
  active=$(printf '%s\n' "$listing" | awk -F'\t' '$2!="completed" && $2!=""{print $4; exit}')
  if [ -z "$active" ]; then
    failed=$(printf '%s\n' "$listing" | awk -F'\t' '$2=="completed" && $3!="cancelled" && $3!="-"{print $4; exit}')
    if [ -n "$failed" ]; then
      echo "full tier did not pass for $sha: $failed" >&2
      exit 1
    fi
    if [ "$dispatched" -eq 0 ]; then
      echo "no usable full-tier run for $sha; dispatching one on $ref"
      "$gh" workflow run check-full.yml "${repo_args[@]}" --ref "$ref"
      dispatched=1
    fi
  else
    echo "waiting for the full tier on $sha: $active"
  fi
  if [ $(($(date +%s) - start)) -ge "$timeout" ]; then
    echo "timed out after ${timeout}s waiting for the full tier on $sha" >&2
    exit 1
  fi
  sleep "$poll"
done
