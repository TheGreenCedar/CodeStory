#!/usr/bin/env bash
set -euo pipefail

contract=${1:?machine contract path is required}
expected_contract_sha=${2:?machine contract checksum is required}
source_sha=${3:?validation source SHA is required}
bootstrap_snapshot=${4:?APT snapshot is required}
bootstrap_jq_version=${5:?jq version is required}
model_seed=${6:--}
test "$model_seed" = - || test "$model_seed" = /tmp/codestory-release-evidence-model-seed

actual_contract_sha=$(sha256sum "$contract" | awk '{print $1}')
test "$actual_contract_sha" = "$expected_contract_sha"

if test -e /Users; then
  test ! -L /Users
  test "$(findmnt -rn -o TARGET -T /Users)" = /
  sudo chmod 000 /Users
fi

export DEBIAN_FRONTEND=noninteractive
printf '%s\n' \
  "APT::Snapshot \"$bootstrap_snapshot\";" \
  'Acquire::Snapshots::URI::Host::ports.ubuntu.com "https://snapshot.ubuntu.com/ubuntu/@SNAPSHOTID@/";' \
  | sudo tee /etc/apt/apt.conf.d/50codestory-snapshot >/dev/null
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  "jq=$bootstrap_jq_version"

get() { jq -er "$1" "$contract"; }
runner_root=$(get '.runner.root')
data_root=$(get '.runner.data_root')
runner_version=$(get '.runner.version')
runner_sha=$(get '.runner.sha256')
node_version=$(get '.guest.node.version')
node_sha=$(get '.guest.node.sha256')
gh_version=$(get '.guest.github_cli.version')
gh_sha=$(get '.guest.github_cli.sha256')
rust_version=$(get '.guest.rust.version')
rustup_sha=$(get '.guest.rust.rustup_init_sha256')
snapshot=$(get '.guest.apt_snapshot')
model_name=$(get '.assets.model.name')
model_sha=$(get '.assets.model.sha256')
drill_repository=$(get '.drill.repository')
drill_commit=$(get '.drill.commit')
qdrant_image=$(get '.assets.qdrant_image')
llama_image=$(get '.assets.llama_image')

test "$snapshot" = "$bootstrap_snapshot"
test "$(get '.guest.apt_packages.jq')" = "$bootstrap_jq_version"
sudo apt-get update
mapfile -t packages < <(jq -r '.guest.apt_packages | to_entries[] | "\(.key)=\(.value)"' "$contract")
sudo apt-get install -y --no-install-recommends "${packages[@]}"
mapfile -t package_names < <(jq -r '.guest.apt_packages | keys[]' "$contract")
sudo apt-mark hold "${package_names[@]}" >/dev/null
for package in "${package_names[@]}"; do
  expected=$(jq -er --arg package "$package" '.guest.apt_packages[$package]' "$contract")
  actual=$(dpkg-query -W -f='${Version}' "$package")
  test "$actual" = "$expected"
done

if ! id codestory-runner >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir "$runner_root/home" \
    --shell /bin/bash codestory-runner
fi
sudo usermod -aG docker -d "$runner_root/home" codestory-runner
sudo install -d -m 0750 -o root -g codestory-runner "$data_root"
sudo install -d -m 0755 "$runner_root"
if ! mountpoint -q "$runner_root"; then
  sudo mount --bind "$data_root" "$runner_root"
fi
fstab_line="$data_root $runner_root none bind,nofail 0 0"
if ! grep -Fqx "$fstab_line" /etc/fstab; then
  printf '%s\n' "$fstab_line" | sudo tee -a /etc/fstab >/dev/null
fi
sudo install -d -o codestory-runner -g codestory-runner -m 0750 \
  "$runner_root/actions-runner" "$runner_root/cache" \
  "$runner_root/cargo" "$runner_root/rustup" "$runner_root/sccache" \
  "$runner_root/home" "$runner_root/tmp" "$runner_root/models" \
  "$runner_root/drills" "$runner_root/artifacts" "$runner_root/validation"
sudo rm -rf "$runner_root/validation/codestory"
sudo install -d -o codestory-runner -g codestory-runner -m 0750 \
  "$runner_root/validation/codestory"
sudo cp -a "$(dirname "$contract")/../.."/. "$runner_root/validation/codestory/"
sudo chown -R codestory-runner:codestory-runner "$runner_root/validation/codestory"
printf '%s\n' "$source_sha" \
  | sudo -u codestory-runner tee "$runner_root/validation/source-sha" >/dev/null

tmp=$(mktemp -d)
cleanup() {
  rm -rf "$tmp"
  if test "$model_seed" != -; then sudo rm -f "$model_seed"; fi
}
trap cleanup EXIT

if ! command -v node >/dev/null || test "$(node --version)" != "v$node_version"; then
  node_archive="node-v${node_version}-linux-arm64.tar.xz"
  curl -fsSLo "$tmp/$node_archive" \
    "https://nodejs.org/dist/v${node_version}/$node_archive"
  printf '%s  %s\n' "$node_sha" "$tmp/$node_archive" | sha256sum -c -
  sudo rm -rf "/opt/node-v${node_version}-linux-arm64"
  sudo tar -xJf "$tmp/$node_archive" -C /opt
fi
for command in node npm npx; do
  sudo ln -sfn "/opt/node-v${node_version}-linux-arm64/bin/$command" "/usr/local/bin/$command"
done
test "$(node --version)" = "v$node_version"

if ! command -v gh >/dev/null || ! gh --version | head -1 | grep -Fq "version $gh_version "; then
  gh_archive="gh_${gh_version}_linux_arm64.tar.gz"
  curl -fsSLo "$tmp/$gh_archive" \
    "https://github.com/cli/cli/releases/download/v${gh_version}/$gh_archive"
  printf '%s  %s\n' "$gh_sha" "$tmp/$gh_archive" | sha256sum -c -
  sudo rm -rf "/opt/gh_${gh_version}_linux_arm64"
  sudo tar -xzf "$tmp/$gh_archive" -C /opt
fi
sudo ln -sfn "/opt/gh_${gh_version}_linux_arm64/bin/gh" /usr/local/bin/gh
gh --version | head -1 | grep -Fq "version $gh_version "

if ! sudo -u codestory-runner env \
    HOME="$runner_root/home" CARGO_HOME="$runner_root/cargo" \
    RUSTUP_HOME="$runner_root/rustup" \
    "$runner_root/cargo/bin/rustc" --version 2>/dev/null \
    | grep -Fq "rustc $rust_version "; then
  curl -fsSLo "$tmp/rustup-init" \
    https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init
  printf '%s  %s\n' "$rustup_sha" "$tmp/rustup-init" | sha256sum -c -
  sudo install -o codestory-runner -g codestory-runner -m 0755 \
    "$tmp/rustup-init" "$runner_root/rustup-init"
  sudo -u codestory-runner env \
    HOME="$runner_root/home" CARGO_HOME="$runner_root/cargo" \
    RUSTUP_HOME="$runner_root/rustup" \
    "$runner_root/rustup-init" -y --profile minimal --default-toolchain "$rust_version"
  sudo rm -f "$runner_root/rustup-init"
fi
for command in cargo rustc rustup; do
  sudo ln -sfn "$runner_root/cargo/bin/$command" "/usr/local/bin/$command"
done

runner="$runner_root/actions-runner"
installed_runner_version=
if sudo test -x "$runner/bin/Runner.Listener"; then
  installed_runner_version=$(sudo -u codestory-runner "$runner/bin/Runner.Listener" --version)
fi
if test -n "$installed_runner_version" && test "$installed_runner_version" != "$runner_version"; then
  if sudo test -f "$runner/.runner"; then
    echo "configured runner version $installed_runner_version does not match $runner_version" >&2
    exit 1
  fi
  sudo rm -rf "$runner"
  sudo install -d -o codestory-runner -g codestory-runner -m 0750 "$runner"
  installed_runner_version=
fi
if test -z "$installed_runner_version"; then
  runner_archive="actions-runner-linux-arm64-${runner_version}.tar.gz"
  curl -fsSLo "$tmp/$runner_archive" \
    "https://github.com/actions/runner/releases/download/v${runner_version}/$runner_archive"
  printf '%s  %s\n' "$runner_sha" "$tmp/$runner_archive" | sha256sum -c -
  sudo tar -xzf "$tmp/$runner_archive" -C "$runner"
  sudo chown -R codestory-runner:codestory-runner "$runner"
fi
test "$(sudo -u codestory-runner "$runner/bin/Runner.Listener" --version)" = "$runner_version"

model="$runner_root/models/$model_name"
if ! sudo -u codestory-runner test -f "$model" \
    || ! printf '%s  %s\n' "$model_sha" "$model" | sudo -u codestory-runner sha256sum -c -; then
  sudo rm -f "$model.partial"
  if test "$model_seed" != - && test -f "$model_seed"; then
    sudo install -o codestory-runner -g codestory-runner -m 0640 \
      "$model_seed" "$model.partial"
  else
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
  fi
  printf '%s  %s\n' "$model_sha" "$model.partial" | sudo -u codestory-runner sha256sum -c -
  sudo -u codestory-runner mv "$model.partial" "$model"
fi

drill_repo="$runner_root/drills/serde-json"
if ! sudo -u codestory-runner test -d "$drill_repo/.git"; then
  sudo -u codestory-runner git clone --filter=blob:none --no-checkout \
    "$drill_repository" "$drill_repo"
fi
sudo -u codestory-runner git -C "$drill_repo" fetch --depth 1 origin "$drill_commit"
sudo -u codestory-runner git -C "$drill_repo" checkout --detach --force "$drill_commit"
sudo -u codestory-runner git -C "$drill_repo" reset --hard "$drill_commit"
sudo -u codestory-runner git -C "$drill_repo" clean -ffdqx
test "$(sudo -u codestory-runner git -C "$drill_repo" rev-parse HEAD)" = "$drill_commit"
test -z "$(sudo -u codestory-runner git -C "$drill_repo" status --porcelain)"

manifest="$runner_root/drills/real-repo-drill-cases.json"
sudo -u codestory-runner jq -n --arg project "$drill_repo" '{
  suite: "codestory-v0.15-release-evidence",
  cases: [{
    slug: "serde-json-deserialization",
    project: $project,
    question: "Explain how serde_json turns an input reader into typed Rust values, from from_reader through Deserializer and the Value representation.",
    anchors: ["from_reader", "Deserializer::from_reader", "Value"],
    expect: {
      source_truth_files: ["src/de.rs", "src/value/mod.rs"],
      false_claims: ["from_reader parses JSON without using Deserializer"],
      min_anchor_resolution: 3,
      allow_partial_bridges: true
    }
  }]
}' | sudo -u codestory-runner tee "$manifest" >/dev/null

sudo -u codestory-runner docker pull "$qdrant_image"
sudo -u codestory-runner docker pull "$llama_image"

test "$(sudo -u codestory-runner sed -n '1p' "$runner_root/validation/source-sha")" = "$source_sha"
sudo -u codestory-runner env \
  HOME="$runner_root/home" CARGO_HOME="$runner_root/cargo" \
  RUSTUP_HOME="$runner_root/rustup" XDG_CACHE_HOME="$runner_root/cache/xdg" \
  CODESTORY_EMBED_MODEL_DIR="$runner_root/models" \
  node "$runner_root/validation/codestory/scripts/setup-retrieval-env.mjs" --check-only

dpkg-query -W -f='${binary:Package}\t${Version}\n' | sort \
  | sudo -u codestory-runner tee "$runner_root/artifacts/native-packages.tsv" >/dev/null
printf '%s\n' "$expected_contract_sha" \
  | sudo -u codestory-runner tee "$runner_root/artifacts/contract-sha256" >/dev/null
