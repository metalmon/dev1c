//! Холодный замок готовности: что должно быть на месте, чтобы человеку вообще
//! было что включать. Спека, раздел «Зелёный значит „работает“: переключатель
//! под замком готовности» — решение заказчика 2026-10-20: «Сам переключатель
//! включения должен быть недоступен, пока нет пользователя и пока не
//! установлены MCP».
//!
//! Отбор условий простой и назван спекой: **смертельно для работы и видно без
//! сети**. Всё, что видно только после включения (отвечает ли публикация,
//! поднята ли среда), — горячее: оно красит точку, но переключатель не
//! запирает, иначе локальный режим запер бы его навсегда (публикацию там
//! поднимает сам коннектор при включении).
//!
//! Функция чистая: ни сети, ни файловой системы. Всё, за чем пришлось бы
//! ходить на диск, передаётся аргументами — тогда весь список условий
//! проверяется тестом, а не живой машиной.

use crate::cfg::Cfg;
use crate::licence;
use common::op::OpState;
use std::path::Path;

/// Заперт ли переключатель. `None` — включать можно; `Some(причина)` —
/// нельзя, и причина короткой строкой для подсказки (в заголовок карточки
/// она не идёт: «не надо там прозу писать, она не лезет», заказчик
/// 2026-10-21).
///
/// Порядок условий — от основания к вершине, тот же, что у `verify::order`:
/// человек читает первое, что сломано, а не последнее.
///
/// Аргументы, которых нет в `Cfg`, — это и есть «видно без сети, но нужен
/// диск»: `params_exists` — лежит ли файл параметров, `base_exists` — есть ли
/// каталог файловой базы, `tools` — полон ли комплект (инструменты работают
/// из него, никуда не раскладываясь)
/// (`ops_host::tools_state`), `platform` — найдена ли платформа
/// (`ops_host::platform_choice` — `Err` несёт причину словами), `mcp_in_kit` — есть ли `mcp-1c` в комплекте.
pub fn cold_gate(
    cfg: &Cfg,
    params_exists: bool,
    base_exists: bool,
    tools: &OpState,
    platform: Result<&Path, &str>,
    mcp_in_kit: bool,
    licence: &licence::State,
) -> Option<String> {
    // Срок — раньше всего остального: когда он вышел, смотреть ниже нечего,
    // работу коннектор не обслуживает (тот же порядок, что в `verify::order`).
    if matches!(licence, licence::State::Expired) {
        return Some(format!("срок действия истёк {}", licence::until_text()));
    }
    if !params_exists {
        return Some("нет файла параметров — заполните поля и нажмите «Сохранить»".into());
    }
    // Отказ лестницы 1С приходит словами и годится как есть: «платформа не
    // найдена» или «версия 8.2.19 (поле platform в 1c-connect.toml) не
    // установлена». Молчаливый откат на новейшую был бы ложью про то, на чём
    // идёт работа (спека, «Версия платформы: та же, что выберет 1С»).
    if let Err(why) = platform {
        return Some(why.to_string());
    }
    if !mcp_in_kit {
        return Some("в комплекте нет mcp-1c".into());
    }
    // «Сделано» и ничего другого. Копии инструментов на машине больше нет
    // (леджер комплекта, пункт 15), поэтому единственный не-`Done` исход —
    // неполный комплект, и совет не «Установить», а взять комплект целиком:
    // поставить нам нечего, брать нечего.
    if !matches!(tools, OpState::Done(_)) {
        return Some("инструментов в комплекте нет (vrunner/, git/, httpd/) — скопируйте полный комплект заново".into());
    }
    let h = &cfg.host;
    // Пустое поле — «не указана база», а не «каталога  нет»: в пустой строке
    // человеку нечего прочитать.
    if h.base_path.trim().is_empty() {
        return Some("не указана база".into());
    }
    // Правило про базу не переписываем: оно уже записано фактом проверки
    // (`verify::base_fact`) — каталог обязателен только файловой базе, а
    // `сервер\база` и сетевой путь существованием не проверяются, иначе
    // серверная база заперла бы переключатель навсегда.
    if !crate::verify::base_fact(&h.base_path, base_exists).ok {
        return Some(format!("каталога базы {} нет", h.base_path));
    }
    // Правило про пользователя базы — там же, где факт (`verify::db_user_is_set`):
    // поверхностей две, но расходиться им нельзя.
    if !crate::verify::db_user_is_set(&h.db_user) {
        return Some("не задан пользователь базы".into());
    }
    // Данные для подключения к среде есть только у туннеля: в локальном режиме
    // этих трёх полей в окне нет вовсе (`panel::shown_fields`), и запирать ими
    // переключатель значило бы спрашивать человека о том, чего ему не
    // показывают.
    if h.mode != "local" {
        if h.server.trim().is_empty() {
            return Some("не задан адрес среды".into());
        }
        if h.password.is_empty() {
            return Some("не задан пароль рабочего места".into());
        }
        if h.workstation.trim().is_empty() {
            return Some("не задано имя рабочего места".into());
        }
    }
    None
}

/// Нужно ли вообще ходить на диск за каталогом базы. Классификация та же, что
/// у `verify::base_fact`: сетевой путь (`\\…`) и `сервер\база` существованием
/// не проверяются — факт их и не смотрит.
///
/// Нужна не ради красоты, а ради главного потока окна: `exists()` по
/// недоступной сетевой шаре висит на таймауте SMB секундами, а замок считается
/// на показ окна, конец каждой операции и «Сохранить». Существование базы
/// считается в своём потоке — в `verify::gather`, тот же запрет и там.
pub fn wants_directory(path: &str) -> bool {
    // Само правило живёт в `verify` — в слое, который собирается всегда:
    // окно идёт по фиче `gui`, и обратное направление (`verify` → `gui`)
    // ломало бы сборку без окна.
    crate::verify::wants_directory(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done() -> OpState {
        OpState::Done("vrunner 3.0.2".into())
    }

    /// Всё на месте — туннельное рабочее место по умолчанию.
    fn open(cfg: &Cfg) -> Option<String> {
        cold_gate(cfg, true, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid)
    }

    #[test]
    fn everything_in_place_opens_the_switch() {
        assert_eq!(open(&crate::cfg::filled_for_test()), None);
        let mut local = crate::cfg::filled_for_test();
        local.host.mode = "local".into();
        assert_eq!(open(&local), None);
    }

    #[test]
    fn an_expired_deadline_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Expired).unwrap();
        assert!(why.contains("срок действия") && why.contains(&licence::until_text()), "{why}");
        // Предупреждение заранее — не замок: работа идёт.
        assert_eq!(cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Soon { days: 3 }), None);
    }

    #[test]
    fn a_missing_params_file_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), false, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
        assert!(why.contains("параметров"), "{why}");
    }

    #[test]
    fn an_empty_db_user_holds_the_switch() {
        let mut c = crate::cfg::filled_for_test();
        c.host.db_user = "   ".into();
        let why = open(&c).unwrap();
        assert!(why.contains("пользователь базы"), "{why}");
    }

    #[test]
    fn a_file_base_without_its_directory_holds_the_switch() {
        let c = crate::cfg::filled_for_test();
        let why = cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
        assert!(why.contains(&c.host.base_path), "{why}");
    }

    #[test]
    fn a_server_base_does_not_hold_the_switch() {
        // `сервер\база` на диске не лежит — его существование не проверяется
        // вовсе, иначе переключатель был бы заперт навсегда.
        let mut c = crate::cfg::filled_for_test();
        c.host.base_path = "srv-1c\\buh".into();
        assert_eq!(cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid), None);
        // Сетевой путь — предупреждение проверки, но не замок.
        c.host.base_path = "\\\\nas\\bases\\buh".into();
        assert_eq!(cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid), None);
    }

    /// Пустое поле объясняется словами, а не пустой строкой в середине фразы.
    #[test]
    fn an_empty_base_says_so_instead_of_naming_an_empty_directory() {
        let mut c = crate::cfg::filled_for_test();
        c.host.base_path = "  ".into();
        assert_eq!(open(&c).as_deref(), Some("не указана база"));
    }

    /// Кого спрашивать у диска: только файловую базу. Правило сверяется с самим
    /// фактом проверки — разойтись им нечем, иначе сетевая база повисла бы на
    /// таймауте SMB прямо на главном потоке окна.
    #[test]
    fn only_a_file_base_is_looked_for_on_disk() {
        for path in ["C:\\Bases\\Buh", "/home/dev1/base", "Buh"] {
            assert!(wants_directory(path), "{path}");
            assert!(!crate::verify::base_fact(path, false).ok, "{path}: факт обязан провалиться без каталога");
        }
        for path in ["\\\\nas\\bases\\buh", "srv-1c\\buh"] {
            assert!(!wants_directory(path), "{path}");
            assert!(crate::verify::base_fact(path, false).ok, "{path}: факт существования не спрашивает");
        }
    }

    #[test]
    fn a_missing_platform_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Err("платформа 1С не найдена"), true, &licence::State::Valid).unwrap();
        assert!(why.contains("платформа"), "{why}");
        // Причина приходит из лестницы как есть: невыполнимый пин запирает
        // переключатель своими словами, а не общим «не найдена».
        let pin = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Err("версия 8.2.19 (поле platform в 1c-connect.toml) на машине не установлена; установлено: 8.3.27.1508"), true, &licence::State::Valid).unwrap();
        assert!(pin.contains("8.2.19") && pin.contains("не установлена"), "{pin}");
    }

    #[test]
    fn a_kit_without_mcp_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), false, &licence::State::Valid).unwrap();
        assert!(why.contains("mcp-1c"), "{why}");
    }

    #[test]
    fn an_incomplete_kit_holds_the_switch() {
        // `Missing` и `Drifted` `tools_state` больше не возвращает — копии
        // инструментов на машине нет, сверять две версии нечем (леджер
        // комплекта, пункт 15). Но замок обязан запираться на ЛЮБОМ не-`Done`,
        // а не на перечне известных исходов: это его работа, а не догадка о
        // том, какие состояния бывают.
        for st in [OpState::Missing, OpState::Drifted { from: "3.0.1".into(), to: "3.0.2".into() }, OpState::Blocked("нет vrunner/".into())] {
            let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &st, Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
            assert!(why.contains("инструмент") && why.contains("комплект"), "{st:?}: {why}");
        }
    }

    /// Адрес среды, пароль и имя рабочего места — условия только туннеля; в
    /// локальном режиме этих полей в окне нет, и замок их не спрашивает.
    #[test]
    fn the_tunnel_asks_for_the_environment_and_the_local_mode_does_not() {
        type Case = (fn(&mut Cfg), &'static str);
        let cases: [Case; 3] = [
            (|c: &mut Cfg| c.host.server = String::new(), "адрес среды"),
            (|c: &mut Cfg| c.host.password = String::new(), "пароль"),
            (|c: &mut Cfg| c.host.workstation = String::new(), "рабочего места"),
        ];
        for (field, mark) in cases {
            let mut tunnel = crate::cfg::filled_for_test();
            field(&mut tunnel);
            let why = open(&tunnel).unwrap_or_else(|| panic!("туннель без «{mark}» обязан запереть переключатель"));
            assert!(why.contains(mark), "{why}");
            let mut local = crate::cfg::filled_for_test();
            local.host.mode = "local".into();
            field(&mut local);
            assert_eq!(open(&local), None, "в локальном режиме поля «{mark}» в окне нет");
        }
    }
}
