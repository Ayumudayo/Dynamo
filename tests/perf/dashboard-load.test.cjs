const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const zlib = require('node:zlib');
const fs = require('node:fs');

const {
  percentile,
  buildRequestOptions,
  normalizeLoopbackHostname,
  runLoad,
  runCli,
  validateLoadOptions,
  validateResultArtifact,
} = require('../../scripts/perf/dashboard-load.cjs');
const {
  validateHandoff,
  assertInstanceSnapshot,
  fetchInstanceSnapshot,
} = require('../../scripts/perf/assert-dashboard-instance.cjs');
const {
  assertBudgets,
  runCli: runBudgetCli,
} = require('../../scripts/perf/assert-budgets.cjs');

const NOW = 1_800_000_000_000;
const REVISION = 'a'.repeat(40);
const NONCE = 'b'.repeat(64);
const HASH = 'c'.repeat(64);

function temporaryDirectory(t) {
  const root = process.env.TEMP || process.env.TMP || process.cwd();
  const separator = root.includes('\\') ? '\\' : '/';
  const directory = fs.mkdtempSync(`${root}${separator}dynamo-perf-`);
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return { directory, separator };
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });
  return server.address().port;
}

async function close(server) {
  await new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}

function handoffFor(port, overrides = {}) {
  return {
    schema_version: 1,
    issued_at_unix_ms: NOW - 1_000,
    expires_at_unix_ms: NOW + 120_000,
    revision: REVISION,
    nonce: NONCE,
    pid: 4242,
    host: '127.0.0.1',
    port,
    dynamic_port: true,
    fixture_mode: 'Public',
    fixture: {
      version: 'public-v1',
      sha256: HASH,
    },
    source_state: {
      head: REVISION,
      clean: true,
      diff_sha256: 'd'.repeat(64),
    },
    environment: {
      fingerprint_sha256: 'e'.repeat(64),
      os_build: 'test-os',
      arch: 'x64',
      cpu_model: 'test-cpu',
      logical_cores: 8,
      power_profile: 'test-profile',
      node_version: process.version,
      rustc_version: 'rustc-test',
      cargo_profile: 'release',
    },
    ...overrides,
  };
}

function instanceFor(overrides = {}) {
  return {
    schema_version: 1,
    revision: REVISION,
    nonce: NONCE,
    pid: 4242,
    fixture_mode: 'Public',
    fixture: {
      version: 'public-v1',
      sha256: HASH,
    },
    outbound_calls: 0,
    browser_outbound_attempts: 0,
    ...overrides,
  };
}

function resultArtifact(overrides = {}) {
  return {
    schema_version: 1,
    runner_version: 'dashboard-load-v1',
    source_state: {
      head: REVISION,
      clean: true,
      diff_sha256: 'd'.repeat(64),
    },
    fixture: {
      version: 'public-v1',
      sha256: HASH,
    },
    environment: {
      fingerprint_sha256: 'e'.repeat(64),
      os_build: 'test-os',
      arch: 'x64',
      cpu_model: 'test-cpu',
      logical_cores: 8,
      power_profile: 'test-profile',
      node_version: process.version,
      rustc_version: 'rustc-test',
      cargo_profile: 'release',
    },
    instance: {
      revision: REVISION,
      nonce: NONCE,
      pid: 4242,
      fixture_mode: 'Public',
      outbound_calls_before: 0,
      outbound_calls_after: 0,
      browser_outbound_attempts: 0,
    },
    path: '/',
    requests: 20,
    concurrency: 4,
    ok: 20,
    failed: 0,
    decoded_bytes: 81_920,
    wire_bytes: 1_000,
    content_encodings: { gzip: 20 },
    p50_ms: 20,
    p95_ms: 25,
    max_ms: 30,
    statuses: { 200: 20 },
    ...overrides,
  };
}

test('runLoad records bounded concurrency, decoded and wire bytes, and percentiles', async (t) => {
  const body = Buffer.alloc(4096, 'a');
  const compressed = zlib.gzipSync(body);
  let active = 0;
  let maximumActive = 0;
  const server = http.createServer((_request, response) => {
    active += 1;
    maximumActive = Math.max(maximumActive, active);
    setTimeout(() => {
      response.writeHead(200, {
        'content-encoding': 'gzip',
        'content-type': 'text/plain',
      });
      response.end(compressed, () => {
        active -= 1;
      });
    }, 20);
  });
  t.after(() => close(server));
  const port = await listen(server);

  const result = await runLoad({
    url: `http://127.0.0.1:${port}/healthz`,
    requests: 20,
    concurrency: 4,
  });

  assert.equal(result.ok, 20);
  assert.equal(result.failed, 0);
  assert.equal(result.decoded_bytes, 20 * 4096);
  assert.ok(result.wire_bytes > 0);
  assert.ok(result.wire_bytes < result.decoded_bytes);
  assert.equal(result.content_encodings.gzip, 20);
  assert.deepEqual(result.statuses, { 200: 20 });
  assert.ok(result.p50_ms <= result.p95_ms);
  assert.ok(result.p95_ms <= result.max_ms);
  assert.ok(maximumActive <= 4);
});

test('percentile uses the nearest-rank definition', () => {
  const samples = [1, 2, 3, 4, 5];
  assert.equal(percentile(samples, 0.5), 3);
  assert.equal(percentile(samples, 0.95), 5);
  assert.equal(percentile([], 0.95), 0);
});

test('runLoad rejects invalid request and concurrency bounds before I/O', async () => {
  await assert.rejects(
    runLoad({
      url: 'http://127.0.0.1:49152/healthz',
      requests: 0,
      concurrency: 1,
    }),
    /requests/,
  );
  await assert.rejects(
    runLoad({
      url: 'http://127.0.0.1:49152/healthz',
      requests: 1,
      concurrency: 2,
    }),
    /concurrency cannot exceed requests/,
  );
});

test('runLoad counts HTTP 500 as a failed request without throwing', async (t) => {
  const server = http.createServer((_request, response) => {
    response.writeHead(500, { 'content-type': 'text/plain' });
    response.end('failed');
  });
  t.after(() => close(server));
  const port = await listen(server);

  const result = await runLoad({
    url: `http://127.0.0.1:${port}/failure`,
    requests: 2,
    concurrency: 1,
  });

  assert.equal(result.ok, 0);
  assert.equal(result.failed, 2);
  assert.deepEqual(result.statuses, { 500: 2 });
});

test('runLoad accounts for Brotli transfer and decoded bytes', async (t) => {
  const body = Buffer.alloc(2048, 'b');
  const compressed = zlib.brotliCompressSync(body);
  const server = http.createServer((_request, response) => {
    response.writeHead(200, { 'content-encoding': 'br' });
    response.end(compressed);
  });
  t.after(() => close(server));
  const port = await listen(server);

  const result = await runLoad({
    url: `http://127.0.0.1:${port}/brotli`,
    requests: 1,
    concurrency: 1,
  });

  assert.equal(result.ok, 1);
  assert.equal(result.content_encodings.br, 1);
  assert.equal(result.decoded_bytes, body.length);
  assert.equal(result.wire_bytes, compressed.length);
});

test('runLoad fails closed on corrupt and unsupported encodings', async (t) => {
  const server = http.createServer((request, response) => {
    if (request.url === '/corrupt') {
      response.writeHead(200, { 'content-encoding': 'gzip' });
      response.end('not-gzip');
      return;
    }
    response.writeHead(200, { 'content-encoding': 'deflate' });
    response.end('unsupported');
  });
  t.after(() => close(server));
  const port = await listen(server);

  const corrupt = await runLoad({
    url: `http://127.0.0.1:${port}/corrupt`,
    requests: 1,
    concurrency: 1,
  });
  const unsupported = await runLoad({
    url: `http://127.0.0.1:${port}/unsupported`,
    requests: 1,
    concurrency: 1,
  });

  assert.equal(corrupt.failed, 1);
  assert.equal(corrupt.ok, 0);
  assert.equal(unsupported.failed, 1);
  assert.equal(unsupported.ok, 0);
});

test('runLoad terminates a stalled response within the configured total timeout', async (t) => {
  const server = http.createServer((_request, _response) => {});
  t.after(() => close(server));
  const port = await listen(server);
  const started = Date.now();

  const result = await runLoad({
    url: `http://127.0.0.1:${port}/stall`,
    requests: 1,
    concurrency: 1,
    timeouts: { connect_ms: 25, header_ms: 40, body_ms: 40, total_ms: 60 },
  });

  assert.equal(result.failed, 1);
  assert.ok(Date.now() - started < 1_000);
});

test('runLoad terminates a body that stalls after response headers', async (t) => {
  const server = http.createServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'text/plain' });
    response.write('partial');
  });
  t.after(() => close(server));
  const port = await listen(server);

  const result = await runLoad({
    url: `http://127.0.0.1:${port}/body-stall`,
    requests: 1,
    concurrency: 1,
    timeouts: { connect_ms: 25, header_ms: 80, body_ms: 40, total_ms: 100 },
  });

  assert.equal(result.ok, 0);
  assert.equal(result.failed, 1);
  assert.deepEqual(result.statuses, { 200: 1 });
});

test('handoff validation rejects stale and non-loopback instances', () => {
  assert.throws(
    () => validateHandoff(handoffFor(49152, { expires_at_unix_ms: NOW - 1 }), NOW),
    /expired/,
  );
  assert.throws(
    () => validateHandoff(handoffFor(49152, { host: '192.0.2.10' }), NOW),
    /loopback/,
  );
});

test('IPv6 loopback URL brackets are normalized for the request boundary', () => {
  const validated = validateLoadOptions({
    url: 'http://[::1]:49152/healthz',
    requests: 1,
    concurrency: 1,
  });
  assert.equal(validated.url.hostname, '[::1]');
  assert.equal(normalizeLoopbackHostname(validated.url.hostname), '::1');
  assert.equal(buildRequestOptions(validated.url).hostname, '::1');
});

test('instance validation rejects stale identity and outbound drift', () => {
  const handoff = validateHandoff(handoffFor(49152), NOW);
  const identityMismatches = [
    instanceFor({ revision: 'f'.repeat(40) }),
    instanceFor({ nonce: 'f'.repeat(64) }),
    instanceFor({ pid: 4243 }),
    instanceFor({ fixture_mode: 'ReadOnly' }),
    instanceFor({ fixture: { version: 'public-v1', sha256: 'f'.repeat(64) } }),
  ];
  for (const snapshot of identityMismatches) {
    assert.throws(
      () => assertInstanceSnapshot(handoff, snapshot, 'pre-load'),
      /identity/,
    );
  }
  assert.throws(
    () => assertInstanceSnapshot(handoff, instanceFor({ outbound_calls: 1 }), 'post-load'),
    /outbound/,
  );
});

test('instance validation has a hard deadline even when the response drips bytes', async (t) => {
  const server = http.createServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.write('{');
    const interval = setInterval(() => response.write(' '), 5);
    response.on('close', () => clearInterval(interval));
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoff = validateHandoff(handoffFor(port), NOW);
  const started = Date.now();

  await assert.rejects(
    fetchInstanceSnapshot(handoff, { timeout_ms: 40 }),
    /timed out/,
  );
  assert.ok(Date.now() - started < 1_000);
});

test('runCli validates the instance before and after load and publishes one artifact', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const body = Buffer.alloc(4096, 'a');
  const compressed = zlib.gzipSync(body);
  let instanceReads = 0;
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      instanceReads += 1;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    if (request.url === '/measure') {
      response.writeHead(200, { 'content-encoding': 'gzip' });
      response.end(compressed);
      return;
    }
    response.writeHead(404);
    response.end();
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });

  const execution = await runCli({
    env: {
      PERF_INSTANCE_HANDOFF: handoffPath,
      PERF_PATH: '/measure',
      PERF_REQUESTS: '4',
      PERF_CONCURRENCY: '2',
      PERF_OUT: outputPath,
    },
    now: () => NOW,
    stdout: () => {},
  });

  assert.equal(execution.exitCode, 0);
  assert.equal(instanceReads, 2);
  const artifact = JSON.parse(fs.readFileSync(outputPath, 'utf8'));
  assert.equal(artifact.schema_version, 1);
  assert.equal(artifact.runner_version, 'dashboard-load-v1');
  assert.equal(artifact.path, '/measure');
  assert.equal(artifact.ok, 4);
  assert.equal(artifact.failed, 0);
  assert.equal(artifact.instance.outbound_calls_before, 0);
  assert.equal(artifact.instance.outbound_calls_after, 0);
  assert.equal(artifact.instance.browser_outbound_attempts, 0);
  const serialized = JSON.stringify(artifact);
  assert.doesNotMatch(serialized, /cookie|authorization|storage_state|query/i);
  assert.deepEqual(
    fs.readdirSync(directory).sort(),
    ['handoff.json', 'result.json'],
  );
});

test('runCli rejects expiry during pre-load validation before workload I/O', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  let instanceReads = 0;
  let workloadRequests = 0;
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      instanceReads += 1;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    workloadRequests += 1;
    response.writeHead(200, { 'content-type': 'text/plain' });
    response.end('must-not-run');
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(
    handoffPath,
    JSON.stringify(handoffFor(port, { expires_at_unix_ms: NOW + 1_000 })),
    { flag: 'wx' },
  );
  const clock = [NOW, NOW + 2_000];

  await assert.rejects(
    runCli({
      env: {
        PERF_INSTANCE_HANDOFF: handoffPath,
        PERF_PATH: '/measure',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: outputPath,
      },
      now: () => clock.shift(),
      stdout: () => {},
    }),
    /expired during pre-load validation/,
  );
  assert.equal(instanceReads, 1);
  assert.equal(workloadRequests, 0);
  assert.equal(fs.existsSync(outputPath), false);
  assert.deepEqual(fs.readdirSync(directory), ['handoff.json']);
});

test('runCli rejects post-load instance drift without publishing a result', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  let loaded = false;
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor({ outbound_calls: loaded ? 1 : 0 })));
      return;
    }
    loaded = true;
    response.writeHead(200);
    response.end('ok');
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });

  await assert.rejects(
    runCli({
      env: {
        PERF_INSTANCE_HANDOFF: handoffPath,
        PERF_PATH: '/measure',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: outputPath,
      },
      now: () => NOW,
      stdout: () => {},
    }),
    /outbound/,
  );
  assert.equal(fs.existsSync(outputPath), false);
  assert.deepEqual(fs.readdirSync(directory), ['handoff.json']);
});

test('runCli rejects a handoff that expires during post-load instance validation', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    response.writeHead(200, { 'content-type': 'text/plain' });
    response.end('ok');
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(
    handoffPath,
    JSON.stringify(handoffFor(port, { expires_at_unix_ms: NOW + 1_000 })),
    { flag: 'wx' },
  );
  const clock = [NOW, NOW, NOW, NOW + 2_000];

  await assert.rejects(
    runCli({
      env: {
        PERF_INSTANCE_HANDOFF: handoffPath,
        PERF_PATH: '/measure',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: outputPath,
      },
      now: () => clock.shift(),
      stdout: () => {},
    }),
    /expired during post-load validation/,
  );
  assert.equal(fs.existsSync(outputPath), false);
  assert.deepEqual(fs.readdirSync(directory), ['handoff.json']);
});

test('runCli rejects an existing output before any network request and preserves it', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  let requests = 0;
  const server = http.createServer((_request, response) => {
    requests += 1;
    response.writeHead(500);
    response.end();
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });
  fs.writeFileSync(outputPath, 'owned-by-caller', { flag: 'wx' });

  await assert.rejects(
    runCli({
      env: {
        PERF_INSTANCE_HANDOFF: handoffPath,
        PERF_PATH: '/measure',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: outputPath,
      },
      now: () => NOW,
      stdout: () => {},
    }),
    /already exists/,
  );
  assert.equal(requests, 0);
  assert.equal(fs.readFileSync(outputPath, 'utf8'), 'owned-by-caller');
});

test('runCli rejects caller base URLs and query-bearing paths', async () => {
  await assert.rejects(
    runCli({
      env: {
        PERF_BASE_URL: 'http://127.0.0.1:3000',
        PERF_INSTANCE_HANDOFF: 'unused',
        PERF_PATH: '/?secret=value',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: 'unused',
      },
      stdout: () => {},
    }),
    /PERF_BASE_URL/,
  );
  await assert.rejects(
    runCli({
      env: {
        PERF_INSTANCE_HANDOFF: 'unused',
        PERF_PATH: '/?secret=value',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: 'unused',
      },
      stdout: () => {},
    }),
    /query-free/,
  );
  await assert.rejects(
    runCli({
      env: {
        PERF_STORAGE_STATE: 'must-not-be-read.json',
        PERF_INSTANCE_HANDOFF: 'unused',
        PERF_PATH: '/measure',
        PERF_REQUESTS: '1',
        PERF_CONCURRENCY: '1',
        PERF_OUT: 'unused',
      },
      stdout: () => {},
    }),
    /disabled until the isolated runner validates same-instance cookie scope/,
  );
  for (const forbidden of ['PERF_COOKIE', 'PERF_HEADERS']) {
    await assert.rejects(
      runCli({
        env: {
          [forbidden]: 'must-not-be-read',
          PERF_INSTANCE_HANDOFF: 'unused',
          PERF_PATH: '/measure',
          PERF_REQUESTS: '1',
          PERF_CONCURRENCY: '1',
          PERF_OUT: 'unused',
        },
        stdout: () => {},
      }),
      new RegExp(`${forbidden} is forbidden`),
    );
  }
});

test('result artifacts reject network-path and control-character routes', () => {
  for (const path of ['//authority', '/line\rbreak', '/line\nbreak']) {
    assert.throws(
      () => validateResultArtifact(resultArtifact({ path })),
      /result path is invalid/,
    );
  }
});

test('runCli publishes failed request evidence and returns exit code 2', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    response.writeHead(500, { 'content-type': 'text/plain' });
    response.end('controlled failure');
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });

  const execution = await runCli({
    env: {
      PERF_INSTANCE_HANDOFF: handoffPath,
      PERF_PATH: '/measure',
      PERF_REQUESTS: '1',
      PERF_CONCURRENCY: '1',
      PERF_OUT: outputPath,
    },
    now: () => NOW,
    stdout: () => {},
  });

  assert.equal(execution.exitCode, 2);
  const artifact = JSON.parse(fs.readFileSync(outputPath, 'utf8'));
  assert.equal(artifact.ok, 0);
  assert.equal(artifact.failed, 1);
  assert.deepEqual(artifact.statuses, { 500: 1 });
});

test('runCli publishes corrupt 2xx encoding evidence as a failed request', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    response.writeHead(200, { 'content-encoding': 'gzip' });
    response.end('not-gzip');
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });

  const execution = await runCli({
    env: {
      PERF_INSTANCE_HANDOFF: handoffPath,
      PERF_PATH: '/measure',
      PERF_REQUESTS: '1',
      PERF_CONCURRENCY: '1',
      PERF_OUT: outputPath,
    },
    now: () => NOW,
    stdout: () => {},
  });

  assert.equal(execution.exitCode, 2);
  const artifact = JSON.parse(fs.readFileSync(outputPath, 'utf8'));
  assert.equal(artifact.ok, 0);
  assert.equal(artifact.failed, 1);
  assert.deepEqual(artifact.statuses, { 200: 1 });
  assert.deepEqual(artifact.content_encodings, { gzip: 1 });
});

test('concurrent publishers cannot overwrite the winning result', async (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const server = http.createServer((request, response) => {
    if (request.url === '/__perf/instance') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(instanceFor()));
      return;
    }
    setTimeout(() => {
      response.writeHead(200, { 'content-type': 'text/plain' });
      response.end('ok');
    }, 5);
  });
  t.after(() => close(server));
  const port = await listen(server);
  const handoffPath = `${directory}${separator}handoff.json`;
  const outputPath = `${directory}${separator}result.json`;
  fs.writeFileSync(handoffPath, JSON.stringify(handoffFor(port)), { flag: 'wx' });
  const options = () => ({
    env: {
      PERF_INSTANCE_HANDOFF: handoffPath,
      PERF_PATH: '/measure',
      PERF_REQUESTS: '1',
      PERF_CONCURRENCY: '1',
      PERF_OUT: outputPath,
    },
    now: () => NOW,
    stdout: () => {},
  });

  const outcomes = await Promise.allSettled([runCli(options()), runCli(options())]);
  assert.equal(outcomes.filter((outcome) => outcome.status === 'fulfilled').length, 1);
  assert.equal(outcomes.filter((outcome) => outcome.status === 'rejected').length, 1);
  assert.match(
    outcomes.find((outcome) => outcome.status === 'rejected').reason.message,
    /already exists/,
  );
  const artifact = JSON.parse(fs.readFileSync(outputPath, 'utf8'));
  assert.equal(artifact.ok, 1);
  assert.deepEqual(fs.readdirSync(directory).sort(), ['handoff.json', 'result.json']);
});

test('budget assertions enforce absolute limits and compatible ratio baselines', () => {
  const baseline = resultArtifact({ p95_ms: 25 });
  const current = resultArtifact({ p95_ms: 29.9 });

  const observations = assertBudgets({
    current,
    baseline,
    budget: {
      max_p95_ratio: 1.2,
      max_failed: 0,
      max_decoded_bytes_per_request: 4096,
      require_wire_lt_decoded: true,
      allowed_content_encodings: ['gzip'],
    },
  });

  assert.equal(observations.passed, true);
  assert.throws(
    () => assertBudgets({
      current: resultArtifact({ p95_ms: 31, max_ms: 31 }),
      baseline,
      budget: { max_p95_ratio: 1.2, max_failed: 0 },
    }),
    /p95 ratio/,
  );
  assert.throws(
    () => assertBudgets({
      current,
      baseline: resultArtifact({ path: '/different' }),
      budget: { max_p95_ratio: 1.2, max_failed: 0 },
    }),
    /compatible/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact({ p50_ms: 10, p95_ms: 11 }),
      budget: { max_p95_ms: 10, max_failed: 0 },
    }),
    /p95_ms/,
  );
  assert.throws(
    () => assertBudgets({
      current,
      baseline: resultArtifact({
        ok: 19,
        failed: 1,
        statuses: { 200: 19, 500: 1 },
      }),
      budget: { max_p95_ratio: 1.2, max_failed: 0 },
    }),
    /baseline failed requests/,
  );
  const compatibilityMismatches = [
    resultArtifact({
      fixture: { version: 'public-v1', sha256: 'f'.repeat(64) },
    }),
    resultArtifact({
      environment: {
        ...resultArtifact().environment,
        fingerprint_sha256: 'f'.repeat(64),
      },
    }),
    resultArtifact({
      instance: {
        ...resultArtifact().instance,
        fixture_mode: 'ReadOnly',
      },
    }),
  ];
  for (const incompatible of compatibilityMismatches) {
    assert.throws(
      () => assertBudgets({
        current,
        baseline: incompatible,
        budget: { max_p95_ratio: 1.2, max_failed: 0 },
      }),
      /not compatible/,
    );
  }
});

test('budget assertions reject missing baselines and unknown budget keys', () => {
  assert.throws(
    () => assertBudgets({
      current: resultArtifact(),
      budget: { max_p95_ratio: 1.2, max_failed: 0 },
    }),
    /baseline/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact(),
      budget: { max_failed: 0, permissive: true },
    }),
    /unknown budget key/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact(),
      budget: { max_failed: 0, require_wire_lt_decoded: false },
    }),
    /only be enabled with true/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact({ content_encodings: {} }),
      budget: { max_p95_ms: 30, max_failed: 0 },
    }),
    /missing response evidence/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact({ statuses: {} }),
      budget: { max_p95_ms: 30, max_failed: 0 },
    }),
    /missing response evidence/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact(),
      baseline: resultArtifact(),
      budget: { max_p95_ms: 30, max_failed: 0 },
    }),
    /only allowed for max_p95_ratio/,
  );
  assert.throws(
    () => assertBudgets({
      current: resultArtifact({ content_encodings: { br: 20 } }),
      budget: {
        max_failed: 0,
        allowed_content_encodings: ['gzip'],
      },
    }),
    /content encodings budget exceeded/,
  );
});

test('budget CLI reads validated files and emits only the safe decision', (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const currentPath = `${directory}${separator}current.json`;
  const budgetPath = `${directory}${separator}budget.json`;
  fs.writeFileSync(currentPath, JSON.stringify(resultArtifact()), { flag: 'wx' });
  fs.writeFileSync(
    budgetPath,
    JSON.stringify({ max_p95_ms: 30, max_failed: 0 }),
    { flag: 'wx' },
  );
  let output = '';

  const execution = runBudgetCli(
    [currentPath, budgetPath],
    (value) => { output += value; },
  );

  assert.equal(execution.exitCode, 0);
  assert.equal(execution.decision.passed, true);
  assert.deepEqual(JSON.parse(output), execution.decision);
  assert.doesNotMatch(output, /current\.json|budget\.json|cookie|authorization/i);
});

test('budget CLI emits observed and allowed values on threshold failure', (t) => {
  const { directory, separator } = temporaryDirectory(t);
  const currentPath = `${directory}${separator}current.json`;
  const budgetPath = `${directory}${separator}budget.json`;
  fs.writeFileSync(
    currentPath,
    JSON.stringify(resultArtifact({ p50_ms: 10, p95_ms: 11 })),
    { flag: 'wx' },
  );
  fs.writeFileSync(
    budgetPath,
    JSON.stringify({ max_p95_ms: 10, max_failed: 0 }),
    { flag: 'wx' },
  );
  let output = '';

  const execution = runBudgetCli(
    [currentPath, budgetPath],
    (value) => { output += value; },
  );

  assert.equal(execution.exitCode, 2);
  assert.equal(execution.decision.passed, false);
  assert.deepEqual(execution.decision.checks[0], {
    name: 'p95_ms',
    observed: 11,
    allowed: 10,
    passed: false,
  });
  assert.deepEqual(JSON.parse(output), execution.decision);
  assert.doesNotMatch(output, /current\.json|budget\.json|cookie|authorization/i);
});
