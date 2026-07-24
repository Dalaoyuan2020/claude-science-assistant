#!/usr/bin/env bash
set -euo pipefail

project_root="$PWD"
request_id=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --project-root) project_root="$2"; shift 2 ;;
    --request-id) request_id="$2"; shift 2 ;;
    -h|--help) echo 'Usage: read-result.sh --project-root PATH --request-id ID'; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if ! printf '%s' "$request_id" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'; then
  echo "Invalid request ID." >&2
  exit 2
fi

result="$project_root/reports/csa-agent-outbox/$request_id.json"
if [ ! -f "$result" ]; then
  echo "Result is not available yet: $request_id" >&2
  exit 3
fi
python3 - "$result" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1]).resolve()
data = json.loads(path.read_text(encoding="utf-8-sig"))
result_path = str(data.get("resultPath", ""))
if result_path and (pathlib.PurePath(result_path).is_absolute() or ".." in pathlib.PurePath(result_path).parts):
    raise SystemExit("Unsafe resultPath in outbox record.")
print(json.dumps(data, ensure_ascii=False, indent=2))
PY
