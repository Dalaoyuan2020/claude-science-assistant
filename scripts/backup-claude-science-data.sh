#!/usr/bin/env bash
set -euo pipefail

SOURCE_DIR="${1:-}"
BACKUP_DIR="${2:-}"

if [ -z "$SOURCE_DIR" ] || [ -z "$BACKUP_DIR" ]; then
  echo "Usage: backup-claude-science-data.sh SOURCE_DIR BACKUP_DIR" >&2
  exit 2
fi

SOURCE_DIR="$(realpath -m "$SOURCE_DIR")"
BACKUP_DIR="$(realpath -m "$BACKUP_DIR")"

if [ ! -f "$SOURCE_DIR/operon-cli.db" ]; then
  echo "Claude Science database not found: $SOURCE_DIR/operon-cli.db" >&2
  exit 2
fi
if [ -e "$BACKUP_DIR" ]; then
  echo "Backup destination already exists: $BACKUP_DIR" >&2
  exit 2
fi
case "$BACKUP_DIR/" in
  "$SOURCE_DIR/"*)
    echo "Backup destination must be outside the Claude Science data directory." >&2
    exit 2
    ;;
esac

umask 077
mkdir -p "$BACKUP_DIR/data"

python3 - "$SOURCE_DIR/operon-cli.db" "$BACKUP_DIR/data/operon-cli.db" <<'PY'
import sqlite3
import sys
from pathlib import Path

source = Path(sys.argv[1])
destination = Path(sys.argv[2])
src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
dst = sqlite3.connect(destination)
try:
    src.backup(dst)
finally:
    dst.close()
    src.close()
PY

for file in preferences.json encryption.key install-id; do
  if [ -f "$SOURCE_DIR/$file" ]; then
    cp -p "$SOURCE_DIR/$file" "$BACKUP_DIR/data/$file"
  fi
done

for directory in .oauth-tokens proxy skills; do
  if [ -d "$SOURCE_DIR/$directory" ]; then
    cp -a "$SOURCE_DIR/$directory" "$BACKUP_DIR/data/$directory"
  fi
done

python3 - "$SOURCE_DIR" "$BACKUP_DIR" <<'PY'
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

source = Path(sys.argv[1])
backup = Path(sys.argv[2])
database = backup / "data" / "operon-cli.db"
digest = hashlib.sha256(database.read_bytes()).hexdigest()
manifest = {
    "schemaVersion": 1,
    "createdAt": datetime.now(timezone.utc).isoformat(),
    "sourceDataDir": str(source),
    "databaseBytes": database.stat().st_size,
    "databaseSha256": digest,
    "includesSecrets": True,
    "scope": [
        "operon-cli.db",
        "preferences.json",
        "encryption.key",
        "install-id",
        ".oauth-tokens",
        "proxy",
        "skills",
    ],
    "excluded": ["artifacts", "conda", "r-libs", "runtime", "workspaces"],
}
(backup / "backup-manifest.json").write_text(
    json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
)
PY

chmod -R u+rwX,go-rwx "$BACKUP_DIR"
echo "Backup created: $BACKUP_DIR"
