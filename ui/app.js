import { validateCorrection } from './edits.js';
const $ = id => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__?.core.invoke(command, args) ?? Promise.reject('Open the GrammarBar app.');
let busy = false, connected = false, signingIn = false, polling = null, tooltipOwner = null;
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
  $('fix-button').querySelector('kbd').hidden = busy;
  $('fix-button').querySelector('use').setAttribute('href', busy ? '#i-stop' : '#i-arrow');
  $('connection-dot').classList.toggle('connected', connected);
  $('connect-button').textContent = signingIn ? 'Cancel' : connected ? 'Disconnect' : 'Connect';
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
      if (text) { const element = document.createElement(tag); element.textContent = text; button.append(element); }
    }
    button.addEventListener('mouseenter', showTip); button.addEventListener('focus', showTip);
    button.addEventListener('click', showTip); button.addEventListener('mouseleave', () => { if (document.activeElement !== button) hideTip(); });
    button.addEventListener('blur', hideTip); fragment.append(button);
  }
  hideTip(); $('changes').replaceChildren(fragment); $('result').value = value.text;
  $('changes-section').hidden = value.count === 0;
  $('change-count').textContent = value.count || ''; status(value.count ? '' : 'No changes');
}
function clearResult() { $('changes-section').hidden = true; $('result').value = ''; $('changes').textContent = '—'; $('change-count').textContent = ''; hideTip(); status(); sync(); }
async function fix() {
  if (busy) { await invoke('cancel').catch(e => status(String(e), true)); return; }
  if (!connected || !$('source').value.trim()) return;
  const source = $('source').value;
  if ([...source].length > 20000) { status('Maximum 20,000 characters', true); return; }
  clearResult(); busy = true; sync(); status('Correcting…');
  try { render(validateCorrection(source, await invoke('correct', { text: source }))); }
  catch (error) { status(String(error) === 'Cancelled' ? '' : String(error), String(error) !== 'Cancelled'); }
  finally { busy = false; sync(); }
}
async function refreshAccount() {
  const wasConnected = connected;
  const value = await invoke('account'); connected = !!value;
  $('account-label').textContent = connected ? value.email || 'Connected' : signingIn ? 'Finish signing in in your browser' : 'Not connected';
  if (connected && signingIn) { signingIn = false; clearTimeout(polling); status('', false, true); }
  sync();
  if (connected && !wasConnected) await loadModels();
}
async function pollAccount(deadline) {
  if (!signingIn) return;
  try { await refreshAccount(); }
  catch (error) { signingIn = false; status(String(error), true, true); sync(); return; }
  if (!signingIn) return;
  if (Date.now() > deadline) { signingIn = false; await invoke('cancel_login').catch(() => {}); $('account-label').textContent = 'Not connected'; status('Sign-in timed out', true, true); sync(); return; }
  polling = setTimeout(() => pollAccount(deadline), 1500);
}
$('connect-button').addEventListener('click', async () => {
  $('connect-button').disabled = true; status('', false, true);
  try {
    if (signingIn) { signingIn = false; clearTimeout(polling); await invoke('cancel_login'); $('account-label').textContent = 'Not connected'; }
    else if (connected) { await invoke('logout'); connected = false; $('model').disabled = true; $('account-label').textContent = 'Not connected'; }
    else { await invoke('login'); signingIn = true; $('account-label').textContent = 'Finish signing in in your browser'; polling = setTimeout(() => pollAccount(Date.now() + 180000), 1500); }
  } catch (error) { status(String(error), true, true); }
  finally { $('connect-button').disabled = false; sync(); }
});
async function loadModels() {
  const select = $('model');
  if (!connected || select.dataset.loading) return;
  const selected = select.value;
  select.disabled = true; select.dataset.loading = 'true';
  try {
    const models = await invoke('models');
    const options = models.filter(m => !m.hidden && typeof m.model === 'string' && (!m.inputModalities || m.inputModalities.includes('text')))
      .map(m => new Option(m.displayName || m.model, m.model));
    select.replaceChildren(new Option('Default', ''), ...options);
    if (selected && !options.some(o => o.value === selected)) {
      const unavailable = new Option(`${selected} (unavailable)`, selected); unavailable.disabled = true; select.add(unavailable);
    }
    select.value = selected;
  } catch (error) { status(String(error), true, true); }
  finally { select.disabled = !connected; delete select.dataset.loading; }
}
function openSettings() {
  if (!busy) {
    document.body.classList.add('settings-open'); $('settings-dialog').showModal(); $('close-settings').focus();
    loadModels();
  }
}
$('settings-button').addEventListener('click', openSettings);
$('settings-dialog').addEventListener('close', () => { document.body.classList.remove('settings-open'); $('source').focus(); });
$('close-settings').addEventListener('click', () => $('settings-dialog').close());
$('settings-form').addEventListener('submit', async event => {
  event.preventDefault(); $('save-button').disabled = true;
  try { await invoke('save_settings', { prompt: $('prompt').value, model: $('model').value || null }); status('', false, true); $('settings-dialog').close(); }
  catch (error) { status(String(error), true, true); }
  finally { $('save-button').disabled = false; }
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
  if (event.key === 'Escape') { event.preventDefault(); invoke('hide').catch(() => {}); }
});
$('changes').addEventListener('scroll', hideTip); window.addEventListener('resize', hideTip);
async function initialize() {
  try {
    const saved = await invoke('settings'); $('prompt').value = saved.prompt;
    if (saved.model) { $('model').add(new Option(saved.model, saved.model)); $('model').value = saved.model; }
    if (saved.connected) await refreshAccount();
    else openSettings();
  } catch (error) { status(String(error), true); }
  sync();
}
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
