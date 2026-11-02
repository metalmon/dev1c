//! Разрешение на работу — файл в каталоге состояния (`Kit::state`), а не флаг в памяти окна: скоро у рабочего места несколько процессов (окно, `connect`, `serve`, будущая подкоманда `mcp`), и только диск виден им всем.
//!
//! Закрыто по умолчанию: разрешение — это ровно `"on"` в файле, и только при живом владельце рабочего места; всё непонятное (файла нет, он пуст, в нём чужое слово, владельца не видно) означает «нельзя» — тем же правилом, каким живёт гейт в `gui/gate.rs`.
//!
//! Живость владельца — не флаг на диске (тот не переживает `taskkill`), а признак обслуживания (`single_instance::is_serving`), не занимающий его: проверка живости не должна сама становиться держателем хоть на микросекунду, иначе два читателя (`watch` и прямой `read`, два процесса подкоманды `mcp`) заверяют живость друг друга — находка ревью задачи 2, см. комментарий `is_held`.
//!
//! Круг исправления после ревью `717c873`/`e1ff800`: `window_alive` (теперь
//! `connector_alive`) смотрела в замок РАБОЧЕГО МЕСТА, тем же ключом
//! (пользователь + рабочее место), что и держит `single_instance::acquire`.
//! Тот же дефект, что лечили у `orphan::guard_owner`, с той же причиной:
//! «Сохранить» меняет имя рабочего места у открытого окна без перезапуска
//! (`gui/save.rs`), замок остаётся под старым именем до выхода процесса —
//! после переименования эта проверка видела бы «владельца нет» при живом
//! окне, и MCP молча считались бы выключенными. Два потребителя одного по
//! смыслу вопроса («жив ли тот, кто обслуживает рабочее место») не должны
//! читать его по-разному — починено тем же `is_serving`, что и сторож моста.

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

/// Решение, читать ли разрешение как «да» — чистая функция без диска, чтобы все углы (пустой файл, чужое слово, отсутствие владельца) проверялись без файловой системы. `file` — содержимое, как его прочитал `read` (`None`, если файла нет); `connector_alive` — результат проверки признака обслуживания. Разрешение — это `connector_alive` и буквально `"on"` после обрезки пробелов, ничего больше: поэтому «выключено» и опечатка в файле — то же самое, что файла нет вовсе.
pub fn decide(file: Option<&str>, connector_alive: bool) -> bool {
    connector_alive && file.map(|s| s.trim() == "on").unwrap_or(false)
}

/// Записать признак: `on` при разрешённой работе, `off` — когда владелец её снял или выходит. Возвращает ошибку, а не проглатывает её сама: непрошедший `off` оставляет на диске «on» при уже мёртвом или выключенном владельце — то есть разрешение, которое никто не давал, а не безопасный исход, и о нём обязан узнать вызывающий (окно — в журнал, см. `gui::quit`/`gui::link`), а не тишина (минорная находка ревью задачи 2).
pub fn write(kit: &Kit, on: bool) -> anyhow::Result<()> {
    write_text(&path(kit), if on { "on\n" } else { "off\n" })
}

/// Жив ли тот, кто обслуживает это рабочее место, прямо сейчас —
/// `single_instance::is_serving`, которая смотрит на признак, не занимая
/// его: занятый признак проверкой сам превратил бы её в держателя на те же
/// микросекунды, и соседняя проверка (другой процесс `mcp`, или `watch`
/// рядом с прямым `read`) увидела бы «занято» и сочла бы владельца живым,
/// хотя настоящего уже нет (находка ревью задачи 2). Имя не об «оконном
/// процессе» конкретно: признак берут поровну окно, `connect` и `serve`
/// (`main.rs`, `gui/mod.rs`) — вопрос один, «обслуживает ли кто-то это
/// рабочее место», и headless `serve` отвечает на него так же честно, как
/// окно.
fn connector_alive(kit: &Kit, user: &str) -> bool {
    // Признак не прочитался (права, каталог не готов) — то же самое
    // непонятное, что и чужое слово в файле: закрыто по умолчанию.
    single_instance::is_serving(kit, user).unwrap_or(false)
}

/// Прочитать разрешение прямо сейчас: файл с диска плюс живость владельца, через `decide`.
pub fn read(kit: &Kit, user: &str) -> bool {
    let file = std::fs::read_to_string(path(kit)).ok();
    decide(file.as_deref(), connector_alive(kit, user))
}

/// Опрос файла раз в 200 мс в своём потоке — задачу забирает подкоманда `mcp`, которой нужен ответ на каждый `tools/call` без похода на диск и в реестр Windows синхронно с запросом редактора.
/// ponytail: потолок — вызов, пришедший в эти 200 мс после выключения, ещё пройдёт; опрос чаще значило бы читать файл и держать признак обслуживания на каждый тик, а 200 мс и так короче, чем человек успевает заметить разницу между щелчком и отказом.
pub fn watch(kit: &Kit, user: String) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let out = flag.clone();
    let kit = kit.clone();
    std::thread::spawn(move || loop {
        out.store(read(&kit, &user), Ordering::Relaxed);
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

    /// То, чем на каждой платформе реально закреплён признак обслуживания —
    /// тот же приём, что в `orphan.rs::tests::take_serving_for_test` (не
    /// общий, дублирован тем же порядком, что и `unique` выше — мелкие
    /// тестовые помощники в этом крейте не делят на несколько модулей).
    #[cfg(windows)]
    fn take_serving_for_test(kit: &Kit, user: &str) -> single_instance::Guard {
        single_instance::acquire_serving(kit, user).unwrap().expect("признак свободен в начале теста")
    }
    #[cfg(not(windows))]
    fn take_serving_for_test(kit: &Kit, _user: &str) -> single_instance::Guard {
        single_instance::try_acquire("", &single_instance::lock_target(kit)).unwrap().expect("признак свободен в начале теста")
    }

    /// Важная находка ревью задачи 2: единственная строка, которая и есть
    /// дверь (`connector_alive` внутри `read`), не была покрыта ничем —
    /// тесты `decide` проходили при любой её поломке. Здесь признак
    /// обслуживания держит сам тест (владелец «жив»), `read` обязан увидеть
    /// «on» как разрешение; отпустили признак — тот же файл с тем же «on»
    /// больше не разрешение.
    ///
    /// Круг исправления после ревью `717c873`/`e1ff800`: раньше тест держал
    /// `single_instance::guard_key("owner", &workstation)` — тот самый
    /// замок РАБОЧЕГО МЕСТА, которым `window_alive` ошибочно и смотрела.
    /// Теперь держит `acquire_serving`/`try_acquire(lock_target)` — то же,
    /// что реально берёт коннектор (`single_instance::hold_serving`), без
    /// рабочего места в ключе вовсе. (С задачи 6 у `guard_key` и подписи
    /// такой больше нет — она принимает только пользователя.)
    #[test]
    fn read_follows_the_connector_not_just_the_file() {
        let kit = kit_in(tempdir());
        let user = unique("read-user");
        write(&kit, true).unwrap();
        let guard = take_serving_for_test(&kit, &user);
        assert!(read(&kit, &user), "коннектор обслуживает комплект — файл «on» должен читаться как разрешение");
        drop(guard);
        // Уход владельца проверяем смыслом, а не мгновенностью: контракт —
        // «ушёл коннектор, ушло и разрешение», а не «в ту же микросекунду».
        // Потребитель (`allow::watch`) и сам опрашивает раз в 200 мс, так что
        // доли секунды здесь ничего не значат. Одна мгновенная проба делала
        // тест секундомером: под нагрузкой (CI — два ядра) снятый `flock`
        // успевал ещё разок прочитаться как живой, и тест падал примерно в
        // одном прогоне из трёх, не найдя дефекта в продукте. Не ушло вовсе —
        // тест по-прежнему падает, только позже.
        let gone = (0..100).any(|_| {
            if !read(&kit, &user) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            false
        });
        assert!(gone, "коннектор ушёл — тот же файл «on» больше не разрешение");
    }
}
