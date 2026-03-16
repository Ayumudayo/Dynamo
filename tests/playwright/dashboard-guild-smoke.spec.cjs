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

    if (testInfo.project.name === 'mobile-sanity') {
      return;
    }

    await page.getByTestId('page-tab-modules').click();
    await page.getByTestId('module-settings-button-stock').click();
    await expect(page.getByTestId(/settings-modal-modal-guild-module-stock/)).toBeVisible();
    await page.getByTestId(/modal-close-modal-guild-module-stock/).click();
    await expect(page.getByTestId(/settings-modal-modal-guild-module-stock/)).toBeHidden();
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
