#!/usr/bin/env bash
set -euo pipefail

command=${1:-}
contract=${2:?machine contract path is required}
get() { jq -er "$1" "$contract"; }

repository=$(get '.repository')
runner_name=$(get '.runner.name')
runner_version=$(get '.runner.version')
profile_id=$(get '.profile_id')
runner_root=$(get '.runner.root')
runner="$runner_root/actions-runner"
registration="$runner/.runner"

service_name() {
  if test -f "$runner/.service"; then
    sed -n '1p' "$runner/.service"
  else
    printf 'actions.runner.%s.%s.service\n' "${repository//\//-}" "$runner_name"
  fi
}

configured=false
exact=false
agent_id=null
if sudo -u codestory-runner test -f "$registration"; then
  configured=true
  agent_id=$(sudo -u codestory-runner jq -er '.agentId' "$registration")
  if sudo -u codestory-runner jq -e \
      --arg name "$runner_name" --arg url "https://github.com/$repository" \
      '.agentName == $name and .gitHubUrl == $url and .workFolder == "_work" and .disableUpdate == true' \
      "$registration" >/dev/null; then
    exact=true
  fi
fi

binary_version=
if sudo -u codestory-runner test -x "$runner/bin/Runner.Listener"; then
  binary_version=$(sudo -u codestory-runner "$runner/bin/Runner.Listener" --version)
fi
service=$(service_name)
service_installed=false
service_active=false
if systemctl list-unit-files "$service" --no-legend 2>/dev/null | grep -Fq "$service"; then
  service_installed=true
  systemctl is-active --quiet "$service" && service_active=true
fi

inspect() {
  jq -n \
    --argjson configured "$configured" --argjson exact "$exact" \
    --argjson agent_id "$agent_id" --arg binary_version "$binary_version" \
    --arg expected_version "$runner_version" --arg service "$service" \
    --argjson service_installed "$service_installed" \
    --argjson service_active "$service_active" \
    '{configured:$configured, exact:$exact, agent_id:$agent_id,
      binary_version:$binary_version, expected_version:$expected_version,
      service:$service, service_installed:$service_installed,
      service_active:$service_active}'
}

install_service() {
  test "$binary_version" = "$runner_version"
  if test "$configured" != true; then
    IFS= read -r token
    test -n "$token"
    sudo -u codestory-runner bash -c '
      cd "$1"
      ./config.sh --unattended --url "https://github.com/$2" \
        --token "$3" --name "$4" --labels codestory-release-evidence \
        --work _work --disableupdate
    ' _ "$runner" "$repository" "$token" "$runner_name"
    unset token
    configured=true
    exact=true
  elif test "$exact" != true; then
    echo "configured runner identity does not match the machine contract" >&2
    exit 1
  fi

  service=$(service_name)
  if ! systemctl list-unit-files "$service" --no-legend 2>/dev/null | grep -Fq "$service"; then
    sudo bash -c 'cd "$1" && ./svc.sh install codestory-runner' _ "$runner"
  fi
  dropin="/etc/systemd/system/$service.d"
  sudo install -d -m 0755 "$dropin"
  printf '%s\n' \
    '[Service]' 'UMask=0077' \
    "Environment=HOME=$runner_root/home" \
    "Environment=CARGO_HOME=$runner_root/cargo" \
    "Environment=RUSTUP_HOME=$runner_root/rustup" \
    "Environment=SCCACHE_DIR=$runner_root/sccache" \
    "Environment=TMPDIR=$runner_root/tmp" \
    "Environment=XDG_CACHE_HOME=$runner_root/cache/xdg" \
    "Environment=CODESTORY_CACHE_DIR=$runner_root/cache/codestory" \
    "Environment=CODESTORY_EMBED_ALLOW_CPU=1" \
    "Environment=CODESTORY_REAL_REPO_DRILL_CASES=$runner_root/drills/real-repo-drill-cases.json" \
    "Environment=CODESTORY_RELEASE_EVIDENCE_PROFILE_ID=$profile_id" \
    "Environment=CODESTORY_RELEASE_EVIDENCE_PROVISIONING=$runner_root/artifacts/provisioning.json" \
    | sudo tee "$dropin/override.conf" >/dev/null
  sudo systemctl daemon-reload
  sudo systemctl disable "$service" >/dev/null 2>&1 || true
  sudo systemctl stop "$service" >/dev/null 2>&1 || true
}

case "$command" in
  inspect)
    inspect
    ;;
  configure)
    install_service
    ;;
  start)
    test "$configured" = true
    test "$exact" = true
    test "$binary_version" = "$runner_version"
    sudo systemctl start "$service"
    ;;
  stop)
    if test "$service_installed" = true; then
      sudo systemctl stop "$service"
    fi
    ;;
  forget)
    test "$configured" = true
    test "$exact" = true
    if test "$service_installed" = true; then
      sudo systemctl stop "$service"
      sudo bash -c 'cd "$1" && ./svc.sh uninstall' _ "$runner"
    fi
    sudo rm -f "$runner/.runner" "$runner/.credentials" \
      "$runner/.credentials_rsaparams" "$runner/.service"
    ;;
  *)
    echo "usage: $0 inspect|configure|start|stop|forget CONTRACT" >&2
    exit 2
    ;;
esac
