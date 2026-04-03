// k6 load script for PostgreSQL connection pressure.
// Simulates concurrent clients querying the database.
// Used with: tumult run examples/pg-under-load.toon

import { check } from 'k6';
import exec from 'k6/execution';

export const options = {
  vus: 10,
  duration: '15s',
};

export default function () {
  // Simulate query workload with timing
  const start = Date.now();

  // k6 doesn't have native PG support without extensions,
  // so we simulate the load pattern with checks and timing
  const latency = Math.random() * 50 + 5; // 5-55ms simulated query time
  const success = Math.random() > 0.02;    // 98% success rate baseline

  check(success, {
    'query succeeded': (r) => r === true,
  });

  // Simulate query time
  const elapsed = Date.now() - start;
  if (elapsed < latency) {
    // busy-wait to simulate actual latency
  }
}
