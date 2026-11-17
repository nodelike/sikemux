//! The command an SSH terminal runs.

use sikemux_pty::shell_protocol::{ssh_link_word, SshLink, SSH_LINK_OSC};

/// How many times a dropped connection is retried before the terminal goes
/// back to the local shell.
const RETRIES: &str = "5";

/// The command an ssh terminal runs, through the person's own shell: ssh to
/// `alias`, retried a few times when the connection drops, then back to the
/// local shell. It reports where the connection stands for the rail.
pub fn startup(alias: &str) -> String {
    if cfg!(windows) {
        windows_startup(alias)
    } else {
        format!("/bin/sh -c {}", portable_quote(&unix_loop(alias)))
    }
}

fn posix_quote(value: &str) -> String {
    sikemux_process::single_quoted(value)
}

/// Reads the same in POSIX shells and in fish, where `\\` and `\'` are
/// escapes even inside single quotes.
fn portable_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        match character {
            '\'' => quoted.push_str(r#"'"'"'"#),
            '\\' => quoted.push_str(r#"'"\\"'"#),
            character => quoted.push(character),
        }
    }
    quoted.push('\'');
    quoted
}

fn unix_loop(alias: &str) -> String {
    const LOOP: &str = r#"sikemux_ssh_link() { printf '\033]@OSC@%s\007' "$1"; }; trap 'sikemux_ssh_link @DISCONNECTED@' EXIT; trap 'exit 130' INT TERM HUP; sikemux_ssh_link @CONNECTING@; sikemux_ssh_retries=0; while :; do command ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o PermitLocalCommand=yes -o "LocalCommand=/usr/bin/printf '\033]@OSC@@CONNECTED@\007'" @HOST@; sikemux_ssh_status=$?; stty sane 2>/dev/null || true; printf '\033[0m\033[?25h\033[?1l\033>\033[?2004l\033[?1000l\033[?1002l\033[?1003l\033[?1006l'; if [ "$sikemux_ssh_status" -eq 0 ]; then break; fi; if [ "$sikemux_ssh_status" -eq 130 ]; then printf '\r\nSSH reconnect cancelled. Back at your local shell.\r\n'; break; fi; if [ "$sikemux_ssh_retries" -ge @RETRIES@ ]; then printf '\r\nSSH could not reconnect after @RETRIES@ retries. Back at your local shell.\r\n'; break; fi; sikemux_ssh_retries=$((sikemux_ssh_retries + 1)); printf '\r\nSSH connection lost. Retrying (%s/@RETRIES@) in 3 seconds… Press Ctrl-C to stop.\r\n' "$sikemux_ssh_retries"; sikemux_ssh_link @CONNECTING@; sleep 3 || break; done"#;
    LOOP.replace("@OSC@", SSH_LINK_OSC)
        .replace("@CONNECTING@", ssh_link_word(SshLink::Connecting))
        .replace("@CONNECTED@", ssh_link_word(SshLink::Connected))
        .replace("@DISCONNECTED@", ssh_link_word(SshLink::Disconnected))
        .replace("@RETRIES@", RETRIES)
        .replacen("@HOST@", &posix_quote(alias), 1)
}

fn windows_startup(alias: &str) -> String {
    const LOOP: &str = r#"$sikemuxSshRetries = 0; while ($true) { & ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 @HOST@; $sikemuxSshStatus = $LASTEXITCODE; [Console]::Write(([char]27 + '[0m' + [char]27 + '[?25h' + [char]27 + '[?1l' + [char]27 + '>' + [char]27 + '[?2004l' + [char]27 + '[?1000l' + [char]27 + '[?1002l' + [char]27 + '[?1003l' + [char]27 + '[?1006l')); if ($sikemuxSshStatus -eq 0) { break }; if ($sikemuxSshStatus -eq 130) { Write-Host 'SSH reconnect cancelled. Back at your local shell.'; break }; if ($sikemuxSshRetries -ge @RETRIES@) { Write-Host 'SSH could not reconnect after @RETRIES@ retries. Back at your local shell.'; break }; $sikemuxSshRetries++; Write-Host "SSH connection lost. Retrying ($sikemuxSshRetries/@RETRIES@) in 3 seconds... Press Ctrl-C to stop."; Start-Sleep -Seconds 3 }"#;
    let host = format!("'{}'", alias.replace('\'', "''"));
    LOOP.replace("@RETRIES@", RETRIES)
        .replacen("@HOST@", &host, 1)
}
