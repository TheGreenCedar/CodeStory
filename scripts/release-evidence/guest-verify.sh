#!/usr/bin/env bash
set -euo pipefail

contract=${1:?machine contract path is required}
get() { jq -er "$1" "$contract"; }

contract_sha=$(sha256sum "$contract" | awk '{print $1}')
profile_id=$(get '.profile_id')
repository=$(get '.repository')
runner_name=$(get '.runner.name')
runner_version=$(get '.runner.version')
runner_root=$(get '.runner.root')
model_name=$(get '.assets.model.name')
model_sha=$(get '.assets.model.sha256')
drill_commit=$(get '.drill.commit')
qdrant_image=$(get '.assets.qdrant_image')
llama_image=$(get '.assets.llama_image')

mount_table=$(findmnt -rn -o TARGET,SOURCE,FSTYPE,OPTIONS | sort)
printf '%s\n' "$mount_table"
unexpected_host_mounts=$(printf '%s\n' "$mount_table" | awk '
  $3 ~ /^(virtiofs|9p|fuse[.](sshfs|lima|osxfs|grpcfuse|virtiofs))$/ { print }
  $1 == "/Users" || $1 ~ /^\/Users\// { print }
')
if test -n "$unexpected_host_mounts"; then
  echo "unexpected host-backed mount detected:" >&2
  printf '%s\n' "$unexpected_host_mounts" >&2
  exit 1
fi
for host_path in /Users /Users/albert; do
  if test -r "$host_path" || test -w "$host_path"; then
    echo "host path is visible to the runner: $host_path" >&2
    exit 1
  fi
done
running_containers=$(docker ps --format '{{.ID}} {{.Names}} {{.Image}}')
if test -n "$running_containers"; then
  echo "dedicated evidence VM has running containers:" >&2
  printf '%s\n' "$running_containers" >&2
  exit 1
fi
mountpoint -q "$runner_root"
echo "host_mounts=none host_home_visible=false"

test "$(sed -n 's/^ID=//p' /etc/os-release)" = "$(get '.guest.os_id')"
test "$(sed -n 's/^VERSION_ID="\{0,1\}\([^\"]*\)"\{0,1\}$/\1/p' /etc/os-release)" = "$(get '.guest.os_version_id')"
test "$(uname -m)" = "$(get '.guest.architecture')"
test "$(nproc)" = "$(get '.vm.cpus')"
test "$(sed -n 's/^serial: //p' /etc/cloud/build.info)" = "$(get '.vm.base_image.guest_build_serial')"

python3 - "$runner_root" <<'PY'
import os, shutil, sys
root = sys.argv[1]
memory_gib = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 2**30
disk_gib = shutil.disk_usage(root).free / 2**30
assert memory_gib >= 16, memory_gib
assert disk_gib >= 20, disk_gib
print(f"memory_gib={memory_gib:.4f} workspace_free_gib={disk_gib:.2f}")
PY

host_attestation="$runner_root/artifacts/host-attestation.json"
ownership="$runner_root/artifacts/ownership.json"
test -f "$host_attestation"
test -f "$ownership"
boot_id=$(sed -n '1p' /proc/sys/kernel/random/boot_id)
jq -e --arg profile_id "$profile_id" --arg contract_sha "$contract_sha" \
  --arg boot_id "$boot_id" --slurpfile contract "$contract" '
  .schema_version == 1 and .profile_id == $profile_id and
  .contract_sha256 == $contract_sha and .vm.boot_id == $boot_id and
  .host.architecture == $contract[0].host.architecture and
  .host.model == $contract[0].host.model and .host.chip == $contract[0].host.chip and
  .host.macos_version == $contract[0].host.macos_version and
  .host.memory_bytes >= $contract[0].host.minimum_memory_bytes and
  .host.colima_version == $contract[0].host.colima_version and
  .host.colima_git_commit == $contract[0].host.colima_git_commit and
  .host.lima_version == $contract[0].host.lima_version and
  .vm.profile == $contract[0].vm.profile and .vm.type == $contract[0].vm.type and
  .vm.architecture == $contract[0].vm.architecture and
  .vm.runtime == $contract[0].vm.runtime and
  .vm.mount_type == $contract[0].vm.mount_type and .vm.host_mounts == [] and
  .vm.cpus == $contract[0].vm.cpus and .vm.memory_gib == $contract[0].vm.memory_gib and
  .vm.data_disk_gib == $contract[0].vm.data_disk_gib and
  .vm.root_disk_gib == $contract[0].vm.root_disk_gib and
  .vm.base_image_url == $contract[0].vm.base_image.url and
  .vm.base_image_sha512 == $contract[0].vm.base_image.sha512
  ' "$host_attestation" >/dev/null

test "$(sed -n '1p' "$runner_root/artifacts/contract-sha256")" = "$contract_sha"
while IFS=$'\t' read -r package expected; do
  actual=$(dpkg-query -W -f='${Version}' "$package")
  test "$actual" = "$expected"
done < <(jq -r '.guest.apt_packages | to_entries[] | [.key,.value] | @tsv' "$contract")
current_packages=$(mktemp)
observed_identity=$(mktemp)
trap 'rm -f "$current_packages" "$observed_identity"' EXIT
dpkg-query -W -f='${binary:Package}\t${Version}\n' | sort >"$current_packages"
cmp -s "$current_packages" "$runner_root/artifacts/native-packages.tsv"
package_manifest_sha=$(sha256sum "$current_packages" | awk '{print $1}')

test "$(node --version)" = "v$(get '.guest.node.version')"
gh --version | head -1 | grep -Fq "version $(get '.guest.github_cli.version') "
rustc --version | grep -Fq "rustc $(get '.guest.rust.version') "
test "$("$runner_root/actions-runner/bin/Runner.Listener" --version)" = "$runner_version"
jq -e --arg name "$runner_name" --arg url "https://github.com/$repository" '
  .agentName == $name and .gitHubUrl == $url and .workFolder == "_work" and
  .disableUpdate == true and (.agentId | type == "number")
  ' "$runner_root/actions-runner/.runner" >/dev/null
agent_id=$(jq -er '.agentId' "$runner_root/actions-runner/.runner")
jq -e --arg profile_id "$profile_id" --arg repository "$repository" \
  --arg runner_name "$runner_name" --argjson agent_id "$agent_id" '
  .schema_version == 1 and .profile_id == $profile_id and
  .repository == $repository and .runner_name == $runner_name and
  .runner_id == $agent_id
  ' "$ownership" >/dev/null

printf '%s  %s\n' "$model_sha" "$runner_root/models/$model_name" | sha256sum -c -
test "$(git -C "$runner_root/drills/serde-json" rev-parse HEAD)" = "$drill_commit"
test -z "$(git -C "$runner_root/drills/serde-json" status --porcelain)"
jq -e '.cases[0].anchors == ["from_reader", "Deserializer::from_reader", "Value"]' \
  "$runner_root/drills/real-repo-drill-cases.json" >/dev/null
test -f "$runner_root/validation/source-sha"
test ! -e "$runner_root/validation/codestory/.git"
source_sha=$(sed -n '1p' "$runner_root/validation/source-sha")
printf '%s\n' "$source_sha" | grep -Eq '^[0-9a-f]{40}$'

qdrant_id=$(docker image inspect "$qdrant_image" --format '{{.Id}}')
llama_id=$(docker image inspect "$llama_image" --format '{{.Id}}')
test "$(docker image inspect "$qdrant_image" --format '{{.Os}}/{{.Architecture}}')" = linux/arm64
test "$(docker image inspect "$llama_image" --format '{{.Os}}/{{.Architecture}}')" = linux/arm64

memory_bytes=$(awk '/MemTotal/{printf "%.0f", $2 * 1024}' /proc/meminfo)
workspace_free_bytes=$(df --output=avail -B1 "$runner_root" | tail -1 | tr -d ' ')
model_bytes=$(stat -c %s "$runner_root/models/$model_name")
manifest_sha=$(sha256sum "$runner_root/drills/real-repo-drill-cases.json" | awk '{print $1}')
os_pretty=$(sed -n 's/^PRETTY_NAME="\(.*\)"/\1/p' /etc/os-release)
node_version=$(node --version)
cargo_version=$(cargo --version)
rustc_version=$(rustc --version)
python_version=$(python3 --version)
gh_version=$(gh --version | head -1)
docker_version=$(docker version --format '{{.Server.Version}}')
compose_version=$(docker compose version --short)
verified_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
fingerprint="$profile_id/$contract_sha"

jq -n --slurpfile host "$host_attestation" \
  --arg os "$os_pretty" --arg kernel "$(uname -r)" --arg arch "$(uname -m)" \
  --arg boot_id "$boot_id" --arg package_manifest_sha "$package_manifest_sha" \
  --arg node "$node_version" --arg cargo "$cargo_version" --arg rustc "$rustc_version" \
  --arg python "$python_version" --arg gh "$gh_version" \
  --arg docker "$docker_version" --arg compose "$compose_version" \
  '{host: $host[0].host, vm: $host[0].vm,
    guest: {os:$os,kernel:$kernel,arch:$arch,boot_id:$boot_id,
      native_package_manifest_sha256:$package_manifest_sha},
    toolchain:{node:$node,cargo:$cargo,rustc:$rustc,python:$python,
      gh:$gh,docker:$docker,docker_compose:$compose}}' >"$observed_identity"
observed_identity_sha=$(jq -cS . "$observed_identity" | sha256sum | awk '{print $1}')

jq -n --slurpfile observed "$observed_identity" \
  --arg verified_at "$verified_at" --arg profile_id "$profile_id" \
  --arg contract_sha "$contract_sha" --arg fingerprint "$fingerprint" \
  --arg observed_identity_sha "$observed_identity_sha" \
  --arg repository "$repository" --arg runner_name "$runner_name" \
  --arg runner_version "$runner_version" --argjson runner_id "$agent_id" \
  --arg model_name "$model_name" --arg model_sha "$model_sha" \
  --argjson model_bytes "$model_bytes" --arg qdrant "$qdrant_image" \
  --arg qdrant_id "$qdrant_id" --arg llama "$llama_image" --arg llama_id "$llama_id" \
  --arg drill_commit "$drill_commit" --arg manifest_sha "$manifest_sha" \
  --arg source_sha "$source_sha" --argjson memory_bytes "$memory_bytes" \
  --argjson workspace_free_bytes "$workspace_free_bytes" '
  {
    schema_version:2, verified_at:$verified_at, profile_id:$profile_id,
    contract_sha256:$contract_sha, fingerprint:$fingerprint,
    observed_identity:$observed[0], observed_identity_sha256:$observed_identity_sha,
    runner:{repository:$repository,name:$runner_name,id:$runner_id,version:$runner_version,
      labels:["self-hosted","Linux","ARM64","codestory-release-evidence"],automatic_updates:false},
    capacity:{observed_memory_bytes:$memory_bytes,observed_workspace_free_bytes:$workspace_free_bytes},
    assets:{model:{name:$model_name,sha256:$model_sha,bytes:$model_bytes},
      qdrant:{reference:$qdrant,image_id:$qdrant_id,platform:"linux/arm64"},
      llama_server:{reference:$llama,image_id:$llama_id,platform:"linux/arm64"}},
    drill:{repository:"https://github.com/serde-rs/json.git",commit:$drill_commit,
      manifest:"/srv/codestory-release-evidence/drills/real-repo-drill-cases.json",
      manifest_sha256:$manifest_sha},
    validation_source:{repository:"https://github.com/TheGreenCedar/CodeStory.git",commit:$source_sha},
    root:"/srv/codestory-release-evidence"
  }' | tee "$runner_root/artifacts/provisioning.json" >/dev/null
sha256sum "$runner_root/artifacts/provisioning.json"
printf 'machine_fingerprint=%s\n' "$fingerprint"
