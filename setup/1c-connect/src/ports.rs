//! Подбор портов при занятости — вместо того, чтобы молча ВЫВОДИТЬ их из
//! имени рабочего места (`cfg::derived_http_port`, `cfg::Cfg::ports`). На
//! пустом имени вывод отдаёт один и тот же номер каждому, кто его не
//! заполнил, — а бухгалтеры на терминальном сервере сидят каждая в своём
//! сеансе под своим профилем: вторая получает тот же порт, что первая, и её
//! httpd не стартует вовсе (план «тонкий срез ценности», задача 1).
//!
//! `derived_http_port` и сёстры по `cfg.rs` не исчезают — они остаются
//! умолчанием ПЕРВОЙ попытки, с которого подбор начинается. Выбранное в этом
//! сеансе (`Chosen`) пишется в каталог состояния отдельным файлом, не в
//! `1c-connect.toml`: файл параметров принадлежит человеку, программа его не
//! переписывает (`never-overwrite-user-config`).
//!
//! «Имя рабочего места» здесь — умолчание ПИЛОТА: `host.workstation`
//! распознаёт ровно `dev1`-`dev3` (`cfg::number_of`), и контракт подключения
//! это поле уже вычёркивает (`docs/superpowers/specs/2026-11-01-connection-contract.md`,
//! `-tabs-final.md` — `server`/`workstation`/`password` заменяются кодом
//! подключения). Само поле уедет; подбор при занятости — нет: это общий
//! механизм, и ему всё равно, откуда взялось предпочтение первой попытки.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

/// Четыре порта, которые коннектор выбрал в этом сеансе: та же четвёрка, что
/// раньше считала одна формула (`cfg::Ports` + `derived_http_port`), но
/// теперь не выведена, а нащупана и запомнена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub mcp: u16,
    pub vrunner: u16,
    pub web: u16,
    pub http: u16,
}

/// Имя файла в каталоге состояния — свой, не толкается с `install.state`
/// (`Kit::state_get`/`state_set`) и не с `skill-stamps` (`skills::state`).
const FILE: &str = "ports.state";

/// Сколько портов подряд пробовать от умолчания первой попытки — с запасом
/// на любое число рабочих мест на одном терминальном сервере, не только на
/// прибитые `dev1`-`dev3`.
const RANGE: u16 = 50;

/// Первый свободный порт из `count` подряд, начиная с `first`. Проба
/// занятости — параметром: тест не ходит в сеть, а прод-код (`port_is_busy`
/// ниже) слушает настоящий `127.0.0.1`.
///
/// Отказ — словами, с названными границами диапазона: молчаливый откат к
/// нулевому порту или к чужому запрещён (решение владельца, план «тонкий
/// срез ценности»).
pub fn pick(first: u16, count: u16, busy: &dyn Fn(u16) -> bool) -> Result<u16, String> {
    let last = first.saturating_add(count.saturating_sub(1));
    (first..=last).find(|p| !busy(*p)).ok_or_else(|| format!("свободного порта нет в диапазоне {first}-{last}"))
}

/// Настоящая проба занятости: попытка слушать на loopback. Удалось — порт
/// свободен, значит «занят» здесь — отрицание успеха попытки.
fn port_is_busy(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn file_in(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

/// Запомненное в прошлый раз в этом каталоге состояния: и то, что выбрали
/// (`chosen`), и предпочтение, которое этот выбор породило (`preference`,
/// `Cfg::first_attempt_ports()` того сеанса). Второе — круг исправления 3,
/// находка ведущего: без него запомненный выбор перебивал бы ЛЮБОЕ новое
/// предпочтение человека навсегда (явный `host.http_port` не действовал бы
/// никогда, переименование рабочего места не двигало бы `mcp`/`vrunner`/
/// `web`) — вход был замкнут сам на себя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Remembered {
    preference: Chosen,
    chosen: Chosen,
}

/// Запомненное в прошлый раз в этом каталоге состояния — или `None`, если
/// файла нет или он не разбирается. Битый или неполный файл — не отказ, а
/// повод выбрать заново (та же терпимость, что у чтения прочих файлов
/// состояния в этом крейте): следующий `choose` его перезапишет честным.
fn read(state_dir: &Path) -> Option<Remembered> {
    let text = std::fs::read_to_string(file_in(state_dir)).ok()?;
    let (mut pref_mcp, mut pref_vrunner, mut pref_web, mut pref_http) = (None, None, None, None);
    let (mut mcp, mut vrunner, mut web, mut http) = (None, None, None, None);
    for line in text.lines() {
        let (key, val) = line.split_once('=')?;
        let n: u16 = val.trim().parse().ok()?;
        match key.trim() {
            "pref_mcp" => pref_mcp = Some(n),
            "pref_vrunner" => pref_vrunner = Some(n),
            "pref_web" => pref_web = Some(n),
            "pref_http" => pref_http = Some(n),
            "mcp" => mcp = Some(n),
            "vrunner" => vrunner = Some(n),
            "web" => web = Some(n),
            "http" => http = Some(n),
            _ => {}
        }
    }
    Some(Remembered {
        preference: Chosen { mcp: pref_mcp?, vrunner: pref_vrunner?, web: pref_web?, http: pref_http? },
        chosen: Chosen { mcp: mcp?, vrunner: vrunner?, web: web?, http: http? },
    })
}

/// Пишет предпочтение и выбранное в каталог состояния — `pub(crate)`, не
/// только для `choose` ниже: тесту в `publication.rs` нужно посадить в
/// каталог «выбор другого процесса», прежде чем проверять, что дескриптор
/// несёт его, а не выведенный порт.
pub(crate) fn write(state_dir: &Path, preference: &Chosen, chosen: &Chosen) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|e| format!("не создать {}: {e}", state_dir.display()))?;
    let text = format!(
        "pref_mcp={}\npref_vrunner={}\npref_web={}\npref_http={}\nmcp={}\nvrunner={}\nweb={}\nhttp={}\n",
        preference.mcp, preference.vrunner, preference.web, preference.http, chosen.mcp, chosen.vrunner, chosen.web, chosen.http
    );
    let path = file_in(state_dir);
    std::fs::write(&path, text).map_err(|e| format!("не записать {}: {e}", path.display()))
}

/// Выбор на этот сеанс: то, что уже лежит в каталоге состояния, — как есть,
/// без повторной пробы, но ТОЛЬКО если запомненное предпочтение совпадает с
/// нынешним `first` (круг исправления 3). Повторная проба на НЕИЗМЕНИВШЕМСЯ
/// предпочтении была бы неверна: собственный httpd, уже слушающий выбранный
/// порт, читался бы на втором вызове как «занят» и уезжал на соседний — порт
/// дёргался бы при каждом новом обращении в одном и том же сеансе
/// (`publication::Paths::of` зовётся не один раз: установка, проверка
/// дескриптора, остановка). Но запомненное предпочтение разошлось с
/// нынешним (человек задал явный `host.http_port`, переименовал рабочее
/// место) — выбор не вечен, подбор идёт заново от нового `first`, и файл
/// перезаписывается новой парой. Ничего не лежит вовсе — тот же подбор от
/// `first` (умолчание первой попытки — `cfg::derived_http_port` и сёстры).
pub fn choose(state_dir: &Path, first: Chosen) -> Result<Chosen, String> {
    if let Some(remembered) = read(state_dir) {
        if remembered.preference == first {
            return Ok(remembered.chosen);
        }
    }
    let chosen = Chosen {
        mcp: pick(first.mcp, RANGE, &port_is_busy)?,
        vrunner: pick(first.vrunner, RANGE, &port_is_busy)?,
        web: pick(first.web, RANGE, &port_is_busy)?,
        http: pick(first.http, RANGE, &port_is_busy)?,
    };
    write(state_dir, &first, &chosen)?;
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dev1c-ports-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Пример брифа буквально: первый свободный, первый свободный ПОСЛЕ
    /// занятых, и словесный отказ с границами диапазона, когда свободного
    /// нет вовсе.
    #[test]
    fn pick_takes_the_first_free_and_refuses_in_words_when_the_range_is_full() {
        let busy_none = |_p: u16| false;
        assert_eq!(pick(7770, 10, &busy_none), Ok(7770));
        let busy_two = |p: u16| p < 7772;
        assert_eq!(pick(7770, 10, &busy_two), Ok(7772));
        let all_busy = |_p: u16| true;
        let err = pick(7770, 10, &all_busy).unwrap_err();
        assert!(err.contains("7770") && err.contains("7779"), "{err}");
    }

    /// Высокие, явно нигде в проекте не используемые номера — не
    /// `cfg::filled_for_test()`-овские 7771/7781/7791/8081: та четвёрка на
    /// машине разработчика может держать настоящий, живой коннектор
    /// (замер этой задачи: `choose` на ней ловил чужое «занято» и уезжал на
    /// соседний порт, хотя логика была верна — ловушка была в тесте, не в
    /// коде).
    fn sample() -> Chosen {
        Chosen { mcp: 47771, vrunner: 47781, web: 47791, http: 47801 }
    }

    /// `choose` на пустом каталоге состояния подбирает и запоминает —
    /// красное было: до `pick()` этот модуль не существовал вовсе.
    #[test]
    fn choose_picks_and_remembers_when_nothing_is_stored_yet() {
        let dir = tmp("choose-fresh");
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen, sample(), "диапазон свободен — первая попытка и есть выбор");
        assert_eq!(read(&dir), Some(Remembered { preference: sample(), chosen: sample() }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Второй вызов в том же каталоге с НЕИЗМЕНИВШИМСЯ предпочтением — тот
    /// же выбор, даже если первый порт успел стать занятым (например, наш
    /// собственный httpd уже на нём слушает): повторная проба увела бы выбор
    /// на соседний порт при каждом обращении в одном сеансе.
    #[test]
    fn choose_reuses_what_was_remembered_even_if_the_port_looks_busy_now() {
        let dir = tmp("choose-remembered");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy_port = listener.local_addr().unwrap().port();
        write(&dir, &sample(), &Chosen { mcp: 1, vrunner: 2, web: 3, http: busy_port }).unwrap();
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen.http, busy_port, "выбор из состояния не пересматривается на занятости");
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Круг исправления 3, находка ведущего: вход был замкнут сам на себя —
    /// `first_attempt_ports()` кормила `choose` уже улаженным портом, и
    /// запомненный выбор перебивал ЛЮБОЕ новое предпочтение навсегда. Теперь
    /// запомненное отдаётся как есть только при совпадении предпочтения;
    /// разошлось — подбор заново, и файл перезаписывается новой парой.
    #[test]
    fn a_changed_preference_is_not_overridden_by_a_stale_remembered_choice() {
        let dir = tmp("choose-preference-changed");
        // Человек когда-то получил этот выбор для СТАРОГО предпочтения.
        let old_preference = Chosen { mcp: 1, vrunner: 2, web: 3, http: 40000 };
        let stale_chosen = Chosen { mcp: 1, vrunner: 2, web: 3, http: 40000 };
        write(&dir, &old_preference, &stale_chosen).unwrap();
        // Предпочтение сменилось (например, человек задал явный
        // `host.http_port`) — подбор обязан пойти от НОВОГО `first`, не
        // вернуть устаревший выбор.
        let fresh = choose(&dir, sample()).unwrap();
        assert_eq!(fresh, sample(), "предпочтение разошлось — подбор заново от нового first");
        // Файл перезаписан новой парой: следующий вызов с ТЕМ ЖЕ новым
        // предпочтением снова получает его без повторной пробы.
        assert_eq!(read(&dir), Some(Remembered { preference: sample(), chosen: sample() }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Тот же механизм, узко на сёстрах (`mcp`/`vrunner`/`web`), не на
    /// `http`: сегодня их предпочтение двигает только смена `host.workstation`
    /// в `cfg.rs` (`Cfg::ports`/`Cfg::first_attempt_ports`) — пилотное поле,
    /// которое контракт подключения уже вычёркивает (см. докстроку модуля).
    /// Само поле уйдёт, а это утверждение — нет: любое изменение
    /// предпочтения, в любом из четырёх портов, обязано дойти до `pick`, а
    /// не упереться в запомненное для другого предпочтения.
    #[test]
    fn a_changed_sisters_preference_reaches_pick_not_the_stale_remembered_choice() {
        let dir = tmp("choose-sisters-preference");
        let old_preference = Chosen { mcp: 47041, vrunner: 47042, web: 47043, http: 47044 };
        write(&dir, &old_preference, &old_preference).unwrap();
        // `http` не изменился — разошлись только сёстры.
        let new_preference = Chosen { mcp: 47051, vrunner: 47052, web: 47053, http: 47044 };
        let chosen = choose(&dir, new_preference).unwrap();
        assert_eq!(chosen, new_preference, "предпочтение сестёр сменилось — подбор обязан пойти от нового, а не вернуть старое для http же");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Утверждение владельца (вопрос «а порт всегда будет случайный
    /// свободный?»): порт обязан быть ПРЕДСКАЗУЕМЫМ. При свободном прошлом
    /// порте он и выбирается, даже если предпочтительный (умолчание первой
    /// попытки) тоже свободен — иначе у второй бухгалтерки порт прыгал бы
    /// при каждом перезапуске коннектора в зависимости от того, кто первой
    /// встала, а по номеру порта человек или админ заводит правило в
    /// средствах защиты: прыгающий номер это правило обнуляет.
    #[test]
    fn a_free_remembered_port_wins_over_an_also_free_preferred_one() {
        let dir = tmp("choose-stability");
        let remembered = Chosen { mcp: 47011, vrunner: 47012, web: 47013, http: 47014 };
        // Предпочтение этого прошлого выбора — то же самое `sample()`, что
        // подаётся ниже: вопрос теста не «предпочтение сменилось», а
        // «предпочтение НЕ сменилось, а запомненный порт просто свободен».
        write(&dir, &sample(), &remembered).unwrap();
        // `sample()` — тот же `first`, что и породил `remembered`: он тоже
        // свободен, и всё равно не должен победить прошлый выбор.
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen, remembered, "предпочтительный свободен — и всё равно не должен победить прошлый выбор");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Битый файл состояния — не отказ `choose`, а повод выбрать заново. Все
    /// четыре ключа на месте (иначе падение было бы и так, по нехватке
    /// поля) — битый ровно один, числом не разбирается.
    #[test]
    fn a_broken_state_file_is_not_a_refusal_it_is_recomputed() {
        let dir = tmp("choose-broken");
        std::fs::create_dir_all(&dir).unwrap();
        // Все восемь ключей на месте (иначе падение было бы и так, по
        // нехватке поля) — битый ровно один, числом не разбирается.
        std::fs::write(
            file_in(&dir),
            "pref_mcp=не число\npref_vrunner=2\npref_web=3\npref_http=4\nmcp=1\nvrunner=2\nweb=3\nhttp=4\n",
        )
        .unwrap();
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen, sample());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
