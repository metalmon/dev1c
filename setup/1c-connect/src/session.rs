//! Один сеанс подключения: поток со своим tokio, туннель с переподключением
//! и мост на проброшенные порты. Общая часть команд `connect`/`serve` и
//! окна: все зовут `start_with_enabled`, держат `Session` и в конце — `stop`.
//! Короткая `start` на четыре довода осталась только тестам (её докстрока):
//! с задачи 4 плана «Сопряжение переезжает в бек» у работающей программы
//! всегда есть проводка к узлу, и отдавать её пустой некому.

use crate::bridge;
use crate::cfg::{Cfg, Ports};
use crate::ops_host::McpServer;
use crate::SUBJECT;
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
    /// Выбранное в этом сеансе — улаженный порт публикации и доставшийся
    /// учётке сдвиг моста (`Cfg::carry_session_facts` называет их все) — факт
    /// СЕАНСА, не файла, и перечитывание toml не вправе его стирать (круг
    /// исправления 2, пункт 4): `watch_mcp` каждые 200 мс кладёт сюда
    /// `Cfg::load(kit)` без `ops_host::settle_chosen_ports` (его там и не
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
        cfg.carry_session_facts(&g.1);
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

    /// Слова по `cfg.current().mode`: в туннельном — как раньше, в локальном —
    /// про MCP (слушатели держат порты сами, переподключаться там не от
    /// чего — см. `start`), а не про сеанс связи со средой.
    pub fn label(&self, mode: &str) -> String {
        let local = mode == "local";
        match self {
            State::Off => {
                if local {
                    "Доступ к 1С выключен".into()
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
                let verb = if local { format!("{SUBJECT} включён") } else { "подключён".to_string() };
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
    // Задача 4 плана «тонкий срез ценности», шаг 2а: сторож сборки
    // (`pack.sh`) не даст собрать комплект с несовместимой парой
    // коннектор/mcp-1c, но есть дорога мимо него — положить новый
    // `1c-connect.exe` в уже собранный комплект, не трогая `mcp-1c` рядом
    // (обычный путь владельца). Сторож тогда не зовётся, и ребёнок умирает
    // на старте с этой фразой Go-пакета `flag`. Ключ добавили мы
    // (`bridge.rs::mcp_command`), а умер чужой бинарь — без подсказки
    // человеку узнать, что чинить, неоткуда.
    let hint = if who == "mcp-1c" && said.contains("flag provided but not defined") {
        " — в комплекте старый mcp-1c, он не понимает ключей этой версии коннектора; замените mcp-1c сборкой форка"
    } else {
        ""
    };
    Some(Event::Warn(format!("{what}{because}{hint}")))
}

/// Сеанс без проводки к узлу — только для тестов. Работающая программа зовёт `start_with_enabled`: сеанс и поток узла поднимает
/// один и тот же хозяин (окно, `connect`, `serve`), и проводка между ними у него на руках. До задачи 4 плана «Сопряжение переезжает
/// в бек» этим коротким путём ходили `connect` и `serve` — и оставались без узла, то есть без канала.
#[cfg(test)]
pub fn start(cfg: Cfg, kit: Kit, log: Sink, on_state: impl Fn(State) + Send + Sync + 'static) -> Session {
    start_with_enabled(cfg, kit, log, Arc::new(AtomicBool::new(true)), Wiring::default(), on_state)
}

/// Поднять сеанс в своём потоке. `log` получает все события (туннеля и
/// моста), `on_state` — каждый переход состояния. `enabled` — переключатель
/// «обслуживаются ли вызовы агента»: заводит и держит его окно (`gui::Ui`),
/// он живёт дольше любого одного сеанса (переживает «Сохранить» и
/// переключатель в локальном режиме), сюда только передаётся, а не
/// создаётся заново на каждый перезапуск. Транспорт — по одной оси, где
/// база: здесь — свой мост, на другом компьютере — труба в канал (`Toward`).
/// Переподключаться сеансу не от чего: `Retrying` остался словом состояния,
/// которое с уходом ssh никто больше не присылает.
/// Чем увести местное соединение агента в канал — то же, что `listen::ToChannel`, названное здесь, чтобы звать сеанс одним именем.
pub type ToChannel = crate::listen::ToChannel;

/// Поток, пришедший на обслуживание, ВМЕСТЕ с тем, что известно о второй стороне.
///
/// Потолок записи той машины — свойство ЭТОГО потока, а не машины: он услышан в её `hello` и кончается вместе с разговором. Прежде
/// он ехал в `Serve::serve` отдельным доводом, и три зовущих из четырёх писали `Ceiling::Full` рукой. Довод, у которого есть
/// «безобидное» значение, рано или поздно получает его по умолчанию: новый зовущий напишет `Full`, соберётся и молча отдаст
/// пишущие инструменты стороне, объявившей у себя «только чтение». Ровно эта дыра и была найдена 11.11 (`f36f5cb`).
///
/// Здесь разрешающего ЗНАЧЕНИЯ нет вовсе — есть названный СЛУЧАЙ. `Local` читается как утверждение «второй стороны нет», и
/// написать его про поток из канала нельзя не заметив.
pub enum Incoming<S> {
    /// Местный агент пришёл на петлю: второй стороны у этого потока нет вовсе, запрещать некому.
    Local(S),
    /// Поток вызовов по каналу: потолок ТОЙ машины приехал сюда вместе с потоком, из того же кадра `hello`, что и допуск.
    FromPeer(S, crate::wire::Ceiling),
}

impl<S> Incoming<S> {
    /// Поток и потолок порознь — для тех, кому нужно то и другое.
    fn split(self) -> (S, crate::wire::Ceiling) {
        match self {
            Incoming::Local(s) => (s, crate::wire::Ceiling::Full),
            Incoming::FromPeer(s, theirs) => (s, theirs),
        }
    }
}

/// Параметры, в которых потолок ТОЙ машины уже сложен со своим: `read_only`, когда та сторона у себя запретила запись.
///
/// Нужна ребёнку `mcp-1c`: его право записи — ключ `--access`, то есть оно попадает в команду при сборке, а не спрашивается на
/// каждый вызов (так делает vrunner, `bridge::Rules::peer`). Запрет любой стороны сильнее разрешения другой, поэтому ужесточение
/// здесь только в одну сторону: своё `read_only` чужим «можно» не отменяется.
pub(crate) fn tightened(cfg: &Cfg, theirs: crate::wire::Ceiling) -> Cfg {
    match theirs {
        crate::wire::Ceiling::Full => cfg.clone(),
        crate::wire::Ceiling::ReadOnly => cfg.clone().with_access("read_only"),
    }
}

/// Чем сеанс связан с потоком узла. Обе ручки про одно — вызовы агента по каналу, — и ездят поэтому вместе.
///
/// `serving` сеанс ЗАПОЛНЯЕТ собой (поток узла найдёт в ней мост, когда по каналу придёт поток вызовов); `to_channel` он ЧИТАЕТ
/// (им местное соединение агента уводится в канал, когда база на другом компьютере). Пустая `serving` — значит сеанса нет, и вызов
/// по каналу получает отказ словами, а не молчание.
///
/// `to_channel` пустым не бывает с задачи 4 плана «Сопряжение переезжает в бек»: узел поднимает всякий, кто держит сеанс — и окно, и
/// `connect`, и `serve`, — поэтому ручка приходит сюда всегда. Отказ «вести некуда» остался ровно там, где он правда («поток узла
/// кончился» — `Peer::calls`), и говорится он на вызов, а не на весь сеанс вперёд.
#[derive(Clone)]
pub struct Wiring {
    pub serving: Serving,
    pub to_channel: ToChannel,
    /// Порты, которые слушает сеанс. `None` — те, что у всех (`Cfg::ports()`, константы): так зовут окно и командная строка.
    ///
    /// Своё число передают только тесты, и ради них же это поле и есть: порты у программы константные, а тесту нужен живой сеанс,
    /// не отбирающий у настоящего коннектора его порт (до 11.11 «живым» в тестах был ssh-сеанс, который ничего не слушал).
    pub ports: Option<Ports>,
}

/// Проводка для теста: потока узла в тестах не поднимают, и ручка канала отвечает теми же словами, какими отвечает `Peer::calls`,
/// когда поток узла кончился. Отказ, а не пустота: «ручки нет» в работающей программе невыразимо (см. докстроку `Wiring`), и
/// фикстура не должна возвращать это в словарь с чёрного хода.
#[cfg(test)]
impl Default for Wiring {
    fn default() -> Self {
        Wiring { serving: Serving::default(), to_channel: Arc::new(|_, _| Err(crate::peer::NO_NODE.to_string())), ports: None }
    }
}

/// Обслуживающая половина ЭТОЙ машины — та, что поднимает мост к серверам инструментов, — выставленная наружу, пока сеанс жив.
///
/// **Зачем.** Канал живёт в потоке узла (`peer`): он обязан принимать знакомство, прощание и отзыв независимо от того, включено
/// ли обслуживание. Мост живёт в сеансе: он и ЕСТЬ обслуживание. Поток вызовов приходит в канал, а обслужить его должен сеанс — значит
/// между ними нужна одна явная ручка, и вот она.
///
/// **Пустота здесь — это ответ, а не беда.** Сеанса нет — доступ к 1С выключен, и поток вызовов получает отказ словами. Второго места,
/// где этот вопрос решался бы, не заводим: «включено ли» спрашивается тем, что ручка полна.
#[derive(Clone, Default)]
pub struct Serving(Arc<Mutex<Option<Open>>>);

/// Чем именно обслуживает этот сеанс — и до каких пор.
#[derive(Clone)]
pub struct Open {
    serve: Arc<Serve>,
    /// Жив ли ещё сеанс. Рвёт потоки вызовов вместе с собой: «выключил — значит выключено», а не «выключено для новых».
    stop: tokio::sync::watch::Receiver<bool>,
}

impl Open {
    pub fn serve(&self) -> &Arc<Serve> {
        &self.serve
    }

    /// Та ли это карточка, по которой мост поднимает базу ПРЯМО СЕЙЧАС.
    ///
    /// **Без этого вопроса вызов уехал бы в чужую базу:** допуск спрашивается по карточке из заголовка потока, а мост работает с
    /// текущим соединением машины — у человека с двумя карточками сторона, зачисленная ко второй, получила бы базу первой (находка
    /// проверки безопасности 11.11, пришедшая вместе с вызовами по каналу: прежде на том конце провода стояла среда разработки).
    ///
    /// Спрашивается она у `Serve`, а не у снимка, сделанного при подъёме сеанса: снимок был бы ВТОРОЙ мерой того же — мост читает
    /// соединение из живой ячейки на каждую просьбу, и расхождение жило бы ровно между ними (вторая проверка безопасности 11.11).
    pub fn is_for(&self, card: &crate::cfg::CardId) -> bool {
        &self.serve.current_card() == card
    }

    /// Ждать конца сеанса. Возвращается, когда обслуживание выключили.
    pub async fn until_stopped(&self) {
        let mut stop = self.stop.clone();
        while !*stop.borrow() {
            if stop.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Serving {
    /// Сеанс поднялся и готов обслуживать, пока не скажут `stop`.
    pub fn put(&self, serve: Arc<Serve>, stop: tokio::sync::watch::Receiver<bool>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(Open { serve, stop });
    }

    /// Сеанс кончился: обслуживать больше нечем.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Чем обслуживать прямо сейчас. `None` — доступ к 1С выключен.
    pub fn now(&self) -> Option<Open> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

pub fn start_with_enabled(
    cfg: Cfg,
    kit: Kit,
    log: Sink,
    enabled: Arc<AtomicBool>,
    wiring: Wiring,
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
            // Порты у всех одни (`ports()`): слушает их местный слушатель, и то же число знает `Serve`. Тройка, считавшаяся от
            // номера рабочего места (`tunnel_ports()`), уехала вместе со средой — пробрасывать её больше некому.
            let active_ports = wiring.ports.unwrap_or_else(|| cfg.ports());
            let serve = Arc::new(Serve::shared(shared_cfg, &cfg, &kit, log.clone(), active_ports, enabled));
            // Объявляем себя: с этой минуты поток узла знает, чем обслуживать вызовы, пришедшие по каналу. Снимается объявление
            // внизу, когда сеанс кончился, — и тем же самым снимается допуск к вызовам.
            wiring.serving.put(serve.clone(), stop_rx.clone());
            let on_event = |e: Event| {
                let mut s = state.lock().unwrap();
                let next = s.clone().next(&e);
                log(for_log(&e, next.is_up()));
                *s = next;
                on_state(s.clone());
            };
            // **Транспорт выбирает ОДНА ОСЬ — где база** (спека сочетаний, «Что показывается в каждом сочетании»). Слушаем свои
            // порты всегда: агент приходит на них в любом сочетании. Разница в том, что отвечает принятому соединению.
            //
            // База ЗДЕСЬ (сочетания 1, 2, 4) — свой мост со своим ребёнком. Вызовы, пришедшие к этой машине ПО КАНАЛУ, обслуживает не
            // сеанс напрямую, а поток узла через `Serving`, и трогать здесь нечего.
            //
            // База на ДРУГОМ компьютере (сочетания 3, 5) — трубой в канал: мост стоит у базы, здесь его нет и быть не должно.
            //
            // Третьего хода нет с 11.11: им был ssh в среду разработки, и среда уехала из программы целиком (план
            // `2026-11-11-env-removal.md`). Поднять туннель этой развилкой стало невыразимо, а не запрещено условием.
            let combo = crate::cfg::Combo::of(cfg.current());
            let toward = crate::listen::Toward::of(combo, wiring.to_channel.clone());
            let r = crate::listen::run(serve, active_ports, toward, on_event, stop_rx).await;
            // Сеанс кончился — снимаем объявление: обслуживать вызовы по каналу больше нечем, и отказ скажет об этом словами.
            wiring.serving.clear();
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
/// Один разбор «порт → мост» на оба транспорта: канал ко второй машине отдаёт
/// сюда свой поток, местный слушатель — `TcpStream`; оба — через один и тот же
/// `dispatch`.
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

    /// Порты, которые слушает этот сеанс. Нужны тому, кто переводит порт в слово (`Served::of_port`).
    pub fn ports(&self) -> Ports {
        self.ports
    }

    /// Обслужить одно принятое соединение: `what` говорит, какой процесс за ним стоит. Строка подключения, пользователь базы и каталог
    /// проекта мост берёт не отсюда (однократно на соединение), а на каждый запрос внутри себя (`rules.live`) — Zed держит это
    /// соединение весь сеанс и сам не переподключается, так что однократного чтения здесь было бы недостаточно
    /// (см. `bridge::pipe_jsonrpc`).
    ///
    /// **Выбор ключуется СЛОВОМ, а не портом** (шаг 0 плана пары B). Порт — свойство местного слушателя: его знает `listen::accept_for`,
    /// он же и переводит порт в слово (`Served::of_port`). По каналу порта нет вовсе — там вид процесса приезжает кадром, — и если бы
    /// выбор остался на порту, у одного понятия стало бы два словаря: номер здесь и слово на проводе.
    /// Карточка, чью базу мост поднимает ПРЯМО СЕЙЧАС — из той же живой ячейки, откуда её берёт сам мост.
    pub fn current_card(&self) -> crate::cfg::CardId {
        self.cfg.get().1.current().id()
    }

    pub fn serve<S>(&self, what: Served, incoming: Incoming<S>) -> tokio::task::JoinHandle<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        // Потолок ТОЙ машины приехал вместе с потоком (`Incoming`), а не отдельным доводом: забыть его стало невыразимо.
        //
        // Ребёнку mcp-1c он доезжает ключом `--access`, то есть попадает в команду при сборке; vrunner-mcp спрашивает его на каждый
        // вызов через `Rules`. Разные дороги у одного правила — потому что разные у самих серверов инструментов.
        let (stream, theirs) = incoming.split();
        let tighten = move |cfg: &Cfg| tightened(cfg, theirs);
        if what == Served::Mcp(McpServer::Vrunner) {
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
                theirs,
            );
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, by_us, err| {
                if let Some(e) = child_exit_line("vrunner-mcp", code, by_us, &err) {
                    log_exit(e);
                }
            }))
        } else if what == Served::Mcp(McpServer::OneC) {
            let (_, cfg0) = self.cfg.get();
            let kit = self.kit.clone();
            // Страж перед сборкой (круг исправления 2, пункт 1, находка
            // автоматической проверки безопасности): `mcp_command_guarded`
            // отказывает, не собрав команду (и не положив пароль в
            // окружение), когда владелец локального порта публикации —
            // не наш httpd.
            let cmd0 = bridge::mcp_command_guarded(&tighten(&cfg0), &kit);
            let mut rules = bridge::Rules::for_mcp(self.enabled.clone(), theirs);
            rules.live = self.cfg.clone();
            rules.rebuild = Some(Arc::new(move |cfg: &Cfg| bridge::mcp_command_guarded(&tighten(cfg), &kit)));
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, by_us, err| {
                if let Some(e) = child_exit_line("mcp-1c", code, by_us, &err) {
                    log_exit(e);
                }
            }))
        } else {
            let log = self.log.clone();
            let web_port = self.web_port;
            tokio::spawn(async move {
                if let Err(e) = bridge::pipe_tcp(stream, web_port, "публикация базы не поднята").await {
                    log(Event::Warn(e));
                }
            })
        }
    }
}

/// Что стоит за принятым соединением: мост к одному из серверов инструментов или наш веб-клиент.
///
/// Заведено шагом 0 плана пары B. Прежде на этот вопрос отвечал НОМЕР ПОРТА прямо внутри `Serve`, и это работало ровно до тех пор, пока
/// соединения приходили по TCP. Вызовы по каналу порта не несут вовсе: вид процесса приезжает в заголовке потока словом. Два словаря
/// одного понятия — номер в одном месте, слово в другом — и есть та самая «две меры на один вопрос», с которой эта ветка борется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// Мост к серверу инструментов: 1С или vrunner.
    Mcp(McpServer),
    /// Наш веб-клиент: труба на локальный порт публикации, без моста.
    Web,
}

impl Served {
    /// Перевести ПОРТ в слово. Единственное место, где номер порта что-то решает, и живёт оно рядом с тем, кто порты слушает.
    /// Неизвестный порт — `None`: ничего не запускается, соединение закрывается само.
    pub fn of_port(port: u16, ports: Ports) -> Option<Served> {
        if port == ports.vrunner {
            Some(Served::Mcp(McpServer::Vrunner))
        } else if port == ports.mcp {
            Some(Served::Mcp(McpServer::OneC))
        } else if port == ports.web {
            Some(Served::Web)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops_host::{port_of, McpServer};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Потолок ТОЙ машины доезжает до ребёнка `mcp-1c` — ключом `--access`, и ужесточает только в одну сторону.
    ///
    /// До 11.11 он до ребёнка не доезжал вовсе: складывался, показывался человеку и писался в книгу, а пишущие инструменты
    /// регистрировались как ни в чём не бывало (дыра контракта «запрет любой стороны сильнее разрешения другой»).
    #[test]
    fn the_other_machines_ceiling_reaches_the_child_and_only_tightens() {
        let mine = crate::cfg::filled_for_test().with_access("read_write");
        assert_eq!(tightened(&mine, crate::wire::Ceiling::Full).access(), "read_write", "та сторона не запрещает — своё слово цело");
        assert_eq!(tightened(&mine, crate::wire::Ceiling::ReadOnly).access(), "read_only", "запретила та сторона — запрет и у ребёнка");

        let locked = crate::cfg::filled_for_test().with_access("read_only");
        assert_eq!(tightened(&locked, crate::wire::Ceiling::Full).access(), "read_only", "чужое «можно» своего запрета не отменяет");
    }

    /// Потолок едет ВМЕСТЕ с потоком, и местный агент не получает его по умолчанию, а объявляет себя местным.
    ///
    /// Прежний тест выше сверяет `tightened` — чистую функцию, которую боевой путь звать не обязан. Дыра 11.11 была ровно в этом
    /// промежутке: функция считала верно, а до ребёнка её ответ не доезжал. Здесь сверяется то самое звено, через которое `serve`
    /// и получает потолок: разрешающего значения, которое можно подставить случайно, у него больше нет — есть названный случай.
    #[test]
    fn the_ceiling_travels_with_the_stream_and_the_local_agent_says_so_by_name() {
        let mine = crate::cfg::filled_for_test().with_access("read_write");
        let through = |incoming: Incoming<&str>| {
            let (_, theirs) = incoming.split();
            tightened(&mine, theirs).access().to_string()
        };
        assert_eq!(through(Incoming::Local("поток агента")), "read_write", "второй стороны нет — запрещать некому");
        assert_eq!(through(Incoming::FromPeer("поток канала", crate::wire::Ceiling::Full)), "read_write", "та сторона не запрещает");
        assert_eq!(
            through(Incoming::FromPeer("поток канала", crate::wire::Ceiling::ReadOnly)),
            "read_only",
            "та сторона запретила — запрет доезжает до ребёнка, а не теряется по дороге"
        );
    }

    /// Карточка, по которой обслуживаются вызовы, читается у ЖИВОГО `Serve`, а не хранится снимком со времени подъёма сеанса.
    ///
    /// Вопрос «та ли это база» задаёт `Open::is_for` (`peer::serve_calls`), а базу мост берёт из живой ячейки параметров на
    /// каждую просьбу. Снимок рядом с ней был бы второй мерой того же: человек переключил текущее соединение, сеанс в эту секунду
    /// ещё не перецикли́лся — и вызов, допущенный по прежней карточке, уехал бы в базу новой (вторая проверка безопасности 11.11).
    #[test]
    fn the_card_calls_are_served_for_is_read_live() {
        let cfg = crate::cfg::cfg_with(&["Buh", "Zup"]);
        let dir = std::env::temp_dir().join(format!("onec-serving-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let live = Live::new(cfg.clone());
        let ports = Ports { mcp: 0, vrunner: 0, web: 0 };
        let log: Sink = Arc::new(|_: Event| {});
        let serve = Serve::shared(live.clone(), &cfg, &Kit::at(&dir), log, ports, Arc::new(AtomicBool::new(true)));
        let (_stop_tx, stop_rx) = watch::channel(false);
        let serving = Serving::default();
        serving.put(Arc::new(serve), stop_rx);
        let open = serving.now().expect("сеанс положил, чем обслуживать");

        let first = cfg.current().id();
        assert!(open.is_for(&first), "подняли сеанс на «Buh» — его вызовы и обслуживаются");

        let mut after = cfg.clone();
        after.switch_current("Zup").unwrap();
        let second = after.current().id();
        live.set(after);
        assert!(!open.is_for(&first), "вызов по прежней карточке получил бы базу новой");
        assert!(open.is_for(&second), "а вызов по новой текущей — обслуживается");
    }

    /// Набор, который `Serve::new` принимает без лишних вопросов: сервер
    /// файловой базы на выдуманном рабочем месте `dev1` — те же значения, что
    /// и у остальных тестов этого файла (`filled_for_test`).
    /// Свободная тройка портов для сеанса в тесте.
    fn test_ports() -> Ports {
        Ports { mcp: crate::ports::test_port(), vrunner: crate::ports::test_port(), web: crate::ports::test_port() }
    }

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
        let task = serve.serve(Served::Mcp(McpServer::OneC), Incoming::Local(theirs));
        let reply = ask(mine, br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).await;
        assert!(reply.contains("\"result\""), "{reply}");
        task.abort();
    }

    #[test]
    fn the_server_argument_picks_the_port() {
        assert_eq!(port_of(McpServer::OneC, ports()), ports().mcp);
        assert_eq!(port_of(McpServer::Vrunner, ports()), ports().vrunner);
    }

    /// Порт переводится в слово ОДНИМ местом, и перевод обратим: что слушатель принял на порту сервера, то и обслужит его мост.
    /// Чужой порт — `None`: ничего не запускается, соединение закрывается само.
    #[test]
    fn a_port_turns_into_the_word_for_what_stands_behind_it() {
        let p = ports();
        assert_eq!(Served::of_port(p.mcp, p), Some(Served::Mcp(McpServer::OneC)));
        assert_eq!(Served::of_port(p.vrunner, p), Some(Served::Mcp(McpServer::Vrunner)));
        assert_eq!(Served::of_port(p.web, p), Some(Served::Web));
        assert_eq!(Served::of_port(1, p), None, "чужой порт мостом не обслуживается");
        for server in [McpServer::OneC, McpServer::Vrunner] {
            assert_eq!(Served::of_port(port_of(server, p), p), Some(Served::Mcp(server)), "перевод обратим: {server:?}");
        }
    }

    /// Два смысла одного события разведены: состояние читает `Ok` как «канал
    /// поднят», журнал показывает тот же момент нейтрально. Проверяем оба —
    /// 30.10 попытка добиться второго сменой события в транспорте отняла
    /// первое, и карточка навсегда застряла на «Включаю…».
    #[test]
    fn the_rise_of_the_channel_is_a_state_signal_and_a_quiet_journal_line() {
        let up = State::Connecting.next(&Event::Ok("Доступ к 1С включён: порты 7771 и 7781".into()));
        assert!(up.is_up(), "по `Ok` связь обязана стать поднятой");
        match for_log(&Event::Ok("Доступ к 1С включён: порты 7771 и 7781".into()), up.is_up()) {
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
        assert_eq!(State::Off.label("local"), "Доступ к 1С выключен");
        assert_eq!(State::Connecting.label("local"), "включаю…");
        assert_eq!(State::Up { since: Instant::now() }.label("local"), "доступ к 1С включён 0 мин");
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
        changed.current_mut().base_path = "C:\\Bases\\Other".into();
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

    /// Сдвиг моста — такой же факт сеанса, и такт наблюдателя обязан щадить и
    /// его: слушатель уже держит 7772, а `Serve` после стирания называл бы
    /// 7771 — то есть агента отправляли бы не туда, где слушают. Тот же
    /// дефект, что у порта публикации, второй дорогой.
    #[test]
    fn a_watcher_tick_does_not_erase_the_bridge_shift_of_this_account() {
        let mut settled = crate::cfg::filled_for_test();
        settled.resolved_bridge_shift = Some(1);
        let live = Live::new(settled);
        assert_eq!(live.get().1.ports().mcp, 7772);

        let reloaded = crate::cfg::filled_for_test();
        assert_eq!(reloaded.resolved_bridge_shift, None, "голый `Cfg::load` сдвига не знает");
        live.set(reloaded);

        assert_eq!(live.get().1.ports().mcp, 7772, "сдвиг обязан пережить такт наблюдателя");
    }

    /// Главная опасность 6B: вызов, получивший строку подключения одной базы
    /// и каталог проекта другой. `Live::get` обязана отдавать поколение и
    /// `Cfg` одним захватом мьютекса — никогда вперемешку, сколько бы
    /// `set` ни звал параллельный поток.
    #[test]
    fn live_cfg_never_hands_out_a_mix_of_two_configurations() {
        let mut a = crate::cfg::filled_for_test();
        a.current_mut().base_path = "C:\\Bases\\A".into();
        a.current_mut().db_user = "UserA".into();
        let mut b = crate::cfg::filled_for_test();
        b.current_mut().base_path = "C:\\Bases\\B".into();
        b.current_mut().db_user = "UserB".into();
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
            let whole_a = cfg.current().base_path == a.current().base_path && cfg.current().db_user == a.current().db_user;
            let whole_b = cfg.current().base_path == b.current().base_path && cfg.current().db_user == b.current().db_user;
            assert!(whole_a || whole_b, "смешались половины двух конфигураций: {:?}", cfg.current());
        }
        writer.join().unwrap();
    }

    #[test]
    fn a_session_that_cannot_bind_its_port_ends_by_itself_and_stop_is_still_fine() {
        // Сеансу нечего поднимать: порт слушателя занят, и подняться он не может. Прежде тем же самым был ssh в несуществующую
        // среду, а до задачи 4 плана «Сопряжение переезжает в бек» — база на другом компьютере при отсутствующем канале
        // (`listen::NO_WINDOW`). Того отказа больше нет: ручка канала есть всегда, и сеанс над чужой базой поднимается.
        let cfg = crate::cfg::filled_for_test();
        let dir = std::env::temp_dir().join(format!("onec-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ports = Ports { mcp: busy.local_addr().unwrap().port(), ..test_ports() };
        let wiring = Wiring { ports: Some(ports), ..Default::default() };
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let mut s =
            start_with_enabled(cfg, Kit::at(&dir), std::sync::Arc::new(|_| {}), Arc::new(AtomicBool::new(true)), wiring, move |st| {
                seen2.lock().unwrap().push(st)
            });
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

        // Живой: база здесь, и местный слушатель стоит на портах, пока его не остановят. Порты — свои, тестовые: общие
        // константы занял бы настоящий коннектор на машине человека.
        let wiring = Wiring { ports: Some(test_ports()), ..Default::default() };
        let mut live = start_with_enabled(
            crate::cfg::filled_for_test(),
            Kit::at(&dir),
            std::sync::Arc::new(|_| {}),
            Arc::new(AtomicBool::new(true)),
            wiring,
            |_| {},
        );
        assert!(!live.finished(), "сеанс только что заведён и никуда не ушёл");
        drop(live);

        // Кончившийся сам: порт слушателя занят, и поднять его нечем. Прежде этим же был «базы здесь нет, а канал держать некому»,
        // и с задачи 4 плана «Сопряжение переезжает в бек» такого отказа нет: ручка канала есть всегда, и сеанс над чужой базой
        // поднимается.
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let wiring = Wiring { ports: Some(Ports { mcp: busy.local_addr().unwrap().port(), ..test_ports() }), ..Default::default() };
        let mut dead = start_with_enabled(
            crate::cfg::filled_for_test(),
            Kit::at(&dir),
            std::sync::Arc::new(|_| {}),
            Arc::new(AtomicBool::new(true)),
            wiring,
            |_| {},
        );
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

    /// Шаг 2а брифа задачи 4 плана «тонкий срез ценности»: обычный путь
    /// владельца — положить свежий `1c-connect.exe` в уже собранный
    /// комплект, не меняя `mcp-1c` рядом. Сторож сборки (`pack.sh`) тогда
    /// не зовётся вовсе, и в комплекте остаётся upstream-бинарь, который
    /// не знает `--profile`/`--toolset`/`--access` — Go-пакет `flag`
    /// печатает «flag provided but not defined: -profile» и выходит.
    /// Человек, увидев только эту строку, не узнает, что чинить: ключ
    /// добавили мы, а умер чужой бинарь.
    #[test]
    fn an_old_mcp_1c_that_does_not_know_the_new_flags_gets_a_hint_to_replace_it() {
        let e = child_exit_line("mcp-1c", 2, false, "flag provided but not defined: -profile").unwrap();
        assert!(matches!(e, Event::Warn(ref t) if t.contains("старый mcp-1c") && t.contains("сборкой форка")), "{e:?}");

        // Подсказка не для любой чужой ошибки — только для этой конкретной.
        let e2 = child_exit_line("mcp-1c", 3, false, "платформа не найдена").unwrap();
        assert!(!matches!(e2, Event::Warn(ref t) if t.contains("старый mcp-1c")), "подсказка появилась на чужой ошибке: {e2:?}");

        // И не для vrunner-mcp — там та же фраза ничего не значила бы:
        // подсказка привязана к бинарю, который мы меняем на форк, а не к
        // словам в stderr.
        let e3 = child_exit_line("vrunner-mcp", 2, false, "flag provided but not defined: -profile").unwrap();
        assert!(!matches!(e3, Event::Warn(ref t) if t.contains("старый mcp-1c")), "подсказка появилась у другого ребёнка: {e3:?}");
    }
}
