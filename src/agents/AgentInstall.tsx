import { useEffect, useState } from "react";
import { create } from "zustand";
import { agentApi, type AgentInfo } from "../api/agents";
import { getIpcTransport } from "../api/transport";
import { fetchResource, invalidate } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { getState } from "../state/store";
import { swallow } from "../state/toast";
import { TerminalPane } from "../terminal/TerminalPane";
import { selectedAgentRuntimeProfiles } from "./agentProfiles";
import { agentStatusLabel } from "./agentStatus";
import { agentLabel, installCommand, loginArgs, type AgentType } from "./catalog";
import { basename, dirname, expandHome } from "../lib/paths";
import type { PtyDirectCommand } from "../state/types";

/** A sign-in the agent offered itself, in place of the catalog's. */
interface OfferedSignIn {
    args: readonly string[];
    env: readonly (readonly [string, string])[];
}

type Job =
    | { kind: "install"; type: AgentType }
    | {
          kind: "signIn";
          type: AgentType;
          program: string;
          configPath: string | null;
          method?: OfferedSignIn;
          /** The chat whose refused prompt goes again once the sign-in works. */
          retry?: string;
      };

const useJob = create<{ job: Job | null; run: number }>(() => ({ job: null, run: 0 }));

function open(job: Job): void {
    useJob.setState((now) => ({ job, run: now.run + 1 }));
}

/** Opens the sheet that runs the agent's own installer where the person can watch it. */
export function installAgent(type: AgentType): void {
    if (installCommand(type)) open({ kind: "install", type });
}

/** Opens the sheet that runs the agent's own sign-in, on the account its profile names. */
export function signInAgent(agent: Pick<AgentInfo, "type" | "command" | "configPath">, from?: { method?: OfferedSignIn; retry?: string }): void {
    if (!from?.method && !loginArgs(agent.type)) return;
    open({ kind: "signIn", type: agent.type, program: agent.command, configPath: agent.configPath ?? null, ...from });
}

function closeSheet(): void {
    useJob.setState({ job: null });
}

const ACCOUNT_VARIABLE: Partial<Record<AgentType, string>> = { claude: "CLAUDE_CONFIG_DIR", codex: "CODEX_HOME" };
const CONFIG_FILES = new Set(["config.toml", "config.yml", "settings.json", "settings.local.json"]);

/** The folder an account keeps its sign-in in: the path itself, or the folder of the config file it names. */
function accountDir(configPath: string, home: string): string {
    const path = expandHome(configPath.trim(), home);
    return CONFIG_FILES.has(basename(path)) ? dirname(path) : path;
}

/** What the sheet runs, and the words it shows for it. */
export function sheetCommand(job: Job, home: string): { direct: PtyDirectCommand; shown: string } {
    if (job.kind === "install") {
        const command = installCommand(job.type)!;
        return { direct: { program: "/bin/bash", args: ["-c", command] }, shown: command };
    }
    const args = [...(job.method?.args ?? loginArgs(job.type)!)];
    const shown = [job.type, ...args].join(" ");
    const variable = ACCOUNT_VARIABLE[job.type];
    const env = (job.method?.env ?? []).map(([name, value]) => `${name}=${value}`);
    if (variable && job.configPath) env.push(`${variable}=${accountDir(job.configPath, home)}`);
    if (env.length === 0) return { direct: { program: job.program, args }, shown };
    return { direct: { program: "/usr/bin/env", args: [...env, job.program, ...args] }, shown };
}

interface PtyExited {
    readonly id: number;
    readonly code: number | null;
}

type Phase = { kind: "running" } | { kind: "checking" } | { kind: "done"; agent: AgentInfo | undefined } | { kind: "failed"; code: number | null };

/** Forgets what was known about every agent and reads the list again, so a new install is found at once. */
export async function checkAgentsAgain(type: AgentType): Promise<AgentInfo | undefined> {
    await agentApi.refreshStatuses();
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.account");
    const { providerProfiles, selectedProviderProfileIds } = getState();
    const agents = await fetchResource(agentCatalogR, selectedAgentRuntimeProfiles(providerProfiles, selectedProviderProfileIds));
    return agents.find((agent) => agent.type === type && agent.available !== false);
}

export function AgentInstallHost() {
    const { job, run } = useJob();
    if (!job) return null;
    return <AgentSheet key={run} job={job} />;
}

function AgentSheet({ job }: { job: Job }) {
    const type = job.type;
    const label = agentLabel(type);
    const title = job.kind === "install" ? `Install ${label}` : `Sign in to ${label}`;
    const [{ direct, shown }] = useState(() => sheetCommand(job, getState().home));
    const [listening, setListening] = useState(false);
    const [ptyId, setPtyId] = useState<number | null>(null);
    const [exit, setExit] = useState<PtyExited | null>(null);
    const [phase, setPhase] = useState<Phase>({ kind: "running" });
    const [attempt, setAttempt] = useState(0);

    useEffect(() => {
        const controller = new AbortController();
        void getIpcTransport()
            .subscribe<PtyExited>("pty_exited", ({ payload }) => setExit(payload), { signal: controller.signal })
            .then(() => setListening(true))
            .catch(() => setListening(true));
        return () => controller.abort();
    }, []);

    useEffect(() => {
        if (!exit || exit.id !== ptyId || phase.kind !== "running") return;
        if (exit.code !== 0) {
            setPhase({ kind: "failed", code: exit.code });
            return;
        }
        setPhase({ kind: "checking" });
        if (job.kind === "signIn" && job.retry) void agentApi.retryFailed(job.retry).catch(swallow("send the prompt again"));
        void checkAgentsAgain(type)
            .then((agent) => setPhase({ kind: "done", agent }))
            .catch(() => setPhase({ kind: "done", agent: undefined }));
    }, [exit, ptyId, phase.kind, type, job]);

    const again = () => {
        setExit(null);
        setPtyId(null);
        setPhase({ kind: "running" });
        setAttempt((count) => count + 1);
    };

    return (
        <div className="experience-backdrop command-popup-backdrop" role="presentation">
            <section className="command-popup agent-install" role="dialog" aria-modal="true" aria-label={title}>
                <header>
                    <span>{title}</span>
                    <code className="agent-install-command">{shown}</code>
                </header>
                {listening ? <TerminalPane key={attempt} directCommand={direct} active visible onPtySession={setPtyId} /> : <div />}
                <footer>
                    <span className="agent-install-state" role="status">
                        {job.kind === "install" ? <InstallState phase={phase} label={label} /> : <SignInState phase={phase} label={label} />}
                    </span>
                    {phase.kind === "failed" && (
                        <button type="button" onClick={again}>
                            Try again
                        </button>
                    )}
                    <button type="button" onClick={closeSheet}>
                        {phase.kind === "running" ? "Cancel" : "Close"}
                    </button>
                </footer>
            </section>
        </div>
    );
}

function InstallState({ phase, label }: { phase: Phase; label: string }) {
    switch (phase.kind) {
        case "running":
            return <>Running {label}'s own installer…</>;
        case "checking":
            return <>Installed. Looking for {label}…</>;
        case "failed":
            return <>The installer stopped{phase.code === null ? "" : ` with exit code ${phase.code}`}. Its output above says why.</>;
        case "done":
            if (!phase.agent) return <>The installer finished, but Sikemux can't find {label} yet. Open a new terminal or restart Sikemux.</>;
            return (
                <>
                    {label} is installed: {agentStatusLabel(phase.agent.status) ?? "found"}.
                </>
            );
    }
}

function SignInState({ phase, label }: { phase: Phase; label: string }) {
    switch (phase.kind) {
        case "running":
            return <>Running {label}'s own sign-in. Finish it in the browser if one opened.</>;
        case "checking":
            return <>Checking {label}'s sign-in…</>;
        case "failed":
            return <>The sign-in stopped{phase.code === null ? "" : ` with exit code ${phase.code}`}. Its output above says why.</>;
        case "done":
            switch (phase.agent?.status?.state) {
                case "ready":
                    return (
                        <>
                            {label} is signed in: {agentStatusLabel(phase.agent.status)}.
                        </>
                    );
                case "signedOut":
                    return <>{label} still reads as signed out. Try again, or check the output above.</>;
                default:
                    return <>The sign-in finished. Start a {label} chat to use it.</>;
            }
    }
}
