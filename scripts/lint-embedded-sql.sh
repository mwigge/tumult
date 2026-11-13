#!/usr/bin/env bash
# Lint (or fix) the SQL the lake embeds in Rust string constants — the
# schema DDL and views in tumult-lake/src/schema/*.rs — with sqlfluff, so
# the standard that applies to docker/init-postgres.sql applies to the
# DuckDB schema as well.
#
#   .quality-venv/bin/pip install -r requirements-quality.txt
#   bash scripts/lint-embedded-sql.sh            # lint, non-zero on findings
#   bash scripts/lint-embedded-sql.sh --fix      # rewrite the constants in place
#
# Every `const NAME: &str = "..."` block becomes one temporary .sql file; the
# DuckDB dialect is used because the schema carries MAP, UBIGINT and JSON
# types the postgres dialect does not know. In --fix mode the formatted SQL
# is written back into the Rust source, escaping quotes as before.
set -euo pipefail
MODE=${1:-lint}
SQLFLUFF=${SQLFLUFF:-.quality-venv/bin/sqlfluff}
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
python3 - extract "$WORK" tumult-lake/src/schema/*.rs <<'PY'
import re, sys, pathlib
mode, work, *files = sys.argv[1:]
pat = re.compile(r'(const [A-Z_]+: &str = ")(\n?)(.*?)(";)', re.S)
for f in files:
    text = pathlib.Path(f).read_text()
    for i, m in enumerate(pat.finditer(text)):
        body = m.group(3).replace('\\"', '"')
        out = pathlib.Path(work) / f"{pathlib.Path(f).stem}__{i}.sql"
        out.write_text(body if body.endswith("\n") else body + "\n")
PY
count=$(ls "$WORK"/*.sql 2>/dev/null | wc -l)
echo "embedded SQL blocks: $count"
if [[ "$MODE" == "--fix" ]]; then
  "$SQLFLUFF" fix --dialect duckdb --config .sqlfluff "$WORK" >/dev/null || true
  python3 - "$WORK" tumult-lake/src/schema/*.rs <<'PY'
import re, sys, pathlib
work, *files = sys.argv[1:]
pat = re.compile(r'(const [A-Z_]+: &str = ")(\n?)(.*?)(";)', re.S)
for f in files:
    p = pathlib.Path(f); text = p.read_text(); i = -1
    def rep(m):
        global i
        i += 1
        fixed = (pathlib.Path(work) / f"{p.stem}__{i}.sql").read_text()
        fixed = fixed.rstrip("\n") + "\n" if m.group(3).endswith("\n") else fixed.rstrip("\n")
        return m.group(1) + m.group(2) + fixed.replace('"', '\\"') + m.group(4)
    new = pat.sub(rep, text)
    if new != text:
        p.write_text(new); print("rewrote", f)
PY
  "$SQLFLUFF" lint --dialect duckdb --config .sqlfluff "$WORK" | tail -3
else
  "$SQLFLUFF" lint --dialect duckdb --config .sqlfluff "$WORK"
fi
