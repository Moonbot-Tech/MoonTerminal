#!/usr/bin/env bash
# Re-run a failed macOS probe only when its job data proves that no step ran.

set -euo pipefail

run_id="${1:?workflow run id is required}"
run_attempt="${2:?workflow run attempt is required}"
repository="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
probe_name='macOS probe (arm64 build, x86_64 check) — diagnostic'
acquisition_message='The job was not acquired by Runner of type hosted even after multiple attempts'

if [[ ! "$run_id" =~ ^[0-9]+$ ]] || [[ ! "$run_attempt" =~ ^[0-9]+$ ]]; then
    echo "[FAIL] workflow run id and attempt must be integers" >&2
    exit 1
fi

# Attempt 1 is the original run. Attempts 2 and 3 are the two permitted automatic retries.
if (( run_attempt >= 3 )); then
    echo "[OK] run $run_id attempt $run_attempt already used both retries"
    exit 0
fi

# A re-run of an old listener still carries its original event payload. Never let that stale event
# classify an earlier attempt and then re-run the run's current, possibly healthy, jobs.
run_json="$(gh api "repos/$repository/actions/runs/$run_id")"
current_status="$(jq -r '.status // empty' <<<"$run_json")"
current_attempt="$(jq -r '.run_attempt // 0' <<<"$run_json")"
if [[ "$current_status" != "completed" ]] || [[ "$current_attempt" -ne "$run_attempt" ]]; then
    echo "[OK] run $run_id is now $current_status at attempt $current_attempt; stale listener, no retry"
    exit 0
fi

jobs_json="$(gh api "repos/$repository/actions/runs/$run_id/attempts/$run_attempt/jobs?per_page=100")"
probe_json="$(jq -c --arg name "$probe_name" '[.jobs[] | select(.name == $name)]' <<<"$jobs_json")"
if [[ "$(jq 'length' <<<"$probe_json")" -ne 1 ]]; then
    echo "[FAIL] expected exactly one macOS probe job in run $run_id attempt $run_attempt" >&2
    exit 1
fi

conclusion="$(jq -r '.[0].conclusion // empty' <<<"$probe_json")"
if [[ "$conclusion" != "cancelled" ]] && [[ "$conclusion" != "failure" ]]; then
    echo "[OK] probe conclusion is $conclusion; no retry"
    exit 0
fi

# Any recorded step, including Set up job, proves a runner was acquired. In particular, a real
# compile failure always stops here and is never re-run.
step_count="$(jq '.[0].steps // [] | length' <<<"$probe_json")"
if (( step_count > 0 )); then
    echo "[OK] probe ran $step_count steps; no runner-acquisition retry"
    exit 0
fi

job_id="$(jq -r '.[0].id' <<<"$probe_json")"
annotations_json="$(gh api "repos/$repository/check-runs/$job_id/annotations?per_page=100")"
# Empty steps alone also cover a probe superseded by a newer push. Any different failure annotation
# vetoes the retry so the obsolete run cannot cancel its higher-priority replacement.
if [[ "$(jq --arg message "$acquisition_message" 'any(.[]; .annotation_level == "failure" and .message != $message)' <<<"$annotations_json")" == "true" ]]; then
    echo "[OK] probe has an unrelated failure annotation; no runner-acquisition retry"
    exit 0
fi

if [[ "$(jq --arg message "$acquisition_message" 'any(.[]; .message == $message)' <<<"$annotations_json")" == "true" ]]; then
    echo "[OK] probe check-run annotations report runner acquisition failure; re-running failed jobs"
elif [[ "$(jq -r '.[0] | has("steps")' <<<"$probe_json")" == "true" ]]; then
    echo "[OK] probe jobs[].steps is empty and no other failure is annotated; re-running failed jobs"
else
    # Older or incomplete job payloads may omit steps. Those require the explicit acquisition
    # annotation; an empty annotations list is not enough to authorize a retry.
    echo "[OK] probe has no runner-acquisition annotation; no retry"
    exit 0
fi

gh run rerun "$run_id" --failed
