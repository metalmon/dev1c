//! Проверки по фактам, общие для обеих программ: что проверено, итог, и при
//! провале — что делать. Отчёт пишется в файл в комплекте: человек присылает
//! файл, а не пересказ.

#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub what: String,
    pub ok: bool,
    /// Информация, а не приговор: «туннель не поднят» — это не провал.
    pub info: bool,
    /// Занято кем-то другим: не ошибка настройки, а чужая рука на базе прямо
    /// сейчас. Ни пройденный, ни проваленный факт.
    pub busy: bool,
    pub detail: String,
    pub hint: String,
}

/// Итог факта — выведен из его полей, а не хранится рядом: два источника
/// правды об одном итоге — ровно та болезнь, против которой эта работа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Good,
    Busy,
    Bad,
}

impl Check {
    pub fn pass(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: false, busy: false, detail: detail.into(), hint: String::new() }
    }
    pub fn fail(what: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { what: what.into(), ok: false, info: false, busy: false, detail: detail.into(), hint: hint.into() }
    }
    pub fn note(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: true, busy: false, detail: detail.into(), hint: String::new() }
    }
    /// Занято: факт проверен, но решить его сейчас нельзя — кто-то другой
    /// держит базу. `ok` и `info` оба `true`, чтобы `summary()` не считал его
    /// ни пройденным (`ok && !info` ложно из-за `info`), ни проваленным
    /// (`!ok` ложно) — без изменений в самой формуле.
    pub fn busy(what: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: true, busy: true, detail: detail.into(), hint: hint.into() }
    }

    pub fn outcome(&self) -> Outcome {
        if self.busy {
            Outcome::Busy
        } else if self.ok {
            Outcome::Good
        } else {
            Outcome::Bad
        }
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
///
/// Метка строится на `outcome()`, а не своей отдельной цепочкой `busy`/`info`/
/// `ok`: было два похожих места, расходящихся молча (находка ревью задачи 2).
/// `info` здесь — не свой исход, а уточнение внутри `Good` (сведение против
/// настоящего прохода): `outcome()` сознательно не различает их — это одно и
/// то же для вердикта (`summary`), разница только в тексте для человека.
fn line(i: usize, c: &Check) -> String {
    let mark = match c.outcome() {
        Outcome::Busy => "busy",
        Outcome::Good if c.info => "info",
        Outcome::Good => " ok ",
        Outcome::Bad => "FAIL",
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

    #[test]
    fn a_busy_check_is_neither_passed_nor_failed() {
        let checks = vec![
            Check::pass("база", "C:\\Bases\\Buh"),
            Check::busy("в базе открыт сеанс", "1 открыт сеанс 1С", "закройте Предприятие"),
            Check::fail("RDP", "порт 3390 не отвечает", "dev1c-setup status"),
        ];
        assert_eq!(summary(&checks), (1, 1));
        assert_eq!(checks[1].outcome(), Outcome::Busy);
        assert_eq!(checks[0].outcome(), Outcome::Good);
        assert_eq!(checks[2].outcome(), Outcome::Bad);
        let text = render("1c-connect", "0.1.0", "", &checks);
        assert!(text.contains("[busy] в базе открыт сеанс"), "{text}");
        assert!(text.contains("→ закройте Предприятие"), "{text}");
        assert!(text.ends_with("Пройдено 1, провалено 1.\n"), "{text}");
    }
}
