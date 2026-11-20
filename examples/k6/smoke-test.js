// k6 smoke test: GET requests against a running HTTP service.
//
// Target: the demo app's checkout route (`make demo` starts it on port
// 8080). Set TARGET_URL to aim it at another service.
import http from 'k6/http';
import { check, sleep } from 'k6';

const target = __ENV.TARGET_URL || 'http://127.0.0.1:8080/checkout/1';

export const options = {
  vus: 2,
  duration: '5s',
};

export default function () {
  const response = http.get(target, { timeout: '5s' });
  check(response, {
    'status is 200': (r) => r.status === 200,
  });
  sleep(0.1);
}
