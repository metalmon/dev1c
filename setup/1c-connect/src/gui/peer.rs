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
//!
//! **Две копии на одной машине этот код умеет, а программа — пока нет.**
//! Канал, зачисление и код с петлёй рассчитаны ровно на этот случай
//! (`ticket::direct_with_loopback`), и двум копиям с РАЗНЫМИ каталогами
//! состояния они дают путь без интернета и без брандмауэра. Но вторую копию
//! не пускает замок единственного экземпляра: `single_instance::guard_key`
//! заведён на пользователя ОС (плюс имя рабочего места), а не на каталог
//! состояния, — и второй запуск уходит до того, как дойдёт сюда. Пускать ли
//! два экземпляра с разными каталогами, решает владелец: замок стоит там не
//! случайно (один `httpd.pid` на пользователя, одни порты рабочего места),
//! и снимать его «чтобы проверить» нельзя. Пока решения нет, этот путь
//! проверяется двумя машинами либо двумя учётными записями Windows.

use super::*;
use crate::channel::{self, Channels, Gone, Reins, Round, Stopped};
use crate::enroll;
use crate::enrolled::{Book, Partner, Partners};
use crate::node::Node;
use crate::wire::{code, BaseFields, Ceiling, Refuse};
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
    /// Попрощаться со ВСЕМИ живыми разговорами — программа уходит.
    ///
    /// Канал, закрытый молча, читается той стороной как ОБРЫВ (`channel::
    /// hold` отдаёт `Gone::Broken` на любую ошибку чтения), а обрыв она
    /// повторяет: `keep_calling` звонит снова и показывает «подключается
    /// снова» — вечно, потому что звонить уже некуда. Прощание словами
    /// превращает это в `Stopped::Farewell`: круги прекращаются, и человек
    /// читает «Та сторона закрыла программу».
    ///
    /// `done` — обычный `std::sync::mpsc`, а не `oneshot`: ждёт его ГЛАВНЫЙ
    /// поток окна на выходе, и ждать он умеет только по-настоящему, со
    /// сроком.
    ByeAll { done: std::sync::mpsc::Sender<()> },
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

    /// Попрощаться со всеми живыми разговорами и ДОЖДАТЬСЯ, пока прощания
    /// уедут. `false` — срок вышел, а кто-то ещё не договорил.
    ///
    /// Ждать обязательно: прощание — это записанный кадр, а записанный кадр
    /// выбрасывается, если процесс уйдёт раньше, чем та сторона признает его
    /// доехавшим (`channel::settle`, и вся беда, ради которой он заведён).
    /// Ждать НЕ бесконечно по тому же правилу, которым живут остальные
    /// ожидания выхода: застрявшее прощание не должно держать «Выход».
    ///
    /// Потока узла нет (его не подняли или он кончился) — прощаться не с кем
    /// и не чем, и это успех, а не отказ: иначе каждый выход без узла стоил
    /// бы человеку полного срока ожидания впустую.
    pub fn bye_all(&self, wait: Duration) -> bool {
        let (done, waited) = std::sync::mpsc::channel();
        if self.cmd.send(Cmd::ByeAll { done }).is_err() {
            return true;
        }
        waited.recv_timeout(wait).is_ok()
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
                let base_id = cards_of(&kit).into_iter().find(|(name, _)| *name == card).map(|(_, c)| c.base.base_id);
                for reins in live.cut(peer, &card) {
                    let base_id = base_id.clone().unwrap_or_default();
                    tokio::spawn(async move { reins.revoke(&base_id, REVOKED_TEXT).await });
                }
            }
            // Прощания идут ВМЕСТЕ, а не одно за другим: каждое ждёт
            // подтверждения до `channel::FAREWELL_PATIENCE`, и очередь из
            // пяти зачисленных сторон стоила бы человеку пяти сроков на
            // «Выходе» (минорная находка ревью задачи 8). Ответ уходит
            // ПОСЛЕ: тот, кто ждёт, ждёт именно договорённости.
            Cmd::ByeAll { done } => {
                let all = live.drain();
                futures::future::join_all(all.into_iter().map(|reins| reins.bye(Refuse::new(code::CLOSED, channel::WINDOW_CLOSED)))).await;
                let _ = done.send(());
            }
        }
    }
}

/// Слова отзыва той стороне. Причина и совет, как велит раздел «Ошибки»
/// спеки: без совета причина человеку не нужна.
const REVOKED_TEXT: &str =
    "Разрешение отозвано на стороне базы.\nЧеловек там снял доступ этому рабочему месту.\nПопросите выдать новый код, \
                            если доступ нужен снова.";

/// Один живой разговор принимающей стороны: вожжи его управляющего потока и
/// карточка, которую он обслуживает.
///
/// `card` — `Option`, и это не удобство, а честность: имя карточки местное и
/// по каналу не ездит, мы сопоставляем его обратно по признаку базы, и
/// сопоставление может не сойтись (человек правил карточки под живым
/// разговором, настройки не прочлись). Пустая строка на этом месте врала бы,
/// что карточка известна и называется «», — и отзыв по такому потоку не
/// находил бы его никогда.
struct Talk {
    /// Номер разговора — им он и снимается с учёта. Нужен затем, что у одной
    /// стороны к одной карточке может идти не один поток (переподключение,
    /// чей прежний поток ещё не осыпался), и снимать надо СВОЙ, а не
    /// «какой-нибудь с тем же именем».
    id: u64,
    /// ВСЕ карточки, которые обслуживает этот поток, а не одна.
    ///
    /// Одна сторона бывает зачислена к нескольким карточкам этой машины, и
    /// тогда `enroll::Outcome::Already` приносит несколько баз сразу. Пока
    /// здесь лежало имя одной (первой из списка), отзыв ВТОРОЙ карточки
    /// такой поток не находил — он оставался живым и продолжал обслуживать
    /// карточку, разрешение по которой человек только что снял (находка
    /// проверки безопасности на коммит `9d232f5`).
    ///
    /// Пустой список — карточка не опознана вовсе; такой поток режется любым
    /// отзывом этой стороны (`cut_these`).
    cards: Vec<String>,
    reins: Reins,
}

/// Живые разговоры принимающей стороны — по СТОРОНЕ, а не по карточке.
///
/// **Почему по стороне (находка проверки безопасности на коммит задачи 9).**
/// Прежде ключом была пара «имя карточки + ключ стороны», а имя бралось
/// перебором по признаку базы и при неудаче оказывалось пустым. Поток,
/// заведённый под пустым или не тем именем, при отзыве не находился и
/// переживал собственный отзыв — то есть отзыв перестаёт быть немедленным
/// ровно в том случае, когда он важнее всего. Ключ стороны подделать нельзя:
/// его доказал сам iroh.
///
/// Отзыв режет каждый поток ЭТОЙ стороны, который держит отозванную
/// карточку, — и те, у которых имени нет вовсе: поток, про который нельзя
/// доказать, что он обслуживает другую карточку, держать после отзыва не за
/// что. Соседние стороны и соседние карточки той же стороны не трогаются
/// (к одной карточке зачисляют сколько угодно сторон, решение владельца
/// 1.11, и отзыв адресуется ОДНОЙ — той, чей отпечаток человек видел).
///
/// Обычный `std::sync::Mutex`, не `tokio`: его держат на одно взятие-изъятие
/// без единого `await` внутри, а сама отправка прощания идёт уже снаружи
/// замка.
#[derive(Clone, Default)]
struct Live {
    talks: Arc<Mutex<HashMap<EndpointId, Vec<Talk>>>>,
    next: Arc<std::sync::atomic::AtomicU64>,
}

impl Live {
    /// Запомнить разговор. Возвращает его номер — им он и снимается
    /// (`forget`).
    fn remember(&self, peer: EndpointId, cards: Vec<String>, reins: Reins) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.talks.lock().unwrap_or_else(|e| e.into_inner()).entry(peer).or_default().push(Talk { id, cards, reins });
        id
    }

    /// Снять разговор с учёта — присмотр за ним кончился. Уже снятого
    /// (отзывом) здесь нет, и это не беда.
    ///
    /// Вожжи возвращаются наружу, а не падают здесь: падать им положено ВНЕ
    /// замка — их `Drop` ничего не ждёт, но держать чужой замок дольше
    /// нужного незачем.
    fn forget(&self, peer: EndpointId, id: u64) -> Option<Reins> {
        let mut held = self.talks.lock().unwrap_or_else(|e| e.into_inner());
        let rows = held.get_mut(&peer)?;
        let at = rows.iter().position(|t| t.id == id)?;
        let gone = rows.remove(at);
        if rows.is_empty() {
            held.remove(&peer);
        }
        Some(gone.reins)
    }

    /// Вожжи всех разговоров этой стороны, которые отзыв карточки обязан
    /// снять (`cut_these` решает, какие именно). Снятые тут же уходят с
    /// учёта: второй отзыв той же карточки резать уже нечего.
    fn cut(&self, peer: EndpointId, card: &str) -> Vec<Reins> {
        let mut held = self.talks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rows) = held.get_mut(&peer) else {
            return Vec::new();
        };
        let doomed = cut_these(&rows.iter().map(|t| t.cards.clone()).collect::<Vec<_>>(), card);
        // С конца: удаление по месту не должно сдвигать места, которые ещё
        // предстоит удалить.
        let mut out: Vec<Reins> = doomed.into_iter().rev().map(|at| rows.remove(at).reins).collect();
        if rows.is_empty() {
            held.remove(&peer);
        }
        out.reverse();
        out
    }

    /// Забрать ВСЕ живые разговоры всех сторон — программа уходит, и дальше
    /// присмотра за ними не будет ни у кого.
    ///
    /// Книга пустеет тем же движением: уходя, мы прощаемся ровно один раз, а
    /// вожжи, оставшиеся в книге после прощания, — это вожжи потока, которого
    /// уже нет.
    fn drain(&self) -> Vec<Reins> {
        let mut held = self.talks.lock().unwrap_or_else(|e| e.into_inner());
        held.drain().flat_map(|(_, rows)| rows.into_iter().map(|t| t.reins)).collect()
    }

    /// Есть ли ещё живой разговор под этим номером. Нужно одному читателю —
    /// проверке щели между решением человека и записью вожжей
    /// (`answering`): если отзыв уже забрал наш разговор, рвать его второй
    /// раз не надо.
    fn alive(&self, peer: EndpointId, id: u64) -> bool {
        let held = self.talks.lock().unwrap_or_else(|e| e.into_inner());
        held.get(&peer).is_some_and(|rows| rows.iter().any(|t| t.id == id))
    }
}

/// Какие разговоры одной стороны режет отзыв карточки `card` — чистое
/// решение, и всё, что в отзыве можно решать неверно, решается здесь.
///
/// Режутся два случая: разговор, который держит ЭТУ карточку — хоть одной из
/// нескольких, — и разговор, карточка которого неизвестна (список пуст).
/// Второй — не перестраховка: имя сопоставляется обратно по признаку базы, и
/// не сошлось оно только там, где настройки под живым разговором изменились
/// или не прочлись. Про такой поток нельзя доказать, что он обслуживает
/// ДРУГУЮ карточку, а отзыв обязан действовать сразу — значит держать его
/// после отзыва не за что.
///
/// Поток, обслуживающий несколько карточек, режется отзывом ЛЮБОЙ из них
/// целиком, а не наполовину: половины у потока нет, а оставить его живым
/// значило бы оставить живым доступ, который человек только что снял. Сторона
/// вернётся сама и заведёт разговор о тех карточках, что у неё остались.
///
/// Разговоры соседних карточек той же стороны не трогаются вовсе: человек
/// снял доступ к одной базе, а не ко всем.
fn cut_these(held: &[Vec<String>], card: &str) -> Vec<usize> {
    held.iter().enumerate().filter(|(_, cards)| cards.is_empty() || cards.iter().any(|name| name == card)).map(|(at, _)| at).collect()
}

/// Держать ли только что зачисленный разговор — проверка щели между решением
/// человека и записью вожжей (находка проверки безопасности).
///
/// Щель настоящая: `accept_side` уже вернул «зачислен», а в книге живых
/// разговоров вожжей ещё нет. Отзыв, случившийся в эти микросекунды, не
/// находит чего рвать, запись об отзыве ложится в книгу допуска — и поток
/// живёт дальше, переживая собственный отзыв. Поэтому после записи вожжей
/// допуск спрашивается у книги ЕЩЁ РАЗ, и ответ разбирается здесь: отзыв
/// либо увидит наши вожжи, либо эта проверка увидит отзыв, третьего порядка
/// у двух событий нет.
///
/// Безымянный разговор не держится: проверить его допуск нечем — `enrolled_to`
/// спрашивают именем карточки, — а «не знаю» на вопросе о допуске значит
/// «нет» везде в этом проекте.
///
/// Спрашивается КАЖДАЯ карточка потока, и хватает одной снятой, чтобы поток
/// не держать: держать его наполовину нельзя, а делить нечего.
fn keep_holding(cards: &[String], all_still_enrolled: bool) -> bool {
    !cards.is_empty() && all_still_enrolled
}

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
    // Базы, о которых договорились: у зачисления одна, у возврата уже
    // зачисленной стороны — ВСЕ, к которым её когда-то пустили. Второй случай
    // приносит несколько, и брать из них первую нельзя (находка проверки
    // безопасности): поток обслуживал бы все, а отзыв находил бы его только
    // по одной.
    let agreed: Vec<String> = match &out {
        enroll::Outcome::Enrolled(base) => vec![base.base_id.clone()],
        enroll::Outcome::Already(bases) => bases.iter().map(|b| b.id.clone()).collect(),
        enroll::Outcome::NotEnrolled(no) => {
            say(&tx, Kind::Say, no.text.clone());
            control.end(None).await;
            return;
        }
    };
    // Имена карточек по базам, о которых договорились: они местные и по
    // каналу не ездят (спека), поэтому обратно сопоставляем здесь, по тем же
    // `base_id`, которыми карточки и представились.
    //
    // Пустой список — не сошлось ни одной, и подставлять пустую строку
    // нельзя: под пустым именем поток не нашёлся бы при отзыве (находка
    // проверки безопасности). Безымянный разговор учитывается как
    // безымянный и режется любым отзывом этой стороны (`cut_these`).
    let served: Vec<String> = cards.iter().filter(|(_, c)| agreed.contains(&c.base.base_id)).map(|(name, _)| name.clone()).collect();
    // Имя базы для прощания — первая из тех, о которых договорились: прощание
    // называет предмет, а не перечисляет всё, к чему сторона допущена.
    let base_id = agreed.first().cloned().unwrap_or_default();
    // Слово состояния — только там, где карточка известна: завести строку
    // состояния для имени, которого в списке нет, значило бы показать
    // человеку карточку-призрак.
    let shown = if served.is_empty() { "карточка не опознана".to_string() } else { served.join("», «") };
    for name in &served {
        flow(&tx, name, Some(link::Flow::Up));
    }
    say(&tx, Kind::Ok, format!("«{shown}»: подключено"));
    let (reins, mut recv) = channel::reins(control);
    let id = live.remember(peer, served.clone(), reins);
    // Щель между решением человека и записью вожжей: спрашиваем допуск ЕЩЁ
    // РАЗ, уже после записи. Отзыв, случившийся в эти микросекунды, либо
    // увидит наши вожжи и порвёт поток сам, либо не увидит — и тогда его
    // увидит эта проверка. Третьего порядка у двух событий нет.
    //
    // Спрашивается КАЖДАЯ карточка потока: сторона, зачисленная к нескольким,
    // держит их все одним потоком, и снятая одна значит снятый поток.
    let still = served.iter().all(|name| book.enrolled_to(name, peer).ok().flatten().is_some());
    if !keep_holding(&served, still) {
        // Рвём только СВОЙ разговор и только если отзыв не забрал его раньше:
        // иначе второе прощание ушло бы в поток, которого уже нет.
        if live.alive(peer, id) {
            if let Some(reins) = live.forget(peer, id) {
                reins.revoke(&base_id, REVOKED_TEXT).await;
            }
        }
        for name in &served {
            flow(&tx, name, Some(link::Flow::Broken));
        }
        say(&tx, Kind::Warn, format!("«{shown}»: разрешение снято, пока шёл разговор — поток закрыт"));
        return;
    }
    let gone = channel::hold(&mut recv, |_| {}).await;
    // Вожжи уходят с учёта и падают здесь: их задача письма дошлёт написанное
    // и снимет поток сама. Если отзыв успел забрать их раньше — забирать
    // нечего, и это не беда.
    drop(live.forget(peer, id));
    for name in &served {
        flow(&tx, name, Some(link::Flow::Broken));
    }
    match gone {
        Gone::Farewell(why) => say(&tx, Kind::Say, format!("«{shown}»: {}", why.text)),
        Gone::Broken(why) => say(&tx, Kind::Say, format!("«{shown}»: {}", why.text)),
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
///
/// Читается ТЕРПИМО (`load_unchecked`), той же мерой, что и в окне. Строгое
/// чтение (`load` с `validate`) отвергает полузаполненную карточку целиком —
/// а только что заведённая карточка полузаполнена всегда: `mode` у неё
/// `"tunnel"`, а адрес среды и рабочее место пусты, и человек их ещё не
/// вписывал. Окно такую карточку показывает и код по ней выдаёт, а приёмная
/// сторона её не видела вовсе и отвечала той машине «карточку удалили» —
/// словами про чужую вину за свою строгость (живая проверка двумя машинами
/// 9.11). Сломанный toml остаётся отказом и здесь: `load_unchecked` снимает
/// проверку заполненности, а не проверку читаемости.
fn cards_of(kit: &Kit) -> Vec<(String, enroll::Card)> {
    match crate::secrets::Store::system(kit.state()) {
        Ok(store) => cards_of_with(kit, &store),
        Err(_) => Vec::new(),
    }
}

/// То же, но подложку секретов даёт вызывающий — ей проверяются тесты (тот же
/// приём и тот же довод, что у `Cfg::load_with`: DPAPI из тестов не зовём).
/// Проверка висит именно здесь, на строке выбора меры чтения, а не на чистом
/// отборе ниже: вернуть сюда строгое `load_with` — и она покраснеет.
fn cards_of_with(kit: &Kit, store: &crate::secrets::Store) -> Vec<(String, enroll::Card)> {
    match crate::cfg::Cfg::load_unchecked_with(kit, store) {
        Ok(cfg) => accepting_cards(&cfg),
        Err(_) => Vec::new(),
    }
}

/// Отбор принимающих карточек из УЖЕ прочитанных настроек — чистая половина
/// `cards_of`.
fn accepting_cards(cfg: &crate::cfg::Cfg) -> Vec<(String, enroll::Card)> {
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

/// Проверяются здесь два решения, и оба — про обход отзыва (находки
/// автоматической проверки безопасности на коммит задачи 9). Остальное в этом
/// файле — проводка: поток, рантайм, два узла, диск; решения из неё вынесены
/// сюда нарочно, чтобы их можно было проверить без живого окна.
#[cfg(test)]
mod tests {
    // Именованный список, а не `super::*`: в области видимости этого модуля
    // живёт `gpui::*`, и его `test` перебил бы `#[test]` стандартной
    // библиотеки (тот же приём, что у тестов `panel.rs`).
    use super::{cut_these, keep_holding};

    /// Разговоры по именам карточек: пустая строка — разговор, чья карточка
    /// не опознана; «А+Б» — разговор, обслуживающий сразу две.
    fn named(names: &[&str]) -> Vec<Vec<String>> {
        names.iter().map(|n| n.split('+').filter(|p| !p.is_empty()).map(|p| p.to_string()).collect()).collect()
    }

    /// Отзыв во время разговора обязан снять поток ЭТОЙ карточки — и только
    /// его.
    ///
    /// Падал так: живые разговоры лежали под ключом «имя карточки + сторона»,
    /// где имя добывалось перебором по признаку базы. Совпало не то имя —
    /// поток не находился, и человек, нажавший «Отозвать», получал запись об
    /// отзыве и живой поток сверху. Отзыв перестаёт быть немедленным ровно
    /// там, где это важнее всего.
    #[test]
    fn revoking_during_a_talk_cuts_that_cards_stream() {
        let talks = named(&["Ромашка", "Василёк", "Ромашка"]);
        assert_eq!(cut_these(&talks, "Ромашка"), vec![0, 2]);
        // Соседняя карточка той же стороны не трогается: человек снял доступ
        // к одной базе, а не ко всем.
        assert_eq!(cut_these(&talks, "Василёк"), vec![1]);
        // Отзыв карточки, у которой живого разговора нет, не режет ничего.
        assert_eq!(cut_these(&talks, "Пион"), Vec::<usize>::new());
    }

    /// Отзыв по стороне, чья карточка не опознана, тоже обязан рвать.
    ///
    /// Падал так: безымянный разговор лежал под ПУСТЫМ именем, и отзыв по
    /// настоящему имени карточки его не находил никогда — то есть именно тот
    /// поток, про который мы меньше всего знаем, переживал отзыв.
    #[test]
    fn a_stream_with_no_card_name_is_cut_too() {
        let talks = named(&["", "Василёк"]);
        assert_eq!(cut_these(&talks, "Ромашка"), vec![0], "безымянный режется любым отзывом этой стороны");
        assert_eq!(cut_these(&talks, "Василёк"), vec![0, 1], "и вместе с названным — тоже");
    }

    /// Поток, обслуживающий НЕСКОЛЬКО карточек, режется отзывом любой из них.
    ///
    /// Падал так: сторона, зачисленная к двум карточкам этой машины,
    /// возвращается одним потоком, и `enroll::Outcome::Already` приносит обе
    /// базы. В учёт шла первая из списка — значит отзыв ВТОРОЙ карточки такой
    /// поток не находил: он оставался живым и продолжал обслуживать доступ,
    /// который человек только что снял (находка проверки безопасности на
    /// коммит 9d232f5).
    #[test]
    fn a_stream_serving_two_cards_is_cut_by_revoking_either() {
        let talks = named(&["Ромашка+Василёк", "Пион"]);
        assert_eq!(cut_these(&talks, "Ромашка"), vec![0], "отзыв первой из двух режет поток");
        assert_eq!(cut_these(&talks, "Василёк"), vec![0], "и отзыв второй — тоже, он не «не та карточка»");
        assert_eq!(cut_these(&talks, "Пион"), vec![1], "сосед, которого отзыв не касается, цел");
        // И держать такой поток нельзя, если снята хотя бы одна из карточек:
        // половины у потока нет, делить нечего.
        let both = vec!["Ромашка".to_string(), "Василёк".to_string()];
        assert!(keep_holding(&both, true), "обе на месте — держим");
        assert!(!keep_holding(&both, false), "одну сняли — поток не держим");
    }

    /// Щель между решением человека и записью вожжей: разговор, у которого
    /// разрешения уже нет, не держится.
    ///
    /// Падал так: проверки после записи вожжей не было вовсе. `accept_side`
    /// вернул «зачислен», вожжи ещё не записаны — и отзыв, попавший в эти
    /// микросекунды, не находил чего рвать; поток переживал собственный отзыв
    /// до следующего круга той стороны.
    #[test]
    fn a_talk_whose_permission_is_already_gone_is_not_held() {
        let one = vec!["Ромашка".to_string()];
        assert!(keep_holding(&one, true), "разрешение на месте — держим");
        assert!(!keep_holding(&one, false), "разрешение сняли в щель — не держим");
        // «Не знаю» на вопросе о допуске значит «нет»: безымянную карточку
        // `enrolled_to` спросить нечем.
        assert!(!keep_holding(&[], true));
        assert!(!keep_holding(&[], false));
    }

    /// Книга живых разговоров: разложены по стороне, снимаются по своему
    /// номеру, и отзыв уносит их с учёта — второй отзыв резать уже нечего.
    ///
    /// Вожжи здесь не завести (за ними задача письма в рантайме), поэтому
    /// проверяется та часть, которая от них не зависит: учёт по стороне и
    /// выбор жертв. Сами вожжи проверены в `channel.rs`.
    #[test]
    fn the_live_book_keeps_talks_under_the_party_not_the_card() {
        // Три разговора одной стороны и один — соседней, по именам карточек.
        let mine = named(&["Ромашка", "", "Василёк"]);
        let doomed = cut_these(&mine, "Ромашка");
        assert_eq!(doomed, vec![0, 1], "своя карточка и безымянный");
        // Удаление идёт с конца — иначе второе место съехало бы после
        // первого удаления, и под нож попал бы «Василёк».
        let mut left = mine.clone();
        for at in doomed.into_iter().rev() {
            left.remove(at);
        }
        assert_eq!(left, named(&["Василёк"]), "сосед той же стороны остался цел");
    }

    /// «Выход» не держится прощанием, когда прощаться не с кем.
    ///
    /// Потока узла может не быть вовсе — его не подняли (нет сети, не открылся
    /// ключ) или он кончился. Приказ тогда никуда не уходит, ответа не будет
    /// НИКОГДА, и выход, который ждёт ответа по сроку, стоил бы человеку
    /// полного срока ожидания на каждом закрытии программы. Поэтому мёртвый
    /// канал приказов — это успех: прощаться не с кем.
    #[test]
    fn quitting_is_not_held_by_a_farewell_to_nobody() {
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        // Поток узла кончился: получателя приказов больше нет.
        drop(rx);
        let peer = super::Peer { cmd, reach: std::sync::Arc::new(std::sync::Mutex::new(None)) };
        let wait = std::time::Duration::from_secs(5);
        let started = std::time::Instant::now();
        assert!(peer.bye_all(wait), "прощаться не с кем — это успех, а не отказ");
        assert!(started.elapsed() < wait / 2, "выход обязан вернуться сразу, а не выстоять срок впустую");
    }

    /// Только что заведённая принимающая карточка обязана быть видна приёмной
    /// стороне.
    ///
    /// Падало так: `cards_of` читала настройки СТРОГО (`load` с `validate`), а
    /// окно — терпимо. Карточка, заведённая в окне минуту назад, полузаполнена
    /// всегда (`mode = "tunnel"`, адрес среды и рабочее место пусты), строгое
    /// чтение отвергало весь файл — и приёмная сторона отвечала той машине
    /// «карточку удалили» про карточку, которая лежит на диске и по которой
    /// окно только что выдало код. Живая проверка двумя машинами 9.11: путь не
    /// проходился вовсе, диалог с отпечатком не показывался ни разу.
    ///
    /// Текст ниже — буква в букву то, что окно пишет на диск при ответе
    /// «Принимаю подключения».
    #[test]
    fn a_half_filled_accepting_card_is_still_seen_by_the_peer() {
        use crate::cfg::Cfg;
        use crate::secrets;
        let text = "current = \"\"\n\n[[connection]]\nname = \"Соединение 1\"\nserver = \"\"\ngitea = \"\"\npassword = \"\"\n\
                    workstation = \"\"\nmode = \"tunnel\"\nbase_path = \"\"\nbase_here = true\nrole = \"accept\"\n";
        let dir = std::env::temp_dir().join(format!("peer-cards-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::cfg::FILE), text).unwrap();
        let kit = super::Kit::at(dir.clone());
        // Подложка секретов — файловая: DPAPI из тестов не зовём (спека).
        let store = secrets::Store::file(&dir);

        // Первая половина проверки: строгое чтение этот файл ОТВЕРГАЕТ. Без
        // неё правка «читать терпимо» стерегла бы пустоту — файл мог бы
        // проходить обе меры одинаково, и тест был бы зелен сам по себе.
        assert!(Cfg::load_with(&kit, &store).is_err(), "строгое чтение обязано отвергать полузаполненную карточку");

        // Вторая: приёмная сторона видит ровно одну принимающую карточку —
        // ту самую, по имени. Спрашивается та самая функция, которая читает
        // диск на каждый входящий разговор.
        let cards = super::cards_of_with(&kit, &store);
        assert_eq!(cards.len(), 1, "карточка на диске есть — значит, и у приёмной стороны она есть");
        assert_eq!(cards[0].0, "Соединение 1");
    }
}
