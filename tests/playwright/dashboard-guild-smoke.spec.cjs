const fs = require('fs');
const path = require('path');
const { test, expect } = require('@playwright/test');

const guildId = process.env.PLAYWRIGHT_GUILD_ID;
const storageStatePath = process.env.PLAYWRIGHT_STORAGE_STATE
  ? path.resolve(process.cwd(), process.env.PLAYWRIGHT_STORAGE_STATE)
  : null;

function requireDashboardSession() {
  return !guildId || !storageStatePath || !fs.existsSync(storageStatePath);
}

async function openGuildPage(page) {
  await page.goto(`/guild/${guildId}`, { waitUntil: 'networkidle' });
  await expect(page.getByTestId('dashboard-shell')).toBeVisible();
  await expect(page.getByTestId('page-tab-overview')).toBeVisible();
  await expect(page.getByTestId('page-tab-modules')).toBeVisible();
  await expect(page.getByTestId('page-tab-commands')).toBeVisible();
  await expect(page.getByTestId('page-tab-logs')).toBeVisible();
  await expect(page.getByTestId('guild-runtime-summary')).toBeVisible();
}

async function expectNoHorizontalOverflow(page) {
  const overflow = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    viewportWidth: window.innerWidth,
  }));

  expect(
    overflow.documentWidth,
    `document overflowed horizontally: ${overflow.documentWidth}px > ${overflow.viewportWidth}px`
  ).toBeLessThanOrEqual(overflow.viewportWidth + 1);
}

async function expectTabWrapsInsideDialog(page, dialog) {
  await dialog.evaluate((root) => {
    const selector =
      'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    const focusable = Array.from(root.querySelectorAll(selector)).filter(
      (element) => !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length)
    );
    if (focusable.length < 2) {
      throw new Error('Expected at least two focusable elements in the settings dialog');
    }
    focusable[focusable.length - 1].focus();
  });

  await page.keyboard.press('Tab');

  await expect
    .poll(async () =>
      dialog.evaluate((root) => {
        const selector =
          'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
        const focusable = Array.from(root.querySelectorAll(selector)).filter(
          (element) => !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length)
        );
        return document.activeElement === focusable[0];
      })
    )
    .toBe(true);
}

test.describe('dashboard guild smoke', () => {
  test.beforeEach(async () => {
    test.skip(
      requireDashboardSession(),
      'Set PLAYWRIGHT_GUILD_ID and PLAYWRIGHT_STORAGE_STATE after completing manual OAuth login.'
    );
  });

  test('guild page renders and filters module/command cards', async ({ page }, testInfo) => {
    await openGuildPage(page);

    await page.getByTestId('page-tab-modules').click();
    await expect(page.getByTestId('guild-modules-section')).toBeVisible();
    const moduleFilter = page.getByTestId('module-filter');
    await moduleFilter.fill('stock');
    await expect(page.getByTestId('module-card-stock')).toBeVisible();
    await moduleFilter.fill('');

    await page.getByTestId('page-tab-commands').click();
    await expect(page.getByTestId('guild-commands-section')).toBeVisible();
    const stocksTab = page.getByTestId('command-tab-stocks');
    if (await stocksTab.count()) {
      await stocksTab.click();
    }

    const commandFilter = page.getByTestId('command-filter');
    await commandFilter.fill('etf');
    await expect(page.getByTestId('command-card-etf')).toBeVisible();

    await page.getByTestId('page-tab-modules').click();
    const moduleSettingsButton = page.getByTestId('module-settings-button-stock');
    await moduleSettingsButton.click();
    const moduleModal = page.getByTestId(/settings-modal-modal-guild-module-stock/);
    await expect(moduleModal).toBeVisible();

    if (testInfo.project.name === 'mobile-sanity') {
      const moduleDialog = moduleModal.locator('[data-modal-root]');
      await expect(moduleDialog).toHaveAttribute('role', 'dialog');
      await expect(moduleDialog).toHaveAttribute('aria-modal', 'true');
      await expect(moduleModal.locator('input[name="enabled"]').first()).toBeFocused();

      await expectTabWrapsInsideDialog(page, moduleDialog);
      await page.keyboard.press('Escape');
      await expect(moduleModal).toBeHidden();
      await expect(moduleSettingsButton).toBeFocused();

      await page.getByTestId('page-tab-logs').click();
      await expect(page.getByTestId('logs-section')).toBeVisible();
      await expect(page.getByTestId('logs-mobile-list')).toBeVisible();
      const mobileCards = page.locator('[data-testid^="audit-log-card-"]');
      if (await mobileCards.count()) {
        await expect(mobileCards.first()).toBeVisible();
      } else {
        await expect(page.getByTestId('logs-mobile-empty')).toBeVisible();
      }

      await expectNoHorizontalOverflow(page);
      return;
    }

    await page.getByTestId(/modal-close-modal-guild-module-stock/).click();
    await expect(moduleModal).toBeHidden();
  });

  test('etf command modal saves structured settings and toggle updates inline state', async ({ page }, testInfo) => {
    await openGuildPage(page);

    await page.getByTestId('page-tab-commands').click();
    await expect(page.getByTestId('guild-commands-section')).toBeVisible();
    await page.getByTestId('command-tab-stocks').click();
    await page.getByTestId('command-filter').fill('etf');
    const etfCard = page.getByTestId('command-card-etf');
    await expect(etfCard).toBeVisible();

    const etfToggle = page.getByTestId('command-toggle-etf');
    const initiallyChecked = await etfToggle.isChecked();
    await etfToggle.click();
    await expect(page.locator('#card-status-command-etf')).toHaveText(/Saved|Update failed/);
    await etfToggle.setChecked(initiallyChecked);

    if (testInfo.project.name === 'mobile-sanity') {
      return;
    }

    await page.getByTestId('command-settings-button-etf').click();
    const modal = page.getByTestId(/settings-modal-modal-guild-command-etf/);
    await expect(modal).toBeVisible();

    await page.getByTestId('field-ticker_1').locator('input, textarea').fill('SOXL');
    await page.getByTestId('field-ticker_2').locator('input, textarea').fill('TQQQ');
    await page.getByTestId('save-settings-etf').click();
    await expect(modal).toBeHidden();

    await page.getByTestId('command-settings-button-etf').click();
    const reopened = page.getByTestId(/settings-modal-modal-guild-command-etf/);
    await expect(reopened).toBeVisible();
    await expect(
      page.getByTestId('field-ticker_1').locator('input, textarea')
    ).toHaveValue('SOXL');
    await expect(
      page.getByTestId('field-ticker_2').locator('input, textarea')
    ).toHaveValue('TQQQ');
    await page.getByTestId('cancel-settings-etf').click();

    await page.getByTestId('page-tab-logs').click();
    await expect(page.getByTestId('logs-section')).toBeVisible();
    await page.getByTestId('logs-entity-filter').selectOption('command');
    await page.getByTestId('logs-action-filter').selectOption('save_settings');
    await page.getByRole('button', { name: 'Apply' }).click();
    await expect(page.getByTestId('logs-table')).toBeVisible();
    await expect(page.locator('[data-testid^="audit-log-row-"]').first()).toContainText(/etf/i);
  });
});
