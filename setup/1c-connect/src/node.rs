//! Узел канала: личность этой машины и провод к другой.
//!
//! Канал — одно транспортное соединение iroh между двумя машинами
//! (`2026-11-01-connection-contract.md`, «Два слова, которые нельзя путать»).
//! Здесь только нижний слой: завести узел, назвать себя, принять входящее,
//! дозвониться. Что по проводу ездит — в `wire.rs`; кто к какой карточке
//! допущен — выше.

use iroh::endpoint::{presets, Connection};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use zeroize::Zeroizing;

/// ALPN — ровно это, без номера версии (решение спеки, раздел «Версии»).
/// Номер в ALPN давал бы отказ на уровне транспорта: соединение просто не
/// состоялось бы — без слов, без причины и без возможности сказать человеку,
/// какую машину обновлять. Версия договаривается первым сообщением
/// (`wire::Msg::Hello`, поля `proto_min`/`proto_max`) именно затем, чтобы
/// отказ был словами и дошёл до окна обеих сторон.
pub const ALPN: &[u8] = b"1c-connect";

/// Файл ключа узла в каталоге состояния.
///
/// Ключ — личность машины: по открытой его половине машину зачисляют к
/// карточкам на той стороне, и отпечаток этого ключа человек сверяет вслух по
/// телефону перед тем, как нажать «Разрешить» (решение протокола №7). Потеряв
/// файл, человек теряет ВСЕ свои зачисления и обходит всех партнёров заново —
/// поэтому ключ лежит в состоянии рядом с остальным, что программа пишет сама
/// (`Kit::state`), а не заводится заново на каждый запуск.
///
/// В файле лежит ЗАПЕЧАТАННОЕ значение, тем же хранилищем, что и пароли
/// (`secrets::Store`; на Windows это DPAPI, привязанный к учётной записи).
/// Довод сильнее, чем у пароля базы: кто забрал ключ, тот выдаёт себя за эту
/// машину перед всеми, кто её уже разрешил, а второго рубежа у нас нет —
/// личность в этом протоколе доказывает сам iroh и ничего сверх этого не
/// предъявляется.
const KEY_FILE: &str = "node.key";

/// Имя поля для хранилища: оно же подмешивается в связанные данные, так что
/// шифртекст ключа узла не откроется под именем пароля базы и наоборот
/// (`secrets::Store::seal`).
const KEY_FIELD: &str = "node_key";

/// Узел этой машины. Клонируется дёшево (внутри `iroh::Endpoint`, он сам за
/// `Arc`): принимающей половине и зовущей нужен один и тот же узел.
#[derive(Debug, Clone)]
pub struct Node {
    endpoint: Endpoint,
}

impl Node {
    /// Поднимает узел, взяв ключ из состояния или заведя его при первом
    /// запуске.
    ///
    /// Пресет `N0`: релеи и поиск адреса по ключу через серверы n0. Релей —
    /// запасной путь, когда прямой через NAT не пробивается; без него канал
    /// держался бы только там, где обе машины видны друг другу напрямую, а это
    /// не тот случай, под который писался протокол.
    pub async fn start(state_dir: &Path) -> Result<Self, String> {
        let secret = key(state_dir)?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| format!("не удалось открыть узел: {e}. Проверьте, не запрещает ли брандмауэр исходящие соединения UDP."))?;
        Ok(Self { endpoint })
    }

    /// Кто мы на проводе: открытый ключ. Это он записывается в карточку при
    /// разрешении и он же показывается человеку отпечатком.
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// То, что кладётся в тикет: кто (ключ) и как достать (прямые адреса и
    /// релей).
    ///
    /// Отдельного типа не заводим — это ровно `EndpointAddr`, он же и
    /// сериализуется. Релей попадает сюда только после того, как узел вышел в
    /// сеть (`Endpoint::online`); до того тикет несёт прямые адреса, и по нему
    /// дозвонится та машина, что в одной сети с этой.
    pub fn ticket_parts(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Ждёт входящего канала. `None` — узел закрыт, принимать больше нечего;
    /// это конец цикла приёма, а не беда.
    pub async fn accept(&self) -> Option<Result<Connection, String>> {
        let incoming = self.endpoint.accept().await?;
        Some(incoming.await.map_err(|e| format!("входящее подключение не состоялось: {e}")))
    }

    /// Зовёт ту сторону. Принимает и один `EndpointId` (адрес ищется по ключу
    /// через серверы n0), и полный `EndpointAddr` из тикета — во втором случае
    /// поиск не нужен вовсе, и так соединяются две машины в одной сети и два
    /// узла в одном процессе.
    pub async fn dial(&self, to: impl Into<EndpointAddr>) -> Result<Connection, String> {
        self.endpoint.connect(to, ALPN).await.map_err(|e| {
            format!("не удалось дозвониться до той стороны: {e}. Проверьте, запущена ли программа там и жив ли код подключения.")
        })
    }

    /// Закрывает узел, дав той стороне узнать об уходе. Без этого она узнала
    /// бы о закрытии по тишине, то есть не раньше тайм-аута.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }

    /// Для слоёв выше: им нужен сам `Endpoint` — открыть поток, спросить о
    /// том конце, подождать выхода в сеть.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

fn key_path(state_dir: &Path) -> PathBuf {
    state_dir.join(KEY_FILE)
}

/// Хранилище секретов для ключа узла.
///
/// `#[cfg(test)]`-развилка — тот же приём, что у `gui::save::store_for`: спека
/// секретов прямо запрещает звать DPAPI из тестов (он привязан к пользователю
/// машины, а набор гоняется и на `windows-latest` в CI). Подложка файла в
/// тестах проверяет настоящий путь шифрования, а не притворяется.
#[cfg(test)]
fn store_for(state_dir: &Path) -> Result<crate::secrets::Store, String> {
    Ok(crate::secrets::Store::file(state_dir))
}

#[cfg(not(test))]
fn store_for(state_dir: &Path) -> Result<crate::secrets::Store, String> {
    crate::secrets::Store::system(state_dir)
}

/// Ключ из состояния, а если файла НЕТ ВОВСЕ — новый, тут же запечатанный и
/// записанный.
///
/// Разделение «файла нет» и «файл есть, но непригоден» здесь главное.
/// Непригодный файл (пустой, обрезанный, не открывающийся, не разбирающийся)
/// — это ОТКАЗ СЛОВАМИ, а не повод завести новую личность: молча заведённый
/// ключ отменил бы все зачисления разом, и человек узнал бы об этом не
/// здесь, а по тому, что его перестали пускать, — и пошёл бы искать беду на
/// той стороне.
///
/// Пустой файл — не выдуманный случай: запись ниже сознательно не зовёт
/// `sync_all`, а NTFS журналирует переименование отдельно от данных, так что
/// крах питания оставляет ровно нулевой файл.
fn key(state_dir: &Path) -> Result<SecretKey, String> {
    let store = store_for(state_dir)?;
    let path = key_path(state_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => from_file(&store, &path, &text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => make(&store, &path),
        Err(e) => Err(format!(
            "Ключ этой машины не прочитался.\nФайл {} есть, но его не удалось открыть: {e}\nПроверьте права на каталог и повторите запуск; файл \
             не удаляйте — в нём личность этой машины.",
            path.display()
        )),
    }
}

/// Разбор того, что лежит в файле. Любая неудача — отказ, и в каждом тексте
/// есть третья строка: что человеку делать и чем это обойдётся.
fn from_file(store: &crate::secrets::Store, path: &Path, text: &str) -> Result<SecretKey, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!(
            "Ключ этой машины пуст.\nФайл {} есть, но в нём ничего нет — так бывает после краха питания в тот миг, когда ключ записывался.\
             \nЕсли у вас есть копия этого файла, верните её. Если нет — удалите файл: программа заведёт новую личность, но зачисления, \
             выданные под старым ключом, недействительны, и каждую сторону придётся разрешить заново.",
            path.display()
        ));
    }
    // Значение без признака шифрования `Store::open` отдаёт как есть (то же
    // правило, по которому читаются пароли в toml). Для ключа узла этого
    // мало: в первых сборках ветки он ложился открытым текстом, и остаться
    // таким навсегда он не должен — запечатываем и перезаписываем на месте.
    let plain = !text.starts_with(crate::secrets::PREFIX);
    let hex = store.open(KEY_FIELD, text).map(Zeroizing::new).map_err(|e| match e {
        crate::secrets::OpenError::Foreign => format!(
            "Ключ этой машины не открывается.\nОн запечатан для другой учётной записи или другой машины — так бывает, когда комплект \
             скопировали вместе с файлом {}.\nЕсли это та же машина и та же учётная запись, не удаляйте файл и сообщите нам. Если комплект \
             переехал — удалите файл: программа заведёт новую личность, но зачисления, выданные под старым ключом, недействительны, и каждую \
             сторону придётся разрешить заново.",
            path.display()
        ),
        crate::secrets::OpenError::Broken(why) => format!(
            "Ключ этой машины не прочитался.\nПодвело не само значение, а чтение: {why}\nПовторите запуск; если повторяется — сообщите нам \
             вместе с журналом, файл {} не удаляйте.",
            path.display()
        ),
    })?;
    let secret = SecretKey::from_str(hex.trim()).map_err(|e| {
        format!(
            "Ключ этой машины не читается.\nОн открылся, но это не ключ узла: {e}\nУдалите файл {}, чтобы завести новый ключ, — но зачисления \
             этой машины придётся получить заново на каждой стороне, с которой вы работаете.",
            path.display()
        )
    })?;
    if plain {
        // По возможности, а не обязательно: ключ уже на руках и он рабочий,
        // так что отказать в запуске из-за неудавшейся ПЕРЕПЕЧАТКИ значило бы
        // лишить человека работы ради улучшения, которое повторится при
        // следующем запуске.
        if let Ok(sealed) = store.seal(KEY_FIELD, &hex) {
            let _ = replace_sealed(path, &sealed);
        }
    }
    Ok(secret)
}

/// Новая личность. Пишется `create_new` прямо в целевой файл: так тот, кто
/// проиграл гонку двух одновременных экземпляров, не затирает чужой ключ
/// своим, а берёт тот, что уже лежит. Через общий `node.key.tmp` эта гонка
/// кончалась бы двумя личностями, из которых одна тихо пропадала.
fn make(store: &crate::secrets::Store, path: &Path) -> Result<SecretKey, String> {
    let secret = SecretKey::generate();
    let hex = Zeroizing::new(crate::secrets::hex::encode(&secret.to_bytes()));
    let sealed = store.seal(KEY_FIELD, &hex).map_err(|e| {
        format!(
            "Ключ этой машины не удалось запечатать.\nХранилище секретов отказало: {e}\nПовторите запуск; класть ключ открытым текстом нельзя \
             — это личность машины, и по ней её пускают к базам."
        )
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            format!(
                "Ключ этой машины некуда записать.\nКаталог {} не создался: {e}\nПроверьте права на него и повторите запуск.",
                dir.display()
            )
        })?;
    }
    match std::fs::File::create_new(path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(format!("{sealed}\n").as_bytes()).map_err(|e| {
                format!(
                    "Ключ этой машины не записался.\nЗапись в {} не удалась: {e}\nПроверьте место на диске и права на каталог, потом повторите \
                     запуск.",
                    path.display()
                )
            })?;
            Ok(secret)
        }
        // Другой экземпляр успел первым: его ключ и есть личность этой
        // машины, и спорить с ним нечем — зачисления выданы на него.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                format!(
                    "Ключ этой машины не прочитался.\nЕго только что записал другой запущенный экземпляр, но файл {} не открылся: {e}\nЗакройте \
                     лишние окна программы и повторите запуск.",
                    path.display()
                )
            })?;
            from_file(store, path, &text)
        }
        Err(e) => Err(format!(
            "Ключ этой машины не записался.\nФайл {} не создался: {e}\nПроверьте права на каталог и повторите запуск.",
            path.display()
        )),
    }
}

/// Перепечатка существующего файла: временный файл и переименование, чтобы
/// старое значение жило до последнего мига. Тот же приём, что у
/// `editors::write_and_rename`; `sync_all` не зовём по тому же доводу, что и
/// там — а последствие нулевого файла здесь закрыто отказом словами в
/// `from_file`, а не молчаливой генерацией.
fn replace_sealed(path: &Path, sealed: &str) -> Result<(), String> {
    let tmp = path.with_extension("key.tmp");
    std::fs::write(&tmp, format!("{sealed}\n")).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("не удалось переименовать {} в {}: {e}", tmp.display(), path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::time::Duration;

    /// Сколько ждём сеть в тесте. Не «на всякий случай побольше»: без предела
    /// упавший шаг выглядит как зависший прогон, и прогон стоит минут.
    const PATIENCE: Duration = Duration::from_secs(20);

    /// Свой каталог на каждый тест: ключ узла — файл, и тесты не должны
    /// отбирать его друг у друга. Тот же приём, что в `cfg.rs`, — без крейта
    /// `tempfile`.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("onec-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Адрес узла для звонка внутри процесса: его ключ и свой же порт на
    /// петле.
    ///
    /// Почему не `ticket_parts()`: iroh кладёт в адрес узла петлю только
    /// тогда, когда других сетевых интерфейсов нет вовсе (`socket.rs`,
    /// «Include loopback addresses only if there are no other interfaces»), а
    /// на рабочей машине они есть — и звонок пошёл бы на свой же адрес в
    /// локальной сети, то есть через брандмауэр. Это проверяло бы настройки
    /// Windows, а не наш узел: тест с `ticket_parts()` на этой машине висел,
    /// пока прямой путь не пробивался, а релей был недостижим. Петля не
    /// фильтруется никогда, и путь по ней — тот же прямой путь iroh.
    fn by_loopback(node: &Node) -> EndpointAddr {
        let port = node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find_map(|s| if s.is_ipv4() { Some(s.port()) } else { None })
            .expect("узел обязан был занять порт IPv4");
        EndpointAddr::new(node.id()).with_ip_addr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    #[tokio::test]
    async fn two_nodes_in_one_process_meet_by_node_id() {
        let (da, db) = (dir("meet-a"), dir("meet-b"));
        let a = Node::start(&da).await.unwrap();
        let b = Node::start(&db).await.unwrap();
        let to = by_loopback(&b);
        assert_eq!(to.id, b.id(), "звоним по ключу узла, а не по чему-то ещё");
        assert_eq!(b.ticket_parts().id, b.id(), "в тикет уезжает тот же ключ");

        let listening = b.clone();
        let heard = tokio::spawn(async move {
            let conn = tokio::time::timeout(PATIENCE, listening.accept()).await.unwrap().unwrap().unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let mut byte = [0u8; 1];
            recv.read_exact(&mut byte).await.unwrap();
            send.write_all(&[byte[0] + 1]).await.unwrap();
            send.finish().unwrap();
            // Соединение отдаём наружу живым: брошенное здесь закрыло бы
            // канал раньше, чем та сторона дочитает ответ.
            (byte[0], conn)
        });

        let conn = tokio::time::timeout(PATIENCE, a.dial(to)).await.expect("дозвон не уложился в срок").unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(&[7]).await.unwrap();
        send.finish().unwrap();
        let mut back = [0u8; 1];
        recv.read_exact(&mut back).await.unwrap();
        assert_eq!(back[0], 8, "та сторона должна была ответить");
        let (heard_byte, _held) = tokio::time::timeout(PATIENCE, heard).await.expect("та сторона не ответила в срок").unwrap();
        assert_eq!(heard_byte, 7, "та сторона должна была услышать наш байт");
        a.close().await;
        b.close().await;
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
    }

    #[tokio::test]
    async fn the_node_key_survives_a_restart() {
        let d = dir("restart");
        let first = Node::start(&d).await.unwrap();
        let was = first.id();
        first.close().await;
        drop(first);

        let again = Node::start(&d).await.unwrap();
        assert_eq!(again.id(), was, "ключ узла — личность машины, перезапуск её не меняет");
        assert_eq!(again.ticket_parts().id, was, "отпечаток и тикет считаются от открытого ключа, а не от файла");
        again.close().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_key_that_does_not_open_is_refused_in_words_not_replaced_in_silence() {
        let d = dir("sealed");
        let node = Node::start(&d).await.unwrap();
        node.close().await;

        let on_disk = std::fs::read_to_string(key_path(&d)).unwrap();
        assert!(on_disk.trim().len() > 64, "ключ на диске запечатан, а не лежит голым hex (голый — ровно 64 знака): {on_disk}");

        // Комплект унесли на другую машину или открыли под другой учётной
        // записью: запечатанное значение то же, а ключ хранилища другой.
        std::fs::write(d.join("secret.key"), [7u8; 32]).unwrap();
        let no = Node::start(&d).await.unwrap_err();
        assert!(no.contains("зачислени") || no.contains("разрешен"), "отказ обязан сказать, что станет с зачислениями: {no}");
        assert_eq!(
            std::fs::read_to_string(key_path(&d)).unwrap(),
            on_disk,
            "новая личность молча не заводится: это отменило бы все зачисления, а человек узнал бы об этом по тому, что его перестали пускать"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Пустой файл достижим не только рукой: запись без `sync_all` плюс
    /// журналируемое отдельно переименование NTFS дают ровно нулевой файл
    /// после краха питания. Молча перезаписать его значит отменить все
    /// зачисления человека — то самое, чего `key` обещает не делать.
    #[tokio::test]
    async fn an_empty_key_file_is_refused_not_overwritten() {
        let d = dir("empty-key");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(key_path(&d), "").unwrap();

        let no = Node::start(&d).await.unwrap_err();
        assert!(no.contains("пуст"), "отказ обязан назвать, что с файлом: {no}");
        assert!(no.contains("разрешить заново") || no.contains("зачислени"), "и сказать, чем обойдётся новая личность: {no}");
        assert_eq!(std::fs::read_to_string(key_path(&d)).unwrap(), "", "пустой файл не перезаписывается");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Ключ, однажды легший открытым текстом (так он лежал в первом коммите
    /// ветки), не остаётся таким навсегда: следующий запуск его запечатывает,
    /// не меняя личности.
    #[tokio::test]
    async fn a_plain_key_is_sealed_on_the_next_start() {
        let d = dir("plain-key");
        std::fs::create_dir_all(&d).unwrap();
        let plain = format!("{}07", "00".repeat(31));
        std::fs::write(key_path(&d), format!("{plain}\n")).unwrap();

        let node = Node::start(&d).await.unwrap();
        let was = node.id();
        node.close().await;

        let on_disk = std::fs::read_to_string(key_path(&d)).unwrap();
        assert!(on_disk.starts_with(crate::secrets::PREFIX), "голый ключ перепечатан запечатанным: {on_disk}");
        assert!(!on_disk.contains(&plain), "открытого значения на диске больше нет");

        let again = Node::start(&d).await.unwrap();
        assert_eq!(again.id(), was, "перепечатка не меняет личность машины");
        again.close().await;
        let _ = std::fs::remove_dir_all(&d);
    }
}
