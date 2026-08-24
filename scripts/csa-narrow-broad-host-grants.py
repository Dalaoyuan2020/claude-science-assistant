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
import time
from pathlib import Path
from typing import Any


BROAD_DRVFS_RW = re.compile(
    r"^rw:(/mnt/[A-Za-z](?:/(?:Downloads|Documents|Desktop)|"
    r"/Users/[^/]+/(?:Downloads|Documents|Desktop))?)/?$",
    re.IGNORECASE,
)
ANY_DRVFS_RW = re.compile(r"^rw:(/mnt/[A-Za-z](?:/.*)?)/?$", re.IGNORECASE)


def _migration_complete(preferences: dict[str, Any]) -> bool:
    migrated = preferences.get("_migratedToApprovalGrants", False)
    if not isinstance(migrated, bool):
        raise TypeError("_migratedToApprovalGrants must be a boolean")
    return migrated


def _approval_host_keys(preferences: dict[str, Any]) -> list[str]:
    """Return host keys loaded from either approvalGrants storage schema."""

    approval = preferences.get("approvalGrants", {})
    if isinstance(approval, list):
        host: list[str] = []
        for index, item in enumerate(approval):
            if not isinstance(item, dict):
                raise TypeError(f"approvalGrants[{index}] must be an object")
            kind = item.get("kind")
            key = item.get("key")
            if not isinstance(kind, str) or not isinstance(key, str):
                raise TypeError(
                    f"approvalGrants[{index}].kind and .key must be strings"
                )
            if kind == "host":
                host.append(key)
        return host
    if not isinstance(approval, dict):
        raise TypeError("approvalGrants must be an object or legacy array")
    always = approval.get("always", {})
    if not isinstance(always, dict):
        raise TypeError("approvalGrants.always must be an object")
    allow = always.get("allow", {})
    if not isinstance(allow, dict):
        raise TypeError("approvalGrants.always.allow must be an object")
    host = allow.get("host", [])
    if not isinstance(host, list):
        raise TypeError("approvalGrants.always.allow.host must be an array")
    for index, item in enumerate(host):
        if not isinstance(item, str):
            raise TypeError(
                f"approvalGrants.always.allow.host[{index}] must be a string"
            )
    return host


def _legacy_host_rows(preferences: dict[str, Any]) -> list[dict[str, Any]]:
    """Validate and return the legacy hostGrants array, if present."""

    if "hostGrants" not in preferences:
        return []
    legacy = preferences["hostGrants"]
    if not isinstance(legacy, list):
        raise TypeError("hostGrants must be an array")
    for index, item in enumerate(legacy):
        if not isinstance(item, dict):
            raise TypeError(f"hostGrants[{index}] must be an object")
        path = item.get("path")
        mode = item.get("mode")
        if not isinstance(path, str):
            raise TypeError(f"hostGrants[{index}].path must be a string")
        if mode not in {"ro", "rw"}:
            raise TypeError(f"hostGrants[{index}].mode must be 'ro' or 'rw'")
        expires_at = item.get("expiresAt")
        if expires_at is not None and (
            isinstance(expires_at, bool) or not isinstance(expires_at, (int, float))
        ):
            raise TypeError(f"hostGrants[{index}].expiresAt must be a number")
    return legacy


def _legacy_host_key(item: dict[str, Any]) -> str:
    return f"{item['mode']}:{item['path']}"


def _legacy_host_is_active(item: dict[str, Any], now_ms: float) -> bool:
    expires_at = item.get("expiresAt")
    # Match the vendor's `if (expiresAt && expiresAt <= Date.now())` check.
    return expires_at in (None, 0) or expires_at > now_ms


def _effective_host_keys(
    preferences: dict[str, Any], *, now_ms: float | None = None
) -> list[str]:
    """Reproduce the vendor load union without touching any granted path."""

    host = list(_approval_host_keys(preferences))
    legacy = _legacy_host_rows(preferences)
    migrated = _migration_complete(preferences)
    if not migrated:
        current_ms = time.time() * 1000 if now_ms is None else now_ms
        host.extend(
            _legacy_host_key(item)
            for item in legacy
            if _legacy_host_is_active(item, current_ms)
        )
    return host


def _broad_grants(preferences: dict[str, Any]) -> list[str]:
    return sorted(
        {
            item
            for item in _effective_host_keys(preferences)
            if BROAD_DRVFS_RW.fullmatch(item)
        }
    )


def _origin_containers(approval: dict[str, Any]) -> list[dict[str, Any]]:
    always = approval.get("always", {})
    candidates = [approval.get("alwaysOrigins")]
    if isinstance(always, dict):
        candidates.append(always.get("alwaysOrigins"))
    containers: list[dict[str, Any]] = []
    for candidate in candidates:
        if candidate is None or any(candidate is existing for existing in containers):
            continue
        if not isinstance(candidate, dict):
            raise TypeError("alwaysOrigins must be an object")
        containers.append(candidate)
    return containers


def narrow_preferences(preferences: dict[str, Any]) -> tuple[dict[str, Any], list[str]]:
    """Return a deep JSON copy with only exact broad global RW grants removed."""

    updated = json.loads(json.dumps(preferences))
    approval_keys = _approval_host_keys(updated)
    legacy = _legacy_host_rows(updated)
    migrated = _migration_complete(updated)
    current_ms = time.time() * 1000
    approval_broad = {
        item for item in approval_keys if BROAD_DRVFS_RW.fullmatch(item)
    }
    legacy_broad = {
        _legacy_host_key(item)
        for item in legacy
        if not migrated
        and _legacy_host_is_active(item, current_ms)
        and BROAD_DRVFS_RW.fullmatch(_legacy_host_key(item))
    }
    broad = sorted(approval_broad | legacy_broad)
    if not broad:
        return updated, []

    approval = updated.get("approvalGrants", {})
    if isinstance(approval, list):
        updated["approvalGrants"] = [
            item
            for item in approval
            if not (item["kind"] == "host" and item["key"] in approval_broad)
        ]
    else:
        always = approval.get("always", {})
        allow = always.get("allow", {})
        host = allow.get("host", [])
        allow["host"] = [item for item in host if item not in approval_broad]
        for container in _origin_containers(approval):
            host_origins = container.get("host", {})
            if not isinstance(host_origins, dict):
                raise TypeError("alwaysOrigins.host must be an object")
            for grant in approval_broad:
                host_origins.pop(grant, None)

    if not migrated and "hostGrants" in updated:
        updated["hostGrants"] = [
            item
            for item in legacy
            if not (
                _legacy_host_is_active(item, current_ms)
                and _legacy_host_key(item) in legacy_broad
            )
        ]

    return updated, broad


def convert_drvfs_writes_to_read_only(
    preferences: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, str]]:
    """Preserve access while converting every persistent DrvFS RW grant to RO."""

    updated = json.loads(json.dumps(preferences))
    approval_keys = _approval_host_keys(updated)
    legacy = _legacy_host_rows(updated)
    migrated = _migration_complete(updated)
    current_ms = time.time() * 1000
    approval = updated.get("approvalGrants", {})
    approval_conversions: dict[str, str] = {}
    for item in approval_keys:
        match = ANY_DRVFS_RW.fullmatch(item)
        if match:
            approval_conversions[item] = f"ro:{match.group(1).rstrip('/')}"
    legacy_conversions: dict[str, str] = {}
    if not migrated:
        for item in legacy:
            key = _legacy_host_key(item)
            match = ANY_DRVFS_RW.fullmatch(key)
            if _legacy_host_is_active(item, current_ms) and match:
                legacy_conversions[key] = f"ro:{match.group(1).rstrip('/')}"
    conversions = {**approval_conversions, **legacy_conversions}
    if not conversions:
        return updated, {}

    if isinstance(approval, list):
        for item in approval:
            if item["kind"] == "host" and item["key"] in approval_conversions:
                item["key"] = approval_conversions[item["key"]]
    else:
        always = approval.get("always", {})
        allow = always.get("allow", {})
        host = allow.get("host", [])
        projected: list[Any] = []
        for item in host:
            replacement = approval_conversions.get(item, item)
            if replacement not in projected:
                projected.append(replacement)
        allow["host"] = projected
        for container in _origin_containers(approval):
            host_origins = container.get("host", {})
            if not isinstance(host_origins, dict):
                raise TypeError("alwaysOrigins.host must be an object")
            for original, replacement in approval_conversions.items():
                origin = host_origins.pop(original, None)
                if origin is not None and replacement not in host_origins:
                    host_origins[replacement] = origin

    if not migrated:
        for item in legacy:
            if (
                _legacy_host_is_active(item, current_ms)
                and _legacy_host_key(item) in legacy_conversions
            ):
                item["mode"] = "ro"

    return updated, conversions


def _open_nofollow_flags(base: int) -> int:
    flags = base
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    return flags


def _read_regular_nofollow(
    path: Path, *, make_private: bool = False, owner: tuple[int, int] | None = None
) -> tuple[bytes, os.stat_result]:
    if not hasattr(os, "O_NOFOLLOW") and path.is_symlink():
        raise OSError(f"refusing symlink: {path}")
    descriptor = os.open(path, _open_nofollow_flags(os.O_RDONLY))
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise OSError(f"refusing non-regular file: {path}")
        if make_private:
            if hasattr(os, "fchmod"):
                os.fchmod(descriptor, 0o600)
            else:
                os.chmod(path, 0o600)
            if owner is not None and hasattr(os, "fchown"):
                try:
                    os.fchown(descriptor, owner[0], owner[1])
                except PermissionError:
                    pass
            os.fsync(descriptor)
            metadata = os.fstat(descriptor)
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            return stream.read(), metadata
    finally:
        os.close(descriptor)


def _write_exclusive(
    path: Path,
    payload: bytes,
    mode: int = 0o600,
    owner: tuple[int, int] | None = None,
) -> None:
    flags = _open_nofollow_flags(os.O_WRONLY | os.O_CREAT | os.O_EXCL)
    descriptor = os.open(path, flags, mode)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise OSError(f"refusing non-regular backup: {path}")
        if hasattr(os, "fchmod"):
            os.fchmod(descriptor, mode)
        else:
            os.chmod(path, mode)
        if owner is not None and hasattr(os, "fchown"):
            try:
                os.fchown(descriptor, owner[0], owner[1])
            except PermissionError:
                pass
        with os.fdopen(descriptor, "wb", closefd=False) as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        os.close(descriptor)


def _fsync_directory(path: Path) -> None:
    if not hasattr(os, "O_DIRECTORY"):
        return
    directory_fd = os.open(
        path, _open_nofollow_flags(os.O_RDONLY | os.O_DIRECTORY)
    )
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def apply_narrowing(path: Path, *, convert_drvfs_to_ro: bool = False) -> dict[str, Any]:
    original, metadata = _read_regular_nofollow(path)
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

    backup = path.with_name(f"{path.name}.csa-v0.1.6-{digest[:16]}.bak")
    try:
        backup_bytes, _ = _read_regular_nofollow(
            backup,
            make_private=True,
            owner=(metadata.st_uid, metadata.st_gid),
        )
        if backup_bytes != original:
            raise FileExistsError(f"backup exists with different content: {backup}")
    except FileNotFoundError:
        _write_exclusive(
            backup,
            original,
            owner=(metadata.st_uid, metadata.st_gid),
        )
        _fsync_directory(path.parent)

    encoded = (json.dumps(updated, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.csa-", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        if hasattr(os, "fchmod"):
            os.fchmod(descriptor, stat.S_IMODE(metadata.st_mode))
        else:
            os.chmod(temporary, stat.S_IMODE(metadata.st_mode))
        if hasattr(os, "fchown"):
            try:
                os.fchown(descriptor, metadata.st_uid, metadata.st_gid)
            except PermissionError:
                pass
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        current, current_metadata = _read_regular_nofollow(path)
        if (
            current != original
            or current_metadata.st_dev != metadata.st_dev
            or current_metadata.st_ino != metadata.st_ino
        ):
            raise RuntimeError("preferences changed concurrently; refusing to replace it")
        os.replace(temporary, path)
        _fsync_directory(path.parent)
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
