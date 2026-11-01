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

/// Выбранное в прошлый раз в этом каталоге состояния — или `None`, если
/// файла нет или он не разбирается. Битый или неполный файл — не отказ, а
/// повод выбрать заново (та же терпимость, что у чтения прочих файлов
/// состояния в этом крейте): следующий `choose` его перезапишет честным.
fn read(state_dir: &Path) -> Option<Chosen> {
    let text = std::fs::read_to_string(file_in(state_dir)).ok()?;
    let mut mcp = None;
    let mut vrunner = None;
    let mut web = None;
    let mut http = None;
    for line in text.lines() {
        let (key, val) = line.split_once('=')?;
        let n: u16 = val.trim().parse().ok()?;
        match key.trim() {
            "mcp" => mcp = Some(n),
            "vrunner" => vrunner = Some(n),
            "web" => web = Some(n),
            "http" => http = Some(n),
            _ => {}
        }
    }
    Some(Chosen { mcp: mcp?, vrunner: vrunner?, web: web?, http: http? })
}

/// Пишет выбранное в каталог состояния — `pub(crate)`, не только для
/// `choose` ниже: тесту в `publication.rs` нужно посадить в каталог
/// «выбор другого процесса», прежде чем проверять, что дескриптор несёт его,
/// а не выведенный порт.
pub(crate) fn write(state_dir: &Path, chosen: &Chosen) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|e| format!("не создать {}: {e}", state_dir.display()))?;
    let text = format!("mcp={}\nvrunner={}\nweb={}\nhttp={}\n", chosen.mcp, chosen.vrunner, chosen.web, chosen.http);
    let path = file_in(state_dir);
    std::fs::write(&path, text).map_err(|e| format!("не записать {}: {e}", path.display()))
}

/// Выбор на этот сеанс: то, что уже лежит в каталоге состояния, — как есть,
/// без повторной пробы. Повторная проба была бы неверна: собственный httpd,
/// уже слушающий выбранный порт, читался бы на втором вызове как «занят» и
/// уезжал на соседний — порт дёргался бы при каждом новом обращении в одном
/// и том же сеансе (`publication::Paths::of` зовётся не один раз: установка,
/// проверка дескриптора, остановка). Ничего не лежит — подбор от `first`
/// (умолчание первой попытки — `cfg::derived_http_port` и сёстры), и
/// результат сохраняется для всех, кто спросит после.
pub fn choose(state_dir: &Path, first: Chosen) -> Result<Chosen, String> {
    if let Some(remembered) = read(state_dir) {
        return Ok(remembered);
    }
    let chosen = Chosen {
        mcp: pick(first.mcp, RANGE, &port_is_busy)?,
        vrunner: pick(first.vrunner, RANGE, &port_is_busy)?,
        web: pick(first.web, RANGE, &port_is_busy)?,
        http: pick(first.http, RANGE, &port_is_busy)?,
    };
    write(state_dir, &chosen)?;
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
        assert_eq!(read(&dir), Some(sample()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Второй вызов в том же каталоге — тот же выбор, даже если первый порт
    /// успел стать занятым (например, наш собственный httpd уже на нём
    /// слушает): повторная проба увела бы выбор на соседний порт при каждом
    /// обращении в одном сеансе.
    #[test]
    fn choose_reuses_what_was_remembered_even_if_the_port_looks_busy_now() {
        let dir = tmp("choose-remembered");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy_port = listener.local_addr().unwrap().port();
        write(&dir, &Chosen { mcp: 1, vrunner: 2, web: 3, http: busy_port }).unwrap();
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen.http, busy_port, "выбор из состояния не пересматривается на занятости");
        drop(listener);
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
        write(&dir, &remembered).unwrap();
        // `sample()` — другая, тоже свободная четвёрка: предпочтительный
        // порт этого вызова никем не занят, и всё равно не должен победить.
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
        std::fs::write(file_in(&dir), "mcp=не число\nvrunner=2\nweb=3\nhttp=4\n").unwrap();
        let chosen = choose(&dir, sample()).unwrap();
        assert_eq!(chosen, sample());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
