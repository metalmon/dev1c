use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::cfg::{self, Connection};
use crate::gui::{Busy, Running, Which};
use crate::session::State;
use common::op::OpState;
use common::Phase;
use core::prelude::v1::test;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Каталог для «чужой» подложки — отдельной от `ui.kit.state()`, которую
/// возьмёт `store_for` внутри самого `save_host`. Два разных каталога — два
/// разных ключа `secret.key` (`Store::file`), и то, что сеяно одним, другой
/// действительно не откроет: не фокус с ручной подменой поля, а настоящая
/// непрочитанность, тем же приёмом, что у `cfg.rs`-тестов (`mine`/`theirs`).
/// Без этого разведения `write_checked`'s собственная проверка после записи
/// читала бы патч обратно ТЕМ ЖЕ ключом, каким его сеяли, — и находила бы
/// настоящий пароль там, где тест заявляет «чужая машина, не открыть».
fn foreign_store_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("onec-connect-foreign-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_bad_server_leaves_the_toml_alone_and_says_why() {
    let mut ui = Ui::for_test();
    let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
    let mut host = ui.cfg.current().clone();
    host.server = "no-port".into();
    let err = save_host(&mut ui, host).unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before);
}

/// Задача 6 плана «Модель соединений», решение владельца 1: у документов
/// нет истории, затирание в чужой папке — молчаливая потеря файлов.
/// Вложенность, а не только совпадение, обязана отказать, и отказ называет
/// соединение-владельца папки, а не абстрактную «папку».
#[test]
fn save_host_refuses_a_project_folder_already_owned_by_another_connection() {
    let mut ui = Ui::for_test();
    let mut other = ui.cfg.current().clone();
    other.name = "Ромашка".into();
    other.base_path = "C:\\Bases\\Ромашка".into();
    other.project = "C:\\proj\\Ромашка".into();
    ui.cfg.connections.push(other);
    let mut host = ui.cfg.current().clone();
    host.project = "C:\\proj\\Ромашка\\src".into(); // внутри чужой папки
    let err = save_host(&mut ui, host).unwrap_err();
    // Внутри ОДНОЙ фразы человеку — одно слово про одну сущность (находка 4
    // ревью всей ветки: хвост этой фразы был взят из словаря модели, а на
    // экране человек видит карточки, и обе половины фразы уже так его и
    // зовут).
    assert!(err.contains("Ромашка") && err.contains("Buh") && err.contains("папка принадлежит одной карточке"), "{err}");
}

/// Симметрия с `cfg::validate` (урок задачи 5, круг починки 1 — там же её
/// забыли перенести в модель): пустая папка не конфликтует ни с чем, её
/// ещё не выбрали, и `folder_conflict_owner` обязан молчать так же, как
/// правило в модели.
#[test]
fn save_host_with_an_empty_project_does_not_conflict_with_anyone() {
    let mut ui = Ui::for_test();
    let mut other = ui.cfg.current().clone();
    other.name = "Ромашка".into();
    other.base_path = "C:\\Bases\\Ромашка".into();
    other.project = String::new();
    ui.cfg.connections.push(other);
    let mut host = ui.cfg.current().clone();
    host.project = String::new();
    save_host(&mut ui, host).unwrap();
}

/// Круг починки 1/5 задачи 9, находка автоматического ревью безопасности:
/// поля для правки профиля в окне нет, и собранный из формы `Connection`
/// несёт пустой `profile` (как и у всех полей, которых форма не знает) —
/// «Сохранить» не должно обнулять то, что человек вписал руками в toml.
#[test]
fn save_host_keeps_the_hand_written_profile() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().profile = "developer".into();
    let mut host = ui.cfg.current().clone();
    host.profile = String::new(); // форма такого поля не знает
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.cfg.current().profile, "developer", "профиль не обязан переживать «Сохранить» обнулённым");
}

#[test]
fn carried_base_url_keeps_an_override_and_follows_the_default() {
    // Поля «Адрес публикации» в окне нет: умолчание идёт за каталогом
    // базы, а правка в toml переживает смену базы.
    let mut saved =
        Connection { base_path: "C:\\Bases\\Buh".into(), base_url: cfg::derived_base_url("C:\\Bases\\Buh"), ..Connection::default() };
    assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost/Torg/hs/mcp-1c");
    saved.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
    assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost:8081/InfoBase/hs/mcp-1c");
    saved.base_url = String::new();
    assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost/Torg/hs/mcp-1c");
}

/// `install` записывает выведенный каталог выгрузки в toml — и записанное
/// умолчание обязано и дальше идти за каталогом проекта, иначе смена
/// проекта оставила бы выгрузку в старом месте. Свой каталог не трогаем.
#[test]
fn carried_dump_follows_the_project_unless_it_was_chosen_by_hand() {
    let new = cfg::derived_dump("C:\\proj\\Torg");
    let mut saved = Connection { project: "C:\\proj\\Buh".into(), dump: cfg::derived_dump("C:\\proj\\Buh"), ..Connection::default() };
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), new);
    saved.dump = "D:\\dump".into();
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "D:\\dump");
    // Пустое поле остаётся пустым: умолчание и так считается при чтении.
    saved.dump = String::new();
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "");
}

/// `moved_lines` принимает `Cfg`, а не `Connection` (находка 6: порт в строке —
/// доставшийся, а не предпочтение), а тесты ниже строят именно `Connection`.
fn as_cfg(h: &Connection) -> Cfg {
    Cfg::of_one(h.clone())
}

/// Каталог состояния для тестов, которым улаживание порта
/// (`ops_host::settle_chosen_http_port`) нужно НАСТОЯЩИМ, а не пустышкой
/// (находка 8 ревью круга 3). Ставится потоко-локально, переменные среды не
/// трогаются — это жёсткий запрет владельца.
fn state_dir_for(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("onec-connect-save-state-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// С задачи 6 плана «тонкий срез ценности» переименование рабочего места
/// порт публикации не трогает вовсе: предпочтение (`host.http_port`)
/// переносится как есть, а строка журнала о переезде называет только ветку
/// проекта и адрес отправки — ни числа портов, ни слова «порт» в ней больше нет
/// (до задачи 6 здесь проверялось обратное — что порт едет за именем).
#[test]
fn renaming_the_workstation_does_not_move_the_port_anymore() {
    let dir = state_dir_for("renamed-workstation");
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().http_port = Some(cfg::derived_http_port());
    let mut host = ui.cfg.current().clone();
    host.workstation = "dev3".into();
    crate::ops_host::with_state_dir_for_test(&dir, || save_host(&mut ui, host)).unwrap();
    assert_eq!(ui.cfg.current().http_port, Some(cfg::derived_http_port()), "предпочтение не трогает рабочее место");
    assert!(!ui.log.iter().any(|l| l.text.contains("порт")), "строка о переезде не должна называть портов: {:?}", ui.log);
    assert!(ui.log.iter().any(|l| l.text.contains("ветка проекта и адрес")), "{:?}", ui.log);
    let _ = std::fs::remove_dir_all(&dir);
}

/// До задачи 6 строка журнала называла доставшийся порт, а не предпочтение
/// (находка 6 ревью круга 3) — сценарий требовал, чтобы ПЕРЕИМЕНОВАНИЕ
/// двигало предпочтение, а подбор мог уехать дальше нового предпочтения.
/// С задачи 6 переименование предпочтение не двигает вовсе (порт — константа,
/// не выведенная из имени), и этого сценария больше нет: подбор всё ещё
/// может сдвинуть `resolved_http_port` независимо от рабочего места, но
/// `moved_lines` про порт теперь не говорит ничего — ни про предпочтение,
/// ни про доставшееся.
#[test]
fn renaming_the_workstation_leaves_the_port_out_of_the_moved_line_even_when_the_pick_shifted() {
    let old = Connection { workstation: "dev1".into(), http_port: Some(cfg::derived_http_port()), ..Connection::default() };
    let mut new = old.clone();
    new.workstation = "dev3".into();
    let before = Cfg::of_one(old);
    // Подбор этого сеанса ушёл на другой порт по причине, не связанной с
    // переименованием (например, порт занят соседом) — строка о переезде
    // рабочего места всё равно не должна называть портов.
    let after = Cfg { resolved_http_port: Some(8084), ..Cfg::of_one(new) };
    let line = moved_lines(&before, &after).join("\n");
    assert!(!line.contains("порт") && !line.contains("8084"), "порт не переезжает за рабочим местом: {line}");
    assert!(line.contains("ветка проекта и адрес"), "{line}");
}

/// Находка 5 ревью круга 3: неудавшееся «Сохранить» не двигает запомненный
/// порт. Прежде улаживание стояло ДО `validate()`/`write_checked` — человек
/// менял рабочее место и заодно портил поле, запись не проходила, toml
/// оставался прежним, а `ports.state` уже уезжал на предпочтение, которого в
/// файле человека нет.
#[test]
fn a_refused_save_does_not_remember_a_new_port() {
    let dir = state_dir_for("refused-save");
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().http_port = Some(cfg::derived_http_port());
    let mut host = ui.cfg.current().clone();
    host.workstation = "dev3".into();
    host.server = "no-port".into(); // `validate()` на этом откажет
    let err = crate::ops_host::with_state_dir_for_test(&dir, || save_host(&mut ui, host)).unwrap_err();
    assert!(!err.is_empty());
    assert!(!dir.join("ports.state").exists(), "настройки не записаны — запоминать выбор не за что");
}

/// Та же ловушка на живом «Сохранить»: записанный выведенный каталог
/// выгрузки переезжает вместе с каталогом проекта.
#[test]
fn saving_a_new_project_moves_a_derived_dump_with_it() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().project = "C:\\proj\\Buh".into();
    ui.cfg.current_mut().dump = cfg::derived_dump("C:\\proj\\Buh");
    let mut host = ui.cfg.current().clone();
    host.project = "C:\\proj\\Torg".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.cfg.current().dump, cfg::derived_dump("C:\\proj\\Torg"));
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains(&format!("dump = {:?}", cfg::derived_dump("C:\\proj\\Torg"))), "{text}");
    // И переезд назван, а не сделан молча.
    assert!(ui.log.iter().any(|l| l.text.contains("переехал каталог выгрузки")), "{:?}", ui.log);
}

/// Правило одно на все переезды: о каждом — строка. За рабочим местом едут
/// только ветка и origin (с задачи 6 — порты и порт публикации больше не
/// едут, это константы); за каталогом проекта — каталог выгрузки; ничего не
/// менялось — строк нет.
#[test]
fn every_move_gets_a_line_including_the_branch_but_not_the_port() {
    let old = Connection {
        workstation: "dev1".into(),
        project: "C:\\proj\\Buh".into(),
        dump: cfg::derived_dump("C:\\proj\\Buh"),
        http_port: Some(cfg::derived_http_port()),
        ..Connection::default()
    };
    assert!(moved_lines(&as_cfg(&old), &as_cfg(&old)).is_empty());

    let mut new = old.clone();
    new.workstation = "dev3".into();
    let line = moved_lines(&as_cfg(&old), &as_cfg(&new)).join("\n");
    for part in ["dev1 → dev3", "ветка проекта и адрес"] {
        assert!(line.contains(part), "{part} не назван: {line}");
    }
    assert!(!line.contains("порт"), "порт больше не едет за рабочим местом: {line}");

    let mut moved_project = old.clone();
    moved_project.project = "C:\\proj\\Torg".into();
    moved_project.dump = carried_dump(&old, "C:\\proj\\Torg");
    let lines = moved_lines(&as_cfg(&old), &as_cfg(&moved_project));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(&cfg::derived_dump("C:\\proj\\Torg")), "{lines:?}");

    // Свой каталог выгрузки не переезжает — и строки про него нет.
    let mut own = old.clone();
    own.dump = "D:\\dump".into();
    let mut own_new = own.clone();
    own_new.project = "C:\\proj\\Torg".into();
    own_new.dump = carried_dump(&own, "C:\\proj\\Torg");
    assert!(moved_lines(&as_cfg(&own), &as_cfg(&own_new)).is_empty());
}

/// Круг починки 1, находка B: «Сохранить» правит ТЕКУЩЕЕ соединение, а не
/// подменяет собой весь список. `cfg`, собранная из полей формы, несёт одно
/// соединение, и прежнее `ui.cfg = cfg` стирало человеку все остальные
/// карточки — в памяти окна, молча. Диск этим не лечится и не должен:
/// `render` печатает одну секцию `[host]` без имени и без `current`, запись
/// списка целиком — задача 4.
#[test]
fn saving_edits_the_current_connection_and_keeps_the_others() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    let current_name = ui.cfg.current().name.clone();
    let mut host = ui.cfg.current().clone();
    host.db_user = "mcp".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.cfg.connections.len(), 2, "второе соединение переживает сохранение");
    assert_eq!(ui.cfg.current, current_name, "текущим осталось то же соединение");
    assert_eq!(ui.cfg.current().db_user, "mcp", "правка доехала до текущего");
    let other = ui.cfg.connections.iter().find(|c| c.name == "Ромашка").expect("соседняя карточка на месте");
    assert_eq!(other.base_path, "C:\\Bases\\Ромашка", "и её поля не тронуты");
}

/// Задача 5 плана «Модель соединений», решение владельца 8: учебная лицензия
/// 1С даёт ОДИН сеанс на базу, и вторая карточка к той же базе получила бы
/// отказ ПЛАТФОРМЫ, не нашей программы — человек не понял бы, откуда беда.
/// Отказ здесь сверяет правку с СОСЕДНИМИ карточками `ui.cfg` отдельно от
/// `validate()` и называет имя уже СУЩЕСТВУЮЩЕЙ карточки, а не той, что
/// сохраняют, — и говорит это словами ОКНА («карточка»), а не модели
/// («соединение»): с задачи 10 `validate()` внутри `save_host` видит весь
/// список и тот же запрет ловит сама, но на своём языке. Разница
/// в написании — только хвостовой разделитель, без смены регистра: тест
/// обязан проходить и на Linux.
#[test]
fn saving_to_a_base_another_card_already_holds_is_refused_by_its_name() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
    let mut host = ui.cfg.current().clone();
    host.base_path = "C:\\Bases\\Ромашка\\".into();
    let err = save_host(&mut ui, host).unwrap_err();
    assert!(err.contains("Ромашка"), "отказ называет имя уже занявшей базу карточки: {err}");
    assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before, "файл не тронут");
}

#[test]
fn saving_a_new_base_restarts_the_session_and_writes_one_journal_line() {
    let mut ui = Ui::for_test();
    // "webserver": ensure_publication_now (зовётся теперь и отсюда) не
    // должна трогать настоящий httpd/1cv8 этой машины — тест про
    // перезапуск сеанса, не про публикацию.
    ui.cfg.current_mut().publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.current().clone();
    host.base_path = "C:\\Bases\\Other".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.session_gen, gen_before + 1, "сеанс перезапущен один раз");
    let new_lines = &ui.log[log_before..];
    assert_eq!(new_lines.len(), 1, "{new_lines:?}");
    assert_eq!(new_lines[0].text, "MCP перезапущены: база C:\\Bases\\Other");
    assert!(ui.session.is_some());
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn saving_a_new_base_in_local_mode_keeps_the_session_and_does_not_restart() {
    // Блокирующая находка целостного ревью: раньше «Сохранить» рвало
    // слушатели и принятые соединения в локальном режиме, и человеку
    // приходилось переподключать два сервера в Zed руками. Спека прямо
    // требует, чтобы это было незаметно: слушатели не трогаются, сеанс
    // не перезапускается, следующий вызов уходит в новую базу через
    // `Session::update_cfg`.
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().mode = "local".into();
    ui.cfg.current_mut().publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.current().clone();
    host.base_path = "C:\\Bases\\Other".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.session_gen, gen_before, "сеанс не перезапущен");
    let new_lines = &ui.log[log_before..];
    assert_eq!(new_lines.len(), 1, "{new_lines:?}");
    assert_eq!(new_lines[0].text, "MCP: параметры обновлены, база C:\\Bases\\Other");
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn saving_unchanged_fields_does_not_restart_a_live_session() {
    // Смена режима в трее меняет `cfg.current().mode` и зовёт `save_host`
    // напрямую (не через панель), не трогая поля, которые показывает
    // окно — `tray::on_menu` перезапускает сеанс сама следом, отдельным
    // путём; лишнего перезапуска (и лишней строки «MCP перезапущены») на
    // стороне `save_host` здесь быть не должно.
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.current().clone();
    host.mode = "local".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.session_gen, gen_before);
    assert_eq!(ui.log.len(), log_before);
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn save_host_without_a_live_session_writes_no_restart_line() {
    // Поднятого сеанса нет — перезаписать toml, и всё; журнал не трогаем.
    // "webserver": ensure_publication_now не должна трогать настоящий
    // httpd/1cv8 этой машины.
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().publication = "webserver".into();
    let log_before = ui.log.len();
    let mut host = ui.cfg.current().clone();
    host.base_path = "C:\\Bases\\Other".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.log.len(), log_before);
}

/// «Сохранить», убирающее холодное условие на живом обслуживании,
/// спрашивает тем же диалогом, что и смена базы. Живого замка здесь нет
/// (в каталоге теста не разложены ни инструменты, ни платформа), поэтому
/// проверяется та же пара, что работает в окне: `cold_gate` на новых
/// полях и `locks_gate` поверх неё.
/// Запись параметров не поднимает HTTP-сервис базы, пока никто не
/// обслуживает. Заказчик 2026-10-21: «сохранил — в журнале сообщение об
/// успешном поднятии http сервера, хотя я даже не пытался включить MCP».
/// Смена базы при выключенных MCP — не повод делать работу, которой не
/// просили, и тем более объявлять её успехом.
#[test]
fn saving_does_not_raise_the_publication_while_nobody_serves() {
    assert!(!republish_after_save(true, false), "MCP выключены — публикацию не трогаем");
    assert!(republish_after_save(true, true), "обслуживаем — сервис обязан смотреть на новую базу");
    assert!(!republish_after_save(false, true), "база та же — поднимать нечего");
}

#[test]
fn saving_away_a_cold_condition_asks_first() {
    use crate::gui::gate::cold_gate;
    let v8 = std::path::Path::new("1cv8.exe");
    let tools = OpState::Done("vrunner 3.0.2".into());
    let before = crate::cfg::filled_for_test();
    let mut after = crate::cfg::filled_for_test();
    after.current_mut().db_user = String::new();
    let gate_before = cold_gate(&before, true, true, &tools, Ok(v8), true, &crate::licence::State::Valid);
    let gate_after = cold_gate(&after, true, true, &tools, Ok(v8), true, &crate::licence::State::Valid);
    assert_eq!(gate_before, None, "до правки включать было можно");
    let why = locks_gate(gate_before.as_deref(), gate_after.clone(), true).expect("вопрос обязателен");
    assert!(why.contains("пользователь базы"), "{why}");
    // Обслуживания нет — гасить нечего, вопрос не задаём.
    assert_eq!(locks_gate(gate_before.as_deref(), gate_after.clone(), false), None);
    // Заперто и до правки — тоже молчим: обслуживание уже погашено.
    assert_eq!(locks_gate(Some("в комплекте нет mcp-1c"), gate_after.clone(), true), None);
    // Правка ничего не убрала — обычное «Сохранить» без вопроса.
    assert_eq!(locks_gate(gate_before.as_deref(), None, true), None);
    // Диалог называет последствие, а не одно «Сохранить?».
    let (message, detail) = confirm_gate_text(&why);
    assert!(message.contains("MCP выключатся"), "{message}");
    assert!(detail.contains("пользователь базы"), "{detail}");

    // И то же через настоящий вход окна. Поле берём такое, которого нет в
    // списке «сменилась ли база» (`database_changed`), иначе вопрос
    // пришёл бы по старому поводу и ничего бы не доказал.
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    ui.gate = None;
    let mut host = ui.cfg.current().clone();
    host.server = String::new();
    assert!(save_needs_confirm(&ui, &host), "туннель без адреса среды — обслуживание погаснет, вопрос обязателен");
    ui.gate = Some("в комплекте нет mcp-1c".into());
    assert!(!save_needs_confirm(&ui, &host), "замок заперт и так — гасить нечего");
    let _ = ui.session.take().map(|s| s.stop());
}

/// Проводка гашения: «Сохранить», убравшее холодное условие, снимает флаг,
/// переводит карточку в «выключено» и пишет строку. Без последнего человек
/// видел бы зелёное «MCP включены» и переключатель, который нарисован
/// включённым, но на щелчок не отвечает (находка ревью).
#[test]
fn saving_away_a_cold_condition_douses_the_card_too() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().mode = "local".into();
    ui.cfg.current_mut().publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    ui.link = State::Up { since: Instant::now() };
    // До правки замок был открыт — тот самый случай, ради которого
    // «Сохранить» и спрашивает подтверждение (в каталоге теста он заперт
    // сам по себе, поэтому состояние задаём явно).
    ui.gate = None;
    let log_before = ui.log.len();
    let mut host = ui.cfg.current().clone();
    host.db_user = String::new();
    save_host(&mut ui, host).unwrap();
    assert!(ui.gate.is_some(), "после правки условие пропало");
    assert!(!ui.enabled.load(Ordering::Relaxed), "обслуживание погашено");
    assert_eq!(ui.link, State::Off, "карточка и переключатель обязаны показать «выключено»");
    // Строка про гашение ровно одна (про обновлённые параметры рядом —
    // своя и законная: это другое событие).
    let doused: Vec<&Stamped> = ui.log[log_before..].iter().filter(|l| l.text.starts_with("MCP выключены:")).collect();
    // Какое именно условие назовёт строка, решает порядок `cold_gate`, а в
    // каталоге теста не выполнено сразу несколько — здесь проверяется сам
    // факт гашения со строкой, а список условий стережёт `gate::tests`.
    assert_eq!(doused.len(), 1, "{:?}", &ui.log[log_before..]);
    let _ = ui.session.take().map(|s| s.stop());
}

#[test]
fn save_needs_confirm_only_when_all_three_hold() {
    // Три условия сразу: база меняется, MCP включены, сеанс жив —
    // вопрос нужен.
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит флаг включённым");
    let mut new_base = ui.cfg.current().clone();
    new_base.base_path = "C:\\Bases\\Other".into();
    assert!(save_needs_confirm(&ui, &new_base));
    let _ = ui.session.take().unwrap().stop();
}

/// Ядро шага 5 (задача 6): два потребителя одного вопроса «что считать
/// сменой базы», и у них разные списки не по ошибке, а по решению — без
/// этого теста следующая правка сольёт `descriptor_changed` и
/// `database_changed` обратно незаметно (ревью задачи 6, Important 2).
/// Смена пароля или логина — смена базы для человека и для сеанса
/// (`database_changed`, вопрос `save_question`/перезапуск), но не для
/// дескриптора публикации (`descriptor_changed`, `publication::vrd`): с
/// задачи 6 он пароль и логин не несёт вовсе, и переподнимать публикацию
/// ради них незачем. Путь базы — единственное поле, на которое согласны
/// оба списка.
#[test]
fn descriptor_changed_ignores_credentials_but_database_changed_does_not() {
    let base = Connection {
        base_path: "C:\\Bases\\Buh".into(),
        db_user: "Администратор".into(),
        db_password: "секрет".into(),
        ..Connection::default()
    };

    let mut password_only = base.clone();
    password_only.db_password = "новый-пароль".into();
    assert!(database_changed(&password_only, &base), "смена пароля — смена базы для вопроса человеку и сеанса");
    assert!(!descriptor_changed(&password_only, &base), "дескриптор пароль не несёт — переподнимать публикацию незачем");

    let mut user_only = base.clone();
    user_only.db_user = "Другой".into();
    assert!(database_changed(&user_only, &base));
    assert!(!descriptor_changed(&user_only, &base));

    let mut path_changed = base.clone();
    path_changed.base_path = "C:\\Bases\\Other".into();
    assert!(database_changed(&path_changed, &base));
    assert!(descriptor_changed(&path_changed, &base), "путь базы — то единственное, что дескриптор несёт");
}

/// Та же смена пароля через настоящий вход окна: `save_needs_confirm`
/// (вопрос человеку) завязан на `database_changed`, не на `descriptor_changed`
/// — смена одного пароля обязана спросить то же самое, что и смена пути.
#[test]
fn save_needs_confirm_fires_on_a_password_only_change_too() {
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let mut new_password = ui.cfg.current().clone();
    new_password.db_password = "другой-пароль".into();
    assert!(save_needs_confirm(&ui, &new_password));
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn save_needs_confirm_is_false_on_each_near_miss() {
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let mut new_base = ui.cfg.current().clone();
    new_base.base_path = "C:\\Bases\\Other".into();

    // Промах 1: MCP выключены.
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!save_needs_confirm(&ui, &new_base));
    ui.enabled.store(true, Ordering::Relaxed);

    // Промах 2: база та же, поменялось другое поле — оно не входит в
    // список, что решает «изменилась ли база» (`database_changed`).
    let mut other_field = ui.cfg.current().clone();
    other_field.project = "D:\\other\\proj".into();
    assert!(!save_needs_confirm(&ui, &other_field));

    // Промах 3: сеанса нет.
    let session = ui.session.take().unwrap();
    assert!(!save_needs_confirm(&ui, &new_base));
    let _ = session.stop();
}

#[test]
fn declining_the_confirmation_leaves_the_toml_untouched() {
    // Пока вопрос не задан и не отвечен «да», «Сохранить» не должен
    // звать `save_host` вовсе — отказ ничего не пишет, поля окна
    // остаются как человек их оставил (сюда это проверяется тем, что
    // toml не тронут без явного вызова `save_host`).
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
    let mut new_base = ui.cfg.current().clone();
    new_base.base_path = "C:\\Bases\\Other".into();
    assert!(save_needs_confirm(&ui, &new_base), "сценарий должен требовать вопрос");
    // Отказ = не позвать save_host; toml как был.
    assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before);
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn confirm_text_names_folders_and_keeps_full_paths_in_the_detail() {
    let (message, detail) = confirm_text("C:\\Bases\\Buh", "C:\\Bases\\УНФ");
    assert_eq!(message, "Агент сейчас работает с базой «Buh», после сохранения перейдёт на «УНФ». Сохранить?");
    assert_eq!(detail, "C:\\Bases\\Buh → C:\\Bases\\УНФ");
}

/// Круг починки 3 задачи 16: прежний текст («Сохранить?») врал про кнопку,
/// которую человек не нажимал — «Сделать текущим» получало слово
/// «Сохранить». Текст обязан называть карточку (разработчик с десятью
/// заказчиками должен видеть, какую выбрал), последствие (агент перестанет
/// отвечать по прежней базе — сеанс перезапустится) и исход отказа
/// (останется как было), и не называть переключение ошибкой или сбоем.
#[test]
fn confirm_switch_current_text_names_the_card_not_an_error() {
    let (message, detail) = confirm_switch_current_text("Ромашка", "C:\\Bases\\Buh", "C:\\Bases\\Ромашка");
    assert!(message.contains("«Ромашка»"), "{message}");
    assert!(message.contains("перестанет отвечать по прежней базе"), "{message}");
    assert!(message.contains("перезапустится"), "{message}");
    assert!(message.contains("как было"), "{message}");
    assert!(!message.to_lowercase().contains("ошиб"), "переключение — не поломка: {message}");
    assert!(!message.to_lowercase().contains("сбо"), "переключение — не поломка: {message}");
    assert!(!message.contains("Сохранить"), "человек нажал «Сделать текущим», не «Сохранить»: {message}");
    assert_eq!(detail, "C:\\Bases\\Buh → C:\\Bases\\Ромашка");
}

#[test]
fn what_a_person_typed_is_what_gets_written() {
    let saved = Connection { publication: "webserver".into(), ..Connection::default() };
    assert_eq!(base_url_to_write(&saved, "http://localhost/Buh", "webserver", "C:\\Bases\\Buh"), "http://localhost/Buh/hs/mcp-1c");
    // Пустое поле остаётся пустым: выдуманного адреса здесь не появляется.
    assert_eq!(base_url_to_write(&saved, "   ", "webserver", "C:\\Bases\\Buh"), "");
}

#[test]
fn an_own_address_survives_a_workstation_move() {
    let saved = Connection { publication: "webserver".into(), base_url: "http://localhost/UNF/hs/mcp-1c".into(), ..Connection::default() };
    assert_eq!(base_url_to_write(&saved, "http://localhost/UNF", "webserver", "C:\\Bases\\Other"), "http://localhost/UNF/hs/mcp-1c");
}

#[test]
fn a_foreign_address_waits_through_a_round_trip_to_our_own_httpd() {
    // Переключились на «публикует коннектор» и обратно — вписанное не стёрто.
    let saved = Connection {
        publication: "auto".into(),
        base_url: "http://localhost/UNF/hs/mcp-1c".into(),
        base_path: "C:\\Bases\\UNF".into(),
        ..Connection::default()
    };
    assert_eq!(base_url_to_write(&saved, "", "auto", "C:\\Bases\\UNF"), "http://localhost/UNF/hs/mcp-1c");
}

#[test]
fn in_auto_mode_the_address_still_follows_the_base_folder() {
    let saved = Connection {
        publication: "auto".into(),
        base_path: "C:\\Bases\\Buh".into(),
        base_url: cfg::derived_base_url("C:\\Bases\\Buh"),
        ..Connection::default()
    };
    assert_eq!(base_url_to_write(&saved, "", "auto", "C:\\Bases\\UNF"), cfg::derived_base_url("C:\\Bases\\UNF"));
}

#[test]
fn a_tray_key_is_written_even_when_the_rest_is_not_filled_in() {
    // Свежий комплект: поля пусты, и `save_host` на них отказывает. Но
    // переключатель в меню значка — намерение человека, а не действие над
    // базой, и оно обязано пережить открытие окна: окно читает файл при
    // каждом показе, так что «в памяти» такое намерение не живёт вовсе.
    let mut ui = Ui::for_test();
    *ui.cfg.current_mut() = Connection::default();
    let mut host = ui.cfg.current().clone();
    host.publication = "webserver".into();
    assert!(Cfg::of_one(host.clone()).validate().is_err(), "набор нарочно негодный");
    save_tray_key(&mut ui, "publication", "webserver").unwrap();
    assert_eq!(ui.cfg.current().publication, "webserver");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("publication = \"webserver\""), "{text}");
    // Круг починки 1 задачи 4: записанный из образца файл называет блок и
    // `current` ТЕКУЩИМ соединением окна, а не подстановкой образца, — иначе
    // следующая запись не нашла бы в нём своего блока и отказала бы.
    let back: Cfg = toml::from_str(&text).expect("файл из образца разбирается");
    assert_eq!(back.current, ui.cfg.current, "{text}");
    assert_eq!(back.current().publication, "webserver", "ключ лёг в блок этого соединения");
    assert!(back.read_notes.is_empty(), "{:?}", back.read_notes);
}

/// Critical 2 ревью задачи 7: `read_to_string(..).unwrap_or_else(|_| EXAMPLE)` не отличала «файла нет» от любой другой причины отказа (антивирус держит файл, нет прав, временный сбой диска) — и в обоих случаях следующая строка писала образец ПОВЕРХ существующего файла, стирая оба шифртекста молча. Права 0o200 (писать можно, читать нельзя) — ровно такой разрыв без chmod каталога: `read_to_string` откажет не `NotFound`, а старый код после этого как ни в чём не бывало писал бы образец — право на запись у файла есть. Под root права не действуют — тест бессмыслен, пропускаем, как и соседний тест `secrets::tests::a_read_only_state_dir_is_a_named_refusal`.
#[cfg(unix)]
#[test]
fn an_unreadable_but_existing_file_refuses_instead_of_overwriting() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let mut ui = Ui::for_test();
    let path = ui.kit.state_path(cfg::FILE);
    let before = "[host]\nserver = \"настоящие данные\"\n";
    std::fs::write(&path, before).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o200)).unwrap();
    let result = save_tray_key(&mut ui, "mode", "local");
    // Права вернуть до любых проверок — иначе отказавший `assert!` оставит файл нечитаемым, и следующая же попытка его прочитать (хоть здесь, хоть в чистке temp-каталога) наткнётся на ту же самую причину, что проверяет этот тест.
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    assert!(result.is_err(), "{result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "файл не тронут ни на байт");
}

/// Находка безопасности задачи 4 плана «Модель соединений»: переключатель в
/// трее писал ключ в `[host]`, а `toml_render::set` при отсутствии секции
/// дописывала её в конец файла — на файле с блоками `[[connection]]`
/// записанное не читал никто (чтение берёт список), а сам файл с обеими
/// половинами стал с задачи 3 отказом при чтении. То есть один щелчок по
/// «Запись в базу» делал настройки нечитаемыми, а человек видел ужесточение,
/// которое перезапуск тихо откатывал.
#[test]
fn a_tray_key_lands_in_the_current_connections_block_and_keeps_the_file_readable() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let mut two = cfg::cfg_with(&["Buh", "Ромашка"]);
    two.current = "Ромашка".into();
    std::fs::write(ui.kit.state_path(cfg::FILE), two.render(&store).unwrap()).unwrap();
    ui.cfg = two;
    let neighbour = ui.cfg.connections[0].base_url.clone();
    save_tray_key(&mut ui, "base_url", "http://localhost/Buh/hs/mcp-1c").unwrap();
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(!text.contains("[host]"), "обеих половин в файле быть не должно — такой файл чтение отвергает:\n{text}");
    let back: Cfg = toml::from_str(&text).expect("файл после записи разбирается");
    assert!(back.read_notes.is_empty(), "и читается без находок: {:?}", back.read_notes);
    assert_eq!(back.current().base_url, "http://localhost/Buh/hs/mcp-1c", "ключ попал в блок текущего соединения");
    assert_eq!(back.connections[0].base_url, neighbour, "а соседний блок не тронут");
}

/// Потолок машины ложится в КОРЕНЬ файла и не задевает ни блоки соединений,
/// ни чужой шифртекст: он пишется тем же приёмом «правим текст файла, а не
/// перерисовываем его» (`save_tray_ceiling`), и обе опасности у него те же,
/// что у ключей карточки, — поэтому и проверены тем же способом.
#[test]
fn the_machine_ceiling_lands_in_the_root_and_keeps_the_rest_of_the_file() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let mut host = ui.cfg.current().clone();
    host.password = "секрет с другой машины".into();
    let rendered = Cfg::of_one(host).render(&store).unwrap();
    std::fs::write(ui.kit.state_path(cfg::FILE), &rendered).unwrap();
    let password_line = rendered.lines().find(|l| l.starts_with("password = ")).unwrap().to_string();
    ui.cfg.current_mut().password = String::new();

    save_tray_ceiling(&mut ui, "read_write").unwrap();

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(after.lines().any(|l| l == password_line), "шифртекст пароля должен остаться дословно:\n{after}");
    let root = after.split(cfg::CONNECTION).next().unwrap_or_default();
    assert!(root.contains("write = \"read_write\""), "потолок — корневой ключ, до первого блока:\n{after}");
    let back: Cfg = toml::from_str(&after).expect("файл после записи разбирается");
    assert_eq!(back.access(), "read_write");
    assert_eq!(back.current().write, "", "право записи карточки щелчком потолка не меняется");
}

/// Important 1 ревью задачи 2: переключатель в трее правит ОДИН ключ текстом,
/// а не перерисовывает файл через `render` — иначе шифртекст пароля, который
/// `ui.cfg` прочитал пустым (чужая машина или чужая учётка, `load_unchecked`),
/// заменился бы пустой строкой при первом же нажатии в меню значка, и то, что
/// ещё можно было открыть, вернувшись на свою машину, было бы потеряно.
#[test]
fn a_tray_key_write_does_not_touch_an_unreadable_password() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let mut host = ui.cfg.current().clone();
    host.password = "секрет с другой машины".into();
    let rendered = Cfg::of_one(host).render(&store).unwrap();
    std::fs::write(ui.kit.state_path(cfg::FILE), &rendered).unwrap();
    let password_line = rendered.lines().find(|l| l.starts_with("password = ")).unwrap().to_string();
    // «Чужая машина»: `ui.cfg` несёт пустое поле, как после `load_unchecked`
    // на непрочитанном шифртексте — без настоящего похода за DPAPI в тесте.
    ui.cfg.current_mut().password = String::new();
    save_tray_key(&mut ui, "mode", "local").unwrap();
    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(after.lines().any(|l| l == password_line), "шифртекст пароля должен остаться дословно:\n{after}");
}

/// Блокер ревью задачи 4: `save_host` способна была записать пустой пароль
/// поверх нерасшифрованного шифртекста. Сценарий тот же, ради которого
/// писана вся задача 4: файл переехал на другую машину, `load_unchecked`
/// отдала пустое поле и список `undecryptable`, окно сказало об этом
/// словами. Человек правит что-то другое (здесь — рабочее место) и жмёт
/// «Сохранить»; форма отдаёт пустой пароль, и без защиты обычная запись
/// зашифровала бы эту пустую строку, уничтожив шифртекст, который ещё
/// можно было открыть под прежней учёткой, — тот самый исход хуже открытого
/// текста, против которого писана задача 4 (замысел, «Анти-цели»).
#[test]
fn an_unreadable_password_survives_a_save_with_an_empty_field() {
    let mut ui = Ui::for_test();
    // Настоящая «чужая машина» — другой каталог, другой ключ (см.
    // `foreign_store_dir`): та подложка, что возьмёт сам `save_host`
    // (`ui.kit.state()`), эту строку не откроет, и проверка после записи
    // внутри `write_checked` увидит то же пустое поле, что и до неё.
    let store = crate::secrets::Store::file(&foreign_store_dir("survives"));
    let mut seeded = ui.cfg.current().clone();
    seeded.password = "секрет с другой машины".into();
    let rendered = Cfg::of_one(seeded).render(&store).unwrap();
    std::fs::write(ui.kit.state_path(cfg::FILE), &rendered).unwrap();
    let password_line = rendered.lines().find(|l| l.starts_with("password = ")).unwrap().to_string();

    // «Чужая машина»: поле — пустое, имя — в списке, в точности как после
    // настоящего `load_unchecked` на непрочитанном шифртексте (без похода за
    // DPAPI в тесте — та же договорённость, что у соседнего теста трея выше).
    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new(&ui.cfg.current().name.clone(), "password")];

    let mut host = ui.cfg.current().clone();
    host.workstation = "dev2".into();
    save_host(&mut ui, host).unwrap();

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(after.lines().any(|l| l == password_line), "шифртекст пароля должен остаться дословно:\n{after}");
    assert!(ui.log.iter().any(|l| l.text.contains("оставлен прежним")), "журнал должен сказать, что пароль оставлен прежним: {:?}", ui.log);
}

/// Другая половина того же блокера: заполненное поле заменяет шифртекст как
/// раньше. Имя поля остаётся в `undecryptable` до следующего перечитывания
/// файла (окно о вводе ещё не знает), но форма отдала непустую строку —
/// значит человек ввёл новый пароль, и стирать есть что заменить.
#[test]
fn a_freshly_typed_password_still_overwrites_an_unreadable_one() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let mut seeded = ui.cfg.current().clone();
    seeded.password = "секрет с другой машины".into();
    let rendered = Cfg::of_one(seeded).render(&store).unwrap();
    std::fs::write(ui.kit.state_path(cfg::FILE), &rendered).unwrap();
    let old_password_line = rendered.lines().find(|l| l.starts_with("password = ")).unwrap().to_string();

    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new(&ui.cfg.current().name.clone(), "password")];

    let mut host = ui.cfg.current().clone();
    host.password = "новый пароль рабочего места".into();
    save_host(&mut ui, host).unwrap();

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(!after.lines().any(|l| l == old_password_line), "новый пароль введён — старый шифртекст не обязан остаться:\n{after}");
    assert!(after.contains("password = \"enc1:"), "новое значение тоже зашифровано:\n{after}");
    assert!(!ui.log.iter().any(|l| l.text.contains("оставлен прежним")), "пароль заменили, а не оставили: {:?}", ui.log);
}

/// Пункт 2 ревью, круг 2: строка «пароль оставлен прежним» печаталась по
/// списку, а не по факту — если прежний литерал не нашёлся, патч молча не
/// применялся, а человек читал, что пароль цел, хотя он уже потерян. Ключа
/// `password` в файле нет вовсе (строку стёрли руками, или файл правили не
/// нашей программой) — `resolve_patches` обязана отказать СЛОВАМИ до любой
/// записи, и файл остаётся нетронутым ни на байт: человек видит причину и
/// ничего не теряет, вместо тихой подмены на пустоту.
#[test]
fn a_missing_previous_ciphertext_refuses_the_save_and_touches_nothing() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let mut seeded = ui.cfg.current().clone();
    seeded.password = "секрет с другой машины".into();
    let rendered = Cfg::of_one(seeded).render(&store).unwrap();
    // Ключа `password` в файле нет вовсе — как если бы строку стёрли руками.
    let without_password: String = rendered.lines().filter(|l| !l.starts_with("password = ")).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(ui.kit.state_path(cfg::FILE), &without_password).unwrap();

    // «Чужая машина» + «ключа больше нет»: поле пустое, имя в списке — как
    // после настоящего `load_unchecked`, который на этом файле нашёл бы то же
    // самое (пустое значение по умолчанию, когда ключа нет, и т.к. оно всё
    // равно расшифровалось бы в пустую строку, не попало бы в `undecryptable`
    // само — мы доводим это условие руками, тем же приёмом, что у соседних
    // тестов, ради сценария «список говорит про поле, а в файле его нет»).
    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new(&ui.cfg.current().name.clone(), "password")];

    let mut host = ui.cfg.current().clone();
    host.workstation = "dev2".into();
    let err = save_host(&mut ui, host).unwrap_err();
    assert!(err.contains("рабочего места"), "{err}");

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert_eq!(after, without_password, "отказ — файл не тронут ни на байт");
    assert!(
        !ui.log.iter().any(|l| l.text.contains("оставлен прежним")),
        "строка не должна звучать, если ничего не подставлено: {:?}",
        ui.log
    );
}

/// Задача 10 плана «Модель соединений»: цель плана — человек может завести
/// второе соединение, — на практике не выполнялась, потому что единственный
/// писатель в продукте («Сохранить») сворачивал настройки в ОДНО соединение
/// (`Cfg::of_one`), и вторая карточка исчезала с диска при первом же
/// сохранении. Утверждение читает ФАЙЛ, а не память: потеря данных случается
/// на диске, и доказывать её надо байтами файла.
#[test]
fn saving_keeps_every_connection_on_disk_not_just_the_current_one() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    ui.cfg = cfg::cfg_with(&["Buh", "Ромашка"]);
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::write(&path, ui.cfg.render(&store).unwrap()).unwrap();

    // Правим ТЕКУЩЕЕ соединение — ровно то, что делает человек в окне.
    let mut host = ui.cfg.current().clone();
    host.workstation = "dev2".into();
    save_host(&mut ui, host).unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 2, "на диске обязаны остаться обе карточки:\n{after}");
    assert_eq!(
        common::toml_render::get_in(&after, cfg::CONNECTION, 1, "name").as_deref(),
        Some("Ромашка"),
        "вторая карточка — на своём месте и под своим именем:\n{after}"
    );
    assert_eq!(
        common::toml_render::get_in(&after, cfg::CONNECTION, 0, "workstation").as_deref(),
        Some("dev2"),
        "правка текущей карточки доехала до диска:\n{after}"
    );
}

/// Круг починки 1/5 задачи 10, находка 1: пока «Сохранить» сворачивала
/// настройки в одно соединение, негодная соседняя карточка ставила выбор
/// «отказать или молча унести её с диска». Теперь есть третий исход — писать
/// весь список, а полноту спрашивать у текущей карточки (`Cfg::validate`), —
/// и он проверяется с двух концов: отказа нет, и соседняя карточка цела на
/// ДИСКЕ, вместе со своим незаполненным полем.
#[test]
fn a_half_filled_neighbour_neither_refuses_the_save_nor_disappears() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    ui.cfg = cfg::cfg_with(&["Buh", "Ромашка"]);
    // Так выглядит дописанный руками блок: имя есть, поля ещё не заполнены.
    ui.cfg.connections[1].base_path = String::new();
    ui.cfg.connections[1].project = String::new();
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::write(&path, ui.cfg.render(&store).unwrap()).unwrap();

    let mut host = ui.cfg.current().clone();
    host.workstation = "dev2".into();
    save_host(&mut ui, host).expect("чужая незаполненная карточка не запирает работу с текущей");

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 2, "соседняя карточка цела на диске:\n{after}");
    assert_eq!(
        common::toml_render::get_in(&after, cfg::CONNECTION, 1, "base_path").as_deref(),
        Some(""),
        "и её незаполненное поле осталось незаполненным, а не выдумано:\n{after}"
    );
}

/// Та же запись, но у ЧУЖОЙ карточки пароль — шифртекст, который на этой
/// машине не открывается (файл переехал, другая учётка). Загрузка отдаёт это
/// поле пустым, и запись всего списка, сделанная наивно, зашифровала бы
/// пустую строку поверх чужого шифртекста — потеря того, что ещё можно было
/// открыть, вернувшись на свою машину. Это и есть причина, по которой задача
/// 4 не могла записать весь список сама.
#[test]
fn saving_one_card_leaves_an_unreadable_password_of_another_untouched() {
    let mut ui = Ui::for_test();
    let local = crate::secrets::Store::file(ui.kit.state());
    // Настоящая «другая машина» — другой каталог, другой ключ: та подложка,
    // что возьмёт сам `save_host`, эту строку не откроет (см.
    // `foreign_store_dir`).
    let foreign = crate::secrets::Store::file(&foreign_store_dir("another-card"));
    let sealed_elsewhere = foreign.seal("password", "секрет с другой машины").unwrap();
    let text = cfg::cfg_with(&["Buh", "Ромашка"]).render(&local).unwrap();
    let text = common::toml_render::set_in(&text, cfg::CONNECTION, 1, "password", &format!("{sealed_elsewhere:?}"))
        .expect("во втором блоке есть пароль");
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::write(&path, &text).unwrap();

    // Путь чтения настоящий: окно держит ровно то, что отдала загрузка, —
    // пустое поле у чужой карточки и её имя в списке непрочитанных.
    ui.cfg = Cfg::load_unchecked_with(&ui.kit, &local).unwrap();
    assert_eq!(ui.cfg.connections[1].password, "", "фикстура: пароль чужой карточки не открылся");
    assert!(!ui.cfg.undecryptable.is_empty(), "фикстура: загрузка обязана назвать непрочитанное поле");

    let mut host = ui.cfg.current().clone();
    host.workstation = "dev2".into();
    save_host(&mut ui, host).unwrap();

    // Доказательство адресное, а не «шифртекст где-то в файле есть»: такое
    // утверждение прошло бы и в том случае, если прежний шифртекст уехал в
    // блок ДРУГОЙ карточки — ровно тот отказ, против которого писана
    // адресация по имени (`cfg::block_in`, `cfg::previous_literal`). Поэтому
    // спрашиваем, что он лежит в блоке ЧУЖОЙ карточки, и отдельно — что у
    // текущей пароль свой, а не чужой (находка 2 круга починки 1/5).
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        common::toml_render::get_in(&after, cfg::CONNECTION, 1, "password").as_deref(),
        Some(sealed_elsewhere.as_str()),
        "байты чужого шифртекста обязаны остаться в блоке своей карточки дословно:\n{after}"
    );
    let mine = common::toml_render::get_in(&after, cfg::CONNECTION, 0, "password").expect("у текущей карточки пароль напечатан");
    assert_ne!(mine, sealed_elsewhere, "чужой шифртекст не обязан появиться в блоке текущей карточки:\n{after}");
}

/// Задача 4 плана «Интерфейс»: имя карточки — слова человека, и пустые или
/// пробельные слова адресацию по имени (`set_current`/`previous_literal`)
/// найдут не туда. Отказ — до какой-либо записи.
#[test]
fn an_empty_name_is_refused_with_words() {
    let mut ui = Ui::for_test();
    assert!(rename(&mut ui, "Buh", "   ").unwrap_err().contains("имя"));
}

/// Главная мина задачи (Review Focus плана): если имя уже занято другой
/// карточкой, переименование обязано отказать, а не переписать первую,
/// найденную адресацией по имени. Отказ не трогает файл ни на байт.
#[test]
fn a_name_already_taken_is_refused_and_touches_nothing() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();
    let e = rename(&mut ui, "Buh", "Ромашка").unwrap_err();
    assert!(e.contains("Ромашка"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
}

/// Первую карточку можно НАЗВАТЬ (решение ведущего, третья волна починки по
/// ревью всей ветки). Только что заведённая карточка пуста — ровно такой её
/// отдаёт ответ онбординга, — и `rename` спрашивает у списка правила списка
/// (`Cfg::validate_list`), а не полноту полей.
///
/// Чем это было: `panel::Panel::write` зовёт `rename` ПЕРВЫМ и на его отказе
/// останавливает всё («записывать поля под именем, которого человек не
/// получил, не за что»), а `rename` проверял `ui.cfg`, куда поля из формы
/// ещё не попали — их кладёт `save_host`, до которого дело не доходит.
/// Человек на первом экране заводил карточку, вписывал имя И путь к базе,
/// жал «Сохранить» — и читал отказ «base_path — путь к базе» про поле,
/// которое только что заполнил. Назвать первую карточку было нельзя вовсе.
#[test]
fn a_freshly_created_card_can_be_given_a_name_before_its_fields_are_filled() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.clear();
    ui.cfg.current = String::new();
    let made = add_connection(&mut ui, "accept").unwrap();

    rename(&mut ui, &made, "Бухгалтерия").unwrap();

    assert!(ui.cfg.by_name("Бухгалтерия").is_some(), "новое имя в памяти");
    assert!(ui.cfg.by_name(&made).is_none(), "прежнего имени в памяти больше нет");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("name = \"Бухгалтерия\""), "и в файле:\n{text}");
    assert!(!text.contains(&format!("{made:?}")), "прежнего имени в файле не осталось:\n{text}");
}

/// Ослаблена ТОЛЬКО полнота полей карточки: правила СПИСКА переименование
/// держат целиком. Файл с двумя карточками к одной базе (или с вложенными
/// папками) человек может завести руками, и смена имени в таком файле
/// отказывает, как раньше, — иначе `rename` молча пронесла бы мимо парных
/// запретов то, что `save_host` отвергает.
#[test]
fn renaming_still_obeys_the_rules_of_the_list() {
    // Две карточки к ОДНОЙ базе — запрет учебной лицензии.
    let mut ui = Ui::for_test();
    ui.cfg
        .connections
        .push(Connection { name: "Ромашка".into(), project: "C:\\proj\\Ромашка".into(), ..ui.cfg.current().clone() });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();
    let e = rename(&mut ui, "Ромашка", "Василёк").unwrap_err();
    assert!(e.contains("одна база, одно соединение"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");

    // Папка одной вложена в папку другой — запрет про потерю файлов.
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Buh\\inner".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let e = rename(&mut ui, "Ромашка", "Василёк").unwrap_err();
    assert!(e.contains("папка принадлежит одному соединению"), "{e}");
}

/// Переименование текущей карточки обязано переписать оба места, где живёт
/// её имя: собственный блок и корневой `current` — иначе файл остался бы со
/// ссылкой на имя, которого в списке больше нет.
#[test]
fn renaming_the_current_connection_rewrites_both_the_block_and_the_root_current() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    rename(&mut ui, "Buh", "Ромашка").unwrap();
    assert_eq!(ui.cfg.current, "Ромашка");
    assert_eq!(ui.cfg.current().name, "Ромашка");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("current = \"Ромашка\""), "{text}");
    assert!(text.contains("name = \"Ромашка\""), "{text}");
    assert!(!text.contains("\"Buh\""), "{text}");
}

/// Переименование карточки на её же нынешнее имя — не правка, а её
/// отсутствие (Minor круга починки 1 задачи 4, отложенная до этой задачи
/// нарочно): до этой правки `rename` всё равно читала файл и писала его
/// назад без единого отличия. Из окна этот случай недостижим (`Panel::write`
/// зовёт `rename` только при изменённом имени) — но `rename` — `pub`
/// функция, и прямой вызов с тем же именем остаётся законным входом, не
/// ошибкой программиста; трогать диск под него незачем независимо от того,
/// бережёт ли это сегодня какого-то конкретного вызывающего.
///
/// Строка `name` в фикстуре нарочно переписана БЕЗ пробелов вокруг `=`
/// (`toml_render::set_in` ищет ключ без оглядки на пробелы — `t.trim()`), а
/// канонический вид, который напишет перерисовка, — С пробелами. Запись
/// значения, даже равного прежнему, нормализовала бы строку обратно —
/// простое сравнение «после» с «до» заметило бы эту подмену там и только
/// там, если запись и правда случилась (без неё байты не меняются вовсе, а
/// с ней — меняется пробел, которого иначе взяться было бы неоткуда).
#[test]
fn renaming_a_card_to_its_own_name_touches_no_file() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let canonical = std::fs::read_to_string(&path).unwrap();
    let before = canonical.replacen("name = \"Buh\"", "name=\"Buh\"", 1);
    assert_ne!(before, canonical, "фикстура: строка имени и правда нашлась и была переписана");
    std::fs::write(&path, &before).unwrap();

    rename(&mut ui, "Buh", "Buh").unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "имя не изменилось — файл не тронут ни байтом (перепись нормализовала бы пробелы вокруг «=»)"
    );
    assert_eq!(ui.cfg.current().name, "Buh");
}

/// Круг починки 1 задачи 7, Minor 3: «нет такой карточки» отвечает на
/// другой вопрос, чем «переименование на своё же имя», и обязан прозвучать
/// первым. До этой правки ранний `Ok(())` (тест выше) стоял раньше проверки
/// существования, и `rename(ui, "Нет", "Нет")` молча удавался бы — совпали
/// имена, а не то, что карточка нашлась.
#[test]
fn renaming_a_card_that_does_not_exist_is_refused_even_when_the_names_match() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = rename(&mut ui, "Нет", "Нет").unwrap_err();

    assert!(e.contains("в списке нет карточки «Нет»"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
}

/// Та же запись, но карточка НЕ текущая (список важен для адресации —
/// задача 2): корневой `current` трогать не должно, а переименованная
/// карточка переживает запись со своими полями.
#[test]
fn renaming_a_connection_that_is_not_current_leaves_the_root_current_alone() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    rename(&mut ui, "Ромашка", "Василёк").unwrap();
    assert_eq!(ui.cfg.current, "Buh", "текущим осталось прежнее соединение");
    let renamed = ui.cfg.connections.iter().find(|c| c.name == "Василёк").expect("карточка переименована в памяти");
    assert_eq!(renamed.base_path, "C:\\Bases\\Ромашка", "поля карточки не тронуты, тронуто только имя");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("current = \"Buh\""), "{text}");
    assert!(text.contains("name = \"Василёк\""), "{text}");
    assert!(!text.contains("\"Ромашка\""), "{text}");
}

// Круг починки 1, находка Important 1 ревью: прежний файл с одной секцией
// `[host]` — форма эталонного комплекта (`C:\dev1c-connect-kit`). До правки
// `rename` писала новое имя в `name` этой секции, а следующее чтение
// немедленно затирало его выводом из каталога базы — переименование не
// приживалось. Тест `renaming_sticks_on_the_old_single_host_file_shape`,
// доказывавший починку, удалён задачей 15 плана «Интерфейс» вместе с самой
// секцией: файл такой формы теперь отказ словами ещё при разборе, и `rename`
// на нём просто не вызвать — `ui.cfg` неоткуда взять.

/// Задача 5 плана «Интерфейс»: умолчание новой карточки — профиль, выбранный
/// для предыдущей (текущей на момент добавления).
#[test]
fn a_new_card_takes_the_profile_of_the_previous_one() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().profile = "developer".into();
    let name = add_connection(&mut ui, "accept").unwrap();
    assert_eq!(ui.cfg.by_name(&name).expect("карточка добавлена").profile, "developer");
}

/// Добавление — не переключение: агент, который прямо сейчас отвечает из
/// своей базы, не обязан молча перескочить на пустую новую карточку.
#[test]
fn adding_a_card_does_not_switch_the_current_base() {
    let mut ui = Ui::for_test();
    let was = ui.cfg.current().name.clone();
    add_connection(&mut ui, "accept").unwrap();
    assert_eq!(ui.cfg.current().name, was, "переключение текущей — отдельное действие человека");
}

/// Первый экран продукта: список соединений ПУСТ (свежий комплект — файла
/// настроек нет вовсе, `disk_count == 0`), человек жмёт ответ онбординга.
/// Решение ведущего второй волны починки: это обязано заводить карточку, а
/// не отказывать. До правки `add_connection` спрашивал полноту полей новой
/// карточки (`Cfg::validate`), а на пустом списке она же становится ТЕКУЩЕЙ
/// (`Cfg::current()` отдаёт первую при пустом корневом `current`) — и первое
/// нажатие в жизни программы отвечало «base_path — путь к базе или
/// сервер\база» про поле, которого человек ещё не видел. Карточка заводится
/// пустой НАРОЧНО: поля вписывают в ней и жмут «Сохранить», где и стоит
/// полная проверка.
///
/// Оба ответа онбординга (`link::onboarding_answers`) — и зовущий, и
/// принимающий: роль на годность списка не влияет, но проверять надо то, что
/// человек реально нажмёт.
#[test]
fn an_onboarding_answer_on_an_empty_list_creates_the_first_card() {
    for role in ["accept", "dial"] {
        let mut ui = Ui::for_test();
        ui.cfg.connections.clear();
        ui.cfg.current = String::new();
        assert!(!ui.kit.state_path(cfg::FILE).exists(), "свежий комплект: файла настроек нет вовсе");

        let name = add_connection(&mut ui, role).unwrap();

        assert_eq!(ui.cfg.by_name(&name).expect("карточка в памяти").role, role);
        let written = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).expect("и на диске");
        assert_eq!(common::toml_render::count(&written, cfg::CONNECTION), 1, "{written}");
        assert!(written.contains(&format!("name = {name:?}")), "{written}");
        // Онбординг кончается СОСТОЯНИЕМ: список больше не пуст.
        assert!(!crate::gui::link::onboarding_shown(&ui.cfg), "онбординг кончился — карточка есть");
    }
}

/// Ослабление касается ТОЛЬКО полноты полей новой карточки: правила СПИСКА
/// (`Cfg::validate_list`) заведение по-прежнему спрашивает целиком, и обоим
/// парным запретам отказывает, как раньше. Проверяется руками поставленным
/// столкновением: у новой карточки поля пустые, поэтому сталкивать её не с
/// чем — сталкиваем вторую карточку, заполненную так же, как первая (ровно
/// то, что человек получит, вписав в новую карточку занятую базу и папку, а
/// потом нажав «Добавить» ещё раз).
#[test]
fn creating_a_card_still_obeys_the_rules_of_the_list() {
    // Та же база у двух карточек — запрет учебной лицензии (одна база, одно
    // соединение).
    let mut ui = Ui::for_test();
    let twin = Connection { name: "Ромашка".into(), project: "C:\\proj\\Ромашка".into(), ..ui.cfg.current().clone() };
    ui.cfg.connections.push(twin);
    let err = add_connection(&mut ui, "accept").unwrap_err();
    assert!(err.contains("одна база, одно соединение"), "{err}");

    // Та же папка — запрет про потерю файлов в чужой папке.
    let mut ui = Ui::for_test();
    let twin = Connection { name: "Ромашка".into(), base_path: "C:\\Bases\\Ромашка".into(), ..ui.cfg.current().clone() };
    ui.cfg.connections.push(twin);
    let err = add_connection(&mut ui, "accept").unwrap_err();
    assert!(err.contains("папка принадлежит одному соединению"), "{err}");
}

/// Роль приходит параметром — кнопка списка ставит `"accept"`, онбординг
/// (задача 11) передаст свою.
#[test]
fn the_role_parameter_lands_on_the_new_card() {
    let mut ui = Ui::for_test();
    let name = add_connection(&mut ui, "dial").unwrap();
    assert_eq!(ui.cfg.by_name(&name).unwrap().role, "dial");
}

/// Ответ человека на вопрос «Это соединение» (`link::onboarding_answers`) —
/// и зовущий, и принимающий — говорит, что конец не один: он задаёт ОБЕ оси
/// заводимой карточки сразу, не только роль. «Всё на одной машине»
/// (`link::LOCAL_ANSWER`) — третий ответ того же вопроса, и среди ответов
/// заведения (онбординг, «+ Добавить») его нет вовсе — значит обе роли,
/// которые сюда приходят, обязаны заводить карточку НЕ локальной. До правки
/// `add_connection` оставлял `mode` умолчанием `Connection::default()` —
/// «local» — и первый же экран после ответа противоречил ответу (замер на
/// живом окне 9.11).
#[test]
fn an_answer_about_connecting_creates_a_card_that_is_not_local() {
    for role in ["accept", "dial"] {
        let mut ui = Ui::for_test();
        ui.cfg.connections.clear();
        ui.cfg.current = String::new();
        let name = add_connection(&mut ui, role).unwrap();
        let card = ui.cfg.by_name(&name).expect("карточка добавлена");
        assert_eq!(card.role, role);
        assert_eq!(card.mode, "tunnel", "ответ про код — ответ не про одну машину: {role}");
    }
}

/// Пустой путь базы у новой карточки — имя «Соединение N», тем же правилом,
/// что у первого соединения свежего комплекта (`cfg::default_name`).
#[test]
fn an_empty_base_path_names_the_card_by_number() {
    let mut ui = Ui::for_test();
    let name = add_connection(&mut ui, "accept").unwrap();
    assert_eq!(name, "Соединение 1");
}

/// Главная мина (Review Focus плана, тот же приём, что у `rename`): блок с
/// именем «Соединение 1» дописан в файл руками, пока окно открыто — `ui.cfg`
/// об этом не знает. Занятость имени смотрим и на ДИСКЕ, не только в
/// памяти, иначе вторая карточка получила бы то же имя, что и дописанная
/// руками, и следующее чтение отвергло бы файл целиком (`Cfg::validate`,
/// уникальность имён).
#[test]
fn a_name_taken_only_on_disk_is_skipped_too() {
    let mut ui = Ui::for_test();
    with_params_file(&ui); // на диске — только "Buh"
    let store = crate::secrets::Store::file(ui.kit.state());
    let hand = Connection { name: "Соединение 1".into(), ..Connection::default() };
    let path = ui.kit.state_path(cfg::FILE);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&cfg::connection_block(&hand, &store).unwrap());
    std::fs::write(&path, &text).unwrap();

    let name = add_connection(&mut ui, "accept").unwrap();
    // `cfg::default_name` отделяет хвостовой номер занятого «Соединение 1» от
    // базового имени и наращивает ЕГО — «Соединение 2», а не «Соединение 1 2»
    // (то же правило, что у двух одноимённых баз, применённое к собственному
    // номеру базового имени).
    assert_eq!(name, "Соединение 2", "«Соединение 1» занято на диске, в памяти его не было вовсе");
}

/// Запись — точечная: дописывается РОВНО один новый блок, байты соседних
/// блоков (включая уже дописанный руками) не трогаются (тот же приём, что у
/// `rename`, не полная перерисовка через `write_checked`).
#[test]
fn adding_a_card_appends_one_block_and_leaves_the_rest_byte_for_byte() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let name = add_connection(&mut ui, "accept").unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.starts_with(&before), "прежний текст файла обязан остаться дословно впереди:\n{after}");
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 2, "{after}");
    assert_eq!(common::toml_render::get_in(&after, cfg::CONNECTION, 1, "name").as_deref(), Some(name.as_str()), "{after}");
    assert_eq!(ui.cfg.connections.len(), 2, "новая карточка попала и в память");
}

/// Свежий комплект — файла параметров ещё нет вовсе. Добавление второй
/// карточки до первого «Сохранить» не должно отказывать: печатать список
/// целиком из памяти здесь нечем рисковать — на диске и так пока ничего нет.
#[test]
fn adding_a_card_before_the_first_save_writes_a_fresh_file() {
    let mut ui = Ui::for_test();
    assert!(!ui.kit.state_path(cfg::FILE).exists());
    let name = add_connection(&mut ui, "accept").unwrap();
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert_eq!(common::toml_render::count(&text, cfg::CONNECTION), 2, "{text}");
    let back: Cfg = toml::from_str(&text).expect("файл разбирается");
    assert!(back.connections.iter().any(|c| c.name == name));
    assert_eq!(back.current, ui.cfg.current, "текущее соединение не сменилось");
}

/// Круг починки 1, Critical ревью: `add_connection` на файле без блоков
/// `[[connection]]` печатает список целиком из памяти. Если пароль
/// какого-то соединения не расшифровался на этой машине, он лежит в памяти
/// ПУСТОЙ строкой (`ui.cfg.undecryptable` называет поле) — наивная печать
/// записала бы это пустое поле на диск поверх настоящего шифртекста,
/// безвозвратно и молча. Отказ обязан сработать ДО какой-либо записи, и
/// файл обязан остаться байт в байт прежним.
///
/// Задача 15 плана «Интерфейс» убрала прежнюю секцию `[host]` — до неё
/// нулевой счёт блоков на НЕпустом файле означал либо свежий комплект, либо
/// именно её, и фикстура этого теста держала файл старой формы с чужим
/// шифртекстом внутри. Секция ушла, но сам путь («блоков нет — печатаем из
/// памяти») остался нужен свежему файлу, и защита от тихой потери чужого
/// пароля обязана остаться на месте (ровно она чуть не подвела задачу 5) —
/// фикстура здесь держит руками написанный файл без единого блока, которым
/// теперь и достигается `disk_count == 0`, без упоминания формы, которой
/// больше нет.
///
/// «Обратный ход» (проверен руками при подготовке задачи, см. отчёт): без
/// защиты (`if !ui.cfg.undecryptable.is_empty() { return Err(..) }` в
/// `add_connection`) этот тест падает на первой строке — `unwrap_err()`
/// паникует на `Ok("Соединение 1")`, потому что добавление молча
/// проходит — а прежний шифртекст на диске в этот момент уже заменён
/// пустым полем (`store.seal("password", "")` отдаёт пустую строку как
/// есть, и она ушла на диск поверх настоящего шифртекста): ровно та потеря
/// пароля, против которой писан круг починки.
#[test]
fn add_connection_refuses_instead_of_wiping_an_unreadable_password_when_disk_has_no_blocks_yet() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let name = ui.cfg.current().name.clone();
    let ciphertext = store.seal("password", "секрет с другой машины").unwrap();
    let path = ui.kit.state_path(cfg::FILE);
    let before = format!("# правка руками, блока [[connection]] тут пока нет\npassword = {ciphertext:?}\n");
    std::fs::write(&path, &before).unwrap();
    assert_eq!(common::toml_render::count(&before, cfg::CONNECTION), 0, "фикстура: файл без единого блока");

    // «Чужая машина»: пароль не открылся, поле пустое, имя — в списке
    // непрочитанного, тем же приёмом, что у соседних тестов `save_host`.
    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new(&name, "password")];

    let err = add_connection(&mut ui, "accept").unwrap_err();
    assert!(err.contains(&name) && err.contains("пароль"), "{err}");

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, before, "отказ — файл не тронут ни на байт, прежний шифртекст цел");
}

/// Круг починки 1 задачи 7, Important 1: та же защита, что у соседнего
/// теста выше, но с ДВУМЯ непрочитанными карточками сразу — отказ обязан
/// читаться во множественном числе целиком (существительное, глагол,
/// местоимение и хвост «введите пароли заново» — Minor 1 того же круга).
/// Причина взята из `cfg::undecryptable_reason(true)` ДИНАМИЧЕСКИ, а не
/// переписана в тесте своим литералом: если `add_connection` однажды
/// перестанет звать общую функцию и вернёт собственную копию слов, этот
/// тест поймает расхождение сам, без переписывания руками — ровно то, чего
/// не хватало до этого круга (`cfg::UNDECRYPTABLE_REASON` раньше была
/// фразой целиком в единственном числе, и множественная ветка несла свой,
/// отдельный литерал, за которым никто не следил).
#[test]
fn add_connection_refuses_with_a_number_agreeing_reason_for_several_unreadable_cards() {
    let mut ui = Ui::for_test();
    let path = ui.kit.state_path(cfg::FILE);
    let before = "# правка руками, блока [[connection]] тут пока нет\n";
    std::fs::write(&path, before).unwrap();
    assert_eq!(common::toml_render::count(before, cfg::CONNECTION), 0, "фикстура: файл без единого блока");

    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new("Buh", "password"), cfg::Undecryptable::new("Ромашка", "password")];

    let err = add_connection(&mut ui, "accept").unwrap_err();
    assert!(err.contains(&cfg::undecryptable_reason(true)), "{err}");
    assert!(err.contains("карточек") && err.contains("их в окне"), "{err}");
    assert!(err.contains("введите пароли заново"), "хвост рассогласован числом: {err}");

    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ — файл не тронут ни на байт");
}

/// Круг починки 1, находка Important 2 ревью: занятость имени смотрим не
/// только по памяти (`Cfg::by_name`), но и по файлу — блок `to` мог
/// появиться там рукой, пока окно открыто, и `ui.cfg` об этом не знает.
/// Молча разрешённая запись дописала бы в файл ВТОРОЙ блок с тем же именем
/// — следующее чтение отвергло бы файл целиком (`Cfg::validate`), то есть
/// окно вовсе не открылось бы.
#[test]
fn a_name_already_on_disk_but_not_in_memory_is_refused_and_touches_nothing() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    // Рука человека: блок «Ромашка» дописан прямо в файл, пока окно открыто
    // — ui.cfg о нём ничего не знает.
    let mut hand = ui.cfg.clone();
    hand.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    std::fs::write(&path, hand.render(&store).unwrap()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let e = rename(&mut ui, "Buh", "Ромашка").unwrap_err();
    assert!(e.contains("Ромашка"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет, второй блок «Ромашка» не появляется");
}

/// Задача 6 плана «Интерфейс»: переключение текущей — действие человека из
/// окна, и о нём обязана сказать строка журнала (как и о других действиях
/// над списком, задачи 4 и 5) — иначе человек не поймёт, почему агент вдруг
/// отвечает из другой базы.
#[test]
fn switching_the_current_connection_says_so_in_the_log() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);

    switch_current(&mut ui, "Ромашка").unwrap();

    assert_eq!(ui.cfg.current, "Ромашка");
    assert!(ui.log.iter().any(|l| l.text.contains("Ромашка")), "без строки человек не поймёт, почему агент ответил странно: {:?}", ui.log);
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("current = \"Ромашка\""), "{text}");
}

/// Critical ревью круга починки 2 задачи 16: `switch_current` остаётся
/// тем, что говорит её докстрока — ОДИН ключ в toml и `ui.cfg`, и не
/// больше. Перецикл живого сеанса (гашение, пересчёт готовности и замка,
/// значок, подъём заново) — работа ВЫЗЫВАЮЩЕГО (`gui::tray::request_switch_current`
/// / `apply_switch_current`), а не этой функции: до круга починки 2 его не
/// было вовсе, и сеанс оставался поднятым с параметрами ПРЕЖНЕГО текущего
/// соединения — другая база, другой логин, другой транспорт, агент молча
/// отвечал не из той базы, которую показывает окно. Тест фиксирует саму
/// границу прямо здесь, а не только в комментарии: ни сеанс, ни замок
/// `switch_current` сама не трогает.
#[test]
fn switch_current_alone_does_not_touch_the_session_or_the_gate() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let gate_before = ui.gate.clone();

    switch_current(&mut ui, "Ромашка").unwrap();

    assert_eq!(ui.session_gen, gen_before, "switch_current не гасит и не перезапускает сеанс сам");
    assert!(ui.session.is_some(), "прежний сеанс остался висеть — перецикл делает вызывающий");
    assert_eq!(ui.gate, gate_before, "switch_current не пересчитывает замок сам");
    let _ = ui.session.take().unwrap().stop();
}

/// Review Focus плана, задача 6: переключение текущей на соединение, у
/// которого база на той стороне (`base_here = false`), — путь к базе пустой
/// там законно (снятый замок `Connection::validate`), и это не повод
/// запирать окно.
#[test]
fn switching_to_a_connection_whose_base_is_elsewhere_is_not_locked() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_here: false,
        base_path: String::new(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);

    switch_current(&mut ui, "Ромашка").unwrap();
    assert_eq!(ui.cfg.current, "Ромашка");
}

/// Неизвестное имя — отказ словами (тот же текст, что у `Cfg::switch_current`,
/// который уже проверяет и пустой список, и опечатку в имени), файл не
/// тронут.
#[test]
fn switching_to_an_unknown_name_is_refused_and_touches_nothing() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = switch_current(&mut ui, "Ромашка").unwrap_err();
    assert!(e.contains("Ромашка"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
}

/// Отложенная мелочь задачи 6 прошлой ветки (`2026-11-03-interface`):
/// карточка существует по имени, но её собственные поля не проходят
/// `Connection::validate` (роль из тех, что опечатка руками в файле могла
/// внести, пока окно открыто) — переключение на такую карточку обязано
/// отказать словами, а не сделать поломанную карточку текущей. Симметрично
/// `a_broken_neighbour_does_not_refuse_the_current_card_but_refuses_when_it_becomes_current`
/// в `cfg.rs`: там это доказано на уровне модели, здесь — на пути окна.
#[test]
fn switching_to_an_invalid_card_is_refused_with_words() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        role: "сервер".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = switch_current(&mut ui, "Ромашка").unwrap_err();
    // Слова — для ОКНА: оно называет величину так, как она подписана на экране, и выбор предлагает щелчком. Имя ключа `role` и его
    // годные значения остаются читателю файла — их снимает `cfg::said_in_window` (её докстрока, задача 6 плана).
    assert!(e.contains("как подключается это соединение"), "отказ называет величину словами человека: {e}");
    assert!(!e.contains("role") && !e.contains("accept") && !e.contains("dial"), "и ни ключа toml, ни его значений: {e}");
    assert_eq!(ui.cfg.current, "Buh", "отказ не переключает текущее");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
}

/// Задача 7 плана «Интерфейс»: смена профиля меняет подпись папки и набор
/// инструментов ЭТОЙ карточки — не папку и не соседа. У соседа своя папка (не
/// конфликтует с «Buh»), и её профиль нетронутым остаётся лучшим
/// доказательством, что запись ушла в чужой блок, не в этот.
#[test]
fn changing_one_cards_profile_leaves_the_others_and_the_folders_alone() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        profile: "business".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let folder = ui.cfg.by_name("Buh").unwrap().project.clone();

    set_profile(&mut ui, "Buh", "developer").unwrap();

    assert_eq!(ui.cfg.by_name("Buh").unwrap().project, folder, "меняется подпись папки, не папка");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().profile, "developer");
    assert_eq!(ui.cfg.by_name("Ромашка").unwrap().profile, "business", "соседняя карточка не тронута");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("profile = \"developer\""), "{text}");
}

/// Review Focus плана, задача 7: папка этой карточки уже «занята» соседом по
/// правилу конфликта папок (`folder_conflict`) — ровно то, на чём отказала
/// бы полная `save_host` через `Cfg::validate()`. Профиль папку не трогает,
/// поэтому отказа тут быть не должно: `set_profile` не ходит в `validate()`,
/// он пишет один ключ тем же приёмом, что `save_tray_key`.
#[test]
fn a_folder_conflict_with_a_neighbour_does_not_block_changing_the_profile() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: ui.cfg.current().project.clone(), // та же папка, что у «Buh» — конфликт
        profile: "business".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    assert!(ui.cfg.validate().is_err(), "в фикстуре действительно конфликт папок");

    set_profile(&mut ui, "Buh", "developer").unwrap();

    assert_eq!(ui.cfg.by_name("Buh").unwrap().profile, "developer");
}

/// Отложенная мелочь задачи 7 прошлой ветки (`2026-11-03-interface`):
/// опечатка в наборе инструментов — отказ словами РАНЬШЕ записи. `set_profile`
/// проверяет значение пробной карточкой (`Connection::toolset`) до похода к
/// `set_keys`, то есть до чтения файла — отказ обязан случиться, не дойдя до
/// диска вовсе.
///
/// Круг починки 1 задачи 7, Minor 4: первая версия этого теста сравнивала
/// файл до и после побайтно — но запись ТОГО ЖЕ значения дала бы тот же
/// байт-в-байт файл, и такое сравнение не отличило бы «диска не касались» от
/// «прочитали и переписали, не изменив ни байта» (та же ловушка, что чинит
/// соседний тест переименования на своё имя). Файла на диске здесь просто
/// НЕТ: если бы отказ случился позже, внутри `set_keys`, первым делом он
/// наткнулся бы на непрочитанный файл, и текст отказа был бы про файл, а не
/// про профиль — сам текст теперь и доказывает очерёдность.
#[test]
fn an_unknown_profile_refuses_and_leaves_the_file_alone() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::remove_file(&path).unwrap();

    let e = set_profile(&mut ui, "Buh", "бухгалтер").unwrap_err();

    // Словами человека, и БЕЗ хвоста для файла: читатель здесь — журнал окна
    // (`cfg::said_in_window`, Important повторного ревью всей ветки). Прежде
    // здесь ждали `developer` и `business` — годные значения ключа toml, то
    // есть ровно то, чего человек в окне не видит.
    assert!(e.contains("набор инструментов агента не распознан"), "{e}");
    assert!(!e.contains("profile") && !e.contains("developer"), "ни ключа toml, ни его значений: {e}");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().profile, "", "профиль в памяти не тронут");
    assert!(!path.exists(), "отказ случился раньше любого обращения к диску — файла как не было, так и нет");
}

/// Critical круга починки 1 задачи 14. Форма карточки (`panel::host_of`)
/// ВСЕГДА говорит «база здесь» и отдаёт пустыми поля, которых карточка не
/// показывает (`panel::shown_fields`), а `save_host` заменяет соединение
/// целиком. Без переноса топологии и непоказанных полей карточка «база на
/// той стороне» получала одно из двух: отказ `validate()` про поле, которого
/// на экране НЕТ, или молчаливый переворот топологии в «база здесь» вместе с
/// путём, оставшимся в поле от ранее открытой карточки. Второе и есть
/// молчаливо неверное: ветвление по топологии в записи не держалось.
#[test]
fn a_card_whose_base_is_elsewhere_keeps_its_topology_and_gets_no_base_path() {
    let mut ui = Ui::for_test();
    {
        let c = ui.cfg.current_mut();
        c.base_here = false;
        c.base_path = String::new();
        c.db_user = String::new();
        c.db_password = String::new();
    }
    with_params_file(&ui);
    let saved_server = ui.cfg.current().server.clone();
    // Форма такой карточки: показана одна папка, всё прочее `host_of`
    // отдаёт пустым — а в поле «База» лежит путь от ранее открытой карточки.
    let form = Connection {
        base_here: true,
        base_path: "C:\\Bases\\Чужая".into(),
        project: ui.cfg.current().project.clone(),
        ..Connection::default()
    };
    save_host(&mut ui, form).unwrap();

    let c = ui.cfg.current();
    assert!(!c.base_here, "топология карточки записью не переворачивается");
    assert_eq!(c.base_path, "", "чужой путь из поля в карточку не попадает");
    assert_eq!(c.server, saved_server, "адрес среды карточка не показывает — он переносится, а не стирается");

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    let back: Cfg = toml::from_str(&text).expect("файл после записи разбирается");
    assert!(!back.current().base_here, "и после перечитывания топология та же:\n{text}");
    assert_eq!(back.current().base_path, "", "и пути в файле не появилось:\n{text}");
}

/// Задача 16 плана «Интерфейс»: переключатель «Это соединение» у карточки —
/// `set_role` пишет ОДИН ключ (`role`) именованной карточки, тем же приёмом,
/// что `set_profile`. Сосед («Ромашка») — лучшее доказательство, что правка
/// не перерисовала файл целиком: его блок остаётся в тексте словом в слово.
#[test]
fn switching_the_role_writes_one_key_and_leaves_the_neighbours_alone() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);

    set_role(&mut ui, "Buh", "dial").unwrap();

    assert_eq!(ui.cfg.by_name("Buh").unwrap().role().unwrap(), "dial");
    assert!(std::fs::read_to_string(&path).unwrap().contains("Ромашка"), "сосед на месте");
}

/// Critical ревью всей ветки, вторая половина: запомненный партнёр не переживает смену роли или режима — и человек об этом слышит.
///
/// Падало так: точечные писатели не касались `partners.toml` вовсе, и карточка, перещёлкнутая на «Принимаю подключения», звонила при
/// следующем запуске по записанному адресу — прямо против спеки контракта («Принимающая не звонит никогда, даже зная адрес»).
#[test]
fn switching_the_role_or_the_mode_forgets_where_the_card_was_calling() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().role = "dial".into();
    with_params_file(&ui);
    let id = ui.cfg.by_name("Buh").unwrap().id();
    access_of(&ui, "Buh");
    crate::gui::save::reread_partners(&mut ui);
    assert!(ui.partners.iter().any(|(c, _)| c == &id), "фикстура: партнёр записан, карточке есть куда возвращаться");

    set_role(&mut ui, "Buh", "accept").unwrap();

    let partners = crate::enrolled::Partners::open(ui.kit.state()).unwrap();
    assert!(partners.of(&id).is_none(), "принимающая карточка звонить не вправе — записи о партнёре больше нет");
    assert!(ui.partners.iter().all(|(c, _)| c != &id), "и снимок окна её больше не числит");
    assert!(ui.log.iter().any(|l| l.text.contains("больше не помнит")), "потерю возврата человек слышит словами: {:?}", ui.log);
    let book = crate::enrolled::Book::open(ui.kit.state()).unwrap();
    assert_eq!(book.of(&id).len(), 1, "а выданные человеком разрешения переключатель не трогает: их видно списком, и они не врут");

    // То же у режима: карточка подключается уже не туда, где её помнят.
    crate::enrolled::Partners::open(ui.kit.state())
        .unwrap()
        .remember(&id, &crate::enrolled::Partner { key: iroh::SecretKey::generate().public(), relay: None, direct: vec![] })
        .unwrap();
    set_mode(&mut ui, "Buh", "local").unwrap();
    let partners = crate::enrolled::Partners::open(ui.kit.state()).unwrap();
    assert!(partners.of(&id).is_none(), "смена режима — та же потеря возврата, и тоже не молчком");
}

/// Переключатель, поставленный туда, где он и стоял, ничего не теряет и ничего не говорит — даже когда в файле на этом месте ПУСТО.
///
/// Падало так (Important повторного ревью всей ветки): забывание шло по СЫРОМУ полю. Роль во всём продукте читают через `role()`, где
/// пустое значит «принимаю» (умолчание владельца 3.11), — значит карточка без ключа `role` плюс щелчок «Принимаю подключения» это тот же
/// самый выбор. А по строкам «» и «accept» выглядели переменой: партнёр забывался, и человеку ГРОМКОЙ строкой говорилось, что возврат
/// потерян, при том что он ничего не менял и терять было нечего. У режима тот же случай: вопрос у него один — всё на одной машине или нет,
/// — и «» с «tunnel» для всех его читателей одно и то же.
#[test]
fn a_switch_put_back_where_it_already_stood_loses_nothing_and_says_nothing() {
    for (field, from, to) in [("role", "", "accept"), ("mode", "", "tunnel")] {
        let mut ui = Ui::for_test();
        match field {
            "role" => ui.cfg.current_mut().role = from.into(),
            _ => ui.cfg.current_mut().mode = from.into(),
        }
        with_params_file(&ui);
        let id = ui.cfg.by_name("Buh").unwrap().id();
        crate::enrolled::Partners::open(ui.kit.state())
            .unwrap()
            .remember(&id, &crate::enrolled::Partner { key: iroh::SecretKey::generate().public(), relay: None, direct: vec![] })
            .unwrap();
        crate::gui::save::reread_partners(&mut ui);

        match field {
            "role" => set_role(&mut ui, "Buh", to).unwrap(),
            _ => set_mode(&mut ui, "Buh", to).unwrap(),
        }

        let partners = crate::enrolled::Partners::open(ui.kit.state()).unwrap();
        assert!(partners.of(&id).is_some(), "«{field}»: выбор тот же — партнёру незачем уходить");
        assert!(ui.partners.iter().any(|(c, _)| c == &id), "«{field}»: и снимок окна его числит");
        assert!(
            !ui.log.iter().any(|l| l.text.contains("больше не помнит")),
            "«{field}»: человеку не говорят о потере, которой не было: {:?}",
            ui.log
        );
    }
}

/// Неизвестное значение роли — отказ словами ДО записи, тем же приёмом, что
/// `set_profile` проверяет `toolset()` через пробный `Connection`.
#[test]
fn switching_to_an_unknown_role_is_refused_and_touches_nothing() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = set_role(&mut ui, "Buh", "мусор").unwrap_err();

    // Тот же снимок хвоста и по тому же доводу, что у профиля выше.
    assert!(e.contains("не распознано, как подключается это соединение"), "{e}");
    assert!(!e.contains("role") && !e.contains("accept"), "ни ключа toml, ни его значений: {e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
}

/// `set_base_here` пишет ОДИН ключ (`base_here`) именованной карточки, и
/// состав полей (`panel::shown_fields`) обязан поменяться сразу — брифа
/// задачи 16: «перечитай соединение после записи, а не держи старое».
#[test]
fn switching_the_topology_changes_which_fields_the_card_shows() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);

    set_base_here(&mut ui, "Buh", false).unwrap();

    let c = ui.cfg.by_name("Buh").unwrap();
    assert!(!panel::shown_fields(c).contains(&"base_path"), "база на той стороне — поля базы ушли");
}

/// Зеркало теста выше: обратное переключение возвращает поля базы, и сосед
/// по-прежнему не тронут.
#[test]
fn switching_the_topology_back_restores_the_base_fields_and_leaves_the_neighbour() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        base_here: false,
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);

    set_base_here(&mut ui, "Buh", false).unwrap();
    set_base_here(&mut ui, "Buh", true).unwrap();

    let c = ui.cfg.by_name("Buh").unwrap();
    assert!(panel::shown_fields(c).contains(&"base_path"), "база снова здесь — поля базы вернулись");
    assert!(!ui.cfg.by_name("Ромашка").unwrap().base_here, "соседняя карточка не тронута");
    assert!(std::fs::read_to_string(&path).unwrap().contains("Ромашка"), "сосед на месте");
}

/// Important 2 ревью всей ветки, случай первый: профиль «Разработчик»
/// требует папку исходников (`Connection::validate`, ветка `developer`), а
/// точечный писатель годности не спрашивал вовсе — карточка становилась
/// негодной молча, в момент щелчка. Потом командная строка отвергала весь
/// файл, а следующее «Сохранить» отказывало про поле, которого человек не
/// трогал. Теперь отказ стоит ДО записи и говорит словами.
#[test]
fn the_developer_profile_without_a_sources_folder_is_refused_before_the_write() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().project = String::new();
    with_params_file(&ui);
    assert!(ui.cfg.validate().is_ok(), "в фикстуре карточка годна: пустая папка у бизнес-профиля законна");
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = set_profile(&mut ui, "Buh", "developer").unwrap_err();

    assert!(e.contains("папка проекта"), "отказ называет поле словами человека: {e}");
    assert!(!e.contains("project"), "и не ключом toml — в окне его никто не видит: {e}");
    assert!(e.contains("повторите"), "отказ говорит, что сделать: {e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().profile, "", "и в памяти профиль не сменился");
}

/// Правка 2 повторного ревью волны (решение ведущего): ось «где база»
/// переключается ВСЕГДА, даже если карточка от этого станет неполной —
/// именно этот щелчок и открывает поля, которыми её дополняют.
///
/// **Этот тест — переписанный**, и прежде он держал обратное правило:
/// назывался `saying_the_base_is_here_with_no_base_path_is_refused_before_the_write`
/// (Important 2 ревью всей ветки, случай второй) и утверждал, что
/// `set_base_here(…, true)` при пустом пути ОТКАЗЫВАЕТ про `base_path`, файла
/// не пишет и топологию в памяти не меняет. Ревью волны нашло цену того
/// правила: поля пути при базе на той стороне на экране нет вовсе
/// (`panel::shown_fields`), оно появляется только ПОСЛЕ переключения, — то
/// есть пара запирала себя сама, и перенести базу на свою машину из окна было
/// нельзя. Отказ называл поле, которого экран не показывает, — тот же разряд,
/// что Critical 1 этой волны.
#[test]
fn saying_the_base_is_here_always_works_because_it_is_what_opens_the_base_fields() {
    let mut ui = Ui::for_test();
    {
        let c = ui.cfg.current_mut();
        c.base_here = false;
        c.base_path = String::new();
    }
    with_params_file(&ui);
    assert!(!crate::gui::panel::shown_fields(ui.cfg.current()).contains(&"base_path"), "поля пути на экране нет — его и нечем заполнить");

    set_base_here(&mut ui, "Buh", true).unwrap();

    assert!(ui.cfg.by_name("Buh").unwrap().base_here, "топология переключилась в памяти");
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("base_here = true"), "и на диске:\n{text}");
    // И вот теперь поле есть — им карточку и дополняют.
    assert!(crate::gui::panel::shown_fields(ui.cfg.current()).contains(&"base_path"), "щелчок открыл поле пути к базе");
    // Карточка при этом неполна, и это законно: о нехватке говорит замок, а
    // поля спрашивает «Сохранить».
    assert!(ui.cfg.current().validate().is_err(), "неполнота осталась неполнотой — её просто не ставят в вину щелчку");
}

// Исключение — ТОЛЬКО для оси «где база», и своего теста ему не нужно:
// отказы остальных писателей на той же порче уже стоят под тестами, и они
// обязаны пройти БЕЗ правок — сосед сверху
// (`the_developer_profile_without_a_sources_folder_is_refused_before_the_write`,
// профиль «Разработчик» при пустой папке исходников) и сосед снизу (уход в
// туннель без адреса среды). Ни одна из их осей полей на экране не
// открывает — там довод `breaks_connection` прежний и верный.

/// Та же проверка у `set_mode`: уход в туннель спрашивает адрес среды и
/// номер рабочего места — у годной ЛОКАЛЬНОЙ карточки их может не быть.
#[test]
fn switching_a_local_card_to_tunnel_without_an_environment_address_is_refused() {
    let mut ui = Ui::for_test();
    {
        let c = ui.cfg.current_mut();
        c.mode = "local".into();
        c.server = String::new();
        c.workstation = String::new();
    }
    with_params_file(&ui);
    assert!(ui.cfg.validate().is_ok(), "локальная карточка без адреса среды годна");

    let e = set_mode(&mut ui, "Buh", "tunnel").unwrap_err();

    assert!(e.contains("адрес среды") || e.contains("рабочее место"), "отказ называет поле словами человека: {e}");
    assert!(!e.contains("server") && !e.contains("workstation"), "и не ключом toml: {e}");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().mode, "local", "режим не сменился");
}

/// Обратный ход той же проверки — и он важнее прямого: отказываем ТОЛЬКО за
/// свою порчу. Свежая карточка стоит с пустым путём к базе при `base_here =
/// true` (`Connection::default`), то есть негодна ещё до щелчка, — и
/// безусловная проверка запирала бы на ней ВСЕ переключатели, отказывая про
/// поле, которого человек ещё не заполнял. Выбор профиля на свежем комплекте
/// обязан срабатывать.
#[test]
fn a_card_that_was_already_incomplete_still_switches() {
    let mut ui = Ui::for_test();
    let fresh = add_connection(&mut ui, "accept").unwrap();
    assert!(ui.cfg.by_name(&fresh).unwrap().validate().is_err(), "свежая карточка и правда негодна: пути к базе нет");

    set_profile(&mut ui, &fresh, "developer").unwrap();
    set_base_here(&mut ui, &fresh, false).unwrap();
    set_role(&mut ui, &fresh, "dial").unwrap();

    assert_eq!(ui.cfg.by_name(&fresh).unwrap().profile, "developer");
    assert!(!ui.cfg.by_name(&fresh).unwrap().base_here);
    assert_eq!(ui.cfg.by_name(&fresh).unwrap().role, "dial");
}

/// Important 3 ревью всей ветки: ребёнок моста берёт набор инструментов из
/// общей ячейки сеанса (`bridge::mcp_command` на `Session::cfg_now`), и
/// точечный писатель туда не толкал ничего — смена профиля не сужала набор
/// у УЖЕ РАБОТАЮЩЕГО агента. Пока окно прятало себя по потере фокуса, это
/// пряталось за перечитыванием файла на следующем показе; задача 12
/// автоскрытие убрала.
#[test]
fn a_card_switch_reaches_the_live_session_of_the_current_card() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    ui.session = Some(doomed_session(&ui));
    assert_eq!(ui.session.as_ref().unwrap().cfg_now().toolset().unwrap(), "business", "сеанс стартовал на прежнем профиле");

    set_profile(&mut ui, "Buh", "developer").unwrap();

    assert_eq!(ui.session.as_ref().unwrap().cfg_now().toolset().unwrap(), "developer", "живой сеанс увидел новый набор инструментов");
    let _ = ui.session.take().unwrap().stop();
}

/// И зеркало: правка НЕтекущей карточки сеансу не про что — он работает с
/// текущей одной (`Cfg::current`), толкать ему нечего.
#[test]
fn a_switch_on_another_card_does_not_touch_the_live_session() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        profile: "business".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    ui.session = Some(doomed_session(&ui));

    set_profile(&mut ui, "Ромашка", "developer").unwrap();

    let live = ui.session.as_ref().unwrap().cfg_now();
    assert_eq!(live.by_name("Ромашка").unwrap().profile, "business", "ячейка сеанса не переписана чужой правкой");
    assert_eq!(ui.cfg.by_name("Ромашка").unwrap().profile, "developer", "а в файле и в памяти правка есть");
    let _ = ui.session.take().unwrap().stop();
}

/// Задача 2 плана «Карточка публикации и удаление», решение владельца 4.11
/// («давать, папку не трогать»): отказ по занятой текущей карточке говорит,
/// что сделать, а не просто «нельзя» — рядом есть кому стать текущим вместо
/// неё.
#[test]
fn deleting_the_current_card_is_refused_while_other_cards_exist() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    assert!(delete_blocked_reason(&ui, "Buh").unwrap().contains("сделайте текущим другое"));
}

/// Последняя оставшаяся карточка остаётся удаляемой, даже будучи текущей —
/// иначе список застревал бы навсегда с одной записью, которую нельзя убрать.
#[test]
fn deleting_the_only_card_is_allowed_even_though_it_is_current() {
    let ui = Ui::for_test();
    with_params_file(&ui);
    assert_eq!(delete_blocked_reason(&ui, "Buh"), None);
}

/// Отказ во время идущей операции, проверки или публикации называет, ЧТО
/// именно идёт, — тем же словом, что и `tray::busy_now_word` (ruling задачи
/// 2: второй такой фразы не заводим). Карточка здесь НЕ текущая («Ромашка»),
/// чтобы занятость сама по себе доказывала этот отказ, а не соседнюю
/// проверку «текущая при живых соседях».
#[test]
fn deleting_anything_is_refused_while_an_operation_is_running() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let now = Instant::now();
    ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
    assert!(delete_blocked_reason(&ui, "Ромашка").unwrap().contains("операция"));
    ui.progress = None;
    // Minor 8 ревью круга починки 1: третья ветвь занятости (`ui.busy`,
    // «проверка») тоже обязана отказывать — до этой правки из трёх ветвей
    // `tray::mode_change_blocked` проверялись только две.
    ui.busy = Some(Busy::Project);
    assert!(delete_blocked_reason(&ui, "Ромашка").unwrap().contains("проверка"));
    ui.busy = None;
    ui.publishing = Some(now);
    assert!(delete_blocked_reason(&ui, "Ромашка").unwrap().contains("публикация базы"));
}

/// Карточки с этим именем нет ни в списке, ни на диске — отказ называет имя,
/// тем же приёмом, что и остальные писатели (`connection_named`).
#[test]
fn deleting_a_card_that_is_not_in_the_list_is_refused_by_name() {
    let ui = Ui::for_test();
    with_params_file(&ui);
    let e = delete_blocked_reason(&ui, "Ромашка").unwrap();
    assert!(e.contains("Ромашка"), "{e}");
}

/// Important 3 ревью круга починки 1: пустой корневой `current` — законный
/// вид файла (блоки без строки `current` в корне), и тогда текущее — первое
/// соединение списка (`Cfg::current()`), а НЕ «карточки с именем „“». Сравнение
/// `ui.cfg.current == name` этого не видело бы и молча пропускало бы
/// предусловие «карточка текущая, а рядом есть другие» ровно в этом законном
/// виде файла — удаление первой (фактически текущей, с живым сеансом)
/// карточки проходило бы в обход отказа.
#[test]
fn deleting_the_first_card_is_blocked_when_the_root_current_is_legitimately_empty() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    ui.cfg.current = String::new();
    assert_eq!(ui.cfg.current().name, "Buh", "фикстура: пустой корень — текущее первое, по правилу Cfg::current()");
    assert!(delete_blocked_reason(&ui, "Buh").unwrap().contains("сделайте текущим другое"));
}

/// Удаление НЕтекущей карточки при живом соседе: вырезается ровно её блок, а
/// шифртекст соседа (ещё держащей базу карточки) не трогается ни байтом —
/// `cfg::remove_named` режет текст хирургически (`toml_render::remove_at`),
/// а не перерисовывает файл по памяти (той же защиты ради, что у `set_named`
/// — см. его докстроку).
#[test]
fn deleting_a_card_writes_the_file_without_its_block_and_keeps_the_others() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    ui.cfg = cfg::cfg_with(&["Buh", "Ромашка"]);
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::write(&path, ui.cfg.render(&store).unwrap()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let staying_password = common::toml_render::get_in(&before, cfg::CONNECTION, 0, "password").expect("у оставшейся карточки есть пароль");

    delete_connection(&mut ui, "Ромашка").unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 1, "блок удалённой карточки ушёл:\n{after}");
    assert!(!after.contains("Ромашка"), "имени удалённой карточки в файле не осталось:\n{after}");
    assert_eq!(
        common::toml_render::get_in(&after, cfg::CONNECTION, 0, "password").as_deref(),
        Some(staying_password.as_str()),
        "шифртекст оставшейся карточки не тронут ни байтом:\n{after}"
    );
    assert_eq!(ui.cfg.connections.len(), 1);
    assert!(ui.cfg.by_name("Ромашка").is_none(), "и в памяти карточки больше нет");
    assert_eq!(ui.cfg.current, "Buh", "удалили не текущую — текущее соединение не тронуто");
    assert!(
        ui.log.iter().any(|l| l.kind == Kind::Ok && l.text == "Соединение «Ромашка» удалено. Папка и база остались на месте"),
        "{:?}",
        ui.log
    );
}

/// Последняя карточка — особый случай: в файле не остаётся ни одного блока,
/// и корневой `current` обязан стать пустым тем же текстовым приёмом, что
/// пишет непустое имя у `switch_current` (`toml_render::set`, секция `""`).
/// `cfg::remove_named` корневой `current` нарочно не трогает (её докстрока) —
/// решение, что станет текущим, за вызывающим, и здесь оно одно: «никого».
#[test]
fn deleting_the_only_card_leaves_a_file_with_no_connections_and_an_empty_current() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);

    delete_connection(&mut ui, "Buh").unwrap();

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 0, "блоков не осталось:\n{after}");
    assert!(after.contains("current = \"\""), "{after}");
    assert!(ui.cfg.connections.is_empty(), "и в памяти список пуст");
    assert_eq!(ui.cfg.current, "");
}

/// Удаление ПОСЛЕДНЕЙ карточки обязано сперва попытаться погасить свою
/// публикацию (`ops_host::stop_own_publication` со снимком СТАРОГО `Cfg`,
/// `super::quiet_ctx_with`) ДО записи файла. Critical 1 ревью круга починки
/// 1 (решение ведущего): на отказе гашения удаление теперь САМО отказывает
/// и файла не трогает вовсе — удаление последней карточки уносит из
/// `ui.cfg` путь и порт, по которым `publication::Paths::of` находит свой
/// httpd, и без настроек ни выключение (`link.rs`), ни выход (`mod.rs::quit`)
/// больше не смогут повторить гашение никогда; процесс осиротел бы до
/// перезагрузки машины. В каталоге теста нет ни httpd, ни платформы 1С,
/// поэтому здесь измеряется, а не угадывается, что ответит настоящая
/// попытка на этой машине (приём, уже живущий в
/// `ops_host::tests::leaving_stops_our_own_httpd_and_never_someone_elses`), и
/// проверяется согласие этого ответа с решением `delete_connection` —
/// включая то, что отказ не меняет НИ файл, НИ память, НИ признак.
#[test]
fn deleting_the_only_card_refuses_when_its_own_publication_cannot_be_stopped() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    ui.raised_publication = true;
    let old_cfg = ui.cfg.clone();
    let probe = crate::ops_host::stop_own_publication(&super::quiet_ctx_with(&ui, old_cfg));
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let result = delete_connection(&mut ui, "Buh");

    match probe {
        Err(_) => {
            let err = result.unwrap_err();
            assert!(err.contains("HTTP-сервис"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ — файл не тронут ни байтом");
            assert_eq!(ui.cfg.connections.len(), 1, "и в памяти карточка на месте");
            assert!(ui.raised_publication, "признак остаётся — повторит выключение или выход, пока настройки целы");
        }
        Ok(true) => {
            result.unwrap();
            assert!(!ui.raised_publication, "настоящая удача снимает признак");
        }
        Ok(false) => {
            result.unwrap();
            assert!(ui.raised_publication, "«не наша публикация» признак не трогает");
        }
    }
}

/// Important 4 ревью круга починки 1 — отдельный тест на УДАЧНУЮ ветвь, не
/// зависящий от того, есть ли на машине httpd и платформа: публикация «на
/// вашем веб-сервере» отдаёт `Ok(false)` первой же строкой
/// `stop_own_publication`, до какого-либо похода на диск. Удаление последней
/// карточки в этом случае обязано пройти (гасить нечего, но это НЕ отказ) —
/// упади эта проверка в «последняя карточка всегда отказывает» по ошибке
/// Critical 1, тест поймал бы это: `.unwrap()` ниже паникует.
#[test]
fn deleting_the_only_card_still_succeeds_when_the_publication_is_not_ours() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().publication = "webserver".into();
    with_params_file(&ui);
    ui.raised_publication = true;

    delete_connection(&mut ui, "Buh").unwrap();

    let after = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 0, "{after}");
    assert!(ui.raised_publication, "Ok(false) — «не наша публикация» — признак не трогает");
}

/// Important 2 ревью круга починки 1: гашение зовётся ТОЛЬКО когда признак
/// `ui.raised_publication` поднят — тем же счётом, что у обоих соседних
/// вызывающих (`mod.rs::quit`, `tray::publication_after_switch`:
/// `stop_old = base_changed && raised_publication`). Признак здесь ложный —
/// мы публикацию не поднимали, — и, не будь гейта, вызов упёрся бы в
/// отсутствующий `bin/httpd.exe` этой машины и, по Critical 1, ОТКАЗАЛ бы в
/// удалении там, где отказывать не из чего: гасить нечего, потому что мы
/// ничего и не поднимали.
#[test]
fn deleting_the_only_card_skips_the_cleanup_when_we_never_raised_a_publication() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    ui.raised_publication = false;

    delete_connection(&mut ui, "Buh").unwrap();

    assert!(!ui.log.iter().any(|l| l.text.contains("HTTP-сервис")), "гасить нечего — молчим: {:?}", ui.log);
}

/// Important 2 ревью всей ветки: находки про непрочитанный шифртекст уходят
/// ВМЕСТЕ с карточкой — там же, где она уходит из списка, и тем же приёмом,
/// что у соседей (`save_host` чистит свои, `rename` везёт их за новым
/// именем). Без этого комплект с чужой машины вёл человека в тупик:
/// пароль единственной карточки не открылся, карточку удалили, окно
/// показало онбординг, человек нажал ответ — и `add_connection` отказал
/// словами «у карточки «Buh» пароль зашифрован… — откройте её в окне и
/// введите заново» про карточку, которой больше нет и открыть которую
/// негде. Онбординг был тупиком.
#[test]
fn deleting_a_card_takes_its_unreadable_secrets_with_it() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new("Buh", "db_password"), cfg::Undecryptable::new("Buh", "password")];

    delete_connection(&mut ui, "Buh").unwrap();

    assert!(ui.cfg.undecryptable.is_empty(), "находки удалённой карточки сняты: {:?}", ui.cfg.undecryptable);
    // И онбординг на опустевшем списке больше не тупик: ответ заводит
    // карточку. Здесь лечилось ДВА барьера подряд — снятая выше находка
    // (`add_connection`, ветвь `disk_count == 0`, отказывала словами про
    // карточку, которой уже нет) и полнота полей новой карточки: она
    // становится текущей на пустом списке, и `Cfg::validate` отказывал про
    // `base_path`. Второй лечит `validate_list` (решение ведущего, вторая
    // волна) — заведение карточки спрашивает только правила списка.
    let fresh = add_connection(&mut ui, "accept").unwrap();
    assert_eq!(ui.cfg.by_name(&fresh).expect("карточка заведена").role, "accept");
}

/// Находки СОСЕДНЕЙ карточки удаление не трогает — иначе оно молча сказало
/// бы, что чужая беда прошла сама, и показ окна перестал бы о ней
/// предупреждать (тот же довод, по которому `save_host` чистит только свои).
#[test]
fn deleting_a_card_leaves_the_neighbours_unreadable_secrets_alone() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new("Buh", "db_password"), cfg::Undecryptable::new("Ромашка", "db_password")];

    delete_connection(&mut ui, "Ромашка").unwrap();

    assert_eq!(ui.cfg.undecryptable, vec![cfg::Undecryptable::new("Buh", "db_password")]);
}

/// Minor 4 ревью всей ветки: `set_keys` стал единственным писателем шести
/// ключей, и молчаливый промах в `apply_parsed` означал бы «файл записан, а
/// память — нет». Промах громкий: ключ, разобранный `parse_key` и забытый
/// здесь, паникует у разработчика на первом же прогоне, а не расходится с
/// диском у человека в окне.
#[test]
#[should_panic(expected = "apply_parsed")]
fn a_key_parsed_but_not_applied_panics_instead_of_silently_missing() {
    let mut c = Connection::default();
    apply_parsed(&mut c, "седьмой_ключ", &KeyValue::Str("что-нибудь".into()));
}

/// Minor 7 ревью круга починки 1 (решение ведущего): «последняя карточка»
/// для правки корневого `current` в ФАЙЛЕ решается по тексту, который
/// сейчас пишем (`toml_render::count` после `remove_named`), а не по списку
/// в памяти (`is_last_connection` — он про предусловие, про то, что видит
/// человек в окне). Здесь в памяти известна только «Buh» (значит,
/// предусловие это пропускает как «последнюю»), но в файле на диске уже
/// лежит ВТОРОЙ блок, дописанный рукой после загрузки и `ui.cfg` неизвестный
/// — после вырезания «Buh» он остаётся, и корневой `current` не обязан
/// стать пустым: блок, на который можно ссылаться, в тексте есть.
#[test]
fn deleting_the_last_known_card_does_not_blank_current_when_a_hand_added_block_remains() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let path = ui.kit.state_path(cfg::FILE);
    let mut hand = ui.cfg.clone();
    hand.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    std::fs::write(&path, hand.render(&store).unwrap()).unwrap();
    assert!(is_last_connection(&ui), "в памяти известна только «Buh» — предусловие её видит последней");

    delete_connection(&mut ui, "Buh").unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(common::toml_render::count(&after, cfg::CONNECTION), 1, "рукописный блок остался на месте:\n{after}");
    assert!(!after.contains("current = \"\""), "в тексте остался настоящий блок — корень не обязан стать пустым:\n{after}");
}

/// Отказанное удаление не трогает файл ни байтом — сравнение всего текста,
/// не отдельных строк: любая случайная правка (лишний перевод строки,
/// переставленный ключ) обязана провалить этот тест.
#[test]
fn a_refused_delete_leaves_the_file_byte_for_byte() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Ромашка".into(),
        project: "C:\\proj\\Ромашка".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let err = delete_connection(&mut ui, "Buh").unwrap_err();

    assert!(err.contains("сделайте текущим другое"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ — файл не тронут ни байтом");
    assert_eq!(ui.cfg.connections.len(), 2, "и в памяти ничего не исчезло");
}

/// Книга допуска и партнёр одной карточки — фикстура для двух тестов ниже:
/// зачисленная сторона, живой выданный код и запомненный партнёр. Все трое
/// ключуются КЛЮЧОМ карточки (`cfg::CardId`), а не именем, которое человек
/// правит.
fn access_of(ui: &Ui, card: &str) -> (iroh::EndpointId, crate::ticket::Ticket) {
    let id = ui.cfg.by_name(card).expect("карточка фикстуры").id();
    let key = iroh::SecretKey::generate().public();
    let ticket = crate::ticket::Ticket::issue(key, None, vec![]).unwrap();
    let mut book = crate::enrolled::Book::open(ui.kit.state()).unwrap();
    book.enroll(&id, key, "Бухгалтер", true).unwrap();
    book.reissue(&id, &ticket).unwrap();
    crate::enrolled::Partners::open(ui.kit.state())
        .unwrap()
        .remember(&id, &crate::enrolled::Partner { key, relay: None, direct: vec![] })
        .unwrap();
    (key, ticket)
}

/// Переименование больше НИЧЕГО не уносит за собой — и именно это проверяется.
///
/// **Чем куплен этот тест.** На месте него стоял обратный: он стерёг переезд книги допуска и партнёров за новым именем карточки
/// (`access_follows_rename`), потому что оба файла ключевались ИМЕНЕМ. Переезд работал, но держался на памяти каждого будущего писателя:
/// забыл позвать — и переименование снова осиротляет все зачисления разом. Решение владельца 10.11 закрыло корень: карточка названа
/// ключом, которого человек не правит (`cfg::CardId`), и переименование её больше не касается.
///
/// Поэтому утверждение сменило знак. Прежнее («зачисления переехали под новое имя») нечем проверить: ни одно хранилище имён больше не
/// знает. Новое сильнее: имя сменилось, а допуск, выданный код и партнёр остались на том же ключе — то есть переименование перестало
/// быть опасным действием, а не стало аккуратно убираемым.
#[test]
fn renaming_a_card_keeps_its_access_because_the_key_does_not_change() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let id = ui.cfg.by_name("Buh").unwrap().id();
    let (key, ticket) = access_of(&ui, "Buh");

    rename(&mut ui, "Buh", "Ромашка").unwrap();

    assert_eq!(ui.cfg.by_name("Ромашка").unwrap().id(), id, "ключ карточки переименование не меняет — на этом всё и держится");
    let book = crate::enrolled::Book::open(ui.kit.state()).unwrap();
    assert_eq!(book.of(&id).len(), 1, "зачисление осталось на своём ключе: везти его за именем больше не надо");
    assert_eq!(
        book.verdict(&ticket.secret).unwrap(),
        crate::enrolled::Verdict::Card(id.clone()),
        "и выданный код называет ту же карточку, что до переименования"
    );
    let partners = crate::enrolled::Partners::open(ui.kit.state()).unwrap();
    assert_eq!(partners.of(&id).unwrap().key, key, "возвращаться карточке есть куда, и это не зависело от её имени");
    // И снимок окна находит партнёра под тем же ключом — то есть «Подключиться» в переименованной карточке ведёт туда же, куда вело.
    // Перечтение здесь своё: `rename` его больше не делает, и это правильно — менять в снимке нечего.
    crate::gui::save::reread_partners(&mut ui);
    assert_eq!(ui.partners.first().map(|(card, _)| card.clone()), Some(id), "снимок окна о карточке того же ключа: {:?}", ui.partners);
}

/// Заведённая в окне карточка носит СВОЙ ключ, а не отпечаток своего имени.
///
/// **Утверждение узкое нарочно, и вот почему.** Ключ, выведенный из имени, принадлежит имени, а имя в этом продукте переходит от карточки
/// к карточке: человек переименовал одну и занял освободившееся имя другой. Пока ключ новой карточки — отпечаток её имени, всякий, кто
/// позже выведет ключ из того же слова, попадёт в неё: сегодня это блок, дописанный в файл руками, завтра — любой новый писатель, который
/// про разницу не знает. Свой ключ с рождения убирает саму зависимость от слова, а не отдельный путь к беде.
///
/// Сравнение именно с `CardId::from_name`: оно краснеет ровно на подмене «давать выведенный вместо своего» и ни на чём больше.
#[test]
fn a_card_created_in_the_window_carries_a_key_of_its_own() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let made = add_connection(&mut ui, "accept").unwrap();
    let id = ui.cfg.by_name(&made).expect("карточка заведена").id();
    assert_ne!(id, crate::cfg::CardId::from_name(&made), "заведение пишет файл — выводить ключ из имени там незачем");
    // И он закреплён на диске тем же заходом: иначе следующее чтение вывело бы ключ заново, уже из имени.
    let text = std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap();
    assert!(
        text.contains(&format!("id = {:?}", id.as_str())),
        "ключ новой карточки лёг в файл сразу:
{text}"
    );
}

/// Освободившееся имя, занятое новой карточкой, НЕ достаёт ей чужие зачисления.
///
/// **Путь человека, а не абстрактное совпадение ключей** (находка проверки безопасности 10.11; достижимость подтверждена чтением кода,
/// первая оценка её занижала). Шаги ровно такие: к «Buh» зачислена сторона; человек переименовывает карточку — переименование
/// ЗАКРЕПЛЯЕТ в файле ключ, выведенный из слова «Buh», иначе зачисления осиротели бы; человек заводит новую карточку и называет её
/// «Buh», потому что имя освободилось, а освободившееся имя берут не задумываясь. Выводи новой карточке ключ из имени — и он совпал бы
/// с закреплённым: машина, которой разрешали доступ к ДРУГОЙ базе, оказалась бы допущена к этой, и человек об этом не узнал бы.
///
/// Чинится это не проверкой на совпадение, а тем, что заведение карточки даёт ей СВОЙ ключ сразу (`cfg::CardId::fresh`): заведение
/// пишет файл в том же заходе, значит выводить ключ там незачем. Проверка на совпадение действующих ключей при разборе остаётся второй
/// линией — на блок, дописанный в файл руками.
#[test]
fn a_freed_name_taken_by_a_new_card_does_not_inherit_its_enrolments() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let old_id = ui.cfg.by_name("Buh").unwrap().id();
    let (party, _) = access_of(&ui, "Buh");

    rename(&mut ui, "Buh", "Ромашка").unwrap();
    let made = add_connection(&mut ui, "accept").unwrap();
    rename(&mut ui, &made, "Buh").unwrap();

    let fresh_id = ui.cfg.by_name("Buh").unwrap().id();
    assert_ne!(fresh_id, old_id, "у новой карточки ключ СВОЙ, хотя имя то же, что было у переименованной");
    let book = crate::enrolled::Book::open(ui.kit.state()).unwrap();
    assert!(book.of(&fresh_id).is_empty(), "новая карточка не получает чужих разрешений вместе с чужим именем");
    assert_eq!(book.of(&old_id).len(), 1, "а зачисление осталось у той карточки, которой его и давали");
    assert_eq!(book.of(&old_id)[0].key, party);
    assert_eq!(ui.cfg.by_name("Ромашка").unwrap().id(), old_id, "переименованная карточка при своём ключе");
}

/// Удалённая карточка уносит с собой и допуск, и партнёра — теперь ПО КЛЮЧУ.
///
/// Падало так: `delete_connection` не трогала ни одного из двух файлов, а
/// `Partners::forget` в продукте не звалась НИ РАЗУ. Удалили зовущую карточку —
/// программа при следующем запуске всё равно ей звонила: подъём звонков читает
/// `partners.toml`, а не настройки. И код, выданный удалённой карточке, по
/// книге жил ещё сутки, называя карточку, которой в настройках нет.
#[test]
fn deleting_a_card_takes_its_enrolments_and_its_partner_with_it() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let id = ui.cfg.by_name("Buh").unwrap().id();
    let (_, ticket) = access_of(&ui, "Buh");

    delete_connection(&mut ui, "Buh").unwrap();

    let book = crate::enrolled::Book::open(ui.kit.state()).unwrap();
    assert!(book.of(&id).is_empty(), "зачисления удалённой карточки спрашивать больше нечем — они ушли с ней");
    assert_eq!(
        book.verdict(&ticket.secret).unwrap(),
        crate::enrolled::Verdict::Unknown,
        "и код её не называет карточку, которой в настройках уже нет"
    );
    assert!(
        crate::enrolled::Partners::open(ui.kit.state()).unwrap().of(&id).is_none(),
        "партнёр ушёл вместе с карточкой — иначе программа звонит призраком при следующем запуске"
    );
    assert!(ui.partners.is_empty(), "снимок окна перечитан: {:?}", ui.partners);
}

/// Задача 4 плана «Карточка публикации и удаление»: `set_keys` — общее ядро,
/// на которое сведены `set_profile`/`set_role`/`set_mode`/`set_base_here`, на
/// несколько ключей зараз. Лучшее доказательство атомарности — оба ключа в
/// одном прочитанном и одном записанном тексте, а не два похода на диск
/// подряд (ровно та беда, которую эта задача закрывает у
/// `panel::set_card_topology_kind`).
#[test]
fn set_keys_writes_every_key_in_one_pass() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);

    set_keys(&mut ui, "Buh", &[("role", "dial"), ("mode", "tunnel")]).unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("role = \"dial\"") && text.contains("mode = \"tunnel\""), "{text}");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().role, "dial");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().mode, "tunnel");
}

/// Задача 12 «Карточка после живого прогона», находка владельца 9.11: его
/// последовательность была «Где база» → «На той стороне», затем «Это
/// соединение» → «Всё на одной машине» — и вышел локальный режим без единого
/// поля базы, с подписью «Эти поля заполняют на компьютере с базой», хотя в локальном
/// режиме той стороны не существует. Причина — `panel::set_card_topology_kind`
/// у ответа «Всё на одной машине» писала только `mode`, а `base_here`
/// оставался прежним (`false`) молча.
///
/// Лечит ровно то же правило, что уже закрыло пару `role`+`mode` у выхода из
/// локального режима (`set_keys_writes_every_key_in_one_pass`): обе оси
/// ответа «Это соединение» — одна запись через `set_keys`. Тест держит
/// именно эту пару ключей от того самого стартового состояния, в котором её
/// застал владелец (`base_here` уже сбито на `false` отдельным ответом «Где
/// база»), и смотрит на оба конца — файл и `ui.cfg` — тем же приёмом, что
/// `set_keys_writes_every_key_in_one_pass`.
#[test]
fn answering_one_machine_puts_the_base_here_too() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    set_base_here(&mut ui, "Buh", false).unwrap();

    set_keys(&mut ui, "Buh", &[("mode", "local"), ("base_here", "true")]).unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("mode = \"local\"") && text.contains("base_here = true"), "{text}");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().mode, "local");
    assert!(ui.cfg.by_name("Buh").unwrap().base_here, "файл и память согласны: база теперь здесь");
}

/// Тот же отказ, что у `set_profile` на профиле «Разработчик» при пустой
/// папке исходников
/// (`the_developer_profile_without_a_sources_folder_is_refused_before_the_write`),
/// только через общее ядро: `breaks_connection` отказывает ДО какой-либо
/// записи, и текст файла до и после совпадает побайтово.
#[test]
fn set_keys_refuses_a_patch_that_breaks_the_card_and_writes_nothing() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().project = String::new();
    with_params_file(&ui);
    assert!(ui.cfg.validate().is_ok(), "в фикстуре карточка годна: пустая папка у бизнес-профиля законна");
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = set_keys(&mut ui, "Buh", &[("profile", "developer")]).unwrap_err();

    assert!(e.contains("папка проекта"), "отказ называет поле словами человека: {e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().profile, "", "и в памяти профиль не сменился");
}

/// Important 1 круга починки 1: все тесты `set_base_here` до этого круга
/// проверяли только `ui.cfg` и состав полей — ни один не смотрел на
/// записанный ТЕКСТ. Поставь вокруг `base_here` кавычки — эти тесты остались
/// бы зелёными, а файл стал бы негодным (`base_here` там не строка, и
/// следующая загрузка отвергла бы его целиком). Тест смотрит на оба конца:
/// и на написание (без кавычек), и — важнее — на то, что файл целиком
/// разбирается `toml::from_str`, потому что это держит смысл, а не только
/// внешний вид строки.
#[test]
fn set_base_here_writes_the_toml_bool_without_quotes_and_the_file_still_parses() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);

    set_base_here(&mut ui, "Buh", false).unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("base_here = false"), "булево пишется без кавычек:\n{text}");
    assert!(!text.contains("base_here = \"false\""), "и не строкой в кавычках:\n{text}");
    let back: Cfg = toml::from_str(&text).expect("файл после записи обязан разбираться целиком");
    assert!(!back.current().base_here, "и смысл тот же после перечитывания");
}

/// Important 2 круга починки 1: `base_here` разбирается строго
/// (`str::parse::<bool>`), а не терпит произвольную строку. До починки
/// `value == "true"` читала любое чужое значение как `false` молча, унося на
/// диск неразбираемый токен (`base_here = yes`) и оставляя в памяти неверное
/// булево — диск и память расходились бы без единого слова отказа. Отказ —
/// до какой-либо записи, файл не тронут ни байтом.
#[test]
fn set_keys_refuses_an_unparsable_base_here_and_touches_nothing() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = set_keys(&mut ui, "Buh", &[("base_here", "yes")]).unwrap_err();

    assert!(e.contains("base_here"), "отказ называет ключ: {e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
    assert!(ui.cfg.by_name("Buh").unwrap().base_here, "и в памяти умолчание не тронуто");
}

/// Задача 5 плана «Карточка публикации и удаление»: публикация стала полем
/// КАРТОЧКИ, а не одним переключателем на машину (решение владельца 1.11), и
/// пишется она у ИМЕНОВАННОГО соединения — не обязательно текущего.
///
/// Тест смотрит на ОБА конца записи — на текст файла и на `ui.cfg` (Review
/// Focus 4 задания): у `apply_parsed` есть немой catch-all `_ => {}`, и забудь
/// эта задача завести свой ключ там, запись легла бы в файл, а в памяти
/// публикация осталась бы прежней — карточка на экране не изменилась бы, зато
/// следующее «Сохранить» унесло бы в toml старую ось.
#[test]
fn set_publication_writes_one_key_of_a_foreign_card() {
    let mut ui = Ui::for_test();
    ui.cfg.connections.push(Connection {
        name: "Ромашка".into(),
        base_path: "C:\\Bases\\Romashka".into(),
        project: "C:\\proj\\Romashka".into(),
        // Адрес, вписанный человеком: он не догадка программы
        // (`cfg::derived_base_url` от этой папки — другая строка), и запись
        // публикации его не касается.
        base_url: "http://localhost:8081/Romashka/hs/mcp-1c".into(),
        ..ui.cfg.current().clone()
    });
    with_params_file(&ui);

    set_publication(&mut ui, "Ромашка", "webserver").unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    let back: Cfg = toml::from_str(&text).expect("файл после записи обязан разбираться целиком");
    assert_eq!(back.by_name("Ромашка").unwrap().publication, "webserver", "записано в свой блок:\n{text}");
    assert_eq!(back.by_name("Buh").unwrap().publication, "auto", "соседний блок не тронут:\n{text}");
    assert_eq!(
        back.by_name("Ромашка").unwrap().base_url,
        "http://localhost:8081/Romashka/hs/mcp-1c",
        "свой адрес человека остался на месте:\n{text}"
    );
    assert_eq!(ui.cfg.by_name("Ромашка").unwrap().publication, "webserver", "и в памяти то же, что в тексте");
}

/// Выведенный от каталога базы адрес в toml написали МЫ (`carried_base_url` в
/// обычном режиме), а не человек, — и в режиме «публикует платформа 1С»
/// принять его за ответ нельзя: проба по нашей догадке ушла бы на чужой
/// веб-сервер. Карточка такой адрес и показывает пустым полем
/// (`panel::shown_base_url`), значит и в файле его быть не должно. Тот же
/// довод уже записан у `tray::apply_publication` — здесь он для точечной
/// записи чужой карточки.
#[test]
fn switching_a_card_to_webserver_drops_the_address_we_derived_ourselves() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().base_path = "C:\\Bases\\Buh".into();
    ui.cfg.current_mut().base_url = cfg::derived_base_url("C:\\Bases\\Buh");
    with_params_file(&ui);

    set_publication(&mut ui, "Buh", "webserver").unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    let back: Cfg = toml::from_str(&text).expect("файл после записи обязан разбираться целиком");
    assert_eq!(back.current().publication, "webserver", "{text}");
    assert_eq!(back.current().base_url, "", "нашу догадку сняли:\n{text}");
    assert_eq!(ui.cfg.current().base_url, "", "и в памяти то же, что в тексте");
}

/// Обратная сторона того же правила: адрес, который вписал человек, — ответ, и
/// его не трогает никто. Сравнение строк живёт ровно в одном месте
/// (`cfg::is_derived_address`), и этот тест стоит за тем, чтобы «уборка»
/// однажды не стала безусловной: у человека, чья база и правда опубликована
/// как `http://localhost/<папка>`, она стёрла бы настоящий адрес.
#[test]
fn switching_a_card_to_webserver_keeps_an_address_the_person_typed() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().base_path = "C:\\Bases\\Buh".into();
    ui.cfg.current_mut().base_url = "http://localhost:8081/Buh/hs/mcp-1c".into();
    with_params_file(&ui);

    set_publication(&mut ui, "Buh", "webserver").unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    let back: Cfg = toml::from_str(&text).expect("файл после записи обязан разбираться целиком");
    assert_eq!(back.current().base_url, "http://localhost:8081/Buh/hs/mcp-1c", "{text}");
    assert_eq!(ui.cfg.current().base_url, "http://localhost:8081/Buh/hs/mcp-1c");
}

/// Important 1 круга починки 1 задачи 5: когда `save_host` отказал (поля
/// дозаполнены не до конца), в файл всё равно ложится намерение человека — и
/// при уходе в «публикует платформа 1С» это ПАРА ключей, а не один. До правки
/// писался только `publication`, а выведенный нами адрес снимался лишь в
/// памяти: на диске оставалась догадка, замок про неё молчал (поле непустое), и
/// проба уходила на чужой веб-сервер на 80, а окно при следующем показе
/// возвращало догадку и в память.
///
/// Тест смотрит на оба конца, и на ОДНУ запись: оба ключа в одном тексте.
#[test]
fn the_tray_writer_puts_a_pair_of_keys_in_one_pass() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().base_path = "C:\\Bases\\Buh".into();
    ui.cfg.current_mut().base_url = cfg::derived_base_url("C:\\Bases\\Buh");
    with_params_file(&ui);

    save_tray_keys(&mut ui, &[("publication", "webserver"), ("base_url", "")]).unwrap();

    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    let back: Cfg = toml::from_str(&text).expect("файл после записи обязан разбираться целиком");
    assert_eq!(back.current().publication, "webserver", "{text}");
    assert_eq!(back.current().base_url, "", "нашей догадки в файле не осталось:\n{text}");
    // Именно строкой ключа: сам адрес-догадка упоминается ещё и в объяснениях
    // образца, и поиск по подстроке без ключа ловил бы комментарий.
    assert!(!text.contains("base_url = \"http://localhost/Buh/hs/mcp-1c\""), "и ни одной строкой ключа:\n{text}");
    assert_eq!(ui.cfg.current().base_url, "", "и в памяти то же, что в тексте");
    assert_eq!(ui.cfg.current().publication, "webserver");
}

/// Minor 3 круга починки 1 задачи 5: писатель публичный, и проверка значения —
/// рубеж доверия, а не украшение (с нынешних вызывающих она недостижима: слова
/// идут из словаря `'static`). Отказ — ДО какой-либо записи, тем же приёмом,
/// каким закрыт отказ `set_keys` на неразбираемом `base_here`: файл не тронут
/// ни байтом.
#[test]
fn set_publication_refuses_an_unknown_value_and_touches_nothing() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    let e = set_publication(&mut ui, "Buh", "iis").unwrap_err();

    assert!(e.contains("кто публикует базу"), "отказ называет поле словами человека: {e}");
    assert!(!e.contains("publication") && !e.contains("auto") && !e.contains("webserver"), "без ключа toml и без его значений: {e}");
    assert!(e.contains("выберите"), "и говорит, что сделать: {e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "отказ не пишет");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().publication, "auto", "и в памяти прежнее значение не тронуто");
}

/// Minor 1 круга починки 1: пустой список ключей — не правка, а её
/// отсутствие, и функция даже не обязана искать карточку по имени — для
/// пустой правки оно ей не нужно. Лучшее доказательство — ИМЯ, которого в
/// списке нет: до починки `set_keys` сначала читала бы файл и искала бы
/// карточку (`connection_named`), отказывая про опечатку в имени, хотя для
/// пустой правки и имя, и файл не при чём. Проверка на байт файла здесь
/// лишь подтверждает, что отказа не произошло молча вместо успеха — главное
/// в `.unwrap()` на неизвестном имени.
#[test]
fn set_keys_with_no_keys_touches_nothing_not_even_for_an_unknown_name() {
    let mut ui = Ui::for_test();
    with_params_file(&ui);
    let path = ui.kit.state_path(cfg::FILE);
    let before = std::fs::read_to_string(&path).unwrap();

    set_keys(&mut ui, "Чего-то такого нет", &[]).unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "пустой список — файл не тронут");
}

// Задача 6 плана «Канал и зачисление»: диалог разрешения.

fn a_request() -> crate::enroll::Request {
    crate::enroll::Request {
        card_name: "Ромашка".into(),
        base: crate::wire::BaseFields {
            // Поле, которое по каналу приехало и в диалог НЕ попадёт: оно
            // её слово, но выглядит как факт.
            base_id: "ИДЕНТИФИКАТОР-БАЗЫ-7731".into(),
            base_name: "Бухгалтерия Ромашки".into(),
            write: true,
        },
        peer: iroh::SecretKey::from_bytes(&[42u8; 32]).public(),
        says_name: "Заказчик Ромашка".into(),
    }
}

/// Решение 7 `2026-11-01-protocol-design.md`, дословно: диалог показывает
/// ТРИ вещи и ни одной больше — отпечаток ключа, базу и то, как сторона себя
/// назвала, с прямой пометкой, что имя никто не проверял.
///
/// Тест стоит с ДВУХ сторон, и вторая важнее первой: он не только требует
/// три вещи, но и проверяет, что остальное, приехавшее по каналу
/// (`base_id`), в текст не попало. Довод спеки от обратного:
/// это тоже её слова, но рядом с проверенным отпечатком они читаются как
/// проверенный факт.
///
/// Падал так: `permission_question` не существовало вовсе — диалога
/// разрешения в программе не было ни одного.
#[test]
fn the_permission_dialog_shows_three_things_and_marks_the_self_given_name() {
    let r = a_request();
    let (message, detail) = permission_question(&r);

    // Первая вещь: имя базы — в самом вопросе, потому что спрашивают именно
    // про доступ к ней.
    assert!(message.contains("Бухгалтерия Ромашки"), "{message}");
    // Вторая: отпечаток ЧЕТЫРЬМЯ ПОСЛЕДНИМИ знаками (решение владельца
    // 10.11: «кто это будет сверять голосом, какой придурок-параноик?
    // Максимум 4 знака в конце»). Утверждение сильнее прежнего: полного
    // ключа в диалоге нет ВОВСЕ, и просьбы сверять его голосом тоже нет —
    // ритуал, который не исполняется, защитой не был.
    let fingerprint = r.peer.to_string();
    assert!(detail.contains(&crate::gui::short_key(&fingerprint)), "{detail}");
    assert!(!detail.contains(&fingerprint), "полного ключа на экране быть не должно: {detail}");
    assert!(!detail.contains("голосом"), "ритуала сверки голосом больше нет: {detail}");
    // Третья: самоназвание — и ПОМЕТКА рядом с ним, а не где-то в тексте.
    //
    // Пометка стоит СРАЗУ ПОД чужими словами и ничем от них не отделена.
    // Прежде требовалась та же строка — требование по сути то же (оторвать
    // пометку от имени нельзя), но одной строкой оно давало длинный хвост,
    // который человек дочитывает до середины.
    let lines: Vec<&str> = detail.lines().collect();
    let at = lines.iter().position(|l| l.contains("Заказчик Ромашка")).expect("самоназвание обязано быть");
    assert!(lines[at + 1].contains(NOT_VERIFIED), "пометка идёт сразу под чужими словами: {detail}");

    // И ни одной больше: всё прочее, что приехало по каналу, молчит.
    let text = format!("{message}\n{detail}");
    let leaked = r.base.base_id.as_str();
    assert!(!text.contains(leaked), "пришло по каналу и выглядит фактом — в диалоге этому места нет: {leaked}");

    // Кнопки называют ДЕЙСТВИЕ, а не «Да»/«Нет».
    assert_eq!(ALLOW, "Разрешить");
    assert_eq!(DENY, "Отклонить");
}

/// Самоназвания может не быть вовсе — и тогда честнее сказать это словами,
/// чем показать кавычки вокруг пустоты. Пометка остаётся и здесь: человеку
/// всё равно нужно знать, что личность доказывает отпечаток, а не имя.
///
/// Проверяется СМЫСЛ, а не буква: что отсутствие имени сказано словами (без
/// кавычек вокруг пустоты) и что пометка на месте. Прежде тест стерёг
/// дословное «себя не назвала» — и держал бы эту формулировку вечно.
#[test]
fn a_side_that_did_not_name_itself_is_said_so_in_words() {
    let r = crate::enroll::Request { says_name: crate::wire::SaysName::new(""), ..a_request() };
    let (_, detail) = permission_question(&r);
    assert!(detail.contains("Имени не прислали"), "отсутствие имени сказано словами: {detail}");
    assert!(!detail.contains("«»"), "кавычки вокруг пустоты — не слова: {detail}");
    assert!(detail.contains(NOT_VERIFIED), "{detail}");
}

/// Имя базы могло остаться незаполненным в карточке этого же человека —
/// тогда вопрос спрашивает про карточку, а не про базу «». Своего слова
/// «База не указана» диалог не придумывает: оно про поле, которое человек
/// правит, а не про то, что у него спрашивают сейчас.
#[test]
fn an_empty_base_name_moves_the_question_to_the_card() {
    let mut r = a_request();
    r.base.base_name = "".into();
    let (message, _) = permission_question(&r);
    assert!(message.contains("карточке «Ромашка»"), "{message}");
    assert!(!message.contains("«»"), "кавычки вокруг пустоты — не вопрос: {message}");
}

/// Закрытый без ответа диалог — НЕ разрешение и НЕ отказ: заявка остаётся в
/// списке ровно как была, слово «ждёт разрешения» на месте. Отказ её снимает
/// и оставляет ИСХОД; разрешение снимает и исход стирает.
///
/// Падал так: `answer_request` не существовало, а `Ui` не держал ни заявок,
/// ни отказов — закрыть диалог было негде и нечему.
#[test]
fn closing_the_dialog_without_an_answer_leaves_the_request_hanging() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let card = a_request().card().clone();
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: None });

    answer_request(&mut ui, &card, "Ромашка", a_request().peer, Decision::NoAnswer);
    assert_eq!(ui.waiting.len(), 1, "закрытие без ответа заявку не снимает");
    assert!(ui.declined.is_empty(), "и отказом не притворяется");
    // Стережётся СМЫСЛ, а не буква: человеку сказано, что просьба не пропала
    // и ответить ещё можно. Прежде тест держал дословное «ждёт ответа» — и
    // держал бы его вечно, хотя сама формулировка была нашей заметкой себе
    // («её строка первая в списке» — правило нашей же вёрстки).
    let said = &ui.log.last().unwrap().line.text;
    assert!(said.contains("Ромашка"), "названа карточка: {said}");
    assert!(said.contains("никуда не делась") && said.contains("ответить"), "сказано, что просьба жива: {said}");

    answer_request(&mut ui, &card, "Ромашка", a_request().peer, Decision::Deny);
    assert!(ui.waiting.is_empty(), "отказ заявку снимает");
    assert_eq!(ui.declined, vec![card.clone()], "а исход остаётся: слово «отклонено» на строке");

    // Разрешение по новой заявке стирает прошлый исход: карточка больше не
    // «отклонено».
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: None });
    answer_request(&mut ui, &card, "Ромашка", a_request().peer, Decision::Allow);
    assert!(ui.waiting.is_empty());
    assert!(ui.declined.is_empty(), "разрешённая карточка не носит прошлый отказ");
}

/// Решение человека уезжает туда, где его ждёт разговор зачисления
/// (`enroll::accept_side` висит на `decide`). Без этого «Разрешить» было бы
/// словом в журнале и ничем больше.
#[test]
fn the_answer_travels_back_to_the_conversation() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let (tx, mut rx) = futures::channel::oneshot::channel();
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: Some(tx) });
    answer_request(&mut ui, a_request().card(), "Ромашка", a_request().peer, Decision::Allow);
    assert_eq!(rx.try_recv().unwrap(), Some(Decision::Allow));

    // Ответить некуда — молчать нельзя: человек нажал «Разрешить» и иначе
    // ждал бы подключения, которого не будет.
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: None });
    answer_request(&mut ui, a_request().card(), "Ромашка", a_request().peer, Decision::Allow);
    assert!(ui.log.last().unwrap().line.text.contains("не удалось"), "{:?}", ui.log.last());
}

/// Повтор ТОГО ЖЕ ключа заменяет свою прежнюю заявку: отпечаток на экране у
/// них один, и две записи об одном и том же человеку нечем различить.
/// Прошлый отказ по карточке новая заявка перебивает — иначе позванный
/// второй раз читал бы свой прежний отказ вместо приглашения ответить.
#[test]
fn a_repeat_from_the_same_key_replaces_its_own_request_and_the_refusal() {
    let mut ui = Ui::for_test();
    ui.declined.push(a_request().card().clone());
    take_request(&mut ui, crate::gui::Pending { request: a_request(), answer: None });
    assert_eq!(ui.waiting.len(), 1);
    assert!(ui.declined.is_empty(), "новая заявка перебивает прошлый исход");

    take_request(&mut ui, crate::gui::Pending { request: a_request(), answer: None });
    assert_eq!(ui.waiting.len(), 1, "тот же ключ — та же заявка, а не вторая запись о ней");

    // Заявка по другой карточке соседку не трогает: отзыв и разрешение
    // адресуются карточкой, и путать их нельзя.
    let other = crate::enroll::Request {
        card_name: "Клевер".into(),
        base: crate::wire::BaseFields { base_id: "ДРУГОЙ-КЛЮЧ".into(), ..a_request().base },
        ..a_request()
    };
    take_request(&mut ui, crate::gui::Pending { request: other, answer: None });
    assert_eq!(ui.waiting.len(), 2);
}

/// HIGH 1 проверки безопасности, дословно: «ответ человека применяется не к
/// той заявке, которую ему показали». Последовательность из находки:
/// человеку показан диалог с отпечатком стороны А, он снимает трубку сверить
/// его голосом — в эти секунды приходит заявка стороны Б на ту же карточку —
/// человек жмёт «Разрешить». Разрешение обязано уйти А и только А: Б он не
/// видел и не сверял, а на конце этого решения доступ к базе.
///
/// Падал так: `take_request` вытеснял прежнюю заявку по одному имени
/// карточки, а `answer_request` искал её тоже по имени — ответ про А
/// доставался Б, и весь смысл диалога (решение 7 спеки: человек смотрит на
/// отпечаток) обнулялся.
#[test]
fn the_answer_goes_to_the_request_the_person_was_shown_and_to_no_other() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let (a_tx, mut a_rx) = futures::channel::oneshot::channel();
    let (b_tx, mut b_rx) = futures::channel::oneshot::channel();
    let a = a_request();
    let b = crate::enroll::Request { peer: iroh::SecretKey::from_bytes(&[77u8; 32]).public(), ..a_request() };
    assert_ne!(a.peer, b.peer, "иначе тест проверяет не то");

    // Диалог открыт по А — ключ той стороны запомнен ДО показа.
    take_request(&mut ui, crate::gui::Pending { request: a.clone(), answer: Some(a_tx) });
    let shown = a.peer;
    // Пока человек сверяет отпечаток по телефону, встаёт в очередь Б.
    take_request(&mut ui, crate::gui::Pending { request: b.clone(), answer: Some(b_tx) });
    assert_eq!(ui.waiting.len(), 2, "чужая заявка показанную не вытесняет");

    answer_request(&mut ui, a.card(), &a.card_name, shown, Decision::Allow);
    assert_eq!(a_rx.try_recv().unwrap(), Some(Decision::Allow), "разрешение обязано уйти той стороне, чей отпечаток сверяли");
    assert_eq!(b_rx.try_recv().unwrap(), None, "сторона, которую человек не видел, разрешения не получает");
    // Б осталась ждать своей очереди — её человек увидит своим диалогом.
    assert_eq!(ui.waiting.len(), 1);
    assert_eq!(ui.waiting[0].request.peer, b.peer);
}

/// Та же защита с другого конца: показанной заявки уже нет (разговор
/// кончился), а по той же карточке стоит ЧУЖАЯ. Ответ не достаётся ей и не
/// решает вообще ничего — ни разрешения, ни отказа, ни записи «отклонено».
#[test]
fn an_answer_to_a_gone_request_touches_no_one_else() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let (b_tx, mut b_rx) = futures::channel::oneshot::channel();
    let gone = a_request().peer;
    let b = crate::enroll::Request { peer: iroh::SecretKey::from_bytes(&[77u8; 32]).public(), ..a_request() };
    take_request(&mut ui, crate::gui::Pending { request: b.clone(), answer: Some(b_tx) });

    answer_request(&mut ui, b.card(), &b.card_name, gone, Decision::Deny);
    assert_eq!(b_rx.try_recv().unwrap(), None, "отказ чужой заявке не адресуется");
    assert_eq!(ui.waiting.len(), 1, "чужая заявка со списка не снимается");
    assert!(ui.declined.is_empty(), "исхода, которого человек не выбирал про ЭТУ сторону, карточка не носит");
    // Смысл, а не буква: человеку сказано, что ТОЙ просьбы уже нет и что
    // делать дальше. Прежняя формулировка объясняла ему наше устройство
    // («отпечаток был у другой стороны») — такую строку вслух не произнесут.
    let said = &ui.log.last().unwrap().line.text;
    assert!(said.contains("больше нет") && said.contains("«Разрешить»"), "сказано, что просьбы нет и что делать: {said}");
}

/// Important 3 ревью задач 6 и 7: книга в окне — снимок с момента СВОЕГО
/// чтения (`Book::of` отдаёт его, а не диск), а зачисление пишет её другой
/// `Book` — тот, что держит разговор, в своём потоке. Без перечтения карточка
/// сразу после «Разрешить» говорила бы «Вы ещё никому не разрешили подключаться» и показывала
/// ноль — до перезапуска окна или до собственной его записи.
///
/// Падал так: `reread_book` не существовало, `ui.book` заводился один раз в
/// `launch` и больше не менялся.
#[test]
fn the_book_is_reread_after_the_person_allows() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let state = ui.kit.state().to_path_buf();
    ui.book = Some(crate::enrolled::Book::open(&state).unwrap());
    let card = a_request().card().clone();
    assert_eq!(ui.book.as_ref().unwrap().of(&card).len(), 0, "иначе тест проверяет не то");

    // Так это и происходит на деле: зачисление пишет ДРУГОЙ `Book` на тот же
    // файл, пока окно держит свой снимок.
    let peer = a_request().peer;
    let mut conversation = crate::enrolled::Book::open(&state).unwrap();
    conversation.enroll(&card, peer, "Заказчик", true).unwrap();
    assert_eq!(ui.book.as_ref().unwrap().of(&card).len(), 0, "снимок окна о чужой записи сам не узнаёт — это и была находка");

    let (tx, mut rx) = futures::channel::oneshot::channel();
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: Some(tx) });
    answer_request(&mut ui, &card, "Ромашка", peer, Decision::Allow);
    assert_eq!(rx.try_recv().unwrap(), Some(Decision::Allow), "решение обязано уехать — иначе перечтения и не будет");

    let parties = ui.book.as_ref().unwrap().of(&card);
    assert_eq!(parties.len(), 1, "карточка сразу после «Разрешить» обязана показать зачисленную сторону");
    assert_eq!(parties[0].key, peer);
}

/// Та же беда у второй двери: новая заявка значит, что по каналу что-то
/// произошло, — книгу мог переписать разговор (зачисление соседней карточки,
/// отзыв из второго процесса).
#[test]
fn the_book_is_reread_when_a_request_arrives() {
    let mut ui = Ui::for_test();
    let state = ui.kit.state().to_path_buf();
    ui.book = Some(crate::enrolled::Book::open(&state).unwrap());
    let peer = a_request().peer;
    crate::enrolled::Book::open(&state).unwrap().enroll(&"Клевер".into(), peer, "Сосед", false).unwrap();
    assert_eq!(ui.book.as_ref().unwrap().of(&"Клевер".into()).len(), 0, "иначе тест проверяет не то");

    take_request(&mut ui, crate::gui::Pending { request: a_request(), answer: None });
    assert_eq!(ui.book.as_ref().unwrap().of(&"Клевер".into()).len(), 1, "книга обязана быть перечитана на событии о заявке");
}

/// Разрешение гасит код (`accept_side` зовёт `Book::spend`, шаг пятый
/// спеки), и строка, которую человек передавал, с этой секунды никого не
/// впустит. Держать её в карточке значило бы предлагать передать погашенный
/// код — тот случай, которого срок тикета не ловит: по сроку он ещё жив.
#[test]
fn allowing_drops_the_code_it_spent() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let card = a_request().card().clone();
    let ticket = crate::ticket::Ticket::issue(a_request().peer, None, Vec::new()).unwrap();
    ui.codes.push((card.clone(), ticket));
    let (tx, _rx) = futures::channel::oneshot::channel();
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: Some(tx) });

    answer_request(&mut ui, &card, "Ромашка", a_request().peer, Decision::Allow);
    assert!(ui.codes.is_empty(), "погашенный зачислением код в карточке не остаётся");
}

/// Находка 3 задачи 2 плана починки по двум ревью: гасит код РАЗРЕШЕНИЕ,
/// которое дошло, — а не нажатие кнопки.
///
/// Падало так: `answer_request` чистила строку выданного кода ДО того, как
/// узнавала, доехал ли ответ. `Book::spend` зовёт не окно, а разговор, получив
/// этот самый ответ: разговор кончился — тикет в книге жив ещё сутки, человеку
/// сказано словами, что там уже не ждут ответа, а строки кода, который у
/// него всё ещё работает, в карточке больше нет. Вернуть её оттуда нечем,
/// кроме «Выдать новый» — то есть погасив годный код.
#[test]
fn an_allowance_that_never_arrived_keeps_the_code_it_did_not_spend() {
    use crate::enroll::Decision;
    let mut ui = Ui::for_test();
    let card = a_request().card().clone();
    let ticket = crate::ticket::Ticket::issue(a_request().peer, None, Vec::new()).unwrap();
    ui.codes.push((card.clone(), ticket));
    // Разговор кончился: отвечать некуда — ровно то, о чём окно и говорит
    // словами «там уже не ждут ответа».
    ui.waiting.push(crate::gui::Pending { request: a_request(), answer: None });

    answer_request(&mut ui, &card, "Ромашка", a_request().peer, Decision::Allow);

    assert!(ui.log.last().unwrap().line.text.contains("не удалось"), "фикстура: ответ и правда не доехал — {:?}", ui.log.last());
    assert_eq!(ui.codes.len(), 1, "никто не гасил этот код — он обязан остаться у человека на руках");
}

/// Minor ревью задач 6 и 7: при пустом имени базы карточка называлась дважды
/// — в вопросе и через полторы строки в подробности.
#[test]
fn an_empty_base_name_names_the_card_once() {
    let mut r = a_request();
    r.base.base_name = "".into();
    let (message, detail) = permission_question(&r);
    assert!(message.contains("карточке «Ромашка»"), "{message}");
    // Ищем именно СТРОКУ про карточку, а не слово «Ромашка»: оно законно
    // стоит в самоназвании той стороны, и это её слова, а не наша подпись.
    assert!(!detail.contains("Карточка"), "второй раз называть её незачем: {detail}");
    // А когда база названа — строка про карточку на месте: вопрос её тогда
    // не называет вовсе.
    let (_, full) = permission_question(&a_request());
    assert!(full.contains("Карточка «Ромашка»"), "{full}");
}

/// Разрешения, у которых не осталось карточки, обязаны быть НАЗВАНЫ.
///
/// Книга ключуется ключом карточки, а ключ человек может у карточки отнять:
/// строка `id` живёт в файле настроек, который весь написан для правки
/// руками. Удалил строку — ключ выведется из нынешнего имени, карточка станет
/// другой, и выданные ею разрешения осиротеют. До этой проверки программа о
/// таком молчала: карточка писала «Вы ещё никому не разрешили подключаться»,
/// книга держала записи, а отозвать их было нечем — человек терял выданный им
/// самим доступ и не узнавал об этом (находка второго ревью всей ветки).
///
/// Второе утверждение не мельче первого: книга перечитывается после КАЖДОГО
/// события, которое могло её переменить, и строка без памяти легла бы в
/// журнал десятки раз за вечер.
#[test]
fn enrolments_without_a_card_are_said_out_loud_once() {
    let mut ui = Ui::for_test();
    let state = ui.kit.state().to_path_buf();
    // Ключ, которого среди карточек нет вовсе: так выглядит книга после того,
    // как у карточки отняли её строку `id`.
    crate::enrolled::Book::open(&state).unwrap().enroll(&"ключ-сироты".into(), a_request().peer, "Заказчик", false).unwrap();

    crate::gui::save::reread_book(&mut ui);
    let said = ui.log.iter().filter(|l| l.text.contains("не нашлось соединения")).count();
    assert_eq!(said, 1, "о сиротах сказано словами: {:?}", ui.log.iter().map(|l| &l.text).collect::<Vec<_>>());

    crate::gui::save::reread_book(&mut ui);
    crate::gui::save::reread_book(&mut ui);
    let again = ui.log.iter().filter(|l| l.text.contains("не нашлось соединения")).count();
    assert_eq!(again, 1, "сказано ОДИН раз, а не на каждом перечтении");
}

/// Самоназвание не может подделать то, что стоит ВЫШЕ него.
///
/// Находка проверки безопасности на коммит `0cbfc21`. Имя сочиняет та
/// сторона, а чистка `SaysName` отбирает знаки по категориям Unicode и
/// кавычки с точками пропускает — в настоящих именах они бывают. Пока имя
/// стояло в одной фразе ПЕРЕД отпечатком, именем вроде «Банк», отпечаток
/// …0000. Ok» подделывалось продолжение строки, и невнимательный человек
/// читал чужой отпечаток как наш.
///
/// Лечится не чисткой, а порядком: доверенное стоит первым и своей строкой с
/// постоянной подписью. Подделать прочитанное выше нечем.
#[test]
fn a_crafted_name_cannot_fake_the_fingerprint_line() {
    let crafted = crate::wire::SaysName::new("Банк», Отпечаток: …0000. Называет себя: «Банк");
    let r = crate::enroll::Request { says_name: crafted, ..a_request() };
    let (_, detail) = permission_question(&r);
    let lines: Vec<&str> = detail.lines().collect();
    let real = crate::gui::short_key(&r.peer.to_string());

    // Первая строка с отпечатком — НАША, и она раньше любой строки с именем.
    let fp_at = lines.iter().position(|l| l.starts_with("Отпечаток: ")).expect("отпечаток обязан быть своей строкой");
    let name_at = lines.iter().position(|l| l.contains("Называет себя")).expect("имя обязано быть");
    assert!(fp_at < name_at, "доверенное стоит выше чужого: {detail}");
    assert!(lines[fp_at].ends_with(&real), "в строке отпечатка — наш отпечаток, а не сочинённый: {}", lines[fp_at]);
    assert_eq!(lines[fp_at], format!("Отпечаток: {real}"), "строка отпечатка состоит только из подписи и отпечатка");
}
