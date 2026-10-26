//! «Сохранить»: перенос параметров, вопросы перед сменой базы и запись.
//!
//! `save_host` — единственный путь записи; вокруг него перенос того, что
//! человек не вводил заново (`carried_*`), и предупреждения о смене базы.

use super::*;

/// Адрес публикации для записи: умолчание идёт за каталогом базы, правка в
/// toml переживает его смену.
pub fn carried_base_url(saved: &Host, new_base_path: &str) -> String {
    if saved.base_url.is_empty() || saved.base_url == cfg::derived_base_url(&saved.base_path) {
        cfg::derived_base_url(new_base_path)
    } else {
        saved.base_url.clone()
    }
}

/// Каталог выгрузки для записи — тот же приём, что у адреса публикации.
/// `install` дописывает выведенный `<project>\src` в toml, и записанное
/// умолчание обязано и дальше идти за каталогом проекта: иначе смена проекта
/// оставила бы выгрузку в старом месте. Свой каталог (отличается от
/// выведенного) смену `project` переживает. Пустое поле остаётся пустым: там
/// умолчание и так считается при каждом чтении (`Cfg::dump_dir`).
pub fn carried_dump(saved: &Host, new_project: &str) -> String {
    if saved.dump == cfg::derived_dump(&saved.project) {
        cfg::derived_dump(new_project)
    } else {
        saved.dump.clone()
    }
}

/// Порт своей публикации для записи — тот же приём, что у каталога выгрузки:
/// `install` дописывает выведенный `8080 + N` в toml, и записанное умолчание
/// обязано и дальше идти за рабочим местом. Свой порт (отличается от
/// выведенного) смену рабочего места переживает; незаданный остаётся
/// незаданным.
///
/// Переезд не молчаливый: новый порт может оказаться занят — например, вторым
/// коннектором другого рабочего места на этой же машине, — поэтому о нём (как
/// и обо всём прочем, что едет за рабочим местом) `save_host` пишет строку в
/// журнал (`moved_lines`); поднять HTTP-сервис на занятом порту всё равно не
/// выйдет, и операция публикации скажет, кто его держит.
pub fn carried_http_port(saved: &Host, new_workstation: &str) -> Option<u16> {
    if saved.http_port == Some(cfg::derived_http_port(&saved.workstation)) {
        Some(cfg::derived_http_port(new_workstation))
    } else {
        saved.http_port
    }
}

/// Что переехало за переименованием — одной строкой на каждую причину.
/// Правило одно на все переезды: о каждом говорим, ни об одном не молчим
/// (находка ревью задачи 35 — прежде журнал называл только порт публикации,
/// хотя за рабочим местом едут ещё ветка проекта, `origin` и порты, а за
/// каталогом проекта — каталог выгрузки). `Warn`, а не `Ok`: новые порты
/// может держать другой коннектор, а прежний каталог выгрузки остаётся
/// лежать со старым содержимым.
pub fn moved_lines(old: &Host, new: &Host) -> Vec<String> {
    let mut v = Vec::new();
    if old.workstation != new.workstation {
        let p = Cfg { host: new.clone() }.ports();
        let port = match (old.http_port, new.http_port) {
            (Some(from), Some(to)) if from != to => format!(", порт публикации {from} → {to}"),
            _ => String::new(),
        };
        v.push(format!(
            "за рабочим местом {} → {} переехали: ветка проекта и origin, порты {}/{}/{}{port}",
            old.workstation, new.workstation, p.mcp, p.vrunner, p.web
        ));
    }
    if old.project != new.project && old.dump != new.dump {
        v.push(format!("за каталогом проекта переехал каталог выгрузки: {} (прежний остался со старым содержимым)", new.dump));
    }
    v
}

/// Поля, что решают, какую базу обслуживает публикация/MCP — путь, логин и
/// пароль дескриптора `publication::vrd`. Единый список: `save_host` смотрит
/// на него, не поднять ли публикацию заново, а подтверждение перед
/// «Сохранить» (`save_needs_confirm`) — не сменилась ли сама база — читает
/// тот же список, а не второй, отдельно придуманный.
pub fn database_changed(a: &Host, b: &Host) -> bool {
    a.base_path != b.base_path || a.db_user != b.db_user || a.db_password != b.db_password
}

/// Сохранить параметры сервера. Списка инструментов, каталога выгрузки,
/// режима публикации, её порта, адреса Gitea среды и адреса публикации в окне
/// нет (правятся в toml): берём их из текущих параметров, иначе пустые
/// значения из формы затёрли бы toml. Каталог проекта — из формы, и за ним
/// едут умолчания: адрес публикации (`carried_base_url`), каталог выгрузки
/// (`carried_dump`) и — за рабочим местом — порт публикации
/// (`carried_http_port`). О каждом переезде — строка в журнал
/// (`moved_lines`).
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    let old_host = ui.cfg.host.clone();
    host.gitea = ui.cfg.host.gitea.clone();
    host.dump = carried_dump(&ui.cfg.host, &host.project);
    host.platform = ui.cfg.host.platform.clone();
    host.tools = ui.cfg.host.tools.clone();
    host.mode = ui.cfg.host.mode.clone();
    host.publication = ui.cfg.host.publication.clone();
    host.http_port = carried_http_port(&ui.cfg.host, &host.workstation);
    host.base_url = carried_base_url(&ui.cfg.host, &host.base_path);
    // Переезды — не молчком, и все до одного: см. `moved_lines`.
    let moved = moved_lines(&old_host, &host);
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    write_text(&ui.kit.state_path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    let base_path = cfg.host.base_path.clone();
    // Дескриптор публикации (`publication::vrd`) несёт путь базы, логин и
    // пароль — переподнять его нужно при смене любого из трёх, не только
    // пути: иначе смена пароля/логина оставляла бы старый дескриптор живым,
    // и публикация продолжала бы открывать базу под старыми учётными
    // данными (находка повторного ревью).
    let descriptor_changed = database_changed(&cfg.host, &old_host);
    // То же сравнение, что решает, светится ли «Сохранить» (`panel::fields_differ`):
    // поля, которых окно не показывает (в т.ч. `mode`), сеанс не перезапускают
    // здесь — смена режима в трее поднимает его сама, отдельным путём
    // (`tray::on_menu`), и лишний перезапуск в довесок к тому был бы только
    // лишней строкой в журнал и лишним миганием портов.
    let changed = panel::fields_differ(&cfg.host, &old_host, &cfg.host.mode);
    ui.cfg = cfg;
    for text in moved {
        note_line(ui, Line { kind: Kind::Warn, text });
    }
    // Публикация не должна молчать про старую базу: иначе агент отвечает из
    // прежней, пока кто-нибудь не перезапустит HTTP-сервис руками (находка
    // целостного ревью) — до следующего вызова она обязана смотреть на
    // новую. `ensure_publication` сама решает «auto»/«webserver» и когда уже
    // «сделано» — здесь только зовём её, не повторяя эту логику.
    if republish_after_save(descriptor_changed, ui.session.is_some()) {
        ensure_publication_now(ui);
    }
    if changed && ui.session.is_some() {
        if ui.cfg.host.mode == "local" {
            // Локальный режим: слушатели подняты на весь срок жизни процесса
            // и не трогаются переключателем/«Сохранить» (см. `launch`) —
            // спека прямо требует, чтобы смена базы была незаметна для Zed:
            // слушатели и уже принятые соединения не рвутся, следующий вызов
            // агента просто уходит в новую базу. `Serve::dispatch` берёт её
            // из этой же общей ячейки на каждое новое соединение — полный
            // перезапуск сеанса (`restart_session`) здесь не нужен и раньше
            // был блокирующей находкой целостного ревью (человеку пришлось
            // бы переподключать два сервера в Zed руками).
            if let Some(s) = &ui.session {
                s.update_cfg(ui.cfg.clone());
            }
            note_line(ui, Line { kind: Kind::Say, text: format!("MCP: параметры обновлены, база {base_path}") });
        } else {
            restart_session(ui, base_path);
        }
    }
    // Почва могла сдвинуться (база, логин, пароль, порт публикации) —
    // вердикт «дойдёт ли агент до базы» устарел, пересчитываем. Здесь же
    // пересчитывается замок, и если правка убрала холодное условие (стёрли
    // пользователя базы), `refresh_gate` сам гасит обслуживание и говорит об
    // этом в журнал — своей ветки «Сохранить» для этого больше не держит.
    // Спросили об этом раньше, до записи (`save_needs_confirm`): молчать
    // нельзя, агент остаётся без обслуживания.
    run_readiness(ui);
    Ok(())
}

/// Чистое ядро вопроса «погасит ли „Сохранить“ обслуживание»: замок был
/// открыт, обслуживание живо, а на новых параметрах условия не хватает.
/// Заперто и до правки — гасить нечего, вопрос не нужен.
pub fn locks_gate(gate_before: Option<&str>, gate_after: Option<String>, serving: bool) -> Option<String> {
    if gate_before.is_some() || !serving {
        return None;
    }
    gate_after
}

/// То же на живых параметрах. Полей, которых окно не показывает, в форме нет —
/// пустой режим берём из текущих параметров, как это делает сама `save_host`:
/// иначе туннельная проверка адреса среды применилась бы к локальному рабочему
/// месту. Непустой режим — это смена режима из трея, и спрашивать надо как
/// раз про новый: замки у режимов разные (туннелю нужны адрес среды, пароль
/// и имя рабочего места, локальному — нет). «Было» берём из кэша `ui.gate`
/// (он пересчитывается по тем же событиям и его же читает `toggle_link`), на
/// диск ходим ровно раз — за «станет»: нажатие «Сохранить» не должно стоить
/// шести обходов `Program Files` на главном потоке окна.
pub fn save_locks_gate(ui: &Ui, new_host: &Host) -> Option<String> {
    let serving = ui.enabled.load(Ordering::Relaxed) && ui.session.is_some();
    if !serving || ui.gate.is_some() {
        return None;
    }
    let mut host = new_host.clone();
    if host.mode.is_empty() {
        host.mode = ui.cfg.host.mode.clone();
    }
    locks_gate(ui.gate.as_deref(), gate_now(&ui.kit, &Cfg { host }), serving)
}

/// О чём спросить перед «Сохранить», и спрашивать ли вообще: готовый текст
/// диалога или `None`. Один расчёт на нажатие — и «нужен ли вопрос», и «какой
/// текст» отвечает он же, а не два одинаковых похода на диск подряд.
/// Погасшее обслуживание важнее смены базы: оно оставляет агента без ответа
/// вовсе.
pub(crate) fn save_question(ui: &Ui, new_host: &Host) -> Option<(String, String)> {
    if let Some(why) = save_locks_gate(ui, new_host) {
        return Some(confirm_gate_text(&why));
    }
    let live = ui.enabled.load(Ordering::Relaxed) && ui.session.is_some();
    if database_changed(new_host, &ui.cfg.host) && live {
        return Some(confirm_text(&ui.cfg.host.base_path, &new_host.base_path));
    }
    None
}

/// Нужно ли спросить перед «Сохранить»: база меняется (`database_changed`
/// выше — тот же список полей, что у публикации, не второй, отдельно
/// придуманный), агент прямо сейчас может отвечать (`enabled` — его читает
/// мост на каждый вызов, в обоих режимах, не только в локальном), и сеанс
/// жив (`ui.session`). Любая другая правка — каталог проекта, режим,
/// рабочее место — сохраняется без вопроса, как и было задумано (заказчик
/// просил вопрос именно и только в этом сочетании, 2026-10-20).
/// Второй повод спросить — тот же диалог: правка убирает холодное условие и
/// тем гасит обслуживание (`save_locks_gate`). Заказчик требовал вопроса
/// именно в этом сочетании: цель спеки — чтобы смена параметров была
/// незаметной для Zed, а тут агент остаётся без обслуживания.
pub fn save_needs_confirm(ui: &Ui, new_host: &Host) -> bool {
    save_question(ui, new_host).is_some()
}

/// Имя базы для диалога — последний компонент пути («Buh» для C:\Bases\Buh),
/// как 1С-разработчик называет базу в разговоре; полный путь уходит в
/// detail, не в это имя. Пустой/корневой путь без компонента — путь целиком,
/// лишь бы в диалоге не было пустой строки.
pub fn base_label(path: &str) -> &str {
    // Режем по обоим разделителям сами, а не средствами системы: на Linux
    // `Path::file_name` не считает `\` разделителем, и из `сервер\база`
    // (такую базу коннектор принимает на обеих системах) имя не выделилось
    // бы вовсе — в диалоге оказалась бы вся строка. Нашлось, когда тесты
    // окна впервые пошли на Linux (CI, 2026-10-21).
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// Текст диалога подтверждения: два предложения (что сейчас, что станет
/// после, и сам вопрос) и деталь с обоими полными путями.
pub fn confirm_text(old_path: &str, new_path: &str) -> (String, String) {
    let message = format!(
        "Агент сейчас работает с базой «{}», после сохранения перейдёт на «{}». Сохранить?",
        base_label(old_path),
        base_label(new_path)
    );
    (message, format!("{old_path} → {new_path}"))
}

/// Текст того же диалога, когда правка не меняет базу, а гасит обслуживание:
/// называем последствие и причину, а не одно «Сохранить?».
pub fn confirm_gate_text(why: &str) -> (String, String) {
    ("После сохранения MCP выключатся — агент перестанет отвечать. Сохранить?".to_string(), format!("включать будет нечего: {why}"))
}

/// Диалог перед «Сохранить», когда база меняется под живым и включённым MCP
/// (см. `save_needs_confirm`) — иначе `proceed` зовётся сразу, без вопроса.
/// Платформенный диалог (`Window::prompt`), тот же приём, что у выбора
/// каталога (`Panel::pick_dir`): `dialog_open` не даёт окну спрятаться под
/// диалогом (`Panel::new`'s `observe_window_activation`) и не даёт открыть
/// его второй раз; ответ приходит в своём потоке задачи — главный поток
/// ничего не ждёт. `proceed` зовётся ровно раз («да», или вопроса не было)
/// либо никогда (отказ — поля окна остаются как человек их оставил, здесь
/// ничего не трогаем).
pub fn confirm_database_change(window: &mut Window, cx: &mut App, new_host: &Host, proceed: impl FnOnce(&mut Window, &mut App) + 'static) {
    // Один диалог на оба повода (спека: «тот же диалог подтверждения, что уже
    // спрашивает при смене базы»), и один расчёт на нажатие: текст приходит
    // вместе с ответом «спрашивать ли».
    let Some((message, detail)) = save_question(cx.global::<Ui>(), new_host) else {
        proceed(window, cx);
        return;
    };
    if cx.global::<Ui>().dialog_open {
        // Молчать нельзя: человек нажал кнопку и обязан узнать, почему ничего
        // не произошло. Молчаливый выход отсюда и был вторым тупиком той же
        // кнопки.
        set_status(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: "Сначала ответьте на уже открытый вопрос".into() }, None);
        return;
    }
    cx.global_mut::<Ui>().dialog_open = true;
    let rx = window.prompt(
        PromptLevel::Warning,
        &message,
        Some(&detail),
        &[PromptButton::ok("Да, сохранить"), PromptButton::cancel("Нет, не сохранять")],
        cx,
    );
    cx.spawn(async move |cx| {
        let yes = matches!(rx.await, Ok(0));
        cx.update(|cx| {
            cx.global_mut::<Ui>().dialog_open = false;
            if !yes {
                return;
            }
            let Some(w) = cx.global::<Ui>().window else { return };
            let _ = w.update(cx, |_, window, cx| {
                window.activate_window();
                proceed(window, cx);
            });
        });
    })
    .detach();
}

/// Поднять/проверить HTTP-сервис на уже записанных `ui.cfg`: после смены
/// базы («Сохранить», см. выше), при включении MCP (`toggle_link`) и при
/// старте с «подключаться при запуске» (`launch`). `ensure_publication` сама
/// идемпотентна и решает режим, но пробует связь с таймаутом в 3 с, в режиме
/// «webserver» ходит в сеть за версией расширения, а после запуска своего
/// httpd ждёт его готовности — всё это в своём потоке, а не на главном
/// потоке окна (находка повторного ревью: окно подвисало на эти секунды).
/// Ответ идёт `Ev::Publish`: журнал на отказе и карточка — в `handle`, не
/// здесь, иначе карточка соврала бы, что всё в порядке, пока агент отвечает
/// с ошибкой.
/// Поднимать ли HTTP-сервис базы после записи параметров. Условий два, и
/// второе появилось по замечанию заказчика 2026-10-21: «сохранил — в журнале
/// сообщение об успешном поднятии http сервера, хотя я даже не пытался
/// включить MCP».
///
/// Переменилась база — старый дескриптор указывает не туда, и сервис обязан
/// посмотреть на новую, иначе агент отвечал бы из прежней базы. Но нужно это
/// только тому, кто сейчас обслуживает: при выключенном переключателе мост
/// отказывает ещё до публикации, и поднимать её — работа, которой никто не
/// просил, да ещё и объявленная в журнале как успех. Включат MCP —
/// `ensure_publication` сходит сама.
pub fn republish_after_save(database_changed: bool, serving: bool) -> bool {
    database_changed && serving
}

/// Подъём публикации в своём потоке. Голос к базе он берёт не здесь, а внутри
/// `ops_host::extension_reach` — в единственном месте, которое правда говорит
/// с базой (Important 2 ревью круга 2): мьютекс вокруг потока можно отменить,
/// убрав одну строку, а дорогу к базе мимо `extension_reach` не проложить.
pub fn ensure_publication_now(ui: &mut Ui) {
    let ctx = ctx_for(ui);
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        let r = ops_host::ensure_publication(&ctx).map_err(|e| format!("{e:#}"));
        let _ = tx.unbounded_send(Ev::Publish(r));
    });
}

#[cfg(test)]
mod tests;
