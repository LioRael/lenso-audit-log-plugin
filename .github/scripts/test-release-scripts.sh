#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/release-set.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/bin"
sha=1111111111111111111111111111111111111111
allowed="$(jq -c .packages "$SCRIPT_DIR/release-policy.json")"
python3 - "$SCRIPT_DIR/release-policy.json" "$scratch/metadata.json" <<'PY'
import json,sys
policy=json.load(open(sys.argv[1]))
deps={'lenso-audit-log-core':['lenso-capability-audit-log'],'lenso-audit-log-postgres-plugin':['lenso-capability-audit-log','lenso-audit-log-core'],'lenso-audit-log-d1-plugin':['lenso-capability-audit-log','lenso-audit-log-core']}
json.dump({'packages':[{'name':p['package_name'],'version':p['version'],'publish':None,'dependencies':[{'name':name,'kind':None} for name in deps.get(p['package_name'],[])]} for p in policy['packages']]},open(sys.argv[2],'w'))
PY
cat >"$scratch/bin/git" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  'fetch origin main --no-tags') ;;
  'rev-parse HEAD') printf '%s\n' "$MOCK_SHA" ;;
  'rev-parse refs/remotes/origin/main') printf '%s\n' "${MOCK_MAIN_SHA:-$MOCK_SHA}" ;;
  'status --porcelain --untracked-files=no') printf '%s' "${MOCK_DIRTY:-}" ;;
  *) exit 2 ;;
esac
EOF
cat >"$scratch/bin/cargo" <<'EOF'
#!/usr/bin/env bash
[[ "$*" == 'metadata --locked --no-deps --format-version 1' ]] || exit 2
cat "$MOCK_METADATA"
EOF
cat >"$scratch/bin/curl" <<'EOF'
#!/usr/bin/env bash
url="${@: -1}"
relative="${url#https://crates.io/api/v1/crates/}"
package="${relative%%/*}"
if [[ "$url" == *"/0."* ]]; then
  if [[ ",${MOCK_VISIBLE_PACKAGES:-}," == *",$package,"* ]]; then printf '200\n'
  else printf '%s\n' "${MOCK_VERSION_STATUS:-404}"; fi
else
  if [[ ",${MOCK_NEW_NAMES:-}," == *",$package,"* ]]; then printf '404\n'
  else printf '%s\n' "${MOCK_NAMESPACE_STATUS:-200}"; fi
fi
EOF
cat >"$scratch/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  *git/ref/heads/main*) printf '%s\n' "$MOCK_SHA" ;;
  *git/ref/tags/*) jq -nc --arg sha "${MOCK_TAG_SHA:-$MOCK_SHA}" '{object:{type:"commit",sha:$sha}}' ;;
  *releases/tags/*)
    tag="${@: -1}"
    tag="${tag##*/}"
    jq -nc --arg tag "$tag" '{tag_name:$tag,name:$tag,draft:false,prerelease:false}' ;;
  *actions/workflows/ci.yml*) printf '99\n' ;;
  *attempts/2/jobs*)
    jq -nc --arg sha "$MOCK_SHA" --arg conclusion "${MOCK_JOB_CONCLUSION:-success}" --argjson missing "${MOCK_MISSING_JOB:-false}" --argjson duplicate "${MOCK_DUPLICATE_JOB:-false}" --argjson attempt "${MOCK_JOB_ATTEMPT:-2}" --argjson names "$MOCK_JOBS" '
      [$names[] | {name:.,head_sha:$sha,run_attempt:$attempt,status:"completed",conclusion:$conclusion}]
      | if $missing then .[1:] else . end
      | if $duplicate then . + [.[0]] else . end | [{jobs:.}]' ;;
  *actions/runs/88*)
    jq -nc --arg sha "${MOCK_RUN_SHA:-$MOCK_SHA}" --arg branch "${MOCK_BRANCH:-candidate/manual-release-guard}" --arg conclusion "${MOCK_RUN_CONCLUSION:-success}" --argjson attempt "${MOCK_RUN_ATTEMPT:-2}" '{workflow_id:99,path:".github/workflows/ci.yml",name:"CI",event:"push",head_branch:$branch,head_sha:$sha,run_attempt:$attempt,status:"completed",conclusion:$conclusion}' ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$scratch/bin/"*
base=("PATH=$scratch/bin:$PATH" "GITHUB_TOKEN=fixture" "GITHUB_REPOSITORY=$(jq -r .repository "$SCRIPT_DIR/release-policy.json")" "GITHUB_REF=refs/heads/main" "GITHUB_EVENT_NAME=workflow_dispatch" "RELEASE_SHA=$sha" "RELEASE_MODE=dry-run" "RELEASE_SET=$allowed" "CANDIDATE_RUN_ID=88" "CANDIDATE_ATTEMPT=2" "MOCK_SHA=$sha" "MOCK_METADATA=$scratch/metadata.json" "MOCK_JOBS=$(jq -c .required_jobs "$SCRIPT_DIR/release-policy.json")")
gate() { env "${base[@]}" "$@" bash "$SCRIPT_DIR/release-gate.sh"; }
reject() {
  local expected="$1"; shift
  local output
  if output="$(gate "$@" 2>&1)"; then echo "expected rejection: $expected" >&2; exit 1; fi
  [[ "$output" == *"$expected"* ]] || { printf '%s\n' "$output" >&2; exit 1; }
}
gate >/dev/null
gate RELEASE_MODE=publish RELEASE_CONFIRMATION=publish >/dev/null
reject 'confirmation text publish' RELEASE_MODE=publish
reject 'outside the exact package/version allowlist' 'RELEASE_SET=[{"package_name":"lenso-unapproved","version":"0.1.0"}]'
reject 'invalid release_set' 'RELEASE_SET=[{"package_name":"lenso-capability-audit-log","version":"0.1.1","extra":true}]'
reject 'non-empty approved subset' RELEASE_SET='[]'
reject 'candidate run does not prove' MOCK_BRANCH=delta/verify/obsolete
reject 'candidate run does not prove' MOCK_RUN_SHA=2222222222222222222222222222222222222222
reject 'candidate run does not prove' MOCK_RUN_ATTEMPT=1
reject 'successful' MOCK_MISSING_JOB=true
reject 'successful' MOCK_DUPLICATE_JOB=true
reject 'successful' MOCK_JOB_CONCLUSION=failure
reject 'successful' MOCK_JOB_ATTEMPT=1
reject 'current remote main' MOCK_MAIN_SHA=2222222222222222222222222222222222222222
reject 'tracked modifications' MOCK_DIRTY=' M source.rs'
reject 'unexpected registry response' MOCK_VERSION_STATUS=503
reject 'first-name bootstrap' RELEASE_MODE=publish RELEASE_CONFIRMATION=publish MOCK_NAMESPACE_STATUS=404
gate RELEASE_SET='[]' MOCK_VERSION_STATUS=200 >/dev/null
capability_stage='[{"package_name":"lenso-capability-audit-log","version":"0.1.1"}]'
postgres_stage='[{"package_name":"lenso-audit-log-postgres-plugin","version":"0.1.1"}]'
gate "RELEASE_SET=$capability_stage" >/dev/null
gate "RELEASE_SET=$capability_stage" RELEASE_MODE=publish RELEASE_CONFIRMATION=publish MOCK_NEW_NAMES=lenso-audit-log-core,lenso-audit-log-d1-plugin >/dev/null
reject 'requires lenso-capability-audit-log' "RELEASE_SET=$postgres_stage"
reject 'requires lenso-audit-log-core' "RELEASE_SET=$postgres_stage" MOCK_VISIBLE_PACKAGES=lenso-capability-audit-log
gate "RELEASE_SET=$postgres_stage" MOCK_VISIBLE_PACKAGES=lenso-capability-audit-log,lenso-audit-log-core >/dev/null
reject 'not pending' "RELEASE_SET=$capability_stage" MOCK_VISIBLE_PACKAGES=lenso-capability-audit-log
python3 "$SCRIPT_DIR/release-config.py" --release-set "$capability_stage" --output "$scratch/stage.toml"
python3 - "$scratch/stage.toml" <<'PY'
import sys,tomllib
config=tomllib.load(open(sys.argv[1],'rb'))
assert config['workspace']['release'] is False
assert config['package']==[{'name':'lenso-capability-audit-log','release':True}]
PY
env EXPECTED_RELEASE_SET="$allowed" ACTUAL_RELEASES=null bash "$SCRIPT_DIR/release-plan.sh" >/dev/null
if env EXPECTED_RELEASE_SET="$allowed" 'ACTUAL_RELEASES=[{"package_name":"lenso-unapproved","version":"0.1.0"}]' bash "$SCRIPT_DIR/release-plan.sh" >"$scratch/plan.log" 2>&1; then exit 1; fi
actual="$(jq -c 'map(. + {tag:(.package_name + "@" + .version)})' <<<"$allowed")"
post() { env "${base[@]}" RELEASE_MODE=publish RELEASE_CONFIRMATION=publish EXPECTED_RELEASE_SET="$allowed" RELEASE_ACTION_OUTCOME=success MOCK_VERSION_STATUS=200 "ACTUAL_RELEASES=$actual" "$@" bash "$SCRIPT_DIR/release-postcondition.sh"; }
post >/dev/null
post_reject() {
  local expected="$1"; shift
  local output
  if output="$(post "$@" 2>&1)"; then echo "expected postcondition rejection: $expected" >&2; exit 1; fi
  [[ "$output" == *"$expected"* ]] || { printf '%s\n' "$output" >&2; exit 1; }
}
post_reject 'does not match approved release_set' 'ACTUAL_RELEASES=[]'
post_reject 'did not succeed' RELEASE_ACTION_OUTCOME=failure
post_reject 'does not point to approved source_sha' MOCK_TAG_SHA=2222222222222222222222222222222222222222
post_reject 'not visible on crates.io' MOCK_VERSION_STATUS=404
python3 - "$SCRIPT_DIR" <<'PY'
import json,pathlib,sys,tomllib
scripts=pathlib.Path(sys.argv[1])
policy=json.loads((scripts/'release-policy.json').read_text())
config=tomllib.loads((scripts.parent/'release-owner.toml').read_text())
assert config['workspace']['release'] is False
assert {p['name'] for p in config['package'] if p['release']} == {p['package_name'] for p in policy['packages']}
PY
python3 "$SCRIPT_DIR/test-release-archives.py"
printf '%s\n' 'exact release source, package set, CI attempt and archive tests passed'
