//! Тикет: одна строка, которую человек пересылает как умеет.
//!
//! Спека: `docs/superpowers/specs/2026-11-01-protocol-design.md`, раздел
//! «Зачисление: от «пришёл с тикетом» до «разрешён»», решение №3. Хранит
//! тикеты и решает их судьбу `enrolled::Book`; здесь — только сам тикет и
//! его печатаемый вид.
//!
//! **Зачем в тикете секрет, раз допуск всё равно даёт человек.** Без
//! секрета любой, кому строку переслали дальше, чем предполагалось, может
//! стучаться и дёргать человека диалогом разрешения — а человек, которого
//! дёргают, рано или поздно нажимает «да». Секрет сам по себе ничего не
//! открывает и не доказывает личность (личность доказывает узел iroh —
//! `node.rs`, отпечаток его ключа сверяют голосом по телефону); секрет
//! делает ЗАЯВКУ одноразовой: один тикет — одна заявка. Это замок на
//! дверном звонке, а не вторая проверка личности.
//!
//! **Срок — сутки.** Код, отправленный вечером, обязан работать утром;
//! дольше нельзя, потому что тикет пересылают почтой и в мессенджерах, где
//! он остаётся лежать неопределённо долго, — а чем дольше он живёт, тем
//! дольше кто-то посторонний, которому он попал в руки по ошибке, может
//! дёргать человека диалогом разрешения.
//!
//! **Печатаемый вид — чистый hex, без разделителей.** Тикет несут вручную:
//! копируют в мессенджер, в письмо, иногда перепечатывают с листа. Base64
//! принёс бы `+`, `/`, и остаток без `=`-паддинга — простой алфавит, но уже
//! с символами, которые некоторые клиенты правят как пунктуацию; JSON сам
//! по себе — кавычки и скобки, которые редакторы чата трактуют как разметку.
//! Hex — это только `0-9a-f`, каждый байт — два предсказуемых знака, и
//! строка разбирается одинаково в любом регистре: если автозамена поднимет
//! первую букву строки (так делают некоторые клавиатуры), код всё равно
//! прочитается. Длина при этом не экономится, но тикет живут секунды между
//! «скопировал» и «вставил», а не передаются по байту — платить за это
//! нечем.

use aes_gcm::aead::Generate;
use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::{Zeroize, Zeroizing};

/// Срок тикета — сутки (решение №3 спеки протокола). Константа, а не магическое
/// число в двух местах (`issue`, docstring), — следующий правящий код увидит
/// довод рядом со значением, а не пойдёт искать его в спеке.
pub const LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Тикет: то, что человек выдаёт и пересылает, чтобы та сторона дозвонилась
/// и заявила о себе одним секретом.
#[derive(Debug, Clone)]
pub struct Ticket {
    /// Кто принимает заявку — ключ узла той машины, что выдала тикет.
    pub node: EndpointId,
    pub relay: Option<String>,
    pub direct: Vec<String>,
    /// Секрет — тайна, как ключ узла (`node::key`): держим его в памяти
    /// через `Zeroizing`, как и там, и не кладём в журнал ни в каком виде.
    pub secret: Zeroizing<[u8; 32]>,
    pub until: SystemTime,
}

impl Ticket {
    /// Выдаёт новый тикет с текущего момента на `LIFETIME` вперёд.
    ///
    /// `until` считается от секунд, а не от полного `SystemTime` с долями:
    /// код несёт срок как целое число секунд (ниже, `to_code`), и округление
    /// здесь, а не при печати, делает округление единственным — круглый код
    /// после `from_code` возвращает ровно то же значение, каким его завели.
    ///
    /// Случайный секрет берётся готовым генератором ОС (`aes-gcm` уже в
    /// зависимостях ради шифрования параметров, `secrets.rs`) — тем же
    /// приёмом, каким там заводится ключ файловой подложки. Отказ генератора
    /// случайных чисел — именованная ошибка, а не паника: по той же причине,
    /// по которой `secrets::file_backend::seal` не паникует на нём же.
    pub fn issue(node: EndpointId, relay: Option<String>, direct: Vec<String>) -> Result<Self, String> {
        let mut secret: [u8; 32] = Generate::try_generate().map_err(|e| format!("не удалось получить случайный секрет тикета: {e}"))?;
        let sealed = Zeroizing::new(secret);
        secret.zeroize();
        Ok(Self { node, relay, direct, secret: sealed, until: UNIX_EPOCH + Duration::from_secs(now_secs()) + LIFETIME })
    }

    /// Жив ли тикет на момент `now`. Сутки — включительно: тикет, выданный
    /// вечером, обязан работать утром ровно сутки спустя, а не до неё.
    pub fn alive(&self, now: SystemTime) -> bool {
        now <= self.until
    }

    /// Одна строка для пересылки — hex одного JSON-блока (см. докстрою модуля).
    ///
    /// `expect` ниже не паника на чужом вводе: сериализуется только то, что
    /// завели сами (строки, числа, ключ узла), и `serde_json` может отказать
    /// тут лишь на нечисловом `NaN`/бесконечности или нечитаемом ключе
    /// карты — ни того, ни другого в этой структуре нет.
    pub fn to_code(&self) -> String {
        let wire = Wire {
            node: self.node,
            relay: self.relay.clone(),
            direct: self.direct.clone(),
            secret: hex::encode(&*self.secret),
            until: self.until.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        };
        let json = serde_json::to_vec(&wire).expect("тикет собран из строк, чисел и ключа узла — сериализация не может отказать");
        hex::encode(&json)
    }

    /// Разбирает код обратно. Любая порча — отказ словами, а не паника:
    /// человек, вставивший код не целиком или не туда, должен прочитать
    /// причину, а не увидеть сбой программы.
    pub fn from_code(code: &str) -> Result<Ticket, String> {
        let json = hex::decode(code.trim())
            .map_err(|e| format!("Код не читается: {e}.\nПроверьте, что он скопирован целиком, без пробелов и переносов строки."))?;
        let wire: Wire = serde_json::from_slice(&json)
            .map_err(|e| format!("Код не разобрался: {e}.\nЭто не тикет этой программы, или он обрезан при копировании."))?;
        let mut secret_bytes = hex::decode(&wire.secret).map_err(|_| "Код повреждён: секрет внутри него не читается.".to_string())?;
        let secret: [u8; 32] = match secret_bytes.as_slice().try_into() {
            Ok(a) => a,
            Err(_) => {
                secret_bytes.zeroize();
                return Err("Код повреждён: секрет внутри него не той длины.".to_string());
            }
        };
        secret_bytes.zeroize();
        Ok(Ticket {
            node: wire.node,
            relay: wire.relay,
            direct: wire.direct,
            secret: Zeroizing::new(secret),
            until: UNIX_EPOCH + Duration::from_secs(wire.until),
        })
    }
}

fn now_secs() -> u64 {
    // `unwrap_or(ZERO)`, а не `expect`: отказать здесь может только часы
    // машины, переведённые раньше 1970-го, — это не повод падать, пусть
    // лучше тикет родится просроченным, чем программа встанет.
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

/// То, что едет внутри кода, до оборачивания в hex. `EndpointId` сериализуется
/// сам (строкой — его собственный `Serialize`, тот же hex, что у `Display`);
/// секрет — отдельным полем-строкой, а не массивом байт, чтобы JSON оставался
/// компактным и читаемым в отладке до финального hex-слоя.
#[derive(Serialize, Deserialize)]
struct Wire {
    node: EndpointId,
    relay: Option<String>,
    direct: Vec<String>,
    secret: String,
    until: u64,
}

/// Hex без разделителей — дублирует `secrets::hex` (тот модуль не публичный):
/// мелкий файловый помощник в этом крейте принято не делить между модулями
/// (тот же приём, что у `unique`/`tempdir` в тестах `allow.rs`).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        let b = s.as_bytes();
        if !b.len().is_multiple_of(2) {
            return Err(format!("нечётная длина ({})", b.len()));
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        };
        b.chunks(2)
            .map(|pair| {
                let hi = digit(pair[0]).ok_or_else(|| "не hex-символ".to_string())?;
                let lo = digit(pair[1]).ok_or_else(|| "не hex-символ".to_string())?;
                Ok(hi << 4 | lo)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    fn sample() -> Ticket {
        Ticket::issue(node_id(), Some("relay.example".into()), vec!["192.168.1.5:7890".into()]).unwrap()
    }

    #[test]
    fn a_code_survives_the_round_trip() {
        let t = sample();
        let code = t.to_code();
        assert!(!code.is_empty());
        assert!(code.chars().all(|c| c.is_ascii_hexdigit()), "код обязан быть чистым hex: {code}");
        assert!(!code.contains(' '), "в коде не бывает пробелов");

        let back = Ticket::from_code(&code).unwrap();
        assert_eq!(back.node, t.node);
        assert_eq!(back.relay, t.relay);
        assert_eq!(back.direct, t.direct);
        assert_eq!(*back.secret, *t.secret, "секрет обязан доехать тем же");
        assert_eq!(back.until, t.until);
    }

    #[test]
    fn a_code_older_than_a_day_is_dead() {
        let t = sample();
        assert!(t.alive(t.until), "ровно сутки — тикет ещё жив");
        assert!(!t.alive(t.until + Duration::from_secs(1)), "сутки и секунда — тикет уже мёртв");
    }

    #[test]
    fn a_spoiled_code_is_refused_with_words_not_a_panic() {
        let code = sample().to_code();

        let truncated = &code[..code.len() / 2];
        assert!(Ticket::from_code(truncated).is_err());

        let mut chars: Vec<char> = code.chars().collect();
        let mid = chars.len() / 2;
        chars[mid] = if chars[mid] == 'z' { 'y' } else { 'z' }; // 'z' — не hex-символ
        let spoiled: String = chars.into_iter().collect();
        let no = Ticket::from_code(&spoiled).unwrap_err();
        assert!(!no.is_empty());

        let empty = Ticket::from_code("").unwrap_err();
        assert!(!empty.is_empty());
    }
}
