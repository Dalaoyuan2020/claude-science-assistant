import importlib.util
import json
import os
import stat
from pathlib import Path


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
