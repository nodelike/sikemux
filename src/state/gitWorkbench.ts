import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { git } from "../api/git";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import type { Draft } from "@shared/wire.generated";
import { swallow } from "./toast";
import { runGitCmd } from "./git";
import { errorText } from "@shared/lib/format";
import { DEFAULT_AI_PROVIDER } from "../git/gitPaneConstants";
import type { AgentType } from "@shared/agents/catalog";

interface Operation {
    label: string;
    busy: boolean;
    error: string | null;
    result: string | null;
}

interface GitWorkbenchState {
    drafts: Record<string, string>;
    operations: Record<string, Operation>;
    provider: AgentType;
    model: string;
}

/** How long typing rests before a commit message is kept by the core. */
const DRAFT_PAUSE_MS = 300;

const draftKey = (repo: string) => `git:${repo}`;

/** Each repository's commit message as the core last kept it. */
const kept = new Map<string, string>();
const pausing = new Map<string, number>();

/**
 * Commit messages this app kept itself before the core kept drafts, held in its saved
 * state until the core has every one of them.
 */
let uncarried: Record<string, string> | null = null;

export const useGitWorkbench = create<GitWorkbenchState>()(
    persist(() => ({ drafts: {}, operations: {}, provider: DEFAULT_AI_PROVIDER, model: "" }), {
        name: "sikemux.git.workbench",
        partialize: ({ provider, model }) => ({ provider, model, ...(uncarried ? { drafts: uncarried } : {}) }),
        storage: createJSONStorage(() => localStorage),
        onRehydrateStorage: () => (state) => {
            const saved = Object.entries(state?.drafts ?? {}).filter(
                (entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].trim() !== "",
            );
            uncarried = saved.length ? Object.fromEntries(saved) : null;
        },
    }),
);

export function setGitDraft(repo: string, value: string | ((current: string) => string)): void {
    useGitWorkbench.setState((state) => ({
        drafts: { ...state.drafts, [repo]: typeof value === "function" ? value(state.drafts[repo] ?? "") : value },
    }));
    window.clearTimeout(pausing.get(repo));
    pausing.set(
        repo,
        window.setTimeout(() => {
            pausing.delete(repo);
            keepDraft(repo, useGitWorkbench.getState().drafts[repo] ?? "").catch(swallow("keep the commit message"));
        }, DRAFT_PAUSE_MS),
    );
}

function keepDraft(repo: string, text: string): Promise<void> {
    const before = kept.get(repo) ?? "";
    if (before === text) return Promise.resolve();
    kept.set(repo, text);
    return coreCall<void>("setDraft", { key: draftKey(repo), text }).catch((error: unknown) => {
        if (kept.get(repo) === text) kept.set(repo, before);
        throw error;
    });
}

/**
 * Takes the commit messages the core keeps. One follows the core's only while nothing
 * was typed here since they last agreed, so words typed here are never overwritten.
 */
function takeDrafts(drafts: Record<string, Draft>, asked: readonly string[] | null): void {
    const repos = new Set(asked ? [] : [...kept.keys()]);
    for (const key of asked ?? Object.keys(drafts)) if (key.startsWith("git:")) repos.add(key.slice(4));
    const current = useGitWorkbench.getState().drafts;
    const next = { ...current };
    let changed = false;
    for (const repo of repos) {
        const text = drafts[draftKey(repo)]?.text ?? "";
        const before = kept.get(repo) ?? "";
        if (before === text) continue;
        if ((current[repo] ?? "") === before) {
            next[repo] = text;
            changed = true;
        }
        kept.set(repo, text);
    }
    if (changed) useGitWorkbench.setState({ drafts: next });
}

/** Hands the core the commit messages this app kept itself, once each is taken. */
async function carry(): Promise<void> {
    if (!uncarried) return;
    await Promise.all(Object.entries(uncarried).map(([repo, text]) => keepDraft(repo, text)));
    uncarried = null;
    useGitWorkbench.setState({});
}

/** Keeps the commit messages in step with the drafts the core keeps. */
export function watchDrafts(): () => void {
    let alive = true;
    const read = (keys: readonly string[] | null) =>
        coreCall<Record<string, Draft>>("drafts", keys ? { keys } : null)
            .then((drafts) => alive && takeDrafts(drafts, keys))
            .catch(swallow("read the kept drafts"));
    void carry()
        .catch(swallow("hand the core the saved commit messages"))
        .then(() => read(null));
    const stop = onCoreChanged("drafts", (change) => {
        const keys = change.paths?.filter((key) => key.startsWith("git:")) ?? null;
        if (!keys || keys.length) void read(keys);
    });
    return () => {
        alive = false;
        stop();
    };
}

export async function runRepositoryGit(repo: string, label: string, action: () => Promise<unknown>): Promise<boolean> {
    if (useGitWorkbench.getState().operations[repo]?.busy) return false;
    const update = (operation: Operation) => useGitWorkbench.setState((state) => ({ operations: { ...state.operations, [repo]: operation } }));
    update({ label, busy: true, error: null, result: null });
    try {
        const result = await runGitCmd(label, action, { repo, showError: false });
        update({ label, busy: false, error: null, result: typeof result === "string" && result.trim() ? result : `${label} completed` });
        return true;
    } catch (error) {
        update({ label, busy: false, error: errorText(error), result: null });
        return false;
    }
}

export async function commitGitDraft(repo: string): Promise<boolean> {
    const draft = useGitWorkbench.getState().drafts[repo] ?? "";
    if (!draft.trim()) return false;
    return runRepositoryGit(repo, "Commit staged changes", async () => {
        const status = await git.status(repo);
        if (!status.files.some((file) => file.index !== " " && file.index !== "?")) throw new Error("Stage the changes you want to commit first.");
        const result = await git.commit(repo, draft.trim());
        if (useGitWorkbench.getState().drafts[repo] === draft) setGitDraft(repo, "");
        return result;
    });
}

export async function generateGitDraft(repo: string): Promise<boolean> {
    const { provider, model, drafts } = useGitWorkbench.getState();
    const draft = drafts[repo] ?? "";
    return runRepositoryGit(repo, `Generate message with ${provider}`, async () => {
        const message = await git.aiMessage(repo, provider, model);
        if (!message.trim()) throw new Error("The provider returned an empty message. Your draft was kept.");
        if ((useGitWorkbench.getState().drafts[repo] ?? "") !== draft)
            throw new Error("Your draft changed during generation. It was kept; generate again when ready.");
        setGitDraft(repo, message);
        return "Commit message generated. Review it before committing.";
    });
}
