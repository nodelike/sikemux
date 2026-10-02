import { RESUME_WINDOW_MS } from "../chat/sessionRecovery";

/** How a terminal agent's process ended, as the core reports it. */
export interface AgentProcessExit {
    readonly code: number | null;
    readonly signal: string | null;
    /** Sikemux asked for it to end: a close, sleep, switch to chat, restart or Quit and Stop Everything. */
    readonly killed: boolean;
}

export type AgentExitReading = "killed" | "quit" | "unstartable" | "crashed";

/* macOS names a signal "Interrupt: 2", Linux "Interrupt". */
const SIGNAL_NUMBERS: Readonly<Record<string, number>> = { Interrupt: 2, Quit: 3 };

/* The person's own Ctrl-C and Ctrl-\ reach a CLI as SIGINT and SIGQUIT. A CLI
   or shell that catches one exits with 128 plus its number instead. */
const PERSON_SIGNALS: ReadonlySet<number> = new Set([2, 3]);
const SIGNAL_EXIT_BASE = 128;

function signalNumber(signal: string): number | null {
    const trailing = /(\d+)\s*$/.exec(signal);
    if (trailing) return Number(trailing[1]);
    return SIGNAL_NUMBERS[signal.split(":")[0].trim()] ?? null;
}

export function readAgentExit(exit: AgentProcessExit): AgentExitReading {
    if (exit.killed) return "killed";
    if (exit.signal !== null) {
        const number = signalNumber(exit.signal);
        return number !== null && PERSON_SIGNALS.has(number) ? "quit" : "crashed";
    }
    if (exit.code === null) return "crashed";
    if (exit.code === 0) return "quit";
    if (exit.code === 126 || exit.code === 127) return "unstartable";
    if (exit.code > SIGNAL_EXIT_BASE && PERSON_SIGNALS.has(exit.code - SIGNAL_EXIT_BASE)) return "quit";
    return "crashed";
}

/* The same rules as a chat's: a second death soon after a resume is a crash
   loop, so the choice goes to the person. */
export function afterAgentExit({
    exit,
    resumeId,
    lastResumeAt,
    now,
}: {
    exit: AgentProcessExit;
    resumeId: string | undefined;
    lastResumeAt: number | null;
    now: number;
}): "resume" | "give-up" | "leave" {
    const reading = readAgentExit(exit);
    if (reading === "killed" || reading === "quit") return "leave";
    if (lastResumeAt !== null && now - lastResumeAt < RESUME_WINDOW_MS) return "give-up";
    if (reading === "unstartable" || !resumeId) return "leave";
    return "resume";
}

export function describeAgentExit(exit: AgentProcessExit): string {
    if (exit.signal !== null) return `The agent was stopped by ${exit.signal}.`;
    if (exit.code === null) return "The agent stopped and its exit status could not be read.";
    return `The agent exited with code ${exit.code}.`;
}
