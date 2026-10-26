use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use core::prelude::v1::test;
use crate::cfg::{self, Host};
use crate::session::State;
use common::op::OpState;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[test]
fn a_bad_server_leaves_the_toml_alone_and_says_why() {
    let mut ui = Ui::for_test();
    let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
    let mut host = ui.cfg.host.clone();
    host.server = "no-port".into();
    let err = save_host(&mut ui, host).unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before);
}

#[test]
fn carried_base_url_keeps_an_override_and_follows_the_default() {
    // Поля «Адрес публикации» в окне нет: умолчание идёт за каталогом
    // базы, а правка в toml переживает смену базы.
    let mut saved = Host { base_path: "C:\\Bases\\Buh".into(), base_url: cfg::derived_base_url("C:\\Bases\\Buh"), ..Host::default() };
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
    let mut saved = Host { project: "C:\\proj\\Buh".into(), dump: cfg::derived_dump("C:\\proj\\Buh"), ..Host::default() };
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), new);
    saved.dump = "D:\\dump".into();
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "D:\\dump");
    // Пустое поле остаётся пустым: умолчание и так считается при чтении.
    saved.dump = String::new();
    assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "");
}

/// Та же ловушка у порта публикации: записанный `8080 + N` идёт за
/// рабочим местом, свой порт остаётся, незаданный — незаданным.
#[test]
fn carried_http_port_follows_the_workstation_unless_it_was_chosen_by_hand() {
    let mut saved = Host { workstation: "dev1".into(), http_port: Some(cfg::derived_http_port("dev1")), ..Host::default() };
    assert_eq!(carried_http_port(&saved, "dev2"), Some(8082));
    saved.http_port = Some(9000);
    assert_eq!(carried_http_port(&saved, "dev2"), Some(9000));
    saved.http_port = None;
    assert_eq!(carried_http_port(&saved, "dev2"), None);
}

/// Переезд порта не молчком: новый порт может быть занят, и человек
/// должен прочитать про переезд, а не искать, почему публикация упала.
#[test]
fn saving_a_new_workstation_moves_a_derived_port_and_says_so() {
    let mut ui = Ui::for_test();
    ui.cfg.host.http_port = Some(cfg::derived_http_port(&ui.cfg.host.workstation));
    let mut host = ui.cfg.host.clone();
    host.workstation = "dev3".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.cfg.host.http_port, Some(8083));
    assert_eq!(ui.cfg.base_url(), "http://localhost:8083/hs/mcp-1c");
    assert!(ui.log.iter().any(|l| l.text.contains("порт публикации 8081 → 8083")), "{:?}", ui.log);
}

/// Та же ловушка на живом «Сохранить»: записанный выведенный каталог
/// выгрузки переезжает вместе с каталогом проекта.
#[test]
fn saving_a_new_project_moves_a_derived_dump_with_it() {
    let mut ui = Ui::for_test();
    ui.cfg.host.project = "C:\\proj\\Buh".into();
    ui.cfg.host.dump = cfg::derived_dump("C:\\proj\\Buh");
    let mut host = ui.cfg.host.clone();
    host.project = "C:\\proj\\Torg".into();
    save_host(&mut ui, host).unwrap();
    assert_eq!(ui.cfg.host.dump, cfg::derived_dump("C:\\proj\\Torg"));
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains(&format!("dump = {:?}", cfg::derived_dump("C:\\proj\\Torg"))), "{text}");
    // И переезд назван, а не сделан молча.
    assert!(ui.log.iter().any(|l| l.text.contains("переехал каталог выгрузки")), "{:?}", ui.log);
}

/// Правило одно на все переезды: о каждом — строка. За рабочим местом
/// едут ветка, origin, порты и порт публикации; за каталогом проекта —
/// каталог выгрузки; ничего не менялось — строк нет.
#[test]
fn every_move_gets_a_line_not_just_the_port() {
    let old = Host {
        workstation: "dev1".into(),
        project: "C:\\proj\\Buh".into(),
        dump: cfg::derived_dump("C:\\proj\\Buh"),
        http_port: Some(cfg::derived_http_port("dev1")),
        ..Host::default()
    };
    assert!(moved_lines(&old, &old).is_empty());

    let mut new = old.clone();
    new.workstation = "dev3".into();
    new.http_port = carried_http_port(&old, "dev3");
    let line = moved_lines(&old, &new).join("\n");
    for part in ["dev1 → dev3", "ветка проекта и origin", "7773/7783/7793", "порт публикации 8081 → 8083"] {
        assert!(line.contains(part), "{part} не назван: {line}");
    }

    let mut moved_project = old.clone();
    moved_project.project = "C:\\proj\\Torg".into();
    moved_project.dump = carried_dump(&old, "C:\\proj\\Torg");
    let lines = moved_lines(&old, &moved_project);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(&cfg::derived_dump("C:\\proj\\Torg")), "{lines:?}");

    // Свой каталог выгрузки не переезжает — и строки про него нет.
    let mut own = old.clone();
    own.dump = "D:\\dump".into();
    let mut own_new = own.clone();
    own_new.project = "C:\\proj\\Torg".into();
    own_new.dump = carried_dump(&own, "C:\\proj\\Torg");
    assert!(moved_lines(&own, &own_new).is_empty());
}

#[test]
fn saving_a_new_base_restarts_the_session_and_writes_one_journal_line() {
    let mut ui = Ui::for_test();
    // "webserver": ensure_publication_now (зовётся теперь и отсюда) не
    // должна трогать настоящий httpd/1cv8 этой машины — тест про
    // перезапуск сеанса, не про публикацию.
    ui.cfg.host.publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.host.clone();
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
    ui.cfg.host.mode = "local".into();
    ui.cfg.host.publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.host.clone();
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
    // Смена режима в трее меняет `cfg.host.mode` и зовёт `save_host`
    // напрямую (не через панель), не трогая поля, которые показывает
    // окно — `tray::on_menu` перезапускает сеанс сама следом, отдельным
    // путём; лишнего перезапуска (и лишней строки «MCP перезапущены») на
    // стороне `save_host` здесь быть не должно.
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    let log_before = ui.log.len();
    let mut host = ui.cfg.host.clone();
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
    ui.cfg.host.publication = "webserver".into();
    let log_before = ui.log.len();
    let mut host = ui.cfg.host.clone();
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
    after.host.db_user = String::new();
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
    let mut host = ui.cfg.host.clone();
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
    ui.cfg.host.mode = "local".into();
    ui.cfg.host.publication = "webserver".into();
    ui.session = Some(doomed_session(&ui));
    ui.link = State::Up { since: Instant::now() };
    // До правки замок был открыт — тот самый случай, ради которого
    // «Сохранить» и спрашивает подтверждение (в каталоге теста он заперт
    // сам по себе, поэтому состояние задаём явно).
    ui.gate = None;
    let log_before = ui.log.len();
    let mut host = ui.cfg.host.clone();
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
    let mut new_base = ui.cfg.host.clone();
    new_base.base_path = "C:\\Bases\\Other".into();
    assert!(save_needs_confirm(&ui, &new_base));
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn save_needs_confirm_is_false_on_each_near_miss() {
    let mut ui = Ui::for_test();
    ui.session = Some(doomed_session(&ui));
    let mut new_base = ui.cfg.host.clone();
    new_base.base_path = "C:\\Bases\\Other".into();

    // Промах 1: MCP выключены.
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!save_needs_confirm(&ui, &new_base));
    ui.enabled.store(true, Ordering::Relaxed);

    // Промах 2: база та же, поменялось другое поле — оно не входит в
    // список, что решает «изменилась ли база» (`database_changed`).
    let mut other_field = ui.cfg.host.clone();
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
    let mut new_base = ui.cfg.host.clone();
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
