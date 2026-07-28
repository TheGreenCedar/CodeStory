#!/usr/bin/env bash
# Collect the Actions job evidence that .github/scripts/lost-runner-recovery.mjs classifies.
#
# Both halves of the lost-runner contract -- the bounded automatic rerun and the withheld-claim
# fallback -- read the same three facts about a failed job, so they read them through one collector
# rather than two copies of the same jq. Successful jobs are recorded without annotation or log
# lookups because the classifier only ever inspects failures.
#
# Every lookup here fails closed. "This job had no annotations" and "this token may not read
# annotations" are different facts about the world that an earlier version of this script both
# reported as `[]`; the second one is a repository misconfiguration and has to stop the run rather
# than quietly re-describe a lost runner as an ordinary assertion failure. The same goes for the
# log blob: only a 404 means "the runner never uploaded one", and every other outcome -- 403, a
# rate limit, a transport error -- is an error, never the permissive answer.
#
# The annotations endpoint needs the `checks: read` token scope. Any job that runs this script must
# declare it; .github/scripts/check-workflow-policy.mjs refuses a workflow that does not.
#
# usage: collect-actions-job-evidence.sh <run_id> <run_attempt> <output.json>
set -euo pipefail

run_id="$1"
run_attempt="$2"
output="$3"
mkdir -p "$(dirname "$output")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Ask one Actions endpoint for its HTTP status without letting a transport failure impersonate an
# answer. Prints the final status code of the redirect chain, or nothing at all when gh never got
# a response -- callers treat "nothing" as an error, never as a verdict.
http_status() {
  local target="$1" response
  response="$(gh api --include --silent "$target" 2>/dev/null || true)"
  printf '%s\n' "$response" |
    awk 'toupper($1) ~ /^HTTP\// { code = $2 } END { if (code != "") print code }'
}

# Every attempt, not just the current one. The recovery bound counts how many times *this job* was
# lost to its runner, which is a different number from how many times the run was re-run: a release
# re-run for an unrelated reason must not consume a host's one automatic retry before it is owed.
: > "$work/jobs.ndjson"
attempt=1
while [ "$attempt" -le "$run_attempt" ]; do
  gh api --paginate \
    "repos/$GITHUB_REPOSITORY/actions/runs/$run_id/attempts/$attempt/jobs?per_page=100" \
    --jq '.jobs[]' >> "$work/jobs.ndjson"
  attempt=$((attempt + 1))
done
jq -s '.' "$work/jobs.ndjson" > "$work/jobs.json"

# One row per execution: a job Actions carried forward unchanged is listed under the same id by
# every later attempt, and counting it twice would spend a recovery that never happened.
jq -c '[.[] | {id, name, status, conclusion, run_attempt, steps}]
       | group_by(.id) | map(max_by(.run_attempt)) | sort_by(.id) | .[]' \
  "$work/jobs.json" > "$work/selected.json"

: > "$work/rows.json"
# Read from a file rather than a pipe so the loop runs in this shell: a `exit 1` below has to stop
# the collector, not just a subshell that the pipeline would then report as success.
while IFS= read -r job; do
  job_id="$(jq -r '.id' <<<"$job")"
  if [ "$(jq -r '.conclusion' <<<"$job")" = failure ]; then
    if ! annotations="$(gh api "repos/$GITHUB_REPOSITORY/check-runs/$job_id/annotations" 2>"$work/annotations.err")"; then
      echo "::error::Cannot read annotations for job $job_id: $(tr -d '\n' < "$work/annotations.err")." >&2
      echo "::error::The lost-runner signature is unreadable without the checks: read token scope." >&2
      exit 1
    fi
    if ! jq -e 'type == "array"' >/dev/null 2>&1 <<<"$annotations"; then
      echo "::error::Annotations for job $job_id are not a JSON array." >&2
      exit 1
    fi
    # A runner that lost communication never uploaded its log blob, so this endpoint 404s. That is
    # one of the three parts of the signature and is not inferable from the job record alone.
    log_status="$(http_status "repos/$GITHUB_REPOSITORY/actions/jobs/$job_id/logs")"
    case "$log_status" in
      2??) log_uploaded=true ;;
      404|410) log_uploaded=false ;;
      *)
        echo "::error::Log blob probe for job $job_id answered ${log_status:-no HTTP status}." >&2
        echo "::error::Only 404 means the runner uploaded no log; every other answer is an error." >&2
        exit 1
        ;;
    esac
    # Recorded state, so a reader of the evidence can see which answer produced the verdict rather
    # than having to assume one.
    probe="$(jq -nc \
      --arg log_http_status "$log_status" \
      '{annotations_read: true, log_http_status: ($log_http_status | tonumber)}')"
  else
    annotations='[]'
    log_uploaded=true
    probe='{"annotations_read":false,"log_http_status":null,"skipped":"conclusion_not_failure"}'
  fi
  jq -c \
    --argjson annotations "$annotations" \
    --argjson log_uploaded "$log_uploaded" \
    --argjson probe "$probe" \
    '. + {annotations: $annotations, log_uploaded: $log_uploaded, evidence_probe: $probe}' <<<"$job" \
    >> "$work/rows.json"
done < "$work/selected.json"

jq -s '.' "$work/rows.json" > "$output"
