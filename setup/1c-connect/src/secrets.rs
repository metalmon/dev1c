//! Шифрование и расшифровка значений в файле параметров (`1c-connect.toml`) — единственная точка, где пароль превращается в шифртекст на диске и обратно в строку в памяти; остальной код про пароль знает ровно столько же, сколько знал бы про открытый текст (решение владельца 29.10, спека `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md`).
//!
//! Никакого своего шифра и своего формата: на Windows — готовое хранилище системы (DPAPI пользователя, `CryptProtectData`/`CryptUnprotectData`), на Linux и в тестах — готовый AEAD (`aes-gcm`) с ключом в файле. Выбор подложки решает владелец (DPAPI там, где он даёт настоящее свойство «украденный файл бесполезен на другой машине»), а не исполнитель по удобству.

use std::path::{Path, PathBuf};
use zeroize::Zeroize;

/// Префикс шифртекста в toml: по нему человек, открывший файл, видит, что
/// значение не пароль и подставлять его куда-либо бессмысленно. Значение без
/// префикса читается как открытый текст — всегда, а не только в переходный
/// период: это и путь обновления старых файлов, и честный выход для закрытого
/// контура, где человек сознательно хочет открытый текст.
pub const PREFIX: &str = "enc1:";

/// Имя файла ключа подложки `file` и Linux-подложки `system` — рядом с
/// остальным состоянием коннектора (`Kit::state`).
const KEY_FILE: &str = "secret.key";

/// Отказ расшифровки. Граница между двумя вариантами — не «что звучит хуже»,
/// а «кто виноват»: `Foreign` — ровно всё, что связано с несовпадением
/// шифртекста и ключа или с самим шифртекстом (неверный тег AEAD, DPAPI
/// другого пользователя, нечитаемый hex, урезанный шифртекст, подмена поля в
/// AAD) — снаружи эти случаи неотличимы друг от друга и от «ключа никогда не
/// было», и обвинять человека в порче файла нельзя (спека, «Анти-цели»:
/// любой отказ расшифровки — `Foreign`). `Broken` — только то, что сломано у
/// НАС: ошибка ОС при чтении своего же файла ключа, отказ генератора
/// случайных чисел. Это не про шифртекст человека, и смешивать две причины в
/// одной ветке значило бы потерять обе.
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

    /// `field` — имя поля (`"password"`, `"db_password"`…), подмешанное как
    /// связанные данные: на Linux это AAD у AEAD, на Windows — `entropy` у
    /// DPAPI. Без этого шифртексты двух полей одного toml взаимозаменяемы —
    /// переставив их местами, человек получил бы валидную расшифровку
    /// чужого пароля, и коннектор отправил бы пароль базы туда, где ждут
    /// пароль рабочего места (ревью задачи 1, Minor 7). Пустую строку не
    /// шифруем и возвращаем пустой: иначе `render` параметров перестал бы
    /// быть детерминированным (два прогона с одним и тем же пустым паролем
    /// писали бы разные файлы из-за случайного nonce), а в задаче 2 на этом
    /// падают тесты образца, где оба пароля пусты.
    pub fn seal(&self, field: &str, plain: &str) -> Result<String, String> {
        if plain.is_empty() {
            return Ok(String::new());
        }
        let sealed = match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi => dpapi::seal(field.as_bytes(), plain.as_bytes())?,
            Backend::File(dir) => file_backend::seal(dir, field.as_bytes(), plain.as_bytes())?,
        };
        Ok(format!("{PREFIX}{}", hex::encode(&sealed)))
    }

    /// Значение без префикса — открытый текст, и это касается пустой строки
    /// тоже: старый файл с паролем открытым текстом обязан читаться как есть,
    /// а не падать на пустом поле. `field` — то же имя, что было у `seal`;
    /// другое имя (или переставленное значение) не расшифруется — это
    /// `Foreign`, а не «битый файл» (см. `OpenError`).
    pub fn open(&self, field: &str, value: &str) -> Result<String, OpenError> {
        let Some(hex_value) = value.strip_prefix(PREFIX) else {
            return Ok(value.to_string());
        };
        // Нечитаемый hex — тоже «шифртекст не наш»: отличить порчу от
        // подмены снаружи нельзя, и спека прямо запрещает называть это
        // битым файлом (ревью задачи 1, Important 5).
        let sealed = hex::decode(hex_value).map_err(|_| OpenError::Foreign)?;
        let plain = match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi => dpapi::open(field.as_bytes(), &sealed)?,
            Backend::File(dir) => file_backend::open(dir, field.as_bytes(), &sealed)?,
        };
        // Расшифровалось, но не в UTF-8 — тот же случай, что и неверный
        // ключ: AEAD дал байты, но не наши. Затираем их перед отказом — это
        // всё равно расшифрованное значение, просто не наше (Important 4).
        match String::from_utf8(plain) {
            Ok(s) => Ok(s),
            Err(e) => {
                let mut bytes = e.into_bytes();
                bytes.zeroize();
                Err(OpenError::Foreign)
            }
        }
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
        aead::{Aead, Generate, KeyInit, Payload},
        Aes256Gcm, Key, Nonce,
    };
    use common::fsperm;
    use std::path::{Path, PathBuf};
    use zeroize::{Zeroize, Zeroizing};

    /// Nonce AES-GCM — 12 байт; константа, а не `Nonce::<_>::default().len()`,
    /// чтобы длина была видна на месте разбора шифртекста без похода в
    /// документацию крейта.
    const NONCE_LEN: usize = 12;

    fn key_path(dir: &Path) -> PathBuf {
        dir.join(KEY_FILE)
    }

    /// Ключ всегда ровно 32 байта по типу вызывающей стороны (`[u8; 32]`) —
    /// `new_from_slice` здесь не может отказать по длине, и заворачивать его
    /// в `Result` ради формальности незачем.
    fn cipher_from(key: &[u8; 32]) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(key).expect("ключ Aes256Gcm всегда 32 байта — гарантия типа")
    }

    /// Почему чтение ключа не получилось. `NotFound` вызывающий превращает в
    /// `Foreign` (ключа нет — ровно тот случай из описания `OpenError`), всё
    /// остальное — в `Broken`: это не про чужой ключ, а про то, что своего не
    /// достать (ошибка ОС, либо файл на диске не той длины).
    enum KeyReadError {
        NotFound,
        BadLength(usize),
        Io(std::io::Error),
    }

    impl KeyReadError {
        fn describe(self, path: &Path) -> String {
            match self {
                KeyReadError::NotFound => format!("ключа в {} нет", path.display()),
                KeyReadError::BadLength(n) => format!("ключ в {} повреждён — {n} байт вместо 32", path.display()),
                KeyReadError::Io(e) => format!("не удалось прочитать ключ {}: {e}", path.display()),
            }
        }
    }

    /// Читает существующий ключ и поджимает его права до 0600: файл мог
    /// появиться раньше этого фикса или права на него могли ослабить снаружи
    /// между запусками, и поджимать стоит на каждом чтении, а не только в
    /// момент создания (ревью задачи 1, Important 3). Ошибку поджатия не
    /// считаем фатальной для самого чтения — отсутствие строгих прав на
    /// Linux уже названо компромиссом (`ponytail:` выше), а отказ вернуть
    /// пароль человеку из-за одного `chmod` был бы хуже.
    fn read_key_bytes(path: &Path) -> Result<Zeroizing<[u8; 32]>, KeyReadError> {
        match std::fs::read(path) {
            // `mut bytes`: это сырая копия ключа, и она обязана быть затёрта
            // перед возвратом в обоих исходах (успех и `BadLength`) — иначе
            // обещание «ключ везде как Zeroizing» не выполняется там, где он
            // ненадолго лежит в обычном `Vec<u8>` (ревью задачи 1, круг 3,
            // пункт 2).
            Ok(mut bytes) => {
                let len = bytes.len();
                let key: Option<[u8; 32]> = bytes.as_slice().try_into().ok();
                bytes.zeroize();
                match key {
                    Some(key) => {
                        let _ = fsperm::tighten_file(path);
                        Ok(Zeroizing::new(key))
                    }
                    None => Err(KeyReadError::BadLength(len)),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(KeyReadError::NotFound),
            Err(e) => Err(KeyReadError::Io(e)),
        }
    }

    fn read_key(dir: &Path) -> Result<Zeroizing<[u8; 32]>, OpenError> {
        let path = key_path(dir);
        read_key_bytes(&path).map_err(|e| match e {
            KeyReadError::NotFound => OpenError::Foreign,
            other => OpenError::Broken(other.describe(&path)),
        })
    }

    /// Не нашли ключ — пробуем завести свой. `AlreadyExists` значит, что
    /// другой процесс выиграл гонку между нашей проверкой и записью (Critical
    /// 2 ревью задачи 1): перезаписывать его ключ нельзя — это стёрло бы все
    /// пароли, которые он уже успел зашифровать под своим ключом, — поэтому
    /// `create_new` атомарен одним системным вызовом (вместе с правами 0600
    /// на Unix, тем же вызовом, а не отдельным `chmod` после — иначе между
    /// записью и `chmod` было бы окно, в которое файл лежит с правами по
    /// умолчанию).
    fn create_key_file(path: &Path) -> Result<Zeroizing<[u8; 32]>, CreateKeyError> {
        use std::io::Write;
        let mut generated: Key<Aes256Gcm> =
            Key::<Aes256Gcm>::try_generate().map_err(|e| CreateKeyError::Failed(format!("не удалось получить случайный ключ: {e}")))?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = match opts.open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(CreateKeyError::AlreadyExists),
            Err(e) => return Err(CreateKeyError::Failed(format!("не удалось сохранить ключ шифрования в {}: {e}", path.display()))),
        };
        if let Err(e) = f.write_all(generated.as_slice()) {
            return Err(CreateKeyError::Failed(format!("не удалось сохранить ключ шифрования в {}: {e}", path.display())));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(generated.as_slice());
        // `generated` — обычный `GenericArray`, не `Zeroizing`: без этого он
        // остался бы незатёртой второй копией ключа до конца функции (ревью
        // задачи 1, круг 3, пункт 2).
        generated.as_mut_slice().zeroize();
        Ok(Zeroizing::new(key))
    }

    enum CreateKeyError {
        AlreadyExists,
        Failed(String),
    }

    /// Ключ для шифрования: читаем существующий или заводим новый. Каталог —
    /// через `ensure_private_dir` (0700 на создании вместо umask обычного
    /// `create_dir_all`) и затем безусловно `tighten_dir`, как делает
    /// `publication::ensure_pub_dir`: на стенде три рабочих места делят один
    /// диск, и унаследованные права группы/остальных на этом каталоге — путь,
    /// которым соседний пользователь мог прочитать или подменить
    /// `secret.key` (ревью задачи 1, круг 3, пункт 1). `tighten_dir` при этом
    /// только снимает биты группы/остальных и не поднимает владельца — тест
    /// на каталог с суженными человеком правами (симуляция внешнего отказа)
    /// остаётся в силе: см. комментарий у `common::fsperm::tighten_dir`.
    /// Отказывает ровно здесь, а не в конструкторе `Store::file` — узнать об
    /// этом раньше первого реального шифрования негде и не нужно.
    fn load_or_create_key(dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
        fsperm::ensure_private_dir(dir)?;
        fsperm::tighten_dir(dir)?;
        let path = key_path(dir);
        match create_key_file(&path) {
            Ok(key) => Ok(key),
            Err(CreateKeyError::AlreadyExists) => read_key_bytes(&path).map_err(|e| e.describe(&path)),
            Err(CreateKeyError::Failed(e)) => Err(e),
        }
    }

    pub fn seal(dir: &Path, field: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
        let key = load_or_create_key(dir)?;
        let cipher = cipher_from(&key);
        // Случайный на каждый вызов — повтор секрета не выдаёт себя
        // одинаковым шифртекстом. `try_generate`, а не `generate`: отказ
        // генератора случайных чисел — именованный отказ (`Broken` по духу,
        // здесь — `String`), а не паника и падение процесса (ревью задачи 1,
        // Minor 6).
        let nonce = Nonce::try_generate().map_err(|e| format!("не удалось получить случайный nonce: {e}"))?;
        // Имя поля — связанные данные (AAD): шифртекст, созданный для одного
        // поля, не расшифруется под именем другого (Minor 7).
        let ciphertext = cipher.encrypt(&nonce, Payload { msg: plain, aad: field }).map_err(|_| "шифрование не удалось".to_string())?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn open(dir: &Path, field: &[u8], sealed: &[u8]) -> Result<Vec<u8>, OpenError> {
        if sealed.len() < NONCE_LEN {
            // Короче, чем nonce, — не отличить от чужого/урезанного
            // шифртекста, и называть это битым файлом запрещено спекой
            // (ревью задачи 1, Important 5): `Foreign`, а не `Broken`.
            return Err(OpenError::Foreign);
        }
        let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_LEN);
        let key = read_key(dir)?;
        let cipher = cipher_from(&key);
        // Длина `nonce_bytes` уже проверена выше (ровно `NONCE_LEN`) —
        // `try_from` здесь не может отказать по размеру; отказ всё равно
        // `Foreign`, а не `Broken`, по той же причине, что и выше.
        let nonce = Nonce::try_from(nonce_bytes).map_err(|_| OpenError::Foreign)?;
        // Неверный тег AEAD или неверное AAD (чужой ключ, чужая машина,
        // подмена байт, подмена имени поля) — всё это неотличимо друг от
        // друга и от «ключа никогда не было», поэтому `Foreign`, а не
        // `Broken` (спека, «любой отказ расшифровки»).
        cipher.decrypt(&nonce, Payload { msg: ciphertext, aad: field }).map_err(|_| OpenError::Foreign)
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
    use zeroize::Zeroize;

    /// Затирает буфер, который выделил сам DPAPI (`LocalAlloc`), прежде чем отдать его системе через `LocalFree` — `bytes` уже своя копия, `output.pbData` дальше не нужен никому, кроме освобождения, и без этого шифртекст (`seal`) или открытый пароль (`open`) остаются лежать в куче процесса неопределённо долго после `LocalFree` (Minor ревью задачи 7 — файловая подложка бьётся за это же везде, у DPAPI не было).
    unsafe fn zero_local_alloc(output: &CRYPT_INTEGER_BLOB) {
        if !output.pbData.is_null() {
            std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize).zeroize();
        }
    }

    /// Имя поля — `pOptionalEntropy`: тот же механизм связанных данных, что
    /// AAD у AEAD-подложки (Minor 7 ревью задачи 1) — симметрично по смыслу,
    /// разным API DPAPI и AEAD.
    fn entropy_blob(field: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: field.len() as u32, pbData: field.as_ptr() as *mut u8 }
    }

    pub fn seal(field: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let entropy = entropy_blob(field);
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: `input`/`entropy` живут до конца вызова (мы их не двигаем),
        // DPAPI сам выделяет `output.pbData` через `LocalAlloc` — освобождаем
        // его сами через `LocalFree`, как требует документация функции.
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        if ok == 0 {
            return Err(format!("DPAPI отказался шифровать значение (код {})", unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe {
            zero_local_alloc(&output);
            LocalFree(output.pbData as _);
        }
        Ok(bytes)
    }

    pub fn open(field: &[u8], sealed: &[u8]) -> Result<Vec<u8>, OpenError> {
        let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
        let entropy = entropy_blob(field);
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            // Чужой пользователь, чужая машина, испорченный блоб, не то имя
            // поля — DPAPI не говорит, какой из случаев, и мы тоже не называем
            // «битый файл» (спека, «Анти-цели»): это всегда `Foreign`.
            return Err(OpenError::Foreign);
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe {
            zero_local_alloc(&output);
            LocalFree(output.pbData as _);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Имя поля для тестов, которым важен сам факт шифрования, а не то,
    /// какое именно поле; тесты про смешивание полей заводят два разных
    /// имени явно.
    const FIELD: &str = "password";

    /// Временный каталог, который убирает себя сам (Minor 8 ревью задачи 1):
    /// прежняя версия `tmp()` каталоги не чистила, а `readonly`-тест оставлял
    /// их с правами 0500. Права возвращаются к 0700 перед удалением — права
    /// самого каталога не мешают `remove_dir_all` его содержимого на всех
    /// платформах одинаково, а восстановить их дешевле, чем гадать.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir().join(format!("secrets-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TmpDir(d)
        }
    }

    impl std::ops::Deref for TmpDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> TmpDir {
        TmpDir::new(name)
    }

    #[test]
    fn a_value_without_the_prefix_comes_back_untouched() {
        let dir = tmp("plain");
        let s = Store::file(&dir);
        assert_eq!(s.open(FIELD, "открытый пароль").unwrap(), "открытый пароль");
        assert_eq!(s.open(FIELD, "").unwrap(), "");
    }

    #[test]
    fn an_empty_secret_is_not_sealed() {
        let dir = tmp("empty");
        let s = Store::file(&dir);
        assert_eq!(s.seal(FIELD, "").unwrap(), "", "пустое значение обязано остаться пустым");
    }

    #[test]
    fn a_sealed_value_carries_the_prefix_and_not_the_secret() {
        let dir = tmp("seal");
        let s = Store::file(&dir);
        let sealed = s.seal(FIELD, "пароль 1С").unwrap();
        assert!(sealed.starts_with(PREFIX), "{sealed}");
        assert!(!sealed.contains("пароль"), "секрет виден в шифртексте: {sealed}");
        assert_eq!(s.open(FIELD, &sealed).unwrap(), "пароль 1С");
    }

    #[test]
    fn a_secret_with_quotes_and_cyrillic_survives_the_round_trip() {
        let dir = tmp("round");
        let s = Store::file(&dir);
        for plain in [r#"па"роль"#, r"C:\x\y", "пароль 1С", "p w", "ПРОБЕЛ в конце "] {
            let sealed = s.seal(FIELD, plain).unwrap();
            assert_eq!(s.open(FIELD, &sealed).unwrap(), plain, "не сошлось на {plain:?}");
        }
    }

    #[test]
    fn the_same_secret_seals_differently_twice() {
        // Случайный nonce: одинаковый шифртекст выдал бы, что пароль не менялся.
        let dir = tmp("nonce");
        let s = Store::file(&dir);
        assert_ne!(s.seal(FIELD, "секрет").unwrap(), s.seal(FIELD, "секрет").unwrap());
    }

    #[test]
    fn a_foreign_ciphertext_is_named_foreign_not_broken() {
        let mine_dir = tmp("mine");
        let theirs_dir = tmp("theirs");
        let mine = Store::file(&mine_dir);
        let theirs = Store::file(&theirs_dir);
        let sealed = mine.seal(FIELD, "секрет").unwrap();
        assert!(matches!(theirs.open(FIELD, &sealed), Err(OpenError::Foreign)));
    }

    #[test]
    fn a_ciphertext_without_a_key_is_foreign_too() {
        let dir = tmp("nokey");
        let sealed = Store::file(&dir).seal(FIELD, "секрет").unwrap();
        std::fs::remove_file(dir.join("secret.key")).unwrap();
        assert!(matches!(Store::file(&dir).open(FIELD, &sealed), Err(OpenError::Foreign)));
    }

    /// Minor 7: переставленные местами значения двух полей не расшифровать
    /// друг под другом — имя поля подмешано как связанные данные, и подмена
    /// неотличима от чужого шифртекста.
    #[test]
    fn a_value_sealed_for_one_field_does_not_open_under_another() {
        let dir = tmp("fields");
        let s = Store::file(&dir);
        let sealed = s.seal("password", "секрет рабочего места").unwrap();
        assert!(matches!(s.open("db_password", &sealed), Err(OpenError::Foreign)));
        assert_eq!(s.open("password", &sealed).unwrap(), "секрет рабочего места");
    }

    /// Minor 8: ветка `Broken` не была покрыта вовсе — ключ на диске не той
    /// длины — не про чужой шифртекст, а про то, что свой ключ повреждён.
    #[test]
    fn a_key_file_of_the_wrong_length_is_broken_not_foreign() {
        let dir = tmp("badkey");
        std::fs::write(dir.join("secret.key"), "слишком короткий").unwrap();
        // Шифртекст синтаксически валиден (нужна только длина не короче
        // nonce) — до ключа дело дойти обязано, и споткнуться должно именно
        // на нём, а не раньше.
        let sealed = format!("{PREFIX}{}", hex::encode(&[0u8; 20]));
        assert!(matches!(Store::file(&dir).open(FIELD, &sealed), Err(OpenError::Broken(_))));
    }

    /// Minor 8: шифртекст короче nonce — тоже `Foreign` (Important 5), а не
    /// повод упасть на вычитании длины или объявить файл битым.
    #[test]
    fn a_ciphertext_shorter_than_the_nonce_is_foreign() {
        let dir = tmp("short");
        let sealed = format!("{PREFIX}{}", hex::encode(&[0u8; 4]));
        assert!(matches!(Store::file(&dir).open(FIELD, &sealed), Err(OpenError::Foreign)));
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
        let e = Store::file(&dir).seal(FIELD, "секрет").unwrap_err();
        assert!(e.contains("ключ") && e.contains(&dir.display().to_string()), "{e}");
    }

    // Обычным прогоном не идёт: запрет спеки («в тестах нельзя звать DPAPI») про то, что DPAPI привязан к пользователю машины, и набор тестов не должен зависеть от того, под кем он запущен. Круговой прогон `seal`+`open` в одном процессе этого не нарушает, поэтому он идёт отдельным шагом CI на Windows (`build.yml`, шаг с `-- --ignored`) и руками на машине человека.
    #[cfg(windows)]
    #[ignore = "зовёт DPAPI пользователя — отдельным шагом CI на Windows, не обычным прогоном"]
    #[test]
    fn the_system_store_on_windows_round_trips_through_dpapi() {
        let dir = tmp("dpapi");
        let s = Store::system(&dir).unwrap();
        let sealed = s.seal(FIELD, "пароль 1С").unwrap();
        assert!(!sealed.contains("пароль"));
        assert_eq!(s.open(FIELD, &sealed).unwrap(), "пароль 1С");
    }
}
