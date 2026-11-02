//! Один сеанс подключения: поток со своим tokio, туннель с переподключением
//! и мост на проброшенные порты. Общая часть команды `connect` и окна: обе
//! зовут `start`, держат `Session` и в конце — `stop`.

use crate::cfg::{Cfg, Ports};
use crate::{bridge, editors, tunnel};
use common::kit::Kit;
use common::{Event, Sink};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};

/// Параметры с номером поколения: `set` меняет оба одним захватом мьютекса,
/// так что `get` видит либо целиком старое, либо целиком новое — никогда
/// смесь полей одной конфигурации с полями другой. `get`/`set` только
/// клонируют и сразу отпускают мьютекс — лока через `await` здесь нет и
/// быть не должно (см. `bridge::pipe_jsonrpc`, главный читатель).
#[derive(Clone)]
pub struct Live(Arc<Mutex<(u64, Cfg)>>);

impl Live {
    pub fn new(cfg: Cfg) -> Live {
        Live(Arc::new(Mutex::new((0, cfg))))
    }

    /// Поколение и снимок параметров одним захватом — пара всегда согласована.
    pub fn get(&self) -> (u64, Cfg) {
        let g = self.0.lock().unwrap();
        (g.0, g.1.clone())
    }

    /// Поколение растёт только на настоящей смене параметров — равный тому,
    /// что уже в ячейке, `cfg` не трогает ни его, ни содержимое. Сравнение
    /// лежит здесь, а не у вызывающего: иначе каждый новый читатель (окно,
    /// `ops_host::run_mcp`, будущий код) обязан был бы повторить его сам и
    /// рано или поздно забыл бы (находка ревью задачи 3, второй проход).
    /// Без этого такт опроса, перечитывающий один и тот же toml раз в 200 мс
    /// (`ops_host::watch_mcp`), бил бы поколение на каждый тик, и
    /// `bridge::pipe_jsonrpc` рестартовал бы ребёнка практически на каждый
    /// вызов инструмента вместо «после правки параметров».
    /// Выбранный в этом сеансе порт (`Cfg::resolved_http_port`) — факт
    /// СЕАНСА, не файла, и перечитывание toml не вправе его стирать (круг
    /// исправления 2, пункт 4): `watch_mcp` каждые 200 мс кладёт сюда
    /// `Cfg::load(kit)` без `ops_host::settle_chosen_http_port` (его там и не
    /// должно быть — такт читает диск, а не подбирает порты), и голый
    /// `Cfg::load` всегда отдаёт `resolved_http_port: None`. Без переноса
    /// улаженный порт жил бы в ячейке ровно до следующего такта: `http_port()`
    /// откатывался бы на выведенный, `--base` для `mcp-1c` указывал бы не
    /// туда, где слушает httpd, — тот же дефект, ради которого вся задача
    /// писалась, вернувшийся второй дорогой. Перенос — ДО сравнения на
    /// равенство: иначе голая перезагрузка файла с тем же содержимым читалась
    /// бы как «параметры сменились» (пустое `None` ≠ улаженный `Some`) и
    /// дёргала рестарт ребёнка напрасно.
    pub fn set(&self, mut cfg: Cfg) {
        let mut g = self.0.lock().unwrap();
        if cfg.resolved_http_port.is_none() {
            cfg.resolved_http_port = g.1.resolved_http_port;
        }
        if g.1 == cfg {
            return;
        }
        g.0 = g.0.wrapping_add(1);
        g.1 = cfg;
    }
}

/// Что с туннелем. Переходы — по событиям `tunnel::run` (и только по ним:
/// предупреждения моста состояние не меняют) и по его завершению.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Off,
    Connecting,
    Up { since: Instant },
    Retrying(String),
    Failed(String),
}

impl State {
    /// Сеанс жив (поток работает), даже если сейчас переподключается.
    pub fn is_on(&self) -> bool {
        !matches!(self, State::Off | State::Failed(_))
    }

    pub fn is_up(&self) -> bool {
        matches!(self, State::Up { .. })
    }

    /// Следующее состояние после события туннеля.
    ///
    /// `Event::Ok` здесь — сигнал «канал поднят», а не цвет строки: транспорты
    /// (`listen::run`, `tunnel::run`) шлют его именно за этим. Цветом он
    /// занимался заодно, и это стоило дорого: смена `Ok` на `Say` ради того,
    /// чтобы журнал не зеленел раньше вердикта, отняла у состояния переход в
    /// «поднято» — карточка навсегда застревала на «Включаю…» (30.10). Теперь
    /// за цвет отвечает `for_log` ниже, за состояние — этот разбор, и спутать
    /// их больше негде.
    pub fn next(self, e: &Event) -> State {
        match e {
            Event::Ok(_) => State::Up { since: Instant::now() },
            Event::Warn(why) => State::Retrying(why.clone()),
            Event::Fail(why) => State::Failed(why.clone()),
            _ => self,
        }
    }

    /// Состояние после того, как `tunnel::run` вернулся: остановлен человеком
    /// или упал на том, что чинить ему же.
    ///
    /// (`for_log` — ниже, рядом с `State`: он про журнал, а не про состояние.)
    pub fn ended(r: &anyhow::Result<()>) -> State {
        match r {
            Ok(()) => State::Off,
            Err(e) => State::Failed(format!("{e:#}")),
        }
    }

    /// Слова по `cfg.host.mode`: в туннельном — как раньше, в локальном —
    /// про MCP (слушатели держат порты сами, переподключаться там не от
    /// чего — см. `start`), а не про сеанс связи со средой.
    pub fn label(&self, mode: &str) -> String {
        let local = mode == "local";
        match self {
            State::Off => {
                if local {
                    "MCP выключены".into()
                } else {
                    "не подключён".into()
                }
            }
            State::Connecting => {
                if local {
                    "включаю…".into()
                } else {
                    "подключаюсь…".into()
                }
            }
            State::Up { since } => {
                let m = since.elapsed().as_secs() / 60;
                let verb = if local { "MCP включены" } else { "подключён" };
                if m < 60 {
                    format!("{verb} {m} мин")
                } else {
                    format!("{verb} {} ч {:02} мин", m / 60, m % 60)
                }
            }
            State::Retrying(why) => format!("переподключаюсь: {why}"),
            State::Failed(why) => {
                if local {
                    why.clone()
                } else {
                    format!("не подключён: {why}")
                }
            }
        }
    }
}

/// Каким событие пойдёт в ЖУРНАЛ — в отличие от того, каким его прочло
/// состояние (`State::next`). Подъём канала транспорт шлёт как `Ok`, потому
/// что по нему состояние становится «поднято», но для человека это не итог, а
/// начало проверки: зелёное в этой программе значит «работать можно», и
/// правило «зелёной точки до вердикта готовности не бывает» карточка держит
/// давно. Журнал же ставил ✓ раньше вердикта, и следующая строка её отменяла —
/// «сперва пишет Подключено, горит зелёный, потом Не всё готово» (владелец,
/// 30.10). Поэтому здесь, и только здесь, подъём понижается до нейтрального.
///
/// Чистая функция с тестом рядом: прежняя попытка решить то же самое сменой
/// события в транспорте выглядела так же безобидно и отняла у связи переход в
/// «поднято».
fn for_log(e: &Event, now_up: bool) -> Event {
    match e {
        Event::Ok(t) if now_up => Event::Say(t.clone()),
        other => other.clone(),
    }
}

/// Живой сеанс. `stop` опускает туннель и ждёт поток; просто уронить
/// значение — то же самое, но без ожидания.
pub struct Session {
    stop: watch::Sender<bool>,
    done: Option<oneshot::Receiver<anyhow::Result<()>>>,
    /// Та же ячейка, что держит и обслуживающий `Serve` (см. `Serve::dispatch`):
    /// «Сохранить» в локальном режиме (`gui::save_host`) меняет параметры
    /// через неё, не поднимая новый сеанс — слушатели и уже принятые
    /// соединения это не трогает, следующий же вызов агента просто уходит в
    /// новую базу (спека «Смена проекта/базы… незаметна для Zed»). Мост сам
    /// перечитывает её на каждый запрос и, если ребёнок уже запущен не на
    /// тех параметрах, перезапускает его — см. `bridge::pipe_jsonrpc`.
    cfg: Live,
}

impl Session {
    /// Остановить и дождаться. Не звать изнутри tokio-рантайма.
    pub fn stop(mut self) -> anyhow::Result<()> {
        let _ = self.stop.send(true);
        match self.done.take() {
            Some(rx) => rx.blocking_recv().unwrap_or(Ok(())),
            None => Ok(()),
        }
    }

    /// Кончился ли сеанс сам — не ожидая его. Спрашивают перед повторной
    /// попыткой: в `Option` лежит и живой сеанс, и тот, чей поток уже ушёл
    /// (отказ bind на старте — `listen::run` возвращает `Err` сразу), а
    /// поднимать новый поверх первого нельзя и нужно поверх второго. Решает
    /// по самому сеансу (`done`), а не по запомненному где-то тексту отказа:
    /// отказ — следствие конца, а не его причина.
    ///
    /// Итог кончившегося сеанса здесь теряется, и это нарочно: о причине уже
    /// сказано состоянием (`State::Failed` через `on_state`), а второй раз её
    /// некому слушать — `stop` после этого отвечает `Ok`.
    pub fn finished(&mut self) -> bool {
        match self.done.as_mut() {
            Some(rx) => match rx.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => false,
                _ => {
                    self.done = None;
                    true
                }
            },
            None => true,
        }
    }

    /// Дождаться, пока сеанс закончится сам (ошибка, которую не чинят
    /// повтором). Отмена ожидания сеанс не трогает.
    pub async fn wait(&mut self) -> anyhow::Result<()> {
        match self.done.as_mut() {
            Some(rx) => {
                let r = rx.await;
                self.done = None;
                r.unwrap_or(Ok(()))
            }
            None => Ok(()),
        }
    }

    /// Подменить параметры, которые мост перечитывает на каждый запрос —
    /// без перезапуска сеанса. Уже идущий у ребёнка вызов доигрывает со
    /// старыми; следующий застанет мост свободным и пересоздаст ребёнка на
    /// новых (см. `bridge::pipe_jsonrpc`). В туннельном режиме не зовётся
    /// (там порты живут внутри SSH и сеанс всё равно перезапускают целиком),
    /// но и там это не повредит.
    pub fn update_cfg(&self, cfg: Cfg) {
        self.cfg.set(cfg);
    }

    /// Параметры, на которых ребёнок работает прямо сейчас — последний
    /// `Cfg`, положенный сюда `update_cfg` (или тот, с которым сеанс
    /// стартовал). Нужен тому, кто хочет подменить одно поле на живом,
    /// заведомо годном `Cfg`, не толкая в сеанс чужой, непроверенный —
    /// см. `gui::tray::apply_access`, круг исправления задачи 3.
    pub fn cfg_now(&self) -> Cfg {
        self.cfg.get().1
    }
}

/// Контейнерный случай задачи 7: выключили связь в окне — туннель ложится, порт рабочего места перестаёт слушать, `nc` на стороне Zed выходит, и Zed показывает упавший сервер MCP, который сам уже никогда не поднимет (спека «MCP в редакторе», «Факты…»). Вместо этого — правим `"enabled"` в `settings.json` Zed внутри контейнера по тому же SSH, что держит туннель: `false` при выключении, `true` при включении, и Zed гасит/поднимает серверы сам, человек видит «выключен» вместо «упал».
///
/// Вся логика здесь — читаем файл (`exec` — первый вызов), правим его `editors::set_enabled_zed` (`exe: None` — имя `1c`/`vrunner` в контейнере наше по построению, положила его только наша же правка). Нечего менять — старый образ без наших записей, нечитаемый JSONC, повторное подключение на уже верном состоянии — выходим без второго вызова (Important 3 ревью задачи: лишняя перезапись чужого файла не нужна и опасна). Есть что менять — пишем во временный файл рядом (`exec` — второй вызов) и переименовываем его поверх настоящего (`exec` — третий вызов): `cat > PATH` сам усекает файл ДО приёма данных, и обрыв посередине (сеть легла на полпути) оставил бы человека с пустыми настройками, а `mv` — одна атомарная операция файловой системы. Канал подставляется параметром, поэтому вся цепочка проверяется без настоящего SSH. Своего скрипта в образе не заводим и `sed` не посылаем: правка живёт в одном месте, и путь временного файла — константа рядом с настоящим, никакой интерполяции.
///
/// ponytail: правка без блокировки чужого файла — человек, правящий `settings.json` в Zed ровно в этот момент, потеряет свою правку. Цена read-modify-write без замка на чужой файл; поднять до блокировки, если это окажется больше, чем теоретической неприятностью.
///
/// `Ok` несёт не только успех, но и то, что увидело чтение (`editors::found_by_name` по тексту, который всё равно пришлось прочитать ради правки) — решение владельца 29.10: это и материал для факта набора готовности (`verify::editor_mcp_fact`), и второго похода по SSH ради одного факта быть не должно.
pub fn flip_with(exec: &dyn Fn(&str) -> Result<String, String>, on: bool) -> Result<editors::Found, String> {
    const PATH: &str = "~/.config/zed/settings.json";
    const TMP: &str = "~/.config/zed/settings.json.tmp-1c-connect";
    let text = exec(&format!("cat {PATH}"))?;
    let found = editors::found_by_name(&text);
    let edited = editors::set_enabled_zed(&text, on, None);
    if edited == text {
        return Ok(found);
    }
    // Содержимое — после первого перевода строки, байтами на stdin команды
    // `cat > …`, а не подстановкой в саму команду (решение владельца №2):
    // `Tunnel::exec` делит по первому `\n` и шлёт хвост отдельным кадром.
    exec(&format!("cat > {TMP}\n{edited}"))?;
    exec(&format!("mv {TMP} {PATH}"))?;
    Ok(found)
}

/// Последнее увиденное по SSH состояние записей `1c`/`vrunner` в настройках Zed в контейнере — материал факта набора готовности (`verify::editor_mcp_fact`). Живёт дольше одного сеанса (у окна, рядом с `enabled`), потому что расчёт готовности идёт своим тактом, независимо от того, подключается туннель прямо сейчас или нет. `None` — ещё не читали (до первого удачного подключения) либо последнее чтение отказало.
pub type EditorMcpSeen = Arc<Mutex<Option<editors::Found>>>;

/// Текущее значение `EditorMcpSeen` для расчёта готовности — без паники на отравленном мьютексе (Minor 7 ревью): расчёт важнее одной утерянной записи, и `.lock().ok()` молча отвечает «не знаем», а не валит поток такта.
pub fn seen(cell: &EditorMcpSeen) -> Option<editors::Found> {
    cell.lock().ok().and_then(|g| g.clone())
}

/// Поглощает результат `flip_with` в `seen` — единое место, которое не разойдётся с тем, что реально делает `flip_remote_servers` (Important 4 ревью задачи: тест на «старое значение не переживает отказ» зовёт эту же функцию, а не копирует её строку себе). Ошибка стирает прежнее значение на `None`, а не хранит старую оценку: раз читать не вышло СЕЙЧАС, выдавать за свежую правду старое — врать о том, что мы знаем прямо сейчас (решение владельца 29.10, третье состояние факта — «не смог проверить», а не старая оценка). Отравленный мьютекс (Minor 7 ревью) — не повод валить расчёт сеанса: `.lock().ok()` молча пропускает обновление, а не паникует.
fn remember(seen: &EditorMcpSeen, r: &Result<editors::Found, String>) {
    if let Ok(mut g) = seen.lock() {
        *g = r.as_ref().ok().cloned();
    }
}

/// Текст предупреждения для отказа правки — решение владельца №3: связь важнее настройки редактора, и вызывающий (`tunnel::run`) зовёт этим текстом `Event::Warn`, никогда не оборачивая отказ правки в отказ подключения (Important 4 ревью задачи: тест, что это именно предупреждение, а не отказ сеанса, зовёт этот же текст, а не только `flip_with::is_err()`).
pub fn flip_warning(on: bool, e: &str) -> String {
    let what = if on { "включены" } else { "выключены" };
    format!("серверы MCP у Zed в среде не {what}: {e}")
}

/// Обёртка `flip_with`, подставляющая настоящий канал — `Tunnel::exec` по уже поднятому туннелю. `Tunnel::exec` асинхронный, а канал `flip_with` ждёт обычный (синхронный) — `block_in_place` уводит текущую задачу туннеля на отдельный поток рантайма ровно на время этих команд, не блокируя остальных (рантайм здесь многопоточный — `tunnel::run` зовётся из потока, поднявшего его `Runtime::new()`, см. `start_with_enabled`).
///
/// Важное свойство (Important 5 финального ревью): эта функция не отказывает на однопоточном
/// рантайме — она ПАНИКУЕТ. `block_in_place` требует многопоточный `Runtime` (`tokio::test` без
/// `flavor = "multi_thread"` поднимает однопоточный) — позвать отсюда на нём значит упасть, а не
/// получить `Err`. Живой тест `tunnel_live.rs` уже ловил эту панику вместо честного «SSH недоступен».
pub fn flip_remote_servers(t: &tunnel::Tunnel, on: bool, seen: &EditorMcpSeen) -> Result<(), String> {
    let r = tokio::task::block_in_place(|| {
        let exec = |cmd: &str| -> Result<String, String> { tokio::runtime::Handle::current().block_on(t.exec(cmd)) };
        flip_with(&exec, on)
    });
    remember(seen, &r);
    r.map(|_| ())
}

/// Как `start_with_enabled`, но с MCP всегда включённым — для CLI
/// (`main.rs`: `connect`/`serve`), у которого нет окна и некому держать
/// переключатель дольше одного сеанса.
/// Строка журнала про ушедшего ребёнка моста — или её отсутствие.
///
/// Три исхода, и путать их нельзя:
///
/// * ушёл сам и по-хорошему (код 0) — молчим, это конец разговора с агентом;
/// * **сняли мы** (`by_us`: агент отключился, а ребёнок не ушёл за 5 с по
///   закрытию входа) — молчим тоже: это наша штатная уборка за отключившимся
///   агентом, человек ничего не делал и делать ему нечего;
/// * ушёл сам с ненулевым кодом — вот это и есть беда, и только тут Warn.
///
/// Пустого «после двоеточия ничего» не бывает: ребёнок, не сказавший ничего,
/// так и называется словами. Живой случай 25.10 на стенде — «mcp-1c
/// завершился с кодом -1: », где `-1` означал «снят сигналом», то есть нашу
/// же уборку, а пустота после двоеточия — что процесс ничего не написал в
/// stderr. Человек читал это как поломку (STATUS, пункт 25). Тогда текст
/// сделали человеческим, но саму строку оставили — и в покое, пока Zed
/// переподключается, журнал наполнялся «mcp-1c остановлен: агент
/// отключился» на каждый цикл (находка владельца 26.10). Говорить нечего —
/// молчим.
///
/// Чистая: решение о строке проверяется тестом без процессов и сокетов.
pub(crate) fn child_exit_line(who: &str, code: i32, by_us: bool, tail: &str) -> Option<Event> {
    let said = tail.trim();
    let because = if said.is_empty() { String::new() } else { format!(": {said}") };
    if by_us {
        return None;
    }
    if code == 0 {
        return None;
    }
    let what =
        if code < 0 { format!("{who} снят сигналом") } else { format!("{who} завершился с кодом {code}") };
    let because = if said.is_empty() { " — на stderr он ничего не сказал".to_string() } else { because };
    Some(Event::Warn(format!("{what}{because}")))
}

pub fn start(cfg: Cfg, kit: Kit, log: Sink, on_state: impl Fn(State) + Send + Sync + 'static) -> Session {
    start_with_enabled(cfg, kit, log, Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(None)), on_state)
}

/// Поднять сеанс в своём потоке. `log` получает все события (туннеля и
/// моста), `on_state` — каждый переход состояния. `enabled` — переключатель
/// «обслуживаются ли вызовы агента»: заводит и держит его окно (`gui::Ui`),
/// он живёт дольше любого одного сеанса (переживает «Сохранить» и
/// переключатель в локальном режиме), сюда только передаётся, а не
/// создаётся заново на каждый перезапуск. Транспорт — по `cfg.host.mode`:
/// `tunnel` держит SSH с переподключением, `local` слушает оба порта на
/// петле сам (всё на одной машине — переподключаться там не от чего,
/// `Retrying` в этом режиме не бывает). `editor_mcp` — та же ячейка, тем же
/// правилом: заводит и держит окно, сюда только передаётся, и расчёт
/// готовности (`verify.rs`) читает её независимо от того, идёт сеанс прямо
/// сейчас или нет (решение владельца 29.10).
pub fn start_with_enabled(
    cfg: Cfg,
    kit: Kit,
    log: Sink,
    enabled: Arc<AtomicBool>,
    editor_mcp: EditorMcpSeen,
    on_state: impl Fn(State) + Send + Sync + 'static,
) -> Session {
    let (stop_tx, stop_rx) = watch::channel(false);
    let (done_tx, done_rx) = oneshot::channel();
    // Общая ячейка параметров: заведена здесь (не внутри `Serve::new`), чтобы
    // и поток сеанса, и вызывающий (через `Session::update_cfg`) держали один
    // и тот же `Live` — иначе подмена снаружи не дошла бы до того `Serve`,
    // что реально обслуживает соединения.
    let shared_cfg = Live::new(cfg.clone());
    let session_cfg = shared_cfg.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                on_state(State::Failed(e.to_string()));
                let _ = done_tx.send(Err(e.into()));
                return;
            }
        };
        let r = rt.block_on(async {
            on_state(State::Connecting);
            let state = Mutex::new(State::Connecting);
            let serve = Arc::new(Serve::shared(shared_cfg, &cfg, &kit, log.clone(), cfg.ports(), enabled));
            let on_event = |e: Event| {
                let mut s = state.lock().unwrap();
                let next = s.clone().next(&e);
                log(for_log(&e, next.is_up()));
                *s = next;
                on_state(s.clone());
            };
            let r = if cfg.host.mode == "local" {
                crate::listen::run(serve, cfg.ports(), on_event, stop_rx).await
            } else {
                let dispatch = serve.clone();
                let on_incoming = move |inc: tunnel::Incoming| {
                    dispatch.dispatch(inc.port, inc.channel.into_stream());
                };
                tunnel::run(&cfg, kit.state_path("known_hosts"), on_event, on_incoming, stop_rx, editor_mcp.clone()).await
            };
            on_state(State::ended(&r));
            r
        });
        // Задачи моста (и дети под ними) живут на этом `rt` — уронить его
        // здесь и ждёт их завершения раньше, чем окно узнает «не подключён»;
        // иначе человек видит разрыв, а `oscript` под ним ещё жив и держит
        // единственный сеанс учебной лицензии в образе.
        drop(rt);
        let _ = done_tx.send(r);
    });
    Session { stop: stop_tx, done: Some(done_rx), cfg: session_cfg }
}

/// Что отвечает на соединение, принятое на порт: на mcp-1c — мост
/// `pipe_jsonrpc` с процессом mcp-1c на stdio, на vrunner — тот же мост с
/// процессом `vrunner-mcp`, на порту веб-клиента — локальный веб-сервер.
/// Один разбор «порт → мост» на оба транспорта: туннель отдаёт сюда канал
/// russh как поток (`into_stream`), локальный слушатель — `TcpStream`; оба —
/// через один и тот же `dispatch`.
pub struct Serve {
    /// Общая ячейка: «Сохранить» (`Session::update_cfg`) подменяет её на
    /// лету, и мост (`bridge::pipe_jsonrpc`) перечитывает её на каждый
    /// запрос — не только при приёме соединения (см. заголовок сессии).
    cfg: Live,
    kit: Kit,
    log: Sink,
    ports: Ports,
    web_port: u16,
    tools_dir: std::path::PathBuf,
    enabled: Arc<AtomicBool>,
}

impl Serve {
    /// `ports` приходит от вызывающего (а не считается заново из `cfg`
    /// внутри), чтобы тесты могли завести слушатели на эфемерных портах и
    /// `Serve` узнавал об этом напрямую, не трогая 777x/778x. Параметры здесь
    /// неподменяемы (своя ячейка на один раз) — для этого у вызывающего,
    /// которому нужна подмена на ходу, есть `shared` ниже.
    pub fn new(cfg: &Cfg, kit: &Kit, log: Sink, ports: Ports, enabled: Arc<AtomicBool>) -> Serve {
        Serve::shared(Live::new(cfg.clone()), cfg, kit, log, ports, enabled)
    }

    /// Как `new`, но с уже заведённой снаружи общей ячейкой `cfg` — чтобы
    /// `start_with_enabled` мог отдать тот же `Live` и в `Session`
    /// (`update_cfg` подменяет её, не трогая `Serve`), а `ops_host::run_mcp` —
    /// своему потоку опроса (`watch_mcp`), который кладёт в неё свежий `Cfg`
    /// после «Сохранить» в окне или правки toml руками (находка ревью задачи
    /// 3, Important 3: без общей ячейки редактор навсегда работал бы со
    /// строкой подключения, бывшей на момент старта). `pub(crate)`, не `pub`:
    /// только этот крейт заводит свою `Live` заранее, снаружи о ней знать
    /// незачем. `cfg_now` — разовое чтение того, что не должно меняться на
    /// ходу одного сеанса (порт веб-клиента): смена режима/портов и так
    /// перезапускает сеанс целиком.
    pub(crate) fn shared(cfg: Live, cfg_now: &Cfg, kit: &Kit, log: Sink, ports: Ports, enabled: Arc<AtomicBool>) -> Serve {
        // Инструменты берутся из комплекта по месту: в профиль ничего не
        // переносится (леджер комплекта, пункт 15).
        let tools_dir = kit.root.clone();
        Serve { cfg, kit: kit.clone(), log, ports, web_port: cfg_now.web_port(), tools_dir, enabled }
    }

    /// Разобрать одно принятое соединение: порт решает, какой процесс за ним
    /// стоит. Неизвестный порт — `None`, ничего не запускается и `stream`
    /// просто закрывается, когда функция вернётся. Строка подключения,
    /// пользователь базы и каталог проекта мост берёт не отсюда (однократно
    /// на соединение), а на каждый запрос внутри себя (`rules.live`) — Zed
    /// держит это соединение весь сеанс и сам не переподключается, так что
    /// однократного чтения здесь было бы недостаточно (см. `bridge::pipe_jsonrpc`).
    pub fn dispatch<S>(&self, port: u16, stream: S) -> Option<tokio::task::JoinHandle<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        if port == self.ports.vrunner {
            let (_, cfg0) = self.cfg.get();
            let tools_dir = self.tools_dir.clone();
            let cmd0 = Ok(bridge::vrunner_command(&cfg0, &tools_dir));
            let rebuild: crate::bridge::Rebuild = {
                let tools_dir = tools_dir.clone();
                Arc::new(move |cfg: &Cfg| Ok(bridge::vrunner_command(cfg, &tools_dir)))
            };
            let rules = bridge::Rules::for_vrunner(
                self.cfg.clone(),
                tools_dir,
                rebuild,
                Arc::new(bridge::configurator_count),
                self.enabled.clone(),
            );
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            Some(tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, by_us, err| {
                if let Some(e) = child_exit_line("vrunner-mcp", code, by_us, &err) {
                    log_exit(e);
                }
            })))
        } else if port == self.ports.mcp {
            let (_, cfg0) = self.cfg.get();
            let kit = self.kit.clone();
            // Страж перед сборкой (круг исправления 2, пункт 1, находка
            // автоматической проверки безопасности): `mcp_command_guarded`
            // отказывает, не собрав команду (и не положив пароль в
            // окружение), когда владелец локального порта публикации —
            // не наш httpd.
            let cmd0 = bridge::mcp_command_guarded(&cfg0, &kit);
            let mut rules = bridge::Rules::for_mcp(self.enabled.clone());
            rules.live = self.cfg.clone();
            rules.rebuild = Some(Arc::new(move |cfg: &Cfg| bridge::mcp_command_guarded(cfg, &kit)));
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            Some(tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, by_us, err| {
                if let Some(e) = child_exit_line("mcp-1c", code, by_us, &err) {
                    log_exit(e);
                }
            })))
        } else if port == self.ports.web {
            let log = self.log.clone();
            let web_port = self.web_port;
            Some(tokio::spawn(async move {
                if let Err(e) = bridge::pipe_tcp(stream, web_port).await {
                    log(Event::Warn(e));
                }
            }))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops_host::{port_of, McpServer};
    use std::path::Path;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Канал подменён: проверяем, что мы читаем файл, правим своей же
    /// функцией и пишем обратно — ни одного `sed` со стороны (задача 7).
    #[test]
    fn flipping_the_remote_file_reads_modifies_writes() {
        let seen = std::sync::Mutex::new(Vec::new());
        let exec = |cmd: &str| -> Result<String, String> {
            seen.lock().unwrap().push(cmd.to_string());
            Ok(crate::editors::merge_zed("", Path::new("/opt/1c-connect")).unwrap())
        };
        // Решение владельца 29.10: то же чтение, что правит `enabled`, несёт и материал факта набора готовности — обе наши записи на месте.
        assert_eq!(flip_with(&exec, false).unwrap(), crate::editors::Found::Ours);
        let seen = seen.lock().unwrap();
        assert!(seen[0].contains("cat") && seen[0].contains(".config/zed/settings.json"), "{:?}", seen[0]);
        assert!(seen[1].contains("\"enabled\": false"), "{:?}", seen[1]);
    }

    /// Файла нет (или `cat` отказал) — отказ `flip_with`, но сеанс от этого не падает: проверяем сам текст, который `tunnel::run` кладёт в `Event::Warn` (`flip_warning`) — именно он превращает отказ правки в предупреждение журнала, а не в отказ подключения (Important 4 ревью: именем теста обещано ровно это, а не только `is_err()`).
    #[test]
    fn a_missing_remote_file_is_not_an_error_for_the_session() {
        let exec = |_: &str| -> Result<String, String> { Err("No such file".into()) };
        let e = flip_with(&exec, true).unwrap_err();
        let warn = flip_warning(true, &e);
        assert!(warn.contains("No such file"), "{warn}");
        // Слово о подключении здесь не звучит вовсе — это и есть разница
        // между предупреждением и отказом сеанса.
        assert!(!warn.contains("подключ"), "{warn}");
    }

    /// Старый образ без наших записей — факт обязан увидеть именно это, а не молча считать «настроено» (решение владельца 29.10).
    #[test]
    fn flip_with_reports_missing_records_too() {
        let exec = |_: &str| -> Result<String, String> { Ok("{}".to_string()) };
        assert_eq!(flip_with(&exec, true).unwrap(), crate::editors::Found::Absent);
    }

    /// `flip_remote_servers` стирает прежнее `seen` на `None` при отказе, а не оставляет старую оценку — «не смог проверить», а не «опять как в прошлый раз» (решение владельца 29.10). Зовёт `remember` — ровно то, что `flip_remote_servers` зовёт сама (Important 4 ревью: тест не копирует её строку себе, иначе остался бы зелёным при регрессии обёртки).
    #[test]
    fn a_stale_seen_value_is_not_kept_on_a_fresh_failure() {
        let seen: EditorMcpSeen = Arc::new(Mutex::new(Some(crate::editors::Found::Ours)));
        let r: Result<crate::editors::Found, String> = Err("связь оборвалась".into());
        remember(&seen, &r);
        assert_eq!(*seen.lock().unwrap(), None, "старое «настроено» не должно переживать отказ чтения");
    }

    /// Набор, который `Serve::new` принимает без лишних вопросов: сервер
    /// файловой базы на выдуманном рабочем месте `dev1` — те же значения, что
    /// и у остальных тестов этого файла (`filled_for_test`).
    fn filled() -> Cfg {
        crate::cfg::filled_for_test()
    }

    /// Порты для теста — не 777x/778x настоящей машины, а те, что выводит
    /// сам `Cfg::ports()` по номеру рабочего места `filled()`.
    fn ports() -> Ports {
        filled().ports()
    }

    fn tempdir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("onec-session-mcp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn kit_in(dir: std::path::PathBuf) -> Kit {
        Kit::at(dir)
    }

    /// Пишет запрос агента в `stream` и отдаёт первую строку ответа — тем же
    /// приёмом, что `bridge.rs` гоняет `tokio::io::duplex` в своих тестах, но
    /// без ожидания и закрытия: мосту этого соединения закрывать не нужно,
    /// пока тест сам не позвал `task.abort()`.
    async fn ask<S>(stream: S, request: &[u8]) -> String
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (r, mut w) = tokio::io::split(stream);
        w.write_all(request).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(r);
        tokio::time::timeout(std::time::Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("мост обязан ответить сам, без ребёнка")
            .unwrap();
        line
    }

    #[tokio::test]
    async fn the_stdio_path_answers_initialize_without_a_child() {
        // Комплекта в тестах нет, ребёнок не поднимется — и это главный случай:
        // мост обязан ответить сам, иначе Zed сочтёт сервер упавшим.
        let (mine, theirs) = tokio::io::duplex(64 * 1024);
        let serve = Serve::new(&filled(), &kit_in(tempdir()), Arc::new(|_| {}), ports(), Arc::new(AtomicBool::new(true)));
        let task = serve.dispatch(ports().mcp, theirs).unwrap();
        let reply = ask(mine, br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).await;
        assert!(reply.contains("\"result\""), "{reply}");
        task.abort();
    }

    #[test]
    fn the_server_argument_picks_the_port() {
        assert_eq!(port_of(McpServer::OneC, ports()), ports().mcp);
        assert_eq!(port_of(McpServer::Vrunner, ports()), ports().vrunner);
    }

    /// Два смысла одного события разведены: состояние читает `Ok` как «канал
    /// поднят», журнал показывает тот же момент нейтрально. Проверяем оба —
    /// 30.10 попытка добиться второго сменой события в транспорте отняла
    /// первое, и карточка навсегда застряла на «Включаю…».
    #[test]
    fn the_rise_of_the_channel_is_a_state_signal_and_a_quiet_journal_line() {
        let up = State::Connecting.next(&Event::Ok("MCP включены: порты 7771 и 7781".into()));
        assert!(up.is_up(), "по `Ok` связь обязана стать поднятой");
        match for_log(&Event::Ok("MCP включены: порты 7771 и 7781".into()), up.is_up()) {
            Event::Say(t) => assert!(t.contains("7771"), "{t}"),
            other => panic!("подъём канала в журнале обязан быть нейтральным, а не зелёным: {other:?}"),
        }
        // Всё прочее идёт в журнал как есть: понижается ровно подъём.
        assert!(matches!(for_log(&Event::Ok("иное".into()), false), Event::Ok(_)));
        assert!(matches!(for_log(&Event::Warn("оборвалась".into()), false), Event::Warn(_)));
        assert!(matches!(for_log(&Event::Fail("отказ".into()), false), Event::Fail(_)));
    }

    #[test]
    fn the_tunnel_events_drive_the_state() {
        let s = State::Connecting.next(&Event::Ok("туннель поднят".into()));
        assert!(s.is_up() && s.is_on());
        let s = s.next(&Event::Warn("оборвалась".into()));
        assert_eq!(s, State::Retrying("оборвалась".into()));
        assert!(s.is_on() && !s.is_up());
        // Lines that are not verdicts change nothing.
        assert_eq!(s.clone().next(&Event::Say("x".into())), s);
        assert_eq!(State::ended(&Ok(())), State::Off);
        let f = State::ended(&Err(anyhow::anyhow!("пароль")));
        assert!(matches!(&f, State::Failed(w) if w.contains("пароль")) && !f.is_on());
    }

    #[test]
    fn labels_say_how_long_and_why() {
        assert_eq!(State::Up { since: Instant::now() }.label("tunnel"), "подключён 0 мин");
        assert!(State::Retrying("нет связи".into()).label("tunnel").contains("нет связи"));
        assert_eq!(State::Off.label("tunnel"), "не подключён");
    }

    #[test]
    fn local_mode_labels_speak_about_mcp() {
        assert_eq!(State::Off.label("local"), "MCP выключены");
        assert_eq!(State::Connecting.label("local"), "включаю…");
        assert_eq!(State::Up { since: Instant::now() }.label("local"), "MCP включены 0 мин");
        // Отказ — сама причина, без слова «подключён»: в локальном режиме
        // связи со средой нет, есть только слушатели на петле.
        assert_eq!(State::Failed("порт занят".into()).label("local"), "порт занят");
    }

    /// Находка ревью задачи 3 (второй проход): `set` на тех же параметрах не
    /// обязан двигать поколение — иначе такт опроса, перечитывающий
    /// неизменный toml раз в 200 мс (`ops_host::watch_mcp`), бил бы его
    /// каждый тик, и `bridge::pipe_jsonrpc` рестартовал бы ребёнка
    /// практически на каждый вызов инструмента вместо «после правки
    /// параметров». Второе — ближе к самой болезни: такт перечита на
    /// неизменном файле (здесь — повторный `set` тем же `Cfg`, тот же вызов,
    /// каким `watch_mcp` кладёт свежепрочитанный toml) не меняет поколение;
    /// менявшийся `Cfg` — меняет.
    #[test]
    fn set_bumps_the_generation_only_on_a_real_change() {
        let cfg = crate::cfg::filled_for_test();
        let live = Live::new(cfg.clone());
        let (gen0, _) = live.get();
        // Тик перечита на неизменном файле: тот же `Cfg`, тот же вызов.
        live.set(cfg.clone());
        let (gen1, unchanged) = live.get();
        assert_eq!(gen0, gen1, "одни и те же параметры не обязаны двигать поколение");
        assert_eq!(unchanged, cfg);
        let mut changed = cfg.clone();
        changed.host.base_path = "C:\\Bases\\Other".into();
        live.set(changed.clone());
        let (gen2, got) = live.get();
        assert_ne!(gen1, gen2, "настоящая смена параметров обязана двинуть поколение");
        assert_eq!(got, changed);
    }

    /// Круг исправления 2, пункт 4: улаженный порт — факт сеанса, и такт
    /// наблюдателя (`watch_mcp`, каждые 200 мс кладёт голый `Cfg::load`,
    /// без `resolved_http_port`) не имеет права его стереть. Без переноса в
    /// `set` `http_port()`/`base_url()` после такого такта откатывались бы
    /// на выведенный порт — `--base` для `mcp-1c` указывал бы не туда, где
    /// слушает httpd, ровно тот же дефект, ради которого вся задача писалась.
    #[test]
    fn a_watcher_tick_does_not_erase_the_port_settled_this_session() {
        let mut settled = crate::cfg::filled_for_test();
        settled.resolved_http_port = Some(9100);
        let live = Live::new(settled.clone());
        assert_eq!(live.get().1.http_port(), 9100);

        // Такт наблюдателя: тот же файл, перечитанный с диска голым
        // `Cfg::load` — `resolved_http_port` в нём пуст, как у любого
        // свежего `Cfg`.
        let reloaded = crate::cfg::filled_for_test();
        assert_eq!(reloaded.resolved_http_port, None);
        live.set(reloaded);

        let after = live.get().1;
        assert_eq!(after.http_port(), 9100, "улаженный порт обязан пережить такт наблюдателя");
        assert_eq!(after.base_url(), "http://127.0.0.1:9100/hs/mcp-1c");
    }

    /// Главная опасность 6B: вызов, получивший строку подключения одной базы
    /// и каталог проекта другой. `Live::get` обязана отдавать поколение и
    /// `Cfg` одним захватом мьютекса — никогда вперемешку, сколько бы
    /// `set` ни звал параллельный поток.
    #[test]
    fn live_cfg_never_hands_out_a_mix_of_two_configurations() {
        let mut a = crate::cfg::filled_for_test();
        a.host.base_path = "C:\\Bases\\A".into();
        a.host.db_user = "UserA".into();
        let mut b = crate::cfg::filled_for_test();
        b.host.base_path = "C:\\Bases\\B".into();
        b.host.db_user = "UserB".into();
        let live = Live::new(a.clone());
        let writer_live = live.clone();
        let (wa, wb) = (a.clone(), b.clone());
        let writer = std::thread::spawn(move || {
            for _ in 0..2000 {
                writer_live.set(wb.clone());
                writer_live.set(wa.clone());
            }
        });
        for _ in 0..20_000 {
            let (_, cfg) = live.get();
            let whole_a = cfg.host.base_path == a.host.base_path && cfg.host.db_user == a.host.db_user;
            let whole_b = cfg.host.base_path == b.host.base_path && cfg.host.db_user == b.host.db_user;
            assert!(whole_a || whole_b, "смешались половины двух конфигураций: {:?}", cfg.host);
        }
        writer.join().unwrap();
    }

    #[test]
    fn a_session_that_cannot_dial_ends_by_itself_and_stop_is_still_fine() {
        // Nothing listens there: the first attempt is terminal (no session
        // ever existed), so wait() returns the error and stop() after it
        // does not hang.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.server = format!("127.0.0.1:{port}");
        let dir = std::env::temp_dir().join(format!("onec-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let mut s = start(cfg, Kit::at(&dir), std::sync::Arc::new(|_| {}), move |st| seen2.lock().unwrap().push(st));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(s.wait());
        assert!(r.is_err(), "{r:?}");
        assert!(s.stop().is_ok());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.first(), Some(&State::Connecting));
        assert!(matches!(seen.last(), Some(State::Failed(_))), "{seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `finished` отвечает по самому сеансу и не ждёт его: это условие
    /// повторной попытки после отказа bind (`gui::needs_session`) — в
    /// `Option` лежит и живой сеанс, и тот, чей поток уже ушёл, и путать их
    /// нельзя ни в ту, ни в другую сторону.
    #[test]
    fn finished_tells_a_live_session_from_one_that_ended_by_itself() {
        let dir = std::env::temp_dir().join(format!("onec-session-finished-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Живой: на том конце слушает немой сокет — рукопожатие не кончается
        // ни успехом, ни отказом, пока слушатель жив.
        let quiet = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.server = format!("127.0.0.1:{}", quiet.local_addr().unwrap().port());
        let mut live = start(cfg, Kit::at(&dir), std::sync::Arc::new(|_| {}), |_| {});
        assert!(!live.finished(), "сеанс только что заведён и никуда не ушёл");
        // Не `stop`: рукопожатие ждёт до 15 с, а тесту ждать нечего — роняем
        // значение (тот же сигнал, но без ожидания) и отпускаем слушателя.
        drop(live);
        drop(quiet);

        // Кончившийся сам: звонить некуда, первая попытка терминальная.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.server = format!("127.0.0.1:{port}");
        let mut dead = start(cfg, Kit::at(&dir), std::sync::Arc::new(|_| {}), |_| {});
        let ended = (0..200).any(|_| {
            if dead.finished() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
            false
        });
        assert!(ended, "сеанс, которому некуда звонить, кончается сам");
        assert!(dead.finished(), "второй вопрос — тот же ответ");
        assert!(dead.stop().is_ok(), "остановить кончившийся сеанс не беда");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Три исхода ребёнка моста, и ни одного пустого «после двоеточия».
    /// Живой случай 25.10: в журнале стенда висело «mcp-1c завершился с кодом
    /// -1: » — на деле наша же уборка после отключения агента.
    #[test]
    fn a_child_that_went_away_is_reported_by_what_actually_happened() {
        // Сняли мы — штатная уборка за отключившимся агентом: человек ничего
        // не делал, ничего не сломалось, и строки быть не должно вовсе. Zed
        // переподключается постоянно, и прежняя строка сыпалась в журнал на
        // каждый цикл (находка владельца 26.10).
        assert!(child_exit_line("mcp-1c", -1, true, "").is_none(), "о своей уборке говорить нечего");
        // Даже когда ребёнок что-то сказал на stderr: сняли его мы, и это
        // по-прежнему не беда.
        assert!(child_exit_line("vrunner-mcp", -1, true, "interrupted").is_none(), "сняли мы — молчим и с хвостом stderr");

        // Ушёл сам и по-хорошему — молчим.
        assert!(child_exit_line("mcp-1c", 0, false, "").is_none());

        // Ушёл сам с ошибкой и ничего не сказал — так и пишем словами, а не
        // пустотой после двоеточия.
        let e = child_exit_line("vrunner-mcp", 3, false, "  ").expect("ненулевой код — предупреждение");
        match e {
            Event::Warn(t) => {
                assert!(t.contains("с кодом 3"), "{t}");
                assert!(t.contains("ничего не сказал"), "{t}");
                assert!(!t.trim_end().ends_with(':'), "строка не должна обрываться двоеточием: {t}");
            }
            other => panic!("{other:?}"),
        }

        // Сказал — причина едет как есть.
        let e = child_exit_line("vrunner-mcp", 3, false, "платформа не найдена").unwrap();
        assert!(matches!(e, Event::Warn(ref t) if t.contains("платформа не найдена")), "{e:?}");

        // Снят сигналом НЕ нами — про код -1 тоже не говорим числом.
        let e = child_exit_line("mcp-1c", -1, false, "").unwrap();
        assert!(matches!(e, Event::Warn(ref t) if t.contains("снят сигналом")), "{e:?}");
    }
}
