// Окно — тонкая оболочка: всё решает движок, страница показывает его состояние
// и зовёт команды. Логики подключения здесь нет намеренно.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const KEYS = ['server', 'workstation', 'password', 'base_path', 'db_user', 'db_password', 'base_url', 'dump'];
let busy = false;
let unread = 0;

function el(tag, text, children = [], cls = '') {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text;
  for (const c of children) e.append(c);
  return e;
}

// Значок состояния: ✓ / ! / ✕ / · в кружке.
function icon(kind) {
  const glyph = { ok: '✓', warn: '!', bad: '✕', dim: '·' }[kind] || '·';
  return el('span', glyph, [], `ic ${kind}`);
}

function row(kind, title, detail, extra) {
  const text = el('div', '', [el('div', title, [], 't'), el('div', detail, [], 'd')]);
  if (extra) text.append(el('div', extra, [], 'fix'));
  return el('li', '', [icon(kind), text]);
}

function showTab(name) {
  for (const t of document.querySelectorAll('.tab')) t.classList.toggle('active', t.dataset.tab === name);
  for (const p of document.querySelectorAll('.panel')) p.hidden = p.id !== `panel-${name}`;
  if (name === 'log') { unread = 0; badge('b-log', 0); }
}

function badge(id, n, cls = '') {
  const b = $(id);
  b.hidden = !n;
  b.textContent = n;
  b.className = `badge ${cls}`;
}

function logLine(kind, text) {
  const log = $('log');
  log.append(el('div', text, [], kind));
  log.scrollTop = log.scrollHeight;
  if ($('panel-log').hidden && kind !== 'say') badge('b-log', ++unread, kind === 'fail' ? 'bad' : '');
}

let machine = null;

// Карточка состояния: одно состояние, одна причина, один переключатель.
// Переключатель заперт, пока адрес среды не задан — подключаться некуда.
function showLink(link) {
  const m = machine || { configured: false, host: {}, ports: [0, 0, 0] };
  const route = `${m.host.server} · рабочее место ${m.host.workstation}`;
  const bridge = `mcp-1c → порт ${m.ports[0]}, веб-клиент → ${m.ports[2]}`;
  let state, sub, ind;
  if (!m.configured && !link.on) {
    [state, sub, ind] = ['Не настроено', 'Укажите адрес среды на вкладке «Параметры»', ''];
  } else if (link.kind === 'up') {
    [state, sub, ind] = [`Подключён${link.minutes ? ` · ${link.minutes} мин` : ''}`, bridge, 'up'];
  } else if (link.kind === 'connecting') {
    [state, sub, ind] = ['Подключаюсь…', route, 'busy'];
  } else if (link.kind === 'retrying') {
    [state, sub, ind] = ['Переподключаюсь', link.why, 'busy'];
  } else if (link.kind === 'failed') {
    [state, sub, ind] = ['Не подключён', link.why, 'bad'];
  } else {
    [state, sub, ind] = ['Не подключён', route, ''];
  }
  $('state').textContent = state;
  $('sub').textContent = sub;
  $('sub').title = sub;
  $('ind').className = `ind ${ind}`;
  $('toggle').checked = link.on;
  $('toggle').disabled = !m.configured && !link.on;
}

function setBusy(on, which) {
  busy = on;
  for (const b of document.querySelectorAll('[data-do="install"],[data-do="uninstall"],[data-do="check"],[data-do="save"]')) b.disabled = on;
  $('p-ext').classList.toggle('on', on && which === 'ext');
  $('p-check').classList.toggle('on', on && which === 'check');
}

async function refresh() {
  machine = await invoke('machine');
  const m = machine;
  for (const k of KEYS) $(`f-${k}`).value = m.host[k];
  $('kit').textContent = `Комплект: ${m.kit}`;
  $('kit').title = m.kit;
  showLink(m.link);
  await refreshOps();
}

async function refreshOps() {
  const ops = await invoke('states');
  const ul = $('ops');
  ul.replaceChildren();
  for (const o of ops) {
    const kind = { done: 'ok', missing: 'dim', drift: 'warn', blocked: 'dim' }[o.kind] || 'dim';
    ul.append(row(kind, o.title, o.detail));
  }
}

async function save() {
  const host = {};
  for (const k of KEYS) host[k] = $(`f-${k}`).value;
  $('save-hint').textContent = '';
  try {
    await invoke('save_cfg', { host });
    $('save-hint').textContent = 'Сохранено в 1c-connect.toml.';
    await refresh();
  } catch (e) {
    $('save-hint').textContent = String(e);
  }
}

async function check() {
  if (busy) return;
  showTab('check');
  setBusy(true, 'check');
  $('check-hint').textContent = 'Проверяю…';
  try {
    const checks = await invoke('check');
    const ul = $('checks');
    ul.replaceChildren();
    let failed = 0;
    for (const c of checks) {
      if (!c.ok) failed++;
      ul.append(row(c.ok ? (c.info ? 'dim' : 'ok') : 'bad', c.what, c.detail, c.ok ? '' : c.hint));
    }
    badge('b-check', failed, 'bad');
    $('check-hint').textContent = failed ? `Провалено: ${failed}. Отчёт записан в last-check.txt.` : 'Все факты в порядке. Отчёт записан в last-check.txt.';
  } catch (e) {
    $('check-hint').textContent = String(e);
  }
  setBusy(false);
}

function run(which) {
  if (busy) return;
  setBusy(true, 'ext');
  logLine('say', which === 'uninstall' ? '— снимаю расширение —' : '— устанавливаю расширение —');
  invoke('run', { which });
}

async function act(what) {
  try {
    switch (what) {
      case 'connect': await invoke('connect'); showTab('log'); break;
      case 'disconnect': await invoke('disconnect'); showLink(await invoke('link')); break;
      case 'web': await invoke('open_web'); break;
      case 'save': await save(); break;
      case 'install': run('install'); break;
      case 'uninstall': run('uninstall'); break;
      case 'check': await check(); break;
      case 'clear-log': $('log').replaceChildren(); break;
      case 'hide': await invoke('hide'); break;
      case 'quit': await invoke('quit'); break;
    }
  } catch (e) {
    logLine('fail', String(e));
    showTab('log');
  }
}

$('toggle').addEventListener('change', (ev) => act(ev.target.checked ? 'connect' : 'disconnect'));

document.addEventListener('click', (ev) => {
  const t = ev.target.closest('.tab');
  if (t) return showTab(t.dataset.tab);
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

refresh().then(() => showTab(machine.configured ? 'log' : 'params'));
// «подключён N мин» обновляется само.
setInterval(async () => showLink(await invoke('link')), 30000);
