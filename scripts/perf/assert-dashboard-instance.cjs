'use strict';

const http = require('node:http');
const fs = require('node:fs');

const HANDOFF_SCHEMA_VERSION = 1;
const INSTANCE_SCHEMA_VERSION = 1;
const MAX_HANDOFF_LIFETIME_MS = 15 * 60 * 1000;
const MAX_INSTANCE_BODY_BYTES = 64 * 1024;
const HEX_40 = /^[0-9a-f]{40}$/;
const HEX_64 = /^[0-9a-f]{64}$/;
const SAFE_VERSION = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const FIXTURE_MODES = new Set(['Public', 'GuildDetail', 'ReadOnly']);
const LOOPBACK_HOSTS = new Set(['127.0.0.1', '::1']);

class PerfContractError extends Error {
  constructor(message) {
    super(message);
    this.name = 'PerfContractError';
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

function assertInteger(value, minimum, maximum, label) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    fail(`${label} must be an integer in the accepted range`);
  }
}

function validateFixture(fixture) {
  assertExactKeys(fixture, ['version', 'sha256'], 'fixture');
  if (typeof fixture.version !== 'string' || !SAFE_VERSION.test(fixture.version)) {
    fail('fixture version is invalid');
  }
  if (typeof fixture.sha256 !== 'string' || !HEX_64.test(fixture.sha256)) {
    fail('fixture hash is invalid');
  }
  return { version: fixture.version, sha256: fixture.sha256 };
}

function validateSourceState(sourceState) {
  assertExactKeys(sourceState, ['head', 'clean', 'diff_sha256'], 'source_state');
  if (typeof sourceState.head !== 'string' || !HEX_40.test(sourceState.head)) {
    fail('source_state head is invalid');
  }
  if (sourceState.clean !== true) fail('source_state must be clean');
  if (typeof sourceState.diff_sha256 !== 'string' || !HEX_64.test(sourceState.diff_sha256)) {
    fail('source_state diff hash is invalid');
  }
  return {
    head: sourceState.head,
    clean: true,
    diff_sha256: sourceState.diff_sha256,
  };
}

function validateEnvironment(environment) {
  const keys = [
    'fingerprint_sha256',
    'os_build',
    'arch',
    'cpu_model',
    'logical_cores',
    'power_profile',
    'node_version',
    'rustc_version',
    'cargo_profile',
  ];
  assertExactKeys(environment, keys, 'environment');
  if (typeof environment.fingerprint_sha256 !== 'string'
      || !HEX_64.test(environment.fingerprint_sha256)) {
    fail('environment fingerprint is invalid');
  }
  for (const key of ['os_build', 'arch', 'cpu_model', 'power_profile', 'node_version', 'rustc_version']) {
    if (typeof environment[key] !== 'string'
        || environment[key].length === 0
        || environment[key].length > 256) {
      fail(`environment ${key} is invalid`);
    }
  }
  assertInteger(environment.logical_cores, 1, 65_536, 'environment logical_cores');
  if (environment.cargo_profile !== 'debug' && environment.cargo_profile !== 'release') {
    fail('environment cargo_profile is invalid');
  }
  return { ...environment };
}

function validateHandoff(value, now = Date.now()) {
  const keys = [
    'schema_version',
    'issued_at_unix_ms',
    'expires_at_unix_ms',
    'revision',
    'nonce',
    'pid',
    'host',
    'port',
    'dynamic_port',
    'fixture_mode',
    'fixture',
    'source_state',
    'environment',
  ];
  assertExactKeys(value, keys, 'instance handoff');
  if (value.schema_version !== HANDOFF_SCHEMA_VERSION) fail('instance handoff schema is unsupported');
  assertInteger(value.issued_at_unix_ms, 0, Number.MAX_SAFE_INTEGER, 'handoff issued time');
  assertInteger(value.expires_at_unix_ms, 0, Number.MAX_SAFE_INTEGER, 'handoff expiry time');
  if (value.expires_at_unix_ms <= value.issued_at_unix_ms
      || value.expires_at_unix_ms - value.issued_at_unix_ms > MAX_HANDOFF_LIFETIME_MS) {
    fail('instance handoff lifetime is invalid');
  }
  if (!Number.isSafeInteger(now) || now < value.issued_at_unix_ms - 5_000) {
    fail('instance handoff is not yet valid');
  }
  if (now > value.expires_at_unix_ms) fail('instance handoff expired');
  if (typeof value.revision !== 'string' || !HEX_40.test(value.revision)) {
    fail('instance revision is invalid');
  }
  if (typeof value.nonce !== 'string' || !HEX_64.test(value.nonce)) {
    fail('instance nonce is invalid');
  }
  assertInteger(value.pid, 1, 4_294_967_295, 'instance pid');
  if (typeof value.host !== 'string' || !LOOPBACK_HOSTS.has(value.host)) {
    fail('instance host must be an exact loopback address');
  }
  assertInteger(value.port, 1, 65_535, 'instance port');
  if (value.dynamic_port !== true) fail('instance port must be runner-assigned');
  if (typeof value.fixture_mode !== 'string' || !FIXTURE_MODES.has(value.fixture_mode)) {
    fail('instance fixture mode is invalid');
  }
  const fixture = validateFixture(value.fixture);
  const sourceState = validateSourceState(value.source_state);
  if (sourceState.head !== value.revision) fail('handoff source and instance revisions differ');
  const environment = validateEnvironment(value.environment);

  return Object.freeze({
    schema_version: HANDOFF_SCHEMA_VERSION,
    issued_at_unix_ms: value.issued_at_unix_ms,
    expires_at_unix_ms: value.expires_at_unix_ms,
    revision: value.revision,
    nonce: value.nonce,
    pid: value.pid,
    host: value.host,
    port: value.port,
    dynamic_port: true,
    fixture_mode: value.fixture_mode,
    fixture: Object.freeze(fixture),
    source_state: Object.freeze(sourceState),
    environment: Object.freeze(environment),
  });
}

function loadAndValidateHandoff(filePath, now = Date.now()) {
  if (typeof filePath !== 'string' || filePath.length === 0 || filePath.length > 32_767) {
    fail('PERF_INSTANCE_HANDOFF is required');
  }
  if (!(/^[A-Za-z]:[\\/]/.test(filePath) || filePath.startsWith('\\\\') || filePath.startsWith('/'))) {
    fail('PERF_INSTANCE_HANDOFF must be an absolute path');
  }
  let metadata;
  let body;
  try {
    metadata = fs.lstatSync(filePath);
    if (!metadata.isFile() || metadata.isSymbolicLink()) fail('instance handoff must be a regular file');
    if (metadata.size <= 0 || metadata.size > MAX_INSTANCE_BODY_BYTES) {
      fail('instance handoff size is invalid');
    }
    body = fs.readFileSync(filePath, 'utf8');
  } catch (error) {
    if (error instanceof PerfContractError) throw error;
    fail('instance handoff could not be opened');
  }
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch (_error) {
    fail('instance handoff is not valid JSON');
  }
  return validateHandoff(parsed, now);
}

function validateInstanceSnapshot(value) {
  const keys = [
    'schema_version',
    'revision',
    'nonce',
    'pid',
    'fixture_mode',
    'fixture',
    'outbound_calls',
    'browser_outbound_attempts',
  ];
  assertExactKeys(value, keys, 'instance snapshot');
  if (value.schema_version !== INSTANCE_SCHEMA_VERSION) fail('instance snapshot schema is unsupported');
  if (typeof value.revision !== 'string' || !HEX_40.test(value.revision)) {
    fail('instance snapshot revision is invalid');
  }
  if (typeof value.nonce !== 'string' || !HEX_64.test(value.nonce)) {
    fail('instance snapshot nonce is invalid');
  }
  assertInteger(value.pid, 1, 4_294_967_295, 'instance snapshot pid');
  if (typeof value.fixture_mode !== 'string' || !FIXTURE_MODES.has(value.fixture_mode)) {
    fail('instance snapshot fixture mode is invalid');
  }
  const fixture = validateFixture(value.fixture);
  assertInteger(value.outbound_calls, 0, Number.MAX_SAFE_INTEGER, 'instance outbound count');
  assertInteger(
    value.browser_outbound_attempts,
    0,
    Number.MAX_SAFE_INTEGER,
    'browser outbound count',
  );
  return { ...value, fixture };
}

function assertInstanceSnapshot(handoff, value, phase) {
  const snapshot = validateInstanceSnapshot(value);
  const identityMatches = snapshot.revision === handoff.revision
    && snapshot.nonce === handoff.nonce
    && snapshot.pid === handoff.pid
    && snapshot.fixture_mode === handoff.fixture_mode
    && snapshot.fixture.version === handoff.fixture.version
    && snapshot.fixture.sha256 === handoff.fixture.sha256;
  if (!identityMatches) fail(`${phase} instance identity mismatch`);
  if (snapshot.outbound_calls !== 0) fail(`${phase} server outbound activity detected`);
  if (snapshot.browser_outbound_attempts !== 0) fail(`${phase} browser outbound activity detected`);
  return Object.freeze(snapshot);
}

function loopbackHostForUrl(host) {
  return host === '::1' ? '[::1]' : host;
}

function fetchInstanceSnapshot(handoff, options = {}) {
  const timeoutMs = options.timeout_ms ?? 3_000;
  assertInteger(timeoutMs, 1, 30_000, 'instance request timeout');
  const requestOptions = {
    protocol: 'http:',
    hostname: handoff.host,
    port: handoff.port,
    path: '/__perf/instance',
    method: 'GET',
    headers: {
      accept: 'application/json',
      host: `${loopbackHostForUrl(handoff.host)}:${handoff.port}`,
    },
    agent: false,
  };

  return new Promise((resolve, reject) => {
    let settled = false;
    let deadlineTimer;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      if (deadlineTimer) clearTimeout(deadlineTimer);
      if (error) reject(error);
      else resolve(value);
    };
    const request = http.request(requestOptions, (response) => {
      const chunks = [];
      let bytes = 0;
      response.on('data', (chunk) => {
        bytes += chunk.length;
        if (bytes > MAX_INSTANCE_BODY_BYTES) {
          request.destroy();
          finish(new PerfContractError('instance response exceeded the size limit'));
          return;
        }
        chunks.push(chunk);
      });
      response.on('end', () => {
        if (response.statusCode !== 200) {
          finish(new PerfContractError('instance endpoint returned a non-success status'));
          return;
        }
        const contentType = String(response.headers['content-type'] || '').split(';', 1)[0].trim();
        if (contentType !== 'application/json') {
          finish(new PerfContractError('instance endpoint returned an invalid content type'));
          return;
        }
        let parsed;
        try {
          parsed = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        } catch (_error) {
          finish(new PerfContractError('instance endpoint returned invalid JSON'));
          return;
        }
        finish(null, parsed);
      });
      response.on('error', () => {
        finish(new PerfContractError('instance response failed'));
      });
    });
    request.setTimeout(timeoutMs, () => {
      request.destroy();
      finish(new PerfContractError('instance request timed out'));
    });
    deadlineTimer = setTimeout(() => {
      request.destroy();
      finish(new PerfContractError('instance request timed out'));
    }, timeoutMs);
    request.on('error', () => {
      finish(new PerfContractError('instance request failed'));
    });
    request.end();
  });
}

async function fetchAndAssertInstance(handoff, phase, options = {}) {
  const snapshot = await fetchInstanceSnapshot(handoff, options);
  return assertInstanceSnapshot(handoff, snapshot, phase);
}

module.exports = {
  PerfContractError,
  assertInstanceSnapshot,
  fetchAndAssertInstance,
  fetchInstanceSnapshot,
  loadAndValidateHandoff,
  validateEnvironment,
  validateFixture,
  validateHandoff,
  validateSourceState,
};
