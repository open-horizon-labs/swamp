#!/usr/bin/env bash
# The release gate: succeeds only when BOTH `ci.yml` (the fast tier plus the
# platform-isolation, traversal and archive checks) and `check-full.yml`
# (the full tier: compile-fail cases, mutation sweep, cost test) have a
# successful run for exactly <sha>, so a release is never published from a
# commit either has not passed. Both run on every push to main, so a tag on
# a merged commit normally finds finished or in-progress runs and only
# waits; the full tier's ~27 minutes are not spent a second time.
#
# Per workflow:
#   * a successful run for <sha>                 -> that gate is met
#   * every active run already has a failed job  -> fail now (it cannot pass;
#                                                   do not wait for the rest)
#   * a queued or running run with no failure    -> wait for it
#   * a run that failed or timed out             -> fail (never re-run over a
#                                                   real failure)
#   * no run, or only cancelled runs             -> dispatch one on <ref>, wait
#
# usage: verify-gates.sh <sha> <ref> [repo]
#   GH                 the gh binary (default: gh)
#   VERIFY_WORKFLOWS   space-separated workflow files (default: ci.yml check-full.yml)
#   VERIFY_TIMEOUT     seconds to wait in total (default 5400)
#   VERIFY_POLL        seconds between polls (default 30)
set -euo pipefail

sha=${1:?usage: verify-gates.sh <sha> <ref> [repo]}
ref=${2:?usage: verify-gates.sh <sha> <ref> [repo]}
repo=${3:-${GITHUB_REPOSITORY:-}}
gh=${GH:-gh}
workflows=${VERIFY_WORKFLOWS:-ci.yml check-full.yml}
timeout=${VERIFY_TIMEOUT:-5400}
poll=${VERIFY_POLL:-30}
repo_args=()
[ -n "$repo" ] && repo_args=(--repo "$repo")

runs() {
  "$gh" run list "${repo_args[@]}" --workflow "$1" --commit "$sha" \
    --limit 50 --json databaseId,status,conclusion,url \
    --jq '.[] | [.databaseId, .status, (.conclusion // "-"), .url] | @tsv'
}

failed_jobs() {
  "$gh" run view "$1" "${repo_args[@]}" --json jobs \
    --jq '.jobs[] | select(.conclusion == "failure" or .conclusion == "timed_out") | .name'
}

start=$(date +%s)
dispatched=" "
while :; do
  all_met=1
  for wf in $workflows; do
    listing=$(runs "$wf")
    success=$(printf '%s\n' "$listing" | awk -F'\t' '$3=="success"{print $4; exit}')
    if [ -n "$success" ]; then
      echo "$wf passed for $sha: $success"
      continue
    fi
    all_met=0
    active=$(printf '%s\n' "$listing" | awk -F'\t' '$2!="completed" && $2!=""{print $1"\t"$4}')
    if [ -z "$active" ]; then
      failed=$(printf '%s\n' "$listing" | awk -F'\t' '$2=="completed" && $3!="cancelled" && $3!="-"{print $4; exit}')
      if [ -n "$failed" ]; then
        echo "$wf did not pass for $sha: $failed" >&2
        exit 1
      fi
      case "$dispatched" in
        *" $wf "*) ;;
        *)
          echo "no usable $wf run for $sha; dispatching one on $ref"
          "$gh" workflow run "$wf" "${repo_args[@]}" --ref "$ref"
          dispatched="$dispatched$wf "
          ;;
      esac
      continue
    fi
    # Fail now only when every active run already has a failed job: one
    # that has not could still be the run that passes.
    doomed=1
    while IFS=$'\t' read -r id url; do
      [ -z "$id" ] && continue
      names=$(failed_jobs "$id")
      if [ -z "$names" ]; then
        doomed=0
        echo "waiting for $wf on $sha: $url"
      else
        echo "$wf: job failed while the run is still going: $(printf '%s' "$names" | tr '\n' ',') ($url)"
      fi
    done <<<"$active"
    if [ "$doomed" -eq 1 ]; then
      echo "$wf cannot pass for $sha: every active run has a failed job (still going)" >&2
      exit 1
    fi
  done
  [ "$all_met" -eq 1 ] && exit 0
  if [ $(($(date +%s) - start)) -ge "$timeout" ]; then
    echo "timed out after ${timeout}s waiting for $workflows on $sha" >&2
    exit 1
  fi
  sleep "$poll"
done
