use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
            return false;
        }
        true
    })
}

fn paint(code: &str, s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    if enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: impl AsRef<str>) -> String {
    paint("1", s)
}
pub fn dim(s: impl AsRef<str>) -> String {
    paint("2", s)
}
pub fn green(s: impl AsRef<str>) -> String {
    paint("32", s)
}
pub fn yellow(s: impl AsRef<str>) -> String {
    paint("33", s)
}
pub fn red(s: impl AsRef<str>) -> String {
    paint("31", s)
}
pub fn cyan(s: impl AsRef<str>) -> String {
    paint("36", s)
}
pub fn magenta(s: impl AsRef<str>) -> String {
    paint("35", s)
}
