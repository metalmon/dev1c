//! Хранилище зачислений и выданных тикетов.
//!
//! Живое состояние программы, а не настройки человека: `1c-connect.toml`
//! хранит пароли и поля карточек, которые человек заводит руками, а здесь —
//! то, что программа запомнила сама про допуск (кто зачислен, какие тикеты
//! ещё не погашены). Разные по природе данные, разные файлы — смешивать их
//! значило бы, что бэкап параметров тащит за собой чужие ключи, а чистка
//! зачислений рискует задеть пароль базы. Файл лежит в каталоге состояния
//! (`Kit::state`), пишется при каждом изменении, читается при старте; формат
//! — тот же `toml`, что у параметров, но свой файл.
//!
//! Спека: `docs/superpowers/specs/2026-11-01-protocol-design.md`, раздел
//! «Зачисление»; `docs/superpowers/specs/2026-11-01-connection-contract.md`,
//! раздел «Что хранится после».

use crate::secrets::hex;
use crate::single_instance;
use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Имя файла в каталоге состояния — рядом с `node.key`, `allow`, `install.state`.
const FILE: &str = "enrolled.toml";

/// Одна зачисленная сторона: открытый ключ и то, что о ней решено.
///
/// Зачисленных сторон на одну карточку может быть сколько угодно (решение
/// владельца 1.11) — предел назначает лицензия 1С на одновременные
/// сеансы, а не мы; поэтому это список, а не одно поле у карточки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub key: EndpointId,
    /// Как сторона назвала себя при заявке (протокол, `says_name`) — в окне
    /// это помечается «её слова, не проверено».
    ///
    /// Чужая строка, и сюда она попадает уже вычищенной — без управляющих и
    /// невидимых знаков, обрезанной по длине (`enroll::as_said`). Чистит
    /// зачисление, а не книга: чистить надо ОДИН раз и до всего остального,
    /// иначе показанное человеку и записанное здесь разойдутся, а спор между
    /// ними разрешать нечем.
    pub says_name: String,
    pub write: bool,
}

/// Запись о зачислении внутри файла — то же, что `Party`, плюс карточка, к
/// которой это зачисление относится: файл один на все карточки рабочего
/// места, `Party` наружу отдаётся уже отфильтрованной по карточке (`of`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Enrolled {
    connection: String,
    key: EndpointId,
    says_name: String,
    write: bool,
}

/// Выданный, но ещё не погашенный тикет.
///
/// **Секрета здесь нет — есть `fingerprint`, sha256 от него (решение
/// ведущего по замечанию проверки безопасности).** Хранящей стороне секрет
/// самой не нужен: ей нужно уметь проверить ПРЕДЪЯВЛЕННЫЙ секрет, а не
/// предъявить свой, — а это ровно то, что умеет отпечаток: посчитать тот же
/// хэш от пришедшего и сравнить. Украсть файл тогда бессмысленно: из
/// отпечатка секрет не собрать обратно никак — это сильнее, чем запечатать
/// секрет через `secrets::Store` (как ключ узла в `node.rs`), потому что
/// запечатанное открывается той же учётной записью, а отпечаток не
/// открывается вообще. Это и точнее ложится на спеку: «тикет не хранится
/// нигде после погашения» теперь верно и ДО погашения тоже.
///
/// `until` — unix-секунды, та же точность, с которой тикет и был выдан
/// (`Ticket::issue`), так что сравнение со сроком не теряет и не находит
/// лишней секунды.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IssuedTicket {
    connection: String,
    fingerprint: String,
    until: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BookFile {
    #[serde(default)]
    enrolled: Vec<Enrolled>,
    #[serde(default)]
    tickets: Vec<IssuedTicket>,
}

/// Хранилище: зачисления и выданные тикеты одного рабочего места.
///
/// `data` — отражение диска НА МОМЕНТ последнего своего чтения или записи
/// этим объектом, не единственная правда: окно и отдельный процесс
/// `1c-connect mcp` держат РАЗНЫЕ `Book` на один файл одновременно (замок
/// рабочего места — `single_instance` — на пользователя, не на этот файл),
/// поэтому каждая мутация (`mutate`) перечитывает файл заново под замком
/// файла, а не доверяет этому снимку (круг починки 1, Important 4).
pub struct Book {
    path: PathBuf,
    data: BookFile,
}

impl Book {
    /// Читает файл состояния или заводит пустую книгу, если его ещё нет —
    /// нет файла значит нет истории, а не беда (первый запуск).
    ///
    /// Просроченные тикеты выбрасываются сразу при чтении: держать то, что
    /// уже не сработает, незачем, а если что-то успели выбросить — тут же
    /// перезаписываем файл, чтобы старый хвост не пережил ещё один
    /// перезапуск без единого изменения.
    pub fn open(state_dir: &Path) -> Result<Book, String> {
        let path = state_dir.join(FILE);
        let _guard = lock(&path)?;
        let mut data = load_raw(&path)?;
        if prune_expired(&mut data) {
            write_book(&path, &data)?;
        }
        Ok(Book { path, data })
    }

    /// Проверяет тикет БЕЗ записи: жив ли по сроку, не потрачен ли, к какой
    /// карточке — ровно шаг третий спеки «Зачисление» («Принимающая
    /// проверяет тикет... Не сошлось — отказ с причиной словами и
    /// закрытие; человека при этом не беспокоят вовсе»). Погашение —
    /// отдельный шаг, пятый, и отдельный метод (`spend`), который зовётся
    /// только после решения человека.
    ///
    /// Без этого разделения (круг починки 1, Critical 1) автору зачисления
    /// пришлось бы выбирать между двумя одинаково плохими путями: гасить
    /// тикет ДО разрешения (тогда обрыв сети посреди ожидания сжигает код
    /// человеку безо всякой его вины — ту же беду уже решает правило «гасим
    /// после разрешения», но решает её только для `spend`, если есть кем
    /// проверить раньше) или звать диалог разрешения на любой пришедший
    /// мусор без проверки (а человека, которого дёргают диалогом разрешения
    /// достаточно часто, рано или поздно нажимает «да» — ровно то, от чего
    /// секрет тикета и был заведён, решение 3 спеки протокола).
    ///
    /// Перечитывает файл заново (как и `mutate` у пишущих методов) и тоже
    /// берёт межпроцессный замок, хотя сам ничего не пишет: это не защита от
    /// половины файла (её и без замка даёт `write_atomic` переименованием —
    /// читатель между началом и концом замены увидит либо целый старый файл,
    /// либо целый новый, никогда половину), а защита от гонки с ПОБОЧНЫМ
    /// эффектом чтения — `load_raw` переименовывает файл в карантин, увидев
    /// порчу (`quarantine`), и без замка это переименование могло бы
    /// столкнуться с чужой записью, которая в тот же миг кладёт файл на
    /// место после `rename` в `write_atomic` у другого процесса.
    pub fn check(&self, secret: &[u8; 32]) -> Result<String, String> {
        match self.verdict(secret)? {
            Verdict::Card(name) => Ok(name),
            Verdict::Unknown => Err(UNKNOWN_TICKET.to_string()),
            Verdict::Expired => Err(EXPIRED_TICKET.to_string()),
        }
    }

    /// То же, что `check`, но ВЕРДИКТОМ, а не словами.
    ///
    /// Нужно тому, кто обязан различать три разных случая, а не два (круг
    /// починки 2 задачи 5, Important 6). `check` сводит «кода нет»,
    /// «код устарел» и «книга не прочиталась» к одному `Err` со строкой, и
    /// разобрать их обратно можно только по тексту — то есть по совпадению
    /// слов, которое переживёт ровно до первой правки формулировки. А разница
    /// между ними не косметическая: первые два — обычная жизнь замка на
    /// дверном звонке, им место в спокойном отказе той стороне; третий — НАША
    /// беда, и он обязан краснеть в нашем журнале и не выдавать той стороне
    /// ни путей, ни подробностей.
    ///
    /// Поэтому здесь `Err` значит ровно одно: книгу не удалось прочитать.
    /// Любой вердикт по самому коду — это `Ok`.
    pub fn verdict(&self, secret: &[u8; 32]) -> Result<Verdict, String> {
        let _guard = lock(&self.path)?;
        let data = load_raw(&self.path)?;
        Ok(match locate(&data, secret) {
            Found::At(pos) => Verdict::Card(data.tickets[pos].connection.clone()),
            Found::Expired => Verdict::Expired,
            Found::Unknown => Verdict::Unknown,
        })
    }

    /// Есть ли вообще хоть один живой выданный код — с диска, под замком.
    ///
    /// Вопрос узкий, а нужен он затем, что принимающая сторона иначе не
    /// знает, ждать ли ей заявку. Уже зачисленная сторона обычно просто
    /// переподключается и `enroll` не шлёт вовсе — а спека не даёт ей слова
    /// сказать «мне не о чем просить». Ждать её заявку на всякий случай
    /// значило бы задержать разговор на весь срок ожидания при каждом
    /// переподключении; не ждать вовсе — проглотить законную заявку на ВТОРУЮ
    /// карточку. Выданный код и есть тот признак, по которому эти два случая
    /// различаются без единого нового поля на проводе: нет непогашенных
    /// кодов — ничья заявка не могла бы сработать, и ждать нечего.
    pub fn has_live_ticket(&self) -> Result<bool, String> {
        let _guard = lock(&self.path)?;
        let data = load_raw(&self.path)?;
        let now = now_secs();
        Ok(data.tickets.iter().any(|t| t.until >= now))
    }

    /// Запоминает выданный тикет: отпечаток секрета (не сам секрет — см.
    /// докстроку `IssuedTicket`), к какой карточке он относится и когда он
    /// умирает. Пишет на диск сразу же — без этого второй процесс (или этот
    /// же после перезапуска) не увидел бы код, который уже отдали человеку.
    pub fn issue(&mut self, connection: &str, ticket: &crate::ticket::Ticket) -> Result<(), String> {
        self.write_ticket(connection, ticket, false)
    }

    /// Перевыпуск: новый код карточки ГАСИТ все прежние её коды, и гасит в
    /// одной записи с появлением нового.
    ///
    /// Так читает «Выдать новый код» человек, и читает он верно — особенно
    /// когда нажимает это именно потому, что прежняя строка куда-то утекла:
    /// «новый» без «старый больше не работает» оставляет утёкший код живым
    /// ещё на сутки (`ticket::LIFETIME`), и нажавший об этом не узнает.
    ///
    /// Одна `mutate`, а не «погасить, потом выдать»: между двумя записями
    /// может оборваться питание, и карточка осталась бы либо без кода вовсе
    /// (погасили, выдать не успели), либо с двумя живыми (выдали, погасить
    /// не успели) — второе ровно то, от чего перевыпуск и заводится.
    ///
    /// Отличается от `issue` только этим. `issue` оставлена как есть: одна
    /// карточка С ДВУМЯ живыми кодами — законное состояние хранилища (так
    /// проверяется, что `spend` гасит ровно один), и запрещать его в самом
    /// нижнем слое незачем. Выдаёт код человеку только окно, и оно зовёт
    /// `reissue` всегда (`gui::panel::issue_code`).
    pub fn reissue(&mut self, connection: &str, ticket: &crate::ticket::Ticket) -> Result<(), String> {
        self.write_ticket(connection, ticket, true)
    }

    fn write_ticket(&mut self, connection: &str, ticket: &crate::ticket::Ticket, douse_old: bool) -> Result<(), String> {
        let now = now_secs();
        let until = ticket.until.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs();
        if until < now {
            // Круг починки 1, Минор 6: раньше выдача уже просроченного
            // тикета тихо возвращала успех, а первая же запись (чистка
            // просрочки внутри `persist`) выбрасывала его обратно — человек
            // получал на руки код, которого в хранилище уже не было. Лучше
            // отказать здесь словами, чем соврать об успехе. Гашение прежних
            // при таком отказе тоже не идёт: карточка не должна остаться без
            // кода из-за попытки выдать негодный.
            return Err("Этот тикет уже просрочен — выдавать его некому.".to_string());
        }
        let connection = connection.to_string();
        let fingerprint = fingerprint(&ticket.secret);
        self.mutate(move |data| {
            if douse_old {
                data.tickets.retain(|t| t.connection != connection);
            }
            data.tickets.push(IssuedTicket { connection, fingerprint, until })
        })
    }

    /// Гасит тикет по секрету и отдаёт имя карточки, к которой он выдан.
    ///
    /// Зовётся ровно тогда, когда человек на стороне базы РАЗРЕШИЛ заявку —
    /// не раньше (шаг пятый спеки; шаг третий — непортящая проверка,
    /// `check`, выше). Тикет после погашения не хранится нигде (спека, «Что
    /// хранится после») — запись УДАЛЯЕТСЯ, а не помечается «потрачен»: как
    /// следствие, повторная попытка по тому же секрету получает тот же
    /// отказ, что и секрет, которого никогда не было. Это сознательно: раз
    /// хранить «потрачен» запрещено, различить два случая снаружи нечем, и
    /// спека это допускает.
    pub fn spend(&mut self, secret: &[u8; 32]) -> Result<String, String> {
        let secret = *secret;
        self.mutate(move |data| match locate(data, &secret) {
            Found::At(pos) => Ok(data.tickets.remove(pos).connection),
            Found::Expired => Err(EXPIRED_TICKET.to_string()),
            Found::Unknown => Err(UNKNOWN_TICKET.to_string()),
        })?
    }

    /// Зачисляет ключ к карточке. Повторная заявка того же ключа заменяет
    /// старую запись, а не плодит дубликат — у ключа на карточке одно
    /// решение, а не история решений.
    pub fn enroll(&mut self, connection: &str, key: EndpointId, says_name: &str, write: bool) -> Result<(), String> {
        let connection = connection.to_string();
        let says_name = says_name.to_string();
        self.mutate(move |data| {
            data.enrolled.retain(|p| !(p.connection == connection && p.key == key));
            data.enrolled.push(Enrolled { connection, key, says_name, write });
        })
    }

    /// Отзывает ОДНУ сторону у карточки. Зачисленных сторон бывает сколько
    /// угодно (решение владельца 1.11) — отзыв стирает запись только этого
    /// ключа, остальные зачисления той же карточки не трогает вовсе.
    pub fn revoke(&mut self, connection: &str, key: EndpointId) -> Result<(), String> {
        let connection = connection.to_string();
        self.mutate(move |data| data.enrolled.retain(|p| !(p.connection == connection && p.key == key)))
    }

    /// Зачислен ли ЭТОТ ключ к ЭТОЙ карточке — вопрос допуска, и ответ на
    /// него читается с диска, под замком, а не из снимка в памяти.
    ///
    /// **Правило, которое держит этот метод: решение о допуске никогда не
    /// принимается по `of`** (круг починки 1 задачи 5, HIGH 1). Снимок в
    /// памяти годится для ПОКАЗА — списка в окне, который и так
    /// перерисовывается, — но не для «пускать ли». Между тем, как процесс
    /// открыл книгу, и тем, как по этой книге кого-то пустили, человек в
    /// окне успевает нажать «Отозвать разрешение», и отзыв пишет другой
    /// `Book` на тот же файл. Спека про отзыв говорит «отправляется и тут же
    /// рвёт потоки этой базы» — отзыв, который не действует немедленно, не
    /// отзыв вовсе, а по устаревшему снимку отозванная сторона входила бы
    /// ровно как зачисленная.
    ///
    /// Под замком, но без перезаписи, — как и `check`: `write_atomic`
    /// заменяет файл переименованием, и читатель увидит либо целый старый,
    /// либо целый новый, никогда половину.
    pub fn enrolled_to(&self, connection: &str, key: EndpointId) -> Result<Option<Party>, String> {
        let _guard = lock(&self.path)?;
        let data = load_raw(&self.path)?;
        Ok(data.enrolled.iter().find(|p| p.connection == connection && p.key == key).map(|p| Party {
            key: p.key,
            says_name: p.says_name.clone(),
            write: p.write,
        }))
    }

    /// Кто зачислен к этой карточке прямо сейчас — из снимка, обновлённого
    /// последним чтением или записью ЭТОГО объекта (см. докстроку `Book`).
    ///
    /// Для ПОКАЗА. Решать по нему, пускать ли кого-то, нельзя — на это есть
    /// `enrolled_to`, читающий диск (см. его докстроку).
    pub fn of(&self, connection: &str) -> Vec<Party> {
        self.data
            .enrolled
            .iter()
            .filter(|p| p.connection == connection)
            .map(|p| Party { key: p.key, says_name: p.says_name.clone(), write: p.write })
            .collect()
    }

    /// Единственное место, которое меняет файл: берёт межпроцессный замок,
    /// перечитывает файл заново (а не доверяет `self.data` — круг починки 1,
    /// Important 4: другой `Book`, живущий в соседнем процессе на тот же
    /// файл, мог дописать его между нашим предыдущим чтением и этим
    /// вызовом), даёт вызывающему изменить свежий снимок, чистит просрочку и
    /// пишет. `self.data` обновляется ТОЛЬКО после успешной записи — если
    /// запись не удалась, в памяти остаётся прежнее (совпадающее с диском)
    /// состояние, а не та правка, которая на диск не попала (иначе
    /// `spend`, доложивший об ошибке записи, вернул бы тикет к жизни в
    /// памяти этого процесса, а на следующем перезапуске — и на диске).
    fn mutate<T>(&mut self, f: impl FnOnce(&mut BookFile) -> T) -> Result<T, String> {
        let _guard = lock(&self.path)?;
        let mut fresh = load_raw(&self.path)?;
        let result = f(&mut fresh);
        prune_expired(&mut fresh);
        write_book(&self.path, &fresh)?;
        self.data = fresh;
        Ok(result)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

/// Общий поиск для `check`, `spend` и `verdict`: по секрету — за постоянное
/// время (`constant_time_eq`) — и по сроку. `spend` после успеха
/// дополнительно удаляет найденную запись сам; здесь только позиция и
/// решение, жива ли она.
///
/// Отдаёт ВЕРДИКТ, а не `Result<usize, String>`: слова отказа — дело
/// вызывающего (`check` их и пишет), а здесь они только мешали бы — тому,
/// кому нужен не текст, а различие (`verdict`), пришлось бы разбирать текст
/// обратно.
fn locate(data: &BookFile, secret: &[u8; 32]) -> Found {
    let want = fingerprint(secret);
    let now = now_secs();
    match data.tickets.iter().position(|t| constant_time_eq(&t.fingerprint, &want)) {
        None => Found::Unknown,
        Some(pos) if data.tickets[pos].until < now => Found::Expired,
        Some(pos) => Found::At(pos),
    }
}

/// Что нашлось по секрету — внутренний вид `Verdict`, с позицией записи для
/// `spend`. Наружу позиция не ездит: она верна только под тем же замком, под
/// которым читалась.
enum Found {
    At(usize),
    Expired,
    Unknown,
}

/// Вердикт по предъявленному коду. Три случая, а не два, — см. `verdict`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Код настоящий и жив; вот карточка, к которой он выдан.
    Card(String),
    /// Срок кода вышел.
    Expired,
    /// Такого кода в книге нет: его либо не выдавали, либо уже погасили.
    /// Различить эти два снаружи нечем нарочно — хранить «потрачен» спека
    /// запрещает («тикет не хранится нигде после погашения»).
    Unknown,
}

/// Слова `check` на два вердикта из трёх. Вынесены в константы, потому что
/// их же обязан знать `spend` — два писателя одного текста разошлись бы на
/// первой правке.
const UNKNOWN_TICKET: &str = "Код не найден или уже погашен.\nПопросите новый на той машине, которая его выдала.";
const EXPIRED_TICKET: &str = "Код устарел.\nПопросите новый на той машине, которая его выдала.";

/// Выбрасывает тикеты, срок которых уже прошёл. Возвращает `true`, если
/// что-то выбросили, — `open` отличает этим «надо дописать файл» от «файл и
/// так в порядке».
fn prune_expired(data: &mut BookFile) -> bool {
    let now = now_secs();
    let before = data.tickets.len();
    data.tickets.retain(|t| t.until >= now);
    before != data.tickets.len()
}

/// sha256 от секрета — то, что лежит в файле вместо самого секрета (см.
/// докстроку `IssuedTicket`). Hex, а не сырые байты: тот же вид, что у
/// остального в этом toml, читается в журнале разбора беды без расшифровщика.
fn fingerprint(secret: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(secret);
    hex::encode(h.finalize().as_slice())
}

/// Сравнение двух отпечатков за постоянное время: без единого раннего
/// выхода внутри цикла по содержимому, только по длине. Длина отпечатка не
/// секрет (у sha256 она всегда одна), так что выход по ней ничего не выдаёт
/// — в отличие от выхода по первому несовпавшему байту, который и выдаёт,
/// сколько байт угадано верно.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Путь файла межпроцессного замка — ОТДЕЛЬНЫЙ от `enrolled.toml`, никогда
/// не переименовываемый. `write_atomic`/`write_book` заменяют данные
/// переименованием: замок на САМОМ `enrolled.toml` держался бы за старый
/// инод уже после первой же записи, защищая файл, которого под этим путём
/// больше нет, — ровно та ошибка, по которой `single_instance.rs` запирает
/// замком КАТАЛОГ состояния, а не файл, который держит `acquire`.
fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("toml.lock")
}

/// Берёт межпроцессный замок с короткими повторами (круг починки 1,
/// Important 4): окно и подкоманда `mcp` живут одновременно и оба держат
/// свой `Book` на один файл — голый один `try` отказывал бы человеку на
/// совпадении в доли миллисекунды, которое само пройдёт. Предел — не
/// вечность: зависший держатель называет себя отказом словами, а не тихим
/// бесконечным ожиданием.
fn lock(path: &Path) -> Result<single_instance::Guard, String> {
    let lock_path = lock_path(path);
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match single_instance::file_lock(&lock_path) {
            Ok(Some(guard)) => return Ok(guard),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Файл зачислений занят другим процессом дольше полсекунды.\nПовторите действие.\nЕсли это повторяется, сообщите нам, \
                         назвав файл {}.",
                        lock_path.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(format!("не удалось взять замок {}: {e}", lock_path.display())),
        }
    }
}

/// Читает файл в структуру или заводит пустую (файла нет — не беда, первый
/// запуск).
///
/// Непонятный файл — не повод терять ВСЕ зачисления сразу (круг починки 1,
/// Important 5): он откладывается рядом (`enrolled.toml.broken`) для
/// разбора беды, а книга продолжает жить пустой — отказ не возвращается
/// вызывающему, только слово в журнал через `quarantine`. Это касается и
/// файла от прежнего коммита ветки (с полем `secret` вместо `fingerprint`):
/// он тоже не разберётся новой схемой и уйдёт тем же путём — с той
/// разницей, что `quarantine` вычищает из отложенной копии узнаваемое поле
/// `secret`, а не сохраняет голый секрет под новым именем навсегда, и
/// говорит об этой вычистке словами в тот же журнал (круг починки 2):
/// человек может открыть отложенный файл сам и сравнить его с прежним, и
/// молчание о том, что мы его сами изменили, было бы тем самым «молчаливо
/// неверным», от которого лечится весь проект.
///
/// **Решение владельца (круг починки 2, спорить не предложено): карантин —
/// «всё или ничего», частичного спасения читаемых записей из непонятного
/// файла НЕТ.** Наполовину разобранный файл опаснее целого карантина:
/// человек считал бы кого-то зачисленным, а записи о нём на самом деле уже
/// нет, и узнал бы об этом в самый неподходящий момент. Зачисления
/// восстанавливаются тем же способом, каким выдавались, — заново зачислить
/// дешевле, чем жить с недоверенной серединой файла.
fn load_raw(path: &Path) -> Result<BookFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(data) => Ok(data),
            Err(e) => {
                quarantine(path, &text, &e.to_string());
                Ok(BookFile::default())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BookFile::default()),
        Err(e) => Err(format!("не удалось прочитать {}: {e}", path.display())),
    }
}

/// Откладывает непонятный файл рядом и говорит об этом словами в журнал
/// (`eprintln!` — тот же приём, что у `single_instance::acquire_serving`
/// для похожих «не вышло, но не фатально» случаев). Переименование, а не
/// копирование: тогда исходный путь освобождается, и следующая же запись
/// честно заводит там свежий файл, а не натыкается на тот же разбитый
/// текст по кругу.
///
/// Когда `redact_legacy_secret` и правда что-то вычистила — это отдельная
/// правка отложенной копии, и о ней нельзя молчать (круг починки 2):
/// человек может открыть `enrolled.toml.broken` сам, сравнить с тем, что
/// было, и увидеть не то, что мы туда отложили, а то, что мы это ещё и
/// переписали. Вторая строка журнала называет это прямо — что секрет
/// прежнего образца был и его вычистили, а не просто что файл отложен.
fn quarantine(path: &Path, text: &str, why: &str) {
    let broken = path.with_extension("toml.broken");
    let safe = redact_legacy_secret(text);
    let redacted = safe != text;
    let renamed = std::fs::rename(path, &broken).is_ok();
    if !renamed || redacted {
        let _ = std::fs::write(&broken, &safe);
    }
    eprintln!("{}: файл зачислений не разобрался ({why}) — отложен как {}, начинаю с пустой книги", path.display(), broken.display());
    if redacted {
        eprintln!(
            "{}: в отложенной копии был секрет тикета прежнего образца (поле `secret`) — он вычищен, хранить его незачем",
            broken.display()
        );
    }
}

/// Вычищает значение поля `secret` — единственное узнаваемое отличие
/// формата ДО перехода на отпечаток (коммит с заголовком про sha256-отпечаток
/// секрета тикета). Карантинная копия нужна для разбора беды (структура,
/// число записей, имена карточек), а не для того, чтобы голый секрет из
/// файла прежней версии продолжил лежать на диске открытым текстом под
/// новым именем бесконечно.
fn redact_legacy_secret(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("secret ") || trimmed.starts_with("secret=") || trimmed.starts_with("secret\t") {
                "secret = \"<вычищено при карантине>\"".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_book(path: &Path, data: &BookFile) -> Result<(), String> {
    let text = toml::to_string_pretty(data).map_err(|e| format!("зачисления не собрались в toml: {e}"))?;
    write_atomic(path, &text)
}

/// Через временный файл и переименование — тот же приём, что у `node::write_sealed`:
/// этот файл несёт зачисления ВСЕХ карточек рабочего места, и обрыв питания
/// посреди прямой записи стёр бы их все разом, а не только то одно
/// изменение, ради которого шла запись.
///
/// На Unix временный файл создаётся с правами 0600 сразу при создании, одним
/// системным вызовом (требование 3 замечания безопасности) — так между
/// появлением файла на диске и ограничением прав не остаётся окна, в которое
/// файл мог бы прочитать кто-то кроме владельца; переименование права не
/// меняет, так что итоговый файл получает их тем же путём. На Windows
/// отдельного пути сузить права ФАЙЛА этот крейт не заводит ни для одного
/// файла состояния (приватность секретов там несёт DPAPI — `secrets.rs` —
/// а не ACL файла): файл наследует права каталога состояния (`Kit::state`),
/// и если своего пути нет — то нет, и это стоит сказать прямо, а не
/// промолчать.
#[cfg(unix)]
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("не удалось переименовать {} в {}: {e}", tmp.display(), path.display())
    })
}

#[cfg(not(unix))]
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("не удалось переименовать {} в {}: {e}", tmp.display(), path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::Ticket;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("onec-enrolled-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn node_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    fn a_ticket() -> Ticket {
        Ticket::issue(node_id(), None, vec![]).unwrap()
    }

    /// MEDIUM 2 проверки безопасности: «Выдать новый код» не гасил прежний.
    /// Человек, нажимая это, думает ровно одно — старый больше не работает,
    /// — и нажимает он это чаще всего потому, что старый куда-то утёк. Код,
    /// оставшийся живым, живёт ещё сутки (`ticket::LIFETIME`), и нажавший об
    /// этом не узнает.
    ///
    /// Падал так: `reissue` не существовало, окно звало `issue`, та только
    /// дописывала запись — после выдачи нового ПРЕЖНИЙ код по-прежнему
    /// опознавался `check` и гасился `spend`, то есть впускал.
    #[test]
    fn a_reissued_code_douses_the_previous_one_for_that_card() {
        let d = dir("reissue");
        let old = a_ticket();
        let new = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.reissue("buh", &old).unwrap();
        assert_eq!(book.check(&old.secret).unwrap(), "buh", "иначе тест проверяет не то");

        book.reissue("buh", &new).unwrap();
        assert!(book.check(&old.secret).is_err(), "прежний код обязан быть отвергнут — именно за этим и нажимают «Выдать новый»");
        assert_eq!(book.check(&new.secret).unwrap(), "buh", "а новый — работать");

        // Гашение дошло до файла, а не только до снимка в памяти: иначе
        // перезапуск вернул бы утёкший код к жизни.
        drop(book);
        let mut again = Book::open(&d).unwrap();
        assert!(again.spend(&old.secret).is_err(), "прежний код вернулся после перезапуска");
        assert_eq!(again.spend(&new.secret).unwrap(), "buh");
    }

    /// Чужая карточка перевыпуском не задевается: гасятся коды ТОЙ карточки,
    /// которой выдают новый, — у соседки свой код и своя сторона.
    #[test]
    fn a_reissue_does_not_touch_another_cards_code() {
        let d = dir("reissue-neighbour");
        let neighbour = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.reissue("mcp", &neighbour).unwrap();
        book.reissue("buh", &a_ticket()).unwrap();
        assert_eq!(book.check(&neighbour.secret).unwrap(), "mcp", "перевыпуск одной карточки погасил код другой");
    }

    /// Негодный новый код прежние не гасит: карточка не должна остаться без
    /// кода вовсе из-за попытки выдать просроченный.
    #[test]
    fn a_refused_reissue_leaves_the_working_code_alone() {
        let d = dir("reissue-refused");
        let good = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.reissue("buh", &good).unwrap();
        let mut stale = a_ticket();
        stale.until = UNIX_EPOCH;
        assert!(book.reissue("buh", &stale).is_err(), "просроченный код выдавать некому");
        assert_eq!(book.check(&good.secret).unwrap(), "buh", "отказ выдачи погасил годный код");
    }

    #[test]
    fn a_secret_spends_once_and_the_second_try_is_refused() {
        let d = dir("spend-once");
        let ticket = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        let name = book.spend(&ticket.secret).unwrap();
        assert_eq!(name, "buh");

        let no = book.spend(&ticket.secret).unwrap_err();
        assert!(!no.is_empty(), "второй раз по тому же секрету — отказ, а не паника");
    }

    #[test]
    fn an_unspent_secret_survives_a_restart_and_a_spent_one_does_not_come_back() {
        let d = dir("restart");
        let unspent = a_ticket();
        let spent = a_ticket();

        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &unspent).unwrap();
        book.issue("buh", &spent).unwrap();
        book.spend(&spent.secret).unwrap();
        drop(book);

        // Второй процесс или этот же после перезапуска — файл читается заново.
        let mut again = Book::open(&d).unwrap();
        assert_eq!(again.spend(&unspent.secret).unwrap(), "buh", "непотраченный тикет обязан пережить перезапуск");
        assert!(again.spend(&spent.secret).is_err(), "потраченный тикет не возвращается после перезапуска");
    }

    #[test]
    fn revoking_one_party_leaves_the_others_alone() {
        let d = dir("revoke-one");
        let a = node_id();
        let b = node_id();
        let mut book = Book::open(&d).unwrap();
        book.enroll("buh", a, "Бухгалтер", true).unwrap();
        book.enroll("buh", b, "Разработчик", false).unwrap();

        book.revoke("buh", a).unwrap();

        let left: Vec<EndpointId> = book.of("buh").into_iter().map(|p| p.key).collect();
        assert_eq!(left, vec![b], "отозванного ключа нет, второй — на месте");
    }

    #[test]
    fn a_ticket_is_not_spent_until_the_person_has_allowed() {
        let d = dir("not-spent-early");
        let ticket = a_ticket();

        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        // Заявка пришла, человек ещё думает — и тут сеть рвётся. Погашение
        // не случилось: оно происходит ПОСЛЕ разрешения, а не в момент
        // прихода заявки, так что оборванная попытка не стоит человеку
        // нового тикета.
        drop(book);
        let mut book = Book::open(&d).unwrap();

        // Человек наконец разрешает — только теперь тикет гасится, и ровно
        // один раз.
        assert_eq!(book.spend(&ticket.secret).unwrap(), "buh");
        assert!(book.spend(&ticket.secret).is_err(), "второй раз тот же код уже не работает");
    }

    /// Критично 1 круга починки 1: `check` — непортящая проверка (шаг
    /// третий спеки), `spend` — погашение (шаг пятый, после разрешения
    /// человека). Без разделения пришлось бы выбирать между сожжённым
    /// обрывом сети кодом и диалогом на любой мусор — ровно то, от чего
    /// секрет тикета и был заведён.
    #[test]
    fn checking_a_ticket_does_not_spend_it_but_spend_does() {
        let d = dir("check-vs-spend");
        let ticket = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        assert_eq!(book.check(&ticket.secret).unwrap(), "buh", "проверка обязана узнать карточку");
        assert_eq!(book.check(&ticket.secret).unwrap(), "buh", "повторная проверка — тот же результат: check не гасит");
        assert_eq!(book.spend(&ticket.secret).unwrap(), "buh", "а spend гасит");
        assert!(book.check(&ticket.secret).is_err(), "после spend проверка уже не находит тикет");
        assert!(book.spend(&ticket.secret).is_err(), "и spend повторно тоже не находит");
    }

    #[test]
    fn checking_an_unknown_secret_is_refused() {
        let d = dir("check-unknown");
        let book = Book::open(&d).unwrap();
        assert!(book.check(&[0x22u8; 32]).is_err(), "неизвестный секрет не подтверждается проверкой");
    }

    #[test]
    fn an_expired_ticket_is_refused_and_removed_not_returned_as_valid() {
        let d = dir("expired");
        let mut ticket = a_ticket();
        ticket.until = SystemTime::now() - Duration::from_secs(1);
        let mut book = Book::open(&d).unwrap();

        // Круг починки 1, Минор 6: явно просроченный тикет отказывает уже
        // при выдаче — не тихий успех с подвохом при первой же попытке
        // погасить.
        assert!(book.issue("buh", &ticket).is_err(), "просроченный тикет выдавать нечем — отказ сразу");
        assert!(book.spend(&ticket.secret).is_err(), "и погасить его тоже нечем — он не попал в хранилище");
    }

    /// Замечание безопасности, требование «неверный секрет отвергается»: и
    /// попытка с чужим секретом должна провалиться, и настоящий тикет
    /// обязан пережить такую попытку — неверная заявка не имеет права
    /// погасить чужой код.
    #[test]
    fn a_wrong_secret_is_refused_and_does_not_burn_the_real_ticket() {
        let d = dir("wrong-secret");
        let ticket = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        let wrong = [0xA5u8; 32];
        assert!(book.spend(&wrong).is_err(), "чужой секрет не должен гаситься как настоящий");
        assert_eq!(book.spend(&ticket.secret).unwrap(), "buh", "настоящий тикет обязан быть жив после неверной попытки");
    }

    /// Замечание безопасности, требование «в файле секрета нет ни в каком
    /// виде»: ищем и сырой hex секрета, и сами байты секрета на любой
    /// границе — файл держит только отпечаток, из которого секрет не
    /// собрать обратно (Important 3 круга починки 1: раньше здесь искался
    /// только hex, байты — нет).
    #[test]
    fn the_file_never_holds_the_raw_secret_in_any_form() {
        let d = dir("no-secret-on-disk");
        let ticket = a_ticket();
        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        let bytes = std::fs::read(d.join(FILE)).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let secret_hex = hex::encode(&*ticket.secret);
        assert!(!text.contains(&secret_hex), "секрет в hex-виде не должен попадать в файл: {text}");
        assert!(
            !bytes.windows(32).any(|w| w == &ticket.secret[..]),
            "сырые байты секрета не должны обнаружиться в файле ни на одной границе"
        );
        assert!(text.contains(&fingerprint(&ticket.secret)), "а отпечаток там быть обязан — иначе check/spend нечем сверять: {text}");
    }

    /// Замечание безопасности, требование «просроченный тикет исчезает из
    /// файла сам»: пруним и при чтении (имитация файла, оставшегося от
    /// прошлой версии программы — дописываем запись руками, минуя `issue`,
    /// который теперь сам отказывает заводить просроченный тикет), и при
    /// записи (следующая же запись, не обязательно про тикеты, подчищает
    /// заодно).
    #[test]
    fn an_expired_ticket_is_pruned_on_read_and_on_write() {
        let d = dir("prune");
        std::fs::create_dir_all(&d).unwrap();
        let stale_fp = fingerprint(&[0x11u8; 32]);
        let hand_written = format!("[[tickets]]\nconnection = \"buh\"\nfingerprint = \"{stale_fp}\"\nuntil = 1\n");
        std::fs::write(d.join(FILE), &hand_written).unwrap();

        let mut book = Book::open(&d).unwrap();
        let after_open = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(!after_open.contains(&stale_fp), "чтение обязано переписать файл без просроченной записи: {after_open}");

        // А если бы что-то и просочилось мимо чтения — любая следующая
        // запись (не обязательно про тикеты) подчищает её тоже.
        std::fs::write(d.join(FILE), &hand_written).unwrap();
        book.enroll("buh", node_id(), "Кто-то", false).unwrap();
        let after_write = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(!after_write.contains(&stale_fp), "запись обязана переписать файл без просроченной записи: {after_write}");
    }

    /// Важно 4 круга починки 1: окно и подкоманда `mcp` держат РАЗНЫЕ
    /// объекты `Book` на один файл (замок рабочего места — на пользователя,
    /// не на файл). Прежняя версия перезаписывала файл целиком своим
    /// снимком — второй писатель молча стирал первого.
    #[test]
    fn two_book_handles_on_the_same_file_do_not_lose_each_others_writes() {
        let d = dir("two-writers");
        let mut window = Book::open(&d).unwrap();
        let mut mcp = Book::open(&d).unwrap();

        let a = a_ticket();
        let b = a_ticket();
        window.issue("buh", &a).unwrap();
        mcp.issue("buh", &b).unwrap();

        assert_eq!(window.spend(&a.secret).unwrap(), "buh", "выдача первого писателя не должна стереть выдачу второго");
        assert_eq!(mcp.spend(&b.secret).unwrap(), "buh", "и наоборот");
    }

    /// Важно 5 круга починки 1: битый файл не должен утягивать с собой ВСЕ
    /// зачисления безвозвратно. Откладывается рядом, книга продолжает жить
    /// пустой, дальше можно зачислять и выдавать тикеты снова.
    #[test]
    fn a_broken_file_is_quarantined_not_fatal() {
        let d = dir("broken-file");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FILE), "это не toml вовсе: {{{").unwrap();

        let mut book = Book::open(&d).expect("битый файл не должен отказывать в открытии книги");
        assert!(book.of("buh").is_empty(), "после карантина книга пустая, а не в панике");
        assert!(d.join("enrolled.toml.broken").exists(), "непонятный файл обязан быть отложен рядом");

        let key = node_id();
        book.enroll("buh", key, "Кто-то", false).unwrap();
        assert_eq!(book.of("buh").len(), 1, "после карантина зачисление снова работает");
    }

    /// Файл до перехода на отпечаток (поле `secret` вместо `fingerprint`)
    /// новой схемой тоже не разбирается — и уходит тем же путём в карантин,
    /// а не остаётся лежать с голым секретом открытым текстом под прежним
    /// именем.
    #[test]
    fn a_file_from_before_the_fingerprint_fix_is_quarantined_and_its_secret_is_redacted() {
        let d = dir("legacy-secret");
        std::fs::create_dir_all(&d).unwrap();
        let legacy =
            "[[tickets]]\nconnection = \"buh\"\nsecret = \"deadbeef000000000000000000000000000000000000000000000000000000\"\nuntil = \
                      99999999999\n";
        std::fs::write(d.join(FILE), legacy).unwrap();

        let book = Book::open(&d).expect("старый формат не должен отказывать в открытии книги");
        assert!(book.of("buh").is_empty());

        let broken_text = std::fs::read_to_string(d.join("enrolled.toml.broken")).unwrap();
        assert!(
            !broken_text.contains("deadbeef"),
            "секрет из старого файла не должен лежать в карантинной копии открытым текстом: {broken_text}"
        );
    }
}
