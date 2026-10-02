import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import {
    git,
    hasUnstaged,
    isStaged,
    type GitBranch,
    type GitCommit,
    type GitFile,
    type GitRemote,
    type GitRemoteBranch,
    type GitStash,
} from "../api/git";
import * as cmd from "../state/commands";
import { openGitCheatsheet, openGitConfirm, openGitMenu, openGitPrompt, toggleGitCmdLog } from "../state/git";
import { invalidate, useCachedResourceEnabled } from "../state/resources";
import { gitDiscoveredReposR, gitOverviewR, gitRemoteBranchesR, gitRemotesR, gitStashesR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { commitGitDraft, generateGitDraft, runRepositoryGit, useGitWorkbench } from "../state/gitWorkbench";
import { whenStageStill } from "../state/nativeViews";
import { errMessage, notify } from "../state/toast";
import { DEFAULT_GIT_VIEW, type GitPanel } from "../state/types";
import { copyText } from "../lib/clipboard";
import { GitHostShell } from "../codehost/components/GitHostShell";
import { BranchPullChip } from "../codehost/components/BranchPullChip";
import { useBranchPulls, useHostRepo } from "../codehost/project";
import { compose, showItem } from "../codehost/state";
import type { Pull } from "../codehost/types";
import { FileIcon } from "../ui/FileIcon";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import {
    IconCheckout,
    IconChevron,
    IconCopy,
    IconDiscard,
    IconFetch,
    IconFile,
    IconGit,
    IconMerge,
    IconMinus,
    IconMore,
    IconPencil,
    IconPlus,
    IconPull,
    IconPullRequest,
    IconPush,
    IconSearch,
    IconTrash,
    IconWarning,
} from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { GitCmdLogBar } from "./GitCmdLogBar";
import { GitComposer } from "./GitComposer";
import { AuthorAvatar } from "./AuthorAvatar";
import { GitGraph } from "./GitGraph";
import { GitModalRenderer } from "./GitModalRenderer";
import { VirtualPanelRows } from "./VirtualPanelRows";
import { GitColumns } from "./GitColumns";
import { HISTORY_CLEARANCE, HISTORY_MIN, ResizeHandle } from "./ResizeHandle";
import { SkeletonRows } from "../ui/Skeleton";
import { EmptyState } from "../ui/Panel";
import { AI_MODELS, AI_PROVIDER_LABEL, GIT_HELP, GIT_PANEL_BY_KEY, defaultAiModel } from "./gitPaneConstants";
import { filterByQuery, isInRange } from "./gitPaneLogic";
import type { GitAiProvider } from "./gitPaneTypes";
import { basename as basenameOf, dirname } from "../lib/paths";

const CommitReview = lazy(() => import("./CommitReview").then((module) => ({ default: memo(module.CommitReview) })));
const MergeReview = lazy(() => import("./MergeReview").then((module) => ({ default: memo(module.MergeReview) })));

const ROW_HEIGHT = 26;

type Side = "staged" | "unstaged";
type FileEntry = { kind: "group"; side: Side; count: number } | { kind: "file"; side: Side; file: GitFile };
type BranchEntry =
    | { kind: "local"; branch: GitBranch }
    | { kind: "remote"; remote: GitRemote; open: boolean }
    | { kind: "remoteBranch"; remote: string; branch: GitRemoteBranch };

const branchEntryKey = (entry: BranchEntry) =>
    entry.kind === "local" ? `l:${entry.branch.name}` : entry.kind === "remote" ? `r:${entry.remote.name}` : `rb:${entry.branch.full_ref}`;

const firstLine = (text: string) => text.trim().split("\n")[0] ?? "";

function RowButton({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
    return (
        <Tooltip label={label}>
            <button
                type="button"
                tabIndex={-1}
                className="git-row-act"
                aria-label={label}
                onClick={(event) => {
                    event.stopPropagation();
                    onClick();
                }}>
                {children}
            </button>
        </Tooltip>
    );
}

function GitWorkbench({
    paneId,
    repo,
    active,
    fetching,
    onLeaveRepo,
}: {
    paneId: string;
    repo: string;
    active: boolean;
    fetching: boolean;
    onLeaveRepo: (() => void) | null;
}) {
    const paneRootRef = useRef<HTMLDivElement>(null);
    const historyRef = useRef<HTMLDivElement>(null);
    const storedView = useStore((s) => s.gitViews[paneId]);
    const view = {
        ...DEFAULT_GIT_VIEW,
        ...(storedView ?? {}),
        selected: { ...DEFAULT_GIT_VIEW.selected, ...(storedView?.selected ?? {}) },
    };
    const sel = view.selected;
    const panel: GitPanel = view.panel;
    const openRemote = view.openRemote;
    const modalOpen = useStore((s) => s.gitModal !== null);
    const cmdLogOpen = useStore((s) => s.gitCmdLogOpen);

    const overview = useCachedResourceEnabled(fetching && !!repo, gitOverviewR, repo || "");
    const hostRepo = useHostRepo(repo || null, fetching && !!repo);
    const branchPulls = useBranchPulls(hostRepo.repo, fetching && panel === "branches");
    const openBranchPull = (pull: Pull) => {
        cmd.setGitView(paneId, { area: "pulls" });
        showItem(paneId, pull.number);
    };
    const remotesRes = useCachedResourceEnabled(fetching && !!repo, gitRemotesR, repo || "");
    const stashesRes = useCachedResourceEnabled(fetching && !!repo, gitStashesR, repo || "");
    const remoteBranchesRes = useCachedResourceEnabled(fetching && !!repo && !!openRemote, gitRemoteBranchesR, repo || "", openRemote ?? "");
    const overviewLoading = !!repo && overview.status === "loading" && !overview.data;
    const overviewError = !!repo && overview.status === "error" && !overview.data ? (overview.error ?? "failed to load git state") : null;
    const status = repo ? (overview.data?.status ?? null) : null;
    const branches = useMemo(() => (repo ? (overview.data?.branches ?? []) : []), [repo, overview.data?.branches]);
    const commits = useMemo(() => (repo ? (overview.data?.log ?? []) : []), [repo, overview.data?.log]);
    const files = useMemo(() => status?.files ?? [], [status?.files]);
    const remotes = useMemo(() => (repo ? (remotesRes.data ?? []) : []), [repo, remotesRes.data]);
    const stashes = useMemo(() => (repo ? (stashesRes.data ?? []) : []), [repo, stashesRes.data]);
    const remoteBranches = useMemo(() => (repo && openRemote ? (remoteBranchesRes.data ?? []) : []), [repo, openRemote, remoteBranchesRes.data]);
    const currentBranch = branches.find((b) => b.current)?.name ?? status?.branch ?? "";
    const onReviewSaved = useCallback(() => invalidate((kind, args) => kind === "git.overview" && args[0] === repo), [repo]);

    const [localBusy, setBusy] = useState<string | null>(null);
    const messageRef = useRef<HTMLTextAreaElement>(null);
    const aiProvider = useGitWorkbench((state) => state.provider);
    const aiModel = useGitWorkbench((state) => state.model);
    const sharedOperation = useGitWorkbench((state) => state.operations[repo]);
    const busy = sharedOperation?.busy ? sharedOperation.label : localBusy;
    const generating = !!sharedOperation?.busy && sharedOperation.label.startsWith("Generate message");
    const [menu, setMenu] = useState<{ x: number; y: number; items: CtxItem[]; alignRight: boolean } | null>(null);
    const [queries, setQueries] = useState<Record<GitPanel, string>>({ files: "", commits: "", branches: "" });
    const [fileFilterOpen, setFileFilterOpen] = useState(false);
    const [commitSearchOpen, setCommitSearchOpen] = useState(false);
    const filterInputs = useRef<Record<GitPanel, HTMLInputElement | null>>({ files: null, commits: null, branches: null });
    const [rangeAnchor, setRangeAnchor] = useState<number | null>(null);

    // Every operation runs through the workbench, so its outcome is reported once, here.
    const reported = useRef(sharedOperation);
    useEffect(() => {
        if (!sharedOperation || sharedOperation === reported.current || sharedOperation.busy) return;
        reported.current = sharedOperation;
        if (sharedOperation.error) notify("error", firstLine(sharedOperation.error));
        else if (sharedOperation.result && sharedOperation.result !== `${sharedOperation.label} completed`)
            notify("success", firstLine(sharedOperation.result));
    }, [sharedOperation]);

    useEffect(() => {
        if (fileFilterOpen) filterInputs.current.files?.focus();
    }, [fileFilterOpen]);

    useEffect(() => {
        if (commitSearchOpen) filterInputs.current.commits?.focus();
    }, [commitSearchOpen]);

    const filteredFiles = useMemo(() => filterByQuery(files, queries.files, (f) => [f.path]), [files, queries.files]);
    const filteredCommits = useMemo(() => filterByQuery(commits, queries.commits, (c) => [c.subject, c.hash, c.author]), [commits, queries.commits]);
    const stagedFiles = useMemo(() => filteredFiles.filter(isStaged), [filteredFiles]);
    const unstagedFiles = useMemo(() => filteredFiles.filter(hasUnstaged), [filteredFiles]);

    const fileEntries = useMemo<FileEntry[]>(() => {
        const side = (name: Side, list: GitFile[]): FileEntry[] =>
            list.length
                ? [{ kind: "group", side: name, count: list.length }, ...list.map((file) => ({ kind: "file" as const, side: name, file }))]
                : [];
        return [...side("staged", stagedFiles), ...side("unstaged", unstagedFiles)];
    }, [stagedFiles, unstagedFiles]);

    const branchEntries = useMemo<BranchEntry[]>(() => {
        const q = queries.branches.toLowerCase();
        const match = (name: string) => !q || name.toLowerCase().includes(q);
        const entries: BranchEntry[] = branches.filter((b) => match(b.name)).map((branch) => ({ kind: "local", branch }));
        for (const remote of remotes) {
            const open = remote.name === openRemote;
            entries.push({ kind: "remote", remote, open });
            if (open)
                for (const branch of remoteBranches)
                    if (!branch.is_head_pointer && match(branch.name)) entries.push({ kind: "remoteBranch", remote: remote.name, branch });
        }
        return entries;
    }, [branches, remotes, remoteBranches, openRemote, queries.branches]);

    const lenFor = (p: GitPanel) => (p === "files" ? fileEntries.length : p === "commits" ? filteredCommits.length : branchEntries.length);
    const clampSel = (p: GitPanel) => Math.max(0, Math.min(lenFor(p) - 1, sel[p]));
    const selectedFileEntry = fileEntries[clampSel("files")];
    const selectedFile = selectedFileEntry?.kind === "file" ? selectedFileEntry : fileEntries.find((e) => e.kind === "file");
    const selectedFileIndex = selectedFile ? fileEntries.indexOf(selectedFile) : -1;
    const selectedCommit = filteredCommits[clampSel("commits")];
    const selectedBranchEntry = branchEntries[clampSel("branches")];

    const setPanel = (p: GitPanel) => cmd.setGitView(paneId, { panel: p });
    const setSel = (p: GitPanel, index: number) => cmd.setGitView(paneId, { panel: p, selected: { ...sel, [p]: index } });
    const historyOpen = view.historyOpen;
    const setHistoryOpen = (open: boolean) =>
        cmd.setGitView(paneId, open ? { historyOpen: true } : { historyOpen: false, panel: panel === "commits" ? "files" : panel });
    const setOpenRemote = (name: string | null) => cmd.setGitView(paneId, { openRemote: name });

    const errorTimerRef = useRef<number | undefined>(undefined);
    async function run<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
        window.clearTimeout(errorTimerRef.current);
        setBusy(label || "running");
        try {
            if (useGitWorkbench.getState().operations[repo]?.busy) return undefined;
            let out: T | undefined;
            const succeeded = await runRepositoryGit(repo, label || "Git operation", async () => {
                out = await fn();
                return out;
            });
            if (!succeeded) throw new Error(useGitWorkbench.getState().operations[repo]?.error || "Another Git operation is running.");
            setBusy(null);
            return out;
        } catch (err) {
            const msg = errMessage(err);
            setBusy(`✗ ${msg.length > 80 ? msg.slice(0, 80) + "…" : msg}`);
            errorTimerRef.current = window.setTimeout(() => setBusy(null), 3500);
            return undefined;
        } finally {
            void overview.refresh().catch(() => {});
            void stashesRes.refresh().catch(() => {});
        }
    }

    const refreshRepoState = () => {
        void overview.refresh().catch(() => {});
        void stashesRes.refresh().catch(() => {});
        void remotesRes.refresh().catch(() => {});
        if (openRemote) void remoteBranchesRes.refresh().catch(() => {});
    };

    // Menus open from their button's nearer edge, so one at the pane's right edge stays over the pane.
    const openMenuAt = (anchor: HTMLElement, items: CtxItem[]) => {
        const rect = anchor.getBoundingClientRect();
        const pane = paneRootRef.current?.getBoundingClientRect();
        const alignRight = !!pane && rect.left - pane.left > pane.width / 2;
        setMenu({ x: alignRight ? rect.right : rect.left, y: rect.bottom + 4, items, alignRight });
    };

    // ---- Changes ----

    const stageFile = (file: GitFile) => void run("", () => git.stage(repo, file.path));
    const unstageFile = (file: GitFile) => void run("", () => git.unstage(repo, file.path));
    const stageAll = () => void run("", () => git.stageAll(repo));
    const unstageAll = () => void run("", () => git.unstageAll(repo));
    const discardFile = (file: GitFile) =>
        openGitConfirm({
            title: `Discard changes to ${basenameOf(file.path)}?`,
            body:
                file.worktree === "?"
                    ? "The file is new and will be deleted. This can't be undone."
                    : "Unstaged changes to this file will be lost. This can't be undone.",
            destructive: true,
            confirmLabel: "discard",
            onConfirm: () => run(`discarding ${file.path}`, () => git.discardFiles(repo, [file.path], "unstaged")).then(() => {}),
        });

    const discardAllUnstaged = () => {
        const targets = unstagedFiles;
        if (targets.length === 0) return;
        const added = targets.filter((f) => f.worktree === "?").length;
        const count = `${targets.length} file${targets.length === 1 ? "" : "s"}`;
        openGitConfirm({
            title: `Discard unstaged changes in ${count}?`,
            body: `${added ? `${added} new file${added === 1 ? " is" : "s are"} deleted and the rest go back to what is staged or committed. ` : "Every file goes back to what is staged or committed. "}Staged changes are kept. This can't be undone.`,
            destructive: true,
            confirmLabel: "discard all",
            onConfirm: () =>
                run(`discarding ${count}`, () =>
                    git.discardFiles(
                        repo,
                        targets.map((f) => f.path),
                        "unstaged",
                    ),
                ).then(() => {}),
        });
    };

    const fileRange = (): FileEntry[] | null => {
        if (rangeAnchor === null) return null;
        const a = Math.min(rangeAnchor, selectedFileIndex);
        const b = Math.max(rangeAnchor, selectedFileIndex);
        return fileEntries.slice(a, b + 1);
    };

    const toggleStage = () => {
        const range = fileRange();
        if (range) {
            const paths = [...new Set(range.flatMap((e) => (e.kind === "file" && e.side === "unstaged" ? [e.file.path] : [])))];
            setRangeAnchor(null);
            if (paths.length) void run("staging range", () => git.stagePaths(repo, paths));
            return;
        }
        if (!selectedFile) return;
        if (selectedFile.side === "staged") unstageFile(selectedFile.file);
        else stageFile(selectedFile.file);
    };

    const openFilesDiscardMenu = () => {
        const range = fileRange();
        const targets = range
            ? [...new Map(range.flatMap((e) => (e.kind === "file" ? [[e.file.path, e.file] as const] : []))).values()]
            : selectedFile
              ? [selectedFile.file]
              : [];
        if (targets.length === 0) return;
        const label = targets.length > 1 ? `Discard ${targets.length} files` : `Discard changes — ${basenameOf(targets[0].path)}`;
        const apply = (mode: "all" | "unstaged" | "staged", confirmKey: "d" | "u" | "s") => () =>
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
                    await run(`discarding ${targets.length} file${targets.length > 1 ? "s" : ""} (${mode})`, () =>
                        git.discardFiles(
                            repo,
                            targets.map((t) => t.path),
                            mode,
                        ),
                    );
                },
            });
        openGitMenu(label, [
            { key: "d", label: "discard all changes", hint: "staged + worktree", destructive: true, run: apply("all", "d") },
            { key: "u", label: "discard unstaged changes", destructive: true, run: apply("unstaged", "u") },
            { key: "s", label: "unstage changes", run: apply("staged", "s") },
        ]);
    };

    const openFilesStashMenu = () => {
        const stash = (mode: "all" | "staged" | "unstaged") => () =>
            openGitPrompt({
                title: mode === "all" ? "Stash everything" : mode === "staged" ? "Stash staged changes" : "Stash unstaged changes",
                placeholder: "(optional) stash message",
                onConfirm: (msg) => {
                    void run(`stashing (${mode})`, async () => {
                        await git.stashPush(repo, mode, msg || null);
                        return `Stashed ${mode === "all" ? "your changes" : `${mode} changes`}`;
                    });
                },
            });
        openGitMenu("Stash", [
            { key: "s", label: "stash everything (working tree + index + untracked)", run: stash("all") },
            { key: "i", label: "stash staged only", run: stash("staged") },
            { key: "u", label: "stash unstaged only (keep index)", run: stash("unstaged") },
        ]);
    };

    const doCommit = () => void commitGitDraft(repo).then(() => overview.refresh().catch(() => {}));
    const generateCommitMessage = () => void generateGitDraft(repo);

    const openAgentMenu = (anchor: HTMLElement) =>
        openMenuAt(
            anchor,
            (Object.keys(AI_MODELS) as GitAiProvider[]).flatMap((provider, i) => [
                ...(i > 0 ? [{ sep: true }] : []),
                ...AI_MODELS[provider].map((model) => ({
                    label: `${AI_PROVIDER_LABEL[provider]} · ${model}`,
                    hint: provider === aiProvider && model === aiModel ? "✓" : undefined,
                    run: () => useGitWorkbench.setState({ provider, model }),
                })),
            ]),
        );

    // ---- Stashes ----

    const openStashMenu = (s: GitStash) =>
        openGitMenu(`${s.refname} · ${s.message}`, [
            {
                key: "a",
                label: "apply stash",
                hint: "keep stash",
                run: () => void run(`applying ${s.refname}`, () => git.stashApply(repo, s.refname, s.sha)),
            },
            {
                key: "p",
                label: "pop stash",
                hint: "apply + drop",
                run: () => void run(`popping ${s.refname}`, () => git.stashPop(repo, s.refname, s.sha)),
            },
            {
                key: "b",
                label: "create branch from stash",
                run: () =>
                    openGitPrompt({
                        title: `Branch from ${s.refname}`,
                        placeholder: "branch name",
                        onConfirm: (name) => {
                            const n = name.trim();
                            if (n) void run(`branching from ${s.refname}`, () => git.stashBranch(repo, s.refname, s.sha, n));
                        },
                    }),
            },
            {
                key: "r",
                label: "rename stash",
                run: () =>
                    openGitPrompt({
                        title: `Rename ${s.refname}`,
                        initial: s.message,
                        onConfirm: (message) => {
                            const m = message.trim();
                            if (m && m !== s.message) void run(`renaming ${s.refname}`, () => git.stashRename(repo, s.refname, s.sha, m));
                        },
                    }),
            },
            {
                key: "d",
                label: "drop stash",
                destructive: true,
                run: () =>
                    openGitConfirm({
                        title: `Drop ${s.refname}?`,
                        body: "This permanently deletes the stash entry.",
                        destructive: true,
                        confirmLabel: "drop",
                        onConfirm: () => run(`dropping ${s.refname}`, () => git.stashDrop(repo, s.refname, s.sha)).then(() => {}),
                    }),
            },
        ]);

    const openStashesMenu = () =>
        openGitMenu(
            "Stashes",
            stashes.map((s, i) => ({ key: i < 9 ? String(i + 1) : undefined, label: s.message, hint: s.refname, run: () => openStashMenu(s) })),
        );

    // ---- History ----

    const openCommitBranchPrompt = (c: GitCommit) =>
        openGitPrompt({
            title: `Branch from ${c.hash}`,
            placeholder: "branch name",
            onConfirm: (name) => {
                const n = name.trim();
                if (n)
                    void run(`creating ${n} from ${c.hash}`, async () => {
                        await git.branchCreate(repo, n, c.hash);
                        return `Created ${n} from ${c.hash} and switched to it`;
                    });
            },
        });

    const openCommitResetMenu = (c: GitCommit) => {
        const resetTo = (mode: "soft" | "mixed" | "hard") => () =>
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
                onConfirm: () => run(`reset --${mode} ${c.hash}`, () => git.reset(repo, c.hash, mode)).then(() => {}),
            });
        openGitMenu(`Reset to ${c.hash}`, [
            { key: "s", label: "soft reset", hint: "keep changes staged", run: resetTo("soft") },
            { key: "m", label: "mixed reset", hint: "keep changes unstaged", run: resetTo("mixed") },
            { key: "h", label: "hard reset", hint: "discard later changes", destructive: true, run: resetTo("hard") },
        ]);
    };

    const openCommitRevertConfirm = (c: GitCommit) =>
        openGitConfirm({
            title: `Revert ${c.hash}?`,
            body: "Creates a new commit that reverses this commit. Existing history is preserved.",
            confirmLabel: "revert",
            onConfirm: () => run(`reverting ${c.hash}`, () => git.revert(repo, c.hash)).then(() => {}),
        });

    const copyValue = (value: string, what: string) =>
        copyText(value).then(
            () => notify("success", `Copied ${what}`),
            (err) => notify("error", errMessage(err)),
        );

    const openCommitRowMenu = (c: GitCommit) =>
        openGitMenu(`${c.hash} · ${c.subject}`, [
            { key: "b", label: "create branch from commit", run: () => openCommitBranchPrompt(c) },
            { key: "r", label: "reset to this commit", hint: "choose soft / mixed / hard", run: () => openCommitResetMenu(c) },
            { key: "v", label: "revert this commit", hint: "new inverse commit", run: () => openCommitRevertConfirm(c) },
            { key: "y", label: "copy hash", run: () => void copyValue(c.full_hash || c.hash, c.hash) },
        ]);

    // ---- Branches ----

    const checkoutBranch = (name: string) => void run(`checking out ${name}`, () => git.checkout(repo, name));

    const openNewBranchPrompt = (startPoint = "") =>
        openGitPrompt({
            title: startPoint ? `New branch from ${startPoint}` : "New branch",
            placeholder: "branch name",
            onConfirm: (name) => {
                const n = name.trim();
                if (n)
                    void run("creating branch…", async () => {
                        await git.branchCreate(repo, n, startPoint || undefined);
                        return `Created ${n}${startPoint ? ` from ${startPoint}` : ""} and switched to it`;
                    });
            },
        });

    const openCheckoutPrompt = () =>
        openGitPrompt({
            title: "Checkout branch",
            placeholder: "branch name (- for previous)",
            suggestions: branches.map((b) => ({ value: b.name, hint: b.current ? "current" : (b.upstream ?? "") })),
            onConfirm: (name) => {
                const target = name.trim();
                if (target) checkoutBranch(target);
            },
        });

    const mergeRef = (ref: string, squash = false) =>
        void run(`${squash ? "squash " : ""}merging ${ref}…`, async () => {
            const out = await (squash ? git.mergeSquash(repo, ref) : git.merge(repo, ref));
            return squash ? `Squashed ${ref} into the index. Review and commit.` : `Merged ${ref}${out ? ` · ${firstLine(out)}` : ""}`;
        });

    const openMergeMenu = (ref: string) =>
        openGitMenu(`Merge ${ref} into ${currentBranch || "HEAD"}`, [
            { key: "m", label: "regular merge", run: () => mergeRef(ref) },
            { key: "s", label: "squash merge", run: () => mergeRef(ref, true) },
        ]);

    const openBranchRenamePrompt = (b: GitBranch) =>
        openGitPrompt({
            title: `Rename branch · ${b.name}`,
            initial: b.name,
            onConfirm: (name) => {
                const n = name.trim();
                if (n && n !== b.name) void run(`renaming branch ${b.name} → ${n}`, () => git.branchRename(repo, b.name, n));
            },
        });

    const confirmBranchDelete = (branch: string, force: boolean) =>
        openGitConfirm({
            title: `${force ? "Force delete" : "Delete"} ${branch}?`,
            body: force
                ? "This deletes the local branch even if it has commits that are not merged anywhere else."
                : "Deletes the local branch only. Use force delete if Git refuses because the branch is not merged.",
            destructive: true,
            confirmLabel: force ? "force delete" : "delete",
            onConfirm: () => run(`${force ? "force " : ""}deleting branch ${branch}`, () => git.branchDelete(repo, branch, force)).then(() => {}),
        });

    const openBranchDeleteMenu = (b: GitBranch) =>
        openGitMenu(`Delete ${b.name}`, [
            {
                key: "d",
                label: "delete local branch",
                destructive: true,
                disabled: b.current,
                hint: b.current ? "(can't delete current branch)" : undefined,
                run: () => confirmBranchDelete(b.name, false),
            },
            {
                key: "D",
                label: "force delete local branch",
                destructive: true,
                disabled: b.current,
                hint: b.current ? "(can't delete current branch)" : "git branch -D",
                run: () => confirmBranchDelete(b.name, true),
            },
        ]);

    const localBranchItems = (b: GitBranch): CtxItem[] => [
        ...(b.current
            ? []
            : [
                  { label: "Check out", hint: "↵", run: () => checkoutBranch(b.name) },
                  { label: `Merge into ${currentBranch || "HEAD"}`, hint: "M", run: () => mergeRef(b.name) },
                  { label: `Squash merge into ${currentBranch || "HEAD"}`, run: () => mergeRef(b.name, true) },
                  { sep: true },
              ]),
        { label: "New branch from here", hint: "n", run: () => openNewBranchPrompt(b.name) },
        { label: "Rename…", hint: "R", run: () => openBranchRenamePrompt(b) },
        { label: "Copy name", run: () => void copyValue(b.name, b.name) },
        ...(b.current ? [] : [{ sep: true }, { label: "Delete…", hint: "d", danger: true, run: () => openBranchDeleteMenu(b) }]),
    ];

    const checkoutRemoteBranch = (remote: string, rb: GitRemoteBranch) =>
        void run(`checking out ${rb.full_ref}`, async () => {
            await git.checkoutRemoteBranch(repo, remote, rb.name, rb.tracked_by ?? null);
            return `Switched to ${rb.tracked_by ?? rb.name}`;
        });

    const setUpstreamTo = (rb: GitRemoteBranch) => {
        if (!currentBranch) return;
        void run(`setting upstream of ${currentBranch}`, async () => {
            await git.setUpstream(repo, currentBranch, rb.full_ref);
            return `${currentBranch} now tracks ${rb.full_ref}`;
        });
    };

    const confirmRemoteBranchDelete = (remote: string, rb: GitRemoteBranch) =>
        openGitConfirm({
            title: `Delete ${rb.full_ref}?`,
            body: `Pushes a delete to ${remote}. The branch will be gone for everyone.`,
            destructive: true,
            confirmLabel: "delete on remote",
            onConfirm: () =>
                run(`deleting ${rb.full_ref}`, async () => {
                    await git.deleteRemoteBranch(repo, remote, rb.name);
                    await remoteBranchesRes.refresh();
                }).then(() => {}),
        });

    const remoteBranchItems = (remote: string, rb: GitRemoteBranch): CtxItem[] => [
        {
            label: rb.tracked_by ? `Check out ${rb.tracked_by}` : "Check out as a local branch",
            hint: "↵",
            run: () => checkoutRemoteBranch(remote, rb),
        },
        { label: `Merge into ${currentBranch || "HEAD"}`, hint: "M", run: () => mergeRef(rb.full_ref) },
        { label: `Set as upstream of ${currentBranch || "HEAD"}`, hint: "u", disabled: !currentBranch, run: () => setUpstreamTo(rb) },
        { label: "Copy name", run: () => void copyValue(rb.full_ref, rb.full_ref) },
        { sep: true },
        { label: `Delete on ${remote}…`, danger: true, run: () => confirmRemoteBranchDelete(remote, rb) },
    ];

    const doFetch = (remote: string | null) =>
        void run(remote ? `fetching ${remote}…` : "fetching all remotes…", async () => {
            const out = await git.fetch(repo, remote);
            await remotesRes.refresh();
            if (openRemote) await remoteBranchesRes.refresh();
            return out.trim() ? out : `Fetched ${remote ?? "all remotes"}`;
        });

    const openAddRemotePrompt = () =>
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
                            return `Added remote ${n}`;
                        });
                    },
                });
            },
        });

    const remoteItems = (r: GitRemote): CtxItem[] => [
        { label: `Fetch ${r.name}`, hint: "f", run: () => doFetch(r.name) },
        {
            label: "Edit URL…",
            run: () =>
                openGitPrompt({
                    title: `Edit url · ${r.name}`,
                    initial: r.url,
                    onConfirm: (u) => {
                        const url = u.trim();
                        if (url && url !== r.url)
                            void run(`setting url for ${r.name}`, async () => {
                                await git.remoteSetUrl(repo, r.name, url);
                                await remotesRes.refresh();
                            });
                    },
                }),
        },
        {
            label: "Rename…",
            run: () =>
                openGitPrompt({
                    title: `Rename remote · ${r.name}`,
                    initial: r.name,
                    onConfirm: (n) => {
                        const next = n.trim();
                        if (next && next !== r.name)
                            void run(`renaming ${r.name} → ${next}`, async () => {
                                await git.remoteRename(repo, r.name, next);
                                await remotesRes.refresh();
                                if (openRemote === r.name) setOpenRemote(next);
                            });
                    },
                }),
        },
        { label: "Copy URL", run: () => void copyValue(r.url, `${r.name} URL`) },
        { sep: true },
        {
            label: "Remove remote…",
            danger: true,
            run: () =>
                openGitConfirm({
                    title: `Remove remote ${r.name}?`,
                    body: "Removes the local remote configuration. Won't touch the upstream repo.",
                    destructive: true,
                    confirmLabel: "remove",
                    onConfirm: () =>
                        run(`removing remote ${r.name}`, async () => {
                            await git.remoteRemove(repo, r.name);
                            await remotesRes.refresh();
                        }).then(() => {}),
                }),
        },
    ];

    const activateBranchEntry = (entry: BranchEntry | undefined) => {
        if (!entry) return;
        if (entry.kind === "local") {
            if (!entry.branch.current) checkoutBranch(entry.branch.name);
        } else if (entry.kind === "remote") setOpenRemote(entry.open ? null : entry.remote.name);
        else checkoutRemoteBranch(entry.remote, entry.branch);
    };

    // ---- Toolbar ----

    const pushRepo = () => void run("pushing…", async () => `Pushed · ${firstLine(await git.push(repo))}`);
    const pullRepo = () => void run("pulling…", async () => `Pulled · ${firstLine(await git.pull(repo))}`);
    const openPullRequest = () => {
        if (hostRepo.repo) {
            cmd.setGitView(paneId, { area: "pulls" });
            compose(paneId, "pull");
            return;
        }
        void run("opening PR…", async () => {
            const url = await git.prOpen(repo);
            return `Opened the pull request page · ${url}`;
        });
    };

    const openBranchPicker = (anchor: HTMLElement) =>
        openMenuAt(anchor, [
            { label: "New branch…", hint: "N", run: () => openNewBranchPrompt() },
            { label: "Check out by name…", hint: "c", run: openCheckoutPrompt },
            { label: "Show all branches", hint: "3", run: () => setPanel("branches") },
            { sep: true },
            ...branches.map((b) => ({
                label: b.name,
                hint: b.current ? "current" : (b.upstream ?? undefined),
                disabled: b.current,
                run: () => checkoutBranch(b.name),
            })),
        ]);

    const openHelpCheatsheet = () => openGitCheatsheet("Git pane keybindings", GIT_HELP);

    const openMoreMenu = (anchor: HTMLElement) =>
        openMenuAt(anchor, [
            { label: "Fetch all remotes", hint: "F", run: () => doFetch(null) },
            { label: "Add remote…", run: openAddRemotePrompt },
            { sep: true },
            { label: "Stash changes…", hint: "s", disabled: files.length === 0, run: openFilesStashMenu },
            { label: "Discard unstaged changes…", danger: true, disabled: unstagedFiles.length === 0, run: discardAllUnstaged },
            { label: stashes.length ? `Stashes (${stashes.length})…` : "No stashes", disabled: stashes.length === 0, run: openStashesMenu },
            { sep: true },
            { label: "Refresh", hint: "r", run: refreshRepoState },
            { label: cmdLogOpen ? "Hide command log" : "Show command log", hint: "@", run: toggleGitCmdLog },
            { label: "Keyboard shortcuts", hint: "?", run: openHelpCheatsheet },
        ]);

    // ---- Keyboard ----

    const moveSel = (d: number) => {
        const len = lenFor(panel);
        if (len === 0) return;
        let next = Math.max(0, Math.min(len - 1, (panel === "files" ? selectedFileIndex : clampSel(panel)) + d));
        if (panel === "files") {
            while (fileEntries[next]?.kind === "group" && next > 0 && next < len - 1) next += d;
            if (fileEntries[next]?.kind === "group") next = fileEntries.findIndex((e, i) => e.kind === "file" && (d > 0 ? i > next : true));
            if (next < 0) return;
        }
        setSel(panel, next);
    };

    const focusFilter = () => {
        if (panel === "files") setFileFilterOpen(true);
        else if (panel === "commits") setCommitSearchOpen(true);
        else filterInputs.current[panel]?.focus();
    };

    // Rebuilt every render because it closes over the current selection, but
    // registered once so the window keeps a single listener.
    const onKeyRef = useRef<(e: KeyboardEvent) => void>(() => {});
    onKeyRef.current = (e: KeyboardEvent) => {
        if (e.altKey || e.metaKey) return;
        if (useStore.getState().pickerOpen) return;
        const ae = document.activeElement;
        if (!ae || !paneRootRef.current?.contains(ae) || ae.closest('[role="dialog"], [role="listbox"], [role="separator"], [role="menu"]')) return;
        if (e.key === "Tab") return;
        if (ae.closest('button, [role="button"]') && !ae.closest(".git-row, .gg-row") && ["Enter", " ", "ArrowUp", "ArrowDown"].includes(e.key))
            return;
        if (ae.closest('input, textarea, [contenteditable="true"]')) return;
        if (ae.closest(".cm-editor")) return;
        if (view.area !== "local") return;
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
        else if (k === "/") focusFilter();
        else if (k === "Escape") {
            if (rangeAnchor !== null) setRangeAnchor(null);
            else if (queries[panel]) setQueries((q) => ({ ...q, [panel]: "" }));
            else handled = false;
        } else if (GIT_PANEL_BY_KEY[k]) setPanel(GIT_PANEL_BY_KEY[k]!);
        else if (k === "h" && panel !== "branches") {
            if (historyOpen) setHistoryOpen(false);
            else cmd.setGitView(paneId, { historyOpen: true, panel: "commits" });
        } else if (k === "j" || k === "ArrowDown") moveSel(1);
        else if (k === "k" || k === "ArrowUp") moveSel(-1);
        else if (k === "r" && panel === "commits" && selectedCommit) openCommitResetMenu(selectedCommit);
        else if (k === "r") refreshRepoState();
        else if (k === "F") doFetch(null);
        else if (k === "P") pushRepo();
        else if (k === "p") pullRepo();
        else if (panel === "files" && k === "v") setRangeAnchor((a) => (a === null ? selectedFileIndex : null));
        else if (panel === "files" && k === " ") toggleStage();
        else if (panel === "files" && k === "a") (unstagedFiles.length ? stageAll : unstageAll)();
        else if (panel === "files" && k === "c") messageRef.current?.focus();
        else if (panel === "files" && k === "C") doCommit();
        else if (panel === "files" && k === "g") generateCommitMessage();
        else if (panel === "files" && k === "d") openFilesDiscardMenu();
        else if (panel === "files" && k === "s") openFilesStashMenu();
        else if (panel === "commits" && (k === "Enter" || k === " ") && selectedCommit) openCommitRowMenu(selectedCommit);
        else if (panel === "commits" && k === "b" && selectedCommit) openCommitBranchPrompt(selectedCommit);
        else if (panel === "commits" && k === "v" && selectedCommit) openCommitRevertConfirm(selectedCommit);
        else if (panel === "branches" && (k === "Enter" || k === " ")) activateBranchEntry(selectedBranchEntry);
        else if (panel === "branches" && k === "n")
            openNewBranchPrompt(
                selectedBranchEntry?.kind === "local"
                    ? selectedBranchEntry.branch.name
                    : selectedBranchEntry?.kind === "remoteBranch"
                      ? selectedBranchEntry.branch.full_ref
                      : "",
            );
        else if (panel === "branches" && k === "N") openNewBranchPrompt();
        else if (panel === "branches" && k === "c") openCheckoutPrompt();
        else if (panel === "branches" && k === "M" && selectedBranchEntry?.kind === "local") openMergeMenu(selectedBranchEntry.branch.name);
        else if (panel === "branches" && k === "M" && selectedBranchEntry?.kind === "remoteBranch")
            openMergeMenu(selectedBranchEntry.branch.full_ref);
        else if (panel === "branches" && k === "d" && selectedBranchEntry?.kind === "local") openBranchDeleteMenu(selectedBranchEntry.branch);
        else if (panel === "branches" && k === "d" && selectedBranchEntry?.kind === "remoteBranch")
            confirmRemoteBranchDelete(selectedBranchEntry.remote, selectedBranchEntry.branch);
        else if (panel === "branches" && k === "R" && selectedBranchEntry?.kind === "local") openBranchRenamePrompt(selectedBranchEntry.branch);
        else if (panel === "branches" && k === "u" && selectedBranchEntry?.kind === "remoteBranch") setUpstreamTo(selectedBranchEntry.branch);
        else if (panel === "branches" && k === "f")
            doFetch(
                selectedBranchEntry?.kind === "remote"
                    ? selectedBranchEntry.remote.name
                    : selectedBranchEntry?.kind === "remoteBranch"
                      ? selectedBranchEntry.remote
                      : null,
            );
        else handled = false;
        if (handled) {
            e.preventDefault();
            e.stopPropagation();
        }
    };

    useEffect(() => {
        if (!active || modalOpen || menu) return;
        const onKey = (e: KeyboardEvent) => onKeyRef.current(e);
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [active, modalOpen, menu]);

    useEffect(() => {
        const root = paneRootRef.current;
        if (!active || !root || root.contains(document.activeElement)) return;
        root.focus({ preventScroll: true });
    }, [active]);

    const focusKey = `${panel}:${sel[panel]}`;
    useEffect(() => {
        if (!document.activeElement?.closest(".git-row, .gg-row")) return;
        paneRootRef.current?.querySelector<HTMLElement>(".git-list .git-row.sel, .git-list .gg-row.sel")?.focus();
    }, [focusKey]);

    const graphHandlers = useRef({ select: (_i: number) => {}, activate: () => {} });
    graphHandlers.current = {
        select: (i) => setSel("commits", i),
        activate: () => selectedCommit && openCommitRowMenu(selectedCommit),
    };
    const onGraphSelect = useCallback((i: number) => graphHandlers.current.select(i), []);
    const onGraphActivate = useCallback(() => graphHandlers.current.activate(), []);

    // The review is memoised, so its header buttons get a callback that keeps its identity.
    const reviewActionsRef = useRef((_file: GitFile): ReactNode => null);
    reviewActionsRef.current = (file) => (
        <>
            {hasUnstaged(file) && (
                <RowButton label="Discard changes" onClick={() => discardFile(file)}>
                    <IconDiscard size={12} />
                </RowButton>
            )}
            {isStaged(file) && (
                <RowButton label={`Unstage ${file.path}`} onClick={() => unstageFile(file)}>
                    <IconMinus size={12} />
                </RowButton>
            )}
            {hasUnstaged(file) && (
                <RowButton label={`Stage ${file.path}`} onClick={() => stageFile(file)}>
                    <IconPlus size={12} />
                </RowButton>
            )}
        </>
    );
    const reviewActions = useCallback((file: GitFile) => reviewActionsRef.current(file), []);

    // ---- Render ----

    const ahead = status?.ahead ?? 0;
    const behind = status?.behind ?? 0;
    const upstream = status?.upstream ?? null;
    const fileRowRange =
        rangeAnchor === null ? null : ([Math.min(rangeAnchor, selectedFileIndex), Math.max(rangeAnchor, selectedFileIndex)] as [number, number]);

    const fileRow = (entry: FileEntry, i: number) => {
        if (entry.kind === "group") {
            const staged = entry.side === "staged";
            return (
                <div className={`git-group git-file-group${i > 0 ? " follows" : ""}`}>
                    <span className="git-label">{staged ? "Staged" : "Unstaged"}</span>
                    <span className="git-count">{entry.count}</span>
                    <span className="git-section-actions">
                        {i === 0 && (
                            <button type="button" className="git-text-btn" onClick={openFilesStashMenu}>
                                Stash
                            </button>
                        )}
                        {!staged && (
                            <button type="button" className="git-text-btn danger" onClick={discardAllUnstaged}>
                                Discard all
                            </button>
                        )}
                        <button type="button" className="git-text-btn" onClick={staged ? unstageAll : stageAll}>
                            {staged ? "Unstage all" : "Stage all"}
                        </button>
                    </span>
                </div>
            );
        }
        const { file, side } = entry;
        const selected = panel === "files" && selectedFileIndex === i;
        const dir = dirname(file.path);
        return (
            <div
                role="button"
                tabIndex={selected ? 0 : -1}
                aria-label={`${file.path}, ${side}`}
                className={`git-row git-file-row${selected ? " sel" : ""}${isInRange(fileRowRange, i) ? " ranged" : ""}`}
                onFocus={() => setSel("files", i)}
                onClick={(event) => {
                    event.currentTarget.focus();
                    setSel("files", i);
                }}>
                <FileIcon name={basenameOf(file.path)} size={14} />
                <span className="git-row-name">
                    {basenameOf(file.path)}
                    {dir && <span className="git-row-dir">{dir}</span>}
                </span>
                <span className="git-row-actions">
                    {side === "unstaged" && (
                        <RowButton label="Discard changes" onClick={() => discardFile(file)}>
                            <IconDiscard size={12} />
                        </RowButton>
                    )}
                    {side === "staged" ? (
                        <RowButton label={`Unstage ${file.path}`} onClick={() => unstageFile(file)}>
                            <IconMinus size={12} />
                        </RowButton>
                    ) : (
                        <RowButton label={`Stage ${file.path}`} onClick={() => stageFile(file)}>
                            <IconPlus size={12} />
                        </RowButton>
                    )}
                </span>
                <FileStatus code={side === "staged" ? file.index : file.worktree} />
            </div>
        );
    };

    const branchRow = (entry: BranchEntry, i: number) => {
        const selected = panel === "branches" && clampSel("branches") === i;
        const common = {
            role: "button",
            tabIndex: selected ? 0 : -1,
            onFocus: () => setSel("branches", i),
            onClick: (event: MouseEvent<HTMLDivElement>) => {
                event.currentTarget.focus();
                setSel("branches", i);
            },
        } as const;
        if (entry.kind === "remote") {
            return (
                <div
                    {...common}
                    aria-expanded={entry.open}
                    className={`git-row git-remote-row${selected ? " sel" : ""}`}
                    onClick={(event) => {
                        common.onClick(event);
                        setOpenRemote(entry.open ? null : entry.remote.name);
                    }}>
                    <span className={`git-remote-chev${entry.open ? " open" : ""}`}>
                        <IconChevron size={10} />
                    </span>
                    <span className="git-row-name">{entry.remote.name}</span>
                    {entry.open && remoteBranchesRes.status === "loading" && <span className="git-panel-spinner" />}
                    <span className="git-row-actions">
                        <RowButton label={`Fetch ${entry.remote.name}`} onClick={() => doFetch(entry.remote.name)}>
                            <IconFetch size={12} />
                        </RowButton>
                        <MoreButton onOpen={(anchor) => openMenuAt(anchor, remoteItems(entry.remote))} />
                    </span>
                </div>
            );
        }
        if (entry.kind === "remoteBranch") {
            const rb = entry.branch;
            return (
                <div {...common} className={`git-row git-branch-row remote${selected ? " sel" : ""}`} title={rb.full_ref}>
                    <span className="git-dot remote" />
                    <span className="git-row-name">{rb.name}</span>
                    {rb.tracked_by && <span className="git-row-hint">tracked</span>}
                    {entry.remote === "origin" && branchPulls.get(rb.name) && (
                        <BranchPullChip pull={branchPulls.get(rb.name)!} onOpen={() => openBranchPull(branchPulls.get(rb.name)!)} />
                    )}
                    <span className="git-row-actions">
                        <RowButton
                            label={rb.tracked_by ? `Check out ${rb.tracked_by}` : "Check out as a local branch"}
                            onClick={() => checkoutRemoteBranch(entry.remote, rb)}>
                            <IconCheckout size={12} />
                        </RowButton>
                        <MoreButton onOpen={(anchor) => openMenuAt(anchor, remoteBranchItems(entry.remote, rb))} />
                    </span>
                </div>
            );
        }
        const b = entry.branch;
        return (
            <div
                {...common}
                className={`git-row git-branch-row${b.current ? " current" : ""}${selected ? " sel" : ""}`}
                title={b.upstream ? `tracks ${b.upstream}` : b.name}>
                <span className={`git-dot${b.current ? " current" : ""}`} />
                <span className="git-row-name">{b.name}</span>
                {branchPulls.get(b.name) && (
                    <BranchPullChip pull={branchPulls.get(b.name)!} onOpen={() => openBranchPull(branchPulls.get(b.name)!)} />
                )}
                {b.current && ahead > 0 && (
                    <Tooltip label={`Push ${ahead} commit${ahead > 1 ? "s" : ""}`}>
                        <button
                            type="button"
                            tabIndex={-1}
                            className="git-sync-chip"
                            onClick={(event) => {
                                event.stopPropagation();
                                pushRepo();
                            }}>
                            <IconPush size={10} />
                            {ahead}
                        </button>
                    </Tooltip>
                )}
                {b.current && behind > 0 && <span className="git-row-hint behind">↓{behind}</span>}
                <span className="git-row-actions">
                    {!b.current && (
                        <>
                            <RowButton label="Check out" onClick={() => checkoutBranch(b.name)}>
                                <IconCheckout size={12} />
                            </RowButton>
                            <RowButton label={`Merge into ${currentBranch || "HEAD"}`} onClick={() => mergeRef(b.name)}>
                                <IconMerge size={12} />
                            </RowButton>
                        </>
                    )}
                    <MoreButton onOpen={(anchor) => openMenuAt(anchor, localBranchItems(b))} />
                </span>
            </div>
        );
    };

    const commitHead = (c: GitCommit) => (
        <div className="git-detail">
            <h2 className="git-detail-title">{c.subject}</h2>
            <div className="git-detail-meta">
                <AuthorAvatar name={c.author} email={c.author_email} />
                <span>{c.author}</span>
                <span>·</span>
                <span>{c.date}</span>
                <span>·</span>
                <span className="mono git-detail-hash">{c.hash}</span>
                {c.unpushed ? <span className="git-chip warn">not pushed</span> : upstream && <span className="git-chip">pushed</span>}
                {c.refs
                    .filter((r) => r !== "HEAD")
                    .map((r) => (
                        <span key={r} className="git-chip">
                            {r.replace(/^HEAD -> |^tag: /, "")}
                        </span>
                    ))}
            </div>
            <div className="git-detail-actions">
                <button type="button" className="git-btn" onClick={() => void copyValue(c.full_hash || c.hash, c.hash)}>
                    <IconCopy size={12} />
                    Copy hash
                </button>
                <button type="button" className="git-btn" onClick={() => openCommitBranchPrompt(c)}>
                    <IconGit size={12} />
                    Branch from here
                </button>
                <button type="button" className="git-btn warn" onClick={() => openCommitResetMenu(c)}>
                    <IconDiscard size={12} />
                    Reset to here
                </button>
                <button type="button" className="git-btn danger" onClick={() => openCommitRevertConfirm(c)}>
                    Revert
                </button>
            </div>
        </div>
    );

    const branchHead = (entry: BranchEntry) => {
        if (entry.kind === "remote") {
            return (
                <div className="git-detail">
                    <h2 className="git-detail-title mono">{entry.remote.name}</h2>
                    <div className="git-detail-meta">
                        <span className="mono">{entry.remote.url}</span>
                    </div>
                    <div className="git-detail-actions">
                        {remoteItems(entry.remote)
                            .filter((item) => !item.sep)
                            .map((item) => (
                                <button key={item.label} type="button" className={`git-btn${item.danger ? " danger" : ""}`} onClick={item.run}>
                                    {item.label}
                                </button>
                            ))}
                    </div>
                </div>
            );
        }
        if (entry.kind === "remoteBranch") {
            const rb = entry.branch;
            return (
                <div className="git-detail">
                    <h2 className="git-detail-title mono">{rb.full_ref}</h2>
                    <div className="git-detail-meta">
                        {rb.tracked_by ? (
                            <span>
                                tracked by <span className="mono">{rb.tracked_by}</span>
                            </span>
                        ) : (
                            <span>no local branch</span>
                        )}
                    </div>
                    <div className="git-detail-actions">
                        <button type="button" className="git-btn primary" onClick={() => checkoutRemoteBranch(entry.remote, rb)}>
                            <IconCheckout size={12} />
                            {rb.tracked_by ? `Check out ${rb.tracked_by}` : "Check out"}
                        </button>
                        <button type="button" className="git-btn" onClick={() => openMergeMenu(rb.full_ref)}>
                            <IconMerge size={12} />
                            Merge into {currentBranch || "HEAD"}
                        </button>
                        <button type="button" className="git-btn" disabled={!currentBranch} onClick={() => setUpstreamTo(rb)}>
                            Set as upstream
                        </button>
                        <button type="button" className="git-btn danger" onClick={() => confirmRemoteBranchDelete(entry.remote, rb)}>
                            <IconTrash size={12} />
                            Delete on {entry.remote}
                        </button>
                    </div>
                </div>
            );
        }
        const b = entry.branch;
        return (
            <div className="git-detail">
                <h2 className="git-detail-title mono">
                    {b.name}
                    {b.current && <span className="git-chip live">checked out</span>}
                </h2>
                <div className="git-detail-meta">
                    {b.upstream ? (
                        <span>
                            tracks <span className="mono">{b.upstream}</span>
                        </span>
                    ) : (
                        <span>no upstream</span>
                    )}
                </div>
                {b.current && (
                    <div className="git-stats">
                        <div className="git-stat">
                            <b className="ahead">{ahead}</b>
                            <span>to push</span>
                        </div>
                        <div className="git-stat">
                            <b className="behind">{behind}</b>
                            <span>to pull</span>
                        </div>
                        <div className="git-stat">
                            <b>{files.length}</b>
                            <span>uncommitted</span>
                        </div>
                    </div>
                )}
                <div className="git-detail-actions">
                    {b.current ? (
                        <>
                            <button type="button" className="git-btn primary" onClick={pushRepo}>
                                <IconPush size={12} />
                                {upstream ? "Push" : "Publish"}
                            </button>
                            <button type="button" className="git-btn" onClick={pullRepo}>
                                <IconPull size={12} />
                                Pull
                            </button>
                            <button type="button" className="git-btn" onClick={() => openBranchRenamePrompt(b)}>
                                <IconPencil size={12} />
                                Rename
                            </button>
                            <button type="button" className="git-btn" onClick={() => openNewBranchPrompt(b.name)}>
                                <IconPlus size={12} />
                                New branch from here
                            </button>
                        </>
                    ) : (
                        <>
                            <button type="button" className="git-btn primary" onClick={() => checkoutBranch(b.name)}>
                                <IconCheckout size={12} />
                                Check out
                            </button>
                            <button type="button" className="git-btn" onClick={() => mergeRef(b.name)}>
                                <IconMerge size={12} />
                                Merge into {currentBranch || "HEAD"}
                            </button>
                            <button type="button" className="git-btn" onClick={() => mergeRef(b.name, true)}>
                                Squash merge
                            </button>
                            <button type="button" className="git-btn danger" onClick={() => openBranchDeleteMenu(b)}>
                                <IconTrash size={12} />
                                Delete
                            </button>
                        </>
                    )}
                </div>
            </div>
        );
    };

    const loadingOrError = (rows: number, label: string) =>
        overviewLoading ? (
            <SkeletonRows rows={rows} label={label} />
        ) : overviewError ? (
            <EmptyState tone="error" icon={<IconWarning size={14} />} title="Git error" message={overviewError} />
        ) : null;

    const filterInput = (p: GitPanel, placeholder: string) => (
        <label className="git-filter">
            <IconSearch size={12} />
            <input
                ref={(input) => {
                    filterInputs.current[p] = input;
                }}
                value={queries[p]}
                placeholder={placeholder}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                    const value = event.target.value;
                    setQueries((q) => ({ ...q, [p]: value }));
                    setSel(p, 0);
                }}
                onKeyDown={(event) => {
                    event.stopPropagation();
                    if (event.key === "Escape" || event.key === "Enter") {
                        if (event.key === "Escape") setQueries((q) => ({ ...q, [p]: "" }));
                        if (p === "files") setFileFilterOpen(false);
                        if (p === "commits") setCommitSearchOpen(false);
                        window.requestAnimationFrame(() =>
                            paneRootRef.current
                                ?.querySelector<HTMLElement>(".git-list .git-row.sel, .git-list .gg-row.sel, .git-list .git-row")
                                ?.focus(),
                        );
                    }
                }}
            />
        </label>
    );

    let left: ReactNode;
    let right: ReactNode;
    if (panel !== "branches") {
        const latest = commits[0];
        left = (
            <>
                <GitComposer
                    repo={repo}
                    busy={!!busy}
                    generating={generating}
                    stagedCount={files.filter(isStaged).length}
                    agentLabel={`${AI_PROVIDER_LABEL[aiProvider]} · ${(aiModel || defaultAiModel(aiProvider)).replace(/^[^/]+\//, "")}`}
                    messageRef={messageRef}
                    onCommit={doCommit}
                    onGenerate={generateCommitMessage}
                    onPickAgent={openAgentMenu}
                />
                {(fileFilterOpen || queries.files) && filterInput("files", "Filter files")}
                <div className="git-list">
                    {fileEntries.length === 0 &&
                        (loadingOrError(5, "Loading changed files") ?? (
                            <EmptyState
                                icon={<IconGit size={14} />}
                                title={queries.files ? "No matches" : "Nothing to commit"}
                                message={queries.files ? `Nothing matches "${queries.files}".` : "The working tree is clean."}
                            />
                        ))}
                    <VirtualPanelRows
                        items={fileEntries}
                        selectedIndex={selectedFileIndex}
                        focused={panel === "files"}
                        estimateSize={ROW_HEIGHT}
                        getKey={(entry) => (entry.kind === "group" ? `g:${entry.side}` : `${entry.side}:${entry.file.path}`)}
                        renderRow={fileRow}
                    />
                </div>
                {historyOpen && (
                    <ResizeHandle
                        targetRef={historyRef}
                        axis="y"
                        grows={-1}
                        min={HISTORY_MIN}
                        max={() => (historyRef.current?.parentElement?.clientHeight ?? HISTORY_MIN + HISTORY_CLEARANCE) - HISTORY_CLEARANCE}
                        size={view.historyHeight}
                        label="Resize the history"
                        className="git-history-split"
                        onResize={(height) => cmd.setGitView(paneId, { historyHeight: height })}
                    />
                )}
                <div
                    ref={historyRef}
                    className={`git-history${historyOpen ? " open" : ""}`}
                    style={historyOpen && view.historyHeight ? { flex: `0 0 ${view.historyHeight}px`, minHeight: HISTORY_MIN } : undefined}>
                    <div className="git-history-head">
                        <button
                            type="button"
                            className="git-history-toggle"
                            aria-expanded={historyOpen}
                            onClick={() => {
                                if (historyOpen) {
                                    setCommitSearchOpen(false);
                                    setHistoryOpen(false);
                                } else cmd.setGitView(paneId, { historyOpen: true, panel: "commits" });
                            }}>
                            <span className="git-history-chev">
                                <IconChevron size={10} />
                            </span>
                            <span className="git-label">History</span>
                            {commits.length > 0 && <span className="git-count">{commits.length}</span>}
                            {!historyOpen && latest && <span className="git-history-latest">{latest.subject}</span>}
                            {!historyOpen && <kbd>h</kbd>}
                        </button>
                        {historyOpen &&
                            (commitSearchOpen || queries.commits ? (
                                <label className="git-history-search">
                                    <IconSearch size={11} />
                                    <input
                                        ref={(input) => {
                                            filterInputs.current.commits = input;
                                        }}
                                        value={queries.commits}
                                        placeholder="Search commits"
                                        aria-label="Search commits"
                                        spellCheck={false}
                                        autoCapitalize="off"
                                        autoCorrect="off"
                                        onChange={(event) => {
                                            const value = event.target.value;
                                            setQueries((q) => ({ ...q, commits: value }));
                                            setSel("commits", 0);
                                        }}
                                        onBlur={() => {
                                            if (!queries.commits) setCommitSearchOpen(false);
                                        }}
                                        onKeyDown={(event) => {
                                            event.stopPropagation();
                                            if (event.key !== "Escape" && event.key !== "Enter") return;
                                            if (event.key === "Escape") setQueries((q) => ({ ...q, commits: "" }));
                                            setCommitSearchOpen(false);
                                            window.requestAnimationFrame(() =>
                                                paneRootRef.current
                                                    ?.querySelector<HTMLElement>(".git-history .gg-row.sel, .git-history .gg-row")
                                                    ?.focus(),
                                            );
                                        }}
                                    />
                                </label>
                            ) : (
                                <Tooltip label="Search commits (/)">
                                    <button
                                        type="button"
                                        className="git-row-act"
                                        aria-label="Search commits"
                                        onClick={() => {
                                            cmd.setGitView(paneId, { panel: "commits" });
                                            setCommitSearchOpen(true);
                                        }}>
                                        <IconSearch size={12} />
                                    </button>
                                </Tooltip>
                            ))}
                    </div>
                    {historyOpen && (
                        <>
                            <div className="git-list">
                                {filteredCommits.length === 0 ? (
                                    (loadingOrError(8, "Loading commits") ?? (
                                        <EmptyState
                                            title={queries.commits ? "No matches" : "No commits"}
                                            message={queries.commits ? `Nothing matches "${queries.commits}".` : "No commits on this branch yet."}
                                        />
                                    ))
                                ) : (
                                    <GitGraph
                                        commits={filteredCommits}
                                        selectedIndex={clampSel("commits")}
                                        focused={panel === "commits"}
                                        range={null}
                                        onSelect={onGraphSelect}
                                        onActivate={onGraphActivate}
                                    />
                                )}
                            </div>
                        </>
                    )}
                </div>
            </>
        );
        right =
            panel === "commits" && selectedCommit ? (
                <CommitReview
                    key={selectedCommit.hash}
                    repo={repo}
                    rev={selectedCommit.hash}
                    title={selectedCommit.hash}
                    subtitle={selectedCommit.subject}
                    head={commitHead(selectedCommit)}
                    onOpenFile={cmd.requestOpenFile}
                />
            ) : filteredFiles.length > 0 ? (
                <MergeReview
                    repo={repo}
                    files={filteredFiles}
                    focusPath={selectedFile?.file.path}
                    onOpenFile={cmd.requestOpenFile}
                    onSaved={onReviewSaved}
                    fileActions={reviewActions}
                />
            ) : latest ? (
                <CommitReview
                    key={latest.hash}
                    repo={repo}
                    rev={latest.hash}
                    title={latest.hash}
                    subtitle={latest.subject}
                    head={commitHead(latest)}
                    onOpenFile={cmd.requestOpenFile}
                />
            ) : (
                <EmptyState message="Nothing to review." />
            );
    } else {
        const localCount = branchEntries.filter((e) => e.kind === "local").length;
        left = (
            <>
                {filterInput("branches", "Filter branches")}
                <div className="git-list">
                    {branchEntries.length === 0 &&
                        (loadingOrError(5, "Loading branches") ?? (
                            <EmptyState
                                title={queries.branches ? "No matches" : "No branches"}
                                message={queries.branches ? `Nothing matches "${queries.branches}".` : "This repository has no branches yet."}
                            />
                        ))}
                    {localCount > 0 && (
                        <div className="git-group">
                            <span>Local</span>
                            <span className="git-count">{localCount}</span>
                        </div>
                    )}
                    <VirtualPanelRows
                        items={branchEntries}
                        selectedIndex={clampSel("branches")}
                        focused={panel === "branches"}
                        estimateSize={ROW_HEIGHT}
                        getKey={branchEntryKey}
                        renderRow={branchRow}
                    />
                </div>
                <div className="git-left-foot">
                    <button type="button" className="git-btn wide" onClick={() => openNewBranchPrompt()}>
                        <IconPlus size={12} />
                        New branch
                        <kbd>N</kbd>
                    </button>
                </div>
            </>
        );
        const entry = selectedBranchEntry;
        right = !entry ? (
            <EmptyState message="Select a branch to see its details." />
        ) : entry.kind === "remote" ? (
            <div className="commit-review">{branchHead(entry)}</div>
        ) : (
            <CommitReview
                key={branchEntryKey(entry)}
                repo={repo}
                rev={entry.kind === "local" ? entry.branch.name : entry.branch.full_ref}
                title=""
                subtitle=""
                head={branchHead(entry)}
                onOpenFile={cmd.requestOpenFile}
            />
        );
    }

    return (
        <div ref={paneRootRef} className="git-pane" tabIndex={-1}>
            <GitHostShell
                paneId={paneId}
                cwd={repo}
                area={view.area}
                active={active}
                onArea={(area) => cmd.setGitView(paneId, { area })}
                local={[
                    {
                        id: "changes",
                        label: "Changes (1)",
                        icon: <IconFile size={16} />,
                        count: files.length,
                        on: view.area === "local" && panel !== "branches",
                        onSelect: () => cmd.setGitView(paneId, { area: "local", panel: panel === "branches" ? "files" : panel }),
                    },
                    {
                        id: "branches",
                        label: "Branches (2)",
                        icon: <IconGit size={16} />,
                        on: view.area === "local" && panel === "branches",
                        onSelect: () => cmd.setGitView(paneId, { area: "local", panel: "branches" }),
                    },
                ]}>
                <div className="git-toolbar">
                    {onLeaveRepo && (
                        <Tooltip label="Back to the repositories in this folder">
                            <button type="button" className="git-btn git-back" onClick={onLeaveRepo}>
                                <IconChevron size={11} className="git-back-icon" />
                                {basenameOf(repo)}
                            </button>
                        </Tooltip>
                    )}
                    <Tooltip label="Switch branch">
                        <button type="button" className="git-btn git-switch" onClick={(event) => openBranchPicker(event.currentTarget)}>
                            <IconGit size={13} />
                            <span className="git-switch-name">{overviewLoading ? "…" : currentBranch || "detached"}</span>
                            <IconChevron size={9} className="git-switch-chev" />
                        </button>
                    </Tooltip>
                    {overviewError && <span className="git-tb-chip error">git error</span>}
                    {busy && (
                        <span className={`git-tb-busy${busy.startsWith("✗") ? " error" : ""}`}>
                            {!busy.startsWith("✗") && <span className="git-panel-spinner" />}
                            <span>{busy}</span>
                        </span>
                    )}
                    <span className="git-tb-grow" />
                    <Tooltip label="Fetch all remotes (F)">
                        <button type="button" className="git-btn" onClick={() => doFetch(null)}>
                            <IconFetch size={13} />
                            Fetch
                        </button>
                    </Tooltip>
                    <Tooltip label={behind > 0 ? `Pull ${behind} commit${behind > 1 ? "s" : ""} (p)` : "Pull (p)"}>
                        <button type="button" className="git-btn" onClick={pullRepo}>
                            <IconPull size={13} />
                            Pull
                            {behind > 0 && <span className="git-btn-count">{behind}</span>}
                        </button>
                    </Tooltip>
                    <Tooltip
                        label={
                            !upstream
                                ? "Publish branch and set upstream (P)"
                                : ahead > 0
                                  ? `Push ${ahead} commit${ahead > 1 ? "s" : ""} (P)`
                                  : "Push (P)"
                        }>
                        <button type="button" className={`git-btn${ahead > 0 || !upstream ? " primary" : ""}`} onClick={pushRepo}>
                            <IconPush size={13} />
                            {upstream ? "Push" : "Publish"}
                            {ahead > 0 && <span className="git-btn-count">{ahead}</span>}
                        </button>
                    </Tooltip>
                    <Tooltip label="Open pull request (⌃P)">
                        <button type="button" className="git-btn" onClick={openPullRequest}>
                            <IconPullRequest size={13} />
                            Pull request
                        </button>
                    </Tooltip>
                    <MoreButton label="Remotes, stashes and more" onOpen={openMoreMenu} className="git-btn icon" />
                </div>
                <GitColumns
                    paneId={paneId}
                    left={
                        <>
                            {left}
                            {cmdLogOpen && <GitCmdLogBar />}
                        </>
                    }
                    right={
                        <div className="git-right-review">
                            <Suspense fallback={<SkeletonRows rows={6} label="Loading diff preview" />}>{right}</Suspense>
                            {busy && !busy.startsWith("✗") && (
                                <div className="git-busy-overlay">
                                    <div className="git-busy-card">
                                        <span className="git-busy-spinner" />
                                        <span className="git-busy-label">{busy}</span>
                                    </div>
                                </div>
                            )}
                        </div>
                    }
                />
            </GitHostShell>
            {menu && <TreeContextMenu x={menu.x} y={menu.y} items={menu.items} alignRight={menu.alignRight} onClose={() => setMenu(null)} />}
            <GitModalRenderer paneId={paneId} active={active} />
        </div>
    );
}

function FileStatus({ code }: { code: string }) {
    const letter = code === "?" ? "U" : code.trim();
    const cls = letter === "A" || letter === "U" ? "added" : letter === "D" ? "deleted" : letter === "R" || letter === "C" ? "renamed" : "modified";
    return <span className={`git-status ${cls}`}>{letter}</span>;
}

function MoreButton({
    onOpen,
    label = "More",
    className = "git-row-act",
}: {
    onOpen: (anchor: HTMLElement) => void;
    label?: string;
    className?: string;
}) {
    return (
        <Tooltip label={label}>
            <button
                type="button"
                tabIndex={className === "git-row-act" ? -1 : undefined}
                className={className}
                aria-label={label}
                aria-haspopup="menu"
                onClick={(event) => {
                    event.stopPropagation();
                    onOpen(event.currentTarget);
                }}>
                <IconMore size={13} />
            </button>
        </Tooltip>
    );
}

function isMissingRepository(error: string | null | undefined): boolean {
    return !!error && /could not find repository/i.test(error);
}

function GitRepoPicker({ paneId, root, active }: { paneId: string; root: string; active: boolean }) {
    const discovered = useCachedResourceEnabled(active && !!root, gitDiscoveredReposR, root || "");
    const repos = discovered.data ?? [];

    if (discovered.status === "loading" && !discovered.data) {
        return (
            <div className="git-pane git-repo-picker">
                <SkeletonRows rows={4} />
            </div>
        );
    }

    if (repos.length === 0) {
        return (
            <div className="git-pane git-repo-picker">
                <EmptyState
                    tone="error"
                    icon={<IconWarning size={14} />}
                    title="Not a repository"
                    message={
                        discovered.status === "error"
                            ? (discovered.error ?? "could not read this folder")
                            : `No repository at ${root}, and none in the folders directly inside it.`
                    }
                />
            </div>
        );
    }

    return (
        <div className="git-pane git-repo-picker">
            <div className="git-toolbar">
                <span className="git-tb-status">
                    <IconGit size={13} className="git-tb-icon" />
                    <span className="git-tb-branch">{basenameOf(root)}</span>
                    <span className="git-tb-changed">{repos.length === 1 ? "1 repository inside" : `${repos.length} repositories inside`}</span>
                </span>
            </div>
            <ul className="git-repo-list">
                {repos.map((entry) => (
                    <li key={entry.path}>
                        <button type="button" className="git-repo-row" onClick={() => cmd.setGitView(paneId, { repo: entry.path })}>
                            <IconGit size={13} className={`git-repo-icon${entry.changes > 0 ? " dirty" : ""}`} />
                            <span className="git-repo-name">{entry.name}</span>
                            <span className="git-repo-branch">{entry.branch || "no commits"}</span>
                            {entry.ahead > 0 && <span className="git-repo-sync">{entry.ahead}↑</span>}
                            {entry.behind > 0 && <span className="git-repo-sync">{entry.behind}↓</span>}
                            <span className={`git-repo-changes${entry.changes > 0 ? " dirty" : ""}`}>{entry.changes > 0 ? entry.changes : "—"}</span>
                        </button>
                    </li>
                ))}
            </ul>
        </div>
    );
}

/**
 * Whether the pane has come to rest on screen. A switch slides the screen in,
 * and fetching, re-rendering and repainting during that slide is what drops
 * its frames, so the refresh waits for the stage to stop.
 */
function useSettled(active: boolean): boolean {
    const [settled, setSettled] = useState(active);
    const [wasActive, setWasActive] = useState(active);
    if (active !== wasActive) {
        setWasActive(active);
        if (!active) setSettled(false);
    }
    useEffect(() => {
        if (!active || settled) return;
        return whenStageStill(() => setSettled(true));
    }, [active, settled]);
    return active && settled;
}

/**
 * `visible` keeps Git's state fresh while it is on screen, split beside the
 * pane being worked in included; `active` is only for taking the keyboard.
 */
export function GitPane({ paneId, cwd, active, visible }: { paneId: string; cwd: string; active: boolean; visible: boolean }) {
    const fetching = useSettled(visible);
    const selectedRepo = useStore((s) => s.gitViews[paneId]?.repo ?? null);
    const rootOverview = useCachedResourceEnabled(fetching && !!cwd && !selectedRepo, gitOverviewR, cwd || "");
    const settledRoot = useRef<{ cwd: string; missing: boolean } | null>(null);
    if (rootOverview.status !== "loading") {
        settledRoot.current = { cwd, missing: rootOverview.status === "error" && !rootOverview.data && isMissingRepository(rootOverview.error) };
    }
    const rootMissing = settledRoot.current?.cwd === cwd && settledRoot.current.missing;

    if (!selectedRepo && rootMissing) return <GitRepoPicker paneId={paneId} root={cwd} active={fetching} />;
    return (
        <GitWorkbench
            paneId={paneId}
            repo={selectedRepo ?? cwd}
            active={active}
            fetching={fetching}
            onLeaveRepo={selectedRepo ? () => cmd.setGitView(paneId, { repo: null }) : null}
        />
    );
}
