//! The statements that read and write `entity_versions` for one batch.
//!
//! Their number does not grow with the batch. What the batch names or
//! changes travels to the engine as JSON arrays, a bounded piece at a time:
//!
//! * new rows are inserted straight from the array;
//! * the keys to read and the rows to update are first copied into two
//!   temporary tables of the writer's connection and joined from there.
//!   Joined to the array directly, the engine plans for one row and
//!   compares every stored row with every element: 5 000 updates took two
//!   seconds that way and 15 ms through the table.
//!
//! The temporary tables belong to the connection, hold one batch at most
//! and are never written to the lake file.

use std::collections::HashMap;

use duckdb::params;
use serde_json::{json, Value};

use super::{Fold, Latest, Prepared, Slot};
use crate::error::StoreError;
use crate::Writer;

/// Rows sent to the engine in one statement, at most.
const CHUNK_ROWS: usize = 1_000;

/// Bytes after which a statement is sent even with fewer rows. One row
/// carries a description and relationships of 64 KiB together at most, as
/// the conversion from the wire bounds them.
const CHUNK_BYTES: usize = 4 * 1024 * 1024;

const STAGING: &str = "
CREATE TEMP TABLE IF NOT EXISTS entity_batch_keys (
    entity_key VARCHAR,
    environment VARCHAR
);
CREATE TEMP TABLE IF NOT EXISTS entity_batch_changes (
    entity_key VARCHAR,
    environment VARCHAR,
    valid_from_ns BIGINT,
    last_seen_ns BIGINT,
    last_received_ns BIGINT,
    wire_form VARCHAR,
    deleted_at_ns BIGINT,
    delete_reason VARCHAR
);
DELETE FROM entity_batch_keys;
DELETE FROM entity_batch_changes;";

const STAGE_KEYS: &str = "
INSERT INTO entity_batch_keys
SELECT u.entity_key, u.environment
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        \"entity_key\": \"VARCHAR\", \"environment\": \"VARCHAR\"
    }]')) AS u
)";

/// Joined on the key and the environment: of a key that a sender planted
/// in many environments, the rows of those the batch names are read.
const LOAD_LATEST: &str = "
SELECT entity_key, environment, valid_from_ns, last_seen_ns, interval_s,
       wire_form, state_hash, deleted_at_ns, delete_reason, observer,
       last_received_ns
FROM entity_versions
WHERE (entity_key, environment) IN (
    SELECT entity_key, environment FROM entity_batch_keys
)
QUALIFY row_number() OVER (
    PARTITION BY entity_key, environment ORDER BY valid_from_ns DESC
) = 1";

/// The description and the relationships travel as text and are cast, so
/// the stored JSON is the canonical text the state hash was taken over.
const INSERT_VERSIONS: &str = "
INSERT INTO entity_versions (
    entity_key, environment, entity_type, entity_id, state_hash,
    description, relationships, valid_from_ns, last_seen_ns, interval_s,
    deleted_at_ns, delete_reason, wire_form, observer, schema_url,
    last_received_ns
)
SELECT u.entity_key, u.environment, u.entity_type, u.entity_id,
       u.state_hash, CAST(u.description AS JSON),
       CAST(u.relationships AS JSON), u.valid_from_ns, u.last_seen_ns,
       u.interval_s, u.deleted_at_ns, u.delete_reason, u.wire_form,
       u.observer, u.schema_url, u.last_received_ns
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        \"entity_key\": \"VARCHAR\", \"environment\": \"VARCHAR\",
        \"entity_type\": \"VARCHAR\",
        \"entity_id\": \"MAP(VARCHAR, VARCHAR)\",
        \"state_hash\": \"VARCHAR\", \"description\": \"VARCHAR\",
        \"relationships\": \"VARCHAR\", \"valid_from_ns\": \"BIGINT\",
        \"last_seen_ns\": \"BIGINT\", \"interval_s\": \"BIGINT\",
        \"deleted_at_ns\": \"BIGINT\", \"delete_reason\": \"VARCHAR\",
        \"wire_form\": \"VARCHAR\", \"observer\": \"VARCHAR\",
        \"schema_url\": \"VARCHAR\", \"last_received_ns\": \"BIGINT\"
    }]')) AS u
)";

const STAGE_CHANGES: &str = "
INSERT INTO entity_batch_changes
SELECT u.entity_key, u.environment, u.valid_from_ns, u.last_seen_ns,
       u.last_received_ns, u.wire_form, u.deleted_at_ns, u.delete_reason
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        \"entity_key\": \"VARCHAR\", \"environment\": \"VARCHAR\",
        \"valid_from_ns\": \"BIGINT\", \"last_seen_ns\": \"BIGINT\",
        \"last_received_ns\": \"BIGINT\", \"wire_form\": \"VARCHAR\",
        \"deleted_at_ns\": \"BIGINT\", \"delete_reason\": \"VARCHAR\"
    }]')) AS u
)";

/// The interval is not among the columns: another interval is another row.
const UPDATE_VERSIONS: &str = "
UPDATE entity_versions AS v
SET last_seen_ns = u.last_seen_ns,
    last_received_ns = u.last_received_ns,
    wire_form = u.wire_form,
    deleted_at_ns = u.deleted_at_ns,
    delete_reason = u.delete_reason
FROM entity_batch_changes AS u
WHERE v.entity_key = u.entity_key
    AND v.environment = u.environment
    AND v.valid_from_ns = u.valid_from_ns";

/// Hand `rows` to `run` as JSON arrays of at most [`CHUNK_ROWS`] elements
/// and little more than [`CHUNK_BYTES`]; nothing is run for no rows.
fn in_chunks(
    rows: impl Iterator<Item = Value>,
    mut run: impl FnMut(&str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut text = String::new();
    let mut held = 0;
    for row in rows {
        text.push(if held == 0 { '[' } else { ',' });
        text.push_str(&row.to_string());
        held += 1;
        if held < CHUNK_ROWS && text.len() < CHUNK_BYTES {
            continue;
        }
        text.push(']');
        run(&text)?;
        text.clear();
        held = 0;
    }
    if held == 0 {
        return Ok(());
    }
    text.push(']');
    run(&text)
}

fn version_json((key, environment): &Slot, row: &Latest) -> Option<Value> {
    let unsaved = row.unsaved.as_ref()?;
    Some(json!({
        "entity_key": key,
        "environment": environment,
        "entity_type": unsaved.entity_type,
        "entity_id": unsaved.entity_id,
        "state_hash": row.state_hash,
        "description": unsaved.description,
        "relationships": unsaved.relationships,
        "valid_from_ns": row.valid_from_ns,
        "last_seen_ns": row.last_seen_ns,
        "interval_s": row.interval_s,
        "deleted_at_ns": row.deleted_at_ns,
        "delete_reason": row.delete_reason,
        "wire_form": row.wire_form,
        "observer": row.observer,
        "schema_url": unsaved.schema_url,
        "last_received_ns": row.last_received_ns,
    }))
}

fn change_json((key, environment): &Slot, row: &Latest) -> Option<Value> {
    if row.unsaved.is_some() || !row.dirty {
        return None;
    }
    Some(json!({
        "entity_key": key,
        "environment": environment,
        "valid_from_ns": row.valid_from_ns,
        "last_seen_ns": row.last_seen_ns,
        "last_received_ns": row.last_received_ns,
        "wire_form": row.wire_form,
        "deleted_at_ns": row.deleted_at_ns,
        "delete_reason": row.delete_reason,
    }))
}

impl Writer {
    /// Empty the temporary tables, creating them on first use.
    fn clear_entity_staging(&self) -> Result<(), StoreError> {
        self.entities.ran();
        Ok(self.conn.execute_batch(STAGING)?)
    }

    /// Replace the staged keys with `slots`, each once.
    pub(super) fn stage_entity_keys<'k>(
        &self,
        slots: impl Iterator<Item = &'k Slot>,
    ) -> Result<(), StoreError> {
        self.clear_entity_staging()?;
        let mut slots: Vec<&Slot> = slots.collect();
        slots.sort_unstable();
        slots.dedup();
        let mut stage = self.conn.prepare(STAGE_KEYS)?;
        let staged = slots
            .into_iter()
            .map(|(key, environment)| json!({"entity_key": key, "environment": environment}));
        in_chunks(staged, |chunk| {
            self.entities.ran();
            stage.execute(params![chunk])?;
            Ok(())
        })
    }

    /// The latest stored row of every entity the batch names.
    pub(super) fn load_latest(
        &self,
        prepared: &[Prepared<'_>],
    ) -> Result<HashMap<Slot, Latest>, StoreError> {
        self.stage_entity_keys(prepared.iter().map(|item| &item.slot))?;
        self.entities.ran();
        let mut stmt = self.conn.prepare(LOAD_LATEST)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                Latest {
                    valid_from_ns: row.get(2)?,
                    last_seen_ns: row.get(3)?,
                    interval_s: row.get(4)?,
                    wire_form: row.get(5)?,
                    state_hash: row.get(6)?,
                    deleted_at_ns: row.get(7)?,
                    delete_reason: row.get(8)?,
                    observer: row.get(9)?,
                    last_received_ns: row.get(10)?,
                    unsaved: None,
                    dirty: false,
                    batch_ts_ns: None,
                },
            ))
        })?;
        let mut latest = HashMap::new();
        for row in rows {
            let (slot, value) = row?;
            latest.insert(slot, value);
        }
        Ok(latest)
    }

    /// Insert the rows the batch opened.
    pub(super) fn insert_versions(&self, fold: &Fold<'_>) -> Result<(), StoreError> {
        let mut insert = self.conn.prepare(INSERT_VERSIONS)?;
        let opened = fold
            .rows()
            .filter_map(|(slot, row)| version_json(slot, row));
        in_chunks(opened, |chunk| {
            self.entities.ran();
            insert.execute(params![chunk])?;
            Ok(())
        })
    }

    /// Write the mutable columns of the stored rows the batch changed, in
    /// one UPDATE whatever their number.
    pub(super) fn update_versions(&self, fold: &Fold<'_>) -> Result<(), StoreError> {
        let mut stage = self.conn.prepare(STAGE_CHANGES)?;
        let mut staged = false;
        let changed = fold.rows().filter_map(|(slot, row)| change_json(slot, row));
        in_chunks(changed, |chunk| {
            self.entities.ran();
            stage.execute(params![chunk])?;
            staged = true;
            Ok(())
        })?;
        if !staged {
            return Ok(());
        }
        self.entities.ran();
        self.conn.execute(UPDATE_VERSIONS, [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(rows: Vec<Value>) -> Vec<String> {
        let mut seen = Vec::new();
        in_chunks(rows.into_iter(), |chunk| {
            seen.push(chunk.to_owned());
            Ok(())
        })
        .unwrap();
        seen
    }

    /// A sender names the environment, so it can plant one key in as many
    /// as the lake holds. A batch reads the rows of the environments it
    /// names and no others.
    #[test]
    fn the_latest_rows_are_read_for_the_environments_the_batch_names() {
        use crate::{EntityEventKind, EntityEventRow, EntityLimits, Store, WireForm};
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let planted = |environment: &str| EntityEventRow {
            kind: EntityEventKind::State,
            ts_ns: 1,
            received_at_ns: 1,
            entity_type: "host".into(),
            entity_id: vec![("host.id".into(), "everywhere".into())],
            description: json!({}),
            relationships: json!([]),
            interval_s: None,
            delete_reason: None,
            wire_form: WireForm::Spec,
            observer: "tok:a".into(),
            schema_url: None,
            environment: environment.into(),
        };
        let events = [planted("one"), planted("two"), planted("three")];
        writer
            .apply_entity_events(&events, &EntityLimits::default())
            .unwrap();
        let named = [planted("two")];
        let prepared: Vec<Prepared<'_>> = named.iter().map(Prepared::new).collect();
        let latest = writer.load_latest(&prepared).unwrap();
        let environments: Vec<&str> = latest.keys().map(|slot| slot.1.as_str()).collect();
        assert_eq!(environments, vec!["two"]);
    }

    #[test]
    fn no_rows_run_nothing() {
        assert!(chunks(Vec::new()).is_empty());
    }

    #[test]
    fn rows_travel_as_json_arrays_of_bounded_length() {
        assert_eq!(chunks(vec![json!("a"), json!(1)]), vec![r#"["a",1]"#]);
        let many = chunks(vec![json!(0); CHUNK_ROWS * 2 + 1]);
        let lengths: Vec<usize> = many
            .iter()
            .map(|chunk| serde_json::from_str::<Vec<Value>>(chunk).unwrap().len())
            .collect();
        assert_eq!(lengths, vec![CHUNK_ROWS, CHUNK_ROWS, 1]);
    }

    #[test]
    fn a_chunk_is_sent_once_it_is_large_enough() {
        let large = json!("x".repeat(CHUNK_BYTES / 2));
        let sent = chunks(vec![large.clone(), large.clone(), large]);
        let lengths: Vec<usize> = sent
            .iter()
            .map(|chunk| serde_json::from_str::<Vec<Value>>(chunk).unwrap().len())
            .collect();
        assert_eq!(lengths, vec![2, 1]);
    }

    #[test]
    fn a_failing_statement_stops_the_batch() {
        let mut runs = 0;
        let failed = in_chunks(vec![json!(0); CHUNK_ROWS + 1].into_iter(), |_| {
            runs += 1;
            Err(StoreError::Internal("refused".into()))
        });
        assert!(failed.is_err());
        assert_eq!(runs, 1);
    }
}
