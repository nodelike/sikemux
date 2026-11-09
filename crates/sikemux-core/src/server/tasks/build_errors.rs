//! The errors a Gradle build printed, kept beside the language servers'
//! diagnostics so the editor and agents read them in one place.

use super::*;
use crate::server::lsp::{self, LspDiagnostic, LspDiagnosticSeverity, LspPos, LspRange};
use crate::tasks::build_errors::{self as found, BuildError, Severity};

const TOOL: &str = "gradle";
const MAX_FILES: usize = 512;
const MAX_PER_FILE: usize = 500;
const MAX_FOR_AGENT: usize = 50;

/// A Gradle line with no column covers the whole line.
fn diagnostic(error: BuildError) -> LspDiagnostic {
    let line = error.line - 1;
    LspDiagnostic {
        range: LspRange {
            start: LspPos {
                line,
                character: error.column.map_or(0, |column| column - 1),
            },
            end: LspPos {
                line: line + 1,
                character: 0,
            },
        },
        severity: Some(match error.severity {
            Severity::Error => LspDiagnosticSeverity::Error,
            Severity::Warning => LspDiagnosticSeverity::Warning,
        }),
        code: error.code,
        source: Some(error.source.to_owned()),
        message: error.message,
    }
}

/// Replaces what the project's last Gradle run reported with what this one printed.
pub(super) fn publish(core: &Core, project: &str, cwd: &Path, lines: &[String]) {
    let mut by_file: Vec<(String, Vec<LspDiagnostic>)> = Vec::new();
    for error in found::gradle(lines, cwd) {
        let path = error.path.to_string_lossy().into_owned();
        let index = match by_file.iter().position(|(known, _)| *known == path) {
            Some(index) => index,
            None if by_file.len() < MAX_FILES => {
                by_file.push((path, Vec::new()));
                by_file.len() - 1
            }
            None => continue,
        };
        let diagnostics = &mut by_file[index].1;
        if diagnostics.len() < MAX_PER_FILE {
            diagnostics.push(diagnostic(error));
        }
    }
    lsp::publish_build(core, project, TOOL, by_file);
}

/// The errors the project's last Gradle run printed, as `workspace_inspect` gives them.
pub(super) fn for_agent(project: &Path) -> Vec<Value> {
    lsp::published_by(&project.to_string_lossy(), TOOL)
        .into_iter()
        .flat_map(|set| {
            let file = relative(project, Path::new(&set.path));
            set.diagnostics
                .into_iter()
                .filter(|diagnostic| diagnostic.severity == Some(LspDiagnosticSeverity::Error))
                .map(move |diagnostic| {
                    json!({
                        "file": file,
                        "line": diagnostic.range.start.line + 1,
                        "column": diagnostic.range.start.character + 1,
                        "source": diagnostic.source,
                        "message": crate::text::clipped(&diagnostic.message, 1000),
                    })
                })
        })
        .take(MAX_FOR_AGENT)
        .collect()
}
