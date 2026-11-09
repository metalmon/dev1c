//! Канал: соединение iroh на пару машин и управляющие потоки внутри него.
//!
//! Нижний слой — узел и провод — в `node.rs`; разговор зачисления — в
//! `enroll.rs`. Здесь то, что между ними: кто принимает входящие, кто звонит,
//! как долго живёт поток и чем его конец отличается от обрыва.
//!
//! **Управляющий поток — на СОЕДИНЕНИЕ, а не на канал** (решение владельца
//! 1.11, спека, раздел «Потоки»). У каждой карточки свой управляющий поток, и
//! открывает его та сторона, которая по ЭТОЙ карточке зовущая. Канал при этом
//! остаётся один на пару машин: внутри него уживается по одному управляющему
//! потоку на живую карточку, и направления у них бывают разные. Довод спеки:
//! иначе машина, у которой с одним и тем же партнёром соединения обоих
//! направлений (аутсорсер звонит по базе заказчика и сам принимает звонок того
//! же заказчика к своей тестовой базе), нарушала бы правило «принимающая не
//! звонит» самим фактом своего существования.
//!
//! **Закрытие управляющего потока — конец ЭТОГО соединения, а не канала.**
//! Соседние карточки в том же канале живут дальше. Канал закрывается сам,
//! когда в нём не осталось ни одного живого управляющего потока ни в каком
//! направлении, — и считает их `Hold`, снимаемый `Drop`, а не пересчёт по
//! запросу: путей выхода из разговора много, и последний забудут.
//!
//! **Своего пинга нет** (решение спеки). Жив ли путь — знает iroh, и второй
//! таймер поверх его таймера дал бы только два разных мнения о том, когда
//! считать связь потерянной. Поэтому `hold` ждёт кадр БЕЗ своего срока: на
//! управляющем потоке молчание законно часами (у человека на той стороне может
//! не меняться ничего), и срок здесь означал бы «разорвать живое».
//!
//! **`bye` отличает намеренный уход от обрыва.** Выключили обслуживание,
//! закрыли окно, снимается машина — человек на той стороне обязан прочитать
//! причину, а не «связь пропала». Поэтому конец потока — не одно событие, а
//! два разных (`Gone`), и перепутать их нечем.
//!
//! Спека: `docs/superpowers/specs/2026-11-01-protocol-design.md`, разделы
//! «Один канал на машину, а не на базу», «Потоки», «Обрыв и переподключение».

use crate::node::Node;
use crate::wire::{code, Frame, Msg, Refuse};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use iroh::{EndpointAddr, EndpointId};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncRead;

/// Код закрытия канала на проводе QUIC — ноль, «закрыли нарочно и без жалоб».
///
/// Причина, если она есть, уже сказана словами в `bye` по управляющему потоку;
/// второй её экземпляр числом не прочитает никто, а разные числа тут
/// означали бы второе мнение о том же событии.
const CLOSED_BY_US: u32 = 0;

/// Предел задержки переподключения — минута (спека, «Обрыв и
/// переподключение»: «секунда, две, четыре и так до минуты, дальше раз в
/// минуту бесконечно»). Сдаваться незачем: сервер заказчика может быть
/// выключен на ночь.
pub const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// Сколько уходящий ждёт, пока его прощание признают доехавшим
/// (`Control::bye`). Пять секунд — это «провод жив, просто далеко»; больше
/// брать нечего: не доехало за них, значит той стороне и так достанется
/// обрыв, а держать закрытие окна человека дольше незачем.
const FAREWELL_PATIENCE: Duration = Duration::from_secs(5);

/// Чем кончился управляющий поток.
///
/// Два случая, а не один, и это главное, что знает этот модуль: человеку в
/// них говорят РАЗНОЕ. Намеренный уход несёт причину («обслуживание
/// выключили», «окно закрыли»), обрыв — совет подключиться заново. Свести их
/// в один `Refuse` со кодом внутри значило бы отдать различение на
/// сравнение строк, то есть потерять его при первой же правке текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gone {
    /// Та сторона ушла НАРОЧНО и сказала почему: `bye` (выключили
    /// обслуживание, закрыли окно, снимается машина) или `revoked` (сняли
    /// разрешение). Повторять звонок по своей воле нечего — причина у
    /// человека на руках, и дальше ход его.
    Farewell(Refuse),
    /// Поток кончился без слов: связь пропала, или та сторона закрыла его
    /// молча. Это повторяется (`keep_calling`).
    Broken(Refuse),
}

/// Управляющий поток одного СОЕДИНЕНИЯ (карточки) внутри канала.
///
/// Половины потока — поля, а не методы: читающая висит в `hold`, пока
/// пишущая шлёт `state` и `revoked`, и занимать обе одним `&mut self`
/// значило бы запретить это. Две разные половины одного значения компилятор
/// отдаёт порознь сам.
///
/// Пока эта вещь жива — жив и канал. Её `Drop` снимает счёт, и последний
/// снятый закрывает канал.
pub struct Control {
    pub send: SendStream,
    pub recv: RecvStream,
    _hold: Hold,
}

impl Control {
    /// Уйти нарочно: причина словами, потом конец своей половины потока.
    ///
    /// Забирает себя целиком (`self`), а не `&mut self`: после прощания по
    /// этому потоку говорить больше нечего, и возможность сказать ещё что-то
    /// — это ровно та возможность, от которой `bye` и отличает обрыв. Отказ
    /// записи проглатывается: мы уходим, и жаловаться на то, что прощание не
    /// доехало, некому — та сторона увидит обрыв, то есть худший, но верный
    /// исход.
    ///
    /// **Ждём, пока прощание признают доехавшим, и это не вежливость.** Этот
    /// поток может быть последним в канале, и тогда `Drop` ниже закроет сам
    /// канал — а закрытие QUIC выбрасывает всё неподтверждённое. Без этого
    /// ожидания человек на той стороне читал бы «связь пропала» вместо
    /// причины ровно в том случае, ради которого `bye` и заведён: выключили
    /// обслуживание, закрыли окно, снимается машина. `stopped` после
    /// `finish` завершается тогда, когда та сторона дочитала поток.
    ///
    /// Срок у ожидания свой и короткий: уходящему незачем висеть на мёртвом
    /// проводе — прощание не доехало, значит та сторона и так увидит обрыв.
    pub async fn bye(mut self, why: Refuse) {
        let _ = Frame::write(&mut self.send, &Msg::Bye(why)).await;
        let _ = self.send.finish();
        let _ = tokio::time::timeout(FAREWELL_PATIENCE, self.send.stopped()).await;
    }
}

/// Счёт живых управляющих потоков канала.
///
/// `Drop`, а не снятие руками: разговор кончается девятью путями (отказ
/// книги, отказ человека, обрыв, срок, паника задачи), и девятый забудут.
/// Снятие на `Drop` работает и на отмене всей задачи — а задачу бросают
/// именно так, когда человек выключает обслуживание или закрывает окно.
struct Hold {
    conn: Connection,
    live: Arc<AtomicUsize>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        // `fetch_sub` вернул единицу — снятый был последним, и канал держать
        // больше нечем. Закрытие синхронное (`Connection::close` не `async`),
        // и это единственная причина, по которой правило спеки «канал
        // закрывается сам» вообще выразимо в `Drop`.
        if self.live.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.conn.close(VarInt::from_u32(CLOSED_BY_US), b"");
        }
    }
}

/// Канал — ОДНО соединение iroh на пару машин (спека, «Один канал на машину,
/// а не на базу»).
///
/// Клонируется дёшево: `Connection` внутри за `Arc`, счёт потоков — общий
/// нарочно, иначе две копии канала считали бы «живых потоков не осталось»
/// каждая по-своему.
#[derive(Debug, Clone)]
pub struct Channel {
    conn: Connection,
    live: Arc<AtomicUsize>,
}

impl Channel {
    /// Кто на том конце. Ключ узла — то, что доказал сам iroh, а не то, что
    /// сказали в `hello`.
    pub fn peer(&self) -> EndpointId {
        self.conn.remote_id()
    }

    /// Жив ли канал прямо сейчас. Нужно не протоколу (он узнаёт о конце из
    /// потока), а тому, кто спрашивает «закрылся ли он сам»: окну в журнал и
    /// тесту про последний поток.
    pub fn is_open(&self) -> bool {
        self.conn.close_reason().is_none()
    }

    /// Открыть управляющий поток: по ЭТОЙ карточке зовущая — эта сторона.
    ///
    /// Счёт берётся ДО открытия потока нарочно: между `fetch_add` и первым
    /// кадром канал не должен считаться опустевшим, иначе соседний поток,
    /// кончившийся в этот самый миг, закрыл бы канал у нас под руками.
    pub async fn call(&self) -> Result<Control, String> {
        let hold = self.hold();
        let (send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| format!("не удалось открыть управляющий поток: {e}. Проверьте связь и подключитесь заново."))?;
        Ok(Control { send, recv, _hold: hold })
    }

    /// Принять управляющий поток, который открыла та сторона: по этой
    /// карточке зовущая — она.
    pub async fn answer(&self) -> Result<Control, String> {
        let hold = self.hold();
        let (send, recv) = self.conn.accept_bi().await.map_err(|e| format!("входящий управляющий поток не открылся: {e}"))?;
        Ok(Control { send, recv, _hold: hold })
    }

    fn hold(&self) -> Hold {
        self.live.fetch_add(1, Ordering::SeqCst);
        Hold { conn: self.conn.clone(), live: self.live.clone() }
    }
}

/// Живые каналы этой машины — по одному на партнёра.
///
/// Нужен затем, чтобы вторая карточка к тому же партнёру ехала ПОТОКОМ в уже
/// живом канале, а не вторым соединением: второй канал к тому же узлу — это
/// второй проход пробивания NAT, второй разговор с релеем и второе состояние
/// перегрузки, и ничего из этого он не покупает (спека, «Один канал на
/// машину»). Правило «канал закрывается, когда не осталось живых потоков ни
/// в каком направлении» без этой книги не выразимо вовсе: без неё у каждого
/// направления был бы свой канал и свой счёт.
///
/// Потолок: два одновременных звонка к одному и тому же партнёру могут
/// успеть завести два канала — проверка и звонок не под одним замком
/// нарочно, держать замок через `await` дозвона значило бы запереть на это
/// время все остальные карточки. Последствие безвредное: в книге остаётся
/// последний, прежний живёт своими потоками и закрывается, когда они
/// кончатся. Выйдет это из разряда «безвредно» тогда, когда карточек к
/// одному партнёру станет много и человек начнёт подключать их залпом; тогда
/// здесь встанет замок на партнёра (`tokio::sync::Mutex` на запись), а не
/// на всю книгу.
#[derive(Clone, Default)]
pub struct Channels(Arc<Mutex<HashMap<EndpointId, Channel>>>);

impl Channels {
    /// Канал к этому партнёру: живой — тот же самый, нет такого — новый
    /// звонок.
    pub async fn to(&self, node: &Node, whom: EndpointAddr) -> Result<Channel, String> {
        if let Some(live) = self.live(whom.id) {
            return Ok(live);
        }
        Ok(self.keep(node.dial(whom).await?))
    }

    /// Запомнить канал — входящий или только что набранный.
    ///
    /// Отравленный замок не повод отказывать: данные берутся из паники
    /// (`into_inner`), и хуже лишнего канала от этого не станет — тот же
    /// приём и тот же довод, что у оговорки кода в `enroll::claim`.
    pub fn keep(&self, conn: Connection) -> Channel {
        let peer = conn.remote_id();
        let channel = Channel { conn, live: Arc::new(AtomicUsize::new(0)) };
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|_, ch| ch.is_open());
        held.insert(peer, channel.clone());
        channel
    }

    fn live(&self, peer: EndpointId) -> Option<Channel> {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|_, ch| ch.is_open());
        held.get(&peer).cloned()
    }
}

/// Держит управляющий поток, пока та сторона говорит.
///
/// Возвращается ровно тогда, когда это соединение кончилось, — и говорит,
/// кончилось оно словами или молча.
///
/// `heard` зовётся на каждое сообщение, которое не кончает поток: `state`
/// (что-то изменилось на стороне базы), `refuse` (та сторона отказала в
/// чём-то, что мы просили), всё прочее этой версии. Своего разбора здесь
/// нет нарочно: что делать с `state`, знает окно, а не провод.
///
/// **Срока ожидания здесь НЕТ**, и это решение спеки, а не недосмотр: своего
/// пинга протокол не держит, молчание на управляющем потоке законно часами,
/// и срок означал бы разорвать живое по своему таймеру, поссорив его с
/// таймером iroh.
///
/// `revoked` кончает поток прощанием, а не обрывом: разрешение снял человек,
/// и той стороне положено прочитать его слова. Поле `base_id` при этом не
/// читается — на потоке, который и есть одна карточка, оно повторяет то, что
/// читающий уже знает; разбирать его значило бы заводить случай «отозвали
/// чужую базу по нашему потоку», которого в протоколе нет.
pub async fn hold<R: AsyncRead + Unpin>(r: &mut R, mut heard: impl FnMut(Msg)) -> Gone {
    loop {
        match Frame::read(r).await {
            Ok(Msg::Bye(why)) => return Gone::Farewell(why),
            Ok(Msg::Revoked { text, .. }) => return Gone::Farewell(Refuse::new(code::REVOKED, text)),
            Ok(msg) => heard(msg),
            Err(no) => return Gone::Broken(no),
        }
    }
}

/// Приём входящих: на каждый канал — свой присмотр, на каждый управляющий
/// поток в нём — свой разговор.
///
/// Возвращается, когда узел закрыт и принимать больше нечего, — это конец
/// работы, а не беда (`Node::accept`).
///
/// Три задачи, а не одна, и это не щедрость: канал живёт дольше любого
/// своего потока, а поток — дольше разговора по нему. Один цикл на всё
/// означал бы, что вторая карточка ждёт, пока человек ответит по первой.
///
/// `note` — строка в журнал о беде, которая никому больше не достанется:
/// входящее подключение, не состоявшееся на рукопожатии, сказать некому —
/// того конца у нас уже нет.
pub async fn accept_all<T, Fut>(node: &Node, channels: &Channels, note: impl Fn(String), talk: T)
where
    T: Fn(Control) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    while let Some(incoming) = node.accept().await {
        let conn = match incoming {
            Ok(conn) => conn,
            Err(why) => {
                note(why);
                continue;
            }
        };
        let channel = channels.keep(conn);
        let talk = talk.clone();
        tokio::spawn(async move {
            // Поток за потоком, пока канал жив: `accept_bi` отказывает ровно
            // тогда, когда он закрылся, и этого отказа ждать не надо — он и
            // есть конец присмотра.
            while let Ok(control) = channel.answer().await {
                tokio::spawn(talk(control));
            }
        });
    }
}

/// Чем кончился один круг звонка зовущей стороны.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Round {
    /// Дошли до «подключено» и держали поток; вот чем он кончился.
    Held(Gone),
    /// Разговор зачисления кончился исходом: человек отклонил, человек не
    /// ответил, код сгорел. Сами не повторяем — повтор и есть то дёрганье, от
    /// которого заведён секрет тикета (спека, «Зачисление»).
    NotEnrolled(Refuse),
    /// Не дозвонились, или разговор сорвался на полуслове. Это повторяется.
    Failed(String),
}

/// Почему зовущая сторона перестала звонить. Третьего случая нет: всё
/// остальное повторяется бесконечно, и возврата из `keep_calling` у него не
/// бывает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// Та сторона простилась словами или сняла разрешение.
    Farewell(Refuse),
    /// Исход зачисления.
    NotEnrolled(Refuse),
}

/// Задержка перед попыткой номер `attempt`. Нулевая попытка — первая, она
/// идёт сразу: ждать там нечего.
///
/// Секунда, две, четыре и так до минуты, дальше раз в минуту бесконечно
/// (спека, «Обрыв и переподключение»). Сдаваться незачем: сервер заказчика
/// может быть выключен на ночь, и человек не должен нажимать «подключиться»
/// утром.
///
/// `checked_pow`: номер попытки растёт без предела, и переполнение степени
/// здесь — не выдуманный случай, а неделя без связи.
pub fn backoff(attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let seconds = 2u64.checked_pow(attempt - 1).unwrap_or(u64::MAX);
    Duration::from_secs(seconds.min(LONGEST_WAIT.as_secs()))
}

/// Зовёт, пока звать есть смысл: круг за кругом с нарастающей задержкой.
///
/// **Зовёт всегда одна и та же сторона** — та, что вставляла код при
/// заведении (спека). Принимающая не звонит никогда, даже зная адрес: иначе
/// «кто кого зовёт» переставало бы быть свойством заведения и становилось
/// гонкой. Поэтому у этой функции нет ни развилки по роли, ни признака
/// направления: её зовёт только зовущая сторона, и роль записана в карточке.
///
/// Один круг — дело вызывающего (`round`): дозвониться, открыть управляющий
/// поток, провести зачисление, держать. Так здесь остаётся ровно правило
/// повтора, а всё, что к нему не относится, — снаружи; и так же тест может
/// подставить свой круг, не поднимая узлов.
///
/// **Ладка сбрасывается после удачного круга.** Соединение, прожившее часы и
/// оборвавшееся, возвращается через секунду, а не через минуту: нарастание
/// задержек защищает от бесплодного долбления в выключенную машину, а не
/// наказывает за прошлые обрывы.
///
/// **Признака остановки здесь нет.** Человек, выключивший обслуживание или
/// закрывший окно, роняет саму задачу — а вместе с ней `Control` и `Hold`,
/// то есть канал закрывается тем же `Drop`, что и всегда. Второй путь
/// остановки (флаг, за которым надо подглядывать) означал бы два разных
/// мнения о том, звоним ли мы ещё.
pub async fn keep_calling<F, Fut>(mut round: F) -> Stopped
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Round>,
{
    let mut attempt = 0u32;
    loop {
        match round().await {
            Round::Held(Gone::Farewell(why)) => return Stopped::Farewell(why),
            Round::NotEnrolled(no) => return Stopped::NotEnrolled(no),
            // Круг дошёл до «подключено» — ладка начинается заново.
            Round::Held(Gone::Broken(_)) => attempt = 0,
            Round::Failed(_) => {}
        }
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(backoff(attempt)).await;
    }
}

/// Слова прощания на три случая, в которых уходим МЫ. Третья строка — совет,
/// как велит раздел «Ошибки» спеки: причина без совета человеку не нужна.
pub const SERVING_OFF: &str = "Та сторона выключила обслуживание.\nЧеловек там снял галочку, и вызовы агента больше не \
                               обслуживаются.\nПопросите включить её снова — подключение восстановится само.";

/// Окно закрыли: программа на той стороне больше не работает.
pub const WINDOW_CLOSED: &str = "Та сторона закрыла программу.\nСоединение закончилось нарочно, а не оборвалось.\nПопросите запустить её \
                                 снова — подключение восстановится само.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enroll::{accept_side, dial_side, Card, Decision, Outcome};
    use crate::enrolled::Book;
    use crate::ticket::Ticket;
    use crate::wire::{BaseFields, CardState, Ceiling};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use tokio::io::AsyncWriteExt;

    /// Предел ожидания сети в тесте: упавший шаг обязан выглядеть упавшим, а
    /// не зависшим прогоном.
    const WAIT: Duration = Duration::from_secs(20);

    /// Свой каталог на каждый тест — ключ узла и книга зачислений лежат
    /// файлами (тот же приём, что в `node.rs` и `enroll.rs`).
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("onec-channel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Адрес узла для звонка внутри процесса — петля и свой порт. Довод тот
    /// же, что у одноимённых помощников в `node.rs` и `enroll.rs`: iroh
    /// кладёт петлю в адрес узла только когда других интерфейсов нет вовсе, а
    /// на рабочей машине звонок пошёл бы на свой же адрес в локальной сети,
    /// то есть через брандмауэр — и тест проверял бы настройки Windows.
    fn by_loopback(node: &Node) -> EndpointAddr {
        let port = node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find_map(|s| if s.is_ipv4() { Some(s.port()) } else { None })
            .expect("узел обязан был занять порт IPv4");
        EndpointAddr::new(node.id()).with_ip_addr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    fn fields(id: &str) -> BaseFields {
        BaseFields {
            base_id: id.to_string(),
            base_name: format!("База {id}"),
            publication: format!("http://127.0.0.1:8080/{id}"),
            write: true,
        }
    }

    /// Прощание и обрыв — два разных события, и человеку в них говорят
    /// разное. Проверяется на памяти, без узлов: предмет здесь — чтение
    /// потока, а не сеть.
    #[tokio::test]
    async fn a_deliberate_goodbye_is_told_apart_from_a_break() {
        // Ушли нарочно и сказали почему.
        let (mut theirs, mut ours) = tokio::io::duplex(4096);
        Frame::write(&mut theirs, &Msg::Bye(Refuse::new(code::MCP_OFF, SERVING_OFF))).await.unwrap();
        drop(theirs);
        let end = hold(&mut ours, |msg| panic!("кроме прощания ничего не присылали, а пришло {msg:?}")).await;
        let Gone::Farewell(why) = end else {
            panic!("намеренный уход обязан отличаться от обрыва: {end:?}")
        };
        assert_eq!(why.code, code::MCP_OFF, "причина ухода — её, а не наша придуманная");
        assert!(why.text.contains("выключила обслуживание"), "человек читает причину, а не «связь пропала»: {}", why.text);

        // Поток кончился молча — это обрыв, и совет человеку другой.
        let (theirs, mut ours) = tokio::io::duplex(4096);
        drop(theirs);
        let end = hold(&mut ours, |_| unreachable!("по закрытому потоку ничего не приходит")).await;
        let Gone::Broken(no) = end else { panic!("молчаливый конец — обрыв, а не прощание: {end:?}") };
        assert!(
            no.text.contains("подключитесь заново") || no.text.contains("Подключитесь заново"),
            "обрыв советует вернуться: {}",
            no.text
        );

        // Обрыв посреди кадра — тоже обрыв, а не прощание.
        let (mut theirs, mut ours) = tokio::io::duplex(4096);
        theirs.write_all(&[0, 0, 0, 9, b'{']).await.unwrap();
        drop(theirs);
        let end = hold(&mut ours, |_| unreachable!("целого кадра там нет")).await;
        assert!(matches!(end, Gone::Broken(_)), "половина кадра — обрыв: {end:?}");

        // Снятое разрешение — тоже намеренный уход: его словами сказал
        // человек, и читать их обязаны, а не видеть «связь пропала».
        let (mut theirs, mut ours) = tokio::io::duplex(4096);
        Frame::write(&mut theirs, &Msg::Revoked { base_id: "b1".into(), text: "Доступ отозван на стороне базы.".into() }).await.unwrap();
        drop(theirs);
        let end = hold(&mut ours, |_| unreachable!("после отзыва говорить нечего")).await;
        let Gone::Farewell(why) = end else { panic!("отзыв — намеренный уход: {end:?}") };
        assert_eq!(why.code, code::REVOKED);
        assert!(why.text.contains("отозван"), "{}", why.text);

        // И сообщения, которые поток НЕ кончают, доходят до того, кто их
        // ждёт, а не глотаются здесь.
        let (mut theirs, mut ours) = tokio::io::duplex(4096);
        let said = Msg::State { base_id: "b1".into(), state: CardState::Connected, write: true, serving: true, ceiling: None, text: None };
        Frame::write(&mut theirs, &said).await.unwrap();
        Frame::write(&mut theirs, &Msg::Bye(Refuse::new(code::MCP_OFF, SERVING_OFF))).await.unwrap();
        drop(theirs);
        let mut heard = Vec::new();
        let end = hold(&mut ours, |msg| heard.push(msg)).await;
        assert!(matches!(end, Gone::Farewell(_)));
        assert_eq!(heard, vec![said], "состояние обязано дойти до окна, а не кончиться здесь");
    }

    /// Закрытие управляющего потока кончает ОДНО соединение: соседняя
    /// карточка в том же канале живёт, а сам канал закрывается только тогда,
    /// когда живых потоков не осталось ни одного.
    #[tokio::test]
    async fn closing_the_control_stream_ends_that_connection_only() {
        let (da, db) = (dir("one-of-two-a"), dir("one-of-two-b"));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let (mine, theirs) = (Channels::default(), Channels::default());

        let listening = base.clone();
        let theirs_there = theirs.clone();
        let answered = tokio::spawn(async move {
            let conn = listening.accept().await.unwrap().unwrap();
            let channel = theirs_there.keep(conn);
            let one = channel.answer().await.unwrap();
            let two = channel.answer().await.unwrap();
            (channel, one, two)
        });

        let channel = tokio::time::timeout(WAIT, mine.to(&caller, by_loopback(&base))).await.expect("дозвон не уложился в срок").unwrap();
        assert_eq!(channel.peer(), base.id(), "канал ведёт к тому, кому звонили");
        let mut first = channel.call().await.unwrap();
        let mut second = channel.call().await.unwrap();
        // По кадру в каждый: поток, в который не написали, та сторона не
        // увидит вовсе — QUIC заводит его на первом байте.
        Frame::write(&mut first.send, &Msg::EnrollWait).await.unwrap();
        Frame::write(&mut second.send, &Msg::EnrollWait).await.unwrap();

        let (their_channel, mut their_first, mut their_second) =
            tokio::time::timeout(WAIT, answered).await.expect("та сторона не приняла оба потока в срок").unwrap();
        assert_eq!(Frame::read(&mut their_first.recv).await.unwrap(), Msg::EnrollWait);
        assert_eq!(Frame::read(&mut their_second.recv).await.unwrap(), Msg::EnrollWait);
        assert_eq!(their_channel.peer(), caller.id(), "у канала два конца, и оба знают, кто на том");

        // Первая карточка прощается и закрывает свой поток.
        first.bye(Refuse::new(code::MCP_OFF, SERVING_OFF)).await;
        let end = tokio::time::timeout(WAIT, hold(&mut their_first.recv, |_| {})).await.expect("прощание не доехало в срок");
        assert!(matches!(end, Gone::Farewell(_)), "ушли словами: {end:?}");
        drop(their_first);

        // Канал жив, и соседняя карточка по нему говорит.
        assert!(channel.is_open(), "канал закрылся вместе с ОДНОЙ карточкой — остальные остались бы без связи ни за что");
        assert!(their_channel.is_open(), "и на том конце тоже");
        let state =
            Msg::State { base_id: "b2".into(), state: CardState::Connected, write: false, serving: true, ceiling: None, text: None };
        Frame::write(&mut their_second.send, &state).await.unwrap();
        assert_eq!(
            tokio::time::timeout(WAIT, Frame::read(&mut second.recv)).await.expect("соседняя карточка замолчала").unwrap(),
            state,
            "соседняя карточка обязана жить дальше"
        );

        // Ушёл последний поток — канал закрывается сам.
        drop(second);
        assert!(!channel.is_open(), "канал обязан закрыться, когда живых управляющих потоков не осталось ни одного");
        drop(their_second);
        assert!(!their_channel.is_open(), "и на том конце тоже: там тоже кончился последний поток");

        caller.close().await;
        base.close().await;
    }

    /// Вторая карточка к тому же партнёру едет ПОТОКОМ в уже живом канале, а
    /// не вторым соединением (спека, «Один канал на машину, а не на базу»).
    ///
    /// Доказательство — счёт потоков, а не слово «канал тот же»: на одном
    /// канале два потока дают счёт два, и снятие первого канала не закрывает.
    /// Будь это два канала, у каждого был бы свой счёт, и первый закрылся бы
    /// вместе со своим единственным потоком.
    #[tokio::test]
    async fn a_second_card_to_the_same_partner_goes_down_the_same_channel() {
        let (da, db) = (dir("one-channel-a"), dir("one-channel-b"));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let (mine, theirs) = (Channels::default(), Channels::default());

        let listening = base.clone();
        let theirs_there = theirs.clone();
        let answered = tokio::spawn(async move {
            let conn = listening.accept().await.unwrap().unwrap();
            let channel = theirs_there.keep(conn);
            (channel.answer().await.unwrap(), channel.answer().await.unwrap())
        });

        let to = by_loopback(&base);
        let first = tokio::time::timeout(WAIT, mine.to(&caller, to.clone())).await.expect("дозвон не уложился в срок").unwrap();
        // Вторая карточка спрашивает канал заново — и обязана получить тот же.
        let second = mine.to(&caller, to).await.unwrap();
        assert_eq!(first.peer(), second.peer());

        let mut one = first.call().await.unwrap();
        let mut two = second.call().await.unwrap();
        // По кадру в каждый: поток, в который не написали, та сторона не
        // увидит вовсе — QUIC заводит его на первом байте.
        Frame::write(&mut one.send, &Msg::EnrollWait).await.unwrap();
        Frame::write(&mut two.send, &Msg::EnrollWait).await.unwrap();
        let (mut their_one, their_two) = tokio::time::timeout(WAIT, answered)
            .await
            .expect("второй звонок завёл ВТОРОЕ соединение: та сторона не дождалась второго потока в первом")
            .unwrap();
        assert_eq!(Frame::read(&mut their_one.recv).await.unwrap(), Msg::EnrollWait);

        drop(one);
        assert!(first.is_open(), "канал закрылся со снятием одного потока — значит второй потоком в нём не ехал");
        assert!(second.is_open(), "и это один и тот же канал, а не два");
        drop(two);
        assert!(!first.is_open(), "а вот теперь живых потоков не осталось");

        drop((their_one, their_two));
        caller.close().await;
        base.close().await;
    }

    /// Приём входящих: каждому управляющему потоку — свой разговор, и второй
    /// не ждёт, пока кончится первый.
    #[tokio::test]
    async fn the_accept_loop_gives_every_control_stream_its_own_talk() {
        let (da, db) = (dir("accept-all-a"), dir("accept-all-b"));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let (mine, theirs) = (Channels::default(), Channels::default());

        let heard: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (listening, there, said) = (base.clone(), theirs.clone(), heard.clone());
        tokio::spawn(async move {
            accept_all(
                &listening,
                &there,
                |why| panic!("входящее подключение обязано было состояться: {why}"),
                move |mut control| {
                    let said = said.clone();
                    async move {
                        if let Ok(msg) = Frame::read(&mut control.recv).await {
                            said.lock().unwrap().push(format!("{msg:?}"));
                        }
                        control.bye(Refuse::new(code::MCP_OFF, SERVING_OFF)).await;
                    }
                },
            )
            .await;
        });

        let channel = tokio::time::timeout(WAIT, mine.to(&caller, by_loopback(&base))).await.expect("дозвон не уложился").unwrap();
        let (mut one, mut two) = (channel.call().await.unwrap(), channel.call().await.unwrap());
        Frame::write(&mut one.send, &Msg::EnrollWait).await.unwrap();
        Frame::write(&mut two.send, &Msg::Refuse(Refuse::new(code::BUSY, "занято"))).await.unwrap();

        // Оба разговора кончаются прощанием — значит оба состоялись.
        for (which, control) in [("первый", &mut one), ("второй", &mut two)] {
            let end = tokio::time::timeout(WAIT, hold(&mut control.recv, |_| {})).await.expect("разговор не кончился в срок");
            assert!(matches!(end, Gone::Farewell(_)), "{which} поток не получил своего разговора: {end:?}");
        }
        let said = heard.lock().unwrap().clone();
        assert_eq!(said.len(), 2, "каждому потоку — свой разговор: {said:?}");

        caller.close().await;
        base.close().await;
    }

    /// Задержки нарастают до минуты и дальше не растут. Отдельным тестом:
    /// ладка — правило спеки, и проверять его прогоном в двадцать минут
    /// никто не станет.
    #[test]
    fn the_delays_grow_to_a_minute_and_stay_there() {
        assert_eq!(backoff(0), Duration::ZERO, "первая попытка идёт сразу: ждать там нечего");
        for (attempt, seconds) in [(1u32, 1u64), (2, 2), (3, 4), (4, 8), (5, 16), (6, 32), (7, 60)] {
            assert_eq!(backoff(attempt).as_secs(), seconds, "попытка {attempt}");
        }
        assert_eq!(backoff(99).as_secs(), 60, "дальше раз в минуту бесконечно, а не разом сдаёмся");
        assert_eq!(backoff(u32::MAX).as_secs(), 60, "и переполнение сдвига не роняет программу через неделю без связи");
    }

    /// Зовущая сторона возвращается после обрыва сама — и БЕЗ нового кода:
    /// второе подключение человека не спрашивает, личность доказывает iroh.
    #[tokio::test]
    async fn the_dialling_side_comes_back_after_a_break() {
        let (da, db) = (dir("comes-back-a"), dir("comes-back-b"));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let peer = caller.id();
        let mut book = Book::open(&db).unwrap();
        let ticket = Ticket::issue(base.id(), None, vec![]).unwrap();
        book.issue("Ромашка", &ticket).unwrap();
        let code = ticket.to_code();
        let cards = vec![Card { name: "Ромашка".into(), base: fields("b1") }];

        // Принимающая сторона: первый круг зачисляет и рвёт канал молча (так
        // рвёт его сеть), второй — узнаёт уже зачисленного и прощается
        // словами, чтобы звонившая перестала звонить и тест кончился.
        let listening = base.clone();
        let theirs = Channels::default();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for round in 0..2 {
                let conn = listening.accept().await.unwrap().unwrap();
                let channel = theirs.keep(conn);
                let mut control = channel.answer().await.unwrap();
                let out = accept_side(&mut control.send, &mut control.recv, peer, &mut book, &cards, |_| async {
                    Decision::Allow { write: true }
                })
                .await;
                seen.push(out);
                if round == 1 {
                    control.bye(Refuse::new(code::MCP_OFF, SERVING_OFF)).await;
                } else {
                    // Круг нулевой — обрыв: ни единого слова той стороне,
                    // поток просто кончается, и `Control` падает здесь,
                    // закрывая последним снятым `Hold` сам канал.
                    //
                    // Перед этим ждём, чтобы уже НАПИСАННОЕ (`enroll_ok`)
                    // признали доехавшим: иначе рвётся не связь, а наш
                    // собственный ответ, и тест проверял бы гонку записи с
                    // закрытием канала, а не возврат зовущей стороны.
                    let _ = control.send.finish();
                    let _ = tokio::time::timeout(WAIT, control.send.stopped()).await;
                }
            }
            seen
        });

        let mine = Channels::default();
        let to = by_loopback(&base);
        let rounds = AtomicUsize::new(0);
        let enrolled = AtomicBool::new(false);
        let stopped = tokio::time::timeout(
            WAIT,
            keep_calling(|| {
                let (mine, caller, to, code) = (&mine, &caller, to.clone(), &code);
                let (rounds, enrolled) = (&rounds, &enrolled);
                async move {
                    rounds.fetch_add(1, Ordering::SeqCst);
                    let channel = match mine.to(caller, to).await {
                        Ok(channel) => channel,
                        Err(why) => return Round::Failed(why),
                    };
                    let mut control = match channel.call().await {
                        Ok(control) => control,
                        Err(why) => return Round::Failed(why),
                    };
                    // Код предъявляется ОДИН раз: после зачисления
                    // предъявлять нечего, и он к тому же погашен.
                    let show = if enrolled.load(Ordering::SeqCst) { "" } else { code.as_str() };
                    match dial_side(&mut control.send, &mut control.recv, show, "Разработчик", Ceiling::Full).await {
                        Ok(Outcome::Enrolled(_)) | Ok(Outcome::Already(_)) => {
                            enrolled.store(true, Ordering::SeqCst);
                            Round::Held(hold(&mut control.recv, |_| {}).await)
                        }
                        Ok(Outcome::NotEnrolled(no)) => Round::NotEnrolled(no),
                        Err(why) => Round::Failed(why),
                    }
                }
            }),
        )
        .await
        .expect("зовущая сторона не уложилась в срок");

        let Stopped::Farewell(why) = &stopped else {
            panic!("второй круг кончился прощанием той стороны: {stopped:?}")
        };
        assert_eq!(why.code, code::MCP_OFF, "и причину ухода звонившая прочитала словами");
        assert_eq!(rounds.load(Ordering::SeqCst), 2, "после обрыва зовущая вернулась сама, её никто не просил");

        let seen = tokio::time::timeout(WAIT, server).await.expect("принимающая сторона не уложилась в срок").unwrap();
        assert!(matches!(seen[0], Ok(Outcome::Enrolled(_))), "первый круг — зачисление: {:?}", seen[0]);
        assert!(matches!(seen[1], Ok(Outcome::Already(_))), "второй круг человека не спрашивает и нового кода не требует: {:?}", seen[1]);

        caller.close().await;
        base.close().await;
    }
}
