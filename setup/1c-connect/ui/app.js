// Окно — тонкая оболочка: всё решает движок, страница показывает его состояние
// и зовёт команды. Логики подключения здесь нет намеренно.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const KEYS = ['server', 'workstation', 'password', 'base_path', 'db_user', 'db_password', 'base_url', 'dump'];
let busy = false;

function el(tag, text, children = [], cls = '') {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text;
  for (const c of children) e.append(c);
  return e;
}

function logLine(kind, text) {
  const log = $('log');
  log.append(el('div', text, [], kind));
  log.scrollTop = log.scrollHeight;
}

function showLink(link) {
  $('state').textContent = link.label;
  $('dot').className = 'dot' + (link.up ? ' up' : link.on ? ' busy' : '');
  document.querySelector('[data-do="connect"]').disabled = link.on;
  document.querySelector('[data-do="disconnect"]').disabled = !link.on;
}

function setBusy(on) {
  busy = on;
  for (const b of document.querySelectorAll('[data-do="install"],[data-do="uninstall"],[data-do="check"],[data-do="save"]')) b.disabled = on;
}

async function refresh() {
  const m = await invoke('machine');
  for (const k of KEYS) $(`f-${k}`).value = m.host[k];
  $('kit').textContent = `Комплект: ${m.kit}`;
  $('route').textContent = `${m.host.workstation} → ${m.host.server}; порты ${m.ports[0]} (mcp-1c) и ${m.ports[2]} (веб-клиент) · ${m.web_url}`;
  $('lead').textContent = !m.file_exists
    ? 'Параметров ещё нет: заполните и сохраните, затем установите расширение и подключайтесь.'
    : m.is_sample
      ? 'Адрес среды всё ещё из образца: впишите свой (его печатает dev1c-setup install).'
      : 'Туннель в контейнер и мост к mcp-1c. Окно можно свернуть — связь живёт, пока не нажать «Выход».';
  showLink(m.link);
  await refreshOps();
}

async function refreshOps() {
  const ops = await invoke('states');
  const ul = $('ops');
  ul.replaceChildren();
  for (const o of ops) {
    const mark = { done: '✓', missing: '·', drift: '!', blocked: '–' }[o.kind] || '·';
    ul.append(el('li', '', [el('span', mark, [], 'mark'), el('span', o.title, [el('small', o.label)], 'what')], o.kind));
  }
}

async function save() {
  const host = {};
  for (const k of KEYS) host[k] = $(`f-${k}`).value;
  $('save-hint').textContent = '';
  try {
    await invoke('save_cfg', { host });
    $('save-hint').textContent = 'Сохранено.';
    await refresh();
  } catch (e) {
    $('save-hint').textContent = String(e);
  }
}

async function check() {
  if (busy) return;
  setBusy(true);
  $('check-hint').textContent = 'Проверяю…';
  try {
    const checks = await invoke('check');
    const ul = $('checks');
    ul.replaceChildren();
    let failed = 0;
    for (const c of checks) {
      const cls = c.ok ? (c.info ? 'info' : 'pass') : 'fail';
      if (!c.ok) failed++;
      const what = el('span', c.what, [el('small', c.detail)], 'what');
      if (c.hint) what.append(el('span', c.hint, [], 'fix'));
      ul.append(el('li', '', [el('span', c.ok ? (c.info ? '·' : '✓') : '✗', [], 'mark'), what], cls));
    }
    $('check-hint').textContent = failed ? `Провалено: ${failed}. Отчёт — last-check.txt в комплекте.` : 'Всё в порядке.';
  } catch (e) {
    $('check-hint').textContent = String(e);
  }
  setBusy(false);
}

function run(which) {
  if (busy) return;
  setBusy(true);
  logLine('say', which === 'uninstall' ? '— снимаю расширение —' : '— устанавливаю расширение —');
  invoke('run', { which });
}

async function act(what) {
  try {
    switch (what) {
      case 'connect': await invoke('connect'); break;
      case 'disconnect': await invoke('disconnect'); break;
      case 'web': await invoke('open_web'); break;
      case 'save': await save(); break;
      case 'install': run('install'); break;
      case 'uninstall': run('uninstall'); break;
      case 'check': await check(); break;
      case 'hide': await invoke('hide'); break;
      case 'quit': await invoke('quit'); break;
    }
  } catch (e) {
    logLine('fail', String(e));
  }
}

document.addEventListener('click', (ev) => {
  const b = ev.target.closest('[data-do]');
  if (b) act(b.dataset.do);
});

listen('conn:line', (e) => logLine(e.payload.kind, e.payload.text));
listen('conn:state', (e) => showLink(e.payload));
listen('conn:done', async (e) => {
  logLine(e.payload ? 'fail' : 'ok', e.payload ? `Остановлено: ${e.payload}` : 'Готово.');
  setBusy(false);
  await refreshOps();
});
listen('conn:run-check', () => check());

refresh();
// «подключён N мин» — обновляется само.
setInterval(async () => showLink(await invoke('link')), 30000);
