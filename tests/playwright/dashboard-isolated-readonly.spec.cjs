'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const {
  LOCKED_FONT_ROUTES,
  attachUiProof,
  beginColdUiNavigation,
  browserRun,
  buildUiProof,
  captureUiMetrics,
  expect,
  expectTabWrapsInsideDialog,
  readLayoutSnapshot,
  test,
} = require('./helpers/isolated-dashboard.cjs');

const repositoryRoot = path.resolve(__dirname, '..', '..');
const fontLock = JSON.parse(fs.readFileSync(
  path.join(repositoryRoot, 'crates', 'dashboard', 'assets', 'fonts', 'fonts.lock.json'),
  'utf8',
));
const fontsByHash = new Map(fontLock.assets.map((asset) => [asset.sha256, asset]));
const fontsByPath = new Map(LOCKED_FONT_ROUTES.map((asset) => [asset.path, asset]));

function digest(buffer) {
  return crypto.createHash('sha256').update(buffer).digest('hex');
}

async function openGuildPage(page) {
  const navigation = await beginColdUiNavigation(
    page,
    `/guild/${browserRun.guildId}`,
    '[data-testid="guild-runtime-summary"]',
  );
  await expect(page.getByTestId('dashboard-shell')).toBeVisible();
  await expect(page.getByTestId('page-tab-overview')).toBeVisible();
  await expect(page.getByTestId('page-tab-modules')).toBeVisible();
  await expect(page.getByTestId('page-tab-commands')).toBeVisible();
  await expect(page.getByTestId('page-tab-logs')).toBeVisible();
  await expect(page.getByTestId('guild-runtime-summary')).toBeVisible();
  return navigation;
}

test.describe('isolated dashboard read-only baseline', () => {
  test('[public-responsive-reduced-motion] public shell remains keyboard reachable and overflow free', async ({ page, browserIdentity }, testInfo) => {
    const navigation = await beginColdUiNavigation(page, '/', '.hero h1');
    await expect(page.getByTestId('dashboard-shell')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
    await expect(page.locator('.hero h1')).toContainText('Manage Dynamo');
    await page.evaluate(() => {
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    });
    await page.keyboard.press('Tab');
    const focusedTag = await page.evaluate(() => document.activeElement?.tagName || '');
    expect(['A', 'BUTTON', 'INPUT', 'SELECT', 'TEXTAREA']).toContain(focusedTag);

    await page.emulateMedia({ reducedMotion: 'reduce' });
    const motion = await page.locator('.button').first().evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        transitionDuration: style.transitionDuration,
        animationName: style.animationName,
      };
    });
    expect(motion.transitionDuration).toBe('0s');
    expect(motion.animationName).toBe('none');
    const metrics = await captureUiMetrics(page, navigation);
    await attachUiProof(testInfo, buildUiProof(
      testInfo,
      'public-responsive-reduced-motion',
      metrics,
      { browserIdentity },
    ));
  });

  test('[guild-readonly-dialog] guild navigation, filters, dialog focus, and logs stay read only', async ({ page, browserIdentity }, testInfo) => {
    const navigation = await openGuildPage(page);
    const initialMetrics = await captureUiMetrics(page, navigation);
    const layouts = [{
      horizontalOverflowPx: initialMetrics.horizontal_overflow_px,
      domNodes: initialMetrics.dom_nodes,
    }];

    await page.getByTestId('page-tab-modules').click();
    await expect(page.getByTestId('guild-modules-section')).toBeVisible();
    const moduleFilter = page.getByTestId('module-filter');
    await moduleFilter.fill('stock');
    await expect(page.getByTestId('module-card-stock')).toBeVisible();
    await moduleFilter.fill('');
    layouts.push(await readLayoutSnapshot(page));

    await page.getByTestId('page-tab-commands').click();
    await expect(page.getByTestId('guild-commands-section')).toBeVisible();
    const stocksTab = page.getByTestId('command-tab-stocks');
    await expect(stocksTab).toBeVisible();
    await stocksTab.click();
    const commandFilter = page.getByTestId('command-filter');
    await commandFilter.fill('etf');
    await expect(page.getByTestId('command-card-etf')).toBeVisible();
    layouts.push(await readLayoutSnapshot(page));

    await page.getByTestId('page-tab-modules').click();
    const settingsButton = page.getByTestId('module-settings-button-stock');
    await settingsButton.click();
    const modal = page.getByTestId('settings-modal-modal-guild-module-stock');
    const dialog = modal.locator('[data-modal-root]');
    await expect(modal).toBeVisible();
    await expect(dialog).toHaveAttribute('role', 'dialog');
    await expect(dialog).toHaveAttribute('aria-modal', 'true');
    await expect(dialog).toHaveAttribute('aria-labelledby', /modal-title-/);
    const initialControl = modal.locator('input[name="enabled"]').first();
    await expect(initialControl).toBeFocused();
    const initialFocus = await initialControl.evaluate((element) => document.activeElement === element);
    layouts.push(await readLayoutSnapshot(page));
    await expectTabWrapsInsideDialog(page, dialog);
    const tabWrap = true;
    await page.keyboard.press('Escape');
    await expect(modal).toBeHidden();
    const escapeCloses = await modal.evaluate((element) => element.hidden === true);
    await expect(settingsButton).toBeFocused();
    const returnFocus = await settingsButton.evaluate((element) => document.activeElement === element);

    await page.getByTestId('page-tab-logs').click();
    await expect(page.getByTestId('logs-section')).toBeVisible();
    if (testInfo.project.name === 'small-tablet' || testInfo.project.name === 'mobile-sanity') {
      await expect(page.getByTestId('logs-mobile-list')).toBeVisible();
    } else {
      await expect(page.getByTestId('logs-table')).toBeVisible();
    }
    const finalLayout = await readLayoutSnapshot(page);
    layouts.push(finalLayout);
    const metrics = {
      ...initialMetrics,
      horizontal_overflow_px: Math.max(
        finalLayout.horizontalOverflowPx,
        ...layouts.map((layout) => layout.horizontalOverflowPx),
      ),
      dom_nodes: Math.max(...layouts.map((layout) => layout.domNodes)),
    };
    await attachUiProof(testInfo, buildUiProof(testInfo, 'guild-readonly-dialog', metrics, {
      browserIdentity,
      dialogFocus: {
        applicable: true,
        initial_focus: initialFocus,
        tab_wrap: tabWrap,
        escape_closes: escapeCloses,
        return_focus: returnFocus,
      },
    }));
  });

  test('[same-origin-font-proof] rendered Fira faces come only from locked same-origin bytes', async ({ page, browserIdentity }, testInfo) => {
    const pendingFontResponses = [];
    page.on('response', (response) => {
      let target;
      try {
        target = new URL(response.url());
      } catch (_error) {
        return;
      }
      if (!target.pathname.endsWith('.woff2')) return;
      pendingFontResponses.push((async () => {
        const headers = await response.allHeaders();
        const body = await response.body();
        return { target, headers, body, status: response.status() };
      })());
    });

    const navigation = await beginColdUiNavigation(page, '/', '.hero h1');
    await page.evaluate(async () => {
      await Promise.all([
        document.fonts.load("400 16px 'Fira Sans'", 'Dynamo'),
        document.fonts.load("700 16px 'Fira Sans'", 'Dynamo'),
        document.fonts.load("500 16px 'Fira Code'", 'Dynamo'),
        document.fonts.load("700 16px 'Fira Code'", 'Dynamo'),
        document.fonts.ready,
      ]);
    });

    const computedFamilies = await page.evaluate(() => ({
      body: getComputedStyle(document.body).fontFamily,
      heading: getComputedStyle(document.querySelector('.hero h1')).fontFamily,
      sans400: document.fonts.check("400 16px 'Fira Sans'", 'Dynamo'),
      sans700: document.fonts.check("700 16px 'Fira Sans'", 'Dynamo'),
      code500: document.fonts.check("500 16px 'Fira Code'", 'Dynamo'),
      code700: document.fonts.check("700 16px 'Fira Code'", 'Dynamo'),
    }));
    expect(computedFamilies.body).toContain('Fira Sans');
    expect(computedFamilies.heading).toContain('Fira Code');
    expect(computedFamilies.sans400).toBe(true);
    expect(computedFamilies.sans700).toBe(true);
    expect(computedFamilies.code500).toBe(true);
    expect(computedFamilies.code700).toBe(true);

    await expect.poll(() => pendingFontResponses.length).toBeGreaterThanOrEqual(3);
    const responses = await Promise.all([...pendingFontResponses]);
    const observedHashes = new Set();
    for (const response of responses) {
      expect(response.target.origin).toBe(browserRun.baseUrl);
      expect(response.status).toBe(200);
      const route = fontsByPath.get(response.target.pathname);
      expect(route, `font route was not in the exact browser allowlist: ${response.target.pathname}`).toBeTruthy();
      const hash = route.sha256;
      const asset = fontsByHash.get(hash);
      expect(asset, `font route hash was not present in fonts.lock.json: ${hash}`).toBeTruthy();
      expect(digest(response.body)).toBe(hash);
      expect(response.body.length).toBe(asset.bytes);
      expect(response.headers['content-type']).toBe('font/woff2');
      expect(response.headers['cache-control']).toBe('public, max-age=31536000, immutable');
      expect(response.headers.etag).toBe(`"${hash}"`);
      observedHashes.add(hash);
    }

    for (const file of ['FiraSans-Regular.woff2', 'FiraSans-Bold.woff2', 'FiraCode-Variable.woff2']) {
      const asset = fontLock.assets.find((value) => value.file === file);
      expect(observedHashes.has(asset.sha256), `${file} was not loaded by the browser`).toBe(true);
    }
    const metrics = await captureUiMetrics(page, navigation);
    await attachUiProof(testInfo, buildUiProof(testInfo, 'same-origin-font-proof', metrics, {
      browserIdentity,
      lockedFontHashes: [...observedHashes],
    }));
  });
});
