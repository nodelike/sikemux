import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { git, hasUnstaged, isStaged } from "../api/git";
import * as cmd from "../state/commands";
import { openGitCheatsheet, openGitConfirm, openGitMenu, openGitPrompt, toggleGitCmdLog } from "../state/git";
import { useResourceEnabled } from "../state/resources";
import { gitOverviewR, gitRemoteBranchesR, gitRemotesR, gitStashesR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { commitGitDraft, generateGitDraft, runRepositoryGit, setGitDraft, setGitProvider, useGitWorkbench } from "../state/gitWorkbench";
import { errMessage, reportError } from "../state/toast";
import { DEFAULT_GIT_VIEW, type GitPanel } from "../state/types";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import { FileIcon } from "./FileIcon";
import { CopyButton } from "./CopyButton";
import { IconCommit, IconFetch, IconGit, IconPull, IconPullRequest, IconPush, IconRefresh, IconSparkle, IconWarning, IconChevron } from "./Icons";
import { GitCmdLogBar } from "./git/GitCmdLogBar";
import { GitGraph } from "./git/GitGraph";
import { GitModalRenderer } from "./git/GitModalRenderer";
import { GitPanelBlock } from "./git/GitPanelBlock";
import { GitSelect } from "./git/GitSelect";
import { GitToolbarButton } from "./git/GitToolbarButton";
import { VirtualPanelRows } from "./git/VirtualPanelRows";
import { SkeletonRows } from "./Skeleton";
import { EmptyState } from "./Panel";
import { DEFAULT_AI_PROVIDER, AI_MODELS, AI_PROVIDER_LABEL, GIT_HELP, GIT_PANEL_BY_KEY, defaultAiModel } from "./git/gitPaneConstants";
import { filterByQuery, isGitAiProvider, isInRange, rangeBadge } from "./git/gitPaneLogic";
import type { GitAiProvider, RightView } from "./git/gitPaneTypes";
import { basename as basenameOf } from "../lib/paths";

const CommitReview = lazy(() => import("./CommitReview").then((module) => ({ default: module.CommitReview })));
const MergeReview = lazy(() => import("./MergeReview").then((module) => ({ default: module.MergeReview })));

export function GitPane({ paneId, cwd, active }: { paneId: string; cwd: string; active: boolean }) {
    const paneRootRef = useRef<HTMLDivElement>(null);
    const repo = cwd;
    const storedView = useStore((s) => s.gitViews[paneId]);
    const view = {
        ...DEFAULT_GIT_VIEW,
        ...(storedView ?? {}),
        selected: { ...DEFAULT_GIT_VIEW.selected, ...(storedView?.selected ?? {}) },
        remoteDrill: storedView?.remoteDrill ?? DEFAULT_GIT_VIEW.remoteDrill,
        remoteBranchSelected: storedView?.remoteBranchSelected ?? DEFAULT_GIT_VIEW.remoteBranchSelected,
    };
    const sel = view.selected;
    const panel: GitPanel = view.panel === "status" ? "files" : view.panel;
    const remoteDrill = view.remoteDrill ?? null;
    const remoteBranchSelected = view.remoteBranchSelected ?? {};
    const modalOpen = useStore((s) => s.gitModal !== null);

    const overview = useResourceEnabled(active && !!repo, gitOverviewR, repo || "");
    const remotesRes = useResourceEnabled(active && !!repo, gitRemotesR, repo || "");
    const stashesRes = useResourceEnabled(active && !!repo, gitStashesR, repo || "");
    const remoteBranchesRes = useResourceEnabled(active && !!repo && !!remoteDrill, gitRemoteBranchesR, repo || "", remoteDrill ?? "");
    const overviewLoading = !!repo && overview.status === "loading" && !overview.data;
    const overviewError = !!repo && overview.status === "error" && !overview.data ? (overview.error ?? "failed to load git state") : null;
    const status = repo ? (overview.data?.status ?? null) : null;
    const branches = useMemo(() => (repo ? (overview.data?.branches ?? []) : []), [repo, overview.data?.branches]);
    const commits = useMemo(() => (repo ? (overview.data?.log ?? []) : []), [repo, overview.data?.log]);
    const files = useMemo(() => status?.files ?? [], [status?.files]);
    const remotes = useMemo(() => (repo ? (remotesRes.data ?? []) : []), [repo, remotesRes.data]);
    const stashes = useMemo(() => (repo ? (stashesRes.data ?? []) : []), [repo, stashesRes.data]);
    const remoteBranches = useMemo(() => (repo && remoteDrill ? (remoteBranchesRes.data ?? []) : []), [repo, remoteDrill, remoteBranchesRes.data]);
    const currentBranch = branches.find((b) => b.current)?.name ?? status?.branch ?? "";

    const [right, setRight] = useState<RightView>({ mode: "output", text: "" });
    const [localBusy, setBusy] = useState<string | null>(null);
    const commitText = useGitWorkbench((state) => state.drafts[repo] ?? "");
    const setCommitText = (value: string | ((current: string) => string)) => setGitDraft(repo, value);
    const commitInputRef = useRef<HTMLTextAreaElement>(null);
    const aiProvider = useGitWorkbench((state) => state.provider);
    const aiModel = useGitWorkbench((state) => state.model);
    const sharedOperation = useGitWorkbench((state) => state.operations[repo]);
    const busy = sharedOperation?.busy ? sharedOperation.label : localBusy;
    const setAiProvider = setGitProvider;
    const setAiModel = (model: string) => useGitWorkbench.setState({ model });
    useEffect(() => {
        if (sharedOperation?.error || sharedOperation?.result)
            setRight({ mode: "output", text: sharedOperation.error || sharedOperation.result || "" });
    }, [sharedOperation]);
    const rightRef = useRef<HTMLDivElement>(null);
    const [branchInput, setBranchInput] = useState<{ startPoint: string } | null>(null);
    const [branchText, setBranchText] = useState("");
    const branchInputRef = useRef<HTMLInputElement>(null);
    const [searchByPanel, setSearchByPanel] = useState<Record<GitPanel, string>>({
        status: "",
        files: "",
        branches: "",
        remotes: "",
        commits: "",
        stashes: "",
    });
    const [searchOpen, setSearchOpen] = useState(false);
    const searchInputRef = useRef<HTMLInputElement>(null);
    const [rangeByPanel, setRangeByPanel] = useState<Record<GitPanel, number | null>>({
        status: null,
        files: null,
        branches: null,
        remotes: null,
        commits: null,
        stashes: null,
    });

    useEffect(() => {
        if (branchInput) branchInputRef.current?.focus();
    }, [branchInput]);

    useEffect(() => {
        if (searchOpen) searchInputRef.current?.focus();
    }, [searchOpen]);

    const fileQuery = searchByPanel.files;
    const branchQuery = searchByPanel.branches;
    const commitQuery = searchByPanel.commits;
    const stashQuery = searchByPanel.stashes;
    const filteredFiles = useMemo(() => filterByQuery(files, fileQuery, (f) => [f.path]), [files, fileQuery]);
    const filteredBranches = useMemo(() => filterByQuery(branches, branchQuery, (b) => [b.name]), [branches, branchQuery]);
    const filteredCommits = useMemo(() => filterByQuery(commits, commitQuery, (c) => [c.subject, c.hash]), [commits, commitQuery]);
    const filteredStashes = useMemo(() => filterByQuery(stashes, stashQuery, (s) => [s.message, s.branch, s.refname]), [stashes, stashQuery]);

    const remotesQuery = searchByPanel.remotes;
    const filteredRemotes = useMemo(() => filterByQuery(remotes, remotesQuery, (r) => [r.name, r.url]), [remotes, remotesQuery]);
    const filteredRemoteBranches = useMemo(() => filterByQuery(remoteBranches, remotesQuery, (b) => [b.name]), [remoteBranches, remotesQuery]);
    const remoteBranchSel = remoteDrill ? (remoteBranchSelected[remoteDrill] ?? 0) : 0;

    const lenFor = (p: GitPanel) =>
        p === "status"
            ? 1
            : p === "files"
              ? filteredFiles.length
              : p === "branches"
                ? filteredBranches.length
                : p === "remotes"
                  ? remoteDrill
                      ? filteredRemoteBranches.length
                      : filteredRemotes.length
                  : p === "commits"
                    ? filteredCommits.length
                    : filteredStashes.length;

    const rangeFor = (p: GitPanel): [number, number] | null => {
        const a = rangeByPanel[p];
        if (a === null) return null;
        const s = sel[p];
        return [Math.min(a, s), Math.max(a, s)];
    };

    useEffect(() => {
        if (!active) return;
        if (overviewLoading) {
            setRight({ mode: "output", text: "loading git state..." });
            return;
        }
        if (overviewError) {
            setRight({ mode: "output", text: `x ${overviewError}` });
            return;
        }
        if (panel === "files") {
            if (filteredFiles.length === 0) {
                const c = filteredCommits[0] ?? commits[0];
                if (c) setRight({ mode: "commit", rev: c.hash, title: c.hash, subtitle: c.subject });
                else setRight({ mode: "output", text: "" });
                return;
            }
            setRight({ mode: "merge", files: filteredFiles });
        } else if (panel === "commits") {
            if (filteredCommits.length === 0) return;
            const c = filteredCommits[Math.min(sel.commits, filteredCommits.length - 1)];
            if (c)
                setRight({
                    mode: "commit",
                    rev: c.hash,
                    title: c.hash,
                    subtitle: c.subject,
                });
        } else if (panel === "branches") {
            if (filteredBranches.length === 0) return;
            const b = filteredBranches[Math.min(sel.branches, filteredBranches.length - 1)];
            if (b)
                setRight({
                    mode: "commit",
                    rev: b.name,
                    title: b.name,
                    subtitle: "branch tip",
                });
        } else if (panel === "remotes") {
            if (remoteDrill) {
                if (filteredRemoteBranches.length === 0) {
                    setRight({ mode: "output", text: `(no branches under ${remoteDrill}/)` });
                    return;
                }
                const rb = filteredRemoteBranches[Math.min(remoteBranchSel, filteredRemoteBranches.length - 1)];
                if (rb)
                    setRight({
                        mode: "commit",
                        rev: rb.full_ref,
                        title: rb.full_ref,
                        subtitle: rb.tracked_by ? `tracked by ${rb.tracked_by}` : "remote branch tip",
                    });
            } else {
                if (filteredRemotes.length === 0) {
                    setRight({
                        mode: "output",
                        text: remotes.length === 0 ? "(no remotes configured)" : "no matches",
                    });
                    return;
                }
                const r = filteredRemotes[Math.min(sel.remotes, filteredRemotes.length - 1)];
                if (r)
                    setRight({
                        mode: "output",
                        text: `remote: ${r.name}\nurl:    ${r.url}\n\npress enter to browse this remote's branches\npress f to fetch · n to add · r to rename · e to edit url · d to delete`,
                    });
            }
        } else if (panel === "stashes") {
            if (filteredStashes.length === 0) {
                setRight({
                    mode: "output",
                    text: stashes.length === 0 ? "(no stashes)" : "no matches",
                });
                return;
            }
            const s = filteredStashes[Math.min(sel.stashes, filteredStashes.length - 1)];
            if (s)
                setRight({
                    mode: "commit",
                    rev: s.refname,
                    title: s.refname,
                    subtitle: s.message,
                });
        }
    }, [
        panel,
        sel.commits,
        sel.branches,
        sel.remotes,
        sel.stashes,
        remoteDrill,
        remoteBranchSel,
        status,
        commits,
        files.length,
        stashes.length,
        filteredFiles,
        filteredCommits,
        filteredBranches,
        filteredRemotes,
        filteredRemoteBranches,
        filteredStashes,
        remotes.length,
        overviewLoading,
        overviewError,
        active,
    ]);

    const errorTimerRef = useRef<number | undefined>(undefined);
    async function run<T>(label: string, fn: () => Promise<T>, opts?: { silent?: boolean }): Promise<T | undefined> {
        if (errorTimerRef.current) {
            window.clearTimeout(errorTimerRef.current);
            errorTimerRef.current = undefined;
        }
        if (!opts?.silent) setBusy(label || "running");
        try {
            if (useGitWorkbench.getState().operations[repo]?.busy) {
                setBusy(null);
                return undefined;
            }
            let out: T | undefined;
            const succeeded = await runRepositoryGit(repo, label || "Git operation", async () => {
                out = await fn();
                return out;
            });
            if (!succeeded) throw new Error(useGitWorkbench.getState().operations[repo]?.error || "Another Git operation is running.");
            if (typeof out === "string" && out && !opts?.silent) {
                setRight({ mode: "output", text: out });
            }
            setBusy(null);
            void overview.refresh().catch(reportError("git refresh"));
            void stashesRes.refresh().catch(reportError("stash refresh"));
            return out;
        } catch (err) {
            const msg = errMessage(err);
            setRight({ mode: "output", text: `✗ ${msg}` });
            reportError(label || "git")(err);
            setBusy(`✗ ${msg.length > 80 ? msg.slice(0, 80) + "…" : msg}`);
            errorTimerRef.current = window.setTimeout(() => {
                setBusy(null);
                errorTimerRef.current = undefined;
            }, 3500);
            void overview.refresh().catch(reportError("git refresh"));
            void stashesRes.refresh().catch(reportError("stash refresh"));
            return undefined;
        }
    }

    const setPanel = (p: GitPanel) => cmd.setGitView(paneId, { panel: p });
    const setSel = (next: typeof sel) => cmd.setGitView(paneId, { selected: next });

    const setRemoteDrill = (name: string | null) => cmd.setGitView(paneId, { remoteDrill: name });
    const setRemoteBranchSel = (name: string, idx: number) =>
        cmd.setGitView(paneId, {
            remoteBranchSelected: { ...remoteBranchSelected, [name]: idx },
        });

    const toggleStage = () => {
        const r = rangeFor("files");
        if (r) {
            const slice = filteredFiles.slice(r[0], r[1] + 1);
            void run("staging range", async () => {
                for (const f of slice) {
                    if (hasUnstaged(f)) await git.stage(repo, f.path);
                }
            });
            return;
        }
        const f = filteredFiles[sel.files];
        if (!f) return;
        void run("", async () => {
            if (hasUnstaged(f)) await git.stage(repo, f.path);
            else await git.unstage(repo, f.path);
        });
    };

    const doCommit = (_message: string) => {
        void commitGitDraft(repo).then(() => overview.refresh().catch(reportError("git refresh")));
    };
    const generateCommitMessage = () => {
        void generateGitDraft(repo);
    };

    const moveSel = (d: number) => {
        const len = lenFor(panel);
        if (len === 0) return;
        if (panel === "remotes" && remoteDrill) {
            setRemoteBranchSel(remoteDrill, Math.max(0, Math.min(len - 1, remoteBranchSel + d)));
            return;
        }
        setSel({
            ...sel,
            [panel]: Math.max(0, Math.min(len - 1, sel[panel] + d)),
        });
    };

    const doCreateBranch = (name: string, startPoint: string) => {
        if (!name.trim()) return;
        closeBranchInput();
        void run("creating branch…", async () => {
            await git.branchCreate(repo, name.trim(), startPoint || undefined);
            return `✓ created + checked out ${name.trim()}${startPoint ? `\n  from ${startPoint}` : ""}`;
        });
    };
    const openBranchInput = (startPoint = "") => {
        setBranchInput({ startPoint });
        setBranchText("");
    };
    const closeBranchInput = () => {
        setBranchInput(null);
        setBranchText("");
    };

    const doMerge = (branch: string) => {
        void run(`merging ${branch}…`, async () => {
            const out = await git.merge(repo, branch);
            return `✓ merged ${branch}${out ? `\n\n${out}` : ""}`;
        });
    };

    const openBranchRenamePrompt = () => {
        const b = filteredBranches[sel.branches];
        if (!b) return;
        openGitPrompt({
            title: `Rename branch · ${b.name}`,
            initial: b.name,
            onConfirm: (name) => {
                const n = name.trim();
                if (!n || n === b.name) return;
                void run(`renaming branch ${b.name} → ${n}`, async () => {
                    await git.branchRename(repo, b.name, n);
                    return `✓ ${b.name} → ${n}`;
                });
            },
        });
    };

    const refreshRepoState = () => {
        void overview.refresh().catch(reportError("git refresh"));
        void stashesRes.refresh().catch(reportError("stash refresh"));
        if (panel === "remotes") {
            void remotesRes.refresh().catch(reportError("remote refresh"));
            if (remoteDrill) void remoteBranchesRes.refresh().catch(reportError("remote branch refresh"));
        }
    };
    const pushRepo = () => void run("pushing…", async () => `↑ ${await git.push(repo)}`);
    const pullRepo = () => void run("pulling…", async () => `↓ ${await git.pull(repo)}`);
    const openPullRequest = (compact = false) =>
        void run("opening PR…", async () => {
            const url = await git.prOpen(repo);
            return compact ? `→ ${url}` : `→ opened pull-request page\n${url}`;
        });

    const openFilesDiscardMenu = () => {
        const f = filteredFiles[sel.files];
        if (!f) return;
        const r = rangeFor("files");
        const targets = r ? filteredFiles.slice(r[0], r[1] + 1) : [f];
        const label = r ? `Discard ${targets.length} files` : `Discard changes — ${basenameOf(f.path)}`;
        const apply = (mode: "all" | "unstaged" | "staged", confirmKey: "d" | "u" | "s") => () => {
            openGitConfirm({
                title: label,
                body:
                    mode === "all"
                        ? `This will discard BOTH staged and unstaged changes in ${targets.length} file${targets.length > 1 ? "s" : ""}. Cannot be undone.`
                        : mode === "unstaged"
                          ? `Discard unstaged changes in ${targets.length} file${targets.length > 1 ? "s" : ""}? (staged changes preserved)`
                          : `Unstage ${targets.length} file${targets.length > 1 ? "s" : ""} from the index? (worktree preserved)`,
                destructive: true,
                confirmLabel: mode === "staged" ? "unstage" : "discard",
                initialFocus: "confirm",
                confirmKey,
                onConfirm: async () => {
                    await run(r ? `discarding ${targets.length} files (${mode})` : `discarding ${f.path}`, async () => {
                        for (const t of targets) await git.discardFile(repo, t.path, mode);
                    });
                },
            });
        };
        openGitMenu(label, [
            {
                key: "d",
                label: "discard all changes",
                hint: "staged + worktree",
                destructive: true,
                run: apply("all", "d"),
            },
            {
                key: "u",
                label: "discard unstaged changes",
                destructive: true,
                run: apply("unstaged", "u"),
            },
            {
                key: "s",
                label: "unstage changes",
                run: apply("staged", "s"),
            },
        ]);
    };

    const openFilesStashMenu = () => {
        const stash = (mode: "all" | "staged" | "unstaged") => () => {
            openGitPrompt({
                title: mode === "all" ? "Stash everything" : mode === "staged" ? "Stash staged changes" : "Stash unstaged changes",
                placeholder: "(optional) stash message",
                onConfirm: (msg) => {
                    void run(`stashing (${mode})`, async () => {
                        await git.stashPush(repo, mode, msg || null);
                        return `✓ stashed (${mode})${msg ? `\n  ${msg}` : ""}`;
                    });
                },
            });
        };
        openGitMenu("Stash", [
            {
                key: "s",
                label: "stash everything (working tree + index + untracked)",
                run: stash("all"),
            },
            {
                key: "i",
                label: "stash staged only",
                run: stash("staged"),
            },
            {
                key: "u",
                label: "stash unstaged only (keep index)",
                run: stash("unstaged"),
            },
        ]);
    };

    const selectedStash = () => filteredStashes[sel.stashes];

    const applySelectedStash = () => {
        const s = selectedStash();
        if (!s) return;
        void run(`applying ${s.refname}`, async () => {
            await git.stashApply(repo, s.refname, s.sha);
            return `✓ applied ${s.refname}`;
        });
    };

    const popSelectedStash = () => {
        const s = selectedStash();
        if (!s) return;
        void run(`popping ${s.refname}`, async () => {
            await git.stashPop(repo, s.refname, s.sha);
            return `✓ popped ${s.refname}`;
        });
    };

    const openStashBranchPrompt = () => {
        const s = selectedStash();
        if (!s) return;
        openGitPrompt({
            title: `Branch from ${s.refname}`,
            placeholder: "branch name",
            onConfirm: (name) => {
                const n = name.trim();
                if (!n) return;
                void run(`branching from ${s.refname}`, async () => {
                    await git.stashBranch(repo, s.refname, s.sha, n);
                    return `✓ created ${n} from ${s.refname}`;
                });
            },
        });
    };

    const openStashRenamePrompt = () => {
        const s = selectedStash();
        if (!s) return;
        openGitPrompt({
            title: `Rename ${s.refname}`,
            initial: s.message,
            onConfirm: (message) => {
                const m = message.trim();
                if (!m || m === s.message) return;
                void run(`renaming ${s.refname}`, async () => {
                    await git.stashRename(repo, s.refname, s.sha, m);
                    return `✓ renamed ${s.refname}`;
                });
            },
        });
    };

    const openStashDropConfirm = () => {
        const s = selectedStash();
        if (!s) return;
        openGitConfirm({
            title: `Drop ${s.refname}?`,
            body: "This permanently deletes the stash entry.",
            destructive: true,
            confirmLabel: "drop",
            onConfirm: () => {
                void run(`dropping ${s.refname}`, async () => {
                    await git.stashDrop(repo, s.refname, s.sha);
                    return `✓ dropped ${s.refname}`;
                });
            },
        });
    };

    const openStashRowMenu = () => {
        const s = filteredStashes[sel.stashes];
        if (!s) return;
        const title = `${s.refname} · ${s.message}`;
        openGitMenu(title, [
            {
                key: "a",
                label: "apply stash",
                hint: "keep stash",
                run: applySelectedStash,
            },
            {
                key: "p",
                label: "pop stash",
                hint: "apply + drop",
                run: popSelectedStash,
            },
            {
                key: "b",
                label: "create branch from stash",
                run: openStashBranchPrompt,
            },
            {
                key: "r",
                label: "rename stash",
                run: openStashRenamePrompt,
            },
            {
                key: "d",
                label: "drop stash",
                destructive: true,
                run: openStashDropConfirm,
            },
        ]);
    };

    const selectedCommit = () => filteredCommits[sel.commits];

    const openCommitBranchPrompt = () => {
        const c = selectedCommit();
        if (!c) return;
        openGitPrompt({
            title: `Branch from ${c.hash}`,
            placeholder: "branch name",
            onConfirm: (name) => {
                const n = name.trim();
                if (!n) return;
                void run(`creating ${n} from ${c.hash}`, async () => {
                    await git.branchCreate(repo, n, c.hash);
                    return `✓ created + checked out ${n}\n  from ${c.hash}`;
                });
            },
        });
    };

    const openCommitResetMenu = () => {
        const c = selectedCommit();
        if (!c) return;
        const resetTo = (mode: "soft" | "mixed" | "hard") => () => {
            openGitConfirm({
                title: `${mode} reset to ${c.hash}?`,
                body:
                    mode === "soft"
                        ? "Moves HEAD to this commit and keeps all later changes staged."
                        : mode === "mixed"
                          ? "Moves HEAD to this commit and keeps all later changes in the working tree."
                          : "Moves HEAD to this commit and discards all later changes from the index and working tree.",
                destructive: mode === "hard",
                confirmLabel: `${mode} reset`,
                onConfirm: async () => {
                    await run(`reset --${mode} ${c.hash}`, async () => {
                        await git.reset(repo, c.hash, mode);
                        return `✓ reset --${mode} ${c.hash}`;
                    });
                },
            });
        };
        openGitMenu(`Reset to ${c.hash}`, [
            { key: "s", label: "soft reset", hint: "keep changes staged", run: resetTo("soft") },
            { key: "m", label: "mixed reset", hint: "keep changes unstaged", run: resetTo("mixed") },
            { key: "h", label: "hard reset", hint: "discard later changes", destructive: true, run: resetTo("hard") },
        ]);
    };

    const openCommitRevertConfirm = () => {
        const c = selectedCommit();
        if (!c) return;
        openGitConfirm({
            title: `Revert ${c.hash}?`,
            body: "Creates a new commit that reverses this commit. Existing history is preserved.",
            confirmLabel: "revert",
            onConfirm: async () => {
                await run(`reverting ${c.hash}`, async () => {
                    await git.revert(repo, c.hash);
                    return `✓ reverted ${c.hash}`;
                });
            },
        });
    };

    const openCommitRowMenu = () => {
        const c = selectedCommit();
        if (!c) return;

        openGitMenu(`${c.hash} · ${c.subject}`, [
            {
                key: "b",
                label: "create branch from commit",
                run: openCommitBranchPrompt,
            },
            {
                key: "r",
                label: "reset to this commit",
                hint: "choose soft / mixed / hard",
                run: openCommitResetMenu,
            },
            {
                key: "v",
                label: "revert this commit",
                hint: "new inverse commit",
                run: openCommitRevertConfirm,
            },
        ]);
    };

    const confirmBranchDelete = (branch: string, force: boolean) => {
        openGitConfirm({
            title: `${force ? "Force delete" : "Delete"} ${branch}?`,
            body: force
                ? "This deletes the local branch even if it has commits that are not merged anywhere else."
                : "Deletes the local branch only. Use force delete if Git refuses because the branch is not merged.",
            destructive: true,
            confirmLabel: force ? "force delete" : "delete",
            onConfirm: async () => {
                await run(`${force ? "force " : ""}deleting branch ${branch}`, async () => {
                    await git.branchDelete(repo, branch, force);
                    return `✓ ${force ? "force " : ""}deleted branch ${branch}`;
                });
            },
        });
    };

    const openBranchVerbsMenu = (mode: "merge" | "delete") => {
        const b = filteredBranches[sel.branches];
        if (!b) return;
        if (mode === "merge") {
            openGitMenu(`Merge ${b.name} into HEAD`, [
                {
                    key: "m",
                    label: "regular merge",
                    run: () => doMerge(b.name),
                },
                {
                    key: "s",
                    label: "squash merge",
                    run: () => {
                        void run(`squash merging ${b.name}…`, async () => {
                            const out = await git.mergeSquash(repo, b.name);
                            return `✓ squash merged ${b.name}${out ? `\n\n${out}` : ""}`;
                        });
                    },
                },
            ]);
        } else {
            openGitMenu(
                `Delete ${b.name}`,
                (
                    [
                        ["d", "delete local branch", false, undefined],
                        ["D", "force delete local branch", true, "git branch -D"],
                    ] as const
                ).map(([key, label, force, hint]) => ({
                    key,
                    label,
                    destructive: true,
                    disabled: b.current,
                    hint: b.current ? "(can't delete current branch)" : hint,
                    run: () => confirmBranchDelete(b.name, force),
                })),
            );
        }
    };

    const doFetch = (remote: string | null) => {
        void run(remote ? `fetching ${remote}…` : "fetching all remotes…", async () => {
            const out = await git.fetch(repo, remote);
            await remotesRes.refresh();
            if (remoteDrill) await remoteBranchesRes.refresh();
            return out.trim().length > 0 ? out : `✓ fetched ${remote ?? "all"}`;
        });
    };

    const openAddRemotePrompt = () => {
        openGitPrompt({
            title: "Add remote",
            placeholder: "name (e.g. upstream) — then you'll enter the URL",
            onConfirm: (name) => {
                const n = name.trim();
                if (!n) return;
                openGitPrompt({
                    title: `Add remote · ${n}`,
                    placeholder: "URL (https://… or git@…)",
                    onConfirm: (url) => {
                        const u = url.trim();
                        if (!u) return;
                        void run(`adding remote ${n}`, async () => {
                            await git.remoteAdd(repo, n, u);
                            await remotesRes.refresh();
                            return `✓ added remote ${n} → ${u}`;
                        });
                    },
                });
            },
        });
    };

    const openRemoteRowMenu = () => {
        const r = filteredRemotes[sel.remotes];
        if (!r) return;
        openGitMenu(`Remote · ${r.name}`, [
            {
                key: "enter",
                label: "browse branches",
                run: () => setRemoteDrill(r.name),
            },
            {
                key: "f",
                label: "fetch this remote",
                hint: "git fetch --prune <name>",
                run: () => doFetch(r.name),
            },
            {
                key: "e",
                label: "edit url",
                run: () => {
                    openGitPrompt({
                        title: `Edit url · ${r.name}`,
                        initial: r.url,
                        onConfirm: (u) => {
                            const url = u.trim();
                            if (!url || url === r.url) return;
                            void run(`setting url for ${r.name}`, async () => {
                                await git.remoteSetUrl(repo, r.name, url);
                                await remotesRes.refresh();
                                return `✓ ${r.name} → ${url}`;
                            });
                        },
                    });
                },
            },
            {
                key: "r",
                label: "rename",
                run: () => {
                    openGitPrompt({
                        title: `Rename remote · ${r.name}`,
                        initial: r.name,
                        onConfirm: (n) => {
                            const newName = n.trim();
                            if (!newName || newName === r.name) return;
                            void run(`renaming ${r.name} → ${newName}`, async () => {
                                await git.remoteRename(repo, r.name, newName);
                                await remotesRes.refresh();
                                return `✓ ${r.name} → ${newName}`;
                            });
                        },
                    });
                },
            },
            {
                key: "d",
                label: "delete remote",
                destructive: true,
                run: () => {
                    openGitConfirm({
                        title: `Remove remote ${r.name}?`,
                        body: `Removes the local remote configuration. Won't touch the upstream repo.`,
                        destructive: true,
                        confirmLabel: "remove",
                        onConfirm: () => {
                            void run(`removing remote ${r.name}`, async () => {
                                await git.remoteRemove(repo, r.name);
                                await remotesRes.refresh();
                                return `✓ removed ${r.name}`;
                            });
                        },
                    });
                },
            },
        ]);
    };

    const openRemoteBranchMenu = () => {
        if (!remoteDrill) return;
        const rb = filteredRemoteBranches[remoteBranchSel];
        if (!rb || rb.is_head_pointer) return;
        openGitMenu(`${rb.full_ref}`, [
            {
                key: "space",
                label: rb.tracked_by ? `checkout local ${rb.tracked_by}` : `checkout (create local ${rb.name})`,
                run: () => {
                    void run(`checking out ${rb.full_ref}`, async () => {
                        await git.checkoutRemoteBranch(repo, remoteDrill, rb.name, rb.tracked_by ?? null);
                        return `✓ on ${rb.tracked_by ?? rb.name}`;
                    });
                },
            },
            {
                key: "M",
                label: `merge ${rb.full_ref} into HEAD`,
                run: () => doMerge(rb.full_ref),
            },
            {
                key: "u",
                label: `set as upstream of ${currentBranch || "HEAD"}`,
                disabled: !currentBranch,
                run: () => {
                    if (!currentBranch) return;
                    void run(`setting upstream of ${currentBranch}`, async () => {
                        await git.setUpstream(repo, currentBranch, rb.full_ref);
                        return `✓ ${currentBranch} now tracks ${rb.full_ref}`;
                    });
                },
            },
            {
                key: "d",
                label: "delete remote branch",
                destructive: true,
                run: () => {
                    openGitConfirm({
                        title: `Delete ${rb.full_ref}?`,
                        body: `Pushes a delete to ${remoteDrill}. The upstream branch will be gone for everyone.`,
                        destructive: true,
                        confirmLabel: "delete upstream",
                        onConfirm: () => {
                            void run(`deleting ${rb.full_ref}`, async () => {
                                await git.deleteRemoteBranch(repo, remoteDrill, rb.name);
                                await remoteBranchesRes.refresh();
                                return `✓ deleted ${rb.full_ref}`;
                            });
                        },
                    });
                },
            },
        ]);
    };

    const openHelpCheatsheet = () => openGitCheatsheet("Git pane keybindings", GIT_HELP);

    useEffect(() => {
        if (!active || branchInput || modalOpen) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.altKey || e.metaKey) return;
            if (useStore.getState().pickerOpen) return;
            const ae = document.activeElement;
            if (!ae || !paneRootRef.current?.contains(ae) || ae.closest('[role="dialog"], [role="listbox"], [role="separator"]')) return;
            if (e.key === "Tab") return;
            if (ae.closest('button, [role="button"]') && !ae.closest(".git-row, .gg-row") && ["Enter", " ", "ArrowUp", "ArrowDown"].includes(e.key))
                return;
            if (ae.closest('input, textarea, [contenteditable="true"]') && !searchOpen) return;
            if (ae && ae.closest(".cm-editor")) return;
            if (ae && ae.closest(".git-commit-panel")) return;
            // The embedded shell owns every key while it has focus, otherwise
            // typing `git status` would trip the panel's single-letter verbs.
            // The split handle is excluded too so its arrow keys resize.
            if (searchOpen) {
                if (e.key === "Escape") {
                    setSearchOpen(false);
                    setSearchByPanel((s) => ({ ...s, [panel]: "" }));
                    e.preventDefault();
                    e.stopPropagation();
                }
                return;
            }
            const k = e.key;
            if (e.ctrlKey) {
                if (k === "p" || k === "P") {
                    e.preventDefault();
                    openPullRequest();
                }
                return;
            }
            if (busy) return;

            let handled = true;
            if (k === "?") openHelpCheatsheet();
            else if (k === "@") toggleGitCmdLog();
            else if (k === "/") setSearchOpen(true);
            else if (k === "v" && panel === "commits") openCommitRevertConfirm();
            else if (k === "v") {
                setRangeByPanel((s) => ({
                    ...s,
                    [panel]: s[panel] === null ? sel[panel] : null,
                }));
            } else if (k === "Escape") {
                if (rangeByPanel[panel] !== null) {
                    setRangeByPanel((s) => ({ ...s, [panel]: null }));
                } else if (searchByPanel[panel]) {
                    setSearchByPanel((s) => ({ ...s, [panel]: "" }));
                } else if (panel === "remotes" && remoteDrill) {
                    setRemoteDrill(null);
                } else handled = false;
            } else if (GIT_PANEL_BY_KEY[k]) {
                const nextPanel = GIT_PANEL_BY_KEY[k]!;
                setPanel(nextPanel);
            } else if (k === "j" || k === "ArrowDown") moveSel(1);
            else if (k === "k" || k === "ArrowUp") moveSel(-1);
            else if (k === "r") {
                if (panel === "stashes") openStashRenamePrompt();
                else if (panel === "commits") openCommitResetMenu();
                else if (panel === "remotes" && !remoteDrill) openRemoteRowMenu();
                else refreshRepoState();
            } else if (k === "F") doFetch(null);
            else if (k === "P") pushRepo();
            else if (k === "p" && panel === "stashes") popSelectedStash();
            else if (k === "p") pullRepo();
            else if (panel === "files" && k === " ") toggleStage();
            else if (panel === "files" && k === "a") {
                const anyUnstaged = filteredFiles.some(hasUnstaged);
                void run("", () => (anyUnstaged ? git.stageAll(repo) : git.unstageAll(repo)));
            } else if (panel === "files" && k === "c") commitInputRef.current?.focus();
            else if (panel === "files" && k === "C") doCommit(commitText);
            else if (panel === "files" && k === "g") void generateCommitMessage();
            else if (panel === "files" && k === "d") openFilesDiscardMenu();
            else if (panel === "files" && k === "s") openFilesStashMenu();
            else if (panel === "branches" && (k === "Enter" || k === " ")) {
                const b = filteredBranches[sel.branches];
                if (b && !b.current) void run("", () => git.checkout(repo, b.name));
            } else if (panel === "branches" && k === "n") {
                const b = filteredBranches[sel.branches];
                openBranchInput(b?.name);
            } else if (panel === "branches" && k === "N") {
                openBranchInput();
            } else if (panel === "branches" && k === "M") {
                openBranchVerbsMenu("merge");
            } else if (panel === "branches" && k === "d") {
                openBranchVerbsMenu("delete");
            } else if (panel === "branches" && k === "R") {
                openBranchRenamePrompt();
            } else if (panel === "branches" && k === "c") {
                openGitPrompt({
                    title: "Checkout branch",
                    placeholder: "branch name (- for previous)",
                    suggestions: branches.map((b) => ({
                        value: b.name,
                        hint: b.current ? "current" : (b.upstream ?? ""),
                    })),
                    onConfirm: (name) => {
                        const target = name.trim();
                        if (!target) return;
                        void run(`checking out ${target}`, () => git.checkout(repo, target));
                    },
                });
            } else if (panel === "remotes" && !remoteDrill && k === "Enter") {
                const r = filteredRemotes[sel.remotes];
                if (r) setRemoteDrill(r.name);
            } else if (panel === "remotes" && !remoteDrill && k === "n") {
                openAddRemotePrompt();
            } else if (panel === "remotes" && !remoteDrill && k === "f") {
                const r = filteredRemotes[sel.remotes];
                if (r) doFetch(r.name);
            } else if (panel === "remotes" && !remoteDrill && k === "e") {
                openRemoteRowMenu();
            } else if (panel === "remotes" && !remoteDrill && (k === "d" || k === "r")) {
                openRemoteRowMenu();
            } else if (panel === "remotes" && remoteDrill && k === "Enter") {
                openRemoteBranchMenu();
            } else if (panel === "remotes" && remoteDrill && (k === " " || k === "Space")) {
                const rb = filteredRemoteBranches[remoteBranchSel];
                if (rb && !rb.is_head_pointer) {
                    void run(`checking out ${rb.full_ref}`, async () => {
                        await git.checkoutRemoteBranch(repo, remoteDrill, rb.name, rb.tracked_by ?? null);
                        return `✓ on ${rb.tracked_by ?? rb.name}`;
                    });
                }
            } else if (panel === "remotes" && remoteDrill && k === "M") {
                const rb = filteredRemoteBranches[remoteBranchSel];
                if (rb && !rb.is_head_pointer) doMerge(rb.full_ref);
            } else if (panel === "remotes" && remoteDrill && k === "u") {
                const rb = filteredRemoteBranches[remoteBranchSel];
                if (rb && !rb.is_head_pointer && currentBranch) {
                    void run(`setting upstream of ${currentBranch}`, async () => {
                        await git.setUpstream(repo, currentBranch, rb.full_ref);
                        return `✓ ${currentBranch} now tracks ${rb.full_ref}`;
                    });
                }
            } else if (panel === "remotes" && remoteDrill && k === "d") {
                openRemoteBranchMenu();
            } else if (panel === "remotes" && remoteDrill && k === "f") {
                doFetch(remoteDrill);
            } else if (panel === "commits" && (k === "Enter" || k === " ")) {
                openCommitRowMenu();
            } else if (panel === "commits" && k === "b") {
                openCommitBranchPrompt();
            } else if (panel === "commits" && k === "v") {
                openCommitRevertConfirm();
            } else if (panel === "stashes" && k === "Enter") {
                openStashRowMenu();
            } else if (panel === "stashes" && (k === " " || k === "a")) {
                applySelectedStash();
            } else if (panel === "stashes" && k === "p") {
                popSelectedStash();
            } else if (panel === "stashes" && k === "b") {
                openStashBranchPrompt();
            } else if (panel === "stashes" && k === "d") {
                openStashDropConfirm();
            } else handled = false;
            if (handled) {
                e.preventDefault();
                e.stopPropagation();
            }
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    });

    useEffect(() => {
        if (!document.activeElement?.closest(".git-row, .gg-row")) return;
        paneRootRef.current?.querySelector<HTMLElement>(".git-panel.focused .git-row.sel, .git-panel.focused .gg-row.sel")?.focus();
    }, [sel, remoteBranchSel]);

    const panelFiles = panel === "files";
    const filesRange = rangeFor("files");
    const branchesRange = rangeFor("branches");
    const remotesRange = rangeFor("remotes");
    const commitsRange = rangeFor("commits");
    const stashesRange = rangeFor("stashes");
    const stagedCount = filteredFiles.filter(isStaged).length;
    const unstagedCount = filteredFiles.filter(hasUnstaged).length;
    const hasUpstream = !!status?.upstream;
    const upstreamLabel = status?.upstream ?? "no upstream";
    const toolbarBranchText = overviewLoading ? "loading" : overviewError ? "git error" : currentBranch || status?.branch || "—";
    const changedText = overviewLoading
        ? "loading repo..."
        : overviewError
          ? `x ${overviewError}`
          : files.length === 0
            ? "clean"
            : `${files.length} changed · ${stagedCount} staged · ${unstagedCount} unstaged`;
    // Only the focused panel opens up; the rest stay at the height their rows
    // need, so the log sits collapsed until you actually go to it.
    const panelFlex = (focused: boolean) => (focused ? 1 : 0);

    const fileEmptyText = overviewError ?? (fileQuery ? `Nothing matches "${fileQuery}".` : "No uncommitted changes.");
    const branchEmptyText = overviewError ?? (branchQuery ? `Nothing matches "${branchQuery}".` : "This repository has no branches yet.");
    const commitEmptyText = overviewError ?? (commitQuery ? `Nothing matches "${commitQuery}".` : "No commits on this branch yet.");

    return (
        <div ref={paneRootRef} className="git-pane">
            <div className="git-toolbar">
                <span className="git-tb-status">
                    <IconGit size={13} className={`git-tb-icon${files.length > 0 ? " dirty" : ""}`} />
                    <span className="git-tb-branch" title={overviewError ?? `upstream: ${upstreamLabel}`}>
                        {toolbarBranchText}
                    </span>
                    {!!currentBranch && <CopyButton className="git-tb-copy" value={currentBranch} label="branch name" size={12} />}
                    <span className={`git-tb-changed${overviewError ? " error" : ""}`}>{changedText}</span>
                    {stashes.length > 0 && <span className="git-tb-stash">{stashes.length === 1 ? "1 stash" : `${stashes.length} stashes`}</span>}
                    {busy && (
                        <span className={`git-tb-busy${busy.startsWith("✗") ? " error" : ""}`}>
                            {!busy.startsWith("✗") && <span className="git-panel-spinner" />}
                            <span>{busy}</span>
                        </span>
                    )}
                </span>
                <span className="git-tb-grow" />
                <button type="button" className="git-tbtn" aria-pressed={panel === "remotes"} onClick={() => setPanel("remotes")}>
                    Remotes
                </button>
                <button type="button" className="git-tbtn" aria-pressed={panel === "stashes"} onClick={() => setPanel("stashes")}>
                    Stashes
                </button>
                <GitToolbarButton
                    className="live"
                    icon={<IconPush size={13} />}
                    count={status?.ahead}
                    kbd="P"
                    onClick={pushRepo}
                    title={
                        !hasUpstream
                            ? "Publish branch and set upstream (P)"
                            : status && status.ahead > 0
                              ? `Push ${status.ahead} commit${status.ahead > 1 ? "s" : ""} (P)`
                              : "Push (P)"
                    }>
                    {hasUpstream ? "push" : "publish"}
                </GitToolbarButton>
                <GitToolbarButton
                    icon={<IconPull size={13} />}
                    count={status?.behind}
                    kbd="p"
                    onClick={pullRepo}
                    title={
                        status && status.behind > 0
                            ? `Pull ${status.behind} commit${status.behind > 1 ? "s" : ""}; auto-rebase if branches diverged (p)`
                            : "Pull; auto-rebase if branches diverged (p)"
                    }>
                    pull
                </GitToolbarButton>
                <GitToolbarButton icon={<IconFetch size={13} />} kbd="F" onClick={() => doFetch(null)} title="Fetch all remotes (F)">
                    fetch
                </GitToolbarButton>
                <GitToolbarButton icon={<IconPullRequest size={13} />} onClick={() => openPullRequest(true)} title="Open pull request (⌃P)">
                    PR
                </GitToolbarButton>
                <GitToolbarButton className="icon" icon={<IconRefresh size={14} />} onClick={refreshRepoState} title="Refresh (r)" />
            </div>
            <div className="git-body">
                <div className="git-left">
                    <div className="panel panel-block git-commit-panel">
                        <div className="panel-head">
                            <button
                                type="button"
                                className="git-cp-headfocus"
                                onClick={() => commitInputRef.current?.focus()}
                                title="Focus commit message">
                                <span className="panel-index">1</span>
                                <span className="panel-label">Commit</span>
                            </button>
                            <div className="git-cp-head-actions">
                                <GitSelect
                                    title="Local AI CLI"
                                    width={62}
                                    value={aiProvider}
                                    label={AI_PROVIDER_LABEL[aiProvider]}
                                    options={(Object.keys(AI_PROVIDER_LABEL) as GitAiProvider[]).map((provider) => ({
                                        value: provider,
                                        label: AI_PROVIDER_LABEL[provider],
                                    }))}
                                    onSelect={(value) => {
                                        const provider = isGitAiProvider(value) ? value : DEFAULT_AI_PROVIDER;
                                        setAiProvider(provider);
                                        setAiModel(defaultAiModel(provider));
                                    }}
                                />
                                <GitSelect
                                    title="AI model"
                                    width={96}
                                    value={aiModel || defaultAiModel(aiProvider)}
                                    label={aiModel || defaultAiModel(aiProvider)}
                                    options={AI_MODELS[aiProvider].map((model) => ({ value: model, label: model }))}
                                    onSelect={setAiModel}
                                />
                                <button
                                    className="git-cp-ai"
                                    type="button"
                                    disabled={!!busy}
                                    onClick={generateCommitMessage}
                                    title={`Generate with locally installed ${AI_PROVIDER_LABEL[aiProvider]} · ${aiModel || defaultAiModel(aiProvider)} (g) — does not stage or commit`}>
                                    <IconSparkle size={15} />
                                </button>
                            </div>
                        </div>
                        <div className="git-cp-body">
                            <textarea
                                ref={commitInputRef}
                                className="git-cp-input"
                                placeholder="commit message…"
                                value={commitText}
                                spellCheck={false}
                                readOnly={!!busy}
                                rows={3}
                                onChange={(e) => setCommitText(e.target.value)}
                                onKeyDown={(e) => {
                                    if (busy) e.preventDefault();
                                    else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) doCommit(commitText);
                                    else if (e.key === "Escape") {
                                        setCommitText("");
                                        commitInputRef.current?.blur();
                                    }
                                    e.stopPropagation();
                                }}
                            />
                            <div className="git-cp-actions">
                                <button
                                    className="git-cp-commit"
                                    type="button"
                                    disabled={!!busy || !commitText.trim()}
                                    onClick={() => doCommit(commitText)}
                                    title={`Commit staged (${PRIMARY_SHORTCUT}⏎)`}>
                                    <IconCommit size={13} />
                                    commit
                                </button>
                            </div>
                        </div>
                    </div>
                    {branchInput && (
                        <div className="git-commit-bar">
                            <input
                                ref={branchInputRef}
                                className="git-commit-input"
                                placeholder={branchInput.startPoint ? `new branch (from ${branchInput.startPoint})…` : "new branch (from HEAD)…"}
                                value={branchText}
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                                onChange={(e) => setBranchText(e.target.value)}
                                onKeyDown={(e) => {
                                    if (e.key === "Enter") doCreateBranch(branchText, branchInput.startPoint);
                                    else if (e.key === "Escape") closeBranchInput();
                                    e.stopPropagation();
                                }}
                            />
                        </div>
                    )}
                    {searchOpen && (
                        <div className="git-commit-bar">
                            <input
                                ref={searchInputRef}
                                className="git-commit-input"
                                placeholder={`filter ${panel}…`}
                                value={searchByPanel[panel]}
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                                onChange={(e) => setSearchByPanel((s) => ({ ...s, [panel]: e.target.value }))}
                                onKeyDown={(e) => {
                                    if (e.key === "Enter" || e.key === "Escape") {
                                        setSearchOpen(false);
                                        if (e.key === "Escape") {
                                            setSearchByPanel((s) => ({ ...s, [panel]: "" }));
                                        }
                                    }
                                    e.stopPropagation();
                                }}
                            />
                        </div>
                    )}

                    <GitPanelBlock
                        n={2}
                        label="Files"
                        focused={panel === "files"}
                        onFocus={() => setPanel("files")}
                        flex={panelFlex(panelFiles)}
                        actions={[
                            {
                                key: "a",
                                label: filteredFiles.some(hasUnstaged) ? "stage all" : "unstage all",
                                onClick: () => {
                                    const anyUnstaged = filteredFiles.some(hasUnstaged);
                                    void run("", () => (anyUnstaged ? git.stageAll(repo) : git.unstageAll(repo)));
                                },
                            },
                            { key: "s", label: "stash", tone: "warn", onClick: openFilesStashMenu },
                        ]}
                        rangeBadge={rangeBadge(filesRange)}
                        filterBadge={searchByPanel.files || null}>
                        {filteredFiles.length === 0 &&
                            (overviewLoading ? (
                                <SkeletonRows rows={5} label="Loading changed files" />
                            ) : (
                                <EmptyState
                                    tone={overviewError ? "error" : "neutral"}
                                    icon={overviewError ? <IconWarning size={14} /> : <IconGit size={14} />}
                                    title={overviewError ? "Git error" : fileQuery ? "No matches" : "Clean tree"}
                                    message={fileEmptyText}
                                />
                            ))}
                        <VirtualPanelRows
                            items={filteredFiles}
                            selectedIndex={sel.files}
                            focused={panelFiles}
                            getKey={(f) => f.path}
                            renderRow={(f, i) => (
                                <div
                                    role="button"
                                    tabIndex={sel.files === i ? 0 : -1}
                                    onFocus={() => {
                                        setPanel("files");
                                        setSel({ ...sel, files: i });
                                    }}
                                    className={`panel-row git-row${panelFiles && sel.files === i ? " sel" : ""}${isInRange(filesRange, i) ? " ranged" : ""}`}
                                    onClick={(event) => {
                                        event.currentTarget.focus();
                                        setPanel("files");
                                        setSel({ ...sel, files: i });
                                    }}>
                                    <span className={`gf-x${isStaged(f) ? " on" : ""}`}>{f.index.trim()}</span>
                                    <span className={`gf-y${hasUnstaged(f) ? " on" : ""}`}>{f.worktree.trim()}</span>
                                    <FileIcon name={basenameOf(f.path)} size={14} />
                                    <span className="git-path">{f.path}</span>
                                </div>
                            )}
                        />
                    </GitPanelBlock>

                    <GitPanelBlock
                        n={3}
                        label="Branches"
                        focused={panel === "branches"}
                        onFocus={() => setPanel("branches")}
                        flex={panelFlex(panel === "branches")}
                        actions={[
                            {
                                key: "n",
                                label: "new",
                                onClick: () => openBranchInput(),
                            },
                            { key: "M", label: "merge", onClick: () => openBranchVerbsMenu("merge") },
                        ]}
                        rangeBadge={rangeBadge(branchesRange)}
                        filterBadge={searchByPanel.branches || null}>
                        {filteredBranches.length === 0 &&
                            (overviewLoading ? (
                                <SkeletonRows rows={5} label="Loading branches" />
                            ) : (
                                <EmptyState
                                    tone={overviewError ? "error" : "neutral"}
                                    icon={overviewError ? <IconWarning size={14} /> : undefined}
                                    title={overviewError ? "Git error" : branchQuery ? "No matches" : "No branches"}
                                    message={branchEmptyText}
                                />
                            ))}
                        <VirtualPanelRows
                            items={filteredBranches}
                            selectedIndex={sel.branches}
                            focused={panel === "branches"}
                            getKey={(b) => b.name}
                            renderRow={(b, i) => (
                                <div
                                    role="button"
                                    tabIndex={sel.branches === i ? 0 : -1}
                                    onFocus={() => {
                                        setPanel("branches");
                                        setSel({ ...sel, branches: i });
                                    }}
                                    className={`panel-row git-row${panel === "branches" && sel.branches === i ? " sel" : ""}${isInRange(branchesRange, i) ? " ranged" : ""}`}
                                    onClick={(event) => {
                                        event.currentTarget.focus();
                                        setPanel("branches");
                                        setSel({ ...sel, branches: i });
                                    }}>
                                    <span className={`gb-dot${b.current ? " cur" : ""}`} />
                                    <span className="git-path">{b.name}</span>
                                    {b.current && status && (status.ahead > 0 || status.behind > 0) && (
                                        <span className="branch-track">
                                            {status.ahead > 0 && <span className="branch-track-up">↑{status.ahead}</span>}
                                            {status.behind > 0 && <span className="branch-track-down">↓{status.behind}</span>}
                                        </span>
                                    )}
                                </div>
                            )}
                        />
                    </GitPanelBlock>

                    <GitPanelBlock
                        n={4}
                        label="Commits"
                        focused={panel === "commits"}
                        onFocus={() => setPanel("commits")}
                        flex={panelFlex(panel === "commits")}
                        actions={[
                            { key: "b", label: "branch", onClick: openCommitBranchPrompt },
                            { key: "r", label: "reset", tone: "warn", onClick: openCommitResetMenu },
                            { key: "v", label: "revert", tone: "danger", onClick: openCommitRevertConfirm },
                        ]}
                        rangeBadge={rangeBadge(commitsRange)}
                        filterBadge={searchByPanel.commits || null}>
                        {filteredCommits.length === 0 ? (
                            overviewLoading ? (
                                <SkeletonRows rows={8} label="Loading commits" />
                            ) : (
                                <EmptyState
                                    tone={overviewError ? "error" : "neutral"}
                                    icon={overviewError ? <IconWarning size={14} /> : undefined}
                                    title={overviewError ? "Git error" : commitQuery ? "No matches" : "No commits"}
                                    message={commitEmptyText}
                                />
                            )
                        ) : (
                            <GitGraph
                                commits={filteredCommits}
                                selectedIndex={Math.min(sel.commits, filteredCommits.length - 1)}
                                focused={panel === "commits"}
                                range={commitsRange}
                                onSelect={(i) => {
                                    setPanel("commits");
                                    setSel({ ...sel, commits: i });
                                }}
                                onActivate={openCommitRowMenu}
                            />
                        )}
                    </GitPanelBlock>

                    {panel === "remotes" && (
                        <GitPanelBlock
                            n={5}
                            label={remoteDrill ? `Remotes · ${remoteDrill}` : "Remotes"}
                            focused={panel === "remotes"}
                            onFocus={() => {
                                setPanel("remotes");
                                if (remoteDrill) setRemoteDrill(null);
                            }}
                            flex={2.6}
                            actions={[
                                { key: "n", label: "add", onClick: openAddRemotePrompt },
                                { key: "F", label: "fetch", onClick: () => doFetch(null) },
                            ]}
                            rangeBadge={rangeBadge(remotesRange)}
                            filterBadge={searchByPanel.remotes || null}>
                            {remoteDrill ? (
                                <>
                                    <button
                                        type="button"
                                        className="panel-row git-row git-row-back"
                                        onClick={() => setRemoteDrill(null)}
                                        title="Back to remotes (esc)">
                                        <IconChevron size={10} className="git-row-back-glyph" />
                                        <span className="git-path">{remoteDrill}</span>
                                        <span className="panel-row-hint">esc to go back</span>
                                    </button>
                                    {filteredRemoteBranches.length === 0 &&
                                        (remoteBranchesRes.status === "loading" ? (
                                            <SkeletonRows rows={4} label="Loading remote branches" />
                                        ) : (
                                            <EmptyState
                                                tone={remoteBranchesRes.status === "error" && !remoteBranchesRes.data ? "error" : "neutral"}
                                                icon={
                                                    remoteBranchesRes.status === "error" && !remoteBranchesRes.data ? (
                                                        <IconWarning size={14} />
                                                    ) : undefined
                                                }
                                                message={
                                                    remoteBranchesRes.status === "error" && !remoteBranchesRes.data
                                                        ? (remoteBranchesRes.error ?? "failed to load remote branches")
                                                        : remotesQuery
                                                          ? "no matches"
                                                          : `no branches under ${remoteDrill}/`
                                                }
                                            />
                                        ))}
                                    {filteredRemoteBranches.map((rb, i) => {
                                        return (
                                            <div
                                                key={rb.full_ref}
                                                role="button"
                                                tabIndex={remoteBranchSel === i ? 0 : -1}
                                                onFocus={() => {
                                                    setPanel("remotes");
                                                    setRemoteBranchSel(remoteDrill, i);
                                                }}
                                                className={`panel-row git-row${
                                                    panel === "remotes" && remoteBranchSel === i ? " sel" : ""
                                                }${isInRange(remotesRange, i) ? " ranged" : ""}${rb.is_head_pointer ? " muted" : ""}`}
                                                onClick={(event) => {
                                                    event.currentTarget.focus();
                                                    setPanel("remotes");
                                                    setRemoteBranchSel(remoteDrill, i);
                                                }}
                                                title={rb.is_head_pointer ? `${rb.full_ref} (HEAD pointer)` : rb.full_ref}>
                                                <span className="gb-dot remote" />
                                                <span className="git-path">{rb.name}</span>
                                                {rb.tracked_by && <span className="git-tracked">↻ {rb.tracked_by}</span>}
                                                {rb.is_head_pointer && <span className="panel-row-hint">HEAD</span>}
                                            </div>
                                        );
                                    })}
                                </>
                            ) : (
                                <>
                                    {filteredRemotes.length === 0 &&
                                        (remotesRes.status === "loading" ? (
                                            <SkeletonRows rows={3} label="Loading remotes" />
                                        ) : remotesRes.status === "error" && !remotesRes.data ? (
                                            <EmptyState
                                                tone="error"
                                                icon={<IconWarning size={14} />}
                                                message={remotesRes.error ?? "failed to load remotes"}
                                            />
                                        ) : remotesQuery ? (
                                            <EmptyState message="no matches" />
                                        ) : (
                                            <EmptyState
                                                icon={<IconGit size={14} />}
                                                title="No remotes"
                                                message="This repository has no remotes configured yet."
                                                action={{ label: "Add remote", onClick: openAddRemotePrompt }}
                                            />
                                        ))}
                                    {filteredRemotes.map((r, i) => {
                                        return (
                                            <div
                                                key={r.name}
                                                role="button"
                                                tabIndex={sel.remotes === i ? 0 : -1}
                                                onFocus={() => {
                                                    setPanel("remotes");
                                                    setSel({ ...sel, remotes: i });
                                                }}
                                                className={`panel-row git-row${panel === "remotes" && sel.remotes === i ? " sel" : ""}${isInRange(remotesRange, i) ? " ranged" : ""}`}
                                                onClick={(event) => {
                                                    event.currentTarget.focus();
                                                    setPanel("remotes");
                                                    setSel({ ...sel, remotes: i });
                                                }}
                                                onDoubleClick={() => setRemoteDrill(r.name)}
                                                title={r.url}>
                                                <span className="gb-dot remote" />
                                                <span className="git-path">{r.name}</span>
                                                <span className="git-remote-url">{r.url}</span>
                                            </div>
                                        );
                                    })}
                                </>
                            )}
                        </GitPanelBlock>
                    )}

                    {(stashes.length > 0 || panel === "stashes") && (
                        <GitPanelBlock
                            n={6}
                            label="Stashes"
                            focused={panel === "stashes"}
                            onFocus={() => setPanel("stashes")}
                            flex={panelFlex(panel === "stashes")}
                            actions={[
                                { key: "p", label: "pop", onClick: popSelectedStash, disabled: filteredStashes.length === 0 },
                                { key: "d", label: "drop", tone: "danger", onClick: openStashDropConfirm, disabled: filteredStashes.length === 0 },
                            ]}
                            rangeBadge={rangeBadge(stashesRange)}
                            filterBadge={searchByPanel.stashes || null}>
                            {filteredStashes.length === 0 &&
                                (stashesRes.status === "loading" ? (
                                    <SkeletonRows rows={3} label="Loading stashes" />
                                ) : (
                                    <EmptyState
                                        message={stashQuery ? "No matching stashes." : "No stashed changes."}
                                        action={
                                            !stashQuery && files.length ? { label: "Stash working changes", onClick: openFilesStashMenu } : undefined
                                        }
                                    />
                                ))}
                            {filteredStashes.map((s, i) => {
                                return (
                                    <div
                                        key={s.refname}
                                        role="button"
                                        tabIndex={sel.stashes === i ? 0 : -1}
                                        onFocus={() => {
                                            setPanel("stashes");
                                            setSel({ ...sel, stashes: i });
                                        }}
                                        className={`panel-row git-row${panel === "stashes" && sel.stashes === i ? " sel" : ""}${isInRange(stashesRange, i) ? " ranged" : ""}`}
                                        onClick={(event) => {
                                            event.currentTarget.focus();
                                            setPanel("stashes");
                                            setSel({ ...sel, stashes: i });
                                        }}
                                        onDoubleClick={openStashRowMenu}>
                                        <span className="gc-hash">{s.refname}</span>
                                        <span className="git-path">{s.message}</span>
                                        {s.branch && <span className="panel-row-hint">{s.branch}</span>}
                                    </div>
                                );
                            })}
                        </GitPanelBlock>
                    )}

                    <GitCmdLogBar />
                </div>

                <div className="git-right" ref={rightRef}>
                    <div className="git-right-review">
                        <Suspense fallback={<SkeletonRows rows={6} label="Loading diff preview" />}>
                            {right.mode === "merge" ? (
                                <MergeReview
                                    repo={repo}
                                    files={right.files}
                                    focusPath={filteredFiles[Math.min(sel.files, filteredFiles.length - 1)]?.path}
                                    onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                                    onSaved={() => void overview.refresh().catch(reportError("git refresh"))}
                                />
                            ) : right.mode === "commit" ? (
                                <CommitReview
                                    key={right.rev}
                                    repo={repo}
                                    rev={right.rev}
                                    title={right.title}
                                    subtitle={right.subtitle}
                                    onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                                />
                            ) : (
                                <pre className="git-output">{right.text || "—"}</pre>
                            )}
                        </Suspense>
                        {busy && (
                            <div className="git-busy-overlay">
                                <div className={`git-busy-card${busy.startsWith("✗") ? " error" : ""}`}>
                                    {!busy.startsWith("✗") && <span className="git-busy-spinner" />}
                                    <span className="git-busy-label">{busy}</span>
                                </div>
                            </div>
                        )}
                    </div>
                </div>
            </div>

            <GitModalRenderer paneId={paneId} active={active} />
        </div>
    );
}
