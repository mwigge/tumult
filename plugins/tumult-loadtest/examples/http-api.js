// k6 load test: HTTP API endpoint
// Usage: k6 run --vus 10 --duration 30s http-api.js

import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
    thresholds: {
        http_req_duration: ['p(95)<500'],
        http_req_failed: ['rate<0.01'],
    },
};

const BASE_URL = __ENV.TARGET_URL || 'http://localhost:8080';

export default function () {
    const res = http.get(`${BASE_URL}/api/health`);

    check(res, {
        'status is 200': (r) => r.status === 200,
        'response time < 500ms': (r) => r.timings.duration < 500,
    });

    sleep(1);
}
