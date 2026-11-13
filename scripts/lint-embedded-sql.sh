#!/usr/bin/env bash
# Lint the SQL the lake embeds in Rust string constants (the schema DDL in
# tumult-lake/src/schema/*.rs) with sqlfluff, so the standard that applies
# to docker/init-postgres.sql applies to the DuckDB schema as well.
#
#   .quality-venv/bin/pip install -r requirements-quality.txt
#   bash scripts/lint-embedded-sql.sh [path/to/sqlfluff]
#
# Each `const DDL: &str = "..."` block becomes one temporary .sql file; the
# DuckDB dialect is used because the schema carries MAP, UBIGINT and JSON
# types that the postgres dialect does not know.
set -euo pipefail
SQLFLUFF=${1:-.quality-venv/bin/sqlfluff}
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
n=0
for f in tumult-lake/src/schema/*.rs; do
  name=$(basename "$f" .rs)
  python3 - "$f" "$WORK/$name.sql" <<'PY'
import re, sys
src, dst = sys.argv[1], sys.argv[2]
text = open(src).read()
blocks = re.findall(r'const [A-Z_]+: &str = "\n?(.*?)";', text, re.S)
sql = "\n".join(b.replace('\\"', '"') for b in blocks)
# Drop the Rust-side line comments inside the strings (-- stays).
open(dst, "w").write(sql)
PY
  [ -s "$WORK/$name.sql" ] && n=$((n + 1))
done
echo "linting $n schema modules"
"$SQLFLUFF" lint --dialect duckdb --config .sqlfluff "$WORK"
