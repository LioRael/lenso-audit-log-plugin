#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/release-set.sh"
fail() { echo "::error title=Release gate::$*" >&2; exit 1; }

for variable in GITHUB_TOKEN GITHUB_REPOSITORY GITHUB_REF GITHUB_EVENT_NAME RELEASE_SHA RELEASE_SET RELEASE_MODE CANDIDATE_RUN_ID CANDIDATE_ATTEMPT; do
  [[ -n "${!variable:-}" ]] || fail "missing required environment variable: $variable"
done
policy="$SCRIPT_DIR/release-policy.json"
[[ "$GITHUB_REPOSITORY" == "$(jq -r .repository "$policy")" ]] || fail "unexpected repository"
[[ "$GITHUB_REF" == refs/heads/main && "$GITHUB_EVENT_NAME" == workflow_dispatch ]] || fail "release requires a manual main workflow"
case "$RELEASE_MODE" in dry-run|publish) ;; *) fail "unsupported release mode" ;; esac
if [[ "$RELEASE_MODE" == publish ]]; then
  [[ "${RELEASE_CONFIRMATION:-}" == publish ]] || fail "publish mode requires confirmation text publish"
fi
source_sha="${RELEASE_SHA,,}"
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || fail "source_sha must be a full 40-character hexadecimal commit SHA"
[[ "$CANDIDATE_RUN_ID" =~ ^[1-9][0-9]*$ && "$CANDIDATE_ATTEMPT" =~ ^[1-9][0-9]*$ ]] || fail "candidate run and attempt must be positive integers"
release_set="$(release_set_canonical "$RELEASE_SET")" || fail "invalid release_set"
allowed="$(release_set_canonical "$(jq -c .packages "$policy")")"
jq -e --argjson allowed "$allowed" 'all(.[]; . as $p | any($allowed[]; . == $p))' <<<"$release_set" >/dev/null || fail "release_set is outside the exact package/version allowlist"
[[ "$RELEASE_MODE" != publish || "$release_set" != '[]' ]] || fail "publish requires a non-empty release_set"

git fetch origin main --no-tags >/dev/null
main_sha="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq '.object.sha')" || fail "remote main readback failed"
[[ "$main_sha" == "$source_sha" && "$(git rev-parse HEAD)" == "$source_sha" && "$(git rev-parse refs/remotes/origin/main)" == "$source_sha" ]] || fail "source_sha must be the checked-out current remote main"
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || fail "release source has tracked modifications"

metadata="$(cargo metadata --locked --no-deps --format-version 1)" || fail "cargo metadata failed"
pending='[]'
while IFS=$'\t' read -r package version; do
  jq -e --arg package "$package" --arg version "$version" '[.packages[] | select(.name == $package and .version == $version and .publish != [])] | length == 1' <<<"$metadata" >/dev/null || fail "source does not contain the allowed public package/version: ${package}@${version}"
  status="$(curl --silent --show-error --location --retry 2 --max-time 20 --output /dev/null --write-out '%{http_code}' "https://crates.io/api/v1/crates/${package}/${version}")" || fail "registry read failed"
  case "$status" in
    200) ;;
    404)
      pending="$(jq -c --arg package "$package" --arg version "$version" '. + [{package_name:$package,version:$version}]' <<<"$pending")"
      if [[ "$RELEASE_MODE" == publish ]]; then
        name_status="$(curl --silent --show-error --location --retry 2 --max-time 20 --output /dev/null --write-out '%{http_code}' "https://crates.io/api/v1/crates/${package}")" || fail "crate namespace read failed"
        [[ "$name_status" == 200 ]] || fail "${package} requires separately authorized first-name bootstrap; this workflow cannot allocate it"
      fi
      ;;
    *) fail "unexpected registry response ${status} for ${package}@${version}" ;;
  esac
done < <(jq -r '.[] | [.package_name,.version] | @tsv' <<<"$allowed")
pending="$(release_set_canonical "$pending")"
[[ "$release_set" == "$pending" ]] || fail "release_set does not match the registry-derived pending subset: ${pending}"

workflow_id="$(gh api "repos/${GITHUB_REPOSITORY}/actions/workflows/ci.yml" --jq '.id')" || fail "CI workflow read failed"
[[ "$workflow_id" =~ ^[1-9][0-9]*$ ]] || fail "invalid CI workflow identity"
run="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs/${CANDIDATE_RUN_ID}")" || fail "candidate run read failed"
jq -e --arg sha "$source_sha" --argjson workflow "$workflow_id" --argjson attempt "$CANDIDATE_ATTEMPT" '
  .workflow_id == $workflow and .path == ".github/workflows/ci.yml" and .name == "CI"
  and .event == "push" and (.head_branch | startswith("candidate/"))
  and .head_sha == $sha and .run_attempt == $attempt and .status == "completed" and .conclusion == "success"
' <<<"$run" >/dev/null || fail "candidate run does not prove this exact SHA and attempt"
jobs="$(gh api --paginate --slurp "repos/${GITHUB_REPOSITORY}/actions/runs/${CANDIDATE_RUN_ID}/attempts/${CANDIDATE_ATTEMPT}/jobs?per_page=100")" || fail "candidate jobs read failed"
while IFS= read -r job; do
  jq -e --arg name "$job" --arg sha "$source_sha" --argjson attempt "$CANDIDATE_ATTEMPT" '
    [.[]?.jobs[]? | select(.name == $name and .head_sha == $sha and .run_attempt == $attempt)]
    | length == 1 and .[0].status == "completed" and .[0].conclusion == "success"
  ' <<<"$jobs" >/dev/null || fail "candidate attempt needs exactly one successful ${job} job"
done < <(jq -r '.required_jobs[]' "$policy")
printf 'Release gate passed: source %s, run %s attempt %s, pending %s\n' "$source_sha" "$CANDIDATE_RUN_ID" "$CANDIDATE_ATTEMPT" "$pending"
