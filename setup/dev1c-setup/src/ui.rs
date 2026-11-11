//! Слова командной строки: подсказка про комплект, отчёт о целостности,
//! адреса машины и строки чистки.
//!
//! Назывался «решения окна без GPUI» и нёс их все — окна у этой программы нет
//! с 12.11, и вместе с ним ушла та половина. Остался тот же приём, ради
//! которого модуль и заведён: решение о словах принимается здесь и проверяется
//! тестом, а печатает его вызывающий.

use common::op::CleanLevel;

/// Что человек обязан узнать прежде, чем согласится. Снять контейнер —
/// дело обратимое; удалить данные — нет, и беды там две разного рода:
/// репозитории с проектами не восстановить ничем, а платформа, демо-база,
/// модель и образ вернутся из архива комплекта. Пока этой строки не было,
/// «удалить тома» стояло в списке рядом с «docker rm -f» и выглядело таким
/// же пустяком.
pub fn clean_note(level: CleanLevel) -> String {
    match level {
        // Переносы расставлены руками: это самые длинные строки программы, и
        // читают их перед необратимым действием, а не по диагонали.
        CleanLevel::Stack => "Данные остаются на месте: репозитории, проекты и настройки рабочих мест,\n\
             платформа 1С, демо-база, файл модели.\n\
             Поднять среду заново: dev1c-setup install"
            .into(),
        CleanLevel::Data => "Исчезнет безвозвратно: репозитории Gitea, проекты и настройки\n\
             рабочих мест — больше их нет нигде.\n\
             Вернётся из архива комплекта: платформа 1С, демо-база, файл модели\n\
             и образ стенда. Поставить заново: dev1c-setup install"
            .into(),
    }
}

/// Чем кончается очистка. Своими словами, а не молчанием: до этой строки
/// настоящая работа говорила МЕНЬШЕ сухого прогона — тот печатал «на машине
/// ничего не менялось», а эта не печатала ничего.
pub fn clean_done(level: CleanLevel) -> String {
    match level {
        CleanLevel::Stack => "Снято. Данные на месте — поднять среду заново: dev1c-setup install".into(),
        CleanLevel::Data => "Удалено вместе с данными. Поставить заново из комплекта: dev1c-setup install".into(),
    }
}

/// Если адрес этой машины не удалось узнать, в строках «Доступа» остаётся
/// заглушка — лучше честной заглушки только настоящий адрес.
pub const ADDRESS_UNKNOWN: &str = "<адрес>";

/// `<логин>@<адрес>` для команд scp в преградах: логин спрашивать не у кого
/// (сервер без интернета, ответить некому), а адрес — настоящий, если машина
/// его уже знает, и заглушка, если нет.
pub fn host_hint() -> String {
    let host = local_addresses().first().cloned().unwrap_or_else(|| ADDRESS_UNKNOWN.to_string());
    format!("<логин>@{host}")
}

/// Порядок адресов для «Доступа»: сперва адреса локальной сети (по ним
/// ходят рабочие места), потом всё остальное — например, адрес Tailscale.
/// Машина с туннелем иначе показала бы администратору адрес, по которому в
/// его сети к среде никто не подключится.
pub fn order_addresses(found: &[String]) -> Vec<String> {
    // Ярус 0 — сети, в которых стоят рабочие места. Ярус 1 — 172.16/12:
    // частная сеть, но в неё же Docker и Hyper-V кладут свои мосты, поэтому
    // ниже настоящей. Ярус 2 — всё остальное, включая Tailscale (100.64/10).
    let tier = |a: &String| {
        if a.starts_with("192.168.") || a.starts_with("10.") {
            0
        } else if a
            .strip_prefix("172.")
            .and_then(|r| r.split('.').next())
            .and_then(|o| o.parse::<u8>().ok())
            .is_some_and(|o| (16..=31).contains(&o))
        {
            1
        } else {
            2
        }
    };
    let mut out: Vec<String> = Vec::new();
    for want in 0..=2 {
        for a in found.iter().filter(|a| tier(a) == want) {
            // 169.254/16 — адрес, который машина дала себе сама, когда DHCP
            // не ответил: по нему не подключится никто.
            if !a.starts_with("169.254.") && !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

/// Итог сверки комплекта: сколько файлов совпало или чего не хватает.
/// Перечень обрывается: половина несовпавшего комплекта — это сотни имён, и
/// строка в журнал должна остаться читаемой.
pub fn integrity_report(ok: usize, bad: &[String]) -> Result<String, String> {
    const SHOWN: usize = 20;
    if bad.is_empty() {
        return Ok(format!("комплект цел: {}, все суммы совпали", common::word::count(ok as u64, "файл", "файла", "файлов")));
    }
    let mut names = bad.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("; ");
    if bad.len() > SHOWN {
        names.push_str(&format!("; и ещё {}", bad.len() - SHOWN));
    }
    Err(format!("комплект скопирован не полностью или испорчен — возьмите его заново. Совпало {ok}, не совпало {}: {names}", bad.len()))
}

/// Адреса этой машины: тот, с которого система пошла бы наружу (UDP-сокету
/// хватает `connect`, пакетов не отправляем), плюс всё, что система знает о
/// собственном имени. Пусто — «Доступ» покажет заглушку.
pub fn local_addresses() -> Vec<String> {
    use std::net::ToSocketAddrs;
    let mut found = Vec::new();
    if let Some(ip) = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| s.connect("8.8.8.8:80").ok().map(|_| s))
        .and_then(|s| s.local_addr().ok())
        .map(|a| a.ip())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
    {
        found.push(ip.to_string());
    }
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    if !host.is_empty() {
        if let Ok(addrs) = (host.as_str(), 0u16).to_socket_addrs() {
            for a in addrs.filter(|a| a.is_ipv4() && !a.ip().is_loopback()) {
                found.push(a.ip().to_string());
            }
        }
    }
    order_addresses(&found)
}

#[cfg(test)]
mod tests {
    /// Главное в предупреждении — не перечень, а разделение: что вернётся из
    /// комплекта, а что не вернётся ничем.
    #[test]
    fn the_clean_warning_separates_what_comes_back_from_what_does_not() {
        let data = super::clean_note(common::op::CleanLevel::Data);
        assert!(data.contains("безвозвратно") && data.contains("репозитории Gitea"), "{data}");
        assert!(data.contains("платформа 1С") && data.contains("из архива комплекта"), "{data}");
        let stack = super::clean_note(common::op::CleanLevel::Stack);
        assert!(stack.contains("остаются на месте"), "{stack}");
        assert!(!stack.contains("безвозвратно"), "снять контейнер — дело обратимое: {stack}");
    }

    use super::*;

    #[test]
    fn a_damaged_kit_names_at_most_twenty_files() {
        let ok = integrity_report(42, &[]).unwrap();
        assert!(ok.contains("42"), "{ok}");
        let many: Vec<String> = (0..50).map(|i| format!("нет: f{i}")).collect();
        let bad = integrity_report(0, &many).unwrap_err();
        assert!(bad.contains("f0") && bad.contains("f19"), "{bad}");
        assert!(!bad.contains("f20"), "перечень должен обрываться: {bad}");
        assert!(bad.contains("и ещё 30"), "{bad}");
    }

    #[test]
    fn the_lan_address_comes_before_the_tunnel_one() {
        // На машине с Tailscale наружу система ходит через 100.64/10, но
        // рабочие места в сети клиента подключаются по локальному адресу.
        let found = vec!["100.77.83.96".to_string(), "192.168.1.10".to_string(), "100.77.83.96".to_string()];
        assert_eq!(order_addresses(&found), vec!["192.168.1.10", "100.77.83.96"]);
        // Мосты Docker и Hyper-V живут в 172.16/12 и не должны обгонять
        // настоящую сеть: у них свой, второй ярус.
        assert_eq!(order_addresses(&["172.17.0.1".into(), "10.0.5.7".into()]), vec!["10.0.5.7", "172.17.0.1"]);
        assert_eq!(order_addresses(&["172.17.0.1".into(), "100.77.83.96".into()]), vec!["172.17.0.1", "100.77.83.96"]);
        // 172.32 — уже не частная сеть, в начало не лезет.
        assert_eq!(order_addresses(&["172.32.0.1".into(), "172.20.0.5".into()]), vec!["172.20.0.5", "172.32.0.1"]);
        // Адрес без DHCP (169.254/16) не адрес вовсе.
        assert_eq!(order_addresses(&["169.254.3.4".into(), "192.168.1.10".into()]), vec!["192.168.1.10"]);
        assert!(order_addresses(&[]).is_empty());
    }
}
