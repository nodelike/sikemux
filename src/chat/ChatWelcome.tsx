import { Calendar, isEmptyActivity, Overview, useActivitySummary } from "../shell/ActivityPage";
import { AgentFace } from "../buddies/Buddy";
import { basename } from "../lib/paths";
import type { Agent } from "../state/types";
import "../styles/activity.css";

export function ChatWelcome({ cwd, agent }: { cwd: string; agent: Agent }) {
    const { summary, failed } = useActivitySummary(cwd);
    const totals = summary?.totals;
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
                <span className="chat-welcome-title">{basename(cwd) || cwd}</span>
            </div>
            {!failed && (
                <div className="chat-welcome-activity">
                    {!isEmptyActivity(totals) && <Overview totals={totals} />}
                    <Calendar days={summary?.days ?? []} loaded={!!summary} />
                </div>
            )}
        </div>
    );
}
