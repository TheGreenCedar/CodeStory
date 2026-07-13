#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
contract="$repo_root/scripts/release-evidence/machine-contract.json"

get() { jq -er "$1" "$contract"; }
profile=$(get '.vm.profile')
profile_id=$(get '.profile_id')
repository=$(get '.repository')
runner_name=$(get '.runner.name')
runner_version=$(get '.runner.version')
runner_root=$(get '.runner.root')
guest_owner_path="$runner_root/artifacts/ownership.json"
model_name=$(get '.assets.model.name')
model_sha=$(get '.assets.model.sha256')
model_source=${CODESTORY_RELEASE_EVIDENCE_MODEL_SEED:-"$HOME/Library/Caches/dev.codestory.codestory/retrieval/models/$model_name"}
base_image_url=$(get '.vm.base_image.url')
base_image_sha=$(get '.vm.base_image.sha512')
base_image_path="$HOME/Library/Caches/dev.codestory.codestory/release-evidence-runner/$(basename "$base_image_url")"
profile_dir="$HOME/.colima/$profile"
lima_dir="$HOME/.colima/_lima/colima-$profile"
owner_path="$profile_dir/codestory-release-evidence-owner.json"
contract_sha=$(shasum -a 256 "$contract" | awk '{print $1}')

usage() {
  echo "usage: $0 provision|verify|start|stop|unregister|destroy" >&2
  exit 2
}

profile_exists() {
  test -d "$profile_dir" || test -d "$lima_dir"
}

profile_running() {
  colima status --profile "$profile" >/dev/null 2>&1
}

require_lifecycle_tools() {
  command -v colima >/dev/null
  command -v jq >/dev/null
}

colima_git_commit() {
  colima version | awk '/^git commit:/{print $3}'
}

require_provisioning_eligibility() {
  require_lifecycle_tools
  for tool in curl gh git limactl ruby shasum; do command -v "$tool" >/dev/null; done
  test "$(uname -s)" = Darwin
  test "$(uname -m)" = "$(get '.host.architecture')"
  test "$(sysctl -n hw.model)" = "$(get '.host.model')"
  test "$(sysctl -n machdep.cpu.brand_string)" = "$(get '.host.chip')"
  test "$(sw_vers -productVersion)" = "$(get '.host.macos_version')"
  test "$(sysctl -n hw.memsize)" -ge "$(get '.host.minimum_memory_bytes')"
  test "$(colima version | awk 'NR == 1 {print $3}')" = "$(get '.host.colima_version')"
  test "$(colima_git_commit)" = "$(get '.host.colima_git_commit')"
  test "$(limactl --version | awk '{print $3}')" = "$(get '.host.lima_version')"
  test -z "$(git -C "$repo_root" status --porcelain)"
  gh auth status >/dev/null
  gh api "repos/$repository/actions/runners" >/dev/null
}

ensure_base_image() {
  mkdir -p "$(dirname "$base_image_path")"
  if ! test -f "$base_image_path" \
      || ! printf '%s  %s\n' "$base_image_sha" "$base_image_path" | shasum -a 512 -c - >/dev/null 2>&1; then
    rm -f "$base_image_path.partial"
    curl -fL --retry 3 -o "$base_image_path.partial" "$base_image_url"
    printf '%s  %s\n' "$base_image_sha" "$base_image_path.partial" | shasum -a 512 -c -
    mv "$base_image_path.partial" "$base_image_path"
  fi
}

owner_json() {
  test -f "$owner_path"
  jq -e --arg profile_id "$profile_id" --arg profile "$profile" \
    --arg repository "$repository" --arg runner_name "$runner_name" '
    .schema_version == 1 and .profile_id == $profile_id and .profile == $profile and
    .repository == $repository and .runner_name == $runner_name and
    ((.runner_id | type) == "number" or .runner_id == null)
    ' "$owner_path" >/dev/null
  cat "$owner_path"
}

write_ownership() {
  local runner_id=${1:-null}
  local created_at
  local tmp
  if test -f "$owner_path"; then
    created_at=$(jq -er '.created_at' "$owner_path")
  else
    created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  fi
  tmp=$(mktemp)
  jq -n --arg profile_id "$profile_id" --arg profile "$profile" \
    --arg repository "$repository" --arg runner_name "$runner_name" \
    --arg created_at "$created_at" --argjson runner_id "$runner_id" \
    '{schema_version:1,profile_id:$profile_id,profile:$profile,repository:$repository,
      runner_name:$runner_name,runner_id:$runner_id,created_at:$created_at}' >"$tmp"
  mkdir -p "$profile_dir"
  install -m 0600 "$tmp" "$owner_path"
  if profile_running && colima ssh --profile "$profile" -- test -d "$runner_root"; then
    colima ssh --profile "$profile" -- sudo -u codestory-runner \
      tee "$guest_owner_path" <"$tmp" >/dev/null
  fi
  rm -f "$tmp"
}

remote_by_name() {
  gh api "repos/$repository/actions/runners" --jq \
    ".runners | map(select(.name == \"$runner_name\"))"
}

remote_by_id() {
  local runner_id=$1
  gh api "repos/$repository/actions/runners/$runner_id"
}

remote_by_id_or_absent() {
  local runner_id=$1
  local error
  local remote
  error=$(mktemp)
  if remote=$(gh api "repos/$repository/actions/runners/$runner_id" 2>"$error"); then
    rm -f "$error"
    printf '%s\n' "$remote"
    return
  fi
  if grep -Fq 'HTTP 404' "$error"; then
    rm -f "$error"
    printf 'null\n'
    return
  fi
  cat "$error" >&2
  rm -f "$error"
  return 1
}

assert_remote_exact() {
  local remote=$1
  local runner_id=$2
  jq -e --argjson id "$runner_id" --arg name "$runner_name" \
    --arg version "$runner_version" --slurpfile contract "$contract" '
    .id == $id and .name == $name and .os == "Linux" and
    (.version == $version or (.status == "offline" and .version == null)) and
    ((.labels | map(.name)) as $labels |
      ($contract[0].runner.labels - $labels | length) == 0)
    ' <<<"$remote" >/dev/null
}

assert_not_busy() {
  local remotes
  local count
  local owner_id
  local remote
  remotes=$(remote_by_name)
  count=$(jq 'length' <<<"$remotes")
  test "$count" -le 1
  if test "$count" -eq 0; then
    if test -f "$owner_path" && test "$(jq -r '.runner_id' "$owner_path")" != null; then
      echo "owned runner is missing from GitHub; destroy and reprovision" >&2
      exit 1
    fi
    return
  fi
  if ! test -f "$owner_path"; then
    echo "runner name is already registered without a local ownership marker" >&2
    exit 1
  fi
  owner_id=$(jq -r '.runner_id' "$owner_path")
  if test "$owner_id" = null; then
    echo "runner name is registered but the ownership marker has no runner ID" >&2
    exit 1
  fi
  remote=$(jq -c '.[0]' <<<"$remotes")
  assert_remote_exact "$remote" "$owner_id"
  test "$(jq -r '.busy' <<<"$remote")" = false || {
    echo "runner $owner_id is busy; refusing to mutate it" >&2
    exit 1
  }
}

yaml_json() {
  ruby -ryaml -rjson -e 'puts JSON.generate(YAML.load_file(ARGV[0]))' "$1"
}

assert_vm_config() {
  local lima
  local colima
  lima=$(yaml_json "$lima_dir/lima.yaml")
  colima=$(yaml_json "$profile_dir/colima.yaml")
  jq -e --arg image "$base_image_path" --slurpfile contract "$contract" '
    .vmType == $contract[0].vm.type and .arch == $contract[0].vm.architecture and
    .cpus == $contract[0].vm.cpus and .memory == (($contract[0].vm.memory_gib * 1024 | tostring) + "MiB") and
    .disk == (($contract[0].vm.root_disk_gib | tostring) + "GiB") and
    .mountType == $contract[0].vm.mount_type and (.mounts // []) == [] and
    .images[0].location == $image
    ' <<<"$lima" >/dev/null
  jq -e --slurpfile contract "$contract" '
    .cpu == $contract[0].vm.cpus and .memory == $contract[0].vm.memory_gib and
    .disk == $contract[0].vm.data_disk_gib and .arch == $contract[0].vm.architecture and
    .runtime == $contract[0].vm.runtime and .vmType == $contract[0].vm.type and
    .autoActivate == $contract[0].vm.activate_host_context and
    .mountType == $contract[0].vm.mount_type and (.mounts // []) == []
    ' <<<"$colima" >/dev/null
}

start_profile() {
  local fresh=false
  if profile_exists; then
    owner_json >/dev/null
  else
    fresh=true
  fi
  if ! profile_running; then
    colima start --profile "$profile" --activate=false --cpu "$(get '.vm.cpus')" \
      --memory "$(get '.vm.memory_gib')" --disk "$(get '.vm.data_disk_gib')" \
      --root-disk "$(get '.vm.root_disk_gib')" --runtime "$(get '.vm.runtime')" \
      --arch "$(get '.vm.architecture')" --vm-type "$(get '.vm.type')" \
      --mount-type "$(get '.vm.mount_type')" --mount none \
      --disk-image "$base_image_path"
  fi
  assert_vm_config
  if test "$fresh" = true; then write_ownership null; fi
}

sync_validation_source() {
  local source_sha
  source_sha=${CODESTORY_RELEASE_EVIDENCE_SHA:-$(git -C "$repo_root" rev-parse HEAD)}
  colima ssh --profile "$profile" -- rm -rf /tmp/codestory-provision
  colima ssh --profile "$profile" -- mkdir -p /tmp/codestory-provision
  git -C "$repo_root" archive --format=tar "$source_sha" \
    | colima ssh --profile "$profile" -- tar -xf - -C /tmp/codestory-provision
  printf '%s\n' "$source_sha"
}

sync_model_seed() {
  local guest_seed=/tmp/codestory-release-evidence-model-seed
  colima ssh --profile "$profile" -- rm -f "$guest_seed"
  if test -f "$model_source"; then
    printf '%s  %s\n' "$model_sha" "$model_source" | shasum -a 256 -c - >/dev/null
    colima ssh --profile "$profile" -- tee "$guest_seed" <"$model_source" >/dev/null
    printf '%s\n' "$guest_seed"
  else
    printf '%s\n' -
  fi
}

provision_guest() {
  local source_sha=$1
  local model_seed=$2
  colima ssh --profile "$profile" -- sudo bash \
    /tmp/codestory-provision/scripts/release-evidence/guest-provision.sh \
    /tmp/codestory-provision/scripts/release-evidence/machine-contract.json \
    "$contract_sha" "$source_sha" "$(get '.guest.apt_snapshot')" \
    "$(get '.guest.apt_packages.jq')" "$model_seed"
}

runner_inspect() {
  colima ssh --profile "$profile" -- sudo bash \
    "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" inspect \
    "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
}

configure_runner() {
  local state
  local token
  local agent_id
  state=$(runner_inspect)
  test "$(jq -r '.binary_version' <<<"$state")" = "$runner_version"
  if test "$(jq -r '.configured' <<<"$state")" = true; then
    test "$(jq -r '.exact' <<<"$state")" = true
    colima ssh --profile "$profile" -- sudo bash \
      "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" configure \
      "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json" </dev/null
  else
    token=$(gh api --method POST "repos/$repository/actions/runners/registration-token" --jq .token)
    printf '%s\n' "$token" | colima ssh --profile "$profile" -- sudo bash \
      "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" configure \
      "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
    unset token
  fi
  state=$(runner_inspect)
  test "$(jq -r '.configured and .exact' <<<"$state")" = true
  agent_id=$(jq -er '.agent_id' <<<"$state")
  write_ownership "$agent_id"
}

write_host_attestation() {
  local boot_id
  local tmp
  assert_vm_config
  boot_id=$(colima ssh --profile "$profile" -- sed -n '1p' /proc/sys/kernel/random/boot_id)
  tmp=$(mktemp)
  jq -n --arg profile_id "$profile_id" --arg contract_sha "$contract_sha" \
    --arg architecture "$(uname -m)" --arg model "$(sysctl -n hw.model)" \
    --arg chip "$(sysctl -n machdep.cpu.brand_string)" \
    --arg macos "$(sw_vers -productVersion)" \
    --argjson memory "$(sysctl -n hw.memsize)" \
    --arg colima "$(colima version | awk 'NR == 1 {print $3}')" \
    --arg colima_commit "$(colima_git_commit)" \
    --arg lima "$(limactl --version | awk '{print $3}')" \
    --arg profile "$profile" --arg type "$(get '.vm.type')" \
    --arg vm_arch "$(get '.vm.architecture')" --arg runtime "$(get '.vm.runtime')" \
    --arg mount_type "$(get '.vm.mount_type')" --argjson cpus "$(get '.vm.cpus')" \
    --argjson memory_gib "$(get '.vm.memory_gib')" \
    --argjson data_disk_gib "$(get '.vm.data_disk_gib')" \
    --argjson root_disk_gib "$(get '.vm.root_disk_gib')" \
    --arg image_url "$base_image_url" --arg image_sha "$base_image_sha" \
    --arg boot_id "$boot_id" --arg attested_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '
    {schema_version:1,profile_id:$profile_id,contract_sha256:$contract_sha,
      attested_at:$attested_at,
      host:{architecture:$architecture,model:$model,chip:$chip,macos_version:$macos,
        memory_bytes:$memory,colima_version:$colima,colima_git_commit:$colima_commit,
        lima_version:$lima},
      vm:{profile:$profile,type:$type,architecture:$vm_arch,runtime:$runtime,
        mount_type:$mount_type,host_mounts:[],cpus:$cpus,memory_gib:$memory_gib,
        data_disk_gib:$data_disk_gib,root_disk_gib:$root_disk_gib,
        base_image_url:$image_url,base_image_sha512:$image_sha,boot_id:$boot_id}}
    ' >"$tmp"
  colima ssh --profile "$profile" -- sudo -u codestory-runner \
    tee "$runner_root/artifacts/host-attestation.json" <"$tmp" >/dev/null
  rm -f "$tmp"
}

guest_verify() {
  colima ssh --profile "$profile" -- sudo -u codestory-runner env \
    HOME="$runner_root/home" CARGO_HOME="$runner_root/cargo" \
    RUSTUP_HOME="$runner_root/rustup" PATH="/usr/local/bin:/usr/bin:/bin" \
    bash "$runner_root/validation/codestory/scripts/release-evidence/guest-verify.sh" \
    "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
}

assert_guest_idle() {
  local containers
  containers=$(colima ssh --profile "$profile" -- docker ps \
    --format '{{.ID}} {{.Names}} {{.Image}}')
  if test -n "$containers"; then
    echo "dedicated evidence VM has running containers; refusing to continue:" >&2
    printf '%s\n' "$containers" >&2
    return 1
  fi
}

start_runner() {
  colima ssh --profile "$profile" -- sudo bash \
    "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" start \
    "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
}

wait_online() {
  local runner_id
  local remote
  runner_id=$(jq -er '.runner_id' "$owner_path")
  for _ in {1..20}; do
    remote=$(remote_by_id "$runner_id")
    if test -n "$remote" && test "$(jq -r '.status' <<<"$remote")" = online; then break; fi
    sleep 1
  done
  test -n "$remote"
  assert_remote_exact "$remote" "$runner_id"
  test "$(jq -r '.status' <<<"$remote")" = online
  test "$(jq -r '.busy' <<<"$remote")" = false
  printf '%s\n' "$remote"
}

wait_offline() {
  local runner_id
  local remote
  runner_id=$(jq -r '.runner_id' "$owner_path")
  if test "$runner_id" = null; then
    remote=$(remote_by_name)
    test "$(jq 'length' <<<"$remote")" -eq 0
    return
  fi
  for _ in {1..20}; do
    remote=$(remote_by_id "$runner_id")
    if test "$(jq -r '.status' <<<"$remote")" = offline; then break; fi
    sleep 1
  done
  assert_remote_exact "$remote" "$runner_id"
  test "$(jq -r '.status' <<<"$remote")" = offline
  test "$(jq -r '.busy' <<<"$remote")" = false
}

verify_runner() {
  owner_json >/dev/null
  profile_running
  assert_not_busy
  assert_guest_idle
  quiesce_runner
  write_host_attestation
  guest_verify
  start_runner
  wait_online
}

stop_local_runner() {
  if ! colima ssh --profile "$profile" -- sudo test -f \
      "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh"; then
    test "$(jq -r '.runner_id' "$owner_path")" = null
    return
  fi
  colima ssh --profile "$profile" -- sudo bash \
    "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" stop \
    "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
  test "$(runner_inspect | jq -r '.service_active')" = false
}

quiesce_runner() {
  assert_not_busy
  stop_local_runner
  wait_offline
}

unregister_runner() {
  local state
  local runner_id
  local remote
  local confirmed_absent
  profile_exists || return 0
  owner_json >/dev/null
  if ! profile_running; then colima start --profile "$profile" --activate=false; fi
  if ! state=$(runner_inspect); then return 1; fi
  if test "$(jq -r '.configured' <<<"$state")" != true; then
    if test "$(jq -r '.runner_id' "$owner_path")" != null; then
      echo "local runner is unconfigured but the ownership marker retains a runner ID" >&2
      return 1
    fi
    if ! gh auth status >/dev/null 2>&1; then
      echo "GitHub authentication unavailable; retaining the proof-owned VM until remote absence is confirmed" >&2
      return 1
    fi
    remote=$(remote_by_name)
    if test "$(jq 'length' <<<"$remote")" -ne 0; then
      echo "runner name remains registered without an owned runner ID" >&2
      return 1
    fi
    return 0
  fi
  if test "$(jq -r '.exact' <<<"$state")" != true; then
    echo "local runner identity does not match the ownership contract" >&2
    return 1
  fi
  runner_id=$(jq -er '.agent_id' <<<"$state")
  if test "$runner_id" != "$(jq -r '.runner_id' "$owner_path")"; then
    echo "local runner ID does not match the durable ownership marker" >&2
    return 1
  fi
  if ! gh auth status >/dev/null 2>&1; then
    echo "GitHub authentication unavailable; runner, credentials, and VM left unchanged" >&2
    return 1
  fi
  remote=$(remote_by_id_or_absent "$runner_id")
  confirmed_absent=false
  if test "$remote" = null; then
    stop_local_runner
    confirmed_absent=true
  else
    if ! assert_remote_exact "$remote" "$runner_id"; then
      echo "remote runner identity does not match the durable ownership marker" >&2
      return 1
    fi
    if test "$(jq -r '.busy' <<<"$remote")" = true; then
      echo "runner $runner_id is busy; refusing to unregister it" >&2
      return 2
    fi
    quiesce_runner
    gh api --method DELETE "repos/$repository/actions/runners/$runner_id"
    if test "$(remote_by_id_or_absent "$runner_id")" != null; then
      echo "runner $runner_id still exists after deletion" >&2
      return 1
    fi
    confirmed_absent=true
  fi
  test "$confirmed_absent" = true
  colima ssh --profile "$profile" -- sudo bash \
    "$runner_root/validation/codestory/scripts/release-evidence/guest-runner.sh" forget \
    "$runner_root/validation/codestory/scripts/release-evidence/machine-contract.json"
  write_ownership null
}

destroy_runner() {
  profile_exists || return 0
  owner_json >/dev/null
  unregister_runner
  colima stop --profile "$profile" >/dev/null 2>&1 || true
  colima delete --profile "$profile" --force --data
}

command=${1:-}
case "$command" in
  provision)
    require_provisioning_eligibility
    ensure_base_image
    assert_not_busy
    start_profile
    assert_guest_idle
    quiesce_runner
    source_sha=$(sync_validation_source)
    model_seed=$(sync_model_seed)
    provision_guest "$source_sha" "$model_seed"
    configure_runner
    write_host_attestation
    guest_verify
    start_runner
    wait_online
    ;;
  verify)
    require_provisioning_eligibility
    verify_runner
    ;;
  start)
    require_provisioning_eligibility
    ensure_base_image
    start_profile
    assert_not_busy
    assert_guest_idle
    quiesce_runner
    write_host_attestation
    guest_verify
    start_runner
    wait_online
    ;;
  stop)
    require_lifecycle_tools
    if profile_exists; then
      owner_json >/dev/null
      if profile_running; then
        quiesce_runner
        colima stop --profile "$profile"
      else
        assert_not_busy
        wait_offline
      fi
    fi
    ;;
  unregister)
    require_lifecycle_tools
    unregister_runner
    ;;
  destroy)
    require_lifecycle_tools
    destroy_runner
    ;;
  *) usage ;;
esac
