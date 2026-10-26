//! Какая версия платформы — та же, что выберет 1С, а не просто самая новая.
//! Спека, раздел «Версия платформы: та же, что выберет 1С, а не просто самая
//! новая».
//!
//! Лестница, сверху вниз, первое совпадение побеждает: переменная
//! `ONEC_PLATFORM` (её разбирает `ops_host`, там диск), поле `platform` в
//! `1c-connect.toml`, пин базы (`Version=` в её строке в `ibases.v8i`), общий
//! пин (`DefaultVersion=<маска>=<версия>` в `1CEStart.cfg`) и, если пинов
//! нет, самая новая — то, что коннектор делал всегда. Каждая ступень названа
//! словами: факт проверки говорит не только какая версия, но и почему.
//!
//! **Пин, который нельзя выполнить, — отказ, а не откат на новейшую.** Версия
//! названа человеком или списком баз, её на машине нет — молча взять другую
//! значило бы соврать про то, на чём идёт работа.
//!
//! Весь выбор — чистые функции: установленное и пины передаются аргументами,
//! на диск ходит одна `pins_on_disk`. Поэтому лестница проверяется тестом, а
//! не машиной с тремя установленными версиями.

use crate::cfg::Cfg;
use std::path::{Path, PathBuf};

/// Почему выбрана эта версия. Порядок — та же лестница.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Env,
    Field,
    BasePin,
    DefaultVersion,
    Newest,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Why::Env => "переменная ONEC_PLATFORM",
            Why::Field => "поле platform в 1c-connect.toml",
            Why::BasePin => "пин базы в ibases.v8i",
            Why::DefaultVersion => "DefaultVersion в 1CEStart.cfg",
            Why::Newest => "самая новая из установленных",
        };
        f.write_str(s)
    }
}

/// Выбранная платформа: путь к исполняемому файлу, её версия и причина.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub exe: PathBuf,
    pub version: String,
    pub why: Why,
}

/// Пины, какие нашлись на диске (`pins_on_disk`). Пусто — пинов нет, и это
/// не беда: 1С без пина тоже берёт новейшую.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pins {
    /// `Version=` строки нашей базы в `ibases.v8i`.
    pub base: Option<String>,
    /// Строки `DefaultVersion=<маска>=<версия>` из `1CEStart.cfg`, в порядке
    /// старшинства: пользовательский файл раньше общего.
    pub default: Vec<(String, String)>,
}

/// Версия как число по компонентам: `8.3.9` старее `8.3.27`, хотя как строки —
/// наоборот.
fn key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Подходит ли версия под маску: маска — префикс по компонентам, `8.3`
/// подходит и `8.3.21.1484`, и `8.3.27.1508`; полная версия подходит только
/// себе. Маска из нечисловых частей не подходит ничему — иначе опечатка в
/// поле тихо выбрала бы новейшую.
pub fn matches_mask(version: &str, mask: &str) -> bool {
    let (v, m) = (key(version), key(mask));
    if m.is_empty() || mask.split('.').any(|p| p.parse::<u32>().is_err()) {
        return false;
    }
    v.len() >= m.len() && v[..m.len()] == m[..]
}

/// Как база выглядит в строке подключения `ibases.v8i`, в сравнимом виде:
/// `file:<путь>` или `srv:<сервер>/<база>`. Регистр и вид разделителей не
/// считаются: один и тот же каталог человек пишет и так, и иначе.
fn connect_key(base_path: &str) -> String {
    let p = base_path.trim().trim_end_matches(['\\', '/']);
    // Та же классификация, что у `ops_host::ib_switch`: `сервер\база` — это
    // путь с обратной косой и без двоеточия диска.
    if p.contains('\\') && !p.contains(':') {
        if let Some((srv, re)) = p.rsplit_once('\\') {
            return format!("srv:{}/{}", srv.to_lowercase(), re.to_lowercase());
        }
    }
    format!("file:{}", p.replace('\\', "/").to_lowercase())
}

/// То же из значения `Connect=` строки списка баз:
/// `File="C:\Bases\Buh";` или `Srvr="srv-1c";Ref="buh";`.
fn connect_key_of_value(value: &str) -> Option<String> {
    let field = |name: &str| -> Option<String> {
        let at = value.find(name)?;
        let rest = &value[at + name.len()..];
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let inner = rest.strip_prefix('"')?;
        Some(inner[..inner.find('"')?].to_string())
    };
    if let Some(file) = field("File") {
        return Some(connect_key(&file));
    }
    let (srv, re) = (field("Srvr")?, field("Ref")?);
    Some(format!("srv:{}/{}", srv.trim().to_lowercase(), re.trim().to_lowercase()))
}

/// Пин версии для **нашей** базы из текста `ibases.v8i`: ищем её строку по
/// данным подключения и читаем `Version=`. Базы нет в списке — пина нет (у
/// 1С в этом случае тоже).
pub fn base_pin(v8i: &str, base_path: &str) -> Option<String> {
    let want = connect_key(base_path);
    let mut connect = None;
    let mut version = None;
    let mut matched = None;
    // Файл начинается с BOM, разделы — `[Имя базы]`, дальше `Ключ=Значение`.
    for line in v8i.trim_start_matches('\u{feff}').lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if connect.as_deref() == Some(want.as_str()) {
                matched = version.take();
                break;
            }
            connect = None;
            version = None;
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        match k.trim() {
            "Connect" => connect = connect_key_of_value(v),
            "Version" => version = Some(v.trim().trim_matches('"').to_string()).filter(|s: &String| !s.is_empty()),
            _ => {}
        }
    }
    if matched.is_none() && connect.as_deref() == Some(want.as_str()) {
        matched = version;
    }
    matched
}

/// Общие пины из текста `1CEStart.cfg`: строки `DefaultVersion=8.3=8.3.21.1484`
/// как пары (маска, версия). Остальные строки файла (`InstalledLocation`,
/// `LANGUAGES`) нас тут не касаются.
pub fn default_versions(cfg_text: &str) -> Vec<(String, String)> {
    cfg_text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("DefaultVersion="))
        .filter_map(|rest| rest.split_once('='))
        .map(|(mask, version)| (mask.trim().to_string(), version.trim().to_string()))
        .filter(|(m, v)| !m.is_empty() && !v.is_empty())
        .collect()
}

/// Каталоги установки из `1CEStart.cfg` (строки `InstalledLocation=`). Они
/// **добавляются** к стандартным, а не заменяют их: платформа может лежать
/// вне `Program Files` — наш же `install` регистрирует там теневой каталог
/// учебной платформы.
pub fn installed_locations(cfg_text: &str) -> Vec<PathBuf> {
    cfg_text.lines().filter_map(|l| l.trim().strip_prefix("InstalledLocation=")).map(|p| PathBuf::from(p.trim())).collect()
}

/// Самая новая из подходящих под маску.
fn newest<'a>(installed: &'a [(String, PathBuf)], mask: Option<&str>) -> Option<&'a (String, PathBuf)> {
    installed.iter().filter(|(v, _)| mask.is_none_or(|m| matches_mask(v, m))).max_by_key(|(v, _)| key(v))
}

fn missing(want: &str, source: &str, installed: &[(String, PathBuf)]) -> String {
    let mut have: Vec<&str> = installed.iter().map(|(v, _)| v.as_str()).collect();
    have.sort_by_key(|v| key(v));
    let list = if have.is_empty() { "ни одной".to_string() } else { have.join(", ") };
    format!("версия {want} ({source}) на машине не установлена; установлено: {list}")
}

/// Выбор по лестнице. Чистая: `installed` — что нашлось на диске (версия,
/// путь к исполняемому файлу), `pins` — что нашлось в файлах 1С.
///
/// `Err` — либо платформы нет вовсе, либо назван пин, которого на машине нет:
/// в обоих случаях текст годится и факту проверки, и причине замка.
pub fn resolve(cfg: &Cfg, installed: &[(String, PathBuf)], pins: &Pins) -> Result<Choice, String> {
    if installed.is_empty() {
        return Err("платформа 1С не найдена".into());
    }
    let pick = |m: &str, why: Why, source: &str| match newest(installed, Some(m)) {
        Some((v, exe)) => Ok(Choice { exe: exe.clone(), version: v.clone(), why }),
        None => Err(missing(m, source, installed)),
    };
    let field = cfg.host.platform.trim();
    if !field.is_empty() {
        return pick(field, Why::Field, "поле platform в 1c-connect.toml");
    }
    if let Some(pin) = pins.base.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        return pick(pin, Why::BasePin, "пин базы в ibases.v8i");
    }
    // Маску спрашивает ярлык базы; у нас её роль играет линия новейшей
    // установленной — ближайшее честное приближение, и оно названо в факте.
    // Пин применяется только если его маска подходит этой линии: при 8.3 и
    // 8.5 на машине строка `DefaultVersion=8.3=…` новейшую 8.5 не трогает —
    // так же, как у 1С, где ярлык 8.5 этой строки не спрашивает.
    let (top, _) = newest(installed, None).expect("список не пуст");
    if let Some((_, pinned)) = pins.default.iter().find(|(mask, _)| matches_mask(top, mask)) {
        return pick(pinned, Why::DefaultVersion, "строка DefaultVersion в 1CEStart.cfg");
    }
    let (v, exe) = newest(installed, None).expect("список не пуст");
    Ok(Choice { exe: exe.clone(), version: v.clone(), why: Why::Newest })
}

/// Единственное место этого модуля, которое ходит на диск: читает список баз
/// и оба `1CEStart.cfg` (пользовательский раньше общего).
pub fn pins_on_disk(cfg: &Cfg) -> Pins {
    let v8i = starter_dirs().into_iter().map(|d| d.join("ibases.v8i")).find_map(|p| read_1c_text(&p));
    let base = v8i.as_deref().and_then(|text| base_pin(text, &cfg.host.base_path));
    let default = starter_dirs()
        .into_iter()
        .map(|d| d.join(starter_cfg_name()))
        .filter_map(|p| read_1c_text(&p))
        .flat_map(|t| default_versions(&t))
        .collect();
    Pins { base, default }
}

/// Имя файла настроек лаунчера: на Windows инструменты 1С пишут его с
/// заглавными буквами, на Linux — строчными.
fn starter_cfg_name() -> &'static str {
    if cfg!(windows) {
        "1CEStart.cfg"
    } else {
        "1cestart.cfg"
    }
}

/// Каталоги лаунчера в порядке старшинства: пользовательский раньше общего.
/// У учебной платформы свой лаунчер и свой каталог (`1cestartt`), поэтому
/// смотрим оба — какая платформа выбрана, выяснится уже по пинам.
fn starter_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        for var in ["APPDATA", "PROGRAMDATA"] {
            if let Some(base) = std::env::var_os(var) {
                dirs.push(PathBuf::from(base).join("1C").join("1CEStart"));
            }
        }
    } else if let Some(home) = std::env::var_os("HOME") {
        for name in ["1cestart", "1cestartt"] {
            dirs.push(PathBuf::from(&home).join(".1C").join(name));
        }
    }
    dirs
}

/// Текст файла 1С: `1CEStart.cfg` на Windows — UTF-16LE с BOM, `ibases.v8i` —
/// UTF-8 с BOM. Читаем байты и разбираем по BOM, а не по имени файла: один
/// читатель на оба, и ошибиться в выборе нечем.
fn read_1c_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() >= 2 && bytes[0..2] == [0xFF, 0xFE] {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    Some(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Vec<(String, PathBuf)> {
        [
            ("8.3.21.1484", "/opt/1cv8/x86_64/8.3.21.1484/1cv8"),
            ("8.3.27.1508", "/opt/1cv8/x86_64/8.3.27.1508/1cv8"),
            ("8.5.1.1150", "/opt/1cv8/x86_64/8.5.1.1150/1cv8"),
        ]
        .into_iter()
        .map(|(v, p)| (v.to_string(), PathBuf::from(p)))
        .collect()
    }

    fn cfg_with(platform: &str, base: &str) -> Cfg {
        let mut c = crate::cfg::filled_for_test();
        c.host.platform = platform.into();
        c.host.base_path = base.into();
        c
    }

    #[test]
    fn no_pins_means_the_newest() {
        let got = resolve(&cfg_with("", "/home/dev1/base"), &installed(), &Pins::default()).unwrap();
        assert_eq!((got.version.as_str(), got.why), ("8.5.1.1150", Why::Newest));
    }

    #[test]
    fn the_field_beats_the_pins() {
        let pins = Pins { base: Some("8.3.21.1484".into()), default: vec![("8.3".into(), "8.3.21.1484".into())] };
        let got = resolve(&cfg_with("8.3.27", "/home/dev1/base"), &installed(), &pins).unwrap();
        assert_eq!((got.version.as_str(), got.why), ("8.3.27.1508", Why::Field));
    }

    #[test]
    fn the_base_pin_beats_the_default_version() {
        let pins = Pins { base: Some("8.3.21".into()), default: vec![("8.5".into(), "8.5.1.1150".into())] };
        let got = resolve(&cfg_with("", "/home/dev1/base"), &installed(), &pins).unwrap();
        assert_eq!((got.version.as_str(), got.why), ("8.3.21.1484", Why::BasePin));
    }

    /// Маска берёт новейшую подходящую, а не первую попавшуюся.
    #[test]
    fn a_mask_takes_the_newest_match() {
        let mut all = installed();
        all.push(("8.3.27.2214".to_string(), PathBuf::from("/opt/1cv8/x86_64/8.3.27.2214/1cv8")));
        let got = resolve(&cfg_with("8.3", "/home/dev1/base"), &all, &Pins::default()).unwrap();
        assert_eq!(got.version, "8.3.27.2214");
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        let all = vec![("8.3.9.1000".to_string(), PathBuf::from("/a/1cv8")), ("8.3.27.1508".to_string(), PathBuf::from("/b/1cv8"))];
        assert_eq!(resolve(&cfg_with("8.3", "/home/dev1/base"), &all, &Pins::default()).unwrap().version, "8.3.27.1508");
        assert!(matches_mask("8.3.27.1508", "8.3"));
        assert!(!matches_mask("8.3.9.1000", "8.3.27"));
    }

    /// Пин, который нельзя выполнить, — отказ со списком установленного, а не
    /// молчаливый откат на новейшую: иначе работа шла бы не на той платформе,
    /// о которой человек просил.
    #[test]
    fn an_unsatisfiable_pin_is_an_error_not_the_newest() {
        let e = resolve(&cfg_with("8.2.19", "/home/dev1/base"), &installed(), &Pins::default()).unwrap_err();
        assert!(e.contains("8.2.19") && e.contains("platform в 1c-connect.toml"), "{e}");
        assert!(e.contains("8.3.21.1484") && e.contains("8.5.1.1150"), "в отказе обязан быть список установленного: {e}");
        let pins = Pins { base: Some("8.4".into()), default: Vec::new() };
        let e = resolve(&cfg_with("", "/home/dev1/base"), &installed(), &pins).unwrap_err();
        assert!(e.contains("8.4") && e.contains("ibases.v8i"), "{e}");
        // Опечатка в маске не выбирает новейшую молча.
        let e = resolve(&cfg_with("восемь", "/home/dev1/base"), &installed(), &Pins::default()).unwrap_err();
        assert!(e.contains("восемь"), "{e}");
    }

    #[test]
    fn without_a_platform_there_is_no_choice() {
        let e = resolve(&cfg_with("", "/home/dev1/base"), &[], &Pins::default()).unwrap_err();
        assert_eq!(e, "платформа 1С не найдена");
    }

    /// Строка `DefaultVersion` применяется только к своей линии: при 8.3 и 8.5
    /// на машине пин `8.3=…` новейшую 8.5 не трогает — так же, как у 1С, где
    /// ярлык 8.5 этой строки не спрашивает.
    #[test]
    fn a_default_version_applies_only_to_its_own_line() {
        let pins = Pins { base: None, default: vec![("8.3".into(), "8.3.21.1484".into())] };
        let got = resolve(&cfg_with("", "/home/dev1/base"), &installed(), &pins).unwrap();
        assert_eq!((got.version.as_str(), got.why), ("8.5.1.1150", Why::Newest));
        // Убрали 8.5 — линия новейшей стала 8.3, и пин заработал.
        let only83: Vec<_> = installed().into_iter().filter(|(v, _)| v.starts_with("8.3")).collect();
        let got = resolve(&cfg_with("", "/home/dev1/base"), &only83, &pins).unwrap();
        assert_eq!((got.version.as_str(), got.why), ("8.3.21.1484", Why::DefaultVersion));
    }

    const V8I: &str = "\u{feff}[Бухгалтерия]\r\nConnect=File=\"C:\\Bases\\Buh\";\r\nID=1\r\nVersion=8.3.21\r\n\r\n[Демо]\r\nConnect=File=\"/home/dev1/base\";\r\nID=2\r\nApp=Auto\r\n\r\n[Склад]\r\nConnect=Srvr=\"SRV-1C\";Ref=\"wh\";\r\nVersion=8.3.27.1508\r\n";

    #[test]
    fn base_pin_finds_our_file_base() {
        // Регистр и вид разделителей не считаются: тот же каталог.
        assert_eq!(base_pin(V8I, "C:\\Bases\\Buh").as_deref(), Some("8.3.21"));
        assert_eq!(base_pin(V8I, "c:/bases/buh/").as_deref(), Some("8.3.21"));
    }

    #[test]
    fn base_pin_matches_a_server_base() {
        assert_eq!(base_pin(V8I, "srv-1c\\wh").as_deref(), Some("8.3.27.1508"));
    }

    #[test]
    fn a_base_without_a_pin_or_absent_from_the_list_has_none() {
        // Есть в списке, но без `Version=` — и это последняя база файла.
        assert_eq!(base_pin(V8I, "/home/dev1/base"), None);
        assert_eq!(base_pin(V8I, "/home/dev3/base"), None);
        assert_eq!(base_pin("", "/home/dev1/base"), None);
    }

    /// Последняя база в файле разбирается так же, как остальные: у неё нет
    /// следующего `[раздела]`, который закрыл бы предыдущий.
    #[test]
    fn the_last_base_of_the_file_is_read_too() {
        let v8i = "\u{feff}[Демо]\r\nConnect=File=\"/home/dev1/base\";\r\nVersion=8.3.27\r\n";
        assert_eq!(base_pin(v8i, "/home/dev1/base").as_deref(), Some("8.3.27"));
    }

    const CFG: &str = "LANGUAGES=ru\r\nInstalledLocation=C:\\Users\\dev\\AppData\\Local\\dev1c\\platform\\8.3.27.1508\r\nDefaultVersion=8.3=8.3.21.1484\r\nDefaultVersion=8.5=8.5.1.1150\r\n";

    #[test]
    fn default_version_reads_the_mask_form() {
        assert_eq!(
            default_versions(CFG),
            vec![("8.3".to_string(), "8.3.21.1484".to_string()), ("8.5".to_string(), "8.5.1.1150".to_string())]
        );
        assert!(default_versions("LANGUAGES=ru\r\n").is_empty());
    }

    #[test]
    fn installed_locations_are_read_whole() {
        assert_eq!(installed_locations(CFG), vec![PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\platform\\8.3.27.1508")]);
        assert!(installed_locations("LANGUAGES=ru\r\n").is_empty());
    }
}
