// k6 load script — real PostgreSQL queries under chaos.
//
// Requires: k6 v1.7+ (auto-provisions xk6-sql and postgres driver)
// Target: PostgreSQL on localhost:15432 (Tumult Docker stack)
//
// Creates a test table, inserts rows, and queries them continuously.
// When chaos kills connections, k6 captures errors as failed checks,
// proving the disruption in numbers.

import sql from 'k6/x/sql';
import driver from 'k6/x/sql/driver/postgres';
import { check, sleep } from 'k6';
import { Counter, Trend } from 'k6/metrics';

const pgErrors = new Counter('pg_errors');
const pgQueryMs = new Trend('pg_query_duration_ms');

const connStr =
  'postgres://tumult:tumult_test@localhost:15432/tumult_test?sslmode=disable';

export const options = {
  vus: 5,
  duration: '20s',
  thresholds: {
    checks: ['rate>0.70'],
    pg_query_duration_ms: ['p(95)<1000'],
  },
};

export function setup() {
  const db = sql.open(driver, connStr);
  db.exec(`
    CREATE TABLE IF NOT EXISTS tumult_chaos_test (
      id SERIAL PRIMARY KEY,
      value TEXT NOT NULL,
      created_at TIMESTAMP DEFAULT NOW()
    )
  `);
  db.exec('TRUNCATE TABLE tumult_chaos_test');
  db.close();
}

export default function () {
  const start = Date.now();
  let db;

  try {
    db = sql.open(driver, connStr);

    db.exec(
      "INSERT INTO tumult_chaos_test (value) VALUES ('k6-' || md5(random()::text))"
    );

    const rows = db.query(
      'SELECT count(*) AS cnt FROM tumult_chaos_test'
    );

    check(rows, {
      'query returned rows': (r) => r.length > 0,
    });

    pgQueryMs.add(Date.now() - start);
    db.close();
  } catch (e) {
    pgErrors.add(1);
    pgQueryMs.add(Date.now() - start);
    check(null, { 'pg connection available': () => false });
    if (db) {
      try {
        db.close();
      } catch (_) {
        /* already dead */
      }
    }
  }

  sleep(0.05);
}

export function teardown() {
  try {
    const db = sql.open(driver, connStr);
    db.exec('DROP TABLE IF EXISTS tumult_chaos_test');
    db.close();
  } catch (_) {
    /* PG may still be recovering */
  }
}
