'use strict';

// Validate the launcher-issued capability before importing Playwright. The
// browser-artifacts leaf must still be absent here, so Playwright's clear-output
// task can only observe ENOENT before it creates worker-owned artifacts.
const path = require('node:path');
const {
  loadBrowserRunContext,
  playwrightMaxFailures,
} = require('./scripts/perf/dashboard-browser-contract.cjs');

const browserRun = loadBrowserRunContext(process.env);
const { defineConfig } = require('@playwright/test');

module.exports = defineConfig({
  testDir: path.resolve(__dirname, 'tests', 'playwright'),
  testMatch: browserRun.selection.specs.map((value) => path.basename(value)),
  timeout: 45_000,
  globalTimeout: 12 * 60_000,
  expect: {
    timeout: 8_000,
  },
  fullyParallel: false,
  forbidOnly: true,
  failOnFlakyTests: true,
  retries: 0,
  repeatEach: 1,
  workers: 1,
  // A declared Red run must execute the full 3 x 4 matrix so an expected
  // assertion cannot conceal skipped cells. Playwright uses zero for no cap.
  maxFailures: playwrightMaxFailures(browserRun.proof.expected_outcome),
  quiet: true,
  preserveOutput: 'never',
  reporter: [['json', { outputFile: browserRun.reportPath }]],
  outputDir: browserRun.outputDir,
  use: {
    headless: true,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    launchOptions: {
      args: [
        '--disable-background-networking',
        '--disable-component-update',
        '--disable-default-apps',
        '--disable-domain-reliability',
        '--disable-sync',
        '--metrics-recording-only',
        '--no-first-run',
        '--safebrowsing-disable-auto-update',
      ],
    },
  },
  projects: browserRun.selection.projects.map((project) => ({
    name: project.name,
    use: { viewport: { ...project.viewport } },
  })),
});
