import { useContext, useState } from "react";
import { useNow } from "../ui/useNow";
import { AgentMark } from "../agents/AgentMark";
import { ChatAgentContext } from "./chatAgent";
import { elapsedLabel } from "@shared/chat/durationLabel";
import { Shimmer } from "../ui/Shimmer";

/* Keeps its own clock so a ticking second redraws this row alone, not the
   whole transcript. */
export function ChatActivity({ label }: { label: string }) {
  const agentType = useContext(ChatAgentContext);
  const [started] = useState(Date.now);
  const seconds = Math.max(0, Math.round((useNow() - started) / 1000));
  return (
    <div className="chat-activity" role="status">
      <span
        className={`chat-activity-mark agent-glyph ${agentType}`}
        aria-hidden="true"
      >
        <AgentMark type={agentType} size={21} />
      </span>
      <span className="chat-activity-label chat-shimmer">
        <Shimmer>{label}</Shimmer>
      </span>
      {seconds > 0 && (
        <span className="chat-activity-elapsed" aria-hidden="true">
          {elapsedLabel(seconds)}
        </span>
      )}
    </div>
  );
}
