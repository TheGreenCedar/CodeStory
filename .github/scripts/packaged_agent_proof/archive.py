"""Archive for packaged CodeStory proof."""

from __future__ import annotations

import hashlib
import json
import stat
import tarfile
import zipfile
from pathlib import Path

from native_binary_contract import (
    NativeBinaryError,
    inspect_runtime_layout,
    runtime_artifact_role,
)

from .foundation import (
    LOWER_TIER_NONCLAIMS,
    NATIVE_ENGINE_MARKER_PREFIX,
    NATIVE_ENGINE_MARKER_SUFFIX,
    NATIVE_MANIFEST_FILE,
    SERVER_PROOF_MARKER_PREFIX,
    SERVER_PROOF_MARKER_SUFFIX,
    SERVER_PROOF_SCHEMA_VERSION,
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .contracts import (
    normalized_backend,
    require_sha256,
    sha256,
)

def expected_archive_digest(checksum_file: Path, archive: Path) -> str:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 2 and len(parts[0]) == 64:
            records[parts[-1].lstrip("*")] = parts[0].lower()
        elif len(parts) == 1 and len(parts[0]) == 64:
            records[archive.name] = parts[0].lower()
    require(archive.name in records, f"checksum file does not name {archive.name}")
    return records[archive.name]


def safe_target(root: Path, name: str) -> Path:
    target = (root / name).resolve()
    require(target.is_relative_to(root.resolve()), f"archive member escapes extraction root: {name}")
    return target


def unpack_archive(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                safe_target(destination, member.filename)
            handle.extractall(destination)
        return
    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            members = handle.getmembers()
            for member in members:
                safe_target(destination, member.name)
                require(not member.issym() and not member.islnk(), f"archive contains link: {member.name}")
            handle.extractall(destination, members=members)
        return
    raise ProofFailure(f"unsupported archive format: {archive}")


def find_cli(root: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe"}
    matches = [path for path in root.rglob("*") if path.is_file() and path.name in names]
    require(len(matches) == 1, f"archive must contain exactly one native CodeStory executable; found {len(matches)}")
    cli = matches[0]
    cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli


def binary_markers(path: Path, marker_prefix: str, marker_suffix: str) -> list[str]:
    prefix = marker_prefix.encode("ascii")
    suffix = marker_suffix.encode("ascii")
    markers: set[bytes] = set()
    overlap = b""
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            block = overlap + chunk
            offset = 0
            while True:
                start = block.find(prefix, offset)
                if start < 0:
                    break
                end = block.find(suffix, start)
                if end < 0:
                    break
                end += len(suffix)
                markers.add(block[start:end])
                offset = end
            overlap = block[-4096:]
    decoded = []
    for marker in sorted(markers):
        try:
            decoded.append(marker.decode("ascii"))
        except UnicodeDecodeError as exc:
            raise ProofFailure(f"packaged marker {marker_prefix!r} is not ASCII") from exc
    return decoded


def native_engine_markers(path: Path) -> list[str]:
    return binary_markers(path, NATIVE_ENGINE_MARKER_PREFIX, NATIVE_ENGINE_MARKER_SUFFIX)


def server_proof_markers(path: Path) -> list[str]:
    return binary_markers(path, SERVER_PROOF_MARKER_PREFIX, SERVER_PROOF_MARKER_SUFFIX)


def ordered_contract_digest(domain: str, values: list[str]) -> str:
    digest = hashlib.sha256()
    for value in [domain, *values]:
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)
    return digest.hexdigest()


def embedding_contract_digest(model: dict, embedding: dict, tokenizer: dict) -> str:
    string_fields = [
        (model, "file_name", False),
        (model, "sha256", False),
        (embedding, "family", False),
        (embedding, "query_prefix", False),
        (embedding, "document_prefix", True),
        (embedding, "pooling", False),
        (embedding, "normalization", False),
        (embedding, "element_type", False),
        (tokenizer, "container", False),
        (tokenizer, "tokenizer_sha256", False),
        (tokenizer, "config_sha256", False),
    ]
    for owner, field, allow_empty in string_fields:
        value = owner.get(field)
        require(
            isinstance(value, str) and (allow_empty or bool(value)),
            f"native embedding contract field {field} is invalid",
        )
    for owner, field in (
        (model, "size_bytes"),
        (embedding, "dimension"),
        (embedding, "vector_schema_version"),
    ):
        require(
            type(owner.get(field)) is int and owner[field] > 0,
            f"native embedding contract field {field} is invalid",
        )
    return ordered_contract_digest(
        "codestory-native-embedding-contract-v1",
        [
            model["file_name"],
            str(model["size_bytes"]),
            model["sha256"],
            embedding["family"],
            str(embedding["dimension"]),
            embedding["query_prefix"],
            embedding["document_prefix"],
            embedding["pooling"],
            embedding["normalization"],
            embedding["element_type"],
            str(embedding["vector_schema_version"]),
            tokenizer["container"],
            tokenizer["tokenizer_sha256"],
            tokenizer["config_sha256"],
        ],
    )


def parse_native_build_identity(identity: str) -> dict[str, str]:
    parts = identity.split("|")
    require(parts[0] == "codestory-native-engine-v1", "native engine build schema is unsupported")
    require(parts[-1] == "end", "native engine build identity terminator is missing")
    fields: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed native engine build field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty native engine build field: {part!r}")
        require(key not in fields, f"duplicate native engine build field: {key}")
        fields[key] = value
    required = {
        "target",
        "os",
        "arch",
        "linkage",
        "backend_loading",
        "backends",
        "llama_cpp_crate",
        "llama_cpp_commit",
        "model_sha256",
        "embedding_contract_sha256",
        "model_embedded",
        "producer",
    }
    missing = sorted(required - fields.keys())
    require(not missing, "native engine build identity is missing fields: " + ", ".join(missing))
    return fields


def parse_server_proof_identity(identity: str) -> dict[str, object]:
    parts = identity.split("|")
    require(
        parts[0] == SERVER_PROOF_MARKER_PREFIX.removesuffix("|"),
        "embedding server proof schema is unsupported",
    )
    require(parts[-1] == "end", "embedding server proof identity terminator is missing")
    raw: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed embedding server proof field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty embedding server proof field: {part!r}")
        require(key not in raw, f"duplicate embedding server proof field: {key}")
        raw[key] = value
    required = {
        "bootstrap",
        "protocol_schema",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
        "clock_policy",
        "query_capacity",
        "bulk_capacity",
        "idle_timeout_ms",
    }
    missing = sorted(required - raw.keys())
    require(not missing, "embedding server proof identity is missing fields: " + ", ".join(missing))
    require(set(raw) == required, "embedding server proof identity contains unknown fields")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(raw[field], f"embedding server proof {field}")
    require(raw["clock_policy"] == "awake_monotonic", "embedding server proof clock policy is unsupported")
    numeric: dict[str, int] = {}
    for field in ("bootstrap", "protocol_schema", "query_capacity", "bulk_capacity", "idle_timeout_ms"):
        try:
            numeric[field] = int(raw[field])
        except ValueError as exc:
            raise ProofFailure(f"embedding server proof {field} is not an integer") from exc
        require(numeric[field] > 0, f"embedding server proof {field} must be positive")
    require(numeric["bootstrap"] == 1, "embedding server bootstrap version is unsupported")
    require(numeric["protocol_schema"] == 1, "embedding server protocol schema is unsupported")
    require(numeric["query_capacity"] == 64, "embedding server query capacity is not the accepted value")
    require(numeric["bulk_capacity"] == 64, "embedding server bulk capacity is not the accepted value")
    require(numeric["idle_timeout_ms"] == 60_000, "embedding server idle timeout is not the accepted value")
    return {
        "schema_version": SERVER_PROOF_SCHEMA_VERSION,
        "bootstrap_version": numeric["bootstrap"],
        "protocol_schema_version": numeric["protocol_schema"],
        "protocol_sha256": raw["protocol_sha256"],
        "constant_set_sha256": raw["constant_set_sha256"],
        "measurement_protocol_sha256": raw["measurement_protocol_sha256"],
        "clock_policy": raw["clock_policy"],
        "query_capacity": numeric["query_capacity"],
        "bulk_capacity": numeric["bulk_capacity"],
        "idle_timeout_ms": numeric["idle_timeout_ms"],
        "lower_tier_nonclaims": sorted(LOWER_TIER_NONCLAIMS),
    }


def load_native_manifest(root: Path, cli: Path, expected_version: str) -> dict:
    matches = [path for path in root.rglob(NATIVE_MANIFEST_FILE) if path.is_file()]
    require(
        len(matches) == 1,
        f"archive must contain exactly one native engine manifest; found {len(matches)}",
    )
    try:
        manifest = json.loads(matches[0].read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"native engine manifest is not valid JSON: {exc}") from exc
    require(isinstance(manifest, dict), "native engine manifest is not an object")
    require(manifest.get("schema_version") == 3, "native engine manifest schema is unsupported")
    require(
        manifest.get("release_version") == expected_version,
        "native engine manifest version does not match expected release",
    )
    asset_target = manifest.get("asset_target")
    target_contract = TARGET_CONTRACTS.get(asset_target)
    require(target_contract is not None, f"native manifest has unsupported asset target: {asset_target}")

    binary = manifest.get("binary")
    source = manifest.get("source")
    engine = manifest.get("engine")
    model = manifest.get("model")
    embedding = manifest.get("embedding")
    tokenizer = manifest.get("tokenizer_config")
    accelerator = manifest.get("accelerator")
    server_proof = manifest.get("server_proof")
    runtime_artifacts = manifest.get("runtime_artifacts")
    require(isinstance(binary, dict), "native engine manifest has no binary descriptor")
    require(isinstance(source, dict), "native engine manifest has no source descriptor")
    for field in ("commit", "tree"):
        value = source.get(field)
        require(
            isinstance(value, str)
            and len(value) == 40
            and all(char in "0123456789abcdef" for char in value),
            f"native engine manifest source {field} is invalid",
        )
    require(source.get("tracked_dirty") is False, "native engine manifest was built from tracked changes")
    require(isinstance(engine, dict), "native engine manifest has no engine descriptor")
    require(isinstance(model, dict), "native engine manifest has no model descriptor")
    require(isinstance(embedding, dict), "native engine manifest has no embedding descriptor")
    require(isinstance(tokenizer, dict), "native engine manifest has no tokenizer descriptor")
    require(isinstance(accelerator, dict), "native engine manifest has no accelerator descriptor")
    require(isinstance(server_proof, dict), "native engine manifest has no server proof descriptor")
    require(isinstance(runtime_artifacts, list), "native engine manifest has no runtime artifact set")
    require(
        cli.name == target_contract["binary_name"],
        "packaged executable name does not match asset target",
    )
    require(binary.get("name") == cli.name, "native engine manifest names a different binary")
    cli_sha256 = sha256(cli)
    require(binary.get("sha256") == cli_sha256, "packaged binary digest does not match native manifest")
    artifact_paths: list[Path] = []
    for descriptor in runtime_artifacts:
        require(isinstance(descriptor, dict), "native runtime artifact descriptor is invalid")
        name = descriptor.get("name")
        require(
            isinstance(name, str) and name == Path(name).name,
            "native runtime artifact name is not a basename",
        )
        path = cli.parent / name
        require(path.is_file(), f"native runtime artifact is missing: {name}")
        artifact_paths.append(path)
    discovered = sorted(
        [
            path.name
            for path in cli.parent.iterdir()
            if path.is_file()
            and runtime_artifact_role(path.name, target_contract["target_os"]) is not None
        ],
        key=str.lower,
    )
    require(
        discovered == sorted([path.name for path in artifact_paths], key=str.lower),
        "archive native runtime artifacts do not match the manifest",
    )
    try:
        binary_identity, inspected_artifacts = inspect_runtime_layout(
            cli,
            artifact_paths,
            target_os=target_contract["target_os"],
            expected_format=target_contract["binary_format"],
            expected_arch=target_contract["target_arch"],
            linkage=target_contract["linkage"],
            backend_loading=target_contract["backend_loading"],
        )
    except (OSError, NativeBinaryError) as exc:
        raise ProofFailure(f"packaged native runtime layout is invalid: {exc}") from exc
    inspected_artifacts = [
        {**descriptor, "sha256": sha256(cli.parent / str(descriptor["name"]))}
        for descriptor in inspected_artifacts
    ]
    require(runtime_artifacts == inspected_artifacts, "native runtime artifact evidence is stale")
    require(binary == {
        "name": cli.name,
        "sha256": cli_sha256,
        "format": binary_identity["format"],
        "arch": binary_identity["arch"],
        "needed": binary_identity["needed"],
    }, "native manifest binary descriptor does not match the executable")
    require(
        binary_identity["format"] == target_contract["binary_format"],
        "packaged executable format does not match asset target",
    )
    require(
        binary_identity["arch"] == target_contract["target_arch"],
        "packaged executable architecture does not match asset target",
    )
    require(engine.get("build_contract_schema_version") == 2, "native engine build contract is unsupported")
    build_identity = engine.get("build_identity")
    require(
        isinstance(build_identity, str)
        and build_identity.startswith(NATIVE_ENGINE_MARKER_PREFIX)
        and build_identity.endswith("|end"),
        "native engine build identity is malformed",
    )
    build_fields = parse_native_build_identity(build_identity)
    binary_markers = native_engine_markers(cli)
    require(
        binary_markers == [build_identity],
        "packaged executable native engine marker does not match manifest",
    )
    server_markers = server_proof_markers(cli)
    require(
        len(server_markers) == 1,
        "packaged executable must contain exactly one embedding server proof marker",
    )
    marker_server_proof = parse_server_proof_identity(server_markers[0])
    require(
        set(server_proof) == {*marker_server_proof, "constant_set_status"},
        "native manifest server proof fields do not match the binary marker schema",
    )
    for field, value in marker_server_proof.items():
        require(
            server_proof.get(field) == value,
            f"packaged executable embedding server proof field {field} does not match manifest",
        )
    require(
        server_proof.get("constant_set_status") in {"unfrozen", "frozen"},
        "native manifest server proof constant-set status is invalid",
    )
    require(engine.get("linkage") == target_contract["linkage"], "packaged native engine linkage is wrong")
    require(build_fields["linkage"] == engine["linkage"], "native build linkage contradicts manifest")
    require(
        engine.get("backend_loading") == target_contract["backend_loading"],
        "packaged native backend loading mode is wrong",
    )
    require(
        build_fields["backend_loading"] == engine["backend_loading"],
        "native backend loading mode contradicts manifest",
    )
    compiled_backends = engine.get("compiled_backends")
    require(
        isinstance(compiled_backends, list)
        and compiled_backends
        and all(isinstance(item, str) and item for item in compiled_backends),
        "native manifest has no compiled backend set",
    )
    require(compiled_backends[0] == "cpu", "native manifest does not make CPU capability explicit")
    require(
        build_fields["backends"].split(",") == compiled_backends,
        "native build backend set contradicts manifest",
    )
    for manifest_field, build_field in (
        ("target_triple", "target"),
        ("target_os", "os"),
        ("target_arch", "arch"),
        ("llama_cpp_crate_version", "llama_cpp_crate"),
        ("llama_cpp_source_commit", "llama_cpp_commit"),
    ):
        require(
            engine.get(manifest_field) == build_fields[build_field],
            f"native build {build_field} contradicts manifest",
        )
    for manifest_field, expected in (
        ("target_triple", target_contract["target_triple"]),
        ("target_os", target_contract["target_os"]),
        ("target_arch", target_contract["target_arch"]),
    ):
        require(
            engine.get(manifest_field) == expected,
            f"native manifest {manifest_field} does not match asset target",
        )
    require(
        compiled_backends == target_contract["compiled_backends"],
        "native manifest backend set does not match asset target",
    )
    model_digest = model.get("sha256")
    require(
        isinstance(model_digest, str)
        and len(model_digest) == 64
        and all(char in "0123456789abcdefABCDEF" for char in model_digest),
        "native manifest lacks an exact model digest",
    )
    require(model.get("embedded") is True, "native manifest does not prove an embedded model")
    require(build_fields["model_embedded"] == "true", "native build marker says the model is absent")
    require(build_fields["model_sha256"] == model_digest, "native build model digest contradicts manifest")
    contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
    require(
        build_fields["embedding_contract_sha256"] == contract_sha256,
        "native build embedding contract contradicts manifest",
    )
    require(
        engine.get("embedding_contract_sha256") == contract_sha256,
        "native engine embedding contract digest contradicts manifest",
    )
    producer = model.get("producer")
    require(isinstance(producer, dict), "native manifest lacks model producer identity")
    require(
        build_fields["producer"] == f"{producer.get('name')}@{producer.get('version')}",
        "native build producer contradicts manifest",
    )
    require(
        producer.get("version") == expected_version,
        "native model producer version does not match expected release",
    )
    require(
        accelerator.get("cpu_fallback") == "explicit_only",
        "native manifest permits implicit CPU fallback",
    )
    require(
        accelerator.get("package_claim") == "compiled_capability_only",
        "native manifest overstates package-time accelerator proof",
    )
    require(
        accelerator.get("runtime_execution") == "not_proven_by_package",
        "native manifest overstates package-time execution proof",
    )
    expected_backend = accelerator.get("expected_protected_backend")
    require(
        expected_backend == target_contract["expected_protected_backend"],
        "native manifest protected backend does not match asset target",
    )
    require(
        accelerator.get("non_claim_reason") == target_contract["non_claim_reason"],
        "native manifest accelerator non-claim does not match asset target",
    )
    return manifest


def verify_runtime_against_manifest(
    manifest: dict,
    runtime: dict,
    expected_policy: str | None,
) -> dict:
    identities = [
        runtime.get("identity"),
        runtime.get("second_host_identity"),
        runtime.get("rejoin_identity"),
    ]
    require(all(isinstance(identity, dict) for identity in identities), "runtime proof omitted engine identity")
    engine = manifest["engine"]
    model = manifest["model"]
    accelerator = manifest["accelerator"]
    compiled_backends = engine["compiled_backends"]
    observed_backend = ""
    for label, identity in zip(("first plugin host", "second plugin host", "rejoined plugin host"), identities):
        require(
            identity.get("embedding_ggml_build_identity") == engine["build_identity"],
            f"{label} loaded a different native engine build than the package manifest",
        )
        require(
            identity.get("embedding_model_sha256") == model["sha256"],
            f"{label} loaded a different embedding model than the package manifest",
        )
        current_backend = normalized_backend(identity.get("embedding_backend"))
        require(
            current_backend in compiled_backends,
            f"{label} selected backend {current_backend!r} outside the compiled package contract",
        )
        if not observed_backend:
            observed_backend = current_backend
        require(current_backend == observed_backend, "native backend changed across process restart")

    policy = str(identities[0].get("embedding_policy") or "")
    require(policy == expected_policy, "runtime policy does not match the requested proof lane")
    if policy == "accelerated":
        expected_backend = accelerator.get("expected_protected_backend")
        require(
            isinstance(expected_backend, str) and bool(expected_backend),
            "this package target has no protected accelerator execution claim",
        )
        require(
            observed_backend == expected_backend,
            "runtime accelerator backend does not match the protected package contract",
        )
        execution = "proven_by_live_runtime"
        non_claim_reason = None
    else:
        require(policy == "cpu_explicit", "runtime used neither protected acceleration nor explicit CPU")
        require(observed_backend == "cpu", "explicit CPU proof selected a non-CPU backend")
        execution = "explicit_cpu_execution"
        non_claim_reason = (
            accelerator.get("non_claim_reason")
            or "explicit_cpu_execution_does_not_prove_acceleration"
        )

    return {
        "build_identity": engine["build_identity"],
        "model_sha256": model["sha256"],
        "policy": policy,
        "backend": observed_backend,
        "execution": execution,
        "answer_quality_claim": False,
        "non_claim_reason": non_claim_reason,
    }
