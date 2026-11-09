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

    // Ниже — три кода, которых в таблице спеки нет: она описывает отказы,
    // которые ЕДУТ той стороне, а эти три рождаются при чтении кадра, когда
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

/// Для журнала и для цепочек `{e}`, НЕ для окна: код здесь попадает внутрь
/// строки, а спека («Ошибки») требует обратного — код отдельным полем, в
/// тексте человеку его быть не должно. Человеку показывается `text`, и только
/// он.
impl std::fmt::Display for Refuse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]", self.text, self.code)
    }
}

/// Предел самоназвания — восемьдесят ЗНАКОВ.
///
/// Это одна строка в диалоге разрешения, рядом с отпечатком ключа.
/// Восьмидесяти хватает на «ООО „Ромашка“, бухгалтерия» и не хватает на
/// абзац, которым чужая сторона оттеснила бы отпечаток за край окна, — а
/// отпечаток здесь единственное, что человек сверяет голосом по телефону.
///
/// В знаках, а не в байтах: предел виден человеку, а он видит буквы, и
/// русское имя не должно умещаться вдвое хуже латинского.
pub const SAYS_NAME_LIMIT: usize = 80;

/// Самоназвание той стороны — чужие слова, и ТИП, а не правило.
///
/// Спека велит показывать его с пометкой «сказано той стороной, не
/// проверено». Значит это чужая строка на доверительной границе, и
/// обращаться с ней надо соответственно: выбросить то, чем рисуют, и
/// обрезать то, чем оттесняют.
///
/// **Почему тип, а не функция чистки.** Поле едет в двух сообщениях
/// (`hello` и `enroll`), а показывает и хранит его третий код. Правило
/// «не забудь вычистить» держится до первого автора, который забудет, —
/// поэтому внутренняя строка приватна, а единственный способ получить
/// `SaysName` из чего угодно (включая разбор JSON с провода — `serde(from =
/// "String")`) проходит через `new`. Невычищенного самоназвания в программе
/// не существует как пути, а не как запрета.
///
/// **Почему по КАТЕГОРИЯМ, а не по перечню знаков.** Перечень, собранный
/// руками, всегда неполон: в первом же варианте этого кода не нашлось
/// U+2028 и U+2029 (разделители строки и абзаца — ровно то, чем дорисовывают
/// поддельную строку вплотную к отпечатку), мягкого переноса и арабской
/// метки. Выбрасываются целые категории Unicode:
/// * `Cc` — управляющие (перевод строки, возврат каретки, забой): ими
///   дописывают в диалоге вторую строку, например поддельное «отпечаток
///   совпал» вплотную к настоящему отпечатку;
/// * `Cf` — форматирующие (нулевой ширины, разворот направления письма,
///   метка порядка байтов): ими переставляют показанное местами, не меняя
///   ни одной видимой буквы;
/// * `Zl`, `Zp` — разделители строки и абзаца: то же, что `Cc`, другими
///   кодами;
/// * `Co`, `Cn` — частное использование и неназначенные: как они
///   нарисуются, не знает никто, включая нас.
///
/// Пробельное (`Zs`) остаётся, но подряд идущее схлопывается в один пробел:
/// имя с полусотней пробелов растянуло бы строку диалога, не потратив ни
/// одного знака предела зря.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct SaysName(String);

impl SaysName {
    /// Единственная дорога внутрь: чистит категориями, схлопывает пробельное,
    /// срезает края, обрезает по пределу.
    ///
    /// Предел применяется ПОСЛЕ чистки — иначе полоса невидимых знаков
    /// выедала бы его, и в окне осталось бы три буквы из восьмидесяти.
    pub fn new(raw: &str) -> Self {
        use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
        let mut out = String::new();
        let mut kept = 0usize;
        // Пробел не пишется сразу, а запоминается: так подряд идущие
        // схлопываются в один, а край строки не получает его вовсе.
        let mut pending_space = false;
        for c in raw.chars() {
            let keep = match c.general_category() {
                GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::Surrogate => continue,
                GeneralCategory::SpaceSeparator => {
                    pending_space = true;
                    continue;
                }
                _ => c,
            };
            if pending_space && !out.is_empty() {
                if kept + 1 >= SAYS_NAME_LIMIT {
                    break;
                }
                out.push(' ');
                kept += 1;
            }
            pending_space = false;
            if kept >= SAYS_NAME_LIMIT {
                break;
            }
            out.push(keep);
            kept += 1;
        }
        Self(out)
    }

    /// То, что можно показать и записать: вычищенное по построению.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for SaysName {
    fn from(raw: String) -> Self {
        Self::new(&raw)
    }
}

impl From<&str> for SaysName {
    fn from(raw: &str) -> Self {
        Self::new(raw)
    }
}

impl From<SaysName> for String {
    fn from(name: SaysName) -> String {
        name.0
    }
}

impl std::fmt::Display for SaysName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Своими руками: в журнале нужна строка, а не `SaysName("строка")` — имя
/// обёртки там только мешает читать. Печатать опасного нечего, значение
/// вычищено по построению.
impl std::fmt::Debug for SaysName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
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

/// Зачем открыт этот управляющий поток — ОДНА мера на обе стороны.
///
/// **Чем куплено.** Прежде «придёт ли заявка» решали разными мерами: зовущая
/// сторона смотрела, есть ли у неё код, а принимающая — есть ли на машине
/// ХОТЬ ОДИН живой тикет. Выдали код второй карточке — и переподключение
/// первой, которой просить нечего, висело до срока терпения (тридцать
/// секунд): всё это время вожжей на поток нет, то есть отзыв в это окно не
/// немедленный, а в живой поток уезжал мусорный отказ «та сторона не
/// ответила» о стороне, которая ответила всё, что должна была. Теперь
/// спрашивается ровно то, что знает только зовущая, и спрашивается у неё
/// самой.
///
/// **Почему одним полем, а не двумя.** Два поля («жду зачисления» и «вот моя
/// карточка») разрешали бы себе противоречить — «возвращаюсь» вместе с
/// заявкой, — и у приёмной стороны снова было бы два мнения об одном. Здесь
/// противоречие невыразимо: карточка живёт ВНУТРИ возврата.
///
/// **Совместимость.** Поле обязательное, и сторона старой сборки, не
/// приславшая его, получает отказ `разбор` словами (сборка эта никому не
/// отгружена, ALPN взят без номера версии нарочно — см. отчёт задачи).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// Заявка по коду подключения. Какую карточку она просит, скажет сам
    /// тикет: зовущая сторона о карточках той машины ничего не знает и назвать
    /// их не может.
    Enroll,
    /// Возврат по уже данному допуску — поток ОДНОЙ карточки, и вот она
    /// (`base_id`, как его назвала та сторона в `enroll_ok`).
    ///
    /// `None` — допуск помним, а карточку нет. Так выглядит возврат после
    /// перезапуска, пока зовущая сторона не хранит признак базы рядом с
    /// партнёром; та сторона отвечает тогда всем списком, как и раньше.
    Return(Option<String>),
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
    /// и больше ни на что не влияет. `why` — зачем поток открыт: заявка или
    /// возврат по названной карточке (см. `Why`).
    Hello { proto_min: u32, proto_max: u32, side: Side, kit: String, says_name: SaysName, ceiling: Ceiling, why: Why },
    /// Версия выбрана, вот базы, к которым этот ключ уже зачислен. Пустой
    /// список — законный ответ: пришли с тикетом, зачисления ещё нет.
    ///
    /// Возврат, назвавший свою карточку (`Why::Return` с признаком базы),
    /// получает в списке ЕЁ ОДНУ, а не всё, к чему эта сторона допущена: поток
    /// у каждой карточки свой, и числить его за всеми сразу значит рвать
    /// четыре соседних связи отзывом одной (спека, `revoked`).
    HelloOk { proto: u32, side: Side, bases: Vec<BaseBrief> },
    /// Пришёл с тикетом, прошу зачислить.
    Enroll {
        ticket_secret: String,
        says_name: SaysName,
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
        w.write_all(&frame).await.map_err(|e| Refuse::new(code::BROKEN, not_sent(&e)))?;
        w.flush().await.map_err(|e| Refuse::new(code::BROKEN, not_sent(&e)))
    }

    /// Читает один кадр.
    ///
    /// Порядок шагов здесь и есть защита: четыре байта заголовка, сверка с
    /// пределом, и только потом буфер на тело. Буфер по обещанной длине до
    /// сверки — это чужая ошибка, съедающая нашу память молча.
    ///
    /// **Срока ожидания здесь НЕТ, и это решение, а не недосмотр.** Та
    /// сторона может открыть поток и молчать сколько угодно: iroh живое
    /// соединение не рвёт, своего пинга протокол не предусматривает, и
    /// `read` будет ждать вечно. Срок ставит ВЫЗЫВАЮЩИЙ, потому что он у
    /// каждого потока свой: на потоке MCP молчание часами законно (человек
    /// просто не задаёт вопросов), а в разговоре зачисления та же пауза —
    /// повод сказать человеку, что та сторона не отвечает. Общий срок внутри
    /// `read` сломал бы первое, чтобы починить второе. Оборачивайте вызов в
    /// `tokio::time::timeout` там, где ждать нечего.
    pub async fn read<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg, Refuse> {
        // Своими руками, а не `read_exact`: тот на обрыве не говорит, сколько
        // успел прочитать, а по буферу этого не узнать — заголовок ЛЮБОГО
        // сообщения короче 64 КиБ начинается с двух нулей, и все наши
        // управляющие сообщения короче. По нулям в буфере обрыв на втором
        // байте выглядел бы ровно как закрытый поток, а человеку в этих двух
        // случаях говорят разное.
        let mut header = [0u8; 4];
        let mut got = 0usize;
        while got < header.len() {
            match r.read(&mut header[got..]).await {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Err(Refuse::new(code::BROKEN, broken_header(&e))),
            }
        }
        if got == 0 {
            // Ровно на границе кадра — честный конец разговора.
            return Err(Refuse::new(
                code::CLOSED,
                "Та сторона закрыла поток.\nРазговор окончен.\nЕсли это вышло неожиданно, подключитесь заново.",
            ));
        }
        if got < header.len() {
            return Err(Refuse::new(
                code::BROKEN,
                "Связь прервалась на середине сообщения.\nЗаголовок кадра доехал не целиком.\nПодключитесь заново.",
            ));
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

/// Слова на отказ ввода-вывода при записи кадра. Третья строка — совет, как и
/// требует раздел «Ошибки»: без неё человек читает причину и не знает, что
/// делать.
fn not_sent(e: &std::io::Error) -> String {
    format!("Сообщение не ушло той стороне.\nСвязь подвела на записи: {e}\nПодключитесь заново; если повторяется — проверьте сеть.")
}

/// Слова на отказ ввода-вывода при чтении заголовка. Третья строка-совет
/// обязательна (спека, «Ошибки»): причина без совета человеку не нужна, он и
/// так видит, что не получилось.
fn broken_header(e: &std::io::Error) -> String {
    format!("Сообщение с той стороны не прочиталось.\nСвязь подвела на заголовке кадра: {e}\nПодключитесь заново.")
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
        // Имя сообщения знакомое, а тело — нет. Два разных случая на один
        // отказ, и совет обязан годиться обоим: либо у той стороны сборка
        // старше нашей и в её сообщении нет поля, которое мы требуем (ровно
        // это случается со `hello` без `why`), либо это наша ошибка. Совет
        // только про журнал послал бы человека с расхождением версий не туда
        // — и это та же беда, за которую названа находка 2 живого прогона.
        _ => Refuse::new(
            code::MALFORMED,
            format!(
                "Сообщение не разобралось.\nРазбор споткнулся: {e}\nЕсли комплекты на двух машинах разных версий — обновите тот, что \
                 старше; иначе сообщите это вместе с журналом."
            ),
        ),
    }
}

/// Имена сообщений этой версии. Вывести их из `Msg` нечем — serde имён
/// вариантов наружу не отдаёт, — поэтому список держится здесь и сверяется с
/// `Msg` тестом `the_list_of_known_names_matches_the_messages` в ОБЕ стороны:
/// и имя из `Msg`, не попавшее в список (отказ соврал бы «обновите комплект»
/// на сообщение, которое мы понимаем), и имя в списке, которого `Msg` больше
/// не производит, роняют тест.
pub const KNOWN: [&str; 10] = ["hello", "hello_ok", "enroll", "enroll_wait", "enroll_ok", "enroll_no", "state", "revoked", "refuse", "bye"];

fn is_known(name: &str) -> bool {
    KNOWN.contains(&name)
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
                why: Why::Enroll,
            },
            // Все три вида «зачем открыт поток» — по отдельному кадру:
            // возврат с названной карточкой и возврат без неё едут разными
            // полями внутри одного значения, и доехать обязаны оба.
            Msg::Hello {
                proto_min: 1,
                proto_max: 1,
                side: Side::WithAgents,
                kit: "0.1.0".into(),
                says_name: "Ромашка".into(),
                ceiling: Ceiling::ReadOnly,
                why: Why::Return(Some("b1".into())),
            },
            Msg::Hello {
                proto_min: 1,
                proto_max: 1,
                side: Side::WithAgents,
                kit: "0.1.0".into(),
                says_name: "Ромашка".into(),
                ceiling: Ceiling::ReadOnly,
                why: Why::Return(None),
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

    /// Заголовок ЛЮБОГО сообщения короче 64 КиБ начинается с двух нулей, а
    /// все наши управляющие сообщения короче. Обрыв ровно на этих двух нулях
    /// нельзя путать с «та сторона закрыла поток»: человеку в одном случае
    /// говорят «разговор окончен», а в другом «связь прервалась,
    /// подключитесь заново».
    #[tokio::test]
    async fn a_header_of_zeros_cut_short_is_an_interruption_not_a_farewell() {
        let no = Frame::read(&mut Cursor::new(vec![0u8, 0])).await.unwrap_err();
        assert_eq!(no.code, code::BROKEN, "два нуля и обрыв — это обрыв, а не прощание");
        let no = Frame::read(&mut Cursor::new(vec![0u8])).await.unwrap_err();
        assert_eq!(no.code, code::BROKEN, "один байт — тоже начатый кадр");
    }

    /// `hello` старой сборки — без поля «зачем открыт поток» — получает отказ
    /// СЛОВАМИ, а не молчаливое непонимание.
    ///
    /// Поле сделано обязательным нарочно: умолчание («считать возвратом»)
    /// тихо вернуло бы ровно тот дефект, от которого оно заведено, — поток,
    /// числящийся за всеми карточками партнёра сразу. Цена названа вслух:
    /// сборки разных версий не договорятся по управляющему потоку вовсе. Это
    /// приемлемо сейчас и только сейчас — продукт никому не отгружен, а ALPN
    /// взят без номера версии нарочно (спека), чтобы отказ был словами, а не
    /// обрывом на уровне транспорта.
    #[tokio::test]
    async fn a_hello_from_an_older_kit_is_refused_in_words() {
        let old =
            r#"{"msg":"hello","proto_min":1,"proto_max":1,"side":"with_agents","kit":"0.0.9","says_name":"Ромашка","ceiling":"full"}"#;
        let no = Frame::read(&mut Cursor::new(framed(old.as_bytes()))).await.unwrap_err();
        assert_eq!(no.code, code::MALFORMED, "отказ, а не молчаливое согласие с непонятым: {no:?}");
        assert!(no.text.contains("обновите"), "и совет обязан годиться расхождению версий, а не только журналу: {}", no.text);
        // А та же сборка со своим полем читается как обычно — иначе тест
        // стерёг бы не поле, а опечатку в собранном руками JSON.
        let now = old.trim_end_matches('}').to_string() + r#","why":"enroll"}"#;
        assert!(matches!(Frame::read(&mut Cursor::new(framed(now.as_bytes()))).await, Ok(Msg::Hello { why: Why::Enroll, .. })));
    }

    /// Список знакомых имён ломается, если разойдётся с `Msg`: в одну
    /// сторону — имя из `Msg` не названо знакомым (и отказ соврал бы
    /// «обновите комплект» на сообщение, которое мы понимаем), в другую — в
    /// списке осталось имя, которого `Msg` больше не производит.
    #[test]
    fn the_list_of_known_names_matches_the_messages() {
        let produced: Vec<String> =
            every_message().iter().map(|m| serde_json::to_value(m).unwrap()["msg"].as_str().unwrap().to_string()).collect();
        for name in &produced {
            assert!(is_known(name), "имя «{name}» есть в Msg, но не в списке знакомых");
        }
        for name in KNOWN {
            assert!(produced.contains(&name.to_string()), "имя «{name}» в списке знакомых, но Msg его не производит");
        }
    }

    /// Грязного самоназвания не существует как ПУТИ: единственная дорога
    /// внутрь — `new`, и разбор кадра с провода идёт через неё же
    /// (`serde(from = "String")`). Поэтому проверяем самое недоверчивое, что
    /// можно: сырой JSON, собранный руками в обход всякого `Msg`.
    #[test]
    fn a_self_given_name_from_the_wire_arrives_already_clean() {
        // Перевод строки, забой, разделители строки и абзаца, разворот
        // направления письма, мягкий перенос, арабская метка, нулевая ширина,
        // метка порядка байтов — и полоса пробелов на закуску.
        let dirty = "  Ро\u{8}машка\n\r\u{2028}ОТПЕЧАТОК\u{2029}\u{202e}\u{00ad}\u{061c}\u{200b}\u{feff}  тут  ";
        let json = serde_json::json!({ "msg": "enroll", "ticket_secret": "ab", "says_name": dirty }).to_string();
        let Msg::Enroll { says_name, .. } = serde_json::from_str::<Msg>(&json).unwrap() else { panic!("собирали enroll") };

        let shown = says_name.as_str();
        assert_eq!(shown, "РомашкаОТПЕЧАТОК тут", "чистится по категориям, пробелы схлопываются, края срезаются: {shown:?}");
        assert!(!shown.chars().any(|c| c.is_control()), "{shown:?}");

        // И обратно на провод уезжает уже вычищенное, а не исходная грязь.
        let back = serde_json::to_value(Msg::Enroll { ticket_secret: "ab".into(), says_name, base: None }).unwrap();
        assert_eq!(back["says_name"], "РомашкаОТПЕЧАТОК тут");
    }

    /// Длинное имя обрезается по знакам, а не по байтам: предел виден
    /// человеку, а он видит буквы.
    #[test]
    fn a_long_self_given_name_is_cut_by_letters_not_bytes() {
        let long = SaysName::new(&"я".repeat(500));
        assert_eq!(long.as_str().chars().count(), SAYS_NAME_LIMIT, "ровно предел знаков");
        assert_eq!(long.as_str().len(), SAYS_NAME_LIMIT * 2, "в байтах вдвое больше — и это не причина пускать меньше букв");
        // Невидимое не выедает предел: иначе полоса разметки оставила бы в
        // окне три буквы из восьмидесяти.
        let padded = SaysName::new(&format!("{}Ромашка", "\u{200b}".repeat(500)));
        assert_eq!(padded.as_str(), "Ромашка");
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
