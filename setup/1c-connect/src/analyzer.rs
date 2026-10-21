//! Строки платформы для `bsl-analyzer` — файл `bsl-analyzer.toml` в корне
//! проекта. Спека, раздел «Строка платформы для анализатора: её пишет
//! коннектор».
//!
//! Пишем два ключа, и они делают разное (замер соседней сессии на нашей
//! 0.3.2, `check-config`):
//!
//! * `min_platform_version` — минимальная платформа, на которой код обязан
//!   компилироваться. Пока ключа нет, диагностика
//!   `PlatformMemberNewerThanMinVersion` **молчит полностью**; с ключом она
//!   называет каждое использование элемента платформы, которого на этой
//!   версии нет (замер 2026-10-21: `min = 8.3.1` на `Асинх`/`СтрРазделить`
//!   даёт диагностику, `min = 8.3.27` и отсутствие ключа — тишину).
//!
//!   Пишем версию этой машины, и это сознательно **безопасное, а не
//!   полезное** значение: польза у ключа появляется, когда там стоит самая
//!   старая платформа, на которой код обязан работать (платформа
//!   заказчика), а её коннектор знать не может — это решение человека.
//!   Наша версия ложных тревог не даёт (код, написанный на этой платформе,
//!   её же элементами и ограничен), а ключ в файле делает ручку заметной:
//!   комментарий рядом говорит, на что её менять.
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

/// Что должно стоять в файле при этой платформе и этом каталоге. Пусто —
/// писать нечего: версия не разобралась (лживый toml хуже отсутствующего)
/// или `target` не нужен, потому что клиент не новее каталога.
pub fn expected(client: &str, catalogue: &str) -> Vec<(&'static str, String)> {
    let l = line(client);
    let (ours, theirs) = (parts(&l), parts(catalogue));
    if ours.is_empty() {
        return Vec::new();
    }
    let mut v = vec![(MIN, l.clone())];
    if !theirs.is_empty() && ours > theirs {
        v.push((TARGET, l));
    }
    v
}

/// Что в файле стоит сейчас — только наши ключи, значениями как написано.
/// Разбор один на всех: пишущий `ensure` и факт проверки смотрят одними
/// глазами. Файла нет — пусто.
pub fn found(project: &Path) -> Vec<(&'static str, String)> {
    let Ok(text) = std::fs::read_to_string(project.join(FILE)) else {
        return Vec::new();
    };
    // Порядок — наш, а не файла: так списки `found` и `expected` сравнимы
    // целиком, а не по одному ключу.
    [MIN, TARGET]
        .into_iter()
        .filter_map(|key| {
            let value = text.lines().map(str::trim).filter(|t| !t.starts_with('#')).filter_map(|t| t.split_once('=')).find(|(k, _)| k.trim() == key)?.1;
            Some((key, value.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
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
    let path = project.join(FILE);
    let mut head = String::from(
        "# Записал 1c-connect: версия платформы на этой машине.\n\
         # min_platform_version — самая старая платформа, на которой код обязан\n\
         # работать. Здесь стоит версия этой машины: так проверка включена, но\n\
         # молчит. Поставьте версию заказчика, и bsl-analyzer начнёт называть\n\
         # элементы платформы, которых у него нет.\n",
    );
    for (k, v) in &add {
        head.push_str(&format!("{k} = {v:?}\n"));
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => format!("{head}\n{t}"),
        Err(_) => head,
    };
    std::fs::write(&path, text).map_err(|e| format!("не удалось записать {}: {e}", path.display()))?;
    Ok(add.iter().map(|(k, _)| *k).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimum_version_is_written_whenever_the_platform_is_known() {
        // Клиент не новее каталога — `target` не нужен, но `min` нужен всегда.
        assert_eq!(expected("8.3.21.1234", "8.3.27"), vec![(MIN, "8.3.21".to_string())]);
        assert_eq!(expected("8.3.27.1508", "8.3.27"), vec![(MIN, "8.3.27".to_string())]);
    }

    #[test]
    fn a_newer_client_also_gets_the_target_line() {
        assert_eq!(expected("8.5.1.1150", "8.3.27"), vec![(MIN, "8.5.1".to_string()), (TARGET, "8.5.1".to_string())]);
    }

    #[test]
    fn an_unparsable_version_writes_nothing() {
        assert!(expected("", "8.3.27").is_empty());
        // Каталог не разобрался — `min` всё равно пишем, `target` без
        // сравнения не выводится.
        assert_eq!(expected("8.5.1.1150", "неизвестно"), vec![(MIN, "8.5.1".to_string())]);
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        assert!(expected("8.3.27.1508", "8.3.9").iter().any(|(k, _)| *k == TARGET));
        assert!(!expected("8.3.9.1000", "8.3.27").iter().any(|(k, _)| *k == TARGET));
    }

    use crate::project::tempdir::Dir;

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
    fn a_fresh_file_carries_both_lines() {
        let d = Dir::new();
        let want = expected("8.5.1.1150", "8.3.27");
        assert_eq!(ensure(d.path(), &want), Ok(vec![MIN, TARGET]));
        assert_eq!(found(d.path()), want);
        // Повторный вызов файл не меняет.
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert_eq!(ensure(d.path(), &want), Ok(Vec::new()));
        assert_eq!(std::fs::read_to_string(d.path().join(FILE)).unwrap(), text);
    }

    #[test]
    fn a_written_line_is_left_alone() {
        let d = Dir::new();
        let mine = "# моё\nmin_platform_version = \"8.3.21\"\ntarget_platform_version = \"8.3.21\"\n";
        std::fs::write(d.path().join(FILE), mine).unwrap();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(Vec::new()));
        assert_eq!(std::fs::read_to_string(d.path().join(FILE)).unwrap(), mine);
    }

    /// Свой ключ человека цел, недостающий дописан, остальное не тронуто.
    #[test]
    fn an_existing_file_keeps_its_other_keys() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "# мой файл\nmin_platform_version = \"8.3.21\"\n\n[diagnostics]\nenabled = true\n").unwrap();
        assert_eq!(ensure(d.path(), &expected("8.5.1.1150", "8.3.27")), Ok(vec![TARGET]));
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(text.contains("# мой файл") && text.contains("[diagnostics]"), "{text}");
        // Корневой ключ — до первого раздела, иначе он попал бы в `[diagnostics]`.
        assert!(text.find(TARGET) < text.find("[diagnostics]"), "{text}");
        assert_eq!(found(d.path()), vec![(MIN, "8.3.21".to_string()), (TARGET, "8.5.1".to_string())]);
    }

    /// Закомментированный ключ — не ключ: иначе пример в комментарии
    /// запретил бы запись.
    #[test]
    fn a_commented_key_is_not_a_key() {
        let d = Dir::new();
        std::fs::write(d.path().join(FILE), "# min_platform_version = \"8.3.21\"\n").unwrap();
        assert!(found(d.path()).is_empty());
        assert_eq!(ensure(d.path(), &expected("8.3.27.1508", "8.3.27")), Ok(vec![MIN]));
        assert_eq!(found(d.path()), vec![(MIN, "8.3.27".to_string())]);
    }
}
