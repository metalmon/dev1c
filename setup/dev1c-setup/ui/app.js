// Окно — тонкая оболочка: всё решает движок, страница показывает его состояние
// и зовёт команды. Логики развёртывания здесь нет намеренно.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const KEYS = ['server.name', 'server.port', 'server.memory', 'server.engine', 'server.gpu', 'model.url', 'model.name', 'model.key', 'model.file'];
let machine = null;
let busy = false;
let unread = 0;

function el(tag, text, children = [], cls = '') {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text;
  for (const c of children) e.append(c);
  return e;
}

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

function setBusy(on, which) {
  busy = on;
  for (const b of document.querySelectorAll('.btn[data-do]:not([data-do="quit"]):not([data-do="clear-log"])')) b.disabled = on;
  $('p-ops').classList.toggle('on', on && which === 'ops');
  $('p-check').classList.toggle('on', on && which === 'check');
}

function get(obj, path) {
  return path.split('.').reduce((o, k) => o[k], obj);
}

async function refresh() {
  machine = await invoke('machine');
  for (const k of KEYS) {
    const f = $(`f-${k}`);
    const v = get(machine.cfg, k);
    if (f.type === 'checkbox') f.checked = v; else f.value = v;
  }
  $('kit').textContent = `Комплект: ${machine.kit}`;
  const ops = machine.ops;
  const done = ops.filter((o) => o.kind === 'done').length;
  const drift = ops.filter((o) => o.kind === 'drift').length;
  const container = ops.find((o) => o.id === 'container');
  const running = container && container.kind === 'done';
  $('state').textContent = running
    ? (drift ? 'Среда работает, параметры разошлись' : 'Среда работает')
    : done ? 'Среда установлена не полностью' : 'Среда не установлена';
  $('ind').className = 'ind' + (running ? (drift ? ' busy' : ' up') : '');
  $('sub').textContent = running
    ? `${machine.cfg.server.name} · ${container.label}`
    : `сделано ${done} из ${ops.length} — «Установить» выполняет только недостающее`;
  $('notice').hidden = machine.file_exists;
  $('notice-text').textContent = 'Параметров ещё нет: проверьте значения на вкладке «Параметры», сохраните и установите.';

  const ul = $('ops');
  ul.replaceChildren();
  for (const o of ops) {
    const kind = { done: 'ok', missing: 'dim', drift: 'warn', blocked: 'dim' }[o.kind] || 'dim';
    ul.append(row(kind, o.title, o.label));
  }
  badge('b-ops', ops.length - done, drift ? 'bad' : '');

  const [rdp, gitea, model, ssh] = machine.ports;
  const rows = [
    ['Рабочие места', `mstsc /v:<адрес>:${rdp}`, 'dev1 / dev1, dev2 / dev2, dev3 / dev3'],
    ['Репозитории Gitea', `http://<адрес>:${gitea}`, 'те же логины'],
    ['Сервер модели', `http://<адрес>:${model}`, 'OpenAI-совместимый, для Zed внутри и снаружи'],
    ['Для 1c-connect', `<адрес>:${ssh}`, 'адрес среды в 1c-connect.toml разработчика'],
  ];
  $('access').replaceChildren(...rows.map(([k, v, d]) => el('li', '', [el('span', k, [], 't'), el('div', '', [el('code', v), el('div', d, [], 'd')])])));
}

async function save() {
  const cfg = { server: {}, model: {} };
  for (const k of KEYS) {
    const [s, key] = k.split('.');
    const f = $(`f-${k}`);
    cfg[s][key] = f.type === 'checkbox' ? f.checked : f.type === 'number' ? Number(f.value) : f.value;
  }
  try {
    await invoke('save_cfg', { cfg });
    $('save-hint').textContent = 'Сохранено в dev1c.toml. Применить — «Операции» → «Применить изменения».';
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
  showTab('log');
  setBusy(true, 'ops');
  logLine('say', `— ${{ install: 'установка', change: 'применяю изменения', 'clean-stack': 'снимаю контейнер', 'clean-data': 'удаляю контейнер и данные' }[which]} —`);
  invoke('run', { which });
}

async function act(what) {
  if (busy) return;
  try {
    switch (what) {
      case 'save': await save(); break;
      case 'check': await check(); break;
      case 'quit': await invoke('quit'); break;
      case 'clear-log': $('log').replaceChildren(); break;
      case 'ask-clean-data': $('confirm-box').hidden = false; $('confirm').value = ''; $('confirm').focus(); break;
      case 'cancel-clean': $('confirm-box').hidden = true; break;
      case 'clean-data':
        if ($('confirm').value !== machine.cfg.server.name) {
          $('ops-hint').textContent = `Введите имя контейнера (${machine.cfg.server.name}): удаление стирает репозитории, проекты и настройки.`;
          return;
        }
        $('confirm-box').hidden = true;
        run('clean-data');
        break;
      default: run(what);
    }
  } catch (e) {
    logLine('fail', String(e));
    showTab('log');
  }
}

document.addEventListener('click', (ev) => {
  const t = ev.target.closest('.tab');
  if (t) return showTab(t.dataset.tab);
  const b = ev.target.closest('[data-do]');
  if (b) act(b.dataset.do);
});

listen('dev1c:line', (e) => logLine(e.payload.kind, e.payload.text));
listen('dev1c:done', async (e) => {
  logLine(e.payload ? 'fail' : 'ok', e.payload ? `Остановлено: ${e.payload}` : 'Готово.');
  setBusy(false);
  await refresh();
});

refresh();
