#!/usr/bin/env bash
set -euo pipefail

project_root="$PWD"
task_kind="custom"
title="External Agent diagnosis"
note="Read-only diagnosis requested from Claude Science."
requested_action="diagnose"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --project-root) project_root="$2"; shift 2 ;;
    --task-kind) task_kind="$2"; shift 2 ;;
    --title) title="$2"; shift 2 ;;
    --note) note="$2"; shift 2 ;;
    --requested-action) requested_action="$2"; shift 2 ;;
    -h|--help)
      echo 'Usage: submit-request.sh --project-root PATH --task-kind KIND --title TITLE --note NOTE'
      exit 0
      ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$task_kind" in dataset|environment|vm|migration|custom) ;; *) echo "Invalid task kind" >&2; exit 2 ;; esac
case "$requested_action" in diagnose|plan|review) ;; *) echo "Invalid requested action" >&2; exit 2 ;; esac

export CSA_ROOT="$project_root" CSA_KIND="$task_kind" CSA_TITLE="$title"
export CSA_NOTE="$note" CSA_ACTION="$requested_action" CSA_CWD="$PWD"

python3 - <<'PY'
import datetime
import json
import os
import pathlib
import re
import uuid

root = pathlib.Path(os.environ["CSA_ROOT"]).resolve()
if not root.is_dir():
    raise SystemExit("Project root does not exist.")
title = os.environ["CSA_TITLE"]
note = os.environ["CSA_NOTE"]
cwd = os.environ["CSA_CWD"]
if len(title) > 240 or len(note) > 12000 or len(cwd) > 4096 or "\0" in title + note + cwd:
    raise SystemExit("Request fields are invalid or too long.")
patterns = (
    r"(?i)(api[_-]?key|(?:access[_-]?|refresh[_-]?|id[_-]?)?token|authorization|password|private[_-]?key|secret)\s*[:=]\s*\S+",
    r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{8,}",
    r"\bsk-[A-Za-z0-9_-]{12,}\b",
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----",
)
if any(re.search(pattern, title + "\n" + note) for pattern in patterns):
    raise SystemExit("Request appears to contain a credential; redact it first.")
inbox = root / "reports" / "csa-agent-inbox"
inbox.mkdir(parents=True, exist_ok=True)
stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
request_id = f"req-{stamp}-{uuid.uuid4().hex[:8]}"
payload = {
    "schemaVersion": 1,
    "source": "claude-science-skill",
    "taskKind": os.environ["CSA_KIND"],
    "title": title,
    "cwd": cwd,
    "note": note,
    "requestedAction": os.environ["CSA_ACTION"],
    "approvalMode": "manual",
    "policyId": "manual-only",
    "createdAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
target = inbox / f"{request_id}.json"
temporary = inbox / f".{request_id}.tmp"
temporary.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
temporary.replace(target)
print(request_id)
PY
