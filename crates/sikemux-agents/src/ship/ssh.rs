//! What `ssh`, `scp`, `sftp` and `rsync` change on another computer.

use super::{changes_cluster, flag_value, has_flag, only_queries, CURL_DATA};
use crate::git_guard::{base_name, command_words, git_args, pieces, shell_script, Piece};

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
    let git_server = host.trim_start_matches("ssh://").starts_with("git@");
    (!reads && !git_server).then(|| host_of(host))
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
    args.iter().any(|arg| {
        arg.strip_prefix('-')
            .is_some_and(|letters| !letters.starts_with(['-', 'e']) && letters.contains('n'))
    })
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
    let script = ["2>&1", "1>&2", ">&2", "&>/dev/null", "&> /dev/null"]
        .iter()
        .fold(script.to_owned(), |script, quiet| {
            script.replace(quiet, " ")
        });
    pieces(&script).iter().all(|piece| match piece {
        Piece::Words(words) => command_reads(words),
        Piece::Open | Piece::Close => true,
    })
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
    if writes_a_file(words) || !substitutions_read(words) {
        return false;
    }
    let Some(words) = command_words(words) else {
        return true;
    };
    let words = past_sudo_options(words);
    if let Some(script) = shell_script(words) {
        return script_reads(script);
    }
    let Some((program, args)) = words.split_first() else {
        return true;
    };
    program.starts_with('#') || program_reads(base_name(program), args)
}

fn past_sudo_options(words: &[String]) -> &[String] {
    let mut at = 0;
    while let Some(option) = words.get(at).filter(|word| word.starts_with('-')) {
        at += match option.as_str() {
            "-u" | "-g" | "--user" | "--group" => 2,
            _ => 1,
        };
    }
    words.get(at..).unwrap_or_default()
}

/// Whether a command's output goes to a file. A word with spaces was quoted, so a `>` in it is
/// text.
fn writes_a_file(words: &[String]) -> bool {
    words.iter().enumerate().any(|(at, word)| {
        let to = match word.rsplit_once('>') {
            Some((_, to)) if !word.contains(char::is_whitespace) => to,
            _ => return false,
        };
        let to = match to.is_empty() {
            true => words.get(at + 1).map_or("", String::as_str),
            false => to,
        };
        to != "/dev/null"
    })
}

/// Whether the commands quoted inside `words`, as `"$(…)"`, only read.
fn substitutions_read(words: &[String]) -> bool {
    words.iter().all(|word| {
        let inner = word.split("$(").skip(1);
        let reads = inner
            .map(|text| text.split(')').next().unwrap_or(text))
            .all(script_reads);
        reads && !word.contains('`')
    })
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
    "xxd",
    "zcat",
    "zgrep",
];

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
        "sed" => !args.iter().any(|arg| {
            arg.starts_with("--in-place")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'))
        }),
        "awk" => !args.iter().any(|arg| arg.contains("system(")),
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
        "git" => git_args(args).first().is_some_and(|does| {
            matches!(
                does.as_str(),
                "status" | "log" | "diff" | "show" | "rev-parse" | "describe" | "ls-files"
            )
        }),
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
