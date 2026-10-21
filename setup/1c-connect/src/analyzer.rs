//! Строки платформы для `bsl-analyzer` — файл `bsl-analyzer.toml` в корне
//! проекта. Спека, раздел «Строка платформы для анализатора: её пишет
//! коннектор».
//!
//! Ключ пишем один, и разобраться стоит с двумя — они делают разное
//! (замеры на нашей 0.3.2 в живом контейнере):
//!
//! * `min_platform_version` — **не пишем, и это решение заказчика
//!   2026-10-21.** Ключ работает: пока его нет, диагностика
//!   `PlatformMemberNewerThanMinVersion` молчит полностью, а с ним называет
//!   использование элементов платформы новее минимума (замер 2026-10-21:
//!   `min = 8.3.1` на `Асинх`/`СтрРазделить` даёт диагностику, `min =
//!   8.3.27` — тишину). Но полезен он только когда там стоит **самая
//!   старая платформа, на которой код обязан работать** — продуктивная
//!   версия заказчика, а её на машине нет ни в каком виде.
//!
//!   Версия, которую находит `find_1cv8`, — это платформа машины, где
//!   работает коннектор: в образе стенд, у разработчика самая новая из
//!   установленных (их может быть несколько). Записать её как минимум
//!   значило бы выдумать число и ошибиться в обе стороны: стенд новее
//!   прода — анализатор промолчит про то, чего у заказчика нет (то самое
//!   тихое выключение проверки, против которого вся задача); стенд
//!   старее — заругается на верный код. Поэтому ключа нет вовсе, а в
//!   файле рядом стоит закомментированный пример: ручка видна, числа мы
//!   не сочиняем.
//! * `target_platform_version` — наоборот, верных проверок не добавляет
//!   (фактов для чужой линии в каталоге анализатора просто нет), он умеет
//!   только **погасить ложные срабатывания**. Поэтому пишем ровно в одном
//!   случае — платформа клиента новее каталога: тогда имена из нового
//!   выпуска приходят как unresolved, и агент начинает «исправлять» верный
//!   код. Равная линия — ключ лишний; старее — ложных «нет такого имени» и
//!   без ключа не будет (старый API — подмножество нового).
//!
//! `compatibility_mode` не пишем: анализатор берёт режим совместимости из
//! `<CompatibilityMode>` самой выгрузки, и наша строка там была бы третьим
//! мнением о том, что уже написано в данных.
//!
//! Ключи корневые, без раздела, и имена — из списка, который 0.3.2
//! принимает: неизвестное поле она отвергает ошибкой разбора, а сломанный
//! `bsl-analyzer.toml` на JSON не откатывается.
//!
//! Линию каталога не угадываем: её называет сам анализатор в манифесте
//! собранного корпуса. Манифеста нет (корпус строится при первом обращении,
//! а файл нужен раньше — при раскладке проекта; в туннельном режиме кэш
//! анализатора лежит в контейнере и коннектору не виден вовсе) — в силе
//! встроенная линия анализатора.

use std::path::{Path, PathBuf};

/// Имя файла в корне проекта. В `.gitignore` не идёт: он нужен второй
/// стороне — анализатор читает его там, где лежит выгрузка.
pub const FILE: &str = "bsl-analyzer.toml";

/// Встроенная линия фактов `bsl-analyzer` 0.3.2 в нашем образе — замер
/// 2026-10-21 (её же WARN при деградации называет `8.3.27.2214`).
/// Сверить при обновлении анализатора; расхождение видит `verify::analyzer_fact`.
pub const BUNDLED: &str = "8.3.27";

pub const MIN: &str = "min_platform_version";
pub const TARGET: &str = "target_platform_version";

/// Линия — три компонента версии: `8.5.1.1150` → `8.5.1`. Анализатор
/// аттестован на линию, а сборку не учитывает вовсе.
pub fn line(version: &str) -> String {
    version.splitn(4, '.').take(3).collect::<Vec<_>>().join(".")
}

/// Покомпонентно числами: `8.3.9` старее `8.3.27`, хотя как строки — наоборот.
fn parts(line: &str) -> Vec<u32> {
    line.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// `~/.cache` по XDG, иначе `$HOME/.cache`. `None` — ни того, ни другого
/// (Windows: кэша анализатора на этой машине нет вовсе).
pub fn cache_dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".cache"))
}

/// Линия каталога анализатора: `platform-help/auto/current` называет хеш
/// активного снимка, `auto/<хеш>/manifest.json` — его `platform_version`.
/// Любая осечка (нет кэша, нет манифеста, он не разбирается) — встроенная
/// линия: она свойство нашей же сборки анализатора, а не догадка.
pub fn catalogue_line(cache: &Path) -> String {
    let auto = cache.join("bsl-analyzer").join("platform-help").join("auto");
    let read = || -> Option<String> {
        let hash = std::fs::read_to_string(auto.join("current")).ok()?;
        let text = std::fs::read_to_string(auto.join(hash.trim()).join("manifest.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let l = line(v.get("platform_version")?.as_str()?);
        (!parts(&l).is_empty()).then_some(l)
    };
    read().unwrap_or_else(|| BUNDLED.to_string())
}

/// Линия платформы этой машины — три компонента или `None`, если версия
/// не разобралась.
pub fn machine_line(client: &str) -> Option<String> {
    let l = line(client);
    (!parts(&l).is_empty()).then_some(l)
}

/// Что должно стоять в файле при этой платформе и этом каталоге. Ключ
/// один — `target_platform_version`, и только когда линия машины **новее**
/// каталога анализатора: иначе он не нужен (равная линия) или бесполезен
/// (старее — ложных «нет такого имени» не бывает). Пусто — файла не будет
/// вовсе: лживый toml хуже отсутствующего.
pub fn expected(client: &str, catalogue: &str) -> Vec<(&'static str, String)> {
    let Some(l) = machine_line(client) else { return Vec::new() };
    let (ours, theirs) = (parts(&l), parts(catalogue));
    if theirs.is_empty() || ours <= theirs {
        return Vec::new();
    }
    vec![(TARGET, l)]
}

/// Что в файле стоит сейчас — только наши ключи, значениями как написано.
/// Разбор один на всех: пишущий `ensure` и факт проверки смотрят одними
/// глазами. Файла нет — пусто.
///
/// Читаем **только корень файла, до первого `[раздела]`**: ключ внутри
/// раздела — не наш ключ, и анализатор его не видит (ревью задачи 36
/// проверило живьём: `[diagnostics] min_platform_version = …` проходит как
/// «Configuration is valid», потому что раздел глотает любые ключи). Читай
/// мы его как корневой — `ensure` отказался бы писать настоящий, и проверка
/// осталась бы выключенной молча.
pub fn found(project: &Path) -> Vec<(&'static str, String)> {
    let Ok(text) = std::fs::read_to_string(project.join(FILE)) else {
        return Vec::new();
    };
    let root: Vec<&str> = text.lines().map(str::trim).take_while(|t| !t.starts_with('[')).filter(|t| !t.starts_with('#')).collect();
    // Порядок — наш, а не файла: так списки `found` и `expected` сравнимы
    // целиком, а не по одному ключу.
    [MIN, TARGET]
        .into_iter()
        .filter_map(|key| {
            let value = root.iter().filter_map(|t| t.split_once('=')).find(|(k, _)| k.trim() == key)?.1;
            // Хвостовой комментарий в значение не идёт: `min = "8.5.1" # версия
            // заказчика` — верный toml, и прочитать его надо так же верно.
            let value = value.split('#').next().unwrap_or(value);
            Some((key, value.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
}

/// Зачем ключ нужен — рядом с ним же: человек читает файл, а не спеку.
fn why(_key: &str) -> &'static str {
    "# target_platform_version — линия платформы этой машины: она новее\n# каталога анализатора, и без этой строки он считает имена нового\n# выпуска неизвестными.\n"
}

/// Подсказка про ручку, которой программа не трогает. Идёт **только в новый
/// файл** и **только закомментированной**: ключа нет — значит анализатор
/// ведёт себя ровно так, как без файла, и никакого числа мы не выдумали. А
/// ручка при этом видна тому, кто файл откроет.
fn knob() -> String {
    format!(
        "\n# {MIN} — самая старая платформа, на которой код обязан работать\n\
         # (продуктивная версия заказчика). Коннектор её не знает и не\n\
         # подставляет: версия этой машины тут была бы выдумкой. Снимите\n\
         # комментарий и впишите свою — тогда bsl-analyzer начнёт называть\n\
         # элементы платформы, которых у заказчика нет.\n\
         # {MIN} = \"8.3.21\"\n"
    )
}

/// Кладёт недостающие ключи в `bsl-analyzer.toml` корня проекта и возвращает
/// их имена. Ключ, который в файле уже есть, не трогаем — его строка воля
/// человека, а расхождение покажет `verify::analyzer_fact`.
///
/// Чужой файл не перерисовываем — дописываем **в начало**, и только в
/// начало: ключи корневые, а в TOML корневые ключи обязаны стоять до первого
/// `[раздела]`. Дописывание в конец положило бы наш ключ внутрь чужого
/// раздела (`common::toml_render::set` для корня не годится именно этим).
pub fn ensure(project: &Path, want: &[(&'static str, String)]) -> Result<Vec<&'static str>, String> {
    let have = found(project);
    let add: Vec<_> = want.iter().filter(|(k, _)| !have.iter().any(|(got, _)| got == k)).collect();
    if add.is_empty() {
        return Ok(Vec::new());
    }
    // Объяснение к каждому ключу своё, и дописывается только с ним: общая
    // шапка «здесь версия этой машины» над чужой строкой человека была бы
    // ложью о его значении, а при второй дописи — ещё и повтором.
    let mut head = String::new();
    for (k, v) in &add {
        head.push_str(why(k));
        head.push_str(&format!("{k} = {v:?}\n"));
    }
    let path = project.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => format!("{head}\n{t}"),
        Err(_) => format!("{head}{}", knob()),
    };
    std::fs::write(&path, text).map_err(|e| format!("не удалось записать {}: {e}", path.display()))?;
    Ok(add.iter().map(|(k, _)| *k).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::tempdir::Dir;

    /// Минимум коннектор не пишет вовсе — только подсказкой в комментарии:
    /// версия этой машины (в образе — стенд, у разработчика — самая новая из
    /// установленных) не имеет отношения к продуктивной версии заказчика, а
    /// именно она и есть смысл ключа.
    #[test]
    fn the_minimum_is_never_written_only_hinted() {
        assert_eq!(expected("8.3.21.1234", "8.3.27"), Vec::new());
        assert_eq!(expected("8.3.27.1508", "8.3.27"), Vec::new());
        let d = Dir::new();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        // Ключа нет — значит анализатор ведёт себя как без файла; а ручка видна.
        assert_eq!(found(d.path()), vec![(TARGET, "8.5.1".to_string())]);
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(text.contains(&format!("# {MIN} = ")), "подсказка про ручку обязана быть:
{text}");
        assert!(!text.lines().any(|l| l.trim_start().starts_with(MIN)), "незакомментированного минимума быть не должно:
{text}");
    }

    #[test]
    fn only_a_newer_machine_gets_the_target_line() {
        assert_eq!(expected("8.5.1.1150", "8.3.27"), vec![(TARGET, "8.5.1".to_string())]);
        // Равная линия — ключ не нужен; старее — бесполезен.
        assert!(expected("8.3.27.1508", "8.3.27").is_empty());
        assert!(expected("8.3.21.1234", "8.3.27").is_empty());
    }

    #[test]
    fn an_unparsable_version_writes_nothing() {
        assert!(expected("", "8.3.27").is_empty());
        assert_eq!(machine_line(""), None);
        // Каталог не разобрался — сравнивать не с чем, и строки нет.
        assert!(expected("8.5.1.1150", "неизвестно").is_empty());
        assert_eq!(machine_line("8.5.1.1150").as_deref(), Some("8.5.1"));
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        assert_eq!(expected("8.3.27.1508", "8.3.9"), vec![(TARGET, "8.3.27".to_string())]);
        assert!(expected("8.3.9.1000", "8.3.27").is_empty());
    }

    fn cache_with(hash: &str, manifest: &str) -> Dir {
        let d = Dir::new();
        let auto = d.path().join("bsl-analyzer").join("platform-help").join("auto");
        std::fs::create_dir_all(auto.join(hash)).unwrap();
        std::fs::write(auto.join("current"), format!("{hash}\n")).unwrap();
        std::fs::write(auto.join(hash).join("manifest.json"), manifest).unwrap();
        d
    }

    #[test]
    fn the_manifest_names_the_catalogue_line() {
        let d = cache_with("abc123", r#"{"corpus_id":"installed-8.3.27.1508","platform_version":"8.3.27.1508"}"#);
        assert_eq!(catalogue_line(d.path()), "8.3.27");
    }

    #[test]
    fn without_a_manifest_the_bundled_line_stands() {
        let empty = Dir::new();
        assert_eq!(catalogue_line(empty.path()), BUNDLED);
        // Манифест есть, но мусор — тоже встроенная линия, без паники.
        assert_eq!(catalogue_line(cache_with("abc123", "{ not json").path()), BUNDLED);
        assert_eq!(catalogue_line(cache_with("abc123", r#"{"platform_version":""}"#).path()), BUNDLED);
    }

    #[test]
    fn a_fresh_file_carries_the_target_line() {
        let d = Dir::new();
        let want = expected("8.5.1.1150", "8.3.27");
        assert_eq!(ensure(d.path(), &want), Ok(vec![TARGET]));
        assert_eq!(found(d.path()), want);
        // Повторный вызов файл не меняет.
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert_eq!(ensure(d.path(), &want), Ok(Vec::new()));
        assert_eq!(std::fs::read_to_string(d.path().join(FILE)).unwrap(), text);
    }

    #[test]
    fn a_written_line_is_left_alone() {
        let d = Dir::new();
        let mine = "# моё
target_platform_version = \"8.3.21\"
";
        std::fs::write(d.path().join(FILE), mine).unwrap();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(Vec::new()));
        assert_eq!(std::fs::read_to_string(d.path().join(FILE)).unwrap(), mine);
    }

    /// Свой ключ человека цел, недостающий дописан, остальное не тронуто.
    #[test]
    fn an_existing_file_keeps_its_other_keys() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "# мой файл
min_platform_version = \"8.3.21\"

[diagnostics]
enabled = true
").unwrap();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(text.contains("# мой файл") && text.contains("[diagnostics]"), "{text}");
        // Корневой ключ — до первого раздела, иначе он попал бы в `[diagnostics]`.
        assert!(text.find(TARGET) < text.find("[diagnostics]"), "{text}");
        // Минимум человека цел и значением, и тем, что он остался его.
        assert_eq!(found(d.path()), vec![(MIN, "8.3.21".to_string()), (TARGET, "8.5.1".to_string())]);
    }

    /// Ключ внутри чужого раздела — не наш ключ: анализатор его не видит
    /// (раздел глотает любые ключи), и прочитать его как корневой значило бы
    /// отказаться писать настоящий, оставив проверку выключенной молча.
    #[test]
    fn a_key_inside_a_section_is_not_ours() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "[diagnostics]
target_platform_version = \"8.3.21\"
").unwrap();
        assert!(found(d.path()).is_empty());
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        assert_eq!(found(d.path()), vec![(TARGET, "8.5.1".to_string())]);
        // И наша строка легла до раздела, иначе её не увидел бы и анализатор.
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(text.find(TARGET) < text.find("[diagnostics]"), "{text}");
    }

    /// Хвостовой комментарий — верный toml, и в значение он не идёт: иначе
    /// факт проверки краснел бы на правильном файле.
    #[test]
    fn a_trailing_comment_is_not_part_of_the_value() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "min_platform_version = \"8.3.21\" # версия заказчика
").unwrap();
        assert_eq!(found(d.path()), vec![(MIN, "8.3.21".to_string())]);
    }

    /// Объяснение дописывается только к тому ключу, который дописали: своей
    /// подсказки про минимум в чужом файле быть не должно — она говорит
    /// «снимите комментарий и впишите свою», а строка там уже есть.
    #[test]
    fn the_explanation_belongs_to_the_key_it_explains() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "min_platform_version = \"8.2.19\"
").unwrap();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(text.contains(why(TARGET)), "объяснение своего ключа обязано быть:
{text}");
        // Сверяемся с самим текстом подсказки, а не с её пересказом: иначе
        // переформулировка выхолостила бы защиту (находка повторного ревью).
        assert!(!text.contains(knob().trim()), "подсказка про ручку поверх чужой строки лишняя:
{text}");
        assert_eq!(found(d.path()), vec![(MIN, "8.2.19".to_string()), (TARGET, "8.5.1".to_string())]);
    }

    /// Закомментированный ключ — не ключ: иначе наша же подсказка про
    /// минимум запретила бы запись (и пример в комментарии — тоже).
    #[test]
    fn a_commented_key_is_not_a_key() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "# target_platform_version = \"8.3.21\"
").unwrap();
        assert!(found(d.path()).is_empty());
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        assert_eq!(found(d.path()), vec![(TARGET, "8.5.1".to_string())]);
    }
}
