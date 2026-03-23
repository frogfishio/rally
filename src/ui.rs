// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

/// Returns the embedded HTML/JS dashboard page.
pub fn dashboard_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Rally — Rally your services</title>
<style>
  :root {
    --bg: #0f1117;
    --surface: #1a1d27;
    --border: #2a2d3a;
    --accent: #00c4a7;
    --accent2: #7c6af7;
    --text: #e2e4f0;
    --text-dim: #6b6f8a;
    --green: #22c55e;
    --red: #ef4444;
    --orange: #f97316;
    --yellow: #eab308;
    --font: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    background: var(--bg);
    color: var(--text);
    font-family: var(--font);
    font-size: 13px;
    min-height: 100vh;
  }
  header {
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    padding: 14px 24px;
    display: flex;
    align-items: center;
    gap: 16px;
  }
  header h1 {
    font-size: 18px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.04em;
    text-transform: lowercase;
  }
  header .subtitle {
    color: var(--text-dim);
    font-size: 12px;
  }
  header .conn-badge {
    margin-left: auto;
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
    background: var(--border);
    color: var(--text-dim);
    transition: all .3s;
  }
  header .conn-badge.connected { background: #052; color: var(--green); }
  header .conn-badge.error { background: #400; color: var(--red); }
  header .header-btn {
    padding: 5px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: var(--font);
    font-size: 11px;
    cursor: pointer;
  }
  header .header-btn:hover { background: var(--border); }

  main { padding: 24px; display: flex; flex-direction: column; gap: 20px; }

  /* Summary bar */
  .summary-bar {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .summary-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 14px 22px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 110px;
  }
  .summary-card .count { font-size: 28px; font-weight: 700; }
  .summary-card .label { color: var(--text-dim); font-size: 11px; text-transform: uppercase; letter-spacing: .06em; }
  .count.running { color: var(--green); }
  .count.stopped { color: var(--red); }
  .count.healthy { color: var(--accent); }
  .count.unhealthy { color: var(--orange); }

  /* Process grid */
  .process-list { display: flex; flex-direction: column; gap: 12px; }

  .process-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    transition: border-color .2s;
  }
  .process-card.selected { border-color: var(--accent2); }

  .process-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 18px;
    cursor: pointer;
    user-select: none;
  }
  .process-header:hover { background: rgba(255,255,255,0.03); }

  .state-dot {
    width: 10px; height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--text-dim);
  }
  .state-dot.running { background: var(--green); box-shadow: 0 0 8px var(--green); animation: pulse 2s infinite; }
  .state-dot.pending { background: var(--yellow); }
  .state-dot.installing { background: var(--accent); box-shadow: 0 0 8px var(--accent); }
  .state-dot.disabled { background: var(--text-dim); }
  .state-dot.external { background: var(--accent2); box-shadow: 0 0 8px var(--accent2); }
  .state-dot.failed  { background: var(--red); }
  .state-dot.killed  { background: var(--text-dim); }
  .state-dot.exited  { background: var(--orange); }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50%       { opacity: 0.45; }
  }

  .process-name { font-weight: 700; font-size: 14px; flex: 1; }
  .process-cmd  { color: var(--text-dim); font-size: 11px; flex: 2; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .process-cmd a,
  .info-table a {
    color: var(--accent);
    text-decoration: none;
  }
  .process-cmd a:hover,
  .info-table a:hover {
    text-decoration: underline;
  }

  .badge {
    padding: 2px 9px;
    border-radius: 999px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: .04em;
  }
  .badge.running   { background: #052; color: var(--green); }
  .badge.pending   { background: #440; color: var(--yellow); }
  .badge.installing { background: #033; color: var(--accent); }
  .badge.disabled  { background: #222; color: var(--text-dim); }
  .badge.external  { background: #24194d; color: #b7a8ff; }
  .badge.failed    { background: #400; color: var(--red); }
  .badge.killed    { background: #222; color: var(--text-dim); }
  .badge.exited    { background: #320; color: var(--orange); }
  .badge.healthy   { background: #034; color: var(--accent); }
  .badge.unhealthy { background: #320; color: var(--orange); }
  .badge.unknown   { background: #222; color: var(--text-dim); }
  .badge.not_configured { background: #1a1d27; color: var(--text-dim); }

  .process-meta { color: var(--text-dim); font-size: 11px; white-space: nowrap; }

  .process-actions { display: flex; gap: 6px; }
  .btn {
    padding: 4px 12px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: var(--font);
    font-size: 11px;
    cursor: pointer;
    transition: background .2s, color .2s;
  }
  .btn:hover { background: var(--border); }
  .btn.danger { border-color: var(--red); color: var(--red); }
  .btn.danger:hover { background: #400; }
  .btn.primary { border-color: var(--accent); color: var(--accent); }
  .btn.primary:hover { background: #054; }

  /* Detail / log panel */
  .process-detail {
    display: none;
    border-top: 1px solid var(--border);
    padding: 14px 18px;
  }
  .process-detail.open { display: block; }

  .detail-tabs {
    display: flex;
    gap: 2px;
    margin-bottom: 12px;
  }
  .tab-btn {
    padding: 5px 14px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: var(--text-dim);
    font-family: var(--font);
    font-size: 12px;
    cursor: pointer;
    transition: background .15s, color .15s;
  }
  .tab-btn:hover { background: var(--border); color: var(--text); }
  .tab-btn.active { background: var(--accent2); color: #fff; }

  .tab-panel { display: none; }
  .tab-panel.active { display: block; }

  /* Info table */
  .info-table { width: 100%; border-collapse: collapse; }
  .info-table td { padding: 5px 0; vertical-align: top; }
  .info-table td:first-child { color: var(--text-dim); padding-right: 24px; width: 150px; white-space: nowrap; }

  /* Log viewer */
  .log-viewer {
    background: #0a0c14;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 10px 14px;
    height: 300px;
    overflow-y: auto;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .log-line { display: flex; gap: 10px; }
  .log-ts  { color: var(--text-dim); flex-shrink: 0; }
  .log-stream.stderr { color: var(--orange); }
  .log-stream.stdout { color: var(--text-dim); }
  .log-text { color: var(--text); }

  .log-controls { display: flex; gap: 8px; align-items: center; margin-bottom: 8px; }
  .log-filter { flex: 1; }
  .log-filter input {
    width: 100%;
    padding: 5px 10px;
    background: #0a0c14;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-family: var(--font);
    font-size: 12px;
    outline: none;
  }
  .log-filter input:focus { border-color: var(--accent2); }
  .autoscroll-label { color: var(--text-dim); font-size: 11px; display: flex; align-items: center; gap: 4px; }

  .env-list { display: flex; flex-direction: column; gap: 4px; }
  .env-entry {
    display: flex;
    gap: 8px;
    font-size: 12px;
    padding: 4px 0;
    border-bottom: 1px solid var(--border);
  }
  .env-key { color: var(--accent); min-width: 200px; word-break: break-all; }
  .env-val { color: var(--text-dim); word-break: break-all; }

  .empty-msg { color: var(--text-dim); font-style: italic; }
</style>
</head>
<body>
<header>
  <h1>▶ Rally</h1>
  <span class="subtitle">Rally your services</span>
  <button class="header-btn" onclick="reloadConfig()">reload config</button>
  <span class="conn-badge" id="conn-badge">connecting…</span>
</header>
<main>
  <div class="summary-bar" id="summary-bar"></div>
  <div class="process-list" id="process-list">
    <p style="color:var(--text-dim)">Loading…</p>
  </div>
</main>

<script>
// ── State ──────────────────────────────────────────────────────────────────
let processes = [];
let selectedName = null;
let activeTab = {};   // name -> tab id
let autoscroll = {};  // name -> bool
let logFilter = {};   // name -> string
let envMode = {};     // name -> managed | all
let pollInterval = null;

// ── API ────────────────────────────────────────────────────────────────────
async function fetchStatus() {
  const res = await fetch('/api/status');
  if (!res.ok) throw new Error('API error ' + res.status);
  return res.json();
}

async function actionKill(name) {
  await fetch('/api/kill/' + encodeURIComponent(name), { method: 'POST' });
}

async function actionRestart(name) {
  await fetch('/api/restart/' + encodeURIComponent(name), { method: 'POST' });
}

async function actionEnable(name) {
  await fetch('/api/enable/' + encodeURIComponent(name), { method: 'POST' });
}

async function actionDisable(name) {
  await fetch('/api/disable/' + encodeURIComponent(name), { method: 'POST' });
}

async function actionReload() {
  const res = await fetch('/api/reload', { method: 'POST' });
  if (!res.ok) throw new Error('Reload failed ' + res.status);
}

// ── Render ─────────────────────────────────────────────────────────────────
function stateClass(state) {
  if (typeof state === 'object') return 'exited';
  return state;
}

function stateLabel(state) {
  if (typeof state === 'object' && state.exited !== undefined) return 'exited(' + state.exited + ')';
  if (state === 'exited') return 'exited';
  return state;
}

function healthClass(h) { return h.toLowerCase().replace(' ', '_'); }

function fmtTs(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  return d.toLocaleTimeString() + ' ' + d.toLocaleDateString();
}

function elapsedSince(iso) {
  if (!iso) return '—';
  const secs = Math.floor((Date.now() - new Date(iso)) / 1000);
  if (secs < 60) return secs + 's';
  if (secs < 3600) return Math.floor(secs/60) + 'm ' + (secs%60) + 's';
  return Math.floor(secs/3600) + 'h ' + Math.floor((secs%3600)/60) + 'm';
}

function renderSummary(procs) {
  const running   = procs.filter(p => stateClass(p.state) === 'running').length;
  const stopped   = procs.filter(p => stateClass(p.state) !== 'running').length;
  const healthy   = procs.filter(p => p.health === 'healthy').length;
  const unhealthy = procs.filter(p => p.health === 'unhealthy').length;
  document.getElementById('summary-bar').innerHTML = `
    <div class="summary-card"><div class="count">${procs.length}</div><div class="label">Total</div></div>
    <div class="summary-card"><div class="count running">${running}</div><div class="label">Running</div></div>
    <div class="summary-card"><div class="count stopped">${stopped}</div><div class="label">Stopped</div></div>
    <div class="summary-card"><div class="count healthy">${healthy}</div><div class="label">Healthy</div></div>
    <div class="summary-card"><div class="count unhealthy">${unhealthy}</div><div class="label">Unhealthy</div></div>
  `;
}

function renderLogs(proc) {
  const filter = (logFilter[proc.name] || '').toLowerCase();
  const lines = proc.logs.filter(l => !filter || l.text.toLowerCase().includes(filter));
  return lines.map(l => {
    const ts  = new Date(l.timestamp).toISOString().substring(11, 23);
    const txt = escHtml(l.text);
    return `<div class="log-line"><span class="log-ts">${ts}</span><span class="log-stream ${l.stream}">[${l.stream}]</span><span class="log-text">${txt}</span></div>`;
  }).join('');
}

function escHtml(s) {
  return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function isHttpUrl(value) {
  return typeof value === 'string' && /^(https?:\/\/)/i.test(value);
}

function renderAccessValue(access) {
  if (!access) return '—';
  if (isHttpUrl(access)) {
    const safeUrl = escAttr(access);
    return `<a href="${safeUrl}" target="_blank" rel="noopener noreferrer" onclick="event.stopPropagation()">${escHtml(access)}</a>`;
  }
  return escHtml(access);
}

function renderEnv(env) {
  const entries = Object.entries(env || {});
  if (!entries.length) return '<p class="empty-msg">No environment variables configured.</p>';
  return '<div class="env-list">' + entries.map(([k,v]) =>
    `<div class="env-entry"><span class="env-key">${escHtml(k)}</span><span class="env-val">${escHtml(v)}</span></div>`
  ).join('') + '</div>';
}

function getEnvMode(name) { return envMode[name] || 'managed'; }

function renderEnvPanel(proc) {
  const mode = getEnvMode(proc.name);
  const visibleEnv = mode === 'all' ? (proc.env || {}) : (proc.managed_env || {});
  const managedCount = Object.keys(proc.managed_env || {}).length;
  const totalCount = Object.keys(proc.env || {}).length;
  const ambientCount = Math.max(0, totalCount - managedCount);

  return `
    <div class="log-controls">
      <div class="process-meta">showing ${mode === 'all' ? totalCount : managedCount} env vars${mode === 'managed' && ambientCount ? `, hiding ${ambientCount} ambient` : ''}</div>
      <div class="process-actions">
        <button class="btn${mode === 'managed' ? ' primary' : ''}" onclick="setEnvMode('${escAttr(proc.name)}','managed')">managed</button>
        <button class="btn${mode === 'all' ? ' primary' : ''}" onclick="setEnvMode('${escAttr(proc.name)}','all')">all</button>
      </div>
    </div>
    ${renderEnv(visibleEnv)}
  `;
}

function renderInfoTable(proc) {
  const sc = stateClass(proc.state);
  const watchPaths = (proc.watch_paths || []).length
    ? proc.watch_paths.map(escHtml).join('<br>')
    : '—';
  const envProvider = proc.env_provider || null;
  const envProviderLoadedAt = envProvider && envProvider.loaded_at
    ? fmtTs(envProvider.loaded_at)
    : '—';
  return `
  <table class="info-table">
    <tr><td>State</td><td><span class="badge ${sc}">${stateLabel(proc.state)}</span></td></tr>
    <tr><td>Enabled</td><td>${proc.enabled ? 'true' : 'false'}</td></tr>
    <tr><td>Env provider</td><td>${envProvider ? 'enabled' : 'disabled'}</td></tr>
    <tr><td>Provider status</td><td>${envProvider ? escHtml(envProvider.status) : '—'}</td></tr>
    <tr><td>Provider command</td><td>${envProvider ? escHtml(envProvider.command) : '—'}</td></tr>
    <tr><td>Provider format</td><td>${envProvider ? escHtml(envProvider.format) : '—'}</td></tr>
    <tr><td>Provider keys</td><td>${envProvider ? envProvider.key_count : '—'}</td></tr>
    <tr><td>Provider loaded</td><td>${envProviderLoadedAt}</td></tr>
    <tr><td>PID</td><td>${proc.pid || '—'}</td></tr>
    <tr><td>Restarts</td><td>${proc.restart_count}</td></tr>
    <tr><td>Last restart</td><td>${proc.last_restart_reason ? escHtml(proc.last_restart_reason) : '—'}</td></tr>
    <tr><td>Last error</td><td>${proc.last_error ? escHtml(proc.last_error) : '—'}</td></tr>
    <tr><td>Started</td><td>${fmtTs(proc.started_at)}</td></tr>
    <tr><td>Uptime</td><td>${proc.started_at && sc === 'running' ? elapsedSince(proc.started_at) : '—'}</td></tr>
    <tr><td>Exit time</td><td>${fmtTs(proc.exit_time)}</td></tr>
    <tr><td>Health</td><td><span class="badge ${healthClass(proc.health)}">${proc.health}</span></td></tr>
    <tr><td>Watching</td><td>${proc.watch_enabled ? 'enabled' : 'disabled'}</td></tr>
    <tr><td>Watch debounce</td><td>${proc.watch_debounce_millis ? proc.watch_debounce_millis + 'ms' : '—'}</td></tr>
    <tr><td>Watch paths</td><td>${watchPaths}</td></tr>
    <tr><td>Access</td><td>${renderAccessValue(proc.access)}</td></tr>
    <tr><td>Cargo</td><td>${proc.cargo ? escHtml(proc.cargo) : '—'}</td></tr>
    <tr><td>Command</td><td>${escHtml(proc.command)}</td></tr>
    <tr><td>Args</td><td>${proc.args.length ? proc.args.map(escHtml).join(' ') : '—'}</td></tr>
  </table>`;
}

function getTab(name) { return activeTab[name] || 'info'; }
function getAutoScroll(name) { return autoscroll[name] !== false; }

function renderCard(proc) {
  const sc    = stateClass(proc.state);
  const hc    = healthClass(proc.health);
  const sel   = proc.name === selectedName;
  const tab   = getTab(proc.name);
  const managedCount = Object.keys(proc.managed_env || {}).length;
  const totalCount = Object.keys(proc.env || {}).length;
  const cmdFull = proc.command + (proc.args.length ? ' ' + proc.args.join(' ') : '');
  const accessLabel = proc.access || cmdFull;
  const accessTitle = proc.access ? proc.access : cmdFull;
  const isEnabled = proc.enabled !== false;
  const isRunning = sc === 'running';

  return `
  <div class="process-card${sel ? ' selected' : ''}" id="card-${slugify(proc.name)}">
    <div class="process-header" onclick="toggleCard('${escAttr(proc.name)}')">
      <div class="state-dot ${sc}"></div>
      <div class="process-name">${escHtml(proc.name)}</div>
      <div class="process-cmd" title="${escAttr(accessTitle)}">${renderAccessValue(accessLabel)}</div>
      <span class="badge ${sc}">${stateLabel(proc.state)}</span>
      ${!isEnabled ? `<span class="badge disabled">disabled</span>` : ''}
      ${proc.health !== 'not_configured' ? `<span class="badge ${hc}">${proc.health}</span>` : ''}
      <div class="process-meta">${isRunning && proc.started_at ? elapsedSince(proc.started_at) : ''}</div>
      <div class="process-actions" onclick="event.stopPropagation()">
        ${isEnabled
          ? (isRunning
              ? `<button class="btn danger" onclick="killProc('${escAttr(proc.name)}')">kill</button>`
              : `<button class="btn primary" onclick="restartProc('${escAttr(proc.name)}')">start</button>`)
          : `<button class="btn primary" onclick="enableProc('${escAttr(proc.name)}')">enable</button>`}
        ${isEnabled ? `<button class="btn" onclick="restartProc('${escAttr(proc.name)}')">restart</button>` : ''}
        ${isEnabled ? `<button class="btn" onclick="disableProc('${escAttr(proc.name)}')">disable</button>` : ''}
      </div>
    </div>
    <div class="process-detail${sel ? ' open' : ''}" id="detail-${slugify(proc.name)}">
      <div class="detail-tabs">
        <button class="tab-btn${tab==='info'?' active':''}" onclick="switchTab('${escAttr(proc.name)}','info')">Info</button>
        <button class="tab-btn${tab==='logs'?' active':''}" onclick="switchTab('${escAttr(proc.name)}','logs')">Logs (${proc.logs.length})</button>
        <button class="tab-btn${tab==='env'?' active':''}" onclick="switchTab('${escAttr(proc.name)}','env')">Env (${managedCount}/${totalCount})</button>
      </div>
      <div class="tab-panel${tab==='info'?' active':''}" id="tab-info-${slugify(proc.name)}">
        ${renderInfoTable(proc)}
      </div>
      <div class="tab-panel${tab==='logs'?' active':''}" id="tab-logs-${slugify(proc.name)}">
        <div class="log-controls">
          <div class="log-filter"><input type="text" placeholder="filter logs…" value="${escAttr(logFilter[proc.name]||'')}" oninput="setLogFilter('${escAttr(proc.name)}', this.value)"></div>
          <label class="autoscroll-label"><input type="checkbox" ${getAutoScroll(proc.name)?'checked':''} onchange="setAutoScroll('${escAttr(proc.name)}', this.checked)"> auto-scroll</label>
          <button class="btn" onclick="clearLogs('${escAttr(proc.name)}')">clear</button>
        </div>
        <div class="log-viewer" id="logs-${slugify(proc.name)}">${renderLogs(proc)}</div>
      </div>
      <div class="tab-panel${tab==='env'?' active':''}" id="tab-env-${slugify(proc.name)}">
        ${renderEnvPanel(proc)}
      </div>
    </div>
  </div>`;
}

function slugify(name) { return name.replace(/[^a-zA-Z0-9]/g, '_'); }
function escAttr(s) { return s.replace(/"/g,'&quot;').replace(/'/g,'&#39;'); }

function renderAll(procs) {
  renderSummary(procs);
  const list = document.getElementById('process-list');
  list.innerHTML = procs.map(renderCard).join('');
  // After render, scroll logs if autoscroll is on
  procs.forEach(p => {
    if (getAutoScroll(p.name) && getTab(p.name) === 'logs') {
      const el = document.getElementById('logs-' + slugify(p.name));
      if (el) el.scrollTop = el.scrollHeight;
    }
  });
}

// ── Interactions ───────────────────────────────────────────────────────────
function toggleCard(name) {
  selectedName = selectedName === name ? null : name;
  refresh();
}

function switchTab(name, tab) {
  activeTab[name] = tab;
  refresh();
}

function setLogFilter(name, val) {
  logFilter[name] = val;
  refresh();
}

function setAutoScroll(name, val) {
  autoscroll[name] = val;
}

function setEnvMode(name, mode) {
  envMode[name] = mode;
  refresh();
}

async function clearLogs(name) {
  await fetch('/api/clear-logs/' + encodeURIComponent(name), { method: 'POST' });
  refresh();
}

async function killProc(name) {
  await actionKill(name);
  refresh();
}

async function restartProc(name) {
  await actionRestart(name);
  refresh();
}

async function enableProc(name) {
  await actionEnable(name);
  refresh();
}

async function disableProc(name) {
  await actionDisable(name);
  refresh();
}

async function reloadConfig() {
  await actionReload();
  refresh();
}

// ── Polling / SSE ──────────────────────────────────────────────────────────
const badge = document.getElementById('conn-badge');

function setConnected(ok) {
  badge.textContent = ok ? '● live' : '● disconnected';
  badge.className = 'conn-badge ' + (ok ? 'connected' : 'error');
}

async function refresh() {
  try {
    processes = await fetchStatus();
    renderAll(processes);
    setConnected(true);
  } catch(e) {
    setConnected(false);
  }
}

function startPolling() {
  refresh();
  if (pollInterval) clearInterval(pollInterval);
  pollInterval = setInterval(refresh, 2000);
}

// Try SSE first; fall back to polling
function connectSSE() {
  const es = new EventSource('/api/events');
  es.onopen  = () => setConnected(true);
  es.onmessage = () => refresh();
  es.onerror = () => {
    setConnected(false);
    es.close();
    startPolling();
  };
}

// Start
connectSSE();
refresh();
</script>
</body>
</html>"#
}
