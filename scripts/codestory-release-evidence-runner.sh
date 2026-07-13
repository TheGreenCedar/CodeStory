#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
profile=codestory-release-evidence
repository=TheGreenCedar/CodeStory
runner_name=codestory-release-evidence-m5-colima-arm64
runner_root=/srv/codestory-release-evidence
source_root=/var/lib/docker/codestory-release-evidence
expected_host_model=Mac17,4
expected_host_chip='Apple M5'
expected_host_os=26.5.2
expected_colima_version=0.10.3
machine_fingerprint=colima-vz0.10.3/mac17.4/apple-m5/macos26.5.2/linux-arm64/4vcpu/17GiB/no-host-mount-v1

runner_version=2.335.1
runner_sha256=6d1e85bfd1a506a8b17c1f1b9b57dba458ffed90898799aaa9f599520b0d9207
node_version=24.18.0
node_sha256=58c9520501f6ae2b52d5b210444e24b9d0c029a58c5011b797bc1fe7105886f6
gh_version=2.96.0
gh_sha256=06f86ec7103d41993b76cd78072f43595c34aaa56506d971d9860e67140bf909
rust_version=1.97.0
rustup_sha256=9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792
model_sha256=ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c
drill_commit=827a315bf2198558f0325b07bcc1e2cd973aba2f
qdrant_image='qdrant/qdrant:v1.12.5@sha256:05fecce7dce45d1254e0468bc037e8210e187fd56fa847688b012293d5f08aae'
llama_image='ghcr.io/ggml-org/llama.cpp:server@sha256:f16ca66f3ba316b7a7a16003ddfa88d29c3404fbe86550da086736864c11574c'

usage() {
  echo "usage: $0 provision|verify|start|stop|unregister|destroy" >&2
  exit 2
}

require_host_tools() {
  test "$(uname -s)" = Darwin
  test "$(uname -m)" = arm64
  command -v colima >/dev/null
  command -v gh >/dev/null
  test "$(sysctl -n hw.model)" = "$expected_host_model"
  test "$(sysctl -n machdep.cpu.brand_string)" = "$expected_host_chip"
  test "$(sw_vers -productVersion)" = "$expected_host_os"
  test "$(colima version | awk 'NR == 1 {print $3}')" = "$expected_colima_version"
  test "$(sysctl -n hw.memsize)" -ge 25769803776
  test -z "$(git -C "$repo_root" status --porcelain)"
  gh auth status >/dev/null
}

vm() {
  colima ssh --profile "$profile" -- "$@"
}

start_profile() {
  if colima status --profile "$profile" >/dev/null 2>&1; then
    return
  fi
  colima start --profile "$profile" \
    --cpu 4 --memory 17 --disk 80 \
    --runtime docker --vm-type vz --mount-type virtiofs --mount none
}

provision_guest() {
  colima ssh --profile "$profile" -- env \
    RUNNER_ROOT="$runner_root" SOURCE_ROOT="$source_root" \
    RUNNER_VERSION="$runner_version" RUNNER_SHA256="$runner_sha256" \
    NODE_VERSION="$node_version" NODE_SHA256="$node_sha256" \
    GH_VERSION="$gh_version" GH_SHA256="$gh_sha256" \
    RUST_VERSION="$rust_version" RUSTUP_SHA256="$rustup_sha256" \
    MODEL_SHA256="$model_sha256" DRILL_COMMIT="$drill_commit" \
    QDRANT_IMAGE="$qdrant_image" LLAMA_IMAGE="$llama_image" \
    bash -s <<'GUEST'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  ca-certificates curl git jq build-essential pkg-config libssl-dev \
  cmake clang libclang-dev libsqlite3-dev python3 python3-venv \
  tar xz-utils unzip zstd lsof procps

if ! id codestory-runner >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir "$RUNNER_ROOT/home" \
    --shell /bin/bash codestory-runner
fi
sudo usermod -aG docker -d "$RUNNER_ROOT/home" codestory-runner
sudo install -d -m 0750 -o root -g codestory-runner "$SOURCE_ROOT"
sudo install -d -m 0755 "$RUNNER_ROOT"
if ! mountpoint -q "$RUNNER_ROOT"; then
  sudo mount --bind "$SOURCE_ROOT" "$RUNNER_ROOT"
fi
fstab_line="$SOURCE_ROOT $RUNNER_ROOT none bind,nofail 0 0"
if ! grep -Fqx "$fstab_line" /etc/fstab; then
  printf '%s\n' "$fstab_line" | sudo tee -a /etc/fstab >/dev/null
fi
sudo install -d -o codestory-runner -g codestory-runner -m 0750 \
  "$RUNNER_ROOT/actions-runner" "$RUNNER_ROOT/cache" \
  "$RUNNER_ROOT/cargo" "$RUNNER_ROOT/rustup" "$RUNNER_ROOT/sccache" \
  "$RUNNER_ROOT/home" "$RUNNER_ROOT/tmp" "$RUNNER_ROOT/models" "$RUNNER_ROOT/drills" \
  "$RUNNER_ROOT/artifacts" "$RUNNER_ROOT/validation"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

node_archive="node-v${NODE_VERSION}-linux-arm64.tar.xz"
curl -fsSLo "$tmp/$node_archive" \
  "https://nodejs.org/dist/v${NODE_VERSION}/$node_archive"
printf '%s  %s\n' "$NODE_SHA256" "$tmp/$node_archive" | sha256sum -c -
sudo rm -rf "/opt/node-v${NODE_VERSION}-linux-arm64"
sudo tar -xJf "$tmp/$node_archive" -C /opt
for command in node npm npx; do
  sudo ln -sfn "/opt/node-v${NODE_VERSION}-linux-arm64/bin/$command" \
    "/usr/local/bin/$command"
done

gh_archive="gh_${GH_VERSION}_linux_arm64.tar.gz"
curl -fsSLo "$tmp/$gh_archive" \
  "https://github.com/cli/cli/releases/download/v${GH_VERSION}/$gh_archive"
printf '%s  %s\n' "$GH_SHA256" "$tmp/$gh_archive" | sha256sum -c -
sudo rm -rf "/opt/gh_${GH_VERSION}_linux_arm64"
sudo tar -xzf "$tmp/$gh_archive" -C /opt
sudo ln -sfn "/opt/gh_${GH_VERSION}_linux_arm64/bin/gh" /usr/local/bin/gh

if ! sudo -u codestory-runner env \
    HOME="$RUNNER_ROOT/home" CARGO_HOME="$RUNNER_ROOT/cargo" \
    RUSTUP_HOME="$RUNNER_ROOT/rustup" \
    "$RUNNER_ROOT/cargo/bin/rustc" --version \
    | grep -Fq "rustc $RUST_VERSION "; then
  curl -fsSLo "$tmp/rustup-init" \
    https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init
  printf '%s  %s\n' "$RUSTUP_SHA256" "$tmp/rustup-init" | sha256sum -c -
  sudo install -o codestory-runner -g codestory-runner -m 0755 \
    "$tmp/rustup-init" "$RUNNER_ROOT/rustup-init"
  sudo -u codestory-runner env \
    HOME="$RUNNER_ROOT/home" CARGO_HOME="$RUNNER_ROOT/cargo" \
    RUSTUP_HOME="$RUNNER_ROOT/rustup" \
    "$RUNNER_ROOT/rustup-init" -y --profile minimal \
    --default-toolchain "$RUST_VERSION"
  sudo rm -f "$RUNNER_ROOT/rustup-init"
fi
for command in cargo rustc rustup; do
  sudo ln -sfn "$RUNNER_ROOT/cargo/bin/$command" "/usr/local/bin/$command"
done

runner_archive="actions-runner-linux-arm64-${RUNNER_VERSION}.tar.gz"
curl -fsSLo "$tmp/$runner_archive" \
  "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/$runner_archive"
printf '%s  %s\n' "$RUNNER_SHA256" "$tmp/$runner_archive" | sha256sum -c -
if [ ! -x "$RUNNER_ROOT/actions-runner/bin/Runner.Listener" ]; then
  sudo tar -xzf "$tmp/$runner_archive" -C "$RUNNER_ROOT/actions-runner"
  sudo chown -R codestory-runner:codestory-runner \
    "$RUNNER_ROOT/actions-runner"
fi
sudo "$RUNNER_ROOT/actions-runner/bin/installdependencies.sh"

model="$RUNNER_ROOT/models/bge-base-en-v1.5.Q8_0.gguf"
if ! sudo -u codestory-runner test -f "$model" \
    || ! printf '%s  %s\n' "$MODEL_SHA256" "$model" \
    | sudo -u codestory-runner sha256sum -c -; then
  sudo rm -f "$model.partial"
  downloaded=false
  for url in \
    https://huggingface.co/BAAI/bge-base-en-v1.5-GGUF/resolve/main/bge-base-en-v1.5.Q8_0.gguf \
    https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q8_0.gguf; do
    if sudo -u codestory-runner curl -fL --retry 3 -o "$model.partial" "$url"; then
      downloaded=true
      break
    fi
  done
  test "$downloaded" = true
  printf '%s  %s\n' "$MODEL_SHA256" "$model.partial" \
    | sudo -u codestory-runner sha256sum -c -
  sudo -u codestory-runner mv "$model.partial" "$model"
fi

drill_repo="$RUNNER_ROOT/drills/serde-json"
if ! sudo -u codestory-runner test -d "$drill_repo/.git"; then
  sudo -u codestory-runner git clone --filter=blob:none --no-checkout \
    https://github.com/serde-rs/json.git "$drill_repo"
fi
sudo -u codestory-runner git -C "$drill_repo" fetch --depth 1 origin "$DRILL_COMMIT"
sudo -u codestory-runner git -C "$drill_repo" checkout --detach "$DRILL_COMMIT"
test "$(sudo -u codestory-runner git -C "$drill_repo" rev-parse HEAD)" = "$DRILL_COMMIT"
test -z "$(sudo -u codestory-runner git -C "$drill_repo" status --porcelain)"

manifest="$RUNNER_ROOT/drills/real-repo-drill-cases.json"
sudo -u codestory-runner jq -n --arg project "$drill_repo" '{
  suite: "codestory-v0.15-release-evidence",
  cases: [{
    slug: "serde-json-deserialization",
    project: $project,
    question: "Explain how serde_json turns an input reader into typed Rust values, from from_reader through Deserializer and the Value representation.",
    anchors: ["from_reader", "Deserializer", "Value"],
    expect: {
      source_truth_files: ["src/de.rs", "src/value/mod.rs"],
      false_claims: ["from_reader parses JSON without using Deserializer"],
      min_anchor_resolution: 3,
      allow_partial_bridges: true
    }
  }]
}' | sudo -u codestory-runner tee "$manifest" >/dev/null

sudo -u codestory-runner docker pull "$QDRANT_IMAGE"
sudo -u codestory-runner docker pull "$LLAMA_IMAGE"
GUEST
}

sync_validation_source() {
  local source_sha
  source_sha=${CODESTORY_RELEASE_EVIDENCE_SHA:-$(git -C "$repo_root" rev-parse HEAD)}
  vm sudo rm -rf "$runner_root/validation/codestory"
  vm sudo install -d -o codestory-runner -g codestory-runner -m 0750 \
    "$runner_root/validation/codestory"
  git -C "$repo_root" archive --format=tar "$source_sha" \
    | colima ssh --profile "$profile" -- sudo -u codestory-runner \
      tar -xf - -C "$runner_root/validation/codestory"
  printf '%s\n' "$source_sha" \
    | colima ssh --profile "$profile" -- sudo -u codestory-runner \
      tee "$runner_root/validation/source-sha" >/dev/null
  vm sudo -u codestory-runner env \
    HOME="$runner_root/home" CARGO_HOME="$runner_root/cargo" \
    RUSTUP_HOME="$runner_root/rustup" XDG_CACHE_HOME="$runner_root/cache/xdg" \
    CODESTORY_EMBED_MODEL_DIR="$runner_root/models" \
    node "$runner_root/validation/codestory/scripts/setup-retrieval-env.mjs" \
    --check-only
}

register_runner() {
  local token
  token=$(gh api --method POST "repos/$repository/actions/runners/registration-token" --jq .token)
  colima ssh --profile "$profile" -- env \
    REGISTRATION_TOKEN="$token" RUNNER_ROOT="$runner_root" \
    REPOSITORY="$repository" RUNNER_NAME="$runner_name" \
    MACHINE_FINGERPRINT="$machine_fingerprint" \
    bash -s <<'REGISTER'
set -euo pipefail
runner="$RUNNER_ROOT/actions-runner"
if ! sudo -u codestory-runner test -f "$runner/.runner"; then
  sudo -u codestory-runner env REGISTRATION_TOKEN="$REGISTRATION_TOKEN" bash -c '
    cd "$1"
    ./config.sh --unattended --url "https://github.com/$2" \
      --token "$REGISTRATION_TOKEN" --name "$3" \
      --labels codestory-release-evidence --work _work \
      --replace --disableupdate
  ' _ "$runner" "$REPOSITORY" "$RUNNER_NAME"
fi

service="actions.runner.${REPOSITORY//\//-}.${RUNNER_NAME}.service"
dropin="/etc/systemd/system/$service.d"
sudo install -d -m 0755 "$dropin"
printf '%s\n' \
  '[Service]' 'UMask=0077' \
  "Environment=HOME=$RUNNER_ROOT/home" \
  "Environment=CARGO_HOME=$RUNNER_ROOT/cargo" \
  "Environment=RUSTUP_HOME=$RUNNER_ROOT/rustup" \
  "Environment=SCCACHE_DIR=$RUNNER_ROOT/sccache" \
  "Environment=TMPDIR=$RUNNER_ROOT/tmp" \
  "Environment=XDG_CACHE_HOME=$RUNNER_ROOT/cache/xdg" \
  "Environment=CODESTORY_CACHE_DIR=$RUNNER_ROOT/cache/codestory" \
  "Environment=CODESTORY_EMBED_MODEL_DIR=$RUNNER_ROOT/models" \
  "Environment=CODESTORY_REAL_REPO_DRILL_CASES=$RUNNER_ROOT/drills/real-repo-drill-cases.json" \
  "Environment=CODESTORY_RELEASE_EVIDENCE_MACHINE_FINGERPRINT=$MACHINE_FINGERPRINT" \
  | sudo tee "$dropin/override.conf" >/dev/null

if ! systemctl list-unit-files "$service" --no-legend | grep -q "$service"; then
  sudo bash -c 'cd "$1" && ./svc.sh install codestory-runner' _ "$runner"
fi
sudo systemctl daemon-reload
sudo systemctl enable "$service"
sudo systemctl restart "$service"
REGISTER
  unset token
}

verify_runner() {
  local runner_json
  runner_json=
  for _ in {1..15}; do
    runner_json=$(gh api "repos/$repository/actions/runners" --jq \
      ".runners[] | select(.name == \"$runner_name\") | select(.status == \"online\") | select(.version == \"$runner_version\") | select(([\"self-hosted\",\"Linux\",\"ARM64\",\"codestory-release-evidence\"] - [.labels[].name]) | length == 0) | {id,name,os,status,busy,version,labels:[.labels[].name]}"
    )
    test -n "$runner_json" && break
    sleep 1
  done
  test -n "$runner_json"
  printf '%s\n' "$runner_json"
  colima ssh --profile "$profile" -- env \
    RUNNER_ROOT="$runner_root" MODEL_SHA256="$model_sha256" \
    DRILL_COMMIT="$drill_commit" QDRANT_IMAGE="$qdrant_image" \
    LLAMA_IMAGE="$llama_image" MACHINE_FINGERPRINT="$machine_fingerprint" \
    RUNNER_NAME="$runner_name" RUNNER_VERSION="$runner_version" \
    bash -s <<'VERIFY'
set -euo pipefail
mount_table=$(findmnt -rn -o TARGET,SOURCE,FSTYPE,OPTIONS | sort)
printf '%s\n' "$mount_table"
unexpected_host_mounts=$(printf '%s\n' "$mount_table" | awk '
  $3 ~ /^(virtiofs|9p|fuse[.](sshfs|lima|osxfs|grpcfuse|virtiofs))$/ { print }
  $1 == "/Users" || $1 ~ /^\/Users\// { print }
')
if [ -n "$unexpected_host_mounts" ]; then
  echo "unexpected host-backed mount detected:" >&2
  printf '%s\n' "$unexpected_host_mounts" >&2
  exit 1
fi
for host_path in /Users /Users/albert; do
  if sudo -u codestory-runner test -e "$host_path" \
      || sudo -u codestory-runner test -r "$host_path" \
      || sudo -u codestory-runner test -w "$host_path"; then
    echo "host path is visible to the runner: $host_path" >&2
    exit 1
  fi
done
echo "host_mounts=none host_home_visible=false"

sudo -u codestory-runner python3 - <<'PY'
import os, shutil
root = "/srv/codestory-release-evidence"
memory_gib = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 2**30
disk_gib = shutil.disk_usage(f"{root}/actions-runner/_work").free / 2**30
assert memory_gib >= 16, memory_gib
assert disk_gib >= 20, disk_gib
print(f"memory_gib={memory_gib:.4f} workspace_free_gib={disk_gib:.2f}")
PY
printf '%s  %s\n' "$MODEL_SHA256" \
  "$RUNNER_ROOT/models/bge-base-en-v1.5.Q8_0.gguf" \
  | sudo -u codestory-runner sha256sum -c -
test "$(sudo -u codestory-runner git -C "$RUNNER_ROOT/drills/serde-json" rev-parse HEAD)" = "$DRILL_COMMIT"
test -z "$(sudo -u codestory-runner git -C "$RUNNER_ROOT/drills/serde-json" status --porcelain)"
sudo -u codestory-runner jq -e \
  '.cases[0].anchors == ["from_reader", "Deserializer", "Value"]' \
  "$RUNNER_ROOT/drills/real-repo-drill-cases.json" >/dev/null
sudo -u codestory-runner docker image inspect "$QDRANT_IMAGE" \
  --format 'qdrant={{.Os}}/{{.Architecture}} {{.Id}}'
sudo -u codestory-runner docker image inspect "$LLAMA_IMAGE" \
  --format 'llama={{.Os}}/{{.Architecture}} {{.Id}}'
actual=$(sudo -u codestory-runner env \
  CODESTORY_RELEASE_EVIDENCE_MACHINE_FINGERPRINT="$MACHINE_FINGERPRINT" \
  node "$RUNNER_ROOT/validation/codestory/scripts/codestory-release-evidence-gate.mjs" fingerprint)
test "$actual" = "$MACHINE_FINGERPRINT"
echo "machine_fingerprint=$actual"

artifact="$RUNNER_ROOT/artifacts/provisioning.json"
memory_bytes=$(awk '/MemTotal/{printf "%.0f", $2 * 1024}' /proc/meminfo)
workspace_free_bytes=$(sudo -u codestory-runner df --output=avail -B1 \
  "$RUNNER_ROOT/actions-runner/_work" | tail -1 | tr -d ' ')
model_bytes=$(sudo -u codestory-runner stat -c %s \
  "$RUNNER_ROOT/models/bge-base-en-v1.5.Q8_0.gguf")
manifest_sha=$(sudo -u codestory-runner sha256sum \
  "$RUNNER_ROOT/drills/real-repo-drill-cases.json" | awk '{print $1}')
source_sha=$(sudo -u codestory-runner sed -n '1p' \
  "$RUNNER_ROOT/validation/source-sha")
qdrant_id=$(sudo -u codestory-runner docker image inspect "$QDRANT_IMAGE" \
  --format '{{.Id}}')
llama_id=$(sudo -u codestory-runner docker image inspect "$LLAMA_IMAGE" \
  --format '{{.Id}}')
os_pretty=$(sed -n 's/^PRETTY_NAME="\(.*\)"/\1/p' /etc/os-release)
node_version=$(node --version)
cargo_version=$(sudo -u codestory-runner env \
  HOME="$RUNNER_ROOT/home" CARGO_HOME="$RUNNER_ROOT/cargo" \
  RUSTUP_HOME="$RUNNER_ROOT/rustup" /usr/local/bin/cargo --version)
rustc_version=$(sudo -u codestory-runner env \
  HOME="$RUNNER_ROOT/home" CARGO_HOME="$RUNNER_ROOT/cargo" \
  RUSTUP_HOME="$RUNNER_ROOT/rustup" /usr/local/bin/rustc --version)
python_version=$(python3 --version)
gh_version=$(gh --version | head -1)
docker_version=$(docker version --format '{{.Server.Version}}')
compose_version=$(docker compose version --short)
verified_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

sudo -u codestory-runner jq -n \
  --arg verified_at "$verified_at" \
  --arg runner_name "$RUNNER_NAME" \
  --arg runner_version "$RUNNER_VERSION" \
  --arg machine_fingerprint "$MACHINE_FINGERPRINT" \
  --arg os "$os_pretty" --arg kernel "$(uname -r)" --arg arch "$(uname -m)" \
  --arg node "$node_version" --arg cargo "$cargo_version" \
  --arg rustc "$rustc_version" --arg python "$python_version" \
  --arg gh "$gh_version" --arg docker "$docker_version" \
  --arg compose "$compose_version" \
  --arg model_sha256 "$MODEL_SHA256" --argjson model_bytes "$model_bytes" \
  --arg qdrant "$QDRANT_IMAGE" --arg qdrant_id "$qdrant_id" \
  --arg llama "$LLAMA_IMAGE" --arg llama_id "$llama_id" \
  --arg drill_commit "$DRILL_COMMIT" --arg drill_manifest_sha256 "$manifest_sha" \
  --arg source_sha "$source_sha" \
  --argjson memory_bytes "$memory_bytes" \
  --argjson workspace_free_bytes "$workspace_free_bytes" \
  '{
    schema_version: 1,
    verified_at: $verified_at,
    runner: {
      repository: "TheGreenCedar/CodeStory",
      name: $runner_name,
      version: $runner_version,
      labels: ["self-hosted", "Linux", "ARM64", "codestory-release-evidence"],
      automatic_updates: false
    },
    machine: {
      fingerprint: $machine_fingerprint,
      os: $os,
      kernel: $kernel,
      arch: $arch,
      vcpus: 4,
      configured_memory_gib: 17,
      configured_disk_gib: 80,
      observed_memory_bytes: $memory_bytes,
      observed_workspace_free_bytes: $workspace_free_bytes,
      host_mounts: [],
      host_home_visible: false
    },
    toolchain: {
      node: $node,
      cargo: $cargo,
      rustc: $rustc,
      python: $python,
      gh: $gh,
      docker: $docker,
      docker_compose: $compose
    },
    assets: {
      model: {name: "bge-base-en-v1.5.Q8_0.gguf", sha256: $model_sha256, bytes: $model_bytes},
      qdrant: {reference: $qdrant, image_id: $qdrant_id, platform: "linux/arm64"},
      llama_server: {reference: $llama, image_id: $llama_id, platform: "linux/arm64"}
    },
    drill: {
      repository: "https://github.com/serde-rs/json.git",
      commit: $drill_commit,
      manifest: "/srv/codestory-release-evidence/drills/real-repo-drill-cases.json",
      manifest_sha256: $drill_manifest_sha256
    },
    validation_source: {repository: "https://github.com/TheGreenCedar/CodeStory.git", commit: $source_sha},
    root: "/srv/codestory-release-evidence"
  }' | sudo -u codestory-runner tee "$artifact" >/dev/null
sudo -u codestory-runner sha256sum "$artifact"
VERIFY
}

unregister_runner() {
  local token
  token=$(gh api --method POST "repos/$repository/actions/runners/remove-token" --jq .token)
  colima ssh --profile "$profile" -- env \
    REMOVE_TOKEN="$token" RUNNER_ROOT="$runner_root" \
    REPOSITORY="$repository" RUNNER_NAME="$runner_name" \
    bash -s <<'REMOVE'
set -euo pipefail
runner="$RUNNER_ROOT/actions-runner"
service="actions.runner.${REPOSITORY//\//-}.${RUNNER_NAME}.service"
if systemctl list-unit-files "$service" --no-legend | grep -q "$service"; then
  sudo systemctl stop "$service"
  sudo bash -c 'cd "$1" && ./svc.sh uninstall' _ "$runner"
fi
if sudo -u codestory-runner test -f "$runner/.runner"; then
  sudo -u codestory-runner env REMOVE_TOKEN="$REMOVE_TOKEN" bash -c '
    cd "$1" && ./config.sh remove --unattended --token "$REMOVE_TOKEN"
  ' _ "$runner"
fi
REMOVE
  unset token
}

command=${1:-}
case "$command" in
  provision)
    require_host_tools
    start_profile
    provision_guest
    sync_validation_source
    register_runner
    verify_runner
    ;;
  verify)
    require_host_tools
    verify_runner
    ;;
  start)
    require_host_tools
    start_profile
    ;;
  stop)
    require_host_tools
    colima stop --profile "$profile"
    ;;
  unregister)
    require_host_tools
    unregister_runner
    ;;
  destroy)
    require_host_tools
    unregister_runner
    colima delete --profile "$profile" --force
    ;;
  *) usage ;;
esac
