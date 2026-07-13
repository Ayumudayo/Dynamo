'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const repositoryRoot = path.resolve(__dirname, '..', '..');
const packageJsonPath = path.join(repositoryRoot, 'package.json');
const guardPath = path.join(
  repositoryRoot,
  'scripts',
  'perf',
  'refuse-unsafe-dashboard-smoke.cjs'
);
const configPath = path.join(repositoryRoot, 'playwright.dashboard.config.cjs');
const guildSpecPath = path.join(
  repositoryRoot,
  'tests',
  'playwright',
  'dashboard-guild-smoke.spec.cjs'
);

const guardCommand = 'node scripts/perf/refuse-unsafe-dashboard-smoke.cjs';
const refusalMessage =
  'REFUSED: dashboard smoke requires the isolated read-only runner; unsafe direct execution is disabled.\n';

function hostileEnvironment() {
  return {
    ...process.env,
    PLAYWRIGHT_BASE_URL: 'http://dashboard-secret.invalid',
    PLAYWRIGHT_GUILD_ID: 'guild-secret-marker',
    PLAYWRIGHT_STORAGE_STATE: 'storage-secret-marker.json',
  };
}

function runNode(args) {
  return spawnSync(process.execPath, args, {
    cwd: repositoryRoot,
    env: hostileEnvironment(),
    encoding: 'utf8',
    timeout: 5_000,
    windowsHide: true,
  });
}

function assertFixedRefusal(result) {
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 2);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, refusalMessage);

  const output = `${result.stdout}${result.stderr}`;
  assert.doesNotMatch(output, /dashboard-secret|guild-secret|storage-secret/);
}

test('all dashboard smoke aliases fail closed through the fixed guard', () => {
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));

  for (const alias of [
    'dashboard:smoke',
    'dashboard:smoke:auth',
    'dashboard:smoke:headed',
  ]) {
    assert.equal(packageJson.scripts[alias], guardCommand);
  }

  assert.equal(packageJson.scripts['dashboard:smoke:install'], 'playwright install chromium');
  assertFixedRefusal(runNode([guardPath]));
});

test('loading the Playwright config directly cannot bypass the guard', () => {
  assertFixedRefusal(runNode(['-e', `require(${JSON.stringify(configPath)})`]));
});

test('guild smoke keeps only the read-only accessibility flow', () => {
  const source = fs.readFileSync(guildSpecPath, 'utf8');

  assert.match(source, /guild page renders and filters module\/command cards/);
  assert.match(source, /toHaveAttribute\('role', 'dialog'\)/);
  assert.match(source, /toHaveAttribute\('aria-modal', 'true'\)/);
  assert.match(source, /expectTabWrapsInsideDialog/);
  assert.match(source, /keyboard\.press\('Escape'\)/);

  for (const destructivePattern of [
    /command-toggle-/,
    /save-settings-/,
    /\.setChecked\s*\(/,
    /\.check\s*\(/,
    /\.uncheck\s*\(/,
    /SOXL/,
    /TQQQ/,
    /save_settings/,
    /Update failed/,
    /Saved\|/,
  ]) {
    assert.doesNotMatch(source, destructivePattern);
  }
});
