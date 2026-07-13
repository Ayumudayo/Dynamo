'use strict';

const {
  BrowserContractError,
  CHROMIUM_REVISION,
  CHROMIUM_VERSION,
  LOCKED_FONT_ROUTES,
  UI_PROOF_ATTACHMENT,
  UI_PROOF_BUDGETS,
  isAllowedBrowserReadUrl,
  isSameHarnessWebSocketOrigin,
  loadBrowserWorkerContext,
  recordBrowserOutboundAttempt,
} = require('../../../scripts/perf/dashboard-browser-contract.cjs');

// Keep the capability check ahead of the Playwright import for direct-entry
// fail-closed behavior in workers as well as in the config process.
const browserRun = loadBrowserWorkerContext(process.env);
const { test: base, expect } = require('@playwright/test');
const localPlaywrightVersion = require('@playwright/test/package.json').version;
if (localPlaywrightVersion !== browserRun.proof.playwright_version) {
  throw new BrowserContractError('worker Playwright version did not match launch proof');
}

const AUTH_PATHS = new Set(['/login', '/logout', '/auth/discord/callback']);

function safeGuardFailure(state) {
  if (state.guardError) return state.guardError;
  if (state.outboundAttempts !== 0) {
    return new BrowserContractError('browser attempted a non-loopback request');
  }
  if (state.writeAttempts !== 0) {
    return new BrowserContractError('browser attempted a dashboard write');
  }
  if (state.authAttempts !== 0) {
    return new BrowserContractError('browser attempted an authentication route');
  }
  if (state.forbiddenPathAttempts !== 0) {
    return new BrowserContractError('browser attempted a forbidden same-origin route');
  }
  if (state.serviceWorkers !== 0) {
    return new BrowserContractError('browser attempted to create a service worker');
  }
  if (state.pageErrors !== 0) {
    return new BrowserContractError('dashboard page emitted an unhandled error');
  }
  if (state.consoleErrors !== 0) {
    return new BrowserContractError('dashboard page emitted a console error');
  }
  return null;
}

async function blockOutbound(state, route) {
  state.outboundAttempts += 1;
  try {
    await recordBrowserOutboundAttempt(browserRun);
  } catch (error) {
    state.guardError = error instanceof BrowserContractError
      ? error
      : new BrowserContractError('browser outbound counter update failed');
  }
  await route.abort('blockedbyclient');
}

function rounded(value, digits) {
  const scale = 10 ** digits;
  return Math.round(value * scale) / scale;
}

async function beginColdUiNavigation(page, url, firstTextSelector) {
  await page.addInitScript(({ targetSelector }) => {
    const metrics = {
      cls: 0,
      clsSupported: typeof PerformanceObserver === 'function'
        && PerformanceObserver.supportedEntryTypes.includes('layout-shift'),
      fontReadyMs: null,
      targetTextVisibleMs: null,
    };
    window.__dynamoUiProofMetrics = metrics;
    if (metrics.clsSupported) {
      const observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (!entry.hadRecentInput) metrics.cls += entry.value;
        }
      });
      observer.observe({ type: 'layout-shift', buffered: true });
    }
    addEventListener('DOMContentLoaded', () => {
      requestAnimationFrame(async () => {
        await document.fonts.ready;
        metrics.fontReadyMs = performance.now();
      });
    }, { once: true });
    const observer = new MutationObserver(() => {
      if (metrics.targetTextVisibleMs !== null) return;
      const target = document.querySelector(targetSelector);
      if (!target || !target.textContent || target.textContent.trim() === '') return;
      requestAnimationFrame(() => {
        if (metrics.targetTextVisibleMs !== null) return;
        const style = getComputedStyle(target);
        const bounds = target.getBoundingClientRect();
        if (style.visibility !== 'hidden'
            && style.display !== 'none'
            && Number(style.opacity) > 0
            && bounds.width > 0
            && bounds.height > 0) {
          metrics.targetTextVisibleMs = performance.now();
          observer.disconnect();
        }
      });
    });
    observer.observe(document, { childList: true, subtree: true, characterData: true });
  }, { targetSelector: firstTextSelector });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await expect(page.locator(firstTextSelector)).toBeVisible();
  return Object.freeze({ firstTextSelector });
}

async function readLayoutSnapshot(page) {
  return page.evaluate(() => ({
    horizontalOverflowPx: Math.max(
      0,
      document.documentElement.scrollWidth - window.innerWidth,
      document.body ? document.body.scrollWidth - window.innerWidth : 0,
    ),
    domNodes: document.querySelectorAll('*').length,
  }));
}

async function captureUiMetrics(page, navigation) {
  await expect(page.locator(navigation.firstTextSelector)).toBeVisible();
  const browserMetrics = await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const metrics = window.__dynamoUiProofMetrics;
    const firstContentfulPaint = performance.getEntriesByName('first-contentful-paint')[0];
    if (!metrics
        || !metrics.clsSupported
        || metrics.fontReadyMs === null
        || metrics.targetTextVisibleMs === null
        || !firstContentfulPaint) {
      throw new Error('Required browser performance observers were unavailable');
    }
    const heading = document.querySelector('.hero h1, .hero h2, h1, h2');
    if (!heading) throw new Error('A rendered heading is required for computed font proof');
    return {
      fontReadyMs: metrics.fontReadyMs,
      firstTextVisibleMs: Math.max(firstContentfulPaint.startTime, metrics.targetTextVisibleMs),
      cls: metrics.cls,
      computedFonts: {
        body: getComputedStyle(document.body).fontFamily,
        heading: getComputedStyle(heading).fontFamily,
      },
    };
  });
  const layout = await readLayoutSnapshot(page);
  return Object.freeze({
    first_text_visible_ms: rounded(browserMetrics.firstTextVisibleMs, 3),
    font_ready_ms: rounded(browserMetrics.fontReadyMs, 3),
    cls: rounded(browserMetrics.cls, 6),
    horizontal_overflow_px: layout.horizontalOverflowPx,
    dom_nodes: layout.domNodes,
    computed_fonts: Object.freeze({ ...browserMetrics.computedFonts }),
  });
}

function buildUiProof(testInfo, testId, metrics, options = {}) {
  const dialogApplicable = testId === 'guild-readonly-dialog';
  const dialogFocus = options.dialogFocus || {
    applicable: false,
    initial_focus: null,
    tab_wrap: null,
    escape_closes: null,
    return_focus: null,
  };
  if (dialogApplicable !== dialogFocus.applicable) {
    throw new BrowserContractError('dialog proof applicability does not match the UI test');
  }
  if (!options.browserIdentity) {
    throw new BrowserContractError('browser identity is required for UI proof');
  }
  return {
    schema_version: 1,
    test_id: testId,
    project: testInfo.project.name,
    browser_identity: { ...options.browserIdentity },
    first_text_visible_ms: metrics.first_text_visible_ms,
    font_ready_ms: metrics.font_ready_ms,
    cls: metrics.cls,
    horizontal_overflow_px: metrics.horizontal_overflow_px,
    dom_nodes: metrics.dom_nodes,
    computed_fonts: { ...metrics.computed_fonts },
    locked_font_hashes: [...(options.lockedFontHashes || [])].sort(),
    dialog_focus: { ...dialogFocus },
  };
}

async function attachUiProof(testInfo, proof) {
  // Attach measured values before asserting budgets. An intentionally Red
  // run can then publish the real failed-cell evidence instead of fabricating
  // or losing the measurement that caused the assertion.
  await testInfo.attach(UI_PROOF_ATTACHMENT, {
    body: Buffer.from(JSON.stringify(proof), 'utf8'),
    contentType: 'application/json',
  });
  expect(proof.first_text_visible_ms, 'cold first text exceeded its budget')
    .toBeLessThanOrEqual(UI_PROOF_BUDGETS.first_text_visible_ms);
  expect(proof.font_ready_ms, 'font readiness exceeded its budget')
    .toBeLessThanOrEqual(UI_PROOF_BUDGETS.font_ready_ms);
  expect(proof.cls, 'cumulative layout shift exceeded its budget')
    .toBeLessThanOrEqual(UI_PROOF_BUDGETS.cls);
  expect(proof.horizontal_overflow_px, 'horizontal overflow exceeded its budget')
    .toBe(UI_PROOF_BUDGETS.horizontal_overflow_px);
  expect(proof.computed_fonts.body).toContain('Fira Sans');
  expect(proof.computed_fonts.heading).toContain('Fira Code');
  if (proof.dialog_focus.applicable) {
    expect(proof.dialog_focus.initial_focus).toBe(true);
    expect(proof.dialog_focus.tab_wrap).toBe(true);
    expect(proof.dialog_focus.escape_closes).toBe(true);
    expect(proof.dialog_focus.return_focus).toBe(true);
  }
}

const test = base.extend({
  browserIdentity: async ({ browser }, use) => {
    const browserIdentity = Object.freeze({
      playwright_version: localPlaywrightVersion,
      browser_name: browser.browserType().name(),
      browser_revision: CHROMIUM_REVISION,
      browser_version: browser.version(),
    });
    if (browserIdentity.browser_name !== 'chromium'
        || browserIdentity.browser_version !== CHROMIUM_VERSION) {
      throw new BrowserContractError('unexpected browser identity');
    }
    await use(browserIdentity);
  },

  context: async ({ browser }, use, testInfo) => {
    const project = browserRun.selection.projects.find((value) => value.name === testInfo.project.name);
    if (!project) throw new BrowserContractError('Playwright selected an unknown project');
    const context = await browser.newContext({
      baseURL: browserRun.baseUrl,
      storageState: browserRun.storageState,
      viewport: { ...project.viewport },
      locale: 'en-US',
      timezoneId: 'UTC',
      colorScheme: 'dark',
      reducedMotion: 'no-preference',
      deviceScaleFactor: 1,
      serviceWorkers: 'block',
      acceptDownloads: false,
    });
    const state = {
      outboundAttempts: 0,
      writeAttempts: 0,
      authAttempts: 0,
      forbiddenPathAttempts: 0,
      serviceWorkers: 0,
      pageErrors: 0,
      consoleErrors: 0,
      guardError: null,
    };

    await context.exposeBinding('__dynamoRecordServiceWorkerAttempt', () => {
      state.serviceWorkers += 1;
    });
    await context.addInitScript(() => {
      const serviceWorker = navigator.serviceWorker;
      if (!serviceWorker || typeof serviceWorker.register !== 'function') return;
      Object.defineProperty(serviceWorker, 'register', {
        configurable: false,
        enumerable: false,
        writable: false,
        value: async () => {
          await window.__dynamoRecordServiceWorkerAttempt();
          throw new DOMException('Service workers are disabled in the isolated dashboard', 'SecurityError');
        },
      });
    });

    await context.route('**/*', async (route) => {
      const request = route.request();
      let target;
      try {
        target = new URL(request.url());
      } catch (_error) {
        state.forbiddenPathAttempts += 1;
        await route.abort('blockedbyclient');
        return;
      }
      if (target.origin !== browserRun.baseUrl
          || target.username !== ''
          || target.password !== '') {
        await blockOutbound(state, route);
        return;
      }
      if (AUTH_PATHS.has(target.pathname)) {
        state.authAttempts += 1;
        await route.abort('blockedbyclient');
        return;
      }
      if (target.pathname.startsWith('/__perf/')) {
        state.forbiddenPathAttempts += 1;
        await route.abort('blockedbyclient');
        return;
      }
      const method = request.method().toUpperCase();
      if (method !== 'GET' && method !== 'HEAD') {
        state.writeAttempts += 1;
        await route.abort('blockedbyclient');
        return;
      }
      if (!isAllowedBrowserReadUrl(request.url(), browserRun.baseUrl, browserRun.guildId)) {
        state.forbiddenPathAttempts += 1;
        await route.abort('blockedbyclient');
        return;
      }
      await route.continue();
    });

    await context.routeWebSocket('**', async (webSocket) => {
      let external = true;
      try {
        external = !isSameHarnessWebSocketOrigin(webSocket.url(), browserRun.baseUrl);
      } catch (_error) {
        // An unparsable WebSocket URL is forbidden and never connected.
      }
      if (external) {
        state.outboundAttempts += 1;
        try {
          await recordBrowserOutboundAttempt(browserRun);
        } catch (error) {
          state.guardError = error instanceof BrowserContractError
            ? error
            : new BrowserContractError('browser outbound counter update failed');
        }
      } else {
        state.forbiddenPathAttempts += 1;
      }
      await webSocket.close({ code: 1008, reason: 'blocked' });
    });

    context.on('serviceworker', () => {
      state.serviceWorkers += 1;
    });
    context.on('weberror', () => {
      state.pageErrors += 1;
    });
    context.on('console', (message) => {
      if (message.type() === 'error') state.consoleErrors += 1;
    });

    let useError;
    try {
      await use(context);
    } catch (error) {
      useError = error;
    } finally {
      await context.close().catch(() => {
        if (!useError) useError = new BrowserContractError('browser context cleanup failed');
      });
    }
    const guardFailure = safeGuardFailure(state);
    if (guardFailure) throw guardFailure;
    if (useError) throw useError;
  },

  page: async ({ context }, use) => {
    const page = await context.newPage();
    try {
      await use(page);
    } finally {
      await page.close().catch(() => {});
    }
  },
});

async function expectTabWrapsInsideDialog(page, dialog) {
  const selector =
    'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
  await dialog.evaluate((root, focusableSelector) => {
    const focusable = Array.from(root.querySelectorAll(focusableSelector)).filter(
      (element) => !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length),
    );
    if (focusable.length < 2) throw new Error('Expected at least two focusable dialog elements');
    focusable[focusable.length - 1].focus();
  }, selector);
  await page.keyboard.press('Tab');
  await expect.poll(async () => dialog.evaluate((root, focusableSelector) => {
    const focusable = Array.from(root.querySelectorAll(focusableSelector)).filter(
      (element) => !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length),
    );
    return document.activeElement === focusable[0];
  }, selector)).toBe(true);
}

module.exports = {
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
};
