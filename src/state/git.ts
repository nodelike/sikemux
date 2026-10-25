import { emit } from "./bus";
import type { GitCheatsheetSection, GitMenuItem, GitPromptSuggestion } from "./gitTypes";
import { reportError } from "./toast";
import { errorText } from "@shared/lib/format";
import { getState, setState } from "./store";

function currentPaneId(): string | null {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return null;
    return st.windows[session.activeWindowId]?.activePaneId ?? null;
}

export function openGitMenu(title: string, items: GitMenuItem[]): void {
    setState({ gitModal: { ownerPaneId: currentPaneId(), kind: "menu", title, items } });
}

export function openGitConfirm(opts: {
    title: string;
    body: string;
    confirmLabel?: string;
    cancelLabel?: string;
    destructive?: boolean;
    initialFocus?: "confirm" | "cancel";
    confirmKey?: string;
    onConfirm: () => void | Promise<void>;
}): void {
    setState({
        gitModal: {
            ownerPaneId: currentPaneId(),
            kind: "confirm",
            title: opts.title,
            body: opts.body,
            confirmLabel: opts.confirmLabel,
            cancelLabel: opts.cancelLabel,
            destructive: opts.destructive,
            initialFocus: opts.initialFocus,
            confirmKey: opts.confirmKey,
            onConfirm: opts.onConfirm,
        },
    });
}

export function openGitPrompt(opts: {
    title: string;
    placeholder?: string;
    initial?: string;
    multiline?: boolean;
    suggestions?: GitPromptSuggestion[];
    onConfirm: (value: string) => void | Promise<void>;
}): void {
    setState({
        gitModal: {
            ownerPaneId: currentPaneId(),
            kind: "prompt",
            title: opts.title,
            placeholder: opts.placeholder,
            initial: opts.initial,
            multiline: opts.multiline,
            suggestions: opts.suggestions,
            onConfirm: opts.onConfirm,
        },
    });
}

export function openGitCheatsheet(title: string, sections: GitCheatsheetSection[]): void {
    setState({ gitModal: { ownerPaneId: currentPaneId(), kind: "cheatsheet", title, sections } });
}

export function closeGitModal(): void {
    setState({ gitModal: null });
}

const LOG_LIMIT = 200;
let nextLogId = 1;

function pushLogEntry(label: string): number {
    const id = nextLogId++;
    setState((st) => {
        const entry = { id, ts: Date.now(), label, status: "running" as const };
        const next = [...st.gitCmdLog, entry];
        return {
            gitCmdLog: next.length > LOG_LIMIT ? next.slice(-LOG_LIMIT) : next,
        };
    });
    return id;
}

function patchLogEntry(id: number, patch: { status: "ok" | "error"; detail?: string }): void {
    setState((st) => ({
        gitCmdLog: st.gitCmdLog.map((e) => (e.id === id ? { ...e, ...patch } : e)),
    }));
}

export function toggleGitCmdLog(): void {
    setState((st) => ({ gitCmdLogOpen: !st.gitCmdLogOpen }));
}

export function clearGitCmdLog(): void {
    setState({ gitCmdLog: [] });
}

export async function runGitCmd<T>(label: string, fn: () => Promise<T>, opts?: { showError?: boolean; repo?: string | null }): Promise<T> {
    const id = pushLogEntry(label);
    try {
        const out = await fn();
        const detail = typeof out === "string" ? (out.length > 4096 ? out.slice(0, 4096) + "…" : out) : undefined;
        patchLogEntry(id, { status: "ok", detail });
        if (opts?.repo) emit({ type: "git-refresh", repo: opts.repo });
        return out;
    } catch (e) {
        const msg = errorText(e);
        patchLogEntry(id, {
            status: "error",
            detail: msg.length > 4096 ? msg.slice(0, 4096) + "…" : msg,
        });
        if (opts?.showError !== false) reportError(label)(e);
        throw e;
    }
}

export function dispatchGitMenuKey(k: string): boolean {
    const m = getState().gitModal;
    if (!m || m.kind !== "menu") return false;
    const item = m.items.find((i) => i.key === k);
    if (!item || item.disabled) return false;
    closeGitModal();
    void item.run();
    return true;
}
