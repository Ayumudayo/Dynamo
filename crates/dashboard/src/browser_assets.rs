use std::sync::OnceLock;

use super::font_assets::FONT_FACE_CSS;

pub(crate) fn dashboard_styles() -> &'static str {
    static STYLES: OnceLock<String> = OnceLock::new();
    STYLES
        .get_or_init(|| {
            let mut styles =
                String::with_capacity(FONT_FACE_CSS.len() + DASHBOARD_BASE_STYLES.len());
            styles.push_str(FONT_FACE_CSS);
            styles.push('\n');
            styles.push_str(DASHBOARD_BASE_STYLES);
            styles
        })
        .as_str()
}

const DASHBOARD_BASE_STYLES: &str = r#"
:root {
  --bg: #0c0f17;
  --sidebar: #0b0e15;
  --panel: #171b24;
  --panel-strong: #1f2430;
  --panel-border: rgba(255, 255, 255, 0.04);
  --text: #f8fafc;
  --muted: #7f8ba3;
  --accent: #dd2e53;
  --accent-text: #ff9aae;
  --accent-button: #bc173d;
  --accent-button-hover: #d61f4b;
  --accent-strong: #ff4d6d;
  --accent-soft: rgba(221, 46, 83, 0.16);
  --success: #48e5b2;
  --danger: #f97316;
  --shadow: 0 18px 48px rgba(0, 0, 0, 0.28);
}
* { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; background: var(--bg); color: var(--text); font-family: 'Fira Sans', 'Fira Sans Fallback', sans-serif; font-synthesis: none; }
body { position: relative; }
.backdrop {
  position: fixed; inset: 0;
  background:
    radial-gradient(circle at left bottom, rgba(221, 46, 83, 0.18), transparent 18%),
    radial-gradient(circle at top left, rgba(61, 84, 143, 0.12), transparent 24%),
    linear-gradient(180deg, #0b0e15, #0c0f17);
  pointer-events: none;
}
.app-shell { position: relative; display: grid; grid-template-columns: 252px minmax(0, 1fr); min-height: 100vh; }
.sidebar {
  position: sticky; top: 0; align-self: start; height: 100vh; padding: 24px 18px;
  background: rgba(8, 10, 16, 0.96); border-right: 1px solid rgba(255,255,255,0.05);
  display: flex; flex-direction: column; gap: 28px;
}
.content-shell { padding: 28px 28px 40px; min-width: 0; }
.content-topbar, .panel, section, article, details { border: 1px solid var(--panel-border); background: var(--panel); box-shadow: var(--shadow); }
.content-topbar {
  display: flex; justify-content: space-between; gap: 18px; align-items: center;
  padding: 18px 20px; border-radius: 18px; margin-bottom: 20px;
}
.content-topbar-right { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; justify-content: end; }
.sidebar-brand, .session-summary, .guild-card-head { display: flex; align-items: center; gap: 14px; }
.app-avatar, .user-avatar, .guild-avatar, .app-avatar-fallback, .user-avatar-fallback, .guild-avatar-fallback {
  width: 56px; height: 56px; border-radius: 18px; object-fit: cover; flex: none;
  display: grid; place-items: center; font-family: 'Fira Code', 'Fira Code Fallback', monospace; font-weight: 700;
  background: linear-gradient(135deg, rgba(221, 46, 83, 0.22), rgba(61, 84, 143, 0.18));
  border: 1px solid rgba(255,255,255,0.06);
}
.eyebrow { margin: 0 0 6px; color: var(--accent-text); font-size: 12px; letter-spacing: 0.16em; text-transform: uppercase; font-family: 'Fira Code', 'Fira Code Fallback', monospace; }
h1, h2, h3, legend { margin: 0; font-family: 'Fira Code', 'Fira Code Fallback', monospace; }
.sidebar-nav { display: grid; gap: 8px; }
.nav-link {
  color: var(--muted); text-decoration: none; padding: 13px 14px; border-radius: 14px;
  transition: background-color 180ms ease, color 180ms ease, border-color 180ms ease;
  border: 1px solid transparent; cursor: pointer; font-weight: 600;
}
.nav-link.active { color: var(--text); background: rgba(221, 46, 83, 0.14); border-color: rgba(221,46,83,0.2); }
.nav-link:hover:not(.active) { color: var(--text); background: rgba(221, 46, 83, 0.06); border-color: rgba(221,46,83,0.10); }
.nav-submenu { display: grid; gap: 2px; margin: 0 0 4px 12px; padding: 0 0 0 8px; }
.nav-sub-link { color: var(--muted); text-decoration: none; padding: 7px 10px; border-radius: 8px; cursor: pointer; font-size: 0.90rem; }
.nav-sub-link.active { color: var(--accent-text); font-weight: 600; }
.nav-sub-link:hover:not(.active) { color: var(--text); background: rgba(221,46,83,0.04); }
.sidebar-footer { margin-top: auto; padding-top: 12px; border-top: 1px solid rgba(255,255,255,0.06); }
.sidebar-footnote { color: var(--muted); font-size: 12px; }
.lede { margin: 8px 0 0; color: var(--muted); max-width: 70ch; line-height: 1.6; }
.stat-strip, .hero-card dl { display: grid; grid-template-columns: repeat(2, minmax(120px, 1fr)); gap: 12px; }
.stat, .hero-card dl > div {
  padding: 14px 16px; border-radius: 16px; background: var(--panel-strong); border: 1px solid rgba(255,255,255,0.04);
}
.stat span, dt { display: block; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
.stat strong, dd { margin: 8px 0 0; font-size: 24px; font-weight: 700; }
.hero { display: grid; grid-template-columns: 1.6fr 1fr; gap: 16px; padding: 20px; border-radius: 18px; margin-bottom: 18px; }
.hero.compact { grid-template-columns: 1.4fr 0.8fr; }
.actions { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 18px; }
.button {
  display: inline-flex; align-items: center; justify-content: center; text-decoration: none; cursor: pointer;
  padding: 12px 18px; border-radius: 10px; border: 1px solid transparent; font-weight: 700;
  transition: transform 180ms ease, background-color 180ms ease, border-color 180ms ease, color 180ms ease;
}
.button:hover { transform: translateY(-1px); }
.button-primary { background: var(--accent-button); color: #fff6fa; }
.button-primary:hover { background: var(--accent-button-hover); }
.button-secondary { background: var(--panel-strong); color: var(--text); border-color: rgba(255,255,255,0.06); }
.grid { display: grid; gap: 14px; }
.grid.two { grid-template-columns: repeat(2, minmax(0, 1fr)); margin-bottom: 16px; }
.grid.three { grid-template-columns: repeat(3, minmax(0, 1fr)); }
.panel, section, article, details { padding: 14px; border-radius: 14px; margin-bottom: 14px; }
.guild-card p, .panel p { color: var(--muted); line-height: 1.6; }
.pill {
  display: inline-flex; align-items: center; padding: 6px 10px; border-radius: 999px;
  font-size: 12px; font-family: 'Fira Code', 'Fira Code Fallback', monospace; border: 1px solid rgba(255,255,255,0.08);
}
.pill-success { color: #bbf7d0; background: rgba(72, 229, 178, 0.14); }
.pill-warn { color: #fdba74; background: rgba(249, 115, 22, 0.12); }
.toolbar-panel, .section-block { margin-bottom: 16px; }
.dashboard-page-shell { display: grid; gap: 0; }
.dashboard-page-shell-task-first { display: flex; flex-direction: column; }
.dashboard-page-shell-task-first .dashboard-page-overview { order: 1; }
.dashboard-page-shell-task-first .dashboard-page-tabs { order: 2; }
.dashboard-page-shell-task-first .dashboard-page-active { order: 3; }
.dashboard-page-overview, .dashboard-page-tabs, .dashboard-page-active { min-width: 0; }
.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.toolbar-search { width: min(100%, 320px); max-width: 320px; margin: 0; }
.toolbar-select { width: min(100%, 180px); margin: 0; }
.compact-toolbar { justify-content: flex-start; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
.compact-search { max-width: 240px; height: 40px; padding: 10px 12px; }
.section-heading { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 14px; flex-wrap: wrap; }
.section-heading > div { min-width: 0; flex: 1 1 220px; }
.section-heading .toolbar-search { flex: 1 1 220px; }
.compact-heading { margin-bottom: 12px; }
.module-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; }
.compact-module-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.command-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.compact-command-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.summary-card-head, .detail-panel-head { display: flex; justify-content: space-between; align-items: start; gap: 12px; }
.detail-panel-status { display: flex; align-items: center; }
.summary-card { min-height: 156px; display: flex; flex-direction: column; justify-content: space-between; }
.summary-card:hover, .guild-card:hover, .info-panel:hover, .logs-panel:hover {
  border-color: rgba(221,46,83,0.12);
  background: #1a1f2a;
  box-shadow: 0 14px 28px rgba(0,0,0,0.18);
}
.summary-card h3, .detail-panel h2, .command-detail-card h3 { font-size: 0.95rem; line-height: 1.15; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
.summary-card p, .detail-panel p, .command-detail-card p { font-size: 0.88rem; margin: 8px 0 0; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.summary-card .state-summary { display: block; -webkit-line-clamp: unset; overflow: visible; overflow-wrap: anywhere; }
.summary-card-subtitle { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; margin-top: 8px; }
.detail-panel p, .summary-card p, .info-panel p { color: var(--muted); }
.detail-meta { margin: 8px 0 0; }
.detail-stack { display: grid; gap: 18px; }
.command-detail-card { border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); border-radius: 14px; padding: 16px; }
.summary-card-meta { display: inline-flex; align-items: center; gap: 8px; margin: 6px 10px 0 0; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.summary-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 3px 7px; border-radius: 8px; font-size: 12px; }
.guild-card-meta { display: flex; align-items: center; gap: 10px; margin: 14px 0 16px; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.guild-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 4px 8px; border-radius: 8px; }
.empty-state { min-height: 220px; display: flex; flex-direction: column; justify-content: center; }
.filter-feedback { margin: 0 0 12px; color: var(--muted); font-size: 0.88rem; }
.filter-empty { margin: 0 0 12px; color: var(--muted); }
.card-action { margin-top: 10px; }
.tab-row { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 12px; }
.page-tab-row { margin-bottom: 14px; }
.tab-button {
  appearance: none; border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); color: var(--muted);
  padding: 7px 11px; border-radius: 10px; font: inherit; font-size: 0.85rem; font-weight: 700; cursor: pointer; width: auto; margin: 0; min-height: 38px;
}
.tab-button.active, .tab-button:hover { color: var(--text); background: rgba(221, 46, 83, 0.16); border-color: rgba(221,46,83,0.24); }
.sync-panel {
  display: flex; justify-content: space-between; align-items: center; gap: 14px;
  padding: 14px 16px; margin: 0 0 14px; border-radius: 14px;
  border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong);
}
.sync-panel-copy { min-width: 0; }
.sync-panel h3 { margin: 0; font-size: 0.98rem; }
.sync-panel p { margin: 6px 0 0; font-size: 0.88rem; color: var(--muted); line-height: 1.55; }
.sync-panel-status { margin-top: 8px; font-size: 12px; color: var(--muted-soft); }
.sync-panel-actions { display: flex; align-items: center; gap: 10px; flex-shrink: 0; }
.sync-panel-ok { border-color: rgba(72,229,178,0.18); background: linear-gradient(180deg, rgba(72,229,178,0.08), rgba(21,24,32,0.92)); }
.sync-panel-warn { border-color: rgba(221,46,83,0.26); background: linear-gradient(180deg, rgba(221,46,83,0.10), rgba(21,24,32,0.92)); }
.sync-panel-info { border-color: rgba(6,138,221,0.22); background: linear-gradient(180deg, rgba(6,138,221,0.09), rgba(21,24,32,0.92)); }
.sync-panel-error { border-color: rgba(245,158,11,0.24); background: linear-gradient(180deg, rgba(245,158,11,0.08), rgba(21,24,32,0.92)); }
.sync-panel-muted { border-color: rgba(255,255,255,0.06); background: rgba(255,255,255,0.02); }
.toggle-switch { position: relative; display: inline-flex; width: 44px; height: 24px; align-items: center; cursor: pointer; }
.toggle-switch input { position: absolute; inset: 0; opacity: 0; margin: 0; cursor: pointer; }
.toggle-slider { width: 44px; height: 24px; border-radius: 999px; background: #2a313e; border: 1px solid rgba(255,255,255,0.06); position: relative; transition: background-color 150ms ease; }
.toggle-slider::after { content: ''; position: absolute; top: 2px; left: 2px; width: 18px; height: 18px; border-radius: 50%; background: #aab3c5; transition: transform 150ms ease, background-color 150ms ease; }
.toggle-switch input:checked + .toggle-slider { background: rgba(72, 229, 178, 0.24); }
.toggle-switch input:checked + .toggle-slider::after { transform: translateX(20px); background: var(--success); }
.settings-modal-overlay { position: fixed; inset: 0; background: rgba(7, 9, 14, 0.78); display: grid; place-items: center; padding: 18px; z-index: 50; }
.settings-modal-overlay[hidden] { display: none !important; }
.settings-modal { width: min(640px, calc(100vw - 24px)); max-width: 100%; max-height: min(82vh, 860px); overflow: auto; background: #11151e; border: 1px solid rgba(255,255,255,0.08); border-radius: 18px; box-shadow: 0 30px 80px rgba(0,0,0,0.45); outline: none; }
.settings-modal-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; padding: 14px 16px 10px; position: sticky; top: 0; background: #11151e; z-index: 2; }
.settings-modal-body { padding: 0 16px 16px; }
.modal-close { width: auto; min-width: 40px; padding: 8px 12px; font-size: 24px; line-height: 1; background: transparent; color: var(--text); }
form, .advanced-json-form { margin-top: 12px; }
label, small, legend { color: var(--text); }
small, .section-help { color: var(--muted); font-size: 12px; line-height: 1.5; }
input, textarea, select, button {
  width: 100%; margin-top: 6px; margin-bottom: 0; border-radius: 10px; border: 1px solid rgba(255,255,255,0.06);
  background: #262b36; color: var(--text); padding: 10px 12px; font: inherit;
}
input[type='checkbox'] { width: auto; margin-right: 8px; }
button { width: auto; cursor: pointer; background: var(--accent-soft); color: #ffd5df; }
button:hover { background: rgba(221, 46, 83, 0.24); }
a:focus-visible, button:focus-visible, input:focus-visible, select:focus-visible, textarea:focus-visible, [tabindex]:focus-visible {
  outline: 3px solid #7dd3fc; outline-offset: 3px;
}
.toggle-switch input:focus-visible + .toggle-slider { outline: 3px solid #7dd3fc; outline-offset: 3px; }
fieldset { border: 1px solid rgba(255,255,255,0.06); border-radius: 14px; padding: 12px; margin-top: 12px; }
.settings-section > p { margin: 4px 0 0; }
.settings-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; margin-top: 10px; }
.settings-grid-basic { margin-bottom: 12px; }
.settings-field { display: grid; gap: 6px; align-content: start; }
.settings-field-span-2 { grid-column: 1 / -1; }
.settings-field-textarea textarea { min-height: 112px; resize: vertical; }
.toggle-field { display: flex; justify-content: space-between; align-items: start; gap: 12px; cursor: pointer; }
.toggle-field input { margin-top: 2px; }
.toggle-field-copy { display: grid; gap: 4px; }
.toggle-field-copy strong { font-size: 0.92rem; }
.modal-status-row { display: grid; gap: 8px; margin-top: 4px; }
.modal-actions { position: sticky; bottom: 0; display: flex; align-items: center; justify-content: flex-end; gap: 10px; margin-top: 14px; padding-top: 12px; background: linear-gradient(180deg, rgba(17,21,30,0), #11151e 24px); }
.modal-status { margin-right: auto; font-size: 12px; color: var(--muted); }
.modal-status[data-kind='success'], .card-status[data-kind='success'] { color: #bbf7d0; }
.modal-status[data-kind='error'], .card-status[data-kind='error'] { color: #fdba74; }
.card-status { font-size: 12px; color: var(--muted); min-height: 16px; }
.compact-actions { display: flex; align-items: center; gap: 8px; margin-top: 12px; }
.button-compact { padding: 8px 12px; font-size: 0.86rem; }
.button-disabled { opacity: 0.46; pointer-events: none; cursor: default; }
.compact-grid-two { gap: 12px; }
.compact-info-panel { min-height: 120px; }
.logs-panel { overflow-x: auto; }
.logs-table { width: 100%; border-collapse: collapse; table-layout: fixed; }
.logs-table th, .logs-table td { text-align: left; padding: 10px 12px; border-bottom: 1px solid rgba(255,255,255,0.06); vertical-align: top; font-size: 0.9rem; }
.logs-table th { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
.logs-table tbody tr:hover { background: rgba(255,255,255,0.02); }
.logs-mobile-list { display: none; gap: 10px; }
.audit-log-card { border: 1px solid rgba(255,255,255,0.06); border-radius: 14px; background: var(--panel-strong); padding: 12px; }
.audit-log-card-head { display: flex; align-items: start; justify-content: space-between; gap: 12px; margin-bottom: 10px; }
.audit-log-card-time { margin: 0; color: var(--muted); font-size: 12px; letter-spacing: 0.04em; }
.audit-log-card-fields { display: grid; gap: 10px; margin: 0; }
.audit-log-card-field { display: grid; gap: 4px; }
.audit-log-card-field-summary { padding-top: 4px; border-top: 1px solid rgba(255,255,255,0.06); }
.audit-log-card-field dt { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; }
.audit-log-card-field dd { margin: 0; color: var(--text); line-height: 1.5; overflow-wrap: anywhere; }
.logs-mobile-empty { color: var(--muted); text-align: center; padding: 18px 12px; border: 1px dashed rgba(255,255,255,0.08); border-radius: 14px; }
.empty-cell { color: var(--muted); text-align: center; padding: 18px 12px !important; }
.logs-pagination { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 10px; }
.page-count { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
details summary { cursor: pointer; color: var(--text); font-weight: 600; }
article { margin-top: 16px; }
a { color: var(--accent-text); }
.runtime-notice { border-color: rgba(249,115,22,0.22); background: rgba(124, 45, 18, 0.22); }
.content-body > section[id], .content-body > section.section-block { scroll-margin-top: 24px; }
@media (max-width: 1100px) {
  .app-shell { grid-template-columns: 1fr; }
  .sidebar { position: relative; height: auto; }
  .hero, .hero.compact, .grid.two, .grid.three, .module-grid, .command-grid, .compact-module-grid, .compact-command-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .toolbar { flex-direction: column; align-items: stretch; }
  .content-shell { padding: 20px; }
  .settings-grid { grid-template-columns: 1fr; }
}
@media (max-width: 820px) {
  .sidebar { padding: 14px; gap: 14px; }
  .sidebar-brand { gap: 10px; }
  .sidebar-brand .app-avatar, .sidebar-brand .app-avatar-fallback { width: 40px; height: 40px; border-radius: 13px; }
  .sidebar-brand h1 { font-size: 1.1rem; }
  .sidebar-nav { gap: 4px; }
  .nav-link { padding: 9px 10px; }
  .sidebar-footer { padding-top: 0; border-top: 0; }
  .sidebar-footnote { display: none; }
  .content-topbar, .hero, .hero.compact, .grid.two, .grid.three, .module-grid, .command-grid, .compact-module-grid, .compact-command-grid { grid-template-columns: 1fr; }
  .content-topbar { display: grid; align-items: stretch; }
  .content-topbar-right { justify-content: start; }
  .settings-modal { width: min(96vw, 640px); }
  .section-heading { align-items: stretch; }
  .section-heading .toolbar-search, .section-heading .compact-search { max-width: none; width: 100%; }
  .dashboard-page-shell-task-first { display: flex; flex-direction: column; }
  .dashboard-page-shell-task-first .dashboard-page-tabs { order: 1; }
  .dashboard-page-shell-task-first .dashboard-page-active { order: 2; }
  .dashboard-page-shell-task-first .dashboard-page-overview { order: 3; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero { padding: 16px; margin-top: 8px; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero h1 { font-size: 1.35rem; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero .lede { font-size: 0.92rem; }
  .logs-panel { overflow-x: visible; }
  .logs-table { display: none; }
  .logs-mobile-list { display: grid; }
  .logs-pagination { flex-wrap: wrap; }
}
@media (max-width: 560px) {
  .sync-panel { flex-direction: column; align-items: stretch; }
  .sync-panel-actions, .sync-panel-actions .button { width: 100%; }
  .empty-state { min-height: 120px; }
  .settings-modal-overlay { padding: 12px; align-items: start; overflow-y: auto; }
  .settings-modal-head { align-items: start; }
  .toggle-field { flex-direction: column; }
  .modal-actions { flex-direction: column-reverse; align-items: stretch; }
  .modal-actions .button { width: 100%; }
  .modal-status { margin-right: 0; min-height: 18px; }
}
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { transition: none !important; animation: none !important; }
}
"#;

pub(crate) fn dashboard_ui_script() -> &'static str {
    r#"
function filterGuildCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-guild-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const guildName = card.getAttribute('data-guild-name') || '';
    const visible = guildName.includes(value);
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('guild-filter-status', 'guild-filter-empty', visibleCount, 'server');
}

function filterModuleCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-module-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const moduleName = card.getAttribute('data-module-name') || '';
    const visible = moduleName.includes(value);
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('module-filter-status', 'module-filter-empty', visibleCount, 'module');
}

function filterCommandCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-command-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const commandName = card.getAttribute('data-command-name') || '';
    const category = window.__activeCommandCategory || 'all';
    const categoryMatch = category === 'all' || card.getAttribute('data-command-category') === category;
    const visible = commandName.includes(value) && categoryMatch;
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('command-filter-status', 'command-filter-empty', visibleCount, 'command');
}

function updateFilterFeedback(statusId, emptyId, visibleCount, itemLabel) {
  const status = document.getElementById(statusId);
  const empty = document.getElementById(emptyId);
  const noun = `${itemLabel}${visibleCount === 1 ? '' : 's'}`;
  if (status) status.textContent = `Showing ${visibleCount} ${noun}.`;
  if (empty) empty.hidden = visibleCount !== 0;
}

function setCommandCategory(category, button) {
  window.__activeCommandCategory = category;
  document.querySelectorAll('.command-category-row .command-tab, .command-category-row .tab-button').forEach((item) => {
    item.classList.remove('active');
    item.setAttribute('aria-pressed', 'false');
  });
  if (button) {
    button.classList.add('active');
    button.setAttribute('aria-pressed', 'true');
  }
  const currentSearch = document.getElementById('command-filter');
  filterCommandCards(currentSearch ? currentSearch.value : '');
}
"#
}
