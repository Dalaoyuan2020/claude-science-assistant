#!/usr/bin/env python3
"""Audit or reversibly remove overly broad persistent DrvFS write grants.

This tool never walks a granted path.  Mutation is explicit: --apply removes
only exact broad global RW grants, while --convert-drvfs-rw-to-ro preserves
read access and revokes persistent writes under /mnt/<drive>.  The original
bytes are backed up before an atomic replacement so an operator can restore
them if needed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import tempfile
from pathlib import Path
from typing import Any


BROAD_DRVFS_RW = re.compile(
    r"^rw:(/mnt/[A-Za-z](?:/(?:Downloads|Documents|Desktop)|"
    r"/Users/[^/]+/(?:Downloads|Documents|Desktop))?)/?$",
    re.IGNORECASE,
)
ANY_DRVFS_RW = re.compile(r"^rw:(/mnt/[A-Za-z](?:/.*)?)/?$", re.IGNORECASE)


def _broad_grants(preferences: dict[str, Any]) -> list[str]:
    approval = preferences.get("approvalGrants", {})
    if not isinstance(approval, dict):
        raise TypeError("approvalGrants must be an object")
    always = approval.get("always", {})
    if not isinstance(always, dict):
        raise TypeError("approvalGrants.always must be an object")
    allow = always.get("allow", {})
    if not isinstance(allow, dict):
        raise TypeError("approvalGrants.always.allow must be an object")
    host = allow.get("host", [])
    if not isinstance(host, list):
        raise TypeError("approvalGrants.always.allow.host must be an array")
    return sorted(
        {
            item
            for item in host
            if isinstance(item, str) and BROAD_DRVFS_RW.fullmatch(item)
        }
    )


def narrow_preferences(preferences: dict[str, Any]) -> tuple[dict[str, Any], list[str]]:
    """Return a deep JSON copy with only exact broad global RW grants removed."""

    updated = json.loads(json.dumps(preferences))
    broad = _broad_grants(updated)
    if not broad:
        return updated, []

    approval = updated["approvalGrants"]
    always = approval["always"]
    allow = always["allow"]
    allow["host"] = [item for item in allow["host"] if item not in broad]

    origins = always.get("alwaysOrigins", approval.get("alwaysOrigins"))
    # Claude Science 0.1.25 stores alwaysOrigins beside `always`, under
    # approvalGrants.  Accept a nested future shape without creating fields.
    origin_containers: list[dict[str, Any]] = []
    top_origins = approval.get("alwaysOrigins")
    if top_origins is not None:
        if not isinstance(top_origins, dict):
            raise TypeError("approvalGrants.alwaysOrigins must be an object")
        origin_containers.append(top_origins)
    if origins is not None and origins is not top_origins:
        if not isinstance(origins, dict):
            raise TypeError("alwaysOrigins must be an object")
        origin_containers.append(origins)
    for container in origin_containers:
        host_origins = container.get("host", {})
        if not isinstance(host_origins, dict):
            raise TypeError("alwaysOrigins.host must be an object")
        for grant in broad:
            host_origins.pop(grant, None)

    legacy = updated.get("hostGrants")
    if legacy is not None:
        if not isinstance(legacy, list):
            raise TypeError("hostGrants must be an array")
        broad_paths = {BROAD_DRVFS_RW.fullmatch(grant).group(1).rstrip("/") for grant in broad}
        updated["hostGrants"] = [
            item
            for item in legacy
            if not (
                isinstance(item, dict)
                and str(item.get("mode", "")).lower() == "rw"
                and str(item.get("path", "")).rstrip("/") in broad_paths
            )
        ]

    return updated, broad


def convert_drvfs_writes_to_read_only(
    preferences: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, str]]:
    """Preserve access while converting every persistent DrvFS RW grant to RO."""

    updated = json.loads(json.dumps(preferences))
    approval = updated.get("approvalGrants", {})
    if not isinstance(approval, dict):
        raise TypeError("approvalGrants must be an object")
    always = approval.get("always", {})
    if not isinstance(always, dict):
        raise TypeError("approvalGrants.always must be an object")
    allow = always.get("allow", {})
    if not isinstance(allow, dict):
        raise TypeError("approvalGrants.always.allow must be an object")
    host = allow.get("host", [])
    if not isinstance(host, list):
        raise TypeError("approvalGrants.always.allow.host must be an array")

    conversions: dict[str, str] = {}
    for item in host:
        if not isinstance(item, str):
            continue
        match = ANY_DRVFS_RW.fullmatch(item)
        if match:
            conversions[item] = f"ro:{match.group(1).rstrip('/')}"
    if not conversions:
        return updated, {}

    projected: list[Any] = []
    for item in host:
        replacement = conversions.get(item, item)
        if replacement not in projected:
            projected.append(replacement)
    allow["host"] = projected

    origin_containers: list[dict[str, Any]] = []
    for candidate in (approval.get("alwaysOrigins"), always.get("alwaysOrigins")):
        if candidate is None or any(candidate is existing for existing in origin_containers):
            continue
        if not isinstance(candidate, dict):
            raise TypeError("alwaysOrigins must be an object")
        origin_containers.append(candidate)
    for container in origin_containers:
        host_origins = container.get("host", {})
        if not isinstance(host_origins, dict):
            raise TypeError("alwaysOrigins.host must be an object")
        for original, replacement in conversions.items():
            origin = host_origins.pop(original, None)
            if origin is not None and replacement not in host_origins:
                host_origins[replacement] = origin

    legacy = updated.get("hostGrants")
    if legacy is not None:
        if not isinstance(legacy, list):
            raise TypeError("hostGrants must be an array")
        for item in legacy:
            if not isinstance(item, dict):
                continue
            path = str(item.get("path", "")).rstrip("/")
            if str(item.get("mode", "")).lower() == "rw" and re.match(
                r"^/mnt/[A-Za-z](?:/|$)", path
            ):
                item["mode"] = "ro"

    return updated, conversions


def _write_exclusive(path: Path, payload: bytes, mode: int = 0o600) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(path, flags, mode)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        os.close(descriptor)


def apply_narrowing(path: Path, *, convert_drvfs_to_ro: bool = False) -> dict[str, Any]:
    original = path.read_bytes()
    preferences = json.loads(original.decode("utf-8"))
    if not isinstance(preferences, dict):
        raise TypeError("preferences root must be an object")
    if convert_drvfs_to_ro:
        updated, conversions = convert_drvfs_writes_to_read_only(preferences)
        removed: list[str] = []
    else:
        updated, removed = narrow_preferences(preferences)
        conversions = {}
    digest = hashlib.sha256(original).hexdigest()
    result: dict[str, Any] = {
        "preferences": str(path),
        "parse_ok": True,
        "removed_count": len(removed),
        "removed": removed,
        "converted_count": len(conversions),
        "converted": conversions,
        "changed": bool(removed or conversions),
        "original_sha256": digest,
        "backup": None,
    }
    if not removed and not conversions:
        return result

    metadata = path.stat()
    backup = path.with_name(f"{path.name}.csa-v0.1.6-{digest[:16]}.bak")
    if backup.exists():
        if backup.read_bytes() != original:
            raise FileExistsError(f"backup exists with different content: {backup}")
    else:
        _write_exclusive(backup, original)
        os.chmod(backup, 0o600)
        if hasattr(os, "chown"):
            try:
                os.chown(backup, metadata.st_uid, metadata.st_gid)
            except PermissionError:
                pass

    encoded = (json.dumps(updated, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.csa-", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, stat.S_IMODE(metadata.st_mode))
        if hasattr(os, "chown"):
            try:
                os.chown(temporary, metadata.st_uid, metadata.st_gid)
            except PermissionError:
                pass
        if path.read_bytes() != original:
            raise RuntimeError("preferences changed concurrently; refusing to replace it")
        os.replace(temporary, path)
        if hasattr(os, "O_DIRECTORY"):
            directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
    finally:
        if temporary.exists():
            temporary.unlink()

    result["backup"] = str(backup)
    return result


def audit(path: Path) -> dict[str, Any]:
    preferences = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(preferences, dict):
        raise TypeError("preferences root must be an object")
    broad = _broad_grants(preferences)
    return {
        "preferences": str(path),
        "parse_ok": True,
        "broad_count": len(broad),
        "broad": broad,
        "changed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--preferences",
        type=Path,
        default=Path.home() / ".claude-science" / "preferences.json",
    )
    parser.add_argument("--apply", action="store_true")
    parser.add_argument(
        "--convert-drvfs-rw-to-ro",
        action="store_true",
        help="atomically preserve read access while revoking persistent writes on /mnt/<drive>",
    )
    args = parser.parse_args()
    try:
        if args.convert_drvfs_rw_to_ro:
            result = apply_narrowing(args.preferences, convert_drvfs_to_ro=True)
        else:
            result = apply_narrowing(args.preferences) if args.apply else audit(args.preferences)
    except (OSError, ValueError, TypeError, RuntimeError) as error:
        print(json.dumps({"parse_ok": False, "changed": False, "error": str(error)}))
        return 1
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
