import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { git, type GitCompare } from "../../api/git";
import { CommitReview } from "../../git/CommitReview";
import { FoldPanel } from "../../git/FoldPanel";
import { GitColumns } from "../../git/GitColumns";
import { GitGraph } from "../../git/GitGraph";
import { FileIcon } from "../../ui/FileIcon";
import { basename, dirname } from "../../lib/paths";
import { notify, reportError } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { Checkbox, Dropdown, EmptyState, IconClose, IconGit, IconPush, PRIMARY_SHORTCUT, SkeletonRows, Tooltip } from "../../plugin-api/ui";
import { requestOpenFile, setGitView } from "../../state/commands";
import { useStore } from "../../state/store";
import { hostApi, type RepoRef } from "../api";
import { defaultBase, isUsualBase } from "../compose";
import { useHost } from "../registry";
import { hostBranchesR } from "../resources";

const STATUS_CLASS = { A: "added", M: "modified", D: "deleted", R: "renamed" } as const;

type Comparison = { key: string; data: GitCompare | null; error: string | null };

interface Props {
    paneId: string;
    repo: RepoRef;
    /** The project folder, when this is its own repository. */
    cwd: string | null;
    head: string | null;
    active: boolean;
    onCreated: (number: number) => void;
    onCancel: () => void;
}

/** A new pull request, laid out like Changes: the message box and the branch's files and commits on the left, its diff on the right. */
export function NewPullForm({ paneId, repo, cwd, head: startingHead, active, onCreated, onCancel }: Props) {
    const host = useHost();
    const branches = useResourceEnabled(active, hostBranchesR, repo);
    const names = useMemo(() => branches.data ?? [], [branches.data]);
    const [head, setHead] = useState(startingHead && !isUsualBase(startingHead) ? startingHead : "");
    const [base, setBase] = useState<string | null>(null);
    const [title, setTitle] = useState("");
    const [body, setBody] = useState("");
    const [draft, setDraft] = useState(false);
    const [busy, setBusy] = useState(false);
    const [pushing, setPushing] = useState(false);
    const [focus, setFocus] = useState<string | null>(null);
    const [commit, setCommit] = useState<string | null>(null);
    const [comparison, setComparison] = useState<Comparison | null>(null);
    const [pushedAt, setPushedAt] = useState(0);
    const bodyRef = useRef<HTMLTextAreaElement>(null);

    useEffect(() => {
        if (base === null && names.length > 0) setBase(defaultBase(names, head || null));
    }, [base, names, head]);

    const compareKey = cwd && base && head && base !== head ? `${base}\n${head}\n${pushedAt}` : null;
    useEffect(() => {
        if (!cwd || !base || !head || !compareKey) return;
        let cancelled = false;
        git.compare(cwd, base, head).then(
            (data) => !cancelled && setComparison({ key: compareKey, data, error: null }),
            (error: unknown) => !cancelled && setComparison({ key: compareKey, data: null, error: String(error) }),
        );
        return () => {
            cancelled = true;
        };
    }, [cwd, base, head, compareKey]);
    const compared = comparison && comparison.key === compareKey ? comparison : null;
    const files = compared?.data?.files ?? [];
    const commits = compared?.data?.commits ?? [];
    const mergeBase = compared?.data?.merge_base ?? null;

    const soleSubject = commits.length === 1 ? commits[0].subject : "";
    useEffect(() => {
        if (soleSubject) setTitle((was) => was || soleSubject);
    }, [soleSubject]);

    useLayoutEffect(() => {
        const el = bodyRef.current;
        if (!el) return;
        el.style.height = "auto";
        el.style.height = `${el.scrollHeight}px`;
    }, [body]);

    const pushed = !head || names.length === 0 || names.includes(head);
    const ready = !!title.trim() && !!head.trim() && !!base && base !== head && pushed && !busy;

    const create = async () => {
        if (!base || !ready) return;
        setBusy(true);
        try {
            const made = await hostApi(repo.provider).createPull(repo, { title: title.trim(), head: head.trim(), base, body, draft });
            notify("success", `Opened #${made.number}`);
            invalidate((kind) => kind === "host.pulls");
            onCreated(made.number);
        } catch (error) {
            reportError("Could not open the pull request")(error);
        } finally {
            setBusy(false);
        }
    };

    const push = async () => {
        if (!cwd) return;
        setPushing(true);
        try {
            await git.push(cwd);
            notify("success", `Pushed ${head}`);
            invalidate((kind) => kind === "host.branches");
            setPushedAt(Date.now());
        } catch (error) {
            reportError(`Could not push ${head}`)(error);
        } finally {
            setPushing(false);
        }
    };

    const onKeyDown = (event: KeyboardEvent) => {
        event.stopPropagation();
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void create();
        } else if (event.key === "Escape") (event.currentTarget as HTMLElement).blur();
    };

    const openLabel = busy ? "Opening…" : draft ? "Open as a draft" : "Open pull request";
    const left = (
        <>
            <div className="pr-new-head">
                <Dropdown
                    className="pr-new-branch"
                    icon={<IconGit size={11} />}
                    value={head}
                    options={[
                        ...(head && !names.includes(head) ? [{ value: head, label: head }] : []),
                        ...(head ? [] : [{ value: "", label: "Choose a branch" }]),
                        ...names.filter((name) => name !== base).map((name) => ({ value: name, label: name })),
                    ]}
                    onChange={(next) => {
                        setHead(next);
                        setCommit(null);
                        setFocus(null);
                    }}
                    title="The branch with the changes"
                    search="Find a branch"
                    menuWidth={300}
                />
                <span className="pr-new-into">into</span>
                <Dropdown
                    className="pr-new-branch"
                    icon={<IconGit size={11} />}
                    value={base ?? ""}
                    options={names.filter((name) => name !== head).map((name) => ({ value: name, label: name }))}
                    onChange={(next) => {
                        setBase(next);
                        setCommit(null);
                        setFocus(null);
                    }}
                    title="The branch the changes land in"
                    search="Find a branch"
                    menuWidth={300}
                />
                <span className="gha-page-spacer" />
                <Tooltip label="Cancel">
                    <button type="button" className="gha-icon-btn" aria-label="Cancel" onClick={onCancel}>
                        <IconClose size={11} />
                    </button>
                </Tooltip>
            </div>
            {!pushed && (
                <div className="pr-new-push">
                    <span>Not on {host.name} yet</span>
                    {cwd && head === startingHead && (
                        <button type="button" className="gha-btn" disabled={pushing} onClick={() => void push()}>
                            <IconPush size={12} />
                            {pushing ? "Pushing…" : "Push"}
                        </button>
                    )}
                </div>
            )}
            <div className="git-compose">
                <div className="git-compose-well">
                    <input
                        className="pr-new-title"
                        value={title}
                        onChange={(event) => setTitle(event.target.value)}
                        onKeyDown={onKeyDown}
                        placeholder="Title"
                        aria-label="Title"
                        spellCheck={false}
                    />
                    <textarea
                        ref={bodyRef}
                        className="git-compose-message pr-new-body"
                        value={body}
                        onChange={(event) => setBody(event.target.value)}
                        onKeyDown={onKeyDown}
                        placeholder="What changed, and why"
                        aria-label="Description"
                        rows={3}
                    />
                    <div className="git-compose-foot">
                        {host.capabilities.pulls.draft && (
                            <Checkbox checked={draft} onChange={setDraft}>
                                Draft
                            </Checkbox>
                        )}
                        <Tooltip label={`${openLabel} (${PRIMARY_SHORTCUT}⏎)`}>
                            <button type="button" className="git-compose-commit" disabled={!ready} onClick={() => void create()}>
                                {openLabel}
                            </button>
                        </Tooltip>
                    </div>
                </div>
            </div>
            {compareKey && !compared && <SkeletonRows rows={3} label="Loading files" />}
            {compared?.data && (
                <div className="git-list pr-files">
                    <div className="git-group git-file-group">
                        <span className="git-label">Files</span>
                        {files.length > 0 && <span className="git-count">{files.length}</span>}
                    </div>
                    {files.map((file) => {
                        const dir = dirname(file.path);
                        return (
                            <button
                                key={file.path}
                                type="button"
                                className={`git-row git-file-row${focus === file.path && !commit ? " sel" : ""}`}
                                title={file.path}
                                onClick={() => {
                                    setCommit(null);
                                    setFocus(file.path);
                                }}>
                                <FileIcon name={basename(file.path)} size={14} />
                                <span className="git-row-name">
                                    {basename(file.path)}
                                    {dir && <span className="git-row-dir">{dir}</span>}
                                </span>
                                <span className={`git-status ${STATUS_CLASS[file.status]}`}>{file.status}</span>
                            </button>
                        );
                    })}
                </div>
            )}
            {commits.length > 0 && <CompareHistory paneId={paneId} commits={commits} selected={commit} onSelect={setCommit} />}
        </>
    );

    const right = !cwd ? (
        <EmptyState message="This repository is not checked out here, so its changes cannot be shown." />
    ) : !head || !base ? (
        <EmptyState message="Choose the branch to open a pull request from." />
    ) : compared?.error ? (
        <EmptyState message={`${head} is not in this checkout. Fetch it to see what it changes.`} />
    ) : !compared ? (
        <SkeletonRows rows={6} label="Comparing branches" />
    ) : !mergeBase || files.length === 0 ? (
        <EmptyState message={`${head} has nothing that ${base} does not.`} />
    ) : (
        <div className="pr-right">
            <div className="git-detail">
                <h2 className="git-detail-title">{title.trim() || "New pull request"}</h2>
                <div className="git-detail-meta">
                    <span>
                        {commits.length} commit{commits.length === 1 ? "" : "s"} · {files.length} file{files.length === 1 ? "" : "s"}
                    </span>
                    {commit && (
                        <>
                            <span className="gha-page-spacer" />
                            <span className="gha-dim">
                                Commit <span className="gha-mono">{commit.slice(0, 7)}</span>
                            </span>
                            <button type="button" className="gha-chip" onClick={() => setCommit(null)}>
                                All changes
                            </button>
                        </>
                    )}
                </div>
            </div>
            {commit ? (
                <CommitReview key={commit} repo={cwd} rev={commit} title="" subtitle="" head={<></>} onOpenFile={requestOpenFile} />
            ) : (
                <CommitReview
                    key={compareKey}
                    repo={cwd}
                    rev={head}
                    title=""
                    subtitle=""
                    head={<></>}
                    range={{ base: mergeBase, files: files.map((file) => file.path) }}
                    focusPath={focus}
                    onOpenFile={requestOpenFile}
                />
            )}
        </div>
    );

    return <GitColumns paneId={paneId} left={left} right={<div className="git-right-review">{right}</div>} />;
}

function CompareHistory({
    paneId,
    commits,
    selected,
    onSelect,
}: {
    paneId: string;
    commits: GitCompare["commits"];
    selected: string | null;
    onSelect: (sha: string | null) => void;
}) {
    const open = useStore((s) => s.gitViews[paneId]?.historyOpen ?? false);
    const height = useStore((s) => s.gitViews[paneId]?.historyHeight ?? null);
    const index = selected ? commits.findIndex((row) => row.full_hash === selected) : -1;
    return (
        <FoldPanel
            label="Commits"
            count={commits.length}
            summary={commits[0]?.subject ?? null}
            open={open}
            height={height}
            onToggle={() => {
                if (open) onSelect(null);
                setGitView(paneId, { historyOpen: !open });
            }}
            onResize={(next) => setGitView(paneId, { historyHeight: next })}>
            <div className="git-list">
                <GitGraph
                    commits={commits}
                    selectedIndex={index}
                    focused={index >= 0}
                    range={null}
                    onSelect={(i) => onSelect(commits[i]?.full_hash ?? null)}
                    onActivate={() => {}}
                />
            </div>
        </FoldPanel>
    );
}
