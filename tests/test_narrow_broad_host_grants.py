import importlib.util
import json
import os
import stat
from pathlib import Path

import pytest


SCRIPT = Path(__file__).parents[1] / "scripts" / "csa-narrow-broad-host-grants.py"
SPEC = importlib.util.spec_from_file_location("csa_narrow_broad_host_grants", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def fixture_preferences():
    return {
        "approvalGrants": {
            "always": {"allow": {"host": ["rw:/mnt/e/Downloads", "ro:/mnt/e/datasets", "rw:/mnt/e/project"]}},
            "alwaysOrigins": {
                "host": {
                    "rw:/mnt/e/Downloads": {"project": "one"},
                    "rw:/mnt/e/project": {"project": "two"},
                }
            },
        },
        "hostGrants": [
            {"path": "/mnt/e/Downloads", "mode": "rw"},
            {"path": "/mnt/e/project", "mode": "rw"},
        ],
        "unrelated": {"keep": [1, 2, 3]},
    }


def test_narrowing_changes_only_exact_broad_grant_and_origin():
    original = fixture_preferences()
    updated, removed = MODULE.narrow_preferences(original)
    assert removed == ["rw:/mnt/e/Downloads"]
    assert updated["approvalGrants"]["always"]["allow"]["host"] == [
        "ro:/mnt/e/datasets",
        "rw:/mnt/e/project",
    ]
    assert list(updated["approvalGrants"]["alwaysOrigins"]["host"]) == ["rw:/mnt/e/project"]
    assert updated["hostGrants"] == [{"path": "/mnt/e/project", "mode": "rw"}]
    assert updated["unrelated"] == original["unrelated"]
    assert original == fixture_preferences()


def test_apply_is_atomic_backed_up_private_and_idempotent(tmp_path):
    path = tmp_path / "preferences.json"
    original = (json.dumps(fixture_preferences(), indent=2) + "\n").encode()
    path.write_bytes(original)
    path.chmod(0o600)

    first = MODULE.apply_narrowing(path)
    assert first["changed"] is True
    assert first["removed_count"] == 1
    backup = Path(first["backup"])
    assert backup.read_bytes() == original
    if os.name != "nt":
        assert stat.S_IMODE(backup.stat().st_mode) == 0o600
        assert stat.S_IMODE(path.stat().st_mode) == 0o600

    second = MODULE.apply_narrowing(path)
    assert second["changed"] is False
    assert second["removed_count"] == 0
    assert second["backup"] is None


def test_invalid_grant_shape_is_rejected_without_write(tmp_path):
    path = tmp_path / "preferences.json"
    original = b'{"approvalGrants":{"always":{"allow":{"host":null}}}}\n'
    path.write_bytes(original)
    try:
        MODULE.apply_narrowing(path)
    except TypeError:
        pass
    else:
        raise AssertionError("invalid host grant shape must fail closed")
    assert path.read_bytes() == original
    assert not list(tmp_path.glob("*.bak"))


def test_drvfs_rw_conversion_preserves_read_access_and_ext4_write():
    original = fixture_preferences()
    original["approvalGrants"]["always"]["allow"]["host"].extend(
        ["rw:/home/test/project", "rw:/mnt/e/datasets/mvtec_ad"]
    )
    original["approvalGrants"]["alwaysOrigins"]["host"][
        "rw:/mnt/e/datasets/mvtec_ad"
    ] = {"project": "dataset"}
    updated, conversions = MODULE.convert_drvfs_writes_to_read_only(original)
    assert conversions == {
        "rw:/mnt/e/Downloads": "ro:/mnt/e/Downloads",
        "rw:/mnt/e/project": "ro:/mnt/e/project",
        "rw:/mnt/e/datasets/mvtec_ad": "ro:/mnt/e/datasets/mvtec_ad",
    }
    host = updated["approvalGrants"]["always"]["allow"]["host"]
    assert "rw:/home/test/project" in host
    assert "rw:/mnt/e/datasets/mvtec_ad" not in host
    assert "ro:/mnt/e/datasets/mvtec_ad" in host
    origins = updated["approvalGrants"]["alwaysOrigins"]["host"]
    assert origins["ro:/mnt/e/datasets/mvtec_ad"] == {"project": "dataset"}
    assert updated["hostGrants"][0]["mode"] == "ro"


def test_legacy_only_active_host_grants_are_audited_narrowed_and_converted(tmp_path):
    original = {
        "approvalGrants": {"always": {"allow": {"host": []}}},
        "hostGrants": [
            {"path": "/mnt/e/Downloads", "mode": "rw", "createdAt": 1},
            {"path": "/mnt/e/project", "mode": "rw", "createdAt": 2},
            {"path": "/home/test/project", "mode": "rw", "createdAt": 3},
        ],
    }
    path = tmp_path / "preferences.json"
    path.write_text(json.dumps(original), encoding="utf-8")

    report = MODULE.audit(path)
    assert report["broad"] == ["rw:/mnt/e/Downloads"]

    narrowed, removed = MODULE.narrow_preferences(original)
    assert removed == ["rw:/mnt/e/Downloads"]
    assert narrowed["approvalGrants"]["always"]["allow"]["host"] == []
    assert narrowed["hostGrants"] == original["hostGrants"][1:]

    converted, conversions = MODULE.convert_drvfs_writes_to_read_only(original)
    assert conversions == {
        "rw:/mnt/e/Downloads": "ro:/mnt/e/Downloads",
        "rw:/mnt/e/project": "ro:/mnt/e/project",
    }
    assert [item["mode"] for item in converted["hostGrants"]] == ["ro", "ro", "rw"]


def test_legacy_approval_array_is_supported_for_narrow_and_convert():
    original = {
        "approvalGrants": [
            {
                "kind": "host",
                "key": "rw:/mnt/c/Documents",
                "createdAt": "2025-01-01T00:00:00Z",
            },
            {
                "kind": "host",
                "key": "rw:/mnt/c/project",
                "createdAt": "2025-01-02T00:00:00Z",
            },
            {
                "kind": "network",
                "key": "example.org",
                "createdAt": "2025-01-03T00:00:00Z",
            },
        ],
        "_migratedToApprovalGrants": True,
    }

    assert MODULE._broad_grants(original) == ["rw:/mnt/c/Documents"]
    narrowed, removed = MODULE.narrow_preferences(original)
    assert removed == ["rw:/mnt/c/Documents"]
    assert [item["key"] for item in narrowed["approvalGrants"]] == [
        "rw:/mnt/c/project",
        "example.org",
    ]

    converted, conversions = MODULE.convert_drvfs_writes_to_read_only(original)
    assert conversions == {
        "rw:/mnt/c/Documents": "ro:/mnt/c/Documents",
        "rw:/mnt/c/project": "ro:/mnt/c/project",
    }
    assert [item["key"] for item in converted["approvalGrants"]] == [
        "ro:/mnt/c/Documents",
        "ro:/mnt/c/project",
        "example.org",
    ]


def test_expired_legacy_host_grants_are_ignored_and_preserved():
    original = {
        "approvalGrants": {"always": {"allow": {"host": []}}},
        "hostGrants": [
            {
                "path": "/mnt/e/Downloads",
                "mode": "rw",
                "createdAt": 1,
                "expiresAt": 1,
            },
            {
                "path": "/mnt/e/Documents",
                "mode": "rw",
                "createdAt": 2,
                "expiresAt": 4_102_444_800_000,
            },
        ],
    }

    assert MODULE._broad_grants(original) == ["rw:/mnt/e/Documents"]
    narrowed, removed = MODULE.narrow_preferences(original)
    assert removed == ["rw:/mnt/e/Documents"]
    assert narrowed["hostGrants"] == [original["hostGrants"][0]]

    converted, conversions = MODULE.convert_drvfs_writes_to_read_only(original)
    assert conversions == {"rw:/mnt/e/Documents": "ro:/mnt/e/Documents"}
    assert converted["hostGrants"][0]["mode"] == "rw"
    assert converted["hostGrants"][1]["mode"] == "ro"


def test_migrated_true_ignores_but_preserves_legacy_host_grants():
    original = {
        "approvalGrants": {"always": {"allow": {"host": []}}},
        "_migratedToApprovalGrants": True,
        "hostGrants": [
            {"path": "/mnt/e/Downloads", "mode": "rw", "createdAt": 1}
        ],
    }

    assert MODULE._broad_grants(original) == []
    narrowed, removed = MODULE.narrow_preferences(original)
    assert removed == []
    assert narrowed == original
    converted, conversions = MODULE.convert_drvfs_writes_to_read_only(original)
    assert conversions == {}
    assert converted == original


def test_legacy_and_array_transformations_are_idempotent():
    legacy = {
        "hostGrants": [
            {"path": "/mnt/e/Downloads", "mode": "rw", "createdAt": 1}
        ]
    }
    first_narrow, removed = MODULE.narrow_preferences(legacy)
    second_narrow, removed_again = MODULE.narrow_preferences(first_narrow)
    assert removed == ["rw:/mnt/e/Downloads"]
    assert removed_again == []
    assert second_narrow == first_narrow

    old_array = {
        "approvalGrants": [
            {
                "kind": "host",
                "key": "rw:/mnt/e/project",
                "createdAt": "2025-01-01T00:00:00Z",
            }
        ]
    }
    first_convert, conversions = MODULE.convert_drvfs_writes_to_read_only(old_array)
    second_convert, conversions_again = MODULE.convert_drvfs_writes_to_read_only(
        first_convert
    )
    assert conversions == {"rw:/mnt/e/project": "ro:/mnt/e/project"}
    assert conversions_again == {}
    assert second_convert == first_convert


def test_preferences_symlink_is_refused_without_touching_target(tmp_path):
    target = tmp_path / "real-preferences.json"
    original = (json.dumps(fixture_preferences()) + "\n").encode()
    target.write_bytes(original)
    link = tmp_path / "preferences.json"
    try:
        link.symlink_to(target)
    except OSError as error:
        pytest.skip(f"symlinks unavailable: {error}")

    with pytest.raises(OSError):
        MODULE.apply_narrowing(link)
    assert target.read_bytes() == original
    assert not list(tmp_path.glob("*.bak"))


def test_existing_backup_symlink_is_refused(tmp_path):
    path = tmp_path / "preferences.json"
    original = (json.dumps(fixture_preferences()) + "\n").encode()
    path.write_bytes(original)
    digest = MODULE.hashlib.sha256(original).hexdigest()
    backup = path.with_name(f"{path.name}.csa-v0.1.6-{digest[:16]}.bak")
    decoy = tmp_path / "decoy"
    decoy.write_bytes(original)
    try:
        backup.symlink_to(decoy)
    except OSError as error:
        pytest.skip(f"symlinks unavailable: {error}")

    with pytest.raises(OSError):
        MODULE.apply_narrowing(path)
    assert path.read_bytes() == original
    assert decoy.read_bytes() == original
