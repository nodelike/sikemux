/// Build tools a detected task can need, with the Homebrew formula that
/// installs each.
const BUILD_TOOLS: &[(&str, &str)] = &[
    ("mvn", "maven"),
    ("gradle", "gradle"),
    ("go", "go"),
    ("npm", "node"),
    ("pnpm", "pnpm"),
    ("yarn", "yarn"),
    ("bun", "bun"),
    ("python3", "python"),
    ("uv", "uv"),
    ("poetry", "poetry"),
];

pub fn formula(program: &str) -> Option<&'static str> {
    BUILD_TOOLS
        .iter()
        .find(|(tool, _)| *tool == program)
        .map(|(_, formula)| *formula)
}

pub fn installed(program: &str) -> bool {
    sikemux_process::user_environment::find_executable(program).is_some()
}

/// Installs a build tool with Homebrew; returns where it now is.
pub fn install(program: &str) -> Result<String, String> {
    let formula = formula(program).ok_or("unknown build tool")?;
    brew_install(formula)?;
    sikemux_process::user_environment::find_executable(program)
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{formula} installed, but Sikemux could not find {program} on PATH"))
}

/// Runs `brew install`, failing with a reason a person can act on.
pub fn brew_install(formula: &str) -> Result<(), String> {
    let output = sikemux_process::user_environment::command("brew")
        .args(["install", formula])
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "Homebrew not found on PATH. Install Homebrew first: https://brew.sh".to_string()
            } else {
                format!("run brew install {formula}: {error}")
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("brew install {formula} failed: {}", stderr.trim()))
}
