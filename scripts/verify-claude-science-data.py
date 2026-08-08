#!/usr/bin/env python3
import json
import sqlite3
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify-claude-science-data.py DATABASE", file=sys.stderr)
        return 2

    database = Path(sys.argv[1]).resolve()
    if not database.is_file():
        print(f"Database not found: {database}", file=sys.stderr)
        return 2

    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        quick_check = connection.execute("PRAGMA quick_check").fetchone()[0]
        user_version = connection.execute("PRAGMA user_version").fetchone()[0]
        page_count = connection.execute("PRAGMA page_count").fetchone()[0]
        table_count = connection.execute(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'"
        ).fetchone()[0]
    finally:
        connection.close()

    result = {
        "database": str(database),
        "bytes": database.stat().st_size,
        "quickCheck": quick_check,
        "userVersion": user_version,
        "pageCount": page_count,
        "tableCount": table_count,
    }
    print(json.dumps(result, indent=2))
    return 0 if quick_check == "ok" else 1


if __name__ == "__main__":
    raise SystemExit(main())
