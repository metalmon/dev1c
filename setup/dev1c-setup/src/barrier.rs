//! Преграды, которые называют себя. Сервер без интернета — администратор не
//! спросит нас на ходу, поэтому каждая преграда несёт причину и команду,
//! которая сработает на месте. Модуль — чистый текст и разбор чужого вывода,
//! без обращений к движку контейнеров: вызывающая сторона решает, когда
//! спросить `common::docker::probe`, этот модуль — что сказать по ответу.

use std::path::Path;

/// Разбор ответа движка контейнеров: занятый код — ещё не диагноз, текст
/// внутри называет настоящую причину. Два частых случая — сокет без прав и
/// неподнятый демон — выучены по характерным словам самого Docker; остальное
/// не наше дело, вызывающая сторона скажет что-то более общее. `None` — и код
/// нулевой, и по тексту ничего плохого не видно.
pub fn docker_verdict(out: &str, code: i32) -> Option<String> {
    if code == 0 {
        return None;
    }
    let low = out.to_ascii_lowercase();
    if low.contains("permission denied") {
        return Some(
            "Docker отвечает «в доступе отказано»: пользователь не в группе docker. \
             Добавьте его и войдите заново (новая сессия или `newgrp docker`):\n  sudo usermod -aG docker $USER"
                .into(),
        );
    }
    if low.contains("cannot connect to the docker daemon") || low.contains("daemon running") {
        return Some(
            "Служба Docker не отвечает: демон не запущен или сокет недоступен. Запустите его:\n  sudo systemctl restart docker".into(),
        );
    }
    None
}

/// Видеокарта не достаётся до контейнера. Сервер без интернета — ремонт не
/// может быть «поставьте пакет», он либо уже лежит в комплекте, либо его
/// должен принести администратор. `kit_has_debs` — есть ли в комплекте
/// каталог `nvidia-container-toolkit` с `.deb`-пакетами (его кладёт отдельная
/// задача; на старом комплекте каталога может и не быть).
pub fn gpu_in_container(ok: bool, kit_has_debs: bool) -> Option<String> {
    if ok {
        return None;
    }
    Some(if kit_has_debs {
        "Видеокарта не видна внутри контейнера. В комплекте есть NVIDIA Container Toolkit — установите его и перезапустите движок:\n  sudo dpkg -i nvidia-container-toolkit/*.deb && sudo systemctl restart docker".into()
    } else {
        "Видеокарта не видна внутри контейнера. Нужен NVIDIA Container Toolkit (пакеты .deb в каталоге nvidia-container-toolkit) — на этом сервере нет интернета, их приносит администратор вместе с комплектом.".into()
    })
}

/// Нужного файла нет в комплекте. `what` — что это такое для человека,
/// `path` — куда программа смотрела, `host_hint` — `<логин>@<адрес>` этого
/// сервера (его печатает установка, когда адрес уже известен). Команда — scp
/// каталога комплекта: сервер без интернета, скачивать нечем.
pub fn missing_file(what: &str, path: &Path, host_hint: &str) -> String {
    let dir = path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| path.display().to_string());
    format!(
        "Не найден {what}: {}. Скопируйте комплект целиком на сервер и установите из него, например:\n  scp -r <комплект> {host_hint}:{dir}",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_without_rights_says_how_to_get_them() {
        let t = docker_verdict("permission denied while trying to connect to the Docker daemon socket", 1).unwrap();
        assert!(t.contains("usermod -aG docker"));
        assert!(t.contains("sudo"));
    }

    #[test]
    fn a_dead_daemon_is_not_a_rights_problem() {
        let t = docker_verdict("Cannot connect to the Docker daemon at unix:///var/run/docker.sock", 1).unwrap();
        assert!(t.contains("systemctl"));
        assert!(!t.contains("usermod"));
    }

    #[test]
    fn a_zero_code_is_not_a_problem() {
        assert!(docker_verdict("permission denied", 0).is_none());
    }

    #[test]
    fn a_card_invisible_to_containers_points_at_the_kit_not_at_the_internet() {
        let t = gpu_in_container(false, true).unwrap();
        assert!(t.contains("dpkg -i"));
        assert!(!t.contains("apt"), "сервер без интернета: {t}");
        // Нет пакетов в комплекте — честно говорим, что нужен администратор.
        assert!(gpu_in_container(false, false).unwrap().contains("nvidia-container-toolkit"));
        assert!(gpu_in_container(true, true).is_none());
    }

    #[test]
    fn a_missing_file_comes_with_the_line_that_brings_it() {
        let t = missing_file("архив образа", Path::new("/srv/kit/dev1c-image.tar.gz"), "<user>@<сервер>");
        assert!(t.contains("scp"));
        assert!(t.contains("/srv/kit"));
    }
}
