//! Управляющая дверь живого экземпляра: команда достукивается до того процесса, чей значок в трее, а не поднимает свой второй (закон 2 спеки `2026-11-15-pairing-belongs-to-the-back.md`, раздел 4).
//!
//! **Трудность, из которой дверь и выросла.** Узел, порты и книга принадлежат одному процессу на комплект (`single_instance`). Значит `1c-connect allow …` не может завести свой узел: ему нечем ни принять поток, ни дописать книгу, пока их держит живой. Дверь — тот способ достучаться; `status` её первый клиент, потому что он ничего не меняет.
//!
//! **Почему именно именованный канал, а не то, что в программе уже есть.** Межпроцессная просьба показать окно (`single_instance::serve_show_requests`) сделана именованным СОБЫТИЕМ, и событие без атрибутов безопасности видно только внутри сеанса создателя. Для двери это не годится, и причина не вкусовая: отказ 15.11, сжёгший владельцу рабочий день, состоял ровно в том, что бек жил в одном сеансе (ssh), а человек приходил из другого. Дверь обязана работать между сеансами ОДНОЙ учётной записи, и именованный канал Windows сеансом не ограничен. На Linux — сокет в каталоге состояния (`Kit::state`), рядом с замком: порт на петле спека запрещает отдельным доводом (он открыт любому процессу машины, и продукт уже тратит силы, доказывая, кто его держит, — `holder::of_port`).
//!
//! **Почему у двери явный дескриптор прав, а не умолчание.** Тем же абзацем спека отличает канал от порта на петле: «канал с правами учётной записи отвечает на вопрос „кто это“ средствами ОС, а не нашими догадками». Канал, созданный без дескриптора, этого не делает: умолчание пускает к нему больше, чем нас, и тогда дверь ничем не лучше порта — а по ней поедут разрешения человека (`allow`/`decline`, задача 7 плана), то есть чужое «разрешить» вместо хозяйского. Поэтому прав ровно двое: наша учётная запись (её SID) и SYSTEM, наследование запрещено (`D:P`). На Linux то же делают права на сам сокет (0600) и проверка `SO_PEERCRED`.
//!
//! **Охрана у двери двусторонняя, и это не перестраховка.** Дескриптор и `first_pipe_instance` защищают от того, кто придёт ПОСЛЕ нас; самозванец, успевший создать канал ПЕРВЫМ, нашему беку дверь открыть не даст (и тот скажет об этом словами — `NAME_TAKEN`), а вот клиента принял бы охотно. Поэтому и клиент спрашивает ОС, чей процесс держит сервер, и при расхождении отказывает словами (`STRANGER_DOOR`).
//!
//! **Имя двери считается тем же `single_instance::guard_key`, что и ключ замка.** У замка и двери один вопрос — «чья это копия», — и ответ обязан быть один: второе правило чистки имени разошлось бы с первым при следующей правке. Чужая учётная запись на той же машине стучится в ДРУГУЮ дверь и нашей не видит.
//!
//! **Кадр — тот же `wire::Frame`**, что возит сообщения по каналу между машинами: второго способа упаковки в продукте не заводим (решение ведущего). Поэтому `Frame` и стал общим по полезной нагрузке — им возятся и `wire::Msg`, и `Ask`/`Answer`.
//!
//! **Дверь не поднимает бека.** Клиент, не нашедший живого, говорит слова (`NO_BACK_END`) и уходит: иначе человек получил бы вторую, невидимую копию продукта — ровно то, что весь день 15.11 мешало открыть настоящее окно.
//!
//! Каким кодом уходить — дело КЛИЕНТА, а не двери, и клиенты тут разные. `status` без живого успешен (код 0): он напечатал состояние с диска, за которым человек и пришёл, а отсутствие живого — такое же сведение, как остальные его строки. Командам, которым дверь нужна по существу (`allow`, `decline` — задача 7 плана), отсутствие живого значит «сделать не удалось», и уходить им положено ненулевым кодом (решение ведущего, круг правок 2).

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

/// Имя двери занято — отказ беку при подъёме, и он не молчит. Живут один раз здесь: `open` кладёт их в саму ошибку, а `open_or_say` уносит ту же строку и в журнал, и на экран — тем же приёмом, что `gate::refuse_to_serve`.
///
/// Случая два, и человеку оба названы: либо программа уже запущена (обычное), либо имя занял чужой процесс (тогда наш бек дверь не откроет вовсе — и это верный исход, самозванец не должен получить ни одной команды).
pub const NAME_TAKEN: &str = "имя управляющей двери уже занято: его держит либо вторая копия программы, либо чужой процесс — закройте первую копию, а если её нет, сообщите нам: своих команд такой двери отдавать нельзя";

/// Дверь держит не наш процесс — отказ КЛИЕНТУ. Команда в этом случае не отдаёт чужому ни вопроса, ни тем более разрешения человека.
pub const STRANGER_DOOR: &str = "под именем управляющей двери отвечает чужая учётная запись — это не ваш экземпляр программы; закройте её и сообщите нам, команд этой двери не отдавайте";

/// В дверь стучалась чужая учётная запись — строка в журнал на стороне бека. Молча закрывать дверь нельзя: это единственный след события, по которому человек потом разберётся.
pub const STRANGER_KNOCK: &str = "в управляющую дверь стучалась чужая учётная запись — ответа она не получила";

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
    ///
    /// **`Option`, а не просто список, и это разница ТИПОМ, а не соглашением (решение ведущего, круг правок 1).** `None` — живой экземпляр о связи пока не сообщает (так отвечает путь окна: состояние карточек живёт в `Ui` внутри GPUI, и клиентом двери окно становится задачей 9 плана). `Some` с пустым списком — связей правда нет. Человеку эти два случая говорятся РАЗНЫМИ словами (`status_lines`), а пустой список, который на самом деле незнание, этим типом просто невыразим — поэтому и не нужно соглашения, которое кто-нибудь однажды не прочтёт.
    pub cards: Option<Vec<CardState>>,
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

/// Строки, которыми живой экземпляр рассказывает о себе человеку, — здесь, а не в `main.rs`, ровно затем, чтобы их проверял тест: разницу «о связи пока не сообщает» и «связей нет» человек обязан ПРОЧЕСТЬ словами, а не вывести из пустоты (ограничение «молчаливого ответа не бывает»).
///
/// Слово предмета — общая константа `SUBJECT`, а не своя: выключатель на экране, журнал и проверки называют его одними знаками, и `status` не имеет права назвать его иначе.
pub fn status_lines(v: &StatusView) -> Vec<String> {
    let on = if v.serving { "включён" } else { "выключен" };
    let mut out = vec![format!("Отвечает живой экземпляр: {} {on}.", crate::SUBJECT)];
    match &v.cards {
        None => out.push("  о состоянии связи он пока не сообщает".into()),
        Some(cards) if cards.is_empty() => out.push("  ни одна карточка сейчас не связана".into()),
        Some(cards) => out.extend(cards.iter().map(|c| format!("  {:<16} {}", c.name, c.word))),
    }
    out
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

/// Слова на отказ самого разговора.
///
/// Словарь `wire::Refuse` — словарь СОПРЯЖЕНИЯ, и его ТЕКСТ писан для разговора двух машин: «На том компьютере закрыли связь… подключитесь заново». В двери на том конце не тот компьютер, а свой же процесс на этой машине, и такие слова человека обманывают (находка ревью круга 2 — прежде сюда подставлялся весь текст). Поэтому наружу идут наши слова, а от чужого словаря берётся только КОД — короткое машинное имя причины, годное для диагностики и ничего не обещающее человеку.
fn not_answered(r: &wire::Refuse) -> String {
    format!("живой экземпляр не ответил (причина: {})", r.code.as_str())
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
    let path = pipe_path(name);
    // Права назначаются ДО первого экземпляра канала и держатся всё время приёма: каждый следующий экземпляр создаётся с тем же дескриптором, иначе второй и дальнейшие достались бы умолчанию — то есть дверь была бы заперта только до первого стука.
    let rights = Rights::of_account(&my_sid()?)?;
    let rt = runtime()?;
    // Слушатель заводится ЗДЕСЬ, до возврата, а не внутри потока: отказ — занятое имя, запрет прав — обязан достаться тому, кто дверь открывал, а не пропасть в чужом потоке. Тот же довод, что у `single_instance::serve_show_requests`, где дескриптор события заводится до запуска потока.
    let mut server = {
        let _in_rt = rt.enter();
        pipe(&path, &rights, true)?
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
                let Ok(next) = pipe(&path, &rights, false) else { return };
                let mut taken = std::mem::replace(&mut server, next);
                let _ = tokio::time::timeout(ASK_WAIT, answer(&mut taken, &mine)).await;
            }
        })
    });
    Ok(door)
}

/// Наш SID строкой — тем же способом, которым продукт уже отвечает на «чей это процесс» (`holder::owner_of_pid`, там же замер его прав). Второго правила «кто мы» не заводим: оно разошлось бы с первым, а по этому SID назначаются права двери и сверяется её сервер.
#[cfg(windows)]
fn my_sid() -> io::Result<String> {
    crate::holder::owner_of_pid(std::process::id())
        .ok_or_else(|| io::Error::other("не удалось узнать SID своей учётной записи — права управляющей двери назначить нечем"))
}

/// Дескриптор безопасности двери: полный доступ (`GA`) только нашей учётной записи и SYSTEM, `D:P` — DACL защищён, ничего не наследуется.
///
/// SDDL строкой, а не сборка ACL своими руками: строка читается глазами при разборе беды и вся видна в одном месте, а ручная сборка — три вызова подряд, каждый из которых может соврать молча и оставить дверь открытой.
#[cfg(windows)]
struct Rights(*mut std::ffi::c_void);

// Дескриптор безопасности — просто память, которую мы держим и освобождаем сами; переносить её между потоками безопасно (тот же довод, что у `single_instance::Guard`).
#[cfg(windows)]
unsafe impl Send for Rights {}

#[cfg(windows)]
impl Drop for Rights {
    fn drop(&mut self) {
        // SAFETY: указатель отдал `ConvertStringSecurityDescriptorToSecurityDescriptorW`, он же велит освобождать его `LocalFree`; второй раз `Drop` не зовут.
        unsafe { windows_sys::Win32::Foundation::LocalFree(self.0) };
    }
}

#[cfg(windows)]
impl Rights {
    fn of_account(sid: &str) -> io::Result<Rights> {
        use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
        let sddl = crate::single_instance::wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
        let mut sd = std::ptr::null_mut();
        // SAFETY: строка нуль-терминирована и жива до конца вызова; память под дескриптор выделяет сама функция, освобождает `Drop` выше.
        let ok =
            unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) };
        if ok == 0 {
            let e = std::io::Error::last_os_error();
            return Err(std::io::Error::new(e.kind(), format!("права управляющей двери не собрались (SDDL для {sid}): {e}")));
        }
        Ok(Rights(sd))
    }

    fn attrs(&self) -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

/// Один экземпляр канала с нашими правами. `first` — тот самый `first_pipe_instance`: он ручается, что имя не занято никем, и его отказ не молчит, а получает слова (`NAME_TAKEN`) — занятое имя значит либо вторую копию, либо самозванца, и человеку это надо прочесть.
#[cfg(windows)]
fn pipe(path: &str, rights: &Rights, first: bool) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};
    let mut attrs = rights.attrs();
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    // SAFETY: `attrs` жив до конца вызова и указывает на дескриптор, который держит `rights`; функция только читает его, себе не оставляя.
    let made = unsafe { opts.create_with_security_attributes_raw(path, std::ptr::from_mut(&mut attrs).cast()) };
    match made {
        Err(e) if first && matches!(e.raw_os_error(), Some(c) if c == ERROR_ACCESS_DENIED as i32 || c == ERROR_PIPE_BUSY as i32) => {
            Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{NAME_TAKEN} ({e})")))
        }
        other => other,
    }
}

/// То же, что на Windows, но сокетом в каталоге состояния — см. докстроку выше и заголовок модуля.
#[cfg(not(windows))]
fn open_at(name: &str, kit: &Kit) -> io::Result<Door> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;
    let path = sock_path(name, kit);
    if path.exists() {
        // Файл есть — два разных случая, и путать их нельзя. За ним ОТВЕЧАЮТ: имя занято (вторая копия либо самозванец), и снимать дверь нельзя — человеку слова (`NAME_TAKEN`), как и на Windows отказ `first_pipe_instance`. Проба — обычным блокирующим `connect`: рантайма для неё не нужно, и ждать она не умеет.
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, NAME_TAKEN));
        }
        // Не отвечают — остаток прошлого запуска: `bind` на существующем файле отказал бы «адрес занят», а снять этот файл за нас некому — ядро сокет не убирает (в отличие от `flock`, который держится дескриптором и снимается с процессом).
        let _ = std::fs::remove_file(&path);
    }
    let rt = runtime()?;
    let listener = {
        let _in_rt = rt.enter();
        UnixListener::bind(&path)?
    };
    // Права на сам сокет, а не на каталог состояния: в каталоге живут параметры человека и журнал, и менять их режим ради двери значило бы править то, о чём не просили. 0600 — стучать вправе только наша учётная запись, и отвечает на это ядро, а не наши догадки (спека, раздел 4).
    //
    // ponytail: между `bind` и этой строкой сокет недолго живёт с правами по умолчанию (umask), и в эту щель чужой успел бы соединиться. Закрыта она не здесь, а `peer_is_ours` на приёме: uid пришедшего сверяется с нашим на КАЖДЫЙ стук. Снимать щель заранее нечем — `umask` общий на процесс, и трогать его ради одного файла значило бы править чужие.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let door = Door { view: Arc::new(Mutex::new(StatusView::default())) };
    let mine = door.clone();
    let kit = kit.clone();
    std::thread::spawn(move || {
        rt.block_on(async move {
            loop {
                let Ok((mut taken, _)) = listener.accept().await else { return };
                if !peer_is_ours(&taken) {
                    // Молча закрыть дверь нельзя: эта строка — единственный след события, по которому человек потом разберётся.
                    crate::journal::write(&kit, crate::journal::Kind::Fail, STRANGER_KNOCK);
                    continue;
                }
                let _ = tokio::time::timeout(ASK_WAIT, answer(&mut taken, &mine)).await;
            }
        })
    });
    Ok(door)
}

/// Кто пришёл: uid той стороны спрашивается у ЯДРА (`SO_PEERCRED`), а не выводится из наших догадок — то самое «средствами ОС», которым спека отличает канал от порта на петле.
///
/// Не добыли уверенности — значит «не наш»: отказ незнакомцу дешевле разговора с ним.
#[cfg(not(windows))]
fn peer_is_ours(stream: &tokio::net::UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: обычный вызов libc на дескрипторе живого соединения; буфер и его длина — наш стек, и размер объявлен тем же типом, что читает ядро.
    let got = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, std::ptr::from_mut(&mut cred).cast(), &mut len)
    };
    // SAFETY: `getuid` ничего не принимает и не может отказать.
    got == 0 && cred.uid == unsafe { libc::getuid() }
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

/// Чей процесс держит дверь, в которую мы только что постучали. Спрашивается у ОС: `GetNamedPipeServerProcessId` отдаёт процесс сервера, а владельца его токена называет `holder::owner_of_pid` — тот же способ, которым продукт уже отвечает на «чей это процесс» в двух других местах, и второго правила здесь не заводим.
///
/// **Зачем проверка, когда есть дескриптор.** Дескриптор и `first_pipe_instance` держат только тех, кто придёт ПОСЛЕ нас. Самозванец, успевший создать канал ПЕРВЫМ, нашему беку дверь открыть не даст — но клиента примет, и команда отдала бы ему вопрос, а с задачи 7 плана и разрешение человека. Поэтому обе стороны проверяют друг друга, и это не перестраховка, а два разных случая.
///
/// Своего SID не узнали — тоже отказ: сверять не с чем, а разговаривать с неизвестным дверь не обязана.
#[cfg(windows)]
fn server_is_ours(client: &Knock) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    let mut pid = 0u32;
    // SAFETY: дескриптор живого клиентского конца канала (его держит `client`), `pid` — наш стек.
    let asked = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut pid) };
    if asked == 0 {
        return Err(format!("{STRANGER_DOOR} (не удалось узнать, чей процесс держит дверь: {})", io::Error::last_os_error()));
    }
    match (crate::holder::owner_of_pid(pid), my_sid().ok()) {
        (Some(theirs), Some(mine)) if theirs == mine => Ok(()),
        _ => Err(STRANGER_DOOR.to_string()),
    }
}

/// На Linux тот же вопрос задаётся тем же способом, что и на стороне приёма: `SO_PEERCRED` отвечает обеим сторонам соединения, и клиент спрашивает им, чей процесс держит сокет.
///
/// **Прежде здесь стоял `Ok(())` с доводом «сокет лежит в нашем каталоге состояния, куда чужой не пишет», и довод был НЕПРОВЕРЕННЫЙ** (находка ревью круга 2). Дерево его опровергает: `Kit::at` кладёт состояние в корень комплекта, каталог создаётся по umask, 0700 ему никто не ставит, а путь задаётся снаружи (`--kit`, `ONEC_CONNECT_KIT`) и указывает куда угодно. На пути `bind` я такую же опору уже отверг и поставил проверку у ядра — а на клиентском конце оставил рассуждение. Вопрос один («кто на другом конце»), и ответ на него обязан быть один.
#[cfg(not(windows))]
fn server_is_ours(client: &Knock) -> Result<(), String> {
    if peer_is_ours(client) {
        Ok(())
    } else {
        Err(STRANGER_DOOR.to_string())
    }
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
        // До первого же сказанного слова: вопрос чужому — это уже утечка (а с задачи 7 плана по этой двери поедет разрешение человека).
        server_is_ours(&stream)?;
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
        let shown = StatusView {
            serving: true, cards: Some(vec![CardState { name: "Ромашка".into(), word: "подключено".into() }])
        };
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
        door.show(StatusView { serving: true, cards: Some(vec![]) });
        for n in 1..=3 {
            let Ok(Answer::Status(got)) = ask_at(&name, &kit, Ask::Status) else {
                panic!("стук {n} остался без ответа")
            };
            assert!(got.serving, "стук {n}");
        }
    }

    /// Занятое имя двери — отказ СО СЛОВАМИ, а не пожатие плечами: это либо вторая копия, либо самозванец, и человеку надо прочесть, что именно случилось. На Windows отказывает `first_pipe_instance`, на Linux — проба живого сокета перед тем, как снять остаток; слова у обоих одни.
    #[test]
    fn a_taken_door_name_is_refused_in_words() {
        let (_dir, kit) = kit_for_test();
        let name = test_door("taken");
        let _first = open_at(&name, &kit).expect("первая дверь открылась");
        let Err(no) = open_at(&name, &kit) else {
            panic!("второй под тем же именем дверь достаться не должна")
        };
        assert!(no.to_string().contains(NAME_TAKEN), "слова, а не голый код ОС: {no}");
    }

    /// Дверь на Linux закрыта правами на сам сокет: стучать вправе только наша учётная запись, и сторожит это ядро, а не наша догадка (спека, раздел 4). На Windows то же делает дескриптор безопасности канала — проверить его без второй учётной записи нечем, см. отчёт задачи.
    #[cfg(not(windows))]
    #[test]
    fn the_socket_of_the_door_is_ours_alone() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, kit) = kit_for_test();
        let name = test_door("rights");
        let _door = open_at(&name, &kit).expect("дверь открылась");
        let mode = std::fs::metadata(sock_path(&name, &kit)).expect("сокет на месте").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "ни группе, ни остальным — ничего: {mode:o}");
    }

    /// Имя двери помощнику и каталог его комплекта — переменные окружения: другого способа договориться с ЧУЖИМ процессом у теста нет, а в этом и смысл меры.
    const HELPER_DOOR: &str = "DEV1C_DOOR_HELPER";
    const HELPER_KIT: &str = "DEV1C_DOOR_HELPER_KIT";

    /// Помощник межпроцессной меры: открывает дверь под именем из окружения и живёт, пока его не снимут. Запускает его тест-родитель (`a_door_opened_by_another_process_answers_this_one`), перезапуская ЭТОТ ЖЕ тестовый бинарь, — поэтому он `#[ignore]` и поэтому без переменной окружения не делает ничего.
    #[test]
    #[ignore = "помощник межпроцессной меры: его запускает тест-родитель, а не человек"]
    fn the_helper_holds_a_door() {
        let (Ok(name), Ok(dir)) = (std::env::var(HELPER_DOOR), std::env::var(HELPER_KIT)) else { return };
        let kit = Kit::at(std::path::Path::new(&dir));
        let door = open_at(&name, &kit).expect("помощник открыл дверь");
        door.show(StatusView {
            serving: true, cards: Some(vec![CardState { name: "Ромашка".into(), word: "подключено".into() }])
        });
        // Переживать родителя помощнику незачем: тот снимает его сам, а срок здесь — страховка от осиротевшего процесса, если родитель упадёт.
        std::thread::sleep(Duration::from_secs(30));
    }

    /// **Дверь между ДВУМЯ процессами** — та самая мера, которой круг 1 обойтись не смог, а ради неё дверь и затевалась (бек в одном сеансе, человек в другом).
    ///
    /// Что она ловит, чего не ловит круговой тест в одном процессе: ошибку в дескрипторе безопасности (чужому процессу той же учётной записи дверь обязана открыться, а не захлопнуться), настоящий `GetNamedPipeServerProcessId` на ЧУЖОМ процессе и сверку его SID, а на Linux — права сокета и `SO_PEERCRED` с обеих сторон. Проверить разные УЧЁТНЫЕ ЗАПИСИ так по-прежнему нельзя, и декораций для этого здесь нет.
    #[test]
    fn a_door_opened_by_another_process_answers_this_one() {
        let (dir, kit) = kit_for_test();
        let name = test_door("cross");
        let mut helper = std::process::Command::new(std::env::current_exe().expect("путь к этому же тестовому бинарю"))
            .args(["--exact", "door::tests::the_helper_holds_a_door", "--ignored"])
            .env(HELPER_DOOR, &name)
            .env(HELPER_KIT, dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("помощник запустился");
        // Ждём, пока помощник успеет открыть дверь: сборки нет, но процесс всё же стартует не мгновенно. Опрос, а не сон на глазок, — иначе мера превратилась бы в ожидание самой себя.
        let mut got = None;
        for _ in 0..100 {
            if let Ok(Answer::Status(v)) = ask_at(&name, &kit, Ask::Status) {
                got = Some(v);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = helper.kill();
        let _ = helper.wait();
        let got = got.expect("дверь чужого процесса не ответила");
        assert!(got.serving, "ответил тот, кто дверь открыл, а не умолчание");
        assert_eq!(got.cards.as_deref().unwrap_or_default().len(), 1, "снимок доехал между процессами целиком: {got:?}");
    }

    /// «О связи пока не сообщает» и «связей нет» — РАЗНЫЕ слова человеку, а не одна пустота на оба случая. Тест и держит это различие: пропади оно, `status` отвечал бы молчанием там, где обязан отвечать словами.
    #[test]
    fn not_telling_and_nothing_connected_are_different_words() {
        let unknown = status_lines(&StatusView { serving: true, cards: None });
        let none = status_lines(&StatusView { serving: true, cards: Some(vec![]) });
        let one = status_lines(&StatusView {
            serving: true,
            cards: Some(vec![CardState { name: "Ромашка".into(), word: "подключено".into() }]),
        });
        assert_ne!(unknown, none, "незнание и отсутствие связей не одно и то же");
        assert!(unknown[1].contains("не сообщает"), "{:?}", unknown);
        assert!(none[1].contains("не связана"), "{:?}", none);
        assert!(one[1].contains("Ромашка") && one[1].contains("подключено"), "{:?}", one);
        assert!(one[0].contains(crate::SUBJECT), "предмет назван общим словом: {:?}", one);
        assert!(status_lines(&StatusView { serving: false, cards: None })[0].contains("выключен"));
    }
}
