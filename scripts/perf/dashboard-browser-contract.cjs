'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');

const {
  PerfContractError,
  fetchAndAssertInstance,
  loadAndValidateHandoff,
} = require('./assert-dashboard-instance.cjs');
const { publishExclusiveJson } = require('./dashboard-load.cjs');

const RUNNER_VERSION = 'dashboard-browser-v1';
const PLAYWRIGHT_VERSION = '1.58.2';
const CHROMIUM_REVISION = '1208';
const CHROMIUM_VERSION = '145.0.7632.6';
const MAX_JSON_BYTES = 8 * 1024 * 1024;
const MAX_CONTROL_BODY_BYTES = 64 * 1024;
const MAX_PROOF_LIFETIME_MS = 15 * 60 * 1000;
const HEX_64 = /^[0-9a-f]{64}$/;
const SAFE_COOKIE_NAME = /^[A-Za-z0-9_.-]{1,128}$/;
const SAFE_GUILD_ID = /^[1-9][0-9]{0,19}$/;
const TEST_ID = /^[a-z][a-z0-9-]{0,63}$/;
const PERF_CONTROL_HEADER = 'x-dynamo-perf-control';
const UI_PROOF_ATTACHMENT = 'dynamo-ui-proof-v1.json';
const UI_PROOF_BUDGETS = Object.freeze({
  first_text_visible_ms: 1_000,
  font_ready_ms: 2_000,
  cls: 0.02,
  horizontal_overflow_px: 0,
});

const REQUIRED_ENV_KEYS = Object.freeze([
  'PERF_INSTANCE_HANDOFF',
  'PLAYWRIGHT_PERF_STORAGE_STATE',
  'PLAYWRIGHT_PERF_SELECTION',
  'PLAYWRIGHT_PERF_LAUNCH_PROOF',
  'PLAYWRIGHT_PERF_REPORT',
  'PLAYWRIGHT_PERF_RESULT',
  'PLAYWRIGHT_PERF_OUTPUT_DIR',
]);

const FORBIDDEN_ENV_KEYS = Object.freeze([
  'PLAYWRIGHT_BASE_URL',
  'PLAYWRIGHT_STORAGE_STATE',
  'PLAYWRIGHT_GUILD_ID',
  'PLAYWRIGHT_JSON_OUTPUT_DIR',
  'PLAYWRIGHT_JSON_OUTPUT_FILE',
  'PLAYWRIGHT_JSON_OUTPUT_NAME',
  'PERF_BASE_URL',
  'PERF_COOKIE',
  'PERF_HEADERS',
  'NODE_OPTIONS',
  'NODE_PATH',
  'PWDEBUG',
  'DEBUG',
  'HTTP_PROXY',
  'HTTPS_PROXY',
  'ALL_PROXY',
  'NO_PROXY',
]);

const LOCKED_FONT_ROUTES = Object.freeze([
  Object.freeze({
    file: 'FiraSans-Light.woff2',
    path: '/assets/fonts/fira-sans-light-2315d21be4c62def3fa08de87f29c327187affcd7759a46596d5bacfeb2ff221.woff2',
    sha256: '2315d21be4c62def3fa08de87f29c327187affcd7759a46596d5bacfeb2ff221',
  }),
  Object.freeze({
    file: 'FiraSans-Regular.woff2',
    path: '/assets/fonts/fira-sans-regular-51000d3cc8a601427bdb88625275e0eefc00570f4f2ab7a926fa336abee7098f.woff2',
    sha256: '51000d3cc8a601427bdb88625275e0eefc00570f4f2ab7a926fa336abee7098f',
  }),
  Object.freeze({
    file: 'FiraSans-Medium.woff2',
    path: '/assets/fonts/fira-sans-medium-14d421d4fb35d56b40e958cc9b64eb1528d1822bb75251623955c873a9a175d3.woff2',
    sha256: '14d421d4fb35d56b40e958cc9b64eb1528d1822bb75251623955c873a9a175d3',
  }),
  Object.freeze({
    file: 'FiraSans-SemiBold.woff2',
    path: '/assets/fonts/fira-sans-semibold-01ca0c4a1f02dd4324721cf8766bcbb2edca4dee0fb8884a66e58e231ed62d55.woff2',
    sha256: '01ca0c4a1f02dd4324721cf8766bcbb2edca4dee0fb8884a66e58e231ed62d55',
  }),
  Object.freeze({
    file: 'FiraSans-Bold.woff2',
    path: '/assets/fonts/fira-sans-bold-7dff4e2351ca54fae96180e79ab1d1fdeb74ebb6e6cba53a7d9d7b4941600ada.woff2',
    sha256: '7dff4e2351ca54fae96180e79ab1d1fdeb74ebb6e6cba53a7d9d7b4941600ada',
  }),
  Object.freeze({
    file: 'FiraCode-Variable.woff2',
    path: '/assets/fonts/fira-code-408e876a202f15ea6ee307a70a65cf40ceb222c589a0b17e0a3a371db96dd49f.woff2',
    sha256: '408e876a202f15ea6ee307a70a65cf40ceb222c589a0b17e0a3a371db96dd49f',
  }),
]);

// Keep this exact schema isolated so the Rust harness and launcher can be
// advanced together without weakening the rest of the browser contract.
const COUNTER_KEYS = Object.freeze([
  'schema_version',
  'denied_requests',
  'server_write_attempts',
  'repository_reads',
  'repository_mutations',
  'outbound_calls',
  'browser_outbound_attempts',
  'provider_guild_lookups',
]);

const EXPECTED_SELECTION = Object.freeze({
  schema_version: 1,
  suite_id: 'dashboard-readonly-v1',
  specs: Object.freeze(['tests/playwright/dashboard-isolated-readonly.spec.cjs']),
  projects: Object.freeze([
    Object.freeze({ name: 'desktop', viewport: Object.freeze({ width: 1440, height: 1100 }) }),
    Object.freeze({ name: 'tablet', viewport: Object.freeze({ width: 1024, height: 900 }) }),
    Object.freeze({ name: 'small-tablet', viewport: Object.freeze({ width: 768, height: 1024 }) }),
    Object.freeze({ name: 'mobile-sanity', viewport: Object.freeze({ width: 375, height: 812 }) }),
  ]),
  test_ids: Object.freeze([
    'public-responsive-reduced-motion',
    'guild-readonly-dialog',
    'same-origin-font-proof',
  ]),
  expected_count: 12,
});

class BrowserContractError extends Error {
  constructor(message) {
    super(message);
    this.name = 'BrowserContractError';
  }
}

function fail(message) {
  throw new BrowserContractError(message);
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

function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function isAbsolutePath(value) {
  return typeof value === 'string'
    && value.length > 0
    && value.length <= 32_767
    && path.isAbsolute(value);
}

function readRegularFile(filePath, label, maximumBytes = MAX_JSON_BYTES) {
  if (!isAbsolutePath(filePath)) fail(`${label} path must be absolute`);
  let metadata;
  let body;
  try {
    metadata = fs.lstatSync(filePath);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      fail(`${label} must be a regular file`);
    }
    if (metadata.size <= 0 || metadata.size > maximumBytes) {
      fail(`${label} size is invalid`);
    }
    body = fs.readFileSync(filePath, 'utf8');
    const after = fs.lstatSync(filePath);
    if (!after.isFile()
        || after.isSymbolicLink()
        || after.size !== metadata.size
        || after.mtimeMs !== metadata.mtimeMs) {
      fail(`${label} changed while it was read`);
    }
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    fail(`${label} could not be opened`);
  }
  return body;
}

function parseJson(body, label) {
  try {
    return JSON.parse(body);
  } catch (_error) {
    fail(`${label} is not valid JSON`);
  }
}

function validateStorageState(value, handoff, now = Date.now()) {
  assertExactKeys(value, ['cookies', 'origins'], 'storage state');
  if (!Array.isArray(value.cookies) || value.cookies.length !== 1) {
    fail('storage state must contain exactly one cookie');
  }
  if (!Array.isArray(value.origins) || value.origins.length !== 0) {
    fail('storage state origins must be empty');
  }
  const cookie = value.cookies[0];
  assertExactKeys(
    cookie,
    ['name', 'value', 'domain', 'path', 'expires', 'httpOnly', 'secure', 'sameSite'],
    'storage cookie',
  );
  if (typeof cookie.name !== 'string' || !SAFE_COOKIE_NAME.test(cookie.name)) {
    fail('storage cookie name is invalid');
  }
  if (typeof cookie.value !== 'string'
      || cookie.value.length === 0
      || cookie.value.length > 4_096
      || /[\u0000-\u001f\u007f]/.test(cookie.value)) {
    fail('storage cookie value is invalid');
  }
  if (cookie.domain !== handoff.host) fail('storage cookie domain does not match the instance');
  if (cookie.path !== '/') fail('storage cookie path must be root scoped');
  assertInteger(cookie.expires, 1, Number.MAX_SAFE_INTEGER, 'storage cookie expiry');
  if (cookie.expires <= Math.floor(now / 1_000)) fail('storage cookie expired');
  if (cookie.expires > Math.floor(handoff.expires_at_unix_ms / 1_000)) {
    fail('storage cookie outlives the instance handoff');
  }
  if (cookie.httpOnly !== true || cookie.secure !== false || cookie.sameSite !== 'Strict') {
    fail('storage cookie flags are invalid');
  }
  return Object.freeze({
    cookies: Object.freeze([Object.freeze({ ...cookie })]),
    origins: Object.freeze([]),
  });
}

function validateSelection(value) {
  assertExactKeys(
    value,
    ['schema_version', 'suite_id', 'specs', 'projects', 'test_ids', 'expected_count'],
    'browser selection',
  );
  if (value.schema_version !== EXPECTED_SELECTION.schema_version
      || value.suite_id !== EXPECTED_SELECTION.suite_id) {
    fail('browser selection identity is invalid');
  }
  if (!Array.isArray(value.specs)
      || value.specs.length !== EXPECTED_SELECTION.specs.length
      || value.specs.some((entry, index) => entry !== EXPECTED_SELECTION.specs[index])) {
    fail('browser selection specs are invalid');
  }
  if (!Array.isArray(value.test_ids)
      || value.test_ids.length !== EXPECTED_SELECTION.test_ids.length
      || value.test_ids.some((entry, index) => {
        return typeof entry !== 'string'
          || !TEST_ID.test(entry)
          || entry !== EXPECTED_SELECTION.test_ids[index];
      })) {
    fail('browser selection test ids are invalid');
  }
  if (!Array.isArray(value.projects)
      || value.projects.length !== EXPECTED_SELECTION.projects.length) {
    fail('browser selection projects are invalid');
  }
  const projects = value.projects.map((project, index) => {
    assertExactKeys(project, ['name', 'viewport'], 'browser project');
    assertExactKeys(project.viewport, ['width', 'height'], 'browser project viewport');
    const expected = EXPECTED_SELECTION.projects[index];
    if (project.name !== expected.name
        || project.viewport.width !== expected.viewport.width
        || project.viewport.height !== expected.viewport.height) {
      fail('browser selection projects are invalid');
    }
    return Object.freeze({
      name: project.name,
      viewport: Object.freeze({ ...project.viewport }),
    });
  });
  const expectedCount = value.projects.length * value.test_ids.length;
  if (value.expected_count !== EXPECTED_SELECTION.expected_count
      || value.expected_count !== expectedCount) {
    fail('browser selection expected count is invalid');
  }
  return Object.freeze({
    schema_version: 1,
    suite_id: value.suite_id,
    specs: Object.freeze([...value.specs]),
    projects: Object.freeze(projects),
    test_ids: Object.freeze([...value.test_ids]),
    expected_count: value.expected_count,
  });
}

function isCanonicalGuildQuery(search) {
  if (search === '') return true;
  if (!search.startsWith('?') || search.length === 1) return false;
  const fields = search.slice(1).split('&');
  const tab = fields.shift();
  if (tab === 'tab=overview' || tab === 'tab=modules' || tab === 'tab=commands') {
    return fields.length === 0;
  }
  if (tab !== 'tab=logs') return false;
  let lastRank = 0;
  for (const field of fields) {
    const [key, fieldValue, extra] = field.split('=');
    if (extra !== undefined) return false;
    let rank = 0;
    if (key === 'log_entity' && (fieldValue === 'module' || fieldValue === 'command')) rank = 1;
    else if (key === 'log_action'
        && (fieldValue === 'toggle' || fieldValue === 'save_settings')) rank = 2;
    else if (key === 'log_page'
        && /^(?:[1-9][0-9]{0,3}|10000)$/.test(fieldValue)
        && Number(fieldValue) <= 10_000) rank = 3;
    else return false;
    if (rank <= lastRank) return false;
    lastRank = rank;
  }
  return true;
}

function isAllowedBrowserReadUrl(rawUrl, baseUrl, guildId) {
  let target;
  try {
    target = new URL(rawUrl);
  } catch (_error) {
    return false;
  }
  if (target.origin !== baseUrl
      || target.username !== ''
      || target.password !== ''
      || target.hash !== '') return false;
  if (target.pathname === '/') return target.search === '';
  if (target.pathname === `/guild/${guildId}`) return isCanonicalGuildQuery(target.search);
  return target.search === '' && LOCKED_FONT_ROUTES.some((font) => font.path === target.pathname);
}

function isSameHarnessWebSocketOrigin(rawUrl, baseUrl) {
  let target;
  let base;
  try {
    target = new URL(rawUrl);
    base = new URL(baseUrl);
  } catch (_error) {
    return false;
  }
  const matchingProtocol = (base.protocol === 'http:' && target.protocol === 'ws:')
    || (base.protocol === 'https:' && target.protocol === 'wss:');
  return matchingProtocol
    && target.hostname === base.hostname
    && target.port === base.port
    && target.username === ''
    && target.password === '';
}

function validateLaunchProof(value, handoff, fileHashes, now = Date.now()) {
  assertExactKeys(
    value,
    [
      'schema_version',
      'runner_version',
      'playwright_version',
      'expected_outcome',
      'expected_failure_id',
      'issued_at_unix_ms',
      'expires_at_unix_ms',
      'nonce',
      'guild_id',
      'handoff_sha256',
      'storage_state_sha256',
      'selection_sha256',
    ],
    'browser launch proof',
  );
  if (value.schema_version !== 1 || value.runner_version !== RUNNER_VERSION) {
    fail('browser launch proof identity is invalid');
  }
  if (value.playwright_version !== PLAYWRIGHT_VERSION) {
    fail('browser launch proof Playwright version mismatch');
  }
  if (value.expected_outcome !== 'Pass' && value.expected_outcome !== 'Red') {
    fail('browser launch proof expected outcome is invalid');
  }
  if (value.expected_outcome === 'Pass' && value.expected_failure_id !== null) {
    fail('Pass launch proof must not declare an expected failure id');
  }
  if (value.expected_outcome === 'Red'
      && (typeof value.expected_failure_id !== 'string'
        || !EXPECTED_SELECTION.test_ids.includes(value.expected_failure_id))) {
    fail('Red launch proof expected failure id is invalid');
  }
  assertInteger(value.issued_at_unix_ms, 0, Number.MAX_SAFE_INTEGER, 'launch proof issued time');
  assertInteger(value.expires_at_unix_ms, 0, Number.MAX_SAFE_INTEGER, 'launch proof expiry time');
  if (value.expires_at_unix_ms <= value.issued_at_unix_ms
      || value.expires_at_unix_ms - value.issued_at_unix_ms > MAX_PROOF_LIFETIME_MS
      || value.issued_at_unix_ms < handoff.issued_at_unix_ms - 5_000
      || value.expires_at_unix_ms > handoff.expires_at_unix_ms) {
    fail('browser launch proof lifetime is invalid');
  }
  if (now < value.issued_at_unix_ms - 5_000) fail('browser launch proof is not yet valid');
  if (now > value.expires_at_unix_ms) fail('browser launch proof expired');
  if (value.nonce !== handoff.nonce || !HEX_64.test(value.nonce)) {
    fail('browser launch proof nonce mismatch');
  }
  if (typeof value.guild_id !== 'string' || !SAFE_GUILD_ID.test(value.guild_id)) {
    fail('browser launch proof guild id is invalid');
  }
  for (const [key, actual] of [
    ['handoff_sha256', fileHashes.handoff],
    ['storage_state_sha256', fileHashes.storage],
    ['selection_sha256', fileHashes.selection],
  ]) {
    if (typeof value[key] !== 'string' || !HEX_64.test(value[key]) || value[key] !== actual) {
      const label = key === 'storage_state_sha256'
        ? 'storage state'
        : key.replace('_sha256', '').replace('_', ' ');
      fail(`${label} hash mismatch`);
    }
  }
  return Object.freeze({ ...value });
}

function playwrightMaxFailures(expectedOutcome) {
  if (expectedOutcome === 'Pass') return 1;
  if (expectedOutcome === 'Red') return 0;
  fail('Playwright max-failures outcome is invalid');
}

function assertAbsentLeaf(filePath, expectedName, label, outputDirectory) {
  if (!isAbsolutePath(filePath)) fail(`${label} path must be absolute`);
  if (path.basename(filePath) !== expectedName) fail(`${label} leaf name is invalid`);
  let parent;
  try {
    parent = fs.realpathSync.native(path.dirname(filePath));
  } catch (_error) {
    fail(`${label} parent could not be resolved`);
  }
  if (parent !== outputDirectory) fail(`${label} must be a direct child of the browser output directory`);
  try {
    fs.lstatSync(filePath);
    fail(`${label} already exists`);
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    if (!error || error.code !== 'ENOENT') fail(`${label} availability could not be verified`);
  }
}

function assertAbsentUncreatedLeaf(filePath, expectedName, label, outputDirectory) {
  if (!isAbsolutePath(filePath)) fail(`${label} path must be absolute`);
  if (path.basename(filePath) !== expectedName) fail(`${label} leaf name is invalid`);
  if (path.dirname(filePath) !== outputDirectory) {
    fail(`${label} must be a direct child of the browser output directory`);
  }
  try {
    fs.lstatSync(filePath);
    fail(`${label} already exists`);
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    if (!error || error.code !== 'ENOENT') fail(`${label} availability could not be verified`);
  }
}

function assertAbsentPath(filePath, label, presentMessage = `${label} must be absent`) {
  try {
    fs.lstatSync(filePath);
    fail(presentMessage);
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    if (!error || error.code !== 'ENOENT') fail(`${label} absence could not be verified`);
  }
}

function assertPlaywrightWorkerOutput(outputDirectory) {
  let entries;
  try {
    entries = fs.readdirSync(outputDirectory, { withFileTypes: true });
  } catch (_error) {
    fail('browser output directory could not be enumerated in the worker');
  }
  for (const entry of entries) {
    if (!/^\.playwright-artifacts-[0-9]+$/.test(entry.name)) {
      fail('browser worker output contains a non-Playwright artifact');
    }
    const entryPath = path.join(outputDirectory, entry.name);
    let metadata;
    let nativePath;
    try {
      metadata = fs.lstatSync(entryPath);
      nativePath = fs.realpathSync.native(entryPath);
    } catch (_error) {
      fail('browser worker artifact could not be resolved');
    }
    if (!entry.isDirectory()
        || !metadata.isDirectory()
        || metadata.isSymbolicLink()
        || nativePath !== entryPath) {
      fail('browser worker artifacts must be native regular directories');
    }
  }
}

function pathIsWithin(parent, candidate) {
  const relative = path.relative(parent, candidate);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

function loadBrowserRunContext(env = process.env, now = Date.now(), options = {}) {
  const presentKeys = new Set(Object.keys(env).map((key) => key.toUpperCase()));
  for (const key of FORBIDDEN_ENV_KEYS) {
    if (presentKeys.has(key)) fail(`${key.includes('PROXY') ? 'proxy environment' : key} is forbidden`);
  }
  if ([...presentKeys].some((key) => key.includes('PROXY'))) fail('proxy environment is forbidden');
  for (const key of REQUIRED_ENV_KEYS) {
    if (!Object.hasOwn(env, key) || !isAbsolutePath(env[key])) {
      fail(`${key} is required as an absolute launcher-owned path`);
    }
  }

  const handoffRaw = readRegularFile(env.PERF_INSTANCE_HANDOFF, 'instance handoff', MAX_CONTROL_BODY_BYTES);
  let handoff;
  try {
    handoff = loadAndValidateHandoff(env.PERF_INSTANCE_HANDOFF, now);
  } catch (error) {
    if (error instanceof PerfContractError) fail(error.message);
    throw error;
  }
  const handoffReadback = readRegularFile(
    env.PERF_INSTANCE_HANDOFF,
    'instance handoff',
    MAX_CONTROL_BODY_BYTES,
  );
  if (handoffReadback !== handoffRaw) fail('instance handoff changed during validation');
  if (handoff.host !== '127.0.0.1' || handoff.fixture_mode !== 'ReadOnly') {
    fail('browser runner requires the IPv4 ReadOnly harness');
  }

  const storageRaw = readRegularFile(
    env.PLAYWRIGHT_PERF_STORAGE_STATE,
    'storage state',
    MAX_CONTROL_BODY_BYTES,
  );
  const selectionRaw = readRegularFile(
    env.PLAYWRIGHT_PERF_SELECTION,
    'browser selection',
    MAX_CONTROL_BODY_BYTES,
  );
  const proofRaw = readRegularFile(
    env.PLAYWRIGHT_PERF_LAUNCH_PROOF,
    'browser launch proof',
    MAX_CONTROL_BODY_BYTES,
  );
  const storageState = validateStorageState(parseJson(storageRaw, 'storage state'), handoff, now);
  const selection = validateSelection(parseJson(selectionRaw, 'browser selection'));
  const selectionSha256 = sha256(selectionRaw);
  const proof = validateLaunchProof(
    parseJson(proofRaw, 'browser launch proof'),
    handoff,
    {
      handoff: sha256(handoffRaw),
      storage: sha256(storageRaw),
      selection: selectionSha256,
    },
    now,
  );

  const workerPhase = options.workerPhase === true;
  let nativeParent;
  try {
    nativeParent = fs.realpathSync.native(path.dirname(env.PLAYWRIGHT_PERF_OUTPUT_DIR));
  } catch (_error) {
    fail('browser output parent could not be resolved');
  }
  const outputDirectory = path.join(nativeParent, path.basename(env.PLAYWRIGHT_PERF_OUTPUT_DIR));
  if (outputDirectory !== env.PLAYWRIGHT_PERF_OUTPUT_DIR) {
    fail('browser output directory must use its native final path');
  }
  if (path.basename(outputDirectory) !== 'browser-artifacts') {
    fail('browser output directory must be the launcher-owned browser-artifacts leaf');
  }
  for (const [label, filePath] of [
    ['instance handoff', env.PERF_INSTANCE_HANDOFF],
    ['storage state', env.PLAYWRIGHT_PERF_STORAGE_STATE],
    ['browser selection', env.PLAYWRIGHT_PERF_SELECTION],
    ['browser launch proof', env.PLAYWRIGHT_PERF_LAUNCH_PROOF],
  ]) {
    let nativePath;
    try {
      nativePath = fs.realpathSync.native(filePath);
    } catch (_error) {
      fail(`${label} native path could not be resolved`);
    }
    if (pathIsWithin(outputDirectory, nativePath)) {
      fail(`${label} must remain outside the browser output directory`);
    }
  }

  if (workerPhase) {
    let outputMetadata;
    let nativeOutput;
    try {
      outputMetadata = fs.lstatSync(outputDirectory);
      nativeOutput = fs.realpathSync.native(outputDirectory);
    } catch (_error) {
      fail('browser worker output directory could not be opened');
    }
    if (!outputMetadata.isDirectory()
        || outputMetadata.isSymbolicLink()
        || nativeOutput !== outputDirectory) {
      fail('browser worker output must be a native regular directory');
    }
    assertAbsentLeaf(
      env.PLAYWRIGHT_PERF_REPORT,
      'playwright-report.json',
      'Playwright report',
      outputDirectory,
    );
    assertAbsentLeaf(
      env.PLAYWRIGHT_PERF_RESULT,
      'browser-result.json',
      'browser result',
      outputDirectory,
    );
    assertPlaywrightWorkerOutput(outputDirectory);
  } else {
    assertAbsentUncreatedLeaf(
      env.PLAYWRIGHT_PERF_REPORT,
      'playwright-report.json',
      'Playwright report',
      outputDirectory,
    );
    assertAbsentUncreatedLeaf(
      env.PLAYWRIGHT_PERF_RESULT,
      'browser-result.json',
      'browser result',
      outputDirectory,
    );
    assertAbsentPath(
      outputDirectory,
      'browser output directory',
      'browser output directory must be absent before Playwright starts',
    );
  }

  return Object.freeze({
    runnerVersion: RUNNER_VERSION,
    playwrightVersion: PLAYWRIGHT_VERSION,
    handoff,
    storageState,
    selection,
    selectionSha256,
    proof,
    guildId: proof.guild_id,
    baseUrl: `http://${handoff.host}:${handoff.port}`,
    handoffPath: env.PERF_INSTANCE_HANDOFF,
    storageStatePath: env.PLAYWRIGHT_PERF_STORAGE_STATE,
    selectionPath: env.PLAYWRIGHT_PERF_SELECTION,
    proofPath: env.PLAYWRIGHT_PERF_LAUNCH_PROOF,
    reportPath: env.PLAYWRIGHT_PERF_REPORT,
    resultPath: env.PLAYWRIGHT_PERF_RESULT,
    outputDir: outputDirectory,
  });
}

function loadBrowserWorkerContext(env = process.env, now = Date.now()) {
  return loadBrowserRunContext(env, now, { workerPhase: true });
}

function normalizeReportFile(value) {
  if (typeof value !== 'string') fail('report spec file is invalid');
  return value.replaceAll('\\', '/');
}

function collectReportSpecs(suites, output) {
  if (!Array.isArray(suites)) fail('Playwright report suites are invalid');
  for (const suite of suites) {
    if (!isPlainObject(suite)) fail('Playwright report suite is invalid');
    if (suite.specs !== undefined) {
      if (!Array.isArray(suite.specs)) fail('Playwright report specs are invalid');
      output.push(...suite.specs);
    }
    if (suite.suites !== undefined) collectReportSpecs(suite.suites, output);
  }
}

function assertFiniteNumber(value, minimum, maximum, label) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < minimum || value > maximum) {
    fail(`${label} is outside the accepted range`);
  }
}

function validateUiProof(value, testId, projectName, options = {}) {
  const allowExpectedFailure = options.allowExpectedFailure === true;
  assertExactKeys(
    value,
    [
      'schema_version',
      'test_id',
      'project',
      'first_text_visible_ms',
      'font_ready_ms',
      'cls',
      'horizontal_overflow_px',
      'dom_nodes',
      'computed_fonts',
      'locked_font_hashes',
      'dialog_focus',
      'browser_identity',
    ],
    'UI proof',
  );
  if (value.schema_version !== 1 || value.test_id !== testId || value.project !== projectName) {
    fail('UI proof identity mismatch');
  }
  assertExactKeys(
    value.browser_identity,
    ['playwright_version', 'browser_name', 'browser_revision', 'browser_version'],
    'UI proof browser identity',
  );
  if (value.browser_identity.playwright_version !== PLAYWRIGHT_VERSION
      || value.browser_identity.browser_name !== 'chromium'
      || value.browser_identity.browser_revision !== CHROMIUM_REVISION
      || value.browser_identity.browser_version !== CHROMIUM_VERSION) {
    fail('UI proof browser identity is invalid');
  }
  assertFiniteNumber(value.first_text_visible_ms, 0, 60_000, 'UI proof first text value');
  assertFiniteNumber(value.font_ready_ms, 0, 60_000, 'UI proof font ready value');
  assertFiniteNumber(value.cls, 0, 100, 'UI proof CLS value');
  assertInteger(value.horizontal_overflow_px, 0, 1_000_000, 'UI proof horizontal overflow');
  if (!allowExpectedFailure
      && value.first_text_visible_ms > UI_PROOF_BUDGETS.first_text_visible_ms) {
    fail('UI proof first text budget failed');
  }
  if (!allowExpectedFailure && value.font_ready_ms > UI_PROOF_BUDGETS.font_ready_ms) {
    fail('UI proof font ready budget failed');
  }
  if (!allowExpectedFailure && value.cls > UI_PROOF_BUDGETS.cls) {
    fail('UI proof CLS budget failed');
  }
  if (!allowExpectedFailure
      && value.horizontal_overflow_px !== UI_PROOF_BUDGETS.horizontal_overflow_px) {
    fail('UI proof horizontal overflow budget failed');
  }
  assertInteger(value.dom_nodes, 1, 100_000, 'UI proof DOM nodes');

  assertExactKeys(value.computed_fonts, ['body', 'heading'], 'UI proof computed fonts');
  if (typeof value.computed_fonts.body !== 'string'
      || value.computed_fonts.body.length === 0
      || value.computed_fonts.body.length > 512
      || /[\u0000-\u001f\u007f]/.test(value.computed_fonts.body)
      || typeof value.computed_fonts.heading !== 'string'
      || value.computed_fonts.heading.length === 0
      || value.computed_fonts.heading.length > 512
      || /[\u0000-\u001f\u007f]/.test(value.computed_fonts.heading)) {
    fail('UI proof computed fonts are invalid');
  }
  if (!allowExpectedFailure
      && (!value.computed_fonts.body.includes('Fira Sans')
        || !value.computed_fonts.heading.includes('Fira Code'))) {
    fail('UI proof computed fonts are invalid');
  }

  if (!Array.isArray(value.locked_font_hashes)) fail('UI proof locked font hashes are invalid');
  const allowedHashes = new Set(LOCKED_FONT_ROUTES.map((font) => font.sha256));
  const hashes = [...value.locked_font_hashes];
  if (hashes.some((hash) => typeof hash !== 'string' || !allowedHashes.has(hash))
      || new Set(hashes).size !== hashes.length
      || hashes.some((hash, index) => index > 0 && hashes[index - 1] >= hash)) {
    fail('UI proof locked font hashes are invalid');
  }
  const fontProof = testId === 'same-origin-font-proof';
  const requiredFontHashes = ['FiraSans-Regular.woff2', 'FiraSans-Bold.woff2', 'FiraCode-Variable.woff2']
    .map((file) => LOCKED_FONT_ROUTES.find((font) => font.file === file).sha256);
  if (fontProof) {
    if (!allowExpectedFailure && requiredFontHashes.some((hash) => !hashes.includes(hash))) {
      fail('UI proof is missing a required locked font hash');
    }
  } else if (hashes.length !== 0) {
    fail('UI proof contains font hashes for a non-font test');
  }

  assertExactKeys(
    value.dialog_focus,
    ['applicable', 'initial_focus', 'tab_wrap', 'escape_closes', 'return_focus'],
    'UI proof dialog focus',
  );
  const dialogProof = testId === 'guild-readonly-dialog';
  if (value.dialog_focus.applicable !== dialogProof) fail('UI proof dialog applicability mismatch');
  for (const key of ['initial_focus', 'tab_wrap', 'escape_closes', 'return_focus']) {
    if (dialogProof && typeof value.dialog_focus[key] !== 'boolean') {
      fail('UI proof dialog focus value is invalid');
    }
    if (dialogProof && !allowExpectedFailure && value.dialog_focus[key] !== true) {
      fail('UI proof dialog focus proof failed');
    }
    if (!dialogProof && value.dialog_focus[key] !== null) fail('UI proof dialog focus must be null');
  }

  return Object.freeze({
    ...value,
    computed_fonts: Object.freeze({ ...value.computed_fonts }),
    locked_font_hashes: Object.freeze(hashes),
    dialog_focus: Object.freeze({ ...value.dialog_focus }),
    browser_identity: Object.freeze({ ...value.browser_identity }),
  });
}

function parseUiProofAttachment(result, testId, projectName, options = {}) {
  if (!Array.isArray(result.attachments)) {
    fail('Playwright result UI proof attachments are invalid');
  }
  if (options.allowMissing === true && result.attachments.length === 0) return null;
  if (result.attachments.length !== 1) {
    fail('Playwright result must contain exactly one UI proof attachment');
  }
  const attachment = result.attachments[0];
  assertExactKeys(attachment, ['name', 'contentType', 'body'], 'UI proof attachment');
  if (attachment.name !== UI_PROOF_ATTACHMENT
      || attachment.contentType !== 'application/json'
      || typeof attachment.body !== 'string'
      || attachment.body.length === 0
      || attachment.body.length > 64 * 1024
      || attachment.body.length % 4 !== 0
      || !/^[A-Za-z0-9+/]*={0,2}$/.test(attachment.body)) {
    fail('UI proof attachment is invalid');
  }
  const decoded = Buffer.from(attachment.body, 'base64');
  if (decoded.toString('base64') !== attachment.body || decoded.length > 32 * 1024) {
    fail('UI proof attachment encoding is invalid');
  }
  return validateUiProof(
    parseJson(decoded.toString('utf8'), 'UI proof attachment'),
    testId,
    projectName,
    options,
  );
}

function reportPathMatches(value, expected) {
  const normalized = normalizeReportFile(value);
  return normalized === expected || normalized.endsWith(`/${expected}`);
}

function validateAssertionLocation(value, selection) {
  if (!isPlainObject(value)
      || !reportPathMatches(value.file, selection.specs[0])
        && !reportPathMatches(value.file, 'tests/playwright/helpers/isolated-dashboard.cjs')) {
    fail('expected Red failure location is outside the isolated browser suite');
  }
  assertInteger(value.line, 1, 1_000_000, 'expected Red failure line');
  assertInteger(value.column, 1, 1_000_000, 'expected Red failure column');
}

function stripAnsi(value) {
  return value.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '');
}

function sanitizeAssertionFailure(result, selection) {
  if (!isPlainObject(result.error)
      || !Array.isArray(result.errors)
      || result.errors.length !== 1
      || !isPlainObject(result.errors[0])) {
    fail('expected Red row must contain one assertion error');
  }
  validateAssertionLocation(result.error.location, selection);
  validateAssertionLocation(result.errors[0].location, selection);
  const messages = [result.error.message, result.errors[0].message].map((message) => {
    if (typeof message !== 'string' || message.length === 0 || message.length > 64 * 1024) {
      fail('expected Red assertion message is invalid');
    }
    const stripped = stripAnsi(message).replaceAll('\r\n', '\n');
    if (stripped.length === 0 || /[\u0000\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(stripped)) {
      fail('expected Red assertion message is invalid');
    }
    return stripped;
  });
  const combined = messages.join('\n---\n');
  if (!combined.includes('expect(')
      || /(?:Test timeout|TimeoutError|browser has been closed|worker process|was interrupted|Target page, context or browser has been closed)/i.test(combined)) {
    fail('expected Red failure is not a test assertion');
  }
  return Object.freeze({
    kind: 'assertion',
    signature_sha256: sha256(combined),
  });
}

function validatePlaywrightReport(value, selection, options = {}) {
  const expectedOutcome = options.expectedOutcome === undefined ? 'Pass' : options.expectedOutcome;
  const expectedFailureId = options.expectedFailureId === undefined ? null : options.expectedFailureId;
  if ((expectedOutcome !== 'Pass' && expectedOutcome !== 'Red')
      || (expectedOutcome === 'Pass' && expectedFailureId !== null)
      || (expectedOutcome === 'Red' && !selection.test_ids.includes(expectedFailureId))) {
    fail('Playwright report expectation is invalid');
  }
  if (!isPlainObject(value) || !isPlainObject(value.config)) {
    fail('Playwright report must be an object');
  }
  if (value.config.version !== PLAYWRIGHT_VERSION) {
    fail('Playwright report version mismatch');
  }
  if (!Array.isArray(value.errors) || value.errors.length !== 0) {
    fail('Playwright report contains top-level errors');
  }
  if (!isPlainObject(value.stats)) fail('Playwright report stats are invalid');
  const stats = {};
  for (const key of ['expected', 'skipped', 'flaky', 'unexpected']) {
    assertInteger(value.stats[key], 0, selection.expected_count, `report stats ${key}`);
    stats[key] = value.stats[key];
  }
  if (stats.skipped !== 0 || stats.flaky !== 0) {
    fail('report stats contain skipped or flaky cells');
  }
  if (expectedOutcome === 'Pass'
      && (stats.expected !== selection.expected_count || stats.unexpected !== 0)) {
    fail('report stats did not satisfy the exact pass contract');
  }
  if (expectedOutcome === 'Red'
      && (stats.unexpected < 1 || stats.expected + stats.unexpected !== selection.expected_count)) {
    fail('report stats did not satisfy the exact Red contract');
  }

  const specs = [];
  collectReportSpecs(value.suites, specs);
  if (specs.length !== selection.test_ids.length) {
    fail('Playwright report spec count did not match the exact suite');
  }
  const expectedPairs = [];
  for (const testId of selection.test_ids) {
    for (const project of selection.projects) expectedPairs.push(`${testId}\u0000${project.name}`);
  }
  const canonicalPairs = [...expectedPairs];
  const actualPairs = [];
  const cellByPair = new Map();
  const observedTestIds = new Set();
  let browserIdentityJson;
  let browserIdentity;
  let observedPasses = 0;
  const failures = [];
  for (const spec of specs) {
    if (!isPlainObject(spec)
        || typeof spec.title !== 'string'
        || !Array.isArray(spec.tests)) {
      fail('Playwright report spec row is invalid');
    }
    const idMatch = /^\[([a-z][a-z0-9-]{0,63})\]/.exec(spec.title);
    if (!idMatch || !selection.test_ids.includes(idMatch[1])) {
      fail('Playwright report contains an unknown test id');
    }
    if (observedTestIds.has(idMatch[1])) fail('Playwright report contains a duplicate test id');
    observedTestIds.add(idMatch[1]);
    const normalizedFile = normalizeReportFile(spec.file);
    const expectedFile = selection.specs[0];
    if (normalizedFile !== expectedFile && !normalizedFile.endsWith(`/${expectedFile}`)) {
      fail('Playwright report contains an unknown spec file');
    }
    for (const row of spec.tests) {
      if (!isPlainObject(row)
          || typeof row.projectName !== 'string'
          || row.expectedStatus !== 'passed'
          || !Array.isArray(row.results)
          || row.results.length !== 1
          || row.results[0].retry !== 0
          || !Array.isArray(row.results[0].stdout)
          || row.results[0].stdout.length !== 0
          || !Array.isArray(row.results[0].stderr)
          || row.results[0].stderr.length !== 0
          || !Array.isArray(row.annotations)
          || row.annotations.length !== 0) {
        fail('Playwright report test row shape is invalid');
      }
      const result = row.results[0];
      const isPass = row.status === 'expected'
        && result.status === 'passed'
        && Array.isArray(result.errors)
        && result.errors.length === 0;
      const isExpectedRed = expectedOutcome === 'Red'
        && idMatch[1] === expectedFailureId
        && row.status === 'unexpected'
        && result.status === 'failed';
      if (!isPass && !isExpectedRed) {
        fail('Playwright report row did not match the declared outcome');
      }
      const pair = `${idMatch[1]}\u0000${row.projectName}`;
      actualPairs.push(pair);
      if (cellByPair.has(pair)) fail('Playwright report contains duplicate UI proof cells');
      let proof;
      let errorSummary;
      if (isPass) {
        observedPasses += 1;
        proof = parseUiProofAttachment(result, idMatch[1], row.projectName);
      } else {
        errorSummary = sanitizeAssertionFailure(result, selection);
        proof = parseUiProofAttachment(result, idMatch[1], row.projectName, {
          allowMissing: true,
          allowExpectedFailure: true,
        });
        failures.push(Object.freeze({
          test_id: idMatch[1],
          project: row.projectName,
          error: errorSummary,
          proof_present: proof !== null,
        }));
      }
      if (proof !== null) {
        const identityJson = JSON.stringify(proof.browser_identity);
        if (browserIdentityJson === undefined) {
          browserIdentityJson = identityJson;
          browserIdentity = proof.browser_identity;
        } else if (browserIdentityJson !== identityJson) {
          fail('Playwright report UI proof browser identity drifted across cells');
        }
      }
      cellByPair.set(pair, Object.freeze({
        test_id: idMatch[1],
        project: row.projectName,
        status: isPass ? 'pass' : 'red',
        proof,
        missing_reason: proof === null ? 'assertion-before-proof' : null,
        errors: Object.freeze(errorSummary ? [errorSummary] : []),
      }));
    }
  }
  expectedPairs.sort();
  actualPairs.sort();
  if (actualPairs.length !== expectedPairs.length
      || actualPairs.some((entry, index) => entry !== expectedPairs[index])) {
    fail('report selection did not match the exact suite matrix');
  }
  if (observedPasses !== stats.expected || failures.length !== stats.unexpected) {
    fail('report stats did not match the exact row outcomes');
  }
  if (browserIdentity === undefined) fail('Playwright report did not retain browser identity evidence');
  const cells = canonicalPairs.map((pair) => cellByPair.get(pair));
  return Object.freeze({
    stats: Object.freeze(stats),
    expectedOutcome,
    expectedFailureId,
    failures: Object.freeze(failures),
    uiProof: Object.freeze({
      schema_version: 1,
      budgets: Object.freeze({ ...UI_PROOF_BUDGETS }),
      browser_identity: browserIdentity,
      cells: Object.freeze(cells),
    }),
  });
}

function validateInstanceCounters(value) {
  assertExactKeys(value, COUNTER_KEYS, 'instance counters');
  if (value.schema_version !== 1) fail('instance counters schema is unsupported');
  const result = { schema_version: 1 };
  for (const key of COUNTER_KEYS.slice(1)) {
    assertInteger(value[key], 0, Number.MAX_SAFE_INTEGER, `instance counters ${key}`);
    result[key] = value[key];
  }
  return Object.freeze(result);
}

function requestHarness(handoff, requestPath, options = {}) {
  const method = options.method || 'GET';
  const timeoutMs = options.timeoutMs || 3_000;
  const headers = options.headers || {};
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      if (error) reject(error);
      else resolve(value);
    };
    const request = http.request({
      protocol: 'http:',
      hostname: handoff.host,
      port: handoff.port,
      path: requestPath,
      method,
      headers: {
        host: `${handoff.host}:${handoff.port}`,
        ...headers,
      },
      agent: false,
    }, (response) => {
      const chunks = [];
      let bytes = 0;
      response.on('data', (chunk) => {
        bytes += chunk.length;
        if (bytes > MAX_CONTROL_BODY_BYTES) {
          request.destroy();
          finish(new BrowserContractError('harness control response exceeded the size limit'));
          return;
        }
        chunks.push(chunk);
      });
      response.on('end', () => finish(null, {
        statusCode: response.statusCode,
        contentType: String(response.headers['content-type'] || '').split(';', 1)[0].trim(),
        body: Buffer.concat(chunks),
      }));
      response.on('error', () => finish(new BrowserContractError('harness control response failed')));
    });
    request.on('error', () => finish(new BrowserContractError('harness control request failed')));
    request.setTimeout(timeoutMs, () => {
      request.destroy();
      finish(new BrowserContractError('harness control request timed out'));
    });
    timer = setTimeout(() => {
      request.destroy();
      finish(new BrowserContractError('harness control request timed out'));
    }, timeoutMs);
    request.end();
  });
}

async function fetchAndAssertCounters(handoff, phase) {
  const response = await requestHarness(handoff, '/__perf/counters');
  if (response.statusCode !== 200 || response.contentType !== 'application/json') {
    fail(`${phase} counters endpoint returned an invalid response`);
  }
  let parsed;
  try {
    parsed = JSON.parse(response.body.toString('utf8'));
  } catch (_error) {
    fail(`${phase} counters endpoint returned invalid JSON`);
  }
  return validateInstanceCounters(parsed);
}

async function recordBrowserOutboundAttempt(context) {
  const response = await requestHarness(context.handoff, '/__perf/browser-outbound-attempt', {
    method: 'POST',
    headers: { [PERF_CONTROL_HEADER]: context.storageState.cookies[0].value },
  });
  if (response.statusCode !== 204 || response.body.length !== 0) {
    fail('browser outbound counter update failed');
  }
}

function assertZeroCounters(value, phase) {
  for (const key of [
    'denied_requests',
    'outbound_calls',
    'browser_outbound_attempts',
    'server_write_attempts',
    'repository_mutations',
  ]) {
    if (value[key] !== 0) fail(`${phase} counters contain activity`);
  }
}

function removeConsumedRegularFile(filePath, label) {
  try {
    const metadata = fs.lstatSync(filePath);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      fail(`${label} must remain a regular file until cleanup`);
    }
    fs.unlinkSync(filePath);
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    fail(`${label} cleanup failed`);
  }
  assertAbsentPath(filePath, label);
}

function consumeAndValidateReport(context) {
  const body = readRegularFile(context.reportPath, 'Playwright report', MAX_JSON_BYTES);
  let result;
  let validationError;
  try {
    for (const secret of [
      context.storageState.cookies[0].value,
      context.storageStatePath,
      context.handoffPath,
      context.proofPath,
    ]) {
      if (body.includes(secret)) fail('Playwright report contains private runner material');
    }
    const parsed = parseJson(body, 'Playwright report');
    const validated = validatePlaywrightReport(parsed, context.selection, {
      expectedOutcome: context.proof.expected_outcome,
      expectedFailureId: context.proof.expected_failure_id,
    });
    if (validated.uiProof.browser_identity.playwright_version !== context.proof.playwright_version) {
      fail('Playwright report browser identity did not match launch proof');
    }
    result = Object.freeze({
      stats: validated.stats,
      expectedOutcome: validated.expectedOutcome,
      expectedFailureId: validated.expectedFailureId,
      failures: validated.failures,
      uiProof: validated.uiProof,
      sha256: sha256(body),
      bytes: Buffer.byteLength(body),
    });
  } catch (error) {
    validationError = error;
  }
  removeConsumedRegularFile(context.reportPath, 'Playwright report');
  if (validationError) throw validationError;
  return result;
}

function assertBrowserOutputInventory(context, expectedLeaves) {
  if (!Array.isArray(expectedLeaves)
      || expectedLeaves.some((leaf) => typeof leaf !== 'string' || path.basename(leaf) !== leaf)) {
    fail('browser output inventory expectation is invalid');
  }
  let entries;
  try {
    entries = fs.readdirSync(context.outputDir, { withFileTypes: true });
  } catch (_error) {
    fail('browser output directory could not be enumerated after Playwright');
  }
  const expected = [...expectedLeaves].sort();
  const actual = entries.map((entry) => entry.name).sort();
  if (actual.length !== expected.length
      || actual.some((entry, index) => entry !== expected[index])) {
    fail('browser output inventory contains unexpected artifacts');
  }
  for (const entry of entries) {
    let metadata;
    try {
      metadata = fs.lstatSync(path.join(context.outputDir, entry.name));
    } catch (_error) {
      fail('browser output inventory changed during validation');
    }
    if (!entry.isFile() || !metadata.isFile() || metadata.isSymbolicLink()) {
      fail('browser output inventory must contain only regular files');
    }
  }
}

function consumePlaywrightLastRun(context) {
  const lastRunPath = path.join(context.outputDir, '.last-run.json');
  const body = readRegularFile(lastRunPath, 'Playwright last-run marker', MAX_CONTROL_BODY_BYTES);
  const value = parseJson(body, 'Playwright last-run marker');
  assertExactKeys(value, ['status', 'failedTests'], 'Playwright last-run marker');
  const expectedStatus = context.proof.expected_outcome === 'Red' ? 'failed' : 'passed';
  if (value.status !== expectedStatus || !Array.isArray(value.failedTests)) {
    fail('Playwright last-run marker did not match the declared outcome');
  }
  if (value.failedTests.some((testId) => {
    return typeof testId !== 'string'
      || testId.length === 0
      || testId.length > 1_024
      || /[\u0000-\u001f\u007f]/.test(testId);
  }) || new Set(value.failedTests).size !== value.failedTests.length) {
    fail('Playwright last-run marker failed test ids are invalid');
  }
  if ((context.proof.expected_outcome === 'Pass' && value.failedTests.length !== 0)
      || (context.proof.expected_outcome === 'Red'
        && (value.failedTests.length < 1
          || value.failedTests.length > context.selection.projects.length))) {
    fail('Playwright last-run marker failed test count is invalid');
  }
  try {
    fs.unlinkSync(lastRunPath);
  } catch (_error) {
    fail('Playwright last-run marker cleanup failed');
  }
  try {
    fs.lstatSync(lastRunPath);
    fail('Playwright last-run marker cleanup failed');
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    if (!error || error.code !== 'ENOENT') fail('Playwright last-run marker cleanup failed');
  }
  return Object.freeze({ status: value.status, failedTestCount: value.failedTests.length });
}

const SAFE_CHILD_ENVIRONMENT = new Set([
  'APPDATA',
  'CI',
  'COMSPEC',
  'COMMONPROGRAMFILES',
  'COMMONPROGRAMFILES(X86)',
  'HOME',
  'HOMEDRIVE',
  'HOMEPATH',
  'LOCALAPPDATA',
  'NUMBER_OF_PROCESSORS',
  'OS',
  'PATH',
  'PATHEXT',
  'PROCESSOR_ARCHITECTURE',
  'PROGRAMDATA',
  'PROGRAMFILES',
  'PROGRAMFILES(X86)',
  'SYSTEMDRIVE',
  'SYSTEMROOT',
  'TEMP',
  'TMP',
  'USERDOMAIN',
  'USERNAME',
  'USERPROFILE',
  'WINDIR',
]);

function buildChildEnvironment(env) {
  const result = {};
  for (const [key, value] of Object.entries(env)) {
    const upper = key.toUpperCase();
    if (SAFE_CHILD_ENVIRONMENT.has(upper) || REQUIRED_ENV_KEYS.includes(key)) {
      result[key] = value;
    }
  }
  return result;
}

function publishBrowserResult(context, value) {
  const serialized = JSON.stringify(value);
  for (const secret of [
    context.storageState.cookies[0].value,
    context.storageStatePath,
    context.handoffPath,
    context.proofPath,
  ]) {
    if (serialized.includes(secret)) fail('browser result contains private runner material');
  }
  try {
    publishExclusiveJson(context.resultPath, value);
  } catch (error) {
    if (error instanceof PerfContractError) fail(error.message);
    throw error;
  }
}

function safeErrorMessage(error) {
  if (error instanceof BrowserContractError || error instanceof PerfContractError) {
    return error.message;
  }
  return 'unexpected dashboard browser failure';
}

module.exports = {
  BrowserContractError,
  CHROMIUM_REVISION,
  CHROMIUM_VERSION,
  COUNTER_KEYS,
  EXPECTED_SELECTION,
  LOCKED_FONT_ROUTES,
  PERF_CONTROL_HEADER,
  PLAYWRIGHT_VERSION,
  REQUIRED_ENV_KEYS,
  RUNNER_VERSION,
  UI_PROOF_ATTACHMENT,
  UI_PROOF_BUDGETS,
  assertZeroCounters,
  assertBrowserOutputInventory,
  buildChildEnvironment,
  consumePlaywrightLastRun,
  fetchAndAssertCounters,
  fetchAndAssertInstance,
  isAllowedBrowserReadUrl,
  isSameHarnessWebSocketOrigin,
  loadBrowserRunContext,
  loadBrowserWorkerContext,
  publishBrowserResult,
  playwrightMaxFailures,
  consumeAndValidateReport,
  recordBrowserOutboundAttempt,
  safeErrorMessage,
  validateInstanceCounters,
  validatePlaywrightReport,
  validateSelection,
  validateStorageState,
};
