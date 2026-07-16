#!/usr/bin/env python3
"""Small, dependency-free native import-table inspector for release proof."""

from __future__ import annotations

import struct
from pathlib import Path


class NativeBinaryError(ValueError):
    pass


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise NativeBinaryError(message)


def _cstring(data: bytes, offset: int, limit: int | None = None) -> str:
    _require(0 <= offset < len(data), "native dependency string points outside the file")
    end_limit = len(data) if limit is None else min(len(data), limit)
    end = data.find(b"\0", offset, end_limit)
    _require(end >= 0, "native dependency string is not terminated")
    try:
        return data[offset:end].decode("utf-8")
    except UnicodeDecodeError as exc:
        raise NativeBinaryError("native dependency name is not UTF-8") from exc


def inspect_binary(path: Path) -> dict[str, object]:
    data = path.read_bytes()
    if data.startswith(b"\x7fELF"):
        return _inspect_elf(data)
    if data.startswith(b"MZ"):
        return _inspect_pe(data)
    if data.startswith(b"\xcf\xfa\xed\xfe"):
        return _inspect_macho(data)
    raise NativeBinaryError("release binary is not a supported ELF, PE, or Mach-O image")


def runtime_artifact_role(name: str, target_os: str) -> str | None:
    lower = name.lower()
    if target_os == "windows" and lower.endswith(".dll"):
        stem = lower[:-4]
        if stem.startswith("ggml-vulkan"):
            return "vulkan_backend"
        if stem.startswith("ggml-cpu"):
            return "cpu_backend"
        if stem in {"llama", "ggml", "ggml-base"}:
            return "core"
    if target_os == "linux" and lower.startswith("lib") and ".so" in lower:
        stem = lower[3 : lower.index(".so")]
        if stem.startswith("ggml-vulkan"):
            return "vulkan_backend"
        if stem.startswith("ggml-cpu"):
            return "cpu_backend"
        if stem in {"llama", "ggml", "ggml-base"}:
            return "core"
    return None


def is_vulkan_loader(name: str, target_os: str) -> bool:
    lower = Path(name).name.lower()
    if target_os == "windows":
        return lower == "vulkan-1.dll"
    if target_os == "linux":
        return lower.startswith("libvulkan.so")
    return False


def inspect_runtime_layout(
    executable: Path,
    artifacts: list[Path],
    *,
    target_os: str,
    expected_format: str,
    expected_arch: str,
    linkage: str,
    backend_loading: str,
) -> tuple[dict[str, object], list[dict[str, object]]]:
    binary = inspect_binary(executable)
    _require(binary["format"] == expected_format, "runtime executable format does not match target")
    _require(binary["arch"] == expected_arch, "runtime executable architecture does not match target")
    binary_needed = [str(value) for value in binary["needed"]]
    _require(
        not any(is_vulkan_loader(name, target_os) for name in binary_needed),
        "base executable has a mandatory Vulkan loader dependency",
    )

    descriptors: list[dict[str, object]] = []
    seen_names: set[str] = set()
    for path in sorted(artifacts, key=lambda item: item.name.lower()):
        role = runtime_artifact_role(path.name, target_os)
        _require(role is not None, f"unrecognized packaged native runtime artifact: {path.name}")
        key = path.name.lower()
        _require(key not in seen_names, f"duplicate packaged native runtime artifact: {path.name}")
        seen_names.add(key)
        identity = inspect_binary(path)
        _require(identity["format"] == expected_format, f"native artifact format mismatch: {path.name}")
        _require(identity["arch"] == expected_arch, f"native artifact architecture mismatch: {path.name}")
        needed = [str(value) for value in identity["needed"]]
        has_vulkan_loader = any(is_vulkan_loader(name, target_os) for name in needed)
        if role == "vulkan_backend":
            _require(has_vulkan_loader, f"Vulkan backend does not import the Vulkan loader: {path.name}")
        else:
            _require(not has_vulkan_loader, f"non-Vulkan artifact imports the Vulkan loader: {path.name}")
        descriptors.append(
            {
                "name": path.name,
                "role": role,
                "format": identity["format"],
                "arch": identity["arch"],
                "needed": needed,
            }
        )

    if linkage == "dynamic" or backend_loading == "runtime-modules":
        _require(linkage == "dynamic", "runtime backend modules require dynamic core linkage")
        _require(backend_loading == "runtime-modules", "dynamic package must declare runtime modules")
        roles = {str(descriptor["role"]) for descriptor in descriptors}
        _require("core" in roles, "dynamic package is missing llama.cpp core libraries")
        _require("cpu_backend" in roles, "dynamic package is missing its CPU backend module")
        _require("vulkan_backend" in roles, "dynamic package is missing its Vulkan backend module")
    else:
        _require(linkage == "static", "builtin backend package must use static linkage")
        _require(backend_loading == "builtin", "static package must declare builtin backends")
        _require(not descriptors, "static package unexpectedly contains runtime backend modules")

    local_names = {str(descriptor["name"]).lower() for descriptor in descriptors}
    for owner, needed in [(executable.name, binary_needed), *[(str(item["name"]), item["needed"]) for item in descriptors]]:
        for dependency in needed:
            dependency_name = Path(str(dependency)).name.lower()
            if runtime_artifact_role(dependency_name, target_os) is not None:
                _require(
                    dependency_name in local_names,
                    f"{owner} requires unpackaged native runtime artifact {dependency}",
                )
    return binary, descriptors


def _inspect_elf(data: bytes) -> dict[str, object]:
    _require(len(data) >= 64, "ELF header is truncated")
    _require(data[4] == 2, "ELF image is not 64-bit")
    _require(data[5] == 1, "ELF image is not little-endian")
    machine = struct.unpack_from("<H", data, 18)[0]
    arch = {62: "x86_64", 183: "aarch64"}.get(machine)
    _require(arch is not None, f"unsupported ELF machine: {machine}")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize = struct.unpack_from("<H", data, 54)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    _require(phnum == 0 or phentsize >= 56, "ELF program header size is invalid")
    _require(phoff + phentsize * phnum <= len(data), "ELF program headers are truncated")

    loads: list[tuple[int, int, int]] = []
    dynamic: tuple[int, int] | None = None
    for index in range(phnum):
        offset = phoff + index * phentsize
        p_type, _flags, p_offset, p_vaddr, _paddr, p_filesz, _memsz, _align = (
            struct.unpack_from("<IIQQQQQQ", data, offset)
        )
        _require(p_offset + p_filesz <= len(data), "ELF segment points outside the file")
        if p_type == 1:
            loads.append((p_vaddr, p_filesz, p_offset))
        elif p_type == 2:
            dynamic = (p_offset, p_filesz)

    needed_offsets: list[int] = []
    strtab_vaddr: int | None = None
    strtab_size: int | None = None
    if dynamic is not None:
        offset, size = dynamic
        _require(size % 16 == 0, "ELF dynamic table size is invalid")
        for entry in range(offset, offset + size, 16):
            tag, value = struct.unpack_from("<qQ", data, entry)
            if tag == 0:
                break
            if tag == 1:
                needed_offsets.append(value)
            elif tag == 5:
                strtab_vaddr = value
            elif tag == 10:
                strtab_size = value
    _require(not needed_offsets or strtab_vaddr is not None, "ELF dependencies have no string table")

    def vaddr_to_offset(address: int) -> int:
        for vaddr, filesz, file_offset in loads:
            if vaddr <= address < vaddr + filesz:
                return file_offset + address - vaddr
        raise NativeBinaryError("ELF string table does not map to a file-backed segment")

    needed: list[str] = []
    if strtab_vaddr is not None:
        strtab_offset = vaddr_to_offset(strtab_vaddr)
        end = strtab_offset + (strtab_size or len(data) - strtab_offset)
        _require(end <= len(data), "ELF string table is truncated")
        needed = [_cstring(data, strtab_offset + value, end) for value in needed_offsets]
    return {"format": "elf", "arch": arch, "needed": sorted(set(needed), key=str.lower)}


def _inspect_pe(data: bytes) -> dict[str, object]:
    _require(len(data) >= 64, "PE DOS header is truncated")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    _require(pe_offset + 24 <= len(data), "PE header is truncated")
    _require(data[pe_offset : pe_offset + 4] == b"PE\0\0", "PE signature is missing")
    coff = pe_offset + 4
    machine, section_count, _timestamp, _symbols, _symbol_count, optional_size, _flags = (
        struct.unpack_from("<HHIIIHH", data, coff)
    )
    arch = {0x8664: "x86_64", 0xAA64: "aarch64"}.get(machine)
    _require(arch is not None, f"unsupported PE machine: {machine:#x}")
    optional = coff + 20
    _require(optional + optional_size <= len(data), "PE optional header is truncated")
    _require(optional_size >= 112, "PE optional header is too small")
    magic = struct.unpack_from("<H", data, optional)[0]
    _require(magic in (0x10B, 0x20B), f"unsupported PE optional-header magic: {magic:#x}")
    directory_offset = optional + (112 if magic == 0x20B else 96)
    image_base = struct.unpack_from("<Q" if magic == 0x20B else "<I", data, optional + (24 if magic == 0x20B else 28))[0]
    section_offset = optional + optional_size
    _require(section_offset + section_count * 40 <= len(data), "PE section table is truncated")
    sections: list[tuple[int, int, int, int]] = []
    for index in range(section_count):
        offset = section_offset + index * 40
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from("<IIII", data, offset + 8)
        _require(raw_offset + raw_size <= len(data), "PE section points outside the file")
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset, raw_size))

    def rva_to_offset(rva: int) -> int:
        if rva < len(data):
            # Header RVAs map directly.
            first_section = min((section[0] for section in sections), default=len(data))
            if rva < first_section:
                return rva
        for virtual_address, span, raw_offset, raw_size in sections:
            if virtual_address <= rva < virtual_address + span:
                delta = rva - virtual_address
                _require(delta < raw_size, "PE RVA points into a non-file-backed section tail")
                return raw_offset + delta
        raise NativeBinaryError(f"PE RVA {rva:#x} does not map to a section")

    def directory(index: int) -> tuple[int, int]:
        location = directory_offset + index * 8
        if location + 8 > optional + optional_size:
            return (0, 0)
        return struct.unpack_from("<II", data, location)

    needed: list[str] = []
    import_rva, import_size = directory(1)
    if import_rva:
        cursor = rva_to_offset(import_rva)
        limit = min(len(data), cursor + max(import_size, 20))
        while cursor + 20 <= limit:
            descriptor = struct.unpack_from("<IIIII", data, cursor)
            if not any(descriptor):
                break
            needed.append(_cstring(data, rva_to_offset(descriptor[3])))
            cursor += 20

    delay_rva, delay_size = directory(13)
    if delay_rva:
        cursor = rva_to_offset(delay_rva)
        limit = min(len(data), cursor + max(delay_size, 32))
        while cursor + 32 <= limit:
            descriptor = struct.unpack_from("<IIIIIIII", data, cursor)
            if not any(descriptor):
                break
            name_rva = descriptor[1]
            if descriptor[0] & 1 == 0:
                _require(name_rva >= image_base, "PE delay import address precedes image base")
                name_rva -= image_base
            needed.append(_cstring(data, rva_to_offset(name_rva)))
            cursor += 32
    return {"format": "pe", "arch": arch, "needed": sorted(set(needed), key=str.lower)}


def _inspect_macho(data: bytes) -> dict[str, object]:
    _require(len(data) >= 32, "Mach-O header is truncated")
    cpu_type = struct.unpack_from("<I", data, 4)[0]
    arch = {0x01000007: "x86_64", 0x0100000C: "aarch64"}.get(cpu_type)
    _require(arch is not None, f"unsupported Mach-O CPU type: {cpu_type:#x}")
    command_count, command_bytes = struct.unpack_from("<II", data, 16)
    _require(32 + command_bytes <= len(data), "Mach-O load commands are truncated")
    load_dylib_commands = {0xC, 0x80000018, 0x8000001F, 0x80000023}
    needed: list[str] = []
    cursor = 32
    for _ in range(command_count):
        _require(cursor + 8 <= 32 + command_bytes, "Mach-O load command header is truncated")
        command, size = struct.unpack_from("<II", data, cursor)
        _require(size >= 8 and cursor + size <= 32 + command_bytes, "Mach-O load command size is invalid")
        if command in load_dylib_commands:
            _require(size >= 24, "Mach-O dylib command is truncated")
            name_offset = struct.unpack_from("<I", data, cursor + 8)[0]
            _require(0 < name_offset < size, "Mach-O dylib name offset is invalid")
            needed.append(_cstring(data, cursor + name_offset, cursor + size))
        cursor += size
    return {"format": "mach-o", "arch": arch, "needed": sorted(set(needed), key=str.lower)}
