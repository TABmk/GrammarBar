import { validateCorrection, visible } from './edits.js';
const $ = id => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__?.core.invoke(command, args) ?? Promise.reject('Open the GrammarBar app.');
let busy = false, connected = false, signingIn = false, confirmingLogout = false, analyticsOn = true, view = 'editor', period = 30, insightsStale = true, confirmingClear = false, polling = null, tooltipOwner = null;
let provider = null, savedProvider = null, savedModel = '', providers = [];
const info = () => providers.find(p => p.id === provider);
const status = (message = '', error = false, settings = false) => {
  const node = $(settings ? 'settings-status' : 'status'); node.textContent = message; node.classList.toggle('error', error);
};
function sync() {
  $('fix-button').disabled = !busy && (!$('source').value.trim() || !connected);
  $('source').readOnly = busy;
  $('clear-button').disabled = busy || (!$('source').value && !$('result').value);
  $('settings-button').disabled = busy;
  $('copy-button').disabled = busy || !$('result').value;
  document.body.classList.toggle('busy', busy);
  $('fix-button').querySelector('span').textContent = busy ? 'Cancel' : 'Correct';
  $('fix-button').classList.toggle('primary', !busy); $('fix-button').classList.toggle('secondary', busy);
  $('connection-dot').classList.toggle('connected', connected);
  $('connect-button').textContent = signingIn ? 'Cancel' : confirmingLogout ? 'Sign Out' : connected ? 'Disconnect' : 'Connect';
  $('connect-button').classList.toggle('destructive', confirmingLogout);
  $('code-row').hidden = !(signingIn && provider === 'claude');
  const installed = !!info()?.installed;
  $('install-row').hidden = installed || !provider; $('account-row').hidden = !installed;
  $('model').disabled = !connected || !!$('model').dataset.loading;
}
function hideTip() { $('tooltip').hidden = true; tooltipOwner = null; }
function showTip(event) {
  const button = event.currentTarget;
  tooltipOwner = button;
  const tip = $('tooltip'); tip.textContent = button.dataset.explanation; tip.hidden = false;
  const rect = button.getBoundingClientRect();
  tip.style.left = `${Math.max(12, Math.min(rect.left, innerWidth - tip.offsetWidth - 12))}px`;
  const above = rect.top - tip.offsetHeight - 8;
  tip.style.top = `${above >= 12 ? above : Math.min(innerHeight - tip.offsetHeight - 12, rect.bottom + 8)}px`;
}
function render(value) {
  const fragment = document.createDocumentFragment();
  for (const s of value.segments) {
    if (s.original === s.replacement) { fragment.append(document.createTextNode(s.original)); continue; }
    const button = document.createElement('button');
    button.className = 'change'; button.type = 'button'; button.dataset.explanation = s.explanation;
    button.setAttribute('aria-label', `${s.original ? `Removed: ${s.original}. ` : ''}${s.replacement ? `Added: ${s.replacement}. ` : ''}${s.explanation}`);
    for (const [tag, text] of [['del', s.original], ['ins', s.replacement]]) {
      if (text) { const element = document.createElement(tag); element.textContent = visible(text, tag === 'ins'); if (element.textContent !== text) element.className = 'whitespace'; button.append(element); }
    }
    button.addEventListener('mouseenter', showTip); button.addEventListener('focus', showTip);
    button.addEventListener('click', showTip); button.addEventListener('mouseleave', () => { if (document.activeElement !== button) hideTip(); });
    button.addEventListener('blur', hideTip); fragment.append(button);
  }
  hideTip(); $('changes').replaceChildren(fragment); $('result').value = value.text;
  $('changes-section').hidden = value.count === 0;
  $('change-count').textContent = value.count || '';
  if (value.failed) status(`${value.failed} of ${value.parts} parts could not be checked and were left unchanged. Try again to check them.`, true);
  else status(value.count ? '' : 'No changes');
}
function clearResult() { $('changes-section').hidden = true; $('result').value = ''; $('changes').textContent = '—'; $('change-count').textContent = ''; hideTip(); status(); sync(); }
async function fix() {
  if (busy) { await invoke('cancel').catch(e => status(String(e), true)); return; }
  if (!connected || !$('source').value.trim()) return;
  const source = $('source').value;
  if ([...source].length > 20000) { status('Maximum 20,000 characters', true); return; }
  clearResult(); busy = true; sync(); status('Correcting…');
  const progress = window.__TAURI__ ? new window.__TAURI__.core.Channel() : null;
  if (progress) progress.onmessage = ({ done, total }) => { if (busy && total > 1) status(`Correcting… ${done}/${total}`); };
  try { render(validateCorrection(source, await invoke('correct', { text: source, progress }))); insightsStale = true; }
  catch (error) { status(String(error) === 'Cancelled' ? '' : String(error), String(error) !== 'Cancelled'); }
  finally { busy = false; sync(); }
}
const signedOutLabel = () => signingIn ? 'Finish signing in in your browser' : 'Not connected';
async function refreshAccount() {
  const wasConnected = connected, current = provider;
  if (!info()?.installed) { connected = false; sync(); return; }
  const value = await invoke('account', { provider: current });
  if (current !== provider) return;
  connected = !!value;
  $('account-label').textContent = connected ? `${value.email || 'Connected'}${current === 'claude' ? ' · via Claude Code' : ''}` : signedOutLabel();
  if (connected && signingIn) { signingIn = false; clearTimeout(polling); status('', false, true); }
  sync();
  if (connected && !wasConnected) await loadModels();
}
async function pollAccount(deadline) {
  if (!signingIn) return;
  try { await refreshAccount(); }
  catch (error) { signingIn = false; status(String(error), true, true); sync(); return; }
  if (!signingIn) return;
  if (Date.now() > deadline) { stopSignIn(); status('Sign-in timed out', true, true); return; }
  polling = setTimeout(() => pollAccount(deadline), 1500);
}
function stopSignIn() {
  if (!signingIn) return;
  signingIn = false; clearTimeout(polling); invoke('cancel_login').catch(() => {});
  $('account-label').textContent = 'Not connected'; $('login-code').value = ''; sync();
}
$('connect-button').addEventListener('click', async () => {
  $('connect-button').disabled = true; status('', false, true);
  try {
    if (signingIn) stopSignIn();
    else if (connected && provider === 'claude' && !confirmingLogout) {
      // Claude Code's sign-in is shared with the terminal: ask before signing out.
      confirmingLogout = true; status('This also signs Claude Code out in Terminal. Click Sign Out to confirm.', false, true);
    }
    else if (connected) { confirmingLogout = false; await invoke('logout', { provider }); connected = false; $('account-label').textContent = 'Not connected'; }
    else {
      const url = await invoke('login', { provider });
      $('reopen-button').hidden = !url; $('reopen-button').dataset.url = url || '';
      signingIn = true; $('account-label').textContent = signedOutLabel();
      polling = setTimeout(() => pollAccount(Date.now() + 300000), 1500);
    }
  } catch (error) { status(String(error), true, true); }
  finally { $('connect-button').disabled = false; sync(); }
});
$('code-button').addEventListener('click', async () => {
  const code = $('login-code').value.trim();
  if (!code) { $('login-code').focus(); return; }
  try { await invoke('submit_login_code', { code }); $('login-code').value = ''; status('Checking the code…', false, true); }
  catch (error) { status(String(error), true, true); }
});
$('login-code').addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); $('code-button').click(); } });
$('reopen-button').addEventListener('click', () => invoke('open_sign_in', { url: $('reopen-button').dataset.url }).catch(e => status(String(e), true, true)));
$('install-button').addEventListener('click', () => invoke('open_install', { provider }).catch(e => status(String(e), true, true)));
$('recheck-button').addEventListener('click', async () => {
  $('recheck-button').disabled = true;
  try { await loadProviders(true); status(info()?.installed ? '' : `${info()?.name} was not found`, !info()?.installed, true); await refreshAccount(); }
  catch (error) { status(String(error), true, true); }
  finally { $('recheck-button').disabled = false; }
});
async function loadProviders(refresh) {
  providers = await invoke('providers', { refresh });
  const select = $('provider');
  select.replaceChildren(...providers.map(p => new Option(p.installed ? p.name : `${p.name} (not installed)`, p.id)));
  if (!providers.some(p => p.id === provider)) provider = (providers.find(p => p.installed) || providers[0])?.id ?? null;
  select.value = provider; renderProvider();
}
function renderProvider() {
  const current = info(); if (!current) return;
  $('install-title').textContent = `${current.name} is not installed`;
  $('account-title').textContent = current.id === 'claude' ? 'Claude account' : 'ChatGPT account';
  sync();
}
$('provider').addEventListener('change', async () => {
  stopSignIn(); confirmingLogout = false; status('', false, true);
  provider = $('provider').value; connected = false;
  $('model').replaceChildren(defaultModel()); $('account-label').textContent = 'Not connected';
  if (provider === savedProvider && savedModel) { $('model').add(new Option(savedModel, savedModel)); $('model').value = savedModel; }
  renderProvider();
  try { await refreshAccount(); } catch (error) { status(String(error), true, true); }
});
// Claude's default is Sonnet: as accurate as Opus for this task and much faster.
const defaultModel = () => new Option(provider === 'claude' ? 'Sonnet (recommended)' : 'Default', '');
async function loadModels() {
  const select = $('model');
  if (!connected || select.dataset.loading) return;
  const selected = select.value, current = provider;
  select.disabled = true; select.dataset.loading = 'true';
  try {
    const models = await invoke('models', { provider: current });
    if (current !== provider) return;
    const options = models.filter(m => typeof m.id === 'string').map(m => {
      const option = new Option(m.name || m.id, m.id); if (m.description) option.title = m.description; return option;
    });
    select.replaceChildren(defaultModel(), ...options);
    if (selected && !options.some(o => o.value === selected)) {
      const unavailable = new Option(`${selected} (unavailable)`, selected); unavailable.disabled = true; select.add(unavailable);
    }
    select.value = selected;
  } catch (error) { status(String(error), true, true); }
  finally { delete select.dataset.loading; sync(); }
}
function openSettings() {
  if (!busy) {
    document.body.classList.add('settings-open'); $('settings-dialog').showModal();
    // Like a native pane: no control is highlighted until the keyboard is used.
    $('settings-form').focus();
    loadModels();
  }
}
$('settings-button').addEventListener('click', openSettings);
$('settings-dialog').addEventListener('close', () => {
  document.body.classList.remove('settings-open'); if (view === 'editor') $('source').focus();
  if (confirmingClear) { confirmingClear = false; renderClear(); }
  if (confirmingLogout) { confirmingLogout = false; status('', false, true); sync(); }
  // Closing without saving keeps the provider that corrections actually use.
  if (savedProvider && provider !== savedProvider) { $('provider').value = savedProvider; $('provider').dispatchEvent(new Event('change')); }
});
$('close-settings').addEventListener('click', () => $('settings-dialog').close());
$('settings-form').addEventListener('submit', async event => {
  event.preventDefault(); $('save-button').disabled = true;
  try { if (!provider) throw 'Choose a provider.'; await invoke('save_settings', { prompt: $('prompt').value, model: $('model').value || null, provider, speed: $('speed').value }); savedProvider = provider; savedModel = $('model').value; status('', false, true); $('settings-dialog').close(); }
  catch (error) { status(String(error), true, true); }
  finally { $('save-button').disabled = false; }
});
// Appearance and login item apply right away, like native switches.
$('appearance').addEventListener('change', () => invoke('set_appearance', { appearance: $('appearance').value }).catch(e => status(String(e), true, true)));
$('launch-at-login').addEventListener('change', async () => {
  const box = $('launch-at-login');
  try { await invoke('set_launch_at_login', { enabled: box.checked }); }
  catch (error) { box.checked = !box.checked; status(String(error), true, true); }
});
$('source').addEventListener('input', clearResult);
$('clear-button').addEventListener('click', () => { $('source').value = ''; clearResult(); $('source').focus(); });
$('fix-button').addEventListener('click', fix);
$('copy-button').addEventListener('click', async () => {
  try { await invoke('copy', { text: $('result').value }); status('Copied'); }
  catch (error) { status(String(error), true); }
});
document.addEventListener('pointerdown', event => { if (event.target === document.body || event.target === document.documentElement) invoke('hide').catch(() => {}); });
window.addEventListener('blur', hideTip);
document.addEventListener('keydown', event => {
  if (event.key === 'Escape' && tooltipOwner) { event.preventDefault(); hideTip(); return; }
  if ($('settings-dialog').open) return;
  if (event.metaKey && event.key === 'Enter') { event.preventDefault(); fix(); }
  if (event.metaKey && event.key === ',') { event.preventDefault(); openSettings(); }
  if (event.metaKey && analyticsOn && (event.key === '1' || event.key === '2')) { event.preventDefault(); setView(event.key === '1' ? 'editor' : 'insights'); }
  if (event.key === 'Escape') { event.preventDefault(); invoke('hide').catch(() => {}); }
});
$('changes').addEventListener('scroll', hideTip); window.addEventListener('resize', hideTip);
async function initialize() {
  try {
    const saved = await invoke('settings'); $('prompt').value = saved.prompt; $('speed').value = saved.speed || 'fast';
    $('appearance').value = saved.appearance || 'system';
    analyticsOn = saved.analytics !== false; $('analytics').checked = analyticsOn; renderAnalyticsSwitch();
    invoke('launch_at_login').then(on => { $('launch-at-login').checked = on; }).catch(() => { $('launch-at-login').disabled = true; }); provider = savedProvider = saved.provider; savedModel = saved.model || '';
    if (saved.model) { $('model').add(new Option(saved.model, saved.model)); $('model').value = saved.model; }
    await loadProviders(false);
    if (saved.provider) await refreshAccount();
    if (!connected) openSettings();
  } catch (error) { status(String(error), true); }
  sync();
}
// Insights: local statistics of the user's mistakes.
const CATEGORY_NAMES = {
  articles: 'Articles', prepositions: 'Prepositions', verb_forms: 'Verb forms & tense', agreement: 'Subject–verb agreement',
  plurals: 'Singular & plural', pronouns: 'Pronouns', word_order: 'Word order', word_choice: 'Word choice', spelling: 'Spelling',
  punctuation: 'Punctuation', capitalization: 'Capitalization', spacing: 'Spacing', clarity: 'Clarity', other: 'Other',
};
const categoryName = id => CATEGORY_NAMES[id] || CATEGORY_NAMES.other;
const PERIOD_NAMES = { 7: 'the previous 7 days', 30: 'the previous 30 days' };
const element = (tag, className, text) => { const node = document.createElement(tag); if (className) node.className = className; if (text !== undefined) node.textContent = text; return node; };
const number = value => value.toLocaleString('en-US');
const decimal = value => value.toLocaleString('en-US', { maximumFractionDigits: 1, minimumFractionDigits: value < 10 ? 1 : 0 });
// Fewer mistakes is better, so a falling rate is shown as good news.
function trend(rate, previous) {
  if (rate == null || previous == null || previous === 0 && rate === 0) return null;
  const change = previous === 0 ? 1 : (rate - previous) / previous;
  if (Math.abs(change) < 0.05) return { text: 'Same as before', className: '' };
  return { text: `${change < 0 ? '↓' : '↑'} ${Math.min(999, Math.round(Math.abs(change) * 100))}%`, className: change < 0 ? 'good' : 'bad' };
}
function sparkline(series) {
  const svg = $('sparkline'); svg.replaceChildren();
  const points = series.map((value, index) => [index, value]).filter(([, value]) => value != null);
  if (points.length < 2) return;
  const max = Math.max(...points.map(([, value]) => value), 0.1), step = 100 / Math.max(1, series.length - 1);
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'polyline');
  path.setAttribute('points', points.map(([index, value]) => `${(index * step).toFixed(2)},${(22 - value / max * 20).toFixed(2)}`).join(' '));
  path.setAttribute('vector-effect', 'non-scaling-stroke'); svg.append(path);
}
// "a → an" as a strike-through and an insertion; a lone insertion or deletion
// (often one comma) is too small to see, so it is spelled out.
function fixNode(original, replacement) {
  const fix = element('span', 'fix');
  if (!original) fix.append(element('span', 'fix-note', 'Missing '), element('ins', '', `“${replacement}”`));
  else if (!replacement) fix.append(element('span', 'fix-note', 'Extra '), element('del', '', `“${original}”`));
  else fix.append(element('del', '', original), element('ins', '', replacement));
  return fix;
}
function renderInsights(data) {
  const enough = data.checks >= 3;
  $('insights-empty').hidden = enough; $('insights-body').hidden = !enough;
  $('insights-caption').textContent = data.checks ? `${number(data.checks)} ${data.checks === 1 ? 'check' : 'checks'}` : '';
  if (!enough) return;
  $('tile-rate').textContent = data.rate == null ? '—' : decimal(data.rate);
  const overall = trend(data.rate, data.previousRate);
  $('tile-rate-trend').textContent = overall ? `${overall.text}${overall.text.startsWith('Same') ? '' : ` vs ${PERIOD_NAMES[period] || 'before'}`}` : '';
  $('tile-rate-trend').className = `tile-sub ${overall?.className || ''}`;
  sparkline(data.series);
  $('tile-clean').textContent = data.cleanShare == null ? '—' : `${Math.round(data.cleanShare * 100)}%`;
  $('tile-checks').textContent = `${number(data.errors)} ${data.errors === 1 ? 'mistake' : 'mistakes'} found`;
  $('tile-words').textContent = number(data.words);
  $('tile-style').textContent = data.styleCount ? `+ ${number(data.styleCount)} style ${data.styleCount === 1 ? 'suggestion' : 'suggestions'}` : 'No style suggestions';
  const max = Math.max(1, ...data.categories.map(c => c.count));
  $('categories').replaceChildren(...(data.categories.length ? data.categories.slice(0, 6).map(c => {
    const row = element('div', 'bar-row');
    const fill = element('span', 'bar-fill'); fill.style.width = `${Math.max(3, c.count / max * 100)}%`;
    const bar = element('span', 'bar'); bar.append(fill);
    const change = trend(c.rate, c.previousRate);
    const count = element('span', 'bar-count', number(c.count));
    if (change && change.className) { const arrow = element('span', `bar-trend ${change.className}`, change.text.slice(0, 1)); arrow.title = `${change.text} vs ${PERIOD_NAMES[period] || 'before'}`; count.prepend(arrow); }
    row.append(element('span', 'bar-label', categoryName(c.id)), bar, count);
    return row;
  }) : [element('p', 'muted', 'No mistakes in this period.')]));
  $('style-line').hidden = !data.style.length;
  $('style-line').textContent = data.style.length ? `Style suggestions: ${data.style.slice(0, 3).map(s => `${categoryName(s.id).toLowerCase()} ${s.count}`).join(' · ')}` : '';
  $('example').hidden = !data.example;
  if (data.example) {
    const fix = fixNode(data.example.original, data.example.replacement);
    $('example').replaceChildren(element('span', 'example-title', `Latest ${categoryName(data.categories[0].id).toLowerCase()} mistake`), fix, element('span', 'example-text', data.example.explanation));
  }
  $('repeated').replaceChildren(...(data.repeated.length ? data.repeated.map(item => {
    const row = element('li'); row.title = item.explanation;
    row.append(fixNode(item.original, item.replacement), element('span', 'repeat-category', categoryName(item.category)), element('span', 'repeat-count', `×${item.count}`));
    return row;
  }) : [element('p', 'muted', 'Nothing repeated yet. The same fix made twice shows up here.')]));
}
async function loadInsights() {
  try { renderInsights(await invoke('analytics', { days: period })); insightsStale = false; }
  catch (error) { $('insights-caption').textContent = String(error); }
}
function setView(next) {
  view = analyticsOn ? next : 'editor';
  $('editor-view').hidden = view !== 'editor'; $('insights-view').hidden = view !== 'insights';
  for (const tab of $('view-switch').querySelectorAll('button')) tab.setAttribute('aria-selected', String(tab.dataset.view === view));
  hideTip();
  if (view === 'insights' && insightsStale) loadInsights();
  if (view === 'editor') $('source').focus();
}
for (const tab of $('view-switch').querySelectorAll('button')) tab.addEventListener('click', () => setView(tab.dataset.view));
for (const button of $('period').querySelectorAll('button')) button.addEventListener('click', () => {
  period = button.dataset.days ? Number(button.dataset.days) : null;
  for (const other of $('period').querySelectorAll('button')) other.setAttribute('aria-pressed', String(other === button));
  loadInsights();
});
function renderAnalyticsSwitch() {
  $('view-switch').hidden = !analyticsOn;
  if (!analyticsOn && view !== 'editor') setView('editor');
}
function renderClear() {
  $('clear-analytics').textContent = confirmingClear ? 'Delete' : 'Clear Data';
  $('clear-analytics').classList.toggle('destructive', confirmingClear);
}
$('analytics').addEventListener('change', async () => {
  const box = $('analytics');
  try {
    await invoke('set_analytics', { enabled: box.checked }); analyticsOn = box.checked; renderAnalyticsSwitch();
    status(analyticsOn ? '' : 'Statistics are no longer recorded. Clear Data deletes what was saved.', false, true);
  } catch (error) { box.checked = !box.checked; status(String(error), true, true); }
});
$('clear-analytics').addEventListener('click', async () => {
  if (!confirmingClear) { confirmingClear = true; renderClear(); status('This deletes all saved statistics. Click Delete to confirm.', false, true); return; }
  confirmingClear = false; renderClear();
  try { await invoke('clear_analytics'); insightsStale = true; status('Statistics deleted', false, true); }
  catch (error) { status(String(error), true, true); }
});

initialize();

for (const handle of document.querySelectorAll('.resize-handle')) {
  let drag = null;
  const resize = height => invoke('resize_height', { height }).catch(error => status(String(error), true, $('settings-dialog').open));
  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    event.preventDefault();
    drag = { y: event.screenY, height: innerHeight };
    handle.setPointerCapture(event.pointerId);
  });
  handle.addEventListener('pointermove', event => {
    if (drag) resize(drag.height + event.screenY - drag.y);
  });
  handle.addEventListener('pointerup', () => { drag = null; });
  handle.addEventListener('lostpointercapture', () => { drag = null; });
  handle.addEventListener('keydown', event => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault(); resize(innerHeight + (event.key === 'ArrowDown' ? 40 : -40));
    }
  });
}
