// Окно — тонкая оболочка: всё решает движок, страница показывает его состояние
// и зовёт команды. Логики развёртывания здесь нет намеренно.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const KEYS = ['server.name', 'server.port', 'server.memory', 'server.engine', 'server.gpu', 'model.url', 'model.name', 'model.key', 'model.file'];
let machine = null;
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

function setBusy(on) {
  busy = on;
  for (const b of document.querySelectorAll('.btn[data-do]:not([data-do="quit"])')) b.disabled = on;
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
  const done = machine.ops.filter((o) => o.kind === 'done').length;
  $('lead').textContent = !machine.file_exists
    ? 'Параметров ещё нет: проверьте значения, сохраните и установите.'
    : done === machine.ops.length
      ? 'Среда установлена. Повторная установка выполняет только недостающие шаги.'
      : `Сделано ${done} из ${machine.ops.length}. «Установить» выполняет только недостающее.`;
  const ul = $('ops');
  ul.replaceChildren();
  for (const o of machine.ops) {
    const mark = { done: '✓', missing: '·', drift: '!', blocked: '–' }[o.kind] || '·';
    ul.append(el('li', '', [el('span', mark, [], 'mark'), el('span', o.title, [el('small', o.label)], 'what')], o.kind));
  }
  const [rdp, gitea, model, ssh] = machine.ports;
  const rows = [
    ['Рабочие места', `mstsc /v:<адрес этой машины>:${rdp} — dev1/dev1, dev2/dev2, dev3/dev3`],
    ['Репозитории', `http://<адрес>:${gitea}`],
    ['Сервер модели', `http://<адрес>:${model}`],
    ['Для 1c-connect', `адрес этой машины и порт ${ssh}`],
  ];
  $('access').replaceChildren(...rows.map(([k, v]) => el('div', '', [el('span', k), el('code', v)], 'kv')));
}

async function save() {
  const cfg = { server: {}, model: {} };
  for (const k of KEYS) {
    const [s, key] = k.split('.');
    const f = $(`f-${k}`);
    cfg[s][key] = f.type === 'checkbox' ? f.checked : f.type === 'number' ? Number(f.value) : f.value;
  }
  $('save-hint').textContent = '';
  try {
    await invoke('save_cfg', { cfg });
    $('save-hint').textContent = 'Сохранено.';
    await refresh();
  } catch (e) {
    $('save-hint').textContent = String(e);
  }
}

async function check() {
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
  if (which === 'clean-data' && $('confirm').value !== machine.cfg.server.name) {
    logLine('fail', 'Удаление данных: введите имя контейнера в поле рядом — это стирает репозитории, проекты и настройки.');
    return;
  }
  setBusy(true);
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
      default: run(what);
    }
  } catch (e) {
    logLine('fail', String(e));
  }
}

document.addEventListener('click', (ev) => {
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
