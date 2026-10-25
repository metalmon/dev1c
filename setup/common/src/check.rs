//! Проверки по фактам, общие для обеих программ: что проверено, итог, и при
//! провале — что делать. Отчёт пишется в файл в комплекте: человек присылает
//! файл, а не пересказ.

#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub what: String,
    pub ok: bool,
    /// Информация, а не приговор: «туннель не поднят» — это не провал.
    pub info: bool,
    pub detail: String,
    pub hint: String,
}

impl Check {
    pub fn pass(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: false, detail: detail.into(), hint: String::new() }
    }
    pub fn fail(what: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { what: what.into(), ok: false, info: false, detail: detail.into(), hint: hint.into() }
    }
    pub fn note(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: true, detail: detail.into(), hint: String::new() }
    }
}

/// Сколько прошло и сколько провалено. Информационная строка — ни то, ни
/// другое: «туннель поднят» ничего не проверяет, и записывать её в пройденные
/// значит завышать итог. В отчёте заказчика так и выходило: «пройдено 17» при
/// пятнадцати настоящих проверках и трёх строках про туннели.
pub fn summary(checks: &[Check]) -> (usize, usize) {
    let passed = checks.iter().filter(|c| c.ok && !c.info).count();
    let failed = checks.iter().filter(|c| !c.ok).count();
    (passed, failed)
}

/// Одна строка списка — одинаковая на экране и в отчёте. Раньше её рисовали
/// два похожих цикла, и расходились бы они молча.
fn line(i: usize, c: &Check) -> String {
    let mark = if c.info {
        "info"
    } else if c.ok {
        " ok "
    } else {
        "FAIL"
    };
    let mut s = format!("{:>2}. [{mark}] {}: {}\n", i + 1, c.what, c.detail);
    if !c.hint.is_empty() {
        s.push_str(&format!("          → {}\n", c.hint));
    }
    s
}

/// Текст отчёта: нумерованный список от основания к вершине.
pub fn render(program: &str, version: &str, cfg_text: &str, checks: &[Check]) -> String {
    let mut s = format!("{program} {version}\n\n{cfg_text}\n");
    for (i, c) in checks.iter().enumerate() {
        s.push_str(&line(i, c));
    }
    let (p, f) = summary(checks);
    s.push_str(&format!("\nПройдено {p}, провалено {f}.\n"));
    s
}

/// Напечатать список в терминал, тем же языком, что в отчёте.
pub fn print(checks: &[Check]) {
    for (i, c) in checks.iter().enumerate() {
        print!("{}", line(i, c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_lines_never_count_as_failures() {
        let checks = vec![
            Check::pass("движок", "docker 28"),
            Check::note("туннель dev1", "не поднят"),
            Check::fail("RDP", "порт 3390 не отвечает", "dev1c-setup status"),
        ];
        // Информационная строка не пройденная и не проваленная: pass один,
        // fail один, а note не попадает ни в одно число.
        assert_eq!(summary(&checks), (1, 1));
        let text = render("dev1c-setup", "0.1.0", "", &checks);
        assert!(text.contains(" 2. [info] туннель dev1: не поднят"), "{text}");
        assert!(text.contains("→ dev1c-setup status"));
        assert!(text.ends_with("Пройдено 1, провалено 1.\n"));
    }
}
