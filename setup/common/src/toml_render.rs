//! Rewrite the values of a commented example TOML in place: `change` edits
//! a file the person reads, so the comments must survive.

/// `value(section, key)` returns the new right-hand side (already quoted for
/// strings) or `None` to keep the line as it is.
pub fn render(example: &str, value: impl Fn(&str, &str) -> Option<String>) -> String {
    let mut section = String::new();
    let mut out = String::new();
    for line in example.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
            out.push_str(line);
        } else if let Some((key, _)) = t.split_once(" = ").filter(|_| !t.starts_with('#')) {
            match value(&section, key) {
                Some(v) => out.push_str(&format!("{key} = {v}")),
                None => out.push_str(line),
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EX: &str = "# top\n[a]\n# about x\nx = 1\nname = \"a\"\n\n[b]\nname = \"b\"\n";

    #[test]
    fn values_change_by_section_and_comments_stay() {
        let out = render(EX, |section, key| match (section, key) {
            ("[a]", "x") => Some("2".into()),
            ("[b]", "name") => Some("\"B\"".into()),
            _ => None,
        });
        assert_eq!(out, "# top\n[a]\n# about x\nx = 2\nname = \"a\"\n\n[b]\nname = \"B\"\n");
    }
}
