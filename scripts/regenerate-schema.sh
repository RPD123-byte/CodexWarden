#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
codex_bin="${CODEX_BIN:-$HOME/.codex/packages/standalone/current/codex}"
pinned="$(cat "$repo_dir/src/protocol/schema/PINNED_CODEX_VERSION")"
actual="$($codex_bin --version)"
if [[ "$actual" != "$pinned" ]]; then
  echo "schema pin mismatch: expected '$pinned', got '$actual'" >&2
  exit 2
fi
tmp_dir="$(mktemp -d)"
expected_dir="$(mktemp -d)"
actual_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir" "$expected_dir" "$actual_dir"' EXIT
"$codex_bin" app-server generate-json-schema --experimental --out "$tmp_dir"

# The generator builds aggregate `definitions` maps from hash maps, so key order is not
# deterministic even for the same binary. Compare canonical JSON plus the exact file set.
canonicalize() {
  local source_dir="$1"
  local target_dir="$2"
  while IFS= read -r -d '' source_file; do
    local relative_path="${source_file#"$source_dir"/}"
    mkdir -p "$target_dir/$(dirname "$relative_path")"
    jq -S . "$source_file" > "$target_dir/$relative_path"
  done < <(find "$source_dir" -type f -name '*.json' -print0)
}

canonicalize "$repo_dir/src/protocol/schema/upstream" "$expected_dir"
canonicalize "$tmp_dir" "$actual_dir"
diff -ru "$expected_dir" "$actual_dir"
