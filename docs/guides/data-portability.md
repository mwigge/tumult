---
title: Data portability and recovery
parent: Guides
nav_order: 30
---

# Data portability and recovery

Tumult provides two different artifacts: a portable data archive and a complete operational database backup. The portable archive does not restore a working installation or its credentials.

## Portable data archive

`POST /api/lake/export` and the lake scheduler publish Parquet table snapshots in `KRONIKA_LAKE_DIR`. Format version 2 uses a content-addressed file per table version and an atomically published `_meta.json` manifest. Read exactly the files in that manifest's `snapshots` map. Do not glob every Parquet file: the directory also retains older table versions and may contain files from an interrupted export.

Each committed version contains complete table rows, including late-arriving telemetry, equal event timestamps, changed records and empty tables. Repeating an unchanged export verifies each file against the manifest’s SHA-256 checksum before reuse. Tumult’s snapshot reader rejects damaged files; other consumers should also verify `file_checksums` before loading rows. The next export rebuilds damaged files from retained database rows. Interrupted exports do not change the committed manifest; retries rebuild uncommitted files without verified checksums. Older version-2 manifests without `file_checksums` require a fresh export before reading. `watermark_ns` remains in the API for compatibility but now means snapshot publication time, **not** a boundary proving that every earlier event was received.

The archive includes telemetry, journals, manual evidence and attachment metadata, import provenance, definitions, runs, approval decisions, audit trails, schedules, graph and analysis tables. Attachment URIs are references: externally hosted attachment content must be archived separately by its owner.

Identity credential tables (`users`, `sessions`, `tokens`), authorization mappings, webhook configuration containing signing secrets, operational recovery plans, private execution fingerprints and schema bookkeeping are excluded. The manifest lists these exclusions. Ordinary telemetry, experiment definitions and user-provided evidence can themselves contain sensitive application data; this archive is not an automatic redaction service. Restrict its directory and delivery to authorized recipients. Tumult is currently a single-tenant store; the export covers that installation, not a selectable SaaS tenant.

Version 1 archives used event-time cursors and cannot reliably prove completeness for delayed records. Preserve existing archive files and restrict access to them. Earlier archives may contain password hashes and webhook signing secrets; treat them as privileged backups, not customer download bundles. For a new v2 archive, select a new empty `KRONIKA_LAKE_DIR` and export the full current hot store. Previously purged v1-only records require separate recovery/reconciliation; creating a v2 snapshot does not recover missing hot rows or silently merge historical glob files. Existing nonempty v1 archives are rejected with an actionable migration error.

## Retention

Keep `KRONIKA_RETENTION_DAYS=0` and `TUMULTD_RUN_RETENTION_DAYS=0` (the defaults). Nonzero values are rejected at startup. The explicit retention functions also refuse deletion. Normal reports and queries currently read the hot database, so deleting archived records would make those reports incomplete. Archive querying and a verified end-to-end retention policy must be implemented before enabling automatic hot deletion. Explicit administrative `tumult store purge` remains destructive analytics maintenance; it is not an archive-backed retention policy.

## Complete operational backup and restore

Stop `tumultd` and every other process holding the same database open for writes. Native DuckDB does not permit a separate CLI process to read a file held by a daemon writer. Use an encrypted volume and protect the backup like the live database: it includes password hashes, sessions/token hashes, signing secrets and operational recovery configuration.

```bash
TUMULT_LAKE_PATH=/srv/tumult/lake.duckdb tumult store backup --output /secure-backups/tumult-20261008

tumult store restore --input /secure-backups/tumult-20261008 --output /srv/tumult/restored.duckdb
```

The backup destination must not already exist. A successful backup contains `store.duckdb` and `manifest.json`, with a format version, schema version, explicit credential policy and SHA-256 checksum. It copies the whole database schema and data, including future tables, using [DuckDB database copy](https://duckdb.org/docs/current/sql/statements/copy#copy-from-database--to). On Unix the backup directory is owner-only and database files are mode `0600`.

Restore verifies the manifest and checksum, then publishes a new database file without replacing any existing database or WAL. It rejects incomplete or modified backups. Point `TUMULT_LAKE_PATH` at the restored database, review restored schedules and outbound integrations, then start the daemon. External secrets supplied through the environment, TLS private keys, plugin installations, external attachment bytes and the Parquet archive directory are deployment assets and must be backed up separately. Test recovery into an isolated installation before relying on a backup policy.

`tumult import <directory>` remains the legacy two-table analytics importer; it is not the restore command for operational backups.
