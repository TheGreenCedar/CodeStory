#!/usr/bin/env python3
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import shutil
import stat
import struct
import tarfile
import tempfile
import zipfile
from pathlib import Path

NORMALIZED_MTIME = 315532800  # 1980-01-01T00:00:00Z, valid for zip and tar.
NATIVE_ENGINE_MARKER_PREFIX = b"codestory-native-engine-v1|"
NATIVE_ENGINE_MARKER_SUFFIX = b"|end"
NATIVE_MANIFEST_FILE = "codestory-native-manifest.json"

TARGET_CONTRACTS = {
    "linux-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "x86_64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "linux-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "aarch64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "windows-x64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "x86_64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "expected_protected_backend": "vulkan",
        "non_claim_reason": None,
    },
    "windows-arm64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "aarch64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "expected_protected_backend": None,
        "non_claim_reason": "windows_arm64_accelerator_execution_is_not_protected",
    },
    "macos-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "x86_64-apple-darwin",
        "target_os": "macos",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "metal"],
        "expected_protected_backend": None,
        "non_claim_reason": "macos_x64_accelerator_execution_is_not_protected",
    },
    "macos-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "aarch64-apple-darwin",
        "target_os": "macos",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "metal"],
        "expected_protected_backend": "metal",
        "non_claim_reason": None,
    },
}


class PackageContractError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PackageContractError(message)


def inspect_binary_format(path: Path) -> dict[str, str]:
    with path.open("rb") as handle:
        header = handle.read(4096)
        if header.startswith(b"\x7fELF"):
            require(len(header) >= 20, "ELF binary header is truncated")
            require(header[4] == 2, "ELF binary is not 64-bit")
            require(header[5] == 1, "ELF binary is not little-endian")
            machine = struct.unpack_from("<H", header, 18)[0]
            arch = {62: "x86_64", 183: "aarch64"}.get(machine)
            require(arch is not None, f"unsupported ELF machine: {machine}")
            return {"format": "elf", "arch": arch}

        if header.startswith(b"MZ"):
            require(len(header) >= 64, "PE binary header is truncated")
            pe_offset = struct.unpack_from("<I", header, 0x3C)[0]
            handle.seek(pe_offset)
            pe_header = handle.read(6)
            require(pe_header.startswith(b"PE\0\0"), "PE signature is missing")
            require(len(pe_header) == 6, "PE machine header is truncated")
            machine = struct.unpack_from("<H", pe_header, 4)[0]
            arch = {0x8664: "x86_64", 0xAA64: "aarch64"}.get(machine)
            require(arch is not None, f"unsupported PE machine: {machine:#x}")
            return {"format": "pe", "arch": arch}

        if header.startswith(b"\xcf\xfa\xed\xfe"):
            require(len(header) >= 8, "Mach-O binary header is truncated")
            cpu_type = struct.unpack_from("<I", header, 4)[0]
            arch = {0x01000007: "x86_64", 0x0100000C: "aarch64"}.get(cpu_type)
            require(arch is not None, f"unsupported Mach-O CPU type: {cpu_type:#x}")
            return {"format": "mach-o", "arch": arch}

    raise PackageContractError("release binary is not a supported ELF, PE, or Mach-O executable")


def native_engine_markers(path: Path) -> list[str]:
    markers: set[bytes] = set()
    overlap = b""
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            block = overlap + chunk
            offset = 0
            while True:
                start = block.find(NATIVE_ENGINE_MARKER_PREFIX, offset)
                if start < 0:
                    break
                end = block.find(NATIVE_ENGINE_MARKER_SUFFIX, start)
                if end < 0:
                    break
                end += len(NATIVE_ENGINE_MARKER_SUFFIX)
                markers.add(block[start:end])
                offset = end
            overlap = block[-4096:]
    decoded = []
    for marker in sorted(markers):
        try:
            decoded.append(marker.decode("ascii"))
        except UnicodeDecodeError as exc:
            raise PackageContractError("native engine build marker is not ASCII") from exc
    return decoded


def parse_native_engine_marker(marker: str) -> dict[str, str]:
    parts = marker.split("|")
    require(parts[0] == "codestory-native-engine-v1", "native engine marker schema is unsupported")
    require(parts[-1] == "end", "native engine marker terminator is missing")
    fields: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed native engine marker field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty native engine marker field: {part!r}")
        require(key not in fields, f"duplicate native engine marker field: {key}")
        fields[key] = value
    required = {
        "target",
        "os",
        "arch",
        "linkage",
        "backends",
        "llama_cpp_crate",
        "llama_cpp_commit",
        "model_sha256",
        "embedding_contract_sha256",
        "model_embedded",
        "producer",
    }
    missing = sorted(required - fields.keys())
    require(not missing, "native engine marker is missing fields: " + ", ".join(missing))
    return fields


def load_model_contract(root: Path) -> dict:
    path = root / "crates/codestory-llama-sys/model-contract.json"
    require(path.is_file(), "native model contract is missing")
    try:
        contract = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise PackageContractError(f"could not read native model contract: {exc}") from exc
    require(isinstance(contract, dict), "native model contract is not an object")
    return contract


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


def native_release_manifest(version: str, target: str, binary: Path, root: Path) -> dict:
    target_contract = TARGET_CONTRACTS.get(target)
    require(target_contract is not None, f"unsupported release target: {target}")

    binary_identity = inspect_binary_format(binary)
    require(
        binary_identity["format"] == target_contract["binary_format"],
        f"binary format {binary_identity['format']} does not match target {target}",
    )
    require(
        binary_identity["arch"] == target_contract["target_arch"],
        f"binary architecture {binary_identity['arch']} does not match target {target}",
    )

    markers = native_engine_markers(binary)
    require(len(markers) == 1, f"binary must contain one native engine identity; found {len(markers)}")
    marker = markers[0]
    fields = parse_native_engine_marker(marker)
    require(fields["target"] == target_contract["target_triple"], "native engine target triple does not match package target")
    require(fields["os"] == target_contract["target_os"], "native engine OS does not match package target")
    require(fields["arch"] == target_contract["target_arch"], "native engine architecture does not match package target")
    require(fields["linkage"] == "static", "release packages require statically linked native engine artifacts")
    compiled_backends = fields["backends"].split(",")
    require(
        compiled_backends == target_contract["compiled_backends"],
        "compiled native backends do not match package target contract",
    )
    require(fields["model_embedded"] == "true", "release binary does not contain the embedded model")

    contract = load_model_contract(root)
    runtime = contract.get("runtime", {})
    producer = contract.get("producer", {})
    model = contract.get("model", {})
    embedding = contract.get("embedding", {})
    tokenizer = contract.get("tokenizer_config", {})
    require(isinstance(runtime, dict), "native model runtime contract is invalid")
    require(isinstance(producer, dict), "native model producer contract is invalid")
    require(isinstance(model, dict), "native model descriptor is invalid")
    require(isinstance(embedding, dict), "native embedding contract is invalid")
    require(isinstance(tokenizer, dict), "native tokenizer contract is invalid")
    embedding_descriptor = dict(embedding)
    embedding_descriptor["family"] = runtime.get("embedding_family")
    contract_sha256 = embedding_contract_digest(model, embedding_descriptor, tokenizer)
    producer_identity = f"{producer.get('name')}@{producer.get('version')}"
    require(fields["producer"] == producer_identity, "native engine producer does not match model contract")
    require(fields["model_sha256"] == model.get("sha256"), "embedded model digest does not match model contract")
    require(
        fields["embedding_contract_sha256"] == contract_sha256,
        "native embedding contract digest does not match package inputs",
    )
    require(str(producer.get("version")) == version, "native model producer version does not match release version")
    require(
        fields["llama_cpp_crate"] == runtime.get("llama_cpp_crate_version"),
        "linked llama.cpp crate version does not match model contract",
    )
    require(
        fields["llama_cpp_commit"] == runtime.get("llama_cpp_source_commit"),
        "linked llama.cpp source commit does not match model contract",
    )

    return {
        "schema_version": 1,
        "release_version": version,
        "asset_target": target,
        "binary": {
            "name": target_contract["binary_name"],
            "sha256": sha256_file(binary),
            "format": binary_identity["format"],
            "arch": binary_identity["arch"],
        },
        "engine": {
            "build_contract_schema_version": 1,
            "build_identity": marker,
            "target_triple": fields["target"],
            "target_os": fields["os"],
            "target_arch": fields["arch"],
            "linkage": fields["linkage"],
            "compiled_backends": compiled_backends,
            "llama_cpp_crate_version": fields["llama_cpp_crate"],
            "llama_cpp_source_commit": fields["llama_cpp_commit"],
            "embedding_contract_sha256": fields["embedding_contract_sha256"],
        },
        "model": {
            "file_name": model.get("file_name"),
            "size_bytes": model.get("size_bytes"),
            "sha256": model.get("sha256"),
            "embedded": True,
            "producer": producer,
        },
        "embedding": embedding_descriptor,
        "tokenizer_config": tokenizer,
        "accelerator": {
            "cpu_fallback": "explicit_only",
            "package_claim": "compiled_capability_only",
            "runtime_execution": "not_proven_by_package",
            "expected_protected_backend": target_contract["expected_protected_backend"],
            "non_claim_reason": target_contract["non_claim_reason"],
        },
    }


def copy_required_file(root: Path, relative: str, destination_root: Path) -> None:
    source = root / relative
    if not source.is_file():
        raise FileNotFoundError(f"required release file is missing: {relative}")
    destination = destination_root / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def copy_required_dir(root: Path, relative: str, destination_root: Path) -> None:
    source = root / relative
    if not source.is_dir():
        raise FileNotFoundError(f"required release directory is missing: {relative}")
    destination = destination_root / relative
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination)


def archive_zip(source_dir: Path, archive_path: Path) -> None:
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source_dir.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(
                    path.relative_to(source_dir.parent).as_posix(),
                    date_time=(1980, 1, 1, 0, 0, 0),
                )
                info.compress_type = zipfile.ZIP_DEFLATED
                info.create_system = 3
                info.external_attr = normalized_file_mode(path) << 16
                archive.writestr(info, path.read_bytes())


def archive_tar_gz(source_dir: Path, archive_path: Path) -> None:
    with archive_path.open("wb") as raw:
        with gzip.GzipFile(
            filename="", mode="wb", fileobj=raw, mtime=NORMALIZED_MTIME
        ) as gzip_file:
            with tarfile.open(fileobj=gzip_file, mode="w") as archive:
                for path in [source_dir, *sorted(source_dir.rglob("*"))]:
                    info = archive.gettarinfo(
                        str(path), arcname=path.relative_to(source_dir.parent).as_posix()
                    )
                    info.mtime = NORMALIZED_MTIME
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mode = 0o755 if path.is_dir() else normalized_file_mode(path)
                    if path.is_file():
                        with path.open("rb") as handle:
                            archive.addfile(info, handle)
                    else:
                        archive.addfile(info)


def normalized_file_mode(path: Path) -> int:
    mode = path.stat().st_mode
    if path.suffix.lower() == ".exe" or mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH):
        return 0o755
    return 0o644


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def package_release(
    version: str, target: str, binary: Path, out_dir: Path, root: Path
) -> Path:
    if not binary.is_file():
        raise FileNotFoundError(f"binary does not exist: {binary}")

    out_dir.mkdir(parents=True, exist_ok=True)

    archive_base = f"codestory-cli-v{version}-{target}"
    target_contract = TARGET_CONTRACTS.get(target)
    require(target_contract is not None, f"unsupported release target: {target}")
    archive_ext = ".zip" if target_contract["target_os"] == "windows" else ".tar.gz"
    archive_path = out_dir / f"{archive_base}{archive_ext}"

    with tempfile.TemporaryDirectory(prefix="codestory-release-", dir=out_dir) as temp_dir:
        stage_root = Path(temp_dir) / archive_base
        stage_root.mkdir(parents=True)

        binary_name = target_contract["binary_name"]
        manifest = native_release_manifest(version, target, binary, root)
        shutil.copy2(binary, stage_root / binary_name)
        (stage_root / NATIVE_MANIFEST_FILE).write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

        copy_required_file(root, "README.md", stage_root)
        copy_required_file(root, "LICENSE", stage_root)
        copy_required_file(root, "docs/glossary.md", stage_root)
        copy_required_dir(root, "docs/users", stage_root)
        copy_required_dir(root, "plugins/codestory/skills/codestory-grounding", stage_root)

        if archive_ext == ".zip":
            archive_zip(stage_root, archive_path)
        else:
            archive_tar_gz(stage_root, archive_path)

    checksum = sha256_file(archive_path)
    checksum_line = f"{checksum}  {archive_path.name}\n"
    checksum_path = out_dir / f"{archive_path.name}.sha256"
    checksum_path.write_bytes(checksum_line.encode("utf-8"))
    (out_dir / "SHA256SUMS.txt").write_bytes(checksum_line.encode("utf-8"))
    return archive_path


def native_marker(
    *,
    target: str,
    os_name: str,
    arch: str,
    backends: str,
    embedding_contract_sha256: str,
    linkage: str = "static",
    model_embedded: str = "true",
) -> str:
    return (
        "codestory-native-engine-v1|"
        f"target={target}|os={os_name}|arch={arch}|linkage={linkage}|"
        f"backends={backends}|llama_cpp_crate=0.1.151|"
        "llama_cpp_commit=test-commit|"
        f"model_sha256={'a' * 64}|embedding_contract_sha256={embedding_contract_sha256}|"
        f"model_embedded={model_embedded}|producer=codestory-llama-sys@0.0.0|end"
    )


def synthetic_binary(binary_format: str, arch: str, marker: str) -> bytes:
    if binary_format == "elf":
        header = bytearray(64)
        header[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", header, 18, {"x86_64": 62, "aarch64": 183}[arch])
    elif binary_format == "pe":
        header = bytearray(256)
        header[:2] = b"MZ"
        struct.pack_into("<I", header, 0x3C, 128)
        header[128:132] = b"PE\0\0"
        struct.pack_into("<H", header, 132, {"x86_64": 0x8664, "aarch64": 0xAA64}[arch])
    elif binary_format == "mach-o":
        header = bytearray(64)
        header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", header, 4, {"x86_64": 0x01000007, "aarch64": 0x0100000C}[arch])
    else:
        raise AssertionError(f"unsupported synthetic binary format: {binary_format}")
    return bytes(header) + b"\0" + marker.encode("ascii") + b"\0"


def run_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-package-self-test-") as temp_dir:
        temp_root = Path(temp_dir)
        repo_root = temp_root / "repo"
        (repo_root / "docs/users").mkdir(parents=True)
        (repo_root / "plugins/codestory/skills/codestory-grounding").mkdir(parents=True)
        (repo_root / "README.md").write_text("readme\n", encoding="utf-8")
        (repo_root / "LICENSE").write_text("license\n", encoding="utf-8")
        (repo_root / "docs/glossary.md").write_text("glossary\n", encoding="utf-8")
        (repo_root / "docs/users/guide.md").write_text("guide\n", encoding="utf-8")
        (repo_root / "plugins/codestory/skills/codestory-grounding/SKILL.md").write_text(
            "skill\n", encoding="utf-8"
        )
        model_contract = {
            "schema_version": 1,
            "model": {
                "file_name": "test.gguf",
                "size_bytes": 4,
                "sha256": "a" * 64,
            },
            "runtime": {
                "embedding_family": "inprocess:test",
                "llama_cpp_crate_version": "0.1.151",
                "llama_cpp_source_commit": "test-commit",
            },
            "embedding": {
                "dimension": 768,
                "query_prefix": "query: ",
                "document_prefix": "",
                "pooling": "cls",
                "normalization": "l2",
                "element_type": "f32_le",
                "vector_schema_version": 2,
            },
            "tokenizer_config": {
                "container": "gguf",
                "tokenizer_sha256": "b" * 64,
                "config_sha256": "c" * 64,
            },
            "producer": {"name": "codestory-llama-sys", "version": "0.0.0"},
        }
        model_contract_path = repo_root / "crates/codestory-llama-sys/model-contract.json"
        model_contract_path.parent.mkdir(parents=True)
        model_contract_path.write_text(
            json.dumps(model_contract, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        embedding_descriptor = dict(model_contract["embedding"])
        embedding_descriptor["family"] = model_contract["runtime"]["embedding_family"]
        fixture_contract_sha256 = embedding_contract_digest(
            model_contract["model"],
            embedding_descriptor,
            model_contract["tokenizer_config"],
        )

        linux_binary = temp_root / "codestory-cli"
        linux_binary.write_bytes(
            synthetic_binary(
                "elf",
                "x86_64",
                native_marker(
                    target="x86_64-unknown-linux-gnu",
                    os_name="linux",
                    arch="x86_64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        linux_binary.chmod(0o755)
        linux_arm_binary = temp_root / "codestory-cli-linux-arm64"
        linux_arm_binary.write_bytes(
            synthetic_binary(
                "elf",
                "aarch64",
                native_marker(
                    target="aarch64-unknown-linux-gnu",
                    os_name="linux",
                    arch="aarch64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        linux_arm_binary.chmod(0o755)
        windows_binary = temp_root / "codestory-cli.exe"
        windows_binary.write_bytes(
            synthetic_binary(
                "pe",
                "x86_64",
                native_marker(
                    target="x86_64-pc-windows-msvc",
                    os_name="windows",
                    arch="x86_64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        windows_arm_binary = temp_root / "codestory-cli-arm64.exe"
        windows_arm_binary.write_bytes(
            synthetic_binary(
                "pe",
                "aarch64",
                native_marker(
                    target="aarch64-pc-windows-msvc",
                    os_name="windows",
                    arch="aarch64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        macos_binary = temp_root / "codestory-cli-macos.exe"
        macos_binary.write_bytes(
            synthetic_binary(
                "mach-o",
                "aarch64",
                native_marker(
                    target="aarch64-apple-darwin",
                    os_name="macos",
                    arch="aarch64",
                    backends="cpu,metal",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        macos_binary.chmod(0o755)
        macos_x64_binary = temp_root / "codestory-cli-macos-x64"
        macos_x64_binary.write_bytes(
            synthetic_binary(
                "mach-o",
                "x86_64",
                native_marker(
                    target="x86_64-apple-darwin",
                    os_name="macos",
                    arch="x86_64",
                    backends="cpu,metal",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        macos_x64_binary.chmod(0o755)

        for target, binary in [
            ("linux-x64", linux_binary),
            ("linux-arm64", linux_arm_binary),
            ("windows-x64", windows_binary),
            ("windows-arm64", windows_arm_binary),
            ("macos-x64", macos_x64_binary),
            ("macos-arm64", macos_binary),
        ]:
            first = package_release("0.0.0", target, binary, temp_root / f"{target}-1", repo_root)
            second = package_release("0.0.0", target, binary, temp_root / f"{target}-2", repo_root)
            first_digest = sha256_file(first)
            second_digest = sha256_file(second)
            if first_digest != second_digest:
                raise AssertionError(
                    f"{target} package checksum changed across identical inputs: "
                    f"{first_digest} != {second_digest}"
                )

            with tempfile.TemporaryDirectory(dir=temp_root) as unpacked_raw:
                unpacked = Path(unpacked_raw)
                if zipfile.is_zipfile(first):
                    with zipfile.ZipFile(first) as archive:
                        archive.extractall(unpacked)
                else:
                    with tarfile.open(first) as archive:
                        archive.extractall(unpacked)
                manifests = list(unpacked.rglob(NATIVE_MANIFEST_FILE))
                require(len(manifests) == 1, "package omitted the native engine manifest")
                manifest = json.loads(manifests[0].read_text(encoding="utf-8"))
                require(
                    manifest["binary"]["name"] == TARGET_CONTRACTS[target]["binary_name"],
                    "package did not canonicalize the target binary name",
                )
                require(manifest["engine"]["linkage"] == "static", "manifest lost linkage evidence")
                require(
                    manifest["accelerator"]["runtime_execution"] == "not_proven_by_package",
                    "package incorrectly claimed runtime accelerator execution",
                )

        stale_contract = json.loads(json.dumps(model_contract))
        stale_contract["embedding"]["query_prefix"] = "changed query: "
        model_contract_path.write_text(
            json.dumps(stale_contract, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        try:
            package_release(
                "0.0.0", "linux-x64", linux_binary, temp_root / "stale-contract", repo_root
            )
        except PackageContractError:
            pass
        else:
            raise AssertionError("stale native embedding contract was accepted")
        model_contract_path.write_text(
            json.dumps(model_contract, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

        hostile = temp_root / "hostile-codestory-cli"
        hostile.write_bytes(
            synthetic_binary(
                "elf",
                "x86_64",
                native_marker(
                    target="x86_64-unknown-linux-gnu",
                    os_name="linux",
                    arch="x86_64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                    linkage="dynamic",
                ),
            )
        )
        try:
            package_release("0.0.0", "linux-x64", hostile, temp_root / "hostile", repo_root)
        except PackageContractError:
            pass
        else:
            raise AssertionError("dynamically linked native engine package was accepted")

        wrong_arch = temp_root / "wrong-arch-codestory-cli"
        wrong_arch.write_bytes(
            synthetic_binary(
                "elf",
                "aarch64",
                native_marker(
                    target="x86_64-unknown-linux-gnu",
                    os_name="linux",
                    arch="x86_64",
                    backends="cpu,vulkan",
                    embedding_contract_sha256=fixture_contract_sha256,
                ),
            )
        )
        try:
            package_release("0.0.0", "linux-x64", wrong_arch, temp_root / "wrong-arch", repo_root)
        except PackageContractError:
            pass
        else:
            raise AssertionError("wrong-architecture native engine package was accepted")

    print("package self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser(description="Package a CodeStory CLI release binary.")
    parser.add_argument("--self-test", action="store_true", help="Run package-twice proof.")
    parser.add_argument("--version", help="Release version without v prefix.")
    parser.add_argument("--target", help="Asset target label.")
    parser.add_argument("--binary", help="Built codestory-cli binary path.")
    parser.add_argument("--out-dir", help="Directory for archive and checksum outputs.")
    parser.add_argument("--project-root", default=".", help="Repository root.")
    args = parser.parse_args()

    if args.self_test:
        run_self_test()
        return

    for required in ["version", "target", "binary", "out_dir"]:
        if getattr(args, required) is None:
            parser.error(f"--{required.replace('_', '-')} is required unless --self-test is used")

    root = Path(args.project_root).resolve()
    binary = Path(args.binary).resolve()
    out_dir = Path(args.out_dir).resolve()
    archive_path = package_release(args.version, args.target, binary, out_dir, root)
    checksum_path = out_dir / f"{archive_path.name}.sha256"

    print(f"archive={archive_path}")
    print(f"checksum={checksum_path}")


if __name__ == "__main__":
    main()
