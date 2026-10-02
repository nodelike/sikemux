import { describe, expect, it } from "vitest";
import { RESUME_WINDOW_MS } from "../chat/sessionRecovery";
import { afterAgentExit, describeAgentExit, readAgentExit, type AgentProcessExit } from "./tuiRecovery";

const exit = (code: number | null, signal: string | null = null, killed = false): AgentProcessExit => ({ code, signal, killed });

describe("readAgentExit", () => {
    it("reads exit 0 as the person quitting: /exit, /quit, Ctrl-C twice or Ctrl-D in Claude Code, Codex, OpenCode, Pi, OMP, Grok and Hermes", () => {
        expect(readAgentExit(exit(0))).toBe("quit");
    });

    it("reads the person's own Ctrl-C or Ctrl-\\ as quitting, as a signal or as the 130 and 131 a CLI or shell exits with after catching one", () => {
        expect(readAgentExit(exit(1, "Interrupt: 2"))).toBe("quit");
        expect(readAgentExit(exit(1, "Interrupt"))).toBe("quit");
        expect(readAgentExit(exit(1, "Quit: 3"))).toBe("quit");
        expect(readAgentExit(exit(130))).toBe("quit");
        expect(readAgentExit(exit(131))).toBe("quit");
    });

    it("reads any other signal as a crash: SIGKILL from the OOM killer, SIGSEGV, SIGABRT from a panic, SIGTERM or SIGHUP from outside", () => {
        expect(readAgentExit(exit(1, "Killed: 9"))).toBe("crashed");
        expect(readAgentExit(exit(1, "Segmentation fault: 11"))).toBe("crashed");
        expect(readAgentExit(exit(1, "Abort trap: 6"))).toBe("crashed");
        expect(readAgentExit(exit(1, "Terminated: 15"))).toBe("crashed");
        expect(readAgentExit(exit(1, "Hangup"))).toBe("crashed");
    });

    it("reads a failing code as a crash: 1 from an uncaught error, 101 from a Rust panic, 143 and 129 from Claude Code and OMP catching SIGTERM and SIGHUP", () => {
        for (const code of [1, 2, 101, 129, 137, 139, 143]) expect(readAgentExit(exit(code))).toBe("crashed");
    });

    it("reads an exit whose status could not be read as a crash", () => {
        expect(readAgentExit(exit(null))).toBe("crashed");
    });

    it("reads 126 and 127 as a CLI the shell could not run", () => {
        expect(readAgentExit(exit(126))).toBe("unstartable");
        expect(readAgentExit(exit(127))).toBe("unstartable");
    });

    it("reads anything Sikemux asked for as killed, whatever the process said", () => {
        expect(readAgentExit(exit(1, "Hangup", true))).toBe("killed");
        expect(readAgentExit(exit(143, null, true))).toBe("killed");
        expect(readAgentExit(exit(0, null, true))).toBe("killed");
    });
});

describe("afterAgentExit", () => {
    const base = { exit: exit(1, "Killed: 9"), resumeId: "session-1", lastResumeAt: null, now: 100_000 };

    it("resumes an agent with a saved conversation that crashed", () => {
        expect(afterAgentExit(base)).toBe("resume");
        expect(afterAgentExit({ ...base, exit: exit(1) })).toBe("resume");
        expect(afterAgentExit({ ...base, exit: exit(null) })).toBe("resume");
    });

    it("never resumes a process Sikemux ended", () => {
        expect(afterAgentExit({ ...base, exit: exit(1, "Hangup", true) })).toBe("leave");
        expect(afterAgentExit({ ...base, exit: exit(1, "Hangup", true), lastResumeAt: base.now - 1_000 })).toBe("leave");
    });

    it("never resumes an agent the person quit", () => {
        expect(afterAgentExit({ ...base, exit: exit(0) })).toBe("leave");
        expect(afterAgentExit({ ...base, exit: exit(130) })).toBe("leave");
        expect(afterAgentExit({ ...base, exit: exit(0), lastResumeAt: base.now - 1_000 })).toBe("leave");
    });

    it("never resumes an agent with no saved conversation", () => {
        expect(afterAgentExit({ ...base, resumeId: undefined })).toBe("leave");
    });

    it("leaves a CLI the shell could not run", () => {
        expect(afterAgentExit({ ...base, exit: exit(127) })).toBe("leave");
    });

    it("gives up on a second death within 30 seconds of the last resume, including a resume that could not start", () => {
        expect(afterAgentExit({ ...base, lastResumeAt: base.now - RESUME_WINDOW_MS + 1 })).toBe("give-up");
        expect(afterAgentExit({ ...base, exit: exit(127), lastResumeAt: base.now - 500 })).toBe("give-up");
        expect(afterAgentExit({ ...base, exit: exit(1), resumeId: undefined, lastResumeAt: base.now - 500 })).toBe("give-up");
    });

    it("resumes again once the last resume is more than 30 seconds old", () => {
        expect(afterAgentExit({ ...base, lastResumeAt: base.now - RESUME_WINDOW_MS })).toBe("resume");
    });
});

describe("describeAgentExit", () => {
    it("says how the process ended", () => {
        expect(describeAgentExit(exit(1, "Segmentation fault: 11"))).toBe("The agent was stopped by Segmentation fault: 11.");
        expect(describeAgentExit(exit(1))).toBe("The agent exited with code 1.");
        expect(describeAgentExit(exit(null))).toBe("The agent stopped and its exit status could not be read.");
    });
});
