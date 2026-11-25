//! What `ssh`, `scp`, `sftp` and `rsync` change on another computer.

use super::{changes_cluster, flag_value, has_flag, only_queries, CURL_DATA};
use crate::git_guard::{
    base_name, command_words, git_args, pieces, shell_script, Piece, Unwrapped,
};

pub(super) const PROGRAMS: [&str; 4] = ["ssh", "scp", "sftp", "rsync"];

/// The host `program`, run with `args`, changes; none when it only reads there, or copies from it.
pub(super) fn changed_host(program: &str, args: &[String]) -> Option<String> {
    let args = without_redirections(args);
    let host = match program {
        "ssh" => ssh_changes(&args)?,
        "sftp" => host_of(first_past_options(&args, SFTP_VALUES)?),
        "scp" => remote_host(copied_to(&args, SCP_VALUES)?)?,
        "rsync" if rsync_only_tries(&args) => return None,
        "rsync" => remote_host(copied_to(&args, RSYNC_VALUES)?)?,
        _ => return None,
    };
    (!super::local_host(host)).then(|| host.to_owned())
}

/// The short options that take a value.
const SSH_VALUES: &str = "BbcDEeFIiJLlmOopPQRSWw";
const SCP_VALUES: &[&str] = &["-c", "-D", "-F", "-i", "-J", "-l", "-o", "-P", "-S", "-X"];
const SFTP_VALUES: &[&str] = &[
    "-B", "-b", "-c", "-D", "-F", "-i", "-J", "-l", "-o", "-P", "-R", "-S", "-s", "-X",
];
const RSYNC_VALUES: &[&str] = &[
    "-B",
    "-e",
    "-f",
    "-M",
    "-T",
    "--backup-dir",
    "--bwlimit",
    "--chmod",
    "--chown",
    "--compare-dest",
    "--copy-dest",
    "--exclude",
    "--exclude-from",
    "--files-from",
    "--filter",
    "--include",
    "--include-from",
    "--link-dest",
    "--log-file",
    "--max-size",
    "--min-size",
    "--out-format",
    "--password-file",
    "--port",
    "--rsh",
    "--rsync-path",
    "--temp-dir",
    "--timeout",
];

/// `args` without the redirections of the line they were typed on.
fn without_redirections(args: &[String]) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        let operator = word.trim_start_matches(|c: char| c.is_ascii_digit());
        if !operator.starts_with(['<', '>']) {
            kept.push(word);
        } else if operator.trim_start_matches(['<', '>', '-']).is_empty() {
            words.next();
        }
    }
    kept
}

/// The host an `ssh` command changes: one given a command that does more than read, or none,
/// as it then runs what it is sent on standard input.
fn ssh_changes<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut host = None;
    let mut words = args.iter().copied();
    let mut remote: Vec<&str> = Vec::new();
    while let Some(word) = words.next() {
        match (word.strip_prefix('-'), host) {
            (Some(""), _) | (None, Some(_)) => {
                remote.push(word);
                remote.extend(words.by_ref());
            }
            (Some(letters), _) => {
                // A tunnel, a check of the settings, or an order to a connection already open.
                let value_at = letters.find(|letter| SSH_VALUES.contains(letter));
                let switches = letters.get(..value_at.unwrap_or(letters.len()))?;
                let takes = value_at.and_then(|at| letters.get(at..));
                if switches.contains(['G', 'N', 'V'])
                    || takes.is_some_and(|takes| takes.starts_with(['O', 'Q', 'W']))
                {
                    return None;
                }
                if takes.is_some_and(|takes| takes.len() == 1) {
                    words.next();
                }
            }
            (None, None) => host = Some(word),
        }
    }
    let host = host?;
    let reads = !remote.is_empty() && script_reads(&remote.join(" "));
    (!reads).then(|| host_of(host))
}

fn past_options<'a>(args: &[&'a str], values: &[&str]) -> Vec<&'a str> {
    let mut given = Vec::new();
    let mut words = args.iter().copied();
    while let Some(word) = words.next() {
        if values.contains(&word) {
            words.next();
        } else if !word.starts_with('-') {
            given.push(word);
        }
    }
    given
}

fn first_past_options<'a>(args: &[&'a str], values: &[&str]) -> Option<&'a str> {
    past_options(args, values).first().copied()
}

/// Where a copy goes: the last of its paths.
fn copied_to<'a>(args: &[&'a str], values: &[&str]) -> Option<&'a str> {
    match past_options(args, values).as_slice() {
        [_, .., to] => Some(to),
        _ => None,
    }
}

fn rsync_only_tries(args: &[&str]) -> bool {
    let mut words = args.iter().copied();
    while let Some(arg) = words.next() {
        if arg == "--" {
            break;
        }
        if arg == "--dry-run" || arg == "--help" {
            return true;
        }
        if RSYNC_VALUES.contains(&arg) {
            words.next();
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        if let Some(letters) = arg.strip_prefix('-') {
            for letter in letters.chars() {
                if matches!(letter, 'B' | 'e' | 'f' | 'M' | 'T') {
                    break;
                }
                if letter == 'n' {
                    return true;
                }
            }
        }
    }
    false
}

/// The host of a path on another computer, such as `me@host:folder` or `rsync://host/folder`.
fn remote_host(path: &str) -> Option<&str> {
    let remote =
        path.contains("://") || (path.split('/').next()).is_some_and(|first| first.contains(':'));
    remote.then(|| host_of(path))
}

/// The host in `me@host`, `me@host:folder` or `ssh://me@host:22/folder`.
fn host_of(address: &str) -> &str {
    let address = address
        .split_once("://")
        .map_or(address, |(_, address)| address);
    let address = address.split('/').next().unwrap_or(address);
    let address = address.rsplit('@').next().unwrap_or(address);
    match address.strip_prefix('[').and_then(|v6| v6.split_once(']')) {
        Some((v6, _)) => v6,
        None => address.split(':').next().unwrap_or(address),
    }
}

/// Whether every command of a shell script is known to only read.
fn script_reads(script: &str) -> bool {
    let Some(cleaned) = clean_read_script(script) else {
        return false;
    };
    pieces(&cleaned).iter().all(|piece| match piece {
        Piece::Words(words) => command_reads(words),
        Piece::Open | Piece::Close => true,
    })
}

fn clean_read_script(script: &str) -> Option<String> {
    const SAFE_REDIRECTS: &[&str] = &[
        "2>&1",
        "1>&2",
        ">&2",
        "&>/dev/null",
        "&> /dev/null",
        "2>/dev/null",
        "2> /dev/null",
        "1>/dev/null",
        "1> /dev/null",
        ">/dev/null",
        "> /dev/null",
    ];
    let bytes = script.as_bytes();
    let mut out = String::with_capacity(script.len());
    let mut at = 0;
    let mut in_single = false;
    let mut in_double = false;
    while let Some(rest) = script.get(at..).filter(|rest| !rest.is_empty()) {
        if in_single {
            let ch = rest.chars().next()?;
            out.push(ch);
            at += ch.len_utf8();
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if rest.starts_with('\\') {
                let mut chars = rest.chars();
                out.push(chars.next()?);
                let escaped = chars.next()?;
                out.push(escaped);
                at += 1 + escaped.len_utf8();
                continue;
            }
            if rest.starts_with("$(") || rest.starts_with('`') {
                return None;
            }
            let ch = rest.chars().next()?;
            out.push(ch);
            at += ch.len_utf8();
            if ch == '"' {
                in_double = false;
            }
            continue;
        }
        if rest.starts_with('\\') {
            let mut chars = rest.chars();
            out.push(chars.next()?);
            let escaped = chars.next()?;
            out.push(escaped);
            at += 1 + escaped.len_utf8();
            continue;
        }
        if rest.starts_with("$(")
            || rest.starts_with('`')
            || rest.starts_with("<(")
            || rest.starts_with(">(")
        {
            return None;
        }
        if let Some(matched) = SAFE_REDIRECTS.iter().find(|&&safe| {
            rest.strip_prefix(safe).is_some_and(|after| {
                after.is_empty()
                    || after.starts_with(|c: char| {
                        c.is_whitespace() || matches!(c, ';' | '|' | '&' | ')' | '\n')
                    })
            })
        }) {
            let preceded_by_word = at
                .checked_sub(1)
                .and_then(|prev| bytes.get(prev))
                .is_some_and(|prev| {
                    !matches!(
                        prev,
                        b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b'(' | b')'
                    )
                });
            if !preceded_by_word || matched.starts_with(['>', '&']) {
                out.push(' ');
                at += matched.len();
                continue;
            }
        }
        if rest.starts_with('>') {
            return None;
        }
        let ch = rest.chars().next()?;
        if ch == '\'' {
            in_single = true;
        } else if ch == '"' {
            in_double = true;
        }
        out.push(ch);
        at += ch.len_utf8();
    }
    if in_single || in_double {
        return None;
    }
    Some(out)
}

fn command_reads(words: &[String]) -> bool {
    let starts = (words.iter())
        .take_while(|word| {
            matches!(
                word.as_str(),
                "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "!"
            )
        })
        .count();
    let words = words.get(starts..).unwrap_or_default();
    let words = match command_words(words) {
        Unwrapped::None => return true,
        Unwrapped::UnknownWrapperFlag { .. } => return false,
        Unwrapped::Command { words, .. } => words,
    };
    if let Some(script) = shell_script(&words) {
        return script_reads(script);
    }
    let Some((program, args)) = words.split_first() else {
        return true;
    };
    program.starts_with('#') || program_reads(base_name(program), args)
}

/// Programs that only read, whatever they are given.
const READS: &[&str] = &[
    ":",
    "[",
    "[[",
    "b2sum",
    "basename",
    "cat",
    "cd",
    "cksum",
    "cmp",
    "comm",
    "column",
    "cut",
    "df",
    "diff",
    "dig",
    "dirname",
    "done",
    "du",
    "echo",
    "egrep",
    "esac",
    "exit",
    "export",
    "false",
    "fgrep",
    "fi",
    "file",
    "for",
    "free",
    "getent",
    "grep",
    "head",
    "hexdump",
    "host",
    "id",
    "jq",
    "last",
    "ls",
    "lsblk",
    "lscpu",
    "lsof",
    "md5sum",
    "netstat",
    "nl",
    "nproc",
    "nslookup",
    "od",
    "pgrep",
    "ping",
    "printenv",
    "printf",
    "ps",
    "pwd",
    "readlink",
    "realpath",
    "return",
    "rg",
    "set",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "sleep",
    "ss",
    "stat",
    "strings",
    "tac",
    "tail",
    "test",
    "tr",
    "true",
    "type",
    "uname",
    "unset",
    "uptime",
    "w",
    "wait",
    "wc",
    "which",
    "who",
    "whoami",
    "zcat",
    "zgrep",
];

fn git_reads(args: &[String]) -> bool {
    let sub = git_args(args);
    let global = args.get(..args.len() - sub.len()).unwrap_or_default();
    let unsafe_global = global.iter().any(|arg| {
        arg == "-c"
            || (arg.starts_with("-c") && !arg.starts_with("--"))
            || arg == "--config-env"
            || arg.starts_with("--config-env=")
            || arg == "--exec-path"
            || arg.starts_with("--exec-path=")
            || arg == "--upload-pack"
            || arg.starts_with("--upload-pack=")
    });
    if unsafe_global {
        return false;
    }
    let unsafe_sub = sub.iter().take_while(|arg| *arg != "--").any(|arg| {
        arg == "--ext-diff"
            || arg == "--upload-pack"
            || arg.starts_with("--upload-pack=")
            || arg == "--exec-path"
            || arg.starts_with("--exec-path=")
    });
    if unsafe_sub {
        return false;
    }
    sub.first().is_some_and(|does| {
        matches!(
            does.as_str(),
            "status" | "log" | "diff" | "show" | "rev-parse" | "describe" | "ls-files"
        )
    })
}

fn program_reads(program: &str, args: &[String]) -> bool {
    let first = (args.iter())
        .find(|arg| !arg.starts_with('-'))
        .map_or("", String::as_str);
    let any = |flags: &[&str]| has_flag(args, flags);
    match program {
        "find" => !args.iter().any(|arg| {
            ["-delete", "-exec", "-ok", "-fprint", "-fls"]
                .iter()
                .any(|writes| arg.starts_with(writes))
        }),
        "sort" => !args
            .iter()
            .any(|arg| arg.starts_with("-o") || arg.starts_with("--output")),
        "date" => !any(&["-s", "--set"]),
        "hostname" => first.is_empty(),
        "crontab" => matches!(args, [list] if list == "-l"),
        "journalctl" => !args.iter().any(|arg| {
            ["--vacuum", "--rotate", "--flush", "--sync", "--relinquish"]
                .iter()
                .any(|writes| arg.starts_with(writes))
        }),
        "systemctl" => {
            matches!(
                first,
                "status" | "show" | "cat" | "get-default" | "show-environment"
            ) || first.starts_with("is-")
                || first.starts_with("list-")
        }
        "docker" | "podman" => match first {
            "compose" | "container" | "image" | "volume" | "network" => {
                let does = (args.iter())
                    .skip_while(|arg| *arg != first)
                    .skip(1)
                    .find(|arg| !arg.starts_with('-'));
                does.is_some_and(|does| {
                    matches!(
                        does.as_str(),
                        "ps" | "ls" | "logs" | "inspect" | "top" | "stats"
                    )
                })
            }
            first => matches!(
                first,
                "ps" | "logs"
                    | "inspect"
                    | "images"
                    | "stats"
                    | "top"
                    | "port"
                    | "version"
                    | "info"
                    | "events"
                    | "diff"
                    | "history"
            ),
        },
        "git" => git_reads(args),
        "git-upload-pack" => true,
        "kubectl" | "helm" => !changes_cluster(program, args),
        "psql" | "mysql" => only_queries(args),
        "curl" => curl_reads(args),
        _ => READS.contains(&program),
    }
}

/// Whether curl only gets, whatever host it is pointed at, and keeps no file.
fn curl_reads(args: &[String]) -> bool {
    let sends = has_flag(args, CURL_DATA)
        || has_flag(
            args,
            &["-X", "--request", "-O", "--remote-name", "-K", "--config"],
        )
        || args.iter().any(|arg| {
            !arg.starts_with("--")
                && arg.starts_with('-')
                && arg.contains(['d', 'F', 'T', 'X', 'O', 'K'])
        });
    let saved_to = flag_value(args, &["-o", "--output"]).or_else(|| {
        let at = (args.iter())
            .position(|arg| !arg.starts_with("--") && arg.starts_with('-') && arg.contains('o'))?;
        Some(args.get(at + 1).cloned().unwrap_or_default())
    });
    !sends && saved_to.is_none_or(|to| to == "/dev/null" || to == "-")
}
