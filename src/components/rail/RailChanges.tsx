import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { git, hasUnstaged, isStaged } from "../../api/git";
import { basename, dirname } from "../../lib/paths";
import * as cmd from "../../state/commands";
import { commitGitDraft, generateGitDraft, runRepositoryGit, setGitDraft, setGitProvider, useGitWorkbench } from "../../state/gitWorkbench";
import { promptDialog } from "../../state/dialog";
import { Dropdown } from "../Dropdown";
import { AI_MODELS, AI_PROVIDER_LABEL } from "../git/gitPaneConstants";
import type { GitAiProvider } from "../git/gitPaneTypes";
import { PRIMARY_SHORTCUT } from "../../lib/platform";
import { useResourceEnabled } from "../../state/resources";
import { gitOverviewR } from "../../state/resources.defs";
import { useStore } from "../../state/store";
import { FileIcon } from "../FileIcon";
import { GitGraph } from "../git/GitGraph";
import { gitFileDecoration } from "../git/gitFileStatus";
import { IconCheck, IconChevron, IconCommit, IconGit, IconPlus, IconRefresh, IconSparkle, IconPush, IconPull, IconFetch } from "../Icons";
import { Tooltip } from "../Tooltip";

function relativeDir(path: string): string {
    const dir = dirname(path);
    return dir === "." || dir === "" ? "" : dir;
}

export function RailChanges({ cwd }: { cwd: string }) {
    const overview = useResourceEnabled(!!cwd, gitOverviewR, cwd || "");
    const split = useStore((s) => s.railChangesSplit);
    const message = useGitWorkbench((state) => state.drafts[cwd] ?? "");
    const operation = useGitWorkbench((state) => state.operations[cwd]);
    const provider = useGitWorkbench((state) => state.provider);
    const model = useGitWorkbench((state) => state.model);
    const setMessage = (value: string) => setGitDraft(cwd, value);
    const [changesOpen, setChangesOpen] = useState(true);
    const [graphOpen, setGraphOpen] = useState(true);
    const [branchesOpen, setBranchesOpen] = useState(false);
    const busy = operation?.busy ?? false;
    const [commitIndex, setCommitIndex] = useState(0);
    const inputRef = useRef<HTMLTextAreaElement>(null);
    const splitRef = useRef<HTMLDivElement>(null);

    const status = overview.data?.status;
    const files = status?.files ?? [];
    const log = overview.data?.log ?? [];
    const branches = overview.data?.branches ?? [];
    const branch = status?.branch ?? "";
    const staged = files.filter(isStaged);

    const ready = overview.status === "ok" && !!status;
    const refresh = () => {
        void overview.refresh().catch(() => {});
    };
    const run = (label: string, fn: () => Promise<unknown>) => {
        void runRepositoryGit(cwd, label, fn).then(refresh);
    };
    const commit = () => {
        if (ready && staged.length > 0) void commitGitDraft(cwd).then(refresh);
    };
    const generate = () => {
        if (ready) void generateGitDraft(cwd);
    };
    const createBranch = async () => {
        const name = await promptDialog({
            title: "Create branch",
            label: "Branch name",
            body: `Create a branch from ${branch || "HEAD"} in ${cwd}.`,
            confirmLabel: "Create branch",
        });
        if (name?.trim()) run("Create branch", () => git.branchCreate(cwd, name.trim()));
    };

    const onSplitDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
        const host = splitRef.current;
        if (!host) return;
        event.preventDefault();
        const box = host.getBoundingClientRect();
        const move = (e: PointerEvent) => cmd.setRailChangesSplit((e.clientY - box.top) / box.height);
        const up = () => {
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", up);
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
    };

    if (!cwd) return <div className="rail-note">open a project to see changes</div>;

    const bothOpen = changesOpen && graphOpen;

    return (
        <div className="rail-changes">
            <div className="rail-changes-head">
                <span className="rail-changes-count">
                    {!status ? (overview.status === "error" ? "Unavailable" : "Loading…") : `${files.length} changed`}
                </span>
                {branch && (
                    <span className="rail-branch">
                        <IconGit size={11} />
                        {branch}
                    </span>
                )}
                <Tooltip label="Re-read the repository">
                    <button type="button" className="rail-section-act" aria-label="Refresh changes" disabled={busy} onClick={refresh}>
                        <IconRefresh size={11} />
                    </button>
                </Tooltip>
            </div>

            <div className="rail-git-toolbar" role="group" aria-label="Repository actions">
                <button type="button" disabled={busy || !ready} onClick={() => run("Push", () => git.push(cwd))}>
                    <IconPush size={13} />
                    Push{status?.ahead ? ` ${status.ahead}` : ""}
                </button>
                <button type="button" disabled={busy || !ready} onClick={() => run("Pull", () => git.pull(cwd))}>
                    <IconPull size={13} />
                    Pull{status?.behind ? ` ${status.behind}` : ""}
                </button>
                <button type="button" disabled={busy || !ready} onClick={() => run("Fetch", () => git.fetch(cwd))}>
                    <IconFetch size={13} />
                    Fetch
                </button>
                <button type="button" onClick={cmd.openGitWorkbench}>
                    More Git tools
                </button>
            </div>
            {overview.status === "loading" && (
                <div className="rail-note" role="status">
                    {status ? "Refreshing repository…" : "Reading repository…"}
                </div>
            )}
            {overview.status === "error" && (
                <div className="rail-operation error" role="alert">
                    <span>
                        {status ? "Showing previous data. " : ""}
                        {overview.error || "Unable to read repository."}
                    </span>
                    <button type="button" onClick={refresh}>
                        Retry
                    </button>
                </div>
            )}
            {operation && (
                <div className={`rail-operation${operation.error ? " error" : ""}`} role={operation.error ? "alert" : "status"} aria-live="polite">
                    {operation.busy ? `${operation.label}…` : operation.error || operation.result}
                    {!operation.busy && (
                        <button
                            type="button"
                            aria-label="Dismiss Git result"
                            onClick={() =>
                                useGitWorkbench.setState((state) => ({
                                    operations: Object.fromEntries(Object.entries(state.operations).filter(([repo]) => repo !== cwd)),
                                }))
                            }>
                            Dismiss
                        </button>
                    )}
                </div>
            )}
            <div className="rail-commit">
                <div className="rail-commit-input">
                    <textarea
                        ref={inputRef}
                        value={message}
                        rows={2}
                        placeholder={`Message (${PRIMARY_SHORTCUT}↵ commits staged changes)`}
                        aria-label="Commit message"
                        onChange={(event) => setMessage(event.target.value)}
                        onKeyDown={(event) => {
                            event.stopPropagation();
                            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                                event.preventDefault();
                                commit();
                            }
                        }}
                    />
                    <Tooltip label={`Generate with ${AI_PROVIDER_LABEL[provider]} · ${model}. Your draft is kept if generation fails.`}>
                        <button
                            type="button"
                            className="rail-commit-ai"
                            aria-label="Generate commit message"
                            disabled={busy || !ready || files.length === 0}
                            onClick={generate}>
                            <IconSparkle size={13} />
                        </button>
                    </Tooltip>
                </div>
                <div className="rail-git-provider">
                    <Dropdown
                        label="Commit message provider"
                        value={provider}
                        disabled={busy}
                        options={Object.entries(AI_PROVIDER_LABEL).map(([value, label]) => ({ value, label }))}
                        onChange={(value) => setGitProvider(value as GitAiProvider)}
                    />
                    <Dropdown
                        label="Commit message model"
                        value={model}
                        disabled={busy}
                        options={AI_MODELS[provider].map((value) => ({ value, label: value }))}
                        onChange={(value) => useGitWorkbench.setState({ model: value })}
                    />
                </div>
                <button type="button" className="rail-commit-go" disabled={busy || !ready || !message.trim() || staged.length === 0} onClick={commit}>
                    <IconCommit size={13} />
                    <span>Commit staged{staged.length > 0 ? ` · ${staged.length}` : ""}</span>
                </button>
            </div>

            {status && (
                <div className="rail-split" ref={splitRef}>
                    <section className="rail-section" style={bothOpen ? { flex: `0 0 ${split * 100}%` } : undefined}>
                        <div className="rail-section-heading">
                            <button
                                type="button"
                                className="rail-section-head"
                                aria-expanded={changesOpen}
                                onClick={() => setChangesOpen((open) => !open)}>
                                <span className={`rail-caret${changesOpen ? " open" : ""}`}>
                                    <IconChevron size={11} />
                                </span>
                                <span className="rail-section-label">Changes</span>
                                {files.length > 0 && <span className="rail-section-count">{files.length}</span>}
                            </button>
                            <Tooltip label="Stage everything">
                                <button
                                    type="button"
                                    className="rail-section-act"
                                    aria-label="Stage all changes"
                                    disabled={busy || !ready || files.length === 0}
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        run("Stage all changes", () => git.stageAll(cwd));
                                    }}>
                                    <IconPlus size={11} />
                                </button>
                            </Tooltip>
                        </div>
                        {changesOpen && (
                            <div className="rail-list">
                                {files.length === 0 && ready && <div className="rail-note">Working tree clean</div>}
                                {files.map((file) => {
                                    const decoration = gitFileDecoration(file);
                                    const name = basename(file.path);
                                    const dir = relativeDir(file.path);
                                    return (
                                        <div key={file.path} className={`rail-file-wrap${isStaged(file) ? " staged" : ""}`}>
                                            <button
                                                type="button"
                                                className={`rail-file git-${decoration.cls}`}
                                                title={file.path}
                                                onClick={() => cmd.openDiff(file.path)}>
                                                <FileIcon name={name} size={13} />
                                                <span className="rail-file-name">{name}</span>
                                                {dir && <span className="rail-file-dir">{dir}</span>}
                                                <span className="rail-file-mark" title={decoration.label}>
                                                    {decoration.letter}
                                                </span>
                                            </button>
                                            {hasUnstaged(file) && (
                                                <Tooltip label={`Stage ${name}`}>
                                                    <button
                                                        type="button"
                                                        className="rail-file-stage"
                                                        aria-label={`Stage ${name}`}
                                                        disabled={busy || !ready}
                                                        onClick={() => run(`Stage ${name}`, () => git.stage(cwd, file.path))}>
                                                        <IconPlus size={12} />
                                                    </button>
                                                </Tooltip>
                                            )}
                                            {isStaged(file) && (
                                                <Tooltip label={`Unstage ${name}`}>
                                                    <button
                                                        type="button"
                                                        className="rail-file-stage"
                                                        aria-label={`Unstage ${name}`}
                                                        disabled={busy || !ready}
                                                        onClick={() => run(`Unstage ${name}`, () => git.unstage(cwd, file.path))}>
                                                        <IconCheck size={12} />
                                                    </button>
                                                </Tooltip>
                                            )}
                                        </div>
                                    );
                                })}
                            </div>
                        )}
                    </section>

                    {bothOpen && (
                        <div
                            className="rail-split-handle"
                            role="separator"
                            tabIndex={0}
                            aria-orientation="horizontal"
                            aria-label="Resize the changes and commits panels"
                            aria-valuemin={15}
                            aria-valuemax={85}
                            aria-valuenow={Math.round(split * 100)}
                            onPointerDown={onSplitDrag}
                            onKeyDown={(event) => {
                                if (event.key === "ArrowUp") cmd.setRailChangesSplit(split - 0.05);
                                else if (event.key === "ArrowDown") cmd.setRailChangesSplit(split + 0.05);
                                else return;
                                event.preventDefault();
                            }}
                        />
                    )}

                    <section className="rail-section">
                        <button
                            type="button"
                            className="rail-section-head"
                            aria-expanded={branchesOpen}
                            onClick={() => setBranchesOpen((open) => !open)}>
                            <span className={`rail-caret${branchesOpen ? " open" : ""}`}>
                                <IconChevron size={11} />
                            </span>
                            <span className="rail-section-label">Branches</span>
                            {branches.length > 0 && <span className="rail-section-count">{branches.length}</span>}
                        </button>
                        {branchesOpen && (
                            <button className="rail-new-branch" type="button" disabled={busy || !ready} onClick={() => void createBranch()}>
                                <IconPlus size={12} />
                                New branch
                            </button>
                        )}
                        {branchesOpen && (
                            <div className="rail-list">
                                {branches.length === 0 && <div className="rail-note">no branches</div>}
                                {branches.map((entry) => (
                                    <button
                                        key={entry.name}
                                        type="button"
                                        className={`rail-branch-row${entry.current ? " current" : ""}`}
                                        disabled={busy || !ready || entry.current}
                                        title={entry.upstream ? `tracks ${entry.upstream}` : "no upstream"}
                                        onClick={() => run(`checking out ${entry.name}`, () => git.checkoutSmart(cwd, entry.name))}>
                                        <span className="rail-branch-node" aria-hidden="true" />
                                        <span className="rail-branch-name">{entry.name}</span>
                                        {entry.upstream && <span className="rail-branch-upstream">{entry.upstream}</span>}
                                    </button>
                                ))}
                            </div>
                        )}
                    </section>

                    <section className="rail-section">
                        <button type="button" className="rail-section-head" aria-expanded={graphOpen} onClick={() => setGraphOpen((open) => !open)}>
                            <span className={`rail-caret${graphOpen ? " open" : ""}`}>
                                <IconChevron size={11} />
                            </span>
                            <span className="rail-section-label">Commits</span>
                            {log.length > 0 && <span className="rail-section-count">{log.length}</span>}
                        </button>
                        {graphOpen && (
                            <div className="rail-list rail-graph">
                                {log.length === 0 ? (
                                    <div className="rail-note">no commits yet</div>
                                ) : (
                                    <GitGraph
                                        commits={log}
                                        selectedIndex={Math.min(commitIndex, log.length - 1)}
                                        focused
                                        range={null}
                                        onSelect={(index) => {
                                            setCommitIndex(index);
                                            const entry = log[index];
                                            if (entry) cmd.openCommitDiff(entry.hash, entry.subject);
                                        }}
                                        onActivate={() => {}}
                                    />
                                )}
                            </div>
                        )}
                    </section>
                </div>
            )}
        </div>
    );
}
