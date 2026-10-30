//! `sikemux open`: the files and folders asked for, each with the project it belongs to, opened
//! in the app's window.

use super::*;

pub(super) const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(super) struct OpenArgs {
    wait: bool,
    project: Option<PathBuf>,
    operands: Vec<String>,
}

pub(super) fn parse_open_args(args: Vec<String>) -> Result<Option<OpenArgs>, String> {
    let mut wait = false;
    let mut project = None;
    let mut operands = Vec::new();
    let mut literal = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if literal {
            operands.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => literal = true,
            "-w" | "--wait" => wait = true,
            "-p" | "--project" => {
                let value = iter.next().ok_or("--project requires a directory")?;
                project = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print!("{}", command_help("open").unwrap_or_default());
                return Ok(None);
            }
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            _ => operands.push(arg),
        }
    }
    if operands.is_empty() {
        return Err("open requires at least one path".into());
    }
    Ok(Some(OpenArgs {
        wait,
        project,
        operands,
    }))
}

pub(super) fn execute_open(args: OpenArgs) -> Result<i32, String> {
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?;
    let explicit_project = args
        .project
        .map(|path| resolve_existing(&path, &cwd))
        .transpose()?
        .map(|path| {
            if path.is_dir() {
                Ok(path)
            } else {
                Err("--project must name a directory".to_string())
            }
        })
        .transpose()?;

    let mut targets = Vec::with_capacity(args.operands.len());
    for (index, operand) in args.operands.iter().enumerate() {
        targets.push(resolve_target(
            operand,
            &cwd,
            explicit_project.as_deref(),
            index,
        )?);
    }
    if args.wait
        && targets
            .iter()
            .all(|target| target.kind == CliTargetKind::Directory)
    {
        return Err("--wait requires at least one file target".into());
    }

    let endpoint_path = endpoint_to_read()?;
    let descriptor = connect_or_launch(&endpoint_path)?;
    let request = CliOpenRequest {
        id: format!("request-{}", Uuid::new_v4().simple()),
        cwd: cwd.to_string_lossy().into_owned(),
        wait: args.wait,
        targets,
    };
    let command = CliClientCommand::Open {
        protocol: CLI_PROTOCOL_VERSION,
        token: descriptor.token.clone(),
        request,
    };
    let mut session = session(&descriptor)?;
    session.wait_for_answers(Some(ACCEPT_TIMEOUT))?;
    session.send(&command)?;
    let accepted = session.receive()?;
    let mut failed = false;
    match accepted {
        CliServerResponse::Accepted {
            failed: failures, ..
        } => {
            for failure in failures {
                eprintln!("sikemux: {}", failure.message);
                failed = true;
            }
        }
        CliServerResponse::Error { message } => return Err(message),
        _ => return Err("unexpected response from Sikemux".into()),
    }
    session.wait_for_answers(None)?;

    if args.wait {
        match session.receive()? {
            CliServerResponse::Closed {
                reason: CliCloseReason::TabsClosed,
                ..
            } => {}
            CliServerResponse::Closed {
                reason: CliCloseReason::AppExit,
                ..
            } => return Err("Sikemux exited before the editor tabs were closed".into()),
            CliServerResponse::Error { message } => return Err(message),
            _ => return Err("unexpected wait response from Sikemux".into()),
        }
    }
    Ok(i32::from(failed))
}

pub(super) fn resolve_target(
    operand: &str,
    cwd: &Path,
    explicit_project: Option<&Path>,
    index: usize,
) -> Result<CliOpenTarget, String> {
    let expanded = sikemux_process::paths::expand_home(operand)
        .to_string_lossy()
        .into_owned();
    let raw_path = absolute_candidate(Path::new(&expanded), cwd);
    let (path, line, column) = if raw_path.exists() {
        (raw_path, None, None)
    } else {
        let (path_part, line, column) = split_location(&expanded)?;
        (absolute_candidate(Path::new(path_part), cwd), line, column)
    };
    let path = fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let kind = if metadata.is_file() {
        CliTargetKind::File
    } else if metadata.is_dir() {
        CliTargetKind::Directory
    } else {
        return Err(format!(
            "{} is not a regular file or directory",
            path.display()
        ));
    };
    if kind == CliTargetKind::Directory && (line.is_some() || column.is_some()) {
        return Err(format!(
            "{} is a directory and cannot have a line or column",
            path.display()
        ));
    }
    let project_root = explicit_project
        .map(Path::to_path_buf)
        .unwrap_or_else(|| infer_project_root(&path, kind, cwd));
    Ok(CliOpenTarget {
        id: format!("target-{index}-{}", Uuid::new_v4().simple()),
        kind,
        path: path.to_string_lossy().into_owned(),
        project_root: project_root.to_string_lossy().into_owned(),
        line,
        column,
    })
}

pub(super) fn resolve_existing(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let expanded = sikemux_process::paths::expand_home(&path.to_string_lossy());
    let path = absolute_candidate(&expanded, cwd);
    fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn absolute_candidate(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

pub(super) fn split_location(raw: &str) -> Result<(&str, Option<u32>, Option<u32>), String> {
    let Some((before_last, last)) = raw.rsplit_once(':') else {
        return Ok((raw, None, None));
    };
    let Ok(last_number) = last.parse::<u32>() else {
        return Ok((raw, None, None));
    };
    if last_number == 0 {
        return Err("line and column positions are one-based and must be greater than zero".into());
    }
    if let Some((path, line)) = before_last.rsplit_once(':') {
        if let Ok(line_number) = line.parse::<u32>() {
            if line_number == 0 {
                return Err(
                    "line and column positions are one-based and must be greater than zero".into(),
                );
            }
            return Ok((path, Some(line_number - 1), Some(last_number - 1)));
        }
    }
    Ok((before_last, Some(last_number - 1), None))
}

pub(super) fn infer_project_root(path: &Path, kind: CliTargetKind, cwd: &Path) -> PathBuf {
    if let Some(root) = git_root(path, kind) {
        return root;
    }
    if path.starts_with(cwd) && cwd.is_dir() {
        return cwd.to_path_buf();
    }
    enclosing_folder(path, kind).to_path_buf()
}

/// The project a path opened from outside a shell belongs to: its repository,
/// or else the folder itself, or the folder a file sits in.
pub fn project_root(path: &Path, kind: CliTargetKind) -> PathBuf {
    git_root(path, kind).unwrap_or_else(|| enclosing_folder(path, kind).to_path_buf())
}

pub(super) fn git_root(path: &Path, kind: CliTargetKind) -> Option<PathBuf> {
    sikemux_process::files::repo_root(enclosing_folder(path, kind)).map(Path::to_path_buf)
}

pub(super) fn enclosing_folder(path: &Path, kind: CliTargetKind) -> &Path {
    if kind == CliTargetKind::Directory {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

/// The project a command run here works in: the one Sikemux started this
/// shell or agent in, else the repository around the current folder.
pub fn current_project() -> Result<String, String> {
    if let Some(project) = env::var(vars::PROJECT)
        .ok()
        .filter(|project| !project.is_empty())
    {
        return Ok(project);
    }
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    Ok(project_root(&cwd, CliTargetKind::Directory)
        .to_string_lossy()
        .into_owned())
}
