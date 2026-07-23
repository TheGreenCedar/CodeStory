#!/usr/bin/env bash
set -euo pipefail

dry_run=0
if [[ "${1:-}" == "--dry-run" || "${1:-}" == "-n" ]]; then
  dry_run=1
fi

repo_url="${CODESTORY_REPO_URL:-https://github.com/TheGreenCedar/CodeStory.git}"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
local_checkout_root=""
local_checkout_ref=""
if command -v git >/dev/null 2>&1; then
  local_checkout_root="$(cd "$script_dir/../../../.." 2>/dev/null && pwd || true)"
  if [[ -n "$local_checkout_root" && -d "$local_checkout_root/.git" ]]; then
    local_checkout_ref="$(git -C "$local_checkout_root" rev-parse HEAD 2>/dev/null || true)"
  fi
fi
use_local_checkout=0
if [[ -n "$local_checkout_root" && -z "${CODESTORY_REPO_URL:-}" && -z "${CODESTORY_REPO_REF:-}" ]]; then
  use_local_checkout=1
fi
if [[ -n "${CODESTORY_REPO_REF:-}" ]]; then
  repo_ref="$CODESTORY_REPO_REF"
elif [[ "$use_local_checkout" == "1" && -n "$local_checkout_ref" ]]; then
  repo_ref="working-tree:$local_checkout_ref"
else
  repo_ref=""
fi
repo_ref_for_display="${repo_ref:-remote default branch}"

redact_url_userinfo() {
  printf '%s' "$1" | sed -E 's#^(https?://)[^/@[:space:]]+@#\1***@#'
}

codestory_home="${CODESTORY_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/codestory}"
if [[ "$use_local_checkout" == "1" ]]; then
  source_dir="$local_checkout_root"
else
  source_dir="$codestory_home/src"
fi
bin_dir="$codestory_home/bin"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) binary_name="codestory-cli.exe" ;;
  *) binary_name="codestory-cli" ;;
esac

dest="$bin_dir/$binary_name"
repo_url_for_display="$(redact_url_userinfo "$repo_url")"

echo "CodeStory setup"
echo "  home: $codestory_home"
echo "  source: $source_dir"
echo "  binary: $dest"
echo "  repo: $repo_url_for_display"
echo "  ref: $repo_ref_for_display"

if [[ "$dry_run" == "1" ]]; then
  echo "Dry run only; no clone, build, or copy performed."
  echo "CODESTORY_CLI=$dest"
  exit 0
fi

command -v git >/dev/null 2>&1 || { echo "Required command 'git' was not found on PATH." >&2; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "Required command 'cargo' was not found on PATH." >&2; exit 1; }
command -v node >/dev/null 2>&1 || { echo "Required command 'node' was not found on PATH." >&2; exit 1; }

mkdir -p "$codestory_home" "$bin_dir"

if [[ "$use_local_checkout" != "1" ]]; then
  if [[ ! -d "$source_dir/.git" ]]; then
    if [[ -e "$source_dir" ]] && [[ -n "$(find "$source_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
      echo "Source directory exists but is not a git checkout: $source_dir" >&2
      exit 1
    fi
    git clone "$repo_url" "$source_dir"
  else
    origin_url="$(git -C "$source_dir" config --get remote.origin.url)"
    if [[ "${origin_url%/}" != "${repo_url%/}" ]]; then
      origin_url_for_display="$(redact_url_userinfo "$origin_url")"
      echo "CodeStory source artifact remote is '$origin_url_for_display', expected '$repo_url_for_display'. Set CODESTORY_HOME or CODESTORY_REPO_URL intentionally." >&2
      exit 1
    fi
    dirty="$(git -C "$source_dir" status --porcelain)"
    if [[ -n "$dirty" ]]; then
      echo "CodeStory source artifact has local changes; refusing to update: $source_dir" >&2
      exit 1
    fi
  fi

  git -C "$source_dir" fetch --tags origin
  if [[ -n "$repo_ref" ]]; then
    git -C "$source_dir" checkout --detach "$repo_ref"
  else
    if ! git -C "$source_dir" rev-parse --verify --quiet origin/HEAD >/dev/null; then
      git -C "$source_dir" remote set-head origin --auto
    fi
    git -C "$source_dir" checkout --detach origin/HEAD
  fi
fi

model_source="$(cd "$source_dir" && node scripts/prepare-embedded-model.mjs)"
if [[ -z "$model_source" || ! -f "$model_source" ]]; then
  echo "Embedded model preparation did not return a regular file: $model_source" >&2
  exit 1
fi
CODESTORY_EMBED_MODEL_SOURCE="$model_source" \
  cargo build --release --locked -p codestory-cli --manifest-path "$source_dir/Cargo.toml"

built="$source_dir/target/release/$binary_name"
if [[ ! -f "$built" ]]; then
  echo "Build completed but expected binary was not found: $built" >&2
  exit 1
fi

cp "$built" "$dest"
chmod +x "$dest"
"$dest" --help >/dev/null

echo "CODESTORY_CLI=$dest"
