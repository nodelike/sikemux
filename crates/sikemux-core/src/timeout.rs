//! `timeout` as GNU coreutils has it, which macOS lacks and agents reach for:
//! runs a command, signals its process group once the time is up, and exits
//! 124 when it did.

use std::ffi::OsString;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TIMED_OUT: i32 = 124;
const FAILED: i32 = 125;
const CANNOT_RUN: i32 = 126;
const NOT_FOUND: i32 = 127;

const USAGE: &str =
    "usage: timeout [-s SIGNAL] [-k DURATION] [--preserve-status] [--foreground] DURATION COMMAND [ARG]...";

pub fn run(args: Vec<OsString>) -> i32 {
    match parse(args) {
        Ok(asked) => asked.run(),
        Err(message) => {
            eprintln!("timeout: {message}\n{USAGE}");
            FAILED
        }
    }
}

struct Asked {
    signal: i32,
    kill_after: Option<Duration>,
    preserve: bool,
    foreground: bool,
    limit: Duration,
    command: Vec<OsString>,
}

fn parse(args: Vec<OsString>) -> Result<Asked, String> {
    let mut args = args.into_iter();
    let (mut signal, mut kill_after, mut preserve, mut foreground) =
        (libc::SIGTERM, None, false, false);
    let limit = loop {
        let arg = args.next().ok_or("no duration given")?;
        let text = arg.to_string_lossy().into_owned();
        let (flag, attached) = match text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (text.clone(), None),
        };
        let mut value = |name: &str| {
            attached
                .clone()
                .or_else(|| {
                    args.next()
                        .map(|value| value.to_string_lossy().into_owned())
                })
                .ok_or(format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-s" | "--signal" => signal = signal_of(&value("--signal")?)?,
            "-k" | "--kill-after" => kill_after = Some(duration(&value("--kill-after")?)?),
            "--preserve-status" => preserve = true,
            "--foreground" => foreground = true,
            "-v" | "--verbose" => {}
            _ if flag.starts_with("-s") && flag.len() > 2 => signal = signal_of(&flag[2..])?,
            _ if flag.starts_with("-k") && flag.len() > 2 => {
                kill_after = Some(duration(&flag[2..])?)
            }
            _ => break duration(&text)?,
        }
    };
    let command: Vec<OsString> = args.collect();
    if command.is_empty() {
        return Err("no command given".into());
    }
    Ok(Asked {
        signal,
        kill_after,
        preserve,
        foreground,
        limit,
        command,
    })
}

/// `10`, `1.5s`, `2m`, `1h` or `1d`; zero means no limit.
fn duration(text: &str) -> Result<Duration, String> {
    let (number, unit) = match text.strip_suffix(['s', 'm', 'h', 'd']) {
        Some(number) => (number, text.chars().last().unwrap_or('s')),
        None => (text, 's'),
    };
    let seconds: f64 = number
        .parse()
        .ok()
        .filter(|seconds: &f64| seconds.is_finite() && *seconds >= 0.0)
        .ok_or(format!("invalid time interval {text:?}"))?;
    let scale = match unit {
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => 1.0,
    };
    Ok(Duration::from_secs_f64(seconds * scale))
}

fn signal_of(name: &str) -> Result<i32, String> {
    if let Ok(number) = name.parse() {
        return Ok(number);
    }
    let name = name.trim_start_matches("SIG").to_ascii_uppercase();
    Ok(match name.as_str() {
        "TERM" => libc::SIGTERM,
        "KILL" => libc::SIGKILL,
        "INT" => libc::SIGINT,
        "HUP" => libc::SIGHUP,
        "QUIT" => libc::SIGQUIT,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        _ => return Err(format!("{name:?} is not a signal")),
    })
}

impl Asked {
    fn run(self) -> i32 {
        let mut command = sikemux_process::user_environment::command(&self.command[0]);
        command.args(&self.command[1..]);
        if !self.foreground {
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "timeout: {}: command not found",
                    self.command[0].to_string_lossy()
                );
                return NOT_FOUND;
            }
            Err(error) => {
                eprintln!("timeout: {}: {error}", self.command[0].to_string_lossy());
                return CANNOT_RUN;
            }
        };
        let pid = child.id() as i32;
        let target = if self.foreground { pid } else { -pid };
        let fired = Arc::new(AtomicBool::new(false));
        if !self.limit.is_zero() {
            let fired = fired.clone();
            let (limit, signal, kill_after) = (self.limit, self.signal, self.kill_after);
            std::thread::spawn(move || {
                std::thread::sleep(limit);
                fired.store(true, Ordering::Release);
                // SAFETY: kill only sends a signal to the child or its group.
                unsafe { libc::kill(target, signal) };
                if let Some(after) = kill_after {
                    std::thread::sleep(after);
                    // SAFETY: as above.
                    unsafe { libc::kill(target, libc::SIGKILL) };
                }
            });
        }
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                eprintln!("timeout: {error}");
                return FAILED;
            }
        };
        let code = status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
        match fired.load(Ordering::Acquire) && !self.preserve {
            true => TIMED_OUT,
            false => code,
        }
    }
}
