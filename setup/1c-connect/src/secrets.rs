//! Шифрование и расшифровка значений в файле параметров (`1c-connect.toml`) — единственная точка, где пароль превращается в шифртекст на диске и обратно в строку в памяти; остальной код про пароль знает ровно столько же, сколько знал бы про открытый текст (решение владельца 29.10, спека `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md`).
//!
//! Никакого своего шифра и своего формата: на Windows — готовое хранилище системы (DPAPI пользователя, `CryptProtectData`/`CryptUnprotectData`), на Linux и в тестах — готовый AEAD (`aes-gcm`) с ключом в файле. Выбор подложки решает владелец (DPAPI там, где он даёт настоящее свойство «украденный файл бесполезен на другой машине»), а не исполнитель по удобству.

use std::path::{Path, PathBuf};

/// Префикс шифртекста в toml: по нему человек, открывший файл, видит, что
/// значение не пароль и подставлять его куда-либо бессмысленно. Значение без
/// префикса читается как открытый текст — всегда, а не только в переходный
/// период: это и путь обновления старых файлов, и честный выход для закрытого
/// контура, где человек сознательно хочет открытый текст.
pub const PREFIX: &str = "enc1:";

/// Имя файла ключа подложки `file` и Linux-подложки `system` — рядом с
/// остальным состоянием коннектора (`Kit::state`).
const KEY_FILE: &str = "secret.key";

/// Отказ расшифровки. Снаружи «шифртекст не наш» (другой пользователь, другая
/// машина, ключа нет, ключ не тот) и «файл повреждён» неотличимы для АЕАD и
/// DPAPI одинаково — оба просто отказывают, не называя причину — поэтому
/// обвинять человека в порче файла нельзя (спека, «Анти-цели»): любой отказ
/// самой расшифровки (неверный тег AEAD, DPAPI другого пользователя) —
/// `Foreign`. `Broken` остаётся для другого класса отказов — поломки
/// хранения (ключ на диске не той длины, файл ключа нечитаем по чужой
/// причине, чем отсутствие), которые не про несовпадение ключа и шифртекста,
/// а про то, что у нас на руках вообще не те байты.
#[derive(Debug)]
pub enum OpenError {
    Foreign,
    Broken(String),
}

enum Backend {
    /// Windows: DPAPI пользователя. Ключа на диске нет вовсе — его держит
    /// система, и ровно это даёт нужное свойство: шифртекст не открывается ни
    /// на другой машине, ни у другого пользователя (цель спеки «бэкап и
    /// перенос»).
    #[cfg(windows)]
    Dpapi,
    /// Linux и тесты (на любой платформе): файл ключа в каталоге.
    File(PathBuf),
}

/// Единственная точка превращения пароля в шифртекст и обратно. Какая
/// подложка внутри — не видно снаружи `seal`/`open`: вызывающий код (чтение и
/// запись параметров, задача 2) не разбирает платформу сам.
pub struct Store {
    backend: Backend,
}

impl Store {
    /// Подложка системы: на Windows — DPAPI пользователя, `state_dir` не
    /// используется (ключа на диске нет вовсе, см. `Backend::Dpapi`); на
    /// Linux — файл ключа в этом же каталоге состояния, то есть то же самое,
    /// что `Store::file`. `Result` в подписи — ради единообразия обеих
    /// платформ в вызывающем коде; на деле ни один из путей сегодня не
    /// отказывает при самом создании `Store` — отказать может только
    /// `seal`/`open`.
    pub fn system(state_dir: &Path) -> Result<Store, String> {
        #[cfg(windows)]
        {
            let _ = state_dir;
            Ok(Store { backend: Backend::Dpapi })
        }
        #[cfg(not(windows))]
        {
            Ok(Store::file(state_dir))
        }
    }

    /// Файл ключа в каталоге — подложка для Linux и готовая, честная
    /// пустышка для тестов на любой платформе (она же проверяет настоящий
    /// путь шифрования, а не притворяется). Конструктор ключ не заводит: он
    /// появляется — или отказывает — только при первом `seal` (ровно тогда
    /// понадобится), что и проверяет тест на каталоге без прав на запись.
    pub fn file(dir: &Path) -> Store {
        Store { backend: Backend::File(dir.to_path_buf()) }
    }

    /// Пустую строку не шифруем и возвращаем пустой: иначе `render`
    /// параметров перестал бы быть детерминированным (два прогона с одним и
    /// тем же пустым паролем писали бы разные файлы из-за случайного nonce),
    /// а в задаче 2 на этом падают тесты образца, где оба пароля пусты.
    pub fn seal(&self, plain: &str) -> Result<String, String> {
        if plain.is_empty() {
            return Ok(String::new());
        }
        let sealed = match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi => dpapi::seal(plain.as_bytes())?,
            Backend::File(dir) => file_backend::seal(dir, plain.as_bytes())?,
        };
        Ok(format!("{PREFIX}{}", hex::encode(&sealed)))
    }

    /// Значение без префикса — открытый текст, и это касается пустой строки
    /// тоже: старый файл с паролем открытым текстом обязан читаться как есть,
    /// а не падать на пустом поле.
    pub fn open(&self, value: &str) -> Result<String, OpenError> {
        let Some(hex_value) = value.strip_prefix(PREFIX) else {
            return Ok(value.to_string());
        };
        // Нечитаемый hex — не про несовпадение ключа и шифртекста, а про то,
        // что в файле не те байты (рука человека или урезанная копия) —
        // `Broken`, а не `Foreign`.
        let sealed = hex::decode(hex_value).map_err(|e| OpenError::Broken(format!("шифртекст не в hex: {e}")))?;
        let plain = match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi => dpapi::open(&sealed)?,
            Backend::File(dir) => file_backend::open(dir, &sealed)?,
        };
        // Расшифровалось, но не в UTF-8 — тот же случай, что и неверный
        // ключ: AEAD дал байты, но не наши. Дальше по цепочке это неотличимо
        // от «ключ не тот», поэтому и названо так же.
        String::from_utf8(plain).map_err(|_| OpenError::Foreign)
    }
}

/// Hex вместо base64: кодирование одно на обе подложки (условие задания), а
/// ни один из двух готовых крейтов (`hex`, `base64`) на деле не нужен —
/// выбор без нового имени в `Cargo.toml` проще для ревью, чем выбор с ним.
/// Побочный эффект, который стоит увидеть: hex-алфавит не может содержать
/// кириллицу, так что тест «шифртекст не содержит пароль» доказан самой
/// кодировкой, а не только шифром.
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    /// По байтам, а не по символам строки: срез `&s[i..i+2]` падал бы на
    /// границе многобайтового символа, если в toml окажется что-то не-hex —
    /// а значение туда приходит снаружи (рука человека), и падать на чужом
    /// вводе нельзя.
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

/// Подложка Linux и тестов: AEAD (`aes-gcm`) с ключом в файле.
///
/// `ponytail:` ключ лежит рядом с параметрами и копируется вместе с конфигом
/// — украденная пара «параметры плюс ключ» на другой машине работает, и
/// защита здесь заведомо слабее, чем DPAPI на Windows (спека, «Ловушки»).
/// Путь наверх — secret service системы, когда появится машина, где он есть
/// по умолчанию без отдельного демона; сегодня его нет даже в контейнере
/// стенда, и тянуть живой демон ради одного файла дороже, чем цена этого
/// компромисса.
mod file_backend {
    use super::{OpenError, KEY_FILE};
    use aes_gcm::{
        aead::{Aead, Generate, KeyInit},
        Aes256Gcm, Key, Nonce,
    };
    use std::path::{Path, PathBuf};

    /// Nonce AES-GCM — 12 байт; константа, а не `Nonce::<_>::default().len()`,
    /// чтобы длина была видна на месте разбора шифртекста без похода в
    /// документацию крейта.
    const NONCE_LEN: usize = 12;

    fn key_path(dir: &Path) -> PathBuf {
        dir.join(KEY_FILE)
    }

    fn cipher_from(key: &[u8; 32]) -> Result<Aes256Gcm, String> {
        Aes256Gcm::new_from_slice(key).map_err(|_| "ключ не 32 байта — повреждён".to_string())
    }

    /// Ключ для шифрования: читаем существующий или заводим новый. Ровно
    /// здесь, а не в конструкторе `Store::file`, отказывает каталог без прав
    /// на запись (тест `a_read_only_state_dir_is_a_named_refusal`) — узнать
    /// об этом раньше первого реального шифрования негде и не нужно.
    fn load_or_create_key(dir: &Path) -> Result<[u8; 32], String> {
        let path = key_path(dir);
        if let Ok(bytes) = std::fs::read(&path) {
            return bytes.as_slice().try_into().map_err(|_| format!("ключ в {} повреждён — не 32 байта", path.display()));
        }
        // Каталог может уже существовать (его создал `tmp()` до смены прав) —
        // `create_dir_all` тогда ничего не делает и прав на запись не
        // спрашивает; настоящая проверка прав — на `fs::write` ниже.
        std::fs::create_dir_all(dir).map_err(|e| format!("не удалось создать каталог ключа {}: {e}", dir.display()))?;
        let key: Key<Aes256Gcm> = Key::<Aes256Gcm>::generate();
        std::fs::write(&path, key.as_slice()).map_err(|e| format!("не удалось сохранить ключ шифрования в {}: {e}", path.display()))?;
        // 0600 — только на Unix: на Windows у файла и так нет посторонних
        // читателей по умолчанию, а `set_permissions` с unix-битами там же и
        // не собрался бы.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("не удалось выставить права 0600 на ключ {}: {e}", path.display()))?;
        }
        Ok(key.as_slice().try_into().expect("Key<Aes256Gcm> — 32 байта по типу"))
    }

    /// Ключ для расшифровки: отсутствие файла — `Foreign` (ключа нет —
    /// ровно тот случай из описания `OpenError::Foreign`), а не повод
    /// заводить новый ключ молча — иначе чужой шифртекст с таким же
    /// префиксом притворился бы понятым. Любая другая причина не прочитать
    /// файл (права, диск) — `Broken`: это не про чужой ключ, а про то, что
    /// своего не достать.
    fn read_key(dir: &Path) -> Result<[u8; 32], OpenError> {
        let path = key_path(dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                bytes.as_slice().try_into().map_err(|_| OpenError::Broken(format!("ключ в {} повреждён — не 32 байта", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(OpenError::Foreign),
            Err(e) => Err(OpenError::Broken(format!("не удалось прочитать ключ {}: {e}", path.display()))),
        }
    }

    pub fn seal(dir: &Path, plain: &[u8]) -> Result<Vec<u8>, String> {
        let key = load_or_create_key(dir)?;
        let cipher = cipher_from(&key)?;
        let nonce = Nonce::generate(); // случайный на каждый вызов — повтор секрета не выдаёт себя одинаковым шифртекстом
        let ciphertext = cipher.encrypt(&nonce, plain).map_err(|_| "шифрование не удалось".to_string())?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn open(dir: &Path, sealed: &[u8]) -> Result<Vec<u8>, OpenError> {
        if sealed.len() < NONCE_LEN {
            return Err(OpenError::Broken(format!("шифртекст короче nonce ({} байт)", sealed.len())));
        }
        let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_LEN);
        let key = read_key(dir)?;
        let cipher = cipher_from(&key).map_err(OpenError::Broken)?;
        let nonce = Nonce::try_from(nonce_bytes).map_err(|_| OpenError::Broken("nonce не разобрать".to_string()))?;
        // Неверный тег AEAD — чужой ключ, чужая машина, подмена байт: всё это
        // неотличимо друг от друга и от «ключа никогда не было», поэтому
        // `Foreign`, а не `Broken` (спека, «любой отказ расшифровки»).
        cipher.decrypt(&nonce, ciphertext).map_err(|_| OpenError::Foreign)
    }
}

/// Подложка Windows: DPAPI пользователя (`CryptProtectData`/
/// `CryptUnprotectData`, `crypt32.dll`). Область — пользователь (флаги без
/// `CRYPTPROTECT_LOCAL_MACHINE`): то же самое значение, расшифрованное другим
/// пользователем этой же машины, не откроется — ровно то свойство, которое
/// нужно для бэкапа и переноса файла параметров. `CRYPTPROTECT_UI_FORBIDDEN`
/// — чтобы DPAPI никогда не попытался показать диалог: коннектор вызывает это
/// из кода без интерактивного пользователя (служебные операции, тесты).
#[cfg(windows)]
mod dpapi {
    use super::OpenError;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: `input` живёт до конца вызова (мы его не двигаем), DPAPI
        // сам выделяет `output.pbData` через `LocalAlloc` — освобождаем его
        // сами через `LocalFree`, как требует документация функции.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(format!("DPAPI отказался шифровать значение (код {})", unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe { LocalFree(output.pbData as _) };
        Ok(bytes)
    }

    pub fn open(sealed: &[u8]) -> Result<Vec<u8>, OpenError> {
        let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            // Чужой пользователь, чужая машина, испорченный блоб — DPAPI не
            // говорит, какой из случаев, и мы тоже не называем «битый файл»
            // (спека, «Анти-цели»): это всегда `Foreign`.
            return Err(OpenError::Foreign);
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe { LocalFree(output.pbData as _) };
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        // Местный приём: без новых крейтов, уникально на процесс (publication.rs:786).
        let d = std::env::temp_dir().join(format!("secrets-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_value_without_the_prefix_comes_back_untouched() {
        let s = Store::file(&tmp("plain"));
        assert_eq!(s.open("открытый пароль").unwrap(), "открытый пароль");
        assert_eq!(s.open("").unwrap(), "");
    }

    #[test]
    fn an_empty_secret_is_not_sealed() {
        let s = Store::file(&tmp("empty"));
        assert_eq!(s.seal("").unwrap(), "", "пустое значение обязано остаться пустым");
    }

    #[test]
    fn a_sealed_value_carries_the_prefix_and_not_the_secret() {
        let s = Store::file(&tmp("seal"));
        let sealed = s.seal("пароль 1С").unwrap();
        assert!(sealed.starts_with(PREFIX), "{sealed}");
        assert!(!sealed.contains("пароль"), "секрет виден в шифртексте: {sealed}");
        assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
    }

    #[test]
    fn a_secret_with_quotes_and_cyrillic_survives_the_round_trip() {
        let s = Store::file(&tmp("round"));
        for plain in [r#"па"роль"#, r"C:\x\y", "пароль 1С", "p w", "ПРОБЕЛ в конце "] {
            let sealed = s.seal(plain).unwrap();
            assert_eq!(s.open(&sealed).unwrap(), plain, "не сошлось на {plain:?}");
        }
    }

    #[test]
    fn the_same_secret_seals_differently_twice() {
        // Случайный nonce: одинаковый шифртекст выдал бы, что пароль не менялся.
        let s = Store::file(&tmp("nonce"));
        assert_ne!(s.seal("секрет").unwrap(), s.seal("секрет").unwrap());
    }

    #[test]
    fn a_foreign_ciphertext_is_named_foreign_not_broken() {
        let mine = Store::file(&tmp("mine"));
        let theirs = Store::file(&tmp("theirs"));
        let sealed = mine.seal("секрет").unwrap();
        assert!(matches!(theirs.open(&sealed), Err(OpenError::Foreign)));
    }

    #[test]
    fn a_ciphertext_without_a_key_is_foreign_too() {
        let dir = tmp("nokey");
        let sealed = Store::file(&dir).seal("секрет").unwrap();
        std::fs::remove_file(dir.join("secret.key")).unwrap();
        assert!(matches!(Store::file(&dir).open(&sealed), Err(OpenError::Foreign)));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_state_dir_is_a_named_refusal() {
        // Под root права не действуют — тогда проверять нечего, тест пропускается.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tmp("readonly");
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
        let e = Store::file(&dir).seal("секрет").unwrap_err();
        assert!(e.contains("ключ") && e.contains(&dir.display().to_string()), "{e}");
    }

    // DPAPI на сборочном агенте не зовём (спека: «в тестах нельзя звать DPAPI»):
    // тест остаётся для ручного прогона на машине человека.
    #[cfg(windows)]
    #[ignore = "зовёт DPAPI пользователя — гонять руками, не в CI"]
    #[test]
    fn the_system_store_on_windows_round_trips_through_dpapi() {
        let s = Store::system(&tmp("dpapi")).unwrap();
        let sealed = s.seal("пароль 1С").unwrap();
        assert!(!sealed.contains("пароль"));
        assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
    }
}
