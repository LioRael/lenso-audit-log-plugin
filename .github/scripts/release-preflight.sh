#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/release-set.sh"
: "${RELEASE_SHA:?}" "${RELEASE_SET:?}" "${RUNNER_TEMP:?}"
selected="$(release_set_canonical "$RELEASE_SET")"
packages=()
while IFS= read -r package; do packages+=(-p "$package"); done < <(jq -r '.[].package_name' <<<"$selected")
if [[ ${#packages[@]} -gt 0 ]]; then cargo package --locked "${packages[@]}"; fi
target_dir="$(cargo metadata --locked --no-deps --format-version 1 | jq -r .target_directory)"
python3 "$SCRIPT_DIR/release-archives.py" --source-sha "$RELEASE_SHA" --release-set "$selected" --directory "$target_dir/package" --output "$RUNNER_TEMP/release-archives.json"
