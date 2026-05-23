#!/usr/bin/env bash
set -euo pipefail

dry_run=0
if [[ "${1:-}" == "--dry-run" || "${1:-}" == "-n" ]]; then
  dry_run=1
fi

repo_url="${CODESTORY_REPO_URL:-https://github.com/TheGreenCedar/CodeStory.git}"
DEFAULT_CODESTORY_REPO_REF="d78bf7c66a87e3454d1bfd82ba68cddd8d02004d"
if [[ -n "${CODESTORY_REPO_REF:-}" ]]; then
  repo_ref="$CODESTORY_REPO_REF"
else
  repo_ref="$DEFAULT_CODESTORY_REPO_REF"
fi
if [[ -z "$repo_ref" ]]; then
  echo "CODESTORY_REPO_REF resolved to an empty value." >&2
  exit 1
fi
codestory_home="${CODESTORY_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/codestory}"
source_dir="$codestory_home/src"
bin_dir="$codestory_home/bin"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) binary_name="codestory-cli.exe" ;;
  *) binary_name="codestory-cli" ;;
esac

dest="$bin_dir/$binary_name"

echo "CodeStory setup"
echo "  home: $codestory_home"
echo "  source: $source_dir"
echo "  binary: $dest"
echo "  repo: $repo_url"
echo "  ref: $repo_ref"

if [[ "$dry_run" == "1" ]]; then
  echo "Dry run only; no clone, build, or copy performed."
  echo "CODESTORY_CLI=$dest"
  exit 0
fi

command -v git >/dev/null 2>&1 || { echo "Required command 'git' was not found on PATH." >&2; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "Required command 'cargo' was not found on PATH." >&2; exit 1; }

mkdir -p "$codestory_home" "$bin_dir"

if [[ ! -d "$source_dir/.git" ]]; then
  if [[ -e "$source_dir" ]] && [[ -n "$(find "$source_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "Source directory exists but is not a git checkout: $source_dir" >&2
    exit 1
  fi
  git clone "$repo_url" "$source_dir"
else
  origin_url="$(git -C "$source_dir" config --get remote.origin.url)"
  if [[ "${origin_url%/}" != "${repo_url%/}" ]]; then
    echo "CodeStory source artifact remote is '$origin_url', expected '$repo_url'. Set CODESTORY_HOME or CODESTORY_REPO_URL intentionally." >&2
    exit 1
  fi
  dirty="$(git -C "$source_dir" status --porcelain)"
  if [[ -n "$dirty" ]]; then
    echo "CodeStory source artifact has local changes; refusing to update: $source_dir" >&2
    exit 1
  fi
fi

git -C "$source_dir" fetch --tags origin
git -C "$source_dir" checkout --detach "$repo_ref"

cargo build --release -p codestory-cli --manifest-path "$source_dir/Cargo.toml"

built="$source_dir/target/release/$binary_name"
if [[ ! -f "$built" ]]; then
  echo "Build completed but expected binary was not found: $built" >&2
  exit 1
fi

cp "$built" "$dest"
chmod +x "$dest"
"$dest" --help >/dev/null

echo "CODESTORY_CLI=$dest"
