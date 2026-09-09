import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { git, hasUnstaged, isStaged, type GitFile } from "../../api/git";
import { basename, dirname } from "../../lib/paths";
import * as cmd from "../../state/commands";
import { runGitCmd } from "../../state/git";
import { useResourceEnabled } from "../../state/resources";
import { gitOverviewR } from "../../state/resources.defs";
import { useStore } from "../../state/store";
import { FileIcon } from "../FileIcon";
import { GitGraph } from "../git/GitGraph";
import { gitFileDecoration } from "../git/gitFileStatus";
import { IconCheck, IconChevron, IconCommit, IconGit, IconPlus, IconRefresh, IconSparkle } from "../Icons";
import { Tooltip } from "../Tooltip";

function relativeDir(path: string): string {
    const dir = dirname(path);
    return dir === "." || dir === "" ? "" : dir;
}

export function RailChanges({ cwd }: { cwd: string }) {
    const overview = useResourceEnabled(!!cwd, gitOverviewR, cwd || "");
    const split = useStore((s) => s.railChangesSplit);
    const [message, setMessage] = useState("");
    const [changesOpen, setChangesOpen] = useState(true);
    const [graphOpen, setGraphOpen] = useState(true);
    const [busy, setBusy] = useState(false);
    const [commitIndex, setCommitIndex] = useState(0);
    const inputRef = useRef<HTMLTextAreaElement>(null);
    const splitRef = useRef<HTMLDivElement>(null);

    const status = overview.data?.status;
    const files = status?.files ?? [];
    const log = overview.data?.log ?? [];
    const branch = status?.branch ?? "";
    const staged = files.filter(isStaged);

    const run = (label: string, fn: () => Promise<unknown>) => {
        setBusy(true);
        void runGitCmd(label, fn, { repo: cwd }).finally(() => {
            setBusy(false);
            void overview.refresh();
        });
    };

    const commit = () => {
        const text = message.trim();
        if (!text || busy) return;
        setMessage("");
        run("committing", async () => {
            if (staged.length === 0) await git.stageAll(cwd);
            await git.commit(cwd, text);
        });
    };

    const generate = () => {
        if (busy) return;
        setMessage("");
        run("writing commit message", async () => {
            const written = await git.aiMessage(cwd, "claude", "", (chunk) => setMessage((current) => current + chunk));
            setMessage(written);
        });
    };

    const toggleStage = (file: GitFile) =>
        run("", async () => {
            if (hasUnstaged(file)) await git.stage(cwd, file.path);
            else await git.unstage(cwd, file.path);
        });

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
                <span className="rail-changes-count">{files.length === 0 ? "clean" : `${files.length} changed`}</span>
                {branch && (
                    <span className="rail-branch">
                        <IconGit size={11} />
                        {branch}
                    </span>
                )}
                <Tooltip label="Re-read the repository">
                    <button
                        type="button"
                        className="rail-section-act"
                        aria-label="Refresh changes"
                        disabled={busy}
                        onClick={() => void overview.refresh()}>
                        <IconRefresh size={11} />
                    </button>
                </Tooltip>
            </div>

            <div className="rail-commit">
                <div className="rail-commit-input">
                    <textarea
                        ref={inputRef}
                        value={message}
                        rows={2}
                        placeholder="Message (⌘↵ to commit)"
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
                    <Tooltip label="Write the message for me">
                        <button type="button" className="rail-commit-ai" aria-label="Generate commit message" disabled={busy} onClick={generate}>
                            <IconSparkle size={13} />
                        </button>
                    </Tooltip>
                </div>
                <button type="button" className="rail-commit-go" disabled={busy || !message.trim() || files.length === 0} onClick={commit}>
                    <IconCommit size={13} />
                    <span>Commit{staged.length > 0 ? ` ${staged.length}` : " all"}</span>
                </button>
            </div>

            <div className="rail-split" ref={splitRef}>
                <section className="rail-section" style={bothOpen ? { flex: `0 0 ${split * 100}%` } : undefined}>
                    <button type="button" className="rail-section-head" aria-expanded={changesOpen} onClick={() => setChangesOpen((open) => !open)}>
                        <span className={`rail-caret${changesOpen ? " open" : ""}`}>
                            <IconChevron size={11} />
                        </span>
                        <span className="rail-section-label">Changes</span>
                        {files.length > 0 && <span className="rail-section-count">{files.length}</span>}
                        <Tooltip label="Stage everything">
                            <button
                                type="button"
                                className="rail-section-act"
                                aria-label="Stage all changes"
                                disabled={busy || files.length === 0}
                                onClick={(event) => {
                                    event.stopPropagation();
                                    run("staging all", () => git.stageAll(cwd));
                                }}>
                                <IconPlus size={11} />
                            </button>
                        </Tooltip>
                    </button>
                    {changesOpen && (
                        <div className="rail-list">
                            {files.length === 0 && <div className="rail-note">no changes</div>}
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
                                        <Tooltip label={hasUnstaged(file) ? `Stage ${name}` : `Unstage ${name}`}>
                                            <button
                                                type="button"
                                                className="rail-file-stage"
                                                aria-label={`${hasUnstaged(file) ? "Stage" : "Unstage"} ${name}`}
                                                disabled={busy}
                                                onClick={() => toggleStage(file)}>
                                                {hasUnstaged(file) ? <IconPlus size={11} /> : <IconCheck size={11} />}
                                            </button>
                                        </Tooltip>
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
        </div>
    );
}
