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
const KEY_FILE: &str = "node.key";

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

/// Ключ из состояния, а если его там нет — новый, тут же записанный.
///
/// Нечитаемый файл — это отказ, а не повод завести новый ключ молча: молча
/// заведённый ключ выглядит как потеря всех зачислений без причины, и человек
/// пошёл бы искать беду на той стороне.
fn key(state_dir: &Path) -> Result<SecretKey, String> {
    let path = key_path(state_dir);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if !text.is_empty() {
            return SecretKey::from_str(text).map_err(|e| {
                format!(
                    "ключ узла в {} не читается: {e}. Файл повреждён. Удалите его, чтобы завести новый ключ, — но зачисления этой машины \
                     придётся получить заново на каждой стороне, с которой вы работаете.",
                    path.display()
                )
            });
        }
    }
    let secret = SecretKey::generate();
    let hex: String = secret.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    common::kit::write_text(&path, &format!("{hex}\n")).map_err(|e| {
        format!("не удалось записать ключ узла в {}: {e}. Без записи личность машины менялась бы при каждом запуске.", path.display())
    })?;
    Ok(secret)
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
        let a = Node::start(&dir("meet-a")).await.unwrap();
        let b = Node::start(&dir("meet-b")).await.unwrap();
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
        again.close().await;
    }
}
