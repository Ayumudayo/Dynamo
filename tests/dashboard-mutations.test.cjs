const test = require('node:test');
const assert = require('node:assert/strict');
const transport = require('../crates/dashboard/assets/dashboard-mutations.js');

function response(status, body) {
  return { ok: status >= 200 && status < 300, status, text: async () => body };
}

test('classifies a rejected fetch as an unknown outcome', async () => {
  const result = await transport.request(async () => { throw new TypeError('offline'); }, '/settings');
  assert.deepEqual(result, {
    kind: 'unknown', reason: 'network',
    message: 'The network request did not finish. The outcome is unknown.',
  });
});

test('classifies an AbortController deadline as an unknown outcome', async () => {
  const result = await transport.request(
    async (_url, init) => new Promise((_resolve, reject) => {
      init.signal.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' })));
    }),
    '/settings', {}, { deadlineMs: 1 },
  );
  assert.equal(result.kind, 'unknown');
  assert.equal(result.reason, 'timeout');
});

test('treats a confirmed 4xx response as a definite failure', async () => {
  const result = await transport.request(async () => response(403, '{"message":"Access denied"}'), '/settings');
  assert.deepEqual(result, { kind: 'definite-failure', status: 403, message: 'Access denied' });
});

test('treats a confirmed 5xx non-JSON response as a definite failure', async () => {
  const result = await transport.request(async () => response(503, '<html>unavailable</html>'), '/settings');
  assert.deepEqual(result, { kind: 'definite-failure', status: 503, message: 'Request failed (503).' });
});

test('treats an unparseable successful response as an unknown outcome', async () => {
  const result = await transport.request(async () => response(200, 'not json'), '/settings');
  assert.equal(result.kind, 'unknown');
  assert.equal(result.reason, 'invalid-response');
});

test('returns success only after a confirmed JSON 2xx response', async () => {
  const result = await transport.request(async () => response(200, '{"status":"ok"}'), '/settings');
  assert.deepEqual(result, { kind: 'success', status: 200, body: { status: 'ok' } });
});
