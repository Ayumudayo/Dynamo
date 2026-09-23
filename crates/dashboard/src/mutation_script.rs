const DASHBOARD_MUTATION_TRANSPORT: &str = include_str!("../assets/dashboard-mutations.js");

pub(crate) fn dashboard_script() -> String {
    format!(
        "{DASHBOARD_MUTATION_TRANSPORT}\n{}",
        r#"
function setInlineStatus(id, message, kind = 'info') {
  const target = document.getElementById(id);
  if (!target) return;
  target.dataset.kind = kind;
  target.setAttribute('role', kind === 'error' ? 'alert' : 'status');
  target.setAttribute('aria-live', kind === 'error' ? 'assertive' : 'polite');
  target.setAttribute('aria-atomic', 'true');
  target.textContent = message || '';
}

function setUnknownStatus(id, message) {
  setInlineStatus(id, message, 'error');
  const target = document.getElementById(id);
  if (!target) return;
  const reload = document.createElement('button');
  reload.type = 'button';
  reload.className = 'button button-secondary button-compact';
  reload.textContent = 'Reload';
  reload.addEventListener('click', () => window.location.reload());
  target.append(' ', reload);
}

function snapshotForm(form) {
  return Array.from(form.elements).map((element) => ({
    element,
    disabled: element.disabled,
    checked: element.type === 'checkbox' ? element.checked : undefined,
    value: element.type === 'checkbox' ? undefined : element.value,
  }));
}

function restoreForm(snapshot) {
  for (const item of snapshot) {
    if (item.checked !== undefined) item.element.checked = item.checked;
    if (item.value !== undefined) item.element.value = item.value;
  }
}

async function runFormMutation(form, request, statusId, reconcile) {
  if (form.dataset.dynamoMutationPending === 'true') return { kind: 'pending' };
  form.dataset.dynamoMutationPending = 'true';
  const snapshot = snapshotForm(form);
  for (const item of snapshot) item.element.disabled = true;
  try {
    const outcome = await request();
    if (outcome.kind === 'success') return outcome;
    if (outcome.kind === 'definite-failure') {
      setInlineStatus(statusId, `Error: ${outcome.message}`, 'error');
      return outcome;
    }
    setUnknownStatus(statusId, outcome.message);
    const refreshed = await reconcile();
    if (refreshed.kind === 'success') {
      setInlineStatus(statusId, 'Current values were reloaded after an uncertain save.', 'info');
    } else {
      restoreForm(snapshot);
      setUnknownStatus(statusId, 'The save outcome is unknown and current values could not be reloaded.');
    }
    return outcome;
  } finally {
    for (const item of snapshot) item.element.disabled = item.disabled;
    delete form.dataset.dynamoMutationPending;
  }
}

async function reconcileSettings(url, apply) {
  const outcome = await DynamoMutationTransport.request(fetch, url, { method: 'GET' });
  if (outcome.kind === 'success') apply(outcome.body);
  return outcome;
}

function setToggleConfirmed(input, value) {
  if (!input) return;
  input.checked = value;
  input.defaultChecked = value;
  input.dataset.dynamoConfirmed = String(value);
}

function toggleConfirmedValue(input) {
  if (!input) return false;
  if (input.dataset.dynamoConfirmed === undefined) input.dataset.dynamoConfirmed = String(input.defaultChecked);
  return input.dataset.dynamoConfirmed === 'true';
}

async function runToggleMutation(input, request, statusId) {
  if (!input || input.dataset.dynamoMutationPending === 'true') return false;
  const confirmed = toggleConfirmedValue(input);
  input.dataset.dynamoMutationPending = 'true';
  input.disabled = true;
  try {
    const outcome = await request();
    if (outcome.kind === 'success') {
      setToggleConfirmed(input, input.checked);
      setInlineStatus(statusId, 'Saved', 'success');
    } else if (outcome.kind === 'definite-failure') {
      setToggleConfirmed(input, confirmed);
      setInlineStatus(statusId, `Error: ${outcome.message}`, 'error');
    } else {
      setToggleConfirmed(input, confirmed);
      setUnknownStatus(statusId, outcome.message);
    }
  } finally {
    input.disabled = false;
    delete input.dataset.dynamoMutationPending;
  }
  return false;
}

const MODAL_FOCUSABLE_SELECTOR = 'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
const settingsModalState = {
  activeModalId: null,
  returnFocus: null,
};

function activeSettingsModal() {
  if (!settingsModalState.activeModalId) return null;
  return document.getElementById(settingsModalState.activeModalId);
}

function anyOpenSettingsModal() {
  return document.querySelector('.settings-modal-overlay:not([hidden])');
}

function findModalDialog(modal) {
  return modal ? modal.querySelector('[data-modal-root]') : null;
}

function isFocusableVisible(element) {
  return !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
}

function modalFocusableElements(modal) {
  const dialog = findModalDialog(modal);
  if (!dialog) return [];
  return Array.from(dialog.querySelectorAll(MODAL_FOCUSABLE_SELECTOR)).filter((element) => isFocusableVisible(element));
}

function focusFirstModalControl(modal) {
  const dialog = findModalDialog(modal);
  if (!dialog) return;
  const focusable = modalFocusableElements(modal);
  const preferred = focusable.find((element) => element.matches('input, select, textarea, button:not(.modal-close)'));
  (preferred || focusable[0] || dialog).focus();
}

function openSettingsModal(modalId, trigger) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  const openModal = anyOpenSettingsModal();
  if (openModal && openModal !== modal) {
    openModal.hidden = true;
  }
  settingsModalState.activeModalId = modalId;
  settingsModalState.returnFocus = trigger instanceof HTMLElement
    ? trigger
    : document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
  modal.hidden = false;
  document.body.style.overflow = 'hidden';
  window.requestAnimationFrame(() => focusFirstModalControl(modal));
  return false;
}

function closeSettingsModal(modalId) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  const wasActive = settingsModalState.activeModalId === modalId;
  modal.hidden = true;
  if (!anyOpenSettingsModal()) {
    document.body.style.overflow = '';
  }
  if (wasActive) {
    settingsModalState.activeModalId = null;
    const returnFocus = settingsModalState.returnFocus;
    settingsModalState.returnFocus = null;
    if (returnFocus && typeof returnFocus.focus === 'function') {
      returnFocus.focus();
    }
  }
  return false;
}

function dismissSettingsModal(event, modalId) {
  if (event.target && event.target.id === modalId) {
    closeSettingsModal(modalId);
  }
  return false;
}

document.addEventListener('keydown', (event) => {
  const openModal = activeSettingsModal() || document.querySelector('.settings-modal-overlay:not([hidden])');
  if (!openModal) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    closeSettingsModal(openModal.id);
    return;
  }
  if (event.key !== 'Tab') return;

  const dialog = findModalDialog(openModal);
  if (!dialog) return;
  const focusable = modalFocusableElements(openModal);
  if (!focusable.length) {
    event.preventDefault();
    dialog.focus();
    return;
  }

  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
});

async function toggleDeploymentModule(moduleId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/deployment-settings/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-module-${statusKey(moduleId)}`);
}

async function patchDeploymentModule(event, moduleId) {
  event.preventDefault();
  const form = event.target;
  const body = {
    installed: form.installed.checked,
    enabled: form.enabled.checked,
  };
  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/deployment-settings/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  }), `deployment-status-${moduleId}`, () => reconcileSettings('/api/deployment-settings', (settings) => {
    const current = settings.modules?.[moduleId];
    if (current) { form.installed.checked = !!current.installed; form.enabled.checked = !!current.enabled; }
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`deployment-status-${moduleId}`, 'Saved', 'success');
    closeSettingsModal(`modal-deployment-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleDeploymentCommand(commandId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-command-${statusKey(commandId)}`);
}

async function patchDeploymentCommand(event, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`deployment-command-status-${statusKey(commandId)}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      installed: form.installed.checked,
      enabled: form.enabled.checked,
      configuration,
    }),
  }), `deployment-command-status-${statusKey(commandId)}`, () => reconcileSettings('/api/deployment-settings', (settings) => {
    const current = settings.commands?.[commandId];
    if (current) { form.installed.checked = !!current.installed; form.enabled.checked = !!current.enabled; }
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`deployment-command-status-${statusKey(commandId)}`, 'Saved', 'success');
    closeSettingsModal(`modal-deployment-command-${statusKey(commandId)}`);
  }
  return false;
}

function collectConfiguration(form) {
  const config = {};
  const fields = form.querySelectorAll('[data-setting-key]');
  for (const field of fields) {
    const key = field.dataset.settingKey;
    if (!key || key === '__empty') continue;

    const kind = field.dataset.settingKind;
    if (kind === 'toggle') {
      setPath(config, key, !!field.checked);
      continue;
    }

    const raw = (field.value ?? '').trim();
    if (raw === '') continue;

    if (kind === 'integer') {
      const parsed = Number.parseInt(raw, 10);
      if (Number.isNaN(parsed)) {
        throw new Error(`Invalid integer for ${key}`);
      }
      setPath(config, key, parsed);
      continue;
    }

    if ((raw.startsWith('[') && raw.endsWith(']')) || (raw.startsWith('{') && raw.endsWith('}'))) {
      setPath(config, key, JSON.parse(raw));
      continue;
    }

    setPath(config, key, raw);
  }

  return config;
}

function setPath(target, key, value) {
  const segments = key.split('.');
  let cursor = target;
  for (let i = 0; i < segments.length - 1; i += 1) {
    const segment = segments[i];
    if (typeof cursor[segment] !== 'object' || cursor[segment] === null || Array.isArray(cursor[segment])) {
      cursor[segment] = {};
    }
    cursor = cursor[segment];
  }
  cursor[segments[segments.length - 1]] = value;
}

function statusKey(value) {
  return value.replaceAll(':', '-');
}

async function patchGuildModule(event, guildId, moduleId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`guild-status-${moduleId}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const body = {
    enabled: form.enabled.checked,
    configuration,
  };
  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  }), `guild-status-${moduleId}`, () => reconcileSettings(`/api/guild-settings/${guildId}`, (settings) => {
    const current = settings.modules?.[moduleId];
    if (current) form.enabled.checked = !!current.enabled;
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`guild-status-${moduleId}`, 'Saved', 'success');
    closeSettingsModal(`modal-guild-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleGuildModule(guildId, moduleId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-module-${statusKey(moduleId)}`);
}

async function patchGuildCommand(event, guildId, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`guild-command-status-${statusKey(commandId)}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      enabled: form.enabled.checked,
      configuration,
    }),
  }), `guild-command-status-${statusKey(commandId)}`, () => reconcileSettings(`/api/guild-settings/${guildId}`, (settings) => {
    const current = settings.commands?.[commandId];
    if (current) form.enabled.checked = !!current.enabled;
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`guild-command-status-${statusKey(commandId)}`, 'Saved', 'success');
    closeSettingsModal(`modal-guild-command-${statusKey(commandId)}`);
  }
  return false;
}

async function toggleGuildCommand(guildId, commandId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-command-${statusKey(commandId)}`);
}

async function requestDeploymentCommandSync(button) {
  if (button?.disabled) return false;
  if (button) button.disabled = true;
  try {
    const outcome = await DynamoMutationTransport.request(fetch, '/api/deployment-command-sync', { method: 'POST' });
    if (outcome.kind === 'success') { setInlineStatus('command-sync-inline-status', 'Sync requested', 'success'); window.location.reload(); }
    else if (outcome.kind === 'definite-failure') setInlineStatus('command-sync-inline-status', `Error: ${outcome.message}`, 'error');
    else { setUnknownStatus('command-sync-inline-status', outcome.message); window.location.reload(); }
  } finally { if (button) button.disabled = false; }
  return false;
}

async function requestGuildCommandSync(guildId, button) {
  if (button?.disabled) return false;
  if (button) button.disabled = true;
  try {
    const outcome = await DynamoMutationTransport.request(fetch, `/api/guild-command-sync/${guildId}`, { method: 'POST' });
    if (outcome.kind === 'success') { setInlineStatus('command-sync-inline-status', 'Sync requested', 'success'); window.location.reload(); }
    else if (outcome.kind === 'definite-failure') setInlineStatus('command-sync-inline-status', `Error: ${outcome.message}`, 'error');
    else { setUnknownStatus('command-sync-inline-status', outcome.message); window.location.reload(); }
  } finally { if (button) button.disabled = false; }
  return false;
}

"#
    )
}
