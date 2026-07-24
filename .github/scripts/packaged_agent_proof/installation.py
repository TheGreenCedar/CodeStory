"""Installation for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import tempfile
import threading
import tomllib
from pathlib import Path

from .foundation import (
    CANDIDATE_PRODUCER_WORKFLOW_PATHS,
    LEGACY_TOKENS,
    LOWER_TIER_NONCLAIMS,
    PINNED_CODEX_CLI_VERSION,
    QUALIFICATION_SCHEMA_VERSION,
    REPOSITORY_ROOT,
    ProofFailure,
    require,
)
from .contracts import (
    assert_retained_json_privacy,
    require_exact_keys,
    require_nonempty_string,
    retained_mcp_transcript,
    retained_runtime_evidence,
    sha256,
    write_json,
    write_private_json,
)
from .archive import (
    expected_archive_digest,
    find_cli,
    load_native_manifest,
    unpack_archive,
)
from .process import (
    McpProcess,
)

def directory_contract_sha256(root: Path) -> str:
    require(root.is_dir(), f"plugin package root does not exist: {root}")
    digest = hashlib.sha256()
    files = sorted(path for path in root.rglob("*") if path.is_file())
    require(files, "plugin package root is empty")
    for path in files:
        require(not path.is_symlink(), f"installed plugin package contains a symlink: {path}")
        relative = path.relative_to(root).as_posix().encode("utf-8")
        payload = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "little"))
        digest.update(relative)
        digest.update(len(payload).to_bytes(8, "little"))
        digest.update(payload)
    return digest.hexdigest()


def git_output(repository: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        text=True,
        capture_output=True,
        timeout=30,
    )
    require(
        completed.returncode == 0,
        f"Git identity probe failed: {completed.stderr.strip()}",
    )
    return completed.stdout.strip()


def remote_main_commit(repository_url: str) -> str:
    completed = subprocess.run(
        ["git", "ls-remote", repository_url, "refs/heads/main"],
        text=True,
        capture_output=True,
        timeout=60,
    )
    require(
        completed.returncode == 0,
        f"remote source identity probe failed: {completed.stderr.strip()}",
    )
    fields = completed.stdout.split()
    require(
        len(fields) == 2
        and re.fullmatch(r"[0-9a-f]{40}", fields[0]) is not None
        and fields[1] == "refs/heads/main",
        "remote source main did not resolve to one immutable commit",
    )
    return fields[0]


def marketplace_installed_plugin_identity(
    attestation: dict,
    installed_plugin_data: Path,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    require_exact_keys(
        attestation,
        {
            "schema_version",
            "installation_source",
            "installation",
            "plugin",
            "marketplace",
        },
        "marketplace install attestation",
    )
    installation = attestation["installation"]
    plugin = attestation["plugin"]
    marketplace = attestation["marketplace"]
    require_exact_keys(
        installation,
        {"codex_home", "plugin_root", "plugin_data"},
        "marketplace installation paths",
    )
    require_exact_keys(
        plugin,
        {
            "id",
            "version",
            "source_commit",
            "source_tree",
            "package_sha256",
        },
        "marketplace installed plugin",
    )
    require_exact_keys(
        marketplace,
        {
            "repository",
            "revision",
            "provenance",
            "codex_cli_version",
            "add_result",
            "list_result",
            "plugin_add_result",
            "plugin_list_result",
        },
        "marketplace install producer",
    )
    codex_home = Path(installation["codex_home"]).resolve()
    require(
        attestation["schema_version"] == 2
        and attestation["installation_source"] == "codex_marketplace_install"
        and codex_home.is_dir()
        and Path(installation["plugin_root"]).resolve() == plugin_root
        and Path(installation["plugin_data"]).resolve() == installed_plugin_data
        and installed_plugin_data.is_relative_to(codex_home)
        and plugin_root
        == codex_home
        / "plugins"
        / "cache"
        / "TheGreenCedar"
        / "codestory"
        / manifest["release_version"],
        "marketplace attestation does not identify the exact isolated Codex cache",
    )

    marketplace_add = marketplace["add_result"]
    plugin_add = marketplace["plugin_add_result"]
    marketplace_list = marketplace["list_result"]
    plugin_list = marketplace["plugin_list_result"]
    provenance = marketplace["provenance"]
    require_exact_keys(provenance, {"add", "list"}, "marketplace provenance")
    require_exact_keys(
        provenance["add"],
        {"root", "revision"},
        "marketplace add provenance",
    )
    require_exact_keys(
        provenance["list"],
        {"root", "revision"},
        "marketplace list provenance",
    )
    require(
        marketplace["repository"]
        == "TheGreenCedar/AgentPluginMarketplace"
        and marketplace["codex_cli_version"]
        == f"codex-cli {PINNED_CODEX_CLI_VERSION}"
        and isinstance(marketplace["revision"], str)
        and re.fullmatch(r"[0-9a-f]{40}", marketplace["revision"]) is not None
        and isinstance(marketplace_add, dict)
        and marketplace_add.get("marketplaceName") == "TheGreenCedar"
        and marketplace_add.get("alreadyAdded") is False,
        "marketplace attestation has an invalid pinned Codex producer",
    )
    marketplace_root_raw = marketplace_add.get("installedRoot")
    require(
        isinstance(marketplace_root_raw, str),
        "Codex marketplace add result omitted installedRoot",
    )
    marketplace_root = Path(marketplace_root_raw).resolve()
    require(
        marketplace_root.is_dir()
        and marketplace_root.is_relative_to(codex_home)
        and marketplace_root == codex_home / ".tmp" / "marketplaces" / "TheGreenCedar",
        "Codex marketplace root is outside its isolated home",
    )
    require(
        Path(provenance["add"]["root"]).resolve() == marketplace_root
        and Path(provenance["list"]["root"]).resolve() == marketplace_root
        and provenance["add"]["revision"] == marketplace["revision"]
        and provenance["list"]["revision"] == marketplace["revision"],
        "Codex marketplace add/list provenance does not report the pinned revision",
    )
    require(
        marketplace_list
        == {
            "marketplaces": [
                {
                    "name": "TheGreenCedar",
                    "root": str(marketplace_root),
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": (
                            "https://github.com/"
                            "TheGreenCedar/AgentPluginMarketplace.git"
                        ),
                    },
                }
            ]
        },
        "Codex marketplace list does not match the configured Git snapshot",
    )
    require(
        plugin_add
        == {
            "pluginId": "codestory@TheGreenCedar",
            "name": "codestory",
            "marketplaceName": "TheGreenCedar",
            "version": manifest["release_version"],
            "installedPath": str(plugin_root),
            "authPolicy": "ON_INSTALL",
        },
        "Codex plugin add result does not identify the installed release plugin",
    )
    expected_source = {
        "source": "git-subdir",
        "url": "https://github.com/TheGreenCedar/CodeStory.git",
        "path": "plugins/codestory",
    }
    expected_marketplace_source = {
        "sourceType": "git",
        "source": "https://github.com/TheGreenCedar/AgentPluginMarketplace.git",
    }
    require(
        plugin_list
        == {
            "installed": [
                {
                    "pluginId": "codestory@TheGreenCedar",
                    "name": "codestory",
                    "marketplaceName": "TheGreenCedar",
                    "version": manifest["release_version"],
                    "installed": True,
                    "enabled": True,
                    "source": expected_source,
                    "marketplaceSource": expected_marketplace_source,
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                }
            ],
            "available": [],
        },
        "Codex plugin list does not contain exactly the enabled installed plugin",
    )

    config = tomllib.loads((codex_home / "config.toml").read_text(encoding="utf-8"))
    marketplace_config = config.get("marketplaces", {}).get("TheGreenCedar")
    plugin_config = config.get("plugins", {}).get("codestory@TheGreenCedar")
    require(
        isinstance(marketplace_config, dict)
        and marketplace_config.get("source_type") == "git"
        and marketplace_config.get("source")
        == "https://github.com/TheGreenCedar/AgentPluginMarketplace.git"
        and marketplace_config.get("ref") == marketplace["revision"]
        and plugin_config == {"enabled": True},
        "isolated Codex config does not pin the immutable marketplace revision",
    )
    marketplace_commit = git_output(marketplace_root, "rev-parse", "HEAD")
    require(
        marketplace_commit == marketplace["revision"]
        and git_output(marketplace_root, "status", "--porcelain") == ""
        and git_output(marketplace_root, "remote", "get-url", "origin")
        == "https://github.com/TheGreenCedar/AgentPluginMarketplace.git",
        "Codex marketplace checkout has invalid or mutable Git identity",
    )
    catalog = json.loads(
        (marketplace_root / ".agents" / "plugins" / "marketplace.json").read_text(
            encoding="utf-8"
        )
    )
    matches = [
        plugin
        for plugin in catalog.get("plugins", [])
        if plugin.get("name") == "codestory"
    ]
    require(
        len(matches) == 1 and matches[0].get("source") == expected_source,
        "Codex marketplace catalog does not resolve CodeStory through the release repository",
    )

    source_plugin_root = REPOSITORY_ROOT / "plugins" / "codestory"
    require(
        plugin == {
            "id": "codestory",
            "version": manifest["release_version"],
            "source_commit": manifest["source"]["commit"],
            "source_tree": manifest["source"]["tree"],
            "package_sha256": directory_contract_sha256(plugin_root),
        }
        and git_output(REPOSITORY_ROOT, "rev-parse", "HEAD")
        == manifest["source"]["commit"]
        and git_output(REPOSITORY_ROOT, "rev-parse", "HEAD^{tree}")
        == manifest["source"]["tree"]
        and remote_main_commit(expected_source["url"])
        == manifest["source"]["commit"],
        "marketplace source main is not the exact packaged release commit",
    )
    package_sha256 = directory_contract_sha256(plugin_root)
    require(
        package_sha256 == directory_contract_sha256(source_plugin_root),
        "Codex-installed plugin bytes differ from the packaged release source tree",
    )
    return {
        "schema_version": 2,
        "installation_source": "codex_marketplace_install",
        "codex_cli_version": PINNED_CODEX_CLI_VERSION,
        "marketplace_repository": "TheGreenCedar/AgentPluginMarketplace",
        "marketplace_commit": marketplace_commit,
        "plugin_id": "codestory",
        "plugin_version": manifest["release_version"],
        "plugin_source_commit": manifest["source"]["commit"],
        "plugin_package_sha256": package_sha256,
    }


def prepare_candidate_installed_proof(args: argparse.Namespace) -> dict:
    require(
        args.archive is not None
        and args.checksum_file is not None
        and args.expected_version is not None
        and args.plugin_root is not None
        and args.candidate_plugin_root_output is not None
        and args.candidate_plugin_data_output is not None
        and args.installed_plugin_attestation_output is not None,
        "candidate install preparation requires archive, checksum, version, plugin source, "
        "plugin/data outputs, and attestation output",
    )
    archive = args.archive.resolve()
    checksum = args.checksum_file.resolve()
    source_plugin = args.plugin_root.resolve()
    plugin_output = args.candidate_plugin_root_output.resolve()
    data_output = args.candidate_plugin_data_output.resolve()
    attestation_output = args.installed_plugin_attestation_output.resolve()
    producer = {
        "repository": args.candidate_producer_repository,
        "workflow_path": args.candidate_producer_workflow_path,
        "run_id": args.candidate_producer_run_id,
        "run_attempt": args.candidate_producer_run_attempt,
        "artifact_name": args.candidate_artifact_name,
    }
    require(
        producer["repository"] == "TheGreenCedar/CodeStory"
        and producer["workflow_path"] in CANDIDATE_PRODUCER_WORKFLOW_PATHS
        and isinstance(producer["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None
        and producer["artifact_name"] == archive.name,
        "candidate install producer identity is missing or is not an authenticated release workflow artifact",
    )
    require(
        sha256(archive) == expected_archive_digest(checksum, archive),
        "candidate install archive checksum mismatch",
    )
    require(
        source_plugin.is_dir()
        and not plugin_output.exists()
        and not data_output.exists()
        and not attestation_output.exists(),
        "candidate install outputs must be absent and the source plugin must exist",
    )
    with tempfile.TemporaryDirectory(prefix="codestory-candidate-install-") as raw:
        unpacked = Path(raw) / "unpacked"
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)
        manifest = load_native_manifest(unpacked, cli, args.expected_version)
        repository_root = REPOSITORY_ROOT
        require(
            os.path.samefile(
                source_plugin,
                repository_root / "plugins" / "codestory",
            ),
            "candidate install plugin source is not the checked-in CodeStory plugin",
        )

        def git(*arguments: str) -> str:
            completed = subprocess.run(
                ["git", *arguments],
                cwd=repository_root,
                text=True,
                capture_output=True,
                timeout=30,
            )
            require(
                completed.returncode == 0,
                f"candidate install Git identity probe failed: {completed.stderr.strip()}",
            )
            return completed.stdout.strip()

        require(
            git("rev-parse", "HEAD") == manifest["source"]["commit"]
            and git("rev-parse", "HEAD^{tree}") == manifest["source"]["tree"],
            "candidate plugin checkout does not match the packaged source commit and tree",
        )
        require(
            git("status", "--porcelain", "--untracked-files=all") == "",
            "candidate plugin checkout contains tracked or untracked source drift",
        )
        shutil.copytree(source_plugin, plugin_output)
        expected_archive_name = (
            f"codestory-cli-v{args.expected_version}-"
            f"{manifest['asset_target']}."
            f"{'zip' if manifest['asset_target'].startswith('windows-') else 'tar.gz'}"
        )
        require(
            archive.name == expected_archive_name,
            "candidate install archive name does not match its package target",
        )
        version_root = data_output / "codestory-cli" / args.expected_version
        shutil.copytree(unpacked, version_root)
        relative_cli = cli.relative_to(unpacked).as_posix()
        managed_manifest = {
            "path": relative_cli,
            "sha256": manifest["binary"]["sha256"],
            "version": args.expected_version,
            "build_source": "candidate_archive",
            "repo_ref": manifest["source"]["commit"],
            "archive": archive.name,
            "archive_url": f"candidate-archive:{sha256(archive)}",
            "archive_sha256": sha256(archive),
            "target": manifest["asset_target"],
            "stdio_initialize_verified": True,
            "provisioned_at": f"candidate-proof:{manifest['source']['commit']}",
        }
        write_json(version_root / "manifest.json", managed_manifest)
    attestation = {
        "schema_version": 2,
        "installation_source": "candidate_archive",
        "installation": {
            "plugin_root": str(plugin_output),
            "plugin_data": str(data_output),
        },
        "plugin": {
            "id": "codestory",
            "version": args.expected_version,
            "source_commit": manifest["source"]["commit"],
            "source_tree": manifest["source"]["tree"],
            "package_sha256": directory_contract_sha256(plugin_output),
        },
        "candidate": {
            "archive_sha256": sha256(archive),
            "asset_target": manifest["asset_target"],
            "producer": producer,
        },
    }
    write_json(attestation_output, attestation)
    return {
        "plugin_root": str(plugin_output),
        "plugin_data": str(data_output),
        "attestation": str(attestation_output),
        "source": manifest["source"],
        "archive_sha256": sha256(archive),
        "asset_target": manifest["asset_target"],
    }


def installed_plugin_identity(
    args: argparse.Namespace,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    require(
        args.proof_tier == "installed_runtime",
        "installed plugin identity is valid only at installed_runtime tier",
    )
    require(
        args.installed_plugin_attestation is not None
        and args.installed_plugin_attestation.is_file()
        and args.installed_plugin_data is not None
        and args.installed_plugin_data.is_dir(),
        "installed_runtime proof requires one attestation and its plugin data directory",
    )
    source_plugin_root = REPOSITORY_ROOT / "plugins" / "codestory"
    require(
        not os.path.samefile(plugin_root, source_plugin_root),
        "installed_runtime proof rejects the repository-source plugin root",
    )
    completed = subprocess.run(
        ["git", "-C", str(plugin_root), "rev-parse", "--show-toplevel"],
        text=True,
        capture_output=True,
        timeout=20,
    )
    if completed.returncode == 0:
        checkout = Path(completed.stdout.strip())
        require(
            not ((checkout / "Cargo.toml").is_file() and (checkout / "crates/codestory-cli").is_dir()),
            "installed_runtime proof rejects a plugin launched from a CodeStory source checkout",
        )
    try:
        attestation = json.loads(
            args.installed_plugin_attestation.read_text(encoding="utf-8")
        )
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"installed plugin attestation is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(attestation, dict) and attestation.get("schema_version") == 2,
        "installed plugin attestation schema is unsupported",
    )
    if attestation.get("installation_source") == "codex_marketplace_install":
        return marketplace_installed_plugin_identity(
            attestation,
            args.installed_plugin_data,
            plugin_root,
            manifest,
        )
    require_exact_keys(
        attestation,
        {
            "schema_version",
            "installation_source",
            "installation",
            "plugin",
            "candidate",
        },
        "candidate install attestation",
    )
    installation = attestation["installation"]
    plugin = attestation["plugin"]
    candidate = attestation["candidate"]
    require_exact_keys(
        installation,
        {"plugin_root", "plugin_data"},
        "candidate installation paths",
    )
    require_exact_keys(
        plugin,
        {
            "id",
            "version",
            "source_commit",
            "source_tree",
            "package_sha256",
        },
        "candidate installed plugin",
    )
    require_exact_keys(
        candidate,
        {"archive_sha256", "asset_target", "producer"},
        "candidate install producer",
    )
    package_sha256 = directory_contract_sha256(plugin_root)
    require(
        attestation["installation_source"] == "candidate_archive"
        and Path(installation["plugin_root"]).resolve() == plugin_root
        and Path(installation["plugin_data"]).resolve()
        == args.installed_plugin_data
        and plugin
        == {
            "id": "codestory",
            "version": manifest["release_version"],
            "source_commit": manifest["source"]["commit"],
            "source_tree": manifest["source"]["tree"],
            "package_sha256": package_sha256,
        }
        and candidate
        == {
            "archive_sha256": sha256(args.archive),
            "asset_target": manifest["asset_target"],
            "producer": {
                "repository": args.candidate_producer_repository,
                "workflow_path": args.candidate_producer_workflow_path,
                "run_id": args.candidate_producer_run_id,
                "run_attempt": args.candidate_producer_run_attempt,
                "artifact_name": args.candidate_artifact_name,
            },
        },
        "candidate attestation does not bind the exact archive and source tree",
    )
    return {
        "schema_version": 2,
        "installation_source": "candidate_archive",
        "plugin_id": "codestory",
        "plugin_version": plugin["version"],
        "plugin_source_commit": plugin["source_commit"],
        "plugin_package_sha256": package_sha256,
        "plugin_source_tree": plugin["source_tree"],
        "candidate_archive_sha256": candidate["archive_sha256"],
        "candidate_asset_target": candidate["asset_target"],
        "producer": candidate["producer"],
    }


def verify_managed_runtime_status(
    status: dict,
    *,
    plugin_root: Path,
    manifest: dict,
    archive_sha256: str,
) -> dict:
    plugin = status.get("plugin_runtime")
    require(isinstance(plugin, dict), "installed status omitted plugin_runtime provenance")
    require(plugin.get("cli_source") == "managed", "installed proof did not use the managed runtime")
    require(plugin.get("local_dev_override") is False, "installed proof used a local CLI override")
    require(
        plugin.get("plugin_version") == manifest["release_version"],
        "installed plugin version does not match the package",
    )
    reported_root = plugin.get("plugin_root")
    require(isinstance(reported_root, str), "installed status omitted plugin_root")
    require(
        os.path.samefile(Path(reported_root), plugin_root),
        "installed status names a different plugin root",
    )
    require(
        plugin.get("managed_binary_sha256") == manifest["binary"]["sha256"],
        "installed managed executable does not match the package",
    )
    require(
        plugin.get("archive_sha256") == archive_sha256,
        "installed managed runtime names a different release archive",
    )
    require(
        plugin.get("cli_version") == manifest["release_version"],
        "installed managed executable version does not match the package",
    )
    managed_binary_path = plugin.get("managed_binary_path")
    require(
        isinstance(managed_binary_path, str) and Path(managed_binary_path).is_file(),
        "installed status omitted the managed executable path",
    )
    require(
        sha256(Path(managed_binary_path)) == manifest["binary"]["sha256"],
        "installed managed executable path does not contain the packaged binary",
    )
    for field in ("build_source", "repo_ref", "provisioned_at"):
        require_nonempty_string(plugin.get(field), f"installed plugin_runtime.{field}")
    return {
        "cli_source": "managed",
        "plugin_version": plugin["plugin_version"],
        "managed_binary_sha256": plugin["managed_binary_sha256"],
        "archive_sha256": plugin["archive_sha256"],
        "build_source": plugin["build_source"],
        "repo_ref": plugin["repo_ref"],
        "provisioned_at": plugin["provisioned_at"],
    }


def run_parallel(tasks: dict[str, callable]) -> dict[str, object]:
    results: dict[str, object] = {}
    failures: list[tuple[str, BaseException]] = []
    lock = threading.Lock()

    def invoke(name: str, task) -> None:
        try:
            value = task()
            with lock:
                results[name] = value
        except BaseException as exc:  # noqa: BLE001 - preserve worker failure for the proof.
            with lock:
                failures.append((name, exc))

    threads = [
        threading.Thread(target=invoke, args=(name, task), daemon=True)
        for name, task in tasks.items()
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    if failures:
        failures.sort(key=lambda item: item[0])
        details = "; ".join(f"{name}: {failure}" for name, failure in failures)
        raise ProofFailure(
            f"parallel qualification tasks failed: {details}"
        ) from failures[0][1]
    return results


def isolated_environment(root: Path, policy: str | None, offline: bool) -> dict[str, str]:
    env = dict(os.environ)
    home = root / "home"
    cache = root / "cache"
    data = root / "plugin-data"
    temp = root / "tmp"
    runtime = root / "runtime"
    for path in (home, cache, data, temp, runtime):
        path.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "CODESTORY_CACHE_ROOT": str(cache),
        "CODESTORY_PLUGIN_DATA": str(data),
        "TMPDIR": str(temp),
        "TEMP": str(temp),
        "TMP": str(temp),
        "XDG_RUNTIME_DIR": str(runtime),
        "CODESTORY_EMBED_ALLOW_CPU": "1" if policy == "cpu_explicit" else "0",
    })
    if offline:
        env.update({
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "ALL_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "",
            "CODESTORY_PLUGIN_DISABLE_PROVISION": "1",
        })
    for key in list(env):
        if key.startswith("CODESTORY_EMBED_") and key != "CODESTORY_EMBED_ALLOW_CPU":
            del env[key]
    return env


def qualification_environment(root: Path, env: dict[str, str]) -> tuple[dict[str, str], dict]:
    proof_root = (root / "qualification").resolve()
    proof_root.mkdir(parents=True, exist_ok=True)
    proof_root.chmod(0o700)
    nonce = secrets.token_hex(32)
    qualified = dict(env)
    qualified["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(proof_root)
    qualified["CODESTORY_EMBED_QUALIFICATION_NONCE"] = nonce
    return qualified, {
        "schema_version": QUALIFICATION_SCHEMA_VERSION,
        "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
    }


def assert_no_legacy_state(cache_root: Path) -> None:
    offenders = []
    for path in cache_root.rglob("*"):
        lowered = path.name.lower()
        if any(token in lowered for token in LEGACY_TOKENS) or path.suffix.lower() == ".pid":
            offenders.append(str(path))
    require(not offenders, "legacy process state was created: " + ", ".join(offenders[:10]))


def create_second_repository(root: Path) -> Path:
    repo = root / "second-repository"
    repo.mkdir()
    (repo / "README.md").write_text("# Second repository\n\nA tiny warm-engine reuse fixture.\n", encoding="utf-8")
    (repo / "lib.rs").write_text("pub fn shared_engine_probe() -> &'static str { \"warm\" }\n", encoding="utf-8")
    return repo


def prove_ground_only_runtime(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    out_dir: Path,
    manifest: dict,
) -> dict:
    require(args.plugin_handoff, "ground-only proof requires the ordinary packaged plugin handoff")
    require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
    require(args.project is not None, "--project is required for ground-only proof")
    require(
        not args.additional_project and not args.additional_query,
        "ground-only proof accepts exactly one project",
    )
    project = args.project.resolve()
    require(project.is_dir(), f"ground-only proof repository does not exist: {project}")

    plugin_root = args.plugin_root.resolve()
    provenance = (
        installed_plugin_identity(args, plugin_root, manifest)
        if args.proof_tier == "installed_runtime"
        else None
    )
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
    node = shutil.which("node")
    require(node is not None, "packaged plugin proof requires Node.js for the host launcher")
    qualified_env, _qualification_control = qualification_environment(root, env)
    qualified_env.pop("CODESTORY_CLI", None)
    if args.proof_tier == "installed_runtime":
        require(
            args.installed_plugin_data is not None,
            "installed ground-only proof requires --installed-plugin-data",
        )
        qualified_env["CODESTORY_PLUGIN_DATA"] = str(args.installed_plugin_data.resolve())
        if provenance["installation_source"] == "candidate_archive":
            candidate_archive_sha256 = sha256(args.archive)
            qualified_env[
                "CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256"
            ] = candidate_archive_sha256
            write_private_json(
                Path(qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"])
                / "candidate-managed-install.json",
                {
                    "schema_version": 1,
                    "purpose": "codestory-candidate-managed-install",
                    "archive_sha256": candidate_archive_sha256,
                    "qualification_nonce_sha256": hashlib.sha256(
                        qualified_env[
                            "CODESTORY_EMBED_QUALIFICATION_NONCE"
                        ].encode("ascii")
                    ).hexdigest(),
                },
            )
    else:
        qualified_env["CODESTORY_CLI"] = str(cli)

    host = McpProcess(
        [node, str(launcher)],
        env=qualified_env,
        cwd=project,
        timeout=args.timeout_secs,
    )
    managed_runtime = None
    managed_binary_path = None
    try:
        host.initialize()
        ground_response, ground_attempts = host.tool_until_ready(
            "ground",
            {
                "project": str(project),
                "budget": "strict",
            },
            "installed-ground",
        )
        ground = ground_response["result"]["structuredContent"]
        require(
            isinstance(ground, dict) and ground,
            f"installed runtime ground returned no structured result: {ground!r}",
        )
        if args.proof_tier == "installed_runtime":
            status = host.status(project, "installed-ground-status")
            managed_runtime = verify_managed_runtime_status(
                status,
                plugin_root=plugin_root,
                manifest=manifest,
                archive_sha256=sha256(args.archive),
            )
            if provenance["installation_source"] == "candidate_archive":
                require(
                    managed_runtime["build_source"] == "candidate_archive"
                    and managed_runtime["repo_ref"] == manifest["source"]["commit"],
                    "candidate installed ground did not launch the staged candidate archive",
                )
            else:
                require(
                    managed_runtime["build_source"] == "github_release"
                    and managed_runtime["repo_ref"]
                    == f"v{manifest['release_version']}",
                    "marketplace installed ground did not launch the published release archive",
                )
            managed_binary_path = Path(
                require_nonempty_string(
                    status["plugin_runtime"].get("managed_binary_path"),
                    "installed plugin_runtime.managed_binary_path",
                )
            ).resolve()
            require(
                managed_binary_path.is_relative_to(args.installed_plugin_data.resolve()),
                "installed managed executable is outside the installed plugin data root",
            )
            require(
                managed_binary_path != cli.resolve(),
                "installed ground proof used the unpacked package executable as its managed runtime",
            )

        result = {
            "ground": {
                "status": "pass",
                "attempts": ground_attempts,
                "project_bound": True,
                "response_nonempty": True,
            },
            "installed_plugin": provenance,
            "managed_runtime": managed_runtime,
            "_qualification_cli_path": (
                str(managed_binary_path)
                if managed_binary_path is not None
                else str(cli.resolve())
            ),
            "_qualification_projects": [str(project)],
            "_qualification_forbidden_values": [
                str(project),
                str(plugin_root),
                str(cli.resolve()),
                str(root.resolve()),
                qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
                qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
                *(
                    [str(managed_binary_path)]
                    if managed_binary_path is not None
                    else []
                ),
            ],
            "nonclaims": {
                claim: {
                    "claimed": False,
                    "reason": "installed ground proof does not establish this claim",
                }
                for claim in sorted(LOWER_TIER_NONCLAIMS)
            },
        }
    finally:
        write_json(
            out_dir / "plugin-ground-mcp.json",
            retained_mcp_transcript(host.transcript),
        )
        host.close()

    assert_no_legacy_state(Path(qualified_env["CODESTORY_CACHE_ROOT"]))
    public_runtime_evidence = out_dir / "installed-ground-proof.json"
    write_json(public_runtime_evidence, retained_runtime_evidence(result))
    forbidden_runtime_values = result.get("_qualification_forbidden_values", [])
    for public_artifact in (
        out_dir / "plugin-ground-mcp.json",
        public_runtime_evidence,
    ):
        assert_retained_json_privacy(public_artifact, forbidden_runtime_values)
    return result
