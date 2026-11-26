#!/usr/bin/env python3
"""Make the fixtures of the shipped lake shapes from the release tags.

A shape is the lake that a release makes on its first open. For each shape
this script:

1. writes the tree of the release tag to a temporary folder (`git archive`);
2. adds the example scripts/lake-shapes/dump_shape.rs to the tumult-lake
   crate of that tree;
3. builds the example with the Cargo.lock of the tag, offline;
4. runs it on a new lake file in the temporary folder, with HOME there too.
   The example calls the `Store::open` of that release, so the lake gets
   the DDL that release ran. It then prints the catalog as SQL, with the
   DuckDB of that release: the storage version of the file, the tables,
   the indexes, the sequences, the rows the open wrote and the views. Last,
   it runs tumult-lake/tests/shapes/seed.sql on the lake, to prove that the
   seed rows fit that shape. The DuckDB of 2.19.0 to 2.21.0 has no json
   extension built in and fetches it on the first open, as those releases
   did; it lands in the HOME of the temporary folder;
5. writes the SQL to tumult-lake/tests/shapes/<shape>.sql.

The test tumult-lake/tests/lake_shapes.rs loads each file and the seed
rows into a new lake, upgrades it with today's open, and compares it with a
fresh lake.

Run it inside the memory limit of the other builds, for example:

    systemd-run --user --scope -p MemoryMax=12G \\
        python3 scripts/build-lake-shapes.py --target-dir DIR [shape ...]

usage: build-lake-shapes.py --target-dir DIR [--toolchain NAME] [--work DIR]
                            [shape ...]
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DUMP = ROOT / "scripts/lake-shapes/dump_shape.rs"
SHAPES_DIR = ROOT / "tumult-lake/tests/shapes"
SEED = SHAPES_DIR / "seed.sql"
# The shapes that releases shipped: the name of the file, the tag of the
# first release with that schema version, and the version.
SHAPES = (
    ("v8-2.19.0", "v2.19.0", 8),
    ("v9-2.20.0", "v2.20.0", 9),
    ("v13-2.21.0", "v2.21.0", 13),
    ("v15-2.22.0", "v2.22.0", 15),
    ("v21-3.0.0", "v3.0.0", 21),
    ("v26-3.0.4", "v3.0.4", 26),
)


def header(name: str, tag: str, version: int) -> str:
    release = tag.removeprefix("v")
    return (
        f"-- The shape v{version}: the lake that a first open of Tumult {release}"
        " makes.\n"
        f"-- Made by scripts/build-lake-shapes.py from the tag {tag}. Do not edit\n"
        f"-- it by hand: make it again with the script ({name}).\n"
    )


def extract(tag: str, tree: Path) -> None:
    tree.mkdir(parents=True)
    archive = subprocess.run(
        ["git", "-C", str(ROOT), "archive", tag], check=True, capture_output=True
    ).stdout
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive, check=True)
    examples = tree / "tumult-lake/examples"
    examples.mkdir(exist_ok=True)
    shutil.copy(DUMP, examples / "dump_shape.rs")


def build(tree: Path, toolchain: str, target_dir: Path) -> Path:
    subprocess.run(
        ["cargo", f"+{toolchain}", "build", "--offline", "--locked"]
        + ["-p", "tumult-lake", "--example", "dump_shape"]
        + ["--target-dir", str(target_dir)],
        cwd=tree,
        check=True,
    )
    return target_dir / "debug/examples/dump_shape"


def dump(binary: Path, work: Path) -> str:
    home = work / "home"
    home.mkdir()
    lake = work / "lake.duckdb"
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(home),
        "TUMULT_LAKE_PATH": str(lake),
    }
    return subprocess.run(
        [str(binary), str(lake), str(SEED)],
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout


def stored_version(sql: str) -> int | None:
    found = re.search(
        r"INSERT INTO schema_meta \(key, value\) VALUES "
        r"\('version', CAST\('(\d+)' AS BIGINT\)\);",
        sql,
    )
    return int(found[1]) if found else None


def make(
    name: str, tag: str, version: int, toolchain: str, args: argparse.Namespace
) -> None:
    work = args.work / name
    tree = work / "tree"
    extract(tag, tree)
    binary = build(tree, toolchain, args.target_dir)
    sql = dump(binary, work)
    found = stored_version(sql)
    if found != version:
        sys.exit(f"{name}: the lake of {tag} is at version {found}, not {version}")
    (SHAPES_DIR / f"{name}.sql").write_text(header(name, tag, version) + sql)
    print(f"{name}: written from {tag}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--toolchain", default="1.92.0")
    parser.add_argument(
        "--work",
        type=Path,
        help="an empty folder that keeps the trees and the lakes; by default "
        "a temporary folder that is removed at the end",
    )
    parser.add_argument("shapes", nargs="*", help="names of shapes, all by default")
    args = parser.parse_args()
    known = {name for name, _, _ in SHAPES}
    unknown = sorted(set(args.shapes) - known)
    if unknown:
        parser.error(f"no such shape: {', '.join(unknown)}")
    args.target_dir = args.target_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="lake-shapes-") as tmp:
        args.work = (args.work or Path(tmp)).resolve()
        for name, tag, version in SHAPES:
            if not args.shapes or name in args.shapes:
                make(name, tag, version, args.toolchain, args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
