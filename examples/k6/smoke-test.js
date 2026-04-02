// Minimal k6 smoke test for Tumult loadtest plugin validation.
// Runs a simple check — no external HTTP target needed.
import { check, sleep } from 'k6';

export const options = {
  vus: 2,
  duration: '5s',
};

export default function () {
  const result = Math.random() > 0.01; // 99% success rate
  check(result, {
    'operation succeeded': (r) => r === true,
  });
  sleep(0.1);
}
