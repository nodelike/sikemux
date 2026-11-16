//! Writes what `whoami /groups` prints to `<name>.txt` beside this program, so a program the
//! core starts reports what it runs at. Its first argument names the file; none names it `none`.

fn main() {
    let name: String = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "none".into())
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    // The lowered start every other child takes would report its own level, not this program's.
    #[allow(clippy::disallowed_methods)]
    let report = std::process::Command::new("whoami")
        .arg("/groups")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_else(|error| format!("whoami did not run: {error}"));
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|folder| folder.join(format!("{name}.txt")))
        })
        .expect("this program's folder");
    std::fs::write(beside, report).expect("the report is written");
}
