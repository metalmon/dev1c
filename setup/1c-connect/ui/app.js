// Окно — тонкая оболочка: всё решает движок, страница показывает его состояние
// и зовёт команды. Логики подключения здесь нет намеренно.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const KEYS = ['server', 'workstation', 'password', 'base_path', 'db_user', 'db_password', 'base_url', 'dump', 'project'];
let machine = null;
let busy = false;
let unread = 0;
let statusTimer = null;

// Действия каждой вкладки живут в нижней плашке.
const ACTIONS = {
  params: [],
  ext: [{ do: 'web', label: 'Веб-клиент' }, { do: 'uninstall', label: 'Снять' }, { do: 'install', label: 'Установить', accent: true }],
  check: [{ do: 'check', label: 'Проверить', accent: true }],
  project: [{ do: 'open-dir', label: 'Открыть каталог' }, { do: 'import', label: 'Загрузить из git' }, { do: 'export', label: 'Выгрузить в git', accent: true }],
  log: [{ do: 'clear-log', label: 'Очистить' }],
};

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

// Итог последнего действия в нижней строке; через несколько секунд гаснет.
function status(text, kind = '') {
  const s = $('foot-status');
  s.textContent = text;
  s.className = `path ${kind}`;
  clearTimeout(statusTimer);
  if (kind !== 'bad') statusTimer = setTimeout(() => { s.textContent = ''; s.className = 'path'; }, 6000);
}

function currentTab() {
  return document.querySelector('.tab.active').dataset.tab;
}

function renderActions() {
  const box = $('foot-actions');
  box.replaceChildren();
  for (const a of ACTIONS[currentTab()]) {
    const b = el('button', a.label, [], `btn${a.accent ? ' accent' : ''}`);
    b.dataset.do = a.do;
    b.disabled = busy && a.do !== 'clear-log';
    box.append(b);
  }
}

function showTab(name) {
  for (const t of document.querySelectorAll('.tab')) t.classList.toggle('active', t.dataset.tab === name);
  for (const p of document.querySelectorAll('.panel')) p.hidden = p.id !== `panel-${name}`;
  if (name === 'log') { unread = 0; badge('b-log', 0); }
  if (name === 'project') refreshProject();
  renderActions();
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
  $('p-ext').classList.toggle('on', on && which === 'ext');
  $('p-check').classList.toggle('on', on && which === 'check');
  $('p-project').classList.toggle('on', on && which === 'project');
  renderActions();
}

async function refresh() {
  machine = await invoke('machine');
  for (const k of KEYS) $(`f-${k}`).value = machine.host[k];
  showLink(machine.link);
  await refreshOps();
}

// Вкладка «Проект»: раскладка и состояние ветки, строки от project_status.
async function refreshProject() {
  const ul = $('project');
  try {
    const v = await invoke('project');
    ul.replaceChildren();
    if (!v.ok) { ul.append(row('warn', v.dir, v.why, 'Параметры → Проект, затем Расширение → Установить')); return; }
    ul.append(row('ok', v.dir, `ветка ${v.branch}`));
    for (const l of v.lines) { const i = l.indexOf(': '); ul.append(row('dim', i > 0 ? l.slice(0, i) : l, i > 0 ? l.slice(i + 2) : '')); }
  } catch (e) {
    ul.replaceChildren(row('bad', 'проект', String(e)));
  }
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

// Сохранение — при каждом изменении поля; кнопки нет, итог — в нижней строке.
async function save() {
  const host = {};
  for (const k of KEYS) host[k] = $(`f-${k}`).value;
  try {
    await invoke('save_cfg', { host });
    status('Сохранено в 1c-connect.toml', 'ok');
    await refresh();
  } catch (e) {
    status(String(e), 'bad');
  }
}

async function browse(kind, inputId) {
  // Диалог открывает Rust: на это время окно не прячется при потере фокуса.
  const picked = await invoke('pick', { kind, start: $(inputId).value });
  if (!picked) return;
  $(inputId).value = picked;
  await save();
}

async function check() {
  if (busy) return;
  showTab('check');
  setBusy(true, 'check');
  status('Проверяю…');
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
    status(failed ? `Провалено: ${failed}. Отчёт — last-check.txt в комплекте` : 'Все факты в порядке. Отчёт — last-check.txt', failed ? 'bad' : 'ok');
  } catch (e) {
    status(String(e), 'bad');
  }
  setBusy(false);
}

const RUN_TITLES = { install: '— устанавливаю —', uninstall: '— снимаю расширение —', export: '— выгружаю в git —', import: '— загружаю из git —' };

function run(which) {
  if (busy) return;
  setBusy(true, which === 'export' || which === 'import' ? 'project' : 'ext');
  logLine('say', RUN_TITLES[which] || `— ${which} —`);
  showTab('log');
  invoke('run', { which });
}

async function act(what) {
  try {
    switch (what) {
      case 'connect': await invoke('connect'); showTab('log'); break;
      case 'disconnect': await invoke('disconnect'); showLink(await invoke('link')); break;
      case 'web': await invoke('open_web'); break;
      case 'install': run('install'); break;
      case 'uninstall': run('uninstall'); break;
      case 'export': run('export'); break;
      case 'import': run('import'); break;
      case 'open-dir': await invoke('open_dir'); break;
      case 'check': await check(); break;
      case 'clear-log': $('log').replaceChildren(); break;
      case 'hide': await invoke('hide'); break;
    }
  } catch (e) {
    logLine('fail', String(e));
    status(String(e), 'bad');
    showTab('log');
  }
}

$('toggle').addEventListener('change', (ev) => act(ev.target.checked ? 'connect' : 'disconnect'));
for (const k of KEYS) $(`f-${k}`).addEventListener('change', save);

document.addEventListener('click', (ev) => {
  const t = ev.target.closest('.tab');
  if (t) return showTab(t.dataset.tab);
  const br = ev.target.closest('[data-browse]');
  if (br) return browse(br.dataset.browse, br.dataset.for);
  const b = ev.target.closest('[data-do]');
  if (b) act(b.dataset.do);
});

listen('conn:line', (e) => logLine(e.payload.kind, e.payload.text));
listen('conn:state', (e) => showLink(e.payload));
listen('conn:done', async (e) => {
  logLine(e.payload ? 'fail' : 'ok', e.payload ? `Остановлено: ${e.payload}` : 'Готово.');
  status(e.payload ? `Остановлено: ${e.payload}` : 'Готово', e.payload ? 'bad' : 'ok');
  setBusy(false);
  await refreshOps();
  if (currentTab() === 'project') await refreshProject();
});
listen('conn:run-check', () => check());

refresh().then(() => showTab(machine.configured ? 'log' : 'params'));
// «подключён N мин» обновляется само.
setInterval(async () => showLink(await invoke('link')), 30000);
