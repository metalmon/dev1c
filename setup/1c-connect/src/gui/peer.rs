//! Узел этой машины в окне: принимает входящие заявки и звонит зовущими
//! карточками.
//!
//! Проводка, и только она. Протокол (`enroll`), удержание потока
//! (`channel`), книга допуска (`enrolled`) и слова состояний (`gui::link`)
//! написаны и проверены до этого файла; здесь они встречаются с окном —
//! узел поднимается, входящая заявка доезжает до диалога человека, а
//! зовущая карточка звонит и возвращается после обрыва.
//!
//! **Почему своим потоком с своим рантаймом.** `Node::start` открывает сокет
//! и выходит в сеть, `accept`/`dial` висят часами, а главный поток окна —
//! это GPUI: всё, что на нём ждёт, — это замершее окно. Приём тот же, которым
//! живёт сеанс (`session::start_with_enabled`), и причина та же.
//!
//! **Один узел на процесс, а не по узлу на карточку.** Ключ узла — личность
//! МАШИНЫ (`node.rs`, `node.key` в каталоге состояния), и второй узел здесь
//! означал бы вторую личность у одного рабочего места. Принимающие карточки
//! и зовущие делят его: первым он нужен, чтобы принимать, вторым — чтобы
//! звонить.
//!
//! **Решения человека остаются у человека.** Этот поток не решает, пускать
//! ли кого-то: он посылает заявку в окно (`Ev::Enroll`) и ждёт ответа по
//! одноразовому каналу. Диалог, отпечаток и кнопки — дело окна (`save::
//! ask_permission`), и второго мнения о допуске здесь нет.

use super::*;
use crate::channel::{self, Channels, Gone, Reins, Round, Stopped};
use crate::enroll;
use crate::enrolled::{Book, Partner, Partners};
use crate::node::Node;
use crate::wire::{BaseFields, Ceiling};
use iroh::{EndpointAddr, EndpointId};
use std::collections::HashMap;

/// Сколько ждём выхода в сеть, прежде чем выдавать код.
///
/// Ждём, потому что до выхода в сеть узел не знает ни своего релея, ни
/// адресов, которыми его видно, — а код, выданный раньше, донёс бы до той
/// стороны один ключ, то есть дозвон только поиском через серверы n0. Ждём
/// НЕ бесконечно: машина без интернета в сеть не выйдет никогда, а код по
/// петле ей всё равно нужен (мерка готовности — две копии на одной машине).
const ONLINE_WAIT: Duration = Duration::from_secs(10);

/// Как эту машину найти снаружи — ровно то, что уезжает в код подключения.
///
/// Считается один раз, когда узел вышел в сеть, и живёт в `Peer` под замком:
/// код выдаётся на главном потоке окна (`panel::issue_code`), а знает это
/// только поток узла.
#[derive(Debug, Clone, Default)]
pub struct Reach {
    pub relay: Option<String>,
    pub direct: Vec<String>,
}

/// Ручка на поток узла — то, что живёт в `Ui` и чем окно его просит.
///
/// Остановки у потока нет нарочно, тем же доводом, которым её нет у
/// `channel::keep_calling`: принимать входящие надо всё время, пока программа
/// работает, а «выключить обслуживание» — это про вызовы агента
/// (`Ui::enabled`), не про то, доходит ли до человека заявка на зачисление.
/// Поток кончается с процессом.
#[derive(Clone)]
pub struct Peer {
    cmd: tokio::sync::mpsc::UnboundedSender<Cmd>,
    reach: Arc<Mutex<Option<Reach>>>,
}

/// Что окно умеет попросить у потока узла.
enum Cmd {
    /// Позвонить зовущей карточкой. Пустой `code` — переподключение по
    /// запомненному партнёру (`Partners`), непустой — первая заявка.
    Dial { card: String, code: String },
    /// Снять поток зачисленной стороны ПРЯМО СЕЙЧАС, сказав ей словами.
    ///
    /// Иначе отзыв действовал бы только на следующем круге той стороны, а
    /// спека требует немедленности: `Book::enrolled_to` читает диск на каждое
    /// подключение именно затем, чтобы «Отозвать» срабатывало сразу, — и
    /// живой поток, который переживает отзыв, эту немедленность отменяет.
    Revoke { card: String, peer: EndpointId },
}

impl Peer {
    /// Как нас найти — для кода подключения. `None` — узел ещё не поднялся
    /// или не вышел в сеть; код тогда выдаётся без адресов, как и до этой
    /// задачи.
    pub fn reach(&self) -> Option<Reach> {
        self.reach.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Позвонить карточкой. Отказ — только «поток узла кончился», то есть
    /// случай, в котором звонить нечем вовсе.
    pub fn dial(&self, card: &str, code: &str) -> Result<(), String> {
        self.cmd
            .send(Cmd::Dial { card: card.to_string(), code: code.to_string() })
            .map_err(|_| "Узел не работает — подключиться нечем. Перезапустите программу.".to_string())
    }

    /// Снять живой поток отозванной стороны. Молча: запись об отзыве уже
    /// сделана и уже сказана словами (`panel::revoke_party`), а живого потока
    /// у этой стороны могло и не быть вовсе — тогда снимать нечего, и это не
    /// беда.
    pub fn revoked(&self, card: &str, peer: EndpointId) {
        let _ = self.cmd.send(Cmd::Revoke { card: card.to_string(), peer });
    }
}

/// Поднимает поток узла и отдаёт ручку на него.
///
/// Возвращается сразу: сам узел поднимается в потоке, и о том, поднялся ли он,
/// говорит строка журнала. Иначе открытие окна ждало бы сеть.
pub fn start(kit: &Kit, tx: UnboundedSender<Ev>) -> Peer {
    let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
    let reach = Arc::new(Mutex::new(None));
    let (kit_in, reach_in, tx_in) = (kit.clone(), reach.clone(), tx.clone());
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                say(&tx_in, Kind::Fail, format!("Узел не поднялся: {e}"));
                return;
            }
        };
        rt.block_on(serve(kit_in, tx_in, reach_in, rx));
    });
    Peer { cmd, reach }
}

/// Вся жизнь потока узла: поднять, объявить себя, принимать входящие,
/// звонить зовущими карточками.
async fn serve(kit: Kit, tx: UnboundedSender<Ev>, reach: Arc<Mutex<Option<Reach>>>, mut cmds: tokio::sync::mpsc::UnboundedReceiver<Cmd>) {
    let node = match Node::start(kit.state()).await {
        Ok(node) => node,
        Err(e) => {
            say(&tx, Kind::Fail, e);
            return;
        }
    };
    // Срок, а не бесконечное ожидание: без интернета `online` не вернётся
    // никогда, а принимать по петле и по локальной сети узел умеет уже сейчас.
    let _ = tokio::time::timeout(ONLINE_WAIT, node.endpoint().online()).await;
    let seen = look(&node);
    *reach.lock().unwrap_or_else(|e| e.into_inner()) = Some(seen);
    let channels = Channels::default();
    let live = Live::default();
    tokio::spawn(accepting(node.clone(), channels.clone(), kit.clone(), tx.clone(), live.clone()));
    let mut calls: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    // Карточки, у которых партнёр уже записан, звонят САМИ, без щелчка:
    // человек разрешил подключение однажды, и перезапуск программы не повод
    // просить его нажать «Подключиться» ещё раз (спека контракта,
    // «Что хранится после»).
    match Partners::open(kit.state()) {
        Ok(partners) => {
            for (card, _) in partners.all() {
                start_call(&node, &channels, &kit, &tx, &mut calls, card, String::new());
            }
        }
        Err(e) => say(&tx, Kind::Fail, e),
    }
    while let Some(cmd) = cmds.recv().await {
        match cmd {
            Cmd::Dial { card, code } => start_call(&node, &channels, &kit, &tx, &mut calls, card, code),
            Cmd::Revoke { card, peer } => {
                let Some(reins) = live.lock().unwrap_or_else(|e| e.into_inner()).remove(&(card.clone(), peer)) else {
                    continue;
                };
                let base_id = cards_of(&kit).into_iter().find(|(name, _)| *name == card).map(|(_, c)| c.base.base_id);
                tokio::spawn(async move {
                    reins.revoke(&base_id.unwrap_or_default(), REVOKED_TEXT).await;
                });
            }
        }
    }
}

/// Слова отзыва той стороне. Причина и совет, как велит раздел «Ошибки»
/// спеки: без совета причина человеку не нужна.
const REVOKED_TEXT: &str =
    "Разрешение отозвано на стороне базы.\nЧеловек там снял доступ этому рабочему месту.\nПопросите выдать новый код, \
                            если доступ нужен снова.";

/// Живые разговоры принимающей стороны: карточка и ключ той стороны → вожжи
/// её управляющего потока (`channel::Reins`).
///
/// Ключ парой, а не одной карточкой: к одной карточке зачисляют сколько
/// угодно сторон (решение владельца 1.11), и отзыв адресуется ОДНОЙ из них —
/// ровно той, чей отпечаток человек видел в строке карточки.
///
/// Обычный `std::sync::Mutex`, не `tokio`: держат его на одно взятие-удаление
/// без единого `await` внутри, а сама отправка прощания идёт уже снаружи
/// замка.
type Live = Arc<Mutex<HashMap<(String, EndpointId), Reins>>>;

/// Заводит звонок карточки, заменяя прежний, если он ещё идёт.
///
/// Замена, а не второй звонок рядом: карточка одна, и два звонка по ней
/// означали бы два управляющих потока, из которых слово состояния показывало
/// бы случайный. Прежняя задача снимается (`abort`), а вместе с ней роняются
/// `Control` и `Hold` — канал закрывается тем же `Drop`, которым он
/// закрывается всегда (`channel::Hold`).
fn start_call(
    node: &Node,
    channels: &Channels,
    kit: &Kit,
    tx: &UnboundedSender<Ev>,
    calls: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    card: String,
    code: String,
) {
    let addr = match target(kit, &card, &code) {
        Ok(addr) => addr,
        Err(e) => {
            say(tx, Kind::Fail, e);
            return;
        }
    };
    if let Some(old) = calls.remove(&card) {
        old.abort();
    }
    let handle = tokio::spawn(calling(node.clone(), channels.clone(), kit.clone(), tx.clone(), card.clone(), code, addr));
    calls.insert(card, handle);
}

/// Куда звонить этой карточкой: по вставленному коду либо по запомненному
/// партнёру.
///
/// Пустой код без партнёра — не беда программы, а разговор с человеком:
/// подключаться действительно некуда, и сказать это надо словами, а не
/// молчаливым бездействием кнопки.
fn target(kit: &Kit, card: &str, code: &str) -> Result<EndpointAddr, String> {
    if !code.trim().is_empty() {
        return Ok(crate::ticket::Ticket::from_code(code)?.addr());
    }
    let partner = Partners::open(kit.state())?
        .of(card)
        .ok_or("Кода нет, а подключений по этой карточке ещё не было.\nВставьте код, выданный на машине с базой.")?;
    Ok(addr_of(&partner))
}

/// Адрес партнёра: ключ плюс подсказки того же вида, что ездят в коде
/// (`Ticket::addr`) и по тому же правилу — не разобралось, значит выброшено,
/// ключ остаётся.
fn addr_of(p: &Partner) -> EndpointAddr {
    let mut addr = EndpointAddr::new(p.key);
    for d in &p.direct {
        if let Ok(s) = d.parse::<std::net::SocketAddr>() {
            addr = addr.with_ip_addr(s);
        }
    }
    if let Some(relay) = &p.relay {
        if let Ok(url) = relay.parse::<iroh::RelayUrl>() {
            addr = addr.with_relay_url(url);
        }
    }
    addr
}

/// Зовущая сторона одной карточки: круг за кругом, пока звать есть смысл
/// (`channel::keep_calling`).
///
/// **Код предъявляется ОДИН раз.** Он одноразовый (`Book::spend`), и второй
/// круг с тем же кодом получил бы отказ «код не найден», то есть
/// `Stopped::NotEnrolled` — и переподключение, ради которого круги и
/// существуют, кончилось бы на первом же обрыве. Поэтому после удачного
/// зачисления код гасится здесь же, и следующие круги идут пустыми: зачисление
/// уже есть, предъявлять нечего (`enroll::dial_side` на пустом коде).
///
/// Опустевший код — он же признак «мы уже были подключены»: первый
/// недозвон до зачисления остаётся «подключается», а после — «подключается
/// снова». Второго счётчика для этого не нужно.
async fn calling(node: Node, channels: Channels, kit: Kit, tx: UnboundedSender<Ev>, card: String, code: String, addr: EndpointAddr) {
    flow(&tx, &card, Some(link::Flow::Calling));
    let code = Arc::new(Mutex::new(code));
    let stopped = channel::keep_calling(|| {
        let (node, channels, kit, tx, card, addr, code) =
            (node.clone(), channels.clone(), kit.clone(), tx.clone(), card.clone(), addr.clone(), code.clone());
        async move {
            // Замок берётся и отпускается до первого ожидания — через `await`
            // его не держим ни разу.
            let mine = code.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let round = one_round(&node, &channels, &kit, &tx, &card, addr, &mine).await;
            let enrolled = match &round {
                Round::Held(_) => {
                    code.lock().unwrap_or_else(|e| e.into_inner()).clear();
                    true
                }
                _ => code.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            };
            if enrolled && !matches!(round, Round::NotEnrolled(_)) {
                flow(&tx, &card, Some(link::Flow::Retrying));
            }
            round
        }
    })
    .await;
    flow(&tx, &card, Some(link::Flow::Broken));
    match stopped {
        Stopped::Farewell(why) => say(&tx, Kind::Say, format!("«{card}»: {}", why.text)),
        Stopped::NotEnrolled(no) => say(&tx, Kind::Warn, format!("«{card}»: {}", no.text)),
    }
}

/// Один круг звонка: дозвониться, открыть управляющий поток, провести
/// зачисление, держать поток, пока та сторона говорит.
async fn one_round(
    node: &Node,
    channels: &Channels,
    kit: &Kit,
    tx: &UnboundedSender<Ev>,
    card: &str,
    addr: EndpointAddr,
    code: &str,
) -> Round {
    let channel = match channels.to(node, addr.clone()).await {
        Ok(c) => c,
        Err(e) => return Round::Failed(e),
    };
    let mut control = match channel.call().await {
        Ok(c) => c,
        Err(e) => return Round::Failed(e),
    };
    let out = {
        let (send, recv) = control.halves();
        enroll::dial_side(send, recv, code, &says_name(), ceiling_of(kit)).await
    };
    match out {
        Err(e) => {
            control.end(None).await;
            Round::Failed(e)
        }
        Ok(enroll::Outcome::NotEnrolled(no)) => {
            control.end(None).await;
            Round::NotEnrolled(no)
        }
        Ok(_) => {
            // Куда возвращаться после обрыва — запоминается ТОЛЬКО здесь, и
            // только после того, как та сторона признала нас своими. Ключ
            // берётся у канала (`Channel::peer`), а не у кода: это то, что
            // доказал сам iroh, а не то, что было написано в строке.
            remember(kit, tx, card, channel.peer(), &addr);
            flow(tx, card, Some(link::Flow::Up));
            say(tx, Kind::Ok, format!("«{card}»: подключено"));
            // Сообщения, которые та сторона говорит по живому потоку
            // (`state`), здесь пока не разбираются: что значит «обслуживание
            // выключили» и когда показывать это человеку, знает окно, и
            // решать это в проводке значило бы завести второе мнение
            // (сомнение задачи 8, оставлено нарочно).
            let gone = channel::hold(control.ear(), |_| {}).await;
            // Единственный выход из соединения — через `end`, и на этом пути
            // тоже: сказать нам нечего (`None`), но написанное до этого
            // обязано доехать прежде, чем канал закроется.
            control.end(None).await;
            Round::Held(gone)
        }
    }
}

/// Приём входящих: канал → управляющий поток → разговор зачисления.
///
/// **Почему не `channel::accept_all`.** Та отдаёт наружу один `Control`, а
/// `enroll::accept_side` требует `peer` — ключ узла, который доказал сам
/// iroh, а не то, что сказали в `hello`; из `Control` его взять нечем.
/// Пока `accept_all` не передаёт партнёра в разговор, цикл живёт здесь: три
/// задачи и те же правила, что у неё (канал живёт дольше любого своего
/// потока, поток — дольше разговора по нему), и трогать чужой файл ради
/// одного аргумента мы не станем.
async fn accepting(node: Node, channels: Channels, kit: Kit, tx: UnboundedSender<Ev>, live: Live) {
    while let Some(incoming) = node.accept().await {
        let conn = match incoming {
            Ok(conn) => conn,
            Err(why) => {
                // Сказать об этом той стороне нечем — её конца у нас уже нет.
                say(&tx, Kind::Warn, why);
                continue;
            }
        };
        let channel = channels.keep(conn);
        let (kit, tx, live) = (kit.clone(), tx.clone(), live.clone());
        tokio::spawn(async move {
            let peer = channel.peer();
            // Поток за потоком, пока канал жив: `answer` отказывает ровно
            // тогда, когда он закрылся, и этот отказ и есть конец присмотра.
            while let Ok(control) = channel.answer().await {
                tokio::spawn(answering(kit.clone(), tx.clone(), live.clone(), control, peer));
            }
        });
    }
}

/// Один разговор зачисления на принимающей стороне: заявка в окно, решение
/// человека обратно, и дальше держим поток.
///
/// **Книга открывается своя, на этот разговор.** Так и задумано
/// (`enrolled::Book` — «окно и отдельный процесс держат разные `Book` на один
/// файл»): каждая правка перечитывает файл под межпроцессным замком, поэтому
/// снимок в памяти окна и снимок здесь не спорят. Решение о допуске читает
/// диск (`Book::enrolled_to`), и отзыв в окне действует немедленно.
///
/// **Вожжи зачисленного разговора кладутся в `live`** (`channel::reins`):
/// по ним «Отозвать разрешение» снимает поток в ту же секунду, а не на
/// следующем круге той стороны. Снимаются они оттуда ровно тогда, когда
/// присмотр кончился, — и уронённые вожжи досылают написанное и закрывают
/// поток сами (`channel::settle`).
///
/// **Чего здесь ещё нет.** Прощание при выключении обслуживания
/// (`channel::SERVING_OFF`) и при закрытии окна (`WINDOW_CLOSED`): слова и
/// вожжи для них готовы, не хватает только зова из переключателя
/// (`link::toggle_link`) и с «Выхода». Пока его нет, та сторона читает обрыв
/// вместо причины.
async fn answering(kit: Kit, tx: UnboundedSender<Ev>, live: Live, mut control: crate::channel::Control, peer: EndpointId) {
    let mut book = match Book::open(kit.state()) {
        Ok(book) => book,
        Err(e) => {
            say(&tx, Kind::Fail, e);
            return;
        }
    };
    let cards = cards_of(&kit);
    let only: Vec<enroll::Card> = cards.iter().map(|(_, c)| c.clone()).collect();
    let ask = |request: enroll::Request| {
        let tx = tx.clone();
        async move {
            let (answer, wait) = futures::channel::oneshot::channel();
            if tx.unbounded_send(Ev::Enroll(Pending { request, answer: Some(answer) })).is_err() {
                // Окна больше нет — спрашивать некого, и это не отказ
                // человека, а отсутствие человека.
                return enroll::Decision::NoAnswer;
            }
            // Уронённый передатчик (окно закрыли, заявку вытеснила новая от
            // той же стороны) — тот же третий ответ: решения не было.
            wait.await.unwrap_or(enroll::Decision::NoAnswer)
        }
    };
    let out = {
        let (send, recv) = control.halves();
        enroll::accept_side(send, recv, peer, &mut book, &only, ask).await
    };
    let out = match out {
        Ok(out) => out,
        Err(e) => {
            say(&tx, Kind::Warn, e);
            control.end(None).await;
            return;
        }
    };
    let base_id = match &out {
        enroll::Outcome::Enrolled(base) => base.base_id.clone(),
        enroll::Outcome::Already(bases) => bases.first().map(|b| b.id.clone()).unwrap_or_default(),
        enroll::Outcome::NotEnrolled(no) => {
            say(&tx, Kind::Say, no.text.clone());
            control.end(None).await;
            return;
        }
    };
    // Имя карточки по базе, о которой договорились: оно местное и по каналу
    // не ездит (спека), поэтому обратно его сопоставляем здесь, по тому же
    // `base_id`, которым карточка и представилась.
    let card = cards.iter().find(|(_, c)| c.base.base_id == base_id).map(|(name, _)| name.clone()).unwrap_or_default();
    flow(&tx, &card, Some(link::Flow::Up));
    say(&tx, Kind::Ok, format!("«{card}»: подключено"));
    let (reins, mut recv) = channel::reins(control);
    let key = (card.clone(), peer);
    live.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), reins);
    let gone = channel::hold(&mut recv, |_| {}).await;
    // Вожжи уходят из книги живых и падают здесь: их задача письма дошлёт
    // написанное и снимет поток сама. Если отзыв успел забрать их раньше —
    // забирать нечего, и это не беда.
    drop(live.lock().unwrap_or_else(|e| e.into_inner()).remove(&key));
    flow(&tx, &card, Some(link::Flow::Broken));
    match gone {
        Gone::Farewell(why) => say(&tx, Kind::Say, format!("«{card}»: {}", why.text)),
        Gone::Broken(why) => say(&tx, Kind::Say, format!("«{card}»: {}", why.text)),
    }
}

/// Карточки этого рабочего места, которые ПРИНИМАЮТ подключения, вместе с
/// местными именами.
///
/// Читается с диска на каждый входящий разговор, а не берётся снимком при
/// запуске: человек правит карточки в окне, и разговор обязан видеть то, что
/// в файле сейчас. Цена — одно чтение toml на входящее подключение, то есть
/// раз в минуты.
///
/// Отказ чтения настроек — пустой список: тогда ни одна карточка не
/// опознается, и `accept_side` откажет словами. Это верно: настройки, которые
/// не читаются, не вправе никого пускать.
fn cards_of(kit: &Kit) -> Vec<(String, enroll::Card)> {
    let cfg = match crate::cfg::Cfg::load(kit) {
        Ok(cfg) => cfg,
        Err(_) => return Vec::new(),
    };
    cfg.connections
        .iter()
        .filter(|c| c.mode != "local" && matches!(c.role(), Ok("accept")))
        .map(|c| {
            (
                c.name.clone(),
                enroll::Card {
                    name: c.name.clone(),
                    base: BaseFields {
                        base_id: base_id_of(&c.base_path),
                        base_name: crate::cfg::base_name(&c.base_path).to_string(),
                        publication: c.publication.clone(),
                        write: c.allows_write(),
                    },
                },
            )
        })
        .collect()
}

/// Опознавательный знак базы для провода.
///
/// Ни имя карточки, ни путь к базе сюда не годятся: первое — местное и по
/// каналу не ездит (спека, «По каналу никогда не едут»), второе — путь на
/// диске человека, то есть тоже его дело, а не той стороны. Отпечаток пути
/// говорит ровно то, что нужно: одна и та же база после перезапуска — это
/// она же, а разные базы — разные. Половины sha256 для этого довольно с
/// огромным запасом, а длинная строка в каждом кадре не нужна никому.
fn base_id_of(base_path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(base_path.as_bytes());
    crate::secrets::hex::encode(&h.finalize()[..16])
}

/// Потолок записи этой МАШИНЫ (`wire::Ceiling` — «свойство машины, не
/// карточки»): тот же переключатель, что живёт в меню значка.
fn ceiling_of(kit: &Kit) -> Ceiling {
    match crate::cfg::Cfg::load(kit) {
        Ok(cfg) if cfg.access() == "full" => Ceiling::Full,
        // Не прочлось — «только чтение»: по умолчанию инструментов, пишущих
        // в базу, нет (решение владельца 1.11), и неизвестность не повод
        // выдавать больше, чем разрешено.
        _ => Ceiling::ReadOnly,
    }
}

/// Как эта машина называет себя той стороне. Её слова, не проверенный факт, —
/// и показываются они с этой пометкой (`save::permission_question`).
///
/// Имя компьютера, а не имя карточки: карточка названа человеком по ТОЙ
/// стороне («Ромашка» у разработчика — это заказчик), и прислать его обратно
/// значило бы назваться его же словами о нас. Имя не узналось — пустая
/// строка, и в окне это честное «Себя не назвала».
fn says_name() -> String {
    sysinfo::System::host_name().unwrap_or_default()
}

/// Запоминает, куда карточке возвращаться. Отказ записи — строка в журнал, а
/// не конец подключения: это подключение уже живёт, а потеряно будет лишь
/// возвращение после перезапуска.
fn remember(kit: &Kit, tx: &UnboundedSender<Ev>, card: &str, peer: EndpointId, addr: &EndpointAddr) {
    let partner = Partner {
        key: peer,
        relay: addr.relay_urls().next().map(|u| u.to_string()),
        direct: addr.ip_addrs().map(|a| a.to_string()).collect(),
    };
    let wrote = Partners::open(kit.state()).and_then(|mut p| p.remember(card, &partner));
    if let Err(e) = wrote {
        say(tx, Kind::Warn, format!("Подключение живёт, но запомнить его не удалось: {e}"));
    }
}

/// Что узел знает о себе — плюс своя петля (`ticket::direct_with_loopback`,
/// там и довод, почему петля обязательна).
fn look(node: &Node) -> Reach {
    let addr = node.ticket_parts();
    let seen: Vec<std::net::SocketAddr> = addr.ip_addrs().copied().collect();
    let relay = addr.relay_urls().next().map(|u| u.to_string());
    Reach { relay, direct: crate::ticket::direct_with_loopback(&seen, &node.endpoint().bound_sockets()) }
}

/// Строка в журнал окна из потока узла. Прямой дороги к `note_line` отсюда
/// нет и быть не может — она просит `&mut Ui`, то есть главный поток.
fn say(tx: &UnboundedSender<Ev>, kind: Kind, text: impl Into<String>) {
    let _ = tx.unbounded_send(Ev::Line(Line { kind, text: text.into() }));
}

/// Новое состояние управляющего потока карточки — в окно. Слово человеку из
/// него делает `link::contract_of`/`contract_word`, здесь только факт.
fn flow(tx: &UnboundedSender<Ev>, card: &str, now: Option<link::Flow>) {
    let _ = tx.unbounded_send(Ev::Flow(card.to_string(), now));
}
