import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SSH_MAX_RETRIES, sshStartup } from "./sshStartup";

const PREFIX = "/bin/sh -c ";

/** How fish reads a word made of single- and double-quoted runs. */
function fishWord(quoted: string): string {
    let word = "";
    let index = 0;
    while (index < quoted.length) {
        const quote = quoted[index];
        if (quote !== "'" && quote !== '"') throw new Error(`unquoted ${quote} in ${quoted}`);
        index += 1;
        while (quoted[index] !== quote) {
            const escapes = quote === "'" ? "'\\" : '"\\$';
            if (quoted[index] === "\\" && escapes.includes(quoted[index + 1])) index += 1;
            word += quoted[index];
            index += 1;
        }
        index += 1;
    }
    return word;
}

function loopOf(startup: string): string {
    expect(startup.startsWith(PREFIX)).toBe(true);
    return fishWord(startup.slice(PREFIX.length));
}

describe("sshStartup", () => {
    it("hands the loop to /bin/sh as one word that POSIX shells and fish read the same", () => {
        const startup = sshStartup("host'; touch nope; echo '\\");
        const posix = execFileSync("/bin/sh", ["-c", `printf %s ${startup.slice(PREFIX.length)}`], {
            encoding: "utf8",
        });
        expect(loopOf(startup)).toBe(posix);
        expect(posix).toContain("\\033[0m");
    });

    it("runs ssh with the alias as one argument from any login shell", () => {
        const directory = mkdtempSync(join(tmpdir(), "sikemux-ssh-"));
        const record = join(directory, "args");
        writeFileSync(join(directory, "ssh"), `#!/bin/sh\nprintf '%s\\n' "$@" > '${record}'\n`);
        chmodSync(join(directory, "ssh"), 0o755);
        const alias = "odd 'host' \\ $HOME";
        for (const shell of ["/bin/sh", "/bin/zsh", "/bin/bash"]) {
            execFileSync(shell, ["-c", sshStartup(alias)], {
                env: { ...process.env, PATH: `${directory}:${process.env.PATH}` },
                stdio: "ignore",
            });
            expect(execFileSync("cat", [record], { encoding: "utf8" }).split("\n").at(-2)).toBe(alias);
        }
    });

    it("uses keepalives and stops after five retries", () => {
        const startup = loopOf(sshStartup("prod-db"));

        expect(SSH_MAX_RETRIES).toBe(5);
        expect(startup).toContain("ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 'prod-db'");
        expect(startup).toContain('if [ "$sikemux_ssh_retries" -ge 5 ]; then');
        expect(startup).toContain("Retrying (%s/5)");
        expect(startup).toContain("sleep 3 || break");
        expect(startup).not.toMatch(/[\r\n]/);
    });

    it("quotes SSH aliases before passing them to the shell", () => {
        expect(loopOf(sshStartup("host'; touch nope; echo '"))).toContain("'host'\"'\"'; touch nope; echo '\"'\"''");
    });

    it("restores terminal input modes after every SSH exit", () => {
        const loop = loopOf(sshStartup("prod-db"));
        expect(loop).toContain("stty sane");
        expect(loop).toContain("\\033[?2004l");
        expect(loop).toContain("SSH reconnect cancelled");
    });

    it("builds a quoted PowerShell retry loop on Windows", () => {
        const startup = sshStartup("host'; Write-Host nope", "windows");
        expect(startup).toContain("& ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 'host''; Write-Host nope'");
        expect(startup).toContain("$LASTEXITCODE");
        expect(startup).toContain("Start-Sleep -Seconds 3");
        expect(startup).toContain("[char]27");
        expect(startup).not.toMatch(/[\r\n]/);
    });
});
