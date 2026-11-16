//! Управляющая дверь живого экземпляра: команда достукивается до того процесса, чей значок в трее, а не поднимает свой второй (закон 2 спеки `2026-11-15-pairing-belongs-to-the-back.md`, раздел 4).
//!
//! **Трудность, из которой дверь и выросла.** Узел, порты и книга принадлежат одному процессу на комплект (`single_instance`). Значит `1c-connect allow …` не может завести свой узел: ему нечем ни принять поток, ни дописать книгу, пока их держит живой. Дверь — тот способ достучаться; `status` её первый клиент, потому что он ничего не меняет.
//!
//! **Почему именно именованный канал, а не то, что в программе уже есть.** Межпроцессная просьба показать окно (`single_instance::serve_show_requests`) сделана именованным СОБЫТИЕМ, и событие без атрибутов безопасности видно только внутри сеанса создателя. Для двери это не годится, и причина не вкусовая: отказ 15.11, сжёгший владельцу рабочий день, состоял ровно в том, что бек жил в одном сеансе (ssh), а человек приходил из другого. Дверь обязана работать между сеансами ОДНОЙ учётной записи, и именованный канал Windows сеансом не ограничен. На Linux — сокет в каталоге состояния (`Kit::state`), рядом с замком: порт на петле спека запрещает отдельным доводом (он открыт любому процессу машины, и продукт уже тратит силы, доказывая, кто его держит, — `holder::of_port`).
//!
//! **Имя двери считается тем же `single_instance::guard_key`, что и ключ замка.** У замка и двери один вопрос — «чья это копия», — и ответ обязан быть один: второе правило чистки имени разошлось бы с первым при следующей правке. Чужая учётная запись на той же машине стучится в ДРУГУЮ дверь и нашей не видит.
//!
//! **Кадр — тот же `wire::Frame`**, что возит сообщения по каналу между машинами: второго способа упаковки в продукте не заводим (решение ведущего). Поэтому `Frame` и стал общим по полезной нагрузке — им возятся и `wire::Msg`, и `Ask`/`Answer`.
//!
//! **Дверь не поднимает бека.** Клиент, не нашедший живого, говорит слова (`NO_BACK_END`) и уходит с ненулевым кодом: иначе человек получил бы вторую, невидимую копию продукта — ровно то, что весь день 15.11 мешало открыть настоящее окно.

use crate::single_instance::{current_user, guard_key};
use crate::wire::{self, Frame};
use common::kit::Kit;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// Слова отказа — дословно из спеки (раздел 4): «бек не запущен — поднимите `1c-connect serve` или откройте окно». Живут один раз и здесь: их говорит `ask` всякому клиенту, и их же печатает `status`, объясняя, почему показанное выше прочитано с диска.
pub const NO_BACK_END: &str = "бек не запущен — поднимите 1c-connect serve или откройте окно";

/// Сколько клиент ждёт ответа. Живой отвечает из памяти и мгновенно, так что срок здесь — не про скорость, а про то, чтобы зависший бек не держал набранную команду навсегда: `Frame::read` нарочно без своего срока (см. его докстроку), срок ставит вызывающий.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// О чём клиент спрашивает живого. Задачи 6–9 плана добавят сюда свои случаи (`code`, `dial`, `requests`, `allow`, `decline`, `peers`, `revoke`); `Status` первый, потому что ничего не меняет.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ask {
    Status,
}

/// Что живой отвечает. Случай на случай с `Ask`: вопрос без ответа и ответ без вопроса этой дверью не ходят.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Status(StatusView),
}

/// То, что о себе рассказывает живой экземпляр, и ТОЛЬКО это: обслуживает ли он сейчас и что со связью по карточкам. Ни одного поля про запас — задача 9 дополнит вид, когда `status` правда начнёт печатать больше; дополнять пустое дешевле, чем выпиливать лишнее.
///
/// Чего здесь нет нарочно: всего, что `status` и так читает с диска (комплект, соединение, находки разбора, таблица фактов). Дверь нужна ровно для того, чего на диске НЕТ.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    /// Поднято ли обслуживание вызовов агента прямо сейчас.
    pub serving: bool,
    /// Карточки, которым есть что сказать о связи. Карточка без состояния контракта в список не попадает вовсе: слово «не подключено» ставит строке экран (`gui::link::row`), и второй его копии здесь не будет.
    pub cards: Vec<CardState>,
}

/// Одна карточка словами: как человек её назвал и что сейчас со связью. Слово — из `contract::contract_word` и ниоткуда больше; складывает его тот, кто дверь держит, а не дверь.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardState {
    pub name: String,
    pub word: String,
}

/// Сторона бека: держит приём, пока жива. Клонируется ручкой на ту же дверь — держателю нужно и оставить её на стеке, и отдать копию в замыкание, которое рассказывает ей о новостях (`show`).
#[derive(Clone)]
pub struct Door {
    view: Arc<Mutex<StatusView>>,
}

impl Door {
    /// Рассказать двери, что отвечать. Дверь — почтовый ящик, а не дознаватель: спрашивать живое состояние ей негде (у окна оно внутри GPUI, у `serve` — в замыкании сеанса), поэтому держатель сам кладёт сюда снимок всякий раз, когда тот меняется.
    pub fn show(&self, view: StatusView) {
        // Отравленный замок — не повод валить ответ: внутри только снимок, и прочитать его после чужой паники не опаснее, чем до.
        *self.view.lock().unwrap_or_else(|e| e.into_inner()) = view;
    }

    fn snapshot(&self) -> StatusView {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Имя двери — единственное место, где оно строится (тот же довод, что у `single_instance::instance_mutex_name`: вторая формулировка смотрела бы не на тот объект). Ключ — `guard_key`, тот же, что у замка.
pub fn door_name(user: &str) -> String {
    format!("1c-connect-door-{}", guard_key(user))
}

/// Открыть дверь — сторона бека. Зовут оба варианта `Start` (`gui::launch`) и `fn serve` в сборке без окна: дверь есть у всякого живого экземпляра, потому что клиенты у всех одни.
///
/// Держите возвращённую `Door` до конца процесса: уйдёт она со стека — ящик останется жив (его держит поток приёма), но рассказывать ему о новостях будет некому.
pub fn open(kit: &Kit) -> io::Result<Door> {
    open_at(&door_name(&current_user()), kit)
}

/// Открыть дверь и пережить отказ: слова в журнал и на экран, `None` — двери нет. Отказ двери не смертелен ни для окна, ни для `serve` — обслуживание вызовов агента от неё не зависит, — но молчать о нём нельзя: человек, набравший `1c-connect status`, получил бы «бек не запущен» при живом беке и не узнал бы, почему.
pub fn open_or_say(kit: &Kit) -> Option<Door> {
    match open(kit) {
        Ok(d) => Some(d),
        Err(e) => {
            let text = format!(
                "управляющая дверь не открылась: {e}. Обслуживание и окно работают, но набранные команды до этого экземпляра не \
                 достучатся — остановите его и запустите снова."
            );
            crate::journal::write(kit, crate::journal::Kind::Fail, &text);
            eprintln!("{text}");
            None
        }
    }
}

/// Спросить живого — сторона клиента. Своего бека не поднимает и ничем не владеет: нет живого — `NO_BACK_END` словами.
pub fn ask(kit: &Kit, q: Ask) -> Result<Answer, String> {
    ask_at(&door_name(&current_user()), kit, q)
}

/// Отдельный рантайм на каждую сторону двери: клиент — набранная команда, у которой своего рантайма нет вовсе, а бек открывает дверь до того, как поднимет сеанс (у окна рантайма на главном потоке не будет никогда — там GPUI).
fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build()
}

/// Ответ на один стук. Разговор — ровно один вопрос и ровно один ответ: держать канал открытым между вопросами нечего, а закрытый канал на той стороне `Frame::read` отличает от обрыва словами (`code::CLOSED`).
async fn answer<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, door: &Door) {
    let q: Ask = match Frame::read(stream).await {
        Ok(q) => q,
        // Молчание и мусор в дверь — не событие для человека: стукнуть в неё может что угодно, а отвечать мы умеем только своим.
        Err(_) => return,
    };
    let a = match q {
        Ask::Status => Answer::Status(door.snapshot()),
    };
    let _ = Frame::write(stream, &a).await;
}

/// Слова на отказ самого разговора. Словарь `wire::Refuse` — словарь СОПРЯЖЕНИЯ, и его текст говорит про «тот компьютер»; в двери на том конце не тот компьютер, а свой же процесс, поэтому наружу идут наши слова, а чужие остаются в хвосте для диагностики.
fn not_answered(r: &wire::Refuse) -> String {
    format!("живой экземпляр не ответил: {}", r.text.as_str())
}

/// Отказ, который значит «двери нет»: на Windows канала с таким именем не существует (`NotFound`), на Linux сокет есть, а слушателя за ним нет (`ConnectionRefused`) либо файла нет вовсе.
fn no_door(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

#[cfg(windows)]
type Knock = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(not(windows))]
type Knock = tokio::net::UnixStream;

/// Путь именованного канала — имя двери в пространстве имён каналов этой машины.
#[cfg(windows)]
fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

/// Путь сокета: имя двери файлом в каталоге состояния комплекта — «адрес двери в каталоге состояния, рядом с замком» (спека, раздел 4).
#[cfg(not(windows))]
fn sock_path(name: &str, kit: &Kit) -> std::path::PathBuf {
    kit.state_path(&format!("{name}.sock"))
}

/// Приём — своим потоком со своим рантаймом (тот же приём и та же причина, что у узла и сеанса, см. `peer.rs`): у окна главный поток занят GPUI, а у `serve` — ожиданием Ctrl+C, и ждать стука там нельзя ни тому, ни другому.
///
/// Стуки обслуживаются ПО ОДНОМУ. ponytail: клиентов у двери столько, сколько команд человек набрал руками, а ответ — копия снимка из памяти; потолок — очередь на время одного ответа, и поднимать её в задачу на стук незачем. Срок на разговор при этом обязателен: клиент, открывший канал и замолчавший, иначе запер бы дверь навсегда (`Frame::read` нарочно без своего срока).
#[cfg(windows)]
fn open_at(name: &str, _kit: &Kit) -> io::Result<Door> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let path = pipe_path(name);
    let rt = runtime()?;
    // Слушатель заводится ЗДЕСЬ, до возврата, а не внутри потока: отказ — занятое имя, запрет прав — обязан достаться тому, кто дверь открывал, а не пропасть в чужом потоке. Тот же довод, что у `single_instance::serve_show_requests`, где дескриптор события заводится до запуска потока.
    let mut server = {
        let _in_rt = rt.enter();
        ServerOptions::new().first_pipe_instance(true).create(&path)?
    };
    let door = Door { view: Arc::new(Mutex::new(StatusView::default())) };
    let mine = door.clone();
    std::thread::spawn(move || {
        rt.block_on(async move {
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                // Следующий экземпляр канала — сразу: имя обязано остаться занятым, пока мы заняты этим стуком, иначе клиент, пришедший в эту долю секунды, прочёл бы «бек не запущен» при живом беке.
                let Ok(next) = ServerOptions::new().create(&path) else { return };
                let mut taken = std::mem::replace(&mut server, next);
                let _ = tokio::time::timeout(ASK_WAIT, answer(&mut taken, &mine)).await;
            }
        })
    });
    Ok(door)
}

/// То же, что на Windows, но сокетом в каталоге состояния — см. докстроку выше и заголовок модуля.
#[cfg(not(windows))]
fn open_at(name: &str, kit: &Kit) -> io::Result<Door> {
    use tokio::net::UnixListener;
    let path = sock_path(name, kit);
    // Остаток прошлого запуска: `bind` на существующем файле — отказ «адрес занят», и снять этот файл за нас некому — ядро сокет не убирает (в отличие от `flock`, который держится дескриптором и снимается с процессом). Живого хозяина за файлом быть не может: дверь открывает только бек, а бек на учётную запись один — замок (`single_instance::acquire`) взят раньше.
    let _ = std::fs::remove_file(&path);
    let rt = runtime()?;
    let listener = {
        let _in_rt = rt.enter();
        UnixListener::bind(&path)?
    };
    let door = Door { view: Arc::new(Mutex::new(StatusView::default())) };
    let mine = door.clone();
    std::thread::spawn(move || {
        rt.block_on(async move {
            loop {
                let Ok((mut taken, _)) = listener.accept().await else { return };
                let _ = tokio::time::timeout(ASK_WAIT, answer(&mut taken, &mine)).await;
            }
        })
    });
    Ok(door)
}

/// Сколько раз клиент перепробует занятый канал. Дверь отвечает по одному стуку и заводит следующий экземпляр канала сразу, так что занято бывает на доли секунды; ждать её ровно столько же, сколько ответа (`ASK_WAIT`), — честнее, чем отказать человеку из-за соседней команды.
#[cfg(windows)]
const BUSY_PAUSE: Duration = Duration::from_millis(50);

#[cfg(windows)]
async fn knock(name: &str, _kit: &Kit) -> io::Result<Knock> {
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    let path = pipe_path(name);
    let tries = (ASK_WAIT.as_millis() / BUSY_PAUSE.as_millis()).max(1);
    for _ in 1..tries {
        match ClientOptions::new().open(&path) {
            // «Занято» — не «двери нет»: все заведённые экземпляры канала сейчас разговаривают, и имя за дверью осталось.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => tokio::time::sleep(BUSY_PAUSE).await,
            other => return other,
        }
    }
    ClientOptions::new().open(&path)
}

#[cfg(not(windows))]
async fn knock(name: &str, kit: &Kit) -> io::Result<Knock> {
    tokio::net::UnixStream::connect(sock_path(name, kit)).await
}

fn ask_at(name: &str, kit: &Kit, q: Ask) -> Result<Answer, String> {
    let rt = runtime().map_err(|e| format!("живой экземпляр не спрошен: {e}"))?;
    rt.block_on(async {
        let mut stream = knock(name, kit).await.map_err(|e| {
            if no_door(&e) {
                NO_BACK_END.to_string()
            } else {
                format!("дверь живого экземпляра не отворилась на стук: {e}")
            }
        })?;
        Frame::write(&mut stream, &q).await.map_err(|r| not_answered(&r))?;
        match tokio::time::timeout(ASK_WAIT, Frame::read::<_, Answer>(&mut stream)).await {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(r)) => Err(not_answered(&r)),
            Err(_) => Err(format!(
                "живой экземпляр молчит дольше {} с: он занят или завис — остановите его и запустите снова",
                ASK_WAIT.as_secs()
            )),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Своё имя двери на каждый прогон: на машине может идти настоящая копия, и тест, занявший её имя, отнимал бы у неё приём команд (тот же довод, что у `single_instance::serve_show_requests_with`).
    fn test_door(tag: &str) -> String {
        format!("{}-test-{}-{tag}", door_name(&current_user()), std::process::id())
    }

    fn kit_for_test() -> (crate::project::tempdir::Dir, Kit) {
        let dir = crate::project::tempdir::Dir::new();
        let kit = Kit::at(dir.path());
        (dir, kit)
    }

    /// Двери нет — команда отвечает СЛОВАМИ и ничего не поднимает.
    ///
    /// Второй вопрос в том же тесте и проверяет «ничего не поднимает»: подними первый вопрос бека сам — и второй получил бы ответ, а не тот же отказ.
    #[test]
    fn a_closed_door_is_refused_in_words_and_nothing_is_started() {
        let (_dir, kit) = kit_for_test();
        let name = test_door("closed");
        assert_eq!(ask_at(&name, &kit, Ask::Status).unwrap_err(), NO_BACK_END);
        assert_eq!(ask_at(&name, &kit, Ask::Status).unwrap_err(), NO_BACK_END, "первый отказ не завёл бека тайком");
    }

    /// Имя двери принадлежит учётной записи: чужая на той же машине стучится в ДРУГУЮ дверь.
    #[test]
    fn the_door_is_named_after_the_account() {
        assert_ne!(door_name("ivan"), door_name("petr"));
        assert_eq!(door_name("Иван Петров"), door_name("Иван_Петров"), "чистка имени — та же, что у замка");
    }

    /// Спросили — ответили: круг «клиент → бек → клиент» целиком, без сети. Сверяется и то, что доехал сам снимок, а не только факт ответа: кадр двери возит значение, и подменённое поле здесь было бы видно.
    #[test]
    fn an_open_door_answers_the_one_who_knocks() {
        let (_dir, kit) = kit_for_test();
        let name = test_door("open");
        let door = open_at(&name, &kit).expect("дверь открылась");
        let shown =
            StatusView { serving: true, cards: vec![CardState { name: "Ромашка".into(), word: "подключено".into() }] };
        door.show(shown.clone());
        match ask_at(&name, &kit, Ask::Status) {
            Ok(Answer::Status(got)) => assert_eq!(got, shown, "ответ — тот снимок, что показал держатель"),
            other => panic!("живой обязан ответить: {other:?}"),
        }
    }

    /// Дверь отвечает не одному клиенту: `status` человек набирает когда хочет, и второй стук после первого обязан дойти так же.
    #[test]
    fn a_door_answers_every_knock_not_just_the_first() {
        let (_dir, kit) = kit_for_test();
        let name = test_door("twice");
        let door = open_at(&name, &kit).expect("дверь открылась");
        door.show(StatusView { serving: true, cards: vec![] });
        for n in 1..=3 {
            let Ok(Answer::Status(got)) = ask_at(&name, &kit, Ask::Status) else {
                panic!("стук {n} остался без ответа")
            };
            assert!(got.serving, "стук {n}");
        }
    }
}
