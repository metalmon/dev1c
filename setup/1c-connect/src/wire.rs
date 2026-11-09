//! Кадрирование и управляющие сообщения канала.
//!
//! Одно правило на все потоки канала: **заголовок кадрирован, тело — нет**.
//! Кадр — длина `u32` старшим байтом вперёд, затем JSON. Варинт сэкономил бы
//! три байта на сообщение, которых за минуту десятки, то есть ничего, а
//! четыре байта фиксированной длины читаются глазами в шестнадцатеричном
//! дампе журнала разбора беды.
//!
//! **Почему не перевод строки**, которым режет сегодняшний `jsonrpc.rs`: там
//! он не от хорошей жизни (`LINE_LIMIT` 8 МБ, выше него кадрирование
//! ломается и строка уходит сырой), а поиск `\n` по мегабайтам — это проход по
//! всем байтам ради числа, которое отправитель и так знает.
//!
//! **Почему JSON, а не бинарный формат** (`postcard`, CBOR): `serde_json` уже
//! прямая зависимость проекта и он же язык самого MCP, а объёмное — файлы и
//! отчёты — по этому протоколу едет СЫРЫМИ байтами и через сериализатор не
//! проходит вовсе. Экономить сериализацией нечего, а платить новой
//! зависимостью и нечитаемым журналом есть чем.
//!
//! Спека: `docs/superpowers/specs/2026-11-01-protocol-design.md`, разделы
//! «Кадрирование и кодировка», «Сообщения», «Ошибки».

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Предел управляющего сообщения — 1 МБ (решение протокола №8). Больше в
/// управлении не бывает ничего, а что бывает — едет потоком обмена.
///
/// Предел проверяется ДО чтения тела. Это не придирка к порядку строк: иначе
/// чужой цикл, сорвавшийся по ошибке, съедает нашу память молча — мы честно
/// выделяем буфер на обещанную длину прежде, чем усомнимся в ней.
pub const LIMIT: usize = 1 << 20;

/// Коды отказов — поле провода и журнала, не текст для человека (спека,
/// «Ошибки»: «по-русски, без кода внутри текста»). По-русски код назван в
/// докстроке, на проводе он латиницей, как и имена сообщений.
pub mod code {
    /// `версия` — диапазоны версий не пересеклись.
    pub const VERSION: &str = "version";
    /// `тикет-просрочен` — срок кода вышел.
    pub const TICKET_EXPIRED: &str = "ticket_expired";
    /// `тикет-потрачен` — по этому коду уже подключались.
    pub const TICKET_SPENT: &str = "ticket_spent";
    /// `тикет-чужой` — секрет не от нашей карточки.
    pub const TICKET_FOREIGN: &str = "ticket_foreign";
    /// `не-разрешено` — ключ не зачислен к этой базе.
    pub const NOT_ALLOWED: &str = "not_allowed";
    /// `отклонено` — человек отказал. Исход, не поломка.
    pub const DECLINED: &str = "declined";
    /// `отозвано` — разрешение снято.
    pub const REVOKED: &str = "revoked";
    /// `базы-нет` — карточки с таким `base_id` нет.
    pub const NO_BASE: &str = "no_base";
    /// `mcp-выключены` — переключатель окна выключен.
    pub const MCP_OFF: &str = "mcp_off";
    /// `замок` — обслуживание не идёт не по воле человека.
    pub const LOCK: &str = "lock";
    /// `только-чтение` — пишущий инструмент при потолке.
    pub const READ_ONLY: &str = "read_only";
    /// `база-сменилась` — первый вызов клиента MCP после переключения текущей.
    pub const BASE_CHANGED: &str = "base_changed";
    /// `занято` — ограничение числа сеансов базы. В окне жёлтое, не красное.
    pub const BUSY: &str = "busy";
    /// `папки-нет` — путь синхронизации не существует или нет прав.
    pub const NO_FOLDER: &str = "no_folder";
    /// `сумма-не-сошлась` — файл доехал битым.
    pub const BAD_SUM: &str = "bad_sum";
    /// `перебор` — слишком длинное сообщение или слишком много потоков.
    pub const OVERFLOW: &str = "overflow";
    /// `незнакомое-сообщение` — тип не из этой версии. Почти всегда значит
    /// `версия`; отдельный код нужен, чтобы отличить честное расхождение
    /// версий от ошибки в своём коде.
    pub const UNKNOWN_MESSAGE: &str = "unknown_message";

    // Ниже — два кода, которых в таблице спеки нет: она описывает отказы,
    // которые ЕДУТ той стороне, а эти два рождаются при чтении кадра, когда
    // отвечать уже некому или нечем. Назвать их нужно всё равно — иначе в
    // журнале обрыв связи и ошибка в чужом коде выглядят одинаково.

    /// `обрыв` — поток кончился посреди кадра.
    pub const BROKEN: &str = "broken";
    /// `закрыто` — поток кончился ровно на границе кадра: та сторона закрыла
    /// его нарочно. Это конец разговора, а не поломка.
    pub const CLOSED: &str = "closed";
    /// `разбор` — тело кадра не разобралось: не JSON или поля не те.
    pub const MALFORMED: &str = "malformed";
}

/// Отказ: код для провода и журнала, слова для человека.
///
/// Один тип на три места — `Msg::Refuse`, `Msg::EnrollNo`, `Msg::Bye`: у всех
/// трёх ровно эти два поля, и расписывать их трижды значило бы разрешить им
/// разойтись. Он же возвращается из `Frame::read`, когда кадр не прочитался.
///
/// Правило текста (спека, «Ошибки»): что не вышло — одной строкой, почему —
/// второй, что сделать — третьей, и третья не бывает пустой. Код внутрь
/// текста не кладётся.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refuse {
    pub code: String,
    pub text: String,
}

impl Refuse {
    pub fn new(code: &str, text: impl Into<String>) -> Self {
        Self { code: code.to_string(), text: text.into() }
    }
}

impl std::fmt::Display for Refuse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]", self.text, self.code)
    }
}

/// Какой конец соединения говорит: с базой или с агентами (контракт, «Единица»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    WithBase,
    WithAgents,
}

/// Потолок записи — свойство МАШИНЫ, не карточки (контракт, «Чего в
/// соединении НЕТ»). Едет в `hello` и повторяется в `state`, когда человек
/// переключил его при живом соединении.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ceiling {
    ReadOnly,
    Full,
}

/// Состояние карточки — восемь слов контракта («Что у соединения общего»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// «нет» — соединения ещё не заводили.
    Absent,
    /// «тикет выдан».
    TicketIssued,
    /// «ждёт разрешения».
    Waiting,
    /// «разрешено».
    Allowed,
    /// «подключено».
    Connected,
    /// «разорвано».
    Broken,
    /// «подключается снова».
    Reconnecting,
    /// «отклонено» — исход, а не ошибка.
    Declined,
}

/// База в списке `hello_ok`: к чему этот ключ уже зачислен.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseBrief {
    pub id: String,
    pub name: String,
    pub write: bool,
    pub state: CardState,
}

/// Поля базы: едут от той стороны, у которой база, в том сообщении, которое
/// она и так шлёт (решение владельца 1.11, закрытый вопрос №4) — в `enroll_ok`,
/// когда база на принимающей, и в самом `enroll`, когда база на зовущей.
/// Третьего сообщения ради этого не заводится.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseFields {
    pub base_id: String,
    pub base_name: String,
    pub publication: String,
    pub write: bool,
}

/// Управляющие сообщения. Имена на проводе латиницей, как в таблице спеки.
///
/// Тег внутри объекта (`{"msg":"hello",…}`), а не снаружи: так кадр читается
/// в журнале как одна плоская запись, и так приёмник МОЛЧА ИГНОРИРУЕТ
/// незнакомое поле — внутренний тег у serde этого и требует. Правило спеки:
/// лишнее поле — добавка внутри договорённой версии, падать из-за неё незачем;
/// незнакомое СООБЩЕНИЕ — это действие, и проглотить его молча значит соврать
/// отправителю, что оно исполнено.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Msg {
    /// Первое, что уходит по управляющему потоку. `says_name` — как сторона
    /// себя назвала; человеку показывается с пометкой «сказано той стороной»
    /// и больше ни на что не влияет.
    Hello { proto_min: u32, proto_max: u32, side: Side, kit: String, says_name: String, ceiling: Ceiling },
    /// Версия выбрана, вот базы, к которым этот ключ уже зачислен. Пустой
    /// список — законный ответ: пришли с тикетом, зачисления ещё нет.
    HelloOk { proto: u32, side: Side, bases: Vec<BaseBrief> },
    /// Пришёл с тикетом, прошу зачислить.
    Enroll {
        ticket_secret: String,
        says_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<BaseFields>,
    },
    /// Человек на стороне базы думает. Нужно, чтобы зовущая не сочла молчание
    /// обрывом и не ушла в цикл переподключений.
    EnrollWait,
    /// Ключ записан в карточку.
    EnrollOk {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<BaseFields>,
    },
    /// Человек отказал. Это исход, а не ошибка; зовущая сама не повторяет.
    EnrollNo(Refuse),
    /// Что-то изменилось на стороне базы. Идёт сама, без спроса.
    State {
        base_id: String,
        state: CardState,
        write: bool,
        serving: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ceiling: Option<Ceiling>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Разрешение снято. Рвёт потоки ЭТОЙ базы сразу; если других зачисленных
    /// баз в канале не осталось — закрывается и канал.
    Revoked { base_id: String, text: String },
    /// Общий отказ управляющего потока: версия, тикет, незнакомое сообщение.
    Refuse(Refuse),
    /// Ухожу нарочно: выключили обслуживание, закрыли окно, снимается машина.
    /// Отличает намеренный уход от обрыва — человек на той стороне читает
    /// причину, а не «связь пропала».
    Bye(Refuse),
}

/// Кадр: длина `u32` старшим байтом вперёд, затем JSON.
///
/// Состояния у кадрирования нет — отсюда и пустой тип: длина в заголовке, и
/// недочитанного буфера между вызовами не остаётся.
pub struct Frame;

impl Frame {
    /// Пишет сообщение одним кадром.
    ///
    /// Свой перебор — тоже отказ, и до провода не доходит ни байта: приёмник
    /// на той стороне закрыл бы канал, и человек увидел бы обрыв вместо нашей
    /// ошибки.
    pub async fn write<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<(), Refuse> {
        let body = serde_json::to_vec(msg).map_err(|e| Refuse::new(code::MALFORMED, format!("сообщение не собралось в JSON: {e}")))?;
        if body.len() > LIMIT {
            return Err(Refuse::new(
                code::OVERFLOW,
                format!(
                    "Сообщение не отправлено: оно длиннее предела.\nПредел управляющего сообщения — 1 МБ, а здесь {} байт.\nЭто наша \
                     ошибка, а не ваша: сообщите её вместе с журналом.",
                    body.len()
                ),
            ));
        }
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        // Одной записью, а не заголовок отдельно от тела: половина кадра в
        // проводе — это то, что приёмник не отличит от обрыва.
        w.write_all(&frame).await.map_err(|e| Refuse::new(code::BROKEN, format!("кадр не ушёл: {e}")))?;
        w.flush().await.map_err(|e| Refuse::new(code::BROKEN, format!("кадр не ушёл: {e}")))
    }

    /// Читает один кадр.
    ///
    /// Порядок шагов здесь и есть защита: четыре байта заголовка, сверка с
    /// пределом, и только потом буфер на тело. Буфер по обещанной длине до
    /// сверки — это чужая ошибка, съедающая нашу память молча.
    pub async fn read<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg, Refuse> {
        let mut header = [0u8; 4];
        match r.read_exact(&mut header).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Ноль байт на границе кадра — честный конец разговора;
                // один-три байта — обрыв посреди заголовка. Разница в том,
                // что человеку сказать, поэтому коды разные.
                return Err(if header == [0; 4] {
                    Refuse::new(
                        code::CLOSED,
                        "Та сторона закрыла поток.\nРазговор окончен.\nЕсли это вышло неожиданно, подключитесь заново.",
                    )
                } else {
                    Refuse::new(
                        code::BROKEN,
                        "Связь прервалась на середине сообщения.\nЗаголовок кадра доехал не целиком.\nПодключитесь заново.",
                    )
                });
            }
            Err(e) => return Err(Refuse::new(code::BROKEN, format!("заголовок кадра не прочитался: {e}"))),
        }
        let len = u32::from_be_bytes(header) as usize;
        if len > LIMIT {
            return Err(Refuse::new(
                code::OVERFLOW,
                format!(
                    "Сообщение с той стороны слишком длинное.\nПредел управляющего сообщения — 1 МБ, а заголовок обещает {len} байт.\nЕсли это \
                     повторяется, обновите комплект на обеих машинах и сообщите нам."
                ),
            ));
        }
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).await.map_err(|_| {
            Refuse::new(code::BROKEN, "Связь прервалась на середине сообщения.\nТело кадра доехало не целиком.\nПодключитесь заново.")
        })?;
        serde_json::from_slice(&body).map_err(|e| unknown_or_malformed(&body, e))
    }
}

/// Отличает незнакомое сообщение от неразобравшегося тела.
///
/// Разница важна человеку: незнакомое сообщение почти всегда значит
/// расхождение версий, и тогда совет — обновить комплект; неразобравшееся
/// тело — это ошибка в коде, и советовать человеку обновляться было бы ложью.
/// Второй разбор (в `Value`) стоит того: он случается только на отказе.
fn unknown_or_malformed(body: &[u8], e: serde_json::Error) -> Refuse {
    let named =
        serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v.get("msg").and_then(|m| m.as_str()).map(|s| s.to_string()));
    match named {
        Some(name) if !is_known(&name) => Refuse::new(
            code::UNKNOWN_MESSAGE,
            format!(
                "Та сторона прислала сообщение «{name}», которого эта версия не знает.\nСкорее всего, комплекты разошлись версиями.\nОбновите \
                 комплект на машине, которая старше."
            ),
        ),
        _ => Refuse::new(code::MALFORMED, format!("Сообщение не разобралось.\nРазбор споткнулся: {e}\nСообщите это вместе с журналом.")),
    }
}

/// Имена сообщений этой версии. Список здесь, а не выведен из `Msg`, потому
/// что вывести его из перечисления нечем — зато он короткий и ломается
/// тестом, если разойдётся с `Msg`.
fn is_known(name: &str) -> bool {
    matches!(name, "hello" | "hello_ok" | "enroll" | "enroll_wait" | "enroll_ok" | "enroll_no" | "state" | "revoked" | "refuse" | "bye")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Все сообщения таблицы спеки, по одному на вид, с заполненными полями —
    /// пустое поле не доказывает, что оно доедет.
    fn every_message() -> Vec<Msg> {
        vec![
            Msg::Hello {
                proto_min: 1,
                proto_max: 3,
                side: Side::WithBase,
                kit: "0.1.0".into(),
                says_name: "Ромашка".into(),
                ceiling: Ceiling::Full,
            },
            Msg::HelloOk {
                proto: 3,
                side: Side::WithAgents,
                bases: vec![BaseBrief { id: "b1".into(), name: "Бухгалтерия".into(), write: true, state: CardState::Allowed }],
            },
            Msg::HelloOk { proto: 1, side: Side::WithBase, bases: vec![] },
            Msg::Enroll { ticket_secret: "s3cr3t".into(), says_name: "Ромашка".into(), base: None },
            Msg::Enroll {
                ticket_secret: "s3cr3t".into(),
                says_name: "Ромашка".into(),
                base: Some(BaseFields {
                    base_id: "b1".into(),
                    base_name: "Бухгалтерия".into(),
                    publication: "http://127.0.0.1:8080/buh".into(),
                    write: false,
                }),
            },
            Msg::EnrollWait,
            Msg::EnrollOk { base: None },
            Msg::EnrollOk {
                base: Some(BaseFields {
                    base_id: "b1".into(),
                    base_name: "Бухгалтерия".into(),
                    publication: "коннектор".into(),
                    write: true,
                }),
            },
            Msg::EnrollNo(Refuse::new(code::DECLINED, "Подключение отклонено на стороне базы.")),
            Msg::State {
                base_id: "b1".into(),
                state: CardState::Connected,
                write: true,
                serving: true,
                ceiling: Some(Ceiling::ReadOnly),
                text: Some("потолок переключили".into()),
            },
            Msg::State { base_id: "b1".into(), state: CardState::Broken, write: false, serving: false, ceiling: None, text: None },
            Msg::Revoked { base_id: "b1".into(), text: "Доступ отозван на стороне базы.".into() },
            Msg::Refuse(Refuse::new(code::VERSION, "Версии не сошлись: здесь 3, там 1.")),
            Msg::Bye(Refuse::new(code::MCP_OFF, "Обслуживание выключили в окне.")),
        ]
    }

    #[tokio::test]
    async fn a_frame_survives_the_round_trip() {
        for msg in every_message() {
            let mut wire: Vec<u8> = Vec::new();
            Frame::write(&mut wire, &msg).await.unwrap();
            let mut back = Cursor::new(wire);
            let read = Frame::read(&mut back).await.unwrap();
            assert_eq!(read, msg, "сообщение не доехало тем же");
            assert_eq!(back.position() as usize, back.get_ref().len(), "кадр прочитан не целиком: следующий начнётся не с начала");
        }
    }

    #[tokio::test]
    async fn a_frame_longer_than_the_limit_is_refused_before_its_body_is_read() {
        // В потоке ТОЛЬКО заголовок, тела нет вовсе. Если бы предел
        // проверялся после чтения тела, отказ был бы про обрыв; «перебор»
        // здесь и доказывает, что до тела дело не дошло.
        let header = ((LIMIT + 1) as u32).to_be_bytes();
        let mut wire = Cursor::new(header.to_vec());
        let no = Frame::read(&mut wire).await.unwrap_err();
        assert_eq!(no.code, code::OVERFLOW, "ждали отказ «перебор», получили: {no:?}");
        assert!(no.text.contains("1 МБ"), "в отказе обязан быть назван предел словами: {}", no.text);
    }

    #[tokio::test]
    async fn an_unknown_field_is_ignored_but_an_unknown_message_is_refused() {
        let good = br#"{"msg":"enroll_wait","what_is_this":{"deep":[1,2,3]}}"#;
        assert_eq!(Frame::read(&mut Cursor::new(framed(good))).await.unwrap(), Msg::EnrollWait);

        let strange = br#"{"msg":"dance","base_id":"b1"}"#;
        let no = Frame::read(&mut Cursor::new(framed(strange))).await.unwrap_err();
        assert_eq!(no.code, code::UNKNOWN_MESSAGE, "незнакомое сообщение — отказ словами, а не тишина");
        assert!(no.text.contains("dance"), "отказ обязан назвать, чего не понял: {}", no.text);
    }

    #[tokio::test]
    async fn a_truncated_frame_is_refused_not_hung() {
        // Обрыв в середине заголовка.
        let no = Frame::read(&mut Cursor::new(vec![0u8, 0, 4])).await.unwrap_err();
        assert_eq!(no.code, code::BROKEN);

        // Обрыв в середине тела: заголовок обещает 32 байта, пришло 5.
        let mut wire = 32u32.to_be_bytes().to_vec();
        wire.extend_from_slice(b"{\"msg\"");
        let no = Frame::read(&mut Cursor::new(wire)).await.unwrap_err();
        assert_eq!(no.code, code::BROKEN);

        // Пустой поток — это не обрыв, а честный конец: та сторона закрыла
        // поток, и звать это поломкой значит пугать человека на ровном месте.
        let no = Frame::read(&mut Cursor::new(Vec::new())).await.unwrap_err();
        assert_eq!(no.code, code::CLOSED);
    }

    #[tokio::test]
    async fn our_own_too_long_message_is_refused_at_the_writer() {
        let huge = Msg::Revoked { base_id: "b1".into(), text: "я".repeat(LIMIT) };
        let mut wire: Vec<u8> = Vec::new();
        let no = Frame::write(&mut wire, &huge).await.unwrap_err();
        assert_eq!(no.code, code::OVERFLOW);
        assert!(wire.is_empty(), "за предел не уходит ни байта: приёмник закрыл бы канал");
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut wire = (body.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(body);
        wire
    }
}
