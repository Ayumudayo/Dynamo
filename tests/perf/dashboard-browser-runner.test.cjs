'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const {
  CHROMIUM_REVISION,
  CHROMIUM_VERSION,
  BrowserContractError,
  LOCKED_FONT_ROUTES,
  UI_PROOF_ATTACHMENT,
  UI_PROOF_BUDGETS,
  buildChildEnvironment,
  consumeAndValidateReport,
  isAllowedBrowserReadUrl,
  isSameHarnessWebSocketOrigin,
  loadBrowserRunContext,
  loadBrowserWorkerContext,
  playwrightMaxFailures,
  publishBrowserResult,
  recordBrowserOutboundAttempt,
  safeErrorMessage,
  validatePlaywrightReport,
  validateInstanceCounters,
  validateSelection,
  validateStorageState,
} = require('../../scripts/perf/dashboard-browser-contract.cjs');
const {
  assertLocalPlaywright,
  buildPlaywrightSpawnSpec,
  runBrowserCheckpoint,
} = require('../../scripts/perf/run-dashboard-browser.cjs');

const NOW = 1_800_000_000_000;
const REVISION = 'a'.repeat(40);
const NONCE = 'b'.repeat(64);
const HASH = 'c'.repeat(64);
const COOKIE_VALUE = 'runner-cookie-secret-value';
const GUILD_ID = '123456789012345678';
const SPEC = 'tests/playwright/dashboard-isolated-readonly.spec.cjs';
const PROJECTS = [
  ['desktop', 1440, 1100],
  ['tablet', 1024, 900],
  ['small-tablet', 768, 1024],
  ['mobile-sanity', 375, 812],
];
const TEST_IDS = [
  'public-responsive-reduced-motion',
  'guild-readonly-dialog',
  'same-origin-font-proof',
];
const REPOSITORY_ROOT = path.resolve(__dirname, '..', '..');
const CONFIG_IMPORT_TIMEOUT_MS = 30_000;
const REQUIRED_FONT_FILES = [
  'FiraSans-Regular.woff2',
  'FiraSans-Bold.woff2',
  'FiraCode-Variable.woff2',
];

function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function writeExclusive(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value)}\n`, { flag: 'wx', mode: 0o600 });
}

function handoff() {
  return {
    schema_version: 1,
    issued_at_unix_ms: NOW - 1_000,
    expires_at_unix_ms: NOW + 10 * 60_000,
    revision: REVISION,
    nonce: NONCE,
    pid: 4242,
    host: '127.0.0.1',
    port: 49152,
    dynamic_port: true,
    fixture_mode: 'ReadOnly',
    fixture: { version: 'read-only-v1', sha256: HASH },
    source_state: { head: REVISION, clean: true, diff_sha256: HASH },
    environment: {
      fingerprint_sha256: 'd'.repeat(64),
      os_build: 'Windows test build',
      arch: 'x64',
      cpu_model: 'test cpu',
      logical_cores: 8,
      power_profile: 'balanced',
      node_version: 'v24.0.0',
      rustc_version: 'rustc test',
      cargo_profile: 'debug',
    },
  };
}

function storageState(overrides = {}) {
  return {
    cookies: [{
      name: 'dynamo_perf_session',
      value: COOKIE_VALUE,
      domain: '127.0.0.1',
      path: '/',
      expires: Math.floor((NOW + 9 * 60_000) / 1_000),
      httpOnly: true,
      secure: false,
      sameSite: 'Strict',
      ...overrides,
    }],
    origins: [],
  };
}

function selection(overrides = {}) {
  return {
    schema_version: 1,
    suite_id: 'dashboard-readonly-v1',
    specs: [SPEC],
    projects: PROJECTS.map(([name, width, height]) => ({
      name,
      viewport: { width, height },
    })),
    test_ids: [...TEST_IDS],
    expected_count: 12,
    ...overrides,
  };
}

function instanceSnapshot(overrides = {}) {
  return {
    schema_version: 1,
    revision: REVISION,
    nonce: NONCE,
    pid: 4242,
    fixture_mode: 'ReadOnly',
    fixture: { version: 'read-only-v1', sha256: HASH },
    outbound_calls: 0,
    browser_outbound_attempts: 0,
    ...overrides,
  };
}

function counters(overrides = {}) {
  return {
    schema_version: 1,
    outbound_calls: 0,
    browser_outbound_attempts: 0,
    denied_requests: 0,
    server_write_attempts: 0,
    repository_mutations: 0,
    provider_guild_lookups: 0,
    repository_reads: 0,
    ...overrides,
  };
}

function proofFor(testId, projectName, overrides = {}) {
  const dialogApplicable = testId === 'guild-readonly-dialog';
  return {
    schema_version: 1,
    test_id: testId,
    project: projectName,
    browser_identity: {
      playwright_version: '1.58.2',
      browser_name: 'chromium',
      browser_revision: CHROMIUM_REVISION,
      browser_version: CHROMIUM_VERSION,
    },
    first_text_visible_ms: 125.25,
    font_ready_ms: 240.5,
    cls: 0.001,
    horizontal_overflow_px: 0,
    dom_nodes: 275,
    computed_fonts: {
      body: '"Fira Sans", sans-serif',
      heading: '"Fira Code", monospace',
    },
    locked_font_hashes: testId === 'same-origin-font-proof'
      ? REQUIRED_FONT_FILES
        .map((file) => LOCKED_FONT_ROUTES.find((font) => font.file === file).sha256)
        .sort()
      : [],
    dialog_focus: {
      applicable: dialogApplicable,
      initial_focus: dialogApplicable ? true : null,
      tab_wrap: dialogApplicable ? true : null,
      escape_closes: dialogApplicable ? true : null,
      return_focus: dialogApplicable ? true : null,
    },
    ...overrides,
  };
}

function report(overrides = {}) {
  const specs = TEST_IDS.map((id) => ({
    title: `[${id}] contract row`,
    file: SPEC,
    tests: PROJECTS.map(([projectName]) => ({
      projectName,
      expectedStatus: 'passed',
      status: 'expected',
      annotations: [],
      results: [{
        status: 'passed',
        retry: 0,
        errors: [],
        stdout: [],
        stderr: [],
        attachments: [{
          name: UI_PROOF_ATTACHMENT,
          contentType: 'application/json',
          body: Buffer.from(JSON.stringify(proofFor(id, projectName))).toString('base64'),
        }],
      }],
    })),
  }));
  return {
    config: { version: '1.58.2' },
    suites: [{ title: 'dashboard isolated read-only', file: SPEC, specs }],
    errors: [],
    stats: {
      startTime: '2027-01-15T08:00:00.000Z',
      duration: 1234,
      expected: 12,
      skipped: 0,
      unexpected: 0,
      flaky: 0,
    },
    ...overrides,
  };
}

function markAssertionFailure(value, testId, projectName, options = {}) {
  const spec = value.suites[0].specs.find((entry) => entry.title.startsWith(`[${testId}]`));
  assert.ok(spec, testId);
  const row = spec.tests.find((entry) => entry.projectName === projectName);
  assert.ok(row, projectName);
  const result = row.results[0];
  const location = {
    file: path.join(REPOSITORY_ROOT, SPEC),
    column: 5,
    line: 49,
  };
  const message = options.message || [
    '\u001b[31mError: intentional Red contract assertion\u001b[39m',
    '',
    'expect(received).toBe(expected)',
    '',
    `at ${location.file}:${location.line}:${location.column}`,
  ].join('\n');
  row.status = 'unexpected';
  result.status = 'failed';
  result.error = {
    message,
    stack: message,
    location: { ...location },
    snippet: 'expect(received).toBe(expected);',
  };
  result.errors = [{ message, location: { ...location } }];
  if (options.attachment === false) {
    result.attachments = [];
  } else {
    result.attachments = [{
      name: UI_PROOF_ATTACHMENT,
      contentType: 'application/json',
      body: Buffer.from(JSON.stringify(proofFor(
        testId,
        projectName,
        options.proofOverrides || {},
      ))).toString('base64'),
    }];
  }
  value.stats.expected -= 1;
  value.stats.unexpected += 1;
  return value;
}

function writePlaywrightArtifacts(context, reportValue) {
  fs.mkdirSync(context.outputDir);
  fs.writeFileSync(context.reportPath, `${JSON.stringify(reportValue)}\n`, { flag: 'wx' });
  fs.writeFileSync(path.join(context.outputDir, '.last-run.json'), JSON.stringify({
    status: reportValue.stats.unexpected === 0 ? 'passed' : 'failed',
    failedTests: Array.from(
      { length: reportValue.stats.unexpected },
      (_value, index) => `synthetic-playwright-test-${index + 1}`,
    ),
  }), { flag: 'wx' });
}

function runtime(t, proofOverrides = {}, clock = NOW) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'dynamo-browser-contract-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const outputDir = path.join(directory, 'browser-artifacts');
  const paths = {
    handoff: path.join(directory, 'handoff.json'),
    storage: path.join(directory, 'storage.json'),
    selection: path.join(directory, 'selection.json'),
    proof: path.join(directory, 'proof.json'),
    report: path.join(outputDir, 'playwright-report.json'),
    result: path.join(outputDir, 'browser-result.json'),
    outputDir,
  };
  const values = {
    handoff: {
      ...handoff(),
      issued_at_unix_ms: clock - 1_000,
      expires_at_unix_ms: clock + 10 * 60_000,
    },
    storage: storageState({ expires: Math.floor((clock + 9 * 60_000) / 1_000) }),
    selection: selection(),
  };
  const raw = {
    handoff: `${JSON.stringify(values.handoff)}\n`,
    storage: `${JSON.stringify(values.storage)}\n`,
    selection: `${JSON.stringify(values.selection)}\n`,
  };
  fs.writeFileSync(paths.handoff, raw.handoff, { flag: 'wx', mode: 0o600 });
  fs.writeFileSync(paths.storage, raw.storage, { flag: 'wx', mode: 0o600 });
  fs.writeFileSync(paths.selection, raw.selection, { flag: 'wx', mode: 0o600 });
  const proof = {
    schema_version: 1,
    runner_version: 'dashboard-browser-v1',
    playwright_version: '1.58.2',
    expected_outcome: 'Pass',
    expected_failure_id: null,
    issued_at_unix_ms: clock - 500,
    expires_at_unix_ms: clock + 9 * 60_000,
    nonce: NONCE,
    guild_id: GUILD_ID,
    handoff_sha256: sha256(raw.handoff),
    storage_state_sha256: sha256(raw.storage),
    selection_sha256: sha256(raw.selection),
    ...proofOverrides,
  };
  writeExclusive(paths.proof, proof);
  const env = {
    PERF_INSTANCE_HANDOFF: paths.handoff,
    PLAYWRIGHT_PERF_STORAGE_STATE: paths.storage,
    PLAYWRIGHT_PERF_SELECTION: paths.selection,
    PLAYWRIGHT_PERF_LAUNCH_PROOF: paths.proof,
    PLAYWRIGHT_PERF_REPORT: paths.report,
    PLAYWRIGHT_PERF_RESULT: paths.result,
    PLAYWRIGHT_PERF_OUTPUT_DIR: paths.outputDir,
  };
  return { directory, paths, values, proof, env };
}

test('direct entry requires the complete launcher-only environment and proof', (t) => {
  const fixture = runtime(t);
  for (const key of Object.keys(fixture.env)) {
    const env = { ...fixture.env };
    delete env[key];
    assert.throws(
      () => loadBrowserRunContext(env, NOW),
      (error) => error instanceof BrowserContractError && !String(error.message).includes(COOKIE_VALUE),
      key,
    );
  }

  assert.throws(
    () => loadBrowserRunContext({ ...fixture.env, PLAYWRIGHT_BASE_URL: '' }, NOW),
    /PLAYWRIGHT_BASE_URL is forbidden/,
  );
  assert.throws(
    () => loadBrowserRunContext({ ...fixture.env, node_options: '--require attacker.js' }, NOW),
    /NODE_OPTIONS is forbidden/,
  );
  assert.throws(
    () => loadBrowserRunContext({ ...fixture.env, custom_proxy: 'http:\/\/proxy.invalid' }, NOW),
    /proxy environment is forbidden/,
  );
});

test('runner-only config refuses before importing the Playwright package', () => {
  const result = spawnSync(
    process.execPath,
    ['-e', "require('./playwright.dashboard.isolated.config.cjs')"],
    {
      cwd: REPOSITORY_ROOT,
      env: { PATH: process.env.PATH, SYSTEMROOT: process.env.SYSTEMROOT },
      encoding: 'utf8',
      windowsHide: true,
      timeout: 5_000,
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /PERF_INSTANCE_HANDOFF is required/);
  assert.doesNotMatch(result.stderr, /Cannot find module '@playwright\/test'/);
});

test('valid config loads while browser output, report, and result are still absent', (t) => {
  const fixture = runtime(t);
  const childEnv = buildChildEnvironment({
    ...fixture.env,
    PATH: process.env.PATH,
    SYSTEMROOT: process.env.SYSTEMROOT,
  });
  const result = spawnSync(
    process.execPath,
    [
      '-e',
      `const fs=require('node:fs');Date.now=()=>${NOW};const config=require('./playwright.dashboard.isolated.config.cjs');fs.writeSync(1,JSON.stringify({outputDir:config.outputDir,maxFailures:config.maxFailures}));process.exit(0)`,
    ],
    {
      cwd: REPOSITORY_ROOT,
      env: childEnv,
      encoding: 'utf8',
      windowsHide: true,
      // Importing @playwright/test is a cold, disk-heavy operation on Windows
      // and can exceed five seconds under concurrent builds or virus scanning.
      // The child exits explicitly after a synchronous proof write, so any
      // Playwright-owned open handles cannot extend its lifetime.
      timeout: CONFIG_IMPORT_TIMEOUT_MS,
    },
  );
  assert.equal(result.error, undefined, result.error && result.error.message);
  assert.equal(result.signal, null, `config child terminated by ${result.signal}`);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), {
    outputDir: fixture.paths.outputDir,
    maxFailures: 1,
  });
  assert.equal(fs.existsSync(fixture.paths.outputDir), false);
  assert.equal(fs.existsSync(fixture.paths.report), false);
  assert.equal(fs.existsSync(fixture.paths.result), false);
});

test('Playwright child uses only the fixed local CLI with shell disabled', (t) => {
  const fixture = runtime(t);
  const observed = buildPlaywrightSpawnSpec({
    ...fixture.env,
    PATH: process.env.PATH,
    SYSTEMROOT: process.env.SYSTEMROOT,
    NODE_OPTIONS: '--require attacker.js',
    HTTPS_PROXY: 'http://proxy.invalid',
    BOT_TOKEN: 'secret',
  });
  assert.equal(observed.executable, process.execPath);
  assert.deepEqual([...observed.args], [
    path.join(REPOSITORY_ROOT, 'node_modules', 'playwright', 'cli.js'),
    'test',
    '--config',
    path.join(REPOSITORY_ROOT, 'playwright.dashboard.isolated.config.cjs'),
  ]);
  assert.equal(observed.options.cwd, REPOSITORY_ROOT);
  assert.equal(observed.options.shell, false);
  assert.equal(observed.options.windowsHide, true);
  assert.deepEqual([...observed.options.stdio], ['ignore', 'pipe', 'pipe']);
  assert.equal(Object.hasOwn(observed.options.env, 'NODE_OPTIONS'), false);
  assert.equal(Object.hasOwn(observed.options.env, 'HTTPS_PROXY'), false);
  assert.equal(Object.hasOwn(observed.options.env, 'BOT_TOKEN'), false);
});

test('locked Playwright metadata binds the bundled Chromium revision and version', () => {
  assert.deepEqual(assertLocalPlaywright(), {
    playwright_version: '1.58.2',
    browser_name: 'chromium',
    browser_revision: CHROMIUM_REVISION,
    browser_version: CHROMIUM_VERSION,
  });
});

test('spawned Node bootstrap cannot observe caller Node hooks, proxies, debug flags, or secrets', (t) => {
  const fixture = runtime(t);
  const forbidden = [
    'NODE_OPTIONS',
    'NODE_PATH',
    'DEBUG',
    'HTTP_PROXY',
    'HTTPS_PROXY',
    'ALL_PROXY',
    'NO_PROXY',
    'CUSTOM_PROXY',
    'MONGODB_URI',
    'DISCORD_CLIENT_SECRET',
    'BOT_TOKEN',
    'TOSS_CLIENT_SECRET',
  ];
  const spawnSpec = buildPlaywrightSpawnSpec({
    ...fixture.env,
    PATH: process.env.PATH,
    SYSTEMROOT: process.env.SYSTEMROOT,
    NODE_OPTIONS: '--require=Z:\\definitely-missing\\bootstrap-attack.js',
    NODE_PATH: 'Z:\\attacker-modules',
    DEBUG: '*',
    HTTP_PROXY: 'http://proxy.invalid',
    HTTPS_PROXY: 'http://proxy.invalid',
    ALL_PROXY: 'socks://proxy.invalid',
    NO_PROXY: '*',
    CUSTOM_PROXY: 'http://proxy.invalid',
    MONGODB_URI: 'mongodb://production.invalid',
    DISCORD_CLIENT_SECRET: 'discord-secret',
    BOT_TOKEN: 'bot-secret',
    TOSS_CLIENT_SECRET: 'toss-secret',
  });
  const stub = spawnSync(
    spawnSpec.executable,
    [
      '-e',
      `const keys=${JSON.stringify(forbidden)};process.stdout.write(JSON.stringify(Object.fromEntries(keys.map((key)=>[key,Object.hasOwn(process.env,key)]))))`,
    ],
    {
      cwd: spawnSpec.options.cwd,
      env: spawnSpec.options.env,
      shell: false,
      windowsHide: true,
      encoding: 'utf8',
      timeout: 5_000,
    },
  );
  assert.equal(stub.status, 0, stub.stderr);
  assert.deepEqual(
    JSON.parse(stub.stdout),
    Object.fromEntries(forbidden.map((key) => [key, false])),
  );
});

test('a valid launcher context binds handoff, proof, storage, selection, and output leaves', (t) => {
  const fixture = runtime(t);
  const context = loadBrowserRunContext(fixture.env, NOW);
  assert.equal(context.baseUrl, 'http://127.0.0.1:49152');
  assert.equal(context.guildId, GUILD_ID);
  assert.equal(context.handoff.nonce, NONCE);
  assert.equal(context.proof.playwright_version, '1.58.2');
  assert.equal(context.proof.expected_outcome, 'Pass');
  assert.equal(context.proof.expected_failure_id, null);
  assert.equal(context.storageState.cookies.length, 1);
  assert.equal(context.selection.expected_count, 12);
  assert.deepEqual(context.selection.projects.map((value) => value.name), PROJECTS.map(([name]) => name));
  assert.equal(context.reportPath, fixture.paths.report);
  assert.equal(context.resultPath, fixture.paths.result);
});

test('launch proof binds Pass or one canonical expected Red failure id', (t) => {
  const red = runtime(t, {
    expected_outcome: 'Red',
    expected_failure_id: TEST_IDS[1],
  });
  const redContext = loadBrowserRunContext(red.env, NOW);
  assert.equal(redContext.proof.expected_outcome, 'Red');
  assert.equal(redContext.proof.expected_failure_id, TEST_IDS[1]);

  for (const proofOverrides of [
    { expected_outcome: 'Pass', expected_failure_id: TEST_IDS[0] },
    { expected_outcome: 'Red', expected_failure_id: null },
    { expected_outcome: 'Red', expected_failure_id: 'unknown-test' },
    { expected_outcome: 'Amber', expected_failure_id: null },
  ]) {
    const invalid = runtime(t, proofOverrides);
    assert.throws(() => loadBrowserRunContext(invalid.env, NOW), BrowserContractError);
  }
});

test('Red disables Playwright fail-fast while Pass retains one-failure fail-fast', () => {
  assert.equal(playwrightMaxFailures('Pass'), 1);
  assert.equal(playwrightMaxFailures('Red'), 0);
  assert.throws(() => playwrightMaxFailures('Amber'), /max-failures outcome is invalid/);
});

test('parent context requires an absent launcher-owned browser-artifacts leaf', (t) => {
  const nested = runtime(t);
  fs.mkdirSync(nested.paths.outputDir);
  const nestedStorage = path.join(nested.paths.outputDir, 'storage-input.json');
  const storageRaw = fs.readFileSync(nested.paths.storage, 'utf8');
  fs.writeFileSync(nestedStorage, storageRaw, { flag: 'wx', mode: 0o600 });
  const proof = {
    ...nested.proof,
    storage_state_sha256: sha256(storageRaw),
  };
  fs.rmSync(nested.paths.proof);
  writeExclusive(nested.paths.proof, proof);
  assert.throws(
    () => loadBrowserRunContext({
      ...nested.env,
      PLAYWRIGHT_PERF_STORAGE_STATE: nestedStorage,
    }, NOW),
    /storage state must remain outside the browser output directory/,
  );
  assert.throws(
    () => loadBrowserWorkerContext({
      ...nested.env,
      PLAYWRIGHT_PERF_STORAGE_STATE: nestedStorage,
    }, NOW),
    /storage state must remain outside the browser output directory/,
  );

  const dirty = runtime(t);
  fs.mkdirSync(dirty.paths.outputDir);
  fs.writeFileSync(path.join(dirty.paths.outputDir, 'stale.txt'), 'stale', { flag: 'wx' });
  assert.throws(
    () => loadBrowserRunContext(dirty.env, NOW),
    /browser output directory must be absent before Playwright starts/,
  );

  const wrongLeaf = runtime(t);
  fs.mkdirSync(wrongLeaf.paths.outputDir);
  const renamedOutput = path.join(wrongLeaf.directory, 'browser-output');
  fs.renameSync(wrongLeaf.paths.outputDir, renamedOutput);
  assert.throws(
    () => loadBrowserRunContext({
      ...wrongLeaf.env,
      PLAYWRIGHT_PERF_OUTPUT_DIR: renamedOutput,
      PLAYWRIGHT_PERF_REPORT: path.join(renamedOutput, 'playwright-report.json'),
      PLAYWRIGHT_PERF_RESULT: path.join(renamedOutput, 'browser-result.json'),
    }, NOW),
    /browser-artifacts leaf/,
  );
});

test('worker context accepts only native Playwright artifact directories after startup cleanup', (t) => {
  const empty = runtime(t);
  fs.mkdirSync(empty.paths.outputDir);
  assert.equal(loadBrowserWorkerContext(empty.env, NOW).outputDir, empty.paths.outputDir);

  const fixture = runtime(t);
  loadBrowserRunContext(fixture.env, NOW);
  fs.mkdirSync(fixture.paths.outputDir);
  fs.mkdirSync(path.join(fixture.paths.outputDir, '.playwright-artifacts-0'));
  const worker = loadBrowserWorkerContext(fixture.env, NOW);
  assert.equal(worker.outputDir, fixture.paths.outputDir);
  assert.equal(worker.guildId, GUILD_ID);

  assert.throws(
    () => loadBrowserRunContext(fixture.env, NOW),
    /browser output directory must be absent before Playwright starts/,
  );

  const unexpected = runtime(t);
  fs.mkdirSync(unexpected.paths.outputDir);
  fs.writeFileSync(path.join(unexpected.paths.outputDir, 'stale.txt'), 'stale', { flag: 'wx' });
  assert.throws(
    () => loadBrowserWorkerContext(unexpected.env, NOW),
    /non-Playwright artifact/,
  );

  const reparse = runtime(t);
  const external = path.join(reparse.directory, 'external-artifacts');
  fs.mkdirSync(external);
  fs.mkdirSync(reparse.paths.outputDir);
  fs.symlinkSync(
    external,
    path.join(reparse.paths.outputDir, '.playwright-artifacts-0'),
    process.platform === 'win32' ? 'junction' : 'dir',
  );
  assert.throws(
    () => loadBrowserWorkerContext(reparse.env, NOW),
    /native regular directories/,
  );

  const outputReparse = runtime(t);
  const externalOutput = path.join(outputReparse.directory, 'external-output');
  fs.mkdirSync(externalOutput);
  fs.symlinkSync(
    externalOutput,
    outputReparse.paths.outputDir,
    process.platform === 'win32' ? 'junction' : 'dir',
  );
  assert.throws(
    () => loadBrowserWorkerContext(outputReparse.env, NOW),
    /native regular directory/,
  );
});

test('proof hashes and lifetime fail closed on any stale or swapped input', (t) => {
  const fixture = runtime(t);
  fs.appendFileSync(fixture.paths.storage, ' ');
  assert.throws(() => loadBrowserRunContext(fixture.env, NOW), /storage state hash mismatch/);

  const other = runtime(t);
  const stale = { ...other.proof, expires_at_unix_ms: NOW - 1 };
  fs.rmSync(other.paths.proof);
  writeExclusive(other.paths.proof, stale);
  assert.throws(() => loadBrowserRunContext(other.env, NOW), /launch proof expired/);

  const wrongVersion = runtime(t);
  fs.rmSync(wrongVersion.paths.proof);
  writeExclusive(wrongVersion.paths.proof, {
    ...wrongVersion.proof,
    playwright_version: '1.58.1',
  });
  assert.throws(
    () => loadBrowserRunContext(wrongVersion.env, NOW),
    /launch proof Playwright version mismatch/,
  );
});

test('storage validation requires exactly one nonexpired same-instance strict cookie', () => {
  const expected = handoff();
  assert.equal(validateStorageState(storageState(), expected, NOW).cookies[0].name, 'dynamo_perf_session');

  const invalid = [
    { cookies: [], origins: [] },
    { cookies: storageState().cookies, origins: [{ origin: 'http://127.0.0.1', localStorage: [] }] },
    storageState({ domain: 'localhost' }),
    storageState({ path: '/guild' }),
    storageState({ expires: Math.floor(NOW / 1000) - 1 }),
    storageState({ httpOnly: false }),
    storageState({ secure: true }),
    storageState({ sameSite: 'Lax' }),
    storageState({ value: 'secret\r\nheader' }),
  ];
  for (const value of invalid) {
    assert.throws(() => validateStorageState(value, expected, NOW), BrowserContractError);
  }
});

test('counter schema requires the exact eight Rust harness fields', () => {
  assert.deepEqual(validateInstanceCounters(counters()), counters());
  const missing = counters();
  delete missing.repository_reads;
  assert.throws(() => validateInstanceCounters(missing), /fields do not match schema/);
  assert.throws(
    () => validateInstanceCounters({ ...counters(), extra_counter: 0 }),
    /fields do not match schema/,
  );
});

test('selection validation is an exact immutable 3 by 4 matrix', () => {
  const valid = validateSelection(selection());
  assert.equal(valid.expected_count, 12);
  for (const value of [
    selection({ expected_count: 11 }),
    selection({ specs: ['tests/playwright/other.spec.cjs'] }),
    selection({ projects: selection().projects.slice(0, 3) }),
    selection({ test_ids: [...TEST_IDS, 'extra'] }),
    { ...selection(), unknown: true },
  ]) {
    assert.throws(() => validateSelection(value), BrowserContractError);
  }
});

test('tracked browser suite manifest is the exact runner selection', () => {
  const manifestPath = path.join(
    REPOSITORY_ROOT,
    'tests',
    'perf',
    'dashboard-browser-suite-v1.json',
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  assert.deepEqual(validateSelection(manifest), validateSelection(selection()));
});

test('browser allowlist accepts only canonical same-origin reads and known WebSocket origin', () => {
  const baseUrl = 'http://127.0.0.1:49152';
  for (const suffix of [
    '/',
    `/guild/${GUILD_ID}`,
    `/guild/${GUILD_ID}?tab=overview`,
    `/guild/${GUILD_ID}?tab=modules`,
    `/guild/${GUILD_ID}?tab=commands`,
    `/guild/${GUILD_ID}?tab=logs&log_entity=module&log_action=save_settings&log_page=10000`,
    LOCKED_FONT_ROUTES[0].path,
  ]) {
    assert.equal(isAllowedBrowserReadUrl(`${baseUrl}${suffix}`, baseUrl, GUILD_ID), true, suffix);
  }
  for (const value of [
    `${baseUrl}/?secret=1`,
    `${baseUrl}/guild/${GUILD_ID}?token=secret`,
    `${baseUrl}/guild/${GUILD_ID}?tab=logs&log_page=01`,
    `${baseUrl}/guild/${GUILD_ID}?tab=logs&log_action=toggle&log_entity=module`,
    `${baseUrl}/guild/999`,
    `${baseUrl}/__perf/counters`,
    `${baseUrl}/login`,
    `${baseUrl}/assets/fonts/fake-${'f'.repeat(64)}.woff2`,
    `http://localhost:49152/guild/${GUILD_ID}`,
    `http://user@127.0.0.1:49152/guild/${GUILD_ID}`,
  ]) {
    assert.equal(isAllowedBrowserReadUrl(value, baseUrl, GUILD_ID), false, value);
  }
  assert.equal(isSameHarnessWebSocketOrigin('ws://127.0.0.1:49152/socket', baseUrl), true);
  assert.equal(isSameHarnessWebSocketOrigin('ws://localhost:49152/socket', baseUrl), false);
  assert.equal(isSameHarnessWebSocketOrigin('wss://127.0.0.1:49152/socket', baseUrl), false);
});

test('browser outbound accounting uses only the runner-private cookie capability', async (t) => {
  let observed;
  const server = http.createServer((request, response) => {
    observed = {
      method: request.method,
      url: request.url,
      control: request.headers['x-dynamo-perf-control'],
      publicNonce: request.headers['x-dynamo-perf-nonce'],
    };
    response.writeHead(204);
    response.end();
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const address = server.address();
  await recordBrowserOutboundAttempt({
    handoff: { host: '127.0.0.1', port: address.port, nonce: NONCE },
    storageState: storageState(),
  });
  assert.deepEqual(observed, {
    method: 'POST',
    url: '/__perf/browser-outbound-attempt',
    control: COOKIE_VALUE,
    publicNonce: undefined,
  });
});

test('locked browser font routes match the tracked lock manifest', () => {
  const fontLock = JSON.parse(fs.readFileSync(path.join(
    REPOSITORY_ROOT,
    'crates',
    'dashboard',
    'assets',
    'fonts',
    'fonts.lock.json',
  ), 'utf8'));
  assert.equal(LOCKED_FONT_ROUTES.length, 6);
  for (const route of LOCKED_FONT_ROUTES) {
    const asset = fontLock.assets.find((value) => value.file === route.file);
    assert.ok(asset, route.file);
    assert.equal(asset.sha256, route.sha256);
    assert.match(route.path, new RegExp(`${route.sha256}\\.woff2$`));
  }
});

test('Playwright report validation requires every test-project proof cell and no skips', () => {
  const validatedSelection = validateSelection(selection());
  const valid = validatePlaywrightReport(report(), validatedSelection);
  assert.deepEqual(valid.stats, { expected: 12, skipped: 0, flaky: 0, unexpected: 0 });
  assert.deepEqual(valid.uiProof.budgets, UI_PROOF_BUDGETS);
  assert.deepEqual(valid.uiProof.browser_identity, {
    playwright_version: '1.58.2',
    browser_name: 'chromium',
    browser_revision: CHROMIUM_REVISION,
    browser_version: CHROMIUM_VERSION,
  });
  assert.equal(valid.uiProof.cells.length, 12);
  assert.equal(valid.uiProof.cells[0].test_id, TEST_IDS[0]);
  assert.equal(valid.uiProof.cells[0].project, PROJECTS[0][0]);
  assert.equal(valid.uiProof.cells[0].status, 'pass');
  assert.equal(valid.uiProof.cells[0].proof.first_text_visible_ms, 125.25);

  assert.throws(
    () => validatePlaywrightReport(report({
      stats: { expected: 11, skipped: 1, flaky: 0, unexpected: 0 },
    }), validatedSelection),
    /skipped or flaky/,
  );

  const missingCell = report();
  missingCell.suites[0].specs[0].tests.pop();
  assert.throws(
    () => validatePlaywrightReport(missingCell, validatedSelection),
    /report selection did not match the exact suite matrix/,
  );

  const missingProof = report();
  missingProof.suites[0].specs[0].tests[0].results[0].attachments = [];
  assert.throws(
    () => validatePlaywrightReport(missingProof, validatedSelection),
    /UI proof attachment/,
  );

  const budgetFailure = report();
  budgetFailure.suites[0].specs[0].tests[0].results[0].attachments[0].body = Buffer.from(
    JSON.stringify(proofFor(TEST_IDS[0], PROJECTS[0][0], { first_text_visible_ms: 1_001 })),
  ).toString('base64');
  assert.throws(
    () => validatePlaywrightReport(budgetFailure, validatedSelection),
    /first text budget/,
  );

  const forgedDialog = report();
  forgedDialog.suites[0].specs[1].tests[0].results[0].attachments[0].body = Buffer.from(
    JSON.stringify(proofFor(TEST_IDS[1], PROJECTS[0][0], {
      dialog_focus: {
        applicable: true,
        initial_focus: true,
        tab_wrap: false,
        escape_closes: true,
        return_focus: true,
      },
    })),
  ).toString('base64');
  assert.throws(
    () => validatePlaywrightReport(forgedDialog, validatedSelection),
    /dialog focus proof failed/,
  );

  const browserDrift = report();
  browserDrift.suites[0].specs[2].tests[3].results[0].attachments[0].body = Buffer.from(
    JSON.stringify(proofFor(TEST_IDS[2], PROJECTS[3][0], {
      browser_identity: {
        playwright_version: '1.58.2',
        browser_name: 'chromium',
        browser_revision: CHROMIUM_REVISION,
        browser_version: '145.0.7632.7',
      },
    })),
  ).toString('base64');
  assert.throws(
    () => validatePlaywrightReport(browserDrift, validatedSelection),
    /browser identity is invalid/,
  );
});

test('declared Red report keeps exact failed cells and sanitizes assertion evidence', () => {
  const validatedSelection = validateSelection(selection());
  const failedReport = markAssertionFailure(
    report(),
    TEST_IDS[0],
    PROJECTS[0][0],
    { proofOverrides: { first_text_visible_ms: 1_001 } },
  );
  const valid = validatePlaywrightReport(failedReport, validatedSelection, {
    expectedOutcome: 'Red',
    expectedFailureId: TEST_IDS[0],
  });
  assert.deepEqual(valid.stats, { expected: 11, skipped: 0, flaky: 0, unexpected: 1 });
  assert.equal(valid.expectedOutcome, 'Red');
  assert.equal(valid.expectedFailureId, TEST_IDS[0]);
  assert.equal(valid.failures.length, 1);
  assert.deepEqual(valid.failures[0], {
    test_id: TEST_IDS[0],
    project: PROJECTS[0][0],
    error: {
      kind: 'assertion',
      signature_sha256: valid.failures[0].error.signature_sha256,
    },
    proof_present: true,
  });
  assert.match(valid.failures[0].error.signature_sha256, /^[0-9a-f]{64}$/);
  const failedCell = valid.uiProof.cells[0];
  assert.equal(failedCell.status, 'red');
  assert.equal(failedCell.proof.first_text_visible_ms, 1_001);
  assert.equal(failedCell.missing_reason, null);
  assert.deepEqual(failedCell.errors, [valid.failures[0].error]);
  const serialized = JSON.stringify(valid);
  assert.doesNotMatch(serialized, /intentional Red contract assertion/);
  assert.doesNotMatch(serialized, /"(?:location|message|stack|snippet)"/);
});

test('declared Red report explicitly retains assertion-before-proof cells', () => {
  const validatedSelection = validateSelection(selection());
  const failedReport = markAssertionFailure(
    report(),
    TEST_IDS[1],
    PROJECTS[2][0],
    { attachment: false },
  );
  const valid = validatePlaywrightReport(failedReport, validatedSelection, {
    expectedOutcome: 'Red',
    expectedFailureId: TEST_IDS[1],
  });
  const failedCell = valid.uiProof.cells.find((cell) => {
    return cell.test_id === TEST_IDS[1] && cell.project === PROJECTS[2][0];
  });
  assert.equal(failedCell.status, 'red');
  assert.equal(failedCell.proof, null);
  assert.equal(failedCell.missing_reason, 'assertion-before-proof');
  assert.equal(failedCell.errors.length, 1);
  assert.equal(valid.failures[0].proof_present, false);
  assert.equal(valid.uiProof.cells.filter((cell) => cell.status === 'pass').length, 11);
});

test('declared Red accepts multiple failed projects only for the one bound test id', () => {
  const validatedSelection = validateSelection(selection());
  const failedReport = report();
  markAssertionFailure(failedReport, TEST_IDS[2], PROJECTS[0][0]);
  markAssertionFailure(failedReport, TEST_IDS[2], PROJECTS[3][0], { attachment: false });
  const valid = validatePlaywrightReport(failedReport, validatedSelection, {
    expectedOutcome: 'Red',
    expectedFailureId: TEST_IDS[2],
  });
  assert.equal(valid.stats.expected, 10);
  assert.equal(valid.stats.unexpected, 2);
  assert.equal(valid.failures.length, 2);
  assert.equal(valid.uiProof.cells.filter((cell) => cell.status === 'red').length, 2);

  const wrongTest = report();
  markAssertionFailure(wrongTest, TEST_IDS[1], PROJECTS[0][0]);
  assert.throws(
    () => validatePlaywrightReport(wrongTest, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[2],
    }),
    /did not match the declared outcome/,
  );
});

test('declared Red preserves safe dialog and font proof violations without treating them as Pass', () => {
  const validatedSelection = validateSelection(selection());
  const dialogReport = markAssertionFailure(report(), TEST_IDS[1], PROJECTS[0][0], {
    proofOverrides: {
      dialog_focus: {
        applicable: true,
        initial_focus: true,
        tab_wrap: false,
        escape_closes: true,
        return_focus: true,
      },
    },
  });
  const dialog = validatePlaywrightReport(dialogReport, validatedSelection, {
    expectedOutcome: 'Red',
    expectedFailureId: TEST_IDS[1],
  });
  assert.equal(dialog.uiProof.cells[4].status, 'red');
  assert.equal(dialog.uiProof.cells[4].proof.dialog_focus.tab_wrap, false);

  const fontReport = markAssertionFailure(report(), TEST_IDS[2], PROJECTS[0][0], {
    proofOverrides: { locked_font_hashes: [] },
  });
  const font = validatePlaywrightReport(fontReport, validatedSelection, {
    expectedOutcome: 'Red',
    expectedFailureId: TEST_IDS[2],
  });
  assert.equal(font.uiProof.cells[8].status, 'red');
  assert.deepEqual(font.uiProof.cells[8].proof.locked_font_hashes, []);
});

test('declared Red rejects nonassertion failures, incomplete stats, and extra artifacts', () => {
  const validatedSelection = validateSelection(selection());
  assert.throws(
    () => validatePlaywrightReport(report(), validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /exact Red contract/,
  );

  const timeout = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0], {
    message: 'Error: Test timeout of 45000ms exceeded. expect(locator).toBeVisible()',
    attachment: false,
  });
  assert.throws(
    () => validatePlaywrightReport(timeout, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /not a test assertion/,
  );

  const withTopError = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0]);
  withTopError.errors = [{ message: 'worker crash' }];
  assert.throws(
    () => validatePlaywrightReport(withTopError, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /top-level errors/,
  );

  const flaky = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0]);
  flaky.stats.flaky = 1;
  assert.throws(
    () => validatePlaywrightReport(flaky, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /skipped or flaky/,
  );

  const extraAttachment = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0]);
  extraAttachment.suites[0].specs[0].tests[0].results[0].attachments.push({
    name: 'screenshot',
    contentType: 'image/png',
    body: 'AA==',
  });
  assert.throws(
    () => validatePlaywrightReport(extraAttachment, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /exactly one UI proof attachment/,
  );

  const identityDrift = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0], {
    proofOverrides: {
      browser_identity: {
        playwright_version: '1.58.2',
        browser_name: 'chromium',
        browser_revision: CHROMIUM_REVISION,
        browser_version: '145.0.7632.7',
      },
    },
  });
  assert.throws(
    () => validatePlaywrightReport(identityDrift, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /browser identity is invalid/,
  );

  const foreignLocation = markAssertionFailure(report(), TEST_IDS[0], PROJECTS[0][0]);
  const foreignResult = foreignLocation.suites[0].specs[0].tests[0].results[0];
  foreignResult.error.location.file = path.join(REPOSITORY_ROOT, 'tests', 'other.spec.cjs');
  foreignResult.errors[0].location.file = path.join(REPOSITORY_ROOT, 'tests', 'other.spec.cjs');
  assert.throws(
    () => validatePlaywrightReport(foreignLocation, validatedSelection, {
      expectedOutcome: 'Red',
      expectedFailureId: TEST_IDS[0],
    }),
    /outside the isolated browser suite/,
  );
});

test('child environment strips caller controls and all known production secrets', () => {
  const env = buildChildEnvironment({
    PATH: 'test-path',
    SYSTEMROOT: 'C:\\Windows',
    TEMP: 'C:\\Temp',
    PERF_INSTANCE_HANDOFF: 'handoff',
    PLAYWRIGHT_PERF_STORAGE_STATE: 'storage',
    PLAYWRIGHT_PERF_SELECTION: 'selection',
    PLAYWRIGHT_PERF_LAUNCH_PROOF: 'proof',
    PLAYWRIGHT_PERF_REPORT: 'report',
    PLAYWRIGHT_PERF_RESULT: 'result',
    PLAYWRIGHT_PERF_OUTPUT_DIR: 'output',
    PLAYWRIGHT_BASE_URL: 'http://unsafe.invalid',
    NODE_OPTIONS: '--require attacker.js',
    MONGODB_URI: 'secret',
    DISCORD_CLIENT_SECRET: 'secret',
    BOT_TOKEN: 'secret',
    HTTPS_PROXY: 'http://proxy.invalid',
  });
  assert.equal(env.PATH, 'test-path');
  assert.equal(env.PERF_INSTANCE_HANDOFF, 'handoff');
  for (const key of [
    'PLAYWRIGHT_BASE_URL',
    'NODE_OPTIONS',
    'MONGODB_URI',
    'DISCORD_CLIENT_SECRET',
    'BOT_TOKEN',
    'HTTPS_PROXY',
  ]) {
    assert.equal(Object.hasOwn(env, key), false, key);
  }
});

test('browser result publication is exclusive and contains no cookie or private paths', (t) => {
  const fixture = runtime(t);
  const context = loadBrowserRunContext(fixture.env, NOW);
  fs.mkdirSync(fixture.paths.outputDir);
  const value = {
    schema_version: 1,
    runner_version: 'dashboard-browser-v1',
    exit_classification: 'Pass',
  };
  publishBrowserResult(context, value);
  const body = fs.readFileSync(fixture.paths.result, 'utf8');
  assert.equal(JSON.parse(body).exit_classification, 'Pass');
  assert.doesNotMatch(body, new RegExp(COOKIE_VALUE));
  assert.doesNotMatch(body, /storage\.json|handoff\.json|proof\.json/);
  assert.throws(() => publishBrowserResult(context, value), /already exists/);
});

test('consuming an invalid raw report deletes it before surfacing validation failure', (t) => {
  const fixture = runtime(t);
  const context = loadBrowserRunContext(fixture.env, NOW);
  fs.mkdirSync(fixture.paths.outputDir);
  fs.writeFileSync(
    fixture.paths.report,
    JSON.stringify({ leaked_runner_cookie: COOKIE_VALUE }),
    { flag: 'wx' },
  );
  assert.throws(
    () => consumeAndValidateReport(context),
    /private runner material/,
  );
  assert.equal(fs.existsSync(fixture.paths.report), false);
});

test('orchestrator validates pre/post identity and counters before publishing one result', async (t) => {
  const fixture = runtime(t);
  const stdout = [];
  let instanceReads = 0;
  let counterReads = 0;
  const execution = await runBrowserCheckpoint({
    env: fixture.env,
    now: () => NOW,
    fetchInstance: async () => {
      instanceReads += 1;
      return instanceSnapshot();
    },
    fetchCounters: async () => {
      counterReads += 1;
      return counters(counterReads === 1
        ? {}
        : { provider_guild_lookups: 500, repository_reads: 125 });
    },
    spawnPlaywright: async (context) => {
      writePlaywrightArtifacts(context, report());
      return { exitCode: 0, stdout: '', stderr: '' };
    },
    stdout: (value) => stdout.push(value),
  });

  assert.equal(execution.exitCode, 0);
  assert.equal(instanceReads, 2);
  assert.equal(counterReads, 2);
  assert.equal(stdout.length, 1);
  assert.doesNotMatch(stdout[0], new RegExp(COOKIE_VALUE));
  const result = JSON.parse(fs.readFileSync(fixture.paths.result, 'utf8'));
  assert.deepEqual(result.stats, { expected: 12, skipped: 0, flaky: 0, unexpected: 0 });
  assert.deepEqual(result.browser, {
    playwright_version: '1.58.2',
    browser_name: 'chromium',
    browser_revision: CHROMIUM_REVISION,
    browser_version: CHROMIUM_VERSION,
  });
  assert.equal(result.ui_proof.schema_version, 1);
  assert.equal(result.ui_proof.cells.length, 12);
  assert.equal(result.ui_proof.cells[0].status, 'pass');
  assert.equal(result.ui_proof.cells[0].proof.first_text_visible_ms, 125.25);
  assert.equal(result.expected_outcome, 'Pass');
  assert.equal(result.expected_failure_id, null);
  assert.deepEqual(result.failures, []);
  assert.equal(result.exit_classification, 'Pass');
  assert.equal(result.instance.denied_requests_before, 0);
  assert.equal(result.instance.denied_requests_after, 0);
  assert.equal(result.instance.denied_requests_delta, 0);
  assert.equal(result.instance.repository_mutations_after, 0);
  assert.equal(result.instance.browser_outbound_attempts_after, 0);
  assert.equal(result.instance.provider_guild_lookups_before, 0);
  assert.equal(result.instance.provider_guild_lookups_after, 500);
  assert.equal(result.instance.provider_guild_lookups_delta, 500);
  assert.equal(result.instance.repository_reads_before, 0);
  assert.equal(result.instance.repository_reads_after, 125);
  assert.equal(result.instance.repository_reads_delta, 125);
  assert.equal(fs.existsSync(fixture.paths.report), false);
  assert.deepEqual(fs.readdirSync(fixture.paths.outputDir), ['browser-result.json']);
});

test('orchestrator publishes a truthful Red only for exit 1 and the bound assertion id', async (t) => {
  const fixture = runtime(t, {
    expected_outcome: 'Red',
    expected_failure_id: TEST_IDS[0],
  });
  let counterReads = 0;
  const execution = await runBrowserCheckpoint({
    env: fixture.env,
    now: () => NOW,
    fetchInstance: async () => instanceSnapshot(),
    fetchCounters: async () => {
      counterReads += 1;
      return counters(counterReads === 1
        ? {}
        : { provider_guild_lookups: 500, repository_reads: 125 });
    },
    spawnPlaywright: async (context) => {
      const failedReport = markAssertionFailure(
        report(),
        TEST_IDS[0],
        PROJECTS[0][0],
        { proofOverrides: { first_text_visible_ms: 1_001 } },
      );
      writePlaywrightArtifacts(context, failedReport);
      return { exitCode: 1, stdout: '', stderr: '' };
    },
    stdout: () => {},
  });
  assert.equal(execution.exitCode, 0);
  const result = JSON.parse(fs.readFileSync(fixture.paths.result, 'utf8'));
  assert.equal(result.exit_classification, 'Red');
  assert.equal(result.expected_outcome, 'Red');
  assert.equal(result.expected_failure_id, TEST_IDS[0]);
  assert.deepEqual(result.stats, { expected: 11, skipped: 0, flaky: 0, unexpected: 1 });
  assert.equal(result.failures.length, 1);
  assert.equal(result.failures[0].test_id, TEST_IDS[0]);
  assert.equal(result.failures[0].project, PROJECTS[0][0]);
  assert.equal(result.failures[0].error.kind, 'assertion');
  assert.match(result.failures[0].error.signature_sha256, /^[0-9a-f]{64}$/);
  assert.equal(result.ui_proof.cells[0].status, 'red');
  assert.equal(result.ui_proof.cells[0].proof.first_text_visible_ms, 1_001);
  assert.equal(fs.existsSync(fixture.paths.report), false);
  assert.deepEqual(fs.readdirSync(fixture.paths.outputDir), ['browser-result.json']);
  const serialized = JSON.stringify(result);
  assert.doesNotMatch(serialized, /intentional Red contract assertion|"location"|"message"/);
});

test('orchestrator never publishes Red for exit 0 or a different failing test id', async (t) => {
  const wrongExit = runtime(t, {
    expected_outcome: 'Red',
    expected_failure_id: TEST_IDS[0],
  });
  await assert.rejects(
    runBrowserCheckpoint({
      env: wrongExit.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async () => ({ exitCode: 0, stdout: '', stderr: '' }),
      stdout: () => {},
    }),
    /declared Red outcome/,
  );
  assert.equal(fs.existsSync(wrongExit.paths.result), false);

  const wrongId = runtime(t, {
    expected_outcome: 'Red',
    expected_failure_id: TEST_IDS[0],
  });
  await assert.rejects(
    runBrowserCheckpoint({
      env: wrongId.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(
          context,
          markAssertionFailure(report(), TEST_IDS[1], PROJECTS[0][0]),
        );
        return { exitCode: 1, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /did not match the declared outcome/,
  );
  assert.equal(fs.existsSync(wrongId.paths.result), false);
  assert.equal(fs.existsSync(wrongId.paths.report), false);
});

test('counter drift and failed Playwright runs never publish a result', async (t) => {
  const fixture = runtime(t);
  let counterRead = 0;
  let instanceRead = 0;
  await assert.rejects(
    runBrowserCheckpoint({
      env: fixture.env,
      now: () => NOW,
      fetchInstance: async () => {
        instanceRead += 1;
        return instanceRead === 1
          ? instanceSnapshot()
          : instanceSnapshot({ browser_outbound_attempts: 1 });
      },
      fetchCounters: async () => {
        counterRead += 1;
        return counterRead === 1 ? counters() : counters({ browser_outbound_attempts: 1 });
      },
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /post-browser counters contain activity/,
  );
  assert.equal(fs.existsSync(fixture.paths.result), false);
  assert.equal(fs.existsSync(fixture.paths.report), false);

  const denied = runtime(t);
  let deniedCounterRead = 0;
  await assert.rejects(
    runBrowserCheckpoint({
      env: denied.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => {
        deniedCounterRead += 1;
        return deniedCounterRead === 1
          ? counters()
          : counters({ denied_requests: 1, provider_guild_lookups: 1, repository_reads: 1 });
      },
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /post-browser counters contain activity/,
  );
  assert.equal(fs.existsSync(denied.paths.result), false);
  assert.equal(fs.existsSync(denied.paths.report), false);

  const failed = runtime(t);
  await assert.rejects(
    runBrowserCheckpoint({
      env: failed.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async () => ({ exitCode: 1, stdout: '', stderr: '' }),
      stdout: () => {},
    }),
    /declared Pass outcome/,
  );
  assert.equal(fs.existsSync(failed.paths.result), false);

  const noProviderEvidence = runtime(t);
  await assert.rejects(
    runBrowserCheckpoint({
      env: noProviderEvidence.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /did not produce monotonic guild provider evidence/,
  );
  assert.equal(fs.existsSync(noProviderEvidence.paths.result), false);

  const noRepositoryEvidence = runtime(t);
  let evidenceRead = 0;
  await assert.rejects(
    runBrowserCheckpoint({
      env: noRepositoryEvidence.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => {
        evidenceRead += 1;
        return counters(evidenceRead === 1 ? {} : { provider_guild_lookups: 1 });
      },
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /did not produce monotonic repository read evidence/,
  );
  assert.equal(fs.existsSync(noRepositoryEvidence.paths.result), false);

  const extraArtifact = runtime(t);
  await assert.rejects(
    runBrowserCheckpoint({
      env: extraArtifact.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        fs.writeFileSync(path.join(context.outputDir, 'failed-cell.png'), 'not retained', { flag: 'wx' });
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /output inventory contains unexpected artifacts/,
  );
  assert.equal(fs.existsSync(extraArtifact.paths.result), false);
  assert.equal(fs.existsSync(extraArtifact.paths.report), false);

  const forgedLastRun = runtime(t);
  await assert.rejects(
    runBrowserCheckpoint({
      env: forgedLastRun.env,
      now: () => NOW,
      fetchInstance: async () => instanceSnapshot(),
      fetchCounters: async () => counters(),
      spawnPlaywright: async (context) => {
        writePlaywrightArtifacts(context, report());
        fs.writeFileSync(path.join(context.outputDir, '.last-run.json'), JSON.stringify({
          status: 'failed',
          failedTests: ['forged'],
        }));
        return { exitCode: 0, stdout: '', stderr: '' };
      },
      stdout: () => {},
    }),
    /last-run marker did not match the declared outcome/,
  );
  assert.equal(fs.existsSync(forgedLastRun.paths.result), false);
  assert.equal(fs.existsSync(forgedLastRun.paths.report), false);
});

test('unexpected failures collapse to a fixed nonsecret message', () => {
  assert.equal(safeErrorMessage(new BrowserContractError('safe contract failure')), 'safe contract failure');
  assert.equal(safeErrorMessage(new Error(`leaked ${COOKIE_VALUE}`)), 'unexpected dashboard browser failure');
});
