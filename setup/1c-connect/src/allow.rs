//! Разрешение на работу — файл в каталоге состояния (`Kit::state`), а не флаг в памяти окна: скоро у рабочего места несколько процессов (окно, `connect`, `serve`, будущая подкоманда `mcp`), и только диск виден им всем.
//!
//! Закрыто по умолчанию: разрешение — это ровно `"on"` в файле, и только при живом владельце рабочего места; всё непонятное (файла нет, он пуст, в нём чужое слово, владельца не видно) означает «нельзя» — тем же правилом, каким живёт гейт в `gui/gate.rs`.
//!
//! Живость владельца — не флаг на диске (тот не переживает `taskkill`), а тот же замок, что уже стоит в `single_instance.rs`, но через `single_instance::is_held`, а не `try_acquire`: проверка живости не должна сама становиться держателем замка хоть на микросекунду, иначе два читателя (`watch` и прямой `read`, два процесса подкоманды `mcp`) заверяют живость друг друга — находка ревью задачи 2, см. комментарий `is_held`.

use crate::single_instance;
use common::kit::{write_text, Kit};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Имя файла признака в каталоге состояния — рядом с `install.state` и `1c-connect.log`.
const FILE: &str = "allow";

/// Путь к файлу признака для этого рабочего места.
pub fn path(kit: &Kit) -> PathBuf {
    kit.state_path(FILE)
}

/// Решение, читать ли разрешение как «да» — чистая функция без диска, чтобы все углы (пустой файл, чужое слово, отсутствие владельца) проверялись без файловой системы. `file` — содержимое, как его прочитал `read` (`None`, если файла нет); `window_alive` — результат проверки замка. Разрешение — это `window_alive` и буквально `"on"` после обрезки пробелов, ничего больше: поэтому «выключено» и опечатка в файле — то же самое, что файла нет вовсе.
pub fn decide(file: Option<&str>, window_alive: bool) -> bool {
    window_alive && file.map(|s| s.trim() == "on").unwrap_or(false)
}

/// Записать признак: `on` при разрешённой работе, `off` — когда владелец её снял или выходит. Возвращает ошибку, а не проглатывает её сама: непрошедший `off` оставляет на диске «on» при уже мёртвом или выключенном владельце — то есть разрешение, которое никто не давал, а не безопасный исход, и о нём обязан узнать вызывающий (окно — в журнал, см. `gui::quit`/`gui::link`), а не тишина (минорная находка ревью задачи 2).
pub fn write(kit: &Kit, on: bool) -> anyhow::Result<()> {
    write_text(&path(kit), if on { "on\n" } else { "off\n" })
}

/// Живой ли владелец рабочего места прямо сейчас — `single_instance::is_held`, которая смотрит на замок, не занимая его: занятый замок проверкой сам превратил бы её в держателя на те же микросекунды, и соседняя проверка (другой процесс `mcp`, или `watch` рядом с прямым `read`) увидела бы «занято» и сочла бы владельца живым, хотя настоящего уже нет (находка ревью задачи 2).
fn window_alive(kit: &Kit, user: &str, workstation: &str) -> bool {
    let key = single_instance::guard_key(user, workstation);
    let lock_on = single_instance::lock_target(kit);
    // Замок не открылся вовсе (права, каталог не готов) — то же самое
    // непонятное, что и чужое слово в файле: закрыто по умолчанию.
    single_instance::is_held(&key, &lock_on).unwrap_or(false)
}

/// Прочитать разрешение прямо сейчас: файл с диска плюс живость владельца, через `decide`.
pub fn read(kit: &Kit, user: &str, workstation: &str) -> bool {
    let file = std::fs::read_to_string(path(kit)).ok();
    decide(file.as_deref(), window_alive(kit, user, workstation))
}

/// Опрос файла раз в 200 мс в своём потоке — задачу забирает подкоманда `mcp`, которой нужен ответ на каждый `tools/call` без похода на диск и в реестр Windows синхронно с запросом редактора.
/// ponytail: потолок — вызов, пришедший в эти 200 мс после выключения, ещё пройдёт; опрос чаще значило бы читать файл и держать замок рабочего места на каждый тик, а 200 мс и так короче, чем человек успевает заметить разницу между щелчком и отказом.
pub fn watch(kit: &Kit, user: String, workstation: String) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let out = flag.clone();
    let kit = kit.clone();
    std::thread::spawn(move || loop {
        out.store(read(&kit, &user, &workstation), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    flag
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Строка, уникальная на каждый вызов (счётчик плюс pid) — как `unique` в
    /// `single_instance.rs`, иначе параллельные тесты этого файла спорили бы
    /// за один каталог состояния или за один ключ замка.
    fn unique(tag: &str) -> String {
        use std::sync::atomic::AtomicU64;
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        format!("{tag}-{}-{n}", std::process::id())
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(unique("allow-test"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn kit_in(dir: PathBuf) -> Kit {
        Kit::at(dir)
    }

    #[test]
    fn no_window_means_no_work() {
        assert!(!decide(None, false));
    }

    #[test]
    fn a_stale_allow_file_does_not_allow() {
        // Окно сняли taskkill: файл остался «on», но замок свободен.
        assert!(!decide(Some("on"), false));
    }

    #[test]
    fn the_switch_decides_while_the_window_lives() {
        assert!(decide(Some("on"), true));
        assert!(!decide(Some("off"), true));
        // Чего не понимаем — запрещаем: файл чужой или полузаписанный.
        assert!(!decide(Some(""), true));
        assert!(!decide(Some("вкл"), true));
        assert!(!decide(None, true));
    }

    #[test]
    fn a_written_flag_reads_back() {
        let kit = kit_in(tempdir());
        write(&kit, true).unwrap();
        assert_eq!(std::fs::read_to_string(path(&kit)).unwrap().trim(), "on");
        write(&kit, false).unwrap();
        assert_eq!(std::fs::read_to_string(path(&kit)).unwrap().trim(), "off");
    }

    /// Важная находка ревью задачи 2: единственная строка, которая и есть
    /// дверь (`window_alive` внутри `read`), не была покрыта ничем — тесты
    /// `decide` проходили при любой её поломке. Здесь замок рабочего места
    /// держит сам тест (владелец «жив»), `read` обязан увидеть «on» как
    /// разрешение; отпустили замок — тот же файл с тем же «on» больше не
    /// разрешение.
    #[test]
    fn read_follows_the_owners_lock_not_just_the_file() {
        let kit = kit_in(tempdir());
        let workstation = unique("read-ws");
        write(&kit, true).unwrap();
        let key = single_instance::guard_key("owner", &workstation);
        let lock_on = single_instance::lock_target(&kit);
        let guard = single_instance::try_acquire(&key, &lock_on).unwrap().expect("замок свободен в начале теста");
        assert!(read(&kit, "owner", &workstation), "владелец держит замок — файл «on» должен читаться как разрешение");
        drop(guard);
        assert!(!read(&kit, "owner", &workstation), "владелец ушёл — тот же файл «on» больше не разрешение");
    }
}
