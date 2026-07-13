'use strict';

const http = require('node:http');
const https = require('node:https');
const zlib = require('node:zlib');
const { performance } = require('node:perf_hooks');
const fs = require('node:fs');
const crypto = require('node:crypto');

const {
  PerfContractError,
  fetchAndAssertInstance,
  loadAndValidateHandoff,
  validateEnvironment,
  validateFixture,
  validateSourceState,
} = require('./assert-dashboard-instance.cjs');

const RUNNER_VERSION = 'dashboard-load-v1';
const RESULT_SCHEMA_VERSION = 1;
const MAX_REQUESTS = 1_000_000;
const MAX_CONCURRENCY = 1_024;
const MAX_RESPONSE_BYTES = 16 * 1024 * 1024;
const LOOPBACK_HOSTS = new Set(['127.0.0.1', '::1']);
const SUPPORTED_ENCODINGS = new Set(['identity', 'gzip', 'br']);
const DEFAULT_TIMEOUTS = Object.freeze({
  connect_ms: 1_000,
  header_ms: 3_000,
  body_ms: 5_000,
  total_ms: 10_000,
});

class PerfRequestError extends Error {
  constructor(code, measurement = undefined) {
    super(code);
    this.name = 'PerfRequestError';
    this.code = code;
    this.measurement = measurement;
  }
}

function fail(message) {
  throw new PerfContractError(message);
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function assertExactKeys(value, expected, label) {
  if (!isPlainObject(value)) fail(`${label} must be an object`);
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    fail(`${label} fields do not match schema`);
  }
}

function assertSafeInteger(value, minimum, maximum, label) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    fail(`${label} must be an integer in the accepted range`);
  }
}

function assertFiniteNumber(value, minimum, label) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < minimum) {
    fail(`${label} must be a finite number in the accepted range`);
  }
}

function percentile(sortedSamples, proportion) {
  if (!Array.isArray(sortedSamples) || sortedSamples.length === 0) return 0;
  const index = Math.min(
    sortedSamples.length - 1,
    Math.max(0, Math.ceil(sortedSamples.length * proportion) - 1),
  );
  return sortedSamples[index];
}

function validatePositiveInteger(value, label, maximum) {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    fail(`${label} must be a positive integer in the accepted range`);
  }
  return value;
}

function normalizeLoopbackHostname(hostname) {
  return hostname === '[::1]' ? '::1' : hostname;
}

function validateTimeouts(value = DEFAULT_TIMEOUTS) {
  assertExactKeys(value, ['connect_ms', 'header_ms', 'body_ms', 'total_ms'], 'timeouts');
  const result = {};
  for (const key of ['connect_ms', 'header_ms', 'body_ms', 'total_ms']) {
    result[key] = validatePositiveInteger(value[key], `timeouts.${key}`, 120_000);
  }
  if (result.connect_ms > result.total_ms
      || result.header_ms > result.total_ms
      || result.body_ms > result.total_ms) {
    fail('phase timeouts cannot exceed the total timeout');
  }
  return result;
}

function validateLoadUrl(rawUrl) {
  if (typeof rawUrl !== 'string' || rawUrl.length === 0 || rawUrl.length > 2_048) {
    fail('load URL is invalid');
  }
  let parsed;
  try {
    parsed = new URL(rawUrl);
  } catch (_error) {
    fail('load URL is invalid');
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    fail('load URL protocol is invalid');
  }
  if (!LOOPBACK_HOSTS.has(normalizeLoopbackHostname(parsed.hostname))) {
    fail('load URL must use an exact loopback address');
  }
  if (parsed.username || parsed.password) fail('load URL credentials are forbidden');
  if (parsed.search || parsed.hash) fail('load URL query and fragment are forbidden');
  if (!parsed.port) fail('load URL must include the runner-assigned port');
  return parsed;
}

function validateLoadOptions(options) {
  assertExactKeys(
    options,
    options.timeouts === undefined
      ? ['url', 'requests', 'concurrency']
      : ['url', 'requests', 'concurrency', 'timeouts'],
    'load options',
  );
  const url = validateLoadUrl(options.url);
  const requests = validatePositiveInteger(options.requests, 'requests', MAX_REQUESTS);
  const concurrency = validatePositiveInteger(options.concurrency, 'concurrency', MAX_CONCURRENCY);
  if (concurrency > requests) fail('concurrency cannot exceed requests');
  return { url, requests, concurrency, timeouts: validateTimeouts(options.timeouts) };
}

function decodeBody(encoding, rawBody) {
  if (encoding === 'identity') return Promise.resolve(rawBody);
  return new Promise((resolve, reject) => {
    const callback = (error, decoded) => {
      if (error) {
        reject(new PerfRequestError('response body decompression failed'));
        return;
      }
      resolve(decoded);
    };
    const options = { maxOutputLength: MAX_RESPONSE_BYTES };
    if (encoding === 'gzip') zlib.gunzip(rawBody, options, callback);
    else if (encoding === 'br') zlib.brotliDecompress(rawBody, options, callback);
    else reject(new PerfRequestError('response content encoding is unsupported'));
  });
}

function buildRequestOptions(url) {
  return {
    protocol: url.protocol,
    hostname: normalizeLoopbackHostname(url.hostname),
    port: Number(url.port),
    path: url.pathname,
    method: 'GET',
    headers: {
      accept: '*/*',
      'accept-encoding': 'br, gzip',
    },
    agent: false,
  };
}

function requestRaw(url, timeouts) {
  const transport = url.protocol === 'https:' ? https : http;
  const options = buildRequestOptions(url);

  return new Promise((resolve, reject) => {
    let settled = false;
    let responseStatus;
    let responseEncoding;
    let responseWireBytes = 0;
    let connectTimer;
    let headerTimer;
    let bodyTimer;
    let totalTimer;

    const clearTimers = () => {
      for (const timer of [connectTimer, headerTimer, bodyTimer, totalTimer]) {
        if (timer) clearTimeout(timer);
      }
    };
    const measurement = () => ({
      status: responseStatus,
      contentEncoding: responseEncoding,
      wireBytes: responseWireBytes,
    });
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimers();
      if (error) {
        if (error instanceof PerfRequestError && error.measurement === undefined) {
          error.measurement = measurement();
        }
        reject(error);
      } else {
        resolve(value);
      }
    };
    const abort = (code) => {
      const error = new PerfRequestError(code, measurement());
      request.destroy();
      finish(error);
    };

    const request = transport.request(options, (response) => {
      if (headerTimer) clearTimeout(headerTimer);
      if (connectTimer) clearTimeout(connectTimer);
      responseStatus = response.statusCode;
      const rawEncoding = String(response.headers['content-encoding'] || 'identity')
        .trim()
        .toLowerCase();
      responseEncoding = rawEncoding || 'identity';
      if (!SUPPORTED_ENCODINGS.has(responseEncoding)) {
        response.resume();
        abort('response content encoding is unsupported');
        return;
      }

      const chunks = [];
      bodyTimer = setTimeout(() => abort('response body timed out'), timeouts.body_ms);
      response.on('data', (chunk) => {
        responseWireBytes += chunk.length;
        if (responseWireBytes > MAX_RESPONSE_BYTES) {
          abort('response body exceeded the size limit');
          return;
        }
        chunks.push(chunk);
      });
      response.on('end', () => {
        if (settled) return;
        if (bodyTimer) clearTimeout(bodyTimer);
        const rawBody = Buffer.concat(chunks);
        decodeBody(responseEncoding, rawBody)
          .then((decodedBody) => {
            if (decodedBody.length > MAX_RESPONSE_BYTES) {
              finish(new PerfRequestError('decoded response exceeded the size limit', measurement()));
              return;
            }
            finish(null, {
              status: responseStatus,
              ok: responseStatus >= 200 && responseStatus < 300,
              contentEncoding: responseEncoding,
              wireBytes: responseWireBytes,
              decodedBytes: decodedBody.length,
            });
          })
          .catch((error) => finish(error));
      });
      response.on('error', () => finish(new PerfRequestError('response stream failed', measurement())));
    });

    totalTimer = setTimeout(() => abort('request total deadline exceeded'), timeouts.total_ms);
    headerTimer = setTimeout(() => abort('response headers timed out'), timeouts.header_ms);
    request.on('socket', (socket) => {
      if (!socket.connecting) return;
      connectTimer = setTimeout(() => abort('request connection timed out'), timeouts.connect_ms);
      socket.once('connect', () => {
        if (connectTimer) clearTimeout(connectTimer);
      });
    });
    request.on('error', () => finish(new PerfRequestError('request failed', measurement())));
    request.end();
  });
}

function recordMeasurement(result, measurement) {
  if (!measurement) return;
  if (Number.isSafeInteger(measurement.status)) {
    const key = String(measurement.status);
    result.statuses[key] = (result.statuses[key] || 0) + 1;
  }
  if (typeof measurement.contentEncoding === 'string'
      && SUPPORTED_ENCODINGS.has(measurement.contentEncoding)) {
    result.content_encodings[measurement.contentEncoding]
      = (result.content_encodings[measurement.contentEncoding] || 0) + 1;
  }
  if (Number.isSafeInteger(measurement.wireBytes) && measurement.wireBytes >= 0) {
    result.wire_bytes += measurement.wireBytes;
  }
  if (Number.isSafeInteger(measurement.decodedBytes) && measurement.decodedBytes >= 0) {
    result.decoded_bytes += measurement.decodedBytes;
  }
}

async function runLoad(options) {
  const validated = validateLoadOptions(options);
  let nextRequest = 0;
  const samples = [];
  const result = {
    path: validated.url.pathname,
    requests: validated.requests,
    concurrency: validated.concurrency,
    ok: 0,
    failed: 0,
    decoded_bytes: 0,
    wire_bytes: 0,
    content_encodings: {},
    p50_ms: 0,
    p95_ms: 0,
    max_ms: 0,
    statuses: {},
  };

  async function worker() {
    while (true) {
      const index = nextRequest;
      nextRequest += 1;
      if (index >= validated.requests) return;
      const started = performance.now();
      try {
        const response = await requestRaw(validated.url, validated.timeouts);
        recordMeasurement(result, response);
        if (response.ok) result.ok += 1;
        else result.failed += 1;
      } catch (error) {
        if (!(error instanceof PerfRequestError)) throw error;
        result.failed += 1;
        recordMeasurement(result, error.measurement);
      } finally {
        samples.push(performance.now() - started);
      }
    }
  }

  await Promise.all(Array.from({ length: validated.concurrency }, () => worker()));
  samples.sort((left, right) => left - right);
  result.p50_ms = percentile(samples, 0.5);
  result.p95_ms = percentile(samples, 0.95);
  result.max_ms = samples.at(-1) ?? 0;
  return result;
}

function validateCountMap(value, label, keyValidator) {
  if (!isPlainObject(value)) fail(`${label} must be an object`);
  const result = {};
  for (const [key, count] of Object.entries(value)) {
    if (!keyValidator(key)) fail(`${label} contains an invalid key`);
    assertSafeInteger(count, 1, MAX_REQUESTS, `${label} count`);
    result[key] = count;
  }
  return result;
}

function validateResultArtifact(value) {
  const topLevelKeys = [
    'schema_version',
    'runner_version',
    'source_state',
    'fixture',
    'environment',
    'instance',
    'path',
    'requests',
    'concurrency',
    'ok',
    'failed',
    'decoded_bytes',
    'wire_bytes',
    'content_encodings',
    'p50_ms',
    'p95_ms',
    'max_ms',
    'statuses',
  ];
  assertExactKeys(value, topLevelKeys, 'result artifact');
  if (value.schema_version !== RESULT_SCHEMA_VERSION) fail('result schema is unsupported');
  if (value.runner_version !== RUNNER_VERSION) fail('result runner version is unsupported');
  const sourceState = validateSourceState(value.source_state);
  const fixture = validateFixture(value.fixture);
  const environment = validateEnvironment(value.environment);
  assertExactKeys(
    value.instance,
    [
      'revision',
      'nonce',
      'pid',
      'fixture_mode',
      'outbound_calls_before',
      'outbound_calls_after',
      'browser_outbound_attempts',
    ],
    'result instance',
  );
  if (value.instance.revision !== sourceState.head) fail('result instance revision mismatch');
  if (!/^[0-9a-f]{64}$/.test(value.instance.nonce)) fail('result instance nonce is invalid');
  assertSafeInteger(value.instance.pid, 1, 4_294_967_295, 'result instance pid');
  if (!['Public', 'GuildDetail', 'ReadOnly'].includes(value.instance.fixture_mode)) {
    fail('result fixture mode is invalid');
  }
  for (const key of ['outbound_calls_before', 'outbound_calls_after', 'browser_outbound_attempts']) {
    assertSafeInteger(value.instance[key], 0, Number.MAX_SAFE_INTEGER, `result instance ${key}`);
  }
  if (value.instance.outbound_calls_before !== 0
      || value.instance.outbound_calls_after !== 0
      || value.instance.browser_outbound_attempts !== 0) {
    fail('result contains outbound activity');
  }
  if (typeof value.path !== 'string'
      || !value.path.startsWith('/')
      || value.path.startsWith('//')
      || value.path.includes('?')
      || value.path.includes('#')
      || /[\r\n]/.test(value.path)
      || value.path.length > 2_048) {
    fail('result path is invalid');
  }
  assertSafeInteger(value.requests, 1, MAX_REQUESTS, 'result requests');
  assertSafeInteger(value.concurrency, 1, MAX_CONCURRENCY, 'result concurrency');
  if (value.concurrency > value.requests) fail('result concurrency exceeds requests');
  assertSafeInteger(value.ok, 0, value.requests, 'result ok');
  assertSafeInteger(value.failed, 0, value.requests, 'result failed');
  if (value.ok + value.failed !== value.requests) fail('result request counts do not reconcile');
  assertSafeInteger(value.decoded_bytes, 0, Number.MAX_SAFE_INTEGER, 'result decoded bytes');
  assertSafeInteger(value.wire_bytes, 0, Number.MAX_SAFE_INTEGER, 'result wire bytes');
  const encodings = validateCountMap(
    value.content_encodings,
    'result content encodings',
    (key) => SUPPORTED_ENCODINGS.has(key),
  );
  const statuses = validateCountMap(
    value.statuses,
    'result statuses',
    (key) => /^(?:[1-5][0-9]{2})$/.test(key),
  );
  const encodingResponses = Object.values(encodings).reduce((sum, count) => sum + count, 0);
  const statusResponses = Object.values(statuses).reduce((sum, count) => sum + count, 0);
  const successfulStatuses = Object.entries(statuses)
    .filter(([status]) => status.startsWith('2'))
    .reduce((sum, [, count]) => sum + count, 0);
  if (encodingResponses > value.requests || statusResponses > value.requests) {
    fail('result response counts exceed requests');
  }
  if (encodingResponses < value.ok || statusResponses < value.ok) {
    fail('result successful requests are missing response evidence');
  }
  if (successfulStatuses < value.ok
      || successfulStatuses - value.ok > value.failed
      || statusResponses - successfulStatuses > value.failed) {
    fail('result status classes do not reconcile with request outcomes');
  }
  for (const key of ['p50_ms', 'p95_ms', 'max_ms']) {
    assertFiniteNumber(value[key], 0, `result ${key}`);
  }
  if (value.p50_ms > value.p95_ms || value.p95_ms > value.max_ms) {
    fail('result percentile ordering is invalid');
  }
  return {
    ...value,
    source_state: sourceState,
    fixture,
    environment,
    instance: { ...value.instance },
    content_encodings: encodings,
    statuses,
  };
}

function parseIntegerEnvironment(value, key, maximum) {
  if (typeof value !== 'string' || !/^[1-9][0-9]*$/.test(value)) {
    fail(`${key} must be a positive decimal integer`);
  }
  return validatePositiveInteger(Number(value), key, maximum);
}

function validateRoutePath(value) {
  if (typeof value !== 'string'
      || value.length === 0
      || value.length > 2_048
      || !value.startsWith('/')
      || value.startsWith('//')
      || value.includes('?')
      || value.includes('#')
      || /[\r\n]/.test(value)) {
    fail('PERF_PATH must be an absolute query-free route path');
  }
  return value;
}

function isAbsoluteFilePath(value) {
  return typeof value === 'string'
    && value.length > 0
    && value.length <= 32_767
    && (/^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\\\') || value.startsWith('/'));
}

function splitFilePath(value) {
  if (!isAbsoluteFilePath(value)) fail('PERF_OUT must be an absolute file path');
  const index = Math.max(value.lastIndexOf('/'), value.lastIndexOf('\\'));
  if (index <= 0 || index === value.length - 1) fail('PERF_OUT must name a file');
  return {
    directory: value.slice(0, index),
    separator: value[index],
    name: value.slice(index + 1),
  };
}

function assertOutputAvailable(outputPath) {
  const parts = splitFilePath(outputPath);
  let directoryMetadata;
  try {
    directoryMetadata = fs.lstatSync(parts.directory);
  } catch (_error) {
    fail('PERF_OUT parent directory could not be opened');
  }
  if (!directoryMetadata.isDirectory() || directoryMetadata.isSymbolicLink()) {
    fail('PERF_OUT parent must be a regular directory');
  }
  try {
    fs.lstatSync(outputPath);
    fail('PERF_OUT already exists');
  } catch (error) {
    if (error instanceof PerfContractError) throw error;
    if (!error || error.code !== 'ENOENT') fail('PERF_OUT availability could not be verified');
  }
  return parts;
}

function publishExclusiveJson(outputPath, value, knownParts = undefined) {
  const parts = knownParts || assertOutputAvailable(outputPath);
  const body = `${JSON.stringify(value)}\n`;
  const temporaryPath = `${parts.directory}${parts.separator}.${parts.name}.${process.pid}.${crypto.randomBytes(16).toString('hex')}.tmp`;
  let descriptor;
  let linked = false;
  try {
    descriptor = fs.openSync(temporaryPath, 'wx', 0o600);
    fs.writeFileSync(descriptor, body, { encoding: 'utf8' });
    fs.fsyncSync(descriptor);
    fs.closeSync(descriptor);
    descriptor = undefined;
    const readback = fs.readFileSync(temporaryPath, 'utf8');
    if (readback !== body) fail('temporary result readback failed');
    try {
      fs.linkSync(temporaryPath, outputPath);
      linked = true;
    } catch (error) {
      if (error && error.code === 'EEXIST') fail('PERF_OUT already exists');
      fail('exclusive result publication failed');
    }
    fs.unlinkSync(temporaryPath);
    return;
  } catch (error) {
    if (descriptor !== undefined) {
      try { fs.closeSync(descriptor); } catch (_closeError) { /* owned descriptor cleanup */ }
    }
    try { fs.unlinkSync(temporaryPath); } catch (_cleanupError) { /* owned temp only */ }
    if (error instanceof PerfContractError) throw error;
    if (linked) fail('result was published but final cleanup failed');
    fail('result publication failed');
  }
}

function createBaseUrl(handoff) {
  const host = handoff.host === '::1' ? '[::1]' : handoff.host;
  return `http://${host}:${handoff.port}`;
}

async function runCli(options = {}) {
  const env = options.env || process.env;
  const stdout = options.stdout || ((value) => process.stdout.write(value));
  const now = options.now || Date.now;
  if (Object.prototype.hasOwnProperty.call(env, 'PERF_BASE_URL')) {
    fail('PERF_BASE_URL is forbidden; use the runner-issued handoff');
  }
  for (const forbidden of ['PERF_COOKIE', 'PERF_HEADERS']) {
    if (Object.prototype.hasOwnProperty.call(env, forbidden)) {
      fail(`${forbidden} is forbidden`);
    }
  }
  if (Object.prototype.hasOwnProperty.call(env, 'PERF_STORAGE_STATE')) {
    fail('PERF_STORAGE_STATE is disabled until the isolated runner validates same-instance cookie scope');
  }
  const routePath = validateRoutePath(env.PERF_PATH);
  const requests = parseIntegerEnvironment(env.PERF_REQUESTS, 'PERF_REQUESTS', MAX_REQUESTS);
  const concurrency = parseIntegerEnvironment(env.PERF_CONCURRENCY, 'PERF_CONCURRENCY', MAX_CONCURRENCY);
  if (concurrency > requests) fail('PERF_CONCURRENCY cannot exceed PERF_REQUESTS');
  const outputPath = env.PERF_OUT;
  const outputParts = assertOutputAvailable(outputPath);
  const handoff = loadAndValidateHandoff(env.PERF_INSTANCE_HANDOFF, now());
  const instanceOptions = options.instanceRequestOptions || {};
  const before = await fetchAndAssertInstance(handoff, 'pre-load', instanceOptions);
  if (now() > handoff.expires_at_unix_ms) {
    fail('instance handoff expired during pre-load validation');
  }
  const loadOptions = {
    url: `${createBaseUrl(handoff)}${routePath}`,
    requests,
    concurrency,
  };
  if (options.requestTimeouts) loadOptions.timeouts = options.requestTimeouts;
  const measurement = await runLoad(loadOptions);
  if (now() > handoff.expires_at_unix_ms) fail('instance handoff expired during load');
  const after = await fetchAndAssertInstance(handoff, 'post-load', instanceOptions);
  if (now() > handoff.expires_at_unix_ms) {
    fail('instance handoff expired during post-load validation');
  }
  if (before.outbound_calls !== after.outbound_calls) fail('post-load outbound counter drift detected');
  if (before.browser_outbound_attempts !== after.browser_outbound_attempts) {
    fail('post-load browser outbound counter drift detected');
  }

  const artifact = validateResultArtifact({
    schema_version: RESULT_SCHEMA_VERSION,
    runner_version: RUNNER_VERSION,
    source_state: { ...handoff.source_state },
    fixture: { ...handoff.fixture },
    environment: { ...handoff.environment },
    instance: {
      revision: handoff.revision,
      nonce: handoff.nonce,
      pid: handoff.pid,
      fixture_mode: handoff.fixture_mode,
      outbound_calls_before: before.outbound_calls,
      outbound_calls_after: after.outbound_calls,
      browser_outbound_attempts: after.browser_outbound_attempts,
    },
    ...measurement,
  });
  publishExclusiveJson(outputPath, artifact, outputParts);
  const exitCode = artifact.failed === 0 ? 0 : 2;
  stdout(`${JSON.stringify({ schema_version: 1, result_path: outputPath, exit_code: exitCode })}\n`);
  return { artifact, outputPath, exitCode };
}

function safeErrorMessage(error) {
  if (error instanceof PerfContractError || error instanceof PerfRequestError) return error.message;
  return 'unexpected performance harness failure';
}

if (require.main === module) {
  runCli()
    .then((execution) => {
      process.exitCode = execution.exitCode;
    })
    .catch((error) => {
      process.stderr.write(`dashboard-load failed: ${safeErrorMessage(error)}\n`);
      process.exitCode = 2;
    });
}

module.exports = {
  PerfRequestError,
  buildRequestOptions,
  normalizeLoopbackHostname,
  percentile,
  publishExclusiveJson,
  requestRaw,
  runCli,
  runLoad,
  safeErrorMessage,
  validateLoadOptions,
  validateResultArtifact,
};
