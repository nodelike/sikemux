import { useLayoutEffect, type KeyboardEvent, type RefObject } from "react";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import { setGitDraft, useGitWorkbench } from "../state/gitWorkbench";
import { IconChevron, IconSparkle } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";

export function GitComposer({
    repo,
    busy,
    generating,
    stagedCount,
    agentLabel,
    messageRef,
    onCommit,
    onGenerate,
    onPickAgent,
}: {
    repo: string;
    busy: boolean;
    generating: boolean;
    stagedCount: number;
    agentLabel: string;
    messageRef: RefObject<HTMLTextAreaElement | null>;
    onCommit: () => void;
    onGenerate: () => void;
    onPickAgent: (anchor: HTMLElement) => void;
}) {
    const draft = useGitWorkbench((state) => state.drafts[repo] ?? "");
    const canCommit = !busy && stagedCount > 0 && !!draft.trim();

    // The box grows with the message, up to a limit, rather than scrolling it out of sight.
    useLayoutEffect(() => {
        const el = messageRef.current;
        if (!el) return;
        const fit = () => {
            el.style.height = "auto";
            el.style.height = `${el.scrollHeight}px`;
        };
        fit();
        // Widening the column can unwrap the message, so its height follows the width too.
        let width = el.clientWidth;
        const observer = new ResizeObserver(() => {
            if (el.clientWidth === width) return;
            width = el.clientWidth;
            fit();
        });
        observer.observe(el);
        return () => observer.disconnect();
    }, [draft, messageRef]);
    const commitLabel = stagedCount > 0 ? `Commit ${stagedCount} file${stagedCount === 1 ? "" : "s"}` : "Commit";

    const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
        event.stopPropagation();
        if (busy) {
            event.preventDefault();
            return;
        }
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            if (canCommit) onCommit();
        } else if (event.key === "Escape") event.currentTarget.blur();
    };

    return (
        <div className="git-compose">
            <div className="git-compose-well">
                <textarea
                    ref={messageRef}
                    className="git-compose-message"
                    placeholder={`Message (${PRIMARY_SHORTCUT}⏎ to commit)`}
                    aria-label="Commit message"
                    value={draft}
                    rows={3}
                    spellCheck={false}
                    readOnly={busy}
                    onChange={(event) => setGitDraft(repo, event.target.value)}
                    onKeyDown={onKeyDown}
                />
                <div className="git-compose-foot">
                    <Tooltip label="Write the message from the staged changes (g)">
                        <button type="button" className="git-compose-agent" disabled={busy} onClick={onGenerate}>
                            <span className={`git-compose-spark${generating ? " writing" : ""}`}>
                                <IconSparkle size={11} />
                            </span>
                            <span>{generating ? "writing…" : agentLabel}</span>
                        </button>
                    </Tooltip>
                    <Tooltip label="Pick the agent and model">
                        <button
                            type="button"
                            className="git-compose-agent-pick"
                            aria-label="Pick the agent and model"
                            onClick={(event) => onPickAgent(event.currentTarget)}>
                            <IconChevron size={9} />
                        </button>
                    </Tooltip>
                    <Tooltip label={stagedCount > 0 ? `${commitLabel} (${PRIMARY_SHORTCUT}⏎)` : "Stage files to commit them"}>
                        <button type="button" className="git-compose-commit" disabled={!canCommit} onClick={onCommit}>
                            {commitLabel}
                        </button>
                    </Tooltip>
                </div>
            </div>
        </div>
    );
}
