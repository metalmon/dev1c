//! Холодный замок готовности: что должно быть на месте, чтобы человеку вообще было что включать (окно) или подкоманде `1c-connect mcp` — что агенту вообще отвечать. Спека, раздел «Зелёный значит „работает“: переключатель под замком готовности» — решение заказчика 2026-10-20: «Сам переключатель включения должен быть недоступен, пока нет пользователя и пока не установлены MCP».
//!
//! Отбор условий простой и назван спекой: **смертельно для работы и видно без сети**. Всё, что видно только после включения (отвечает ли публикация, поднята ли среда), — горячее: оно красит точку, но переключатель не запирает, иначе локальный режим запер бы его навсегда (публикацию там поднимает сам коннектор при включении).
//!
//! Функция чистая: ни сети, ни файловой системы. Всё, за чем пришлось бы ходить на диск, передаётся аргументами — тогда весь список условий проверяется тестом, а не живой машиной.
//!
//! Модуль не за фичей `gui`: сами условия не зависят от окна, а подкоманде `1c-connect mcp` (`ops_host::run_mcp`, спека «MCP в редакторе», ловушка «Лицензия и холодный замок») нужны ровно они же — до первого ответа агенту, без второй формулировки причин отказа. Окно видит их тем же путём `gui::gate::cold_gate`/`gui::link::gate_now` — там теперь реэкспорт отсюда.

use crate::cfg;
use crate::cfg::Cfg;
use crate::licence;
use crate::ops_host;
use common::kit::Kit;
use common::op::OpState;
use std::path::Path;

/// Заперт ли переключатель. `None` — включать можно; `Some(причина)` — нельзя, и причина короткой строкой для подсказки (в заголовок карточки она не идёт: «не надо там прозу писать, она не лезет», заказчик 2026-10-21).
///
/// Порядок условий — от основания к вершине, тот же, что у `verify::order`: человек читает первое, что сломано, а не последнее.
///
/// Аргументы, которых нет в `Cfg`, — это и есть «видно без сети, но нужен диск»: `params_exists` — лежит ли файл параметров, `base_exists` — есть ли каталог файловой базы, `tools` — полон ли комплект (инструменты работают из него, никуда не раскладываясь) (`ops_host::tools_state`), `platform` — найдена ли платформа (`ops_host::platform_choice` — `Err` несёт причину словами), `mcp_in_kit` — есть ли `mcp-1c` в комплекте.
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
    // Пустая «Связь» — раньше файла параметров: на первом экране (онбординг)
    // ни полей, ни кнопки «Сохранить» нет вовсе (`panel::link_body` там
    // рисует факт и два ответа, не форму) — отказ ниже звал бы человека
    // туда, куда с этого экрана не попасть (разбор живого прогона 9.11,
    // владелец смотрел в окно вместе с исполнителем). Причина говорит про
    // то, что видно: соединений нет — первое заводится ответом на экране.
    if cfg.connections.is_empty() {
        return Some("соединений нет — создайте первое".into());
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
    // Нерасшифрованный пароль базы (а в туннельном режиме — и пароль рабочего места) — то же условие, что держит CLI (`Cfg::load_with`), только тот отказывает раньше, самим `Cfg::load`, а окно вместо отказа открывается и до этой правки молчало: переключатель включался, мост ехал с пустым паролем, «Установить расширение» — тоже (ревью задачи 7, Important 2). Смертельно для работы и видно без сети — ровно то, чем замок и отбирает условия; причина — короткой строкой, без упоминания окна или кнопок, которых здесь нет.
    if !cfg::blocking_undecryptable(cfg).is_empty() {
        return Some("пароль не расшифрован — введите заново".into());
    }
    // «Сделано» и ничего другого. Копии инструментов на машине больше нет
    // (леджер комплекта, пункт 15), поэтому единственный не-`Done` исход —
    // неполный комплект, и совет не «Установить», а взять комплект целиком:
    // поставить нам нечего, брать нечего.
    if !matches!(tools, OpState::Done(_)) {
        return Some("инструментов в комплекте нет (vrunner/, git/, httpd/) — скопируйте полный комплект заново".into());
    }
    let h = cfg.current();
    // Заполненность полей спрашивается ОДНОЙ мерой на весь продукт (`need::missing`): замок отвечает строкой причины, а подсветке в окне
    // нужно само поле, и из-за этого замок не мог служить валидатором, хотя проверял то же самое. Теперь условия полей живут там, а здесь
    // — только порядок и слова. Правило «обязательность принадлежит ДЕЙСТВИЮ» оттуда же: замку нужен `Deed::Serve`.
    //
    // Та же мера принесла и починку: условия базы спрашивались у ЛЮБОЙ карточки, в том числе у той, что зовёт чужую базу (ревью пути
    // 10.11, находка 2). Такой карточке окно показывает одно поле «Папка документов» и подпись «Эти поля заполняют на компьютере с
    // базой» — то есть замок требовал заполнить то, чего человеку не показывают, и обслуживание у разработчика не включалось никогда.
    // `need::missing` спрашивает про базу только у того, у кого она есть (`base_is_here`).
    let want = crate::need::missing(crate::need::Deed::Serve, h);
    // Пустое поле — «не указана база», а не «каталога  нет»: в пустой строке
    // человеку нечего прочитать. Поэтому пустая база раньше проверки того,
    // существует ли названный ею каталог.
    if want.first() == Some(&crate::need::Field::BasePath) {
        return Some(crate::need::Field::BasePath.said().into());
    }
    // Правило про базу не переписываем: оно уже записано фактом проверки
    // (`verify::base_fact`) — каталог обязателен только файловой базе, а
    // `сервер\база` и сетевой путь существованием не проверяются, иначе
    // серверная база заперла бы переключатель навсегда.
    if h.base_is_here() && !crate::verify::base_fact(&h.base_path, base_exists).ok {
        return Some(format!("каталога базы {} нет", h.base_path));
    }
    // Остальное, чего не хватает обслуживанию: пользователь базы, адрес чужой публикации. Порядок — тот же, что у `need::missing`:
    // порядок решений человека.
    if let Some(f) = want.first() {
        return Some(f.said().into());
    }
    None
}

/// Нужно ли вообще ходить на диск за каталогом базы. Классификация та же, что у `verify::base_fact`: сетевой путь (`\\…`) и `сервер\база` существованием не проверяются — факт их и не смотрит.
///
/// Нужна не ради красоты, а ради главного потока окна: `exists()` по недоступной сетевой шаре висит на таймауте SMB секундами, а замок считается на показ окна, конец каждой операции и «Сохранить». Существование базы считается в своём потоке — в `verify::gather`, тот же запрет и там.
pub fn wants_directory(path: &str) -> bool {
    // Само правило живёт в `verify` — в слое, который собирается всегда:
    // окно идёт по фиче `gui`, и обратное направление (`verify` → `gui`)
    // ломало бы сборку без окна.
    crate::verify::wants_directory(path)
}

/// Холодный замок на текущих параметрах: всё, за чем надо на диск, спрашиваем здесь, решает `cold_gate`. Общая для окна (`gui::link::gate_now` — реэкспорт) и для подкоманды `1c-connect mcp` (`ops_host::run_mcp`): обеим нужен ровно один и тот же вердикт, а не две формулировки причин отказа.
pub fn gate_now(kit: &Kit, cfg: &Cfg) -> Option<String> {
    let tools = ops_host::tools_state(&kit.root);
    // Существование спрашиваем только у файловой базы (`wants_directory`,
    // правило самого факта «база»): `exists()` по недоступной сетевой шаре
    // висит на таймауте SMB, а считается это на главном потоке окна.
    let base = &cfg.current().base_path;
    let base_exists = wants_directory(base) && std::path::Path::new(base).exists();
    let platform = ops_host::platform_choice(cfg);
    cold_gate(
        cfg,
        kit.state_path(crate::cfg::FILE).exists(),
        base_exists,
        &tools,
        // Выбор платформы и причина отказа — одной лестницей (`platform_choice`):
        // отказ запирает переключатель тем же текстом, что показывает проверка.
        platform.as_ref().map(|c| c.exe.as_path()).map_err(String::as_str),
        ops_host::mcp_exe(kit).exists(),
        // Срок спрашиваем у часов, а не у запомненного при запуске: окно может
        // простоять открытым через полночь срока, а подкоманда `mcp` — быть
        // поднятой годами назад Zed, который её никогда не перезапускает.
        &crate::licence::state_now(),
    )
}

/// Отказ набранной команде, которой замок запрещает обслуживать: слова, и только слова.
///
/// **На диск их кладёт не она, и это куплено живым прогоном 25.11 (Linux, один `serve` на пустом журнале).** Прежде эта функция
/// писала в журнал сама и возвращала те же слова ошибкой. Пока запись была ОДНА, это работало; с 24.11 отказ старта кладёт на диск
/// тот, кто его доносит (`journal::record_stop` — он один видит все четыре дороги и знает рамку), и на одно событие стало две
/// строки разного начертания: своя, без рамки, и его, с «Остановлено:». Ровно то, что запрещает `gui::say_aloud`.
///
/// Свели на ДОНОСЯЩЕМ, а не на производителях: производителей отказа много — замок условий (эта функция), замок единственного
/// экземпляра, ключ узла, файл параметров, значок, оконная система, — и писали из них двое. Запись у производителя неполна по
/// устройству: каждый новый отказ пришлось бы не забыть снабдить ею. Запись у доносящего полна по устройству: дорог четыре, и все
/// они проходят через него. Поэтому дело этой функции — сказать, ЧТО не так, одними словами на все пути; что с ними делать,
/// решает дорога.
///
/// **Одна мера на все пути.** Обслуживание по набранной команде поднимают три разных куска кода — `gui::launch` в сборке с окном,
/// `main.rs::serve` в сборке без него и `main.rs::connect` в обеих (третий пришёл с ревью всей работы, Minor 7), — но отказ у них
/// один, потому что он здесь. Двух формулировок одной причины у этого модуля не бывает нарочно: он затем и не за фичей `gui`
/// (см. заголовок), чтобы вердикт и слова были одни на всех потребителей.
///
/// **Почему отказ, а не молчаливая жизнь.** Замок запрещён-но-работающий значит, что агенту всё равно ничего не достанется — нет
/// платформы 1С, базы, mcp-1c или кончился срок. Процесс, который в таком виде стартует, обещает порты, держит их и замок, а
/// причины не говорит нигде — тот самый молчаливый успех, за который эта задача и переписывалась дважды. Окно живёт при запертом
/// замке законно: там человек видит серую точку, запертый переключатель и причину в подсказке, и может её исправить тут же.
/// Набранной команде показать это некому.
/// Комплекта она больше не просит, и это не причёсывание подписи: пока он был в параметрах, писать на диск отсюда было ЧЕМ, и
/// следующий автор написал бы — тем же доводом «след обязан остаться», которым писали мы. Нет комплекта — нет и второй записи; то
/// же средство, которым `peer::start` закрыл отказ ключа (там ключ ТРЕБУЕТСЯ подписью, здесь диск ею недостижим).
pub fn refuse_to_serve(why: &str) -> anyhow::Error {
    let text = format!(
        "{} включить нельзя: {why}. Обслуживать вызовы агента этой машине нечем, и держать замок без обслуживания набранная команда \
         не будет — она уходит. Устраните причину и запустите её снова.",
        crate::SUBJECT
    );
    anyhow::anyhow!("{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done() -> OpState {
        OpState::Done("vrunner 3.0.2".into())
    }

    /// Отказ набранной команде — одна мера: причина названа, слова наружу и слова в журнале одни и те же.
    ///
    /// Сверяется именно совпадение двух мест: напиши кто-нибудь в журнал свой текст вместо общего — и человек, читающий консоль, и
    /// человек, читающий `1c-connect.log`, разошлись бы в понимании одного и того же отказа.
    #[test]
    fn a_locked_gate_refuses_a_typed_serve_in_words_and_writes_nothing_itself() {
        let dir = crate::project::tempdir::Dir::new();
        let kit = Kit::at(dir.path());
        let outside = format!("{:#}", refuse_to_serve("в комплекте нет mcp-1c"));
        assert!(outside.contains("в комплекте нет mcp-1c"), "причина названа: {outside}");
        assert!(outside.contains("запустите её снова"), "и сказано, что делать: {outside}");
        // **Прежде здесь стояло обратное утверждение — «отказ ложится в журнал», — и оно было верным до 24.11.** С тех пор на диск
        // кладёт доносящий (`journal::record_stop`), и запись здесь стала ВТОРОЙ: живой прогон 25.11 на Linux показал на одно
        // событие две строки разного начертания. Утверждение переписано вместе с доводом (докстрока выше), а обещание «в журнале
        // те же слова, а не вторая формулировка» никуда не делось — его держит рамка, одна на запись и на печать, и считает
        // `journal::tests::one_startup_refusal_leaves_exactly_one_line`.
        assert!(
            !kit.state_path(crate::journal::LOG_FILE).exists(),
            "производитель отказа на диск не пишет — иначе у доносящего получится вторая строка"
        );
    }

    /// Всё на месте — туннельное рабочее место по умолчанию.
    fn open(cfg: &Cfg) -> Option<String> {
        cold_gate(cfg, true, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid)
    }

    #[test]
    fn everything_in_place_opens_the_switch() {
        assert_eq!(open(&crate::cfg::filled_for_test()), None);
        let mut local = crate::cfg::filled_for_test();
        local.current_mut().mode = "local".into();
        assert_eq!(open(&local), None);
    }

    #[test]
    fn an_expired_deadline_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Expired)
            .unwrap();
        assert!(why.contains("срок действия") && why.contains(&licence::until_text()), "{why}");
        // Предупреждение заранее — не замок: работа идёт.
        assert_eq!(
            cold_gate(
                &crate::cfg::filled_for_test(),
                true,
                true,
                &done(),
                Ok(Path::new("1cv8.exe")),
                true,
                &licence::State::Soon { days: 3 }
            ),
            None
        );
    }

    /// Разбор живого прогона 9.11: на свежем комплекте, пока «Связь» пуста,
    /// замок отвечал «нет файла параметров — заполните поля и нажмите
    /// „Сохранить“» — звал туда, куда с первого экрана не попасть: ни полей,
    /// ни кнопки «Сохранить» онбординг не показывает (`panel::link_body`),
    /// он показывает факт и два ответа.
    ///
    /// Important 1 круга починки 1: проверка одного лишь ОТСУТСТВИЯ старых
    /// слов не упала бы, если фраза съедет в третьи, никем не одобренные
    /// слова, — сравниваем с самой фразой, которую видел владелец и не
    /// оспорил.
    #[test]
    fn the_first_screen_does_not_send_the_person_after_fields_it_does_not_show() {
        let mut c = crate::cfg::filled_for_test();
        c.connections.clear();
        let why = cold_gate(&c, false, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid)
            .expect("пустая «Связь» обязана запереть переключатель");
        assert_eq!(why, "соединений нет — создайте первое");
    }

    #[test]
    fn a_missing_params_file_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), false, true, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid)
            .unwrap();
        assert!(why.contains("параметров"), "{why}");
    }

    #[test]
    fn webserver_without_an_address_locks_the_switch() {
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().publication = "webserver".into();
        c.current_mut().base_url = String::new();
        assert_eq!(open(&c).as_deref(), Some("не указан адрес публикации"));
        c.current_mut().base_url = "http://localhost/Buh/hs/mcp-1c".into();
        assert_eq!(open(&c), None);
        // В обычном режиме адрес считается сам — замку там не за что цепляться.
        c.current_mut().publication = "auto".into();
        c.current_mut().base_url = String::new();
        assert_eq!(open(&c), None);
    }

    /// Находка 2 ревью пути 10.11: условия базы спрашивались у ЛЮБОЙ карточки, а той, что зовёт чужую базу, окно показывает одно поле
    /// «Папка документов» и подпись «Эти поля заполняют на компьютере с базой». Значит замок требовал заполнить то, чего человеку не
    /// показывают, — и обслуживание у разработчика (истории 2, 3, 4 путей человека) не включалось никогда. Ни один тест этого не видел:
    /// образец `filled_for_test` заполнен весь, а пустыми эти поля бывают именно у такой карточки.
    #[test]
    fn a_card_calling_a_base_elsewhere_is_not_asked_for_one() {
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().base_here = false;
        c.current_mut().base_path = String::new();
        c.current_mut().db_user = String::new();
        assert_eq!(open(&c), None, "базу и её пользователя спрашиваем только у того, у кого база есть");
        // Та же карточка с базой ЗДЕСЬ — замок заперт, и первым называет базу.
        c.current_mut().base_here = true;
        assert_eq!(open(&c).as_deref(), Some("не указана база"));
    }

    /// Critical 1 ревью всей ветки «Карточка публикации и удаление»: база на
    /// ТОЙ стороне — своей публикации у карточки нет вовсе, и замку нечего
    /// требовать. До правки он запирался словами «не задан адрес публикации
    /// — впишите его», а вписать было некуда: при базе на той стороне ни
    /// поля адреса, ни переключателя публикации карточка не показывает
    /// (`panel::shown_fields`, `panel::publication_switch_label`), а подменю
    /// значка, бывшее единственным выходом, эта ветка убрала.
    ///
    /// Записанное `publication = "webserver"` в этом состоянии оставлено
    /// НАРОЧНО: оно ответ человека, и правка его ради удобства замка —
    /// правка чужих данных (решение ведущего).
    #[test]
    fn a_base_on_the_other_side_has_no_publication_of_its_own() {
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().publication = "webserver".into();
        c.current_mut().base_url = String::new();
        // Та же карточка, пока база здесь, — замок заперт (сосед выше).
        assert!(open(&c).is_some());
        c.current_mut().base_here = false;
        assert_eq!(open(&c), None, "база на той стороне — адрес публикации спрашивать не у кого");
        assert!(!c.publication_address_missing(), "и само состояние «адрес не задан» здесь не наступает");
    }

    #[test]
    fn an_empty_db_user_holds_the_switch() {
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().db_user = "   ".into();
        let why = open(&c).unwrap();
        assert!(why.contains("пользователь базы"), "{why}");
    }

    /// Important 2 ревью задачи 7: до этой правки `cold_gate` ничего не знал про `undecryptable` вовсе — переключатель включался, мост ехал с пустым паролем базы, «Установить расширение» — тоже. `db_password` запирает замок в любом режиме.
    #[test]
    fn an_unreadable_database_password_holds_the_switch() {
        let mut c = crate::cfg::filled_for_test();
        c.undecryptable = vec![crate::cfg::Undecryptable::new("Buh", "db_password")];
        let why = open(&c).unwrap();
        assert!(why.contains("не расшифрован"), "{why}");
    }

    /// Пароль рабочего места запирает замок в туннельном режиме — там он нужен для туннеля в среду.
    #[test]
    fn an_unreadable_workstation_password_holds_the_switch_in_tunnel_mode() {
        let mut c = crate::cfg::filled_for_test();
        c.undecryptable = vec![crate::cfg::Undecryptable::new("Buh", "password")];
        let why = open(&c).unwrap();
        assert!(why.contains("не расшифрован"), "{why}");
    }

    /// Круг починки 1/5 задачи 10, находка 3: непрочитанный пароль ЧУЖОЙ карточки запирал переключатель при исправной текущей, и выхода из окна не было — поля чужой карточки окно не показывает вовсе. Мост и команды работают с `current()`, чужой пароль не читает никто. Переключатся на ту карточку — откажет её собственная находка (вторая половина теста).
    #[test]
    fn an_unreadable_password_of_another_card_does_not_hold_the_switch() {
        let mut c = crate::cfg::cfg_with(&["Buh", "Ромашка"]);
        c.undecryptable = vec![crate::cfg::Undecryptable::new("Ромашка", "db_password")];
        assert_eq!(open(&c), None, "чужая беда не отбирает работу у исправной текущей карточки");

        c.current = "Ромашка".into();
        let why = open(&c).unwrap();
        assert!(why.contains("не расшифрован"), "а своя — отбирает: {why}");
    }

    #[test]
    fn a_file_base_without_its_directory_holds_the_switch() {
        let c = crate::cfg::filled_for_test();
        let why = cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
        assert!(why.contains(&c.current().base_path), "{why}");
    }

    #[test]
    fn a_server_base_does_not_hold_the_switch() {
        // `сервер\база` на диске не лежит — его существование не проверяется
        // вовсе, иначе переключатель был бы заперт навсегда.
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().base_path = "srv-1c\\buh".into();
        assert_eq!(cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid), None);
        // Сетевой путь — предупреждение проверки, но не замок.
        c.current_mut().base_path = "\\\\nas\\bases\\buh".into();
        assert_eq!(cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid), None);
    }

    /// Пустое поле объясняется словами, а не пустой строкой в середине фразы.
    #[test]
    fn an_empty_base_says_so_instead_of_naming_an_empty_directory() {
        let mut c = crate::cfg::filled_for_test();
        c.current_mut().base_path = "  ".into();
        assert_eq!(open(&c).as_deref(), Some("не указана база"));
    }

    /// Кого спрашивать у диска: только файловую базу. Правило сверяется с самим фактом проверки — разойтись им нечем, иначе сетевая база повисла бы на таймауте SMB прямо на главном потоке окна.
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
        let why =
            cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Err("платформа 1С не найдена"), true, &licence::State::Valid)
                .unwrap();
        assert!(why.contains("платформа"), "{why}");
        // Причина приходит из лестницы как есть: невыполнимый пин запирает
        // переключатель своими словами, а не общим «не найдена».
        let pin = cold_gate(
            &crate::cfg::filled_for_test(),
            true,
            true,
            &done(),
            Err("версия 8.2.19 (поле platform в 1c-connect.toml) на машине не установлена; установлено: 8.3.27.1508"),
            true,
            &licence::State::Valid,
        )
        .unwrap();
        assert!(pin.contains("8.2.19") && pin.contains("не установлена"), "{pin}");
    }

    #[test]
    fn a_kit_without_mcp_holds_the_switch() {
        let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), false, &licence::State::Valid)
            .unwrap();
        assert!(why.contains("mcp-1c"), "{why}");
    }

    /// Поручение владельца 30.10: `mcp-1c` переехал в свою папку комплекта
    /// (`mcp-1c/`), как `vrunner/`, `git/` и `httpd/`. Замок обязан видеть
    /// бинарь на новом месте (комплект полон) и не видеть на старом — файл в
    /// корне, где он лежал раньше, больше не считается. Проверяется не
    /// булев параметр `cold_gate` (его уже проверяют соседние тесты), а
    /// настоящий `ops_host::mcp_exe` на настоящем диске — ровно то место,
    /// которое `gate_now` зовёт за этим булем.
    #[test]
    fn mcp_in_kit_sees_the_binary_in_its_own_folder_not_in_the_root() {
        let dir = std::env::temp_dir().join(format!("onec-gate-mcp-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let kit = Kit::at(&dir);
        let exe = ops_host::mcp_exe(&kit);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();

        // Старое место — россыпью в корне комплекта: замок не должен его видеть.
        std::fs::write(dir.join(exe.file_name().unwrap()), b"").unwrap();
        assert!(!exe.exists(), "{}", exe.display());
        let why =
            cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), exe.exists(), &licence::State::Valid)
                .unwrap();
        assert!(why.contains("mcp-1c"), "{why}");

        // Новое место — своя папка: замок должен его видеть и не запирать по mcp-1c.
        std::fs::write(&exe, b"").unwrap();
        assert!(exe.exists(), "{}", exe.display());
        assert_eq!(
            cold_gate(&crate::cfg::filled_for_test(), true, true, &done(), Ok(Path::new("1cv8.exe")), exe.exists(), &licence::State::Valid),
            None,
            "полный комплект не должен говорить «в комплекте нет mcp-1c»"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_incomplete_kit_holds_the_switch() {
        // `Missing` и `Drifted` `tools_state` больше не возвращает — копии
        // инструментов на машине нет, сверять две версии нечем (леджер
        // комплекта, пункт 15). Но замок обязан запираться на ЛЮБОМ не-`Done`,
        // а не на перечне известных исходов: это его работа, а не догадка о
        // том, какие состояния бывают.
        for st in [OpState::Missing, OpState::Drifted { from: "3.0.1".into(), to: "3.0.2".into() }, OpState::Blocked("нет vrunner/".into())]
        {
            let why = cold_gate(&crate::cfg::filled_for_test(), true, true, &st, Ok(Path::new("1cv8.exe")), true, &licence::State::Valid)
                .unwrap();
            assert!(why.contains("инструмент") && why.contains("комплект"), "{st:?}: {why}");
        }
    }

    /// Решение владельца 29.10: отсутствие записей MCP у редактора — факт готовности (`verify::editor_mcp_fact`), и он горячий — красит точку жёлтым, но замок его не видит вовсе (сигнатура `cold_gate` не несёт ничего про записи редактора). Запри его этим фактом, и человек не смог бы включиться, чтобы починить (образ, запись «Настроить»).
    #[test]
    fn a_missing_editor_mcp_record_does_not_hold_the_switch() {
        assert_eq!(open(&crate::cfg::filled_for_test()), None, "замок открыт, даже когда факт ниже окажется жёлтым");
        assert!(!crate::verify::editor_mcp_fact().ok, "а факт при этом красный");
    }

    /// `gate_now` — то же самое, что звало бы окно: пустой каталог состояния (ни файла параметров, ни mcp-1c в комплекте) обязан запереть замок, а не паниковать и не возвращать «можно».
    #[test]
    fn gate_now_locks_an_empty_kit() {
        let dir = std::env::temp_dir().join(format!("onec-gate-now-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kit = Kit::at(&dir);
        let why = gate_now(&kit, &crate::cfg::filled_for_test()).expect("пустой каталог обязан запереть замок");
        assert!(why.contains("параметров") || why.contains("mcp-1c"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
