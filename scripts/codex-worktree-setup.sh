#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
node_bin="${CODESTORY_NODE:-node}"
exec "$node_bin" "$script_dir/codex-worktree-setup.mjs" "$@"
