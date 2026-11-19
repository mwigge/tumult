#!/usr/bin/env bash
# Lint (or fix) the SQL of the lake schema with sqlfluff, so the standard
# that applies to docker/init-postgres.sql applies to the DuckDB schema as
# well. It covers:
#   - every `const NAME: &str = "..."` in tumult-lake/src/schema/*.rs but
#     the ones SKIP names below: the DDL, the migrations and the views held
#     in constants;
#   - the SQL that functions of tumult-lake/src/schema/mod.rs build (the
#     views of create_views() and the statements that use the other SQL
#     functions), as a test of tumult-lake keeps it in $BUILT.
# SQL in constants and format! calls elsewhere in the workspace is not
# covered.
#
#   .quality-venv/bin/pip install -r requirements-quality.txt
#   bash scripts/lint-embedded-sql.sh            # lint, non-zero on findings
#   bash scripts/lint-embedded-sql.sh --fix      # rewrite the constants in place
#
# Every constant becomes one temporary .sql file; the DuckDB dialect is used
# because the schema carries MAP, UBIGINT and JSON types the postgres
# dialect does not know. In --fix mode the formatted SQL is written back
# into the Rust source, escaping quotes as before. $BUILT is linted only:
# its SQL is changed in the functions, and the test then writes the file
# again (TUMULT_BLESS=1).
set -euo pipefail
MODE=${1:-lint}
SQLFLUFF=${SQLFLUFF:-.quality-venv/bin/sqlfluff}
BUILT=tumult-lake/tests/golden/sql-built-by-functions.sql
# Constants left out by name. The v8 rebuild copies `SELECT *`, by the
# position of the columns of the old table, and sqlfluff (AM04) asks for a
# column list. A list copies by name, which differs from the copy by
# position on a lake whose old table has its columns in another order: that
# is a change of meaning, not of layout.
export SKIP="MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
python3 - extract "$WORK" tumult-lake/src/schema/*.rs <<'PY'
import os, re, sys, pathlib
mode, work, *files = sys.argv[1:]
pat = re.compile(r'(const ([A-Z0-9_]+): &str = ")(\n?)(.*?)(";)', re.S)
skip = os.environ["SKIP"].split()
for f in files:
    text = pathlib.Path(f).read_text()
    for i, m in enumerate(pat.finditer(text)):
        if m.group(2) in skip:
            continue
        body = m.group(4).replace('\\"', '"')
        out = pathlib.Path(work) / f"{pathlib.Path(f).stem}__{i}.sql"
        out.write_text(body if body.endswith("\n") else body + "\n")
PY
count=$(ls "$WORK"/*.sql 2>/dev/null | wc -l)
echo "embedded SQL blocks: $count, and the SQL built by functions in $BUILT"
if [[ "$MODE" == "--fix" ]]; then
  "$SQLFLUFF" fix --dialect duckdb --config .sqlfluff "$WORK" >/dev/null || true
  python3 - "$WORK" tumult-lake/src/schema/*.rs <<'PY'
import os, re, sys, pathlib
work, *files = sys.argv[1:]
pat = re.compile(r'(const ([A-Z0-9_]+): &str = ")(\n?)(.*?)(";)', re.S)
skip = os.environ["SKIP"].split()
for f in files:
    p = pathlib.Path(f); text = p.read_text(); i = -1
    def rep(m):
        global i
        i += 1
        if m.group(2) in skip:
            return m.group(0)
        fixed = (pathlib.Path(work) / f"{p.stem}__{i}.sql").read_text()
        fixed = fixed.rstrip("\n") + "\n" if m.group(4).endswith("\n") else fixed.rstrip("\n")
        return m.group(1) + m.group(3) + fixed.replace('"', '\\"') + m.group(5)
    new = pat.sub(rep, text)
    if new != text:
        p.write_text(new); print("rewrote", f)
PY
  "$SQLFLUFF" lint --dialect duckdb --config .sqlfluff "$WORK" "$BUILT" | tail -3
else
  "$SQLFLUFF" lint --dialect duckdb --config .sqlfluff "$WORK" "$BUILT"
fi
