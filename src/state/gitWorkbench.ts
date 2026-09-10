import { create } from "zustand";
import { persist } from "zustand/middleware";
import { git } from "../api/git";
import { runGitCmd } from "./git";
import { errMessage } from "./toast";
import { DEFAULT_AI_PROVIDER, defaultAiModel } from "../components/git/gitPaneConstants";
import type { GitAiProvider } from "../components/git/gitPaneTypes";

interface Operation {
    label: string;
    busy: boolean;
    error: string | null;
    result: string | null;
}

interface GitWorkbenchState {
    drafts: Record<string, string>;
    operations: Record<string, Operation>;
    provider: GitAiProvider;
    model: string;
}

export const useGitWorkbench = create<GitWorkbenchState>()(
    persist(() => ({ drafts: {}, operations: {}, provider: DEFAULT_AI_PROVIDER, model: defaultAiModel(DEFAULT_AI_PROVIDER) }), {
        name: "sikemux.git.workbench",
        partialize: ({ drafts, provider, model }) => ({ drafts, provider, model }),
    }),
);

export function setGitDraft(repo: string, value: string | ((current: string) => string)): void {
    useGitWorkbench.setState((state) => ({
        drafts: { ...state.drafts, [repo]: typeof value === "function" ? value(state.drafts[repo] ?? "") : value },
    }));
}

export function setGitProvider(provider: GitAiProvider): void {
    useGitWorkbench.setState({ provider, model: defaultAiModel(provider) });
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
        update({ label, busy: false, error: errMessage(error), result: null });
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
        const message = await git.aiMessage(repo, provider, model, () => {});
        if (!message.trim()) throw new Error("The provider returned an empty message. Your draft was kept.");
        if ((useGitWorkbench.getState().drafts[repo] ?? "") !== draft)
            throw new Error("Your draft changed during generation. It was kept; generate again when ready.");
        setGitDraft(repo, message);
        return "Commit message generated. Review it before committing.";
    });
}
