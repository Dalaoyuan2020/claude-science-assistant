#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(CDPATH= cd -- "$script_dir/.." && pwd)"
task_kind="custom"
title="Subagent request"
note="Read-only diagnosis requested from sandbox."
requested_action="diagnose"
approval_mode="manual"
policy_id="manual-only"
cwd="$(pwd)"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --project-root)
      project_root="$2"
      shift 2
      ;;
    --task-kind)
      task_kind="$2"
      shift 2
      ;;
    --title)
      title="$2"
      shift 2
      ;;
    --note)
      note="$2"
      shift 2
      ;;
    --requested-action)
      requested_action="$2"
      shift 2
      ;;
    --approval-mode)
      approval_mode="$2"
      shift 2
      ;;
    --policy-id)
      policy_id="$2"
      shift 2
      ;;
    --cwd)
      cwd="$2"
      shift 2
      ;;
    -h|--help)
      cat <<'USAGE'
Usage:
  scripts/create-subagent-request.sh \
    --task-kind dataset \
    --title "Dataset download diagnosis" \
    --note "Read-only: plan host-side checks before download."
USAGE
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

case "$task_kind" in
  dataset|environment|vm|migration|custom) ;;
  *)
    echo "Invalid --task-kind: $task_kind" >&2
    exit 2
    ;;
esac

case "$requested_action" in
  diagnose|plan|review) ;;
  *)
    echo "Invalid --requested-action: $requested_action" >&2
    exit 2
    ;;
esac

case "$approval_mode" in
  manual|autoCandidate) ;;
  *)
    echo "Invalid --approval-mode: $approval_mode" >&2
    exit 2
    ;;
esac

export CSA_SUBAGENT_PROJECT_ROOT="$project_root"
export CSA_SUBAGENT_TASK_KIND="$task_kind"
export CSA_SUBAGENT_TITLE="$title"
export CSA_SUBAGENT_NOTE="$note"
export CSA_SUBAGENT_REQUESTED_ACTION="$requested_action"
export CSA_SUBAGENT_APPROVAL_MODE="$approval_mode"
export CSA_SUBAGENT_POLICY_ID="$policy_id"
export CSA_SUBAGENT_CWD="$cwd"

python3 - <<'PY'
import datetime as _dt
import json
import os
import pathlib
import re
import uuid

root = pathlib.Path(os.environ["CSA_SUBAGENT_PROJECT_ROOT"]).resolve()
title = os.environ["CSA_SUBAGENT_TITLE"]
note = os.environ["CSA_SUBAGENT_NOTE"]
cwd = os.environ["CSA_SUBAGENT_CWD"]
if len(title) > 240 or len(note) > 12000 or len(cwd) > 4096:
    raise SystemExit("Subagent request fields are too long. Shorten the title, note, or cwd.")
if "\0" in title + note + cwd:
    raise SystemExit("Subagent request fields must not contain NUL characters.")
sensitive_text = title + "\n" + note
sensitive_patterns = (
    r"(?i)(api[_-]?key|(?:access[_-]?|refresh[_-]?|id[_-]?)?token|authorization|password|private[_-]?key|secret)\s*[:=]\s*\S+",
    r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{8,}",
    r"\bsk-[A-Za-z0-9_-]{12,}\b",
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----",
)
if any(re.search(pattern, sensitive_text) for pattern in sensitive_patterns):
    raise SystemExit(
        "Subagent request appears to contain a credential. Replace it with a redacted error summary."
    )
inbox = root / "reports" / "csa-agent-inbox"
inbox.mkdir(parents=True, exist_ok=True)
stamp = _dt.datetime.now().strftime("%Y%m%d-%H%M%S")
request_id = f"req-{stamp}-{uuid.uuid4().hex[:8]}"
path = inbox / f"{request_id}.json"
request = {
    "schemaVersion": 1,
    "source": "claude-science",
    "taskKind": os.environ["CSA_SUBAGENT_TASK_KIND"],
    "title": title,
    "cwd": cwd,
    "note": note,
    "requestedAction": os.environ["CSA_SUBAGENT_REQUESTED_ACTION"],
    "approvalMode": os.environ["CSA_SUBAGENT_APPROVAL_MODE"],
    "policyId": os.environ["CSA_SUBAGENT_POLICY_ID"],
    "createdAt": _dt.datetime.now(_dt.timezone.utc).isoformat(),
}
path.write_text(json.dumps(request, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print("Subagent request written:")
print(path)
PY
