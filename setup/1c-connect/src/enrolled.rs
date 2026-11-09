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

use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// это помечается «её слова, не проверено», сюда кладётся как есть.
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

/// Выданный, но ещё не погашенный тикет. `secret` — hex, как печатает
/// `ticket::Ticket::to_code` внутри себя; `until` — unix-секунды, та же
/// точность, с которой тикет и был выдан (`Ticket::issue`), так что сравнение
/// со сроком не теряет и не находит лишней секунды.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IssuedTicket {
    connection: String,
    secret: String,
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
pub struct Book {
    path: PathBuf,
    data: BookFile,
}

impl Book {
    /// Читает файл состояния или заводит пустую книгу, если его ещё нет —
    /// нет файла значит нет истории, а не беда (первый запуск).
    pub fn open(state_dir: &Path) -> Result<Book, String> {
        let path = state_dir.join(FILE);
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: файл зачислений повреждён: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BookFile::default(),
            Err(e) => return Err(format!("не удалось прочитать {}: {e}", path.display())),
        };
        Ok(Book { path, data })
    }

    /// Запоминает выданный тикет: по какому секрету его погашать, к какой
    /// карточке он относится и когда он умирает. Пишет на диск сразу же —
    /// без этого второй процесс (или этот же после перезапуска) не увидел
    /// бы код, который уже отдали человеку.
    pub fn issue(&mut self, connection: &str, ticket: &crate::ticket::Ticket) -> Result<(), String> {
        self.data.tickets.push(IssuedTicket {
            connection: connection.to_string(),
            secret: hex_encode(&*ticket.secret),
            until: ticket.until.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        });
        self.persist()
    }

    /// Гасит тикет по секрету и отдаёт имя карточки, к которой он выдан.
    ///
    /// Зовётся ровно тогда, когда человек на стороне базы РАЗРЕШИЛ заявку —
    /// не раньше. Пришедшую заявку проверяют на живость иначе (протокол сам
    /// решает, показывать ли диалог); если показать его и сеть обрывается
    /// посреди ожидания, `spend` этим вызовом не тронут вовсе, и код остаётся
    /// действительным для повторной попытки — обрыв не должен стоить
    /// человеку нового тикета, который он не виноват, что сгорел.
    ///
    /// Тикет после погашения не хранится нигде (спека, «Что хранится
    /// после») — запись УДАЛЯЕТСЯ, а не помечается «потрачен»: как следствие,
    /// повторная попытка по тому же секрету получает тот же отказ, что и
    /// секрет, которого никогда не было. Это сознательно: раз хранить
    /// «потрачен» запрещено, различить два случая снаружи нечем, и спека это
    /// допускает.
    pub fn spend(&mut self, secret: &[u8; 32]) -> Result<String, String> {
        let want = hex_encode(secret);
        let now = now_secs();
        let Some(pos) = self.data.tickets.iter().position(|t| t.secret == want) else {
            return Err("Код не найден или уже погашен.\nПопросите новый на той машине, которая его выдала.".to_string());
        };
        let expired = self.data.tickets[pos].until < now;
        let connection = self.data.tickets.remove(pos).connection;
        self.persist()?;
        if expired {
            return Err("Код устарел.\nПопросите новый на той машине, которая его выдала.".to_string());
        }
        Ok(connection)
    }

    /// Зачисляет ключ к карточке. Повторная заявка того же ключа заменяет
    /// старую запись, а не плодит дубликат — у ключа на карточке одно
    /// решение, а не история решений.
    pub fn enroll(&mut self, connection: &str, key: EndpointId, says_name: &str, write: bool) -> Result<(), String> {
        self.data.enrolled.retain(|p| !(p.connection == connection && p.key == key));
        self.data.enrolled.push(Enrolled { connection: connection.to_string(), key, says_name: says_name.to_string(), write });
        self.persist()
    }

    /// Отзывает ОДНУ сторону у карточки. Зачисленных сторон бывает сколько
    /// угодно (решение владельца 1.11) — отзыв стирает запись только этого
    /// ключа, остальные зачисления той же карточки не трогает вовсе.
    pub fn revoke(&mut self, connection: &str, key: EndpointId) -> Result<(), String> {
        self.data.enrolled.retain(|p| !(p.connection == connection && p.key == key));
        self.persist()
    }

    /// Кто зачислен к этой карточке прямо сейчас.
    pub fn of(&self, connection: &str) -> Vec<Party> {
        self.data
            .enrolled
            .iter()
            .filter(|p| p.connection == connection)
            .map(|p| Party { key: p.key, says_name: p.says_name.clone(), write: p.write })
            .collect()
    }

    fn persist(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(&self.data).map_err(|e| format!("зачисления не собрались в toml: {e}"))?;
        write_atomic(&self.path, &text)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

/// Через временный файл и переименование — тот же приём, что у `node::write_sealed`:
/// этот файл несёт зачисления ВСЕХ карточек рабочего места, и обрыв питания
/// посреди прямой записи стёр бы их все разом, а не только то одно
/// изменение, ради которого шла запись.
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

/// Hex без разделителей — дублирует `ticket::hex`/`secrets::hex` (оба не
/// публичные): мелкий файловый помощник в этом крейте принято не делить
/// между модулями. Только кодирование: секрет сюда приходит уже байтами, а
/// расшифровывать нечего — сравнение идёт по hex обеих сторон.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::Ticket;
    use std::time::Duration;

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

    #[test]
    fn an_expired_ticket_is_refused_and_removed_not_returned_as_valid() {
        let d = dir("expired");
        let mut ticket = a_ticket();
        ticket.until = SystemTime::now() - Duration::from_secs(1);
        let mut book = Book::open(&d).unwrap();
        book.issue("buh", &ticket).unwrap();

        assert!(book.spend(&ticket.secret).is_err(), "просроченный тикет гасить нечем — отказ");
        assert!(book.spend(&ticket.secret).is_err(), "и повторно он не всплывает как живой");
    }
}
