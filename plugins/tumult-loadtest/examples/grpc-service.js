// k6 load test: gRPC service
// Usage: k6 run --vus 5 --duration 30s grpc-service.js

import grpc from 'k6/net/grpc';
import { check, sleep } from 'k6';

const client = new grpc.Client();
client.load(['definitions'], 'service.proto');

const GRPC_ADDR = __ENV.GRPC_ADDR || 'localhost:9090';

export default function () {
    client.connect(GRPC_ADDR, { plaintext: true });

    const response = client.invoke('api.Service/HealthCheck', {});

    check(response, {
        'status is OK': (r) => r && r.status === grpc.StatusOK,
    });

    client.close();
    sleep(0.5);
}
