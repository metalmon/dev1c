use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::cfg::{self, Connection};
use crate::session::State;
use common::op::OpState;
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
/// проекта и `origin` — ни числа портов, ни слова «порт» в ней больше нет
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
    assert!(ui.log.iter().any(|l| l.text.contains("ветка проекта и origin")), "{:?}", ui.log);
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
    assert!(line.contains("ветка проекта и origin"), "{line}");
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
    for part in ["dev1 → dev3", "ветка проекта и origin"] {
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
    let doused: Vec<&Line> = ui.log[log_before..].iter().filter(|l| l.text.starts_with("MCP выключены:")).collect();
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
    save_tray_key(&mut ui, "write", "read_write").unwrap();
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(!text.contains("[host]"), "обеих половин в файле быть не должно — такой файл чтение отвергает:\n{text}");
    let back: Cfg = toml::from_str(&text).expect("файл после записи разбирается");
    assert!(back.read_notes.is_empty(), "и читается без находок: {:?}", back.read_notes);
    assert_eq!(back.current().write, "read_write", "ключ попал в блок текущего соединения");
    assert_eq!(back.connections[0].write, "", "а соседний блок не тронут");
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

/// Круг починки 1, находка Important 1 ревью: прежний файл с одной секцией
/// `[host]` — форма эталонного комплекта (`C:\dev1c-connect-kit`). До правки
/// `rename` писала новое имя в `name` этой секции, а следующее чтение
/// (`cfg.rs`, разбор `CfgFile`) немедленно затирало его выводом из каталога
/// базы — переименование не приживалось, окно говорило «готово», а
/// перечитанный файл называл прежнее имя, да ещё с замечанием «нет такого
/// соединения».
#[test]
fn renaming_sticks_on_the_old_single_host_file_shape() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let path = ui.kit.state_path(cfg::FILE);
    std::fs::write(&path, "[host]\nbase_path = \"C:\\\\Bases\\\\Buh\"\nserver = \"192.168.1.10:3393\"\n").unwrap();
    ui.cfg = Cfg::load_unchecked_with(&ui.kit, &store).unwrap();
    let from = ui.cfg.current().name.clone();

    rename(&mut ui, &from, "Ромашка").unwrap();

    let reread = Cfg::load_unchecked_with(&ui.kit, &store).unwrap();
    assert_eq!(reread.current().name, "Ромашка", "имя обязано прилипнуть на прежнем файле «[host]»");
    assert!(reread.read_notes.is_empty(), "перечитанный файл не обязан жаловаться сам на себя: {:?}", reread.read_notes);
}

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

/// Роль приходит параметром — кнопка списка ставит `"accept"`, онбординг
/// (задача 11) передаст свою.
#[test]
fn the_role_parameter_lands_on_the_new_card() {
    let mut ui = Ui::for_test();
    let name = add_connection(&mut ui, "dial").unwrap();
    assert_eq!(ui.cfg.by_name(&name).unwrap().role, "dial");
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
    // `cfg::default_name` считает занятое «Соединение 1» самим базовым
    // именем, а не числом в нём, и даёт следующее в своём же ряду —
    // «Соединение 1 2» (то же правило, что у двух одноимённых баз).
    assert_eq!(name, "Соединение 1 2", "«Соединение 1» занято на диске, в памяти его не было вовсе");
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
/// `[[connection]]` (здесь — прежняя форма `[host]`, тот же нулевой счёт)
/// печатает список целиком из памяти. Если пароль какого-то соединения не
/// расшифровался на этой машине, он лежит в памяти ПУСТОЙ строкой
/// (`ui.cfg.undecryptable` называет поле) — наивная печать записала бы это
/// пустое поле на диск поверх настоящего шифртекста, безвозвратно и молча.
/// Отказ обязан сработать ДО какой-либо записи, и файл обязан остаться
/// байт в байт прежним.
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
fn add_connection_refuses_instead_of_wiping_an_unreadable_password_on_the_old_file_shape() {
    let mut ui = Ui::for_test();
    let store = crate::secrets::Store::file(ui.kit.state());
    let name = ui.cfg.current().name.clone();
    let ciphertext = store.seal("password", "секрет с другой машины").unwrap();
    let path = ui.kit.state_path(cfg::FILE);
    let before = format!("[host]\nbase_path = \"C:\\\\Bases\\\\Buh\"\nserver = \"192.168.1.10:3393\"\npassword = {ciphertext:?}\n");
    std::fs::write(&path, &before).unwrap();
    assert_eq!(common::toml_render::count(&before, cfg::CONNECTION), 0, "фикстура: файл без блоков — прежняя форма");

    // «Чужая машина»: пароль не открылся, поле пустое, имя — в списке
    // непрочитанного, тем же приёмом, что у соседних тестов `save_host`.
    ui.cfg.current_mut().password = String::new();
    ui.cfg.undecryptable = vec![cfg::Undecryptable::new(&name, "password")];

    let err = add_connection(&mut ui, "accept").unwrap_err();
    assert!(err.contains(&name) && err.contains("пароль"), "{err}");

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, before, "отказ — файл не тронут ни на байт, прежний шифртекст цел");
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
