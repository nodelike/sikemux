import { useEffect, useRef, useState } from "react";
import { errorText } from "../lib/format";
import { useModalFocus } from "../hooks/useModalFocus";
import { shareApi, type ShareView } from "../api/share";
import { copyText } from "../lib/clipboard";
import { useStore } from "../state/store";
import { useOccludeNativeViews } from "../state/nativeViews";
import { notify, reportError } from "../state/toast";
import type { Agent } from "../state/types";
import { IconCopy, IconEye, IconShare } from "../ui/Icons";
import { Kbd } from "../ui/Kbd";
import {
    closeShareDialog,
    DEFAULT_SHARE_LIFETIME_MS,
    openShareDialog,
    refreshShares,
    SHARE_LIFETIMES,
    useChatShare,
    useShares,
    useSharesPolling,
} from "./macShares";
import "../styles/share.css";

function watching(share: ShareView): string {
    if (share.state === "connecting") return "Connecting…";
    if (share.viewers === 0) return "No one watching yet";
    return share.viewers === 1 ? "1 person watching" : `${share.viewers} people watching`;
}

function endsIn(expiresAt: number, now: number): string {
    const minutes = Math.max(1, Math.round((expiresAt - now) / 60_000));
    if (minutes < 60) return `${minutes} min`;
    const hours = Math.round(minutes / 60);
    if (hours < 48) return hours === 1 ? "1 hour" : `${hours} hours`;
    return `${Math.round(hours / 24)} days`;
}

/** Beside the chat's name: a share button, or once shared, a green eye and how many watch. */
export function ShareButton({ agent }: { agent: Agent }) {
    const share = useChatShare(agent.id);
    const shared = share !== null;
    useEffect(() => {
        if (shared) void shareApi.share(agent.id, DEFAULT_SHARE_LIFETIME_MS, agent.title).catch(() => undefined);
    }, [shared, agent.id, agent.title]);
    if (!share)
        return (
            <button
                type="button"
                className="agent-share"
                aria-label="Share this chat read-only"
                title="Share this chat read-only"
                onClick={() => openShareDialog(agent.id)}>
                <IconShare size={13} />
            </button>
        );
    return (
        <button
            type="button"
            className="agent-share live"
            aria-label={watching(share)}
            title={watching(share)}
            onClick={() => openShareDialog(agent.id)}>
            <IconEye size={13} />
            {share.viewers > 0 && <span>{share.viewers}</span>}
        </button>
    );
}

/** The share dialog of whichever chat asked for it, and the polling that keeps shares fresh. Mounted once, at the app's root. */
export function ShareHost() {
    useSharesPolling();
    const agentId = useShares((state) => state.dialog);
    useOccludeNativeViews(agentId !== null);
    if (!agentId) return null;
    return <ShareDialog key={agentId} agentId={agentId} />;
}

function ShareDialog({ agentId }: { agentId: string }) {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const title = useStore((state) => state.agents[agentId]?.title ?? "this chat");
    const share = useChatShare(agentId);
    const refused = useShares((state) => state.shares.find((candidate) => candidate.agentId === agentId && candidate.state === "refused"));
    const [lifetime, setLifetime] = useState<number>(DEFAULT_SHARE_LIFETIME_MS);
    const [working, setWorking] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const now = Date.now();

    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            closeShareDialog();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, []);

    const start = async () => {
        setWorking(true);
        setProblem(null);
        try {
            const started = await shareApi.share(agentId, lifetime, title);
            await refreshShares();
            await copyText(started.url).then(() => notify("success", "copied share link"), reportError("copy"));
        } catch (error) {
            setProblem(errorText(error));
        } finally {
            setWorking(false);
        }
    };

    const stop = async () => {
        if (!share) return;
        setWorking(true);
        try {
            await shareApi.stop(share.id);
            await refreshShares();
            closeShareDialog();
        } catch (error) {
            setProblem(errorText(error));
        } finally {
            setWorking(false);
        }
    };

    return (
        <div className="dlg-scrim" onMouseDown={closeShareDialog}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="dlg share-dlg"
                role="dialog"
                aria-modal="true"
                aria-label={`Share ${title}`}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="dlg-head">
                    <span className="dlg-glyph" aria-hidden="true">
                        <IconShare size={15} />
                    </span>
                    <h2 className="dlg-title">{share ? "Shared read-only" : "Share this chat read-only"}</h2>
                </div>
                <div className="dlg-body">
                    <p>
                        Anyone signed in to Sikemux who has the link can read this chat as it happens, from its start. They cannot type in it or
                        answer its requests.
                    </p>
                    <p>It shows everything the agent read and ran, including the contents of files and any secrets in them.</p>
                </div>
                {share ? (
                    <div className="dlg-field">
                        <label htmlFor="share-link">Link</label>
                        <div className="share-link-row">
                            <input
                                id="share-link"
                                className="dlg-input"
                                readOnly
                                value={share.url}
                                onFocus={(event) => event.currentTarget.select()}
                            />
                            <button
                                type="button"
                                className="dlg-btn share-copy"
                                onClick={() => void copyText(share.url).then(() => notify("success", "copied share link"), reportError("copy"))}>
                                <IconCopy size={13} />
                                Copy
                            </button>
                        </div>
                        <span className="share-status" role="status">
                            {watching(share)} · ends in {endsIn(share.expiresAt, now)}
                        </span>
                    </div>
                ) : (
                    <div className="dlg-field">
                        <span className="share-label" id="share-lifetime">
                            Link works for
                        </span>
                        <div className="share-lifetimes" role="radiogroup" aria-labelledby="share-lifetime">
                            {SHARE_LIFETIMES.map((option) => (
                                <button
                                    key={option.ms}
                                    type="button"
                                    role="radio"
                                    aria-checked={lifetime === option.ms}
                                    className={`share-lifetime${lifetime === option.ms ? " on" : ""}`}
                                    onClick={() => setLifetime(option.ms)}>
                                    {option.label}
                                </button>
                            ))}
                        </div>
                    </div>
                )}
                {(problem ?? refused?.problem) && (
                    <p className="share-problem" role="alert">
                        {problem ?? refused?.problem}
                    </p>
                )}
                <div className="dlg-foot">
                    <span className="dlg-hint">
                        <Kbd>esc</Kbd> close
                    </span>
                    {share ? (
                        <>
                            <button type="button" className="dlg-btn" disabled={working} onClick={() => void stop()}>
                                Stop sharing
                            </button>
                            <button type="button" className="dlg-btn primary" onClick={closeShareDialog}>
                                Done
                            </button>
                        </>
                    ) : (
                        <>
                            <button type="button" className="dlg-btn" onClick={closeShareDialog}>
                                Cancel
                            </button>
                            <button type="button" className="dlg-btn primary" disabled={working} onClick={() => void start()}>
                                {working ? "Creating link…" : "Create link"}
                            </button>
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}
