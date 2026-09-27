import { IS_WINDOWS } from "../lib/platform";

export const SSH_MAX_RETRIES = 5;

function shellQuote(value: string): string {
    return `'${value.replaceAll("'", `'"'"'`)}'`;
}

/** Reads the same in POSIX shells and in fish, where `\\` and `\'` are escapes even inside single quotes. */
function portableQuote(value: string): string {
    return `'${value.replace(/['\\]/g, (character) => (character === "'" ? `'"'"'` : `'"\\\\"'`))}'`;
}

function powershellQuote(value: string): string {
    return `'${value.replaceAll("'", "''")}'`;
}

/**
 * Run SSH through the user's local shell instead of leaving a dead transport
 * as the terminal's last state. A normal logout returns straight to the shell;
 * failed connections restore terminal modes and retry at most five times.
 * The loop runs under /bin/sh so it works whatever the login shell is.
 */
function unixSshStartup(alias: string): string {
    return `/bin/sh -c ${portableQuote(unixSshLoop(alias))}`;
}

function unixSshLoop(alias: string): string {
    const host = shellQuote(alias);
    return `sikemux_ssh_retries=0; while :; do command ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 ${host}; sikemux_ssh_status=$?; stty sane 2>/dev/null || true; printf '\\033[0m\\033[?25h\\033[?1l\\033>\\033[?2004l\\033[?1000l\\033[?1002l\\033[?1003l\\033[?1006l'; if [ "$sikemux_ssh_status" -eq 0 ]; then break; fi; if [ "$sikemux_ssh_status" -eq 130 ]; then printf '\\r\\nSSH reconnect cancelled. Back at your local shell.\\r\\n'; break; fi; if [ "$sikemux_ssh_retries" -ge ${SSH_MAX_RETRIES} ]; then printf '\\r\\nSSH could not reconnect after ${SSH_MAX_RETRIES} retries. Back at your local shell.\\r\\n'; break; fi; sikemux_ssh_retries=$((sikemux_ssh_retries + 1)); printf '\\r\\nSSH connection lost. Retrying (%s/${SSH_MAX_RETRIES}) in 3 seconds… Press Ctrl-C to stop.\\r\\n' "$sikemux_ssh_retries"; sleep 3 || break; done`;
}

function windowsSshStartup(alias: string): string {
    const host = powershellQuote(alias);
    // Windows PowerShell 5.1 has no `e escape, so build ANSI resets with
    // [char]27. Keep the program on one line for predictable -Command parsing.
    const reset =
        "[Console]::Write(([char]27 + '[0m' + [char]27 + '[?25h' + [char]27 + '[?1l' + [char]27 + '>' + [char]27 + '[?2004l' + [char]27 + '[?1000l' + [char]27 + '[?1002l' + [char]27 + '[?1003l' + [char]27 + '[?1006l'))";
    return `$sikemuxSshRetries = 0; while ($true) { & ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 ${host}; $sikemuxSshStatus = $LASTEXITCODE; ${reset}; if ($sikemuxSshStatus -eq 0) { break }; if ($sikemuxSshStatus -eq 130) { Write-Host 'SSH reconnect cancelled. Back at your local shell.'; break }; if ($sikemuxSshRetries -ge ${SSH_MAX_RETRIES}) { Write-Host 'SSH could not reconnect after ${SSH_MAX_RETRIES} retries. Back at your local shell.'; break }; $sikemuxSshRetries++; Write-Host "SSH connection lost. Retrying ($sikemuxSshRetries/${SSH_MAX_RETRIES}) in 3 seconds... Press Ctrl-C to stop."; Start-Sleep -Seconds 3 }`;
}

export function sshStartup(alias: string, platform: "unix" | "windows" = IS_WINDOWS ? "windows" : "unix"): string {
    return platform === "windows" ? windowsSshStartup(alias) : unixSshStartup(alias);
}
