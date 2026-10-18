# Ход долгих операций в `1c-connect`

Дата: 2026-10-18, переписано после ревью в тот же день. Ветка
`worktree-gpui-connect`, поверх спеки окна `2026-10-16-gpui-connect-design.md`
и этапа 6B (`flow.rs`, `local.rs`, `project.rs`).

## Зачем

Первая выгрузка конфигурации (`cf_decompile` большой базы) идёт десятки
минут, первый push тысяч файлов — минуты, загрузка в базу (`cf_load`) и
установка расширения (Конфигуратор) — минуты. Платформа молчит, git
зовётся через `Command::output()` и читается целиком по завершении. Человек
видит кнопку в состоянии «loading» и не знает, идёт ли процесс или всё
повисло. Нужно, чтобы в любой момент было видно: **что** идёт, **сколько**
уже идёт, **есть ли признаки жизни**; и чтобы по окончании человек узнал об
этом, даже если окно спрятано.

## Цели (v1)

- Во время любой долгой операции окно показывает фазу, прошедшее время с
  секундным тиком и последнюю строку активности; трей показывает фазу и
  время в подсказке и значок «занят».
- Отсутствие активности дольше порога видно явно.
- По окончании, если окно спрятано, системное уведомление Windows.
- Командная строка (`1c-connect export|import|install`) печатает те же фазы
  тем же событием.
- Прерывание — только там, где оно заведомо безопасно: git push и pull.

## Отложено (v2, по ответам на вопросы ниже)

- Счётчик файлов выгрузки и проценты со второго раза — после ответа, чистит
  ли `cf_decompile` каталог `src` перед записью (вопрос 1).
- Проценты git (`--progress` потоком) — дорого: потоковый `git()` должен
  воспроизвести `git_flags` с `http.extraHeader: Basic …`,
  `GIT_TERMINAL_PROMPT=0`, `CREATE_NO_WINDOW`, затирание `redact_text`,
  коды возврата, и отличать строки прогресса от `remote:` и вывода rebase.
  Две фазы из пяти не стоят этого в v1.
- Прерывание `cf_decompile` — убийство `1cv8.exe` через Job посреди
  выгрузки может оставить блокировку файловой базы (вопрос 2).
- «Занято агентом» в карточке — сегодня невозможно честно: замок
  `project::LOCK` берут только `project_status|pull|push` (`project::call`),
  а `cf_load`/`cf_decompile` от агента идут через `Route::PlatformCall`
  мимо замка, и `doing()` их не знает. Сначала чинится замок (ниже,
  «Долг»), потом показ.

## Не-цели

- Очередь операций. Вторая операция получает «занято», как сейчас.
- Оценка «осталось N минут».
- Проценты у `cf_load`, установки и снятия расширения: платформа их не
  даёт никогда.

## Анти-цели

- Опрос по таймеру там, где есть событие. Допустимы только секундный тик
  часов и (в v2) опрос каталога для счётчика файлов.
- Разбор человекочитаемых логов платформы ради чисел.
- Прерывание, которое оставляет базу или Конфигуратор в полусостоянии.

## Что есть сейчас (проверено по коду)

| Часть | Как работает | Что мешает |
|---|---|---|
| `flow::export_with(d: &Deps, message, log)` | `lock()` замка проекта → `log("cf_decompile → src …")` → `d.call("cf_decompile", …)` → `d.push(message)` | фазы есть только строками журнала |
| `flow::import_with(d, log)` | `lock()` → проверка `1cv8.exe` → `d.pull()` → `d.call("cf_load", …)` | то же |
| `flow::with_real(ctx, f)` | собирает `Deps` из `local::call` (таймаут 3600 с), `project::push/pull`, `bridge::configurator_count`; `log = ctx.say`; `Fake::run` в тестах собирает `Deps` сам | два места сборки `Deps`; `&dyn Fn` в `Deps` не `Send`, в поток не отдать |
| `local::call(cmd, tool, args, timeout, on_log)` | поток-читатель stdout → канал; `wait_for` ждёт **одним** `recv_timeout(left)` на весь остаток дедлайна; `notifications/message` → `on_log`; по таймауту/выходу `child.kill()`; процесс в Job Object | нечем прервать снаружи, нечем «подать признак жизни» в ожидании |
| `project::push` | `add`, `commit`, при первом разе `push -q origin HEAD:refs/heads/main`, затем `push -q -u origin <ветка>` — все через `Command::output()` | строк до конца нет |
| `project::pull` | `pull --rebase origin <ветка>` через `output()`, без `-q` | то же |
| `project::LOCK`, `doing()` | замок только у `project::call` (`project_status|pull|push`) и у `flow::lock()`; `DOING_NAMES` — три имени | агентские `cf_*` через `Route::PlatformCall` идут **без замка** |
| `bridge` отмена | `Route::Cancel` делает `abort()` задачи, «blocking git call runs to its end» (комментарий в коде) | своя отмена окна будет вести себя иначе, чем агентская |
| `ops_host::Extension::apply` | `docker::exec(ctx, mcp-1c, --install …)` — синхронный `output()`, `Event::Output` в конце | только фаза и время; `Deps` тут нет, есть `ctx: &Ctx` (`Runner`) |
| `common::Event`, `Sink` | `Sink = Arc<dyn Fn(Event)+Send+Sync>`; `Runner::emit` | исчерпывающие `match` по `Event` в **четырёх** местах: `1c-connect/src/gui/mod.rs` (`line_of`), `1c-connect/src/main.rs`, `dev1c-setup/src/main.rs`, `dev1c-setup/src/gui.rs` |
| `kit.journal` | пишет только `Command`/`Output`, остальное отдаёт назад | новое событие и так не попадёт в журнал на диске — правка не нужна |
| `gui::run_op` | поток + `guarded()`, `Ev::Line` по ходу, `Ev::Done` в конце | «занято» без фазы, времени, чисел |
| `gui::handle(Ev::Line)` | `note_line` + `cx.refresh_windows()` на каждую строку | десятки событий в секунду перерисуют окно десятки раз |
| `gui::panel` | карточка состояния = туннель; тик 30 с живёт в `Panel::new` и умирает со скрытием окна (`hide_window` → `forget_window`) | часов для подсказки трея при спрятанном окне нет |
| `gui::tray::set_state(up, on)` | два булевых состояния значка, зовётся из `set_link` на каждом переходе туннеля | третье состояние «занят» затрётся следующим переходом |
| GPUI уведомления | `App::show_system_notification` есть и на Windows реализован; без `App::set_app_identity` для непакетированного exe `notifier()` даёт `Ok(None)` и тост молча не показывается | одна обязательная строка при запуске |
| `gpui-component::Progress` | `value` в процентах `0..100` (clamp), при `loading(true)` `value` игнорируется | — |

## Решения

### Модель: событие хода в `common::Event`

```rust
// setup/common/src/lib.rs
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub phase: Phase,
    pub done: Option<u64>,     // v2: файлы, объекты
    pub total: Option<u64>,    // v2
    pub note: String,          // последняя строка активности, уже затёртая redact_text
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { Decompile, Push, Pull, Load, Install, Uninstall, Check }
impl Phase {
    pub fn title(self) -> &'static str // «Выгружаю конфигурацию», «Отправляю в git», «Получаю из git»,
                                       // «Загружаю в базу», «Устанавливаю расширение», «Снимаю расширение», «Проверяю»
}
pub enum Event { Ok, Warn, Fail, Say, Command, Output{..}, Progress(Progress) }
impl Runner { fn progress(&self, phase: Phase) { self.emit(Event::Progress(Progress { phase, done: None, total: None, note: String::new() })) } }
```

Источник события — **`Runner`/`Sink`, не `Deps`**: так его порождают и
`flow` (у него есть `ctx`), и `ops_host::Extension::apply` (`ctx.progress(Phase::Install)`
первой строкой, `Uninstall` в `revert_apply`), и `verify::run`
(`Phase::Check`), и `local::call` ничего не знает. `Deps` не меняется, `Fake`
тесты не трогаются; `export_with`/`import_with` получают `ctx: &dyn Runner`?
Нет — у них есть `log`, и этого мало. Решение: `with_real` передаёт в
`export_with`/`import_with` не `log`, а две замыкания `log` и `phase: &dyn Fn(Phase)`;
`Fake` даёт пустое `phase`. Это одна строка в каждой функции потока
(`phase(Phase::Decompile)` перед `call`, `phase(Phase::Push)` перед `push`;
`Pull`, `Load`) и одна в `Fake::run`.

Четыре `match` по `Event` получают ветку `Progress`: `1c-connect` CLI печатает
`== {title}`, `dev1c-setup` (`main.rs`, `gui.rs`) печатает как `Say`, окно —
`Ev::Progress`.

### Поток в окне

`Ev::Progress(Progress)` — из `ctx_for`'s sink, как `Ev::Line`.
`Ui.progress: Option<Running>`:

```rust
pub struct Running { pub which: Which, pub started: Instant, pub phase: Phase, pub note: String, pub seen: Instant }
```

`run_op` ставит `Running { phase: Phase первой операции, seen: now }` вместе
с `busy`; `Ev::Progress` обновляет `phase`/`note`/`seen`; `Ev::Line` той же
операции обновляет `note` (текст строки) и `seen`; `Ev::Done` очищает.

**Троттлинг.** `handle` не зовёт `refresh_windows()` на каждое событие во
время операции: `Ui.dirty: bool` взводится, а перерисовку делает секундный
тик (ниже). Вне операции — как сейчас. Это закрывает и `git --progress` в
v2.

### Часы на уровне приложения

Тик переезжает из `Panel::new` в `launch`: одна задача `cx.spawn` на всё
время жизни приложения, интервал 1 с пока `progress.is_some()`, иначе 30 с
(минуты туннеля). На тике: если `dirty` или `progress.is_some()` —
`refresh_windows()`; раз в 60 с при операции — подсказка трея. Окно
спрятано — тик всё равно идёт, трей обновляется.

### Застой

`seen.elapsed() > 5 мин` → в карточке строка «нет новостей N мин» жёлтым.
Прервать можно только `Push`/`Pull` (ниже). Для `Decompile`/`Load`/`Install`
строка остаётся предупреждением; таймаут `local::call` (3600 с) — последний
рубеж, как и сейчас.

### Прерывание (только git)

`Ui.cancel: Arc<AtomicBool>`, новый на каждую операцию. `project::push` и
`pull` получают `cancel: &AtomicBool` и для своих `git push`/`git pull`
переходят с `output()` на `spawn()` + ожидание в цикле `try_wait` по 250 мс
с проверкой флага; взведён — `kill()`, `Err("прервано")`. Вывод по-прежнему
читается целиком после выхода (`wait_with_output`), флаги `git_flags`,
`GIT_TERMINAL_PROMPT`, `CREATE_NO_WINDOW` и `redact_text` — те же, через
общий `fn git_cmd(p, args) -> Command`, на который переводится и `git()`.
Остальные команды git не трогаются. Прерванный push/pull безопасен по
определению git; замок проекта освобождается по `drop`. Кнопка «Прервать» в
карточке видна только при `phase ∈ {Push, Pull}`. Текст итога после
прерывания — «прервано» строкой `Kind::Warn`, не `Fail`: `Ev::Done(Some(..))`
дополняется признаком `cancelled: bool` (вариант `Ev::Done { err: Option<String>, cancelled: bool }`).

`local::call` в v1 не меняется (прерывания у платформенных фаз нет); в v2
его `wait_for` переводится на шаг 1 с, тогда же появится `cancel`.

### Поверхности

- **Карточка состояния** при `progress.is_some()`: кружок жёлтый,
  заголовок `phase.title()`, подстрока «12 мин 05 с», последняя
  строка активности 13 px `caption`, при застое — «нет новостей N мин»,
  при `Push`/`Pull` — «Прервать». Переключатель туннеля `disabled`.
  Отдельного индикатора в карточке нет — он в разделителе (ниже).
- **Полоса хода = разделитель над подвалом** (решение заказчика
  2026-10-18: «превратить разделитель в тоненький модный прогресс-бар»).
  В покое это та же линия 1 px `border.opacity(0.6)`, что и сейчас; пока
  окно чего-то ждёт, линия становится полосой 2 px: дорожка —
  `border.opacity(0.6)`, бегунок — акцент темы (`primary`, жёлтый 1С).
  Полоса отражает **всё, что требует времени** (заказчик, 2026-10-18), а
  не только операции с фазами: `Ui::working()` = `busy.is_some()` (Install/
  Uninstall/Export/Import/Check) **или** туннель в `State::Connecting`
  (включая переподключение) **или** `Ui.pending > 0` — счётчик потоков
  `refresh_ops`/`refresh_project` (плюс при запуске потока, минус на
  `Ev::Ops`/`Ev::Project`; при панике потока `guarded` шлёт `Ev::Done`, и
  счётчик обнуляется там же). Сохранение параметров мгновенное и полосу не
  трогает. Пока `working()`, полоса одна на всё — какая именно работа идёт,
  говорят карточка состояния и журнал, не полоса.
  v1 — без чисел, бегунок длиной в четверть ширины ездит слева направо
  по кругу (~1.2 с на проход, `with_animation` поверх двух `div`, своих
  виджетов не рисовать; `gpui-base::Progress::indeterminate(true)`
  подходит, если его высоту можно задать в 2 px, иначе два `div`).
  v2 — при известных `done`/`total` бегунок стоит и растёт на их долю.
  Как только `working()` становится `false`, полоса за ~300 мс
  возвращается в линию; места в окне не занимает ни в одном состоянии,
  высота окна не меняется.
  Сделано в задаче 17 на `Ui::working()` без фаз; затухания нет — возврат
  мгновенный.
- **Подвал**: главная кнопка в `loading`, меню «…» `disabled` — как сейчас.
- **Трей**: `set_state` принимает одно значение из трёх —
  `Icon::{Off, Up, Busy}`; `set_link` вызывает его с учётом `progress`
  (`Busy` имеет приоритет), `badge` рисует жёлтый кружок для `Busy`.
  Подсказка: «1c-connect: выгружаю конфигурацию, 12 мин» раз в минуту с
  тика.
- **Журнал**: строка `Kind::Say` на каждую фазу (`phase.title()`), итог как
  сейчас.
- **Уведомление**: в `launch` — `cx.set_app_identity(..)` (идентификатор
  приложения; значение — имя exe-продукта `OktaPlus.1c-connect`, строитель
  берёт то, что требует сигнатура). По `Ev::Done`, если `Ui.window.is_none()`:
  `cx.show_system_notification(SystemNotification { tag: "op", title: "1c-connect", body, actions: vec![] })`,
  `body` — итог или текст ошибки или «прервано».
- **CLI**: `== Выгружаю конфигурацию` при каждой фазе; застой CLI не
  отслеживает (Ctrl+C и так есть).

### Долг, который вскрыло ревью (отдельная работа, не эта спека)

- Агентские `cf_load`/`cf_decompile`/`infobase_update` должны брать тот
  же замок проекта, что и `project_*` и окно; иначе окно может выгружать во
  время `cf_load` агента. Правка в `bridge::Route::PlatformCall` и
  `DOING_NAMES`. После неё «занято агентом» в карточке становится честным.
- Отмена у агента (`Route::Cancel` → `abort()`) не останавливает git; когда
  появится `cancel` в `project::push/pull`, мост должен взводить тот же флаг.

### Что меняется по файлам (v1)

| Файл | Правка |
|---|---|
| `common/src/lib.rs` | `Progress`, `Phase`, `Event::Progress`, `Runner::progress` |
| `1c-connect/src/flow.rs` | `phase` замыкание в `export_with`/`import_with`, `with_real` строит его из `ctx.progress`, `Fake::run` даёт пустое |
| `1c-connect/src/project.rs` | `git_cmd`, `push`/`pull` с `cancel`, `spawn` + `try_wait` для двух команд |
| `1c-connect/src/ops_host.rs` | `Extension::apply`/`revert_apply` → `ctx.progress(Install/Uninstall)` |
| `1c-connect/src/verify.rs` | `run` → `ctx.progress(Check)` первой строкой |
| `1c-connect/src/main.rs` | ветка `Event::Progress` |
| `dev1c-setup/src/main.rs`, `dev1c-setup/src/gui.rs` | ветка `Event::Progress` как `Say` |
| `1c-connect/src/gui/mod.rs` | `Ui.progress`, `Ui.cancel`, `Ui.dirty`, `Ev::Progress`, `Ev::Done { err, cancelled }`, тик уровня приложения, `set_app_identity`, уведомление |
| `1c-connect/src/gui/panel.rs` | карточка хода, «Прервать», разделитель над подвалом становится полосой хода, тик из `Panel::new` убран |
| `1c-connect/src/gui/tray.rs` | `set_state(Icon)` с тремя состояниями, жёлтый бейдж |
| `bridge.rs`, `local.rs` | не меняются в v1 |

### Проверки

- `flow`: `Fake` с записью фаз; порядок `Decompile → Push`, `Pull → Load`.
- `project`: `git_cmd` даёт те же флаги, что `git()` (сравнение `Command`
  через `get_args`/`get_envs`); `push` с взведённым `cancel` до запуска
  возвращает «прервано», не трогая `add`/`commit`? Нет — проверка флага
  только вокруг `push`-команд; тест: `cancel` взведён → `Err("прервано")`
  и процесс убит (на фиктивном `git`-скрипте `sleep`).
- `gui`: чистые `progress_text(&Running, now) -> (title, sub)`,
  `is_stale(&Running, now)`, `can_cancel(phase)`; `Ev::Progress` обновляет
  `seen`; `Ev::Done` очищает `progress` и создаёт новый `cancel`;
  `Icon` из `(link, progress)`.
- `common`: `Phase::title` для всех фаз; обе программы собираются
  (`--no-default-features` покрывает `dev1c-setup/src/main.rs`; `gui.rs`
  обеих программ — CI).
- Живой прогон: выгрузка демо-базы с открытым и спрятанным окном (часы
  идут, подсказка трея меняется, уведомление приходит); «Прервать» на push
  (замок свободен, повторный push проходит).

## Вопросы заказчику (меняют v2, не v1)

1. Чистит ли `cf_decompile` (vanessa-runner `decompile` → `DumpConfigToFiles`)
   каталог `src` перед записью, или дописывает поверх? От этого зависит,
   можно ли считать файлы как прогресс. Проверяется на демо-базе одной
   выгрузкой с непустым `src`.
2. Допустимо ли убивать `1cv8.exe` посреди выгрузки? Если файловая база
   остаётся с блокировкой, «Прервать» у выгрузки не делаем вообще.
3. Нужны ли проценты git ради первого push (минуты) при цене переписывания
   `git()`? Моя рекомендация — нет, пока не увидим, что фаза+часы не
   хватает.
