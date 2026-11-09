//! Зачисление: разговор, в котором одна машина приходит с кодом, а человек на
//! другой видит диалог и решает.
//!
//! Здесь только случай первый спеки — **база на принимающей стороне**
//! (бухгалтер держит базу, разработчик звонит с кодом). Порядок сообщений
//! ровно оттуда: `hello` → `hello_ok` → `enroll` → `enroll_wait` →
//! `enroll_ok` / `enroll_no`.
//!
//! **Зачем в тикете секрет, раз допуск всё равно даёт человек.** Без секрета
//! любой, кому код переслали дальше, чем предполагалось, может стучаться и
//! дёргать человека диалогом разрешения — а человека, которого дёргают
//! достаточно часто, рано или поздно нажимает «да». Поэтому секрет
//! проверяется МОЛЧА (`Book::check`) и до всякого диалога: не сошлось —
//! отказ словами и закрытие, человека не беспокоят вовсе. Это не вторая
//! аутентификация (сам по себе секрет ничего не открывает), а замок на
//! дверном звонке.
//!
//! **Зачем `enroll_wait`.** Человек думает минуты: он сверяет отпечаток
//! ключа голосом по телефону. Для зовущей стороны эти минуты тишины
//! неотличимы от обрыва, и она ушла бы в цикл переподключений — то есть
//! принесла бы человеку второй диалог поверх первого. Одно пустое сообщение
//! снимает всю эту беду: «я тебя услышал, ответ будет».
//!
//! **Почему отклонение — исход, а не ошибка.** Ошибку полагается повторить:
//! связь подвела, попробуем ещё. Отказ человека повторять нельзя — повтор и
//! есть то самое дёрганье, от которого завели секрет. Поэтому `enroll_no`
//! возвращается как `Outcome::NotEnrolled`, а не как `Err`: зовущая сторона
//! сама не повторяет, и у кода нет ветки, в которой она это сделала бы
//! «заодно с прочими ошибками».
//!
//! **Чья рука прервала — тем и решается судьба кода.** Прервал ЗВОНЯЩИЙ
//! (бросил трубку, заговорил не в свой черёд) — код израсходован: он получил
//! свою одну заявку и распорядился ею сам, а оставь код живым, и он будет
//! показывать человеку диалог и бросать трубку, пока тот не нажмёт «да». Не
//! ответил ЧЕЛОВЕК (закрыл диалог, вышел срок решения) — код жив: человек
//! ничего не решал, и отменять ему код не за что. Правило не про таймер и не
//! про то, кто был первым, а про то, чья рука прервала.
//!
//! **Решение о допуске читает книгу с диска, а не из памяти.** Снимок в
//! памяти годится для показа; «пускать ли» по нему решать нельзя — человек в
//! окне успевает нажать «Отозвать разрешение», пока этот процесс держит
//! книгу открытой, и отзыв обязан действовать немедленно (`Book::enrolled_to`).
//!
//! **Самоназвание той стороны — чужие слова.** Его показывают человеку рядом
//! с отпечатком ключа, значит чужая строка с переводами строк и забоем
//! нарисовала бы в диалоге поддельную строку вплотную к настоящему
//! отпечатку. Чистится один раз и до всего (`as_said`).
//!
//! **Свой срок у разговора.** `Frame::read` нарочно без срока (см. его
//! докстроку): на потоке MCP молчание часами законно. Здесь — нет: разговор
//! зачисления либо идёт, либо его нет, и молчащая сторона не должна вешать
//! задачу навсегда. Сроков два, потому что ждут двух разных вещей —
//! `PATIENCE` ждёт МАШИНУ, `DECISION` ждёт ЧЕЛОВЕКА.
//!
//! Спека: `docs/superpowers/specs/2026-11-01-protocol-design.md`, раздел
//! «Зачисление», случай первый.

use crate::enrolled::Book;
use crate::secrets::hex;
use crate::ticket::Ticket;
use crate::wire::{code, BaseBrief, BaseFields, CardState, Ceiling, Frame, Msg, Refuse, Side};
use iroh::EndpointId;
use std::future::Future;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// Версия протокола, на которой эта сборка говорит. Диапазон пока из одного
/// номера: `proto_min == proto_max == PROTO`. Сторона обязана уметь говорить
/// по каждому номеру, который объявила (спека, «Версии»), — объявлять больше
/// одного раньше, чем второй номер существует, значит соврать.
pub const PROTO: u32 = 1;

/// Сколько ждём МАШИНУ: кадр, который та сторона уже решила отправить.
///
/// Тридцати секунд хватает на медленное рукопожатие через релей и не хватает
/// на то, чтобы человек счёл программу зависшей. Больше здесь брать незачем:
/// на этих шагах человека нет ни на одном конце, и ждать нечего, кроме сети.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// Сколько ждём ЧЕЛОВЕКА после `enroll_wait`.
///
/// Пять минут — это «отошёл к телефону сверить отпечаток», а не «ушёл на
/// обед»: ушедший на обед закроет диалог сам, и колбэк ответит «не решил».
/// Предел нужен не человеку, а задаче: без него незакрытый диалог держал бы
/// поток и запись в книге открытыми бесконечно.
///
/// Принимающая сторона отмеряет ровно `DECISION`, зовущая — `DECISION +
/// PATIENCE`. Разница нарочная: слово всегда остаётся за стороной с базой, и
/// зовущая получает `bye` словами, а не тишину по своему же сроку.
pub const DECISION: Duration = Duration::from_secs(5 * 60);

/// Карточка соединения глазами зачисления: своё локальное имя плюс поля базы,
/// которые по проводу и уедут.
///
/// Имя карточки — локальное и по каналу не ездит (спека, «По каналу никогда
/// не едут»), но тикет выдан именно к нему (`Book::check` его и возвращает),
/// поэтому без него заявку не с чем сопоставить. Полного `cfg::Connection`
/// здесь не берём: зачислению нужны четыре поля, а не вся карточка с
/// паролями.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub base: BaseFields,
}

/// Что показывается человеку в диалоге разрешения (спека, шаг четвёртый):
/// к какой карточке и базе стучатся, ключ той стороны — его короткий
/// отпечаток сверяют голосом по телефону — и как сторона себя назвала.
///
/// `says_name` — её слова, а не проверенный факт, и в окне он помечается
/// именно так. Отпечаток из `peer` считает окно: вид отпечатка — дело
/// показа, а не провода.
#[derive(Debug, Clone)]
pub struct Request {
    pub card: String,
    pub base: BaseFields,
    pub peer: EndpointId,
    pub says_name: String,
}

/// Решение человека.
///
/// `NoAnswer` — не вежливость, а обязательный третий ответ: человек может
/// закрыть диалог, не решив, и это НЕ разрешение и НЕ отказ. Будь ответов
/// два, автор колбэка вынужден был бы соврать одним из них — а оба вранья
/// дорогие: «разрешил» пускает незачисленного, «отказал» сжигает код и
/// ставит карточке исход, которого человек не выбирал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Пускаю. `write` — разрешил ли человек запись; с потолком карточки он
    /// складывается здесь же, и наружу уезжает уже сложенное.
    Allow { write: bool },
    /// Не пускаю. Исход, а не ошибка.
    Deny,
    /// Закрыл диалог, не решив.
    NoAnswer,
}

/// Чем кончился разговор. Один тип на обе стороны: вопрос «зачислены ли мы»
/// у них общий, а расходятся они только в том, кто спрашивал человека.
///
/// `Err` из `accept_side`/`dial_side` остаётся за тем, что разговором не
/// является вовсе: оборванная связь, неразобравшийся кадр, не та версия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Разрешили сейчас: вот поля базы. В случае первом они приехали в
    /// `enroll_ok` — от той стороны, у которой база.
    Enrolled(BaseFields),
    /// Зачисление уже было: вот базы из `hello_ok`, человека не спрашивали.
    /// Личность доказана самим iroh, и ничего сверх этого не предъявляется
    /// (спека, «Второе и дальнейшие подключения человека не спрашивают»).
    Already(Vec<BaseBrief>),
    /// Не зачислены, и это исход: человек отклонил (`code::DECLINED` — сама
    /// не повторяй, код сгорел), человек ушёл, не решив, или звонящий бросил
    /// трубку. У двух последних один код, `code::CLOSED`, а судьба кода
    /// подключения разная — по правилу «чья рука прервала» (шаг пятый
    /// `accept_side`).
    ///
    /// Один код на два случая двусмысленности не создаёт: до зовущей
    /// стороны `CLOSED` доезжает только в одном из них (сообщением `bye`,
    /// когда человек не решил), потому что в другом её поток уже мёртв и
    /// слова ей не доходят вовсе. Для журнала принимающей стороны разницу
    /// говорит текст.
    NotEnrolled(Refuse),
}

/// Принимающая сторона — та, у которой база, и та, где живёт человек.
///
/// `peer` берётся у соединения (`Connection::remote_id`), а не у сообщения:
/// ключ узла — это то, что доказал сам iroh, и соврать о нём в `hello`
/// нельзя. `cards` — карточки этого рабочего места; `decide` — диалог
/// человека (в тестах подставной).
///
/// Поток не закрывается здесь нарочно: на `Enrolled` он остаётся жить под
/// `state`, а на отказе его закрывает тот, кто его открывал и кто знает, что
/// ещё по этому соединению висит.
pub async fn accept_side<W, R, F, Fut>(
    w: &mut W,
    r: &mut R,
    peer: EndpointId,
    book: &mut Book,
    cards: &[Card],
    decide: F,
) -> Result<Outcome, String>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Decision>,
{
    // Шаг второй: та сторона открыла поток и представилась.
    let (proto_min, proto_max) = match next(r, PATIENCE).await.map_err(|e| e.text)? {
        Msg::Hello { proto_min, proto_max, .. } => (proto_min, proto_max),
        other => return refuse_and_fail(w, Refuse::new(code::UNKNOWN_MESSAGE, out_of_turn("hello", &other))).await,
    };
    // Наименьший из двух максимумов (спека, «Версии»). Одна сверка на два
    // случая: и «их минимум выше нашего максимума» (честное расхождение
    // версий), и «их диапазон вывернут наизнанку» (ошибка в чужом коде) дают
    // выбранный номер ниже их минимума.
    let proto = proto_max.min(PROTO);
    if proto < proto_min {
        return refuse_and_fail(w, Refuse::new(code::VERSION, version_gap(proto_min, proto_max))).await;
    }

    // Что этот ключ уже может: список считается ДО заявки, потому что он же и
    // есть ответ на второе подключение.
    //
    // `enrolled_to`, а не `of`: это решение о ДОПУСКЕ, и читать его из
    // снимка в памяти нельзя. Человек в окне успевает нажать «Отозвать
    // разрешение», пока этот процесс держит книгу открытой, и по снимку
    // отозванная сторона вошла бы как зачисленная — то есть отзыв перестал
    // бы действовать немедленно, как того требует спека.
    let mut bases: Vec<BaseBrief> = Vec::new();
    for c in cards {
        if let Some(p) = book.enrolled_to(&c.name, peer)? {
            bases.push(BaseBrief {
                id: c.base.base_id.clone(),
                name: c.base.base_name.clone(),
                write: p.write,
                // «Разрешено», а не «подключено»: на этом шаге доказано
                // только разрешение. «Подключено» объявит сообщение `state`,
                // когда поднимется обслуживание, — его шлёт не зачисление.
                state: CardState::Allowed,
            });
        }
    }
    let already = !bases.is_empty();
    write_msg(w, &Msg::HelloOk { proto, side: Side::WithBase, bases: bases.clone() }).await?;
    if already {
        // Второе и дальнейшие подключения человека не спрашивают: личность
        // доказана самим iroh. Разговор зачисления на этом кончен, и ждать
        // здесь `enroll` нельзя — поток нужен следующим слоям сейчас, а не
        // через `PATIENCE`.
        //
        // Потолок: ключ, УЖЕ зачисленный к одной карточке, не может этим же
        // соединением заявиться на вторую — заявка просто не будет
        // прочитана. Спека такого случая не описывает (у человека одна
        // карточка на партнёра), а когда опишет, различать «пришёл с
        // тикетом» и «пришёл работать» придётся полем в `hello`, то есть
        // новым номером версии.
        return Ok(Outcome::Already(bases));
    }

    // Шаг второй, конец: заявка с секретом тикета.
    let (ticket_secret, says_name) = match next(r, PATIENCE).await.map_err(|e| e.text)? {
        Msg::Enroll { ticket_secret, says_name, .. } => (ticket_secret, says_name),
        other => return refuse_and_fail(w, Refuse::new(code::UNKNOWN_MESSAGE, out_of_turn("enroll", &other))).await,
    };

    // Чужие слова чистятся сразу, одним местом на показ и на запись (см.
    // `as_said`): дальше они идут и в диалог человека, и в книгу.
    let says_name = as_said(&says_name);

    // Шаг третий: проверка МОЛЧА. Человека здесь нет и быть не должно.
    let Some(secret) = decode_secret(&ticket_secret) else {
        return said_and_not_enrolled(w, Refuse::new(code::TICKET_FOREIGN, FOREIGN_TEXT)).await;
    };
    let card_name = match book.check(&secret) {
        Ok(name) => name,
        Err(why) => return said_and_not_enrolled(w, Refuse::new(ticket_code(&why), why)).await,
    };
    let Some(card) = cards.iter().find(|c| c.name == card_name) else {
        return said_and_not_enrolled(w, Refuse::new(code::NO_BASE, NO_CARD_TEXT)).await;
    };

    // Шаг четвёртый: человек. `enroll_wait` уходит ДО диалога — иначе зовущая
    // сторона просидит минуты в тишине и сочтёт её обрывом.
    write_msg(w, &Msg::EnrollWait).await?;
    let request = Request { card: card_name.clone(), base: card.base.clone(), peer, says_name: says_name.clone() };
    let decision = tokio::time::timeout(DECISION, async {
        tokio::select! {
            // Та сторона молчит, пока ждёт ответа (так и делает `dial_side`),
            // поэтому ЛЮБОЙ кадр здесь — это конец разговора, а не его
            // продолжение: либо обрыв, либо та сторона заговорила не в свой
            // черёд. Отменённое чтение недочитанного кадра поэтому не
            // оставляет мусора в потоке — читать его дальше всё равно некому.
            d = decide(request) => Some(d),
            _ = Frame::read(r) => None,
        }
    })
    .await;
    let decision = match decision {
        Ok(Some(d)) => d,
        // Трубку бросил ЗВОНЯЩИЙ — и тикет гасится. Правило здесь не про
        // таймер, а про то, ЧЬЯ РУКА прервала разговор (круг починки 1,
        // MEDIUM 2). Звонящий получил свою одну заявку и распорядился ею
        // сам: оставь код живым — и он бросает трубку сразу после того, как
        // диалог показан, столько раз, сколько захочет, пока человек не
        // нажмёт «да». Ровно от этого секрет и заводился.
        //
        // Говорить в мёртвый поток нечего, поэтому слова никуда не уходят —
        // но исход записывается тот же, что у любого непрошедшего
        // зачисления.
        Ok(None) => {
            let _ = book.spend(&secret);
            return Ok(Outcome::NotEnrolled(Refuse::new(code::CLOSED, HUNG_UP)));
        }
        // Истёк срок решения — прервал не звонящий, а наш собственный
        // таймер, то есть рука человека так и не дошла до кнопки. Это тот же
        // случай, что закрытое окно, и наказывать человека отменой кода не
        // за что.
        Err(_) => Decision::NoAnswer,
    };

    // Шаг пятый.
    match decision {
        Decision::Allow { write } => {
            // Гасим ДО записи ключа: обратный порядок оставил бы живой код у
            // человека на руках после сбоя записи, то есть позволил бы
            // подключиться по нему второй раз. Сгоревший код при сбое — всего
            // лишь просьба выдать новый.
            //
            // Отказ книги здесь — не наш `Err`, а отказ СЛОВАМИ той стороне:
            // молчаливый выход оставил бы зовущую ждать ответа до конца её
            // собственного срока, то есть пять с лишним минут тишины вместо
            // одной строки, которую человек прочитает сразу.
            if let Err(why) = book.spend(&secret) {
                return said_and_not_enrolled(w, Refuse::new(code::TICKET_SPENT, why)).await;
            }
            // Складываем решение человека с потолком карточки: карточке
            // только на чтение «разрешённая запись» была бы ложью, и зовущая
            // сторона увидела бы флажок, которого нет.
            let write = write && card.base.write;
            if let Err(why) = book.enroll(&card_name, peer, &says_name, write) {
                return said_and_not_enrolled(w, Refuse::new(code::NOT_ALLOWED, why)).await;
            }
            let base = BaseFields { write, ..card.base.clone() };
            write_msg(w, &Msg::EnrollOk { base: Some(base.clone()) }).await?;
            Ok(Outcome::Enrolled(base))
        }
        Decision::Deny => {
            // Код гасится и на отказе: «один тикет — одна заявка» (спека,
            // «Зачем в тикете секрет»). Оставь его живым — и отклонённый
            // звонящий дёргает человека тем же кодом, пока тот не нажмёт
            // «да»; ровно от этого секрет и заведён. Сбоя сети здесь нет:
            // человек ответил, решение состоялось.
            let _ = book.spend(&secret);
            let no = Refuse::new(code::DECLINED, DECLINED_TEXT);
            write_msg(w, &Msg::EnrollNo(no.clone())).await?;
            Ok(Outcome::NotEnrolled(no))
        }
        Decision::NoAnswer => {
            // Тикет НЕ гасим: прервал человек, а не звонящий, и он ничего не
            // выбрал — сжигать ему код за закрытое окно не за что.
            let no = Refuse::new(code::CLOSED, NO_ANSWER_TEXT);
            write_msg(w, &Msg::Bye(no.clone())).await?;
            Ok(Outcome::NotEnrolled(no))
        }
    }
}

/// Зовущая сторона — та, что вставила код.
///
/// `code` разбирается ЛЕНИВО, только если зачисления ещё нет: второе
/// подключение ничего не предъявляет, и требовать от него живой код значило
/// бы придумать правило, которого в спеке нет.
pub async fn dial_side<W, R>(w: &mut W, r: &mut R, code: &str, says_name: &str, ceiling: Ceiling) -> Result<Outcome, String>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let hello = Msg::Hello {
        proto_min: PROTO,
        proto_max: PROTO,
        side: Side::WithAgents,
        kit: env!("CARGO_PKG_VERSION").to_string(),
        says_name: says_name.to_string(),
        ceiling,
    };
    write_msg(w, &hello).await?;

    let bases = match next(r, PATIENCE).await.map_err(|e| e.text)? {
        Msg::HelloOk { bases, .. } => bases,
        Msg::Refuse(no) => return Err(no.text),
        other => return Err(out_of_turn("hello_ok", &other)),
    };
    if !bases.is_empty() {
        // Зачисление уже есть — предъявлять нечего, и код мы даже не
        // разбирали: второе подключение человека не спрашивает.
        return Ok(Outcome::Already(bases));
    }

    let ticket = Ticket::from_code(code)?;
    let enroll = Msg::Enroll {
        // Секрет уезжает hex-ом — тем же видом, в каком он лежал в коде
        // тикета. Сам `Msg::Enroll` в журнал не пишется НИКОГДА: его `Debug`
        // напечатал бы секрет целиком.
        ticket_secret: hex::encode(&*ticket.secret),
        says_name: says_name.to_string(),
        // Поля базы едут от той стороны, у которой база. Здесь база на
        // принимающей (случай первый), и нам сказать нечего.
        base: None,
    };
    write_msg(w, &enroll).await?;

    // Первый ответ: либо сразу исход, либо «ждите» — и тогда ждём человека.
    let answered = match next(r, PATIENCE).await.map_err(|e| e.text)? {
        Msg::EnrollWait => next(r, DECISION + PATIENCE).await.map_err(|e| e.text)?,
        first => first,
    };
    match answered {
        Msg::EnrollOk { base: Some(base) } => Ok(Outcome::Enrolled(base)),
        // Поля базы в случае первом обязательны: без них зовущей стороне
        // нечего записать в карточку, и молча подставить своё она не может.
        Msg::EnrollOk { base: None } => Err("Подключение разрешили, но не сказали, к какой базе.\nТа сторона прислала подтверждение без \
                                             полей базы.\nОбновите комплект на той машине и подключитесь заново."
            .to_string()),
        // Исход, а не ошибка: сами не повторяем.
        Msg::EnrollNo(no) => Ok(Outcome::NotEnrolled(no)),
        // Человек ушёл, не решив, — тоже исход, но код ещё жив.
        Msg::Bye(no) => Ok(Outcome::NotEnrolled(no)),
        // Отказ машины: код негодный, версии не сошлись. Это человеку
        // исправлять, а не нам повторять.
        Msg::Refuse(no) => Err(no.text),
        other => Err(out_of_turn("enroll_ok", &other)),
    }
}

/// Читает кадр со СВОИМ сроком.
///
/// `Frame::read` без срока нарочно (см. его докстроку): срок — за
/// вызывающим, потому что у каждого потока он свой. Здесь он есть всюду, и
/// это ровно тот случай, под который правило и писалось: разговор зачисления
/// не бывает «пока тихо, значит думают» — думают только за `enroll_wait`, и
/// у этого ожидания свой срок (`DECISION`).
async fn next<R: AsyncRead + Unpin>(r: &mut R, within: Duration) -> Result<Msg, Refuse> {
    match tokio::time::timeout(within, Frame::read(r)).await {
        Ok(got) => got,
        Err(_) => Err(Refuse::new(code::BROKEN, silent_for(within))),
    }
}

/// Отправка в позиции `?`: слова отказа кадрирования уже написаны в
/// `wire.rs`, здесь остаётся только взять из него текст для человека, а не
/// `Display` — тот подмешивает код внутрь строки, чего раздел «Ошибки»
/// спеки не разрешает.
async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<(), String> {
    Frame::write(w, msg).await.map_err(|e| e.text)
}

/// Сказать словами и провалиться: отказ уходит той стороне, а нам остаётся
/// `Err` — разговора не было вовсе, зачислять некого и исхода нет. Так
/// кончаются только поломки самого разговора: не та версия, сообщение не в
/// свой черёд.
async fn refuse_and_fail<W: AsyncWrite + Unpin>(w: &mut W, no: Refuse) -> Result<Outcome, String> {
    // Отказ отправки проглатываем: сказать о ней всё равно нечем, а нести
    // наружу надо ту беду, из-за которой мы отказали, а не вторичную.
    let _ = Frame::write(w, &Msg::Refuse(no.clone())).await;
    Err(no.text)
}

/// Сказать словами и вернуть ИСХОД: разговор состоялся, зачисления нет.
///
/// Отличается от `refuse_and_fail` тем, чья это беда. Здесь принимающая
/// сторона отработала правильно — негодный код, удалённая карточка, сбой
/// записи, — и журнал этой машины не должен краснеть там, где замок на
/// дверном звонке просто сработал.
async fn said_and_not_enrolled<W: AsyncWrite + Unpin>(w: &mut W, no: Refuse) -> Result<Outcome, String> {
    write_msg(w, &Msg::Refuse(no.clone())).await?;
    Ok(Outcome::NotEnrolled(no))
}

/// Предел самоназвания — восемьдесят знаков.
///
/// Это одна строка в диалоге разрешения, рядом с отпечатком ключа.
/// Восьмидесяти хватает на «ООО „Ромашка“, бухгалтерия» и не хватает на
/// абзац, которым чужая сторона оттеснила бы отпечаток за край окна, —
/// а отпечаток здесь единственное, что человек сверяет голосом.
/// Считаем в ЗНАКАХ, а не в байтах: предел виден человеку, а он видит
/// буквы, и русское имя не должно умещаться вдвое хуже латинского.
const SAYS_NAME_LIMIT: usize = 80;

/// Чужие слова — к показу и к записи.
///
/// `says_name` приходит с провода, и спека велит показывать его с пометкой
/// «сказано той стороной, не проверено». Значит и обращаться с ним надо как
/// с чужой строкой: выбросить то, чем рисуют, и обрезать то, чем оттесняют
/// (круг починки 1, MEDIUM 2-бис).
///
/// Выбрасываются две семьи знаков, и обе — ради одного и того же обмана.
/// Управляющие (перевод строки, возврат каретки, забой) позволяют дописать
/// в диалоге вторую строку — например, поддельное «ПОДТВЕРЖДЕНО» вплотную к
/// настоящему отпечатку. Невидимые разметочные (нулевой ширины, разворот
/// направления письма, метка порядка байтов) позволяют переставить
/// показанное местами, не меняя ни одной видимой буквы. Ни то, ни другое в
/// имени машины не нужно никому, кроме того, кто его подделывает.
///
/// Чистится в ОДНОМ месте и до всего остального — чтобы показанное человеку
/// и записанное в книгу не могли разойтись: иначе окно говорит одно, а файл
/// хранит другое, и спор между ними разрешать нечем.
fn as_said(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control() && !is_invisible_mark(*c)).take(SAYS_NAME_LIMIT).collect::<String>().trim().to_string()
}

/// Невидимые разметочные знаки, которыми переставляют показанное местами, не
/// меняя букв. `char::is_control` их не ловит — они не управляющие, а
/// «формат».
fn is_invisible_mark(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// Разбирает hex секрета в ровно 32 байта. `Option`, а не `Result` со
/// словами: что именно не так с чужим секретом, человеку не рассказывают —
/// отказ тут один на все случаи, и подсказывать подбирающему, насколько он
/// близок, незачем.
fn decode_secret(hexed: &str) -> Option<[u8; 32]> {
    hex::decode(hexed).ok()?.as_slice().try_into().ok()
}

/// Код провода для отказа, который `Book::check` вернул словами.
///
/// Книга отдаёт текст человеку, а не код для журнала, и собирать код обратно
/// из текста — единственное, что у нас есть, пока она не отдаёт оба. Разница
/// между «устарел» и «не найден» в журнале нужна: первое — обычная жизнь,
/// второе — повод присмотреться.
fn ticket_code(words: &str) -> &'static str {
    if words.contains("устарел") {
        code::TICKET_EXPIRED
    } else {
        code::TICKET_SPENT
    }
}

/// «Пришло не то, чего ждали на этом шаге.»
///
/// Имя пришедшего сообщения берётся из `named`, а НЕ из `{other:?}`:
/// `Msg::Enroll` в `Debug` несёт секрет тикета, и одно такое форматирование
/// в тексте отказа или в журнале выдало бы его целиком.
fn out_of_turn(expected: &str, got: &Msg) -> String {
    format!(
        "Разговор о подключении пошёл не по порядку.\nЖдали «{expected}», а пришло «{}».\nОбновите комплект на обеих машинах: скорее всего, \
         они разошлись версиями.",
        named(got)
    )
}

/// Имя сообщения на проводе. Serde имён вариантов наружу не отдаёт, а
/// `Debug` брать нельзя (см. `out_of_turn`), поэтому перебор здесь. Полноту
/// держит сам компилятор: перебор без заглушки, и новое сообщение в `Msg`
/// уронит сборку здесь, а не напечатает «неизвестно» в журнале.
fn named(msg: &Msg) -> &'static str {
    match msg {
        Msg::Hello { .. } => "hello",
        Msg::HelloOk { .. } => "hello_ok",
        Msg::Enroll { .. } => "enroll",
        Msg::EnrollWait => "enroll_wait",
        Msg::EnrollOk { .. } => "enroll_ok",
        Msg::EnrollNo(_) => "enroll_no",
        Msg::State { .. } => "state",
        Msg::Revoked { .. } => "revoked",
        Msg::Refuse(_) => "refuse",
        Msg::Bye(_) => "bye",
    }
}

fn version_gap(their_min: u32, their_max: u32) -> String {
    format!(
        "Версии не сошлись: здесь {PROTO}, там от {their_min} до {their_max}.\nЭта машина не умеет говорить ни по одной версии, которую \
         понимает та.\nОбновите комплект на машине, которая старше."
    )
}

fn silent_for(within: Duration) -> String {
    format!(
        "Та сторона не ответила.\nЖдали {} с и не дождались ни слова.\nПроверьте, запущена ли программа там, и подключитесь заново.",
        within.as_secs()
    )
}

const FOREIGN_TEXT: &str = "Код не от этой базы.\nОн не читается как код, выданный этой программой.\nПопросите новый код у того, кто вас \
                            подключает.";

const NO_CARD_TEXT: &str =
    "База, к которой вели, больше не заведена.\nКарточку, для которой выдали этот код, на той машине удалили.\nПопросите \
                            завести её заново и выдать новый код.";

const HUNG_UP: &str = "Подключение не состоялось: связь прервалась, пока ждали решения.\nЭта сторона закрыла поток раньше, чем человек \
                       ответил, и код на этом израсходован — заявка по нему одна.\nПопросите новый код.";

const DECLINED_TEXT: &str = "Подключение отклонено на стороне базы.\nЧеловек там посмотрел на отпечаток ключа и решил не пускать эту \
                             машину.\nЕсли это недоразумение, позвоните ему и попросите новый код.";

const NO_ANSWER_TEXT: &str = "Подключение не разрешено и не отклонено.\nЧеловек на стороне базы закрыл диалог, не решив.\nКод подключения \
                              ещё действует — позвоните ему и попробуйте снова.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use iroh::EndpointAddr;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Предел ожидания сети в тесте: упавший шаг должен выглядеть как
    /// упавший, а не как зависший прогон.
    const WAIT: Duration = Duration::from_secs(20);

    /// Свой каталог на каждый тест: ключ узла и книга зачислений — файлы, и
    /// тесты не должны отбирать их друг у друга (тот же приём, что в
    /// `node.rs`).
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("onec-enroll-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Адрес узла для звонка внутри процесса — петля и свой порт.
    ///
    /// Довод тот же, что у одноимённого помощника в `node.rs`: iroh кладёт
    /// петлю в адрес узла только когда других интерфейсов нет вовсе, а на
    /// рабочей машине звонок пошёл бы на свой же адрес в локальной сети, то
    /// есть через брандмауэр. Это проверяло бы настройки Windows, а не наш
    /// разговор.
    fn by_loopback(node: &Node) -> EndpointAddr {
        let port = node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find_map(|s| if s.is_ipv4() { Some(s.port()) } else { None })
            .expect("узел обязан был занять порт IPv4");
        EndpointAddr::new(node.id()).with_ip_addr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    fn fields(id: &str, write: bool) -> BaseFields {
        BaseFields {
            base_id: id.to_string(), base_name: format!("База {id}"), publication: format!("http://127.0.0.1:8080/{id}"), write
        }
    }

    /// Счётчик вызовов диалога: «человека не беспокоят вовсе» проверяется
    /// только так — не словами отказа, а нулём здесь.
    #[derive(Clone, Default)]
    struct Asked(Arc<AtomicUsize>);

    impl Asked {
        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Разговор двух узлов в одном процессе: поднять оба, свести по петле,
    /// провести зачисление целиком и вернуть оба исхода.
    ///
    /// Общая обвязка, а не копия в каждом тесте: тестов шесть, и расходятся
    /// они ровно в двух местах — какой код предъявлен и что решит человек.
    async fn talk(
        name: &str,
        prepare: impl FnOnce(&mut Book, EndpointId, &Node, &std::path::Path) -> (String, Vec<Card>),
        answer: Decision,
    ) -> (Result<Outcome, String>, Result<Outcome, String>, Book, Asked, String) {
        let (da, db) = (dir(&format!("{name}-a")), dir(&format!("{name}-b")));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let peer = caller.id();
        let mut book = Book::open(&db).unwrap();
        let (code, cards) = prepare(&mut book, peer, &base, &db);

        let asked = Asked::default();
        let listening = base.clone();
        let (counter, cards_there) = (asked.clone(), cards.clone());
        let server = tokio::spawn(async move {
            let conn = listening.accept().await.unwrap().unwrap();
            let (mut w, mut r) = conn.accept_bi().await.unwrap();
            let out = accept_side(&mut w, &mut r, peer, &mut book, &cards_there, |_req| async move {
                counter.0.fetch_add(1, Ordering::SeqCst);
                answer
            })
            .await;
            // Соединение держим живым: брошенное здесь закрыло бы канал
            // раньше, чем та сторона дочитает ответ.
            (out, book, conn)
        });

        let conn = tokio::time::timeout(WAIT, caller.dial(by_loopback(&base))).await.expect("дозвон не уложился в срок").unwrap();
        let (mut w, mut r) = conn.open_bi().await.unwrap();
        let mine = tokio::time::timeout(WAIT, dial_side(&mut w, &mut r, &code, "Разработчик", Ceiling::Full))
            .await
            .expect("зовущая сторона не уложилась в срок");
        let (theirs, book, _held) = tokio::time::timeout(WAIT, server).await.expect("принимающая сторона не уложилась в срок").unwrap();

        caller.close().await;
        base.close().await;
        // Каталоги НЕ удаляем, и это не забывчивость: книга отдаётся наружу
        // живой, а `Book::check` перечитывает файл с диска — удалив каталог
        // здесь, мы бы получили «код не найден» на любой вопрос о тикете, то
        // есть проверки «погашен ли он» молча проходили бы всегда. Стирает
        // за собой `dir`, на входе следующего прогона.
        (mine, theirs, book, asked, code)
    }

    /// Секрет обратно из кода — им проверяется, погашен тикет или жив.
    fn secret_of(code: &str) -> [u8; 32] {
        *Ticket::from_code(code).unwrap().secret
    }

    /// Живой тикет к карточке «Ромашка» — обычная заготовка: так выглядит
    /// шаг первый спеки, когда человек уже выдал код и переслал его.
    fn one_live_ticket(book: &mut Book, _peer: EndpointId, base: &Node, _state: &std::path::Path) -> (String, Vec<Card>) {
        let ticket = Ticket::issue(base.id(), None, vec![]).unwrap();
        book.issue("Ромашка", &ticket).unwrap();
        (ticket.to_code(), vec![Card { name: "Ромашка".into(), base: fields("b1", true) }])
    }

    #[tokio::test]
    async fn two_nodes_enroll_each_other_end_to_end() {
        let (mine, theirs, book, asked, code) = talk("e2e", one_live_ticket, Decision::Allow { write: true }).await;

        let Ok(Outcome::Enrolled(base)) = &mine else {
            panic!("зовущая обязана была получить enroll_ok с полями базы: {mine:?}")
        };
        assert_eq!(base.base_id, "b1", "поля базы едут от той стороны, у которой база");
        assert!(base.write, "человек разрешил запись, и потолок карточки её позволяет");
        assert!(matches!(theirs, Ok(Outcome::Enrolled(_))), "принимающая обязана была дойти до того же исхода: {theirs:?}");
        assert_eq!(asked.count(), 1, "человека спрашивают ровно один раз");

        let parties = book.of("Ромашка");
        assert_eq!(parties.len(), 1, "ключ обязан был записаться в карточку: {parties:?}");
        assert_eq!(parties[0].says_name, "Разработчик", "как сторона себя назвала — её слова, кладутся как есть");
        assert!(parties[0].write, "решение человека о записи записано вместе с ключом");
        assert!(
            book.check(&secret_of(&code)).is_err(),
            "тикет погашен: после разрешения он не хранится нигде, и второй заявки по нему нет"
        );
    }

    #[tokio::test]
    async fn a_wrong_secret_is_refused_without_bothering_the_person() {
        // Код выдан к карточке, но предъявлен ДРУГОЙ — так выглядит
        // пересланный дальше, чем предполагалось, или просто подделанный.
        let (mine, theirs, book, asked, _code) = talk(
            "wrong",
            |book, _peer, base, _state| {
                let issued = Ticket::issue(base.id(), None, vec![]).unwrap();
                book.issue("Ромашка", &issued).unwrap();
                let other = Ticket::issue(base.id(), None, vec![]).unwrap();
                (other.to_code(), vec![Card { name: "Ромашка".into(), base: fields("b1", true) }])
            },
            Decision::Allow { write: true },
        )
        .await;

        assert_eq!(asked.count(), 0, "человека при негодном коде не беспокоят ВОВСЕ — это и есть замок на дверном звонке");
        assert!(mine.is_err(), "зовущая обязана прочитать отказ словами: {mine:?}");
        assert!(matches!(theirs, Ok(Outcome::NotEnrolled(_))), "принимающая отказала и закрылась: {theirs:?}");
        assert!(book.of("Ромашка").is_empty(), "негодный код не зачисляет никого");
    }

    #[tokio::test]
    async fn an_expired_ticket_is_refused_without_bothering_the_person() {
        let (mine, theirs, _book, asked, _code) = talk(
            "expired",
            |book, _peer, base, state| {
                let ticket = Ticket::issue(base.id(), None, vec![]).unwrap();
                book.issue("Ромашка", &ticket).unwrap();
                // Состарить тикет иначе нечем: срок — константа суток, а
                // `issue` отказывает выдать уже просроченный. Правим срок на
                // диске: `check` перечитывает файл сам, и путь «код устарел»
                // проверяется настоящий, а не притворный.
                expire_tickets_on_disk(state);
                (ticket.to_code(), vec![Card { name: "Ромашка".into(), base: fields("b1", true) }])
            },
            Decision::Allow { write: true },
        )
        .await;

        assert_eq!(asked.count(), 0, "просроченный код до человека не доходит");
        assert!(mine.is_err(), "зовущая обязана прочитать, что код устарел: {mine:?}");
        let Ok(Outcome::NotEnrolled(no)) = &theirs else {
            panic!("принимающая обязана была отказать: {theirs:?}")
        };
        assert!(no.text.contains("устарел"), "отказ обязан назвать причину словами: {}", no.text);
    }

    #[tokio::test]
    async fn a_key_enrolled_to_one_card_is_refused_at_another() {
        // Ключ узла — это «кто стучится», а не «куда пущен»: зачисление у
        // «Ромашки» не пускает ту же машину к «Васильку».
        let (mine, theirs, _book, asked, _code) = talk(
            "separate",
            |book, peer, base, _state| {
                book.enroll("Ромашка", peer, "Разработчик", true).unwrap();
                book.enroll("Василёк", base.id(), "Кто-то ещё", true).unwrap();
                (
                    // Код не понадобится вовсе: зачисление уже есть, и
                    // второе подключение ничего не предъявляет.
                    String::new(),
                    vec![
                        Card { name: "Ромашка".into(), base: fields("b1", true) },
                        Card { name: "Василёк".into(), base: fields("b2", true) },
                    ],
                )
            },
            Decision::Allow { write: true },
        )
        .await;

        assert_eq!(asked.count(), 0, "второе подключение человека не спрашивает");
        let Ok(Outcome::Already(bases)) = &mine else {
            panic!("зовущая обязана была получить список зачисленных баз: {mine:?}")
        };
        assert_eq!(bases.len(), 1, "в списке ровно та база, к которой этот ключ зачислен: {bases:?}");
        assert_eq!(bases[0].id, "b1", "чужое зачисление «Василька» в наш список не попадает");
        assert!(matches!(theirs, Ok(Outcome::Already(_))), "принимающая обязана была дойти до того же: {theirs:?}");
    }

    #[tokio::test]
    async fn a_refusal_by_the_person_is_an_outcome_not_an_error() {
        let (mine, theirs, book, asked, code) = talk("deny", one_live_ticket, Decision::Deny).await;

        assert_eq!(asked.count(), 1, "отказ — это решение человека, значит его спросили");
        let Ok(Outcome::NotEnrolled(no)) = &mine else { panic!("отклонение — исход, а не Err: {mine:?}") };
        assert_eq!(no.code, code::DECLINED, "код отказа — «отклонено», по нему зовущая и понимает, что повторять нечего");
        assert!(matches!(theirs, Ok(Outcome::NotEnrolled(_))), "принимающая дошла до того же исхода: {theirs:?}");
        assert!(book.of("Ромашка").is_empty(), "отклонённый ключ в карточку не пишется");
        assert!(
            book.check(&secret_of(&code)).is_err(),
            "решение состоялось, значит код погашен: иначе отклонённый звонящий дёргает человека тем же кодом, пока тот не нажмёт «да»"
        );
    }

    #[tokio::test]
    async fn a_person_who_does_not_decide_leaves_the_ticket_unspent() {
        let (mine, theirs, book, asked, code) = talk("silent", one_live_ticket, Decision::NoAnswer).await;

        assert_eq!(asked.count(), 1, "диалог показали, человек его закрыл, не решив");
        let Ok(Outcome::NotEnrolled(no)) = &mine else { panic!("«не решил» — тоже исход, а не Err: {mine:?}") };
        assert_ne!(no.code, code::DECLINED, "«не решил» — это НЕ отказ: иначе зовущая перестанет пробовать, а человек ничего не выбирал");
        assert!(matches!(theirs, Ok(Outcome::NotEnrolled(_))), "принимающая дошла до того же: {theirs:?}");
        assert!(book.of("Ромашка").is_empty(), "без решения человека ключ не зачисляется");
        assert!(book.check(&secret_of(&code)).is_ok(), "человек ничего не выбрал — сжигать ему код за закрытое окно незачем");
    }

    /// Трубку бросил ЗВОНЯЩИЙ, пока человек думал, — и код на этом
    /// израсходован. Иначе он бросает её сразу после показа диалога столько
    /// раз, сколько захочет, пока человек не нажмёт «да»: ровно то дёрганье,
    /// от которого заведён секрет.
    ///
    /// Парный к нему тест — `a_person_who_does_not_decide_leaves_the_ticket_unspent`:
    /// там прервал человек, и код остаётся жив. Правило не про таймер, а про
    /// то, чья рука прервала.
    #[tokio::test]
    async fn a_caller_who_hangs_up_has_spent_its_one_request() {
        let (da, db) = (dir("break-a"), dir("break-b"));
        let caller = Node::start(&da).await.unwrap();
        let base = Node::start(&db).await.unwrap();
        let peer = caller.id();
        let mut book = Book::open(&db).unwrap();
        let ticket = Ticket::issue(base.id(), None, vec![]).unwrap();
        book.issue("Ромашка", &ticket).unwrap();
        let cards = vec![Card { name: "Ромашка".into(), base: fields("b1", true) }];

        let listening = base.clone();
        let server = tokio::spawn(async move {
            let conn = listening.accept().await.unwrap().unwrap();
            let (mut w, mut r) = conn.accept_bi().await.unwrap();
            // Человек не отвечает никогда: диалог открыт, а тот конец
            // исчезает. Ответить вместо него должен сам разговор.
            let out = accept_side(&mut w, &mut r, peer, &mut book, &cards, |_req| std::future::pending()).await;
            (out, book)
        });

        let conn = tokio::time::timeout(WAIT, caller.dial(by_loopback(&base))).await.expect("дозвон не уложился в срок").unwrap();
        let (mut w, mut r) = conn.open_bi().await.unwrap();
        // Доводим разговор до `enroll_wait` — и бросаем трубку.
        Frame::write(&mut w, &hello_from("Разработчик")).await.unwrap();
        assert!(matches!(Frame::read(&mut r).await, Ok(Msg::HelloOk { .. })), "принимающая обязана была ответить hello_ok");
        let enroll = Msg::Enroll { ticket_secret: hex::encode(&*ticket.secret), says_name: "Разработчик".into(), base: None };
        Frame::write(&mut w, &enroll).await.unwrap();
        assert!(matches!(Frame::read(&mut r).await, Ok(Msg::EnrollWait)), "пока человек думает, обязан прийти enroll_wait");
        drop(r);
        drop(w);
        conn.close(0u32.into(), b"");
        drop(conn);

        let (theirs, book) = tokio::time::timeout(WAIT, server).await.expect("принимающая сторона не заметила обрыва в срок").unwrap();
        assert!(matches!(theirs, Ok(Outcome::NotEnrolled(_)) | Err(_)), "обрыв посреди ожидания — не зачисление: {theirs:?}");
        assert!(
            book.check(&ticket.secret).is_err(),
            "трубку бросил звонящий — код израсходован: иначе он показывает человеку диалог и бросает трубку, пока тот не нажмёт «да»"
        );
        assert!(book.of("Ромашка").is_empty(), "никто не зачислен");

        caller.close().await;
        base.close().await;
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
    }

    /// Отзыв между первым и вторым подключением закрывает дверь.
    ///
    /// Книга принимающей стороны открыта и держит снимок, в котором ключ
    /// зачислен; человек в окне (другой `Book` на тот же файл) успевает
    /// нажать «Отозвать разрешение». Доказательство того, что допуск
    /// перечитан с диска, — не слова отказа, а то, что человека СПРОСИЛИ:
    /// по устаревшему снимку разговор кончился бы `Already` и диалога не
    /// случилось бы вовсе.
    #[tokio::test]
    async fn a_revocation_between_two_connections_closes_the_door() {
        let (mine, theirs, book, asked, _code) = talk(
            "revoked",
            |book, peer, base, state| {
                book.enroll("Ромашка", peer, "Разработчик", true).unwrap();
                let ticket = Ticket::issue(base.id(), None, vec![]).unwrap();
                book.issue("Ромашка", &ticket).unwrap();
                // Окно в соседнем процессе: своя книга на тот же файл.
                let mut window = Book::open(state).unwrap();
                window.revoke("Ромашка", peer).unwrap();
                assert!(
                    book.of("Ромашка").iter().any(|p| p.key == peer),
                    "снимок в памяти обязан ещё помнить зачисление — иначе тест не про устаревший снимок"
                );
                (ticket.to_code(), vec![Card { name: "Ромашка".into(), base: fields("b1", true) }])
            },
            Decision::Deny,
        )
        .await;

        assert_eq!(asked.count(), 1, "отозванная сторона проходит заново через человека, а не через устаревший снимок книги");
        assert!(!matches!(mine, Ok(Outcome::Already(_))), "отозванный ключ не получает списка зачисленных баз: {mine:?}");
        assert!(matches!(theirs, Ok(Outcome::NotEnrolled(_))), "человек отказал, значит исход — не зачислены: {theirs:?}");
        assert!(book.of("Ромашка").is_empty(), "отзыв остался в силе");
    }

    /// Самоназвание — чужие слова: его показывают человеку рядом с
    /// отпечатком и его же хранят. Длинное и с управляющими знаками обязано
    /// доехать до диалога уже безопасным.
    ///
    /// Без сети и без второго узла: проверяется разбор чужой строки, а не
    /// провод, — две половины сведены `tokio::io::duplex`, и тест идёт
    /// мгновенно.
    #[tokio::test]
    async fn a_self_given_name_is_cut_to_size_and_stripped_of_what_draws() {
        let d = dir("says-name");
        let node = Node::start(&d).await.unwrap();
        let peer = node.id();
        let mut book = Book::open(&d).unwrap();
        let ticket = Ticket::issue(peer, None, vec![]).unwrap();
        book.issue("Ромашка", &ticket).unwrap();
        let cards = vec![Card { name: "Ромашка".into(), base: fields("b1", true) }];

        // Чужая строка, которой рисуют: перевод строки и забой дописывают в
        // диалоге поддельную строку вплотную к настоящему отпечатку,
        // разворот направления письма переставляет показанное местами, а
        // абзац вместо имени оттесняет отпечаток за край окна.
        let nasty = format!("  Ромашка\n\r\u{8}ОТПЕЧАТОК СОВПАЛ\u{202e}\u{200b} {}", "я".repeat(300));

        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (mut our_r, mut our_w) = tokio::io::split(ours);
        let (mut their_r, mut their_w) = tokio::io::split(theirs);

        let shown = Arc::new(std::sync::Mutex::new(None));
        let slot = shown.clone();
        let server = tokio::spawn(async move {
            let out = accept_side(&mut our_w, &mut our_r, peer, &mut book, &cards, |req| async move {
                *slot.lock().unwrap() = Some(req.says_name.clone());
                Decision::Allow { write: true }
            })
            .await;
            (out, book)
        });

        Frame::write(&mut their_w, &hello_from("Разработчик")).await.unwrap();
        assert!(matches!(Frame::read(&mut their_r).await, Ok(Msg::HelloOk { .. })), "обязан прийти hello_ok");
        let enroll = Msg::Enroll { ticket_secret: hex::encode(&*ticket.secret), says_name: nasty, base: None };
        Frame::write(&mut their_w, &enroll).await.unwrap();
        assert!(matches!(Frame::read(&mut their_r).await, Ok(Msg::EnrollWait)), "обязан прийти enroll_wait");
        assert!(matches!(Frame::read(&mut their_r).await, Ok(Msg::EnrollOk { .. })), "человек разрешил, обязан прийти enroll_ok");

        let (out, book) = tokio::time::timeout(WAIT, server).await.expect("принимающая сторона не уложилась в срок").unwrap();
        assert!(matches!(out, Ok(Outcome::Enrolled(_))), "разговор обязан был дойти до конца: {out:?}");

        let shown = shown.lock().unwrap().clone().expect("диалог человека обязан был получить заявку");
        assert!(!shown.chars().any(|c| c.is_control()), "управляющих знаков в показанном имени быть не должно: {shown:?}");
        assert!(!shown.contains('\u{202e}') && !shown.contains('\u{200b}'), "невидимой разметки тоже: {shown:?}");
        assert!(
            shown.chars().count() <= SAYS_NAME_LIMIT,
            "имя обязано уместиться в предел, а не оттеснить отпечаток: {}",
            shown.chars().count()
        );
        assert!(shown.starts_with("Ромашка"), "видимые буквы при этом сохраняются, а пробелы по краям срезаются: {shown:?}");
        assert_eq!(
            book.of("Ромашка")[0].says_name,
            shown,
            "храним ровно то, что показали: иначе окно говорит одно, файл другое, и спор между ними разрешать нечем"
        );

        node.close().await;
    }

    /// `hello` зовущей стороны, собранный руками: тесты, которые ведут
    /// разговор сами, а не через `dial_side`.
    fn hello_from(says_name: &str) -> Msg {
        Msg::Hello {
            proto_min: PROTO,
            proto_max: PROTO,
            side: Side::WithAgents,
            kit: "тест".into(),
            says_name: says_name.to_string(),
            ceiling: Ceiling::Full,
        }
    }

    /// Состарить все выданные тикеты прямо в файле книги.
    ///
    /// `Book::open` чистит просрочку при чтении, а `issue` отказывается
    /// выдавать уже просроченный — значит просроченную запись в книге иначе
    /// не получить. `check` перечитывает файл сам, поэтому правка на диске
    /// доходит до него без переоткрытия книги.
    fn expire_tickets_on_disk(state: &std::path::Path) {
        let path = state.join("enrolled.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut out = String::new();
        for line in text.lines() {
            if line.trim_start().starts_with("until") {
                out.push_str("until = 1\n");
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        std::fs::write(&path, out).unwrap();
    }
}
