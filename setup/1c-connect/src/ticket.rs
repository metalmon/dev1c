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
//! прочитается. Длина при этом не экономится, но тикет живёт секунды между
//! «скопировал» и «вставил», а не передаётся по байту — платить за это
//! нечем.
//!
//! **Перенос строки и невидимые разделители при копировании — не порча.**
//! Длинную строку мессенджер или почтовый клиент переносит сам, добавляя
//! `\n`, а иногда и невидимые знаки (zero-width space `U+200B`, BOM
//! `U+FEFF`) на месте переноса — это делает чужая программа, не человек, и
//! отказывать на этом нечестно: человек код скопировал целиком. `from_code`
//! поэтому вычищает все пробельные и эти невидимые символы из всей строки
//! перед разбором — в самом коде (чистом hex) их не бывает по построению.

use crate::secrets::hex;
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
        // `checked_add`, а не `+`: тот паникует при переполнении внутреннего
        // представления `SystemTime`. Здесь складываются свои значения (часы
        // машины и константа), а не чужой ввод, но переполнение называется
        // отказом словами везде в этом файле — делать здесь исключение из
        // одного и того же правила было бы странно.
        let now = UNIX_EPOCH
            .checked_add(Duration::from_secs(now_secs()))
            .ok_or_else(|| "не удалось завести тикет: часы этой машины вне диапазона, который умеет считать программа".to_string())?;
        let until = now
            .checked_add(LIFETIME)
            .ok_or_else(|| "не удалось завести тикет: часы этой машины вне диапазона, который умеет считать программа".to_string())?;
        Ok(Self { node, relay, direct, secret: sealed, until })
    }

    /// Жив ли тикет на момент `now`. Сутки — включительно: тикет, выданный
    /// вечером, обязан работать утром ровно сутки спустя, а не до неё.
    pub fn alive(&self, now: SystemTime) -> bool {
        now <= self.until
    }

    /// Одна строка для пересылки — hex одного JSON-блока (см. докстроку модуля).
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
        let json = hex::decode(&strip_noise(code))
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
        // Код — строка, которую человек ВСТАВЛЯЕТ из мессенджера: вход
        // недоверенный целиком, и число в нём может быть чем угодно. Срок
        // дальше `дальше_некуда` от «сейчас» — не изношенные часы, а порча
        // или подделка (честная программа дальше `LIFETIME` не выдаёт
        // ничего); отказываем словами раньше, чем число превратится в
        // `SystemTime` и панику при сложении.
        let ceiling = now_secs().saturating_add(LIFETIME.as_secs()).saturating_add(MAX_FUTURE_SLACK.as_secs());
        if wire.until > ceiling {
            return Err("Код повреждён: срок внутри него невозможен — так честная программа не выдаёт.".to_string());
        }
        let until = UNIX_EPOCH
            .checked_add(Duration::from_secs(wire.until))
            .ok_or_else(|| "Код повреждён: срок внутри него невозможен.".to_string())?;
        Ok(Ticket { node: wire.node, relay: wire.relay, direct: wire.direct, secret: Zeroizing::new(secret), until })
    }
}

/// Запас сверх `LIFETIME` на разницу часов двух машин, которую протокол не
/// синхронизирует и не обязан (спека, «Синхронизация»: часы ни во что не
/// участвуют) — без запаса тикет, выданный на машине с чуть спешащими
/// часами, выглядел бы подделкой на машине с чуть отстающими. Срок внутри
/// чужого кода, уехавший дальше `LIFETIME + MAX_FUTURE_SLACK` от «сейчас»,
/// — не изношенные часы, а порча или подделка: честная программа дальше не
/// выдаёт ничего (`from_code` отказывает на этой границе). 10 минут —
/// заметно больше обычного расхождения часов двух живых машин и заметно
/// меньше суток, то есть не открывает дорогу коду с придуманным сроком.
const MAX_FUTURE_SLACK: Duration = Duration::from_secs(10 * 60);

fn now_secs() -> u64 {
    // `unwrap_or(ZERO)`, а не `expect`: отказать здесь может только часы
    // машины, переведённые раньше 1970-го, — это не повод падать, пусть
    // лучше тикет родится просроченным, чем программа встанет.
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

/// Вычищает из вставленного кода пробелы (включая перенос строки) и
/// невидимые разделители, которые мессенджер или почтовый клиент добавляют
/// САМИ при автоматическом переносе длинной строки — zero-width space
/// (`U+200B`) и BOM/zero-width no-break space (`U+FEFF`). Круг починки 1,
/// Critical 2: прежняя версия делала только `trim()` по краям, а перенос
/// посередине строки (ровно то, что происходит с длинным кодом в реальном
/// мессенджере) отказывал «не hex-символ». Безопасно выкинуть ЛЮБОЕ число
/// этих символов из ЛЮБОГО места строки: в настоящем коде (чистом hex) их
/// не бывает по построению (`to_code`), так что вычистить их — не потерять
/// часть кода, а убрать то, что код не писал.
fn strip_noise(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace() && *c != '\u{200B}' && *c != '\u{FEFF}').collect()
}

/// То, что едет внутри кода, до оборачивания в hex. `EndpointId` сериализуется
/// сам (строкой — его собственный `Serialize`, тот же hex, что у `Display`);
/// секрет — отдельным полем-строкой, а не массивом байт, чтобы JSON оставался
/// компактным и читаемым в отладке до финального hex-слоя.
#[derive(Serialize, Deserialize, Clone)]
struct Wire {
    node: EndpointId,
    relay: Option<String>,
    direct: Vec<String>,
    secret: String,
    until: u64,
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

    /// Ревью безопасности на коммит задачи 2: сложение `UNIX_EPOCH + Duration`
    /// паникует при переполнении, а код — строка, которую человек ВСТАВЛЯЕТ
    /// из мессенджера, то есть вход недоверенный целиком. Проверяем и само
    /// переполнение (`u64::MAX`), и число, которое не переполняет, но уехало
    /// далеко за `LIFETIME + MAX_FUTURE_SLACK` — оно не может быть выдано
    /// честной программой и отвергается тем же отказом.
    #[test]
    fn a_code_with_an_impossible_deadline_is_refused_not_panicked() {
        let base = Wire { node: node_id(), relay: None, direct: vec![], secret: hex::encode(&[0u8; 32]), until: 0 };

        let overflowed = Wire { until: u64::MAX, ..base.clone() };
        let no = Ticket::from_code(&hex::encode(&serde_json::to_vec(&overflowed).unwrap())).unwrap_err();
        assert!(!no.is_empty(), "переполнение обязано отказать словами, а не паникой");

        let far_future = Wire { until: now_secs() + LIFETIME.as_secs() + MAX_FUTURE_SLACK.as_secs() + 10, ..base };
        let no = Ticket::from_code(&hex::encode(&serde_json::to_vec(&far_future).unwrap())).unwrap_err();
        assert!(!no.is_empty(), "срок дальше суток с запасом на расхождение часов — не бывает у честной программы");
    }

    /// Круг починки 1, Critical 2: мессенджер переносит длинную строку сам,
    /// добавляя `\n` и иногда невидимые символы на месте переноса — это не
    /// порча кода, а чужая вёрстка вокруг него. Код обязан разобраться и с
    /// переносом посередине, и с невидимыми разделителями (zero-width space,
    /// BOM), которые некоторые клиенты вставляют туда же.
    #[test]
    fn a_code_broken_by_a_line_wrap_or_invisible_separators_still_decodes() {
        let t = sample();
        let code = t.to_code();
        let mid = code.len() / 2;
        let noisy = format!("{}\n\u{200B}{}\u{FEFF}\r\n", &code[..mid], &code[mid..]);

        let back = Ticket::from_code(&noisy).unwrap();
        assert_eq!(back.node, t.node);
        assert_eq!(*back.secret, *t.secret);
        assert_eq!(back.until, t.until);
    }

    /// Минор 6 круга 1: секрет не светится в `{:?}` только потому, что
    /// `Zeroizing` из крейта `zeroize` печатает `Zeroizing { .. }`, а не
    /// содержимое, — чужое поведение, не наш код. Один тест, который
    /// сломается первым, если это когда-нибудь поменяется апгрейдом крейта.
    #[test]
    fn the_secret_never_shows_up_in_debug_output() {
        let t = sample();
        let printed = format!("{t:?}");
        let secret_hex = hex::encode(&*t.secret);
        assert!(!printed.contains(&secret_hex), "секрет не должен быть виден в Debug: {printed}");
    }
}
