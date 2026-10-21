import { useState } from "react";
import { basename, files, notify, openProjectFile, useNearestProject } from "../../../plugin-api/host";
import type { Exception, Frame } from "../api";
import { frameLabel, groupFrames, projectCandidates } from "../format";

/** Finds a frame's file in the project and opens it at the line, or says it isn't there. */
async function openFrame(frame: Frame, project: { name: string; cwd: string } | null) {
    if (!frame.file) return;
    if (!project) {
        notify("info", "Open the project this code is from to jump to the line.");
        return;
    }
    const candidates = projectCandidates(frame.file, project.cwd);
    const kinds = candidates.length > 0 ? await files.pathKinds(candidates) : [];
    const found = candidates.find((_, index) => kinds[index] === "file");
    if (!found) {
        notify("info", `${basename(frame.file)} isn't in ${project.name}.`);
        return;
    }
    openProjectFile(found, frame.line ?? undefined);
}

function FrameRow({ frame, open, onToggle }: { frame: Frame; open: boolean; onToggle: () => void }) {
    const project = useNearestProject();
    const where = `${frameLabel(frame)}${frame.line ? `:${frame.line}` : ""}${frame.column ? `:${frame.column}` : ""}`;
    return (
        <li className={`sentry-frame${frame.inApp ? " app" : ""}${open ? " open" : ""}`}>
            <div className="sentry-frame-head">
                <button type="button" className="sentry-frame-toggle" aria-expanded={open} disabled={frame.context.length === 0} onClick={onToggle}>
                    <span className="sentry-frame-where">{where}</span>
                    {frame.function ? <span className="sentry-frame-fn">in {frame.function}</span> : null}
                </button>
                {frame.inApp && frame.file ? (
                    <button type="button" className="sentry-button small" aria-label={`Open ${where}`} onClick={() => void openFrame(frame, project)}>
                        Open
                    </button>
                ) : null}
            </div>
            {open && frame.context.length > 0 ? (
                <pre className="sentry-code">
                    {frame.context.map(([number, code]) => (
                        <span key={number} className={number === frame.line ? "ran" : undefined}>
                            <span className="sentry-code-no">{number}</span>
                            {code || " "}
                        </span>
                    ))}
                </pre>
            ) : null}
        </li>
    );
}

function ExceptionBlock({ exception, cause }: { exception: Exception; cause: boolean }) {
    const newest = [...exception.frames].reverse();
    const firstApp = newest.findIndex((frame) => frame.inApp);
    const [opened, setOpened] = useState<Set<number>>(() => new Set([firstApp >= 0 ? firstApp : 0]));
    const [unfolded, setUnfolded] = useState<Set<number>>(new Set());
    const toggle = (index: number) =>
        setOpened((held) => {
            const next = new Set(held);
            if (next.has(index)) next.delete(index);
            else next.add(index);
            return next;
        });
    return (
        <article className="sentry-exception" aria-label={`${exception.kind}: ${exception.value}`}>
            <header>
                {cause ? <span className="sentry-muted">Caused by</span> : null}
                <strong>{exception.kind}</strong>
                <span className="sentry-exception-value">{exception.value}</span>
                {exception.handled === false ? <span className="sentry-chip unhandled">Unhandled</span> : null}
                {exception.handled === true ? <span className="sentry-chip">Handled</span> : null}
            </header>
            {newest.length === 0 ? (
                <p className="sentry-note">No stack trace came with it.</p>
            ) : (
                <ol className="sentry-frames">
                    {groupFrames(newest).map((group) =>
                        group.kind === "frame" ? (
                            <FrameRow key={group.index} frame={group.frame} open={opened.has(group.index)} onToggle={() => toggle(group.index)} />
                        ) : unfolded.has(group.from) ? (
                            group.frames.map((frame, offset) => (
                                <FrameRow
                                    key={group.from + offset}
                                    frame={frame}
                                    open={opened.has(group.from + offset)}
                                    onToggle={() => toggle(group.from + offset)}
                                />
                            ))
                        ) : (
                            <li key={`fold-${group.from}`} className="sentry-fold">
                                <button type="button" onClick={() => setUnfolded((held) => new Set(held).add(group.from))}>
                                    {group.frames.length} library {group.frames.length === 1 ? "frame" : "frames"}
                                </button>
                            </li>
                        ),
                    )}
                </ol>
            )}
        </article>
    );
}

export function StackTrace({ exceptions, message }: { exceptions: Exception[]; message: string | null }) {
    if (exceptions.length === 0) return <p className="sentry-note">{message ?? "This event has no stack trace."}</p>;
    const topFirst = [...exceptions].reverse();
    return (
        <div className="sentry-stack">
            {topFirst.map((exception, index) => (
                <ExceptionBlock key={`${exception.kind}-${index}`} exception={exception} cause={index > 0} />
            ))}
        </div>
    );
}
