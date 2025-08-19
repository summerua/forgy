import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
    stages: [
        { duration: '30s', target: 100 },
        { duration: '30s', target: 100 },
        { duration: '30s', target: 0 },
    ],
};

export default function () {
    const baseURL = 'http://localhost:3000';
    const url = baseURL + '/';

    let response = http.get(url);

    check(response, {
        'status was 200': (r) => r.status === 200,
    });

    sleep(1);
}