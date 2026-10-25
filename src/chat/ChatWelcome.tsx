import { useState } from "react";
import { burntTokens, Calendar, dayText, tokensText, useActivitySummary } from "../shell/ActivityPage";
import { AgentFace } from "../buddies/Buddy";
import { projectMark } from "../hosts/placeMarks";
import { basename } from "@shared/lib/paths";
import * as cmd from "../state/commands";
import type { Agent } from "../state/types";
import { IconChevron } from "../ui/Icons";
import "../styles/activity.css";

export function ChatWelcome({ cwd, agent }: { cwd: string; agent: Agent }) {
    const { summary, failed } = useActivitySummary(cwd);
    const [hovered, setHovered] = useState<number | null>(null);
    const day = hovered === null ? undefined : summary?.days.find((entry) => entry.day === hovered);
    const tokens = hovered === null ? (summary ? burntTokens(summary.totals) : null) : (day?.tokens ?? 0);
    return (
        <div className="chat-welcome">
            <div className="chat-welcome-head">
                <AgentFace
                    agent={agent}
                    size={38}
                    logoSize={40}
                    logoClass={`chat-welcome-mark agent-glyph ${agent.type}`}
                    buddyClass="chat-welcome-mark agent-glyph buddy-mark"
                />
                {!failed && (
                    <div className="chat-welcome-burn" aria-live="polite">
                        <span className="chat-welcome-burn-count">{tokens === null ? "—" : tokensText(tokens)}</span>
                        <span className="chat-welcome-burn-label">
                            {hovered === null ? (
                                <>
                                    tokens burnt in {projectMark(12)}
                                    <span className="chat-welcome-project">{basename(cwd) || cwd}</span>
                                </>
                            ) : (
                                `tokens on ${dayText(hovered)}`
                            )}
                        </span>
                    </div>
                )}
            </div>
            {!failed && (
                <Calendar
                    summary={summary}
                    metric="tokens"
                    onHover={setHovered}
                    footer={
                        <button type="button" className="chat-welcome-more" onClick={() => cmd.openSettings("activity")}>
                            View activity
                            <IconChevron size={10} />
                        </button>
                    }
                />
            )}
        </div>
    );
}
