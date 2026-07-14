#!/usr/bin/env bash
set -euo pipefail

project="."
intended_base_ref="${CODESTORY_INTENDED_BASE_REF:-origin/dev/codestory-next}"
pr_head_ref="${CODESTORY_PR_HEAD_REF:-}"
case "${CODESTORY_BRANCH_HEAD_PROOF:-}" in
  1|true|TRUE|yes|YES) branch_head_proof=1 ;;
  *) branch_head_proof=0 ;;
esac
resolve_cli_only=0
self_test=0

usage() {
  cat <<'EOF'
Usage: scripts/codex-worktree-setup.sh [options]

  --project <path>             Worktree to prepare (default: .)
  --intended-base-ref <ref>    Base ref used in the handoff proof summary
  --pr-head-ref <ref>          Optional PR head used in the proof summary
  --branch-head-proof          Prove only the PR branch head
  --resolve-cli-only           Resolve/install the CLI without indexing
  --self-test                  Run isolated setup tests
EOF
}

while (($#)); do
  case "$1" in
    --project|-Project)
      [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
      project="$2"
      shift 2
      ;;
    --intended-base-ref|-IntendedBaseRef)
      [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
      intended_base_ref="$2"
      shift 2
      ;;
    --pr-head-ref|-PrHeadRef)
      [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
      pr_head_ref="$2"
      shift 2
      ;;
    --branch-head-proof|-BranchHeadProof)
      branch_head_proof=1
      shift
      ;;
    --resolve-cli-only|-ResolveCliOnly)
      resolve_cli_only=1
      shift
      ;;
    --self-test|-SelfTest)
      self_test=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

warn() {
  printf 'warning: %s\n' "$*" >&2
}

canonical_dir() {
  (cd -- "$1" 2>/dev/null && pwd -P)
}

canonical_file() {
  local candidate="$1"
  local parent
  parent="$(canonical_dir "$(dirname -- "$candidate")")" || return 1
  printf '%s/%s\n' "$parent" "$(basename -- "$candidate")"
}

same_path() {
  local left right
  left="$(canonical_dir "$1")" || return 1
  right="$(canonical_dir "$2")" || return 1
  [[ "$left" == "$right" ]]
}

git_text() {
  git "$@" 2>/dev/null
}

git_commit() {
  [[ -n "${1:-}" ]] || return 1
  git_text rev-parse --verify "$1^{commit}" | sed -n '1p'
}

remote_head_name() {
  local ref="${1:-}"
  [[ -n "$ref" ]] || return 1
  case "$ref" in
    origin/*) printf 'refs/heads/%s\n' "${ref#origin/}" ;;
    refs/heads/*) printf '%s\n' "$ref" ;;
    refs/*|????????????????????????????????????????) return 1 ;;
    *) printf 'refs/heads/%s\n' "$ref" ;;
  esac
}

remote_head_result() {
  local remote_ref
  remote_ref="$(remote_head_name "$1")" || return 1
  git_text ls-remote origin "$remote_ref"
}

proof_target() {
  if [[ -n "$pr_head_ref" ]]; then
    if [[ "$branch_head_proof" == "1" ]]; then
      printf 'branch-head:%s\n' "$pr_head_ref"
    else
      printf 'base:%s + pr-head:%s\n' "$intended_base_ref" "$pr_head_ref"
    fi
  else
    printf '%s\n' "$intended_base_ref"
  fi
}

write_handoff_summary() {
  if ! command -v git >/dev/null 2>&1; then
    warn "Git is unavailable; skipping CodeStory handoff proof-target status."
    return
  fi

  local child_head base_commit head_commit branch
  child_head="$(git_commit HEAD || true)"
  if [[ -z "$child_head" ]]; then
    warn "Current directory is not a Git worktree; skipping CodeStory handoff proof-target status."
    return
  fi
  base_commit="$(git_commit "$intended_base_ref" || true)"
  head_commit="$(git_commit "$pr_head_ref" || true)"
  branch="$(git_text symbolic-ref --quiet --short HEAD || true)"
  [[ -n "$branch" ]] || branch="detached:$child_head"

  echo "CodeStory handoff proof target"
  echo "  intended_base_ref: $intended_base_ref"
  echo "  resolved_base_commit: ${base_commit:-unresolved}"
  echo "  child_start_head: $child_head"
  echo "  child_branch_or_detached: $branch"
  echo "  proof_target: $(proof_target)"
  echo "  pr_head_ref: ${pr_head_ref:-none}"
  echo "  pr_head_commit: ${head_commit:-none}"

  local label ref local_commit remote_result remote_commit
  for entry in "intended_base_ref|$intended_base_ref|$base_commit" \
               "main|origin/main|$(git_commit main || true)" \
               "dev/codestory-next|origin/dev/codestory-next|$(git_commit dev/codestory-next || true)"; do
    IFS='|' read -r label ref local_commit <<<"$entry"
    remote_result="$(remote_head_result "$ref" || true)"
    remote_commit="${remote_result%%[[:space:]]*}"
    if [[ "$label" == "intended_base_ref" ]]; then
      echo "  remote_tip_verification.intended_base.command: git ls-remote origin $(remote_head_name "$ref" || echo "$ref")"
      echo "  remote_tip_verification.intended_base.result: ${remote_result:-<no remote tip>}"
    fi
    if [[ -n "$local_commit" && -n "$remote_commit" && "$local_commit" != "$remote_commit" ]]; then
      warn "CodeStory handoff proof target: $label stale: local=$local_commit remote=$remote_commit"
    fi
  done

  if [[ -z "$base_commit" ]]; then
    warn "CodeStory handoff proof target: intended_base_ref unresolved: $intended_base_ref"
  fi
  if [[ -n "$pr_head_ref" ]]; then
    local remote_result
    remote_result="$(remote_head_result "$pr_head_ref" || true)"
    echo "  remote_tip_verification.pr_head.command: git ls-remote origin $(remote_head_name "$pr_head_ref" || echo "$pr_head_ref")"
    echo "  remote_tip_verification.pr_head.result: ${remote_result:-<no remote tip>}"
    [[ -n "$head_commit" ]] || warn "CodeStory handoff proof target: pr_head_ref unresolved: $pr_head_ref"
    [[ "$branch_head_proof" == "0" ]] || warn "CodeStory handoff proof target: branch-head proof requested; default PR proof is current base plus PR head."
  fi
}

expected_version() {
  local manifest="$1/crates/codestory-cli/Cargo.toml"
  local version
  version="$(sed -n -E 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$manifest" | sed -n '1p')"
  [[ -n "$version" ]] || { echo "Unable to read expected codestory-cli version from $manifest." >&2; return 1; }
  printf '%s\n' "$version"
}

install_dir() {
  printf '%s/bin\n' "${CODESTORY_HOME:-$HOME/.codestory}"
}

ACTUAL_VERSION=""
candidate_path() {
  local candidate="$1"
  if [[ "$candidate" == */* ]]; then
    [[ -x "$candidate" ]] || return 1
    canonical_file "$candidate"
  else
    command -v "$candidate" 2>/dev/null
  fi
}

test_cli_candidate() {
  local candidate resolved output
  candidate="$1"
  ACTUAL_VERSION=""
  resolved="$(candidate_path "$candidate")" || return 1
  output="$("$resolved" --version 2>/dev/null | sed -n '1p')" || return 1
  if [[ "$output" =~ ^codestory-cli[[:space:]]+([0-9][0-9A-Za-z.+-]*)$ ]]; then
    ACTUAL_VERSION="${BASH_REMATCH[1]}"
    [[ "$ACTUAL_VERSION" == "$EXPECTED_VERSION" ]] || return 1
    RESOLVED_CLI="$resolved"
    return 0
  fi
  return 1
}

RESOLVED_CLI=""
RESOLVE_ERROR=""
find_cli() {
  local root="$1"
  local home_bin candidate worktree_line sibling
  local -a candidates=()
  local -a stale=()
  home_bin="$(install_dir)"

  [[ -z "${CODESTORY_CLI:-}" ]] || candidates+=("$CODESTORY_CLI")
  candidate="$(command -v codestory-cli 2>/dev/null || true)"
  [[ -z "$candidate" ]] || candidates+=("$candidate")
  candidates+=(
    "$home_bin/codestory-cli"
    "$home_bin/releases/$EXPECTED_VERSION/codestory-cli"
    "$root/target/release/codestory-cli"
  )

  if command -v git >/dev/null 2>&1; then
    while IFS= read -r worktree_line; do
      [[ "$worktree_line" == worktree\ * ]] || continue
      sibling="${worktree_line#worktree }"
      same_path "$sibling" "$root" && continue
      candidates+=("$sibling/target/release/codestory-cli")
    done < <(git worktree list --porcelain 2>/dev/null || true)
  fi

  RESOLVED_CLI=""
  for candidate in "${candidates[@]}"; do
    if test_cli_candidate "$candidate"; then
      return 0
    fi
    [[ -z "$ACTUAL_VERSION" ]] || stale+=("$candidate reported $ACTUAL_VERSION")
  done

  RESOLVE_ERROR="No ready codestory-cli $EXPECTED_VERSION found via CODESTORY_CLI, PATH, the CodeStory install directory, this worktree's target/release, or sibling worktree target/release directories."
  if ((${#stale[@]})); then
    local old_ifs="$IFS"
    IFS='; '
    RESOLVE_ERROR+=" Stale candidates: ${stale[*]}."
    IFS="$old_ifs"
  fi
  return 1
}

release_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64|Darwin:aarch64) echo "macos-arm64" ;;
    Darwin:x86_64) echo "macos-x64" ;;
    Linux:x86_64) echo "linux-x64" ;;
    Linux:aarch64|Linux:arm64) echo "linux-arm64" ;;
    *) return 1 ;;
  esac
}

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print tolower($1)}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print tolower($1)}'
  else
    echo "Neither shasum nor sha256sum is available." >&2
    return 1
  fi
}

install_current_release_cli() {
  local version="$1" target archive_name base_url temp archive sums expected actual extracted destination
  target="$(release_target)" || { echo "No release asset is available for $(uname -s)/$(uname -m)." >&2; return 1; }
  command -v curl >/dev/null 2>&1 || { echo "curl is required to install the current CodeStory release." >&2; return 1; }
  command -v tar >/dev/null 2>&1 || { echo "tar is required to install the current CodeStory release." >&2; return 1; }

  archive_name="codestory-cli-v$version-$target.tar.gz"
  base_url="${CODESTORY_RELEASE_BASE_URL:-https://github.com/TheGreenCedar/CodeStory/releases/download/v$version}"
  temp="$(mktemp -d "${TMPDIR:-/tmp}/codestory-install-XXXXXX")"
  archive="$temp/$archive_name"
  sums="$temp/SHA256SUMS.txt"

  echo
  echo "==> Install current release CLI"
  echo "Trying codestory-cli $version release install before Cargo build."
  if ! curl -fsSL "$base_url/SHA256SUMS.txt" -o "$sums" ||
     ! curl -fsSL "$base_url/$archive_name" -o "$archive"; then
    rm -rf -- "$temp"
    return 1
  fi
  expected="$(awk -v name="$archive_name" '$2 == name || $2 == "*" name { print tolower($1); exit }' "$sums")"
  if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
    echo "SHA256SUMS.txt has no valid entry for $archive_name." >&2
    rm -rf -- "$temp"
    return 1
  fi
  actual="$(sha256_file "$archive")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Downloaded archive checksum mismatch for $archive_name: expected $expected, got $actual" >&2
    rm -rf -- "$temp"
    return 1
  fi

  if ! mkdir -p "$temp/extract" "$(install_dir)" || ! tar -xzf "$archive" -C "$temp/extract"; then
    rm -rf -- "$temp"
    return 1
  fi
  extracted="$(find "$temp/extract" -type f -name codestory-cli -print -quit)"
  if [[ -z "$extracted" ]]; then
    echo "Downloaded archive did not contain codestory-cli." >&2
    rm -rf -- "$temp"
    return 1
  fi
  destination="$(install_dir)/codestory-cli"
  if ! cp "$extracted" "$destination" 2>/dev/null; then
    destination="$(install_dir)/releases/$version/codestory-cli"
    if ! mkdir -p "$(dirname "$destination")" || ! cp "$extracted" "$destination"; then
      rm -rf -- "$temp"
      return 1
    fi
  fi
  chmod +x "$destination"
  if ! test_cli_candidate "$destination"; then
    echo "Installed codestory-cli did not report expected version $version." >&2
    rm -rf -- "$temp"
    return 1
  fi
  rm -rf -- "$temp"
}

find_rehydrate_source() {
  local target="$1" configured line candidate
  if [[ -n "${CODESTORY_REHYDRATE_FROM:-}" ]]; then
    configured="$(canonical_dir "$CODESTORY_REHYDRATE_FROM" || true)"
    if [[ -z "$configured" ]]; then
      warn "Ignoring CODESTORY_REHYDRATE_FROM='$CODESTORY_REHYDRATE_FROM': path does not exist."
    elif ! same_path "$configured" "$target"; then
      printf '%s\n' "$configured"
      return
    fi
  fi
  command -v git >/dev/null 2>&1 || return
  while IFS= read -r line; do
    [[ "$line" == worktree\ * ]] || continue
    candidate="${line#worktree }"
    same_path "$candidate" "$target" && continue
    if [[ -f "$candidate/Cargo.toml" ]]; then
      canonical_dir "$candidate"
      return
    fi
  done < <(git worktree list --porcelain 2>/dev/null || true)
}

run_step() {
  local optional="$1" label="$2"
  local status
  shift 2
  echo
  echo "==> $label"
  if "$@"; then
    return
  else
    status=$?
  fi
  if [[ "$optional" == "1" ]]; then
    warn "$label failed with exit code $status; continuing."
    return 0
  fi
  return "$status"
}

repair_agent() {
  "$1" ready --goal agent --repair --project "$2" --format json --run-id shared-agent
}

doctor_summary() {
  local cli="$1" project_path="$2" json node_command
  if ! json="$("$cli" doctor --project "$project_path" --format json 2>&1)"; then
    echo "$json" >&2
    return 1
  fi
  node_command="${CODESTORY_NODE:-node}"
  # The JavaScript program is intentionally literal.
  # shellcheck disable=SC2016
  printf '%s' "$json" | "$node_command" -e '
    let input = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", chunk => input += chunk);
    process.stdin.on("end", () => {
      const doctor = JSON.parse(input);
      const verdict = goal => (doctor.readiness || []).find(item => item.goal === goal);
      const local = verdict("local_navigation");
      const agent = verdict("agent_packet_search");
      const ready = item => item && item.status === "ready";
      const firstNext = [local, agent]
        .filter(item => !ready(item))
        .flatMap(item => item && item.minimum_next || [])
        .concat(doctor.next_commands || [])
        .find(Boolean);
      console.log("CodeStory worktree readiness");
      console.log(`  local_navigation: ${local ? local.status : "unknown"}`);
      if (local && local.summary) console.log(`    reason: ${local.summary}`);
      console.log(`  agent_packet_search: ${agent ? agent.status : "unknown"}`);
      if (agent && agent.summary) console.log(`    reason: ${agent.summary}`);
      console.log(`  retrieval_mode: ${doctor.retrieval_mode || "unknown"}`);
      console.log(`  degraded_reason: ${doctor.degraded_reason || "none"}`);
      if (firstNext) console.log(`  minimum_next: ${firstNext}`);
      if (!ready(agent)) console.log("  handoff: CodeStory packet/search is unavailable; use direct source reads until the minimum_next command repairs readiness.");
    });
  '
}

run_setup() {
  local project_path expected cli source install_error
  project_path="$(canonical_dir "$project")" || { echo "Project path does not exist: $project" >&2; return 1; }
  EXPECTED_VERSION="$(expected_version "$project_path")"

  pushd "$project_path" >/dev/null
  write_handoff_summary

  local sccache=""
  if [[ -x "$HOME/.cargo/bin/sccache" ]]; then
    sccache="$HOME/.cargo/bin/sccache"
  elif command -v sccache >/dev/null 2>&1; then
    sccache="$(command -v sccache)"
  fi
  if [[ -n "$sccache" ]]; then
    export RUSTC_WRAPPER="$sccache"
    echo "Using RUSTC_WRAPPER=$sccache"
  fi

  if ! find_cli "$project_path"; then
    local resolve_error="$RESOLVE_ERROR"
    if install_error="$(install_current_release_cli "$EXPECTED_VERSION" 2>&1)" && find_cli "$project_path"; then
      [[ -z "$install_error" ]] || echo "$install_error"
      :
    elif [[ "$resolve_cli_only" == "1" ]]; then
      popd >/dev/null
      echo "$resolve_error Current-release install failed: ${install_error:-installed CLI was not discoverable}. Set CODESTORY_CLI to a ready binary." >&2
      return 1
    else
      echo
      echo "==> Build release CLI"
      warn "$resolve_error Current-release install failed: ${install_error:-installed CLI was not discoverable}. Building release CLI with cargo."
      cargo build --release --locked -p codestory-cli
      find_cli "$project_path" || { popd >/dev/null; echo "$RESOLVE_ERROR" >&2; return 1; }
    fi
  fi
  cli="$RESOLVED_CLI"
  echo "CODESTORY_CLI=$cli"
  if [[ "$resolve_cli_only" == "1" ]]; then
    popd >/dev/null
    return
  fi

  source="$(find_rehydrate_source "$project_path" || true)"
  if [[ -n "$source" ]]; then
    run_step 1 "Rehydrate CodeStory cache from $source" "$cli" cache rehydrate --from-project "$source" --project "$project_path"
  else
    echo
    echo "==> Rehydrate CodeStory cache"
    echo "No sibling source worktree found; refreshing this worktree directly."
  fi
  run_step 0 "Refresh SQLite graph/search/doc cache" "$cli" index --project "$project_path" --refresh auto
  run_step 1 "Repair agent sidecar readiness" repair_agent "$cli" "$project_path"
  run_step 1 "Doctor readiness handoff" doctor_summary "$cli" "$project_path"
  popd >/dev/null
}

assert_test() {
  [[ "$1" == "1" ]] || { echo "Self-test failed: $2" >&2; return 1; }
}

invoke_self_test() {
  local base_temp temp old_home old_cli old_path old_rehydrate old_node old_release_base node_bin
  base_temp="$(mktemp -d "${TMPDIR:-/tmp}/codestory-setup-XXXXXX")"
  base_temp="$(canonical_dir "$base_temp")"
  old_home="${CODESTORY_HOME-}"
  old_cli="${CODESTORY_CLI-}"
  old_path="$PATH"
  old_rehydrate="${CODESTORY_REHYDRATE_FROM-}"
  old_node="${CODESTORY_NODE-}"
  old_release_base="${CODESTORY_RELEASE_BASE_URL-}"
  node_bin="$(command -v node)"
  trap 'rm -rf -- "$base_temp"' RETURN

  local expected_branch_head_proof=0
  case "${CODESTORY_BRANCH_HEAD_PROOF:-}" in
    1|true|TRUE|yes|YES) expected_branch_head_proof=1 ;;
  esac
  assert_test "$([[ "$branch_head_proof" == "$expected_branch_head_proof" ]] && echo 1 || echo 0)" "CODESTORY_BRANCH_HEAD_PROOF should match the Windows proof-target override"

  temp="$base_temp/workspace with spaces ü"
  mkdir -p "$temp"

  mkdir -p "$temp/project/crates/codestory-cli" "$temp/source" "$temp/home/bin/releases/0.11.4" "$temp/path"
  printf 'version = "0.11.4"\n' >"$temp/project/crates/codestory-cli/Cargo.toml"
  printf '[workspace]\n' >"$temp/source/Cargo.toml"
  cat >"$temp/path/codestory-cli" <<EOF
#!/bin/sh
echo 'codestory-cli 0.11.3'
EOF
  cat >"$temp/home/bin/releases/0.11.4/codestory-cli" <<EOF
#!/bin/sh
if [ "\${1:-}" = "--version" ]; then
  echo 'codestory-cli 0.11.4'
  exit 0
fi
printf '%s\n' "\$*" >>'$temp/invocations'
if [ "\${1:-}" = "doctor" ]; then
  printf '%s\n' '{"retrieval_mode":"full","degraded_reason":null,"readiness":[{"goal":"local_navigation","status":"ready","summary":"Local navigation ready."},{"goal":"agent_packet_search","status":"ready","summary":"Agent packet/search ready."}],"next_commands":[]}'
fi
EOF
  chmod +x "$temp/path/codestory-cli" "$temp/home/bin/releases/0.11.4/codestory-cli"

  export CODESTORY_HOME="$temp/home"
  unset CODESTORY_CLI
  export PATH="$temp/path:/usr/bin:/bin"
  export CODESTORY_REHYDRATE_FROM="$temp/source"
  export CODESTORY_NODE="$node_bin"
  project="$temp/project"
  resolve_cli_only=0
  EXPECTED_VERSION="0.11.4"
  find_cli "$project"
  assert_test "$([[ "$RESOLVED_CLI" == "$temp/home/bin/releases/0.11.4/codestory-cli" ]] && echo 1 || echo 0)" "stale PATH CLI should be rejected in favor of the current versioned install"
  assert_test "$([[ "$(release_target)" == macos-* || "$(release_target)" == linux-* ]] && echo 1 || echo 0)" "host should map to a supported POSIX release asset"
  assert_test "$([[ "$(remote_head_name origin/dev/codestory-next)" == refs/heads/dev/codestory-next ]] && echo 1 || echo 0)" "origin refs should map to remote head refs"

  local asset_target archive_name archive_hash
  asset_target="$(release_target)"
  archive_name="codestory-cli-v0.11.5-$asset_target.tar.gz"
  mkdir -p "$base_temp/release/payload"
  cat >"$base_temp/release/payload/codestory-cli" <<'EOF'
#!/bin/sh
echo 'codestory-cli 0.11.5'
EOF
  chmod +x "$base_temp/release/payload/codestory-cli"
  tar -czf "$base_temp/release/$archive_name" -C "$base_temp/release/payload" codestory-cli
  archive_hash="$(sha256_file "$base_temp/release/$archive_name")"
  printf '%s  %s\n' "$archive_hash" "$archive_name" >"$base_temp/release/SHA256SUMS.txt"
  export CODESTORY_HOME="$temp/download-home"
  export CODESTORY_RELEASE_BASE_URL="file://$base_temp/release"
  EXPECTED_VERSION="0.11.5"
  install_current_release_cli "$EXPECTED_VERSION" >"$temp/install-output"
  assert_test "$([[ "$RESOLVED_CLI" == "$temp/download-home/bin/codestory-cli" ]] && echo 1 || echo 0)" "checksum-verified current release should install into the POSIX CodeStory home"
  export CODESTORY_HOME="$temp/home"
  unset CODESTORY_RELEASE_BASE_URL
  EXPECTED_VERSION="0.11.4"

  run_setup >"$temp/output" 2>"$temp/errors"
  local expected_log actual_log
  expected_log="cache rehydrate --from-project $temp/source --project $temp/project
index --project $temp/project --refresh auto
ready --goal agent --repair --project $temp/project --format json --run-id shared-agent
doctor --project $temp/project --format json"
  actual_log="$(cat "$temp/invocations")"
  assert_test "$([[ "$actual_log" == "$expected_log" ]] && echo 1 || echo 0)" "setup should rehydrate, index, repair agent readiness, and inspect doctor status in order"
  assert_test "$(grep -q 'agent_packet_search: ready' "$temp/output" && echo 1 || echo 0)" "doctor readiness should be summarized"

  export PATH="$old_path"
  if [[ -n "$old_home" ]]; then export CODESTORY_HOME="$old_home"; else unset CODESTORY_HOME; fi
  if [[ -n "$old_cli" ]]; then export CODESTORY_CLI="$old_cli"; else unset CODESTORY_CLI; fi
  if [[ -n "$old_rehydrate" ]]; then export CODESTORY_REHYDRATE_FROM="$old_rehydrate"; else unset CODESTORY_REHYDRATE_FROM; fi
  if [[ -n "$old_node" ]]; then export CODESTORY_NODE="$old_node"; else unset CODESTORY_NODE; fi
  if [[ -n "$old_release_base" ]]; then export CODESTORY_RELEASE_BASE_URL="$old_release_base"; else unset CODESTORY_RELEASE_BASE_URL; fi
  trap - RETURN
  rm -rf -- "$base_temp"
  echo "codex-worktree-setup POSIX self-test: ok"
}

if [[ "$self_test" == "1" ]]; then
  invoke_self_test
else
  run_setup
fi
