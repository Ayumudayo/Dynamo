/*
 * Dynamo dashboard client.
 *
 * Loaded with `defer` from a same-origin, content-addressed URL. All behaviour
 * is wired through event delegation on `data-action` / `data-*` hooks, so the
 * Content-Security-Policy can forbid inline handlers. Network calls go through
 * DynamoMutationTransport, which classifies every outcome as success,
 * definite failure, or unknown.
 */
(function dynamoDashboard() {
  'use strict';

  const transport = window.DynamoMutationTransport;
  const i18n = readI18n();
  const locale = i18n.__locale || document.documentElement.lang || 'en';
  const SNOWFLAKE = /^\d{17,20}$/;
  const HEX_COLOR = /^#?[0-9a-fA-F]{6}$/;
  const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

  function readI18n() {
    try {
      const node = document.getElementById('dynamo-i18n');
      return node ? JSON.parse(node.textContent || '{}') : {};
    } catch (_) {
      return {};
    }
  }

  function t(key, values = {}) {
    let text = typeof i18n[key] === 'string' ? i18n[key] : key;
    for (const [name, value] of Object.entries(values)) {
      text = text.split(`{${name}}`).join(String(value));
    }
    return text;
  }

  function cssEscape(value) {
    return window.CSS && typeof window.CSS.escape === 'function'
      ? window.CSS.escape(value)
      : String(value).replace(/["\\]/g, '\\$&');
  }

  function isVisible(element) {
    return !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
  }

  // ---------------------------------------------------------------- Network

  function readCsrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.getAttribute('content') || '';
  }

  function request(url, method = 'GET', body) {
    const init = { method, credentials: 'same-origin', headers: { Accept: 'application/json' } };
    if (method !== 'GET' && method !== 'HEAD') {
      const csrf = readCsrfToken();
      if (csrf) init.headers['X-CSRF-Token'] = csrf;
    }
    if (body !== undefined) {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return transport.request(fetch, url, init);
  }

  function failureMessage(outcome) {
    if (outcome.kind === 'definite-failure') {
      const status = outcome.status;
      if (status === 400 || status === 409 || status === 422 || status === 429) {
        return t('http_400', { message: outcome.message });
      }
      if (status === 401) return t('http_401');
      if (status === 403) return outcome.message || t('http_403');
      if (status === 404) return t('http_404');
      if (status >= 500) return t('http_5xx');
      return t('http_other', { status });
    }
    if (outcome.reason === 'timeout') return t('unknown_timeout');
    if (outcome.reason === 'invalid-response') return t('unknown_invalid');
    return t('unknown_network');
  }

  // ----------------------------------------------------------------- Toasts

  function toast(message, options = {}) {
    const region = document.getElementById('toast-region');
    if (!region) return () => {};
    const tone = options.tone || 'info';
    const element = document.createElement('div');
    element.className = 'toast';
    element.dataset.tone = tone;
    element.setAttribute('role', tone === 'error' ? 'alert' : 'status');

    const text = document.createElement('div');
    text.className = 'toast-message';
    text.textContent = message;
    element.append(text);

    const actions = document.createElement('div');
    actions.className = 'toast-actions';
    if (options.action) {
      const actionButton = document.createElement('button');
      actionButton.type = 'button';
      actionButton.className = 'button button-secondary button-compact';
      actionButton.textContent = options.action.label;
      actionButton.addEventListener('click', () => {
        options.action.onClick();
        dismiss();
      });
      actions.append(actionButton);
    }
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'toast-dismiss';
    close.setAttribute('aria-label', t('dismiss'));
    close.textContent = '×';
    close.addEventListener('click', () => dismiss());
    actions.append(close);
    element.append(actions);

    region.append(element);
    while (region.children.length > 4) region.firstElementChild.remove();

    const sticky = tone === 'error' || tone === 'warn' || options.action;
    const timer = sticky ? null : setTimeout(() => dismiss(), options.timeout || 4500);
    function dismiss() {
      if (timer) clearTimeout(timer);
      element.remove();
    }
    return dismiss;
  }

  const reloadAction = () => ({ label: t('reload'), onClick: () => window.location.reload() });

  // ------------------------------------------------------- Status refreshes

  function scopeRoot() {
    return document.querySelector('.scope-page[data-state-url]');
  }

  function setConfirmed(input, value) {
    input.checked = value;
    input.defaultChecked = value;
    input.dataset.confirmed = String(value);
  }

  function confirmedValue(input) {
    if (input.dataset.confirmed === undefined) input.dataset.confirmed = String(input.defaultChecked);
    return input.dataset.confirmed === 'true';
  }

  function entitySelector(kind, id) {
    return `[data-entity-kind="${cssEscape(kind)}"][data-entity-id="${cssEscape(id)}"]`;
  }

  function applyEntityView(view) {
    for (const element of document.querySelectorAll(`[data-entity-status]${entitySelector(view.kind, view.id)}`)) {
      if (element.classList.contains('entity-card')) {
        element.dataset.state = view.state;
        const toggle = element.querySelector('[data-action="toggle-entity"]');
        if (toggle && !toggle.disabled) setConfirmed(toggle, view.own_enabled);
      }
      const badge = element.querySelector('[data-status-badge]');
      if (badge) {
        badge.dataset.tone = view.tone;
        badge.textContent = view.state_label;
      }
      const chain = element.querySelector('[data-chain]');
      if (chain) {
        chain.replaceChildren(...view.chain.map((step) => {
          const item = document.createElement('li');
          item.className = 'chain-step';
          item.dataset.on = String(step.on);
          item.append(step.label);
          const hidden = document.createElement('span');
          hidden.className = 'visually-hidden';
          hidden.textContent = `: ${step.value_label}`;
          item.append(hidden);
          return item;
        }));
      }
      const blocker = element.querySelector('[data-blocker]');
      if (blocker) {
        blocker.textContent = view.blocker || '';
        blocker.hidden = !view.blocker;
      }
    }
  }

  function applySync(sync) {
    for (const panel of document.querySelectorAll('[data-sync-panel]')) {
      panel.dataset.tone = sync.tone;
      const title = panel.querySelector('[data-sync-title]');
      const body = panel.querySelector('[data-sync-body]');
      if (title) title.textContent = sync.title;
      if (body) body.textContent = sync.body;
    }
  }

  function markSyncRequired() {
    applySync({ tone: 'warn', title: t('sync_required_title'), body: t('sync_required_body') });
  }

  async function refreshState() {
    const url = scopeRoot()?.dataset.stateUrl;
    if (!url) return false;
    const outcome = await request(url);
    if (outcome.kind !== 'success' || !outcome.body) return false;
    for (const view of outcome.body.entities || []) applyEntityView(view);
    if (outcome.body.sync) applySync(outcome.body.sync);
    for (const scope of document.querySelectorAll('[data-filter-scope]')) {
      updateStateCounts(scope);
      applyFilters(scope, false);
    }
    return true;
  }

  async function afterChange() {
    const refreshed = await refreshState();
    if (!refreshed) {
      const panel = document.querySelector('[data-sync-panel]');
      if (panel && panel.dataset.tone === 'success') markSyncRequired();
    }
  }

  // ---------------------------------------------------------------- Toggles

  function syncDialogSwitch(kind, id, value) {
    const form = document.querySelector(`[data-settings-form]${entitySelector(kind, id)}`);
    const input = form?.querySelector('[data-entity-switch="enabled"]');
    if (input) {
      input.checked = value;
      input.defaultChecked = value;
    }
  }

  function syncCardSwitch(kind, id, value) {
    const card = document.querySelector(`.entity-card${entitySelector(kind, id)}`);
    const input = card?.querySelector('[data-action="toggle-entity"]');
    if (input) setConfirmed(input, value);
  }

  async function onToggle(input) {
    const card = input.closest('[data-patch-url]');
    if (!card || input.dataset.pending === 'true') return;
    const confirmed = confirmedValue(input);
    const desired = input.checked;
    const name = card.dataset.entityName || '';
    input.dataset.pending = 'true';
    input.disabled = true;
    card.setAttribute('aria-busy', 'true');
    try {
      const outcome = await request(card.dataset.patchUrl, 'PATCH', { enabled: desired });
      if (outcome.kind === 'success') {
        setConfirmed(input, desired);
        syncDialogSwitch(card.dataset.entityKind, card.dataset.entityId, desired);
        toast(t(desired ? 'turned_on' : 'turned_off', { name }), { tone: 'success' });
        afterChange();
      } else if (outcome.kind === 'definite-failure') {
        setConfirmed(input, confirmed);
        toast(t('save_failed', { name, message: failureMessage(outcome) }), { tone: 'error' });
      } else {
        setConfirmed(input, confirmed);
        toast(t('outcome_unknown', { name, message: failureMessage(outcome) }), { tone: 'warn', action: reloadAction() });
        afterChange();
      }
    } finally {
      input.disabled = false;
      delete input.dataset.pending;
      card.removeAttribute('aria-busy');
    }
  }

  // ---------------------------------------------------------------- Dialogs

  const modalState = { active: null, returnFocus: null };
  const pickerSyncs = new WeakMap();
  const structuredSyncs = new WeakMap();

  function dialogFields(form) {
    return Array.from(form.elements).filter((element) => element.matches('input, select, textarea')
      && !element.matches('[data-color-picker-for], [data-picker-ui], [data-structured-ui], [data-preview-sample]')
      && !element.closest('[data-picker-ui], [data-structured-ui]'));
  }

  function fieldDirty(element) {
    if (element.type === 'checkbox' || element.type === 'radio') return element.checked !== element.defaultChecked;
    if (element.tagName === 'SELECT') {
      const fallback = Array.from(element.options).find((option) => option.defaultSelected) || element.options[0];
      return fallback ? element.value !== fallback.value : false;
    }
    return element.value !== element.defaultValue;
  }

  function isDirty(form) {
    return dialogFields(form).some(fieldDirty);
  }

  function commitDefaults(form) {
    for (const element of dialogFields(form)) {
      if (element.type === 'checkbox' || element.type === 'radio') {
        element.defaultChecked = element.checked;
      } else if (element.tagName === 'SELECT') {
        for (const option of element.options) option.defaultSelected = option.selected;
      } else {
        element.defaultValue = element.value;
      }
    }
  }

  function syncHelpers(form) {
    for (const picker of form.querySelectorAll('[data-color-picker-for]')) {
      const text = document.getElementById(picker.dataset.colorPickerFor);
      if (text && HEX_COLOR.test(text.value.trim())) picker.value = `#${text.value.trim().replace('#', '').toLowerCase()}`;
    }
    for (const input of form.querySelectorAll('[data-picker]')) {
      const sync = pickerSyncs.get(input);
      if (sync) sync();
    }
    for (const editor of form.querySelectorAll('[data-structured-editor]')) {
      const sync = structuredSyncs.get(editor);
      if (sync) sync();
    }
    for (const preview of form.querySelectorAll('[data-embed-preview]')) updatePreview(preview);
  }

  function resetDialog(form) {
    form.reset();
    clearErrors(form);
    setModalStatus(form, '', 'info');
    syncHelpers(form);
  }

  function setModalStatus(form, message, kind) {
    const status = form.querySelector('[data-modal-status]');
    if (!status) return;
    status.dataset.kind = kind;
    status.setAttribute('role', kind === 'error' ? 'alert' : 'status');
    status.textContent = message || '';
  }

  function discardBar(overlay) {
    return overlay.querySelector('[data-discard-bar]');
  }

  function focusFirst(overlay) {
    const dialog = overlay.querySelector('[data-modal-root]');
    if (!dialog) return;
    const focusable = Array.from(dialog.querySelectorAll(FOCUSABLE)).filter(isVisible);
    const preferred = focusable.find((element) => element.matches('input, select, textarea'));
    (preferred || focusable[0] || dialog).focus();
  }

  function showModal(overlay, trigger) {
    const open = modalState.active;
    if (open && open !== overlay) closeModal(open, false);
    const modalRoot = document.getElementById('modal-root');
    if (modalRoot && overlay.parentElement !== modalRoot) modalRoot.append(overlay);
    modalState.active = overlay;
    modalState.returnFocus = trigger instanceof HTMLElement ? trigger : document.activeElement;
    const bar = discardBar(overlay);
    if (bar) bar.hidden = true;
    overlay.hidden = false;
    document.body.classList.add('modal-open');
    const shell = document.querySelector('.app-shell');
    if (shell) shell.inert = true;
    window.requestAnimationFrame(() => focusFirst(overlay));
  }

  function closeModal(overlay, restoreFocus = true) {
    const form = overlay.querySelector('[data-settings-form], [data-announcement-channel-form], [data-owner-totp-form]');
    if (form) resetDialog(form);
    overlay.hidden = true;
    if (modalState.active === overlay) modalState.active = null;
    if (!document.querySelector('[data-modal-overlay]:not([hidden])')) {
      document.body.classList.remove('modal-open');
      const shell = document.querySelector('.app-shell');
      if (shell) shell.inert = false;
    }
    const returnFocus = modalState.returnFocus;
    modalState.returnFocus = null;
    if (restoreFocus && returnFocus && typeof returnFocus.focus === 'function') returnFocus.focus();
  }

  function attemptClose(overlay) {
    const form = overlay.querySelector('[data-settings-form]');
    const bar = discardBar(overlay);
    if (form && bar && isDirty(form)) {
      bar.hidden = false;
      bar.querySelector('[data-action="keep-editing"]')?.focus();
      return;
    }
    closeModal(overlay);
  }

  function textFromHtml(html) {
    const template = document.createElement('template');
    template.innerHTML = html;
    return (template.content.textContent || '').trim();
  }

  async function openSettings(trigger) {
    const id = trigger.dataset.modalId;
    let overlay = id ? document.getElementById(id) : null;
    if (!overlay) {
      if (trigger.getAttribute('aria-busy') === 'true') return;
      trigger.setAttribute('aria-busy', 'true');
      try {
        const response = await fetch(trigger.dataset.fragmentUrl, {
          credentials: 'same-origin',
          headers: { Accept: 'text/html' },
        });
        const html = await response.text();
        if (!response.ok) {
          toast(textFromHtml(html) || t('fragment_failed'), { tone: 'error' });
          return;
        }
        const template = document.createElement('template');
        template.innerHTML = html.trim();
        overlay = template.content.querySelector('[data-modal-overlay]');
        if (!overlay || overlay.id !== id) {
          toast(t('fragment_failed'), { tone: 'error' });
          return;
        }
        document.getElementById('modal-root').append(overlay);
        initDialog(overlay);
      } catch (_) {
        toast(t('fragment_failed'), { tone: 'error' });
        return;
      } finally {
        trigger.removeAttribute('aria-busy');
      }
    }
    showModal(overlay, trigger);
  }

  function initDialog(overlay) {
    const form = overlay.querySelector('[data-settings-form]');
    if (!form) return;
    initStructuredEditors(form);
    for (const preview of form.querySelectorAll('[data-embed-preview]')) updatePreview(preview);
    enhancePickers(form);
  }

  function trapFocus(event, overlay) {
    const dialog = overlay.querySelector('[data-modal-root]');
    if (!dialog) return;
    const focusable = Array.from(dialog.querySelectorAll(FOCUSABLE)).filter(isVisible);
    if (!focusable.length) {
      event.preventDefault();
      dialog.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
      event.preventDefault();
      first.focus();
    }
  }

  // ------------------------------------------------- Collect and validate

  function setPath(target, key, value) {
    const segments = key.split('.');
    let cursor = target;
    for (const segment of segments.slice(0, -1)) {
      if (typeof cursor[segment] !== 'object' || cursor[segment] === null || Array.isArray(cursor[segment])) {
        cursor[segment] = {};
      }
      cursor = cursor[segment];
    }
    cursor[segments[segments.length - 1]] = value;
  }

  function getPath(source, key) {
    return key.split('.').reduce((value, segment) => (value && typeof value === 'object' ? value[segment] : undefined), source);
  }

  function lines(raw) {
    return raw.split(/\r?\n|,/).map((item) => item.trim()).filter(Boolean);
  }

  function collect(form) {
    const config = {};
    const errors = [];
    for (const element of form.querySelectorAll('[data-setting-key]')) {
      const key = element.dataset.settingKey;
      const kind = element.dataset.settingKind;
      if (kind === 'toggle') {
        setPath(config, key, element.checked);
        continue;
      }
      const raw = (element.value || '').trim();
      if (raw === '') {
        if (element.dataset.required === 'true') errors.push({ element, message: t('invalid_required') });
        continue;
      }
      switch (kind) {
        case 'integer': {
          if (!/^-?\d+$/.test(raw)) {
            errors.push({ element, message: t('invalid_integer') });
            break;
          }
          const value = Number.parseInt(raw, 10);
          const min = element.getAttribute('min');
          const max = element.getAttribute('max');
          if (min !== null && value < Number(min)) errors.push({ element, message: t('invalid_min', { min }) });
          else if (max !== null && value > Number(max)) errors.push({ element, message: t('invalid_max', { max }) });
          else setPath(config, key, value);
          break;
        }
        case 'channel':
        case 'role':
          if (SNOWFLAKE.test(raw)) setPath(config, key, raw);
          else errors.push({ element, message: t('invalid_snowflake') });
          break;
        case 'role_list': {
          const items = lines(raw);
          if (items.some((item) => !SNOWFLAKE.test(item))) errors.push({ element, message: t('invalid_snowflake') });
          else if (items.length) setPath(config, key, items);
          break;
        }
        case 'text_list': {
          const items = raw.split(/\r?\n/).map((item) => item.trim()).filter(Boolean);
          if (items.length) setPath(config, key, items);
          break;
        }
        case 'color':
          if (HEX_COLOR.test(raw)) setPath(config, key, `#${raw.replace('#', '').toUpperCase()}`);
          else errors.push({ element, message: t('invalid_color') });
          break;
        case 'url': {
          let valid = false;
          try {
            const url = new URL(raw);
            valid = url.protocol === 'http:' || url.protocol === 'https:';
          } catch (_) {
            valid = false;
          }
          if (valid) setPath(config, key, raw);
          else errors.push({ element, message: t('invalid_url') });
          break;
        }
        case 'json':
          try {
            setPath(config, key, JSON.parse(raw));
          } catch (error) {
            errors.push({ element, message: t('invalid_json', { error: error.message }) });
          }
          break;
        case 'invite_ranks': {
          let parsed;
          try {
            parsed = JSON.parse(raw);
          } catch (_) {
            parsed = null;
          }
          const valid = Array.isArray(parsed) && parsed.every((item) => item
            && Number.isInteger(Number(item.invites))
            && Number(item.invites) >= 1
            && SNOWFLAKE.test(String(item.role_id || '').trim()));
          if (!valid) {
            errors.push({ element, message: t('invalid_rank') });
          } else {
            setPath(config, key, parsed.map((item) => ({
              invites: Number(item.invites),
              role_id: String(item.role_id).trim(),
            })));
          }
          break;
        }
        case 'ticket_categories': {
          let parsed;
          try {
            parsed = JSON.parse(raw);
          } catch (_) {
            parsed = null;
          }
          const valid = Array.isArray(parsed) && parsed.every((item) => {
            const name = item ? String(item.name || '').trim() : '';
            const staff = item && Array.isArray(item.staff_roles) ? item.staff_roles : null;
            return name.length >= 1
              && name.length <= 80
              && staff !== null
              && staff.every((role) => SNOWFLAKE.test(String(role || '').trim()));
          });
          if (!valid) {
            errors.push({ element, message: t('invalid_category') });
          } else {
            setPath(config, key, parsed.map((item) => ({
              name: String(item.name).trim(),
              staff_roles: item.staff_roles.map((role) => String(role).trim()).filter(Boolean),
            })));
          }
          break;
        }
        default:
          setPath(config, key, raw);
      }
    }
    return { config, errors };
  }

  function errorTargetFor(element) {
    if (!element.hidden) return element;
    return element.parentElement.querySelector(
      '[data-picker-ui], [data-structured-editor] input, [data-structured-editor] select, [data-structured-editor] button'
    ) || element;
  }

  function showErrors(errors) {
    for (const { element, message } of errors) {
      element.setAttribute('aria-invalid', 'true');
      errorTargetFor(element).setAttribute('aria-invalid', 'true');
      const slot = document.getElementById(`${element.id}-error`);
      if (slot) slot.textContent = message;
    }
  }

  function clearFieldError(element) {
    const field = element.closest('.field');
    if (!field) return;
    for (const invalid of field.querySelectorAll('[aria-invalid]')) invalid.removeAttribute('aria-invalid');
    const slot = field.querySelector('[data-field-error]');
    if (slot) slot.textContent = '';
  }

  function clearErrors(form) {
    for (const invalid of form.querySelectorAll('[aria-invalid]')) invalid.removeAttribute('aria-invalid');
    for (const slot of form.querySelectorAll('[data-field-error]')) slot.textContent = '';
  }

  function applyReconciled(form, settings) {
    const collection = settings?.[`${form.dataset.entityKind}s`];
    const current = collection?.[form.dataset.entityId];
    if (!current) return false;
    for (const input of form.querySelectorAll('[data-entity-switch]')) {
      if (typeof current[input.name] === 'boolean') input.checked = current[input.name];
    }
    const configuration = current.configuration || {};
    for (const element of form.querySelectorAll('[data-setting-key]')) {
      const value = getPath(configuration, element.dataset.settingKey);
      const kind = element.dataset.settingKind;
      if (kind === 'toggle') element.checked = value === true;
      else if (kind === 'json' || kind === 'invite_ranks' || kind === 'ticket_categories') {
        element.value = value === undefined ? (kind === 'json' ? '' : '[]') : JSON.stringify(value, null, 2);
      } else if (Array.isArray(value)) element.value = value.join('\n');
      else element.value = value === undefined || value === null ? '' : String(value);
    }
    commitDefaults(form);
    syncHelpers(form);
    return true;
  }

  async function onSubmit(form) {
    if (form.dataset.pending === 'true') return;
    const overlay = form.closest('[data-modal-overlay]');
    clearErrors(form);
    const { config, errors } = collect(form);
    if (errors.length) {
      showErrors(errors);
      setModalStatus(form, t('fix_fields'), 'error');
      errorTargetFor(errors[0].element).focus();
      return;
    }
    const body = {};
    for (const input of form.querySelectorAll('[data-entity-switch]')) body[input.name] = input.checked;
    if (form.dataset.sendsConfiguration === 'true') body.configuration = config;
    const name = form.dataset.entityName || '';
    const kind = form.dataset.entityKind;
    const id = form.dataset.entityId;

    form.dataset.pending = 'true';
    const elements = Array.from(form.elements).map((element) => ({ element, disabled: element.disabled }));
    for (const item of elements) item.element.disabled = true;
    setModalStatus(form, t('loading'), 'info');
    try {
      const outcome = await request(form.dataset.patchUrl, 'PATCH', body);
      if (outcome.kind === 'success') {
        commitDefaults(form);
        const enabled = form.querySelector('[data-entity-switch="enabled"]');
        if (enabled) syncCardSwitch(kind, id, enabled.checked);
        setModalStatus(form, '', 'info');
        for (const item of elements) item.element.disabled = item.disabled;
        if (overlay) closeModal(overlay);
        toast(t('saved', { name }), { tone: 'success' });
        afterChange();
      } else if (outcome.kind === 'definite-failure') {
        setModalStatus(form, t('save_failed', { name, message: failureMessage(outcome) }), 'error');
      } else {
        const refreshed = await request(form.dataset.reconcileUrl);
        if (refreshed.kind === 'success' && applyReconciled(form, refreshed.body)) {
          setModalStatus(form, t('reconciled', { name }), 'info');
        } else {
          setModalStatus(form, t('reconcile_failed'), 'error');
        }
        toast(t('outcome_unknown', { name, message: failureMessage(outcome) }), { tone: 'warn', action: reloadAction() });
        afterChange();
      }
    } finally {
      for (const item of elements) item.element.disabled = item.disabled;
      delete form.dataset.pending;
    }
  }

  // ---------------------------------------------------- Channel/role pickers

  const resourceRequests = new Map();

  function loadResources(guildId) {
    if (!resourceRequests.has(guildId)) {
      resourceRequests.set(guildId, request(`/api/guild-resources/${guildId}`).then((outcome) => {
        if (outcome.kind !== 'success') throw outcome;
        return outcome.body;
      }));
    }
    return resourceRequests.get(guildId);
  }

  function pickerStatus(input, message) {
    let status = input.parentElement.querySelector('.picker-status');
    if (!status) {
      status = document.createElement('p');
      status.className = 'picker-status';
      input.insertAdjacentElement('afterend', status);
    }
    status.textContent = message;
    status.hidden = !message;
  }

  async function enhancePickers(form) {
    const inputs = Array.from(form.querySelectorAll('[data-picker]'));
    if (!inputs.length || !form.dataset.guildId) return;
    for (const input of inputs) pickerStatus(input, t('picker_loading'));
    let resources;
    try {
      resources = await loadResources(form.dataset.guildId);
    } catch (_) {
      resourceRequests.delete(form.dataset.guildId);
      for (const input of inputs) pickerStatus(input, t('picker_failed'));
      return;
    }
    for (const input of inputs) {
      pickerStatus(input, '');
      if (input.dataset.picker === 'role-list') buildRoleList(input, resources.roles || []);
      else if (input.dataset.picker === 'channel') buildSelect(input, resources.channels || [], '#');
      else buildSelect(input, resources.roles || [], '@');
    }
  }

  function buildSelect(input, items, prefix) {
    if (pickerSyncs.has(input) || !items.length) return;
    const select = document.createElement('select');
    select.id = `${input.id}-picker`;
    select.dataset.pickerUi = '';
    const describedBy = input.getAttribute('aria-describedby');
    if (describedBy) select.setAttribute('aria-describedby', describedBy);
    const label = input.closest('.field')?.querySelector(`label[for="${cssEscape(input.id)}"]`);
    if (label) label.htmlFor = select.id;

    const none = new Option(t('picker_none'), '');
    select.append(none);
    const groups = new Map();
    for (const item of items) {
      const option = new Option(`${prefix}${item.name}`, item.id);
      if (item.category) {
        if (!groups.has(item.category)) {
          const group = document.createElement('optgroup');
          group.label = item.category;
          groups.set(item.category, group);
          select.append(group);
        }
        groups.get(item.category).append(option);
      } else {
        select.append(option);
      }
    }
    const manual = new Option(t('picker_manual'), '__manual');
    select.append(manual);
    input.insertAdjacentElement('beforebegin', select);

    const known = new Set(items.map((item) => item.id));
    const sync = () => {
      const value = input.value.trim();
      select.querySelector('option[data-unknown]')?.remove();
      if (value && !known.has(value)) {
        const unknown = new Option(t('picker_unknown', { id: value }), value);
        unknown.dataset.unknown = '';
        select.insertBefore(unknown, manual);
      }
      select.value = value;
      input.hidden = true;
    };
    select.addEventListener('change', () => {
      if (select.value === '__manual') {
        input.hidden = false;
        input.focus();
        return;
      }
      input.value = select.value;
      input.hidden = true;
      clearFieldError(input);
    });
    pickerSyncs.set(input, sync);
    sync();
  }

  function buildRoleList(textarea, roles) {
    if (pickerSyncs.has(textarea) || !roles.length) return;
    const field = textarea.closest('.field');
    const label = field?.querySelector(`label[for="${cssEscape(textarea.id)}"]`);
    const box = document.createElement('div');
    box.className = 'picker-list';
    box.dataset.pickerUi = '';
    box.setAttribute('role', 'group');
    if (label) box.setAttribute('aria-label', label.textContent.trim());
    const describedBy = textarea.getAttribute('aria-describedby');
    if (describedBy) box.setAttribute('aria-describedby', describedBy);

    const known = new Set(roles.map((role) => role.id));
    const checkboxes = roles.map((role) => {
      const row = document.createElement('label');
      const checkbox = document.createElement('input');
      checkbox.type = 'checkbox';
      checkbox.value = role.id;
      const dot = document.createElement('span');
      dot.className = 'role-dot';
      if (role.color) dot.style.backgroundColor = role.color;
      row.append(checkbox, dot, document.createTextNode(`@${role.name}`));
      row.dataset.search = role.name.toLowerCase();
      box.append(row);
      return checkbox;
    });

    if (roles.length > 10) {
      const filter = document.createElement('input');
      filter.type = 'search';
      filter.className = 'search-input';
      filter.placeholder = t('picker_search_roles');
      filter.setAttribute('aria-label', t('picker_search_roles'));
      filter.dataset.pickerUi = '';
      filter.addEventListener('input', () => {
        const query = filter.value.trim().toLowerCase();
        for (const row of box.querySelectorAll('label')) row.hidden = !!query && !row.dataset.search.includes(query);
      });
      textarea.insertAdjacentElement('beforebegin', filter);
    }
    textarea.insertAdjacentElement('beforebegin', box);

    box.addEventListener('change', () => {
      const unknown = lines(textarea.value).filter((id) => !known.has(id));
      const selected = checkboxes.filter((checkbox) => checkbox.checked).map((checkbox) => checkbox.value);
      textarea.value = [...unknown, ...selected].join('\n');
      clearFieldError(textarea);
    });
    const sync = () => {
      const values = new Set(lines(textarea.value));
      for (const checkbox of checkboxes) checkbox.checked = values.has(checkbox.value);
      textarea.hidden = true;
    };
    pickerSyncs.set(textarea, sync);
    sync();
  }

  // ----------------------------------------------------- Structured editors

  function parseArrayJson(raw) {
    try {
      const parsed = JSON.parse(raw || '[]');
      return Array.isArray(parsed) ? parsed : [];
    } catch (_) {
      return [];
    }
  }

  function buildRoleSelect(roles, currentValue, ariaLabel, onChange) {
    const select = document.createElement('select');
    select.dataset.structuredUi = '';
    select.setAttribute('aria-label', ariaLabel);
    select.append(new Option(t('picker_none'), ''));
    const known = new Set(roles.map((role) => role.id));
    for (const role of roles) {
      select.append(new Option(`@${role.name}`, role.id));
    }
    if (currentValue && !known.has(currentValue)) {
      const unknown = new Option(t('picker_unknown', { id: currentValue }), currentValue);
      unknown.dataset.unknown = '';
      select.append(unknown);
    }
    const manualOption = new Option(t('picker_manual'), '__manual');
    select.append(manualOption);
    select.value = currentValue || '';

    const manual = document.createElement('input');
    manual.type = 'text';
    manual.inputMode = 'numeric';
    manual.placeholder = t('field_role_placeholder');
    manual.dataset.structuredUi = '';
    manual.setAttribute('aria-label', ariaLabel);
    manual.value = currentValue || '';
    manual.hidden = true;

    select.addEventListener('change', () => {
      if (select.value === '__manual') {
        manual.hidden = false;
        manual.focus();
        return;
      }
      manual.value = select.value;
      manual.hidden = true;
      onChange(select.value);
    });
    manual.addEventListener('input', () => onChange(manual.value.trim()));

    const wrap = document.createElement('div');
    wrap.className = 'stack';
    wrap.dataset.structuredUi = '';
    wrap.append(select, manual);
    return wrap;
  }

  function renderInviteRanksEditor(editor, textarea, roles) {
    let items = [];
    const write = () => {
      textarea.value = JSON.stringify(items);
      clearFieldError(textarea);
    };

    const render = () => {
      items = parseArrayJson(textarea.value).map((item) => ({
        invites: Number.parseInt(item?.invites, 10) || 1,
        role_id: String(item?.role_id || ''),
      }));
      editor.replaceChildren();
      const list = document.createElement('div');
      list.className = 'structured-list';

      if (!items.length) {
        const empty = document.createElement('p');
        empty.className = 'structured-empty';
        empty.textContent = t('rank_empty');
        list.append(empty);
      }

      items.forEach((item, index) => {
        const row = document.createElement('div');
        row.className = 'structured-row';
        row.dataset.structuredUi = '';

        const countField = document.createElement('div');
        countField.className = 'field';
        const countLabel = document.createElement('label');
        const countId = `${textarea.id}-rank-invites-${index}`;
        countLabel.htmlFor = countId;
        countLabel.textContent = t('rank_invites');
        const countInput = document.createElement('input');
        countInput.id = countId;
        countInput.type = 'number';
        countInput.min = '1';
        countInput.step = '1';
        countInput.inputMode = 'numeric';
        countInput.dataset.structuredUi = '';
        countInput.value = String(item.invites);
        countInput.addEventListener('input', () => {
          items[index].invites = Number.parseInt(countInput.value, 10) || 0;
          write();
        });
        countField.append(countLabel, countInput);

        const roleField = document.createElement('div');
        roleField.className = 'field';
        const roleLabel = document.createElement('label');
        roleLabel.textContent = t('rank_role');
        if (roles.length) {
          roleField.append(roleLabel, buildRoleSelect(roles, item.role_id, t('rank_role'), (val) => {
            items[index].role_id = val;
            write();
          }));
        } else {
          const roleId = `${textarea.id}-rank-role-${index}`;
          roleLabel.htmlFor = roleId;
          const roleInput = document.createElement('input');
          roleInput.id = roleId;
          roleInput.type = 'text';
          roleInput.inputMode = 'numeric';
          roleInput.placeholder = t('field_role_placeholder');
          roleInput.dataset.structuredUi = '';
          roleInput.value = item.role_id;
          roleInput.addEventListener('input', () => {
            items[index].role_id = roleInput.value.trim();
            write();
          });
          roleField.append(roleLabel, roleInput);
        }

        const removeBtn = document.createElement('button');
        removeBtn.type = 'button';
        removeBtn.className = 'button button-secondary button-compact';
        removeBtn.dataset.structuredUi = '';
        removeBtn.textContent = t('rank_remove');
        removeBtn.addEventListener('click', () => {
          items.splice(index, 1);
          write();
          render();
        });

        row.append(countField, roleField, removeBtn);
        list.append(row);
      });

      const addBtn = document.createElement('button');
      addBtn.type = 'button';
      addBtn.className = 'button button-secondary button-compact structured-add';
      addBtn.dataset.structuredUi = '';
      addBtn.textContent = `+ ${t('rank_add')}`;
      addBtn.addEventListener('click', () => {
        const nextInvites = items.length ? Math.max(...items.map((r) => r.invites || 0)) + 5 : 5;
        items.push({ invites: nextInvites, role_id: '' });
        write();
        render();
      });

      editor.append(list, addBtn);
    };

    structuredSyncs.set(editor, render);
    render();
  }

  function renderTicketCategoriesEditor(editor, textarea, roles) {
    let items = [];
    const write = () => {
      textarea.value = JSON.stringify(items);
      clearFieldError(textarea);
    };

    const render = () => {
      items = parseArrayJson(textarea.value).map((item) => ({
        name: String(item?.name || ''),
        staff_roles: Array.isArray(item?.staff_roles)
          ? item.staff_roles.map((r) => String(r || '').trim()).filter(Boolean)
          : [],
      }));
      editor.replaceChildren();
      const list = document.createElement('div');
      list.className = 'structured-list';

      if (!items.length) {
        const empty = document.createElement('p');
        empty.className = 'structured-empty';
        empty.textContent = t('category_empty');
        list.append(empty);
      }

      const known = new Set(roles.map((role) => role.id));

      items.forEach((item, index) => {
        const card = document.createElement('div');
        card.className = 'structured-card';
        card.dataset.structuredUi = '';

        const head = document.createElement('div');
        head.className = 'structured-card-head';
        const nameField = document.createElement('div');
        nameField.className = 'field';
        const nameId = `${textarea.id}-cat-name-${index}`;
        const nameLabel = document.createElement('label');
        nameLabel.htmlFor = nameId;
        nameLabel.textContent = t('category_name');
        const nameInput = document.createElement('input');
        nameInput.id = nameId;
        nameInput.type = 'text';
        nameInput.maxLength = 80;
        nameInput.dataset.structuredUi = '';
        nameInput.value = item.name;
        nameInput.addEventListener('input', () => {
          items[index].name = nameInput.value;
          write();
        });
        nameField.append(nameLabel, nameInput);

        const removeBtn = document.createElement('button');
        removeBtn.type = 'button';
        removeBtn.className = 'button button-secondary button-compact';
        removeBtn.dataset.structuredUi = '';
        removeBtn.textContent = t('category_remove');
        removeBtn.addEventListener('click', () => {
          items.splice(index, 1);
          write();
          render();
        });

        head.append(nameField, removeBtn);

        const rolesField = document.createElement('div');
        rolesField.className = 'field';
        const rolesLabel = document.createElement('label');
        rolesLabel.textContent = t('category_roles');
        rolesField.append(rolesLabel);

        if (roles.length) {
          const box = document.createElement('div');
          box.className = 'picker-list';
          box.dataset.structuredUi = '';
          box.setAttribute('role', 'group');
          box.setAttribute('aria-label', t('category_roles'));
          const selected = new Set(item.staff_roles);
          const unknown = item.staff_roles.filter((id) => !known.has(id));
          const checkboxes = roles.map((role) => {
            const row = document.createElement('label');
            const checkbox = document.createElement('input');
            checkbox.type = 'checkbox';
            checkbox.value = role.id;
            checkbox.checked = selected.has(role.id);
            checkbox.dataset.structuredUi = '';
            const dot = document.createElement('span');
            dot.className = 'role-dot';
            if (role.color) dot.style.backgroundColor = role.color;
            row.append(checkbox, dot, document.createTextNode(`@${role.name}`));
            box.append(row);
            return checkbox;
          });
          box.addEventListener('change', () => {
            items[index].staff_roles = [
              ...unknown,
              ...checkboxes.filter((cb) => cb.checked).map((cb) => cb.value),
            ];
            write();
          });
          rolesField.append(box);
        } else {
          const rolesArea = document.createElement('textarea');
          rolesArea.rows = 2;
          rolesArea.className = 'mono';
          rolesArea.placeholder = t('field_role_list_placeholder');
          rolesArea.dataset.structuredUi = '';
          rolesArea.setAttribute('aria-label', t('category_roles'));
          rolesArea.value = item.staff_roles.join('\n');
          rolesArea.addEventListener('input', () => {
            items[index].staff_roles = lines(rolesArea.value);
            write();
          });
          rolesField.append(rolesArea);
        }

        card.append(head, rolesField);
        list.append(card);
      });

      const addBtn = document.createElement('button');
      addBtn.type = 'button';
      addBtn.className = 'button button-secondary button-compact structured-add';
      addBtn.dataset.structuredUi = '';
      addBtn.textContent = `+ ${t('category_add')}`;
      addBtn.addEventListener('click', () => {
        items.push({ name: '', staff_roles: [] });
        write();
        render();
      });

      editor.append(list, addBtn);
    };

    structuredSyncs.set(editor, render);
    render();
  }

  async function initStructuredEditors(form) {
    const editors = Array.from(form.querySelectorAll('[data-structured-editor]'));
    if (!editors.length) return;
    const renderAll = (roles) => {
      for (const editor of editors) {
        const textarea = document.getElementById(editor.dataset.for);
        if (!textarea) continue;
        if (editor.dataset.structuredEditor === 'invite_ranks') {
          renderInviteRanksEditor(editor, textarea, roles);
        } else if (editor.dataset.structuredEditor === 'ticket_categories') {
          renderTicketCategoriesEditor(editor, textarea, roles);
        }
      }
    };
    renderAll([]);
    if (!form.dataset.guildId) return;
    try {
      const resources = await loadResources(form.dataset.guildId);
      if (resources?.roles?.length) renderAll(resources.roles);
    } catch (_) {
      // Fall back to the manual ID inputs already rendered.
    }
  }

  // ---------------------------------------------------------- Embed preview

  const SAMPLE_VALUES = {
    '{member:mention}': '@NewMember',
    '{member:name}': 'NewMember',
    '{member:nick}': 'NewMember',
    '{member:tag}': 'NewMember#0001',
    '{server}': 'Dynamo Lounge',
    '{count}': '128',
    '{inviter:name}': 'Alex',
    '{inviter:tag}': 'Alex#0001',
    '{invites}': '12',
    '\\n': '\n',
  };

  function applySampleValues(text, useSample) {
    if (!useSample || !text) return text;
    let output = text;
    for (const [token, sample] of Object.entries(SAMPLE_VALUES)) {
      output = output.split(token).join(sample);
    }
    return output;
  }

  function updatePreview(container) {
    const form = container.closest('form');
    if (!form) return;
    const prefix = container.dataset.embedPreview;
    const useSample = !!container.querySelector('[data-preview-sample]')?.checked;
    const part = (name) => container.querySelector(`[data-preview-part="${name}"]`);

    if (prefix === '__ticket') {
      const getField = (key) => {
        const element = form.querySelector(`[data-setting-key="${cssEscape(key)}"]`);
        return element ? (element.value || '').trim() : '';
      };
      const title = getField('setup_title');
      const description = getField('setup_description');
      const footer = getField('setup_footer');
      const hasEmbed = !!(title || description || footer);
      part('embed').hidden = !hasEmbed;
      if (part('title')) {
        part('title').textContent = title;
        part('title').hidden = !title;
      }
      part('description').textContent = description;
      part('description').hidden = !description;
      part('footer').textContent = footer;
      part('footer').hidden = !footer;
      part('empty').hidden = hasEmbed;
      return;
    }

    const value = (suffix) => {
      const element = form.querySelector(`[data-setting-key="${cssEscape(`${prefix}.${suffix}`)}"]`);
      return element ? (element.value || '').trim() : '';
    };
    const content = applySampleValues(value('content'), useSample);
    const description = applySampleValues(value('embed.description'), useSample);
    const footer = applySampleValues(value('embed.footer'), useSample);
    const image = value('embed.image');
    const color = value('embed.color');
    const hasEmbed = !!(description || footer || image);

    part('content').textContent = content;
    part('content').hidden = !content;
    part('embed').hidden = !hasEmbed;
    part('description').textContent = description;
    part('description').hidden = !description;
    part('footer').textContent = footer;
    part('footer').hidden = !footer;
    part('image').textContent = image;
    part('image').hidden = !image;
    part('embed').style.borderLeftColor = HEX_COLOR.test(color) ? `#${color.replace('#', '')}` : '';
    part('empty').hidden = !!(content || hasEmbed);
  }

  // ---------------------------------------------------------------- Filters

  function pressedValue(scope, attribute) {
    const button = scope.querySelector(`[data-${attribute}][aria-pressed="true"]`);
    return button ? button.getAttribute(`data-${attribute}`) : 'all';
  }

  function setPressed(scope, attribute, value) {
    const buttons = Array.from(scope.querySelectorAll(`[data-${attribute}]`));
    const match = buttons.some((button) => button.getAttribute(`data-${attribute}`) === value);
    for (const button of buttons) {
      const pressed = match ? button.getAttribute(`data-${attribute}`) === value : button.getAttribute(`data-${attribute}`) === 'all';
      button.setAttribute('aria-pressed', String(pressed));
    }
  }

  function updateStateCounts(scope) {
    const items = Array.from(scope.querySelectorAll('[data-filter-item]'));
    for (const badge of scope.querySelectorAll('[data-state-count]')) {
      const state = badge.dataset.stateCount;
      badge.textContent = String(state === 'all' ? items.length : items.filter((item) => item.dataset.state === state).length);
    }
  }

  function syncUrl(values) {
    const url = new URL(window.location.href);
    for (const [key, value] of Object.entries(values)) {
      if (value) url.searchParams.set(key, value);
      else url.searchParams.delete(key);
    }
    if (url.href !== window.location.href) window.history.replaceState(window.history.state, '', url);
  }

  function applyFilters(scope, updateUrl = true) {
    const input = scope.querySelector('[data-filter-input]');
    const query = (input ? input.value : '').trim().toLowerCase();
    const state = pressedValue(scope, 'state-filter');
    const category = pressedValue(scope, 'category-filter');
    const items = Array.from(scope.querySelectorAll('[data-filter-item]'));
    let visible = 0;
    for (const item of items) {
      const matches = (!query || (item.dataset.search || '').includes(query))
        && (state === 'all' || item.dataset.state === state)
        && (category === 'all' || item.dataset.category === category);
      item.hidden = !matches;
      if (matches) visible += 1;
    }
    if (query) {
      for (const details of scope.querySelectorAll('details.selector-section')) {
        if (details.querySelector('[data-filter-item]:not([hidden])')) details.open = true;
      }
    }
    const filtered = !!query || state !== 'all' || category !== 'all';
    const status = scope.querySelector('[data-filter-status]');
    if (status) status.textContent = filtered ? t('filter_count', { count: visible, total: items.length }) : '';
    const empty = scope.querySelector('[data-filter-empty]');
    if (empty) empty.hidden = visible !== 0;
    if (updateUrl) {
      syncUrl({
        q: query || null,
        state: state !== 'all' ? state : null,
        cat: category !== 'all' ? category : null,
      });
    }
  }

  function initFilters() {
    const params = new URLSearchParams(window.location.search);
    for (const scope of document.querySelectorAll('[data-filter-scope]')) {
      const input = scope.querySelector('[data-filter-input]');
      if (input && params.get('q')) input.value = params.get('q');
      if (params.get('state')) setPressed(scope, 'state-filter', params.get('state'));
      if (params.get('cat')) setPressed(scope, 'category-filter', params.get('cat'));
      if (params.get('q') || params.get('state') || params.get('cat')) applyFilters(scope, false);
    }
  }

  // --------------------------------------------------------------- Sync now

  async function requestSync(button) {
    if (button.disabled) return;
    button.disabled = true;
    try {
      const outcome = await request(button.dataset.syncUrl, 'POST');
      if (outcome.kind === 'success') {
        toast(t('sync_requested'), { tone: 'success' });
        await refreshState();
      } else if (outcome.kind === 'definite-failure') {
        toast(t('sync_failed', { message: failureMessage(outcome) }), { tone: 'error' });
      } else {
        toast(t('sync_failed', { message: failureMessage(outcome) }), { tone: 'warn', action: reloadAction() });
        await refreshState();
      }
    } finally {
      button.disabled = false;
    }
  }

  // ------------------------------------------------------------- Navigation

  const navState = { returnFocus: null };

  function navOpen() {
    return document.querySelector('.app-shell')?.hasAttribute('data-nav-open');
  }

  function openNav(toggle) {
    const shell = document.querySelector('.app-shell');
    if (!shell) return;
    shell.setAttribute('data-nav-open', '');
    for (const button of document.querySelectorAll('[data-action="open-nav"]')) button.setAttribute('aria-expanded', 'true');
    const content = document.querySelector('.content-shell');
    if (content) content.inert = true;
    navState.returnFocus = toggle;
    window.requestAnimationFrame(() => {
      const first = Array.from(document.querySelectorAll('#sidebar a, #sidebar button, #sidebar summary')).find(isVisible);
      if (first) first.focus();
    });
  }

  function closeNav(restoreFocus = true) {
    const shell = document.querySelector('.app-shell');
    if (!shell || !shell.hasAttribute('data-nav-open')) return;
    shell.removeAttribute('data-nav-open');
    for (const button of document.querySelectorAll('[data-action="open-nav"]')) button.setAttribute('aria-expanded', 'false');
    const content = document.querySelector('.content-shell');
    if (content) content.inert = false;
    if (restoreFocus && navState.returnFocus) navState.returnFocus.focus();
    navState.returnFocus = null;
  }

  // ------------------------------------------------------------------ Times

  function relativeTime(formatter, date) {
    const seconds = (date.getTime() - Date.now()) / 1000;
    const units = [['year', 31536000], ['month', 2592000], ['week', 604800], ['day', 86400], ['hour', 3600], ['minute', 60]];
    for (const [unit, size] of units) {
      if (Math.abs(seconds) >= size) return formatter.format(Math.round(seconds / size), unit);
    }
    return t('relative_now');
  }

  function formatTimes(root) {
    let absolute;
    let relative;
    try {
      absolute = new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' });
      relative = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
    } catch (_) {
      return;
    }
    for (const element of root.querySelectorAll('time[data-local-time]')) {
      const date = new Date(element.getAttribute('datetime'));
      if (Number.isNaN(date.getTime())) continue;
      element.textContent = absolute.format(date);
      element.title = date.toISOString();
    }
    for (const element of root.querySelectorAll('[data-relative-time]')) {
      const date = new Date(element.dataset.relativeTime);
      if (Number.isNaN(date.getTime())) continue;
      element.textContent = relativeTime(relative, date);
      element.title = absolute.format(date);
    }
  }

  // --------------------------------------------------- Local & Owner Actions

  function insertPlaceholderToken(button) {
    const field = document.getElementById(button.dataset.targetField || '');
    const token = button.dataset.insertToken || '';
    if (!field || !token) return;
    const start = typeof field.selectionStart === 'number' ? field.selectionStart : field.value.length;
    const end = typeof field.selectionEnd === 'number' ? field.selectionEnd : start;
    const next = `${field.value.slice(0, start)}${token}${field.value.slice(end)}`;
    field.value = next;
    const cursor = start + token.length;
    if (typeof field.setSelectionRange === 'function') field.setSelectionRange(cursor, cursor);
    field.dispatchEvent(new Event('input', { bubbles: true }));
    field.focus();
  }

  async function onSubmitAnnouncementChannel(form) {
    if (form.dataset.pending === 'true') return;
    clearErrors(form);
    const input = form.querySelector('[data-setting-key="announcement_channel_id"]');
    if (!input) return;
    const raw = (input.value || '').trim();
    if (raw && !SNOWFLAKE.test(raw)) {
      const error = { element: input, message: t('invalid_snowflake') };
      showErrors([error]);
      setModalStatus(form, t('fix_fields'), 'error');
      errorTargetFor(input).focus();
      return;
    }
    const submitBtn = form.querySelector('button[type="submit"]');
    form.dataset.pending = 'true';
    if (submitBtn) submitBtn.disabled = true;
    setModalStatus(form, t('loading'), 'info');
    try {
      const outcome = await request(form.dataset.patchUrl, 'PATCH', {
        announcement_channel_id: raw || null,
      });
      if (outcome.kind === 'success') {
        input.defaultValue = raw;
        const display = document.querySelector('[data-announcement-channel-display]');
        if (display) {
          const select = document.getElementById(`${input.id}-picker`);
          const selectedOption = select && raw
            ? Array.from(select.options).find((opt) => opt.value === raw)
            : null;
          display.textContent = raw
            ? `${selectedOption ? selectedOption.textContent : '#' + raw} (${raw})`
            : t('picker_none');
        }
        syncHelpers(form);
        const overlay = form.closest('[data-modal-overlay]');
        if (overlay) closeModal(overlay);
        toast(t('announce_channel_saved'), { tone: 'success' });
      } else {
        setModalStatus(form, failureMessage(outcome), 'error');
      }
    } finally {
      delete form.dataset.pending;
      if (submitBtn) submitBtn.disabled = false;
    }
  }

  async function onOwnerSyncRequest(button, url) {
    if (button.dataset.pending === 'true') return;
    button.dataset.pending = 'true';
    button.disabled = true;
    try {
      const outcome = await request(url, 'POST');
      if (outcome.kind === 'success') {
        toast(t('sync_requested'), { tone: 'success' });
      } else {
        toast(t('sync_failed', { message: failureMessage(outcome) }), { tone: 'error' });
      }
    } finally {
      delete button.dataset.pending;
      button.disabled = false;
    }
  }

  function openOwnerTotpModal(trigger) {
    const overlay = document.getElementById('modal-owner-totp-action');
    const form = overlay?.querySelector('[data-owner-totp-form]');
    if (!overlay || !form) return;
    form.dataset.actionUrl = trigger.dataset.url || '';
    form.dataset.ownerOp = trigger.dataset.ownerOp || '';
    form.dataset.blocked = trigger.dataset.blocked || '';
    const title = overlay.querySelector('[data-owner-modal-title]');
    if (title) title.textContent = (trigger.textContent || '').trim();
    const target = overlay.querySelector('[data-owner-modal-target]');
    if (target) target.textContent = trigger.dataset.guildName || '';
    const codeInput = form.querySelector('#owner-action-totp-code');
    if (codeInput) codeInput.value = '';
    clearErrors(form);
    setModalStatus(form, '', 'info');
    showModal(overlay, trigger);
  }

  async function onSubmitOwnerTotpForm(form) {
    if (form.dataset.pending === 'true') return;
    clearErrors(form);
    const codeInput = form.querySelector('#owner-action-totp-code');
    const code = (codeInput?.value || '').trim();
    if (!/^\d{6}$/.test(code)) {
      if (codeInput) {
        showErrors([{ element: codeInput, message: t('invalid_required') }]);
        codeInput.focus();
      }
      return;
    }
    const op = form.dataset.ownerOp;
    const payload = (op === 'block' || op === 'unblock')
      ? { blocked: form.dataset.blocked === 'true', totp_code: code }
      : { totp_code: code };
    const submitBtn = form.querySelector('button[type="submit"]');
    form.dataset.pending = 'true';
    if (submitBtn) submitBtn.disabled = true;
    setModalStatus(form, t('loading'), 'info');
    try {
      const outcome = await request(form.dataset.actionUrl, 'POST', payload);
      if (outcome.kind === 'success') {
        const overlay = form.closest('[data-modal-overlay]');
        if (overlay) closeModal(overlay, false);
        toast(t('owner_action_done'), { tone: 'success' });
        window.location.reload();
      } else {
        setModalStatus(form, failureMessage(outcome), 'error');
      }
    } finally {
      delete form.dataset.pending;
      if (submitBtn) submitBtn.disabled = false;
    }
  }

  async function onSubmitOwnerNotices(form) {
    if (form.dataset.pending === 'true') return;
    const module_notices = {};
    for (const input of form.querySelectorAll('[data-owner-notice-key]')) {
      module_notices[input.dataset.ownerNoticeKey] = (input.value || '').trim();
    }
    const submitBtn = form.querySelector('button[type="submit"]');
    form.dataset.pending = 'true';
    if (submitBtn) submitBtn.disabled = true;
    try {
      const outcome = await request(form.dataset.actionUrl, 'PATCH', { module_notices });
      if (outcome.kind === 'success') {
        toast(t('owner_notices_saved'), { tone: 'success' });
      } else {
        toast(failureMessage(outcome), { tone: 'error' });
      }
    } finally {
      delete form.dataset.pending;
      if (submitBtn) submitBtn.disabled = false;
    }
  }

  async function onSubmitOwnerBroadcast(form) {
    if (form.dataset.pending === 'true') return;
    const title = (form.querySelector('[name="title"]')?.value || '').trim();
    const tone = (form.querySelector('[name="tone"]')?.value || 'info').trim();
    const body = (form.querySelector('[name="body"]')?.value || '').trim();
    const totp_code = (form.querySelector('[name="totp_code"]')?.value || '').trim();
    const submitBtn = form.querySelector('button[type="submit"]');
    form.dataset.pending = 'true';
    if (submitBtn) submitBtn.disabled = true;
    try {
      const outcome = await request(form.dataset.actionUrl, 'POST', {
        title: title || null,
        tone,
        body,
        totp_code,
      });
      if (outcome.kind === 'success') {
        toast(t('owner_broadcast_sent'), { tone: 'success' });
        window.location.reload();
      } else {
        toast(failureMessage(outcome), { tone: 'error' });
      }
    } finally {
      delete form.dataset.pending;
      if (submitBtn) submitBtn.disabled = false;
    }
  }

  // ----------------------------------------------------------------- Events

  document.addEventListener('click', (event) => {
    for (const switcher of document.querySelectorAll('details.guild-switcher[open]')) {
      if (!switcher.contains(event.target)) switcher.removeAttribute('open');
    }

    const tokenButton = event.target.closest('[data-insert-token][data-target-field]');
    if (tokenButton) {
      event.preventDefault();
      insertPlaceholderToken(tokenButton);
      return;
    }

    const filterButton = event.target.closest('[data-state-filter], [data-category-filter]');
    if (filterButton) {
      const scope = filterButton.closest('[data-filter-scope]');
      const attribute = filterButton.hasAttribute('data-state-filter') ? 'state-filter' : 'category-filter';
      if (scope) {
        setPressed(scope, attribute, filterButton.getAttribute(`data-${attribute}`));
        applyFilters(scope);
      }
      return;
    }

    if (event.target.matches('[data-modal-overlay]')) {
      attemptClose(event.target);
      return;
    }

    const actionElement = event.target.closest('[data-action]');
    if (!actionElement) return;
    const overlay = actionElement.closest('[data-modal-overlay]');
    switch (actionElement.dataset.action) {
      case 'open-settings':
        event.preventDefault();
        openSettings(actionElement);
        break;
      case 'open-local-modal': {
        event.preventDefault();
        const targetOverlay = document.getElementById(actionElement.dataset.modalId || '');
        if (targetOverlay) {
          const announceForm = targetOverlay.querySelector('[data-announcement-channel-form]');
          if (announceForm) enhancePickers(announceForm);
          showModal(targetOverlay, actionElement);
        }
        break;
      }
      case 'owner-batch-sync':
        event.preventDefault();
        onOwnerSyncRequest(actionElement, '/api/owner/sync-stale-guilds');
        break;
      case 'owner-guild-sync': {
        event.preventDefault();
        const guildId = actionElement.dataset.guildId;
        if (guildId) onOwnerSyncRequest(actionElement, `/api/owner/guilds/${guildId}/sync`);
        break;
      }
      case 'open-owner-totp-modal':
        event.preventDefault();
        openOwnerTotpModal(actionElement);
        break;
      case 'close-modal':
        if (overlay) attemptClose(overlay);
        break;
      case 'cancel-modal':
        if (overlay) closeModal(overlay);
        break;
      case 'keep-editing': {
        const bar = overlay ? discardBar(overlay) : null;
        if (bar) bar.hidden = true;
        if (overlay) focusFirst(overlay);
        break;
      }
      case 'discard-changes':
        if (overlay) closeModal(overlay);
        break;
      case 'request-sync':
        requestSync(actionElement);
        break;
      case 'open-nav':
        openNav(actionElement);
        break;
      case 'close-nav':
        closeNav();
        break;
      default:
        break;
    }
  });

  document.addEventListener('change', (event) => {
    const target = event.target;
    if (target.matches('[data-action="toggle-entity"]')) {
      onToggle(target);
      return;
    }
    if (target.matches('[data-preview-sample]')) {
      const preview = target.closest('[data-embed-preview]');
      if (preview) updatePreview(preview);
    }
  });

  document.addEventListener('input', (event) => {
    const target = event.target;
    if (target.matches('[data-filter-input]')) {
      const scope = target.closest('[data-filter-scope]');
      if (scope) applyFilters(scope);
      return;
    }
    if (target.matches('[data-color-picker-for]')) {
      const text = document.getElementById(target.dataset.colorPickerFor);
      if (text) {
        text.value = target.value.toUpperCase();
        clearFieldError(text);
      }
    } else if (target.matches('[data-setting-kind="color"]')) {
      const picker = target.parentElement.querySelector('[data-color-picker-for]');
      if (picker && HEX_COLOR.test(target.value.trim())) picker.value = `#${target.value.trim().replace('#', '').toLowerCase()}`;
    }
    if (target.matches('[data-setting-key], [data-color-picker-for]')) {
      clearFieldError(target);
      const form = target.closest('form');
      if (form) {
        for (const preview of form.querySelectorAll('[data-embed-preview]')) updatePreview(preview);
      }
    }
  });

  document.addEventListener('submit', (event) => {
    const form = event.target;
    if (form.matches('[data-settings-form]')) {
      event.preventDefault();
      onSubmit(form);
    } else if (form.matches('[data-announcement-channel-form]')) {
      event.preventDefault();
      onSubmitAnnouncementChannel(form);
    } else if (form.matches('[data-owner-totp-form]')) {
      event.preventDefault();
      onSubmitOwnerTotpForm(form);
    } else if (form.matches('[data-owner-notices-form]')) {
      event.preventDefault();
      onSubmitOwnerNotices(form);
    } else if (form.matches('[data-owner-broadcast-form]')) {
      event.preventDefault();
      onSubmitOwnerBroadcast(form);
    }
  });

  document.addEventListener('keydown', (event) => {
    const overlay = modalState.active;
    if (overlay && !overlay.hidden) {
      if (event.key === 'Escape') {
        event.preventDefault();
        const bar = discardBar(overlay);
        if (bar && !bar.hidden) {
          bar.hidden = true;
          focusFirst(overlay);
        } else {
          attemptClose(overlay);
        }
      } else if (event.key === 'Tab') {
        trapFocus(event, overlay);
      }
      return;
    }
    if (event.key === 'Escape') {
      const openSwitcher = document.querySelector('details.guild-switcher[open]');
      if (openSwitcher) {
        event.preventDefault();
        openSwitcher.removeAttribute('open');
        openSwitcher.querySelector('summary')?.focus();
        return;
      }
    }
    if (event.key === 'Escape' && navOpen()) {
      event.preventDefault();
      closeNav();
    }
  });

  const desktop = window.matchMedia('(min-width: 1101px)');
  const onBreakpoint = (event) => {
    if (event.matches) closeNav(false);
  };
  if (typeof desktop.addEventListener === 'function') desktop.addEventListener('change', onBreakpoint);

  function init() {
    formatTimes(document);
    initFilters();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init, { once: true });
  else init();
}());
