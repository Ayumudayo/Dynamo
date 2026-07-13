'use strict';

const childProcess = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const {
  BrowserContractError,
  CHROMIUM_REVISION,
  CHROMIUM_VERSION,
  PLAYWRIGHT_VERSION,
  RUNNER_VERSION,
  assertBrowserOutputInventory,
  assertZeroCounters,
  buildChildEnvironment,
  consumeAndValidateReport,
  consumePlaywrightLastRun,
  fetchAndAssertCounters,
  fetchAndAssertInstance,
  loadBrowserRunContext,
  publishBrowserResult,
  safeErrorMessage,
} = require('./dashboard-browser-contract.cjs');

const MAX_CAPTURE_BYTES = 1024 * 1024;
const PLAYWRIGHT_DEADLINE_MS = 13 * 60 * 1000;
const REPOSITORY_ROOT = path.resolve(__dirname, '..', '..');
const PLAYWRIGHT_CONFIG = path.join(REPOSITORY_ROOT, 'playwright.dashboard.isolated.config.cjs');
const PLAYWRIGHT_CLI = path.join(REPOSITORY_ROOT, 'node_modules', 'playwright', 'cli.js');
const PLAYWRIGHT_PACKAGE = path.join(
  REPOSITORY_ROOT,
  'node_modules',
  '@playwright',
  'test',
  'package.json',
);
const PLAYWRIGHT_BROWSERS = path.join(
  REPOSITORY_ROOT,
  'node_modules',
  'playwright-core',
  'browsers.json',
);

function fail(message) {
  throw new BrowserContractError(message);
}

function assertRegularFile(filePath, label) {
  try {
    const metadata = fs.lstatSync(filePath);
    if (!metadata.isFile() || metadata.isSymbolicLink()) fail(`${label} must be a regular file`);
  } catch (error) {
    if (error instanceof BrowserContractError) throw error;
    fail(`${label} is unavailable; run npm ci first`);
  }
}

function assertLocalPlaywright() {
  assertRegularFile(PLAYWRIGHT_CONFIG, 'isolated Playwright config');
  assertRegularFile(PLAYWRIGHT_CLI, 'repository-local Playwright CLI');
  assertRegularFile(PLAYWRIGHT_PACKAGE, 'repository-local Playwright package');
  assertRegularFile(PLAYWRIGHT_BROWSERS, 'repository-local Playwright browser registry');
  let packageJson;
  let browserRegistry;
  try {
    packageJson = JSON.parse(fs.readFileSync(PLAYWRIGHT_PACKAGE, 'utf8'));
    browserRegistry = JSON.parse(fs.readFileSync(PLAYWRIGHT_BROWSERS, 'utf8'));
  } catch (_error) {
    fail('repository-local Playwright package metadata is invalid');
  }
  if (packageJson.version !== PLAYWRIGHT_VERSION) {
    fail('repository-local Playwright version mismatch');
  }
  if (!browserRegistry
      || !Array.isArray(browserRegistry.browsers)
      || ['chromium', 'chromium-headless-shell'].some((name) => {
        const browser = browserRegistry.browsers.find((entry) => entry && entry.name === name);
        return !browser
          || browser.revision !== CHROMIUM_REVISION
          || browser.browserVersion !== CHROMIUM_VERSION;
      })) {
    fail('repository-local Chromium revision or version mismatch');
  }
  return Object.freeze({
    playwright_version: PLAYWRIGHT_VERSION,
    browser_name: 'chromium',
    browser_revision: CHROMIUM_REVISION,
    browser_version: CHROMIUM_VERSION,
  });
}

function buildPlaywrightSpawnSpec(env) {
  return Object.freeze({
    executable: process.execPath,
    args: Object.freeze([PLAYWRIGHT_CLI, 'test', '--config', PLAYWRIGHT_CONFIG]),
    options: Object.freeze({
      cwd: REPOSITORY_ROOT,
      env: Object.freeze(buildChildEnvironment(env)),
      shell: false,
      windowsHide: true,
      stdio: Object.freeze(['ignore', 'pipe', 'pipe']),
    }),
  });
}

function spawnPlaywrightProcess(context, env) {
  assertLocalPlaywright();
  return new Promise((resolve, reject) => {
    const spawnSpec = buildPlaywrightSpawnSpec(env);
    const child = childProcess.spawn(
      spawnSpec.executable,
      spawnSpec.args,
      spawnSpec.options,
    );
    let stdout = '';
    let stderr = '';
    let settled = false;
    let captureExceeded = false;
    let deadline;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      if (deadline) clearTimeout(deadline);
      if (error) reject(error);
      else resolve(value);
    };
    const capture = (key, chunk) => {
      const next = key === 'stdout' ? stdout + chunk : stderr + chunk;
      if (Buffer.byteLength(next) > MAX_CAPTURE_BYTES) {
        captureExceeded = true;
        child.kill();
        return;
      }
      if (key === 'stdout') stdout = next;
      else stderr = next;
    };
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => capture('stdout', chunk));
    child.stderr.on('data', (chunk) => capture('stderr', chunk));
    child.on('error', () => finish(new BrowserContractError('Playwright process could not be started')));
    child.on('close', (code, signal) => {
      if (captureExceeded) {
        finish(new BrowserContractError('Playwright output exceeded the capture limit'));
        return;
      }
      if (signal !== null) {
        finish(new BrowserContractError('Playwright process terminated unexpectedly'));
        return;
      }
      finish(null, { exitCode: code, stdout, stderr });
    });
    // The outer PowerShell launcher owns the process Job and is authoritative
    // for descendant cleanup. This deadline only requests direct child
    // termination so a wedged CLI cannot keep the Node runner pending forever.
    deadline = setTimeout(() => {
      child.kill();
      finish(new BrowserContractError('Playwright suite exceeded its hard deadline'));
    }, PLAYWRIGHT_DEADLINE_MS);
  });
}

function assertInstanceCounterCoherence(instance, counters, phase) {
  if (instance.outbound_calls !== counters.outbound_calls
      || instance.browser_outbound_attempts !== counters.browser_outbound_attempts) {
    fail(`${phase} instance and counter snapshots disagree`);
  }
}

function assertWorkloadReadEvidence(before, after) {
  if (after.provider_guild_lookups <= before.provider_guild_lookups) {
    fail('browser workload did not produce monotonic guild provider evidence');
  }
  if (after.repository_reads <= before.repository_reads) {
    fail('browser workload did not produce monotonic repository read evidence');
  }
}

function assertCapturedOutputSafe(context, execution) {
  if (!execution || typeof execution !== 'object') {
    fail('Playwright process did not return an execution record');
  }
  const output = `${execution.stdout || ''}${execution.stderr || ''}`;
  for (const secret of [
    context.storageState.cookies[0].value,
    context.storageStatePath,
    context.handoffPath,
    context.proofPath,
  ]) {
    if (output.includes(secret)) fail('Playwright child output contains private runner material');
  }
}

function assertExpectedPlaywrightExit(context, execution) {
  if (!execution || !Number.isSafeInteger(execution.exitCode)) {
    fail('Playwright process did not return a valid exit code');
  }
  const expectedExitCode = context.proof.expected_outcome === 'Red' ? 1 : 0;
  if (execution.exitCode !== expectedExitCode) {
    fail(`Playwright exit code did not match declared ${context.proof.expected_outcome} outcome`);
  }
}

function buildResult(context, before, after, report) {
  return {
    schema_version: 1,
    runner_version: RUNNER_VERSION,
    source_state: { ...context.handoff.source_state },
    fixture: { ...context.handoff.fixture },
    environment: { ...context.handoff.environment },
    browser: { ...report.uiProof.browser_identity },
    instance: {
      revision: context.handoff.revision,
      nonce: context.handoff.nonce,
      pid: context.handoff.pid,
      fixture_mode: context.handoff.fixture_mode,
      denied_requests_before: before.denied_requests,
      denied_requests_after: after.denied_requests,
      denied_requests_delta: after.denied_requests - before.denied_requests,
      outbound_calls_before: before.outbound_calls,
      outbound_calls_after: after.outbound_calls,
      browser_outbound_attempts_before: before.browser_outbound_attempts,
      browser_outbound_attempts_after: after.browser_outbound_attempts,
      server_write_attempts_before: before.server_write_attempts,
      server_write_attempts_after: after.server_write_attempts,
      repository_mutations_before: before.repository_mutations,
      repository_mutations_after: after.repository_mutations,
      provider_guild_lookups_before: before.provider_guild_lookups,
      provider_guild_lookups_after: after.provider_guild_lookups,
      provider_guild_lookups_delta:
        after.provider_guild_lookups - before.provider_guild_lookups,
      repository_reads_before: before.repository_reads,
      repository_reads_after: after.repository_reads,
      repository_reads_delta: after.repository_reads - before.repository_reads,
    },
    selection: {
      suite_id: context.selection.suite_id,
      selection_sha256: context.selectionSha256,
      specs: [...context.selection.specs],
      projects: context.selection.projects.map((project) => project.name),
      test_ids: [...context.selection.test_ids],
      expected_count: context.selection.expected_count,
    },
    stats: { ...report.stats },
    expected_outcome: report.expectedOutcome,
    expected_failure_id: report.expectedFailureId,
    failures: report.failures.map((failure) => ({
      ...failure,
      error: { ...failure.error },
    })),
    ui_proof: report.uiProof,
    report: { sha256: report.sha256, bytes: report.bytes },
    exit_classification: report.expectedOutcome,
  };
}

async function runBrowserCheckpoint(options = {}) {
  const env = options.env || process.env;
  const now = options.now || Date.now;
  const stdout = options.stdout || ((value) => process.stdout.write(value));
  const fetchInstance = options.fetchInstance || fetchAndAssertInstance;
  const fetchCounters = options.fetchCounters || fetchAndAssertCounters;
  const spawnPlaywright = options.spawnPlaywright || spawnPlaywrightProcess;

  const context = loadBrowserRunContext(env, now());
  const beforeInstance = await fetchInstance(context.handoff, 'pre-browser');
  const beforeCounters = await fetchCounters(context.handoff, 'pre-browser');
  assertInstanceCounterCoherence(beforeInstance, beforeCounters, 'pre-browser');
  assertZeroCounters(beforeCounters, 'pre-browser');
  if (now() > context.handoff.expires_at_unix_ms) fail('instance handoff expired before Playwright');

  const execution = await spawnPlaywright(context, env);
  assertCapturedOutputSafe(context, execution);
  assertExpectedPlaywrightExit(context, execution);
  if (now() > context.handoff.expires_at_unix_ms) fail('instance handoff expired during Playwright');
  const report = consumeAndValidateReport(context);
  const lastRun = consumePlaywrightLastRun(context);
  assertBrowserOutputInventory(context, []);
  if (lastRun.failedTestCount !== report.stats.unexpected) {
    fail('Playwright last-run marker did not match report failures');
  }

  const afterInstance = await fetchInstance(context.handoff, 'post-browser');
  const afterCounters = await fetchCounters(context.handoff, 'post-browser');
  assertInstanceCounterCoherence(afterInstance, afterCounters, 'post-browser');
  assertZeroCounters(afterCounters, 'post-browser');
  assertWorkloadReadEvidence(beforeCounters, afterCounters);
  if (now() > context.handoff.expires_at_unix_ms) fail('instance handoff expired after Playwright');

  const result = buildResult(context, beforeCounters, afterCounters, report);
  publishBrowserResult(context, result);
  assertBrowserOutputInventory(context, ['browser-result.json']);
  const output = {
    schema_version: 1,
    result_path: context.resultPath,
    exit_code: 0,
  };
  stdout(`${JSON.stringify(output)}\n`);
  return { exitCode: 0, context, result };
}

if (require.main === module) {
  runBrowserCheckpoint()
    .then(() => {
      process.exitCode = 0;
    })
    .catch((error) => {
      process.stderr.write(`dashboard-browser failed: ${safeErrorMessage(error)}\n`);
      process.exitCode = 2;
    });
}

module.exports = {
  assertLocalPlaywright,
  assertExpectedPlaywrightExit,
  buildPlaywrightSpawnSpec,
  buildResult,
  runBrowserCheckpoint,
  spawnPlaywrightProcess,
};
