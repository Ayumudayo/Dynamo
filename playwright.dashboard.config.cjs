const path = require('path');

const storageState = process.env.PLAYWRIGHT_STORAGE_STATE
  ? path.resolve(process.cwd(), process.env.PLAYWRIGHT_STORAGE_STATE)
  : undefined;

/** @type {import('@playwright/test').PlaywrightTestConfig} */
module.exports = {
  testDir: path.resolve(__dirname, 'tests/playwright'),
  timeout: 30_000,
  expect: {
    timeout: 8_000,
  },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: 'line',
  outputDir: path.resolve(__dirname, 'output/playwright'),
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL || 'http://127.0.0.1:3000',
    headless: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
    storageState,
  },
  projects: [
    {
      name: 'desktop',
      use: { viewport: { width: 1440, height: 1100 } },
    },
    {
      name: 'tablet',
      use: { viewport: { width: 1024, height: 900 } },
    },
    {
      name: 'small-tablet',
      use: { viewport: { width: 768, height: 1024 } },
    },
    {
      name: 'mobile-sanity',
      use: { viewport: { width: 375, height: 812 } },
    },
  ],
};
