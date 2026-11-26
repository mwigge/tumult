# Shipped lake shapes

Each `.sql` file here makes again the lake that a release made on its first
open. Only six schema versions shipped. The tests in `../lake_shapes.rs`
load each shape and the rows of `seed.sql` into a new DuckDB file, open it
with today's `Store::open`, and require two things:

- the catalog is the catalog of a fresh lake, kept in `../golden/catalog.txt`;
- every row of the shape is still there, with the same values.

| File | Version | Release tag | Releases at this version |
|---|---|---|---|
| `v8-2.19.0.sql` | 8 | `v2.19.0` | 2.19.0 |
| `v9-2.20.0.sql` | 9 | `v2.20.0` | 2.20.0 |
| `v13-2.21.0.sql` | 13 | `v2.21.0` | 2.21.0 |
| `v15-2.22.0.sql` | 15 | `v2.22.0` | 2.22.0 |
| `v21-3.0.0.sql` | 21 | `v3.0.0` | 3.0.0 to 3.0.3 |
| `v26-3.0.4.sql` | 26 | `v3.0.4` | 3.0.4 and 3.0.5 |

3.0.2 and 3.0.3 make the same tables, indexes and rows as 3.0.0, and two
more views, `run_roots` and `run_completions`. Today's open makes every view
again, so `v21-3.0.0.sql` stands for all four. 3.0.5 makes the same lake as
3.0.4.

## How a shape is made

`scripts/build-lake-shapes.py` makes each file from its release tag. It
builds the tumult-lake crate of the tag with the `Cargo.lock` of the tag,
adds the example `scripts/lake-shapes/dump_shape.rs`, and runs it on a new
lake file. The example calls the `Store::open` of that release, so the
lake gets exactly the DDL that release ran. Then the DuckDB of that release
prints the catalog as SQL: the storage version of the file, the tables, the
indexes, the sequences, the rows the open wrote and the views. Last, the
example runs `seed.sql` on that lake, to prove that the seed rows fit the
shape. The DuckDB of 2.19.0 to 2.21.0 has no json extension built in and
fetches it on the first open, as those releases did.

A lake file keeps the storage version it was made with, also when a later
DuckDB writes to it. The lakes of 2.19.0 to 2.22.0 (DuckDB 1.5) are of
storage version v1.0.0, and those of 3.0.0 and later (DuckDB 2.0) of v2.0.0.
So a test makes the file of a shape with the storage version that the shape
names in its line `-- storage_compatibility_version = ...`.

Do not edit a shape by hand. Make it again:

```sh
python3 scripts/build-lake-shapes.py --target-dir DIR v8-2.19.0
```

## The seed rows

`seed.sql` is written by hand. Its inserts name their columns, and every
column exists since v8, so one file fits all the shapes. The rows touch the
run tables that the v5 rebuild copies, the manual records that the v8
rebuild copies, the users table, which must not gain the v6 legacy user,
and the tables that later versions add columns to.

## The known difference

A lake first made by 2.19.0 to 2.22.0 keeps the default of
`approval_requests.break_glass` as `CAST('f' AS BOOLEAN)`, the way DuckDB 1.5
wrote it. DuckDB 2.0 writes `false`. The value is the same. Today's open
does not set the default again, so the tests of the four 2.x shapes name
this one line, and fail when it goes away or another line differs.

## What is left out

- Versions 1 to 7. They never shipped: the lake grew from version 1 to 8
  between the tags `v2.18.0` and `v2.19.0`, so no release made them. Releases
  2.14.0 to 2.18.0 shipped the separate analytics store, in its own file
  `analytics.duckdb` and at its own versions 3 and 4, which mean other things
  than lake versions 3 and 4. That store reaches the lake by an import, not by
  an open.
- A lake that one release made and later releases opened in turn is not a
  fixture. Each file is the first open of one release. A check by hand
  opened one lake file with 2.19.0, 2.20.0, 2.21.0, 2.22.0, 3.0.0 and 3.0.4
  in turn, then with the open of main after 3.0.5. Its catalog differed
  from a fresh lake only in the known line above.
